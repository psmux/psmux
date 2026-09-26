// Issue #681: copy mode page motions move a page, not a fixed number of lines.
//
// `C-b` and `C-f` scrolled a constant 10 lines whatever the pane height was,
// `send-keys -X page-up` and `page-down` scrolled a constant 20, and
// `copy-mode -u` scrolled a whole screen. tmux derives every one of them from
// the pane height in a single place, `window_copy_pageup1` and its
// `window_copy_pagedown1` twin (window-copy.c:767-773 and :825-831 at tag 3.7c,
// :723-729 and :781-787 at 3.6a):
//
//     n = 1;
//     if (screen_size_y(s) > 2) {
//             if (half_page)
//                     n = screen_size_y(s) / 2;
//             else
//                     n = screen_size_y(s) - 2;
//     }
//
// So a full page is the height minus two lines, a half page is half the height,
// and a pane of two rows or fewer moves a single line. The cursor keeps its
// screen row unless the history end clamps the scroll, and tmux then shifts the
// cursor by the whole page amount (window-copy.c:775-782 going up, :833-840
// going down), which is what lets repeated page-ups reach the first line.
//
// Both dispatchers are covered. The live path is `input::send_key_to_active`,
// reached by `send-key C-b` from the attached client and by `send-keys -t s C-b`
// from the CLI. `input::handle_key` is the pre-server dispatcher kept for the
// Rust tests, and #596 showed what happens when the two drift apart.
//
// The tests drive the REAL functions over a real PTY-backed pane tree (no psmux
// server and no session is created). Registered from src/input.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const COLS: u16 = 40;
const SCROLLBACK: usize = 500;
/// Lines fed into the pane. Deep enough that a page up from the bottom of a 30
/// row pane never reaches the top of the history.
const FILL: usize = 300;

/// The master, child and writer a pane fixture needs, with no pseudo console
/// and no process behind any of them. See `util::stub_pane_pty`.
///
/// This used to open a real ConPTY and spawn `cmd /c exit` into it, behind five
/// attempts with backoff, because both steps fail often enough under the full
/// parallel suite to be worth retrying. Nothing here ever used the console or
/// the process, so there is nothing left to retry.
fn open_pane_pty(
    rows: u16,
    cols: u16,
) -> (
    Box<dyn portable_pty::MasterPty>,
    Box<dyn portable_pty::Child + Send + Sync>,
    Box<dyn std::io::Write + Send>,
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

fn make_window(id: usize, rows: u16) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, COLS, rows),
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

/// One window, one pane `rows` tall holding `FILL` numbered lines, already in
/// copy mode with the cursor on the bottom row (where tmux parks it on entry)
/// and the view at the live end.
fn copy_app(mode_keys: &str, rows: u16) -> AppState {
    let mut app = AppState::new("copypage".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = mode_keys.to_string();
    let pane = make_pane(0, rows, COLS);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for i in 1..=FILL {
            parser.process(format!("line-{i}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0, rows);
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((rows.saturating_sub(1), 0));
    app
}

fn row(app: &AppState) -> u16 {
    app.copy_pos.expect("copy_pos must be tracked in copy mode").0
}

fn offset(app: &AppState) -> usize {
    app.copy_scroll_offset
}

fn history(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let p = crate::tree::active_pane(&win.root, &win.active_path).expect("active pane");
    let parser = p.term.lock().expect("parser lock");
    parser.screen().scrollback_filled()
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

// ══════════════ the amount itself ══════════════

#[test]
fn page_lines_matches_tmux_for_every_pane_height() {
    // window-copy.c:767-773 (3.7c): full = h - 2, half = h / 2, and a pane of
    // two rows or fewer moves one line.
    let cases: &[(u16, bool, usize)] = &[
        (30, false, 28), (30, true, 15),
        (24, false, 22), (24, true, 12),
        (10, false, 8),  (10, true, 5),
        (5, false, 3),   (5, true, 2),
        (3, false, 1),   (3, true, 1),
        (2, false, 1),   (2, true, 1),
        (1, false, 1),   (1, true, 1),
        (0, false, 1),   (0, true, 1),
    ];
    for &(height, half, want) in cases {
        assert_eq!(
            crate::copy_mode::page_lines(height, half),
            want,
            "height={height} half_page={half}: page size must match tmux window-copy.c:767-773"
        );
    }
}

// ══════════════ live path: input::send_key_to_active ══════════════

#[test]
fn live_page_keys_move_a_full_page() {
    for rows in [30u16, 10] {
        let page = usize::from(rows - 2);
        for key in ["C-b", "pageup", "M-v"] {
            let mut app = copy_app("vi", rows);
            let r0 = row(&app);
            crate::input::send_key_to_active(&mut app, key).unwrap();
            assert_eq!(offset(&app), page, "rows={rows}: {key} must scroll a page of rows-2 lines");
            assert_eq!(row(&app), r0, "rows={rows}: {key} must leave the cursor on its screen row");
        }
        for key in ["C-f", "pagedown"] {
            let mut app = copy_app("vi", rows);
            crate::input::send_key_to_active(&mut app, "C-b").unwrap();
            crate::input::send_key_to_active(&mut app, "C-b").unwrap();
            let before = offset(&app);
            crate::input::send_key_to_active(&mut app, key).unwrap();
            assert_eq!(offset(&app), before - page, "rows={rows}: {key} must scroll back a page");
        }
    }
}

#[test]
fn live_half_page_keys_move_half_a_page() {
    for rows in [30u16, 10] {
        let half = usize::from(rows / 2);
        let mut app = copy_app("vi", rows);
        let r0 = row(&app);
        crate::input::send_key_to_active(&mut app, "C-u").unwrap();
        assert_eq!(offset(&app), half, "rows={rows}: C-u must scroll half the pane height");
        assert_eq!(row(&app), r0, "rows={rows}: C-u must leave the cursor on its screen row");
        crate::input::send_key_to_active(&mut app, "C-d").unwrap();
        assert_eq!(offset(&app), 0, "rows={rows}: C-d must scroll back the same half page");
    }
}

#[test]
fn live_page_keys_move_one_line_in_a_tiny_pane() {
    // screen_size_y(s) > 2 guards the formula in tmux, so a two row pane moves
    // a single line instead of standing still or jumping the buffer.
    for rows in [2u16, 1] {
        let mut app = copy_app("vi", rows);
        crate::input::send_key_to_active(&mut app, "C-b").unwrap();
        assert_eq!(offset(&app), 1, "rows={rows}: a page must be one line");
        let mut app = copy_app("vi", rows);
        crate::input::send_key_to_active(&mut app, "C-u").unwrap();
        assert_eq!(offset(&app), 1, "rows={rows}: a half page must be one line");
    }
}

#[test]
fn live_page_up_stops_at_the_history_top_and_pulls_the_cursor_with_it() {
    let rows = 10u16;
    let mut app = copy_app("vi", rows);
    let top = history(&app);
    // Park the view one line short of the top so the next page up is clamped.
    crate::copy_mode::scroll_copy_up(&mut app, top - 1);
    app.copy_pos = Some((rows - 1, 0));
    crate::input::send_key_to_active(&mut app, "C-b").unwrap();
    assert_eq!(offset(&app), top, "the view must stop at the oldest retained line");
    assert_eq!(
        row(&app), rows - 1 - (rows - 2),
        "a clamped page up moves the cursor by the page amount (window-copy.c:775-782)"
    );
    // Once the cursor is on the top row too, the pane is fully at the top.
    crate::input::send_key_to_active(&mut app, "C-b").unwrap();
    assert_eq!(offset(&app), top, "the view stays at the top");
    assert_eq!(row(&app), 0, "the cursor lands on the first line of the history");
}

#[test]
fn live_page_down_stops_at_the_live_end_and_pulls_the_cursor_with_it() {
    let rows = 10u16;
    let mut app = copy_app("vi", rows);
    crate::copy_mode::scroll_copy_up(&mut app, 3);
    app.copy_pos = Some((0, 0));
    crate::input::send_key_to_active(&mut app, "C-f").unwrap();
    assert_eq!(offset(&app), 0, "the view must stop at the live output");
    assert_eq!(
        row(&app), rows - 2,
        "a clamped page down moves the cursor by the page amount (window-copy.c:833-840)"
    );
}

// ══════════════ pre-server path: input::handle_key ══════════════

#[test]
fn handle_key_page_keys_match_the_live_path() {
    let rows = 30u16;
    let cases: &[(KeyEvent, &str)] = &[
        (ctrl(KeyCode::Char('b')), "C-b"),
        (ctrl(KeyCode::Char('f')), "C-f"),
        (ctrl(KeyCode::Char('u')), "C-u"),
        (ctrl(KeyCode::Char('d')), "C-d"),
        (KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE), "pageup"),
        (KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), "pagedown"),
    ];
    for &(event, name) in cases {
        // Start both dispatchers scrolled up so the down keys have room to move.
        let mut a = copy_app("vi", rows);
        let mut b = copy_app("vi", rows);
        crate::copy_mode::scroll_copy_up(&mut a, 60);
        crate::copy_mode::scroll_copy_up(&mut b, 60);
        crate::input::handle_key(&mut a, event).unwrap();
        crate::input::send_key_to_active(&mut b, name).unwrap();
        assert_eq!(
            (offset(&a), row(&a)), (offset(&b), row(&b)),
            "{name}: handle_key and send_key_to_active must agree on the page amount"
        );
    }
}

// ══════════════ copy-mode -u ══════════════

#[test]
fn copy_mode_dash_u_enters_scrolled_one_page_up() {
    // cmd-copy-mode.c:99-100 calls window_copy_pageup(wp, 0), a page, not a
    // whole screen.
    let rows = 30u16;
    let mut app = copy_app("vi", rows);
    app.mode = Mode::Passthrough;
    app.copy_pos = None;
    app.copy_scroll_offset = 0;
    assert!(crate::copy_mode::enter_copy_mode_page_up(&mut app), "scroll-enter-copy-mode is on by default");
    assert!(matches!(app.mode, Mode::CopyMode), "copy-mode -u must enter copy mode");
    assert_eq!(offset(&app), usize::from(rows - 2), "copy-mode -u must scroll one page, not one screen");
}

// ══════════════ no pane to measure ══════════════

#[test]
fn page_scroll_is_inert_without_an_active_pane() {
    // `active_pane_rows` has no invented fallback height, and it does not need
    // one: the scroll itself walks the same tree and returns on the same miss,
    // so a page motion with no pane under the active path moves nothing.
    let mut app = AppState::new("copypage".to_string());
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((3, 0));
    assert_eq!(crate::copy_mode::active_pane_rows(&app), None, "an empty window list has no pane to measure");
    crate::copy_mode::page_scroll(&mut app, true, false);
    assert_eq!(offset(&app), 0, "with no pane the view must not move");
    assert_eq!(row(&app), 3, "with no pane the cursor must not move either");

    // Same again with a window whose root is a split and whose active path
    // points at no leaf, which is the other way the lookup comes back empty.
    app.windows.push(make_window(0, 24));
    app.active_idx = 0;
    assert_eq!(crate::copy_mode::active_pane_rows(&app), None, "a split with an empty path has no leaf");
    crate::copy_mode::page_scroll(&mut app, false, true);
    assert_eq!(offset(&app), 0, "still nothing to scroll");
    assert_eq!(row(&app), 3, "still nothing to move");
}

#[test]
fn copy_mode_dash_u_is_inert_when_scroll_enter_copy_mode_is_off() {
    // #284: the caller forwards PageUp to the pane instead.
    let mut app = copy_app("vi", 30);
    app.mode = Mode::Passthrough;
    app.copy_scroll_offset = 0;
    app.scroll_enter_copy_mode = false;
    assert!(!crate::copy_mode::enter_copy_mode_page_up(&mut app), "the caller has to handle this case");
    assert!(matches!(app.mode, Mode::Passthrough), "copy mode must not be entered");
    assert_eq!(offset(&app), 0, "nothing must scroll");
}
