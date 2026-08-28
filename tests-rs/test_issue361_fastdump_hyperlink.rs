// Issue #361: the OSC 8 URI must survive the serializer the CLIENT actually
// renders from.  `CtrlReq::DumpState` uses `dump_layout_json_fast`, which builds
// its runs by hand — not through `CellRunJson` / `serialize_screen_rows`.  That
// hand-rolled path had no `link` field, so every pane reached the client with
// `run.link == None` and `build_osc8_overlay` never fired, even on hosts where
// ConPTY passthrough delivers the OSC 8 to psmux.
//
// `test_issue361_serialize_hyperlink.rs` covers the serde path only, which is
// why it stayed green while hyperlinks were dead end to end.
//
// Dummy PTY (no real spawn), so this stays hermetic and portable.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, Node};

const ROWS: u16 = 4;
const COLS: u16 = 40;

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

fn make_pane(id: usize) -> crate::types::Pane {
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: Box::new(DummyMaster),
        writer: Box::new(DummyWriter),
        child: Box::new(DummyChild),
        term,
        last_rows: ROWS,
        last_cols: COLS,
        id,
        title: String::new(),
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

fn make_window() -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
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

/// One window, one pane, with `bytes` already fed through the vt100 parser,
/// dumped exactly the way `CtrlReq::DumpState` dumps it for the client.
fn fast_dump_of(bytes: &[u8]) -> String {
    let mut app = AppState::new("fastdump".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.copy_command = String::new();
    app.set_clipboard = "off".to_string();
    let pane = make_pane(0);
    pane.term.lock().expect("parser lock").process(bytes);
    let mut win = make_window();
    win.root = Node::Leaf(pane);
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app.last_window_area = ratatui::layout::Rect::new(0, 0, COLS, ROWS);
    crate::layout::dump_layout_json_fast(&mut app).expect("fast dump")
}

#[test]
fn fast_dump_run_carries_hyperlink_uri() {
    let json = fast_dump_of(b"\x1b]8;;https://example.com\x1b\\Link\x1b]8;;\x1b\\ plain");
    assert!(
        json.contains("\"link\":\"https://example.com\""),
        "fast dump must serialize the OSC 8 URI: {json}"
    );
}

#[test]
fn fast_dump_breaks_the_run_at_the_link_boundary() {
    // Same style either side of the link, so only the hyperlink id can split it.
    let json = fast_dump_of(b"\x1b]8;;https://example.com\x1b\\Link\x1b]8;;\x1b\\plain");
    assert!(
        json.contains("{\"text\":\"Link\""),
        "linked text must be its own run, not merged with the plain tail: {json}"
    );
}

#[test]
fn fast_dump_omits_link_when_there_is_none() {
    let json = fast_dump_of(b"just text");
    assert!(
        !json.contains("\"link\""),
        "unlinked panes must not grow a link field: {json}"
    );
}

#[test]
fn fast_dump_escapes_the_uri() {
    // A URI containing a quote must not break the JSON.
    let json = fast_dump_of(b"\x1b]8;;https://example.com/a\"b\x1b\\Link\x1b]8;;\x1b\\");
    assert!(
        json.contains("\"link\":\"https://example.com/a\\\"b\""),
        "URI must be JSON-escaped: {json}"
    );
}
