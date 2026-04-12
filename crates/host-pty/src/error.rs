use thiserror::Error;

/// Errors surfaced by the `host-pty` crate.
#[derive(Debug, Error)]
pub enum PtyError {
    /// `forkpty` or a related syscall failed at the libc boundary.
    /// `context` is a short human-readable tag describing which step
    /// failed (e.g. "forkpty", "ioctl(TIOCSWINSZ)", "execvp").
    #[error("pty syscall '{context}' failed: {source}")]
    Syscall {
        context: &'static str,
        #[source]
        source: nix::Error,
    },

    /// The shell binary we tried to exec couldn't be found.
    #[error("shell '{shell}' not found on $PATH")]
    ShellNotFound { shell: String },

    /// `$SHELL` is unset AND we have no fallback.
    #[error("$SHELL is unset and no fallback available")]
    NoShell,

    /// IO on the PTY master fd failed.
    #[error("pty io failed: {0}")]
    Io(#[from] std::io::Error),

    /// A resize was requested with zero rows/cols, which some kernels
    /// reject.
    #[error("cannot resize pty to {cols}x{rows}")]
    InvalidSize { cols: u16, rows: u16 },
}
