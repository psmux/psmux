// Issue #596: Ctrl+P / Ctrl+N in copy mode move the cursor, they do not scroll,
// and Ctrl+Up / Ctrl+Down did nothing at all.
//
// What the reporter saw is real and it is also correct: tmux 3.4 binds
// `C-p` to `cursor-up` and `C-n` to `cursor-down` in the copy-mode table
// (key-bindings.c:571 and :572) and leaves both unbound in copy-mode-vi.
// The keys that scroll the viewport by one line in tmux are `C-Up` and
// `C-Down`, bound in BOTH tables (key-bindings.c:635/:636 for copy-mode and
// :727/:728 for copy-mode-vi). psmux had no arm for those at all, so a real
// Ctrl+Arrow in copy mode was swallowed with no effect.
//
// Two dispatchers had drifted apart on the same keys. The live path is
// `input::send_key_to_active`, reached by `send-key C-p` from the attached
// client and by `send-keys -t s C-p` from the CLI. `input::handle_key` is the
// pre-server dispatcher, kept because Rust tests exercise it, and it was still
// calling `scroll_copy_up`/`scroll_copy_down` for C-p/C-n. These tests pin both
// dispatchers to the same tmux semantics so they cannot drift again.
//
// The tests drive the REAL functions over a real PTY-backed pane tree (no psmux
// server and no session is created). Registered from src/input.rs.

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
        wheel_auth: None,
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

/// One window, one pane holding 60 numbered lines, already in copy mode with the
/// cursor parked in the middle of the viewport so neither a cursor motion nor a
/// scroll is clamped by an edge.
fn copy_app(mode_keys: &str) -> AppState {
    let mut app = AppState::new("issue596".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = mode_keys.to_string();
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
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((ROWS / 2, 0));
    app
}

fn row(app: &AppState) -> u16 {
    app.copy_pos.expect("copy_pos must be tracked in copy mode").0
}

fn offset(app: &AppState) -> usize {
    app.copy_scroll_offset
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

// ══════════════ live path: input::send_key_to_active ══════════════
// This is the function the attached client reaches with `send-key C-p` and the
// CLI reaches with `send-keys -t s C-p`.

#[test]
fn live_ctrl_p_moves_the_cursor_up_without_scrolling() {
    for mode in ["vi", "emacs"] {
        let mut app = copy_app(mode);
        let (r0, o0) = (row(&app), offset(&app));
        crate::input::send_key_to_active(&mut app, "C-p").unwrap();
        assert_eq!(row(&app), r0 - 1, "mode-keys={mode}: C-p must move the cursor up one row (tmux cursor-up)");
        assert_eq!(offset(&app), o0, "mode-keys={mode}: C-p must NOT scroll while the cursor is mid pane");
    }
}

#[test]
fn live_ctrl_n_moves_the_cursor_down_without_scrolling() {
    for mode in ["vi", "emacs"] {
        let mut app = copy_app(mode);
        let (r0, o0) = (row(&app), offset(&app));
        crate::input::send_key_to_active(&mut app, "C-n").unwrap();
        assert_eq!(row(&app), r0 + 1, "mode-keys={mode}: C-n must move the cursor down one row (tmux cursor-down)");
        assert_eq!(offset(&app), o0, "mode-keys={mode}: C-n must NOT scroll while the cursor is mid pane");
    }
}

#[test]
fn live_ctrl_up_scrolls_one_line_and_leaves_the_cursor_alone() {
    for mode in ["vi", "emacs"] {
        let mut app = copy_app(mode);
        let (r0, o0) = (row(&app), offset(&app));
        crate::input::send_key_to_active(&mut app, "C-Up").unwrap();
        assert_eq!(offset(&app), o0 + 1, "mode-keys={mode}: C-Up must scroll the viewport up exactly one line");
        assert_eq!(row(&app), r0, "mode-keys={mode}: C-Up must not move the copy cursor");
    }
}

#[test]
fn live_ctrl_down_scrolls_one_line_back() {
    for mode in ["vi", "emacs"] {
        let mut app = copy_app(mode);
        crate::input::send_key_to_active(&mut app, "C-Up").unwrap();
        crate::input::send_key_to_active(&mut app, "C-Up").unwrap();
        let (r0, o0) = (row(&app), offset(&app));
        assert_eq!(o0, 2, "mode-keys={mode}: two C-Up presses must scroll two lines");
        crate::input::send_key_to_active(&mut app, "C-Down").unwrap();
        assert_eq!(offset(&app), o0 - 1, "mode-keys={mode}: C-Down must scroll back down exactly one line");
        assert_eq!(row(&app), r0, "mode-keys={mode}: C-Down must not move the copy cursor");
    }
}

#[test]
fn live_ctrl_arrow_accepts_the_lowercase_spelling_too() {
    // `send-keys -t s c-up` and a user config spelling both reach the same arm.
    let mut app = copy_app("vi");
    crate::input::send_key_to_active(&mut app, "c-up").unwrap();
    assert_eq!(offset(&app), 1, "lowercase c-up must scroll one line");
    crate::input::send_key_to_active(&mut app, "c-down").unwrap();
    assert_eq!(offset(&app), 0, "lowercase c-down must scroll back one line");
}

#[test]
fn live_ctrl_p_at_the_top_edge_scrolls_instead_of_stalling() {
    // tmux cursor-up drags the viewport once the cursor is already on row 0.
    let mut app = copy_app("vi");
    app.copy_pos = Some((0, 0));
    let o0 = offset(&app);
    crate::input::send_key_to_active(&mut app, "C-p").unwrap();
    assert_eq!(row(&app), 0, "the cursor stays pinned to the top row");
    assert_eq!(offset(&app), o0 + 1, "C-p on the top row must pull one more line of scrollback in");
}

// ══════════════ pre-server path: input::handle_key ══════════════
// Kept for the Rust tests; it must agree with the live path key for key.

#[test]
fn handle_key_ctrl_p_matches_the_live_path() {
    let mut app = copy_app("vi");
    let (r0, o0) = (row(&app), offset(&app));
    crate::input::handle_key(&mut app, ctrl(KeyCode::Char('p'))).unwrap();
    assert_eq!(row(&app), r0 - 1, "handle_key C-p must move the cursor up, like the live path");
    assert_eq!(offset(&app), o0, "handle_key C-p must not scroll mid pane");
}

#[test]
fn handle_key_ctrl_n_matches_the_live_path() {
    let mut app = copy_app("vi");
    let (r0, o0) = (row(&app), offset(&app));
    crate::input::handle_key(&mut app, ctrl(KeyCode::Char('n'))).unwrap();
    assert_eq!(row(&app), r0 + 1, "handle_key C-n must move the cursor down, like the live path");
    assert_eq!(offset(&app), o0, "handle_key C-n must not scroll mid pane");
}

#[test]
fn handle_key_ctrl_arrows_scroll_one_line() {
    let mut app = copy_app("vi");
    let r0 = row(&app);
    crate::input::handle_key(&mut app, ctrl(KeyCode::Up)).unwrap();
    assert_eq!(offset(&app), 1, "handle_key C-Up must scroll one line");
    assert_eq!(row(&app), r0, "handle_key C-Up must not move the copy cursor");
    crate::input::handle_key(&mut app, ctrl(KeyCode::Down)).unwrap();
    assert_eq!(offset(&app), 0, "handle_key C-Down must scroll back one line");
    assert_eq!(row(&app), r0, "handle_key C-Down must not move the copy cursor");
}

#[test]
fn handle_key_plain_arrows_still_move_the_cursor() {
    // The new Ctrl arms sit above the plain Up/Down arms; the plain keys must
    // keep their cursor-motion meaning.
    let mut app = copy_app("vi");
    let (r0, o0) = (row(&app), offset(&app));
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)).unwrap();
    assert_eq!(row(&app), r0 - 1, "plain Up must still move the cursor up");
    assert_eq!(offset(&app), o0, "plain Up must not scroll mid pane");
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)).unwrap();
    assert_eq!(row(&app), r0, "plain Down must still move the cursor back down");
}

// ══════════════ vi only scroll keys: C-e / C-y / J / K ══════════════
// tmux copy-mode-vi binds C-e and J to scroll-down and C-y and K to scroll-up
// (key-bindings.c:645, :653, :680, :681). tmux copy-mode binds C-e to
// end-of-line and leaves C-y, J and K unbound, so mode-keys emacs must keep the
// old end-of-line meaning for C-e and do nothing for the other three.

fn col(app: &AppState) -> u16 {
    app.copy_pos.expect("copy_pos must be tracked in copy mode").1
}

#[test]
fn live_ctrl_y_scrolls_up_and_ctrl_e_scrolls_back_down_in_vi_mode() {
    let mut app = copy_app("vi");
    let (r0, c0) = (row(&app), col(&app));
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    assert_eq!(offset(&app), 1, "vi C-y must scroll the viewport up one line");
    assert_eq!(row(&app), r0, "vi C-y must not move the copy cursor");
    assert_eq!(col(&app), c0, "vi C-y must not move the copy cursor to the line end");
    crate::input::send_key_to_active(&mut app, "C-e").unwrap();
    assert_eq!(offset(&app), 0, "vi C-e must scroll the viewport back down one line");
    assert_eq!(col(&app), c0, "vi C-e must NOT jump to the end of the line");
}

#[test]
fn live_ctrl_e_still_means_end_of_line_in_emacs_mode() {
    let mut app = copy_app("emacs");
    let (r0, o0) = (row(&app), offset(&app));
    crate::input::send_key_to_active(&mut app, "C-e").unwrap();
    assert!(col(&app) > 0, "emacs C-e must jump to the end of the line");
    assert_eq!(offset(&app), o0, "emacs C-e must not scroll");
    assert_eq!(row(&app), r0, "emacs C-e must stay on the same row");
}

#[test]
fn live_ctrl_y_does_nothing_in_emacs_mode() {
    // tmux leaves C-y unbound in the copy-mode table.
    let mut app = copy_app("emacs");
    let (r0, c0, o0) = (row(&app), col(&app), offset(&app));
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    assert_eq!((row(&app), col(&app), offset(&app)), (r0, c0, o0), "emacs C-y must be inert");
}

#[test]
fn live_shift_k_scrolls_up_and_shift_j_scrolls_back_down_in_vi_mode() {
    // Printable keys reach the server as send-text on Windows, so this is the
    // path a real Shift+K press takes.
    let mut app = copy_app("vi");
    let (r0, c0) = (row(&app), col(&app));
    crate::input::send_text_to_active(&mut app, "K").unwrap();
    assert_eq!(offset(&app), 1, "vi K must scroll the viewport up one line");
    assert_eq!((row(&app), col(&app)), (r0, c0), "vi K must not move the copy cursor");
    crate::input::send_text_to_active(&mut app, "J").unwrap();
    assert_eq!(offset(&app), 0, "vi J must scroll the viewport back down one line");
    assert_eq!((row(&app), col(&app)), (r0, c0), "vi J must not move the copy cursor");
}

#[test]
fn live_shift_j_and_k_do_nothing_in_emacs_mode() {
    let mut app = copy_app("emacs");
    let (r0, c0, o0) = (row(&app), col(&app), offset(&app));
    crate::input::send_text_to_active(&mut app, "K").unwrap();
    crate::input::send_text_to_active(&mut app, "J").unwrap();
    assert_eq!((row(&app), col(&app), offset(&app)), (r0, c0, o0), "emacs J and K must be inert");
}

#[test]
fn live_shift_k_honours_a_numeric_count() {
    let mut app = copy_app("vi");
    crate::input::send_text_to_active(&mut app, "3").unwrap();
    assert_eq!(app.copy_count, Some(3), "the digit must accumulate a count");
    crate::input::send_text_to_active(&mut app, "K").unwrap();
    assert_eq!(offset(&app), 3, "3K must scroll three lines, not one");
}

#[test]
fn handle_key_ctrl_e_and_ctrl_y_match_the_live_path() {
    let mut app = copy_app("vi");
    let (r0, c0) = (row(&app), col(&app));
    crate::input::handle_key(&mut app, ctrl(KeyCode::Char('y'))).unwrap();
    assert_eq!(offset(&app), 1, "handle_key vi C-y must scroll up one line");
    assert_eq!((row(&app), col(&app)), (r0, c0), "handle_key vi C-y must not move the cursor");
    crate::input::handle_key(&mut app, ctrl(KeyCode::Char('e'))).unwrap();
    assert_eq!(offset(&app), 0, "handle_key vi C-e must scroll back down one line");
    assert_eq!(col(&app), c0, "handle_key vi C-e must not jump to the end of the line");

    let mut emacs = copy_app("emacs");
    crate::input::handle_key(&mut emacs, ctrl(KeyCode::Char('e'))).unwrap();
    assert!(col(&emacs) > 0, "handle_key emacs C-e must still be end-of-line");
    assert_eq!(offset(&emacs), 0, "handle_key emacs C-e must not scroll");
}

#[test]
fn handle_key_shift_j_and_k_scroll_in_vi_mode_only() {
    let mut app = copy_app("vi");
    let r0 = row(&app);
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(offset(&app), 1, "handle_key vi K must scroll up one line");
    assert_eq!(row(&app), r0, "handle_key vi K must not move the cursor");
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(offset(&app), 0, "handle_key vi J must scroll back down one line");

    let mut emacs = copy_app("emacs");
    crate::input::handle_key(&mut emacs, KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT)).unwrap();
    assert_eq!(offset(&emacs), 0, "handle_key emacs K must be inert");
}

#[test]
fn handle_key_ctrl_up_honours_a_numeric_count() {
    // psmux accumulates a vi-style repeat count; `3 C-Up` scrolls three lines.
    let mut app = copy_app("vi");
    app.copy_count = Some(3);
    crate::input::handle_key(&mut app, ctrl(KeyCode::Up)).unwrap();
    assert_eq!(offset(&app), 3, "a count of 3 must scroll three lines, not one");
}
