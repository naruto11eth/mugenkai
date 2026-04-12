//! Linux PTY management for mugenkai panes.
//!
//! A [`Child`] owns:
//! - A child process spawned under a new pseudo-terminal (`forkpty`)
//! - The master side of that PTY, wrapped in a `tokio::io::unix::AsyncFd`
//!   for non-blocking async reads and writes
//! - The child's pid, so we can signal and reap it on drop
//!
//! This crate is deliberately narrow: it owns PTY lifecycle and byte
//! I/O only. The calling crate (`host-ui`) decides what to *do* with
//! the bytes (forward them to a libghostty-vt `Terminal`, push keys
//! back, etc.).

mod error;
mod winsize;

pub use error::PtyError;
pub use winsize::Winsize;
