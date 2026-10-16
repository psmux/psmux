use super::*;
use std::collections::HashSet;

fn peek_and_commit(ids: &mut ServerIds) -> usize {
    let id = ids.peek_pane();
    ids.commit_pane(id);
    id
}

#[test]
fn a_fresh_app_hands_out_the_first_ids_as_today() {
    let mut app = AppState::new("server_ids".to_string());
    assert_eq!(app.ids.alloc_window(), 1);
    assert_eq!(app.ids.alloc_pane(), 1);
    assert_eq!(app.ids.alloc_forward(), 1);
}

#[test]
fn peek_does_not_consume_an_id() {
    let mut ids = ServerIds::default();
    assert_eq!(ids.peek_pane(), 1);
    assert_eq!(ids.peek_pane(), 1);
    let id = ids.peek_pane();
    ids.commit_pane(id);
    assert_eq!(ids.peek_pane(), 2);
    assert_eq!(ids.alloc_pane(), 2);
    assert_eq!(ids.peek_pane(), 3);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic]
fn committing_a_stale_peek_panics_in_debug_builds() {
    let mut ids = ServerIds::default();
    let stale = ids.peek_pane();
    ids.alloc_pane();
    ids.commit_pane(stale);
}

#[test]
fn interleaved_allocations_never_repeat() {
    let mut ids = ServerIds::default();
    let mut windows = HashSet::new();
    let mut panes = HashSet::new();
    let mut forwards = HashSet::new();
    for i in 0..1000 {
        match i % 4 {
            0 => assert!(windows.insert(ids.alloc_window())),
            1 => assert!(panes.insert(ids.alloc_pane())),
            2 => assert!(panes.insert(peek_and_commit(&mut ids))),
            _ => assert!(forwards.insert(ids.alloc_forward())),
        }
    }
    assert_eq!(windows.len(), 250);
    assert_eq!(panes.len(), 500);
    assert_eq!(forwards.len(), 250);
}
