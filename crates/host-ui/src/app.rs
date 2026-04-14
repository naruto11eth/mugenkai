use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gio, glib};

use host_core::MugenkaiConfig;

use crate::daemon_client::DaemonClient;
use crate::window::MugenkaiWindow;

/// Reverse-DNS app id used for GTK's `application_id` and the desktop
/// entry's `StartupWMClass`.
pub const APP_ID: &str = "com.mugenkai.Mugenkai";

/// Thin wrapper around `adw::Application`.
///
/// Owns the application lifetime and wires the activation callback
/// to construct a [`MugenkaiWindow`] on startup. Loads the user's
/// config once at construction and passes a shared `Rc` to every
/// window — later phases may add reload semantics.
pub struct MugenkaiApplication {
    inner: adw::Application,
    config: Rc<MugenkaiConfig>,
    // RefCell for interior mutability from the non-mut
    // connect_activate closure; Rc so the closure and the outer
    // struct can both reach it.
    windows: Rc<RefCell<Vec<MugenkaiWindow>>>,
    // Sidecar daemon connection. Spawned lazily on first window
    // activation; held in a RefCell<Option<>> so the failure case
    // (daemon binary missing in tests, no bun on PATH) degrades
    // gracefully — mugenkai keeps running as a plain terminal
    // and skill-powered features just no-op until the user sets
    // `MUGENKAI_DAEMON_CMD` and restarts.
    #[allow(dead_code)]
    daemon: Rc<RefCell<Option<DaemonClient>>>,
}

impl MugenkaiApplication {
    /// Build a new application using default config.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MugenkaiConfig::load())
    }

    /// Build a new application with an explicit config, useful
    /// for tests or non-standard paths.
    #[must_use]
    pub fn with_config(config: MugenkaiConfig) -> Self {
        let inner = adw::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();

        // NON_UNIQUE means every mugenkai invocation gets its own
        // process instead of being routed to a running instance
        // via DBus. We may want to flip this later for cmux-style
        // multi-window semantics.

        let config = Rc::new(config);
        let windows: Rc<RefCell<Vec<MugenkaiWindow>>> = Rc::default();
        let daemon: Rc<RefCell<Option<DaemonClient>>> = Rc::default();

        let windows_clone = Rc::clone(&windows);
        let config_clone = Rc::clone(&config);
        let daemon_clone = Rc::clone(&daemon);
        inner.connect_activate(move |app| {
            // Spawn the sidecar daemon the first time a window
            // activates. Subsequent activations reuse the existing
            // connection.
            if daemon_clone.borrow().is_none() {
                match DaemonClient::spawn_and_connect() {
                    Ok(client) => {
                        eprintln!("[mugenkai] daemon ready");
                        *daemon_clone.borrow_mut() = Some(client);
                    }
                    Err(err) => {
                        eprintln!(
                            "[mugenkai] daemon unavailable: {err}. \
                             Running without skills — set MUGENKAI_DAEMON_CMD and \
                             restart to enable."
                        );
                    }
                }
            }

            let window = MugenkaiWindow::with_config(app, &config_clone);
            window.present();
            windows_clone.borrow_mut().push(window);
        });

        Self {
            inner,
            config,
            windows,
            daemon,
        }
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

    /// Expose the loaded config for tests and diagnostics.
    #[must_use]
    pub fn config(&self) -> &MugenkaiConfig {
        &self.config
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
