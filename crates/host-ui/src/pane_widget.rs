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
use gtk4::pango;
use tokio::sync::mpsc as tokio_mpsc;

use host_ghostty::{
    CursorPosition, EffectsHandle, Formatter, MouseAction, MouseButton, MouseEncoder, OutputFormat,
    Terminal, TerminalEvent,
};
use host_pty::{Child, SpawnOptions, Winsize};

use host_core::{
    CursorConfig as CoreCursorConfig, CursorStyle, FontConfig as CoreFontConfig, MugenkaiConfig,
};
use crate::inline_chat::InlineChat;

use crate::pango_markup::{CursorShape, CursorSpan, html_to_pango_markup, splice_cursor};

/// Cloneable sender that writes raw bytes to a pane's PTY child.
pub type PaneInputSender = tokio_mpsc::UnboundedSender<Vec<u8>>;

/// Live per-pane state owned on the GTK main thread.
#[allow(clippy::struct_excessive_bools)]
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
    /// Whether this pane is the tree-focused one. Only focused
    /// panes render a cursor — unfocused panes stay cursor-less
    /// so split layouts don't show phantom cursors everywhere.
    /// Updated via [`PaneWidget::set_focused`] from the
    /// [`crate::split_container::SplitContainer`] on every
    /// focus change and rematerialization.
    focused: bool,
    /// Set when `set_focused` changes focus state; the next 30ms
    /// tick redraws the label to show/hide the cursor
    /// immediately instead of waiting for PTY activity.
    needs_refresh: bool,
    /// Pixel dimensions of one monospace cell, measured once from
    /// the screen label's pango context. Used by the mouse encoder
    /// + libghostty-vt resize, both of which want integer pixels.
    cell_width_px: u32,
    cell_height_px: u32,
    /// Per-cell width in pango units (1/1024 of a pixel). Used by
    /// the resize watcher to compute `cols` with sub-pixel
    /// precision: `cols = (viewport_w * SCALE) / cell_width_units`.
    /// Floor division then guarantees that
    /// `cols * cell_width_units <= viewport_w * SCALE`, so the
    /// rendered grid never exceeds the viewport horizontally.
    cell_width_units: i32,
    /// Last cell grid dimensions applied to the terminal + PTY.
    /// The resize watcher polls the widget every 30ms but only
    /// pushes a resize when these change, avoiding `TIOCSWINSZ`
    /// storms while the user drags the window.
    last_cols: u16,
    last_rows: u16,
    /// Per-pane mouse encoder. Translates GTK scroll/click events
    /// into the protocol bytes the application running in the
    /// terminal expects (X10 / SGR / UTF8 / URXVT). One encoder per
    /// pane so reuse keeps the per-event allocation cost flat.
    mouse_encoder: MouseEncoder,
    /// Pango font description string used to wrap every rendered
    /// label update in `<span font_desc="...">` — this is how the
    /// user's configured monospace font + size reaches the label
    /// without touching GTK CSS. Formatted once at construction
    /// time as `"<family> <size>"` (pango's native syntax).
    font_desc_str: String,
    /// Outer pango `foreground=` color applied to the whole label
    /// markup. ANSI-colored runs from libghostty-vt override it;
    /// uncolored runs inherit — so shell prompts (which typically
    /// use ANSI codes) still render in their own colors while
    /// plain output picks up the theme's terminal-fg.
    terminal_fg: String,
    /// Emit per-refresh cursor diagnostics to stderr when
    /// `MUGENKAI_DEBUG_CURSOR=1` was set at process start. Gated
    /// at `PaneState` construction so the env var only needs to be
    /// checked once and the hot render loop stays branch-cheap.
    debug_cursor: bool,
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
    screen_label: gtk4::Label,
    state: Rc<RefCell<PaneState>>,
    _controller: PaneController,
    inline_chat: Rc<InlineChat>,
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
    /// Build a pane with default cursor + font settings. Used by
    /// tests and stand-alone demos; production code uses
    /// [`with_config`](Self::with_config).
    #[must_use]
    pub fn new() -> Self {
        Self::with_cursor_config(&CoreCursorConfig::default())
    }

    /// Build a pane driven by the user's full configuration —
    /// cursor style, colors, blink cadence, AND font family/size.
    ///
    /// This is the production entry point; [`new`](Self::new) and
    /// [`with_cursor_config`](Self::with_cursor_config) are
    /// convenience wrappers for tests.
    #[must_use]
    pub fn with_config(cfg: &MugenkaiConfig) -> Self {
        let terminal_fg = crate::css::theme_terminal_fg(&cfg.theme);
        Self::with_all(&cfg.cursor, &cfg.font, terminal_fg)
    }

    /// Build a pane with cursor appearance driven by the user's
    /// configuration, using the system default font.
    #[must_use]
    pub fn with_cursor_config(cursor_cfg: &CoreCursorConfig) -> Self {
        Self::with_all(cursor_cfg, &CoreFontConfig::default(), "#e6e8ee")
    }

    /// Build a pane with explicit cursor + font configuration.
    /// Kept for backwards compatibility; new callers should use
    /// [`with_config`](Self::with_config).
    #[must_use]
    pub fn with_cursor_and_font(cursor_cfg: &CoreCursorConfig, font_cfg: &CoreFontConfig) -> Self {
        Self::with_all(cursor_cfg, font_cfg, "#e6e8ee")
    }

    /// Full constructor with explicit cursor, font, and terminal
    /// foreground color. All other constructors delegate here.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn with_all(
        cursor_cfg: &CoreCursorConfig,
        font_cfg: &CoreFontConfig,
        terminal_fg: &str,
    ) -> Self {
        let vbox = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(6)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        // Debug status label — hidden in production builds.
        // Shows "shell running · bells N · titles N" for
        // development diagnostics only.
        let status_label = gtk4::Label::builder()
            .label("starting shell…")
            .halign(gtk4::Align::Start)
            .css_classes(["dim-label"])
            .visible(false)
            .build();
        vbox.append(&status_label);

        let screen_label = gtk4::Label::builder()
            .label("")
            .halign(gtk4::Align::Start)
            .valign(gtk4::Align::Start)
            // Selectable so the user can drag-select text to copy.
            // `refresh_labels` saves + restores selection bounds
            // across set_markup calls so the selection survives
            // the 30ms redraw tick.
            .selectable(true)
            .wrap(false)
            .xalign(0.0)
            .yalign(0.0)
            .build();
        screen_label.add_css_class("monospace");

        let scroller = gtk4::ScrolledWindow::builder()
            // Horizontal policy must stay Automatic. With Never, GTK
            // requests the child's natural width as the scroller's
            // natural width, which then propagates up to the window
            // and makes it grow to fit whatever pango decided the
            // label needed. The cols-computation below uses pango
            // units + floor division to guarantee the grid never
            // exceeds the viewport, so the auto scrollbar stays
            // hidden in practice.
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&screen_label)
            .build();
        vbox.append(&scroller);

        // Build the pango font description string once. Pango's
        // `from_string` accepts `"<family> <size>"` like
        // "JetBrains Mono 11" — we emit the same format here and
        // both the cell-measurement probe and the markup wrapper
        // use it, guaranteeing they agree on glyph metrics.
        let font_desc_str = format!("{} {}", font_cfg.family, font_cfg.size_pt);

        // Measure one monospace cell's pixel size now, before the
        // worker thread starts. We measure with the user's
        // configured font so the cell grid matches what pango
        // will actually lay out when refresh_labels runs.
        let (cell_width_px, cell_height_px, cell_width_units) =
            measure_monospace_cell(&screen_label, &font_desc_str);

        let initial_cols: u16 = 80;
        let initial_rows: u16 = 24;

        let mut terminal =
            Terminal::new(initial_cols, initial_rows, 1000).expect("construct terminal");
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

        let mut mouse_encoder = MouseEncoder::new().expect("construct mouse encoder");
        // Seed the encoder with the initial 80x24 grid so any scroll
        // event before the first resize tick still encodes coherent
        // coordinates. The resize watcher refreshes this whenever
        // the widget's pixel allocation changes.
        mouse_encoder.set_size(
            u32::from(initial_cols) * cell_width_px,
            u32::from(initial_rows) * cell_height_px,
            cell_width_px,
            cell_height_px,
        );

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
            // New panes start unfocused; SplitContainer flips the
            // active pane to focused immediately after construction
            // via set_focused(true).
            focused: false,
            needs_refresh: false,
            cell_width_px,
            cell_height_px,
            cell_width_units,
            last_cols: initial_cols,
            last_rows: initial_rows,
            mouse_encoder,
            font_desc_str,
            terminal_fg: terminal_fg.to_string(),
            debug_cursor: std::env::var("MUGENKAI_DEBUG_CURSOR")
                .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true")),
        }));

        let (pty_bytes_tx, pty_bytes_rx) = mpsc::channel::<Vec<u8>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let (write_tx, write_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, resize_rx) = tokio_mpsc::unbounded_channel::<Winsize>();

        let worker = thread::Builder::new()
            .name("mugenkai-pane-worker".into())
            .spawn(move || {
                run_worker(pty_bytes_tx, shutdown_rx, write_rx, resize_rx);
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
        let resize_tx_tick = resize_tx.clone();
        let timer_source = glib::timeout_add_local(Duration::from_millis(30), move || {
            drain_pty_bytes(
                &pty_bytes_rx,
                &state_tick,
                &write_tx_tick,
                &resize_tx_tick,
                &status_tick,
                &screen_tick,
                &scroller_tick,
            )
        });

        install_scroll_controller(&vbox, &scroller, &state, &write_tx);

        // Inline chat: embedded at the bottom of each pane,
        // toggled via Ctrl+Shift+I. Auto-includes terminal
        // context when the user asks a question.
        let inline_chat = InlineChat::new();
        vbox.append(inline_chat.widget());

        {
            let state_for_ctx = Rc::clone(&state);
            inline_chat.set_get_terminal_text(move || {
                let s = state_for_ctx.borrow();
                Formatter::new(&s.terminal, OutputFormat::Plain, true)
                    .and_then(|f| f.format())
                    .unwrap_or_default()
            });
        }
        {
            let tx = write_tx.clone();
            inline_chat.set_send_to_terminal(move |text: &str| {
                let _ = tx.send(text.as_bytes().to_vec());
            });
        }

        Self {
            widget: vbox,
            write_tx,
            screen_label,
            state,
            _controller: controller,
            inline_chat,
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

    /// Return the visible terminal grid rendered as plain text.
    ///
    /// Used by the "Explain this in chat" action so the chat pane
    /// can ship whatever the user is currently looking at to
    /// Claude without the SGR noise that the HTML formatter would
    /// produce. Falls back to an empty string on formatter error.
    #[must_use]
    pub fn visible_screen_text(&self) -> String {
        let state = self.state.borrow();
        Formatter::new(&state.terminal, OutputFormat::Plain, true)
            .and_then(|f| f.format())
            .unwrap_or_default()
    }

    /// Toggle the inline chat widget at the bottom of this pane.
    pub fn toggle_inline_chat(&self) {
        self.inline_chat.toggle();
    }

    /// The terminal's current title as set by the shell via OSC 0/2.
    #[must_use]
    pub fn terminal_title(&self) -> String {
        self.state.borrow().terminal.title()
    }

    /// True once the pane's shell process has exited (typed `exit`,
    /// Ctrl+D, killed externally, etc.). Set by the 30ms tick when
    /// the pty byte channel becomes disconnected.
    ///
    /// The `SplitContainer` reaper polls this every 200ms and
    /// removes any pane that reports true.
    #[must_use]
    pub fn has_exited(&self) -> bool {
        self.state.borrow().exited
    }

    /// Return the user's current drag-selection as plain text, or
    /// `None` if no selection is active.
    ///
    /// Reads from the selectable `gtk4::Label` that renders the
    /// terminal grid — the selection range tracks pango byte
    /// offsets, not cell coordinates, so the output is whatever
    /// contiguous range of glyphs the user highlighted (which
    /// matches what they expect to copy).
    #[must_use]
    pub fn copy_selection(&self) -> Option<String> {
        let (start, end) = self.screen_label.selection_bounds()?;
        let text = self.screen_label.text();
        let chars: Vec<char> = text.chars().collect();
        #[allow(clippy::cast_sign_loss)]
        let start_usize = start.max(0) as usize;
        #[allow(clippy::cast_sign_loss)]
        let end_usize = end.max(0) as usize;
        let lo = start_usize.min(chars.len());
        let hi = end_usize.min(chars.len());
        if lo >= hi {
            return None;
        }
        Some(chars[lo..hi].iter().collect())
    }

    /// Write `text` to this pane's PTY input channel as if the
    /// user had typed it. Used by the paste action to push
    /// clipboard contents into the shell.
    ///
    /// No bracketed-paste-mode wrapping yet — apps that have DEC
    /// mode 2004 enabled (vim, tmux) will see paste as regular
    /// typing. A later commit can query the terminal's bracketed
    /// paste state and wrap in `ESC[200~ … ESC[201~` when on.
    pub fn paste_text(&self, text: &str) {
        let _ = self.write_tx.send(text.as_bytes().to_vec());
    }

    /// Mark this pane as focused or unfocused from the tree's
    /// perspective. Only focused panes render a cursor; others
    /// stay cursor-less.
    ///
    /// Called by [`crate::split_container::SplitContainer`] on
    /// focus changes and rematerialization. Schedules an
    /// immediate label redraw on the next 30ms tick so the
    /// cursor appears/disappears promptly — up to one tick of
    /// lag, which is imperceptible.
    pub fn set_focused(&self, focused: bool) {
        let mut state = self.state.borrow_mut();
        if state.focused == focused {
            return;
        }
        state.focused = focused;
        state.needs_refresh = true;
        // Reset the blink so the cursor appears solid the moment
        // focus arrives, matching xterm/gnome-terminal behavior.
        if focused {
            state.cursor_blink_on = true;
            state.cursor_blink_last_toggle = std::time::Instant::now();
        }
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
        if let Some(bytes) = key_to_bytes(keyval, modifiers)
            && let Some(tx) = sender_fn()
        {
            let _ = tx.send(bytes);
            return glib::Propagation::Stop;
        }
        // No terminal to forward to (chat/settings tab) or no
        // byte mapping — let the event reach the focused widget
        // so Entry/TextView inputs work normally.
        glib::Propagation::Proceed
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
    resize_tx: &tokio_mpsc::UnboundedSender<Winsize>,
    status_label: &gtk4::Label,
    screen_label: &gtk4::Label,
    scroller: &gtk4::ScrolledWindow,
) -> glib::ControlFlow {
    let mut had_input = false;
    let mut blink_flipped = false;
    let mut write_pty_responses: Vec<Vec<u8>> = Vec::new();

    {
        let mut state_mut = state.borrow_mut();

        loop {
            match pty_bytes_rx.try_recv() {
                Ok(bytes) => {
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
                Err(mpsc::TryRecvError::Empty) => break,
                // The worker thread closed pty_bytes_tx, which only
                // happens after its `child.read` returned Ok(0) —
                // i.e. the shell exited (`exit`, Ctrl+D, kill, etc).
                // Mark the pane as dead so the reaper timer in
                // window.rs can remove it on the next poll.
                Err(mpsc::TryRecvError::Disconnected) => {
                    state_mut.exited = true;
                    state_mut.needs_refresh = true;
                    break;
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
            && state_mut.focused
            && state_mut.cursor_blink_last_toggle.elapsed() >= state_mut.cursor_blink_interval
        {
            // Only blink on the focused pane — unfocused panes
            // have no cursor to flip.
            state_mut.cursor_blink_on = !state_mut.cursor_blink_on;
            state_mut.cursor_blink_last_toggle = std::time::Instant::now();
            blink_flipped = true;
        }

        let resize_dirty = maybe_apply_resize(&mut state_mut, scroller, resize_tx);

        // set_focused schedules a refresh so the cursor can
        // appear/disappear on focus change without waiting for
        // PTY activity or a blink tick.
        let focus_dirty = state_mut.needs_refresh;
        if focus_dirty {
            state_mut.needs_refresh = false;
        }

        if had_input || blink_flipped || focus_dirty || resize_dirty {
            refresh_labels(&state_mut, status_label, screen_label);
        }
    }

    for bytes in write_pty_responses {
        let _ = write_tx.send(bytes);
    }

    // Auto-scroll to bottom:
    //
    // - We defer via `idle_add_local_once` because `set_markup`
    //   invalidates pango layout and `adj.upper()` isn't reliable
    //   until GTK re-runs the layout pass, which happens on the
    //   next idle tick. Reading `adj.upper()` synchronously here
    //   returns the stale value — so the naive `set_value(upper -
    //   page)` call scrolls to the OLD bottom, not the new one.
    //   This is why claude and other alt-screen TUIs appeared
    //   scrolled-to-top when they first drew their UI: the label
    //   grew taller than its last known size, and the scroll
    //   adjustment was still the old (smaller) range.
    //
    // - Scroll on any refresh-triggering event (input, blink,
    //   focus, resize) so alt-screen transitions also snap to
    //   the bottom — that's where the cursor/input row sits in
    //   every TUI we care about (claude, vim, htop, less).
    if had_input {
        let scroller_for_idle = scroller.clone();
        glib::idle_add_local_once(move || {
            let adj = scroller_for_idle.vadjustment();
            adj.set_value(adj.upper() - adj.page_size());
        });
    }

    glib::ControlFlow::Continue
}

fn refresh_labels(state: &PaneState, status_label: &gtk4::Label, screen_label: &gtk4::Label) {
    // Save any active user selection before we rewrite the markup.
    // `set_markup` replaces the label's text and clears selection;
    // we restore the same byte offsets afterwards so drag-selection
    // survives blink/resize ticks. If the terminal content shifts
    // between frames (e.g. `tail -f`) the offsets may point to
    // different characters — acceptable for the v1 feel, and the
    // user can re-drag if needed.
    let saved_selection = screen_label.selection_bounds();

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
    // `trim` is a libghostty-vt hint only: even with trim=false
    // the formatter still stops emitting rows after the last
    // non-blank line (see regression test
    // `formatter_trim_false_emits_full_grid`). We normalise the
    // markup to the full grid ourselves below so splice_cursor
    // can always walk newlines to the cursor's row.
    let html = match Formatter::new(&state.terminal, OutputFormat::Html, true) {
        Ok(formatter) => formatter.format(),
        Err(err) => Err(err),
    };
    let font_desc_attr = &state.font_desc_str;
    let fg_attr = &state.terminal_fg;

    match html {
        Ok(html) => {
            let mut markup = html_to_pango_markup(&html);
            // Pad the markup up to the terminal's row count so
            // splice_cursor can always locate the cursor row by
            // walking newlines inline. libghostty-vt's formatter
            // only emits rows with content even when `trim=false`
            // (confirmed by `formatter_trim_false_emits_full_grid`
            // regression test in host-ghostty) — so without this
            // pad, a cursor that lands on a logically-blank row
            // below the last non-blank one falls into
            // splice_cursor's "past all content" path and the
            // cursor ends up rendered below the viewport. That's
            // the exact symptom users hit inside `claude -c` and
            // after its ungraceful exit: no cursor until `clear`
            // brings the grid back to something the formatter
            // emits in full.
            let emitted_rows = markup.matches('\n').count() + 1;
            let target_rows = state.last_rows as usize;
            if emitted_rows < target_rows {
                markup.reserve(target_rows - emitted_rows);
                for _ in emitted_rows..target_rows {
                    markup.push('\n');
                }
            }
            // Splice the configured cursor at the terminal's
            // current position. Must run after the HTML → pango
            // translation because splice_cursor walks pango tags,
            // not HTML tags. Only splice when ALL of:
            //   - this pane is the tree-focused one
            //   - the terminal's cursor is visible (DEC mode 25)
            //   - the blink phase is currently "on"
            // Unfocused panes stay cursor-less so split layouts
            // don't show phantom cursors on every pane.
            let cursor = state.terminal.cursor_position();
            // Always draw a cursor on the focused pane — ignore
            // DEC mode 25 entirely.
            //
            // Rationale: TUI apps that hide the cursor (vim, less,
            // claude-code) almost always either (a) move the grid
            // cursor to where they want it visually, or (b) draw
            // their own glyph there. Respecting DEC 25 gave us
            // correct behavior in theory but in practice broke
            // two common cases:
            //
            // 1. Claude Code runs in an interactive input loop
            //    that keeps DEC 25 off but expects the terminal
            //    to render a cursor at the grid's cursor row/col.
            //    Respecting DEC 25 left the user typing blind.
            //
            // 2. Apps that hide the cursor and then exit without
            //    restoring it (sigkilled, crashed, or just buggy
            //    cleanup — claude-code is a frequent offender)
            //    left the user with no cursor at the bash prompt
            //    until they typed `clear` or `reset` to force a
            //    terminal reset. We now recover automatically.
            //
            // Tradeoff: apps that hide the cursor for legitimate
            // UI reasons (e.g. mid-redraw flicker suppression)
            // will see a cursor where they didn't want one for
            // one frame. Acceptable — compared to losing the
            // cursor for minutes, a single-frame flicker is
            // invisible.
            let _ = cursor.visible;
            if state.focused && state.cursor_blink_on {
                if state.debug_cursor {
                    debug_log_cursor(state, cursor, &markup);
                }
                markup = splice_cursor(
                    &markup,
                    cursor.row as usize,
                    cursor.col as usize,
                    &state.cursor_span,
                );
            }
            // Wrap the whole rendered grid in an outer span that
            // sets the user's configured monospace font. Pango
            // spans inherit attributes from parents, so the inner
            // fg/bg spans layer on top without overriding this.
            let wrapped = format!(
                "<span font_desc=\"{font_desc_attr}\" foreground=\"{fg_attr}\">{markup}</span>"
            );
            screen_label.set_markup(&wrapped);
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

    // Restore the user's selection range. Offsets are character
    // positions into the label's plain-text view; `select_region`
    // is lenient about out-of-range values and will clamp to the
    // available range.
    if let Some((start, end)) = saved_selection {
        screen_label.select_region(start, end);
    }
}

/// Emit a one-line stderr record describing the current cursor
/// state + the pango markup around the targeted row. Enabled per
/// pane via the `MUGENKAI_DEBUG_CURSOR=1` env var — kept behind
/// that gate because every refresh tick would otherwise spam the
/// logs at ~30Hz.
///
/// Fields logged (space-separated `k=v` pairs):
/// - `grid` — pane's last-applied `cols × rows`
/// - `cur` — `(col, row)` from `ghostty_terminal_get`
/// - `vis_bit` — the DEC mode 25 bit ghostty reports (we ignore
///   it for rendering but log it for correlation)
/// - `alt` — whether the terminal is on the alternate screen
/// - `row_chars` — rendered-character count on the cursor row
/// - `row_preview` — up to 80 chars of plain text from that row
fn debug_log_cursor(state: &PaneState, cursor: CursorPosition, markup: &str) {
    let (row_chars, row_preview) = markup_row_preview(markup, cursor.row as usize);
    eprintln!(
        "[mg-cursor] grid={}x{} cur=({},{}) vis_bit={} alt={} blink_on={} row_chars={} row=\"{}\"",
        state.last_cols,
        state.last_rows,
        cursor.col,
        cursor.row,
        cursor.visible,
        state.terminal.on_alt_screen(),
        state.cursor_blink_on,
        row_chars,
        row_preview,
    );
}

/// Extract a plain-text preview of row `target_row` from pango
/// markup, skipping `<...>` tag content. Also returns the total
/// rendered-character count on that row so the caller can tell
/// whether the cursor's column is past the row's content.
///
/// Public for unit testing inside this crate.
pub(crate) fn markup_row_preview(markup: &str, target_row: usize) -> (usize, String) {
    let mut row: usize = 0;
    let mut in_tag = false;
    let mut count: usize = 0;
    let mut preview = String::new();
    let mut chars = markup.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if ch == '\n' {
            if row == target_row {
                return (count, preview);
            }
            row += 1;
            count = 0;
            preview.clear();
            continue;
        }
        if ch == '&' {
            // Collapse `&lt;` `&gt;` `&amp;` into a single logical
            // char. Any unknown entity still consumes one char.
            let mut entity = String::from("&");
            while let Some(&peek) = chars.peek() {
                entity.push(peek);
                chars.next();
                if peek == ';' {
                    break;
                }
            }
            if row == target_row {
                count += 1;
                if preview.len() < 80 {
                    preview.push_str(&entity);
                }
            }
            continue;
        }
        if row == target_row {
            count += 1;
            if preview.len() < 80 {
                preview.push(ch);
            }
        }
    }
    // Reached EOF: caller may be asking about a row past the end.
    if row == target_row {
        (count, preview)
    } else {
        (0, String::new())
    }
}

/// Attach a `gtk4::EventControllerScroll` to `vbox` that forwards
/// scroll-wheel events to the pty when the running application has
/// activated mouse tracking (vim, htop, less in some modes, tmux),
/// and otherwise lets the surrounding `ScrolledWindow` handle the
/// event natively for normal terminal-scrollback navigation.
///
/// One discrete wheel notch produces one [`MouseButton::WheelUp`]
/// or [`MouseButton::WheelDown`] press. Smooth-scroll deltas
/// accumulate inside `gtk4::EventControllerScroll`'s flag-controlled
/// behavior — using `Discrete` here means the controller pre-quantises
/// for us and we always see integer "ticks".
fn install_scroll_controller(
    vbox: &gtk4::Box,
    scroller: &gtk4::ScrolledWindow,
    state: &Rc<RefCell<PaneState>>,
    write_tx: &PaneInputSender,
) {
    let controller = gtk4::EventControllerScroll::new(
        gtk4::EventControllerScrollFlags::VERTICAL | gtk4::EventControllerScrollFlags::DISCRETE,
    );
    let state_for_scroll = Rc::clone(state);
    let write_tx_for_scroll = write_tx.clone();
    let scroller_for_scroll = scroller.clone();
    controller.connect_scroll(move |_, _dx, dy| {
        // Empty deltas show up occasionally on touchpads; nothing
        // to do.
        if dy == 0.0 {
            return glib::Propagation::Proceed;
        }

        let mut state_mut = state_for_scroll.borrow_mut();

        let mouse_tracking = state_mut.terminal.mouse_tracking_active();
        let alt_screen = state_mut.terminal.on_alt_screen();

        // Discrete-flag scrolls deliver |dy| == 1.0 per notch; on
        // some hardware this can be 2 or 3 if the OS coalesced them.
        // Cap so a frantic touchpad swipe can't dump 50 keys at once.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let ticks = (dy.abs().round() as u32).clamp(1, 8);
        let scroll_up = dy < 0.0;

        // Three cases, ordered by precedence:
        //
        // 1. The app explicitly asked for mouse tracking (htop, tmux,
        //    vim with `set mouse=a`) — encode real wheel events.
        // 2. We're on the alternate screen but the app didn't ask for
        //    mouse (vim/nano default, less). Synthesize Up/Down arrow
        //    presses so scroll wheel still moves the cursor — this is
        //    what gnome-terminal, kitty, and alacritty do.
        // 3. Primary screen + no mouse tracking — let the surrounding
        //    ScrolledWindow handle scrollback natively.
        if mouse_tracking {
            // Split-borrow the cell into &mut encoder + & terminal so
            // we can call sync + encode without tripping the borrow
            // checker.
            let PaneState {
                ref mut mouse_encoder,
                ref terminal,
                ..
            } = *state_mut;

            mouse_encoder.sync_from_terminal(terminal);

            // Centre of the viewport as a stand-in pointer position.
            // Apps that handle wheel events generally don't care where
            // in the pane the wheel ticked from.
            #[allow(clippy::cast_precision_loss)]
            let cx = (scroller_for_scroll.width() as f32) * 0.5;
            #[allow(clippy::cast_precision_loss)]
            let cy = (scroller_for_scroll.height() as f32) * 0.5;

            let button = if scroll_up {
                MouseButton::WheelUp
            } else {
                MouseButton::WheelDown
            };

            for _ in 0..ticks {
                match mouse_encoder.encode(MouseAction::Press, button, cx, cy) {
                    Ok(bytes) if !bytes.is_empty() => {
                        let _ = write_tx_for_scroll.send(bytes);
                    }
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!("[mugenkai] mouse encode failed: {err}");
                        break;
                    }
                }
            }
            glib::Propagation::Stop
        } else if alt_screen {
            let bytes: &[u8] = if scroll_up { b"\x1b[A" } else { b"\x1b[B" };
            for _ in 0..ticks {
                let _ = write_tx_for_scroll.send(bytes.to_vec());
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    vbox.add_controller(controller);
}

/// Poll the scroller's allocated pixel size and, if it differs
/// from the grid we last pushed, resize the libghostty-vt terminal
/// and send a `Winsize` to the pty worker so programs inside
/// (vim, less, htop) receive `SIGWINCH` and redraw.
///
/// Returns `true` if a resize was applied — the caller uses that
/// as a "label dirty" signal so the next `refresh_labels` call
/// renders into the new grid.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
fn maybe_apply_resize(
    state_mut: &mut PaneState,
    scroller: &gtk4::ScrolledWindow,
    resize_tx: &tokio_mpsc::UnboundedSender<Winsize>,
) -> bool {
    let width_px = scroller.width();
    let height_px = scroller.height();
    if width_px <= 0 || height_px <= 0 {
        return false;
    }

    let cw = state_mut.cell_width_px.max(1);
    let ch = state_mut.cell_height_px.max(1);

    // Compute cols using the exact per-cell pango unit width and
    // floor division. This guarantees `cols * cell_width_units` is
    // ≤ `viewport_w * SCALE`, so the rendered grid never exceeds
    // the viewport horizontally — the auto h-scrollbar stays
    // hidden in steady state and the window doesn't grow to fit
    // an oversized child.
    let cell_units = state_mut.cell_width_units.max(1);
    let viewport_units = width_px.saturating_mul(pango::SCALE);
    let cols_signed = (viewport_units / cell_units).max(1);
    let cols_u32 = u32::try_from(cols_signed).unwrap_or(1).clamp(1, 500);

    let rows_u32 = ((height_px as u32) / ch).clamp(1, 300);
    let cols = cols_u32 as u16;
    let rows = rows_u32 as u16;

    if cols == state_mut.last_cols && rows == state_mut.last_rows {
        return false;
    }

    match state_mut.terminal.resize(cols, rows, cw, ch) {
        Ok(()) => {
            state_mut.last_cols = cols;
            state_mut.last_rows = rows;
            // Keep the mouse encoder's screen geometry in sync so
            // scroll events report cell coordinates against the
            // current grid, not the stale 80x24 default.
            state_mut
                .mouse_encoder
                .set_size(cols_u32.saturating_mul(cw), rows_u32.saturating_mul(ch), cw, ch);
            let px_w = u16::try_from(cols_u32.saturating_mul(cw)).unwrap_or(u16::MAX);
            let px_h = u16::try_from(rows_u32.saturating_mul(ch)).unwrap_or(u16::MAX);
            let _ = resize_tx.send(Winsize::new(cols, rows, px_w, px_h));
            true
        }
        Err(err) => {
            eprintln!("[mugenkai] terminal resize to {cols}x{rows} failed: {err}");
            false
        }
    }
}

/// Measure one monospace cell three ways:
///
/// - integer pixels (for the mouse encoder + libghostty-vt resize,
///   both of which want u32 pixels)
/// - pango units (1/1024 of a pixel) for the per-cell width, used
///   by the resize watcher to compute `cols` with sub-pixel
///   precision so the grid never overflows the viewport
///
/// We lay out a probe of N 'M' characters in the label's actual
/// pango context (so we pick up whatever monospace font the user's
/// system applied via the `.monospace` CSS class) and divide the
/// measured probe size by N. Pango units come from
/// `pango::Layout::size`; pixel values come from `pixel_size`.
///
/// Returns `(cell_width_px, cell_height_px, cell_width_units)` —
/// pixels are clamped to ≥1 so subsequent divisions can't trip.
fn measure_monospace_cell(label: &gtk4::Label, font_desc_str: &str) -> (u32, u32, i32) {
    const PROBE_LEN: i32 = 200;

    let ctx = label.pango_context();
    let font_desc = pango::FontDescription::from_string(font_desc_str);

    let layout = pango::Layout::new(&ctx);
    layout.set_font_description(Some(&font_desc));
    layout.set_text(&"M".repeat(PROBE_LEN as usize));
    let (probe_width_px, line_height_px) = layout.pixel_size();
    let (probe_width_units, _) = layout.size();

    // Per-cell width in pango units, computed by dividing the
    // probe's exact unit width by the probe length. This is the
    // value the resize watcher uses for sub-pixel cols arithmetic.
    let cell_width_units = (probe_width_units / PROBE_LEN).max(1);

    // Per-cell width in integer pixels — round to nearest. The
    // mouse encoder + libghostty-vt only need this for screen-size
    // bookkeeping; small rounding is fine.
    let width_px = ((probe_width_px + PROBE_LEN / 2) / PROBE_LEN).max(1);
    let height_px = line_height_px.max(1);

    #[allow(clippy::cast_sign_loss)]
    {
        (width_px as u32, height_px as u32, cell_width_units)
    }
}

fn run_worker(
    pty_bytes_tx: Sender<Vec<u8>>,
    shutdown_rx: Receiver<()>,
    write_rx: tokio_mpsc::UnboundedReceiver<Vec<u8>>,
    resize_rx: tokio_mpsc::UnboundedReceiver<Winsize>,
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
        if let Err(err) = worker_main(&pty_bytes_tx, shutdown_rx, write_rx, resize_rx).await {
            eprintln!("[mugenkai] pane worker exited with error: {err}");
        }
    });
}

async fn worker_main(
    pty_bytes_tx: &Sender<Vec<u8>>,
    shutdown_rx: Receiver<()>,
    mut write_rx: tokio_mpsc::UnboundedReceiver<Vec<u8>>,
    mut resize_rx: tokio_mpsc::UnboundedReceiver<Winsize>,
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

            resize_req = resize_rx.recv() => {
                match resize_req {
                    Some(ws) => {
                        if let Err(err) = child.resize(ws) {
                            eprintln!("[mugenkai] pty resize failed: {err}");
                        }
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

#[cfg(test)]
mod tests {
    use super::markup_row_preview;

    #[test]
    fn markup_row_preview_counts_visible_chars() {
        let markup = "<span foreground=\"#f00\">hello</span>\n<span>world</span>";
        let (count, preview) = markup_row_preview(markup, 0);
        assert_eq!(count, 5);
        assert_eq!(preview, "hello");
    }

    #[test]
    fn markup_row_preview_second_row() {
        let markup = "one\ntwo\nthree";
        let (count, preview) = markup_row_preview(markup, 1);
        assert_eq!(count, 3);
        assert_eq!(preview, "two");
    }

    #[test]
    fn markup_row_preview_skips_tag_content() {
        let markup = "<span foreground=\"#abc\">ab</span>cd";
        let (count, _) = markup_row_preview(markup, 0);
        assert_eq!(count, 4);
    }

    #[test]
    fn markup_row_preview_counts_entity_as_one_char() {
        let markup = "a&lt;b";
        let (count, _) = markup_row_preview(markup, 0);
        assert_eq!(count, 3);
    }

    #[test]
    fn markup_row_preview_row_past_end_is_empty() {
        let markup = "one\ntwo";
        let (count, preview) = markup_row_preview(markup, 50);
        assert_eq!(count, 0);
        assert_eq!(preview, "");
    }

    #[test]
    fn markup_row_preview_caps_preview_at_80_chars() {
        let long_row: String = "x".repeat(200);
        let (count, preview) = markup_row_preview(&long_row, 0);
        assert_eq!(count, 200, "count reflects full row length");
        assert!(
            preview.len() <= 80,
            "preview stays bounded so log lines don't explode"
        );
    }
}
