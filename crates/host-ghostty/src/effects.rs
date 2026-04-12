//! Terminal effects callbacks.
//!
//! libghostty-vt invokes registered callbacks synchronously from
//! inside [`crate::Terminal::vt_write`] when the terminal observes
//! specific input sequences (BEL, OSC 0/2, DA1/DA2/DA3, etc.). The
//! callbacks MUST NOT call back into `vt_write` on the same
//! terminal — libghostty-vt is not reentrant.
//!
//! This module enforces the reentrancy rule by giving trampolines
//! a tiny "send event via channel" API instead of a live `&mut
//! Terminal`. Consumers drain the channel on another tick.

use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};

use host_ghostty_sys as sys;

use crate::terminal::Terminal;

/// Event emitted by the terminal via a libghostty-vt effects
/// callback.
///
/// The PaneWidget drains these on the GTK main loop and translates
/// them into UI actions (ring the notification border, update the
/// window title, write a query response back to the PTY master,
/// etc.). All variants carry owned data so the enum can cross the
/// channel boundary without lifetime constraints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// BEL (0x07) received. Fires notification ring.
    Bell,
    /// Terminal title changed via OSC 0 or OSC 2. The new title is
    /// NOT carried here — the embedder queries it via
    /// `ghostty_terminal_get` after the event fires. For the v0.1
    /// wiring we just mark the pane as "title dirty" and move on.
    TitleChanged,
    /// Terminal wants to write bytes back to the PTY. Typically in
    /// response to a device status report (DA1/DA2/DA3), cursor
    /// position query (CSI 6 n), or XTWINOPS geometry report
    /// (CSI 14/16/18 t).
    WritePty(Vec<u8>),
}

/// Handle an embedder holds to keep trampolines + userdata alive
/// for the lifetime of a terminal.
///
/// The pointer-to-userdata passed to libghostty-vt (via
/// `GHOSTTY_TERMINAL_OPT_USERDATA`) is a pointer into this struct's
/// pinned allocation. Dropping `EffectsHandle` is safe only *after*
/// the associated Terminal has been dropped or had its userdata
/// cleared, which [`Terminal`]'s drop glue guarantees.
pub struct EffectsHandle {
    // The box is kept alive solely to back the userdata pointer we
    // handed to libghostty-vt. We never dereference it through this
    // field — dereferencing happens inside the trampolines via the
    // raw userdata pointer — but dropping the box would invalidate
    // that pointer and trigger use-after-free on the next callback.
    #[allow(dead_code)]
    inner: Box<EffectsInner>,
    rx: Option<Receiver<TerminalEvent>>,
}

struct EffectsInner {
    tx: Sender<TerminalEvent>,
}

impl EffectsHandle {
    /// Register bell / title / write_pty callbacks on the terminal.
    ///
    /// Returns an `EffectsHandle` + a channel receiver. The handle
    /// must be kept alive for as long as the terminal is alive; the
    /// receiver can be moved anywhere and drained at the consumer's
    /// convenience.
    ///
    /// # Errors
    ///
    /// Returns [`crate::GhosttyError`] if any of the underlying
    /// `ghostty_terminal_set` calls fail.
    pub fn install(terminal: &mut Terminal) -> Result<Self, crate::GhosttyError> {
        let (tx, rx) = mpsc::channel();
        let inner = Box::new(EffectsInner { tx });

        // The userdata pointer we hand to libghostty-vt must stay
        // valid for the lifetime of the terminal. We own the box
        // here and only drop it AFTER we've cleared callbacks.
        let userdata_ptr: *const c_void = (&*inner as *const EffectsInner).cast();

        // SAFETY: terminal.as_raw() is live for the duration of the
        // &mut borrow. userdata_ptr stays live as long as `inner` is
        // not dropped, which Drop below enforces. Each set() call is
        // individually safe.
        unsafe {
            // Userdata is a pointer-type option: the "value" passed
            // to set() IS the pointer to forward to callbacks.
            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_USERDATA,
                userdata_ptr,
            );
            crate::GhosttyError::from_result(result)?;

            let bell_fn: sys::GhosttyTerminalBellFn = Some(bell_trampoline);
            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_BELL,
                bell_fn.map_or(ptr::null(), |f| f as *const c_void),
            );
            crate::GhosttyError::from_result(result)?;

            let title_fn: sys::GhosttyTerminalTitleChangedFn = Some(title_trampoline);
            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                title_fn.map_or(ptr::null(), |f| f as *const c_void),
            );
            crate::GhosttyError::from_result(result)?;

            let write_pty_fn: sys::GhosttyTerminalWritePtyFn = Some(write_pty_trampoline);
            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                write_pty_fn.map_or(ptr::null(), |f| f as *const c_void),
            );
            crate::GhosttyError::from_result(result)?;
        }

        Ok(Self {
            inner,
            rx: Some(rx),
        })
    }

    /// Take the event receiver out of the handle.
    ///
    /// Call this once, at consumer setup time. Subsequent calls
    /// return `None`.
    pub fn take_receiver(&mut self) -> Option<Receiver<TerminalEvent>> {
        self.rx.take()
    }

    /// Clear all registered callbacks on the given terminal.
    ///
    /// Must be called before dropping either the terminal or this
    /// handle if they are not already bundled via `Drop` ordering.
    /// Safe to call multiple times.
    pub fn clear(&self, terminal: &mut Terminal) -> Result<(), crate::GhosttyError> {
        // SAFETY: terminal is live under &mut. Clearing each option
        // to null removes the callback / userdata registration. The
        // C API explicitly documents NULL as "clear the option".
        unsafe {
            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_BELL,
                ptr::null(),
            );
            crate::GhosttyError::from_result(result)?;

            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
                ptr::null(),
            );
            crate::GhosttyError::from_result(result)?;

            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                ptr::null(),
            );
            crate::GhosttyError::from_result(result)?;

            let result = sys::ghostty_terminal_set(
                terminal.as_raw(),
                sys::GHOSTTY_TERMINAL_OPT_USERDATA,
                ptr::null(),
            );
            crate::GhosttyError::from_result(result)?;
        }
        Ok(())
    }
}

// SAFETY: EffectsInner contains only a Sender which is Send. The
// trampolines read it from the libghostty-vt internal thread (which
// is the same thread as the caller of vt_write, since libghostty-vt
// is single-threaded per terminal).
unsafe impl Send for EffectsHandle {}

/// C-callable trampoline for `GHOSTTY_TERMINAL_OPT_BELL`.
///
/// # Safety
///
/// - `userdata` must either be null or a pointer originally returned
///   by `EffectsHandle::install` which points at a live
///   `EffectsInner` allocation. libghostty-vt guarantees this by
///   passing back exactly the userdata the embedder registered.
/// - The function is called synchronously from inside
///   `ghostty_terminal_vt_write` on the terminal-owning thread. We
///   do not touch the terminal pointer; all state lives behind
///   `EffectsInner`.
unsafe extern "C" fn bell_trampoline(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let Some(inner) = (unsafe { dispatch_userdata(userdata) }) else {
        return;
    };
    // A dead channel (receiver dropped) means the consumer gave up
    // on events; we silently drop the event instead of panicking.
    let _ = inner.tx.send(TerminalEvent::Bell);
}

unsafe extern "C" fn title_trampoline(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let Some(inner) = (unsafe { dispatch_userdata(userdata) }) else {
        return;
    };
    let _ = inner.tx.send(TerminalEvent::TitleChanged);
}

unsafe extern "C" fn write_pty_trampoline(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    let Some(inner) = (unsafe { dispatch_userdata(userdata) }) else {
        return;
    };
    if data.is_null() || len == 0 {
        return;
    }
    // The bytes are only valid for the duration of this call; copy
    // them into a Vec so the event carries owned data.
    // SAFETY: data + len describe a valid read region per the
    // libghostty-vt callback contract.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let _ = inner.tx.send(TerminalEvent::WritePty(bytes));
}

/// Turn a raw userdata pointer into a typed `&EffectsInner`.
///
/// # Safety
///
/// The caller must guarantee `userdata` is either null or a pointer
/// originally set via `install`. See the trampoline SAFETY notes.
unsafe fn dispatch_userdata<'a>(userdata: *mut c_void) -> Option<&'a EffectsInner> {
    if userdata.is_null() {
        return None;
    }
    // SAFETY: userdata is the pointer we passed to set(USERDATA),
    // which we got from `&*Box<EffectsInner>`. The Box is alive for
    // the lifetime of the EffectsHandle.
    let inner = unsafe { &*(userdata as *const EffectsInner) };
    Some(inner)
}
