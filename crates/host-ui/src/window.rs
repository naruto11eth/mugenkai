use adw::prelude::*;
use gtk4::{gio, glib};

use crate::pane_widget::{PaneWidget, install_pane_key_controller};

/// The top-level application window.
///
/// Layout:
/// ```text
/// AdwApplicationWindow
/// └── AdwToastOverlay (so future errors can surface as toasts)
///     └── AdwToolbarView
///         ├── top-bar: AdwHeaderBar
///         ├── top-bar: AdwTabBar
///         └── content: AdwTabView
///             └── one PaneWidget hosting a libghostty-vt terminal
/// ```
///
/// For v0.1 the pane displays the terminal's plain-text screen
/// state in a label. Real glyph rendering + recursive splits land
/// in Phase 2 (Units 4b, 5).
pub struct MugenkaiWindow {
    inner: adw::ApplicationWindow,
    // Held to keep the pane worker thread alive for the window's
    // lifetime. Dropping the window drops the pane which tells the
    // worker to shut down.
    _pane: PaneWidget,
}

impl MugenkaiWindow {
    #[must_use]
    pub fn new(app: &adw::Application) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("mugenkai")
            .default_width(1280)
            .default_height(800)
            .build();

        let toolbar_view = adw::ToolbarView::new();

        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        // Tab surface
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));

        // Show the tab bar under the header while there's at least one
        // tab. Placed as a second top bar so it sits between the header
        // and the content.
        toolbar_view.add_top_bar(&tab_bar);

        // v0.1 pane: a real libghostty-vt terminal driven by a
        // forkpty'd shell. Text rendering is still a placeholder
        // label — Phase 2 Unit 4b replaces it with a proper GLArea
        // glyph renderer.
        let pane = PaneWidget::new();
        let tab_page = tab_view.append(pane.widget());
        tab_page.set_title("shell");

        // Keyboard input: attach the key controller at the window
        // level so it always fires regardless of which child widget
        // has focus. GTK4 containers like gtk4::Box don't reliably
        // receive key events on their own — window-level capture
        // is the idiom.
        install_pane_key_controller(&window, pane.input_sender());

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        Self::install_shortcuts(&window, app);

        Self {
            inner: window,
            _pane: pane,
        }
    }

    /// Wire `Ctrl+Q` to close the window via a `win.quit` action.
    fn install_shortcuts(window: &adw::ApplicationWindow, app: &adw::Application) {
        let quit_action = gio::SimpleAction::new("quit", None);
        quit_action.connect_activate(glib::clone!(
            #[weak]
            window,
            move |_, _| {
                window.close();
            }
        ));
        window.add_action(&quit_action);
        app.set_accels_for_action("win.quit", &["<Primary>q"]);
    }

    /// Show the window.
    pub fn present(&self) {
        self.inner.present();
    }

    /// Expose the inner `adw::ApplicationWindow` for tests.
    pub fn inner(&self) -> &adw::ApplicationWindow {
        &self.inner
    }
}
