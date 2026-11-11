// Issues #1032 and #1034: `#{pane_tabs}`, `#{scroll_region_upper}`,
// `#{scroll_region_lower}` and `#{origin_flag}` read the pane's live parser,
// as tmux's format.c reads `wp->base` (format_cb_pane_tabs,
// format_cb_scroll_region_upper/lower, format_cb_origin_flag at tag 3.8).
//
// MEASURED on master 32aa803b through OpenConsole (PSMUX_CONPTY_DIR), a 40x10
// pane: after `ESC[4G ESC H` the cursor was right but `#{pane_tabs}` said
// 8,16,24,32; after `ESC[3;6r ESC[?6h` the formats said 0, 9 and 0.
//
// The harness is the one from test_issue985_bracket_paste_flag.rs: a real
// parser fed real bytes, read back through expand_format. These compile on
// master too, where they fail, since only the format names are used.

use crate::types::{AppState, Node, Pane, Window};
use std::net::{TcpListener, TcpStream};

const PANE_ID: usize = 1032;
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
        let mut app = AppState::new("i1032".to_string());
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

    fn resize(&mut self, rows: u16, cols: u16) {
        let Node::Leaf(pane) = &self.app.windows[0].root else {
            panic!("harness window is not a leaf");
        };
        pane.term.lock().expect("term lock").screen_mut().set_size(rows, cols);
    }

    fn fmt(&self, f: &str) -> String {
        super::expand_format(f, &self.app)
    }
}

#[test]
fn pane_tabs_is_the_default_table_until_a_program_changes_it() {
    let h = Harness::new();
    assert_eq!(h.fmt("#{pane_tabs}"), "8,16,24,32");
}

#[test]
fn pane_tabs_follows_hts_and_tbc() {
    let mut h = Harness::new();
    h.emit(b"\x1b[4G\x1bH");
    assert_eq!(h.fmt("#{pane_tabs}"), "3,8,16,24,32");
    h.emit(b"\x1b[9G\x1b[g");
    assert_eq!(h.fmt("#{pane_tabs}"), "3,16,24,32");
    h.emit(b"\r\t");
    assert_eq!(h.fmt("#{cursor_x}"), "3", "HT goes to the stop ESC H set");
    h.emit(b"\x1b[3g");
    assert_eq!(h.fmt("#{pane_tabs}"), "");
    h.emit(b"\r\t");
    assert_eq!(h.fmt("#{cursor_x}"), "39", "no stop left: HT goes to the last column");
}

#[test]
fn pane_tabs_goes_back_to_the_default_on_a_change_of_width_only() {
    let mut h = Harness::new();
    h.emit(b"\x1b[4G\x1bH");
    h.resize(8, COLS);
    assert_eq!(h.fmt("#{pane_tabs}"), "3,8,16,24,32", "height alone keeps the table");
    h.resize(8, 30);
    assert_eq!(h.fmt("#{pane_tabs}"), "8,16,24", "width resets it (screen.c:373)");
}

#[test]
fn ris_puts_the_default_tabs_back() {
    let mut h = Harness::new();
    h.emit(b"\x1b[3g\x1bc");
    assert_eq!(h.fmt("#{pane_tabs}"), "8,16,24,32");
}

#[test]
fn scroll_region_and_origin_formats_read_the_pane() {
    let mut h = Harness::new();
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower},#{origin_flag}"), "0,9,0");
    h.emit(b"\x1b[3;6r");
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower},#{origin_flag}"), "2,5,0");
    h.emit(b"\x1b[?6h");
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower},#{origin_flag}"), "2,5,1");
    h.emit(b"\x1b[2d");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "0,3", "VPA counts from the region top");
    h.emit(b"\x1b[9d");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "0,5", "VPA stops at the region bottom");
    h.emit(b"\x1b#8");
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower},#{origin_flag}"), "0,9,0");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "0,0");
}

#[test]
fn decstbm_ignores_a_bad_region_and_keeps_the_cursor() {
    let mut h = Harness::new();
    h.emit(b"\x1b[3;6r\x1b[5;10H\x1b[6;3r");
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower}"), "2,5");
    assert_eq!(h.fmt("#{cursor_x},#{cursor_y}"), "9,4");
}

#[test]
fn the_region_shrinks_with_the_pane() {
    let mut h = Harness::new();
    h.emit(b"\x1b[3;9r");
    h.resize(5, COLS);
    assert_eq!(h.fmt("#{scroll_region_upper},#{scroll_region_lower}"), "2,4");
    h.emit(b"\x1b[?6h\x1b[50d");
    assert_eq!(h.fmt("#{cursor_y}"), "4");
}
