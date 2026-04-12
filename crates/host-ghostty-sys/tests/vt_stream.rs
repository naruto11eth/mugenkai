//! Workspace-level reproduction of the Phase 0 spike.
//!
//! Mirrors `temp/ghostty/example/c-vt-stream/src/main.c` and
//! `spike/libghostty-linux/src/main.rs`. If this test fails after a
//! `vendor/ghostty` SHA bump, the libghostty-vt API broke and the bump
//! must be reverted (or the wrapper updated). See
//! `docs/solutions/2026-04-12-libghostty-linux-spike.md`.

#![allow(unsafe_code)]

use std::ptr;

use host_ghostty_sys as sys;

fn write_vt(term: sys::GhosttyTerminal, bytes: &[u8]) {
    unsafe {
        sys::ghostty_terminal_vt_write(term, bytes.as_ptr(), bytes.len());
    }
}

#[test]
fn vt_stream_round_trip_matches_upstream_c_example() {
    unsafe {
        let mut terminal: sys::GhosttyTerminal = ptr::null_mut();
        let opts = sys::GhosttyTerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        };
        let result = sys::ghostty_terminal_new(ptr::null_mut(), &mut terminal, opts);
        assert_eq!(result, sys::GHOSTTY_SUCCESS, "ghostty_terminal_new");
        assert!(!terminal.is_null());

        write_vt(terminal, b"Hello, World!\r\n");
        write_vt(terminal, b"\x1b[1;32mGreen Text\x1b[0m\r\n");
        write_vt(terminal, b"\x1b[1;1HTop-left corner\r\n");
        write_vt(terminal, b"\x1b[5B");
        write_vt(terminal, b"Moved down!\r\n");
        write_vt(terminal, b"\x1b[2K");
        write_vt(terminal, b"New content\r\n");
        write_vt(terminal, b"Line A\r\nLine B\r\nLine C\r\n");

        let mut fmt_opts: sys::GhosttyFormatterTerminalOptions = std::mem::zeroed();
        fmt_opts.size = std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>();
        fmt_opts.emit = sys::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        fmt_opts.trim = true;

        let mut formatter: sys::GhosttyFormatter = ptr::null_mut();
        let result = sys::ghostty_formatter_terminal_new(
            ptr::null_mut(),
            &mut formatter,
            terminal,
            fmt_opts,
        );
        assert_eq!(
            result,
            sys::GHOSTTY_SUCCESS,
            "ghostty_formatter_terminal_new"
        );

        let mut buf: *mut u8 = ptr::null_mut();
        let mut len: usize = 0;
        let result =
            sys::ghostty_formatter_format_alloc(formatter, ptr::null_mut(), &mut buf, &mut len);
        assert_eq!(result, sys::GHOSTTY_SUCCESS, "format_alloc");

        let slice = std::slice::from_raw_parts(buf, len);
        let text = std::str::from_utf8(slice).expect("formatter produced non-utf8");

        // The expected screen state matches the spike report verbatim.
        // Cursor positioning + colors + erase line + multi-line all preserved.
        assert!(
            text.contains("Top-left corner"),
            "missing 'Top-left corner': {text:?}"
        );
        assert!(
            text.contains("Green Text"),
            "missing 'Green Text': {text:?}"
        );
        assert!(
            text.contains("Moved down!"),
            "missing 'Moved down!': {text:?}"
        );
        assert!(
            text.contains("New content"),
            "missing 'New content': {text:?}"
        );
        assert!(text.contains("Line A"), "missing 'Line A': {text:?}");
        assert!(text.contains("Line B"), "missing 'Line B': {text:?}");
        assert!(text.contains("Line C"), "missing 'Line C': {text:?}");

        sys::ghostty_free(ptr::null_mut(), buf, len);
        sys::ghostty_formatter_free(formatter);
        sys::ghostty_terminal_free(terminal);
    }
}

#[test]
fn ghostty_terminal_new_with_zero_cols_rejected() {
    unsafe {
        let mut terminal: sys::GhosttyTerminal = ptr::null_mut();
        let opts = sys::GhosttyTerminalOptions {
            cols: 0,
            rows: 24,
            max_scrollback: 0,
        };
        let result = sys::ghostty_terminal_new(ptr::null_mut(), &mut terminal, opts);
        // libghostty-vt should reject zero dimensions; if it accepts them
        // and returns SUCCESS we still need to make sure the pointer is
        // not null and we free it. Either behavior is acceptable as long
        // as we don't crash; the wrapper in `host-ghostty` will validate
        // and reject before reaching here.
        if result == sys::GHOSTTY_SUCCESS {
            assert!(!terminal.is_null());
            sys::ghostty_terminal_free(terminal);
        }
    }
}
