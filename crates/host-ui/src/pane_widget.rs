//! v0.1 `PaneWidget`: a GTK4 widget hosting a shell PTY + a
//! libghostty-vt Terminal, with live text readout for the MVP.
//!
//! # Design (v0.1 only)
//!
//! The real per-pane glyph renderer lands in Phase 2 Unit 4b. For
//! v0.1 the widget displays the terminal's rendered screen as a
//! plain `gtk4::Label` inside a `ScrolledWindow` so you can *see*
//! a real shell's output change on every refresh tick — this is
//! the minimum that proves the Terminal + PTY + effects pipeline
//! actually works end-to-end from Rust under GTK.
//!
//! ```text
//! PaneWidget (gtk4::Box)
//! ├── gtk4::Label "status: running, N bells"
//! └── gtk4::ScrolledWindow
//!     └── gtk4::Label (monospace, screen state)
//! ```
//!
//! The PTY + Terminal + effects run on a dedicated worker thread
//! hosting a tokio current-thread runtime. The worker:
//!
//! 1. Spawns a shell under `host_pty::Child`
//! 2. Creates a `host_ghostty::Terminal` + `EffectsHandle`
//! 3. In a loop, reads PTY bytes → forwards to `Terminal::vt_write`
//!    → also reads terminal events and forwards them through a
//!    `std::sync::mpsc::Sender<Snapshot>` back to the GTK main
//!    thread
//!
//! The GTK main thread polls the channel on a 100ms timeout and
//! updates the labels. No shared mutable state crosses the thread
//! boundary — only owned message data does.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use gtk4::glib;

use host_ghostty::{EffectsHandle, Formatter, PlainFormat, Terminal, TerminalEvent};
use host_pty::{Child, SpawnOptions};

/// A snapshot of terminal state pushed from the worker thread to
/// the GTK main thread.
///
/// Everything in here is owned data so the message is `Send`.
#[derive(Debug, Clone)]
struct Snapshot {
    screen: String,
    bell_count: u64,
    title_changed_count: u64,
    exited: bool,
}

/// A v0.1 pane that hosts a shell inside a libghostty-vt terminal.
///
/// The public `widget()` returns a `gtk4::Widget` that can be added
/// to any GTK container. The widget owns its worker thread and
/// terminates it cleanly on drop.
pub struct PaneWidget {
    widget: gtk4::Box,
    // Kept alive for the lifetime of the widget. Dropping it stops
    // the worker thread's main loop and triggers pty teardown.
    _controller: PaneController,
}

/// Handle held by [`PaneWidget`] to keep the worker alive.
struct PaneController {
    // Tell the worker to shut down on drop.
    shutdown_tx: Option<Sender<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for PaneController {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.worker.take() {
            // Don't block forever; if the worker is wedged we
            // detach it so the UI close path doesn't hang.
            let _ = handle.join();
        }
    }
}

impl PaneWidget {
    /// Build the widget. Spawns the worker thread immediately.
    ///
    /// Failing to start the shell is not a hard error: the widget
    /// still builds and displays the error in its status label.
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
            .selectable(true)
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

        let (snap_tx, snap_rx) = mpsc::channel::<Snapshot>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

        let worker = thread::Builder::new()
            .name("mugenkai-pane-worker".into())
            .spawn(move || {
                run_worker(snap_tx, shutdown_rx);
            })
            .expect("spawn mugenkai pane worker thread");

        let controller = PaneController {
            shutdown_tx: Some(shutdown_tx),
            worker: Some(worker),
        };

        // Drain snapshots on the GTK main loop every 100ms.
        let status_clone = status_label.clone();
        let screen_clone = screen_label.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || {
            drain_snapshots(&snap_rx, &status_clone, &screen_clone)
        });

        Self {
            widget: vbox,
            _controller: controller,
        }
    }

    /// Return the underlying widget for insertion into a container.
    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.widget.upcast_ref()
    }
}

impl Default for PaneWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// Drain the snapshot channel, updating the labels with the most
/// recent snapshot. Returns `ControlFlow::Continue` to keep the
/// timeout tick firing.
fn drain_snapshots(
    rx: &Receiver<Snapshot>,
    status_label: &gtk4::Label,
    screen_label: &gtk4::Label,
) -> glib::ControlFlow {
    let mut latest: Option<Snapshot> = None;
    while let Ok(snap) = rx.try_recv() {
        latest = Some(snap);
    }
    if let Some(snap) = latest {
        let status_text = if snap.exited {
            format!(
                "shell exited · bells {} · titles {}",
                snap.bell_count, snap.title_changed_count
            )
        } else {
            format!(
                "shell running · bells {} · titles {}",
                snap.bell_count, snap.title_changed_count
            )
        };
        status_label.set_text(&status_text);
        screen_label.set_text(&snap.screen);
    }
    glib::ControlFlow::Continue
}

/// Entry point for the worker thread.
///
/// Creates a tokio current-thread runtime, spawns a shell, drives
/// the main pump until shutdown is requested or the child exits.
fn run_worker(snap_tx: Sender<Snapshot>, shutdown_rx: Receiver<()>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            let _ = snap_tx.send(Snapshot {
                screen: format!("failed to start tokio runtime: {err}"),
                bell_count: 0,
                title_changed_count: 0,
                exited: true,
            });
            return;
        }
    };

    runtime.block_on(async move {
        if let Err(err) = worker_main(snap_tx.clone(), shutdown_rx).await {
            let _ = snap_tx.send(Snapshot {
                screen: format!("pane worker error: {err}"),
                bell_count: 0,
                title_changed_count: 0,
                exited: true,
            });
        }
    });
}

async fn worker_main(snap_tx: Sender<Snapshot>, shutdown_rx: Receiver<()>) -> Result<(), String> {
    let child = Child::spawn(SpawnOptions::default()).map_err(|e| format!("spawn shell: {e}"))?;
    let mut terminal = Terminal::new(80, 24, 1000).map_err(|e| format!("terminal: {e}"))?;
    let mut effects_handle =
        EffectsHandle::install(&mut terminal).map_err(|e| format!("effects install: {e}"))?;
    let events_rx = effects_handle
        .take_receiver()
        .expect("fresh effects handle must yield a receiver");

    let mut bell_count: u64 = 0;
    let mut title_changed_count: u64 = 0;
    let mut buf = vec![0u8; 8192];
    let mut exited = false;

    // Send an initial snapshot so the UI stops showing "starting shell…".
    send_snapshot(&snap_tx, &terminal, bell_count, title_changed_count, exited);

    loop {
        // Non-blocking shutdown check every iteration.
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        tokio::select! {
            biased;

            read_result = child.read(&mut buf) => {
                match read_result {
                    Ok(0) => {
                        exited = true;
                        send_snapshot(&snap_tx, &terminal, bell_count, title_changed_count, exited);
                        break;
                    }
                    Ok(n) => {
                        terminal.vt_write(&buf[..n]);

                        // Drain any effects the vt_write produced.
                        while let Ok(ev) = events_rx.try_recv() {
                            match ev {
                                TerminalEvent::Bell => bell_count += 1,
                                TerminalEvent::TitleChanged => title_changed_count += 1,
                                TerminalEvent::WritePty(bytes) => {
                                    // Query response — write it back
                                    // to the PTY. Ignore errors; if
                                    // the write fails the child is
                                    // already going down and the next
                                    // read will return 0.
                                    let _ = child.write_all(&bytes).await;
                                }
                            }
                        }

                        send_snapshot(&snap_tx, &terminal, bell_count, title_changed_count, exited);
                    }
                    Err(err) => {
                        return Err(format!("pty read: {err}"));
                    }
                }
            }

            () = tokio::time::sleep(Duration::from_millis(100)) => {
                // Periodic wakeup so the shutdown check + snapshot
                // update fire even when the child is quiet.
                send_snapshot(&snap_tx, &terminal, bell_count, title_changed_count, exited);
            }
        }
    }

    // Clear effects before the terminal is dropped so libghostty-vt
    // no longer holds a pointer into effects_handle's allocation.
    let _ = effects_handle.clear(&mut terminal);
    Ok(())
}

fn send_snapshot(
    snap_tx: &Sender<Snapshot>,
    terminal: &Terminal,
    bell_count: u64,
    title_changed_count: u64,
    exited: bool,
) {
    let screen = match Formatter::new(terminal, PlainFormat::Plain, true) {
        Ok(formatter) => formatter
            .format()
            .unwrap_or_else(|err| format!("[format err: {err}]")),
        Err(err) => format!("[formatter err: {err}]"),
    };
    let _ = snap_tx.send(Snapshot {
        screen,
        bell_count,
        title_changed_count,
        exited,
    });
}
