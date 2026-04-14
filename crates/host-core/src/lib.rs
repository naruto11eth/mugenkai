pub mod config;
pub mod pane_tree;
pub mod workspace;

pub use config::{
    CursorConfig, CursorStyle, FontConfig, KeyBindings, MugenkaiConfig, PaneConfig,
    to_gtk_accelerator,
};
pub use pane_tree::{
    CloseResult, DEFAULT_SPLIT_RATIO, MAX_SPLIT_RATIO, MIN_SPLIT_RATIO, Node, Orientation, PaneId,
    PaneTree, PaneTreeError, SplitDirection,
};
pub use workspace::{Tab, TabId, Workspace, WorkspaceError};

pub const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"));
