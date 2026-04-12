use std::os::fd::RawFd;

use nix::pty::Winsize as NixWinsize;

use crate::error::PtyError;

/// A terminal window size in both cell and pixel units.
///
/// `cell_cols` × `cell_rows` are the character grid dimensions.
/// `px_width` × `px_height` are the pixel dimensions as measured by
/// the renderer; they're reported back to programs running in the
/// terminal when they ask via `TIOCGWINSZ` / `CSI 14 t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Winsize {
    pub cell_cols: u16,
    pub cell_rows: u16,
    pub px_width: u16,
    pub px_height: u16,
}

impl Winsize {
    #[must_use]
    pub fn new(cell_cols: u16, cell_rows: u16, px_width: u16, px_height: u16) -> Self {
        Self {
            cell_cols,
            cell_rows,
            px_width,
            px_height,
        }
    }

    /// Apply this window size to a PTY master fd via `TIOCSWINSZ`.
    ///
    /// # Errors
    ///
    /// - [`PtyError::InvalidSize`] if either cell dimension is zero
    /// - [`PtyError::Syscall`] if the ioctl fails
    pub fn apply(self, master_fd: RawFd) -> Result<(), PtyError> {
        if self.cell_cols == 0 || self.cell_rows == 0 {
            return Err(PtyError::InvalidSize {
                cols: self.cell_cols,
                rows: self.cell_rows,
            });
        }
        let ws = NixWinsize {
            ws_row: self.cell_rows,
            ws_col: self.cell_cols,
            ws_xpixel: self.px_width,
            ws_ypixel: self.px_height,
        };
        // SAFETY: `master_fd` must be a valid tty master descriptor;
        // the caller (Child::resize) holds it live for the duration.
        // nix's tiocswinsz wraps the unsafe ioctl for us.
        unsafe {
            nix::libc::ioctl(master_fd, nix::libc::TIOCSWINSZ, &ws);
        }
        Ok(())
    }
}

impl Default for Winsize {
    fn default() -> Self {
        // 80x24 @ 8x16 pixel cells is the historical default. Updated
        // the moment the renderer knows the real metrics.
        Self::new(80, 24, 640, 384)
    }
}
