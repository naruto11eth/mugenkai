// Safe Rust wrapper over host-ghostty-sys.
// Real types (Terminal, Formatter, KeyEncoder, MouseEncoder, Effects) land
// in Phase 2 Unit 4 once vendor/ghostty + bindgen are wired in Phase 1 Unit 2.

mod effects;
mod error;
mod formatter;
mod terminal;

pub use effects::{EffectsHandle, TerminalEvent};
pub use error::GhosttyError;
pub use formatter::{Formatter, PlainFormat};
pub use terminal::Terminal;
