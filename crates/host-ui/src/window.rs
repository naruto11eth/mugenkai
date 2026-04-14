use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk4::{gio, glib};

use host_core::pane_tree::SplitDirection;
use host_core::{MugenkaiConfig, to_gtk_accelerator};

use crate::css;
use crate::pane_widget::install_pane_key_controller_dynamic;
use crate::settings_view::SettingsView;
use crate::split_container::{ReapOutcome, SplitContainer};

/// Which kind of content lives in a given tab.
///
/// Terminal tabs own a [`SplitContainer`]; the Settings tab
/// owns a [`SettingsView`]. Keybinding handlers that only make
/// sense for terminal tabs (split, close-pane, focus-next)
/// short-circuit when the active tab is a Settings tab.
#[derive(Clone)]
enum TabKind {
    Terminal(Rc<SplitContainer>),
    Settings(#[allow(dead_code)] Rc<SettingsView>),
}

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
    // Held to keep tab-owned widgets + their worker threads
    // alive for the window's lifetime.
    _tabs: Rc<RefCell<Vec<TabKind>>>,
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

        // Shared vector of tab kinds; index-locked with
        // tab_view.n_pages(). Wrapped in Rc<RefCell<>> so every
        // action handler can clone a reference.
        let tabs: Rc<RefCell<Vec<TabKind>>> = Rc::new(RefCell::new(Vec::new()));

        // Append the initial terminal tab. The new_terminal_tab
        // helper keeps the vector and the tab view in sync.
        new_terminal_tab(&tab_view, &tabs, config, "shell");

        // Keyboard input routes to the active terminal tab's
        // focused pane's input sender. Settings tabs don't route
        // keys — they're fully handled by GTK focus.
        let tabs_for_keys = Rc::clone(&tabs);
        let tab_view_for_keys = tab_view.clone();
        install_pane_key_controller_dynamic(&window, move || {
            let container = active_container(&tab_view_for_keys, &tabs_for_keys)?;
            container.focused_input_sender()
        });

        toolbar_view.set_content(Some(&tab_view));

        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&toolbar_view));

        window.set_content(Some(&toast_overlay));

        install_exit_reaper(&window, &tab_view, &tabs);

        Self::install_shortcuts(&window, app, &tab_view, &tabs, config);

        Self {
            inner: window,
            _tabs: tabs,
        }
    }

    fn install_shortcuts(
        window: &adw::ApplicationWindow,
        app: &adw::Application,
        tab_view: &adw::TabView,
        tabs: &Rc<RefCell<Vec<TabKind>>>,
        config: &Rc<MugenkaiConfig>,
    ) {
        let kb = &config.keybindings;

        install_quit_action(window, app, &kb.quit);
        install_split_action(
            window,
            app,
            tab_view,
            tabs,
            "split-horizontal",
            &kb.split_horizontal,
            SplitDirection::Right,
        );
        install_split_action(
            window,
            app,
            tab_view,
            tabs,
            "split-vertical",
            &kb.split_vertical,
            SplitDirection::Down,
        );
        install_close_pane_action(window, app, tab_view, tabs, &kb.close_pane);
        install_focus_action(
            window,
            app,
            tab_view,
            tabs,
            "focus-next",
            &kb.focus_next,
            /* forward */ true,
        );
        install_focus_action(
            window,
            app,
            tab_view,
            tabs,
            "focus-prev",
            &kb.focus_prev,
            /* forward */ false,
        );
        install_new_tab_action(window, app, tab_view, tabs, config, &kb.new_tab);
        install_close_tab_action(window, app, tab_view, tabs, &kb.close_tab);
        install_cycle_tab_action(window, app, tab_view, "next-tab", &kb.next_tab, true);
        install_cycle_tab_action(window, app, tab_view, "prev-tab", &kb.prev_tab, false);
        install_open_settings_action(window, app, tab_view, tabs, config, &kb.open_settings);
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

/// Append a new terminal tab containing a fresh [`SplitContainer`]
/// and make it active.
fn new_terminal_tab(
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    config: &Rc<MugenkaiConfig>,
    title: &str,
) {
    let container = Rc::new(SplitContainer::with_config(Rc::clone(config)));
    let page = tab_view.append(container.widget());
    page.set_title(title);

    tabs.borrow_mut().push(TabKind::Terminal(container));
    tab_view.set_selected_page(&page);
}

/// Append a new Settings tab with the current config loaded
/// into the form rows.
fn new_settings_tab(
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    config: &MugenkaiConfig,
) {
    // If a Settings tab already exists, activate it instead of
    // spawning a second one.
    {
        let list = tabs.borrow();
        for (idx, kind) in list.iter().enumerate() {
            if matches!(kind, TabKind::Settings(_))
                && let Ok(i32_idx) = i32::try_from(idx)
            {
                let page = tab_view.nth_page(i32_idx);
                tab_view.set_selected_page(&page);
                return;
            }
        }
    }

    let view = SettingsView::new(config);
    let page = tab_view.append(view.widget());
    page.set_title("Settings");
    page.set_icon(Some(&gtk4::gio::ThemedIcon::new("emblem-system-symbolic")));

    tabs.borrow_mut().push(TabKind::Settings(view));
    tab_view.set_selected_page(&page);
}

/// Return the active page position (index into the tabs vector)
/// or `None` if the tab view has no pages.
fn selected_index(tab_view: &adw::TabView) -> Option<usize> {
    let page = tab_view.selected_page()?;
    let idx = tab_view.page_position(&page);
    if idx < 0 {
        None
    } else {
        usize::try_from(idx).ok()
    }
}

/// Return the active tab's `SplitContainer`, or `None` when the
/// active tab is a Settings tab (or the tab view is empty).
fn active_container(
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
) -> Option<Rc<SplitContainer>> {
    let idx = selected_index(tab_view)?;
    match tabs.borrow().get(idx)? {
        TabKind::Terminal(container) => Some(Rc::clone(container)),
        TabKind::Settings(_) => None,
    }
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
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    name: &str,
    accel_str: &str,
    direction: SplitDirection,
) {
    let action = gio::SimpleAction::new(name, None);
    let tab_view = tab_view.clone();
    let tabs = Rc::clone(tabs);
    action.connect_activate(move |_, _| {
        if let Some(container) = active_container(&tab_view, &tabs) {
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
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("close-pane", None);
    let tab_view_clone = tab_view.clone();
    let tabs_clone = Rc::clone(tabs);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            let Some(container) = active_container(&tab_view_clone, &tabs_clone) else {
                // Settings tab: route close-pane to close-tab.
                close_active_tab(&tab_view_clone, &tabs_clone, &window);
                return;
            };
            if !container.close_focused() {
                // Last pane in the tab — close the tab. If
                // that was the last tab, close the window.
                close_active_tab(&tab_view_clone, &tabs_clone, &window);
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
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    name: &str,
    accel_str: &str,
    forward: bool,
) {
    let action = gio::SimpleAction::new(name, None);
    let tab_view = tab_view.clone();
    let tabs = Rc::clone(tabs);
    action.connect_activate(move |_, _| {
        let Some(container) = active_container(&tab_view, &tabs) else {
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
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    config: &Rc<MugenkaiConfig>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("new-tab", None);
    let tab_view = tab_view.clone();
    let tabs = Rc::clone(tabs);
    let config = Rc::clone(config);
    action.connect_activate(move |_, _| {
        new_terminal_tab(&tab_view, &tabs, &config, "shell");
    });
    window.add_action(&action);
    set_accel(app, "win.new-tab", accel_str);
}

fn install_close_tab_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("close-tab", None);
    let tab_view_clone = tab_view.clone();
    let tabs_clone = Rc::clone(tabs);
    action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            close_active_tab(&tab_view_clone, &tabs_clone, &window);
        }
    ));
    window.add_action(&action);
    set_accel(app, "win.close-tab", accel_str);
}

fn install_open_settings_action(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    config: &Rc<MugenkaiConfig>,
    accel_str: &str,
) {
    let action = gio::SimpleAction::new("open-settings", None);
    let tab_view = tab_view.clone();
    let tabs = Rc::clone(tabs);
    let config = Rc::clone(config);
    action.connect_activate(move |_, _| {
        new_settings_tab(&tab_view, &tabs, &config);
    });
    window.add_action(&action);
    set_accel(app, "win.open-settings", accel_str);
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
/// from the tabs vector and closes the window if the last tab
/// was just removed.
fn close_active_tab(
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
    window: &adw::ApplicationWindow,
) {
    let Some(idx) = selected_index(tab_view) else {
        return;
    };
    let Some(page) = tab_view.selected_page() else {
        return;
    };

    tab_view.close_page(&page);
    tab_view.close_page_finish(&page, true);

    {
        let mut list = tabs.borrow_mut();
        if idx < list.len() {
            list.remove(idx);
        }
    }

    if tabs.borrow().is_empty() {
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

/// Install a 200ms timer on the GTK main loop that polls every
/// terminal tab for panes whose shell has exited.
///
/// Dead panes are removed from their [`SplitContainer`] via
/// [`SplitContainer::reap_exited`]. If a container reports
/// [`ReapOutcome::AllPanesExited`], the tab is closed outright;
/// if that was the last tab, the window closes.
///
/// This is what makes `exit` and Ctrl+D in a pane actually
/// collapse the split: the worker thread's pty read returns EOF,
/// the 30ms drain tick flips `state.exited`, and this reaper
/// picks it up on the next poll.
fn install_exit_reaper(
    window: &adw::ApplicationWindow,
    tab_view: &adw::TabView,
    tabs: &Rc<RefCell<Vec<TabKind>>>,
) {
    let window = window.clone();
    let tab_view = tab_view.clone();
    let tabs = Rc::clone(tabs);
    glib::timeout_add_local(Duration::from_millis(200), move || {
        // Walk every terminal tab once. Collect the indices of
        // tabs that need closing so we don't mutate `tabs` while
        // iterating. Settings tabs have no panes to reap.
        let mut dead_tab_indices: Vec<usize> = Vec::new();
        {
            let list = tabs.borrow();
            for (idx, kind) in list.iter().enumerate() {
                if let TabKind::Terminal(container) = kind
                    && container.reap_exited() == ReapOutcome::AllPanesExited
                {
                    dead_tab_indices.push(idx);
                }
            }
        }

        // Close dead tabs back-to-front so earlier indices stay
        // valid. `close_tab_at` pulls the entry out of the
        // tabs vec and removes the corresponding tab view page.
        for idx in dead_tab_indices.iter().rev() {
            close_tab_at(&tab_view, &tabs, *idx);
        }

        if tabs.borrow().is_empty() {
            window.close();
            return glib::ControlFlow::Break;
        }

        glib::ControlFlow::Continue
    });
}

/// Remove the tab at `idx` from both the tab view and the tabs
/// vec. Used by the exit reaper for non-selected tabs (the
/// user-triggered path goes through `close_active_tab`).
fn close_tab_at(tab_view: &adw::TabView, tabs: &Rc<RefCell<Vec<TabKind>>>, idx: usize) {
    let Ok(i32_idx) = i32::try_from(idx) else {
        return;
    };
    let page = tab_view.nth_page(i32_idx);
    tab_view.close_page(&page);
    tab_view.close_page_finish(&page, true);

    let mut list = tabs.borrow_mut();
    if idx < list.len() {
        list.remove(idx);
    }
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
