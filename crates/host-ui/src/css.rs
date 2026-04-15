//! Mugenkai design system — GTK4 CSS variables + theme engine.
//!
//! Every mugenkai widget references `--mg-*` CSS variables instead
//! of hardcoded colors, radii, or spacing. Themes are just CSS
//! strings that reassign those variables; switching theme at
//! runtime = replacing the CSS provider.
//!
//! # Token categories
//!
//! - **Colors:** `--mg-bg-*`, `--mg-text-*`, `--mg-accent-*`,
//!   `--mg-border-*`, `--mg-status-*`
//! - **Effects:** `--mg-radius-*`, `--mg-shadow-*`
//! - **Spacing:** `--mg-space-*`
//!
//! # Architecture
//!
//! Two CSS providers sit on the default display:
//!
//! 1. **Base provider** (installed once, never swapped) — defines
//!    all `.mg-*` widget classes that consume the variables.
//! 2. **Theme provider** (swappable) — sets the `--mg-*` variable
//!    values. Replacing this provider is how runtime theme
//!    switching works.

use std::cell::RefCell;

use gtk4::CssProvider;
use gtk4::gdk;

thread_local! {
    static BASE_INSTALLED: RefCell<bool> = const { RefCell::new(false) };
    static THEME_PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
}

// ─── public API ──────────────────────────────────────────────

/// Install the base widget styles + the initial theme on the
/// default display. Safe to call multiple times — subsequent
/// calls are no-ops for the base provider, and the theme
/// provider is swapped cleanly.
pub fn install_design_system(theme_name: &str, focus_color: &str) {
    let Some(display) = gdk::Display::default() else {
        return;
    };

    // Base provider: widget classes that consume --mg-* vars.
    // Installed once, never replaced.
    BASE_INSTALLED.with(|installed| {
        if !*installed.borrow() {
            let provider = CssProvider::new();
            provider.load_from_string(BASE_CSS);
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            *installed.borrow_mut() = true;
        }
    });

    // Pane focus ring CSS (dynamic per config).
    install_focus_ring(&display, focus_color);

    // Theme provider: sets --mg-* variable values.
    apply_theme(&display, theme_name);
}

/// Switch the active theme at runtime. Called from the Settings
/// tab when the user picks a different preset.
pub fn switch_theme(theme_name: &str) {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    apply_theme(&display, theme_name);
}

/// Return the list of available theme names for UI pickers.
#[must_use]
pub fn available_themes() -> &'static [&'static str] {
    &["mugenkai-dark", "light", "solarized-dark", "dracula", "gruvbox"]
}

// ─── theme engine ────────────────────────────────────────────

fn apply_theme(display: &gdk::Display, theme_name: &str) {
    let css = theme_css(theme_name);
    let new_provider = CssProvider::new();
    new_provider.load_from_string(&css);

    THEME_PROVIDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        // Remove old theme provider if present.
        if let Some(old) = slot.take() {
            gtk4::style_context_remove_provider_for_display(display, &old);
        }
        gtk4::style_context_add_provider_for_display(
            display,
            &new_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        *slot = Some(new_provider);
    });
}

fn install_focus_ring(display: &gdk::Display, focus_color: &str) {
    let css = format!(
        "
        .mg-pane-focused {{
            border: 3px solid {focus_color};
            border-radius: var(--mg-radius-lg);
            padding: 8px;
            margin: 4px;
        }}
        .mg-pane-unfocused {{
            border: 3px solid transparent;
            border-radius: var(--mg-radius-lg);
            padding: 8px;
            margin: 4px;
        }}
        "
    );
    let provider = CssProvider::new();
    provider.load_from_string(&css);
    gtk4::style_context_add_provider_for_display(
        display,
        &provider,
        gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

fn theme_css(name: &str) -> String {
    match name {
        "light" => THEME_LIGHT.to_string(),
        "solarized-dark" => THEME_SOLARIZED_DARK.to_string(),
        "dracula" => THEME_DRACULA.to_string(),
        "gruvbox" => THEME_GRUVBOX.to_string(),
        _ => THEME_MUGENKAI_DARK.to_string(),
    }
}

// ─── base widget CSS ─────────────────────────────────────────
//
// Every class below references `--mg-*` variables defined by
// the active theme. Adding a new widget style = add a class
// here + apply it to the GTK widget in Rust.

const BASE_CSS: &str = "
/* ── global overrides ─────────────────────────────────── */

window, headerbar, tabbar, tabview {
    background-color: var(--mg-bg-primary);
    color: var(--mg-text-primary);
}

headerbar {
    border-bottom: 1px solid var(--mg-border-subtle);
    box-shadow: none;
    min-height: 38px;
}

tabbar {
    background-color: var(--mg-bg-secondary);
    border-bottom: 1px solid var(--mg-border-subtle);
}

/* ── cards ─────────────────────────────────────────────── */

.mg-card {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
    padding: 16px;
}

.mg-card-interactive {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
    padding: 16px;
    transition: border-color 150ms ease;
}
.mg-card-interactive:hover {
    border-color: var(--mg-accent);
}
.mg-card-selected {
    border-color: var(--mg-accent);
    border-width: 2px;
}

/* ── buttons ──────────────────────────────────────────── */

.mg-btn-primary {
    background-color: var(--mg-accent);
    color: var(--mg-text-on-accent);
    border-radius: var(--mg-radius-md);
    padding: 8px 20px;
    font-weight: 600;
    border: none;
    transition: background-color 150ms ease;
}
.mg-btn-primary:hover {
    background-color: var(--mg-accent-hover);
}

.mg-btn-secondary {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-secondary);
    border-radius: var(--mg-radius-md);
    padding: 8px 20px;
    font-weight: 500;
    border: 1px solid var(--mg-border-primary);
    transition: background-color 150ms ease;
}
.mg-btn-secondary:hover {
    background-color: var(--mg-bg-hover);
}

.mg-btn-ghost {
    background: transparent;
    color: var(--mg-text-tertiary);
    border-radius: var(--mg-radius-md);
    padding: 8px 16px;
    border: none;
}
.mg-btn-ghost:hover {
    color: var(--mg-text-secondary);
    background-color: var(--mg-bg-hover);
}

.mg-btn-pill {
    border-radius: var(--mg-radius-full);
    padding: 4px 14px;
    font-size: 12px;
}

/* ── inputs ───────────────────────────────────────────── */

.mg-input {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    padding: 8px 12px;
    caret-color: var(--mg-accent);
}
.mg-input:focus {
    border-color: var(--mg-accent);
    outline: none;
}

/* ── sidebar ──────────────────────────────────────────── */

.mg-sidebar {
    background-color: var(--mg-bg-primary);
    border-right: 1px solid var(--mg-border-subtle);
}

.mg-sidebar-item {
    padding: 8px 12px;
    border-radius: var(--mg-radius-md);
    color: var(--mg-text-secondary);
    transition: background-color 100ms ease;
}
.mg-sidebar-item:hover {
    background-color: var(--mg-bg-hover);
    color: var(--mg-text-primary);
}
.mg-sidebar-item-active {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
}

/* ── text hierarchy ───────────────────────────────────── */

.mg-heading {
    font-size: 22px;
    font-weight: 700;
    color: var(--mg-text-primary);
}

.mg-subheading {
    font-size: 14px;
    font-weight: 400;
    color: var(--mg-text-secondary);
}

.mg-caption {
    font-size: 12px;
    font-weight: 400;
    color: var(--mg-text-tertiary);
}

.mg-dim {
    color: var(--mg-text-quaternary);
}

/* ── sections ─────────────────────────────────────────── */

.mg-section {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
    padding: 20px;
    margin-bottom: 12px;
}

.mg-section-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--mg-text-tertiary);
    letter-spacing: 0.5px;
    margin-bottom: 12px;
}

/* ── entry rows (settings) ────────────────────────────── */

.mg-setting-row {
    padding: 10px 0;
    border-bottom: 1px solid var(--mg-border-subtle);
}
.mg-setting-row:last-child {
    border-bottom: none;
}

/* ── pane focus ring (now uses tokens) ────────────────── */

.mg-pane-focused,
.mg-pane-unfocused {
    transition: border-color 150ms ease;
}

/* ── color swatches (for theme picker) ────────────────── */

.mg-swatch {
    min-width: 22px;
    min-height: 22px;
    border-radius: 9999px;
    border: 2px solid var(--mg-border-primary);
}

/* ── status indicators ────────────────────────────────── */

.mg-status-dot {
    min-width: 8px;
    min-height: 8px;
    border-radius: 9999px;
}

/* ── progress dots (onboarding) ───────────────────────── */

.mg-dot {
    min-width: 8px;
    min-height: 8px;
    border-radius: 9999px;
    background-color: var(--mg-border-primary);
}
.mg-dot-active {
    background-color: var(--mg-accent);
}

/* ── chat ─────────────────────────────────────────────── */

.mg-chat-input {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
    padding: 10px 14px;
    caret-color: var(--mg-accent);
}
.mg-chat-input:focus {
    border-color: var(--mg-accent);
}

.mg-code-block {
    background-color: var(--mg-bg-tertiary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    padding: 12px;
    font-family: monospace;
}

/* ── onboarding overlay ───────────────────────────────── */

.mg-onboarding-page {
    background-color: var(--mg-bg-primary);
    padding: 48px;
}

.mg-onboarding-preview {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-subtle);
    border-radius: var(--mg-radius-xl);
}

/* ── toast overrides ──────────────────────────────────── */

toast {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border-radius: var(--mg-radius-lg);
}

/* ── adw preferences overrides ────────────────────────── */
/* Redesigned for a Warp/Pane-style dark settings panel.  */
/* Generous spacing, clear section hierarchy, controls    */
/* well-separated from labels.                            */

preferencespage {
    background-color: var(--mg-bg-primary);
    padding: 12px 24px;
}

/* ── section cards ────────────────────────────────────── */

preferencesgroup {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-xl);
    padding: 8px 4px;
    margin: 16px 0;
}

/* Section headings — bold, accent-colored left border */
preferencesgroup > label {
    color: var(--mg-text-primary);
    font-weight: 700;
    font-size: 14px;
    letter-spacing: 0.3px;
    padding: 12px 16px 8px 16px;
    border-left: 3px solid var(--mg-accent);
    margin-left: 8px;
    margin-bottom: 4px;
}

/* Section description text */
preferencesgroup > label + label {
    color: var(--mg-text-tertiary);
    font-size: 11px;
    font-weight: 400;
    padding: 0 16px 8px 28px;
    border-left: none;
    margin-left: 0;
}

/* ── setting rows ─────────────────────────────────────── */

row {
    background-color: var(--mg-bg-secondary);
    color: var(--mg-text-primary);
    border-bottom: 1px solid var(--mg-border-subtle);
    border-radius: 0;
    min-height: 56px;
    padding: 6px 16px;
    margin: 0 4px;
}
row:first-child {
    border-radius: var(--mg-radius-lg) var(--mg-radius-lg) 0 0;
}
row:last-child {
    border-bottom: none;
    border-radius: 0 0 var(--mg-radius-lg) var(--mg-radius-lg);
}
row:only-child {
    border-radius: var(--mg-radius-lg);
}
row:hover {
    background-color: var(--mg-bg-hover);
}

row > box > label.title {
    color: var(--mg-text-primary);
    font-weight: 500;
    font-size: 13px;
}
row > box > label.subtitle {
    color: var(--mg-text-tertiary);
    font-size: 11px;
    margin-top: 2px;
}

/* ── entry fields (keybindings, colors, font family) ── */

row entry, row text {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    caret-color: var(--mg-accent);
    min-height: 32px;
    min-width: 160px;
    padding: 4px 10px;
    margin: 4px 0;
}
row entry:focus, row text:focus {
    border-color: var(--mg-accent);
    box-shadow: 0 0 0 1px var(--mg-accent);
}

/* ── spin buttons ─────────────────────────────────────── */

spinbutton {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    min-height: 32px;
}
spinbutton > button {
    background-color: transparent;
    color: var(--mg-text-secondary);
    border: none;
    min-width: 28px;
}
spinbutton > button:hover {
    background-color: var(--mg-bg-hover);
    color: var(--mg-text-primary);
}

/* ── switches ─────────────────────────────────────────── */

switch {
    background-color: var(--mg-bg-tertiary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-full);
    min-width: 44px;
    min-height: 24px;
}
switch:checked {
    background-color: var(--mg-accent);
    border-color: var(--mg-accent);
}
switch slider {
    background-color: var(--mg-text-secondary);
    border-radius: var(--mg-radius-full);
    min-width: 18px;
    min-height: 18px;
}
switch:checked slider {
    background-color: var(--mg-text-on-accent);
}

/* ── combo/dropdown ───────────────────────────────────── */

row dropdown {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    min-height: 32px;
    padding: 0 8px;
}

popover contents {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
    padding: 4px;
}
popover modelbutton,
popover row {
    border-radius: var(--mg-radius-md);
    padding: 6px 12px;
    margin: 2px 4px;
}
popover modelbutton:hover,
popover row:hover {
    background-color: var(--mg-bg-hover);
}

/* ── scrollbars ───────────────────────────────────────── */

scrollbar slider {
    background-color: var(--mg-border-primary);
    border-radius: var(--mg-radius-full);
    min-width: 6px;
    min-height: 6px;
}
scrollbar slider:hover {
    background-color: var(--mg-text-quaternary);
}

/* ── action buttons ──────────────────────────────────── */

button.suggested-action {
    background-color: var(--mg-accent);
    color: var(--mg-text-on-accent);
    border: none;
    border-radius: var(--mg-radius-md);
    font-weight: 600;
}
button.suggested-action:hover {
    background-color: var(--mg-accent-hover);
}

button.destructive-action {
    background-color: var(--mg-status-error);
    color: var(--mg-text-on-accent);
    border: none;
    border-radius: var(--mg-radius-md);
}

button.flat {
    color: var(--mg-text-secondary);
}
button.flat:hover {
    color: var(--mg-text-primary);
    background-color: var(--mg-bg-hover);
}

/* ── list boxes ──────────────────────────────────────── */

listbox {
    background-color: var(--mg-bg-primary);
}
listbox > row {
    background-color: var(--mg-bg-primary);
    border-bottom: 1px solid var(--mg-border-subtle);
}
listbox > row:selected {
    background-color: var(--mg-bg-tertiary);
}

/* ── navigation sidebar ──────────────────────────────── */

listbox.navigation-sidebar {
    background-color: transparent;
}
listbox.navigation-sidebar > row {
    background-color: transparent;
    border: none;
    border-radius: var(--mg-radius-md);
    margin: 2px 6px;
    padding: 6px 10px;
}
listbox.navigation-sidebar > row:hover {
    background-color: var(--mg-bg-hover);
}
listbox.navigation-sidebar > row:selected {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
}

/* ── separator ───────────────────────────────────────── */

separator {
    background-color: var(--mg-border-subtle);
    min-height: 1px;
}

/* ── overlay split view ──────────────────────────────── */

.sidebar-pane {
    background-color: var(--mg-bg-primary);
}

/* ── boxed-list (chat message list) ──────────────────── */

listbox.boxed-list {
    background-color: var(--mg-bg-primary);
    border: none;
}
listbox.boxed-list > row {
    background-color: var(--mg-bg-primary);
    border-bottom: 1px solid var(--mg-border-subtle);
    border-radius: 0;
}
";

// ─── theme definitions ───────────────────────────────────────
//
// Each theme is a `:root { ... }` block that sets every
// `--mg-*` variable. The base CSS consumes them; swapping the
// theme provider is all it takes to retheme the entire app.

const THEME_MUGENKAI_DARK: &str = "
:root {
    /* backgrounds — OLED-adjacent dark ramp */
    --mg-bg-primary:    #0a0e14;
    --mg-bg-secondary:  #12161e;
    --mg-bg-tertiary:   #1a1f2b;
    --mg-bg-hover:      #1f2533;

    /* text — 4-level hierarchy */
    --mg-text-primary:    #e6e8ee;
    --mg-text-secondary:  #9ba1b0;
    --mg-text-tertiary:   #6b7280;
    --mg-text-quaternary: #4b5563;
    --mg-text-on-accent:  #ffffff;

    /* accent — cyan/teal like Warp */
    --mg-accent:        #00bcd4;
    --mg-accent-hover:  #00acc1;

    /* borders */
    --mg-border-primary: #1f2937;
    --mg-border-subtle:  #161b26;

    /* status */
    --mg-status-success: #22c55e;
    --mg-status-warning: #f59e0b;
    --mg-status-error:   #ef4444;

    /* radii */
    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    /* spacing */
    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}
";

const THEME_LIGHT: &str = "
:root {
    --mg-bg-primary:    #f8fafc;
    --mg-bg-secondary:  #ffffff;
    --mg-bg-tertiary:   #f1f5f9;
    --mg-bg-hover:      #e2e8f0;

    --mg-text-primary:    #0f172a;
    --mg-text-secondary:  #475569;
    --mg-text-tertiary:   #94a3b8;
    --mg-text-quaternary: #cbd5e1;
    --mg-text-on-accent:  #ffffff;

    --mg-accent:        #0284c7;
    --mg-accent-hover:  #0369a1;

    --mg-border-primary: #e2e8f0;
    --mg-border-subtle:  #f1f5f9;

    --mg-status-success: #16a34a;
    --mg-status-warning: #d97706;
    --mg-status-error:   #dc2626;

    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}
";

const THEME_SOLARIZED_DARK: &str = "
:root {
    --mg-bg-primary:    #002b36;
    --mg-bg-secondary:  #073642;
    --mg-bg-tertiary:   #0a3e4a;
    --mg-bg-hover:      #0d4a58;

    --mg-text-primary:    #fdf6e3;
    --mg-text-secondary:  #93a1a1;
    --mg-text-tertiary:   #657b83;
    --mg-text-quaternary: #586e75;
    --mg-text-on-accent:  #fdf6e3;

    --mg-accent:        #2aa198;
    --mg-accent-hover:  #268bd2;

    --mg-border-primary: #094552;
    --mg-border-subtle:  #073642;

    --mg-status-success: #859900;
    --mg-status-warning: #b58900;
    --mg-status-error:   #dc322f;

    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}
";

const THEME_DRACULA: &str = "
:root {
    --mg-bg-primary:    #282a36;
    --mg-bg-secondary:  #2e303e;
    --mg-bg-tertiary:   #343746;
    --mg-bg-hover:      #3c3f52;

    --mg-text-primary:    #f8f8f2;
    --mg-text-secondary:  #bfbfbf;
    --mg-text-tertiary:   #6272a4;
    --mg-text-quaternary: #4a5580;
    --mg-text-on-accent:  #f8f8f2;

    --mg-accent:        #bd93f9;
    --mg-accent-hover:  #9b6dff;

    --mg-border-primary: #3a3d4e;
    --mg-border-subtle:  #2e303e;

    --mg-status-success: #50fa7b;
    --mg-status-warning: #f1fa8c;
    --mg-status-error:   #ff5555;

    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}
";

const THEME_GRUVBOX: &str = "
:root {
    --mg-bg-primary:    #1d2021;
    --mg-bg-secondary:  #282828;
    --mg-bg-tertiary:   #32302f;
    --mg-bg-hover:      #3c3836;

    --mg-text-primary:    #ebdbb2;
    --mg-text-secondary:  #bdae93;
    --mg-text-tertiary:   #928374;
    --mg-text-quaternary: #665c54;
    --mg-text-on-accent:  #1d2021;

    --mg-accent:        #d79921;
    --mg-accent-hover:  #fabd2f;

    --mg-border-primary: #3c3836;
    --mg-border-subtle:  #282828;

    --mg-status-success: #b8bb26;
    --mg-status-warning: #fe8019;
    --mg-status-error:   #fb4934;

    --mg-radius-sm:   4px;
    --mg-radius-md:   6px;
    --mg-radius-lg:   8px;
    --mg-radius-xl:   12px;
    --mg-radius-full: 9999px;

    --mg-space-1: 4px;
    --mg-space-2: 8px;
    --mg-space-3: 12px;
    --mg-space-4: 16px;
    --mg-space-6: 24px;
    --mg-space-8: 32px;
}
";
