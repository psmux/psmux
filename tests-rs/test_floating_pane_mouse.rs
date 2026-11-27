// Floating panes and the mouse, as tmux 3.8 does it (7f2a35ad).
//
// A press on a floating pane went to the tiled pane under it: the client hit
// tested the tiled rects only, so a click in a float focused the tiled pane
// (and since 014ad925 took the float's focus away), and a drag on its border
// never reached the server's float drag. tmux looks at floating panes first
// (window_get_active_at, window.c): a press inside one selects it and brings
// it to the front, and a drag on its border moves it (top edge) or resizes it
// (other edges and corners), cmd_resize_pane_mouse_resize_move_floating in
// cmd-resize-pane.c.
//
// The drag tables below are tmux 3.8 measured with a real attached client
// (WSL, `new-pane -x 30 -y 10 -X 20 -Y 5` in a 100x30 window, status off),
// each drag one cell per motion event.

use super::*;
use crate::floating::{drag_step, fit_outer, hit, screen_rect, FloatHit, Geom};
use crate::types::{FloatingPane, Window};

/// Drag from `from` to `to`, one cell a motion (x and y together), as the
/// measuring script did, and return the content area tmux ended with.
fn drag(mut g: Geom, from: (i32, i32), to: (i32, i32)) -> Geom {
    let (mut x, mut y) = from;
    while (x, y) != to {
        let last = (x, y);
        x += (to.0 - x).signum();
        y += (to.1 - y).signum();
        if let Some(n) = drag_step(g, last, (x, y)) {
            g = n;
        }
    }
    g
}

fn content(x: i32, y: i32, w: i32, h: i32) -> Geom {
    Geom { xoff: x, yoff: y, sx: w, sy: h }
}

// ── The drag steps against tmux 3.8 ────────────────────────────────────────

#[test]
fn new_pane_outer_size_is_tmux_content_plus_border() {
    // new-pane -x 30 -y 10 -X 20 -Y 5: pane_left 21, pane_top 6, 28x8.
    assert_eq!(Geom::from_outer(20, 5, 30, 10), content(21, 6, 28, 8));
    assert_eq!(content(21, 6, 28, 8).outer(), (20, 5, 30, 10));
}

#[test]
fn top_edge_moves_the_pane() {
    // Grab (X+5, Y-1), drag by (+5, +3): left 26, top 9, size kept.
    let g = content(21, 6, 28, 8);
    assert_eq!(drag(g, (26, 5), (31, 8)), content(26, 9, 28, 8));
}

#[test]
fn right_and_bottom_edges_resize_one_way() {
    // Right edge (X+W) by +4: width 32, nothing else.
    let g = content(26, 9, 28, 8);
    let g = drag(g, (54, 12), (58, 12));
    assert_eq!(g, content(26, 9, 32, 8));
    // Bottom edge (Y+H) by +2: height 10, nothing else.
    let g = drag(g, (31, 17), (31, 19));
    assert_eq!(g, content(26, 9, 32, 10));
}

#[test]
fn left_edge_resizes_and_moves_the_left_side() {
    // Left edge (X-1) by -3: left 23, width 35.
    let g = content(26, 9, 32, 10);
    assert_eq!(drag(g, (25, 12), (22, 12)), content(23, 9, 35, 10));
}

#[test]
fn corners_resize_both_ways() {
    // Bottom-right (X+W, Y+H) by (+3, +2): 38x12.
    let g = content(23, 9, 35, 10);
    let g = drag(g, (58, 19), (61, 21));
    assert_eq!(g, content(23, 9, 38, 12));
    // Top-left (X-1, Y-1) by (-2, -1): left 21, top 8, 40x13.
    let g = drag(g, (22, 8), (20, 7));
    assert_eq!(g, content(21, 8, 40, 13));
}

#[test]
fn the_cell_next_to_a_corner_is_the_corner() {
    // (X, Y-1), the top edge cell beside the top-left corner, by (+2, +2):
    // tmux ends at left 24, top 10, 38x11 (the left side follows the
    // pointer, which was one cell inside the corner).
    let g = content(21, 8, 40, 13);
    let g = drag(g, (21, 7), (23, 9));
    assert_eq!(g, content(24, 10, 38, 11));
    // (X+W-1, Y+H), the bottom edge cell beside the bottom-right corner, by
    // (+2, +1): tmux ends at 39x12 (the first motion is the bottom edge, the
    // next ones the corner the pointer then holds).
    let g = drag(g, (61, 21), (63, 22));
    assert_eq!(g, content(24, 10, 39, 12));
}

#[test]
fn an_edge_stops_at_the_minimum_size() {
    // Right edge dragged to X+1: width 1 (PANE_MINIMUM), then no further.
    let g = content(24, 10, 39, 12);
    assert_eq!(drag(g, (63, 13), (25, 13)), content(24, 10, 1, 12));
    assert_eq!(drag_step(content(24, 10, 1, 12), (25, 13), (24, 13)), None);
}

#[test]
fn a_pointer_off_the_border_changes_nothing() {
    let g = content(21, 6, 28, 8);
    assert_eq!(drag_step(g, (30, 9), (33, 11)), None);
}

// ── Where a press lands ─────────────────────────────────────────────────────

#[test]
fn every_border_cell_is_border_and_the_rest_is_inside() {
    let r = (20, 5, 30, 10);
    for (x, y) in [(20, 5), (49, 5), (20, 14), (49, 14), (35, 5), (35, 14), (20, 9), (49, 9)] {
        assert_eq!(hit(r, true, x, y), Some(FloatHit::Border), "({x},{y})");
    }
    for (x, y) in [(21, 6), (48, 13), (35, 9)] {
        assert_eq!(hit(r, true, x, y), Some(FloatHit::Inside), "({x},{y})");
    }
    for (x, y) in [(19, 9), (50, 9), (35, 4), (35, 15)] {
        assert_eq!(hit(r, true, x, y), None, "({x},{y})");
    }
}

#[test]
fn a_float_without_a_border_is_all_content() {
    assert_eq!(hit((20, 5, 30, 10), false, 20, 5), Some(FloatHit::Inside));
    assert_eq!(hit((20, 5, 30, 10), false, 49, 14), Some(FloatHit::Inside));
}

#[test]
fn the_hit_rect_is_the_drawn_rect() {
    // Past the right edge of an 80 wide area: drawn (and hit) pulled in.
    assert_eq!(screen_rect(70, 5, 30, 10, 80, 24), Some((50, 5, 30, 10)));
    // Larger than the area: cut to it.
    assert_eq!(screen_rect(0, 0, 120, 40, 80, 24), Some((0, 0, 80, 24)));
    assert_eq!(screen_rect(0, 0, 1, 10, 80, 24), None);
}

#[test]
fn psmux_keeps_the_float_inside_the_window() {
    // A move past the right edge stops there with the size kept.
    let g = content(21, 6, 28, 8);
    let moved = content(80, 6, 28, 8);
    assert_eq!(fit_outer(g, moved, 100, 30), (70, 5, 30, 10));
    // A resize past the right edge stops growing there, without moving.
    let grown = content(21, 6, 120, 8);
    assert_eq!(fit_outer(g, grown, 100, 30), (20, 5, 80, 10));
}

// ── The server ──────────────────────────────────────────────────────────────

fn empty_pane(app: &mut AppState, rows: u16, cols: u16) -> Option<Pane> {
    let id = app.next_pane_id;
    let pane = crate::popup::create_empty_pane(rows, cols, id)?;
    app.next_pane_id += 1;
    Some(pane)
}

fn window_with(root: Node, id: usize) -> Window {
    let pid = match &root { Node::Leaf(p) => p.id, _ => 0 };
    Window {
        root,
        active_path: vec![],
        name: "w".into(),
        id,
        area: Rect { x: 0, y: 0, width: 100, height: 30 },
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![pid],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// A 100x30 window with a tiled pane and two floats: A at (20,5) 30x10
/// (behind) and B at (40,8) 30x10 (in front), no float focused. Returns the
/// app and (tiled, A, B) ids. None when no pseudo console can be opened.
fn rig() -> Option<(AppState, usize, usize, usize)> {
    let r = rig_inner();
    if r.is_none() {
        eprintln!("floating pane mouse: no pseudo console, skipped");
    }
    r
}

fn rig_inner() -> Option<(AppState, usize, usize, usize)> {
    let mut app = AppState::new("floatmouse".to_string());
    app.windows.clear();
    app.active_idx = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 100, height: 30 };
    let tiled = empty_pane(&mut app, 30, 100)?;
    let tid = tiled.id;
    app.windows.push(window_with(Node::Leaf(tiled), 1));
    let mut ids = Vec::new();
    for (x, y) in [(20u16, 5u16), (40, 8)] {
        let pane = empty_pane(&mut app, 8, 28)?;
        ids.push(pane.id);
        let id = pane.id;
        app.windows[0].floating.push(FloatingPane {
            pane, x, y, w: 30, h: 10,
            border: "single".into(),
            id,
            title: String::new(),
            position: None,
            styles: Default::default(),
        });
    }
    Some((app, tid, ids[0], ids[1]))
}

fn order(app: &AppState) -> Vec<usize> {
    app.windows[0].floating.iter().map(|f| f.id).collect()
}

fn focused(app: &AppState) -> Option<usize> {
    let w = &app.windows[0];
    w.floating_focus.map(|i| w.floating[i].id)
}

fn geom(app: &AppState, id: usize) -> (u16, u16, u16, u16) {
    let f = app.windows[0].floating.iter().find(|f| f.id == id).unwrap();
    (f.x, f.y, f.w, f.h)
}

#[test]
fn a_click_inside_a_float_focuses_it_and_brings_it_to_the_front() {
    let Some((mut app, _t, a, b)) = rig() else { return };
    // pane-mouse for A, content relative, as the client sends it.
    handle_client_pane_mouse(&mut app, a, 0, 2, 1, true, false);
    assert_eq!(focused(&app), Some(a));
    assert_eq!(order(&app), vec![b, a], "A is drawn last, in front");
    // The release and motion do not refocus or reorder.
    handle_client_pane_mouse(&mut app, b, 32, 0, 0, true, false);
    handle_client_pane_mouse(&mut app, b, 0, 0, 0, false, false);
    assert_eq!(focused(&app), Some(a));
    assert_eq!(order(&app), vec![b, a]);
}

#[test]
fn a_press_on_a_border_starts_the_drag_on_the_front_float() {
    let Some((mut app, _t, a, b)) = rig() else { return };
    // (45, 8) is B's top edge, inside A's area too: B is in front.
    remote_mouse_down(&mut app, 45, 8);
    assert_eq!(focused(&app), Some(b));
    assert!(app.float_drag.is_some_and(|d| d.id == b));
    // A's own top edge where B does not cover it: A comes to the front.
    remote_mouse_up(&mut app, 45, 8);
    remote_mouse_down(&mut app, 25, 5);
    assert_eq!(focused(&app), Some(a));
    assert_eq!(order(&app), vec![b, a]);
    assert!(app.float_drag.is_some_and(|d| d.id == a));
}

#[test]
fn dragging_the_top_edge_moves_and_another_edge_resizes() {
    let Some((mut app, _t, a, _b)) = rig() else { return };
    // Top edge of A by (+3, +2): moved, size kept.
    remote_mouse_down(&mut app, 25, 5);
    for (x, y) in [(26, 6), (27, 7), (28, 7)] { remote_mouse_drag(&mut app, x, y); }
    remote_mouse_up(&mut app, 28, 7);
    assert_eq!(geom(&app, a), (23, 7, 30, 10));
    assert!(app.float_drag.is_none());
    // Its right edge (x = 52) by +4: 4 wider, and the pane's size follows.
    remote_mouse_down(&mut app, 52, 10);
    for x in 53..=56 { remote_mouse_drag(&mut app, x, 10); }
    remote_mouse_up(&mut app, 56, 10);
    assert_eq!(geom(&app, a), (23, 7, 34, 10));
    let f = app.windows[0].floating.iter().find(|f| f.id == a).unwrap();
    assert_eq!((f.pane.last_cols, f.pane.last_rows), (32, 8));
}

#[test]
fn select_pane_brings_a_float_to_the_front() {
    let Some((mut app, _t, a, b)) = rig() else { return };
    app.windows[0].floating_focus = Some(1);
    crate::tree::focus_pane_by_id(&mut app, a);
    crate::tree::raise_floating_by_id(&mut app, a);
    assert_eq!(order(&app), vec![b, a]);
    assert_eq!(focused(&app), Some(a));
}

#[test]
fn bringing_one_float_forward_keeps_the_focus_on_its_pane() {
    let Some((mut app, _t, a, b)) = rig() else { return };
    app.windows[0].floating_focus = Some(1);
    crate::tree::raise_floating(&mut app.windows[0], 0);
    assert_eq!(order(&app), vec![b, a]);
    assert_eq!(focused(&app), Some(b), "the focus stays on B, now at index 0");
}

// ── The client ──────────────────────────────────────────────────────────────

fn float_json(id: usize, x: u16, y: u16, border: &str) -> crate::client::FloatJson {
    crate::client::FloatJson { id, x, y, w: 30, h: 10, border: border.into(), ..Default::default() }
}

#[test]
fn the_client_hits_the_front_float_first() {
    use crate::client::{float_under, FloatUnder};
    let area = Rect { x: 0, y: 1, width: 100, height: 29 };
    let floats = vec![float_json(2, 20, 5, "single"), float_json(3, 40, 8, "single")];
    // Screen row = window row + 1 (status on top).
    assert_eq!(float_under(area, &floats, 45, 9), FloatUnder::Border { id: 3 });
    assert_eq!(float_under(area, &floats, 45, 12), FloatUnder::Inside { id: 3, wants_mouse: false, col: 4, row: 2 });
    assert_eq!(float_under(area, &floats, 25, 7), FloatUnder::Inside { id: 2, wants_mouse: false, col: 4, row: 0 });
    assert_eq!(float_under(area, &floats, 5, 5), FloatUnder::None);
    let bare = vec![float_json(4, 20, 5, "none")];
    assert_eq!(float_under(area, &bare, 20, 6), FloatUnder::Inside { id: 4, wants_mouse: false, col: 0, row: 0 });
}
