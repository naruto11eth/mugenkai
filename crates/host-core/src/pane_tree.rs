//! Recursive binary split tree for terminal panes.
//!
//! This module is the pure-logic half of mugenkai's pane management.
//! It knows nothing about GTK widgets, libghostty-vt, or PTYs — it
//! manipulates a tree of opaque [`PaneId`]s and tracks which one is
//! focused. The host-ui crate materializes this tree as a widget
//! hierarchy (`gtk4::Paned` containers with `PaneWidget` leaves) via
//! a `PaneId -> Widget` map.
//!
//! # Model
//!
//! A [`PaneTree`] owns one [`Node`]. Every node is either:
//! - a `Leaf(PaneId)` — a single pane, or
//! - a `Split { orientation, first, second, ratio }` — two nodes
//!   side-by-side or stacked with a split divider position.
//!
//! Leaves carry IDs rather than references so the tree can be cloned,
//! compared, and property-tested without touching the widget layer.
//!
//! # Operations
//!
//! - [`PaneTree::split`] — split the focused leaf in a given direction,
//!   inserting a new leaf alongside it. Returns the new pane id.
//! - [`PaneTree::close`] — remove the focused leaf. The parent split
//!   collapses back to its remaining sibling. If the focused leaf was
//!   the only pane, the call yields [`CloseResult::LastPane`] and the
//!   caller decides what to do (close tab, open a new one, etc.).
//! - [`PaneTree::set_focused`], [`PaneTree::focused`] — focus tracking.
//! - [`PaneTree::all_panes`] — flat list of every leaf id in layout order.
//! - [`PaneTree::contains`] — membership test.
//!
//! # Invariants
//!
//! At all times:
//! - The tree contains at least one leaf.
//! - The focused id exists in the tree (enforced by all mutators).
//! - Split ratios are clamped to `[MIN_SPLIT_RATIO, MAX_SPLIT_RATIO]`.
//! - Pane ids never collide: [`PaneTree`] mints them from a
//!   monotonically increasing counter.

use thiserror::Error;

/// Opaque per-pane identifier. Monotonically assigned by
/// [`PaneTree::new`] and [`PaneTree::split`].
pub type PaneId = u64;

/// Orientation of a split. `Horizontal` means the divider runs
/// vertically and the two children sit side-by-side (left + right);
/// `Vertical` means the divider runs horizontally and the children
/// stack top + bottom. This matches GTK4's `gtk4::Orientation`
/// convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// Direction a new pane is placed relative to the focused pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Left,
    Right,
    Up,
    Down,
}

impl SplitDirection {
    #[must_use]
    pub fn orientation(self) -> Orientation {
        match self {
            Self::Left | Self::Right => Orientation::Horizontal,
            Self::Up | Self::Down => Orientation::Vertical,
        }
    }

    /// Whether the NEW pane goes before the focused pane in the
    /// parent split (left/up) or after it (right/down).
    #[must_use]
    pub fn new_is_first(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

/// A node in the split tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Leaf(PaneId),
    Split {
        orientation: Orientation,
        first: Box<Node>,
        second: Box<Node>,
        /// Fraction of the container's space allocated to `first`.
        /// Clamped to `[MIN_SPLIT_RATIO, MAX_SPLIT_RATIO]`.
        ratio: f32,
    },
}

/// Default and clamping bounds for split ratios.
pub const DEFAULT_SPLIT_RATIO: f32 = 0.5;
pub const MIN_SPLIT_RATIO: f32 = 0.1;
pub const MAX_SPLIT_RATIO: f32 = 0.9;

/// Errors surfaced by [`PaneTree`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PaneTreeError {
    /// The target pane id is not present in the tree.
    #[error("pane {0} not found in tree")]
    PaneNotFound(PaneId),
}

/// Outcome of [`PaneTree::close`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseResult {
    /// The pane was removed and focus moved to `new_focus`.
    Removed { new_focus: PaneId },
    /// The pane was the only one in the tree — the tree is now
    /// empty and the caller should react accordingly (typically:
    /// close the containing tab).
    LastPane,
}

/// The split tree itself.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneTree {
    root: Node,
    next_id: u64,
    focused: PaneId,
}

impl PaneTree {
    /// Construct a new tree containing a single pane with id 1.
    #[must_use]
    pub fn new() -> Self {
        let root_id: PaneId = 1;
        Self {
            root: Node::Leaf(root_id),
            next_id: 2,
            focused: root_id,
        }
    }

    /// Root node of the tree. For materialization.
    #[must_use]
    pub fn root(&self) -> &Node {
        &self.root
    }

    /// Currently focused pane id.
    #[must_use]
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Move focus to `id`.
    ///
    /// # Errors
    ///
    /// Returns [`PaneTreeError::PaneNotFound`] if `id` is not a
    /// leaf in the current tree.
    pub fn set_focused(&mut self, id: PaneId) -> Result<(), PaneTreeError> {
        if self.contains(id) {
            self.focused = id;
            Ok(())
        } else {
            Err(PaneTreeError::PaneNotFound(id))
        }
    }

    /// `true` if `id` is a leaf in the tree.
    #[must_use]
    pub fn contains(&self, id: PaneId) -> bool {
        node_contains(&self.root, id)
    }

    /// Flat list of every leaf id, in left-to-right then top-to-bottom
    /// traversal order.
    #[must_use]
    pub fn all_panes(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// Number of leaves in the tree.
    #[must_use]
    pub fn len(&self) -> usize {
        count_leaves(&self.root)
    }

    /// Always at least 1.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Split the focused pane in the given direction. The new pane
    /// is inserted alongside the focused one with a default ratio
    /// and receives focus.
    ///
    /// Returns the new pane's id.
    pub fn split(&mut self, direction: SplitDirection) -> PaneId {
        let new_id = self.next_id;
        self.next_id += 1;
        let target = self.focused;

        self.root = split_leaf(
            std::mem::replace(&mut self.root, Node::Leaf(0)),
            target,
            new_id,
            direction,
        );
        self.focused = new_id;
        new_id
    }

    /// Close the focused pane.
    ///
    /// If the tree contains more than one leaf, the parent split
    /// collapses to its remaining sibling and focus moves to the
    /// first leaf of that sibling (deterministic, depth-first).
    /// Otherwise yields [`CloseResult::LastPane`] and the tree is
    /// left unchanged (it must always contain at least one leaf).
    pub fn close_focused(&mut self) -> CloseResult {
        let target = self.focused;
        self.close(target).expect("focused id must be in the tree")
    }

    /// Close an arbitrary pane by id.
    ///
    /// # Errors
    ///
    /// Returns [`PaneTreeError::PaneNotFound`] if `id` is not a
    /// leaf in the current tree.
    pub fn close(&mut self, id: PaneId) -> Result<CloseResult, PaneTreeError> {
        if !self.contains(id) {
            return Err(PaneTreeError::PaneNotFound(id));
        }

        if self.len() == 1 {
            return Ok(CloseResult::LastPane);
        }

        let (new_root, _removed) =
            remove_leaf(std::mem::replace(&mut self.root, Node::Leaf(0)), id)
                .expect("id membership checked above");
        self.root = new_root;

        // Pick a new focus: if the old focus was the closed pane,
        // pick the first leaf in the remaining tree. Otherwise
        // preserve the previous focus (it survived the removal).
        if self.focused == id {
            self.focused = first_leaf(&self.root);
        }

        Ok(CloseResult::Removed {
            new_focus: self.focused,
        })
    }

    /// Move focus to the next pane in layout order, wrapping at the
    /// end. Called by Ctrl+Tab.
    pub fn focus_next(&mut self) {
        let panes = self.all_panes();
        if panes.len() < 2 {
            return;
        }
        let current = self.focused;
        let idx = panes.iter().position(|&id| id == current).unwrap_or(0);
        let next = panes[(idx + 1) % panes.len()];
        self.focused = next;
    }

    /// Move focus to the previous pane in layout order.
    pub fn focus_prev(&mut self) {
        let panes = self.all_panes();
        if panes.len() < 2 {
            return;
        }
        let current = self.focused;
        let idx = panes.iter().position(|&id| id == current).unwrap_or(0);
        let prev = if idx == 0 { panes.len() - 1 } else { idx - 1 };
        self.focused = panes[prev];
    }
}

impl Default for PaneTree {
    fn default() -> Self {
        Self::new()
    }
}

// ─── private helpers ──────────────────────────────────────────────

fn node_contains(node: &Node, id: PaneId) -> bool {
    match node {
        Node::Leaf(leaf_id) => *leaf_id == id,
        Node::Split { first, second, .. } => node_contains(first, id) || node_contains(second, id),
    }
}

fn collect_leaves(node: &Node, out: &mut Vec<PaneId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { first, second, .. } => {
            collect_leaves(first, out);
            collect_leaves(second, out);
        }
    }
}

fn count_leaves(node: &Node) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split { first, second, .. } => count_leaves(first) + count_leaves(second),
    }
}

fn first_leaf(node: &Node) -> PaneId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { first, .. } => first_leaf(first),
    }
}

fn split_leaf(node: Node, target: PaneId, new_id: PaneId, direction: SplitDirection) -> Node {
    match node {
        Node::Leaf(id) if id == target => {
            let old_leaf = Node::Leaf(id);
            let new_leaf = Node::Leaf(new_id);
            let (first, second) = if direction.new_is_first() {
                (Box::new(new_leaf), Box::new(old_leaf))
            } else {
                (Box::new(old_leaf), Box::new(new_leaf))
            };
            Node::Split {
                orientation: direction.orientation(),
                first,
                second,
                ratio: DEFAULT_SPLIT_RATIO,
            }
        }
        Node::Leaf(id) => Node::Leaf(id),
        Node::Split {
            orientation,
            first,
            second,
            ratio,
        } => Node::Split {
            orientation,
            first: Box::new(split_leaf(*first, target, new_id, direction)),
            second: Box::new(split_leaf(*second, target, new_id, direction)),
            ratio,
        },
    }
}

/// Remove the leaf with `target` id from the tree. Returns the new
/// tree and the removed leaf's id. Returns `None` if the target
/// was not in the subtree.
fn remove_leaf(node: Node, target: PaneId) -> Option<(Node, PaneId)> {
    match node {
        Node::Leaf(id) if id == target => None, // leaf-only removal handled by caller
        leaf @ Node::Leaf(_) => Some((leaf, target)),
        Node::Split {
            orientation,
            first,
            second,
            ratio,
        } => {
            // If the target is one of the immediate children, collapse.
            if let Node::Leaf(fid) = *first
                && fid == target
            {
                return Some((*second, target));
            }
            if let Node::Leaf(sid) = *second
                && sid == target
            {
                return Some((*first, target));
            }
            // Otherwise recurse.
            if node_contains(&first, target) {
                let (new_first, removed) = remove_leaf(*first, target)?;
                Some((
                    Node::Split {
                        orientation,
                        first: Box::new(new_first),
                        second,
                        ratio,
                    },
                    removed,
                ))
            } else if node_contains(&second, target) {
                let (new_second, removed) = remove_leaf(*second, target)?;
                Some((
                    Node::Split {
                        orientation,
                        first,
                        second: Box::new(new_second),
                        ratio,
                    },
                    removed,
                ))
            } else {
                None
            }
        }
    }
}
