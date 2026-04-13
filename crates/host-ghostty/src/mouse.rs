//! Safe wrapper around libghostty-vt's mouse event encoder.
//!
//! Used by `host-ui` to translate GTK scroll/click events into the
//! escape sequences expected by terminal-mode applications (vim,
//! htop, less, tmux). The encoder consults the terminal's current
//! tracking mode (X10 / normal / button / any-event) and output
//! format (X10 / UTF8 / SGR / URXVT) to produce the right bytes for
//! whichever protocol the application activated via DEC modes.
//!
//! # Lifecycle
//!
//! One [`MouseEncoder`] per pane. The encoder allocates a single
//! reusable [`sys::GhosttyMouseEvent`] handle internally, so the
//! per-scroll cost is just a few setters + one encode call.
//! [`MouseEncoder::sync_from_terminal`] should be called before
//! every encode so the encoder picks up DEC-mode changes from the
//! application running inside the terminal.

use std::ffi::c_void;

use host_ghostty_sys as sys;

use crate::error::GhosttyError;
use crate::terminal::Terminal;

/// What kind of mouse event happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

impl MouseAction {
    fn to_sys(self) -> sys::GhosttyMouseAction {
        match self {
            Self::Press => sys::GHOSTTY_MOUSE_ACTION_PRESS,
            Self::Release => sys::GHOSTTY_MOUSE_ACTION_RELEASE,
            Self::Motion => sys::GHOSTTY_MOUSE_ACTION_MOTION,
        }
    }
}

/// Mouse button identity.
///
/// Wheel up/down map to X11/xterm buttons 4 and 5, which the
/// libghostty-vt encoder reports as `BUTTON_FOUR`/`BUTTON_FIVE` and
/// then translates into the per-protocol byte sequences (e.g. SGR
/// codes 64/65) that vim, htop, tmux understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl MouseButton {
    fn to_sys(self) -> sys::GhosttyMouseButton {
        match self {
            Self::Left => sys::GHOSTTY_MOUSE_BUTTON_LEFT,
            Self::Middle => sys::GHOSTTY_MOUSE_BUTTON_MIDDLE,
            Self::Right => sys::GHOSTTY_MOUSE_BUTTON_RIGHT,
            Self::WheelUp => sys::GHOSTTY_MOUSE_BUTTON_FOUR,
            Self::WheelDown => sys::GHOSTTY_MOUSE_BUTTON_FIVE,
        }
    }
}

/// Encoder that turns mouse events into terminal escape sequences.
///
/// Owns one libghostty-vt encoder + one reusable event handle. Both
/// are freed in `Drop`.
///
/// `MouseEncoder` is `!Sync` for the same reason `Terminal` is — the
/// underlying handles are not thread-safe. It is `Send` so it can
/// migrate between threads as long as access stays serialized.
pub struct MouseEncoder {
    handle: sys::GhosttyMouseEncoder,
    event: sys::GhosttyMouseEvent,
}

// SAFETY: We never share `MouseEncoder` between threads concurrently.
// The handles can move between threads as long as access is
// serialized, which is enforced by `&mut self` on every operation.
unsafe impl Send for MouseEncoder {}

impl MouseEncoder {
    /// Allocate a new encoder + reusable event handle.
    ///
    /// # Errors
    ///
    /// Returns [`GhosttyError::NullTerminalHandle`] if libghostty-vt
    /// returns a null handle for either the encoder or the event,
    /// or any [`GhosttyError`] surfaced by the underlying FFI calls.
    pub fn new() -> Result<Self, GhosttyError> {
        let mut handle: sys::GhosttyMouseEncoder = std::ptr::null_mut();
        // SAFETY: passing a null allocator uses libghostty-vt's
        // default. `handle` is a valid out-pointer.
        let result = unsafe { sys::ghostty_mouse_encoder_new(std::ptr::null(), &raw mut handle) };
        GhosttyError::from_result(result)?;
        if handle.is_null() {
            return Err(GhosttyError::NullTerminalHandle);
        }

        let mut event: sys::GhosttyMouseEvent = std::ptr::null_mut();
        // SAFETY: same as above. We only construct the event after
        // the encoder; if event allocation fails we free the encoder
        // before returning to keep the FFI side balanced.
        let result = unsafe { sys::ghostty_mouse_event_new(std::ptr::null(), &raw mut event) };
        if let Err(err) = GhosttyError::from_result(result) {
            // SAFETY: handle is non-null (checked above).
            unsafe { sys::ghostty_mouse_encoder_free(handle) };
            return Err(err);
        }
        if event.is_null() {
            // SAFETY: handle is non-null (checked above).
            unsafe { sys::ghostty_mouse_encoder_free(handle) };
            return Err(GhosttyError::NullTerminalHandle);
        }

        Ok(Self { handle, event })
    }

    /// Pull the current tracking mode + output format from `terminal`
    /// into the encoder.
    ///
    /// Cheap. Call before every encode — terminal apps switch DEC
    /// modes constantly and the encoder needs current state to
    /// produce the right protocol bytes.
    pub fn sync_from_terminal(&mut self, terminal: &Terminal) {
        // SAFETY: both handles are live (we own one, terminal owns
        // the other) and the call is documented as side-effect-free
        // beyond updating the encoder's internal options.
        unsafe {
            sys::ghostty_mouse_encoder_setopt_from_terminal(self.handle, terminal.as_raw());
        }
    }

    /// Tell the encoder how big the surface and one cell are in
    /// pixels.
    ///
    /// Required for X10 / SGR / UTF8 mouse formats — they encode
    /// the cell coordinate, which the encoder derives by dividing
    /// the event's pixel position by `cell_width`/`cell_height`.
    /// Call once on construction and after every renderer resize.
    pub fn set_size(
        &mut self,
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) {
        let size = sys::GhosttyMouseEncoderSize {
            size: std::mem::size_of::<sys::GhosttyMouseEncoderSize>(),
            screen_width,
            screen_height,
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        };
        // SAFETY: handle is live; `size` is a valid struct of the
        // documented type for OPT_SIZE; the pointer is borrowed for
        // the duration of the synchronous setopt call only.
        unsafe {
            sys::ghostty_mouse_encoder_setopt(
                self.handle,
                sys::GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                (&raw const size).cast::<c_void>(),
            );
        }
    }

    /// Encode a single mouse event into PTY-bound bytes.
    ///
    /// `pixel_x`/`pixel_y` are surface-space pixel coordinates
    /// relative to the top-left of the terminal area.
    ///
    /// Returns an empty `Vec` when the encoder produces no output —
    /// that happens for events the current tracking mode ignores
    /// (e.g. wheel motion in X10 mode, releases in some modes).
    /// Callers should treat empty as "nothing to write" and not as
    /// an error.
    ///
    /// # Errors
    ///
    /// Any [`GhosttyError`] surfaced by the underlying encoder. The
    /// 64-byte stack buffer is large enough for every mouse format
    /// libghostty-vt currently produces (longest is ~24 bytes for
    /// SGR with three coordinates), but if libghostty-vt ever
    /// returns `OutOfSpace` we surface it verbatim.
    pub fn encode(
        &mut self,
        action: MouseAction,
        button: MouseButton,
        pixel_x: f32,
        pixel_y: f32,
    ) -> Result<Vec<u8>, GhosttyError> {
        // SAFETY: `self.event` is a live event handle. The setters
        // mutate fields on that handle; they don't touch anything
        // we're holding a reference to.
        unsafe {
            sys::ghostty_mouse_event_set_action(self.event, action.to_sys());
            sys::ghostty_mouse_event_set_button(self.event, button.to_sys());
            sys::ghostty_mouse_event_set_position(
                self.event,
                sys::GhosttyMousePosition {
                    x: pixel_x,
                    y: pixel_y,
                },
            );
        }

        let mut buf = vec![0u8; 64];
        let mut written: usize = 0;
        // SAFETY: `self.handle` and `self.event` are live; `buf` is
        // a valid mutable buffer of `buf.len()` bytes; `written` is
        // a valid out-pointer.
        let result = unsafe {
            sys::ghostty_mouse_encoder_encode(
                self.handle,
                self.event,
                buf.as_mut_ptr().cast::<std::os::raw::c_char>(),
                buf.len(),
                &raw mut written,
            )
        };
        GhosttyError::from_result(result)?;
        buf.truncate(written);
        Ok(buf)
    }
}

impl Drop for MouseEncoder {
    fn drop(&mut self) {
        // SAFETY: both handles were created by `new` and have not
        // been freed yet (we own them exclusively). libghostty-vt's
        // free functions accept null and are no-ops for non-null
        // handles already freed elsewhere — we don't double-free.
        unsafe {
            sys::ghostty_mouse_event_free(self.event);
            sys::ghostty_mouse_encoder_free(self.handle);
        }
    }
}

impl std::fmt::Debug for MouseEncoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MouseEncoder").finish_non_exhaustive()
    }
}
