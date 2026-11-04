// Issue #985: `#{bracket_paste_flag}` printed nothing. tmux 3.7 added it
// (CHANGES: "Add bracket_paste_flag format flag", issue 4951), and format.c
// format_cb_bracket_paste_flag returns "1" when the pane's screen has
// MODE_BRACKETPASTE (DECSET 2004), "0" otherwise, and nothing without a pane.
//
// MEASURED on 8a1e4fea, `-L r985`, a pane that wrote ESC[?2004h:
//
//   display -p '#{bracket_paste_flag}'  -> ''   (also before and after ?2004l)
//
// The harness is the one from test_issue662_mouse_flag_formats.rs: a real
// parser fed real DECSET bytes, read back through expand_format.

use crate::types::{AppState, Node, Pane, Window};
use std::net::{TcpListener, TcpStream};

const PANE_ID: usize = 985;
const COLS: u16 = 40;
const ROWS: u16 = 8;

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let accept_thr = std::thread::spawn(move || listener.accept().expect("accept").0);
    let client = TcpStream::connect(addr).expect("connect");
    let server = accept_thr.join().expect("join accept thread");
    (client, server)
}

/// A pane with a real parser on one end and a live pty peer on the other, kept
/// alive by the returned streams.
fn make_pane(id: usize) -> (Pane, TcpStream, TcpStream) {
    let (reader, reader_peer) = tcp_pair();
    let (writer, peer) = tcp_pair();
    let pane = crate::proxy_pane::create_proxy_pane(
        reader,
        writer,
        "127.0.0.1:1".to_string(),
        "test-key".to_string(),
        "test-session".to_string(),
        id as u64,
        None,
        format!("pane-{id}"),
        ROWS,
        COLS,
        id,
        None,
    )
    .expect("create proxy pane");
    (pane, peer, reader_peer)
}

fn feed(pane: &Pane, bytes: &[u8]) {
    pane.term.lock().expect("term lock").process(bytes);
}

/// One window holding one pane whose parser can be fed real DECSET bytes, so
/// every assertion below goes through the same path a pane program's output
/// takes.
struct Harness {
    app: AppState,
    _peer: TcpStream,
    _reader_peer: TcpStream,
}

impl Harness {
    fn new() -> Harness {
        let (pane, peer, reader_peer) = make_pane(PANE_ID);

        let mut app = AppState::new("i985".to_string());
        app.window_base_index = 0;
        app.windows.push(Window {
            root: Node::Leaf(pane),
            active_path: vec![],
            name: "w0".to_string(),
            id: 0,
            area: app.client_area,
            window_size: None,
            window_options: Default::default(),
            activity_flag: false,
            bell_flag: false,
            silence_flag: false,
            last_output_time: std::time::Instant::now(),
            last_seen_version: 0,
            manual_rename: false,
            layout_index: 0,
            pane_mru: vec![PANE_ID],
            zoom_saved: None,
            linked_from: None,
            floating: Vec::new(),
            floating_focus: None,
        });
        Harness { app, _peer: peer, _reader_peer: reader_peer }
    }

    /// Feed bytes to the pane's parser, the way pane output arrives.
    fn emit(&mut self, bytes: &[u8]) {
        let Node::Leaf(pane) = &self.app.windows[0].root else {
            panic!("harness window is not a leaf");
        };
        feed(pane, bytes);
    }

    fn var(&self, name: &str) -> String {
        super::expand_format(&format!("#{{{name}}}"), &self.app)
    }
}

#[test]
fn a_plain_pane_reports_zero_not_empty() {
    let h = Harness::new();
    assert_eq!(h.var("bracket_paste_flag"), "0", "#985: the name must resolve");
}

#[test]
fn decset_2004_turns_it_on_and_decrst_turns_it_off() {
    let mut h = Harness::new();
    h.emit(b"\x1b[?2004h");
    assert_eq!(h.var("bracket_paste_flag"), "1");
    h.emit(b"\x1b[?2004l");
    assert_eq!(h.var("bracket_paste_flag"), "0");
}

#[test]
fn it_is_independent_of_the_mouse_and_alternate_screen_modes() {
    let mut h = Harness::new();
    h.emit(b"\x1b[?1000h\x1b[?1049h");
    assert_eq!(h.var("bracket_paste_flag"), "0");
    h.emit(b"\x1b[?2004h\x1b[?1049l\x1b[?1000l");
    assert_eq!(h.var("bracket_paste_flag"), "1");
}

#[test]
fn it_works_inside_a_conditional_as_a_client_would_ask() {
    let mut h = Harness::new();
    let ask = "#{?bracket_paste_flag,safe,hold}";
    assert_eq!(super::expand_format(ask, &h.app), "hold");
    h.emit(b"\x1b[?2004h");
    assert_eq!(super::expand_format(ask, &h.app), "safe");
}

#[test]
fn ris_drops_it_as_a_full_reset_does_in_tmux() {
    let mut h = Harness::new();
    h.emit(b"\x1b[?2004h");
    h.emit(b"\x1bc");
    assert_eq!(h.var("bracket_paste_flag"), "0");
}
