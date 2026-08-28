// Issue #612: copy mode search only worked across the visible part of a buffer.
//
// Reported by kirill146: "All of `?`, `/`, `Ctrl+s`, `Ctrl+r` can't see past the
// on-screen area of the buffer."
//
// Root cause: `copy_mode::search_copy_mode` scanned `0..p.last_rows`, the rows
// the vt100 screen currently frames, and stored every hit as a VISIBLE row
// index. The scrollback history was never read and nothing ever moved the
// viewport, so a term that had scrolled off the top could not be found and a
// hit could never be brought back into view.
//
// tmux walks the whole grid. `window_copy_search_jump` (window-copy.c:4470)
// iterates absolute line numbers between 0 and `gd->hsize + gd->sy - 1` and
// then calls `window_copy_scroll_to`, which parks a match that is off screen a
// quarter of a screen up from the bottom (`gap = gd->sy / 4`).
//
// Measured against real tmux 3.4 in WSL with 400 numbered lines on an 80x24
// pane: `search-backward LINE_5` from the bottom lands on LINE_59 with
// `scroll_position` 338. psmux now reports exactly the same pair.
//
// These tests drive the REAL search over a real PTY-backed pane tree carrying a
// genuine vt100 scrollback (no psmux server and no session is created).
// Registered from src/copy_mode.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const ROWS: u16 = 24;
const COLS: u16 = 80;
const SCROLLBACK: usize = 2000;
const TOTAL_LINES: usize = 400;

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
        let child = match pair.slave.spawn_command(cmd) {
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
        mouse_input_cache: None,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
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

/// One window, one pane holding LINE_1 .. LINE_400 on a 24 row screen, so only
/// the last two dozen lines are visible and everything else is in history.
/// The pane is in copy mode with the viewport at the live bottom, which is
/// where a user lands after pressing prefix + [.
fn search_app() -> AppState {
    let mut app = AppState::new("issue612".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    let pane = make_pane(0, ROWS, COLS);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for i in 1..=TOTAL_LINES {
            parser.process(format!("LINE_{i}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((ROWS - 1, 0));
    app
}

/// The text of the line the copy cursor sits on, read the way `#{copy_cursor_line}`
/// reads it: through the visible screen at the current scroll offset.
fn cursor_line(app: &AppState) -> String {
    let (r, _) = app.copy_pos.expect("copy_pos must be set in copy mode");
    let win = &app.windows[app.active_idx];
    let p = active_pane(&win.root, &win.active_path).expect("active pane");
    let parser = p.term.lock().expect("parser lock");
    let screen = parser.screen();
    let mut text = String::new();
    for c in 0..p.last_cols {
        if let Some(cell) = screen.cell(r, c) {
            let t = cell.contents();
            if t.is_empty() { text.push(' '); } else { text.push_str(t); }
        } else { text.push(' '); }
    }
    text.trim_end().to_string()
}

/// The first visible row's text, which proves the viewport really moved.
fn top_visible_line(app: &AppState) -> String {
    let win = &app.windows[app.active_idx];
    let p = active_pane(&win.root, &win.active_path).expect("active pane");
    let parser = p.term.lock().expect("parser lock");
    let screen = parser.screen();
    let mut text = String::new();
    for c in 0..p.last_cols {
        if let Some(cell) = screen.cell(0, c) {
            let t = cell.contents();
            if t.is_empty() { text.push(' '); } else { text.push_str(t); }
        } else { text.push(' '); }
    }
    text.trim_end().to_string()
}

/// The pane's live scroll offset, which is what `#{scroll_position}` reports.
fn pane_scrollback(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let p = active_pane(&win.root, &win.active_path).expect("active pane");
    let parser = p.term.lock().expect("parser lock");
    parser.screen().scrollback()
}

// ═══════════════ the reported bug ═══════════════

#[test]
fn backward_search_reaches_a_term_far_above_the_visible_screen() {
    let mut app = search_app();
    assert_eq!(cursor_line(&app), "", "the pane starts parked below the last line of output");

    crate::copy_mode::search_copy_mode(&mut app, "LINE_120", false);

    assert!(!app.copy_search_matches.is_empty(),
        "`?` LINE_120 must find the line even though it scrolled off the top hundreds of lines ago");
    assert_eq!(cursor_line(&app), "LINE_120",
        "the copy cursor must land ON the match, not stay where it was");
    assert!(app.copy_scroll_offset > 0,
        "the viewport must scroll back into history to show the match, offset was {}",
        app.copy_scroll_offset);
    assert_eq!(app.copy_scroll_offset, pane_scrollback(&app),
        "app.copy_scroll_offset and the pane's own scrollback must agree after a search jump");
}

#[test]
fn forward_search_reaches_a_term_far_below_the_viewport() {
    let mut app = search_app();
    // Park the viewport at the very top of the history, as `g` / history-top does.
    crate::copy_mode::scroll_to_top(&mut app);
    app.copy_pos = Some((ROWS - 1, 0));
    let top_offset = app.copy_scroll_offset;
    assert!(top_offset > 0, "history-top must leave a nonzero scroll offset");

    crate::copy_mode::search_copy_mode(&mut app, "LINE_390", true);

    assert!(!app.copy_search_matches.is_empty(),
        "`/` LINE_390 must find a line that is hundreds of rows BELOW the viewport");
    assert_eq!(cursor_line(&app), "LINE_390", "the copy cursor must land on the forward match");
    assert!(app.copy_scroll_offset < top_offset,
        "the viewport must scroll down toward the live end, offset went {top_offset} -> {}",
        app.copy_scroll_offset);
}

#[test]
fn a_match_already_on_screen_does_not_move_the_viewport() {
    // tmux window_copy_scroll_to leaves data->oy alone when the target line is
    // already framed. Only the cursor moves.
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_395", false);
    assert_eq!(cursor_line(&app), "LINE_395");
    assert_eq!(app.copy_scroll_offset, 0,
        "a hit that is already visible must not scroll the pane");
}

#[test]
fn one_line_above_the_screen_is_enough_to_scroll() {
    // The exact boundary the reporter hit: the last visible row is LINE_400 and
    // the screen holds 24 rows, so LINE_377 is the first line out of reach of
    // the old visible-only scan.
    let mut app = search_app();
    let first_visible = top_visible_line(&app);
    assert!(first_visible.starts_with("LINE_"), "sanity: the screen starts on a numbered line, got {first_visible:?}");
    let first_n: usize = first_visible.trim_start_matches("LINE_").parse().expect("numbered line");

    let just_above = format!("LINE_{}", first_n - 1);
    crate::copy_mode::search_copy_mode(&mut app, &just_above, false);
    assert_eq!(cursor_line(&app), just_above,
        "the line immediately above the viewport must be reachable");
    assert!(app.copy_scroll_offset > 0, "reaching it must scroll the viewport");
}

// ═══════════════ tmux window_copy_scroll_to parity ═══════════════

#[test]
fn an_offscreen_match_is_parked_a_quarter_screen_up_from_the_bottom() {
    // window-copy.c window_copy_scroll_to: gap = gd->sy / 4, offset = py + gap
    // - gd->sy, so a 24 row screen puts the match on row 24 - 6 - 1 = 17 ...
    // measured against real tmux 3.4 the row is 18 for this geometry because
    // the match line itself is included. Pin the measured tmux number.
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_120", false);
    let (row, col) = app.copy_pos.expect("copy_pos");
    assert_eq!(row, 18, "tmux 3.4 parks an offscreen backward match on row 18 of a 24 row screen");
    assert_eq!(col, 0, "the cursor sits at the first column of the match");
    assert_eq!(top_visible_line(&app), "LINE_102",
        "the quarter screen gap must leave exactly 18 lines of context above the match");
}

#[test]
fn search_starts_from_the_cursor_and_walks_the_direction_asked_for() {
    // "LINE_5" matches LINE_5 and LINE_50..LINE_59. Searching backward from the
    // live bottom must stop at the LAST of those, LINE_59, exactly as tmux 3.4
    // does (measured: copy_cursor_line=LINE_59 scroll_position=338).
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_5", false);
    assert_eq!(cursor_line(&app), "LINE_59",
        "a backward search must stop at the nearest match above the cursor, not the topmost one");
}

#[test]
fn search_again_walks_on_in_the_same_direction() {
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_5", false);
    assert_eq!(cursor_line(&app), "LINE_59");
    crate::copy_mode::search_next(&mut app);
    assert_eq!(cursor_line(&app), "LINE_58",
        "`n` after a backward search must continue upward (tmux search-again keeps the direction)");
    crate::copy_mode::search_next(&mut app);
    assert_eq!(cursor_line(&app), "LINE_57");
}

#[test]
fn search_reverse_walks_back_the_other_way() {
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_5", false);
    crate::copy_mode::search_next(&mut app);
    assert_eq!(cursor_line(&app), "LINE_58");
    crate::copy_mode::search_prev(&mut app);
    assert_eq!(cursor_line(&app), "LINE_59",
        "`N` must undo the last `n` step");
}

#[test]
fn every_match_in_the_whole_buffer_is_collected() {
    // "LINE_1" appears on LINE_1, LINE_10..19, LINE_100..199: 1 + 10 + 100 = 111
    // lines, and only the last handful were ever on screen.
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "LINE_1", false);
    assert_eq!(app.copy_search_matches.len(), 111,
        "the scan must cover the scrollback history, not just the 24 visible rows");
}

#[test]
fn the_search_is_case_insensitive_across_history_too() {
    let mut app = search_app();
    crate::copy_mode::search_copy_mode(&mut app, "line_120", false);
    assert_eq!(cursor_line(&app), "LINE_120",
        "a lowercase term must still match an uppercase line held in history");
}

#[test]
fn a_term_that_is_nowhere_leaves_the_cursor_alone() {
    let mut app = search_app();
    let before = app.copy_pos;
    let before_offset = app.copy_scroll_offset;
    crate::copy_mode::search_copy_mode(&mut app, "NOT_IN_THIS_BUFFER", false);
    assert!(app.copy_search_matches.is_empty(), "a term that does not exist must find nothing");
    assert_eq!(app.copy_pos, before, "a failed search must not move the cursor");
    assert_eq!(app.copy_scroll_offset, before_offset, "a failed search must not scroll the pane");
}

#[test]
fn searching_does_not_leave_the_pane_scrolled_somewhere_random() {
    // Reading the history walks the scroll offset across the whole buffer. If
    // that walk is not undone, a failed search would strand the viewport in the
    // middle of the history.
    let mut app = search_app();
    crate::copy_mode::scroll_copy_up(&mut app, 40);
    let parked = app.copy_scroll_offset;
    crate::copy_mode::search_copy_mode(&mut app, "NOTHING_MATCHES_THIS", true);
    assert_eq!(pane_scrollback(&app), parked,
        "a search that finds nothing must restore the scroll offset it borrowed");
}

// ═══════════════ the interactive `?` / `/` / Ctrl+r / Ctrl+s path ═══════════════

/// Drive the CopySearch prompt exactly as a user does: press the opener key,
/// type the term one character at a time, then press Enter. `opener` is either
/// a literal `?` / `/` (which reaches `handle_copy_mode_char`) or a named key
/// like `C-r` / `C-s` (which reaches `send_key_to_active`).
fn type_search(app: &mut AppState, opener: &str, term: &str) {
    if opener.len() == 1 {
        crate::input::send_text_to_active(app, opener).unwrap();
    } else {
        crate::input::send_key_to_active(app, opener).unwrap();
    }
    assert!(matches!(app.mode, Mode::CopySearch { .. }),
        "{opener} must open the copy mode search prompt");
    for ch in term.chars() {
        crate::input::send_text_to_active(app, &ch.to_string()).unwrap();
    }
    // The CLI lowercases named keys before they reach this dispatcher.
    crate::input::send_key_to_active(app, "enter").unwrap();
}

#[test]
fn the_question_mark_prompt_reaches_history() {
    let mut app = search_app();
    type_search(&mut app, "?", "LINE_120");
    assert!(matches!(app.mode, Mode::CopyMode), "Enter must commit the search and return to copy mode");
    assert_eq!(app.copy_search_query, "LINE_120");
    assert_eq!(cursor_line(&app), "LINE_120",
        "the `?` prompt must reach a match hundreds of lines above the screen");
}

#[test]
fn the_slash_prompt_reaches_history() {
    let mut app = search_app();
    crate::copy_mode::scroll_to_top(&mut app);
    app.copy_pos = Some((0, 0));
    type_search(&mut app, "/", "LINE_390");
    assert_eq!(cursor_line(&app), "LINE_390",
        "the `/` prompt must reach a match far below the viewport");
}

#[test]
fn ctrl_r_reaches_history() {
    let mut app = search_app();
    type_search(&mut app, "C-r", "LINE_120");
    assert_eq!(cursor_line(&app), "LINE_120",
        "emacs Ctrl+r must reach a match hundreds of lines above the screen");
}

#[test]
fn ctrl_s_reaches_history() {
    let mut app = search_app();
    crate::copy_mode::scroll_to_top(&mut app);
    app.copy_pos = Some((0, 0));
    type_search(&mut app, "C-s", "LINE_390");
    assert_eq!(cursor_line(&app), "LINE_390",
        "emacs Ctrl+s must reach a match far below the viewport");
}

// The `send-keys -X search-backward <term>` scripting surface lives in the
// server command loop and is covered end to end by
// tests/test_issue612_copy_search_scrollback.ps1.
