//! Tests for Workspace + Tab (pure-logic layer, no GTK).

use host_core::pane_tree::SplitDirection;
use host_core::workspace::{Workspace, WorkspaceError};

#[test]
fn new_workspace_has_one_tab_with_one_pane() {
    let ws = Workspace::new();
    assert_eq!(ws.len(), 1);
    assert_eq!(ws.active_index(), 0);
    assert_eq!(ws.active_tab().title(), "shell");
    assert_eq!(ws.active_tab().tree().len(), 1);
}

#[test]
fn new_tab_appends_and_activates() {
    let mut ws = Workspace::new();
    let new_id = ws.new_tab("build");
    assert_eq!(ws.len(), 2);
    assert_eq!(ws.active_index(), 1);
    assert_eq!(ws.active_tab().id(), new_id);
    assert_eq!(ws.active_tab().title(), "build");
}

#[test]
fn close_last_tab_is_rejected() {
    let mut ws = Workspace::new();
    let err = ws.close_active_tab().unwrap_err();
    assert_eq!(err, WorkspaceError::CannotCloseLastTab);
    assert_eq!(ws.len(), 1);
}

#[test]
fn close_middle_tab_shifts_active_correctly() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    ws.new_tab("tab3");
    assert_eq!(ws.len(), 3);
    assert_eq!(ws.active_index(), 2);

    ws.select_tab(1).unwrap();
    assert_eq!(ws.active_tab().title(), "tab2");

    // Close the middle tab (which is now active)
    let middle_id = ws.active_tab().id();
    ws.close_tab(middle_id).unwrap();
    assert_eq!(ws.len(), 2);
    // Active should now be the tab that was AFTER the closed one,
    // but at the same index - since removing shrinks the vec, idx 1
    // now points to what was idx 2. Our close logic keeps
    // active_index at its prior slot and clamps if needed, so
    // active is now at index 0 (because we decremented).
    // Actually: we closed idx 1 (which equals active_index); we
    // decrement active_index because idx == active_index && > 0.
    assert_eq!(ws.active_index(), 0);
}

#[test]
fn close_tab_before_active_shifts_active_back() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    ws.new_tab("tab3");
    ws.select_tab(2).unwrap();
    assert_eq!(ws.active_index(), 2);

    let first_id = ws.tabs()[0].id();
    ws.close_tab(first_id).unwrap();
    assert_eq!(ws.len(), 2);
    // Active was idx 2; removed idx 0; shift down by 1 → idx 1.
    assert_eq!(ws.active_index(), 1);
    assert_eq!(ws.active_tab().title(), "tab3");
}

#[test]
fn close_tab_after_active_does_not_shift() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    ws.new_tab("tab3");
    ws.select_tab(0).unwrap();
    let last_id = ws.tabs()[2].id();
    ws.close_tab(last_id).unwrap();
    assert_eq!(ws.active_index(), 0);
    assert_eq!(ws.len(), 2);
}

#[test]
fn close_unknown_tab_returns_error() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    let err = ws.close_tab(9999).unwrap_err();
    assert_eq!(err, WorkspaceError::TabNotFound(9999));
}

#[test]
fn select_tab_out_of_range() {
    let mut ws = Workspace::new();
    let err = ws.select_tab(5).unwrap_err();
    assert_eq!(err, WorkspaceError::IndexOutOfRange { index: 5, len: 1 });
}

#[test]
fn move_tab_forward() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    ws.new_tab("tab3");
    // Active at idx 2 ("tab3"). Move tab 0 ("shell") to position 2.
    let shell_id = ws.tabs()[0].id();
    ws.move_tab(0, 2).unwrap();
    // Order now: tab2, tab3, shell
    assert_eq!(ws.tabs()[0].title(), "tab2");
    assert_eq!(ws.tabs()[1].title(), "tab3");
    assert_eq!(ws.tabs()[2].id(), shell_id);
    // Active was "tab3" at idx 2, still "tab3" at idx 1 now.
    assert_eq!(ws.active_tab().title(), "tab3");
    assert_eq!(ws.active_index(), 1);
}

#[test]
fn move_tab_backward_tracks_active() {
    let mut ws = Workspace::new();
    ws.new_tab("tab2");
    ws.new_tab("tab3");
    ws.select_tab(0).unwrap();
    // Active at idx 0 ("shell"). Move tab 2 ("tab3") to position 0.
    ws.move_tab(2, 0).unwrap();
    // Order now: tab3, shell, tab2
    assert_eq!(ws.tabs()[0].title(), "tab3");
    assert_eq!(ws.tabs()[1].title(), "shell");
    assert_eq!(ws.tabs()[2].title(), "tab2");
    // Active was "shell" at idx 0, now at idx 1.
    assert_eq!(ws.active_tab().title(), "shell");
    assert_eq!(ws.active_index(), 1);
}

#[test]
fn each_tab_has_its_own_pane_tree() {
    let mut ws = Workspace::new();
    ws.active_tab_mut().tree_mut().split(SplitDirection::Right);
    assert_eq!(ws.active_tab().tree().len(), 2);

    ws.new_tab("tab2");
    assert_eq!(ws.active_tab().tree().len(), 1);
    // First tab's tree is unchanged.
    assert_eq!(ws.tabs()[0].tree().len(), 2);
}

#[test]
fn tab_title_can_be_updated() {
    let mut ws = Workspace::new();
    ws.active_tab_mut().set_title("vim");
    assert_eq!(ws.active_tab().title(), "vim");
}

#[test]
fn tab_ids_never_collide() {
    let mut ws = Workspace::new();
    let mut seen = std::collections::HashSet::new();
    seen.insert(ws.active_tab().id());
    for i in 0..10 {
        let id = ws.new_tab(format!("tab{i}"));
        assert!(seen.insert(id), "tab id {id} was reused");
    }
}
