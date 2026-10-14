// Issue #803: session options shared the global store.
//
//     psmux set -g -t pr prefix C-a      # global=C-a  session=C-a
//     psmux set    -t pr prefix C-x      # global=C-x  session=C-x   <- global changed
//     psmux set -u -t pr prefix          # global=C-b  session=C-b   <- default, not C-a
//
// tmux keeps a table per session whose parent is the global session table:
// `set` without -g writes the session table, `set -u` removes the entry so the
// global shows through (options_remove_or_default only writes a default into
// a GLOBAL table), reads walk session then global (options_get), `-o` asks the
// target table (options_get_only) and `show -A` marks an inherited value `*`
// (cmd-show-options.c:197).
//
// psmux runs one session per server, so the typed AppState fields hold the
// value the session uses and AppState::session_option_globals keeps the
// global an override shadows. These tests pin the set / unset / show paths
// and the -g writes that land while the session overrides an option.

use super::*;
use crate::types::{AppState, LayoutKind, Node};
use crossterm::event::{KeyCode, KeyModifiers};

fn window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
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

/// A running session: it has a window, so config lines reach session scope.
fn session() -> AppState {
    let mut app = AppState::new("pr".to_string());
    app.windows.clear();
    app.windows.push(window("w", 0));
    app.active_idx = 0;
    app
}

fn ctrl(c: char) -> Option<(KeyCode, KeyModifiers)> {
    Some((KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn set_g(app: &mut AppState, name: &str, value: &str) {
    // The -g route: a session entry diverts it, otherwise the global setter.
    if session_overrides(app, name) {
        write_shadowed_global(app, name, GlobalWrite::Set(value)).unwrap();
    } else {
        apply_set_option(app, name, value, false).unwrap();
        app.user_set_options.insert(name.to_string());
    }
}

fn set_s(app: &mut AppState, name: &str, value: &str) -> String {
    apply_set_session_option(app, name, value, false, false, false, false)
}

fn unset_s(app: &mut AppState, name: &str) {
    apply_set_session_option(app, name, "", true, false, false, false);
}

fn global(app: &AppState, name: &str) -> String {
    session_option_query(app, name, true).0
}

fn session_value(app: &AppState, name: &str) -> String {
    session_option_query(app, name, false).0
}

#[test]
fn the_reporters_prefix_sequence() {
    let mut app = session();
    set_g(&mut app, "prefix", "C-a");
    assert_eq!((global(&app, "prefix"), session_value(&app, "prefix")), ("C-a".into(), "C-a".into()));

    assert_eq!(set_s(&mut app, "prefix", "C-x"), "");
    assert_eq!(global(&app, "prefix"), "C-a", "BUG #803: a session write replaced the global");
    assert_eq!(session_value(&app, "prefix"), "C-x");
    assert_eq!(app.prefix_key, ctrl('x'), "the session's prefix is what the keys see");

    unset_s(&mut app, "prefix");
    assert_eq!(global(&app, "prefix"), "C-a");
    assert_eq!(session_value(&app, "prefix"), "C-a", "BUG #803: -u reset to C-b instead of the global");
    assert_eq!(app.prefix_key, ctrl('a'));
    assert!(!session_overrides(&app, "prefix"));
}

#[test]
fn the_reporters_status_left_sequence() {
    let mut app = session();
    set_g(&mut app, "status-left", "GLOBAL");
    set_s(&mut app, "status-left", "SESS");
    assert_eq!(global(&app, "status-left"), "GLOBAL");
    assert_eq!(session_value(&app, "status-left"), "SESS");
    assert_eq!(app.status_left, "SESS", "the status bar draws the session value");
    unset_s(&mut app, "status-left");
    assert_eq!(app.status_left, "GLOBAL");
    assert_eq!(global(&app, "status-left"), "GLOBAL");
}

#[test]
fn unset_without_a_session_entry_changes_nothing() {
    let mut app = session();
    set_g(&mut app, "status-left", "KEEP");
    unset_s(&mut app, "status-left");
    assert_eq!(app.status_left, "KEEP");
    assert_eq!(global(&app, "status-left"), "KEEP");
}

#[test]
fn unset_falls_back_to_the_default_when_the_global_was_never_set() {
    let mut app = session();
    set_s(&mut app, "status-left", "SESS");
    assert_eq!(app.session_option_globals.get("status-left"), Some(&None));
    assert_eq!(global(&app, "status-left"), "[#S] ");
    unset_s(&mut app, "status-left");
    assert_eq!(app.status_left, "[#S] ");
    assert!(!app.user_set_options.contains("status-left"), "the global is still unset for -o");
}

#[test]
fn global_writes_under_an_override_reach_only_the_global() {
    let mut app = session();
    set_s(&mut app, "status-left", "SESS");
    set_g(&mut app, "status-left", "G2");
    assert_eq!(app.status_left, "SESS");
    assert_eq!(global(&app, "status-left"), "G2");

    write_shadowed_global(&mut app, "status-left", GlobalWrite::Append("+A")).unwrap();
    assert_eq!(global(&app, "status-left"), "G2+A");

    write_shadowed_global(&mut app, "status-left", GlobalWrite::Unset).unwrap();
    assert_eq!(global(&app, "status-left"), "[#S] ");
    assert_eq!(app.status_left, "SESS");
    unset_s(&mut app, "status-left");
    assert_eq!(app.status_left, "[#S] ");

    // A key option is stored the way show prints it.
    set_s(&mut app, "prefix", "C-x");
    write_shadowed_global(&mut app, "prefix", GlobalWrite::Set("c-a")).unwrap();
    assert_eq!(global(&app, "prefix"), "C-a");
    assert_eq!(app.prefix_key, ctrl('x'));
    assert_eq!(
        write_shadowed_global(&mut app, "prefix", GlobalWrite::Set("NoSuchKey")),
        Err("bad key: NoSuchKey".to_string())
    );
    assert_eq!(global(&app, "prefix"), "C-a");
}

#[test]
fn divert_consumes_only_requests_for_an_overridden_option() {
    use crate::types::CtrlReq;
    let mut app = session();
    let passed = divert_shadowed_global_request(
        &mut app,
        CtrlReq::SetOptionQuiet("status-left".into(), "G".into(), false),
    );
    assert!(passed.is_some(), "no session entry: the ordinary global path runs");

    set_s(&mut app, "status-left", "SESS");
    let passed = divert_shadowed_global_request(
        &mut app,
        CtrlReq::SetOptionQuiet("status-left".into(), "G".into(), false),
    );
    assert!(passed.is_none());
    assert_eq!(global(&app, "status-left"), "G");
    assert_eq!(app.status_left, "SESS");

    let (tx, rx) = std::sync::mpsc::channel();
    divert_shadowed_global_request(
        &mut app,
        CtrlReq::SetOptionOnlyIfUnset("status-left".into(), "H".into(), Some(tx)),
    );
    assert_eq!(rx.recv().unwrap(), "ERROR: already set: status-left");

    divert_shadowed_global_request(&mut app, CtrlReq::SetOptionUnset("status-left".into()));
    assert_eq!(global(&app, "status-left"), "[#S] ");
    assert_eq!(app.status_left, "SESS");
}

#[test]
fn only_if_unset_asks_the_session_table() {
    let mut app = session();
    set_g(&mut app, "status-right", "GR");
    assert_eq!(apply_set_session_option(&mut app, "status-right", "S1", false, false, true, false), "");
    assert_eq!(app.status_right, "S1");
    assert_eq!(
        apply_set_session_option(&mut app, "status-right", "S2", false, false, true, false),
        "ERROR: already set: status-right"
    );
    assert_eq!(apply_set_session_option(&mut app, "status-right", "S2", false, false, true, true), "");
    assert_eq!(app.status_right, "S1");
    assert_eq!(global(&app, "status-right"), "GR");
}

#[test]
fn append_and_toggle_work_on_the_session_value() {
    let mut app = session();
    set_g(&mut app, "status-left", "G");
    set_s(&mut app, "status-left", "S");
    apply_set_session_option(&mut app, "status-left", "+x", false, true, false, false);
    assert_eq!(app.status_left, "S+x");
    assert_eq!(global(&app, "status-left"), "G");

    set_g(&mut app, "mouse", "on");
    assert_eq!(apply_set_session_option(&mut app, "mouse", "", false, false, false, false), "");
    assert_eq!(session_value(&app, "mouse"), "off");
    assert_eq!(global(&app, "mouse"), "on");
    unset_s(&mut app, "mouse");
    assert!(app.mouse_enabled);
}

#[test]
fn a_bad_session_value_is_refused_and_leaves_no_entry() {
    let mut app = session();
    let reply = set_s(&mut app, "prefix", "NotAKey");
    assert_eq!(reply, "ERROR: bad key: NotAKey");
    assert!(!session_overrides(&app, "prefix"));
    assert_eq!(app.prefix_key, ctrl('b'));
}

#[test]
fn only_session_scope_names_are_session_options() {
    assert!(is_session_option("prefix"));
    assert!(is_session_option("status-left"));
    assert!(is_session_option("key-table"));
    assert!(!is_session_option("escape-time"), "server option");
    assert!(!is_session_option("remain-on-exit"), "window option");
    assert!(!is_session_option("@user"), "user options keep their one store");
}

#[test]
fn show_listing_for_g_and_a() {
    let mut app = session();
    set_g(&mut app, "status-left", "GL");
    set_s(&mut app, "status-left", "SL");
    let text = "prefix C-b\nstatus-left SL\nescape-time 500\n";
    assert_eq!(
        scope_session_listing(&app, text, true, false),
        "prefix C-b\nstatus-left GL\nescape-time 500\n"
    );
    assert_eq!(
        scope_session_listing(&app, text, false, true),
        "prefix* C-b\nstatus-left SL\nescape-time 500\n",
        "-A marks the inherited session option only"
    );
    assert_eq!(session_option_query(&app, "prefix", false), ("C-b".to_string(), true));
    assert_eq!(session_option_query(&app, "status-left", false), ("SL".to_string(), false));
}

#[test]
fn config_route_session_scope_needs_a_running_session() {
    // Startup: no window yet, so a bare `set` keeps landing in the global
    // table (the rule an untargeted -w line follows, #648).
    let mut boot = AppState::new("boot".to_string());
    boot.windows.clear();
    crate::config::parse_config_content(&mut boot, "set status-left CONF\n");
    assert_eq!(boot.status_left, "CONF");
    assert!(!session_overrides(&boot, "status-left"));

    // Runtime (command prompt, a binding, source-file): the session's table.
    let mut app = session();
    crate::config::parse_config_content(&mut app, "set -g status-left GL\nset status-left SL\n");
    assert_eq!(app.status_left, "SL");
    assert_eq!(global(&app, "status-left"), "GL");
    crate::config::parse_config_content(&mut app, "set -g status-left G2\n");
    assert_eq!(app.status_left, "SL", "-g under an override reaches the global only");
    assert_eq!(global(&app, "status-left"), "G2");
    crate::config::parse_config_content(&mut app, "set -u status-left\n");
    assert_eq!(app.status_left, "G2");
}

#[test]
fn nested_toggle_round_trip_restores_the_users_values() {
    let mut app = session();
    crate::config::parse_config_content(&mut app, "set -g prefix C-a\nset -g status-left GL\n");
    for _ in 0..2 {
        crate::config::parse_config_content(
            &mut app,
            "set prefix None\nset key-table off\nset status-left OFF\n",
        );
        assert_eq!(app.prefix_key, None);
        assert_eq!(app.status_left, "OFF");
        assert_eq!(global(&app, "prefix"), "C-a");
        assert_eq!(global(&app, "status-left"), "GL");
        assert_eq!(get_option_value(&app, "key-table"), "off");
        crate::config::parse_config_content(
            &mut app,
            "set -u prefix\nset -u key-table\nset -u status-left\n",
        );
        assert_eq!(app.prefix_key, ctrl('a'));
        assert_eq!(app.status_left, "GL");
        assert_eq!(get_option_value(&app, "key-table"), "root");
        assert!(app.session_option_globals.is_empty());
    }
}
