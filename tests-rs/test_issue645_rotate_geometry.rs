// Issue #645: rotate-window moved the panes between slots but left every pane
// at the size of the slot it had just LEFT.
//
// Measured on 3.3.8 (7f67a71), `psmux -L ns645` in an isolated data dir:
//
//     psmux new-session -d -s q -x 200 -y 50 cmd.exe
//     psmux split-window -t q:0.0 -l 2 cmd.exe
//     psmux rotate-window -U -t q:0
//     psmux list-panes -t q:0 -F "#{pane_index} h=#{pane_height} top=#{pane_top} bot=#{pane_bottom}"
//
//     after split   0 h=47 top=0  bot=46      0 id=%1
//                   1 h=2  top=48 bot=49      1 id=%4
//     after rotate  0 h=2  top=0  bot=46      0 id=%4   <-- h belongs to the old slot
//                   1 h=47 top=48 bot=49      1 id=%1
//
// `window_layout` came back `1a2b,200x50,0,0[200x47,0,0,4,200x2,0,48,1]`, so the
// CELLS were right and only the panes' own sizes were stale: pane_top/bottom and
// the layout read the tree, pane_height/pane_width read `Pane::last_rows` /
// `last_cols`, which are written only when something resizes the PTY.  Knock on
// effects measured with the same rig: `split-window -t q:0.0 -l 5` was refused
// with "pane too small to split vertically (2 rows, need 5)", and `mode con` in
// the pane that now filled 47 rows still reported a 2 line console.
//
// tmux is the specification (cmd-rotate-window.c:90-103):
//
//     TAILQ_FOREACH_REVERSE(wp, &w->panes, window_panes, entry) {
//         if ((wp2 = TAILQ_PREV(wp, window_panes, entry)) == NULL) break;
//         wp->layout_cell = wp2->layout_cell;
//         wp->xoff = wp2->xoff; wp->yoff = wp2->yoff;
//         window_pane_resize(wp, wp2->sx, wp2->sy);
//     }
//
// Two invariants come out of that and both are asserted below:
//   * the layout tree is never touched, so every cell keeps its size and the
//     window's shape is identical before and after (psmux used to rotate the
//     ROOT SPLIT'S CHILDREN, which dragged whole subtrees into cells sized for
//     something else);
//   * every moved pane is resized to the cell it landed in, PTY included.
//
// Registered from src/window_ops.rs.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

const AREA: Rect = Rect { x: 0, y: 0, width: 200, height: 50 };

/// A valid Pane wrapping a throwaway PTY, tagged with `id`. The PTY is real so
/// `resize_all_panes` exercises the same resize path the server takes.
fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = crate::util::spawn_pty_child(&*pair.slave, cmd).expect("spawn dummy");
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
        mouse_input_cache: None, win32_input_latched: false,
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

fn make_window() -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: AREA,
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

fn empty_app() -> AppState {
    let mut app = AppState::new("issue645".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = AREA;
    app
}

/// A single-level split of `ids.len()` leaves, so slot i has path vec![i].
/// `sizes` are the split's stored sizes, in the same units the layout keeps.
fn app_with_split(kind: LayoutKind, ids: &[usize], sizes: &[u16]) -> AppState {
    let mut app = empty_app();
    let mut win = make_window();
    let children: Vec<Node> = ids.iter().map(|&id| Node::Leaf(make_pane(id, 10, 10))).collect();
    win.root = Node::Split { kind, sizes: sizes.to_vec(), children };
    app.windows.push(win);
    app.active_idx = 0;
    // Start from a consistent state: every pane already sized to its own cell,
    // exactly as the server leaves things after a split.
    crate::tree::resize_all_panes(&mut app);
    app
}

/// ids currently occupying the slots, in DFS leaf order.
fn ids(app: &AppState) -> Vec<usize> {
    crate::tree::collect_pane_ids(&app.windows[0].root)
}

/// (path, rect) for every slot, in DFS leaf order.
fn cells(app: &AppState) -> Vec<(Vec<usize>, Rect)> {
    let mut out = Vec::new();
    crate::tree::compute_rects(&app.windows[0].root, app.windows[0].area, &mut out);
    out
}

/// (last_rows, last_cols) for every pane, in DFS leaf order.
fn sizes_of_panes(app: &AppState) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    fn rec(n: &Node, out: &mut Vec<(u16, u16)>) {
        match n {
            Node::Leaf(p) => out.push((p.last_rows, p.last_cols)),
            Node::Split { children, .. } => { for c in children { rec(c, out); } }
        }
    }
    rec(&app.windows[0].root, &mut out);
    out
}

/// The whole point of #645: after any rotate, the pane sitting in a cell must
/// carry that cell's size. `resize_window_panes` clamps to MIN_PANE_DIM, so the
/// expectation is the cell size with the same clamp applied.
fn assert_panes_match_their_cells(app: &AppState, what: &str) {
    let cells = cells(app);
    let sizes = sizes_of_panes(app);
    assert_eq!(cells.len(), sizes.len(), "{what}: one size per cell");
    for (i, ((path, rect), (rows, cols))) in cells.iter().zip(sizes.iter()).enumerate() {
        let want_rows = rect.height.max(crate::pane::MIN_PANE_DIM);
        let want_cols = rect.width.max(crate::pane::MIN_PANE_DIM);
        assert_eq!(
            (*rows, *cols), (want_rows, want_cols),
            "{what}: slot {i} at path {path:?} is {}x{} but its pane reports {}x{}",
            rect.width, rect.height, cols, rows
        );
    }
}

/// The layout tree's shape: every split's kind, sizes and child count, in DFS
/// order. tmux never touches this during a rotate.
fn shape(app: &AppState) -> Vec<(bool, Vec<u16>, usize)> {
    let mut out = Vec::new();
    fn rec(n: &Node, out: &mut Vec<(bool, Vec<u16>, usize)>) {
        if let Node::Split { kind, sizes, children } = n {
            out.push((matches!(kind, LayoutKind::Horizontal), sizes.clone(), children.len()));
            for c in children { rec(c, out); }
        }
    }
    rec(&app.windows[0].root, &mut out);
    out
}

// ── The reporter's rig: a 47/2 vertical column ────────────────────────────

#[test]
fn rotate_up_resizes_every_pane_to_the_cell_it_landed_in() {
    // The reporter's `-l 2` split: a tall pane over a two row pane.
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 4], &[47, 2]);
    let before_cells: Vec<Rect> = cells(&app).into_iter().map(|(_, r)| r).collect();
    assert_eq!(sizes_of_panes(&app)[0].0, before_cells[0].height, "rig: pane %1 starts tall");
    assert_eq!(sizes_of_panes(&app)[1].0, before_cells[1].height, "rig: pane %4 starts short");

    crate::window_ops::rotate_panes(&mut app, true);

    // tmux -U: the pane in cell 0 goes to the last cell, everyone moves up one.
    assert_eq!(ids(&app), vec![4, 1], "-U moves the second pane into the first cell");
    // The cells themselves did not move (this is what pane_top/bottom and
    // window_layout report, and they were already right before the fix).
    let after_cells: Vec<Rect> = cells(&app).into_iter().map(|(_, r)| r).collect();
    assert_eq!(after_cells, before_cells, "rotate must not move or resize any cell");
    // ...and this is the bug: the sizes used to stay with the pane.
    assert_panes_match_their_cells(&app, "after rotate -U");
    assert_eq!(
        sizes_of_panes(&app)[0].0, before_cells[0].height,
        "the pane now in the tall cell must report the TALL height, not the 2 rows it had"
    );
}

#[test]
fn rotate_down_resizes_every_pane_to_the_cell_it_landed_in() {
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 4], &[47, 2]);

    crate::window_ops::rotate_panes(&mut app, false);

    assert_eq!(ids(&app), vec![4, 1], "with two panes -D is the same permutation as -U");
    assert_panes_match_their_cells(&app, "after rotate -D");
}

// ── Direction, on three panes where -U and -D differ ───────────────────────

#[test]
fn rotate_up_moves_each_pane_one_cell_towards_the_front() {
    // tmux -U: [A,B,C] -> [B,C,A].
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 2, 3], &[34, 4, 10]);

    crate::window_ops::rotate_panes(&mut app, true);

    assert_eq!(ids(&app), vec![2, 3, 1], "-U is tmux's first-pane-to-the-last-cell");
    assert_panes_match_their_cells(&app, "three panes, -U");
}

#[test]
fn rotate_down_moves_each_pane_one_cell_towards_the_back() {
    // tmux -D: [A,B,C] -> [C,A,B].
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 2, 3], &[34, 4, 10]);

    crate::window_ops::rotate_panes(&mut app, false);

    assert_eq!(ids(&app), vec![3, 1, 2], "-D is tmux's last-pane-to-the-first-cell");
    assert_panes_match_their_cells(&app, "three panes, -D");
}

#[test]
fn a_full_cycle_of_rotations_restores_the_original_arrangement() {
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 2, 3], &[34, 4, 10]);
    let start_ids = ids(&app);
    let start_sizes = sizes_of_panes(&app);
    let start_shape = shape(&app);

    for _ in 0..3 { crate::window_ops::rotate_panes(&mut app, true); }

    assert_eq!(ids(&app), start_ids, "three -U rotations of three panes is the identity");
    assert_eq!(sizes_of_panes(&app), start_sizes, "and every pane is back at its own size");
    assert_eq!(shape(&app), start_shape, "and the layout never drifted");
}

#[test]
fn rotate_up_then_down_is_the_identity() {
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 2, 3], &[34, 4, 10]);
    let start_ids = ids(&app);
    let start_sizes = sizes_of_panes(&app);

    crate::window_ops::rotate_panes(&mut app, true);
    crate::window_ops::rotate_panes(&mut app, false);

    assert_eq!(ids(&app), start_ids, "-D undoes -U");
    assert_eq!(sizes_of_panes(&app), start_sizes, "and the sizes come back with them");
}

// ── Width has the same defect on a horizontal row ──────────────────────────

#[test]
fn rotate_carries_width_to_the_new_cell_on_a_horizontal_row() {
    let mut app = app_with_split(LayoutKind::Horizontal, &[1, 4], &[179, 20]);
    let before_cells: Vec<Rect> = cells(&app).into_iter().map(|(_, r)| r).collect();

    crate::window_ops::rotate_panes(&mut app, true);

    assert_eq!(ids(&app), vec![4, 1]);
    assert_panes_match_their_cells(&app, "horizontal row, -U");
    assert_eq!(
        sizes_of_panes(&app)[0].1, before_cells[0].width.max(crate::pane::MIN_PANE_DIM),
        "the pane now in the wide cell must report the WIDE width"
    );
}

// ── Nested layouts: tmux permutes the CELL OCCUPANTS, never the tree ───────

#[test]
fn rotate_keeps_a_nested_layout_shape_and_only_moves_the_panes() {
    // V[ V[ %1, %2 ], %3 ] — the shape `split -l 10` then `split -l 5` builds.
    // Rotating the root's CHILDREN turned this into V[ %3, V[ %1, %2 ] ] and
    // squeezed two panes into the 10 row cell.
    let mut app = empty_app();
    let mut win = make_window();
    win.root = Node::Split {
        kind: LayoutKind::Vertical,
        sizes: vec![39, 10],
        children: vec![
            Node::Split {
                kind: LayoutKind::Vertical,
                sizes: vec![34, 4],
                children: vec![Node::Leaf(make_pane(1, 10, 10)), Node::Leaf(make_pane(2, 10, 10))],
            },
            Node::Leaf(make_pane(3, 10, 10)),
        ],
    };
    app.windows.push(win);
    app.active_idx = 0;
    crate::tree::resize_all_panes(&mut app);
    let start_shape = shape(&app);
    let before_cells: Vec<Rect> = cells(&app).into_iter().map(|(_, r)| r).collect();

    crate::window_ops::rotate_panes(&mut app, true);

    assert_eq!(shape(&app), start_shape, "the layout tree must be untouched by a rotate");
    let after_cells: Vec<Rect> = cells(&app).into_iter().map(|(_, r)| r).collect();
    assert_eq!(after_cells, before_cells, "every cell keeps its position and size");
    assert_eq!(ids(&app), vec![2, 3, 1], "the panes move one cell along, in DFS leaf order");
    assert_panes_match_their_cells(&app, "nested layout, -U");
}

// ── Focus ─────────────────────────────────────────────────────────────────

#[test]
fn rotate_leaves_focus_on_the_same_cell() {
    // tmux re-points w->active at the pane that moved INTO the active pane's
    // cell, so the highlighted slot never jumps under the user.
    let mut app = app_with_split(LayoutKind::Vertical, &[1, 2, 3], &[34, 4, 10]);
    app.windows[0].active_path = vec![1];

    crate::window_ops::rotate_panes(&mut app, true);

    assert_eq!(app.windows[0].active_path, vec![1], "the active CELL does not move");
    assert_eq!(ids(&app)[1], 3, "but a different pane now occupies it");
}

// ── Degenerate inputs ─────────────────────────────────────────────────────

#[test]
fn single_pane_rotate_is_a_no_op() {
    let mut app = app_with_split(LayoutKind::Vertical, &[7], &[100]);
    let start_ids = ids(&app);
    let start_sizes = sizes_of_panes(&app);
    let start_shape = shape(&app);

    crate::window_ops::rotate_panes(&mut app, true);
    crate::window_ops::rotate_panes(&mut app, false);

    assert_eq!(ids(&app), start_ids, "nothing to rotate with one pane");
    assert_eq!(sizes_of_panes(&app), start_sizes);
    assert_eq!(shape(&app), start_shape);
}

#[test]
fn rotate_on_an_empty_window_list_does_not_panic() {
    let mut app = empty_app();
    crate::window_ops::rotate_panes(&mut app, true);
    crate::window_ops::rotate_panes(&mut app, false);
}
