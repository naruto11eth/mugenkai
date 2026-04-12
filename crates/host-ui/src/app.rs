use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gio, glib};

use crate::window::MugenkaiWindow;

/// Reverse-DNS app id used for GTK's `application_id` and the desktop
/// entry's `StartupWMClass`.
pub const APP_ID: &str = "com.mugenkai.Mugenkai";

/// Thin wrapper around `adw::Application`.
///
/// Owns the application lifetime and wires the activation callback to
/// construct a [`MugenkaiWindow`] on startup. Unlike a naive
/// implementation, this struct also keeps the Rust wrapper structs
/// alive in `windows` — otherwise the `_pane` field inside each
/// `MugenkaiWindow` gets dropped the moment `connect_activate`
/// returns, killing the pane worker thread and breaking any
/// channel-based communication with the GTK side. The `adw::Window`
/// would stay on screen (registered with the application) but the
/// Rust half of the ownership graph would be gone.
pub struct MugenkaiApplication {
    inner: adw::Application,
    // RefCell for interior mutability from the non-mut
    // connect_activate closure; Rc so the closure and the outer
    // struct can both reach it.
    windows: Rc<RefCell<Vec<MugenkaiWindow>>>,
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

        let windows: Rc<RefCell<Vec<MugenkaiWindow>>> = Rc::default();

        let windows_clone = Rc::clone(&windows);
        inner.connect_activate(move |app| {
            let window = MugenkaiWindow::new(app);
            window.present();
            windows_clone.borrow_mut().push(window);
        });

        Self { inner, windows }
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

    /// Number of live window wrappers. Primarily for tests.
    #[must_use]
    pub fn window_count(&self) -> usize {
        self.windows.borrow().len()
    }
}

impl Default for MugenkaiApplication {
    fn default() -> Self {
        Self::new()
    }
}
