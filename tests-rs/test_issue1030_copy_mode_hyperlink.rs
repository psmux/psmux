// Issue #1030: OSC 8 hyperlinks were lost on every row psmux drew while the
// pane was in copy mode, so a link scrolled back into view could not be
// clicked.
//
// Copy mode draws the active pane from the per cell `content` array, not from
// `rows_v2`.  Two links of the chain were missing: the server's copy mode cell
// snapshot (`CopyCell` in `dump_layout_json_fast`, and `CellJson` in the serde
// dump) had no link field, and the client's `content` branch never pushed a
// hyperlink run for the OSC 8 overlay.
//
// These tests drive the real chain: a pane parser fed the OSC 8 bytes, the
// real copy mode entry and scroll, the real fast dump the server sends to the
// client, serde back into `LayoutJson`, and the client's real
// `render_layout_json` plus `build_osc8_overlay`.  Stub PTY, no server.
// Registered from src/layout.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Terminal;

use crate::types::{AppState, Node};

const ROWS: u16 = 6;
const COLS: u16 = 40;
const URI: &str = "https://example.com/i1030";
const LINK_TEXT: &str = "CLICKME1030";

fn make_pane() -> crate::types::Pane {
    let size = portable_pty::PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 };
    let (master, writer) = crate::util::stub_pane_pty(size);
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 500)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term,
        last_rows: ROWS,
        last_cols: COLS,
        id: 0,
        title: String::new(),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        dead_info: None,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_app(bytes: &[u8]) -> AppState {
    let mut app = AppState::new("issue1030".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.copy_command = String::new();
    app.set_clipboard = "off".to_string();
    app.mode_keys = "vi".to_string();
    let pane = make_pane();
    pane.term.lock().expect("parser lock").process(bytes);
    let win = crate::types::Window {
        root: Node::Leaf(pane),
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: Rect::new(0, 0, COLS, ROWS),
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
    };
    app.windows.push(win);
    app.active_idx = 0;
    app.last_window_area = Rect::new(0, 0, COLS, ROWS);
    app
}

/// The reporter's recipe: one linked line, then enough filler to push it into
/// history.
fn link_then_filler(filler: usize) -> Vec<u8> {
    let mut bytes = format!("\x1b]8;;{URI}\x1b\\{LINK_TEXT}\x1b]8;;\x1b\\ tail\r\n").into_bytes();
    for i in 1..=filler {
        bytes.extend_from_slice(format!("filler line {i}\r\n").as_bytes());
    }
    bytes
}

/// Copy mode, scrolled back until the linked line is the top row.
fn scrolled_back_dump(filler: usize) -> String {
    let mut app = make_app(&link_then_filler(filler));
    crate::copy_mode::enter_copy_mode(&mut app);
    // The link is `filler` lines above the last one; ROWS - 1 rows show the
    // filler tail plus the empty cursor row, so scroll it to the top row.
    crate::copy_mode::scroll_copy_up(&mut app, filler + 2 - ROWS as usize);
    assert!(app.mode.in_copy());
    crate::layout::dump_layout_json_fast(&mut app).expect("fast dump")
}

fn leaf_content(layout: &LayoutJson) -> &Vec<Vec<CellJson>> {
    match layout {
        LayoutJson::Leaf { content, .. } => content,
        _ => panic!("expected a leaf"),
    }
}

/// Render `layout` the way the client does and return the hyperlink runs it
/// queued for the OSC 8 overlay.
fn client_runs(layout: &LayoutJson) -> Vec<crate::client::HyperlinkRun> {
    let mut terminal = Terminal::new(TestBackend::new(COLS, ROWS)).expect("terminal");
    crate::client::frame_hyperlinks_clear();
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, COLS, ROWS);
            let active_rect = crate::client::compute_active_rect_json(layout, area);
            crate::client::render_layout_json(
                frame,
                layout,
                area,
                false,
                Style::default(),
                Style::default(),
                false,
                Color::Reset,
                active_rect,
                "",
                false,
                "off",
                "",
                1,
                crate::border_lines::border_chars("single"),
                None,
                crate::client::WindowContentStyles::default(),
                crate::pane_border::PaneBorderIndicators::Colour,
            );
        })
        .expect("draw");
    crate::client::frame_hyperlinks_take()
}

// ── server: the copy mode cell snapshot carries the link ──────────────────

#[test]
fn copy_mode_content_cells_carry_the_link_after_scrolling_back() {
    let json = scrolled_back_dump(40);
    let layout: LayoutJson = serde_json::from_str(&json).expect("fast dump parses");
    let content = leaf_content(&layout);
    assert!(!content.is_empty(), "copy mode must send content cells: {json}");
    let top: String = content[0].iter().map(|c| c.text.as_str()).collect();
    assert!(top.starts_with(LINK_TEXT), "the linked line must be the top row: {top:?}");
    for (col, cell) in content[0].iter().enumerate() {
        let want = if col < LINK_TEXT.len() { Some(URI) } else { None };
        assert_eq!(cell.link.as_deref(), want, "cell {col} ({:?})", cell.text);
    }
}

#[test]
fn copy_mode_content_without_links_grows_no_link_field() {
    let mut app = make_app(b"plain text only\r\nmore text\r\n");
    crate::copy_mode::enter_copy_mode(&mut app);
    let json = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    assert!(json.contains("\"content\":[["), "copy mode must send content: {json}");
    assert!(!json.contains("\"link\""), "no link means no link field: {json}");
}

#[test]
fn copy_mode_content_link_is_json_escaped() {
    let bytes = b"\x1b]8;;https://example.com/a\"b\\c\x1b\\Q\x1b]8;;\x1b\\\r\n";
    let mut app = make_app(bytes);
    crate::copy_mode::enter_copy_mode(&mut app);
    let json = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&json).expect("escaped URI keeps the JSON valid");
    assert_eq!(
        leaf_content(&layout)[0][0].link.as_deref(),
        Some("https://example.com/a\"b\\c")
    );
}

#[test]
fn serde_dump_layout_copy_mode_content_carries_the_link() {
    // The `dump-layout` path (serde `CellJson`) had the same hole.
    let mut app = make_app(&link_then_filler(40));
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::copy_mode::scroll_copy_up(&mut app, 40 + 2 - ROWS as usize);
    let json = crate::layout::dump_layout_json(&mut app).expect("dump-layout");
    let layout: LayoutJson = serde_json::from_str(&json).expect("dump-layout parses");
    let content = leaf_content(&layout);
    assert!(!content.is_empty(), "copy mode sends full content: {json}");
    assert_eq!(content[0][0].link.as_deref(), Some(URI));
    assert_eq!(content[0][LINK_TEXT.len()].link, None);
}

// ── client: copy mode rows re-emit the link ───────────────────────────────

#[test]
fn client_copy_mode_render_queues_the_link_for_the_overlay() {
    let json = scrolled_back_dump(40);
    let layout: LayoutJson = serde_json::from_str(&json).expect("fast dump parses");
    if let LayoutJson::Leaf { copy_mode, active, .. } = &layout {
        assert!(*copy_mode && *active, "the content branch is the one under test");
    }
    let runs = client_runs(&layout);
    let linked: Vec<(u16, u16, String, String)> =
        runs.iter().map(|r| (r.x, r.y, r.text.clone(), r.uri.clone())).collect();
    assert_eq!(linked, vec![(0, 0, LINK_TEXT.to_string(), URI.to_string())]);
    let overlay = crate::client::build_osc8_overlay(&runs);
    assert!(
        overlay.contains(&format!("\x1b]8;;{URI}\x1b\\{LINK_TEXT}\x1b]8;;\x1b\\")),
        "the overlay must wrap the link text in OSC 8: {overlay:?}"
    );
}

#[test]
fn client_copy_mode_render_splits_two_links_on_one_row() {
    let bytes = b"\x1b]8;;https://a.example\x1b\\AAA\x1b]8;;\x1b\\ \x1b]8;;https://b.example\x1b\\BB\x1b]8;;\x1b\\\r\n";
    let mut app = make_app(bytes);
    crate::copy_mode::enter_copy_mode(&mut app);
    let json = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&json).expect("parses");
    let linked: Vec<(u16, String, String)> = client_runs(&layout)
        .iter()
        .map(|r| (r.x, r.text.clone(), r.uri.clone()))
        .collect();
    assert_eq!(
        linked,
        vec![
            (0, "AAA".to_string(), "https://a.example".to_string()),
            (4, "BB".to_string(), "https://b.example".to_string()),
        ]
    );
}

#[test]
fn client_copy_mode_render_keeps_wide_linked_glyphs_in_one_run() {
    let bytes = "\x1b]8;;https://ko.example\x1b\\한글x\x1b]8;;\x1b\\ end\r\n";
    let mut app = make_app(bytes.as_bytes());
    crate::copy_mode::enter_copy_mode(&mut app);
    let json = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&json).expect("parses");
    let linked: Vec<(u16, String)> =
        client_runs(&layout).iter().map(|r| (r.x, r.text.clone())).collect();
    assert_eq!(linked, vec![(0, "한글x".to_string())]);
}

#[test]
fn client_copy_mode_render_without_links_queues_nothing() {
    let mut app = make_app(b"no links here\r\n");
    crate::copy_mode::enter_copy_mode(&mut app);
    let json = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&json).expect("parses");
    assert!(client_runs(&layout).is_empty());
}
