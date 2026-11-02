// Raw ring bytes through flush_pane_output and handle_connection's real writer.
// No psmux server, child process or ConPTY is started.
use crate::control::{emit_to_client, flush_pane_output, start_consuming_output, ControlOutputGate};
use crate::control_ring::{Ring, OUTPUT_CHUNK_MAX, TEST_SERIAL};
use crate::types::{AppState, ControlClient, ControlNotification, CtrlReq, Node, Pane, Window};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PANE: usize = 7;
const BARRIER: &str = "%message control-output-bytes-barrier";

struct WireClient {
    id: u64,
    reader: BufReader<TcpStream>,
    handler: JoinHandle<()>,
    gate: Arc<ControlOutputGate>,
}

struct Harness {
    app: AppState,
    ring: Ring,
    clients: Vec<WireClient>,
    was_present: bool,
}

impl Harness {
    fn new() -> Self {
        let size = portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 };
        let (master, writer) = crate::util::stub_pane_pty(size);
        let ring: Ring = Default::default();
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
        let mut app = AppState::new("control-output-bytes".into());
        app.windows.push(Window {
            root: Node::Leaf(pane), active_path: vec![], name: "w".into(), id: 1,
            area: ratatui::layout::Rect::new(0, 0, 80, 24),
            window_size: None, window_options: Default::default(),
            activity_flag: false, bell_flag: false, silence_flag: false,
            last_output_time: Instant::now(), last_seen_version: 0, manual_rename: false,
            layout_index: 0, pane_mru: vec![], zoom_saved: None, linked_from: None,
            floating: vec![], floating_focus: None,
        });
        let was_present = crate::control_ring::CONTROL_CLIENTS_PRESENT.load(Ordering::Acquire);
        Self { app, ring, clients: vec![], was_present }
    }

    fn connect(&mut self, pause_after_ms: Option<u64>) -> usize {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (server, _) = listener.accept().unwrap();
        let (tx, rx) = mpsc::channel();
        let handler = std::thread::spawn(move || {
            crate::server::connection::handle_connection(server, tx, "output-bytes-test", Default::default());
        });
        socket.write_all(b"AUTH output-bytes-test\nCONTROL_QUIET\n").unwrap();
        let CtrlReq::ControlRegister { client_id, echo, notif_tx, output_gate, .. } =
            rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else { panic!("expected the real connection's control registration") };
        assert!(!echo);
        assert_eq!(output_gate.queued_bytes(), 0);
        assert_eq!(output_gate.pause_after_ms(), None);
        output_gate.set_pause_after(pause_after_ms);
        // Install the sender and gate supplied by the production connection,
        // as ControlRegister does; no test constructs output notifications.
        self.app.control_clients.insert(client_id, ControlClient {
            client_id, cmd_counter: 0, echo_enabled: echo, notification_tx: notif_tx,
            paused_panes: Default::default(), subscriptions: Default::default(),
            subscription_values: Default::default(), subscription_last_check: Default::default(),
            output_gate: output_gate.clone(), size: None, window_sizes: Default::default(),
        });
        self.app.register_client(client_id, true);
        start_consuming_output(&self.app);
        self.clients.push(WireClient {
            id: client_id, reader: BufReader::new(socket), handler, gate: output_gate,
        });
        let index = self.clients.len() - 1;
        assert_eq!(self.line(index), "OK");
        let begin = self.line(index);
        assert!(begin.starts_with("%begin ") && begin.ends_with(" 1 0"), "{begin:?}");
        assert_eq!(self.line(index), begin.replacen("%begin", "%end", 1));
        index
    }

    fn line(&mut self, client: usize) -> String {
        let mut line = String::new();
        assert!(self.clients[client].reader.read_line(&mut line).unwrap() > 0, "unexpected wire EOF");
        assert!(line.ends_with('\n'), "unterminated protocol line: {line:?}");
        line.pop();
        line
    }

    fn flush(&mut self, bytes: &[u8]) {
        assert!(self.ring.lock().unwrap().is_empty(), "precondition: previous raw output drained");
        self.ring.lock().unwrap().extend(bytes);
        assert_eq!(self.ring.lock().unwrap().len(), bytes.len());
        flush_pane_output(&mut self.app);
        // The real writer processes this after all preceding notifications.
        // It also makes an empty flush observable without a sleep or timeout.
        for client in &self.clients {
            emit_to_client(&self.app, client.id, ControlNotification::Message {
                text: "control-output-bytes-barrier".into(),
            });
        }
    }

    fn output(&mut self, client: usize, extended: bool) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        loop {
            let line = self.line(client);
            if line == BARRIER { break; }
            assert_eq!(line.starts_with("%extended-output "), extended, "{line:?}");
            let (pane, payload) = crate::scrollback::parse_output(&line)
                .unwrap_or_else(|| panic!("unexpected notification: {line:?}"));
            assert_eq!(pane, PANE);
            chunks.push(crate::scrollback::unescape_output(payload));
        }
        assert_eq!(self.clients[client].gate.queued_bytes(), 0, "real writer must release queued bytes");
        chunks
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.app.control_clients.clear();
        for client in self.clients.drain(..) {
            let _ = client.reader.get_ref().shutdown(Shutdown::Both);
            client.handler.join().unwrap();
            assert!(!crate::types::has_persistent_stream(client.id));
        }
        crate::control_ring::set_consumed(&[], false);
        crate::control_ring::CONTROL_CLIENTS_PRESENT.store(self.was_present, Ordering::Release);
    }
}

#[test]
fn all_bytes_reach_plain_and_extended_writers_and_pause_is_client_local() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    let plain = h.connect(None);
    let extended = h.connect(Some(300_000));
    let bytes: Vec<u8> = (0..=255).collect();
    assert_eq!(bytes.len(), 256);
    assert!(std::str::from_utf8(&bytes).is_err());
    assert_eq!(h.clients.len(), 2);
    assert!(!h.clients[plain].gate.is_paused(PANE));
    assert!(!h.clients[extended].gate.is_paused(PANE));
    h.flush(&bytes);
    assert_eq!(h.output(plain, false).concat(), bytes);
    assert_eq!(h.output(extended, true).concat(), bytes);
    assert!(h.ring.lock().unwrap().is_empty());

    // Exercise the same escaper through both real writer branches, with an
    // explicit oracle independent of the client's decoder. Tab stays literal.
    h.flush(b"a\\b\t\r\n\x00\x7f\x80\xff");
    let escaped = "a\\134b\t\\015\\012\\000\\177\\200\\377";
    assert_eq!(h.line(plain), format!("%output %7 {escaped}"));
    let line = h.line(extended);
    let rest = line.strip_prefix("%extended-output %7 ").unwrap();
    let (age, payload) = rest.split_once(" : ").unwrap();
    assert!(age.parse::<u64>().unwrap() < 300_000);
    assert_eq!(payload, escaped);
    assert!(h.output(plain, false).is_empty());
    assert!(h.output(extended, true).is_empty());

    assert!(h.clients[plain].gate.pause_pane(PANE));
    assert!(h.clients[plain].gate.is_paused(PANE));
    assert!(!h.clients[extended].gate.is_paused(PANE));
    h.flush(b"only the unpaused client receives this\xff");
    assert!(h.output(plain, false).is_empty());
    assert_eq!(h.output(extended, true).concat(), b"only the unpaused client receives this\xff");
    assert!(h.clients[plain].gate.continue_pane(PANE));
    h.flush(b"new output after continue\x80");
    assert_eq!(h.output(plain, false).concat(), b"new output after continue\x80");
    assert_eq!(h.output(extended, true).concat(), b"new output after continue\x80");
}

#[test]
fn incomplete_utf8_tails_reach_the_writer_without_a_later_read() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    let client = h.connect(None);
    for tail in [b"\xc2".as_slice(), b"\xe2", b"\xe2\x94", b"\xf0", b"\xf0\x9f", b"\xf0\x9f\x98"] {
        assert!(std::str::from_utf8(tail).is_err());
        h.flush(tail);
        assert_eq!(h.output(client, false).concat(), tail, "tail must arrive before any continuation");
        assert!(h.ring.lock().unwrap().is_empty(), "tail must not remain in the ring");
    }
}

#[test]
fn raw_chunks_reach_the_writer_in_order_across_the_payload_limit() {
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut h = Harness::new();
    let client = h.connect(None);
    let mut bytes: Vec<u8> = (0..2 * OUTPUT_CHUNK_MAX + 257).map(|i| (i % 256) as u8).collect();
    bytes[OUTPUT_CHUNK_MAX - 1..OUTPUT_CHUNK_MAX + 2].copy_from_slice(b"\xe2\x94\x80");
    bytes.extend_from_slice(b"\xf0\x9f\x98");
    assert!(bytes.len() > 2 * OUTPUT_CHUNK_MAX && bytes.len() < 3 * OUTPUT_CHUNK_MAX);
    assert!(std::str::from_utf8(&bytes).is_err());
    h.flush(&bytes);
    let chunks = h.output(client, false);
    let actual = chunks.concat();
    assert!(actual == bytes, "wire byte mismatch: received {} of {} bytes; first difference {:?}",
        actual.len(), bytes.len(), actual.iter().zip(&bytes).position(|(a, b)| a != b));
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|c| !c.is_empty() && c.len() <= OUTPUT_CHUNK_MAX));
    assert!(h.ring.lock().unwrap().is_empty());
}
