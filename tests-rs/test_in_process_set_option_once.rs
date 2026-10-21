// A set-option run on the server's own command path (a hook, a key binding,
// a menu, confirm-before) ran twice: execute_command_string applied it in
// place and then sent the same line to the server's own control port, where
// it was applied again. `set -ga @x a` from an after-new-window hook left
// "aa", and a bare `set -g mouse` toggled and toggled straight back.
//
// tmux runs each queued command once (cmd-set-option.c:347 appends once with
// options_set_string, :351-353 options_from_string). The arm now applies the
// write once, sends nothing to the port, and records the option name for the
// server loop's after-write work.
//
// The control port here is a real listener, so the old self send is caught as
// a connection rather than lost to a closed port.

use super::*;

fn server_app() -> (AppState, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut app = AppState::new("once".to_string());
    app.control_port = Some(listener.local_addr().unwrap().port());
    (app, listener)
}

fn nothing_sent(listener: &std::net::TcpListener) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(100));
    matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
}

#[test]
fn append_user_option_appends_once() {
    let (mut app, listener) = server_app();
    execute_command_string(&mut app, "set -ga @x a").unwrap();
    assert_eq!(app.user_options.get("@x").map(String::as_str), Some("a"));
    assert!(nothing_sent(&listener), "set-option was sent back to the server's own port");
    assert_eq!(app.in_process_option_writes, vec!["@x".to_string()]);
}

#[test]
fn append_status_right_appends_once() {
    let (mut app, listener) = server_app();
    app.status_right = String::new();
    execute_command_string(&mut app, "set -ga status-right Z").unwrap();
    execute_command_string(&mut app, "set -ga status-right Z").unwrap();
    assert_eq!(app.status_right, "ZZ");
    assert!(nothing_sent(&listener));
}

#[test]
fn bare_boolean_toggles_once() {
    let (mut app, listener) = server_app();
    app.mouse_enabled = true;
    execute_command_string(&mut app, "set -g mouse").unwrap();
    assert!(!app.mouse_enabled, "set -g mouse should toggle on to off");
    assert!(nothing_sent(&listener));
}

#[test]
fn chained_set_is_visible_to_the_next_command() {
    let (mut app, _listener) = server_app();
    execute_command_string(&mut app, "set -g @k v \\; set -gF @k2 \"#{@k}\"").unwrap();
    assert_eq!(app.user_options.get("@k2").map(String::as_str), Some("v"));
    assert_eq!(
        app.in_process_option_writes,
        vec!["@k".to_string(), "@k2".to_string()]
    );
}

#[test]
fn without_a_server_nothing_is_recorded() {
    let mut app = AppState::new("noserver".to_string());
    execute_command_string(&mut app, "set -ga @x a").unwrap();
    assert_eq!(app.user_options.get("@x").map(String::as_str), Some("a"));
    assert!(app.in_process_option_writes.is_empty());
}
