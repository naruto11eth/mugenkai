//! Shared GTK CSS provider for mugenkai-specific styling.
//!
//! Registered once per display via [`install_for_default_display`];
//! subsequent calls are no-ops. The CSS currently defines the
//! `.mugenkai-pane-focused` class used by [`crate::split_container`]
//! to highlight the focused pane with a colored border.

use std::cell::Cell;

use gtk4::CssProvider;
use gtk4::gdk;

thread_local! {
    /// Tracks whether we've already attached a provider to the
    /// default display, so repeated window constructions don't
    /// stack duplicate providers.
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Build the mugenkai CSS for a given focus-ring color and
/// attach it to the default `gdk::Display` if not already
/// installed.
///
/// The CSS selector `.mugenkai-pane-focused` draws a 2px border
/// using `focus_color`, plus a 2px margin on the other side so
/// widget bounds don't shift when the class toggles on/off.
///
/// `focus_color` must be a valid CSS color string (e.g.
/// `#4c71f2`). Invalid values cause GTK to emit a parse warning
/// but the window still works.
pub fn install_for_default_display(focus_color: &str) {
    if INSTALLED.with(Cell::get) {
        return;
    }
    let Some(display) = gdk::Display::default() else {
        return;
    };

    // 4px ring + 12px inner padding + 6px outer margin. The
    // unfocused variant uses a transparent border of the SAME
    // thickness plus the same padding/margin so widget bounds
    // never shift when the class toggles. Padding is the gap
    // between the shell text and the border; margin is the gap
    // between adjacent split panes.
    let css = format!(
        "
        .mugenkai-pane-focused {{
            border: 4px solid {focus_color};
            border-radius: 8px;
            padding: 12px;
            margin: 6px;
        }}
        .mugenkai-pane-unfocused {{
            border: 4px solid transparent;
            border-radius: 8px;
            padding: 12px;
            margin: 6px;
        }}
        "
    );

    let provider = CssProvider::new();
    provider.load_from_string(&css);
    gtk4::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    INSTALLED.with(|installed| installed.set(true));
}
