// Issue #400: swap-pane -U / -D must swap by pane INDEX order, not geometry.
//
// The reporter (Arithmomaniac) filed this against the winget 3.3.6 build where
// swap-pane only moved focus.  That core defect was already fixed (commits
// 6bea4b3 / f956d2f make swap_pane exchange the real layout nodes).  This test
// covers the residual bug the reporter flagged in the "Caution" note: target
// selection reused the SPATIAL FocusDir search, so in a horizontal row
// `0 | 1 | 2 | 3` a `swap-pane -U` found nothing "above" and did nothing,
// instead of swapping the active pane with the PREVIOUS pane by index.
//
// tmux (cmd-swap-pane.c): -U swaps with TAILQ_PREV (wrap to LAST), -D swaps
// with TAILQ_NEXT (wrap to FIRST) in the window's pane list, and focus follows
// the originally active pane to its new slot.
//
// These tests build a REAL PTY-backed pane tree and drive the REAL swap_pane.
// Registered from src/window_ops.rs.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, FocusDir, LayoutKind, Node};
use ratatui::layout::Rect;

/// Build a valid Pane wrapping a throwaway PTY, tagged with `id`.
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

/// A single-level horizontal split of `ids.len()` leaf panes, so pane index i
/// (DFS leaf order) has path vec![i] and holds pane id `ids[i]`.
fn app_with_row(ids: &[usize]) -> AppState {
    let mut app = AppState::new("issue400".to_string());
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

/// pane id currently occupying index slot `i` (DFS leaf order).
fn id_at(app: &AppState, i: usize) -> usize {
    crate::tree::get_nth_pane(&app.windows[0].root, i).map(|p| p.id).unwrap_or(usize::MAX)
}

fn active_index(app: &AppState) -> usize {
    crate::tree::pane_index_in_window(&app.windows[0].root, &app.windows[0].active_path).unwrap()
}

// --- POSITIVE: the exact bug scenario from the report (horizontal row) ---

#[test]
fn swap_up_horizontal_row_swaps_with_previous_index() {
    // 0|1|2|3 = ids 49,50,13,36 (mirrors the reporter's %49 %50 %13 %36 table).
    let mut app = app_with_row(&[49, 50, 13, 36]);
    app.windows[0].active_path = vec![1]; // pane index 1 active

    let ok = crate::window_ops::swap_pane(&mut app, FocusDir::Up);

    assert!(ok, "swap-pane -U must actually swap in a horizontal row (was a no-op before #400 fix)");
    // Index 0 and 1 exchange ids; the rest are untouched.
    assert_eq!(id_at(&app, 0), 50, "prev slot (idx0) now holds the moved active pane");
    assert_eq!(id_at(&app, 1), 49, "idx1 now holds the pane that was at idx0");
    assert_eq!(id_at(&app, 2), 13, "idx2 untouched");
    assert_eq!(id_at(&app, 3), 36, "idx3 untouched");
    // Focus follows the originally-active pane (id 50) to its new slot (idx0).
    assert_eq!(active_index(&app), 0, "focus follows the moved pane to the previous slot");
    assert_eq!(id_at(&app, active_index(&app)), 50, "active pane is still id 50");
}

#[test]
fn swap_down_horizontal_row_swaps_with_next_index() {
    let mut app = app_with_row(&[49, 50, 13, 36]);
    app.windows[0].active_path = vec![1]; // pane index 1 active

    let ok = crate::window_ops::swap_pane(&mut app, FocusDir::Down);

    assert!(ok, "swap-pane -D must swap with the next pane by index");
    assert_eq!(id_at(&app, 1), 13, "idx1 now holds the pane that was at idx2");
    assert_eq!(id_at(&app, 2), 50, "next slot (idx2) now holds the moved active pane");
    assert_eq!(active_index(&app), 2, "focus follows the moved pane to the next slot");
    assert_eq!(id_at(&app, active_index(&app)), 50, "active pane is still id 50");
}

// --- WRAP: tmux wraps -U at the first pane to the LAST, -D at the last to FIRST ---

#[test]
fn swap_up_at_first_pane_wraps_to_last() {
    let mut app = app_with_row(&[49, 50, 13, 36]);
    app.windows[0].active_path = vec![0]; // first pane active

    let ok = crate::window_ops::swap_pane(&mut app, FocusDir::Up);

    assert!(ok, "swap-pane -U at the first pane must wrap to the last");
    assert_eq!(id_at(&app, 0), 36, "idx0 now holds the last pane (id 36)");
    assert_eq!(id_at(&app, 3), 49, "idx3 (last) now holds the moved active pane (id 49)");
    assert_eq!(active_index(&app), 3, "focus follows the moved pane to the last slot");
}

#[test]
fn swap_down_at_last_pane_wraps_to_first() {
    let mut app = app_with_row(&[49, 50, 13, 36]);
    app.windows[0].active_path = vec![3]; // last pane active

    let ok = crate::window_ops::swap_pane(&mut app, FocusDir::Down);

    assert!(ok, "swap-pane -D at the last pane must wrap to the first");
    assert_eq!(id_at(&app, 3), 49, "idx3 now holds the first pane (id 49)");
    assert_eq!(id_at(&app, 0), 36, "idx0 (first) now holds the moved active pane (id 36)");
    assert_eq!(active_index(&app), 0, "focus follows the moved pane to the first slot");
}

// --- EDGE: a single-pane window has nothing to swap with (no panic, no-op) ---

#[test]
fn swap_single_pane_is_noop() {
    let mut app = app_with_row(&[49]);
    app.windows[0].active_path = vec![0];

    let up = crate::window_ops::swap_pane(&mut app, FocusDir::Up);
    let down = crate::window_ops::swap_pane(&mut app, FocusDir::Down);

    assert!(!up, "swap-pane -U on a lone pane must be a no-op");
    assert!(!down, "swap-pane -D on a lone pane must be a no-op");
    assert_eq!(id_at(&app, 0), 49, "the lone pane stays put");
}

// --- REGRESSION GUARD: the old geometry behaviour (do-nothing in a row) is gone ---

#[test]
fn horizontal_row_swap_is_no_longer_geometry_gated() {
    // Before the fix, spatial FocusDir::Up found no pane "above" in a single
    // horizontal row and swap_pane returned false. Prove it now returns true
    // and the id mapping genuinely changed.
    let before: Vec<usize> = (0..4).map(|_| 0).collect();
    let mut app = app_with_row(&[49, 50, 13, 36]);
    app.windows[0].active_path = vec![2];
    let snapshot: Vec<usize> = (0..4).map(|i| id_at(&app, i)).collect();
    assert_ne!(snapshot, before); // sanity: distinct ids

    let ok = crate::window_ops::swap_pane(&mut app, FocusDir::Up);
    let after: Vec<usize> = (0..4).map(|i| id_at(&app, i)).collect();

    assert!(ok, "BUG #400: swap-pane -U in a horizontal row must not be a no-op");
    assert_ne!(snapshot, after, "the pane_index -> pane_id mapping must actually change");
    assert_eq!(after[1], 13, "idx1 got the previously-active pane (id 13)");
    assert_eq!(after[2], 50, "idx2 got its previous neighbour (id 50)");
}
