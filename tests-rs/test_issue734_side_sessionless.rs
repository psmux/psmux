// Issue #734 side finding 1: a held standby (`start-server` + `set -g
// exit-empty off`, no session yet) answered `display-message -p
// '#{session_name}'` with `__warm__`, its internal pool name, plus the spare
// window it keeps for the next new-session (`#I` 0, `#W` pwsh, `#{pane_id}`
// %1). tmux has no session there: cmd-display-message.c finds its target with
// CMD_FIND_CANFAIL, format_defaults gets no session, window or pane, and every
// one of those formats is empty. Server scope formats still resolve.
//
// The server loop runs a held standby's client requests inside
// `set_sessionless(app.is_held_standby())`; these tests drive the format engine
// in that mode against a standby shaped AppState (a name of `__warm__`, one
// window, exit-empty off).

use super::*;
use crate::types::{AppState, LayoutKind, Node, Window};

fn make_window(name: &str, id: usize) -> Window {
    Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
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

fn held_standby() -> AppState {
    let mut app = AppState::new("__warm__".to_string());
    app.exit_empty = false;
    app.windows.push(make_window("pwsh", 1));
    app
}

#[test]
fn a_held_standby_is_recognised_and_a_plain_standby_is_not() {
    let mut app = held_standby();
    assert!(app.is_held_standby());
    app.exit_empty = true;
    assert!(!app.is_held_standby(), "exit-empty on: an internal standby, not the user's empty server");
    let mut claimed = held_standby();
    claimed.session_name = "work".to_string();
    assert!(!claimed.is_held_standby(), "a claimed standby is a session");
}

#[test]
fn session_window_and_pane_formats_are_empty_with_no_session() {
    let app = held_standby();
    let out = with_sessionless(true, || {
        expand_format(
            "[#{session_name}][#S][#{session_id}][#{session_windows}][#I][#W][#{window_index}][#{window_name}][#{pane_id}][#D][#{client_session}]",
            &app,
        )
    });
    assert_eq!(out, "[][][][][][][][][][][]");
    assert!(!out.contains("__warm__"));
}

#[test]
fn server_scope_formats_still_resolve_with_no_session() {
    let app = held_standby();
    let out = with_sessionless(true, || expand_format("#{pid}|#{server_sessions}|#{version}", &app));
    let parts: Vec<&str> = out.split('|').collect();
    assert_eq!(parts[0], std::process::id().to_string());
    assert_eq!(parts[1], "0", "tmux format_cb_server_sessions counts no sessions");
    assert!(!parts[2].is_empty());
}

#[test]
fn server_scope_formats_are_the_same_with_and_without_a_session() {
    // These describe the server, not a session: the held server must answer
    // them exactly as a server with a session does (#{socket_path} went empty
    // when no-session mode routed every lookup through the windowless path).
    let app = held_standby();
    for var in ["socket_path", "start_time", "host", "host_short", "version", "pid", "prefix", "user"] {
        let f = format!("#{{{}}}", var);
        let normal = expand_format(&f, &app);
        let held = with_sessionless(true, || expand_format(&f, &app));
        assert_eq!(held, normal, "#{{{}}}", var);
    }
}

#[test]
fn pane_state_formats_are_empty_with_no_session() {
    let app = held_standby();
    for var in ["cursor_x", "history_size", "alternate_on", "pane_current_command", "pane_current_path", "window_flags", "mouse_any_flag"] {
        let f = format!("#{{{}}}", var);
        assert_eq!(with_sessionless(true, || expand_format(&f, &app)), "", "#{{{}}}", var);
    }
    assert_eq!(with_sessionless(true, || expand_format("[#T][#P][#F][#D]", &app)), "[][][][]");
}

#[test]
fn loops_have_nothing_to_walk_with_no_session() {
    let app = held_standby();
    let out = with_sessionless(true, || {
        expand_format("[#{S:#{session_name}}][#{W:#{window_name}}][#{P:#{pane_id}}]", &app)
    });
    assert_eq!(out, "[][][]");
}

#[test]
fn the_mode_is_scoped_and_the_same_state_formats_normally_outside_it() {
    let app = held_standby();
    // Outside the mode (the server's own work, a claimed session), the state
    // formats as it always did.
    assert_eq!(expand_format("#{session_name}:#W", &app), "__warm__:pwsh");
    assert_eq!(with_sessionless(true, || expand_format("#{session_name}", &app)), "");
    assert!(!sessionless(), "the guard restores the previous mode");
    assert_eq!(expand_format("#S", &app), "__warm__");
}

#[test]
fn a_claimed_session_formats_its_name() {
    let mut app = held_standby();
    app.session_name = "work".to_string();
    let on = app.is_held_standby();
    assert_eq!(with_sessionless(on, || expand_format("#{session_name}|#S|#W", &app)), "work|work|pwsh");
}
