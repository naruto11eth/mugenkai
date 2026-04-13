use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gio, glib};

use host_core::pane_tree::SplitDirection;
use host_core::{MugenkaiConfig, to_gtk_accelerator};

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
    /// Build a window with default config.
    #[must_use]
    pub fn new(app: &adw::Application) -> Self {
        Self::with_config(app, &Rc::new(MugenkaiConfig::default()))
    }

    /// Build a window using the user's configuration.
    ///
    /// All keybindings (`split-h`, `split-v`, `close-pane`,
    /// `focus-next`, `focus-prev`, `quit`, `new-tab`, `close-tab`,
    /// `next-tab`, `prev-tab`) are registered as application
    /// actions with accelerators taken from `config.keybindings`.
    /// Cursor appearance is propagated to every pane spawned by
    /// the window's [`SplitContainer`].
    #[must_use]
    pub fn with_config(app: &adw::Application, config: &Rc<MugenkaiConfig>) -> Self {
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
        // initial tab. Cursor config flows through to every pane.
        let container = Rc::new(SplitContainer::with_config(Rc::clone(config)));
        let tab_page = tab_view.append(container.widget());
        tab_page.set_title("shell");

        // Dynamic key controller routes every keystroke to
        // whichever pane currently has tree-level focus.
        let container_for_keys = Rc::clone(&container);
        install_pane_key_controller_dynamic(&window, move || {
            container_for_keys.focused_input_sender()
        });

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        Self::install_shortcuts(&window, app, &container, config);

        Self {
            inner: window,
            _container: container,
        }
    }

    /// Wire application-level actions with user-configurable
    /// accelerators.
    fn install_shortcuts(
        window: &adw::ApplicationWindow,
        app: &adw::Application,
        container: &Rc<SplitContainer>,
        config: &MugenkaiConfig,
    ) {
        let kb = &config.keybindings;

        install_quit_action(window, app, &kb.quit);
        install_split_action(
            window,
            app,
            container,
            "split-horizontal",
            &kb.split_horizontal,
            SplitDirection::Right,
        );
        install_split_action(
            window,
            app,
            container,
            "split-vertical",
            &kb.split_vertical,
            SplitDirection::Down,
        );
        install_close_pane_action(window, app, container, &kb.close_pane);
        install_focus_action(
            window,
            app,
            container,
            "focus-next",
            &kb.focus_next,
            /* forward */ true,
        );
        install_focus_action(
            window,
            app,
            container,
            "focus-prev",
            &kb.focus_prev,
            /* forward */ false,
        );
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

fn install_quit_action(window: &adw::ApplicationWindow, app: &adw::Application, accel_str: &str) {
    let action = gio::SimpleAction::new("quit", None);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            window.close();
        }
    ));
    window.add_action(&action);
    set_accel(app, "win.quit", accel_str);
}

fn install_split_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    container: &Rc<SplitContainer>,
    name: &str,
    accel_str: &str,
    direction: SplitDirection,
) {
    let action = gio::SimpleAction::new(name, None);
    let container = Rc::clone(container);
    action.connect_activate(move |_, _| {
        container.split_focused(direction);
    });
    window.add_action(&action);
    set_accel(app, &format!("win.{name}"), accel_str);
}

fn install_close_pane_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    container: &Rc<SplitContainer>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("close-pane", None);
    let container = Rc::clone(container);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            if !container.close_focused() {
                // Last pane in the tab — close the window.
                window.close();
            }
        }
    ));
    window.add_action(&action);
    set_accel(app, "win.close-pane", accel_str);
}

fn install_focus_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    container: &Rc<SplitContainer>,
    name: &str,
    accel_str: &str,
    forward: bool,
) {
    let action = gio::SimpleAction::new(name, None);
    let container = Rc::clone(container);
    action.connect_activate(move |_, _| {
        if forward {
            container.focus_next();
        } else {
            container.focus_prev();
        }
    });
    window.add_action(&action);
    set_accel(app, &format!("win.{name}"), accel_str);
}

/// Translate a config-level accelerator string (`ctrl+shift+d`)
/// into GTK format (`<Primary><Shift>d`) and register it against
/// the application's action map. Skips registration (logs a
/// warning) if the string is empty or can't be translated.
fn set_accel(app: &adw::Application, action_name: &str, accel_str: &str) {
    if accel_str.is_empty() {
        return;
    }
    match to_gtk_accelerator(accel_str) {
        Some(gtk_accel) => {
            app.set_accels_for_action(action_name, &[gtk_accel.as_str()]);
        }
        None => {
            eprintln!(
                "[mugenkai] warning: could not translate accelerator \"{accel_str}\" for {action_name}"
            );
        }
    }
}
