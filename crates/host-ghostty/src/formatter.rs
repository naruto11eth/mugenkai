use host_ghostty_sys as sys;

use crate::error::GhosttyError;
use crate::terminal::Terminal;

/// Output format for the terminal formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Strip all escape sequences and render only the printable cell
    /// content. Good for tests and debugging where colors don't matter.
    Plain,
}

impl OutputFormat {
    fn as_sys(self) -> u32 {
        match self {
            Self::Plain => sys::GHOSTTY_FORMATTER_FORMAT_PLAIN,
        }
    }
}

/// A one-shot formatter that renders a terminal's current screen
/// state into a `String`.
///
/// Used primarily in tests to assert on terminal content without
/// caring about the rendering pipeline. The real runtime rendering
/// path goes through `host-renderer` and `ghostty_terminal_grid_ref`,
/// not through the formatter.
pub struct Formatter<'a> {
    handle: sys::GhosttyFormatter,
    _terminal: &'a Terminal,
}

impl<'a> Formatter<'a> {
    /// Build a new formatter tied to the given terminal's current
    /// state.
    ///
    /// The formatter borrows the terminal immutably — multiple
    /// formatters may exist as long as the terminal is not being
    /// mutated elsewhere. `trim` controls whether trailing empty
    /// rows are dropped from the output.
    ///
    /// # Errors
    ///
    /// Returns `GhosttyError::NullFormatterHandle` if libghostty-vt
    /// declines to construct the formatter, or a wrapped
    /// `GhosttyResult` code on FFI failure.
    pub fn new(
        terminal: &'a Terminal,
        format: OutputFormat,
        trim: bool,
    ) -> Result<Self, GhosttyError> {
        // SAFETY: mem::zeroed is legal for ghostty_formatter_terminal_options_s
        // because it is a plain-old-data struct with no invariant
        // beyond size/emit/trim, all of which we set immediately.
        let mut opts: sys::GhosttyFormatterTerminalOptions = unsafe { std::mem::zeroed() };
        opts.size = std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>();
        opts.emit = format.as_sys();
        opts.trim = trim;

        let mut handle: sys::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: terminal handle is live (borrowed), opts is
        // fully initialized, handle is a valid out-pointer.
        let result = unsafe {
            sys::ghostty_formatter_terminal_new(
                std::ptr::null_mut(),
                &mut handle,
                terminal.as_raw(),
                opts,
            )
        };
        GhosttyError::from_result(result)?;

        if handle.is_null() {
            return Err(GhosttyError::NullFormatterHandle);
        }

        Ok(Self {
            handle,
            _terminal: terminal,
        })
    }

    /// Run the formatter and return its output as a UTF-8 `String`.
    ///
    /// Internally calls `ghostty_formatter_format_alloc`, which
    /// mallocs the output buffer; we copy it into a Rust-owned
    /// `String` and free the libghostty-vt buffer before returning.
    ///
    /// # Errors
    ///
    /// - `GhosttyError::FormatterFailure` on non-success result code
    /// - `GhosttyError::NonUtf8Output` if libghostty-vt somehow
    ///   produces non-UTF-8 bytes (should not happen for plain text)
    pub fn format(&self) -> Result<String, GhosttyError> {
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        // SAFETY: handle is live, buf/len are valid out-pointers.
        // libghostty-vt allocates into `buf` on success and we free
        // it before returning.
        let result = unsafe {
            sys::ghostty_formatter_format_alloc(
                self.handle,
                std::ptr::null_mut(),
                &mut buf,
                &mut len,
            )
        };
        if result != sys::GHOSTTY_SUCCESS {
            return Err(GhosttyError::FormatterFailure(result));
        }
        if buf.is_null() || len == 0 {
            return Ok(String::new());
        }

        // SAFETY: buf + len describe a valid byte region allocated
        // by libghostty-vt. We copy it into an owned Vec and then
        // release the original allocation.
        let owned = unsafe {
            let slice = std::slice::from_raw_parts(buf, len);
            let vec = slice.to_vec();
            sys::ghostty_free(std::ptr::null_mut(), buf, len);
            vec
        };

        String::from_utf8(owned).map_err(|err| GhosttyError::NonUtf8Output(err.utf8_error()))
    }
}

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            // SAFETY: handle is non-null and was created by
            // ghostty_formatter_terminal_new.
            unsafe { sys::ghostty_formatter_free(self.handle) };
        }
    }
}

impl std::fmt::Debug for Formatter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formatter")
            .field("handle", &self.handle)
            .finish()
    }
}
