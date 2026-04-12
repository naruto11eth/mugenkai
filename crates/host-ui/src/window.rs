use adw::prelude::*;
use gtk4::{gio, glib};

/// The top-level application window.
///
/// Layout:
/// ```text
/// AdwApplicationWindow
/// └── AdwToastOverlay (so future errors can surface as toasts)
///     └── AdwToolbarView
///         ├── top-bar: AdwHeaderBar
///         └── content: AdwTabView + AdwTabBar
///             └── one placeholder page
/// ```
///
/// For v0.1 the tab content is a single static label. Real `PaneWidget`
/// rendering + recursive splits land in Phase 2 (Units 4, 4b, 5).
pub struct MugenkaiWindow {
    inner: adw::ApplicationWindow,
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

        // Placeholder content. The visible "message" to the user that
        // the terminal layer is coming in a later phase. Replaced by
        // real PaneWidget rendering in Phase 2 Unit 4b.
        let placeholder = gtk4::Label::builder()
            .label("mugenkai 0.1.0-dev\n\nPhase 1: GTK4 host shell.\nTerminal rendering lands in Phase 2.")
            .halign(gtk4::Align::Center)
            .valign(gtk4::Align::Center)
            .justify(gtk4::Justification::Center)
            .wrap(true)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();

        let tab_page = tab_view.append(&placeholder);
        tab_page.set_title("welcome");

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        Self::install_shortcuts(&window, app);

        Self { inner: window }
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
