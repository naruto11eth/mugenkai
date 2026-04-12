use thiserror::Error;

use host_ghostty_sys as sys;

/// Errors surfaced by the safe `host-ghostty` wrapper around libghostty-vt.
///
/// The underlying C library returns a `GhosttyResult` enum for every
/// fallible call. We translate those into `GhosttyError` variants plus
/// a catch-all `UnknownResultCode` for forward compatibility when
/// libghostty-vt adds new error codes we don't yet recognize.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GhosttyError {
    /// Terminal dimensions must be non-zero and within the wrapper's
    /// sanity cap (see `Terminal::new`). libghostty-vt itself may also
    /// reject them, in which case the underlying result is preserved
    /// via `from_result`.
    #[error("invalid terminal size: {cols}x{rows}")]
    InvalidSize { cols: u16, rows: u16 },

    /// `ghostty_terminal_new` returned a null pointer. Rare — usually
    /// only happens under genuine allocator failure.
    #[error("libghostty-vt returned a null terminal handle")]
    NullTerminalHandle,

    /// Constructing a `Formatter` from a live terminal failed at the
    /// FFI layer.
    #[error("libghostty-vt returned a null formatter handle")]
    NullFormatterHandle,

    /// `ghostty_formatter_format_alloc` or a related call returned an
    /// error result. The C-layer code is preserved verbatim.
    #[error("libghostty-vt formatter returned result code {0}")]
    FormatterFailure(i32),

    /// libghostty-vt reported it ran out of memory for an allocation
    /// we requested.
    #[error("libghostty-vt reported out-of-memory")]
    OutOfMemory,

    /// We requested a value from libghostty-vt with a correct shape
    /// but the underlying state didn't have it set.
    #[error("libghostty-vt reported no value available")]
    NoValue,

    /// A formatter output buffer we provided was too small for the
    /// encoded data. Callers should retry with a larger buffer.
    #[error("libghostty-vt reported output buffer too small")]
    OutOfSpace,

    /// We passed a value libghostty-vt considered invalid (e.g. an
    /// out-of-range option tag). This usually indicates a wrapper
    /// bug, not a user bug.
    #[error("libghostty-vt rejected the value as invalid")]
    InvalidValue,

    /// libghostty-vt returned a result code we don't yet understand.
    /// Surfaces the raw integer so callers can debug.
    #[error("libghostty-vt returned unknown result code {0}")]
    UnknownResultCode(i32),

    /// Parser expected UTF-8 output from the formatter but got bytes
    /// that didn't decode. This should not happen for libghostty-vt
    /// plain output but we guard against it.
    #[error("formatter produced non-UTF-8 output: {0}")]
    NonUtf8Output(#[from] std::str::Utf8Error),
}

impl GhosttyError {
    /// Translate a raw `GhosttyResult` into either `Ok(())` or a typed
    /// error variant.
    ///
    /// `sys::GHOSTTY_SUCCESS` is the only value that maps to `Ok`.
    /// Everything else surfaces as an error.
    pub(crate) fn from_result(result: sys::GhosttyResult) -> Result<(), Self> {
        #[allow(non_upper_case_globals)]
        match result {
            sys::GHOSTTY_SUCCESS => Ok(()),
            sys::GHOSTTY_OUT_OF_MEMORY => Err(Self::OutOfMemory),
            sys::GHOSTTY_INVALID_VALUE => Err(Self::InvalidValue),
            sys::GHOSTTY_OUT_OF_SPACE => Err(Self::OutOfSpace),
            sys::GHOSTTY_NO_VALUE => Err(Self::NoValue),
            other => Err(Self::UnknownResultCode(other)),
        }
    }
}
