//! Spike: prove libghostty-vt.so works from Rust on Linux.
//!
//! Mirrors the upstream `example/c-vt-stream/src/main.c` so we can compare
//! output 1:1 against the reference C program.

#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case)]
#![allow(dead_code)]

mod sys {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

use std::ptr;

fn write_vt(term: sys::GhosttyTerminal, bytes: &[u8]) {
    unsafe {
        sys::ghostty_terminal_vt_write(term, bytes.as_ptr(), bytes.len());
    }
}

fn main() {
    unsafe {
        // Create a terminal.
        let mut terminal: sys::GhosttyTerminal = ptr::null_mut();
        let opts = sys::GhosttyTerminalOptions {
            cols: 80,
            rows: 24,
            max_scrollback: 0,
        };
        let result =
            sys::ghostty_terminal_new(ptr::null_mut(), &mut terminal as *mut _, opts);
        assert_eq!(
            result,
            sys::GhosttyResult_GHOSTTY_SUCCESS,
            "ghostty_terminal_new"
        );
        assert!(!terminal.is_null());

        // Drive the same VT sequences the C example does.
        write_vt(terminal, b"Hello, World!\r\n");
        write_vt(terminal, b"\x1b[1;32mGreen Text\x1b[0m\r\n");
        write_vt(terminal, b"\x1b[1;1HTop-left corner\r\n");
        write_vt(terminal, b"\x1b[5B");
        write_vt(terminal, b"Moved down!\r\n");
        write_vt(terminal, b"\x1b[2K");
        write_vt(terminal, b"New content\r\n");
        write_vt(terminal, b"Line A\r\nLine B\r\nLine C\r\n");

        // Read the screen back as plain text via the formatter.
        let mut fmt_opts: sys::GhosttyFormatterTerminalOptions = std::mem::zeroed();
        fmt_opts.size = std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>();
        fmt_opts.emit = sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN;
        fmt_opts.trim = true;

        let mut formatter: sys::GhosttyFormatter = ptr::null_mut();
        let result = sys::ghostty_formatter_terminal_new(
            ptr::null_mut(),
            &mut formatter as *mut _,
            terminal,
            fmt_opts,
        );
        assert_eq!(
            result,
            sys::GhosttyResult_GHOSTTY_SUCCESS,
            "formatter_terminal_new"
        );

        let mut buf: *mut u8 = ptr::null_mut();
        let mut len: usize = 0;
        let result = sys::ghostty_formatter_format_alloc(
            formatter,
            ptr::null_mut(),
            &mut buf as *mut _,
            &mut len as *mut _,
        );
        assert_eq!(result, sys::GhosttyResult_GHOSTTY_SUCCESS, "format_alloc");

        let slice = std::slice::from_raw_parts(buf, len);
        let text = std::str::from_utf8(slice).expect("formatter produced non-utf8");
        println!("{}", text);

        sys::ghostty_free(ptr::null_mut(), buf, len);
        sys::ghostty_formatter_free(formatter);
        sys::ghostty_terminal_free(terminal);
    }

    eprintln!("[spike] libghostty-vt rust embedding GREEN");
}
