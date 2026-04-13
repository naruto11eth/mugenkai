//! GTK4 materializer for a [`host_core::PaneTree`].
//!
//! Translates the pure-logic split tree into a nested hierarchy of
//! `gtk4::Paned` containers with [`PaneWidget`] leaves. Owns:
//!
//! - the tree model (shared with keybinding handlers via `Rc<RefCell>`)
//! - a `HashMap<PaneId, PaneWidget>` of live pane widgets
//! - a root `gtk4::Box` that gets rebuilt from the tree on every
//!   structural change (split, close)
//!
//! # Why re-materialize on every change
//!
//! Incrementally updating a nested `gtk4::Paned` tree is fiddly (you
//! have to reparent children, preserve size hints, swap `start_child`
//! vs `end_child`). Rebuilding from scratch is simpler and our trees
//! are small enough that the cost is invisible. [`PaneWidget`]
//! instances are preserved across rebuilds — only the container
//! chrome moves.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;

use host_core::MugenkaiConfig;
use host_core::pane_tree::{Node, Orientation, PaneId, PaneTree, SplitDirection};

use crate::pane_widget::{PaneInputSender, PaneWidget};

/// A GTK widget hosting a live [`PaneTree`].
///
/// Wraps a `gtk4::Box` that re-materializes the tree as nested
/// `gtk4::Paned` containers on every structural change. Accessible
/// from both the render tick (via [`widget`](Self::widget)) and
/// keybinding handlers (via the `Rc<RefCell<>>` fields).
pub struct SplitContainer {
    /// Outer container. Contents swapped on every rebuild.
    root: gtk4::Box,
    /// The pure-logic tree. Mutated by split/close/focus actions.
    tree: Rc<RefCell<PaneTree>>,
    /// Live [`PaneWidget`] per [`PaneId`]. Grows on split, shrinks on close.
    panes: Rc<RefCell<HashMap<PaneId, PaneWidget>>>,
    /// Config shared with every new pane so splits inherit the
    /// user's cursor + keybinding preferences.
    config: Rc<MugenkaiConfig>,
}

impl SplitContainer {
    /// Build a new `SplitContainer` with default config.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(Rc::new(MugenkaiConfig::default()))
    }

    /// Build a new `SplitContainer` containing one pane (id 1),
    /// with cursor and split-ratio settings inherited from the
    /// user's config. The same `Rc<MugenkaiConfig>` is shared
    /// across every future pane spawned by [`split_focused`](Self::split_focused).
    #[must_use]
    pub fn with_config(config: Rc<MugenkaiConfig>) -> Self {
        let tree = Rc::new(RefCell::new(PaneTree::new()));
        let root_pane = PaneWidget::with_cursor_config(&config.cursor);
        let root_id = tree.borrow().focused();

        let mut map = HashMap::new();
        map.insert(root_id, root_pane);
        let panes = Rc::new(RefCell::new(map));

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .vexpand(true)
            .build();

        let container = Self {
            root,
            tree,
            panes,
            config,
        };
        container.rematerialize();
        container
    }

    /// Return the underlying `gtk4::Widget` for insertion into a
    /// container.
    #[must_use]
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Clone the Rc to the tree for keybinding handlers that need
    /// to mutate it and then call [`rematerialize`](Self::rematerialize).
    #[must_use]
    #[allow(dead_code)]
    pub fn tree_handle(&self) -> Rc<RefCell<PaneTree>> {
        Rc::clone(&self.tree)
    }

    /// Clone the Rc to the pane map.
    #[must_use]
    #[allow(dead_code)]
    pub fn panes_handle(&self) -> Rc<RefCell<HashMap<PaneId, PaneWidget>>> {
        Rc::clone(&self.panes)
    }

    /// Split the focused pane in the given direction. The new
    /// pane inherits cursor settings from the container's config.
    pub fn split_focused(&self, direction: SplitDirection) {
        let new_id = self.tree.borrow_mut().split(direction);
        let new_pane = PaneWidget::with_cursor_config(&self.config.cursor);
        self.panes.borrow_mut().insert(new_id, new_pane);
        self.rematerialize();
    }

    /// Close the focused pane. Returns `true` if a pane was closed,
    /// `false` if we hit the last-pane sentinel (caller should close
    /// the tab).
    pub fn close_focused(&self) -> bool {
        use host_core::pane_tree::CloseResult;
        let closed_id = self.tree.borrow().focused();
        let result = self.tree.borrow_mut().close_focused();
        match result {
            CloseResult::LastPane => false,
            CloseResult::Removed { .. } => {
                self.panes.borrow_mut().remove(&closed_id);
                self.rematerialize();
                true
            }
        }
    }

    /// Cycle focus to the next pane in layout order.
    pub fn focus_next(&self) {
        self.tree.borrow_mut().focus_next();
    }

    /// Cycle focus to the previous pane.
    pub fn focus_prev(&self) {
        self.tree.borrow_mut().focus_prev();
    }

    /// [`PaneInputSender`] for the currently focused pane, if any.
    ///
    /// Used by the window-level keyboard controller to route
    /// keystrokes to the active pane's PTY.
    #[must_use]
    pub fn focused_input_sender(&self) -> Option<PaneInputSender> {
        let focused = self.tree.borrow().focused();
        self.panes
            .borrow()
            .get(&focused)
            .map(PaneWidget::input_sender)
    }

    /// Apply the `.mugenkai-pane-focused` CSS class to the focused
    /// pane and `.mugenkai-pane-unfocused` to every other one.
    /// Called from [`rematerialize`](Self::rematerialize) after
    /// the widget tree is rebuilt.
    fn apply_focus_styling(&self) {
        if !self.config.pane.focus_ring {
            return;
        }
        let focused = self.tree.borrow().focused();
        let panes = self.panes.borrow();
        for (&id, pane) in panes.iter() {
            let widget = pane.widget();
            if id == focused {
                widget.remove_css_class("mugenkai-pane-unfocused");
                widget.add_css_class("mugenkai-pane-focused");
            } else {
                widget.remove_css_class("mugenkai-pane-focused");
                widget.add_css_class("mugenkai-pane-unfocused");
            }
        }
    }

    /// Tear down and rebuild the GTK widget tree from the current
    /// model. Leaves are moved between container slots; the
    /// underlying `PaneWidget` instances (and their worker threads)
    /// are preserved.
    pub fn rematerialize(&self) {
        // Remove every child from the current container tree so the
        // pane widgets aren't held by obsolete parents when we
        // re-parent them. We can't just iterate root.children()
        // because the widget tree might be deeply nested; walk
        // known pane widgets and unparent each one first.
        {
            let panes = self.panes.borrow();
            for pane in panes.values() {
                if let Some(parent) = pane.widget().parent() {
                    if let Some(paned) = parent.downcast_ref::<gtk4::Paned>() {
                        if paned.start_child().is_some_and(|c| c == *pane.widget()) {
                            paned.set_start_child(None::<&gtk4::Widget>);
                        } else if paned.end_child().is_some_and(|c| c == *pane.widget()) {
                            paned.set_end_child(None::<&gtk4::Widget>);
                        }
                    } else if let Some(bx) = parent.downcast_ref::<gtk4::Box>() {
                        bx.remove(pane.widget());
                    }
                }
            }
        }

        // Drain the root Box of any direct children.
        while let Some(child) = self.root.first_child() {
            self.root.remove(&child);
        }

        // Build a fresh widget tree from the current Node.
        {
            let tree = self.tree.borrow();
            let panes = self.panes.borrow();
            let new_widget = build_widget(tree.root(), &panes);
            new_widget.set_hexpand(true);
            new_widget.set_vexpand(true);
            self.root.append(&new_widget);
        }

        // After the tree is visually up to date, re-apply focus
        // CSS classes so the highlighted pane matches the
        // current focus id.
        self.apply_focus_styling();
    }

    /// Cycle focus to the next pane in layout order, then reapply
    /// focus styling so the ring follows.
    pub fn focus_next_styled(&self) {
        self.focus_next();
        self.apply_focus_styling();
    }

    /// Cycle focus to the previous pane in layout order, then
    /// reapply focus styling.
    pub fn focus_prev_styled(&self) {
        self.focus_prev();
        self.apply_focus_styling();
    }
}

impl Default for SplitContainer {
    fn default() -> Self {
        Self::new()
    }
}

/// Recursively build a GTK widget subtree from a pane-tree
/// [`Node`].
///
/// Leaves return the [`PaneWidget`]'s widget. Splits return a
/// configured [`gtk4::Paned`].
fn build_widget(node: &Node, panes: &HashMap<PaneId, PaneWidget>) -> gtk4::Widget {
    match node {
        Node::Leaf(id) => {
            let pane = panes
                .get(id)
                .unwrap_or_else(|| panic!("pane id {id} not in pane map"));
            pane.widget().clone()
        }
        Node::Split {
            orientation,
            first,
            second,
            ratio,
        } => {
            let gtk_orientation = match orientation {
                Orientation::Horizontal => gtk4::Orientation::Horizontal,
                Orientation::Vertical => gtk4::Orientation::Vertical,
            };
            let split_widget = gtk4::Paned::builder()
                .orientation(gtk_orientation)
                .wide_handle(true)
                .resize_start_child(true)
                .resize_end_child(true)
                .shrink_start_child(false)
                .shrink_end_child(false)
                .hexpand(true)
                .vexpand(true)
                .build();

            let start_child = build_widget(first, panes);
            let end_child = build_widget(second, panes);
            start_child.set_hexpand(true);
            start_child.set_vexpand(true);
            end_child.set_hexpand(true);
            end_child.set_vexpand(true);

            split_widget.set_start_child(Some(&start_child));
            split_widget.set_end_child(Some(&end_child));

            // Position can't be set until the widget is realized
            // and has a size. Defer via connect_realize.
            let ratio = *ratio;
            split_widget.connect_realize(move |paned| {
                let alloc_dim = if paned.orientation() == gtk4::Orientation::Vertical {
                    paned.height()
                } else {
                    paned.width()
                };
                if alloc_dim > 0 {
                    #[allow(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        clippy::cast_precision_loss
                    )]
                    let pos = (alloc_dim as f32 * ratio) as i32;
                    paned.set_position(pos);
                }
            });

            split_widget.upcast()
        }
    }
}
