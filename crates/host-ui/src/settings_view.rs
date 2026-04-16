//! In-terminal settings tab.
//!
//! Builds an `adw::PreferencesPage` with editable rows for every
//! knob in [`MugenkaiConfig`] and a footer with Save / Reset
//! buttons. Opens as a regular `AdwTabPage` so users get the
//! configure-from-inside-the-app experience without a separate
//! dialog.
//!
//! # Lifecycle
//!
//! - [`SettingsView::new`] builds the widget tree from a snapshot
//!   of the current config. Rows are populated but not wired to
//!   the config store yet.
//! - [`SettingsView::collect_config`] walks the rows and returns
//!   a fresh `MugenkaiConfig`. Called by the Save button.
//! - Save writes via
//!   [`MugenkaiConfig::write_to_default_path`] and shows a toast
//!   explaining that some changes (keybindings) need a restart.
//!
//! # Non-goals for v0.1
//!
//! - Live reload of keybindings. GTK accelerator maps are set
//!   once per application; changing them requires re-running
//!   `set_accels_for_action`, which we don't yet have a handle
//!   to from the Settings tab. A later commit can wire a
//!   `hot_reload` callback through `MugenkaiApplication`.
//! - Per-field validation UI. Invalid values (e.g. malformed
//!   hex colors) just fall through to defaults on next launch.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use host_core::{CursorStyle, FontConfig, KeyBindings, MugenkaiConfig};

/// The settings tab widget.
///
/// Holds references to every input row so [`collect_config`](Self::collect_config)
/// can read their current values on Save.
pub struct SettingsView {
    root: gtk4::Box,
    rows: SettingsRows,
    toast_overlay: adw::ToastOverlay,
}

struct SettingsRows {
    // ─── keybindings ─────────────────────────────────────────
    quit: gtk4::Entry,
    split_horizontal: gtk4::Entry,
    split_vertical: gtk4::Entry,
    close_pane: gtk4::Entry,
    focus_next: gtk4::Entry,
    focus_prev: gtk4::Entry,
    new_tab: gtk4::Entry,
    close_tab: gtk4::Entry,
    next_tab: gtk4::Entry,
    prev_tab: gtk4::Entry,
    open_settings: gtk4::Entry,
    copy: gtk4::Entry,
    paste: gtk4::Entry,
    open_chat: gtk4::Entry,
    explain_in_chat: gtk4::Entry,
    inline_chat: gtk4::Entry,

    // ─── cursor ──────────────────────────────────────────────
    cursor_style: adw::ComboRow,
    cursor_blink: adw::SwitchRow,
    cursor_blink_interval: adw::SpinRow,
    cursor_color: gtk4::Entry,
    cursor_text_color: gtk4::Entry,

    // ─── panes ───────────────────────────────────────────────
    focus_ring: adw::SwitchRow,
    focus_ring_color: gtk4::Entry,
    split_ratio: adw::SpinRow,

    // ─── font ────────────────────────────────────────────────
    font_family: gtk4::Entry,
    font_size_pt: adw::SpinRow,

    // ─── theme ───────────────────────────────────────────────
    /// Currently-selected theme name; updated by click handlers
    /// on the theme swatch rows. Read by `collect_config` on Save.
    theme: Rc<RefCell<String>>,
}

impl SettingsView {
    /// Build a settings view from a snapshot of the current config.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new(config: &MugenkaiConfig) -> Rc<Self> {
        let page = adw::PreferencesPage::new();
        page.set_title("Settings");
        page.add_css_class("mg-settings");

        // ─── keybindings group ────────────────────────────
        let kb_group = adw::PreferencesGroup::builder()
            .title("Keybindings")
            .description("Accelerator format: ctrl+shift+d, alt+enter, ctrl+page_down")
            .build();

        let (quit_row, quit) = entry_row("Quit", &config.keybindings.quit);
        let (split_horizontal_row, split_horizontal) =
            entry_row("Split horizontal", &config.keybindings.split_horizontal);
        let (split_vertical_row, split_vertical) =
            entry_row("Split vertical", &config.keybindings.split_vertical);
        let (close_pane_row, close_pane) =
            entry_row("Close focused pane", &config.keybindings.close_pane);
        let (focus_next_row, focus_next) =
            entry_row("Focus next pane", &config.keybindings.focus_next);
        let (focus_prev_row, focus_prev) =
            entry_row("Focus previous pane", &config.keybindings.focus_prev);
        let (new_tab_row, new_tab) = entry_row("New tab", &config.keybindings.new_tab);
        let (close_tab_row, close_tab) = entry_row("Close tab", &config.keybindings.close_tab);
        let (next_tab_row, next_tab) = entry_row("Next tab", &config.keybindings.next_tab);
        let (prev_tab_row, prev_tab) = entry_row("Previous tab", &config.keybindings.prev_tab);
        let (open_settings_row, open_settings) =
            entry_row("Open settings", &config.keybindings.open_settings);
        let (copy_row, copy) = entry_row("Copy", &config.keybindings.copy);
        let (paste_row, paste) = entry_row("Paste", &config.keybindings.paste);
        let (open_chat_row, open_chat) = entry_row("Open chat", &config.keybindings.open_chat);
        let (explain_in_chat_row, explain_in_chat) =
            entry_row("Explain in chat", &config.keybindings.explain_in_chat);
        let (inline_chat_row, inline_chat) =
            entry_row("Inline chat toggle", &config.keybindings.inline_chat);

        for row in [
            &quit_row,
            &split_horizontal_row,
            &split_vertical_row,
            &close_pane_row,
            &focus_next_row,
            &focus_prev_row,
            &new_tab_row,
            &close_tab_row,
            &next_tab_row,
            &prev_tab_row,
            &open_settings_row,
            &copy_row,
            &paste_row,
            &open_chat_row,
            &explain_in_chat_row,
            &inline_chat_row,
        ] {
            kb_group.add(row);
        }
        page.add(&kb_group);

        // ─── cursor group ─────────────────────────────────
        let cursor_group = adw::PreferencesGroup::builder().title("Cursor").build();

        let cursor_style = {
            let row = adw::ComboRow::builder().title("Style").build();
            let model = gtk4::StringList::new(&["block", "underline"]);
            row.set_model(Some(&model));
            row.set_selected(match config.cursor.style {
                CursorStyle::Block => 0,
                CursorStyle::Underline => 1,
            });
            row
        };
        let cursor_blink = adw::SwitchRow::builder()
            .title("Blinking")
            .active(config.cursor.blink)
            .build();
        let cursor_blink_interval = adw::SpinRow::with_range(60.0, 2000.0, 10.0);
        cursor_blink_interval.set_title("Blink interval (ms)");
        #[allow(clippy::cast_precision_loss)]
        cursor_blink_interval.set_value(config.cursor.blink_interval_ms as f64);
        let (cursor_color_row, cursor_color) =
            entry_row("Cursor color (hex)", &config.cursor.color);
        let (cursor_text_color_row, cursor_text_color) =
            entry_row("Cursor text color (hex)", &config.cursor.text_color);

        cursor_group.add(&cursor_style);
        cursor_group.add(&cursor_blink);
        cursor_group.add(&cursor_blink_interval);
        cursor_group.add(&cursor_color_row);
        cursor_group.add(&cursor_text_color_row);
        page.add(&cursor_group);

        // ─── pane group ───────────────────────────────────
        let pane_group = adw::PreferencesGroup::builder().title("Panes").build();

        let focus_ring = adw::SwitchRow::builder()
            .title("Focus ring")
            .subtitle("Show a colored border around the focused pane")
            .active(config.pane.focus_ring)
            .build();
        let (focus_ring_color_row, focus_ring_color) =
            entry_row("Focus ring color (hex)", &config.pane.focus_ring_color);
        let split_ratio = adw::SpinRow::with_range(0.1, 0.9, 0.05);
        split_ratio.set_title("Default split ratio");
        split_ratio.set_digits(2);
        split_ratio.set_value(f64::from(config.pane.split_ratio));

        pane_group.add(&focus_ring);
        pane_group.add(&focus_ring_color_row);
        pane_group.add(&split_ratio);
        page.add(&pane_group);

        // ─── font group ───────────────────────────────────
        let font_group = adw::PreferencesGroup::builder().title("Font").build();

        let (font_family_row, font_family) = entry_row(
            "Family (pango, e.g. 'monospace' or 'JetBrains Mono')",
            &config.font.family,
        );
        let font_size_pt = adw::SpinRow::with_range(6.0, 48.0, 0.5);
        font_size_pt.set_title("Size (pt)");
        font_size_pt.set_digits(1);
        font_size_pt.set_value(f64::from(config.font.size_pt));

        font_group.add(&font_family_row);
        font_group.add(&font_size_pt);
        page.add(&font_group);

        // ─── theme group — swatch picker ──────────────────
        let theme_group = adw::PreferencesGroup::builder().title("Theme").build();
        let theme_state: Rc<RefCell<String>> = Rc::new(RefCell::new(config.theme.clone()));
        let theme_list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::None)
            .css_classes(["mg-theme-list"])
            .build();

        let rows_by_idx: Rc<RefCell<Vec<gtk4::ListBoxRow>>> = Rc::new(RefCell::new(Vec::new()));

        for name in crate::css::available_themes() {
            let row = build_theme_row(&name, name == config.theme);
            {
                let theme_state = Rc::clone(&theme_state);
                let rows_by_idx = Rc::clone(&rows_by_idx);
                let name_owned = name.clone();
                let gesture = gtk4::GestureClick::new();
                gesture.connect_released(move |_, _, _, _| {
                    (*theme_state.borrow_mut()).clone_from(&name_owned);
                    crate::css::switch_theme(&name_owned);
                    // Repaint selection highlight on all rows.
                    for r in rows_by_idx.borrow().iter() {
                        let is_selected = r
                            .widget_name()
                            .as_str()
                            .eq_ignore_ascii_case(&name_owned);
                        if is_selected {
                            r.add_css_class("mg-theme-row-selected");
                        } else {
                            r.remove_css_class("mg-theme-row-selected");
                        }
                    }
                });
                row.add_controller(gesture);
            }
            row.set_widget_name(&name);
            rows_by_idx.borrow_mut().push(row.clone());
            theme_list.append(&row);
        }

        theme_group.add(&theme_list);
        page.add(&theme_group);
        let theme = theme_state;

        // ─── action buttons ───────────────────────────────
        let actions = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_top(20)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .halign(gtk4::Align::Fill)
            .build();

        let reset_btn = gtk4::Button::builder()
            .label("Reset to defaults")
            .css_classes(["mg-btn-ghost"])
            .build();
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let save_btn = gtk4::Button::builder()
            .label("Save changes")
            .css_classes(["mg-btn-primary"])
            .build();
        save_btn.set_size_request(140, 36);
        actions.append(&reset_btn);
        actions.append(&spacer);
        actions.append(&save_btn);

        // ─── layout root ──────────────────────────────────
        // Wrap the preferences page in a scrolled window so the
        // content is scrollable, then put it and the action bar
        // in a vertical Box, then wrap that in a toast overlay
        // so the save notification has somewhere to appear.
        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&page)
            .build();

        let inner_box = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        inner_box.append(&scroller);
        inner_box.append(&actions);

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&inner_box));

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&toast_overlay);

        let rows = SettingsRows {
            quit,
            split_horizontal,
            split_vertical,
            close_pane,
            focus_next,
            focus_prev,
            new_tab,
            close_tab,
            next_tab,
            prev_tab,
            open_settings,
            copy,
            paste,
            open_chat,
            explain_in_chat,
            inline_chat,
            cursor_style,
            cursor_blink,
            cursor_blink_interval,
            cursor_color,
            cursor_text_color,
            focus_ring,
            focus_ring_color,
            split_ratio,
            font_family,
            font_size_pt,
            theme,
        };

        let view = Rc::new(Self {
            root,
            rows,
            toast_overlay,
        });

        // Wire the action buttons now that we have an Rc handle
        // to the view — the closures capture `view` so they can
        // call collect_config + save + show_toast.
        {
            let view_clone = Rc::clone(&view);
            save_btn.connect_clicked(move |_| {
                view_clone.save();
            });
        }
        {
            let view_clone = Rc::clone(&view);
            reset_btn.connect_clicked(move |_| {
                view_clone.reset_to_defaults();
            });
        }

        view
    }

    /// Return the underlying widget for insertion into a
    /// `gtk4::Container` (or an `AdwTabView` page).
    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Read current row values into a fresh [`MugenkaiConfig`].
    #[must_use]
    #[allow(clippy::field_reassign_with_default)]
    pub fn collect_config(&self) -> MugenkaiConfig {
        let rows = &self.rows;
        let mut cfg = MugenkaiConfig::default();

        cfg.keybindings = KeyBindings {
            quit: rows.quit.text().to_string(),
            split_horizontal: rows.split_horizontal.text().to_string(),
            split_vertical: rows.split_vertical.text().to_string(),
            close_pane: rows.close_pane.text().to_string(),
            focus_next: rows.focus_next.text().to_string(),
            focus_prev: rows.focus_prev.text().to_string(),
            new_tab: rows.new_tab.text().to_string(),
            close_tab: rows.close_tab.text().to_string(),
            next_tab: rows.next_tab.text().to_string(),
            prev_tab: rows.prev_tab.text().to_string(),
            open_settings: rows.open_settings.text().to_string(),
            copy: rows.copy.text().to_string(),
            paste: rows.paste.text().to_string(),
            open_chat: rows.open_chat.text().to_string(),
            explain_in_chat: rows.explain_in_chat.text().to_string(),
            inline_chat: rows.inline_chat.text().to_string(),
        };

        cfg.cursor.style = match rows.cursor_style.selected() {
            1 => CursorStyle::Underline,
            _ => CursorStyle::Block,
        };
        cfg.cursor.blink = rows.cursor_blink.is_active();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_lossless
        )]
        {
            cfg.cursor.blink_interval_ms = rows.cursor_blink_interval.value() as u64;
        }
        cfg.cursor.color = rows.cursor_color.text().to_string();
        cfg.cursor.text_color = rows.cursor_text_color.text().to_string();

        cfg.pane.focus_ring = rows.focus_ring.is_active();
        cfg.pane.focus_ring_color = rows.focus_ring_color.text().to_string();
        #[allow(clippy::cast_possible_truncation)]
        {
            cfg.pane.split_ratio = rows.split_ratio.value() as f32;
        }

        cfg.font = FontConfig {
            family: rows.font_family.text().to_string(),
            #[allow(clippy::cast_possible_truncation)]
            size_pt: rows.font_size_pt.value() as f32,
        };

        cfg.theme.clone_from(&rows.theme.borrow());

        cfg
    }

    fn save(&self) {
        let cfg = self.collect_config();
        match cfg.write_to_default_path() {
            Ok(path) => {
                self.toast(&format!(
                    "Saved to {}. Keybindings + cursor take effect on next launch.",
                    path.display()
                ));
            }
            Err(err) => {
                self.toast(&format!("Save failed: {err}"));
            }
        }
    }

    fn reset_to_defaults(&self) {
        let defaults = MugenkaiConfig::default();
        let rows = &self.rows;

        rows.quit.set_text(&defaults.keybindings.quit);
        rows.split_horizontal
            .set_text(&defaults.keybindings.split_horizontal);
        rows.split_vertical
            .set_text(&defaults.keybindings.split_vertical);
        rows.close_pane.set_text(&defaults.keybindings.close_pane);
        rows.focus_next.set_text(&defaults.keybindings.focus_next);
        rows.focus_prev.set_text(&defaults.keybindings.focus_prev);
        rows.new_tab.set_text(&defaults.keybindings.new_tab);
        rows.close_tab.set_text(&defaults.keybindings.close_tab);
        rows.next_tab.set_text(&defaults.keybindings.next_tab);
        rows.prev_tab.set_text(&defaults.keybindings.prev_tab);
        rows.open_settings
            .set_text(&defaults.keybindings.open_settings);
        rows.copy.set_text(&defaults.keybindings.copy);
        rows.paste.set_text(&defaults.keybindings.paste);
        rows.open_chat.set_text(&defaults.keybindings.open_chat);
        rows.explain_in_chat
            .set_text(&defaults.keybindings.explain_in_chat);
        rows.inline_chat
            .set_text(&defaults.keybindings.inline_chat);

        rows.cursor_style.set_selected(match defaults.cursor.style {
            CursorStyle::Block => 0,
            CursorStyle::Underline => 1,
        });
        rows.cursor_blink.set_active(defaults.cursor.blink);
        #[allow(clippy::cast_precision_loss)]
        rows.cursor_blink_interval
            .set_value(defaults.cursor.blink_interval_ms as f64);
        rows.cursor_color.set_text(&defaults.cursor.color);
        rows.cursor_text_color
            .set_text(&defaults.cursor.text_color);

        rows.focus_ring.set_active(defaults.pane.focus_ring);
        rows.focus_ring_color
            .set_text(&defaults.pane.focus_ring_color);
        rows.split_ratio
            .set_value(f64::from(defaults.pane.split_ratio));

        rows.font_family.set_text(&defaults.font.family);
        rows.font_size_pt
            .set_value(f64::from(defaults.font.size_pt));

        (*rows.theme.borrow_mut()).clone_from(&defaults.theme);
        crate::css::switch_theme(&defaults.theme);

        self.toast("Reset to defaults. Click Save to persist.");
    }

    fn toast(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(4).build();
        self.toast_overlay.add_toast(toast);
    }
}

/// Build a theme-picker row: theme name on the left, a row of
/// four color swatches on the right. Clicking it selects the
/// theme (wired up by the caller).
fn build_theme_row(name: &str, selected: bool) -> gtk4::ListBoxRow {
    let hbox = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(14)
        .margin_end(14)
        .build();

    let label = gtk4::Label::builder()
        .label(crate::css::theme_display_name(name))
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .css_classes(["mg-field-label"])
        .build();
    hbox.append(&label);

    let swatches = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .build();
    for color in crate::css::theme_swatches(name) {
        let dot = gtk4::DrawingArea::new();
        dot.set_size_request(20, 20);
        dot.add_css_class("mg-swatch");
        attach_inline_css(&dot, &format!("background-color: {color};"));
        swatches.append(&dot);
    }
    hbox.append(&swatches);

    let row = gtk4::ListBoxRow::builder()
        .activatable(true)
        .selectable(false)
        .css_classes(["mg-theme-row"])
        .child(&hbox)
        .build();
    if selected {
        row.add_css_class("mg-theme-row-selected");
    }
    row
}

/// Per-widget inline CSS applied via a private `CssProvider`
/// using a unique generated class name. Mirrors the helper in
/// `onboarding_view.rs`.
fn attach_inline_css(widget: &impl gtk4::prelude::IsA<gtk4::Widget>, css_body: &str) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static INLINE_ID: AtomicU64 = AtomicU64::new(0);
    let id = INLINE_ID.fetch_add(1, Ordering::Relaxed);
    let class = format!("mg-sw{id}");
    let rule = format!(".{class} {{ {css_body} }}");
    widget.add_css_class(&class);
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(&rule);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

/// Build a stacked settings field: big bold label on top, input
/// box below on its own line. Returns both the container (to add
/// to the preferences group) and the inner entry (so callers can
/// read/write its text later).
fn entry_row(title: &str, initial_text: &str) -> (gtk4::Box, gtk4::Entry) {
    let label = gtk4::Label::builder()
        .label(title)
        .halign(gtk4::Align::Start)
        .css_classes(["mg-field-label"])
        .build();

    let entry = gtk4::Entry::builder()
        .text(initial_text)
        .hexpand(true)
        .css_classes(["mg-field-input"])
        .build();

    let container = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .margin_top(10)
        .margin_bottom(10)
        .margin_start(14)
        .margin_end(14)
        .build();
    container.append(&label);
    container.append(&entry);

    (container, entry)
}
