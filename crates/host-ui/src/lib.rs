//! Mugenkai GTK4 + libadwaita host UI.
//!
//! For v0.1 this crate ships:
//! - [`MugenkaiApplication`] — a thin wrapper around `adw::Application`
//! - [`MugenkaiWindow`] — an `adw::ApplicationWindow` with a `HeaderBar`,
//!   a `TabView` placeholder, and a `ToastOverlay`
//!
//! Real per-pane terminal rendering + split trees + notification rings
//! land in Phase 2 (Units 4, 4b, 5) + Phase 3 (Unit 8).

mod app;
mod css;
mod pane_widget;
mod pango_markup;
mod settings_view;
mod split_container;
mod window;

pub use app::{APP_ID, MugenkaiApplication};
pub use pane_widget::PaneWidget;
pub use window::MugenkaiWindow;

/// Entry point called from `host-cli::main`.
///
/// Blocks until the application exits (Ctrl+Q or window close). Maps
/// GTK's internal exit status to `std::process::ExitCode` so callers
/// don't have to pull in gtk4 as a direct dependency.
#[must_use]
pub fn run() -> std::process::ExitCode {
    let app = MugenkaiApplication::new();
    let code = app.run();
    if code == gtk4::glib::ExitCode::SUCCESS {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
