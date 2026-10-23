// Issue #724: list-clients ignored -F and display-message ignored -c, so no
// script could map an attached client to its process or tell a read only
// client (`attach -r`) from a writable one.
//
// These tests drive the format engine against an AppState with hand built
// clients (no server, no sockets) and the pure helpers the CLI and the server
// use to parse `attach -r` and to resolve a client name the way tmux's
// cmd_find_client does.

use super::*;
use crate::types::{ClientInfo, ClientSel};

fn client(id: u64, pid: u32, readonly: bool, control: bool) -> ClientInfo {
    ClientInfo {
        id,
        width: 120,
        height: 29,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: crate::types::client_tty_name(id, Some(pid)),
        is_control: control,
        last_session: None,
        pid,
        readonly,
        focused: false,
    }
}

fn app_with_two_clients() -> AppState {
    let mut app = AppState::new("work".to_string());
    app.client_registry.insert(3, client(3, 16692, false, false));
    app.client_registry.insert(5, client(5, 13276, true, false));
    app.latest_client_id = Some(3);
    app
}

#[test]
fn a_client_is_named_after_its_pid_when_known() {
    assert_eq!(crate::types::client_tty_name(7, Some(4242)), "/dev/pts/4242");
    assert_eq!(crate::types::client_tty_name(7, None), "/dev/pts/7");
    assert_eq!(crate::types::client_tty_name(7, Some(0)), "/dev/pts/7");
}

#[test]
fn register_client_with_pid_records_pid_and_name() {
    let mut app = AppState::new("work".to_string());
    assert!(app.register_client_with_pid(9, false, Some(31104)));
    let ci = &app.client_registry[&9];
    assert_eq!(ci.pid, 31104);
    assert_eq!(ci.tty_name, "/dev/pts/31104");
    assert!(!ci.readonly);
    // The pid-less path keeps the connection id name.
    assert!(app.register_client(10, true));
    assert_eq!(app.client_registry[&10].tty_name, "/dev/pts/10");
    assert_eq!(app.client_registry[&10].pid, 0);
}

#[test]
fn list_clients_format_is_expanded_for_each_client() {
    let app = app_with_two_clients();
    let out = format_list_clients(&app, "#{client_name} #{client_pid} #{client_readonly}", None);
    assert_eq!(out, "/dev/pts/16692 16692 0\n/dev/pts/13276 13276 1\n");
}

#[test]
fn client_termfeatures_include_hyperlinks() {
    let app = app_with_two_clients();
    assert_eq!(expand_format("#{client_termfeatures}", &app), "256,RGB,hyperlinks,title");
    assert_eq!(
        format_list_clients(&app, "#{client_termfeatures}", None),
        "256,RGB,hyperlinks,title\n256,RGB,hyperlinks,title\n",
    );
}

#[test]
fn list_clients_filter_keeps_matching_rows() {
    let app = app_with_two_clients();
    let out = format_list_clients(&app, "#{client_pid}", Some("#{client_readonly}"));
    assert_eq!(out, "13276\n");
    let none = format_list_clients(&app, "#{client_pid}", Some("0"));
    assert_eq!(none, "");
}

#[test]
fn list_clients_default_is_the_tmux_template() {
    let mut app = app_with_two_clients();
    app.status_visible = true;
    let out = format_list_clients(&app, default_list_clients_format(), None);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2);
    let term = expand_format("#{client_termname}", &app);
    assert_eq!(lines[0], format!("/dev/pts/16692: work [120x30 {}] (attached,UTF-8)", term));
    assert_eq!(lines[1], format!("/dev/pts/13276: work [120x30 {}] (attached,read-only,UTF-8)", term));
}

#[test]
fn list_clients_lists_nothing_without_clients() {
    let app = AppState::new("lonely".to_string());
    assert_eq!(format_list_clients(&app, default_list_clients_format(), None), "");
}

#[test]
fn client_flags_follow_tmux_order() {
    let mut ci = client(1, 100, true, true);
    ci.focused = true;
    assert_eq!(client_flags_string(&ci), "attached,focused,control-mode,read-only,UTF-8");
    let plain = client(2, 101, false, false);
    assert_eq!(client_flags_string(&plain), "attached,UTF-8");
}

#[test]
fn control_mode_and_session_variables() {
    let mut app = app_with_two_clients();
    app.client_registry.insert(8, client(8, 777, false, true));
    let out = format_list_clients(&app, "#{client_pid}:#{client_control_mode}:#{client_session}:#{client_utf8}", None);
    assert_eq!(out, "16692:0:work:1\n13276:0:work:1\n777:1:work:1\n");
}

#[test]
fn display_message_client_override_picks_the_named_client() {
    let app = app_with_two_clients();
    let ro = resolve_client_sel(&app, &ClientSel::Name("/dev/pts/13276".into()));
    assert_eq!(ro, Some(5));
    let text = with_format_client(ro, || expand_format("#{client_pid} #{client_readonly} #{client_name}", &app));
    assert_eq!(text, "13276 1 /dev/pts/13276");
    // Outside the override the best client (the latest) answers again.
    assert_eq!(expand_format("#{client_pid}", &app), "16692");
}

#[test]
fn client_sel_by_id_and_unknown_names() {
    let app = app_with_two_clients();
    assert_eq!(resolve_client_sel(&app, &ClientSel::Id(3)), Some(3));
    assert_eq!(resolve_client_sel(&app, &ClientSel::Id(99)), None);
    assert_eq!(resolve_client_sel(&app, &ClientSel::Name("bogus".into())), None);
    // tmux's CMD_CLIENT_CANFAIL: an unknown -c leaves the best client in charge.
    assert_eq!(with_format_client(None, || expand_format("#{client_pid}", &app)), "16692");
}

#[test]
fn client_names_resolve_like_cmd_find_client() {
    assert!(crate::cli::client_spec_matches("/dev/pts/12", "/dev/pts/12"));
    assert!(crate::cli::client_spec_matches("/dev/pts/12", "pts/12"));
    assert!(crate::cli::client_spec_matches("/dev/pts/12", "/dev/pts/12:"));
    assert!(!crate::cli::client_spec_matches("/dev/pts/12", "/dev/pts/1"));
    assert!(!crate::cli::client_spec_matches("/dev/pts/12", "12"));
    assert!(!crate::cli::client_spec_matches("/dev/pts/12", ""));
    assert!(!crate::cli::client_spec_matches("/dev/pts/12", ":"));
}

#[test]
fn a_reconnecting_client_resolves_to_its_newest_entry() {
    let mut app = AppState::new("work".to_string());
    app.client_registry.insert(4, client(4, 500, false, false));
    app.client_registry.insert(9, client(9, 500, false, false));
    assert_eq!(find_client_by_name(&app, "/dev/pts/500"), Some(9));
}

#[test]
fn no_client_means_empty_client_identity() {
    let app = AppState::new("work".to_string());
    assert_eq!(expand_format("#{client_name}|#{client_pid}|#{client_readonly}|#{client_flags}", &app), "|||");
}

#[test]
fn client_times_are_epoch_seconds() {
    let app = app_with_two_clients();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let created: i64 = expand_format("#{client_created}", &app).parse().unwrap();
    let activity: i64 = expand_format("#{client_activity}", &app).parse().unwrap();
    assert!((now - created).abs() <= 2, "created {created} now {now}");
    assert!((now - activity).abs() <= 2, "activity {activity} now {now}");
}

#[test]
fn attach_dash_r_asks_for_read_only() {
    let attach_wants_readonly = |args: &[&str]| crate::cli::attach_client_flags(args.iter().copied()).read_only;
    assert!(attach_wants_readonly(&["-r", "-t", "work"]));
    assert!(attach_wants_readonly(&["-t", "work", "-r"]));
    assert!(attach_wants_readonly(&["-dr"]));
    assert!(attach_wants_readonly(&["-f", "read-only"]));
    assert!(attach_wants_readonly(&["-f", "ignore-size,read-only"]));
    assert!(attach_wants_readonly(&["-fread-only"]));
    assert!(!attach_wants_readonly(&["-t", "work"]));
    assert!(!attach_wants_readonly(&["-t", "-r"]), "a session called -r is a value, not the flag");
    assert!(!attach_wants_readonly(&["-c", "r"]));
    assert!(!attach_wants_readonly(&["-r", "-f", "!read-only"]));
    assert!(!attach_wants_readonly(&["--", "-r"]));
}

#[test]
fn read_only_clients_keep_their_protocol_and_lose_their_input() {
    use crate::server::connection::readonly_client_may_run as may;
    for allowed in ["dump-state", "client-size", "client-attach", "client-flags", "detach-client",
                    "switch-client", "list-clients", "copy-mode", "copy-move", "focus-in"] {
        assert!(may(allowed), "{allowed} must stay allowed");
    }
    for refused in ["send-key", "send-key-raw", "send-text", "send-paste", "paste-buffer-at",
                    "mouse-down", "pane-mouse", "scroll-up", "split-window", "kill-pane",
                    "new-window", "select-pane", "resize-pane", "display-message", "send-keys",
                    "popup-input", "menu-select", "kill-server", "run-shell"] {
        assert!(!may(refused), "{refused} must be refused");
    }
}

#[test]
fn client_last_session_shows_the_session_name_under_l() {
    // The registry keeps the base `ns__alpha`, which switch-client -l resolves;
    // the format shows the name, as #{client_session} and tmux's s->name do.
    let mut app = app_with_two_clients();
    app.socket_name = Some("ns".to_string());
    app.client_registry.get_mut(&3).unwrap().last_session = Some("ns__alpha".to_string());
    assert_eq!(expand_format("#{client_last_session}", &app), "alpha");
    // The default namespace has nothing to strip.
    let mut app = app_with_two_clients();
    app.client_registry.get_mut(&3).unwrap().last_session = Some("alpha".to_string());
    assert_eq!(expand_format("#{client_last_session}", &app), "alpha");
}
