// Follow up to #1032/#1033: the cursor across the alternate screen, read
// through expand_format as display -p shows it. tmux keeps one cursor for
// both screens (screen_alternate_on/off at tag 3.8), and both console hosts
// keep it too.
//
// MEASURED on master 7c320625 through OpenConsole (PSMUX_CONPTY_DIR), 40x10:
//   ESC[5;4H AB ESC[?1049h             psmux 0,0   host 5,4
//   ESC[4G ESC[?1049h                  psmux 0,0   host 3,0
//   ESC[5;4H AB ESC[?47h               psmux 0,0   host 5,4
//   ESC[5;4H AB ESC[?47h ESC[9;20Hx ESC[?47l   psmux 5,4   host 20,8
// The inbox conhost route was right already.

use crate::types::{AppState, Node, Pane, Window};
use std::net::{TcpListener, TcpStream};

const PANE_ID: usize = 1049;
const COLS: u16 = 40;
const ROWS: u16 = 10;

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let accept_thr = std::thread::spawn(move || listener.accept().expect("accept").0);
    let client = TcpStream::connect(addr).expect("connect");
    let server = accept_thr.join().expect("join accept thread");
    (client, server)
}

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

struct Harness {
    app: AppState,
    _peer: TcpStream,
    _reader_peer: TcpStream,
}

impl Harness {
    fn new() -> Harness {
        let (pane, peer, reader_peer) = make_pane(PANE_ID);
        let mut app = AppState::new("i1049".to_string());
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

    fn emit(&mut self, bytes: &[u8]) {
        let Node::Leaf(pane) = &self.app.windows[0].root else {
            panic!("harness window is not a leaf");
        };
        pane.term.lock().expect("term lock").process(bytes);
    }


    fn fmt(&self, f: &str) -> String {
        super::expand_format(f, &self.app)
    }
}

#[test]
fn decset_1049_keeps_the_cursor() {
    let mut h = Harness::new();
    h.emit(b"\x1b[5;4HAB\x1b[?1049h");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y},#{alternate_on}"), "5,4,1");
}

#[test]
fn decset_1049_after_cha_keeps_the_column() {
    let mut h = Harness::new();
    h.emit(b"\x1b[4G\x1b[?1049h");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "3,0");
}

#[test]
fn decrst_1049_puts_back_the_saved_cursor() {
    let mut h = Harness::new();
    h.emit(b"\x1b[5;4HAB\x1b[?1049h\x1b[9;20Hx\x1b[?1049l");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y},#{alternate_on}"), "5,4,0");
}

#[test]
fn decset_47_keeps_the_cursor_and_decrst_47_leaves_it() {
    let mut h = Harness::new();
    h.emit(b"\x1b[5;4HAB\x1b[?47h");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y},#{alternate_on}"), "5,4,1");
    h.emit(b"\x1b[9;20Hx\x1b[?47l");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y},#{alternate_on}"), "20,8,0");
}

#[test]
fn modes_1047_and_1048_change_nothing() {
    let mut h = Harness::new();
    h.emit(b"\x1b[5;4HAB\x1b[?1047h");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y},#{alternate_on}"), "5,4,0");
    h.emit(b"\x1b[?1047l\x1b[?1048h\x1b[9;20H\x1b[?1048l");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "19,8");
}