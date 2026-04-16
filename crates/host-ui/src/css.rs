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
use std::collections::HashMap;
use std::sync::OnceLock;

use gtk4::CssProvider;
use gtk4::gdk;
use host_core::WarpTheme;

use crate::warp_theme_css::{generate_warp_theme_css, warp_theme_swatches};

thread_local! {
    static BASE_INSTALLED: RefCell<bool> = const { RefCell::new(false) };
    static THEME_PROVIDER: RefCell<Option<CssProvider>> = const { RefCell::new(None) };
}

/// Slugs of the built-in themes. Used to (a) seed the picker
/// before the Warp registry is populated, and (b) decide
/// whether a theme name belongs to the hardcoded path or needs
/// a registry lookup.
const BUILTIN_THEME_SLUGS: &[&str] = &["mugenkai-dark", "light", "solarized-dark", "dracula", "gruvbox"];

/// Registry of Warp YAML themes discovered at startup. Populated
/// once via [`register_warp_themes`] before the first window is
/// shown; afterwards every `theme_*` lookup function consults
/// this map before falling back to the built-in match.
///
/// `OnceLock` rather than `RwLock` because we don't currently
/// support live theme reload — adding it later would mean
/// swapping this for `RwLock<HashMap<…>>`. Until then the
/// thread-safe one-shot semantics are exactly what we want.
static WARP_THEME_REGISTRY: OnceLock<HashMap<String, WarpTheme>> = OnceLock::new();

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

/// Populate the Warp theme registry with parsed YAML themes.
///
/// Must be called once at startup, before the first call to
/// any `theme_*` lookup function. Subsequent calls are silently
/// ignored — `OnceLock` enforces single initialization. If the
/// caller wants live reload it should refactor this to a
/// `RwLock<HashMap<…>>`.
///
/// Themes that share a slug with a built-in are kept; built-ins
/// still win at lookup time. This is intentional — we don't
/// want a buggy `mugenkai-dark.yaml` in `~/.warp/themes/` to
/// silently shadow the well-tested fallback path.
#[allow(dead_code, reason = "wired in by startup-scan commit")]
pub fn register_warp_themes(themes: Vec<WarpTheme>) {
    let map: HashMap<String, WarpTheme> =
        themes.into_iter().map(|t| (t.slug(), t)).collect();
    let _ = WARP_THEME_REGISTRY.set(map);
}

/// Return the list of available theme slugs for UI pickers.
/// Built-ins come first in their canonical order, then any
/// Warp-discovered themes sorted by display name so the picker
/// looks stable across launches.
#[must_use]
pub fn available_themes() -> Vec<String> {
    let mut out: Vec<String> = BUILTIN_THEME_SLUGS.iter().map(|s| (*s).to_string()).collect();
    if let Some(registry) = WARP_THEME_REGISTRY.get() {
        let mut warp: Vec<&WarpTheme> = registry
            .iter()
            .filter(|(slug, _)| !BUILTIN_THEME_SLUGS.contains(&slug.as_str()))
            .map(|(_, theme)| theme)
            .collect();
        warp.sort_by(|a, b| a.display_name().cmp(b.display_name()));
        out.extend(warp.into_iter().map(WarpTheme::slug));
    }
    out
}

/// Hex color to use as the outer pango foreground wrapper on
/// terminal panes. ANSI-colored runs (from shell `\e[` codes)
/// override this; uncolored runs inherit it.
///
/// Kept colocated with the theme CSS so changes stay in sync.
/// For Warp-discovered themes we use the YAML's `foreground`
/// directly so the prompt color matches the rest of the
/// terminal text.
#[must_use]
pub fn theme_terminal_fg(name: &str) -> String {
    match name {
        "light" => "#0f172a".to_string(),
        "solarized-dark" => "#fdf6e3".to_string(),
        "dracula" => "#f8f8f2".to_string(),
        "gruvbox" => "#ebdbb2".to_string(),
        // mugenkai-dark (default): phosphor green for path/user
        "mugenkai-dark" => "#4aff00".to_string(),
        slug => warp_theme_for(slug)
            .map_or_else(|| "#4aff00".to_string(), |t| t.foreground.clone()),
    }
}

/// Four-color swatch used by the theme picker to preview each
/// theme. Order: accent, bg-primary, text-primary, bg-tertiary.
#[must_use]
pub fn theme_swatches(name: &str) -> [String; 4] {
    match name {
        "light" => [
            "#0284c7".into(),
            "#f8fafc".into(),
            "#0f172a".into(),
            "#f1f5f9".into(),
        ],
        "solarized-dark" => [
            "#2aa198".into(),
            "#002b36".into(),
            "#fdf6e3".into(),
            "#073642".into(),
        ],
        "dracula" => [
            "#bd93f9".into(),
            "#282a36".into(),
            "#f8f8f2".into(),
            "#343746".into(),
        ],
        "gruvbox" => [
            "#d79921".into(),
            "#1d2021".into(),
            "#ebdbb2".into(),
            "#32302f".into(),
        ],
        "mugenkai-dark" => [
            "#4aff00".into(),
            "#000000".into(),
            "#e6e8ee".into(),
            "#151515".into(),
        ],
        slug => warp_theme_for(slug).map_or_else(
            || {
                [
                    "#4aff00".into(),
                    "#000000".into(),
                    "#e6e8ee".into(),
                    "#151515".into(),
                ]
            },
            warp_theme_swatches,
        ),
    }
}

/// Human-readable label for a theme preset. Falls back to
/// title-cased slug if a Warp theme has no `name:` field and
/// no filename stem (which the loader has already tried).
#[must_use]
pub fn theme_display_name(name: &str) -> String {
    match name {
        "light" => "Light".to_string(),
        "solarized-dark" => "Solarized Dark".to_string(),
        "dracula" => "Dracula".to_string(),
        "gruvbox" => "Gruvbox".to_string(),
        "mugenkai-dark" => "Mugenkai Dark".to_string(),
        slug => warp_theme_for(slug)
            .map_or_else(|| slug.to_string(), |t| t.display_name().to_string()),
    }
}

fn warp_theme_for(slug: &str) -> Option<&'static WarpTheme> {
    WARP_THEME_REGISTRY.get()?.get(slug)
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
        "mugenkai-dark" => THEME_MUGENKAI_DARK.to_string(),
        slug => warp_theme_for(slug)
            .map_or_else(|| THEME_MUGENKAI_DARK.to_string(), generate_warp_theme_css),
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
/* Scoped under .mg-settings so they only affect the      */
/* Settings tab — not terminal panes, chat, or sidebar.   */

.mg-settings {
    background-color: var(--mg-bg-primary);
    padding: 12px 24px;
}

.mg-settings preferencesgroup {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-xl);
    padding: 8px 8px 12px 8px;
    margin: 20px 0;
}

/* Section heading: accent bar + clear gap to first row below */
.mg-settings preferencesgroup > label {
    color: var(--mg-text-primary);
    font-weight: 700;
    font-size: 14px;
    letter-spacing: 0.3px;
    padding: 14px 16px 0 16px;
    border-left: 3px solid var(--mg-accent);
    margin: 4px 0 16px 8px;
}

/* Rows: more vertical padding so controls don't crowd labels. */
.mg-settings row {
    background-color: transparent;
    color: var(--mg-text-primary);
    border-bottom: 1px solid var(--mg-border-subtle);
    border-radius: var(--mg-radius-md);
    padding: 10px 14px;
    margin: 2px 4px;
}
.mg-settings row:last-child {
    border-bottom: none;
}
.mg-settings row:hover {
    background-color: var(--mg-bg-hover);
}
/* Stacked field widgets (from entry_row helper) — big label
   on top, dark input on its own line below. */
.mg-field-label {
    color: var(--mg-text-primary);
    font-weight: 600;
    font-size: 14px;
    letter-spacing: 0.1px;
}
.mg-field-input {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    caret-color: var(--mg-accent);
    font-family: monospace;
    padding: 6px 10px;
    min-height: 34px;
}
.mg-field-input:focus {
    border-color: var(--mg-accent);
}

/* Spin/Combo/Switch rows keep inline layout; restyle labels. */
.mg-settings row .title {
    color: var(--mg-text-primary);
    font-weight: 600;
    font-size: 14px;
}
.mg-settings row .subtitle {
    color: var(--mg-text-tertiary);
    font-size: 11px;
}

/* Theme picker list (Settings) */
.mg-theme-list {
    background-color: transparent;
}
.mg-theme-row {
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: var(--mg-radius-md);
    margin: 4px 0;
    padding: 0;
}
.mg-theme-row:hover {
    background-color: var(--mg-bg-hover);
}
.mg-theme-row-selected {
    border-color: var(--mg-accent);
    background-color: var(--mg-bg-hover);
}

.mg-settings spinbutton {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
    min-height: 32px;
}
.mg-settings spinbutton > button {
    background-color: transparent;
    color: var(--mg-text-secondary);
    border: none;
}
.mg-settings spinbutton > button:hover {
    background-color: var(--mg-bg-hover);
}

.mg-settings switch {
    background-color: var(--mg-bg-tertiary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-full);
}
.mg-settings switch:checked {
    background-color: var(--mg-accent);
    border-color: var(--mg-accent);
}

.mg-settings row dropdown {
    background-color: var(--mg-bg-tertiary);
    color: var(--mg-text-primary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-md);
}

/* ── global overrides (minimal, app-wide) ─────────────── */

scrollbar slider {
    background-color: var(--mg-border-primary);
    border-radius: var(--mg-radius-full);
    min-width: 6px;
    min-height: 6px;
}
scrollbar slider:hover {
    background-color: var(--mg-text-quaternary);
}

popover contents {
    background-color: var(--mg-bg-secondary);
    border: 1px solid var(--mg-border-primary);
    border-radius: var(--mg-radius-lg);
}
popover row:hover {
    background-color: var(--mg-bg-hover);
}

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
    /* backgrounds — pure OLED black with subtle card elevation */
    --mg-bg-primary:    #000000;
    --mg-bg-secondary:  #0a0a0a;
    --mg-bg-tertiary:   #151515;
    --mg-bg-hover:      #1e1e1e;

    /* text — 4-level hierarchy (chrome stays readable white) */
    --mg-text-primary:    #e6e8ee;
    --mg-text-secondary:  #9ba1b0;
    --mg-text-tertiary:   #6b7280;
    --mg-text-quaternary: #4b5563;
    --mg-text-on-accent:  #000000;

    /* accent — phosphor green (user-requested for path/user) */
    --mg-accent:        #4aff00;
    --mg-accent-hover:  #66ff33;

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
