// Copy mode end-of-line went to the last column of the pane whatever the line
// held. tmux (window_copy_cursor_end_of_line, grid_reader_cursor_end_of_line)
// follows a wrapped line to its last row and stops at the end of the text,
// trailing blanks left out; vi keys stay on the last character (tmux 3.7 and
// later, window_copy_cursor_limit) and emacs keys go one past it. start-of-line
// and back-to-indentation go back to the first row of a wrapped line. Every
// position here is what tmux 3.8 reports for the same lines.
//
// These tests drive the real copy mode over a pane tree with a stub PTY (no
// psmux server). Registered from src/copy_mode.rs.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crate::types::Node;

const ROWS: u16 = 10;
const COLS: u16 = 40;
const SCROLLBACK: usize = 200;

const CJK: &str = "\u{65e5}\u{672c}\u{8a9e}";

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
    let child = crate::util::StubChild::exited();
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
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
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

/// One window, one pane that has printed `output`, in copy mode with `keys`.
fn copy_app(output: &str, keys: &str) -> AppState {
    let mut app = AppState::new("copy_eol".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = keys.to_string();
    let pane = make_pane(0, ROWS, COLS);
    pane.term.lock().expect("parser lock").process(output.as_bytes());
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    app.windows.push(win);
    app.active_idx = 0;
    app.last_window_area = ratatui::layout::Rect::new(0, 0, COLS, ROWS);
    enter_copy_mode(&mut app);
    app
}

/// Rows 0 to 7: plain, trailing blanks, empty, indented, a 50 cell line that
/// wraps onto row 5, a CJK line ending in "ab", and one ending on a wide
/// character.
fn lines(keys: &str) -> AppState {
    let long = "0".repeat(50);
    copy_app(&format!("abcdefghi\r\ntrail   \r\n\r\n  indented x\r\n{long}\r\n{CJK}ab\r\n{CJK}\r\n"), keys)
}

fn eol_from(app: &mut AppState, row: u16, col: u16) -> (u16, u16) {
    app.copy_pos = Some((row, col));
    move_to_line_end(app);
    app.copy_pos.expect("copy_pos")
}

#[test]
fn vi_end_of_line_stops_on_the_last_character() {
    let mut app = lines("vi");
    assert_eq!(eol_from(&mut app, 0, 0), (0, 8));
    assert_eq!(eol_from(&mut app, 1, 0), (1, 4), "trailing blanks are not part of the line");
    assert_eq!(eol_from(&mut app, 2, 0), (2, 0), "an empty line stays on column 0");
    assert_eq!(eol_from(&mut app, 3, 5), (3, 11));
    assert_eq!(eol_from(&mut app, 4, 0), (5, 9), "a wrapped line ends on its last row");
    assert_eq!(eol_from(&mut app, 6, 0), (6, 7), "a line ending in 'ab' after CJK");
    assert_eq!(eol_from(&mut app, 7, 0), (7, 4), "a last wide character keeps its first cell");
}

#[test]
fn emacs_end_of_line_goes_one_past_the_last_character() {
    let mut app = lines("emacs");
    assert_eq!(eol_from(&mut app, 0, 0), (0, 9));
    assert_eq!(eol_from(&mut app, 1, 0), (1, 5));
    assert_eq!(eol_from(&mut app, 2, 0), (2, 0));
    assert_eq!(eol_from(&mut app, 3, 0), (3, 12));
    assert_eq!(eol_from(&mut app, 4, 0), (5, 10));
    assert_eq!(eol_from(&mut app, 6, 0), (6, 8));
    assert_eq!(eol_from(&mut app, 7, 0), (7, 6));
}

#[test]
fn end_of_line_follows_a_wrapped_line_to_its_last_row() {
    // The 50 cell line fills row 1 and 10 cells of row 2.
    let long = "0".repeat(50);
    for (keys, want) in [("vi", (2, 9)), ("emacs", (2, 10))] {
        let mut app = copy_app(&format!("abc\r\n{long}\r\nnext\r\n"), keys);
        assert_eq!(eol_from(&mut app, 1, 0), want, "{keys} from the first row");
        assert_eq!(eol_from(&mut app, 2, 3), want, "{keys} from the second row");
        assert_eq!(eol_from(&mut app, 3, 0), (3, if keys == "vi" { 3 } else { 4 }), "{keys} on the line after");
    }
}

#[test]
fn the_lines_fixture_wraps_where_the_tests_expect() {
    let app = lines("vi");
    let win = &app.windows[app.active_idx];
    let pane = active_pane(&win.root, &win.active_path).expect("pane");
    let parser = pane.term.lock().expect("parser lock");
    assert!(parser.screen().row_wrapped(4));
    assert!(!parser.screen().row_wrapped(3));
}

#[test]
fn end_of_line_repeated_stays_put() {
    let mut app = lines("vi");
    app.copy_pos = Some((0, 0));
    for _ in 0..3 {
        move_to_line_end(&mut app);
    }
    assert_eq!(app.copy_pos, Some((0, 8)));
}

#[test]
fn end_of_line_in_a_rectangle_selection_goes_to_the_last_column() {
    let mut app = lines("vi");
    app.copy_pos = Some((0, 0));
    app.copy_anchor = Some((0, 0));
    app.copy_selection_mode = crate::types::SelectionMode::Rect;
    move_to_line_end(&mut app);
    assert_eq!(app.copy_pos, Some((0, COLS - 1)));
    app.copy_selection_mode = crate::types::SelectionMode::Char;
    move_to_line_end(&mut app);
    assert_eq!(app.copy_pos, Some((0, 8)));
}

#[test]
fn start_of_line_and_back_to_indentation_go_to_the_first_row_of_a_wrapped_line() {
    let long = format!("  {}", "x".repeat(60));
    let mut app = copy_app(&format!("top\r\n{long}\r\n"), "vi");
    app.copy_pos = Some((2, 7));
    move_to_line_start(&mut app);
    assert_eq!(app.copy_pos, Some((1, 0)));
    app.copy_pos = Some((2, 7));
    move_to_first_nonblank(&mut app);
    assert_eq!(app.copy_pos, Some((1, 2)));
    // A row that is not a continuation keeps its own row.
    app.copy_pos = Some((0, 2));
    move_to_line_start(&mut app);
    assert_eq!(app.copy_pos, Some((0, 0)));
}

#[test]
fn back_to_indentation_on_an_empty_line_stays_put() {
    let mut app = lines("vi");
    app.copy_pos = Some((2, 0));
    move_to_first_nonblank(&mut app);
    assert_eq!(app.copy_pos, Some((2, 0)));
    app.copy_pos = Some((3, 9));
    move_to_first_nonblank(&mut app);
    assert_eq!(app.copy_pos, Some((3, 2)));
}

#[test]
fn end_of_line_scrolls_down_to_the_rest_of_a_wrapped_line() {
    // A screen of short lines, a 50 cell line, then enough short lines to
    // push it into the history.
    let mut out = String::new();
    for i in 0..ROWS {
        out.push_str(&format!("above {i}\r\n"));
    }
    out.push_str(&format!("{}\r\n", "y".repeat(50)));
    for i in 0..20 {
        out.push_str(&format!("filler {i}\r\n"));
    }
    let mut app = copy_app(&out, "vi");
    // Scroll back until the long line's first row is the last visible row.
    let sb = {
        let win = &app.windows[app.active_idx];
        let pane = active_pane(&win.root, &win.active_path).expect("pane");
        let mut parser = pane.term.lock().expect("parser lock");
        let mut found = None;
        for sb in 0..=SCROLLBACK {
            parser.screen_mut().set_scrollback(sb);
            let s = parser.screen();
            if s.cell(ROWS - 1, 0).is_some_and(|c| c.contents() == "y") && s.row_wrapped(ROWS - 1) {
                found = Some(s.scrollback());
                break;
            }
        }
        found.expect("the long line can sit on the last row")
    };
    app.copy_scroll_offset = sb;
    assert_eq!(eol_from(&mut app, ROWS - 1, 0), (ROWS - 1, 9));
    assert_eq!(app.copy_scroll_offset, sb - 1, "the view moved down one row");
}

// The copy itself is checked through selection_text: yank_selection also
// writes the system clipboard, which a unit test must not touch.
fn one_row_selection_text(p: &mut vt100::Parser, row: u16, c0: u16, c1: u16, cols: u16) -> String {
    let (r, mode) = (row as i64, crate::types::SelectionMode::Char);
    selection_text(p, ROWS, cols, (r, c0), (r, c1), mode, false, 0)
}

#[test]
fn a_one_row_selection_past_the_text_copies_only_the_text() {
    let mut p = vt100::Parser::new(ROWS, COLS, 0);
    p.process(format!("trail   \r\na   b\r\n{CJK}x\r\n").as_bytes());
    let s = &mut p;
    assert_eq!(one_row_selection_text(s, 0, 0, 7, COLS), "trail");
    assert_eq!(one_row_selection_text(s, 0, 0, COLS - 1, COLS), "trail");
    assert_eq!(one_row_selection_text(s, 0, 2, 3, COLS), "ai");
    // Blanks between words inside the selection stay.
    assert_eq!(one_row_selection_text(s, 1, 0, 2, COLS), "a  ");
    assert_eq!(one_row_selection_text(s, 1, 0, 9, COLS), "a   b");
    // A selection wholly past the text copies nothing.
    assert_eq!(one_row_selection_text(s, 0, 20, 30, COLS), "");
    assert_eq!(one_row_selection_text(s, 2, 0, 9, COLS), format!("{CJK}x"));
}
