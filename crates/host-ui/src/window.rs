use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gio, glib};

use host_core::pane_tree::SplitDirection;

use crate::pane_widget::install_pane_key_controller_dynamic;
use crate::split_container::SplitContainer;

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
///             └── one SplitContainer hosting a PaneTree
///                 └── leaves are PaneWidgets running shell PTYs
/// ```
///
/// v0.1 scope: a single tab with recursive splits. Multi-tab
/// comes in a follow-up commit.
pub struct MugenkaiWindow {
    inner: adw::ApplicationWindow,
    // Held to keep the split container + its worker threads alive
    // for the window's lifetime.
    _container: Rc<SplitContainer>,
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

        // Tab surface — ready for multi-tab, currently hosts one.
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        toolbar_view.add_top_bar(&tab_bar);

        // The split container is the ROOT of the pane tree for the
        // initial tab. It owns one PaneWidget to start.
        let container = Rc::new(SplitContainer::new());
        let tab_page = tab_view.append(container.widget());
        tab_page.set_title("shell");

        // Keyboard input: dynamic key controller looks up the
        // currently focused pane's input sender on every
        // keypress, which lets splits work naturally — typing
        // always goes to whichever pane has tree-level focus.
        let container_for_keys = Rc::clone(&container);
        install_pane_key_controller_dynamic(&window, move || {
            container_for_keys.focused_input_sender()
        });

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        Self::install_shortcuts(&window, app, &container);

        Self {
            inner: window,
            _container: container,
        }
    }

    /// Wire application-level actions:
    ///
    /// - `win.quit` (Ctrl+Q) — close the window
    /// - `win.split-horizontal` (Ctrl+Shift+D) — split focused pane left/right
    /// - `win.split-vertical` (Ctrl+Shift+R) — split focused pane top/bottom
    /// - `win.close-pane` (Ctrl+Shift+W) — close focused pane; if last, close window
    /// - `win.focus-next` (Ctrl+Tab) — cycle focus to next pane in layout order
    /// - `win.focus-prev` (Ctrl+Shift+Tab) — cycle focus backwards
    fn install_shortcuts(
        window: &adw::ApplicationWindow,
        app: &adw::Application,
        container: &Rc<SplitContainer>,
    ) {
        // ─── quit ──────────────────────────────────────────────────
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

        // ─── split horizontal ─────────────────────────────────────
        let split_h = gio::SimpleAction::new("split-horizontal", None);
        {
            let container = Rc::clone(container);
            split_h.connect_activate(move |_, _| {
                container.split_focused(SplitDirection::Right);
            });
        }
        window.add_action(&split_h);
        app.set_accels_for_action("win.split-horizontal", &["<Primary><Shift>d"]);

        // ─── split vertical ───────────────────────────────────────
        let split_v = gio::SimpleAction::new("split-vertical", None);
        {
            let container = Rc::clone(container);
            split_v.connect_activate(move |_, _| {
                container.split_focused(SplitDirection::Down);
            });
        }
        window.add_action(&split_v);
        app.set_accels_for_action("win.split-vertical", &["<Primary><Shift>r"]);

        // ─── close pane ───────────────────────────────────────────
        let close_pane = gio::SimpleAction::new("close-pane", None);
        {
            let container = Rc::clone(container);
            close_pane.connect_activate(glib::clone!(
                #[weak]
                window,
                move |_, _| {
                    if !container.close_focused() {
                        // Last pane in the tab — close the window.
                        window.close();
                    }
                }
            ));
        }
        window.add_action(&close_pane);
        app.set_accels_for_action("win.close-pane", &["<Primary><Shift>w"]);

        // ─── focus next ───────────────────────────────────────────
        let focus_next = gio::SimpleAction::new("focus-next", None);
        {
            let container = Rc::clone(container);
            focus_next.connect_activate(move |_, _| {
                container.focus_next();
            });
        }
        window.add_action(&focus_next);
        app.set_accels_for_action("win.focus-next", &["<Primary>Tab"]);

        // ─── focus prev ───────────────────────────────────────────
        let focus_prev = gio::SimpleAction::new("focus-prev", None);
        {
            let container = Rc::clone(container);
            focus_prev.connect_activate(move |_, _| {
                container.focus_prev();
            });
        }
        window.add_action(&focus_prev);
        app.set_accels_for_action("win.focus-prev", &["<Primary><Shift>Tab"]);
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
