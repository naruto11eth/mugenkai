//! Workspace: ordered collection of tabs, each containing a split tree.
//!
//! This is the second pure-logic layer on top of [`PaneTree`]. A
//! [`Workspace`] owns an ordered list of [`Tab`]s plus the index of
//! the currently active tab. Every tab owns one [`PaneTree`], so the
//! full data model is:
//!
//! ```text
//! Workspace
//! └── Vec<Tab>
//!     └── PaneTree
//!         └── Node (recursive split tree of PaneIds)
//! ```
//!
//! Like [`PaneTree`], this module knows nothing about GTK. The host-ui
//! crate materializes each tab as an `adw::TabPage` and each tab's
//! tree as a `SplitContainer`.

use crate::pane_tree::PaneTree;
use thiserror::Error;

/// Opaque per-tab identifier. Monotonically assigned by
/// [`Workspace::new`] / [`Workspace::new_tab`].
pub type TabId = u64;

/// A single tab in the workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    id: TabId,
    title: String,
    tree: PaneTree,
}

impl Tab {
    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: impl Into<String>) {
        self.title = title.into();
    }

    /// Immutable access to the tab's split tree.
    #[must_use]
    pub fn tree(&self) -> &PaneTree {
        &self.tree
    }

    /// Mutable access to the tab's split tree.
    pub fn tree_mut(&mut self) -> &mut PaneTree {
        &mut self.tree
    }
}

/// Errors surfaced by [`Workspace`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WorkspaceError {
    /// No tab with the given id exists in the workspace.
    #[error("tab {0} not found")]
    TabNotFound(TabId),
    /// Attempted to close the last remaining tab.
    #[error("cannot close the last remaining tab")]
    CannotCloseLastTab,
    /// Index used for reorder/select is out of bounds.
    #[error("tab index {index} out of range (len {len})")]
    IndexOutOfRange { index: usize, len: usize },
}

/// An ordered collection of tabs with a designated active tab.
///
/// Invariants:
/// - `tabs` is always non-empty
/// - `active_index < tabs.len()`
/// - Tab ids are unique within the workspace
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    tabs: Vec<Tab>,
    active_index: usize,
    next_tab_id: u64,
}

impl Workspace {
    /// Create a new workspace with a single tab holding one pane.
    #[must_use]
    pub fn new() -> Self {
        let tab = Tab {
            id: 1,
            title: "shell".to_string(),
            tree: PaneTree::new(),
        };
        Self {
            tabs: vec![tab],
            active_index: 0,
            next_tab_id: 2,
        }
    }

    /// Number of tabs.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Always `false` — workspace invariant is >= 1 tab.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// All tabs in display order.
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// Index of the currently active tab.
    #[must_use]
    pub fn active_index(&self) -> usize {
        self.active_index
    }

    /// Borrow the active tab.
    #[must_use]
    pub fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_index]
    }

    /// Mutably borrow the active tab.
    pub fn active_tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_index]
    }

    /// Borrow a tab by id.
    #[must_use]
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// Mutably borrow a tab by id.
    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Position (index) of the tab with `id`, if it exists.
    #[must_use]
    pub fn index_of(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Append a new tab containing one fresh pane and make it active.
    ///
    /// Returns the new tab's id.
    pub fn new_tab(&mut self, title: impl Into<String>) -> TabId {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab {
            id,
            title: title.into(),
            tree: PaneTree::new(),
        });
        self.active_index = self.tabs.len() - 1;
        id
    }

    /// Close the tab with the given id.
    ///
    /// # Errors
    ///
    /// - [`WorkspaceError::TabNotFound`] if the id is unknown
    /// - [`WorkspaceError::CannotCloseLastTab`] if only one tab remains
    pub fn close_tab(&mut self, id: TabId) -> Result<(), WorkspaceError> {
        if self.tabs.len() == 1 {
            return Err(WorkspaceError::CannotCloseLastTab);
        }
        let idx = self.index_of(id).ok_or(WorkspaceError::TabNotFound(id))?;
        self.tabs.remove(idx);
        // Active tab logic: if we closed the active tab, stay at the
        // same index (which now points at the next tab). If the
        // active was AFTER the closed one, shift back by one. If the
        // active was BEFORE, it stays.
        if self.active_index >= idx && self.active_index > 0 {
            self.active_index -= 1;
        }
        if self.active_index >= self.tabs.len() {
            self.active_index = self.tabs.len() - 1;
        }
        Ok(())
    }

    /// Close the active tab (convenience for keybindings).
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::CannotCloseLastTab`] if only one tab remains.
    pub fn close_active_tab(&mut self) -> Result<(), WorkspaceError> {
        let id = self.active_tab().id;
        self.close_tab(id)
    }

    /// Select the tab at `index` as active.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::IndexOutOfRange`] if out of bounds.
    pub fn select_tab(&mut self, index: usize) -> Result<(), WorkspaceError> {
        if index >= self.tabs.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index,
                len: self.tabs.len(),
            });
        }
        self.active_index = index;
        Ok(())
    }

    /// Move the tab at `from` to `to`. Both indices are clamped
    /// in range.
    ///
    /// # Errors
    ///
    /// [`WorkspaceError::IndexOutOfRange`] if either index is out of
    /// bounds.
    pub fn move_tab(&mut self, from: usize, to: usize) -> Result<(), WorkspaceError> {
        if from >= self.tabs.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index: from,
                len: self.tabs.len(),
            });
        }
        if to >= self.tabs.len() {
            return Err(WorkspaceError::IndexOutOfRange {
                index: to,
                len: self.tabs.len(),
            });
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);

        // Track active tab across the move.
        if self.active_index == from {
            self.active_index = to;
        } else if from < self.active_index && self.active_index <= to {
            self.active_index -= 1;
        } else if to <= self.active_index && self.active_index < from {
            self.active_index += 1;
        }
        Ok(())
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}
