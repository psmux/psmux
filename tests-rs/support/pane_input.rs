//! In-memory pane input fixture. No console, child process, or server is started.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, Node, Pane, Window};

#[derive(Clone, Default)]
pub(crate) struct Recorder(Arc<Mutex<Vec<u8>>>);

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
    pub(crate) fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

pub(crate) fn fixture(host_requests_win32: bool) -> (AppState, Recorder) {
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
        dead_info: None,
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
    let mut app = AppState::new("pane-input-test".to_string());
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

pub(crate) fn pane(app: &mut AppState) -> &mut Pane {
    let window = &mut app.windows[0];
    crate::tree::active_pane_mut(&mut window.root, &window.active_path).unwrap()
}
