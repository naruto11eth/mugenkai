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

use std::rc::Rc;

use adw::prelude::*;

use host_core::{CursorStyle, KeyBindings, MugenkaiConfig};

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
    quit: adw::EntryRow,
    split_horizontal: adw::EntryRow,
    split_vertical: adw::EntryRow,
    close_pane: adw::EntryRow,
    focus_next: adw::EntryRow,
    focus_prev: adw::EntryRow,
    new_tab: adw::EntryRow,
    close_tab: adw::EntryRow,
    next_tab: adw::EntryRow,
    prev_tab: adw::EntryRow,
    open_settings: adw::EntryRow,

    // ─── cursor ──────────────────────────────────────────────
    cursor_style: adw::ComboRow,
    cursor_blink: adw::SwitchRow,
    cursor_blink_interval: adw::SpinRow,
    cursor_color: adw::EntryRow,
    cursor_text_color: adw::EntryRow,

    // ─── panes ───────────────────────────────────────────────
    focus_ring: adw::SwitchRow,
    focus_ring_color: adw::EntryRow,
    split_ratio: adw::SpinRow,
}

impl SettingsView {
    /// Build a settings view from a snapshot of the current config.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new(config: &MugenkaiConfig) -> Rc<Self> {
        let page = adw::PreferencesPage::new();
        page.set_title("Settings");

        // ─── keybindings group ────────────────────────────
        let kb_group = adw::PreferencesGroup::builder()
            .title("Keybindings")
            .description("Accelerator format: ctrl+shift+d, alt+enter, ctrl+page_down")
            .build();

        let quit = entry_row("Quit", &config.keybindings.quit);
        let split_horizontal = entry_row("Split horizontal", &config.keybindings.split_horizontal);
        let split_vertical = entry_row("Split vertical", &config.keybindings.split_vertical);
        let close_pane = entry_row("Close focused pane", &config.keybindings.close_pane);
        let focus_next = entry_row("Focus next pane", &config.keybindings.focus_next);
        let focus_prev = entry_row("Focus previous pane", &config.keybindings.focus_prev);
        let new_tab = entry_row("New tab", &config.keybindings.new_tab);
        let close_tab = entry_row("Close tab", &config.keybindings.close_tab);
        let next_tab = entry_row("Next tab", &config.keybindings.next_tab);
        let prev_tab = entry_row("Previous tab", &config.keybindings.prev_tab);
        let open_settings = entry_row("Open settings", &config.keybindings.open_settings);

        for row in [
            &quit,
            &split_horizontal,
            &split_vertical,
            &close_pane,
            &focus_next,
            &focus_prev,
            &new_tab,
            &close_tab,
            &next_tab,
            &prev_tab,
            &open_settings,
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
        let cursor_color = entry_row("Cursor color (hex)", &config.cursor.color);
        let cursor_text_color = entry_row("Cursor text color (hex)", &config.cursor.text_color);

        cursor_group.add(&cursor_style);
        cursor_group.add(&cursor_blink);
        cursor_group.add(&cursor_blink_interval);
        cursor_group.add(&cursor_color);
        cursor_group.add(&cursor_text_color);
        page.add(&cursor_group);

        // ─── pane group ───────────────────────────────────
        let pane_group = adw::PreferencesGroup::builder().title("Panes").build();

        let focus_ring = adw::SwitchRow::builder()
            .title("Focus ring")
            .subtitle("Show a colored border around the focused pane")
            .active(config.pane.focus_ring)
            .build();
        let focus_ring_color = entry_row("Focus ring color (hex)", &config.pane.focus_ring_color);
        let split_ratio = adw::SpinRow::with_range(0.1, 0.9, 0.05);
        split_ratio.set_title("Default split ratio");
        split_ratio.set_digits(2);
        split_ratio.set_value(f64::from(config.pane.split_ratio));

        pane_group.add(&focus_ring);
        pane_group.add(&focus_ring_color);
        pane_group.add(&split_ratio);
        page.add(&pane_group);

        // ─── action buttons ───────────────────────────────
        let actions = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .halign(gtk4::Align::End)
            .build();

        let save_btn = gtk4::Button::builder()
            .label("Save")
            .css_classes(["suggested-action"])
            .build();
        let reset_btn = gtk4::Button::builder()
            .label("Reset to defaults")
            .css_classes(["destructive-action"])
            .build();
        actions.append(&reset_btn);
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
            cursor_style,
            cursor_blink,
            cursor_blink_interval,
            cursor_color,
            cursor_text_color,
            focus_ring,
            focus_ring_color,
            split_ratio,
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

        self.toast("Reset to defaults. Click Save to persist.");
    }

    fn toast(&self, message: &str) {
        let toast = adw::Toast::builder().title(message).timeout(4).build();
        self.toast_overlay.add_toast(toast);
    }
}

fn entry_row(title: &str, initial_text: &str) -> adw::EntryRow {
    adw::EntryRow::builder()
        .title(title)
        .text(initial_text)
        .build()
}
