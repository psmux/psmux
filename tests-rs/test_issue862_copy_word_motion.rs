// Issue #862: copy mode word motions, select-word and the vi word text
// objects, driven through copy mode over a pane with a stub PTY (no psmux
// server).
//
// The motions themselves are tested against tmux 3.7c's positions in
// test_issue862_copy_word_motion_reader.rs. These tests check what copy mode
// does with them: the key table, scrolling the view to the line a motion lands on, the
// selection select-word makes, and the text the selection copies.
//
// Registered from src/copy_mode.rs.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use crate::types::Node;

const ROWS: u16 = 10;
const COLS: u16 = 60;
const SCROLLBACK: usize = 200;

const LINES: [&str; 4] = [
    "foo-bar_baz qux.quux  a@b:c",
    "path/to/file.txt (x+y)=z  ~/dir",
    "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} abc\u{65e5}\u{672c} def",
    "  lead  spaces, \"quoted\" end.",
];

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

/// One window, one pane that has printed `lines`, in copy mode with `keys`.
fn copy_app(lines: &[&str], keys: &str) -> AppState {
    let mut app = AppState::new("copy_word_motion".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = keys.to_string();
    let pane = make_pane(0, ROWS, COLS);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for line in lines {
            parser.process(format!("{line}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    app.windows.push(win);
    app.active_idx = 0;
    app.last_window_area = ratatui::layout::Rect::new(0, 0, COLS, ROWS);
    enter_copy_mode(&mut app);
    app
}

fn at(app: &mut AppState, row: u16, col: u16) -> &mut AppState {
    app.copy_pos = Some((row, col));
    app.copy_anchor = None;
    app
}

fn yanked(app: &mut AppState) -> String {
    yank_selection(app).expect("yank");
    app.paste_top().unwrap_or_default().to_string()
}

fn scroll_offset(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let pane = active_pane(&win.root, &win.active_path).expect("pane");
    let parser = pane.term.lock().expect("parser lock");
    parser.screen().scrollback()
}

#[test]
fn the_default_word_separators_are_tmuxs() {
    let app = AppState::new("s".to_string());
    assert_eq!(app.word_separators, crate::grid_reader::DEFAULT_WORD_SEPARATORS);
}

#[test]
fn each_motion_follows_the_key_table() {
    let mut app = copy_app(&LINES, "vi");
    // `-` and `.` split words, `_` does not (tmux 3.7c: 0:3 0:4 0:12).
    move_word_forward(at(&mut app, 0, 0));
    assert_eq!(app.copy_pos, Some((0, 3)));
    move_word_forward(&mut app);
    move_word_forward(&mut app);
    assert_eq!(app.copy_pos, Some((0, 12)));
    // vi `e` stops on the word's last character, emacs on the cell after.
    move_word_end(at(&mut app, 0, 4));
    assert_eq!(app.copy_pos, Some((0, 10)));
    app.mode_keys = "emacs".to_string();
    move_word_end(at(&mut app, 0, 4));
    assert_eq!(app.copy_pos, Some((0, 11)));
    app.mode_keys = "vi".to_string();
    move_word_backward(at(&mut app, 0, 10));
    assert_eq!(app.copy_pos, Some((0, 4)));
    // Spaces alone: `path/to/file.txt` is one WORD.
    move_word_forward_big(at(&mut app, 1, 0));
    assert_eq!(app.copy_pos, Some((1, 17)));
    move_word_end_big(at(&mut app, 1, 0));
    assert_eq!(app.copy_pos, Some((1, 15)));
    move_word_backward_big(at(&mut app, 1, 15));
    assert_eq!(app.copy_pos, Some((1, 0)));
}

#[test]
fn a_cjk_run_and_the_letters_next_to_it_are_one_word() {
    let mut app = copy_app(&LINES, "vi");
    // tmux 3.7c: 2:17 2:25, a word at a time, not a character.
    move_word_forward(at(&mut app, 2, 0));
    assert_eq!(app.copy_pos, Some((2, 17)));
    move_word_forward(&mut app);
    assert_eq!(app.copy_pos, Some((2, 25)));
    // vi `e` over `abc日本` stops on the second cell of 本, as tmux's does.
    move_word_end(at(&mut app, 2, 17));
    assert_eq!(app.copy_pos, Some((2, 23)));
    move_word_backward(at(&mut app, 2, 25));
    assert_eq!(app.copy_pos, Some((2, 17)));
}

#[test]
fn a_motion_to_a_line_above_the_view_scrolls_it_up_to_the_top_row() {
    let mut lines = vec!["qux.quux"];
    let fillers: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
    lines.extend(fillers.iter().map(String::as_str));
    let mut app = copy_app(&lines, "vi");
    // Scroll back until `f0` (absolute line 1) is the top row; `qux.quux`
    // (line 0) is the one above it.
    {
        let win = &app.windows[app.active_idx];
        let pane = active_pane(&win.root, &win.active_path).expect("pane");
        pane.term.lock().expect("parser lock").screen_mut().set_scrollback(11);
    }
    app.copy_scroll_offset = 11;
    move_word_backward(at(&mut app, 0, 0));
    assert_eq!(app.copy_pos, Some((0, 4)), "on `quux`, at the top row");
    assert_eq!(scroll_offset(&app), 12);
    assert_eq!(app.copy_scroll_offset, 12);
}

#[test]
fn a_motion_to_a_line_below_the_view_scrolls_it_down_to_the_bottom_row() {
    let mut lines: Vec<String> = (0..20).map(|i| format!("f{i}")).collect();
    lines.push("last word".to_string());
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let mut app = copy_app(&refs, "vi");
    {
        let win = &app.windows[app.active_idx];
        let pane = active_pane(&win.root, &win.active_path).expect("pane");
        pane.term.lock().expect("parser lock").screen_mut().set_scrollback(3);
    }
    app.copy_scroll_offset = 3;
    // The bottom row shows `f17`; the next word is `f18` on the line below.
    move_word_forward(at(&mut app, ROWS - 1, 0));
    assert_eq!(app.copy_pos, Some((ROWS - 1, 0)));
    assert_eq!(scroll_offset(&app), 2);
}

#[test]
fn select_word_copies_what_tmux_copies_with_vi_keys() {
    let mut app = copy_app(&LINES, "vi");
    let cases: &[(u16, u16, &str)] = &[
        (0, 0, "foo"),
        (0, 5, "bar_baz"),
        // On a separator: the separators and the word after (tmux 3.7c).
        (0, 3, "-bar_baz"),
        // On whitespace: the word before.
        (0, 21, "quux"),
        (2, 0, "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}"),
        (2, 20, "abc\u{65e5}\u{672c}"),
        (3, 16, "\"quoted"),
    ];
    for &(row, col, want) in cases {
        select_word(at(&mut app, row, col));
        assert_eq!(yanked(&mut app), want, "at {row}:{col}");
    }
}

#[test]
fn select_word_from_leading_whitespace_takes_the_word_on_the_line_above() {
    let mut app = copy_app(&LINES, "vi");
    select_word(at(&mut app, 3, 0));
    assert_eq!(app.copy_pos, Some((2, 27)), "tmux 3.7c: 2:27");
    assert_eq!(yanked(&mut app), "def");
}

#[test]
fn select_word_with_emacs_keys_copies_the_word_too() {
    let mut app = copy_app(&LINES, "emacs");
    // tmux puts the cursor on the cell after the word, which an emacs
    // selection leaves out, so the copy is the word (tmux 3.7c).
    for &(row, col, end, want) in &[
        (0u16, 5u16, 11u16, "bar_baz"),
        (0, 0, 3, "foo"),
        (2, 20, 24, "abc\u{65e5}\u{672c}"),
    ] {
        select_word(at(&mut app, row, col));
        assert_eq!(app.copy_pos, Some((row, end)), "cursor at {row}:{col}");
        assert_eq!(yanked(&mut app), want, "at {row}:{col}");
    }
}

#[test]
fn the_vi_text_objects_use_the_same_words() {
    let mut app = copy_app(&LINES, "vi");
    select_inner_word(at(&mut app, 2, 18));
    assert_eq!(yanked(&mut app), "abc\u{65e5}\u{672c}");
    select_a_word(at(&mut app, 2, 18));
    assert_eq!(yanked(&mut app), "abc\u{65e5}\u{672c} ");
    select_inner_word(at(&mut app, 0, 5));
    assert_eq!(yanked(&mut app), "bar_baz");
    select_inner_word_big(at(&mut app, 1, 3));
    assert_eq!(yanked(&mut app), "path/to/file.txt");
    select_a_word_big(at(&mut app, 1, 3));
    assert_eq!(yanked(&mut app), "path/to/file.txt ");
}
