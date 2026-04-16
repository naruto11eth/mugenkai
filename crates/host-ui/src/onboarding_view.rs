//! First-launch onboarding wizard.
//!
//! A full-window overlay that walks the user through:
//!
//! 1. **Welcome** — "What brings you here?" with two selectable
//!    cards (Agent-first / Terminal).
//! 2. **Theme picker** — 5 preset themes with color swatch
//!    previews and live switch.
//! 3. **Done** — summary + "Get started" button.
//!
//! On finish, writes a `~/.config/mugenkai/.onboarded` marker so
//! the wizard doesn't show again on subsequent launches.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;

use crate::css;

/// Unique counter for generating per-widget CSS class names.
use std::sync::atomic::{AtomicU64, Ordering};
static INLINE_ID: AtomicU64 = AtomicU64::new(0);

/// Apply inline CSS to a single widget via a unique class +
/// private `CssProvider`.
fn set_inline_css(widget: &impl gtk4::prelude::IsA<gtk4::Widget>, css_body: &str) {
    let id = INLINE_ID.fetch_add(1, Ordering::Relaxed);
    let class = format!("mg-i{id}");
    let rule = format!(".{class} {{ {css_body} }}");
    widget.add_css_class(&class);
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(&rule);
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

/// The onboarding wizard widget.
///
/// `on_finish` fires when the user completes the wizard — the
/// caller (app.rs) should then transition to the normal
/// `MugenkaiWindow`.
pub struct OnboardingView;

impl OnboardingView {
    /// Build and present the onboarding wizard.
    ///
    /// `on_finish` is called exactly once when the user clicks
    /// "Get started" on the final page.
    pub fn present(app: &adw::Application, on_finish: impl Fn() + 'static) {
        let stack = gtk4::Stack::new();
        stack.set_transition_type(gtk4::StackTransitionType::SlideLeft);
        stack.set_transition_duration(250);

        let current_page: Rc<Cell<u32>> = Rc::new(Cell::new(0));
        let page_count: u32 = 3;

        // ─── page 0: welcome ─────────────────────────────
        let welcome = build_welcome_page();
        stack.add_named(&welcome, Some("welcome"));

        // ─── page 1: theme picker ────────────────────────
        let themes = build_theme_page();
        stack.add_named(&themes, Some("themes"));

        // ─── page 2: done ────────────────────────────────
        let done = build_done_page();
        stack.add_named(&done, Some("done"));

        // ─── navigation bar (bottom) ─────────────────────
        let back_btn = gtk4::Button::builder()
            .label("Back")
            .css_classes(["mg-btn-ghost"])
            .build();
        let next_btn = gtk4::Button::builder()
            .label("Next")
            .css_classes(["mg-btn-primary"])
            .build();

        let dots = build_dots(page_count);

        let nav = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_start(48)
            .margin_end(48)
            .margin_bottom(32)
            .build();
        nav.append(&back_btn);
        let spacer = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        nav.append(&spacer);
        nav.append(&dots.container);
        let spacer2 = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        spacer2.set_hexpand(true);
        nav.append(&spacer2);
        nav.append(&next_btn);

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();
        root.append(&stack);
        root.append(&nav);

        let page_names = ["welcome", "themes", "done"];

        // Back button
        {
            let stack_c = stack.clone();
            let page_c = Rc::clone(&current_page);
            let dots_c = dots.clone();
            let back_c = back_btn.clone();
            let next_c = next_btn.clone();
            back_btn.connect_clicked(move |_| {
                let p = page_c.get();
                if p > 0 {
                    let new_p = p - 1;
                    page_c.set(new_p);
                    stack_c.set_visible_child_name(page_names[new_p as usize]);
                    update_nav(&back_c, &next_c, &dots_c, new_p, page_count);
                }
            });
        }

        // Next button
        {
            let stack_c = stack.clone();
            let page_c = Rc::clone(&current_page);
            let dots_c = dots.clone();
            let back_c = back_btn.clone();
            let next_c = next_btn.clone();
            let on_finish = Rc::new(on_finish);
            let on_finish_clone = Rc::clone(&on_finish);
            next_btn.connect_clicked(move |_| {
                let p = page_c.get();
                if p < page_count - 1 {
                    let new_p = p + 1;
                    page_c.set(new_p);
                    stack_c.set_visible_child_name(page_names[new_p as usize]);
                    update_nav(&back_c, &next_c, &dots_c, new_p, page_count);
                } else {
                    write_onboarded_marker();
                    on_finish_clone();
                }
            });
        }

        // Initial nav state
        update_nav(&back_btn, &next_btn, &dots, 0, page_count);

        let window = adw::Window::builder()
            .application(app)
            .title("Welcome to mugenkai")
            .default_width(900)
            .default_height(600)
            .content(&root)
            .build();

        window.present();
    }
}

// ─── page builders ───────────────────────────────────────────

fn build_welcome_page() -> gtk4::Box {
    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(24)
        .margin_top(64)
        .margin_start(64)
        .margin_end(64)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();

    let heading = gtk4::Label::builder()
        .label("Welcome to mugenkai")
        .css_classes(["mg-heading"])
        .build();
    let sub = gtk4::Label::builder()
        .label("A Linux-first AI-native terminal emulator.")
        .css_classes(["mg-subheading"])
        .build();

    let cards = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(16)
        .halign(gtk4::Align::Center)
        .margin_top(24)
        .build();

    let agent_card = choice_card(
        "Agent-first development",
        "Chat with Claude alongside your shell. Explain errors,\ngenerate commands, run them — all without leaving the terminal.",
    );
    let terminal_card = choice_card(
        "Just a terminal",
        "A fast, modern terminal with splits, tabs, and themes.\nAI features are there when you need them.",
    );

    cards.append(&agent_card);
    cards.append(&terminal_card);

    page.append(&heading);
    page.append(&sub);
    page.append(&cards);
    page
}

fn build_theme_page() -> gtk4::Box {
    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(16)
        .margin_top(48)
        .margin_start(64)
        .margin_end(64)
        .vexpand(true)
        .build();

    let heading = gtk4::Label::builder()
        .label("Choose a theme")
        .css_classes(["mg-heading"])
        .halign(gtk4::Align::Start)
        .build();
    let sub = gtk4::Label::builder()
        .label("Click to preview. You can change this later in Settings.")
        .css_classes(["mg-subheading"])
        .halign(gtk4::Align::Start)
        .build();

    page.append(&heading);
    page.append(&sub);

    let themes = css::available_themes();
    let swatches: &[&[&str]] = &[
        &["#00bcd4", "#0a0e14", "#e6e8ee", "#1a1f2b"],  // mugenkai-dark
        &["#0284c7", "#f8fafc", "#0f172a", "#f1f5f9"],  // light
        &["#2aa198", "#002b36", "#fdf6e3", "#073642"],  // solarized-dark
        &["#bd93f9", "#282a36", "#f8f8f2", "#343746"],  // dracula
        &["#d79921", "#1d2021", "#ebdbb2", "#32302f"],  // gruvbox
    ];

    let selected: Rc<Cell<usize>> = Rc::new(Cell::new(0));

    let list = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .margin_top(16)
        .build();

    for (idx, name) in themes.iter().enumerate() {
        let colors = swatches.get(idx).copied().unwrap_or(&["#888"]);
        let row = theme_row(name, colors, idx, &selected, &list);
        list.append(&row);
    }

    page.append(&list);
    page
}

fn build_done_page() -> gtk4::Box {
    let page = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(24)
        .vexpand(true)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();

    let heading = gtk4::Label::builder()
        .label("You're all set")
        .css_classes(["mg-heading"])
        .build();
    let sub = gtk4::Label::builder()
        .label("Hit \"Get started\" to launch your terminal.\nCtrl+Shift+A opens the Claude chat pane anytime.\nCtrl+Shift+E explains whatever your terminal is showing.")
        .css_classes(["mg-subheading"])
        .justify(gtk4::Justification::Center)
        .build();

    page.append(&heading);
    page.append(&sub);
    page
}

// ─── helper widgets ──────────────────────────────────────────

fn choice_card(title: &str, description: &str) -> gtk4::Box {
    let card = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(8)
        .css_classes(["mg-card-interactive"])
        .build();
    card.set_size_request(300, -1);

    let t = gtk4::Label::builder()
        .label(title)
        .css_classes(["mg-heading"])
        .halign(gtk4::Align::Start)
        .build();
    // Override heading size for cards — smaller than page headings.

    let d = gtk4::Label::builder()
        .label(description)
        .css_classes(["mg-caption"])
        .halign(gtk4::Align::Start)
        .wrap(true)
        .xalign(0.0)
        .build();

    card.append(&t);
    card.append(&d);
    card
}

fn theme_row(
    name: &str,
    colors: &[&str],
    idx: usize,
    selected: &Rc<Cell<usize>>,
    _list: &gtk4::Box,
) -> gtk4::Box {
    let row = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["mg-card-interactive"])
        .build();

    let label = gtk4::Label::builder()
        .label(name)
        .halign(gtk4::Align::Start)
        .hexpand(true)
        .build();
    row.append(&label);

    let swatch_box = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .build();
    for &color in colors {
        let dot = gtk4::DrawingArea::new();
        dot.set_size_request(22, 22);
        dot.add_css_class("mg-swatch");
        set_inline_css(&dot, &format!("background-color: {color};"));
        swatch_box.append(&dot);
    }
    row.append(&swatch_box);

    // Click to select + live preview
    let gesture = gtk4::GestureClick::new();
    let sel = Rc::clone(selected);
    let name_owned = name.to_string();
    gesture.connect_released(move |_, _, _, _| {
        sel.set(idx);
        css::switch_theme(&name_owned);
    });
    row.add_controller(gesture);

    if idx == selected.get() {
        row.add_css_class("mg-card-selected");
    }

    row
}

#[derive(Clone)]
struct ProgressDots {
    container: gtk4::Box,
    dots: Vec<gtk4::Box>,
}

fn build_dots(count: u32) -> ProgressDots {
    let container = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk4::Align::Center)
        .valign(gtk4::Align::Center)
        .build();
    let mut dots = Vec::new();
    for _ in 0..count {
        let dot = gtk4::Box::builder()
            .css_classes(["mg-dot"])
            .build();
        container.append(&dot);
        dots.push(dot);
    }
    ProgressDots { container, dots }
}

fn update_nav(
    back: &gtk4::Button,
    next: &gtk4::Button,
    dots: &ProgressDots,
    page: u32,
    count: u32,
) {
    back.set_visible(page > 0);
    next.set_label(if page == count - 1 { "Get started" } else { "Next" });

    for (i, dot) in dots.dots.iter().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let i_u32 = i as u32;
        if i_u32 <= page {
            dot.add_css_class("mg-dot-active");
        } else {
            dot.remove_css_class("mg-dot-active");
        }
    }
}

// ─── marker file ─────────────────────────────────────────────

fn onboarded_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("mugenkai")
            .join(".onboarded"),
    )
}

fn write_onboarded_marker() {
    if let Some(path) = onboarded_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "1");
    }
}

/// Check whether the user has already completed onboarding.
#[must_use]
pub fn is_onboarded() -> bool {
    onboarded_path().is_some_and(|p| p.exists())
}
