// Issue #722: copy-mode-vi has no `:` binding, and `send -X goto-line`
// never scrolls.
//
// tmux binds the goto-line prompt to `:` in copy-mode-vi (key-bindings.c:608).
// psmux had no arm for it, so the character fell through to the catch-all in
// `handle_copy_mode_char` and was dropped.
//
// The verb behind the key did not work either. It wrote the number into the
// copy cursor's SCREEN row and left the scroll offset alone, so a line in the
// scrollback could not be reached and a number past the pane height was
// clamped away. tmux `window_copy_goto_line` (window-copy.c:4579-4606) moves
// the scroll offset instead and leaves the cursor row where it was, and it
// reads the number as an absolute line or as a distance from the live bottom
// depending on `copy-mode-line-numbers`
// (`window_copy_line_number_is_absolute`, window-copy.c:4786-4798).
//
// Both live dispatchers are driven here over a real PTY backed pane tree, with
// no psmux server and no session. Registered from src/input.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::copy_line_numbers::CopyLnMode;
use crate::copy_mode::goto_line_offset;
use crate::types::Node;

const ROWS: u16 = 10;
const COLS: u16 = 40;
const SCROLLBACK: usize = 200;
/// Lines written into the fixture pane. More than the pane holds, so there is
/// a history to jump into.
const LINES: usize = 60;

/// A pane's master, child and writer with no pseudo console and no process
/// behind any of them (`util::stub_pane_pty`, added for #695).
fn open_pane_pty(
    rows: u16,
    cols: u16,
) -> (
    Box<dyn portable_pty::MasterPty>,
    Box<dyn portable_pty::Child + Send + Sync>,
    Box<dyn crate::pane::PaneInputSink>,
) {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
    (master, crate::util::StubChild::exited(), writer)
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
        win32_input_latched: false, input_off: false,
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

/// One window, one pane holding `LINES` numbered lines, in copy mode at the
/// live bottom with the cursor parked in the middle of the viewport.
fn copy_app(mode_keys: &str, line_numbers: &str) -> AppState {
    let mut app = AppState::new("issue722".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = mode_keys.to_string();
    app.user_options
        .insert("copy-mode-line-numbers".to_string(), line_numbers.to_string());
    let pane = make_pane(0, ROWS, COLS);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for i in 1..=LINES {
            parser.process(format!("line-{i}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((ROWS / 2, 0));
    app
}

/// Rows of history above the visible area, tmux's `hsize`.
fn hsize(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let pane = crate::tree::active_pane(&win.root, &win.active_path).expect("fixture pane");
    let parser = pane.term.lock().expect("parser lock");
    parser.screen().scrollback_filled()
}

fn offset(app: &AppState) -> usize {
    app.copy_scroll_offset
}

fn typed(app: &AppState) -> &str {
    match app.mode {
        Mode::CopyGoto { ref input } => input.as_str(),
        _ => panic!("expected the goto line prompt to be open, mode is something else"),
    }
}

/// Open the prompt, type a whole line number and accept it, the way the client
/// forwards it: one send-text per character, then the named Enter key.
fn goto(app: &mut AppState, number: &str) {
    crate::input::send_text_to_active(app, ":").unwrap();
    for c in number.chars() {
        crate::input::send_text_to_active(app, &c.to_string()).unwrap();
    }
    crate::input::send_key_to_active(app, "enter").unwrap();
}

// ───────────────────────── the arithmetic ─────────────────────────

#[test]
fn absolute_line_one_parks_on_the_oldest_retained_line() {
    // oy = hsize - (line - 1), so line 1 is the whole history back.
    assert_eq!(goto_line_offset(1, 173, true), 173);
}

#[test]
fn absolute_line_past_the_screen_top_is_the_live_bottom() {
    // hsize + 1 is the first row of the live screen, which is oy = 0.
    assert_eq!(goto_line_offset(174, 173, true), 0);
}

#[test]
fn absolute_clamps_both_ends() {
    // tmux clamps into 1 ..= hsize + 1 rather than refusing the number.
    assert_eq!(goto_line_offset(0, 173, true), 173, "0 clamps up to line 1");
    assert_eq!(goto_line_offset(-1, 173, true), 173, "-1 clamps up to line 1");
    assert_eq!(goto_line_offset(9_999, 173, true), 0, "past the end is the live bottom");
}

#[test]
fn absolute_walks_the_history_one_line_at_a_time() {
    for line in 1..=174i64 {
        let want = 174 - line as usize;
        assert_eq!(goto_line_offset(line, 173, true), want, "line {line}");
    }
}

#[test]
fn default_numbering_counts_back_from_the_live_bottom() {
    // Without absolute numbering the argument IS the offset: oy = lineno.
    assert_eq!(goto_line_offset(0, 173, false), 0);
    assert_eq!(goto_line_offset(97, 173, false), 97);
    assert_eq!(goto_line_offset(173, 173, false), 173);
}

#[test]
fn default_numbering_sends_out_of_range_to_the_oldest_line() {
    // `if (lineno < 0 || lineno > hsize) lineno = hsize;`
    assert_eq!(goto_line_offset(-1, 173, false), 173);
    assert_eq!(goto_line_offset(9_999, 173, false), 173);
}

#[test]
fn an_empty_history_leaves_the_offset_at_zero() {
    for absolute in [true, false] {
        assert_eq!(goto_line_offset(1, 0, absolute), 0, "absolute={absolute}");
        assert_eq!(goto_line_offset(500, 0, absolute), 0, "absolute={absolute}");
    }
}

#[test]
fn goto_line_reads_the_mode_the_way_the_gutter_does() {
    // `copy_line_numbers::is_absolute` already answered this for the gutter
    // (#702), so goto-line asks it rather than deciding again: a number typed
    // at the prompt then always means what the gutter printed. relative and
    // hybrid group with absolute, the way
    // `window_copy_line_number_is_absolute` groups them, because both print an
    // absolute number on the cursor line.
    use crate::copy_line_numbers::is_absolute;
    assert!(is_absolute(CopyLnMode::Absolute));
    assert!(is_absolute(CopyLnMode::Relative));
    assert!(is_absolute(CopyLnMode::Hybrid));
    assert!(!is_absolute(CopyLnMode::Off));
    assert!(!is_absolute(CopyLnMode::Default));

    // And the whole chain from the option string, which is what goto_line reads.
    for name in ["absolute", "relative", "hybrid"] {
        assert!(is_absolute(CopyLnMode::parse(name)), "{name} counts absolute lines");
    }
    for name in ["off", "default"] {
        assert!(!is_absolute(CopyLnMode::parse(name)), "{name} counts back from the bottom");
    }
}

// ───────────────────────── opening the prompt ─────────────────────────

#[test]
fn colon_opens_the_prompt_in_vi_mode() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert_eq!(typed(&app), "", "`:` must open an empty goto line prompt");
}

#[test]
fn the_prompt_says_goto_line() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_text_to_active(&mut app, "4").unwrap();
    let msg = app.status_message.as_ref().map(|(m, _, _)| m.clone()).unwrap_or_default();
    assert_eq!(msg, "(goto line) 4", "the prompt must read like tmux's -p'(goto line)'");
}

#[test]
fn g_still_means_history_top() {
    // The new `:` arm sits next to the `g` arm, so pin that `g` is untouched.
    let mut app = copy_app("vi", "absolute");
    let history = hsize(&app);
    crate::input::send_text_to_active(&mut app, "g").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode), "`g` must not open a prompt");
    assert_eq!(offset(&app), history, "`g` is history-top");
}

#[test]
fn opening_the_prompt_spends_a_pending_count() {
    // A numeric prefix is consumed by the key that follows it (#681), so `10:`
    // must not leave 10 behind to repeat the next motion.
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, "10").unwrap();
    assert_eq!(app.copy_count, Some(10));
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert_eq!(app.copy_count, None, "`:` must spend the pending count");
}

// ───────────────────────── typing in the prompt ─────────────────────────

#[test]
fn characters_are_appended_to_the_number() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    for c in "123".chars() {
        crate::input::send_text_to_active(&mut app, &c.to_string()).unwrap();
    }
    assert_eq!(typed(&app), "123");
}

#[test]
fn one_text_holding_the_key_and_the_number_types_the_number_into_the_prompt() {
    // `send-keys ':500'` and a paste of `:500` arrive as ONE send-text. The
    // characters after `:` must reach the prompt the key just opened, exactly
    // as they do when they arrive one send-text at a time; before, they were
    // run as copy mode keys and became a count nobody asked for.
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":12").unwrap();
    assert_eq!(typed(&app), "12");
    assert_eq!(app.copy_count, None, "the digits must not become a count");
    let history = hsize(&app);
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    assert_eq!(offset(&app), history - 11, "line 12 of the grid");
}

#[test]
fn one_text_holding_a_search_key_and_its_pattern_types_the_pattern() {
    // The same loop served `/` and `?` before `:` existed, with the same
    // defect: `send-keys '?abc'` searched for nothing and ran `a`, `b`, `c`
    // as copy mode keys.
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, "?line-4").unwrap();
    match app.mode {
        Mode::CopySearch { ref input, forward } => {
            assert_eq!(input, "line-4");
            assert!(!forward);
        }
        _ => panic!("`?` must leave the search prompt open"),
    }
}

#[test]
fn text_after_a_key_that_leaves_copy_mode_is_dropped() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, "q:5").unwrap();
    assert!(!app.mode.in_copy(), "`q` leaves copy mode");
    assert!(app.status_message.is_none(), "no prompt opened after copy mode ended");
}

#[test]
fn copy_mode_keys_are_inactive_while_the_prompt_is_open() {
    // `q` exits copy mode and `j` moves the cursor, but not in here.
    let mut app = copy_app("vi", "absolute");
    let row = app.copy_pos.expect("copy_pos").0;
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_text_to_active(&mut app, "q").unwrap();
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    assert_eq!(typed(&app), "qj", "every character belongs to the prompt");
    assert_eq!(app.copy_pos.expect("copy_pos").0, row, "`j` must not move the cursor");
}

#[test]
fn backspace_deletes_the_last_character() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_text_to_active(&mut app, "12").unwrap();
    crate::input::send_key_to_active(&mut app, "backspace").unwrap();
    assert_eq!(typed(&app), "1");
    let msg = app.status_message.as_ref().map(|(m, _, _)| m.clone()).unwrap_or_default();
    assert_eq!(msg, "(goto line) 1", "the prompt must redraw after a backspace");
}

#[test]
fn escape_cancels_and_leaves_the_view_alone() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_text_to_active(&mut app, "1").unwrap();
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode), "esc returns to copy mode");
    assert_eq!(offset(&app), 0, "a cancelled prompt must not scroll");
    assert!(app.status_message.is_none(), "the prompt must not be left on the status line");
}

// ───────────────────────── accepting a number ─────────────────────────

#[test]
fn enter_moves_the_view_to_an_absolute_line() {
    let mut app = copy_app("vi", "absolute");
    let history = hsize(&app);
    assert!(history > 0, "the fixture must have a scrollback to jump into");
    goto(&mut app, "1");
    assert!(matches!(app.mode, Mode::CopyMode), "enter returns to copy mode");
    assert_eq!(offset(&app), history, "line 1 is the oldest retained line");
    assert!(app.status_message.is_none(), "the prompt must be cleared");
}

#[test]
fn enter_reaches_a_line_deeper_than_the_pane_is_tall() {
    // The old implementation wrote the number into the cursor's screen row, so
    // any line past the pane height was unreachable.
    let mut app = copy_app("vi", "absolute");
    let history = hsize(&app);
    let target = 2; // well above ROWS from the top of the buffer
    goto(&mut app, &target.to_string());
    assert_eq!(offset(&app), history - (target - 1));
}

#[test]
fn enter_with_default_numbering_counts_back_from_the_bottom() {
    let mut app = copy_app("vi", "off");
    goto(&mut app, "5");
    assert_eq!(offset(&app), 5, "without absolute numbering the number IS the offset");
}

#[test]
fn a_number_that_is_not_a_number_leaves_the_view_alone() {
    for text in ["abc", "12x", "-", ""] {
        let mut app = copy_app("vi", "absolute");
        goto(&mut app, text);
        assert!(matches!(app.mode, Mode::CopyMode), "{text:?} must still close the prompt");
        assert_eq!(offset(&app), 0, "{text:?} must not scroll");
    }
}

#[test]
fn the_cursor_keeps_its_screen_row() {
    // tmux moves oy and leaves cy alone, so the cursor comes to mean whichever
    // content line is now under it.
    let mut app = copy_app("vi", "absolute");
    let row = app.copy_pos.expect("copy_pos").0;
    goto(&mut app, "1");
    assert_eq!(app.copy_pos.expect("copy_pos").0, row);
    assert!(
        app.copy_pos_scroll_offset.is_none(),
        "a scroll reads the endpoint against the current offset (types.rs)"
    );
}

// ───────────────────────── the send-keys -X verb ─────────────────────────

#[test]
fn the_verb_scrolls_with_the_argument_the_dispatchers_hand_it() {
    // Both dispatchers drop flags before joining the operands, so
    // `send -X goto-line -- 1` and `send -X goto-line 1` arrive the same way,
    // and the server passes everything after the verb name.
    for arg in [" 1", "  1  "] {
        let mut app = copy_app("vi", "absolute");
        let history = hsize(&app);
        crate::copy_mode::run_goto_line(&mut app, arg);
        assert_eq!(offset(&app), history, "arg {arg:?}");
    }
}

#[test]
fn the_verb_ignores_an_argument_outside_the_range_tmux_reads() {
    // strtonum(linestr, -1, INT_MAX, &errstr): anything below -1 or above
    // INT_MAX is an error and the view does not move.
    for arg in [" -2", " 72272299"] {
        let mut app = copy_app("vi", "absolute");
        crate::copy_mode::run_goto_line(&mut app, arg);
        assert_eq!(offset(&app), 0, "arg {arg:?}");
    }
}

// ───────────────────────── the prompt is still copy mode ─────────────────────────

#[test]
fn in_copy_covers_every_copy_mode_state() {
    assert!(Mode::CopyMode.in_copy());
    assert!(Mode::CopySearch { input: String::new(), forward: true }.in_copy());
    assert!(Mode::CopyGoto { input: String::new() }.in_copy());
    assert!(!Mode::Passthrough.in_copy());
    assert!(!Mode::ClockMode.in_copy());
    assert!(!Mode::CommandPrompt { input: String::new(), cursor: 0 }.in_copy());
}

#[test]
fn pane_in_mode_and_pane_mode_answer_while_the_prompt_is_open() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert_eq!(crate::format::expand_format("#{pane_in_mode}", &app), "1");
    assert_eq!(crate::format::expand_format("#{pane_mode}", &app), "copy-mode");
}

#[test]
fn a_focus_change_takes_the_cancelled_prompt_off_the_status_line() {
    // The prompt is a sticky status message and the status line is not per
    // pane, so the text has to go with the prompt: before, `(goto line) 1`
    // stayed on the status line after select-pane, over a pane that was not
    // even in copy mode.
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":1").unwrap();
    crate::copy_mode::switch_with_copy_save(&mut app, |_| {});
    assert!(matches!(app.mode, Mode::CopyMode));
    assert!(app.status_message.is_none(), "the cancelled prompt must not stay on the status line");
}

#[test]
fn a_focus_change_back_to_a_parked_search_shows_that_search_again() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, "/ab").unwrap();
    app.status_message = None;
    crate::copy_mode::switch_with_copy_save(&mut app, |_| {});
    let msg = app.status_message.as_ref().map(|(m, _, _)| m.clone()).unwrap_or_default();
    assert_eq!(msg, "(search down) ab", "the restored search prompt must be on the status line");
}

#[test]
fn switching_panes_cancels_the_prompt_and_stays_in_copy_mode() {
    let mut app = copy_app("vi", "absolute");
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::copy_mode::save_copy_state_to_pane(&mut app);
    crate::copy_mode::restore_copy_state_from_pane(&mut app);
    assert!(
        matches!(app.mode, Mode::CopyMode),
        "a half typed line number is not worth carrying between panes"
    );
}
