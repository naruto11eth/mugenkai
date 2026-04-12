use adw::prelude::*;
use gtk4::{gio, glib};

use crate::window::MugenkaiWindow;

/// Reverse-DNS app id used for GTK's `application_id` and the desktop
/// entry's `StartupWMClass`.
pub const APP_ID: &str = "com.mugenkai.Mugenkai";

/// Thin wrapper around `adw::Application`.
///
/// Owns the application lifetime and wires the activation callback to
/// construct a `MugenkaiWindow` on startup. In Phase 1 this is the entire
/// application; later phases add command-line handlers, custom
/// activation, etc.
pub struct MugenkaiApplication {
    inner: adw::Application,
}

impl MugenkaiApplication {
    #[must_use]
    pub fn new() -> Self {
        let inner = adw::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();

        // NON_UNIQUE means every mugenkai invocation gets its own
        // process instead of being routed to a running instance via
        // DBus. We may want to flip this later for cmux-style
        // multi-window semantics.

        inner.connect_activate(|app| {
            let window = MugenkaiWindow::new(app);
            window.present();
        });

        Self { inner }
    }

    /// Run the application's main loop until exit. Blocking.
    #[must_use]
    pub fn run(&self) -> glib::ExitCode {
        self.inner.run()
    }

    /// Expose the inner `adw::Application` for tests that need it.
    pub fn inner(&self) -> &adw::Application {
        &self.inner
    }
}

impl Default for MugenkaiApplication {
    fn default() -> Self {
        Self::new()
    }
}
