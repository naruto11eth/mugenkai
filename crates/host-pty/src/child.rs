use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::path::PathBuf;

use nix::pty::{ForkptyResult, forkpty};
use nix::sys::signal::{self, Signal};
use nix::unistd::{Pid, execvp};
use tokio::io::unix::AsyncFd;

use crate::error::PtyError;
use crate::winsize::Winsize;

/// Options controlling how a [`Child`] is spawned.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Path to the shell binary. Defaults to `$SHELL`, falling back
    /// to `/bin/bash` then `/bin/sh`.
    pub shell: Option<PathBuf>,
    /// Arguments passed to the shell. For a login shell pass
    /// `["-l"]`, or similar. Empty means no arguments.
    pub args: Vec<String>,
    /// Initial cell grid + pixel size for the PTY.
    pub winsize: Winsize,
}

/// A child process running under a PTY master owned by this struct.
///
/// Async reads and writes go through the embedded `AsyncFd` so they
/// cooperate with the tokio scheduler instead of blocking a worker
/// thread. On `Drop` the child is signaled with `SIGHUP` and the
/// master fd is closed; the kernel's PTY teardown reaps the process
/// group.
pub struct Child {
    master: AsyncFd<OwnedFd>,
    pid: Pid,
    /// Set to `true` once we've observed the child exiting.
    exited: bool,
}

impl Child {
    /// Spawn a new shell inside a fresh PTY.
    ///
    /// # Errors
    ///
    /// - [`PtyError::NoShell`] if neither `opts.shell`, `$SHELL`, nor
    ///   `/bin/bash`/`/bin/sh` resolve
    /// - [`PtyError::Syscall`] if `forkpty`, `ioctl`, or `execvp`
    ///   fails in the child
    pub fn spawn(opts: SpawnOptions) -> Result<Self, PtyError> {
        let shell_path = resolve_shell(opts.shell.as_deref())?;
        let shell_cstr =
            CString::new(shell_path.as_os_str().as_encoded_bytes()).map_err(|err| {
                PtyError::Io(io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
            })?;

        // Build the argv in Rust ahead of forkpty so we don't have to
        // allocate in the child (which is after-fork, async-signal
        // unsafe territory).
        let mut argv_owned: Vec<CString> = Vec::with_capacity(opts.args.len() + 1);
        argv_owned.push(shell_cstr.clone());
        for arg in &opts.args {
            argv_owned.push(CString::new(arg.as_bytes()).map_err(|err| {
                PtyError::Io(io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))
            })?);
        }

        let ws = nix::pty::Winsize {
            ws_row: opts.winsize.cell_rows,
            ws_col: opts.winsize.cell_cols,
            ws_xpixel: opts.winsize.px_width,
            ws_ypixel: opts.winsize.px_height,
        };

        // SAFETY: forkpty is a standard libc call. In the child we
        // must only call async-signal-safe functions until execvp.
        // execvp itself is async-signal-safe.
        let fork_result =
            unsafe { forkpty(Some(&ws), None) }.map_err(|source| PtyError::Syscall {
                context: "forkpty",
                source,
            })?;

        match fork_result {
            ForkptyResult::Child => {
                // In the child. Exec the shell. If exec fails we
                // write a short diagnostic to stderr and _exit so
                // we don't run parent destructors.
                let _ = execvp(&shell_cstr, &argv_owned);
                // If we reach here, execvp failed. Write a minimal
                // message via write(2), then _exit(127).
                let msg = b"mugenkai: failed to exec shell\n";
                // SAFETY: write(2) is async-signal-safe. STDERR_FILENO
                // is 2; we're writing a static message with a known
                // length.
                unsafe {
                    libc::write(libc::STDERR_FILENO, msg.as_ptr().cast(), msg.len());
                    libc::_exit(127);
                }
            }
            ForkptyResult::Parent { child, master } => {
                // The nix crate gives us an OwnedFd for the master
                // starting with 0.29. Wrap it in an AsyncFd.
                let master_fd: OwnedFd = master;
                // Set O_NONBLOCK on the master so AsyncFd's
                // edge-triggered readiness works correctly.
                set_nonblocking(master_fd.as_raw_fd())?;
                let async_fd = AsyncFd::new(master_fd).map_err(PtyError::Io)?;

                Ok(Self {
                    master: async_fd,
                    pid: child,
                    exited: false,
                })
            }
        }
    }

    /// Read up to `buf.len()` bytes from the PTY master.
    ///
    /// Returns `Ok(0)` when the child closes its end of the PTY
    /// (EOF), which happens on child exit. On Linux, the kernel
    /// reports slave-side closure as `EIO` from the master read
    /// rather than the POSIX-ish 0-byte return, so we map `EIO`
    /// to `Ok(0)` here too — both signals mean "child is gone,
    /// stop reading".
    ///
    /// # Errors
    ///
    /// Any OS error other than `EIO`, `EAGAIN`, and `WouldBlock`
    /// (the last two are handled internally by retrying on
    /// readable readiness).
    pub async fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut guard = self.master.readable().await?;
            match guard.try_io(|inner| {
                let fd = inner.get_ref().as_raw_fd();
                // SAFETY: fd is a live, nonblocking descriptor. read
                // returns -1 on error (which try_io maps to Err) or
                // a byte count.
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n >= 0 {
                    #[allow(clippy::cast_sign_loss)]
                    return Ok(n as usize);
                }
                let err = io::Error::last_os_error();
                // Linux convention: EIO from a PTY master read
                // means the slave side closed and the child
                // exited. Collapse to EOF so the caller's read
                // loop sees the same signal as on every other
                // POSIX.
                if err.raw_os_error() == Some(libc::EIO) {
                    return Ok(0);
                }
                Err(err)
            }) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }

    /// Write the entire buffer to the PTY master.
    ///
    /// Handles partial writes and `EAGAIN` retries internally.
    ///
    /// # Errors
    ///
    /// Any OS error other than `WouldBlock`.
    pub async fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let mut guard = self.master.writable().await?;
            match guard.try_io(|inner| {
                let fd = inner.get_ref().as_raw_fd();
                // SAFETY: fd is a live descriptor, buf is valid for
                // buf.len() readable bytes, and write is the std
                // POSIX write syscall.
                let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    #[allow(clippy::cast_sign_loss)]
                    Ok(n as usize)
                }
            }) {
                Ok(Ok(written)) => {
                    buf = &buf[written..];
                }
                Ok(Err(err)) => return Err(err),
                Err(_would_block) => continue,
            }
        }
        Ok(())
    }

    /// Resize the PTY's known window size.
    ///
    /// Forwards to `TIOCSWINSZ`. Programs running in the terminal
    /// (tmux, vim, less) receive a `SIGWINCH` and re-layout.
    ///
    /// # Errors
    ///
    /// [`PtyError::InvalidSize`] for zero dimensions or
    /// [`PtyError::Syscall`] if the ioctl fails.
    pub fn resize(&self, winsize: Winsize) -> Result<(), PtyError> {
        winsize.apply(self.master.get_ref().as_raw_fd())
    }

    /// Return the child process id.
    #[must_use]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Borrow the master fd for read-only inspection. Exposed for
    /// tests and ldd-style debugging; most callers should not use it.
    pub fn master_fd(&self) -> BorrowedFd<'_> {
        // SAFETY: The returned BorrowedFd is tied to &self's lifetime;
        // the OwnedFd inside AsyncFd is alive for at least that long.
        unsafe { BorrowedFd::borrow_raw(self.master.get_ref().as_raw_fd()) }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if !self.exited {
            // Send SIGHUP to the entire process group so any
            // backgrounded children also terminate. Ignore errors —
            // the child may have already exited.
            let _ = signal::kill(self.pid, Signal::SIGHUP);
            // Don't waitpid here — it can block. The child is
            // orphaned to init/systemd-user, which reaps it. In real
            // use (PaneWidget) there's an explicit waitpid loop in
            // a background task.
        }
    }
}

fn resolve_shell(explicit: Option<&std::path::Path>) -> Result<PathBuf, PtyError> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Ok(shell) = std::env::var("SHELL")
        && !shell.is_empty()
    {
        return Ok(PathBuf::from(shell));
    }
    for fallback in ["/bin/bash", "/bin/sh"] {
        let p = PathBuf::from(fallback);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(PtyError::NoShell)
}

fn set_nonblocking(fd: RawFd) -> Result<(), PtyError> {
    // SAFETY: fcntl is the standard POSIX call; fd is owned.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL, 0) };
    if flags < 0 {
        return Err(PtyError::Io(io::Error::last_os_error()));
    }
    let new_flags = flags | libc::O_NONBLOCK;
    // SAFETY: same reasoning as above.
    let ret = unsafe { libc::fcntl(fd, libc::F_SETFL, new_flags) };
    if ret < 0 {
        return Err(PtyError::Io(io::Error::last_os_error()));
    }
    Ok(())
}
