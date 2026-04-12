use host_ghostty_sys as sys;

use crate::error::GhosttyError;

/// Maximum cell dimensions the wrapper accepts.
///
/// Chosen well above any realistic terminal (8k columns × 4k rows) to
/// catch integer truncation / underflow bugs while still allowing
/// reasonable scrollback-heavy use cases. libghostty-vt itself may
/// impose tighter limits at runtime.
const MAX_COLS: u16 = 8192;
const MAX_ROWS: u16 = 4096;

/// A libghostty-vt terminal instance.
///
/// Owns the underlying `GhosttyTerminal` handle and guarantees
/// exclusive access via `&mut self` on mutating methods. The handle
/// is freed in `Drop`.
///
/// `Terminal` is intentionally `!Sync`: libghostty-vt terminals are
/// not thread-safe. It is `Send` because the handle can migrate
/// between threads as long as only one thread accesses it at a time.
///
/// # Reentrancy
///
/// Effects callbacks registered via [`crate::effects`] fire
/// synchronously from within [`Terminal::vt_write`]. Callbacks MUST
/// NOT call back into `vt_write` on the same terminal — libghostty-vt
/// is not reentrant. The safe wrapper enforces this via type-state:
/// callbacks are given a read-only snapshot of the terminal state,
/// not a `&mut Terminal`.
pub struct Terminal {
    handle: sys::GhosttyTerminal,
}

// SAFETY: The handle owns its own internal state and migrates between
// threads safely as long as access is serialized. We guarantee
// serialized access by requiring `&mut self` on all mutating methods
// and by not exposing interior mutability.
unsafe impl Send for Terminal {}

impl Terminal {
    /// Construct a new terminal with the given cell grid size and
    /// scrollback capacity.
    ///
    /// `max_scrollback` is the number of rows of scrollback history
    /// retained beyond the visible viewport. `0` disables scrollback
    /// entirely.
    ///
    /// # Errors
    ///
    /// Returns [`GhosttyError::InvalidSize`] if `cols` or `rows` is
    /// zero or exceeds `MAX_COLS` / `MAX_ROWS`. Returns
    /// [`GhosttyError::NullTerminalHandle`] if libghostty-vt's
    /// internal allocator returns a null pointer.
    pub fn new(cols: u16, rows: u16, max_scrollback: usize) -> Result<Self, GhosttyError> {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return Err(GhosttyError::InvalidSize { cols, rows });
        }

        let opts = sys::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback,
        };

        let mut handle: sys::GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: `handle` is a valid out-pointer, `opts` is
        // correctly initialized with non-zero cols/rows, and we pass
        // a null allocator (libghostty-vt uses its internal default).
        let result = unsafe { sys::ghostty_terminal_new(std::ptr::null_mut(), &mut handle, opts) };

        GhosttyError::from_result(result)?;

        if handle.is_null() {
            return Err(GhosttyError::NullTerminalHandle);
        }

        Ok(Self { handle })
    }

    /// Feed a chunk of VT bytes into the terminal state machine.
    ///
    /// This is the primary input path: bytes read from a PTY child
    /// get forwarded here and the terminal's internal state is
    /// updated accordingly (cursor position, cell contents, SGR
    /// state, scrollback, etc.). Effects callbacks fire synchronously
    /// from inside this call.
    ///
    /// Empty slices are a no-op.
    pub fn vt_write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: `self.handle` is non-null and valid for the lifetime
        // of `self`. `bytes` is a valid pointer for `bytes.len()`
        // readable bytes. libghostty-vt processes them synchronously.
        unsafe {
            sys::ghostty_terminal_vt_write(self.handle, bytes.as_ptr(), bytes.len());
        }
    }

    /// Resize the cell grid.
    ///
    /// The terminal reflows existing content into the new dimensions
    /// where possible. Scrollback is preserved.
    ///
    /// `cell_width_px` and `cell_height_px` are the pixel dimensions
    /// of a single cell as measured by the renderer — libghostty-vt
    /// uses them for reported window-size responses and for the
    /// pixel-coordinate half of mouse tracking. Pass values the
    /// renderer actually observed; passing garbage here will lead to
    /// programs (tmux, vim) rendering at the wrong size.
    ///
    /// # Errors
    ///
    /// Returns [`GhosttyError::InvalidSize`] if `cols` or `rows` is
    /// zero or exceeds the wrapper's cap, or an FFI error if
    /// libghostty-vt rejects the new size for internal reasons.
    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<(), GhosttyError> {
        if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
            return Err(GhosttyError::InvalidSize { cols, rows });
        }
        // SAFETY: handle is live for the lifetime of self; dimensions
        // are validated above.
        let result = unsafe {
            sys::ghostty_terminal_resize(self.handle, cols, rows, cell_width_px, cell_height_px)
        };
        GhosttyError::from_result(result)
    }

    /// Reset the terminal to its initial state.
    ///
    /// Equivalent to sending `ESC c` (RIS) to the terminal. Clears
    /// the screen, resets the cursor, drops scrollback, restores
    /// default colors and modes.
    pub fn reset(&mut self) {
        // SAFETY: handle is live.
        unsafe { sys::ghostty_terminal_reset(self.handle) };
    }

    /// Access the raw FFI handle.
    ///
    /// Exposed for use by sibling modules (`effects`, `formatter`)
    /// that need to call libghostty-vt functions on the same
    /// terminal. Not part of the public API.
    pub(crate) fn as_raw(&self) -> sys::GhosttyTerminal {
        self.handle
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: libghostty-vt's free is idempotent on non-null
        // handles and we only construct `Terminal` around non-null
        // handles (enforced by `new`).
        unsafe { sys::ghostty_terminal_free(self.handle) };
    }
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("handle", &self.handle)
            .finish()
    }
}
