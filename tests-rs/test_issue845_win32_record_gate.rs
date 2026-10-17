// Issue #845: in 3.4.0 named keys (Up, Left, Home, F5, Escape, and
// `send-keys -l "ESC[A"`) never reached a raw mode node child on Windows 10
// 19045, while 3.3.8 delivered `1b5b41` and friends.  3.4.0 started writing
// those keys as win32 input mode records (#729), and a console host that
// predates win32 input mode cannot parse them.
//
// Measured on 26200 with a headless host in front of keyrec.js (raw mode
// node), writing the very same bytes psmux writes:
//
//   host                         ?9001h at startup   record for Up arrives as
//   inbox conhost 26200          yes                 1b5b41
//   OpenConsole 1.25             yes                 1b5b41
//   OpenConsole 1.0.2005 (2020)  NO                  the record's literal text
//   (reporter) conhost 19045     not measured        nothing at all
//
// So the gate is the host's own request for the mode, which psmux sees as the
// first bytes on the pane's output, exactly the signal Windows Terminal acts
// on.  These tests drive `write_pane_input` against a pane whose parser has or
// has not seen `CSI ?9001h`, with a recording writer, so the bytes the pane's
// host would receive are asserted exactly.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::input::{win32_key_record_allowed, write_pane_input};

#[derive(Debug)]
struct DummyChild;

struct DummyMaster;

#[derive(Debug, Clone, Default)]
struct Recorder(Arc<Mutex<Vec<u8>>>);

impl crate::pane::PaneInputSink for Recorder {}

impl std::io::Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
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
        Ok(None)
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(0))
    }
    fn process_id(&self) -> Option<u32> { None }
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> { None }
}

impl portable_pty::MasterPty for DummyMaster {
    fn resize(&self, _size: portable_pty::PtySize) -> Result<(), anyhow::Error> { Ok(()) }
    fn get_size(&self) -> Result<portable_pty::PtySize, anyhow::Error> {
        Ok(portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
    }
    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
        Ok(Box::new(std::io::empty()))
    }
    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
        Ok(Box::new(std::io::sink()))
    }
}

/// The console input mode word libuv leaves on a raw mode node child:
/// `ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT` (#613 notes).
const NODE_RAW_MODE: u32 = 0x0208;

/// A pane whose host wrote `host_output` at startup and whose child reads with
/// `mode`, plus the recorder that sees every byte psmux writes to its input.
fn pane(host_output: &[u8], mode: u32) -> (crate::types::Pane, Recorder) {
    let mut parser = vt100::Parser::new(24, 80, 0);
    parser.process(host_output);
    let rec = Recorder::default();
    let epoch = Instant::now() - Duration::from_secs(2);
    let p = crate::types::Pane {
        master: Box::new(DummyMaster),
        writer: Box::new(rec.clone()),
        child: Box::new(DummyChild),
        term: Arc::new(Mutex::new(parser)),
        last_rows: 24,
        last_cols: 80,
        id: 1,
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
        // Fresh for the 2 s cache window, so no console attach is attempted.
        mouse_input_cache: Some((Instant::now(), Some(mode))),
        win32_input_latched: false,
        input_off: false,
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
    };
    (p, rec)
}

/// What the inbox conhost on 26200 writes first on a psmux pane (pane_raw.bin).
const MODERN_HOST_STARTUP: &[u8] = b"\x1b[?9001h\x1b[?1004h\x1b[?25l\x1b[2J\x1b[m\x1b[H";
/// A host that predates win32 input mode: the same startup without the request.
const OLD_HOST_STARTUP: &[u8] = b"\x1b[?25l\x1b[2J\x1b[m\x1b[H";

fn sent(rec: &Recorder) -> Vec<u8> {
    std::mem::take(&mut *rec.0.lock().unwrap())
}

#[test]
fn gate_matrix() {
    assert!(win32_key_record_allowed(true, true));
    assert!(!win32_key_record_allowed(false, true), "old host: never a record");
    assert!(!win32_key_record_allowed(true, false));
    assert!(!win32_key_record_allowed(false, false));
}

#[test]
fn parser_records_the_host_request() {
    let mut p = vt100::Parser::new(24, 80, 0);
    assert!(!p.screen().win32_input_requested());
    p.process(OLD_HOST_STARTUP);
    assert!(!p.screen().win32_input_requested());
    p.process(MODERN_HOST_STARTUP);
    assert!(p.screen().win32_input_requested());
    // A program turning the mode off on exit says nothing about the host.
    p.process(b"\x1b[?9001l");
    assert!(p.screen().win32_input_requested());
    // RIS resets the screen, not the host behind it.
    p.process(b"\x1bc");
    assert!(p.screen().win32_input_requested());
}

#[test]
fn parser_sees_the_request_in_a_combined_decset() {
    let mut p = vt100::Parser::new(24, 80, 0);
    p.process(b"\x1b[?1004;9001h");
    assert!(p.screen().win32_input_requested());
}

#[test]
fn old_host_gets_plain_vt_for_every_reported_key() {
    // The reporter's keys, as psmux's key tables encode them.
    let keys: &[&[u8]] = &[b"\x1b[A", b"\x1b[D", b"\x1b[H", b"\x1b[15~", b"\x1bOA", b"\x1b[1;5C"];
    let (mut p, rec) = pane(OLD_HOST_STARTUP, NODE_RAW_MODE);
    for k in keys {
        write_pane_input(&mut p, k);
        assert_eq!(sent(&rec), k.to_vec(), "key {:?} must reach an old host as its VT bytes", String::from_utf8_lossy(k));
    }
    assert!(!p.win32_input_latched, "nothing latched, so Escape stays a bare 1b");
    write_pane_input(&mut p, b"\x1b");
    assert_eq!(sent(&rec), b"\x1b".to_vec());
    write_pane_input(&mut p, b"a");
    assert_eq!(sent(&rec), b"a".to_vec());
}

#[test]
fn old_host_escape_stays_bare_even_if_latched() {
    // A latch can only come from a record write, which the gate now refuses on
    // such a host; pin the Escape path on its own anyway.
    let (mut p, rec) = pane(OLD_HOST_STARTUP, NODE_RAW_MODE);
    p.win32_input_latched = true;
    write_pane_input(&mut p, b"\x1b");
    assert_eq!(sent(&rec), b"\x1b".to_vec());
}

#[test]
fn modern_host_keeps_the_729_records() {
    let (mut p, rec) = pane(MODERN_HOST_STARTUP, NODE_RAW_MODE);
    write_pane_input(&mut p, b"\x1b[A");
    assert_eq!(
        String::from_utf8(sent(&rec)).unwrap(),
        crate::input::win32_input_key_seq(0x26, crate::platform::mouse_inject::vk_to_scan(0x26), 0, 0x0100),
    );
    assert!(p.win32_input_latched);
    // #588: once latched, a lone Escape goes as its record too.
    write_pane_input(&mut p, b"\x1b");
    assert_eq!(String::from_utf8(sent(&rec)).unwrap(), "\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_");
}

#[test]
fn clear_history_keeps_the_host_answer() {
    let (p, _rec) = pane(MODERN_HOST_STARTUP, NODE_RAW_MODE);
    let mut app = crate::types::AppState::new("t845".to_string());
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Leaf(p),
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
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
    });
    app.active_idx = 0;
    crate::window_ops::clear_active_pane_history(&mut app);
    let crate::types::Node::Leaf(ref lp) = app.windows[0].root else { panic!() };
    assert!(crate::input::pane_host_parses_win32_input(lp));
}
