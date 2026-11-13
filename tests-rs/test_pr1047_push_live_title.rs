// PR #1047: a frame the shared server pushes by itself carries the pane's
// live OSC title. The parser holds a new title as soon as the program writes
// it, but pane.title was refreshed only on a state query, so an unsolicited
// push expanded #{pane_title} (set-titles-string, status, borders) from the
// old value. tmux sets the pane title as it parses the OSC, so every redraw
// shows the newest one.
//
// The per-session server's push is inline in run_server; the E2E suite
// tests/test_pr1047_live_title_push.ps1 covers it (and this path) end to end.

use super::SessionLoop;
use crate::types::{AppState, Node, Window};
use ratatui::layout::Rect;
use std::net::{TcpListener, TcpStream};

fn tcp_pair() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let accept = std::thread::spawn(move || listener.accept().expect("accept").0);
    let client = TcpStream::connect(addr).expect("connect");
    let server = accept.join().expect("join accept");
    (client, server)
}

/// One pane whose parser has seen `ESC ] 2 ; NEW_TITLE BEL` while pane.title
/// still holds the title from before it.
fn app_with_new_osc_title() -> (AppState, Vec<TcpStream>) {
    let (reader, peer_r) = tcp_pair();
    let (writer, peer_w) = tcp_pair();
    let mut pane = crate::proxy_pane::create_proxy_pane(
        reader,
        writer,
        "127.0.0.1:1".to_string(),
        "test-key".to_string(),
        "test-session".to_string(),
        1,
        None,
        "pane-1".to_string(),
        24,
        80,
        1,
        Some(b"\x1b]2;NEW_TITLE\x07".to_vec()),
    )
    .expect("create proxy pane");
    pane.title = "OLD_TITLE".to_string();
    let win = Window {
        root: Node::Leaf(pane),
        active_path: vec![],
        name: "w0".to_string(),
        id: 0,
        area: Rect { x: 0, y: 0, width: 80, height: 24 },
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![1],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    };
    let mut app = AppState::new("pr1047".to_string());
    app.last_window_area = Rect { x: 0, y: 0, width: 80, height: 24 };
    app.windows.push(win);
    app.active_idx = 0;
    app.allow_set_title = true;
    app.set_titles = true;
    app.set_titles_string = "#{pane_title}".to_string();
    (app, vec![peer_r, peer_w])
}

fn pane_title(app: &AppState) -> String {
    match &app.windows[0].root {
        Node::Leaf(p) => p.title.clone(),
        _ => unreachable!(),
    }
}

#[test]
fn pushed_frame_carries_the_live_osc_title() {
    let (mut app, _peers) = app_with_new_osc_title();
    let mut lp = SessionLoop::new(Default::default());
    super::super::frame::push(&mut app, &mut lp).expect("push");
    let frame = lp.frame.combined_buf.clone();
    assert!(
        frame.contains("\"host_title\":\"NEW_TITLE\""),
        "the pushed frame's host title is the live one: {}",
        frame.split("host_title").nth(1).map(|s| &s[..s.len().min(40)]).unwrap_or("(no host_title)")
    );
    assert!(!frame.contains("OLD_TITLE"), "no part of the pushed frame uses the stale title");
    assert_eq!(pane_title(&app), "NEW_TITLE");
}

#[test]
fn allow_set_title_off_keeps_the_old_title() {
    let (mut app, _peers) = app_with_new_osc_title();
    app.allow_set_title = false;
    let mut lp = SessionLoop::new(Default::default());
    super::super::frame::push(&mut app, &mut lp).expect("push");
    assert_eq!(pane_title(&app), "OLD_TITLE", "allow-set-title off ignores the program's title");
    assert!(lp.frame.combined_buf.contains("\"host_title\":\"OLD_TITLE\""));
}

#[test]
fn a_locked_title_is_not_overwritten_by_a_push() {
    let (mut app, _peers) = app_with_new_osc_title();
    if let Node::Leaf(p) = &mut app.windows[0].root {
        p.title_locked = true;
    }
    let mut lp = SessionLoop::new(Default::default());
    super::super::frame::push(&mut app, &mut lp).expect("push");
    assert_eq!(pane_title(&app), "OLD_TITLE", "select-pane -T locks the title");
}
