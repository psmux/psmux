// Issue #442: swap-pane -s X -t Y must swap the two named panes, honoring -d.
//
// The bug: -s was dropped by the command parser, so `swap-pane -s X -t Y`
// swapped the ACTIVE pane with Y and left X untouched. Fix adds a source
// carrying control request and swap_pane_between(), matching tmux
// cmd-swap-pane.c: without -d the -t pane becomes active (it lands in the src
// slot after the exchange); with -d the previously active pane keeps focus.
//
// These tests drive the REAL swap_pane_between on a real PTY backed pane tree.
// Registered from src/window_ops.rs.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = pair.slave.spawn_command(cmd).expect("spawn dummy");
    let writer = pair.master.take_writer().expect("writer");
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: pair.master,
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
        mouse_input_cache: None,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
    }
}

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
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

fn app_with_row(ids: &[usize]) -> AppState {
    let mut app = AppState::new("issue442".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    let mut win = make_window(0);
    let n = ids.len();
    let children: Vec<Node> = ids.iter().map(|&id| Node::Leaf(make_pane(id, 40, 160 / n as u16))).collect();
    let sizes = vec![(100 / n) as u16; n];
    win.root = Node::Split { kind: LayoutKind::Horizontal, sizes, children };
    app.windows.push(win);
    app.active_idx = 0;
    app
}

fn id_at(app: &AppState, i: usize) -> usize {
    crate::tree::get_nth_pane(&app.windows[0].root, i).map(|p| p.id).unwrap_or(usize::MAX)
}
fn active_index(app: &AppState) -> usize {
    crate::tree::pane_index_in_window(&app.windows[0].root, &app.windows[0].active_path).unwrap()
}
fn active_id(app: &AppState) -> usize {
    crate::tree::get_active_pane_id(&app.windows[0].root, &app.windows[0].active_path).unwrap()
}

// --- POSITIVE: swap two panes neither of which is the active one (the bug) ---

#[test]
fn swap_two_explicit_panes_leaves_the_named_panes_swapped() {
    // ids 2,4,5,6 at idx 0,1,2,3; active is idx1 (id 4), a third party.
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![1];
    let src = vec![0]; // -s = idx0 (id 2)
    let dst = vec![3]; // -t = idx3 (id 6)

    let ok = crate::window_ops::swap_pane_between(&mut app, src, dst, false);

    assert!(ok, "swap-pane -s -t must swap the two named panes");
    // The two NAMED panes exchange slots; the active pane's slot content is
    // NOT the thing being swapped (that was the bug).
    assert_eq!(id_at(&app, 0), 6, "idx0 now holds the -t pane (id 6)");
    assert_eq!(id_at(&app, 3), 2, "idx3 now holds the -s pane (id 2)");
    assert_eq!(id_at(&app, 1), 4, "idx1 (previously active) is untouched");
    assert_eq!(id_at(&app, 2), 5, "idx2 untouched");
}

#[test]
fn without_detach_the_t_pane_becomes_active() {
    // tmux: without -d, active becomes the -t (dst) pane, now at the src slot.
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![1];
    crate::window_ops::swap_pane_between(&mut app, vec![0], vec![3], false);

    assert_eq!(active_id(&app), 6, "active follows the -t pane (id 6)");
    assert_eq!(active_index(&app), 0, "the -t pane now sits in the src slot (idx0)");
}

// --- REGRESSION GUARD: the exact bug scenario cannot recur ---

#[test]
fn active_pane_is_not_the_implicit_source() {
    // Before the fix, -s was ignored and the ACTIVE pane (id 4) got swapped
    // with -t (id 6). Prove id 4 stays put and id 2 (the real -s) moves.
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![1]; // active = id 4
    crate::window_ops::swap_pane_between(&mut app, vec![0], vec![3], false);

    // id 4 must be exactly where it was; the bug would have moved it to idx3.
    let idx_of_4 = (0..4).find(|&i| id_at(&app, i) == 4).unwrap();
    assert_eq!(idx_of_4, 1, "BUG #442: the active pane must NOT be the implicit source");
}

// --- -d (detach): active pane unchanged when it is not one of the swapped ---

#[test]
fn detach_keeps_focus_on_untouched_active_pane() {
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![1]; // active = id 4, not swapped
    crate::window_ops::swap_pane_between(&mut app, vec![0], vec![3], true);

    assert_eq!(active_id(&app), 4, "-d keeps the same pane focused");
    assert_eq!(active_index(&app), 1, "id 4 never moved, so focus stays at idx1");
    assert_eq!(id_at(&app, 0), 6, "the named panes still swapped");
    assert_eq!(id_at(&app, 3), 2, "the named panes still swapped");
}

// --- -d where the active pane IS one of the swapped: focus follows it ---

#[test]
fn detach_follows_active_pane_when_it_is_swapped() {
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![0]; // active = id 2 == the -s pane
    crate::window_ops::swap_pane_between(&mut app, vec![0], vec![3], true);

    assert_eq!(active_id(&app), 2, "-d keeps the same pane (id 2) focused");
    assert_eq!(active_index(&app), 3, "id 2 moved to idx3, focus follows it there");
}

// --- EDGE: swapping a pane with itself is a no-op ---

#[test]
fn swap_same_path_is_noop() {
    let mut app = app_with_row(&[2, 4, 5, 6]);
    app.windows[0].active_path = vec![1];
    let ok = crate::window_ops::swap_pane_between(&mut app, vec![2], vec![2], false);
    assert!(!ok, "swapping a pane with itself must be a no-op");
    assert_eq!(id_at(&app, 2), 5, "layout unchanged");
}
