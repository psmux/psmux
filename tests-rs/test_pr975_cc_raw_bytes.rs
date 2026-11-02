// PR #975: %output carries the pane's raw bytes, octal escaped, and does not
// hold an unfinished UTF-8 character back in the ring. Written against the
// wire line (format_notification) so the same test builds on the old String
// payload and the new byte payload.
use super::*;
use crate::types::{AppState, ControlClient, Node, Pane, Window};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const PANE: usize = 9;

fn app_with_ring() -> (AppState, crate::control_ring::Ring, mpsc::Receiver<ControlNotification>) {
    let size = portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 };
    let (master, writer) = crate::util::stub_pane_pty(size);
    let ring: crate::control_ring::Ring = Default::default();
    let pane = Pane {
        master, writer, child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(vt100::Parser::new(24, 80, 0))),
        last_rows: 24, last_cols: 80, id: PANE,
        title: String::new(), title_locked: false, child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: Instant::now(), last_infer_title: Instant::now(),
        dead: false, dead_info: None, last_text_input: None, last_special_key: None,
        vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None,
        win32_input_latched: false, input_off: false, scroll_fg_cache: None,
        mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(AtomicU32::new(0)),
        copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(),
        squelch_until: None, output_ring: ring.clone(), spawned_at: None,
        start_command: String::new(), cwd_hint: None,
    };
    let mut app = AppState::new("pr975".into());
    app.windows.push(Window {
        root: Node::Leaf(pane), active_path: vec![], name: "w".into(), id: 1,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
        window_size: None, window_options: Default::default(),
        activity_flag: false, bell_flag: false, silence_flag: false,
        last_output_time: Instant::now(), last_seen_version: 0, manual_rename: false,
        layout_index: 0, pane_mru: vec![], zoom_saved: None, linked_from: None,
        floating: vec![], floating_focus: None,
    });
    let (tx, rx) = mpsc::channel();
    app.control_clients.insert(1, ControlClient {
        client_id: 1, cmd_counter: 0, echo_enabled: false, notification_tx: tx,
        paused_panes: Default::default(), subscriptions: Default::default(),
        subscription_values: Default::default(), subscription_last_check: Default::default(),
        output_gate: Arc::new(ControlOutputGate::new(None)), size: None, window_sizes: Default::default(),
    });
    app.register_client(1, true);
    start_consuming_output(&app);
    (app, ring, rx)
}

/// Push `bytes` into the pane ring, run one server loop flush, and return
/// the %output payloads the client would be written.
fn flush(app: &mut AppState, ring: &crate::control_ring::Ring, bytes: &[u8], rx: &mpsc::Receiver<ControlNotification>) -> Vec<String> {
    ring.lock().unwrap().extend(bytes);
    flush_pane_output(app);
    let mut out = Vec::new();
    while let Ok(n) = rx.recv_timeout(Duration::from_millis(50)) {
        let line = format_notification(&n);
        let prefix = format!("%output %{} ", PANE);
        if let Some(p) = line.strip_prefix(&prefix) {
            out.push(p.to_string());
        }
    }
    out
}

#[test]
fn invalid_utf8_and_unfinished_tails_reach_output_as_their_own_bytes() {
    let (mut app, ring, rx) = app_with_ring();
    // A byte that is not UTF-8 goes out as its own octal code, not as the
    // three bytes of U+FFFD (tmux control.c control_append_data copies it).
    assert_eq!(flush(&mut app, &ring, b"A\xffB", &rx), vec!["A\\377B".to_string()]);
    // The start of a character with no more output after it is sent now;
    // it does not wait in the ring for a read that may never come.
    assert_eq!(flush(&mut app, &ring, b"x\xe2\x94", &rx), vec!["x\\342\\224".to_string()]);
    assert_eq!(ring.lock().unwrap().len(), 0);
    // Valid UTF-8 is unchanged from before.
    assert_eq!(flush(&mut app, &ring, "\u{4e2d}\x1b[31m".as_bytes(), &rx), vec!["\\344\\270\\255\\033[31m".to_string()]);
    app.control_clients.clear();
    crate::control_ring::set_consumed(&[], false);
    crate::control_ring::CONTROL_CLIENTS_PRESENT.store(false, std::sync::atomic::Ordering::Release);
}
