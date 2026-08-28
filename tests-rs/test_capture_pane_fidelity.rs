// Regression tests for capture-pane fidelity fixes:
//
//   1. -N preserves trailing cells that carry a non-default background SGR,
//      so replaying a styled capture of a full-screen TUI keeps its painted
//      background (the old renderer trimmed every row after the last
//      non-whitespace character, dropping the right-hand background fill).
//   2. -t %N captures the targeted pane's content even when it is not the
//      active pane (and not in the active window); an unresolvable target
//      falls back to the active pane with no error.
//
// The capture functions only read `Pane::term` / `last_rows` / `last_cols`,
// so these tests use a dummy PTY (no real spawn) and stay hermetic and
// portable across Windows and Unix hosts.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const ROWS: u16 = 6;
const COLS: u16 = 40;

// ── PTY-free pane scaffolding ──────────────────────────────────────────────

#[derive(Debug)]
struct DummyChild;

#[derive(Debug)]
struct DummyWriter;

struct DummyMaster;

impl std::io::Write for DummyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { Ok(buf.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

impl portable_pty::ChildKiller for DummyChild {
    fn kill(&mut self) -> std::io::Result<()> { Ok(()) }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(DummyChild)
    }
}

impl portable_pty::Child for DummyChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        Ok(Some(portable_pty::ExitStatus::with_exit_code(0)))
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(0))
    }
    fn process_id(&self) -> Option<u32> { None }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> { None }
}

impl portable_pty::MasterPty for DummyMaster {
    fn resize(&self, _size: portable_pty::PtySize) -> Result<(), anyhow::Error> { Ok(()) }
    fn get_size(&self) -> Result<portable_pty::PtySize, anyhow::Error> {
        Ok(portable_pty::PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
    }
    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
        Ok(Box::new(std::io::empty()))
    }
    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
        Ok(Box::new(DummyWriter))
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> { None }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> { None }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> { None }
}

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: Box::new(DummyMaster),
        writer: Box::new(DummyWriter),
        child: Box::new(DummyChild),
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
    let mut app = AppState::new("fidelity".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
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

/// Two windows, each with one pane carrying distinct content. The first
/// window stays active, so pane 1 is a valid -t %N target that is NOT the
/// active pane.
fn app_two_windows() -> AppState {
    let mut app = AppState::new("fidelity".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.copy_command = String::new();
    app.set_clipboard = "off".to_string();

    let pane0 = make_pane(0, ROWS, COLS);
    pane0.term.lock().expect("parser lock").process(b"ALPHA-WINDOW");
    let mut win0 = make_window(0);
    win0.root = Node::Leaf(pane0);
    win0.active_path = vec![];

    let pane1 = make_pane(1, ROWS, COLS);
    pane1.term.lock().expect("parser lock").process(b"BETA-WINDOW");
    let mut win1 = make_window(1);
    win1.root = Node::Leaf(pane1);
    win1.active_path = vec![];

    app.windows.push(win0);
    app.windows.push(win1);
    app.active_idx = 0;
    app
}

// ════════════════════════════════════════════════════════════════════════
// -N: preserve trailing styled cells (background SGR) at end of line
// ════════════════════════════════════════════════════════════════════════

/// Red background active, "TUI" written, then EL-to-EOL: a minimal
/// stand-in for a full-screen app painting its background across the
/// whole row.
const BG_PAYLOAD: &[u8] = b"\x1b[48;5;196mTUI\x1b[K\x1b[0m";
const BG_ROW_TAIL: usize = (COLS - 3) as usize; // columns 3..COLS carry bg 196

#[test]
fn styled_capture_without_N_trims_trailing_cells() {
    let mut app = app_showing(BG_PAYLOAD);
    let out = capture_active_pane_styled(&mut app, None, None, None, false)
        .expect("capture").expect("some text");
    let first = out.lines().next().expect("row 0");
    // Trimmed: exactly the three text cells plus the row-end reset. The
    // renderer emits a leading "0" reset param on every SGR change.
    assert_eq!(first, "\x1b[0;48;5;196mTUI\x1b[0m",
        "without -N the styled row must drop the trailing background cells");
}

#[test]
fn styled_capture_with_N_keeps_trailing_background_cells() {
    let mut app = app_showing(BG_PAYLOAD);
    let out = capture_active_pane_styled(&mut app, None, None, None, true)
        .expect("capture").expect("some text");
    let first = out.lines().next().expect("row 0");
    assert!(first.starts_with("\x1b[0;48;5;196mTUI"),
        "row must open with the text under the background SGR, got: {:?}", first);
    let tail = &first["\x1b[0;48;5;196mTUI".len()..];
    assert_eq!(tail, format!("{}\x1b[0m", " ".repeat(BG_ROW_TAIL)),
        "with -N the full row width must be emitted, trailing styled spaces and all");
}

#[test]
fn styled_capture_with_N_keeps_a_fully_styled_blank_row() {
    // A row that is entirely styled spaces (no text at all) must still emit
    // its background when -N is set — the old trim dropped it completely.
    let mut app = app_showing(b"\x1b[48;5;21m\x1b[K\x1b[0m");
    let out = capture_active_pane_styled(&mut app, None, None, None, true)
        .expect("capture").expect("some text");
    let first = out.lines().next().expect("row 0");
    assert_eq!(first, format!("\x1b[0;48;5;21m{}\x1b[0m", " ".repeat(COLS as usize)),
        "an all-background row must emit its SGR plus the full-width spaces, got: {:?}", first);
}

#[test]
fn plain_capture_with_N_keeps_trailing_spaces() {
    let mut app = app_showing(BG_PAYLOAD);
    // Plain no-range path.
    let kept = capture_active_pane_text(&mut app, None, true).expect("capture").expect("text");
    let first = kept.lines().next().expect("row 0");
    assert_eq!(first, format!("TUI{}", " ".repeat(BG_ROW_TAIL)),
        "-N on the plain capture must keep trailing spaces");

    let trimmed = capture_active_pane_text(&mut app, None, false).expect("capture").expect("text");
    assert_eq!(trimmed.lines().next().expect("row 0"), "TUI",
        "without -N the plain capture keeps trimming trailing spaces");
}

#[test]
fn range_capture_with_N_keeps_trailing_spaces() {
    let mut app = app_showing(BG_PAYLOAD);
    let kept = capture_active_pane_range(&mut app, None, None, None, true)
        .expect("capture").expect("text");
    assert_eq!(kept.lines().next().expect("row 0"),
        format!("TUI{}", " ".repeat(BG_ROW_TAIL)));

    let trimmed = capture_active_pane_range(&mut app, None, None, None, false)
        .expect("capture").expect("text");
    assert_eq!(trimmed.lines().next().expect("row 0"), "TUI");
}

// ════════════════════════════════════════════════════════════════════════
// -t %N: capture a pane that is not the active pane
// ════════════════════════════════════════════════════════════════════════

#[test]
fn styled_capture_targets_non_active_pane_by_id() {
    let mut app = app_two_windows();
    // Active window is 0; pane 1 lives in window 1.
    let out = capture_active_pane_styled(&mut app, None, None, Some(1), false)
        .expect("capture").expect("text");
    assert!(out.contains("BETA-WINDOW"),
        "capture of -t %1 must read pane 1, got: {:?}", out);
    assert!(!out.contains("ALPHA-WINDOW"),
        "capture of -t %1 must not read the active pane, got: {:?}", out);
}

#[test]
fn plain_and_range_capture_target_non_active_pane_by_id() {
    let mut app = app_two_windows();
    let text = capture_active_pane_text(&mut app, Some(1), false).expect("capture").expect("text");
    assert!(text.contains("BETA-WINDOW"), "plain -t %1 capture, got: {:?}", text);

    let range = capture_active_pane_range(&mut app, None, None, Some(1), false)
        .expect("capture").expect("text");
    assert!(range.contains("BETA-WINDOW"), "range -t %1 capture, got: {:?}", range);
}

#[test]
fn unresolvable_pane_target_falls_back_to_active_pane() {
    let mut app = app_two_windows();
    // No pane 999 exists: must silently fall back to the active pane
    // (window 0) instead of erroring or returning empty.
    let out = capture_active_pane_styled(&mut app, None, None, Some(999), false)
        .expect("capture").expect("text");
    assert!(out.contains("ALPHA-WINDOW"),
        "missing -t target must fall back to the active pane, got: {:?}", out);
}

#[test]
fn none_target_captures_active_pane_as_before() {
    let mut app = app_two_windows();
    let out = capture_active_pane_styled(&mut app, None, None, None, false)
        .expect("capture").expect("text");
    assert!(out.contains("ALPHA-WINDOW"),
        "no -t target must keep capturing the active pane, got: {:?}", out);
}
