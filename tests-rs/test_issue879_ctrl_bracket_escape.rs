//! #879: interactive Ctrl+[ bypassed the lone-Escape repair from #588.
//! It worked on a fresh pane, then stopped after a win32 input record latched
//! the pane's ConPTY. Exercise the actual attached-client dispatcher, not CLI
//! send-keys, with a recording writer and no console, process, or server.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::input::{send_key_to_active, win32_input_key_seq};
use crate::types::{AppState, Node, Pane, Window};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<u8>>>);

impl crate::pane::PaneInputSink for Recorder {}

impl std::io::Write for Recorder {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Recorder {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

fn fixture(host_requests_win32: bool) -> (AppState, Recorder) {
    let (master, _) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    });
    let mut parser = vt100::Parser::new(24, 80, 0);
    if host_requests_win32 {
        parser.process(b"\x1b[?9001h");
    }
    let recorder = Recorder::default();
    let epoch = Instant::now() - Duration::from_secs(2);
    let pane = Pane {
        master,
        writer: Box::new(recorder.clone()),
        child: crate::util::StubChild::exited(),
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
        // A VT byte reader, not a record reader. Cache the answer so the test
        // never attaches to a console. This is libuv's raw input mode word.
        mouse_input_cache: Some((Instant::now(), Some(0x0208))),
        win32_input_latched: false,
        input_off: false,
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
    };
    let mut app = AppState::new("ctrl-bracket-test".to_string());
    app.windows.push(Window {
        root: Node::Leaf(pane),
        active_path: vec![],
        name: "test".to_string(),
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
        floating: vec![],
        floating_focus: None,
    });
    app.active_idx = 0;
    (app, recorder)
}

fn pane(app: &mut AppState) -> &mut Pane {
    let window = &mut app.windows[0];
    crate::tree::active_pane_mut(&mut window.root, &window.active_path).unwrap()
}

const ESCAPE_ALIASES: &[&str] = &["C-[", "C-3", "C-S-[", "C-S-3", "c-s-3"];

#[test]
fn escape_aliases_stay_raw_on_a_fresh_pane() {
    let (mut app, recorder) = fixture(true);
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), b"\x1b", "{key}");
        assert!(!pane(&mut app).win32_input_latched);
    }
}

#[test]
fn escape_aliases_follow_physical_escape_after_a_named_key_latches_the_pane() {
    let (mut app, recorder) = fixture(true);
    // Named keys already go through write_pane_input. On this host/reader
    // combination, Left writes a win32 record and latches the pane (#729).
    send_key_to_active(&mut app, "left").unwrap();
    assert!(pane(&mut app).win32_input_latched);
    recorder.take();

    send_key_to_active(&mut app, "esc").unwrap();
    let escape = recorder.take();
    assert_eq!(
        escape,
        win32_input_key_seq(27, crate::platform::mouse_inject::vk_to_scan(27), 27, 0)
            .into_bytes()
    );
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), escape, "{key} must match physical Escape");
    }
}

#[test]
fn ordinary_control_bytes_are_unchanged_on_a_latched_pane() {
    let (mut app, recorder) = fixture(true);
    pane(&mut app).win32_input_latched = true;
    for (key, byte) in [
        ("C-w", 0x17),
        ("C-c", 0x03),
        ("C-/", 0x1f),
        ("C-S--", 0x1f),
        ("C-4", 0x1c),
    ] {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), [byte], "{key}");
    }
}

#[test]
fn an_old_host_keeps_raw_escape_even_if_the_latch_is_set() {
    let (mut app, recorder) = fixture(false);
    // #845: never send a win32 record to a host that did not request it.
    pane(&mut app).win32_input_latched = true;
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), b"\x1b", "{key}");
    }
}
