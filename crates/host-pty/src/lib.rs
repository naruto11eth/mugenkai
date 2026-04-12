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
//!
//! # Example
//!
//! ```no_run
//! use host_pty::{Child, SpawnOptions};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut child = Child::spawn(SpawnOptions::default())?;
//! child.write_all(b"echo hello\n").await?;
//! let mut buf = [0u8; 1024];
//! let n = child.read(&mut buf).await?;
//! println!("{}", String::from_utf8_lossy(&buf[..n]));
//! # Ok(())
//! # }
//! ```

mod child;
mod error;
mod winsize;

pub use child::{Child, SpawnOptions};
pub use error::PtyError;
pub use winsize::Winsize;
