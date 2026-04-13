//! Exhaustive tests for the recursive split tree.

use host_core::pane_tree::{
    CloseResult, Node, Orientation, PaneTree, PaneTreeError, SplitDirection,
};

#[test]
fn new_tree_has_one_pane() {
    let tree = PaneTree::new();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree.all_panes(), vec![1]);
    assert_eq!(tree.focused(), 1);
    assert!(matches!(tree.root(), Node::Leaf(1)));
}

#[test]
fn split_right_adds_leaf_and_focuses_new() {
    let mut tree = PaneTree::new();
    let new_id = tree.split(SplitDirection::Right);
    assert_eq!(tree.len(), 2);
    assert_eq!(tree.focused(), new_id);
    assert!(tree.contains(new_id));
    assert!(tree.contains(1));

    let Node::Split {
        orientation,
        first,
        second,
        ..
    } = tree.root()
    else {
        panic!("root should be a split: {:?}", tree.root());
    };
    assert_eq!(*orientation, Orientation::Horizontal);
    // Right means NEW pane is the SECOND child (new_is_first=false).
    assert!(matches!(**first, Node::Leaf(1)));
    assert!(matches!(**second, Node::Leaf(id) if id == new_id));
}

#[test]
fn split_left_inserts_new_pane_as_first_child() {
    let mut tree = PaneTree::new();
    let new_id = tree.split(SplitDirection::Left);
    let Node::Split { first, second, .. } = tree.root() else {
        panic!("root should be a split");
    };
    assert!(matches!(**first, Node::Leaf(id) if id == new_id));
    assert!(matches!(**second, Node::Leaf(1)));
}

#[test]
fn split_up_and_down_produce_vertical_orientation() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Down);
    let Node::Split { orientation, .. } = tree.root() else {
        panic!("root should be a split");
    };
    assert_eq!(*orientation, Orientation::Vertical);

    let mut tree2 = PaneTree::new();
    tree2.split(SplitDirection::Up);
    let Node::Split { orientation, .. } = tree2.root() else {
        panic!("root should be a split");
    };
    assert_eq!(*orientation, Orientation::Vertical);
}

#[test]
fn multiple_splits_build_nested_tree() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right); // 1 | 2 (focus 2)
    tree.split(SplitDirection::Down); //  1 | (2 / 3) (focus 3)
    tree.split(SplitDirection::Left); //  1 | ((4|2) / 3) (focus 4)

    assert_eq!(tree.len(), 4);
    assert_eq!(tree.all_panes().len(), 4);
    assert!(tree.contains(1));
    assert!(tree.contains(2));
    assert!(tree.contains(3));
    assert!(tree.contains(4));
}

#[test]
fn pane_ids_never_collide() {
    let mut tree = PaneTree::new();
    let mut seen = std::collections::HashSet::new();
    seen.insert(tree.focused());
    for _ in 0..20 {
        let new_id = tree.split(SplitDirection::Right);
        assert!(seen.insert(new_id), "id {new_id} was reused");
    }
    assert_eq!(tree.len(), 21);
}

#[test]
fn close_focused_last_pane_returns_last_pane_sentinel() {
    let mut tree = PaneTree::new();
    let result = tree.close_focused();
    assert_eq!(result, CloseResult::LastPane);
    // Tree is untouched.
    assert_eq!(tree.len(), 1);
    assert!(tree.contains(1));
}

#[test]
fn close_focused_two_pane_collapses_to_sibling() {
    let mut tree = PaneTree::new();
    let new_id = tree.split(SplitDirection::Right); // focus on new_id
    let result = tree.close_focused();
    assert_eq!(
        result,
        CloseResult::Removed {
            new_focus: 1, // original leaf
        }
    );
    assert_eq!(tree.len(), 1);
    assert!(!tree.contains(new_id));
    assert!(matches!(tree.root(), Node::Leaf(1)));
    assert_eq!(tree.focused(), 1);
}

#[test]
fn close_focused_nested_rebalances_correctly() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right); // 1 | 2, focus 2
    tree.split(SplitDirection::Down); // 1 | (2 / 3), focus 3
    assert_eq!(tree.len(), 3);
    assert_eq!(tree.focused(), 3);

    tree.close_focused();
    assert_eq!(tree.len(), 2);
    assert!(!tree.contains(3));
    assert!(tree.contains(1));
    assert!(tree.contains(2));
    // 2's branch collapsed back to leaf — root should be split(1, 2)
    let Node::Split { first, second, .. } = tree.root() else {
        panic!("root should still be a split");
    };
    assert!(matches!(**first, Node::Leaf(1)));
    assert!(matches!(**second, Node::Leaf(2)));
}

#[test]
fn close_arbitrary_pane_by_id_works() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right); // focus 2
    tree.split(SplitDirection::Down); // focus 3 — tree is 1 | (2 / 3)

    // Close pane 2, leaving 1 | 3
    let result = tree.close(2).unwrap();
    assert!(matches!(result, CloseResult::Removed { .. }));
    assert!(!tree.contains(2));
    assert!(tree.contains(1));
    assert!(tree.contains(3));
    // Focus preserved because 3 was not the one closed.
    assert_eq!(tree.focused(), 3);
}

#[test]
fn close_unknown_pane_returns_not_found() {
    let mut tree = PaneTree::new();
    let err = tree.close(999).unwrap_err();
    assert_eq!(err, PaneTreeError::PaneNotFound(999));
}

#[test]
fn set_focused_moves_focus_and_rejects_unknown_id() {
    let mut tree = PaneTree::new();
    let new_id = tree.split(SplitDirection::Right);
    tree.set_focused(1).unwrap();
    assert_eq!(tree.focused(), 1);
    tree.set_focused(new_id).unwrap();
    assert_eq!(tree.focused(), new_id);

    assert!(matches!(
        tree.set_focused(9999),
        Err(PaneTreeError::PaneNotFound(9999))
    ));
}

#[test]
fn focus_next_cycles_through_panes_in_order() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right); // 1 | 2, focus 2
    tree.split(SplitDirection::Down); // 1 | (2 / 3), focus 3

    let order = tree.all_panes();
    assert_eq!(order.len(), 3);

    tree.set_focused(order[0]).unwrap();
    tree.focus_next();
    assert_eq!(tree.focused(), order[1]);
    tree.focus_next();
    assert_eq!(tree.focused(), order[2]);
    tree.focus_next();
    assert_eq!(tree.focused(), order[0]); // wrap
}

#[test]
fn focus_prev_cycles_backwards() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right);
    tree.split(SplitDirection::Down);

    let order = tree.all_panes();
    tree.set_focused(order[0]).unwrap();
    tree.focus_prev();
    assert_eq!(tree.focused(), order[2]); // wraps to end
    tree.focus_prev();
    assert_eq!(tree.focused(), order[1]);
}

#[test]
fn focus_next_on_single_pane_is_noop() {
    let mut tree = PaneTree::new();
    tree.focus_next();
    assert_eq!(tree.focused(), 1);
    tree.focus_prev();
    assert_eq!(tree.focused(), 1);
}

#[test]
fn close_returns_new_focus_to_a_live_pane_after_subtree_collapse() {
    let mut tree = PaneTree::new();
    tree.split(SplitDirection::Right); // 1 | 2, focus 2
    tree.split(SplitDirection::Down); // 1 | (2 / 3), focus 3
    tree.split(SplitDirection::Right); // 1 | (2 / (3 | 4)), focus 4

    assert_eq!(tree.len(), 4);
    let result = tree.close_focused(); // kill 4
    if let CloseResult::Removed { new_focus } = result {
        assert!(tree.contains(new_focus), "new focus must be live");
    } else {
        panic!("expected Removed result, got {result:?}");
    }
    assert!(!tree.contains(4));
}

#[test]
fn random_split_close_sequence_preserves_invariants() {
    // Deterministic pseudo-random via a simple LCG seed.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rand = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        seed
    };

    for trial in 0..50 {
        let mut tree = PaneTree::new();
        for step in 0..30 {
            let op = rand() % 3;
            match op {
                0 => {
                    let dir = match rand() % 4 {
                        0 => SplitDirection::Left,
                        1 => SplitDirection::Right,
                        2 => SplitDirection::Up,
                        _ => SplitDirection::Down,
                    };
                    tree.split(dir);
                }
                1 => {
                    // Maybe close focused (but don't drop below 1).
                    if tree.len() > 1 {
                        tree.close_focused();
                    }
                }
                _ => {
                    // Refocus to a random existing pane.
                    let panes = tree.all_panes();
                    let target =
                        panes[usize::try_from(rand() % (u64::MAX / 2)).unwrap_or(0) % panes.len()];
                    tree.set_focused(target).unwrap();
                }
            }

            // Invariants checked after every step.
            assert!(
                !tree.is_empty(),
                "trial {trial} step {step}: tree went empty"
            );
            assert!(
                tree.contains(tree.focused()),
                "trial {trial} step {step}: focus {} not in tree",
                tree.focused()
            );
            let panes = tree.all_panes();
            assert_eq!(
                panes.len(),
                tree.len(),
                "trial {trial} step {step}: all_panes().len() != len()"
            );
            let unique: std::collections::HashSet<_> = panes.iter().copied().collect();
            assert_eq!(
                unique.len(),
                panes.len(),
                "trial {trial} step {step}: duplicate pane ids: {panes:?}"
            );
        }
    }
}
