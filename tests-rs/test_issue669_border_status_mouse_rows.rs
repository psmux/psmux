// Issue #669: "with pane-border-status top, mouse RELEASE and DRAG arrive one
// row below the PRESS" (goranvasilj).
//
// MEASURED, real attached client, real MOUSE_EVENT records injected with
// tests/click_injector.cs and tests/mouse_drag_hold_injector.cs, an SGR mouse
// echo child (tests/mouse_echo_child.cs) logging its raw stdin. One click and
// one drag, both starting on screen row 5:
//
//   before (37e2990), pane-border-status top
//     click(10,5)            press   <ESC>[<0;11;5M      release <ESC>[<0;11;6m
//     drag(10,5)->(10,6)     motions <ESC>[<32;11;6M <ESC>[<32;11;7M
//                            release <ESC>[<0;11;7m
//   after, same gesture
//     click(10,5)            press   <ESC>[<0;11;5M      release <ESC>[<0;11;5m
//     drag(10,5)->(10,6)     motions <ESC>[<32;11;5M <ESC>[<32;11;6M
//                            release <ESC>[<0;11;6m
//
// pane-border-status off and bottom reported row 6 for both press and release
// in either build, which is why the report singles out `top`.
//
// WHY.  psmux converts a screen cell into a pane cell in TWO places. Outside
// copy mode the client resolves the press itself and sends `pane-mouse` with a
// row it has already taken the label row out of (`client::pane_content_inner`),
// but it hands the release, the drag, the wheel and bare motion to the server
// as RAW screen coordinates (`mouse-up X Y`, `mouse-drag X Y`, `scroll-up X Y`,
// `mouse-move X Y`). The server converted those against the pane's LAYOUT SLOT,
// whose top row is the `pane-border-status top` label, so everything it
// converted sat one row below everything the client converted.
//
// TMUX PARITY.  tmux cannot drift like this. `layout_fix_panes` (layout.c)
// bakes the label row into the pane's own geometry exactly once:
//
//     if (layout_add_horizontal_border(root, lc, status)) {
//             if (status == PANE_STATUS_TOP)
//                     wp->yoff++;
//             if (sy > 1)
//                     sy--;
//     }
//
// and every mouse event then converts through the single `cmd_mouse_at`
// (cmd.c), `*yp = y - wp->yoff`, reached from `input_key_mouse`
// (input-keys.c) for presses, releases, drags, motion and the wheel alike.
// One offset, one conversion, so press and release agree by construction.
//
// These tests pin the same property on psmux: one resolver (`PaneLabelRow`)
// that delegates to the one helper the client draws with, used by every
// server-side raw-coordinate handler.

use crate::types::{AppState, LayoutKind, Mode, Node};
use ratatui::layout::Rect;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{
    remote_mouse_down, remote_mouse_drag, remote_mouse_up, toggle_zoom, PaneLabelRow,
};

const AREA: Rect = Rect { x: 0, y: 0, width: 80, height: 24 };

/// The `pane-border-format` the client substitutes when the option is unset
/// (#414). Spelled out here rather than imported so a silent change to the
/// default is a test failure, not a quietly agreeing pair of constants.
const DEFAULT_FORMAT: &str = "#{pane_index} \"#{pane_title}\"";

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
        mouse_input_cache: None,
        win32_input_latched: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(root: Node) -> crate::types::Window {
    crate::types::Window {
        root,
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

/// One window with `leaves` panes stacked vertically (or a single pane when
/// `leaves` is 1), and `pane-border-status` set to `status`.
fn app_with(status: &str, leaves: usize) -> AppState {
    let mut app = AppState::new("issue669".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = AREA;
    if status != "off" {
        app.user_options.insert("pane-border-status".into(), status.into());
    }
    let root = if leaves <= 1 {
        Node::Leaf(make_pane(1, AREA.height, AREA.width))
    } else {
        Node::Split {
            kind: LayoutKind::Vertical,
            sizes: vec![(100 / leaves) as u16; leaves],
            children: (0..leaves)
                .map(|i| Node::Leaf(make_pane(i + 1, AREA.height / leaves as u16, AREA.width)))
                .collect(),
        }
    };
    let mut win = make_window(root);
    win.active_path = if leaves <= 1 { vec![] } else { vec![0] };
    app.windows.push(win);
    app.active_idx = 0;
    app
}

/// The layout slot of the pane at `path`, as every mouse handler computes it.
fn slot(app: &AppState, path: &[usize]) -> Rect {
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    crate::tree::compute_rects(&app.windows[0].root, app.last_window_area, &mut rects);
    rects
        .into_iter()
        .find(|(p, _)| p.as_slice() == path)
        .map(|(_, r)| r)
        .expect("pane slot")
}

// ───────────────────────────── the conversion ─────────────────────────────

#[test]
fn border_status_off_leaves_the_whole_slot_as_content() {
    let app = app_with("off", 1);
    let label = PaneLabelRow::from_options(&app);
    let area = Rect::new(0, 0, 80, 24);
    assert_eq!(label.content(area), area);
    // Screen row 5 is content row 5.
    assert_eq!(label.cell_0based(area, 10, 5), (10, 5));
}

#[test]
fn border_status_top_reserves_the_first_row_of_the_slot() {
    let app = app_with("top", 1);
    let label = PaneLabelRow::from_options(&app);
    let area = Rect::new(0, 0, 80, 24);
    assert_eq!(label.content(area), Rect::new(0, 1, 80, 23));
    // Screen row 5 is content row 4 — the row the reporter's press already got.
    assert_eq!(label.cell_0based(area, 10, 5), (10, 4));
}

#[test]
fn border_status_bottom_reserves_the_last_row_of_the_slot() {
    let app = app_with("bottom", 1);
    let label = PaneLabelRow::from_options(&app);
    let area = Rect::new(0, 0, 80, 24);
    assert_eq!(label.content(area), Rect::new(0, 0, 80, 23));
    // The label is below the content, so rows are unshifted — `bottom` was
    // never affected, and must stay that way.
    assert_eq!(label.cell_0based(area, 10, 5), (10, 5));
}

#[test]
fn the_server_resolver_and_the_client_helper_agree_on_every_status() {
    let area = Rect::new(0, 0, 80, 24);
    for status in ["off", "top", "bottom"] {
        let app = app_with(status, 1);
        let server = PaneLabelRow::from_options(&app).content(area);
        let client = crate::client::pane_content_inner(area, status, DEFAULT_FORMAT);
        assert_eq!(server, client, "status={status}: server and client disagree");
    }
}

#[test]
fn an_unset_pane_border_format_still_costs_the_pane_its_label_row() {
    // #414: tmux's default format is non-empty, so `set pane-border-status top`
    // alone draws a label. If the server treated the unset option as empty it
    // would convert against a row the client never gives the pane.
    let app = app_with("top", 1);
    assert!(!app.user_options.contains_key("pane-border-format"));
    assert_eq!(
        PaneLabelRow::from_options(&app).content(Rect::new(0, 0, 80, 24)),
        Rect::new(0, 1, 80, 23)
    );
    assert_eq!(crate::client::DEFAULT_PANE_BORDER_FORMAT, DEFAULT_FORMAT);
}

#[test]
fn an_explicitly_emptied_pane_border_format_draws_no_label_row() {
    let mut app = app_with("top", 1);
    app.user_options.insert("pane-border-format".into(), String::new());
    let area = Rect::new(0, 0, 80, 24);
    assert_eq!(PaneLabelRow::from_options(&app).content(area), area);
    assert_eq!(
        PaneLabelRow::from_options(&app).content(area),
        crate::client::pane_content_inner(area, "top", "")
    );
}

#[test]
fn a_click_on_the_label_row_itself_reports_the_first_content_row() {
    // The client floors the row at 0 before it sends `pane-mouse`; the raw
    // path must not hand the child a negative row instead.
    let app = app_with("top", 1);
    let label = PaneLabelRow::from_options(&app);
    assert_eq!(label.cell_0based(Rect::new(0, 0, 80, 24), 10, 0), (10, 0));
}

#[test]
fn press_drag_and_release_on_one_screen_row_convert_to_one_content_row() {
    // The bug in one assertion: three conversions of the SAME screen cell.
    for status in ["off", "top", "bottom"] {
        let app = app_with(status, 1);
        let label = PaneLabelRow::from_options(&app);
        let area = Rect::new(0, 0, 80, 24);
        let press = label.cell_0based(area, 10, 5);
        let drag = label.cell_0based(area, 10, 5);
        let release = label.cell_0based(area, 10, 5);
        assert_eq!(press, drag, "status={status}: drag disagrees with press");
        assert_eq!(press, release, "status={status}: release disagrees with press");
    }
}

#[test]
fn a_pane_below_a_split_converts_against_its_own_label_row() {
    // Two stacked panes, both labelled: the lower pane's content starts one row
    // below its own slot, not one row below the window.
    let app = app_with("top", 2);
    let label = PaneLabelRow::from_options(&app);
    let lower = slot(&app, &[1]);
    assert!(lower.y > 0, "lower pane should start below the upper one");
    let content = label.content(lower);
    assert_eq!(content.y, lower.y + 1);
    assert_eq!(content.height, lower.height - 1);
    // A press on the lower pane's first content row is its row 0.
    assert_eq!(label.cell_0based(lower, 3, content.y), (3, 0));
    // And the row under it is row 1, not row 2.
    assert_eq!(label.cell_0based(lower, 3, content.y + 1), (3, 1));
}

#[test]
fn a_zoomed_pane_keeps_its_label_row_reserved() {
    let mut app = app_with("top", 2);
    app.windows[0].active_path = vec![1];
    toggle_zoom(&mut app);
    let label = PaneLabelRow::from_options(&app);
    let zoomed = slot(&app, &[1]);
    assert!(zoomed.height > AREA.height / 2, "zoom should grow the pane");
    let content = label.content(zoomed);
    assert_eq!(content.y, zoomed.y + 1);
    assert_eq!(
        label.cell_0based(zoomed, 10, content.y + 4),
        (10, 4),
        "a zoomed pane still owes a row to its label"
    );
}

// ─────────────────── through the real server handlers ───────────────────
//
// Copy mode is the one place a raw-coordinate handler's result is observable
// without a child process: `remote_mouse_down`/`_drag`/`_up` land in
// `app.copy_pos`, through the same `copy_cell_for_area` the app-forwarding
// path's conversion sits beside. A press and a drag that disagree here are the
// same defect the echo child saw as an off-by-one SGR row.

fn press_then_drag_rows(status: &str, press_row: u16, drag_row: u16) -> (u16, u16) {
    let mut app = app_with(status, 1);
    app.mode = Mode::CopyMode;
    remote_mouse_down(&mut app, 10, press_row);
    let pressed = app.copy_pos.expect("press position").0;
    remote_mouse_drag(&mut app, 10, drag_row);
    let dragged = app.copy_pos.expect("drag position").0;
    (pressed, dragged)
}

#[test]
fn copy_mode_press_and_drag_track_the_content_row_for_every_status() {
    // Screen rows 5 and 9. `off`/`bottom` keep them; `top` shifts both up by
    // the label row, and the shift is the SAME for the press and the drag.
    assert_eq!(press_then_drag_rows("off", 5, 9), (5, 9));
    assert_eq!(press_then_drag_rows("bottom", 5, 9), (5, 9));
    assert_eq!(press_then_drag_rows("top", 5, 9), (4, 8));
}

#[test]
fn copy_mode_release_lands_on_the_same_row_as_a_press_on_that_cell() {
    // A release whose press belonged to another pane (or arrived before copy
    // mode opened) is the one release whose position survives the handler: the
    // #199 micro-click guard consumes it when a press cell is pending, and a
    // finished selection yanks and leaves copy mode. Both of those start from
    // the row this converts, so converting it differently from the press is
    // exactly what the echo child saw as `;6m` under a `;5M`.
    for (status, expect) in [("off", 9u16), ("bottom", 9), ("top", 8)] {
        let mut app = app_with(status, 1);
        app.mode = Mode::CopyMode;
        remote_mouse_down(&mut app, 10, 9);
        let pressed = app.copy_pos.expect("press position").0;

        let mut app = app_with(status, 1);
        app.mode = Mode::CopyMode;
        remote_mouse_up(&mut app, 10, 9);
        let released = app.copy_pos.expect("release position").0;

        assert_eq!(pressed, expect, "status={status}: press row");
        assert_eq!(
            released, pressed,
            "status={status}: release row {released} disagrees with press row {pressed}"
        );
    }
}

#[test]
fn copy_mode_in_the_lower_pane_of_a_split_uses_that_pane_s_own_content() {
    let mut app = app_with("top", 2);
    app.mode = Mode::CopyMode;
    let lower = slot(&app, &[1]);
    let first_content_row = lower.y + 1;
    remote_mouse_down(&mut app, 4, first_content_row + 2);
    assert_eq!(app.copy_pos.expect("press").0, 2);
    remote_mouse_drag(&mut app, 4, first_content_row + 3);
    assert_eq!(app.copy_pos.expect("drag").0, 3);
}
