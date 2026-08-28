// Issue #443 (follow-up): a never-written grid cell must serialize as a space
// on EVERY capture/copy path, not just the three `capture-pane` paths fixed in
// de119a0.
//
// Root cause recap: `vt100::Cell::contents()` returns "" for a cell the cursor
// merely skipped over (CUF `ESC[nC`, CHA `ESC[nG`, HPA, tabs, ...). A serializer
// that pushes `contents()` verbatim therefore contributes nothing for that cell
// and the on-screen gap closes up, fusing adjacent words. `push_capture_cell`
// backfills a space for those cells and skips the trailing half of a wide glyph
// so no phantom column appears after CJK.
//
// de119a0 routed only `capture_active_pane_text` (`-p`), `capture_active_pane_range`
// and `capture_active_pane_styled` (`-e`) through that helper. These paths were
// left on the old inline `push_str(cell.contents())` and still collapse:
//
//   * `capture_active_pane`    — `capture-pane` / `capturep` with no `-p`,
//                                i.e. capture into a paste buffer. Same command
//                                as the issue title, just the buffer variant.
//   * `yank_selection`         — copy-mode yank (`y`, `M-w`, mouse drag release)
//                                in all three selection modes.
//   * `copy_end_of_line`       — copy-mode `D`.
//
// These tests drive the REAL functions over a real PTY-backed pane tree (no
// psmux server and no session is created), feeding VT bytes straight into the
// pane parser. Registered from src/copy_mode.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{Node, SelectionMode};

const ROWS: u16 = 6;
const COLS: u16 = 40;

// "CUFA" then a 4-column cursor-forward, then "CUFB" — the issue's payload.
const CUF_PAYLOAD: &[u8] = b"CUFA\x1b[4CCUFB\x1b[4CCUFC";
const CUF_EXPECTED: &str = "CUFA    CUFB    CUFC";

/// ConPTY creation and the dummy spawn can fail transiently when the full
/// suite churns many short-lived PTYs in parallel; this file added 14 more
/// tests to that pool and one full-suite run saw two of them fail while the
/// same tests pass in isolation every time. Retry with backoff, and when it
/// still fails surface WHICH stage broke instead of a bare `.expect`.
fn open_pane_pty(
    rows: u16,
    cols: u16,
) -> (Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::Child + Send + Sync>, Box<dyn std::io::Write + Send>) {
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
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
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
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None, pane_options: Default::default(),
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
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
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

/// One window, one pane (root is the leaf so `active_path` stays empty), with
/// `bytes` already fed through the pane's vt100 parser.
fn app_showing(bytes: &[u8]) -> AppState {
    let mut app = AppState::new("issue443".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    // Never shell out on yank; keeps the test hermetic.
    app.copy_command = String::new();
    app.set_clipboard = "off".to_string();
    let pane = make_pane(0, ROWS, COLS);
    pane.term.lock().expect("parser lock").process(bytes);
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

fn newest_buffer(app: &AppState) -> String {
    app.paste_buffers.first().cloned().unwrap_or_default()
}

/// The Win32 clipboard is a single process-global resource, and reading it
/// (`GlobalLock` on the clipboard-owned handle) while another thread replaces
/// it (`EmptyClipboard` frees that same handle) is a use-after-free. `cargo
/// test` runs these in parallel, so every test that reaches
/// `copy_to_system_clipboard` (`yank_selection`, `copy_end_of_line`) takes this
/// lock first.
static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

/// Serializes clipboard access for the duration of a test and puts the user's
/// previous clipboard contents back afterwards, so running the suite locally
/// does not silently clobber it.
struct ClipboardGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<String>,
}

impl ClipboardGuard {
    fn new() -> Self {
        // A panicking test must not poison the lock for the rest of the suite.
        let lock = CLIPBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = crate::clipboard::read_from_system_clipboard();
        ClipboardGuard { _lock: lock, prev }
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::clipboard::copy_to_system_clipboard(&prev);
        }
    }
}

fn select_all_of_row0(app: &mut AppState, mode: SelectionMode, last_col: u16) {
    app.copy_anchor = Some((0, 0));
    app.copy_pos = Some((0, last_col));
    app.copy_selection_mode = mode;
    app.copy_anchor_scroll_offset = 0;
    app.copy_scroll_offset = 0;
}

// ════════════════════════════════════════════════════════════════════════════
// push_capture_cell / capture_row_text: the shared serialization contract
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn capture_row_text_backfills_cursor_skipped_cells() {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(CUF_PAYLOAD);
    let got = capture_row_text(parser.screen(), 0, 0..COLS);
    assert_eq!(
        got.trim_end(),
        CUF_EXPECTED,
        "cells skipped by CUF must serialize as spaces, not vanish"
    );
}

#[test]
fn capture_row_text_backfills_absolute_column_moves() {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(b"CHAA\x1b[10GCHAB\x1b[20GCHAC");
    let got = capture_row_text(parser.screen(), 0, 0..COLS);
    assert_eq!(got.trim_end(), "CHAA     CHAB      CHAC", "CHA gaps must survive");
}

#[test]
fn capture_row_text_leaves_no_phantom_column_after_wide_glyphs() {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process("WIDE:\u{4E2D}\u{6587}:END".as_bytes());
    let got = capture_row_text(parser.screen(), 0, 0..COLS);
    assert_eq!(
        got.trim_end(),
        "WIDE:\u{4E2D}\u{6587}:END",
        "the trailing half of a wide glyph must not become a space"
    );
}

#[test]
fn capture_row_text_preserves_literal_spaces() {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(b"SPCA    SPCB");
    let got = capture_row_text(parser.screen(), 0, 0..COLS);
    assert_eq!(got.trim_end(), "SPCA    SPCB", "written spaces are unaffected");
}

#[test]
fn capture_row_text_honors_a_sub_range() {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(CUF_PAYLOAD);
    // Columns 0..12 cover "CUFA    CUFB" exactly.
    let got = capture_row_text(parser.screen(), 0, 0..12);
    assert_eq!(got, "CUFA    CUFB", "sub-range capture keeps interior gaps");
}

#[test]
fn capture_row_text_on_an_untouched_row_is_all_spaces() {
    let parser = vt100::Parser::new(ROWS, COLS, 0);
    let got = capture_row_text(parser.screen(), 3, 0..COLS);
    assert_eq!(got.len(), COLS as usize, "blank row yields one space per column");
    assert!(got.chars().all(|c| c == ' '), "blank row is all spaces, got {got:?}");
}

// ════════════════════════════════════════════════════════════════════════════
// capture_active_pane — `capture-pane` with no `-p` (into a paste buffer)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn capture_pane_into_buffer_keeps_cuf_gaps() {
    let mut app = app_showing(CUF_PAYLOAD);
    capture_active_pane(&mut app).expect("capture");
    assert_eq!(
        first_line(&newest_buffer(&app)),
        CUF_EXPECTED,
        "`capture-pane` (buffer variant) collapsed the CUF gaps"
    );
}

#[test]
fn capture_pane_into_buffer_keeps_no_phantom_column_after_wide_glyphs() {
    let mut app = app_showing("WIDE:\u{4E2D}\u{6587}:END".as_bytes());
    capture_active_pane(&mut app).expect("capture");
    assert_eq!(
        first_line(&newest_buffer(&app)),
        "WIDE:\u{4E2D}\u{6587}:END",
        "`capture-pane` (buffer variant) inserted a phantom column after a wide glyph"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// yank_selection — copy-mode `y` / `M-w` / mouse drag, all selection modes
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn yank_line_selection_keeps_cuf_gaps() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing(CUF_PAYLOAD);
    select_all_of_row0(&mut app, SelectionMode::Line, COLS - 1);
    yank_selection(&mut app).expect("yank");
    assert_eq!(
        first_line(&newest_buffer(&app)),
        CUF_EXPECTED,
        "line-mode yank collapsed the CUF gaps"
    );
}

#[test]
fn yank_char_selection_keeps_cuf_gaps() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing(CUF_PAYLOAD);
    // Single-line char selection covering exactly "CUFA    CUFB".
    select_all_of_row0(&mut app, SelectionMode::Char, 11);
    yank_selection(&mut app).expect("yank");
    assert_eq!(
        newest_buffer(&app),
        "CUFA    CUFB",
        "char-mode yank collapsed the CUF gaps"
    );
}

#[test]
fn yank_rect_selection_keeps_cuf_gaps() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing(CUF_PAYLOAD);
    select_all_of_row0(&mut app, SelectionMode::Rect, COLS - 1);
    yank_selection(&mut app).expect("yank");
    assert_eq!(
        first_line(&newest_buffer(&app)),
        CUF_EXPECTED,
        "rect-mode yank collapsed the CUF gaps"
    );
}

#[test]
fn yank_selection_leaves_no_phantom_column_after_wide_glyphs() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing("WIDE:\u{4E2D}\u{6587}:END".as_bytes());
    select_all_of_row0(&mut app, SelectionMode::Line, COLS - 1);
    yank_selection(&mut app).expect("yank");
    assert_eq!(
        first_line(&newest_buffer(&app)),
        "WIDE:\u{4E2D}\u{6587}:END",
        "line-mode yank inserted a phantom column after a wide glyph"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// copy_end_of_line — copy-mode `D`
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn copy_end_of_line_keeps_cuf_gaps() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing(CUF_PAYLOAD);
    app.copy_pos = Some((0, 0));
    copy_end_of_line(&mut app).expect("copy to eol");
    assert_eq!(
        newest_buffer(&app),
        CUF_EXPECTED,
        "copy-to-end-of-line collapsed the CUF gaps"
    );
}

#[test]
fn copy_end_of_line_from_midline_keeps_cuf_gaps() {
    let _clip = ClipboardGuard::new();
    let mut app = app_showing(CUF_PAYLOAD);
    // Start inside the first gap: columns 6.. are "  CUFB    CUFC".
    app.copy_pos = Some((0, 6));
    copy_end_of_line(&mut app).expect("copy to eol");
    assert_eq!(
        newest_buffer(&app),
        "  CUFB    CUFC",
        "copy-to-end-of-line dropped leading skipped cells"
    );
}
