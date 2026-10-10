// Issue #725: after join-pane pulled a pane into the current window, no pane
// had keyboard focus. Typed keys reached no pane until a pane was clicked or
// selected by index.
//
// Root cause: the graft (tree::replace_leaf_with_split) turns the target leaf
// into a split, and the target window's active_path, which named that leaf,
// was never updated. It then named the new SPLIT. Every input path walks the
// path to a Pane (tree::active_pane_mut) and got None, while
// tree::get_active_pane_id falls back to the first child of a split, so
// #{pane_active} kept reporting the original pane and hid the defect. Any
// later command that saved and restored the focus by pane id re-anchored the
// path, which is why a one shot CLI join often looked fine and a join typed at
// the command prompt lost the next line of input.
//
// tmux: the joined pane becomes active unless -d (cmd-join-pane.c:515 to 517,
// window_set_active_pane inside if (!args_has(args, 'd'))), and -b grafts
// the pane before the target (SPAWN_BEFORE, cmd-join-pane.c:478).
//
// Both copies of join-pane (the server request and the embedded fallback) now
// run commands::join_pane_local, which grafts through tree::graft_pane, and
// graft_pane always leaves active_path on a leaf. Registered from
// src/commands.rs.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    let child = crate::util::StubChild::exited();
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize, name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: Rect::new(0, 0, 160, 40),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// A session of `windows.len()` windows, window i holding the pane ids given.
/// Window display indices are 0, 1, 2 and so on, and pane-base-index is 0.
fn app_with_windows(windows: &[&[usize]]) -> AppState {
    let mut app = AppState::new("issue725".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    app.client_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    let mut next_id = 1usize;
    for (w, ids) in windows.iter().enumerate() {
        let mut win = make_window(w, &format!("win{w}"));
        let n = ids.len().max(1);
        if ids.len() == 1 {
            win.root = Node::Leaf(make_pane(ids[0], 40, 160));
            win.active_path = vec![];
        } else {
            let children: Vec<Node> = ids.iter()
                .map(|&id| Node::Leaf(make_pane(id, 40, 160 / n as u16)))
                .collect();
            win.root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![(100 / n) as u16; n], children };
            win.active_path = vec![0];
        }
        win.pane_mru = ids.to_vec();
        app.windows.push(win);
        app.window_indices.push(w);
        next_id = next_id.max(w + 1);
    }
    app.next_win_id = next_id;
    app.active_idx = 0;
    app
}


fn ids_in(app: &AppState, w: usize) -> Vec<usize> {
    crate::tree::collect_pane_ids(&app.windows[w].root)
}

/// The pane `active_path` names, and ONLY if it names a leaf. This is the
/// walk every input path does (`active_pane_mut`); `get_active_pane_id` would
/// fall back to the first child of a split and hide the defect, exactly as
/// `#{pane_active}` did in the report.
fn focused(app: &AppState, w: usize) -> Option<usize> {
    let win = &app.windows[w];
    if !crate::tree::active_path_is_leaf(win) { return None; }
    crate::tree::active_pane(&win.root, &win.active_path).map(|p| p.id)
}

fn join(app: &mut AppState, src: (usize, Option<usize>), dst: (Option<usize>, Option<usize>), horizontal: bool, detach: bool, before: bool) -> bool {
    crate::commands::join_pane_local(app, Some(src.0), src.1, dst.0, dst.1, horizontal, detach, before)
}

// =========================================================================
// The reporter's repro: a one pane window, join the pane of window 1 into it.
// =========================================================================

#[test]
fn join_into_single_pane_window_focuses_the_joined_pane() {
    let mut app = app_with_windows(&[&[1], &[2], &[3]]);
    assert!(join(&mut app, (1, None), (Some(0), None), true, false, false));
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
    assert!(crate::tree::active_path_is_leaf(&app.windows[0]),
        "active_path {:?} must name a pane, not the split the graft created", app.windows[0].active_path);
    assert_eq!(focused(&app, 0), Some(2), "tmux: the joined pane becomes active (window_set_active_pane)");
    assert_eq!(app.active_idx, 0);
    assert_eq!(app.last_pane_path, vec![0], "select-pane -l goes back to the pane that was active");
    assert_eq!(app.windows[0].pane_mru.first(), Some(&2), "the joined pane is the most recent");
}

#[test]
fn join_detached_keeps_the_original_pane_focused() {
    let mut app = app_with_windows(&[&[1], &[2], &[3]]);
    assert!(join(&mut app, (1, None), (Some(0), None), true, true, false));
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
    assert!(crate::tree::active_path_is_leaf(&app.windows[0]), "path {:?}", app.windows[0].active_path);
    assert_eq!(focused(&app, 0), Some(1), "-d leaves the active pane alone");
}

#[test]
fn join_vertical_focuses_the_joined_pane() {
    let mut app = app_with_windows(&[&[1], &[2]]);
    assert!(join(&mut app, (1, None), (Some(0), None), false, false, false));
    assert!(matches!(app.windows[0].root, Node::Split { kind: LayoutKind::Vertical, .. }));
    assert_eq!(focused(&app, 0), Some(2));
}

#[test]
fn join_before_puts_the_pane_first_and_focuses_it() {
    // cmd-join-pane.c SPAWN_BEFORE: -b grafts left of / above the target.
    let mut app = app_with_windows(&[&[1], &[2]]);
    assert!(join(&mut app, (1, None), (Some(0), None), true, false, true));
    assert_eq!(ids_in(&app, 0), vec![2, 1], "-b puts the joined pane first");
    assert_eq!(focused(&app, 0), Some(2));
    assert_eq!(app.last_pane_path, vec![1]);
}

#[test]
fn join_before_detached_keeps_the_original_pane_focused() {
    let mut app = app_with_windows(&[&[1], &[2]]);
    assert!(join(&mut app, (1, None), (Some(0), None), true, true, true));
    assert_eq!(ids_in(&app, 0), vec![2, 1]);
    assert_eq!(focused(&app, 0), Some(1), "the original moved to index 1 and must still be the active one");
}

#[test]
fn join_into_a_nested_active_pane_lands_on_a_leaf() {
    // The graft one level down: window 0 is [1 | 2] with 2 active.
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    app.windows[0].active_path = vec![1];
    assert!(join(&mut app, (1, None), (Some(0), None), false, false, false));
    assert_eq!(ids_in(&app, 0), vec![1, 2, 3]);
    assert_eq!(app.windows[0].active_path, vec![1, 1]);
    assert_eq!(focused(&app, 0), Some(3));
    assert_eq!(app.last_pane_path, vec![1, 0], "pane 2 moved one level down");
}

#[test]
fn join_beside_a_non_active_target_pane_detached_keeps_the_active_pane() {
    // -t :0.0 while pane 2 (index 1) is active: the graft restructures a
    // DIFFERENT branch, and the active pane must still be pane 2.
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    app.windows[0].active_path = vec![1];
    assert!(join(&mut app, (1, None), (Some(0), Some(0)), false, true, false));
    assert_eq!(ids_in(&app, 0), vec![1, 3, 2]);
    assert_eq!(focused(&app, 0), Some(2));
}

#[test]
fn join_beside_a_non_active_target_pane_focuses_the_joined_pane() {
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    app.windows[0].active_path = vec![1];
    assert!(join(&mut app, (1, None), (Some(0), Some(0)), false, false, false));
    assert_eq!(focused(&app, 0), Some(3));
    assert_eq!(app.last_pane_path, vec![1], "the pane that was active before");
}

#[test]
fn join_into_a_window_that_is_not_current_switches_and_focuses() {
    // join-pane -s :1 -t :2 from window 0 (cmd-join-pane.c:515 to 517).
    let mut app = app_with_windows(&[&[1], &[2], &[3]]);
    assert!(join(&mut app, (1, None), (Some(2), None), true, false, false));
    // Window 1 emptied and was removed, so the target is now at position 1.
    assert_eq!(app.windows.len(), 2);
    assert_eq!(ids_in(&app, 1), vec![3, 2]);
    assert_eq!(app.active_idx, 1);
    assert_eq!(focused(&app, 1), Some(2));
    assert_eq!(focused(&app, 0), Some(1), "the window left behind keeps a focused pane");
}

#[test]
fn join_detached_into_a_window_that_is_not_current() {
    let mut app = app_with_windows(&[&[1], &[2], &[3]]);
    assert!(join(&mut app, (1, None), (Some(2), None), true, true, false));
    assert_eq!(app.active_idx, 0, "-d does not switch");
    assert_eq!(ids_in(&app, 1), vec![3, 2]);
    assert_eq!(focused(&app, 1), Some(3));
}

#[test]
fn source_window_keeps_a_focused_leaf() {
    let mut app = app_with_windows(&[&[1], &[2, 3]]);
    app.windows[1].active_path = vec![1];
    assert!(join(&mut app, (1, Some(1)), (Some(0), None), true, false, false));
    assert_eq!(ids_in(&app, 1), vec![2]);
    assert_eq!(focused(&app, 1), Some(2));
    assert_eq!(focused(&app, 0), Some(3));
}

#[test]
fn failed_joins_change_nothing() {
    let mut app = app_with_windows(&[&[1], &[2]]);
    assert!(!join(&mut app, (0, None), (Some(0), None), true, false, false), "own window");
    assert!(!join(&mut app, (7, None), (Some(0), None), true, false, false), "missing source");
    assert!(!join(&mut app, (1, None), (Some(9), None), true, false, false), "missing target");
    assert_eq!(ids_in(&app, 0), vec![1]);
    assert_eq!(ids_in(&app, 1), vec![2]);
    assert_eq!(focused(&app, 0), Some(1));
}

// =========================================================================
// graft_pane itself, the shape the cross session join (move a pane in from
// another server) uses: it always focuses the new pane.
// =========================================================================

#[test]
fn graft_pane_returns_both_paths_and_anchors_on_a_leaf() {
    let mut app = app_with_windows(&[&[1]]);
    let path = app.windows[0].active_path.clone();
    let g = crate::tree::graft_pane(&mut app.windows[0], &path, LayoutKind::Horizontal,
        Node::Leaf(make_pane(9, 40, 80)), false, true).expect("grafted");
    assert_eq!(g.new_path, vec![1]);
    assert_eq!(g.prev_active_path, Some(vec![0]));
    assert_eq!(focused(&app, 0), Some(9));
}

#[test]
fn graft_pane_on_an_invalid_path_leaves_focus_alone() {
    let mut app = app_with_windows(&[&[1, 2]]);
    app.windows[0].active_path = vec![1];
    // [0, 0] walks THROUGH leaf 1: replace_leaf_with_split refuses it.
    assert!(crate::tree::graft_pane(&mut app.windows[0], &vec![0, 0], LayoutKind::Horizontal,
        Node::Leaf(make_pane(9, 40, 80)), false, true).is_none());
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
    assert_eq!(focused(&app, 0), Some(2));
}

// =========================================================================
// The other commands that restructure a window: each must leave a leaf.
// =========================================================================

#[test]
fn break_pane_leaves_both_windows_on_a_leaf() {
    for detach in [false, true] {
        let mut app = app_with_windows(&[&[1, 2]]);
        app.windows[0].active_path = vec![1];
        let req = crate::window_ops::BreakPaneRequest { detach, ..Default::default() };
        crate::window_ops::break_pane(&mut app, &req).expect("breaks");
        assert_eq!(focused(&app, 0), Some(1), "detach={detach}");
        assert_eq!(focused(&app, 1), Some(2), "detach={detach}");
    }
}

#[test]
fn swap_pane_across_windows_leaves_both_windows_on_a_leaf() {
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    app.windows[0].active_path = vec![1];
    crate::window_ops::swap_pane_by_spec(&mut app, Some("%3"), "%2", false).expect("resolves");
    assert!(crate::tree::active_path_is_leaf(&app.windows[0]));
    assert!(crate::tree::active_path_is_leaf(&app.windows[1]));
    assert!(focused(&app, 0).is_some() && focused(&app, 1).is_some());
}
