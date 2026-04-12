//! Lifecycle + round-trip tests for the safe `host-ghostty` wrapper.
//!
//! These tests exercise the happy path, drop order, reset, resize,
//! and formatter round-trip. They mirror the Phase 0 spike's shape
//! (see `spike/libghostty-linux/src/main.rs`) but use the safe
//! wrapper so any future FFI or drop-order regression shows up as
//! a test failure rather than a runtime crash.

use host_ghostty::{Formatter, GhosttyError, OutputFormat, Terminal};

#[test]
fn construct_and_drop_is_clean() {
    let term = Terminal::new(80, 24, 0).expect("construct terminal");
    drop(term);
}

#[test]
fn new_with_zero_cols_is_rejected() {
    let err = Terminal::new(0, 24, 0).unwrap_err();
    assert!(
        matches!(err, GhosttyError::InvalidSize { cols: 0, rows: 24 }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn new_with_zero_rows_is_rejected() {
    let err = Terminal::new(80, 0, 0).unwrap_err();
    assert!(
        matches!(err, GhosttyError::InvalidSize { cols: 80, rows: 0 }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn new_with_excessive_size_is_rejected() {
    let err = Terminal::new(9000, 5000, 0).unwrap_err();
    assert!(
        matches!(err, GhosttyError::InvalidSize { .. }),
        "unexpected error: {err:?}"
    );
}

#[test]
fn formatter_round_trip_matches_spike_output() {
    let mut term = Terminal::new(80, 24, 0).expect("construct terminal");

    term.vt_write(b"Hello, World!\r\n");
    term.vt_write(b"\x1b[1;32mGreen Text\x1b[0m\r\n");
    term.vt_write(b"\x1b[1;1HTop-left corner\r\n");
    term.vt_write(b"\x1b[5B");
    term.vt_write(b"Moved down!\r\n");
    term.vt_write(b"\x1b[2K");
    term.vt_write(b"New content\r\n");
    term.vt_write(b"Line A\r\nLine B\r\nLine C\r\n");

    let formatter = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
    let text = formatter.format().expect("format");

    assert!(
        text.contains("Top-left corner"),
        "missing Top-left corner: {text:?}"
    );
    assert!(text.contains("Green Text"), "missing Green Text: {text:?}");
    assert!(
        text.contains("Moved down!"),
        "missing Moved down!: {text:?}"
    );
    assert!(
        text.contains("New content"),
        "missing New content: {text:?}"
    );
    assert!(text.contains("Line C"), "missing Line C: {text:?}");
}

#[test]
fn vt_write_empty_slice_is_noop() {
    let mut term = Terminal::new(80, 24, 0).expect("construct terminal");
    term.vt_write(&[]);
    // No assertion — the point is "does not crash and does not
    // deadlock with libghostty-vt on a zero-length write".
    let formatter = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
    let text = formatter.format().expect("format");
    assert!(text.trim().is_empty(), "expected empty, got {text:?}");
}

#[test]
fn reset_clears_terminal_state() {
    let mut term = Terminal::new(20, 4, 0).expect("construct terminal");
    term.vt_write(b"before reset\r\n");

    term.reset();

    let formatter = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
    let text = formatter.format().expect("format");
    assert!(
        !text.contains("before reset"),
        "expected reset to clear screen, got {text:?}"
    );
}

#[test]
fn resize_to_smaller_grid_works() {
    let mut term = Terminal::new(80, 24, 0).expect("construct terminal");
    term.vt_write(b"hello world\r\n");
    term.resize(40, 12, 8, 16).expect("resize");

    // Content from the larger grid should still be accessible via the
    // formatter (reflowed or truncated — both are acceptable).
    let formatter = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
    let _ = formatter.format().expect("format");
}

#[test]
fn resize_with_zero_dimension_is_rejected() {
    let mut term = Terminal::new(80, 24, 0).expect("construct terminal");
    let err = term.resize(0, 24, 8, 16).unwrap_err();
    assert!(matches!(err, GhosttyError::InvalidSize { .. }));
}

#[test]
fn two_terminals_are_independent() {
    let mut a = Terminal::new(20, 4, 0).expect("terminal a");
    let mut b = Terminal::new(20, 4, 0).expect("terminal b");
    a.vt_write(b"only in A\r\n");
    b.vt_write(b"only in B\r\n");

    let fa = Formatter::new(&a, OutputFormat::Plain, true).expect("fa");
    let fb = Formatter::new(&b, OutputFormat::Plain, true).expect("fb");
    let ta = fa.format().expect("ta");
    let tb = fb.format().expect("tb");

    assert!(ta.contains("only in A"), "A missing its own text: {ta:?}");
    assert!(!ta.contains("only in B"), "A leaked B's text: {ta:?}");
    assert!(tb.contains("only in B"), "B missing its own text: {tb:?}");
    assert!(!tb.contains("only in A"), "B leaked A's text: {tb:?}");
}

#[test]
fn formatter_drops_before_terminal() {
    // Prove that the lifetime constraint on Formatter<'a> prevents
    // the terminal from being dropped while a formatter still
    // references it. This is a compile-time property; the test
    // exists as runtime sanity that the structure works as intended.
    let mut term = Terminal::new(20, 4, 0).expect("terminal");
    term.vt_write(b"live\r\n");
    {
        let formatter = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
        let _ = formatter.format().expect("format");
        // formatter drops here
    }
    // terminal is still usable after the formatter dropped.
    term.vt_write(b"after formatter\r\n");
    let f2 = Formatter::new(&term, OutputFormat::Plain, true).expect("formatter");
    let text = f2.format().expect("format");
    assert!(text.contains("after formatter"));
}
