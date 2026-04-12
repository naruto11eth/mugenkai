//! Smoke tests for the mugenkai GTK4 host shell.
//!
//! Tests that need a real display are marked `#[ignore]` so `cargo test`
//! skips them by default. Run them manually with:
//!
//! ```text
//! cargo test -p host-ui -- --ignored --test-threads=1
//! ```
//!
//! GTK's main loop and `GObject` registration are global per-process
//! state, so these tests MUST run on a single thread. The default
//! `cargo test` parallel harness will SIGABRT if two of them try to
//! `adw::init()` concurrently.
//!
//! Test outcomes are intentionally coarse in Phase 1: we only verify
//! that widget construction doesn't panic and that the application
//! exposes the expected reverse-DNS id. Rich behavioral tests
//! (focus, input events, snapshot rendering) land in later phases
//! once `host-renderer` provides offscreen framebuffer comparison.

use adw::prelude::*;
use host_ui::{APP_ID, MugenkaiApplication, MugenkaiWindow};

fn has_display() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("DISPLAY").is_ok()
}

fn ensure_gtk_initialized() {
    // GTK has a global init guarded by a thread-local. Calling
    // `adw::init()` multiple times is safe; it early-returns after the
    // first success.
    let _ = adw::init();
}

#[test]
fn application_id_is_reverse_dns() {
    assert_eq!(APP_ID, "com.mugenkai.Mugenkai");
    assert!(
        APP_ID.starts_with("com."),
        "application id must be reverse-DNS: {APP_ID}"
    );
    // Validate the format GTK expects: alphanumeric segments separated
    // by dots, each segment starting with a letter.
    for segment in APP_ID.split('.') {
        assert!(!segment.is_empty(), "empty segment in {APP_ID}");
        assert!(
            segment.chars().next().unwrap().is_ascii_alphabetic(),
            "segment '{segment}' must start with a letter"
        );
    }
}

#[test]
#[ignore = "needs single-threaded runner + display; run with --ignored --test-threads=1"]
fn application_constructs_without_panicking() {
    if !has_display() {
        eprintln!("no display available; skipping");
        return;
    }
    ensure_gtk_initialized();
    let app = MugenkaiApplication::new();
    assert_eq!(app.inner().application_id().unwrap(), APP_ID);
}

#[test]
#[ignore = "needs single-threaded runner + display; run with --ignored --test-threads=1"]
fn window_presents_on_real_display() {
    if !has_display() {
        eprintln!("no display available; skipping");
        return;
    }
    ensure_gtk_initialized();
    let app = MugenkaiApplication::new();

    // We have to hold the application registered before constructing a
    // window; adw::ApplicationWindow::builder().application(app) wants
    // the underlying GApplication to be registered.
    app.inner().register(gtk4::gio::Cancellable::NONE).unwrap();

    let window = MugenkaiWindow::new(app.inner());
    window.present();

    // After present(), the window is visible and has a title.
    assert_eq!(window.inner().title().as_deref(), Some("mugenkai"));
    assert!(window.inner().is_visible());

    // Close immediately so `cargo test` doesn't hang a real window.
    window.inner().close();
}

#[test]
#[ignore = "needs single-threaded runner + display; run with --ignored --test-threads=1"]
fn two_windows_in_same_app_have_distinct_identities() {
    if !has_display() {
        eprintln!("no display available; skipping");
        return;
    }
    ensure_gtk_initialized();
    let app = MugenkaiApplication::new();
    app.inner().register(gtk4::gio::Cancellable::NONE).unwrap();

    let w1 = MugenkaiWindow::new(app.inner());
    let w2 = MugenkaiWindow::new(app.inner());
    assert!(
        !std::ptr::eq(w1.inner().as_ptr(), w2.inner().as_ptr()),
        "two windows must have distinct GObject pointers"
    );
    w1.inner().close();
    w2.inner().close();
}
