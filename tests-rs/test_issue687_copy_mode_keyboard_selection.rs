// Issue #687: a keyboard selection in copy mode yanks a different range than
// the one the client painted, whenever the view has been scrolled.
//
// Reported against the keyboard route: select whole lines with `V`, move down,
// press Enter, and the text that lands in the buffer is not the text that was
// highlighted.  It runs off the bottom of the buffer instead.
//
// Root cause, introduced by b957aa6 ("selection: pair each column with its own
// row, and carry each end's scroll offset"):
//
//       let anchor_abs = anchor.0 as i64 - anchor_scroll as i64;
//     - let cursor_abs = pos.0 as i64 - current_scroll as i64;
//     + let cursor_abs = pos.0 as i64 - app.copy_pos_scroll_offset as i64;
//
// That commit fixed a real mouse defect: a drag that hits a pane edge scrolls
// the view AFTER the endpoint was recorded, so the endpoint has to keep the
// offset it was measured at.  The endpoint's offset is written in nine places
// and every one of them is a mouse handler.  Nothing on the keyboard route
// writes it: not `enter_copy_mode`, not `v` / `V` / `o`, not a cursor motion,
// not a `send-keys -X` verb, and `CopyModeState` does not carry it across a
// pane switch either.  It therefore stays 0, so the endpoint is resolved
// against the live bottom of the buffer while the anchor is resolved against
// the scrolled view, and the selection is displaced by the scroll offset.
//
// Measured on the released binary with 200 numbered lines, `select-line`, two
// `cursor-down`s and `copy-selection`:
//
//     scroll-up x0     1 line    (empty)
//     scroll-up x3     3 lines   LINE198, LINE199, LINE200
//     scroll-up x10   10 lines   LINE191 ... LINE200
//
// Three lines were selected every time.  The copied range tracks the scroll
// offset and always ends at the live bottom.
//
// The fix makes the endpoint's offset an `Option`: `Some(o)` only while a drag
// has pinned it, `None` meaning "this endpoint is in the current view", which
// is what every route other than a mid drag edge scroll means.  A field that
// ninety call sites have to remember to write is the defect itself.
//
// These tests drive the REAL copy mode over a real PTY-backed pane tree (no
// psmux server and no session is created).  Registered from src/copy_mode.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const ROWS: u16 = 10;
const COLS: u16 = 40;
const SCROLLBACK: usize = 200;

/// ConPTY creation can fail transiently when the suite churns many short-lived
/// PTYs in parallel, so retry with backoff and name the stage that broke.
fn open_pane_pty(
    rows: u16,
    cols: u16,
) -> (
    Box<dyn portable_pty::MasterPty + Send>,
    Box<dyn portable_pty::Child + Send + Sync>,
    Box<dyn std::io::Write + Send>,
) {
    let mut last_err = String::new();
    for attempt in 0u64..5 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(100 * attempt));
        }
        let pty = portable_pty::native_pty_system();
        let pair = match pty.openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }) {
            Ok(p) => p,
            Err(e) => { last_err = format!("openpty: {e:?}"); continue; }
        };
        let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
        cmd.arg("/c");
        cmd.arg("exit");
        let child = match crate::util::spawn_pty_child(&*pair.slave, cmd) {
            Ok(c) => c,
            Err(e) => { last_err = format!("spawn dummy: {e:?}"); continue; }
        };
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => { last_err = format!("take_writer: {e:?}"); continue; }
        };
        return (pair.master, child, writer);
    }
    panic!("PTY-backed pane creation failed after 5 attempts under parallel load: {last_err}");
}

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, child, writer) = open_pane_pty(rows, cols);
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK)));
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
        mouse_input_cache: None, win32_input_latched: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, COLS, ROWS),
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

/// One window, one pane holding 60 numbered lines, in copy mode at the live
/// bottom with the cursor parked mid viewport so no motion is clamped.
fn copy_app() -> AppState {
    let mut app = AppState::new("issue687".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = "vi".to_string();
    let pane = make_pane(0, ROWS, COLS);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for i in 1..=60 {
            parser.process(format!("line-{i}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app.last_window_area = ratatui::layout::Rect::new(0, 0, COLS, ROWS);
    enter_copy_mode(&mut app);
    app.copy_pos = Some((ROWS / 2, 0));
    app
}

/// The text of one visible row, read the way a capture reads it: whatever the
/// user is looking at right now, at the offset the view is parked on.
fn visible_row(app: &AppState, row: u16) -> String {
    let win = &app.windows[app.active_idx];
    let p = active_pane(&win.root, &win.active_path).expect("a pane");
    let parser = p.term.lock().expect("parser lock");
    capture_row_text(parser.screen(), row, 0..COLS).trim_end().to_string()
}

/// The rows `first..=last` of the current view, spelled the way a line mode
/// yank spells them, which is every line followed by a newline.
fn visible_lines(app: &AppState, first: u16, last: u16) -> String {
    (first..=last).map(|r| format!("{}\n", visible_row(app, r))).collect()
}

fn yanked(app: &AppState) -> String {
    app.paste_buffers.first().cloned().unwrap_or_default()
}

// The reported defect.

#[test]
fn a_line_selection_after_scrolling_yanks_the_lines_it_painted() {
    for scroll in [0usize, 1, 3, 7] {
        let mut app = copy_app();
        scroll_copy_up(&mut app, scroll);
        assert_eq!(app.copy_scroll_offset, scroll, "the view must actually move");

        let start = app.copy_pos.expect("a cursor").0;
        crate::input::send_text_to_active(&mut app, "V").unwrap();
        crate::input::send_text_to_active(&mut app, "j").unwrap();
        crate::input::send_text_to_active(&mut app, "j").unwrap();
        let end = app.copy_pos.expect("a cursor").0;
        assert_eq!(end, start + 2, "scroll={scroll}: two j presses move two rows");

        let want = visible_lines(&app, start, end);
        crate::input::send_key_to_active(&mut app, "enter").unwrap();
        assert_eq!(yanked(&app), want,
            "scroll={scroll}: the yank must be the three rows under the selection");
        assert_eq!(yanked(&app).lines().count(), 3,
            "scroll={scroll}: three rows were selected, so three lines are copied");
    }
}

#[test]
fn the_selected_lines_are_the_ones_the_numbers_say() {
    // The same thing again, pinned to concrete content so the test cannot pass
    // by comparing two equally wrong readings of the screen.
    let mut app = copy_app();
    scroll_copy_up(&mut app, 3);
    let (start, end) = (ROWS / 2, ROWS / 2 + 2);
    let first = visible_row(&app, start);
    assert!(first.starts_with("line-"),
        "the fixture must put numbered lines on screen, got {first:?}");
    crate::input::send_text_to_active(&mut app, "V").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    let want = visible_lines(&app, start, end);
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    assert_eq!(yanked(&app), want);
}

#[test]
fn a_character_selection_after_scrolling_yanks_what_it_painted() {
    let mut app = copy_app();
    scroll_copy_up(&mut app, 4);
    let start = app.copy_pos.expect("a cursor").0;
    crate::input::send_text_to_active(&mut app, "v").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    let end = app.copy_pos.expect("a cursor").0;

    // Char mode from column 0 of the first row to column 0 of the second is
    // the first row entire, then one cell of the second.
    let want = format!("{}\n{}", visible_row(&app, start),
        visible_row(&app, end).chars().next().map(String::from).unwrap_or_default());
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    assert_eq!(yanked(&app), want);
}

#[test]
fn a_fresh_copy_mode_session_does_not_inherit_a_pinned_endpoint() {
    // A drag pins the endpoint's offset.  Entering copy mode again must not
    // resolve a keyboard selection against that old pin.
    let mut app = copy_app();
    app.copy_pos_scroll_offset = Some(9);
    exit_copy_mode(&mut app);

    enter_copy_mode(&mut app);
    assert_eq!(app.copy_pos_scroll_offset, None,
        "entering copy mode must leave the endpoint in the current view");
    app.copy_pos = Some((ROWS / 2, 0));
    scroll_copy_up(&mut app, 3);
    let start = app.copy_pos.expect("a cursor").0;
    crate::input::send_text_to_active(&mut app, "V").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    let want = visible_lines(&app, start, start + 1);
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    assert_eq!(yanked(&app), want);
}

/// Pull an integer field out of a frame the server would send to a client.
fn frame_num(frame: &str, key: &str) -> i64 {
    let pat = format!("\"{key}\"");
    let at = frame.find(&pat).unwrap_or_else(|| panic!("{key} in {frame}"));
    let rest = &frame[at + pat.len()..];
    let colon = rest.find(':').expect("colon");
    rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .expect("a number")
}

#[test]
fn the_live_frame_and_the_yank_agree_on_a_keyboard_selection() {
    // `dump_layout_json_fast` is the frame the server actually sends, twice per
    // loop.  `dump_layout_json` is only the `DumpLayout` control request, and
    // it is the one the mouse tests read, which is how the two serialisers came
    // to disagree about the endpoint in the first place.
    let mut app = copy_app();
    scroll_copy_up(&mut app, 3);
    let start = app.copy_pos.expect("a cursor").0;
    crate::input::send_text_to_active(&mut app, "V").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();

    let frame = crate::layout::dump_layout_json_fast(&mut app).expect("a frame");
    assert_eq!(frame_num(&frame, "sel_start_row"), start as i64,
        "the painted selection starts on the row V was pressed on");
    assert_eq!(frame_num(&frame, "sel_end_row"), start as i64 + 2,
        "and ends two rows below, where the cursor is");

    let want = visible_lines(&app, start, start + 2);
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    assert_eq!(yanked(&app), want, "the yank must be the rows the frame painted");
}

#[test]
fn a_pinned_endpoint_is_painted_where_the_yank_reads_it() {
    // What a mouse drag leaves behind when it reaches a pane edge: the endpoint
    // was recorded in the view at offset 3, then the edge scroll moved the view
    // to offset 4.  Both the frame and the yank have to put that endpoint on
    // the content line it was measured on, which is one row further down in the
    // view now on screen.
    let mut app = copy_app();
    scroll_copy_up(&mut app, 3);
    let anchor_row = ROWS / 2;
    app.copy_anchor = Some((anchor_row, 0));
    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
    app.copy_pos = Some((anchor_row - 2, 0));
    app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
    app.copy_selection_mode = crate::types::SelectionMode::Line;
    let pinned_text = visible_lines(&app, anchor_row - 2, anchor_row);

    scroll_copy_up(&mut app, 1);
    assert_eq!(app.copy_scroll_offset, 4, "the view moved under the pinned endpoint");

    let frame = crate::layout::dump_layout_json_fast(&mut app).expect("a frame");
    assert_eq!(frame_num(&frame, "sel_start_row"), anchor_row as i64 - 1,
        "the pinned endpoint paints one row lower in the view that scrolled up");
    assert_eq!(frame_num(&frame, "sel_end_row"), anchor_row as i64 + 1,
        "and so does the anchor");

    yank_selection(&mut app).unwrap();
    assert_eq!(yanked(&app), pinned_text,
        "the yank must still be the content the endpoint was measured on");
    assert_eq!(yanked(&app), visible_lines(&app, anchor_row - 1, anchor_row + 1),
        "which is exactly the rows the frame points at");
}

#[test]
fn walking_the_cursor_off_the_top_row_counts_as_scrolling() {
    // Nobody has to reach for a scroll key.  `move_copy_cursor` scrolls by
    // itself when the cursor would leave the top of the pane, which is what `k`
    // and Up do in copy mode, and it is where the mouse wheel ends up too.  The
    // view is then off the live bottom exactly as a scroll key would have left
    // it, so the same defect applies and the reporter never pressed anything
    // that looks like scrolling.
    let mut app = copy_app();
    for _ in 0..(ROWS + 7) {
        move_copy_cursor(&mut app, 0, -1);
    }
    assert_eq!(app.copy_pos.expect("a cursor").0, 0, "the cursor parks on the top row");
    assert!(app.copy_scroll_offset > 0, "and the view scrolled to get it there");
    assert_eq!(app.copy_pos_scroll_offset, None, "no gesture pinned the endpoint");

    crate::input::send_text_to_active(&mut app, "V").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    let want = visible_lines(&app, 0, 2);
    yank_selection(&mut app).unwrap();
    assert_eq!(yanked(&app), want);
    assert_eq!(yanked(&app).lines().count(), 3);
}
