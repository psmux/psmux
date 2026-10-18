// Issue #855: copy mode cursor-left and cursor-right took two presses to cross
// a wide character: they moved one cell at a time, and the second cell of a
// CJK character or an emoji sequence (`is_wide_continuation`) is padding,
// where the cursor still looks as if it is on the character.
//
// Right steps over the padding as tmux does (grid-reader.c,
// grid_reader_cursor_right); those columns are what tmux 3.7c reported in
// `#{copy_cursor_x}` for the same lines. tmux's left step and its cursor-up and
// cursor-down can stop on padding, and tmux shows that with the terminal's own
// cursor. psmux reverses the cell under the cursor, so on padding only the
// right half of the character was highlighted: here left, up and down move to
// the character's first cell instead, which is where tmux's columns differ.
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

/// 日本語abc: three two-cell characters, then three one-cell ones.
const CJK: &str = "\u{65e5}\u{672c}\u{8a9e}abc";
/// A ZWJ family, a flag, a thumbs up with a skin tone, a heart with VS16, a
/// keycap, then `a`. Each sequence is one two-cell character, as in tmux.
const EMOJI: &str = concat!(
    "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}",
    "\u{1f1ef}\u{1f1f5}",
    "\u{1f44d}\u{1f3fd}",
    "\u{2764}\u{fe0f}",
    "1\u{fe0f}\u{20e3}",
    "a",
);
const ASCII: &str = "abc def";

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

/// One window, one pane that has printed `output`, in copy mode.
fn copy_app(output: &str) -> AppState {
    let mut app = AppState::new("copy_cursor_wide".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = "vi".to_string();
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

fn lines_app() -> AppState {
    copy_app(&format!("{CJK}\r\n{EMOJI}\r\n{ASCII}\r\n"))
}

/// The columns the cursor visits from (`row`, `col`), one step `dx` at a time.
fn walk(app: &mut AppState, row: u16, col: u16, dx: i16, steps: usize) -> Vec<u16> {
    app.copy_pos = Some((row, col));
    (0..steps)
        .map(|_| {
            move_copy_cursor(app, dx, 0);
            let (r, c) = app.copy_pos.expect("copy_pos");
            assert_eq!(r, row, "a left or right step stays on its row");
            c
        })
        .collect()
}

fn with_screen<T>(app: &AppState, f: impl FnOnce(&vt100::Screen) -> T) -> T {
    let win = &app.windows[app.active_idx];
    let pane = active_pane(&win.root, &win.active_path).expect("pane");
    let parser = pane.term.lock().expect("parser lock");
    f(parser.screen())
}

#[test]
fn right_crosses_each_cjk_character_in_one_step() {
    let mut app = lines_app();
    // tmux 3.7c: 0 -> 2 -> 4 -> 6 -> 7 -> 8.
    assert_eq!(walk(&mut app, 0, 0, 1, 5), vec![2, 4, 6, 7, 8]);
}

#[test]
fn left_lands_on_the_first_cell_of_each_cjk_character() {
    let mut app = lines_app();
    // tmux 3.7c: 8 -> 7 -> 6 -> 5 -> 3 -> 1, the second cell of each.
    assert_eq!(walk(&mut app, 0, 8, -1, 5), vec![7, 6, 4, 2, 0]);
    // Column 0 stays put. tmux goes on to the end of the line above there;
    // psmux has never wrapped a left step.
    assert_eq!(walk(&mut app, 0, 0, -1, 1), vec![0]);
}

#[test]
fn left_from_a_second_cell_goes_to_the_character_before() {
    let mut app = lines_app();
    // A cursor left on padding by an older route steps from the character's
    // first cell, as tmux does.
    assert_eq!(walk(&mut app, 0, 3, -1, 1), vec![0]);
    assert_eq!(walk(&mut app, 0, 1, -1, 1), vec![0]);
}

#[test]
fn each_emoji_sequence_is_one_two_cell_character() {
    let app = lines_app();
    with_screen(&app, |screen| {
        let firsts = [
            (0, "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}"),
            (2, "\u{1f1ef}\u{1f1f5}"),
            (4, "\u{1f44d}\u{1f3fd}"),
            (6, "\u{2764}\u{fe0f}"),
            (8, "1\u{fe0f}\u{20e3}"),
        ];
        for (col, text) in firsts {
            let cell = screen.cell(1, col).expect("cell");
            assert_eq!(cell.contents(), text, "column {col}");
            assert!(screen.cell(1, col + 1).expect("cell").is_wide_continuation(), "column {}", col + 1);
        }
        assert_eq!(screen.cell(1, 10).expect("cell").contents(), "a");
    });
}

#[test]
fn right_crosses_each_emoji_sequence_in_one_step() {
    let mut app = lines_app();
    // tmux 3.7c: 0 -> 2 -> 4 -> 6 -> 8 -> 10 -> 11.
    assert_eq!(walk(&mut app, 1, 0, 1, 6), vec![2, 4, 6, 8, 10, 11]);
}

#[test]
fn left_lands_on_the_first_cell_of_each_emoji_sequence() {
    let mut app = lines_app();
    // tmux 3.7c: 11 -> 10 -> 9 -> 7 -> 5 -> 3 -> 1, the second cell of each.
    assert_eq!(walk(&mut app, 1, 11, -1, 6), vec![10, 8, 6, 4, 2, 0]);
}

#[test]
fn a_line_of_one_cell_characters_still_steps_one_cell() {
    let mut app = lines_app();
    assert_eq!(walk(&mut app, 2, 0, 1, 4), vec![1, 2, 3, 4]);
    assert_eq!(walk(&mut app, 2, 4, -1, 4), vec![3, 2, 1, 0]);
}

#[test]
fn a_wide_character_in_the_last_two_columns_stops_right_on_its_first_cell() {
    let line = format!("{}\u{65e5}", "x".repeat(COLS as usize - 2));
    let mut app = copy_app(&format!("{line}\r\n"));
    let first = COLS - 2;
    // Its second cell is the last column, so there is nothing to step to.
    assert_eq!(walk(&mut app, 0, first - 1, 1, 3), vec![first, first, first]);
    assert_eq!(walk(&mut app, 0, first, -1, 1), vec![first - 1]);
}

#[test]
fn a_wide_character_in_the_scrollback_is_crossed_in_one_step() {
    let mut out = format!("{CJK}\r\n");
    for i in 0..30 {
        out.push_str(&format!("filler {i}\r\n"));
    }
    let mut app = copy_app(&out);
    {
        let win = &app.windows[app.active_idx];
        let pane = active_pane(&win.root, &win.active_path).expect("pane");
        let mut parser = pane.term.lock().expect("parser lock");
        parser.screen_mut().set_scrollback(SCROLLBACK);
    }
    let row = with_screen(&app, |screen| {
        (0..ROWS).find(|&r| screen.cell(r, 0).is_some_and(|c| c.contents() == "\u{65e5}"))
    })
    .expect("the CJK line is on screen once scrolled back");
    assert_eq!(walk(&mut app, row, 0, 1, 4), vec![2, 4, 6, 7]);
    assert_eq!(walk(&mut app, row, 6, -1, 3), vec![4, 2, 0]);
}

#[test]
fn up_and_down_onto_a_second_cell_land_on_the_first() {
    let mut app = lines_app();
    // Column 3 of the `abc def` line is the space; above it, column 3 is the
    // second cell of the flag, and above that the second cell of 本. tmux
    // keeps column 3 on both rows.
    app.copy_pos = Some((2, 3));
    move_copy_cursor(&mut app, 0, -1);
    assert_eq!(app.copy_pos, Some((1, 2)));
    app.copy_pos = Some((2, 5));
    move_copy_cursor(&mut app, 0, -1);
    move_copy_cursor(&mut app, 0, -1);
    assert_eq!(app.copy_pos, Some((0, 4)));
    app.copy_pos = Some((0, 5));
    move_copy_cursor(&mut app, 0, 1);
    assert_eq!(app.copy_pos, Some((1, 4)));
}

#[test]
fn up_and_down_onto_a_first_cell_or_a_narrow_one_keep_the_column() {
    let mut app = lines_app();
    app.copy_pos = Some((1, 4));
    move_copy_cursor(&mut app, 0, -1);
    assert_eq!(app.copy_pos, Some((0, 4)));
    app.copy_pos = Some((1, 3));
    move_copy_cursor(&mut app, 0, 1);
    assert_eq!(app.copy_pos, Some((2, 3)));
}

#[test]
fn up_into_the_scrollback_onto_a_second_cell_lands_on_the_first() {
    // Nine fillers and the empty line the cursor is on fill the ten rows, so
    // the CJK line is the one row just above the top of the screen.
    let mut out = format!("{CJK}\r\n");
    for i in 0..(ROWS as usize - 1) {
        out.push_str(&format!("filler {i}\r\n"));
    }
    let mut app = copy_app(&out);
    // Column 5 is the second cell of 語.
    let top = with_screen(&app, |screen| {
        (0..ROWS).find(|&r| screen.cell(r, 0).is_some_and(|c| c.contents() == "f"))
    })
    .expect("a filler row");
    assert_eq!(top, 0, "the CJK line is in the scrollback");
    app.copy_pos = Some((0, 5));
    move_copy_cursor(&mut app, 0, -1);
    let pos = app.copy_pos.expect("copy_pos");
    with_screen(&app, |screen| assert_eq!(screen.cell(pos.0, 0).expect("cell").contents(), "\u{65e5}"));
    assert_eq!(pos, (0, 4));
}
