//! v0.1 `PaneWidget`: a GTK4 widget hosting a shell PTY + a
//! libghostty-vt Terminal, with live text readout for the MVP.
//!
//! # Thread model
//!
//! The `Terminal` + `EffectsHandle` live on the **GTK main thread**.
//! This is required because the real renderer needs same-thread
//! access to the terminal's grid to draw cells, and libghostty-vt
//! terminals are `!Sync`.
//!
//! The **pane worker thread** is a pure PTY byte pump:
//!
//! ```text
//! worker thread                  main thread (GTK)
//! ─────────────                  ──────────────────
//! Child (forkpty bash)
//!  │                             PaneState
//!  │   std mpsc (pty bytes)      ├── Terminal
//!  │ ───────────────────────────►│── EffectsHandle
//!  │                             │
//!  │   tokio mpsc (write bytes)  │   glib::timeout_add_local 30ms:
//!  │ ◄───────────────────────────│     drain pty bytes → vt_write
//!  │                             │     drain effects → counters +
//!  │                             │       WritePty routed back via
//!  │                             │       write_tx
//!  │                             │     refresh label
//!  │   std mpsc (shutdown)       │
//!  │ ◄───────────────────────────│
//! ```
//!
//! The worker owns the [`host_pty::Child`] and runs a tokio
//! current-thread runtime with a two-arm `select!` (read vs write).
//! Keyboard input flows through the tokio mpsc write channel.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use gtk4::gdk;
use gtk4::glib;
use tokio::sync::mpsc as tokio_mpsc;

use host_ghostty::{EffectsHandle, Formatter, OutputFormat, Terminal, TerminalEvent};
use host_pty::{Child, SpawnOptions};

/// Cloneable sender that writes raw bytes to a pane's PTY child.
pub type PaneInputSender = tokio_mpsc::UnboundedSender<Vec<u8>>;

/// Live per-pane state owned on the GTK main thread.
struct PaneState {
    terminal: Terminal,
    effects: EffectsHandle,
    effects_rx: Receiver<TerminalEvent>,
    bell_count: u64,
    title_changed_count: u64,
    exited: bool,
}

impl Drop for PaneState {
    fn drop(&mut self) {
        if let Err(err) = self.effects.clear(&mut self.terminal) {
            eprintln!("[mugenkai] warning: failed to clear effects on pane drop: {err}");
        }
    }
}

/// A v0.1 pane that hosts a shell inside a libghostty-vt terminal.
pub struct PaneWidget {
    widget: gtk4::Box,
    write_tx: PaneInputSender,
    _state: Rc<RefCell<PaneState>>,
    _controller: PaneController,
}

struct PaneController {
    shutdown_tx: Option<Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for PaneController {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}

impl PaneWidget {
    #[must_use]
    pub fn new() -> Self {
        let vbox = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let status_label = gtk4::Label::builder()
            .label("starting shell…")
            .halign(gtk4::Align::Start)
            .css_classes(["dim-label"])
            .build();
        vbox.append(&status_label);

        let screen_label = gtk4::Label::builder()
            .label("")
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            .selectable(false)
            .wrap(false)
            .xalign(0.0)
            .yalign(0.0)
            .build();
        screen_label.add_css_class("monospace");

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&screen_label)
            .build();
        vbox.append(&scroller);

        let mut terminal = Terminal::new(80, 24, 1000).expect("construct terminal");
        let mut effects = EffectsHandle::install(&mut terminal).expect("install effects");
        let effects_rx = effects
            .take_receiver()
            .expect("fresh effects handle must yield a receiver");

        let state = Rc::new(RefCell::new(PaneState {
            terminal,
            effects,
            effects_rx,
            bell_count: 0,
            title_changed_count: 0,
            exited: false,
        }));

        let (pty_bytes_tx, pty_bytes_rx) = mpsc::channel::<Vec<u8>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let (write_tx, write_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();

        let worker = thread::Builder::new()
            .name("mugenkai-pane-worker".into())
            .spawn(move || {
                run_worker(pty_bytes_tx, shutdown_rx, write_rx);
            })
            .expect("spawn mugenkai pane worker thread");

        let controller = PaneController {
            shutdown_tx: Some(shutdown_tx),
            worker: Some(worker),
        };

        refresh_labels(&state.borrow(), &status_label, &screen_label);

        let state_tick = Rc::clone(&state);
        let status_tick = status_label.clone();
        let screen_tick = screen_label.clone();
        let scroller_tick = scroller.clone();
        let write_tx_tick = write_tx.clone();
        glib::timeout_add_local(Duration::from_millis(30), move || {
            drain_pty_bytes(
                &pty_bytes_rx,
                &state_tick,
                &write_tx_tick,
                &status_tick,
                &screen_tick,
                &scroller_tick,
            )
        });

        Self {
            widget: vbox,
            write_tx,
            _state: state,
            _controller: controller,
        }
    }

    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.widget.upcast_ref()
    }

    /// Return a clone of this pane's PTY input sender.
    #[must_use]
    pub fn input_sender(&self) -> PaneInputSender {
        self.write_tx.clone()
    }
}

impl Default for PaneWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// Attach a `gtk4::EventControllerKey` to `widget` that translates
/// keyboard events to PTY byte sequences and forwards them via
/// `write_tx`.
pub fn install_pane_key_controller<W: gtk4::prelude::IsA<gtk4::Widget>>(
    widget: &W,
    write_tx: PaneInputSender,
) {
    let key_controller = gtk4::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    key_controller.connect_key_pressed(move |_controller, keyval, _keycode, modifiers| {
        if let Some(bytes) = key_to_bytes(keyval, modifiers) {
            let _ = write_tx.send(bytes);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    widget.add_controller(key_controller);
}

fn key_to_bytes(keyval: gdk::Key, modifiers: gdk::ModifierType) -> Option<Vec<u8>> {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);

    if ctrl
        && !alt
        && let Some(ch) = keyval.to_unicode()
        && ch.is_ascii_alphabetic()
    {
        let byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
        return Some(vec![byte]);
    }

    match keyval {
        gdk::Key::Return | gdk::Key::KP_Enter => Some(b"\r".to_vec()),
        gdk::Key::Tab => Some(b"\t".to_vec()),
        gdk::Key::BackSpace => Some(vec![0x7F]),
        gdk::Key::Escape => Some(vec![0x1B]),
        gdk::Key::Up => Some(b"\x1b[A".to_vec()),
        gdk::Key::Down => Some(b"\x1b[B".to_vec()),
        gdk::Key::Right => Some(b"\x1b[C".to_vec()),
        gdk::Key::Left => Some(b"\x1b[D".to_vec()),
        gdk::Key::Home => Some(b"\x1b[H".to_vec()),
        gdk::Key::End => Some(b"\x1b[F".to_vec()),
        gdk::Key::Page_Up => Some(b"\x1b[5~".to_vec()),
        gdk::Key::Page_Down => Some(b"\x1b[6~".to_vec()),
        gdk::Key::Delete => Some(b"\x1b[3~".to_vec()),
        _ => {
            let ch = keyval.to_unicode()?;
            let mut buf = [0u8; 4];
            Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
        }
    }
}

fn drain_pty_bytes(
    pty_bytes_rx: &Receiver<Vec<u8>>,
    state: &Rc<RefCell<PaneState>>,
    write_tx: &PaneInputSender,
    status_label: &gtk4::Label,
    screen_label: &gtk4::Label,
    scroller: &gtk4::ScrolledWindow,
) -> glib::ControlFlow {
    let mut had_input = false;
    let mut write_pty_responses: Vec<Vec<u8>> = Vec::new();

    {
        let mut state_mut = state.borrow_mut();

        while let Ok(bytes) = pty_bytes_rx.try_recv() {
            state_mut.terminal.vt_write(&bytes);
            had_input = true;

            let mut events: Vec<TerminalEvent> = Vec::new();
            while let Ok(ev) = state_mut.effects_rx.try_recv() {
                events.push(ev);
            }
            for ev in events {
                match ev {
                    TerminalEvent::Bell => state_mut.bell_count += 1,
                    TerminalEvent::TitleChanged => state_mut.title_changed_count += 1,
                    TerminalEvent::WritePty(bytes) => write_pty_responses.push(bytes),
                }
            }
        }

        if had_input {
            refresh_labels(&state_mut, status_label, screen_label);
        }
    }

    for bytes in write_pty_responses {
        let _ = write_tx.send(bytes);
    }

    if had_input {
        let adj = scroller.vadjustment();
        adj.set_value(adj.upper() - adj.page_size());
    }

    glib::ControlFlow::Continue
}

fn refresh_labels(state: &PaneState, status_label: &gtk4::Label, screen_label: &gtk4::Label) {
    let status_text = if state.exited {
        format!(
            "shell exited · bells {} · titles {}",
            state.bell_count, state.title_changed_count
        )
    } else {
        format!(
            "shell running · bells {} · titles {}",
            state.bell_count, state.title_changed_count
        )
    };
    status_label.set_text(&status_text);

    let screen = match Formatter::new(&state.terminal, OutputFormat::Plain, true) {
        Ok(formatter) => formatter
            .format()
            .unwrap_or_else(|err| format!("[format err: {err}]")),
        Err(err) => format!("[formatter err: {err}]"),
    };
    screen_label.set_text(&screen);
}

fn run_worker(
    pty_bytes_tx: Sender<Vec<u8>>,
    shutdown_rx: Receiver<()>,
    write_rx: tokio_mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("[mugenkai] failed to start tokio runtime: {err}");
            return;
        }
    };

    runtime.block_on(async move {
        if let Err(err) = worker_main(&pty_bytes_tx, shutdown_rx, write_rx).await {
            eprintln!("[mugenkai] pane worker exited with error: {err}");
        }
    });
}

async fn worker_main(
    pty_bytes_tx: &Sender<Vec<u8>>,
    shutdown_rx: Receiver<()>,
    mut write_rx: tokio_mpsc::UnboundedReceiver<Vec<u8>>,
) -> Result<(), String> {
    let child = Child::spawn(SpawnOptions::default()).map_err(|e| format!("spawn shell: {e}"))?;
    let mut buf = vec![0u8; 8192];

    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        tokio::select! {
            biased;

            read_result = child.read(&mut buf) => {
                match read_result {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        if pty_bytes_tx.send(chunk).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        return Err(format!("pty read: {err}"));
                    }
                }
            }

            key_bytes = write_rx.recv() => {
                match key_bytes {
                    Some(bytes) => {
                        let _ = child.write_all(&bytes).await;
                    }
                    None => break,
                }
            }
        }
    }

    Ok(())
}
