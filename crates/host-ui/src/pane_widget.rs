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

use host_core::{CursorConfig as CoreCursorConfig, CursorStyle};

use crate::pango_markup::{CursorShape, CursorSpan, html_to_pango_markup, splice_cursor};

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
    /// Whether the cursor is currently drawn (blink phase).
    /// Flipped by the 30ms tick once the blink interval elapses.
    /// Always true when `cursor_config.blink` is false.
    cursor_blink_on: bool,
    /// Timestamp of the last blink state toggle. Used to throttle
    /// the toggle to the configured blink interval.
    cursor_blink_last_toggle: std::time::Instant,
    /// Pre-built pango span for the configured cursor shape +
    /// colors. Captured at construction time; rebuilt if config
    /// hot-reload lands later.
    cursor_span: CursorSpan,
    /// Blink cadence from config.
    cursor_blink_interval: Duration,
    /// Whether blinking is enabled at all.
    cursor_blink_enabled: bool,
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
    // The glib timeout source that runs drain_pty_bytes every
    // 30ms. Wrapped in Option so Drop can take() and remove() it
    // — removing releases the closure (and its write_tx clone,
    // its state Rc clone, and its label clones), which in turn
    // lets PaneState drop cleanly instead of waiting for the
    // whole GTK main loop to exit.
    timer_source: Option<glib::SourceId>,
}

impl Drop for PaneWidget {
    fn drop(&mut self) {
        if let Some(source) = self.timer_source.take() {
            source.remove();
        }
    }
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
    /// Build a pane with default cursor settings. Used by tests
    /// and stand-alone demos; production code uses
    /// [`with_cursor_config`](Self::with_cursor_config).
    #[must_use]
    pub fn new() -> Self {
        Self::with_cursor_config(&CoreCursorConfig::default())
    }

    /// Build a pane with cursor appearance driven by the user's
    /// configuration.
    #[must_use]
    pub fn with_cursor_config(cursor_cfg: &CoreCursorConfig) -> Self {
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

        // Build the pango cursor span once from the user's
        // cursor config. `cursor_shape_from` + the shape-aware
        // CursorSpan constructor select Block vs Underline.
        let cursor_shape = match cursor_cfg.style {
            CursorStyle::Block => CursorShape::Block,
            CursorStyle::Underline => CursorShape::Underline,
        };
        let cursor_span = CursorSpan::new(cursor_shape, &cursor_cfg.color, &cursor_cfg.text_color);
        let cursor_blink_interval = Duration::from_millis(cursor_cfg.blink_interval_ms);

        let state = Rc::new(RefCell::new(PaneState {
            terminal,
            effects,
            effects_rx,
            bell_count: 0,
            title_changed_count: 0,
            exited: false,
            cursor_blink_on: true,
            cursor_blink_last_toggle: std::time::Instant::now(),
            cursor_span,
            cursor_blink_interval,
            cursor_blink_enabled: cursor_cfg.blink,
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
        let timer_source = glib::timeout_add_local(Duration::from_millis(30), move || {
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
            timer_source: Some(timer_source),
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
#[allow(dead_code)]
pub fn install_pane_key_controller<W: gtk4::prelude::IsA<gtk4::Widget>>(
    widget: &W,
    write_tx: PaneInputSender,
) {
    install_pane_key_controller_dynamic(widget, move || Some(write_tx.clone()));
}

/// Dynamic variant: the key controller looks up the current PTY
/// input sender on every keypress by calling `sender_fn`. Used by
/// multi-pane layouts where the target PTY is whichever pane has
/// tree-level focus at the moment the key fires.
pub fn install_pane_key_controller_dynamic<W, F>(widget: &W, sender_fn: F)
where
    W: gtk4::prelude::IsA<gtk4::Widget>,
    F: Fn() -> Option<PaneInputSender> + 'static,
{
    let key_controller = gtk4::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk4::PropagationPhase::Capture);
    key_controller.connect_key_pressed(move |_controller, keyval, _keycode, modifiers| {
        if let Some(bytes) = key_to_bytes(keyval, modifiers) {
            if let Some(tx) = sender_fn() {
                let _ = tx.send(bytes);
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    widget.add_controller(key_controller);
}

fn key_to_bytes(keyval: gdk::Key, modifiers: gdk::ModifierType) -> Option<Vec<u8>> {
    let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);

    // Ctrl+Shift+<anything> is reserved for window-level
    // accelerators (split H/V, close pane, new tab, etc.).
    // Terminal control bytes are Ctrl+letter — shift doesn't
    // participate, so losing Ctrl+Shift+X to the accelerator
    // system is safe. Returning None lets the capture-phase key
    // controller return Proceed so GTK's shortcut dispatch fires.
    if ctrl && shift {
        return None;
    }

    // Ctrl+Tab / Ctrl+Shift+Tab are reserved for focus cycling
    // between panes. Tab alone still sends `\t` to the PTY.
    // (Ctrl+Shift+Tab is already caught above, but we handle the
    // Tab keyval here for clarity and to cover any GDK variant
    // that reports ISO_Left_Tab with only Ctrl held.)
    if ctrl && matches!(keyval, gdk::Key::Tab | gdk::Key::ISO_Left_Tab) {
        return None;
    }

    // Ctrl+Q is reserved for the window's `win.quit` accelerator.
    if ctrl
        && !alt
        && let Some(ch) = keyval.to_unicode()
        && (ch == 'q' || ch == 'Q')
    {
        return None;
    }

    // Control + letter -> control byte (covers everything except
    // the reserved combos above).
    if ctrl
        && !alt
        && let Some(ch) = keyval.to_unicode()
        && ch.is_ascii_alphabetic()
    {
        let byte = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
        return Some(vec![byte]);
    }

    // Ctrl + anything-that-isn't-a-letter is reserved for window
    // accelerators (Ctrl+Comma = open settings, Ctrl+= / Ctrl+-
    // reserved for future font-size bindings). Returning None
    // lets the capture-phase controller propagate so GTK's
    // shortcut dispatch fires instead of sending the raw char
    // to the PTY.
    if ctrl && !alt {
        return None;
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
    let mut blink_flipped = false;
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

        // PTY activity resets the blink to visible and restarts
        // the interval so the cursor stays solid while the user
        // is typing. When the shell goes idle, the next tick
        // that crosses the configured blink interval starts
        // blinking. If blinking is disabled in config, the
        // cursor stays solid forever.
        if had_input {
            state_mut.cursor_blink_on = true;
            state_mut.cursor_blink_last_toggle = std::time::Instant::now();
        } else if state_mut.cursor_blink_enabled
            && state_mut.cursor_blink_last_toggle.elapsed() >= state_mut.cursor_blink_interval
        {
            state_mut.cursor_blink_on = !state_mut.cursor_blink_on;
            state_mut.cursor_blink_last_toggle = std::time::Instant::now();
            blink_flipped = true;
        }

        if had_input || blink_flipped {
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

    // Ask libghostty-vt to render the screen state as HTML (which
    // includes per-style runs with fg/bg colors + bold/italic/
    // underline), then translate to pango markup and push into the
    // label. set_markup crashes on invalid markup, so fall back to
    // set_text with plain format on parse errors.
    let html = match Formatter::new(&state.terminal, OutputFormat::Html, true) {
        Ok(formatter) => formatter.format(),
        Err(err) => Err(err),
    };
    match html {
        Ok(html) => {
            let mut markup = html_to_pango_markup(&html);
            // Splice the configured cursor at the terminal's
            // current position. Must run after the HTML → pango
            // translation because splice_cursor walks pango tags,
            // not HTML tags. Only splice when (a) the terminal's
            // cursor is visible per DEC mode 25 AND (b) the blink
            // phase is currently "on".
            let cursor = state.terminal.cursor_position();
            if cursor.visible && state.cursor_blink_on {
                markup = splice_cursor(
                    &markup,
                    cursor.row as usize,
                    cursor.col as usize,
                    &state.cursor_span,
                );
            }
            screen_label.set_markup(&markup);
        }
        Err(err) => {
            // HTML format failed (rare) — drop to plain text so
            // the user still sees what's happening.
            let plain = Formatter::new(&state.terminal, OutputFormat::Plain, true)
                .and_then(|f| f.format())
                .unwrap_or_else(|inner| format!("[format err: {err}; {inner}]"));
            screen_label.set_text(&plain);
        }
    }
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

            // Periodic wakeup so the shutdown_rx check at the top
            // of the loop fires even when the shell is idle and
            // nothing is coming in via the write channel. Without
            // this, `close_focused` followed by `worker.join()`
            // in PaneController::drop deadlocks the GTK main
            // thread: the shutdown signal never reaches the
            // select because glib's timeout_add_local closure
            // holds a clone of write_tx alive, so write_rx.recv()
            // never returns None, and child.read() stays blocked
            // on an idle pty.
            () = tokio::time::sleep(Duration::from_millis(50)) => {
                // No-op — the check at the top of `loop` handles
                // the actual shutdown observation.
            }
        }
    }

    Ok(())
}
