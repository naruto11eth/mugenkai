use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk4::{gio, glib};

use host_core::pane_tree::SplitDirection;
use host_core::{MugenkaiConfig, to_gtk_accelerator};

use crate::css;
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
///         ├── top-bar: AdwTabBar (auto-hides when tabs == 1)
///         └── content: AdwTabView
///             └── one SplitContainer per tab
///                 └── leaves are PaneWidgets running shell PTYs
/// ```
///
/// The window owns an `Rc<RefCell<Vec<Rc<SplitContainer>>>>`
/// that tracks split containers in lockstep with the
/// `AdwTabView`'s pages. Cycling / closing tabs manipulates
/// this vector plus the tab view together; the keyboard
/// controller queries the currently active container every
/// keypress so typing routes to whatever pane has focus right
/// now.
pub struct MugenkaiWindow {
    inner: adw::ApplicationWindow,
    // Held to keep the containers + their worker threads alive
    // for the window's lifetime.
    _containers: Rc<RefCell<Vec<Rc<SplitContainer>>>>,
}

impl MugenkaiWindow {
    /// Build a window with default config.
    #[must_use]
    pub fn new(app: &adw::Application) -> Self {
        Self::with_config(app, &Rc::new(MugenkaiConfig::default()))
    }

    /// Build a window using the user's configuration.
    #[must_use]
    pub fn with_config(app: &adw::Application, config: &Rc<MugenkaiConfig>) -> Self {
        css::install_for_default_display(&config.pane.focus_ring_color);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("mugenkai")
            .default_width(1280)
            .default_height(800)
            .build();

        let toolbar_view = adw::ToolbarView::new();

        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        // Tab surface — auto-hides when only one tab exists.
        let tab_view = adw::TabView::new();
        let tab_bar = adw::TabBar::builder()
            .view(&tab_view)
            .autohide(true)
            .build();
        toolbar_view.add_top_bar(&tab_bar);

        // Shared vector of split containers; index-locked with
        // tab_view.n_pages(). Wrapped in Rc<RefCell<>> so
        // every action handler can clone a reference.
        let containers: Rc<RefCell<Vec<Rc<SplitContainer>>>> = Rc::new(RefCell::new(Vec::new()));

        // Append the initial tab. The new_tab helper keeps the
        // vector and the tab view in sync.
        new_tab(&tab_view, &containers, config, "shell");

        // Keyboard input routes to the active tab's focused
        // pane's input sender, looked up fresh on every key.
        let containers_for_keys = Rc::clone(&containers);
        let tab_view_for_keys = tab_view.clone();
        install_pane_key_controller_dynamic(&window, move || {
            let pos = selected_index(&tab_view_for_keys)?;
            let containers = containers_for_keys.borrow();
            let container = containers.get(pos)?;
            container.focused_input_sender()
        });

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        Self::install_shortcuts(&window, app, &tab_view, &containers, config);

        Self {
            inner: window,
            _containers: containers,
        }
    }

    fn install_shortcuts(
        window: &adw::ApplicationWindow,
        app: &adw::Application,
        tab_view: &adw::TabView,
        containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
        config: &Rc<MugenkaiConfig>,
    ) {
        let kb = &config.keybindings;

        install_quit_action(window, app, &kb.quit);
        install_split_action(
            window,
            app,
            tab_view,
            containers,
            "split-horizontal",
            &kb.split_horizontal,
            SplitDirection::Right,
        );
        install_split_action(
            window,
            app,
            tab_view,
            containers,
            "split-vertical",
            &kb.split_vertical,
            SplitDirection::Down,
        );
        install_close_pane_action(window, app, tab_view, containers, &kb.close_pane);
        install_focus_action(
            window,
            app,
            tab_view,
            containers,
            "focus-next",
            &kb.focus_next,
            /* forward */ true,
        );
        install_focus_action(
            window,
            app,
            tab_view,
            containers,
            "focus-prev",
            &kb.focus_prev,
            /* forward */ false,
        );
        install_new_tab_action(window, app, tab_view, containers, config, &kb.new_tab);
        install_close_tab_action(window, app, tab_view, containers, &kb.close_tab);
        install_cycle_tab_action(window, app, tab_view, "next-tab", &kb.next_tab, true);
        install_cycle_tab_action(window, app, tab_view, "prev-tab", &kb.prev_tab, false);
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

/// Append a new tab to `tab_view` containing a fresh
/// [`SplitContainer`], push the container into `containers`,
/// and make the new tab active.
fn new_tab(
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    config: &Rc<MugenkaiConfig>,
    title: &str,
) {
    let container = Rc::new(SplitContainer::with_config(Rc::clone(config)));
    let page = tab_view.append(container.widget());
    page.set_title(title);

    containers.borrow_mut().push(container);
    tab_view.set_selected_page(&page);
}

/// Return the active page position (index into the container
/// vector) or None if the tab view has no pages.
fn selected_index(tab_view: &adw::TabView) -> Option<usize> {
    let page = tab_view.selected_page()?;
    let idx = tab_view.page_position(&page);
    if idx < 0 {
        None
    } else {
        usize::try_from(idx).ok()
    }
}

fn active_container(
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
) -> Option<Rc<SplitContainer>> {
    let idx = selected_index(tab_view)?;
    containers.borrow().get(idx).cloned()
}

// ─── action installers ─────────────────────────────────────────

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
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    name: &str,
    accel_str: &str,
    direction: SplitDirection,
) {
    let action = gio::SimpleAction::new(name, None);
    let tab_view = tab_view.clone();
    let containers = Rc::clone(containers);
    action.connect_activate(move |_, _| {
        if let Some(container) = active_container(&tab_view, &containers) {
            container.split_focused(direction);
        }
    });
    window.add_action(&action);
    set_accel(app, &format!("win.{name}"), accel_str);
}

fn install_close_pane_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("close-pane", None);
    let tab_view_clone = tab_view.clone();
    let containers_clone = Rc::clone(containers);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            let Some(container) = active_container(&tab_view_clone, &containers_clone) else {
                return;
            };
            if !container.close_focused() {
                // Last pane in the tab — close the tab. If
                // that was the last tab, close the window.
                close_active_tab(&tab_view_clone, &containers_clone, &window);
            }
        }
    ));
    window.add_action(&action);
    set_accel(app, "win.close-pane", accel_str);
}

fn install_focus_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    name: &str,
    accel_str: &str,
    forward: bool,
) {
    let action = gio::SimpleAction::new(name, None);
    let tab_view = tab_view.clone();
    let containers = Rc::clone(containers);
    action.connect_activate(move |_, _| {
        let Some(container) = active_container(&tab_view, &containers) else {
            return;
        };
        if forward {
            container.focus_next_styled();
        } else {
            container.focus_prev_styled();
        }
    });
    window.add_action(&action);
    set_accel(app, &format!("win.{name}"), accel_str);
}

fn install_new_tab_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    config: &Rc<MugenkaiConfig>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("new-tab", None);
    let tab_view = tab_view.clone();
    let containers = Rc::clone(containers);
    let config = Rc::clone(config);
    action.connect_activate(move |_, _| {
        new_tab(&tab_view, &containers, &config, "shell");
    });
    window.add_action(&action);
    set_accel(app, "win.new-tab", accel_str);
}

fn install_close_tab_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("close-tab", None);
    let tab_view_clone = tab_view.clone();
    let containers_clone = Rc::clone(containers);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            close_active_tab(&tab_view_clone, &containers_clone, &window);
        }
    ));
    window.add_action(&action);
    set_accel(app, "win.close-tab", accel_str);
}

fn install_cycle_tab_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    name: &str,
    accel_str: &str,
    forward: bool,
) {
    let action = gio::SimpleAction::new(name, None);
    let tab_view = tab_view.clone();
    action.connect_activate(move |_, _| {
        cycle_tab(&tab_view, forward);
    });
    window.add_action(&action);
    set_accel(app, &format!("win.{name}"), accel_str);
}

/// Close the currently active tab. Removes the matching entry
/// from the container vector and closes the window if the last
/// tab was just removed.
fn close_active_tab(
    tab_view: &adw::TabView,
    containers: &Rc<RefCell<Vec<Rc<SplitContainer>>>>,
    window: &adw::ApplicationWindow,
) {
    let Some(idx) = selected_index(tab_view) else {
        return;
    };
    let Some(page) = tab_view.selected_page() else {
        return;
    };

    // Drop the container BEFORE the tab view closes the page.
    // close_page() asynchronously notifies close handlers and
    // then removes the page; for v0.1 we use the synchronous
    // close_page_finish path via close_page + confirm.
    tab_view.close_page(&page);
    tab_view.close_page_finish(&page, true);

    {
        let mut list = containers.borrow_mut();
        if idx < list.len() {
            list.remove(idx);
        }
    }

    if containers.borrow().is_empty() {
        window.close();
    }
}

/// Cycle the selected tab by one position, wrapping at the ends.
fn cycle_tab(tab_view: &adw::TabView, forward: bool) {
    let n = tab_view.n_pages();
    if n < 2 {
        return;
    }
    let Some(current) = tab_view.selected_page() else {
        return;
    };
    let current_idx = tab_view.page_position(&current);
    if current_idx < 0 {
        return;
    }
    let next_idx = if forward {
        (current_idx + 1) % n
    } else {
        (current_idx - 1 + n) % n
    };
    let next = tab_view.nth_page(next_idx);
    tab_view.set_selected_page(&next);
}

/// Translate a config-level accelerator string (`ctrl+shift+d`)
/// into GTK format (`<Primary><Shift>d`) and register it
/// against the application's action map. Skips registration
/// (logs a warning) if the string can't be translated.
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
