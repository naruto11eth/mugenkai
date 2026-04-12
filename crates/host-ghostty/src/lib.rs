// Safe Rust wrapper over host-ghostty-sys.
// Real types (Terminal, Formatter, KeyEncoder, MouseEncoder, Effects) land
// in Phase 2 Unit 4 once vendor/ghostty + bindgen are wired in Phase 1 Unit 2.

mod error;

pub use error::GhosttyError;
