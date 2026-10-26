use super::*;
use std::io::{BufRead, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::Duration;

const TEST_KEY: &str = "pane-border-indicator-test-key";

fn assert_indicator_request(request: CtrlReq, expected: &str) {
    match request {
        CtrlReq::SetOptionQuiet(name, value, false) => {
            assert_eq!(name, "pane-border-indicators");
            assert_eq!(value, expected);
        }
        _ => panic!("expected a global pane-border-indicators assignment"),
    }
}

fn simple_tcp_command(command: &str) -> (String, mpsc::Receiver<CtrlReq>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let handler = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(
            stream,
            request_tx,
            TEST_KEY,
            Default::default(),
        );
    });

    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(client, "AUTH {TEST_KEY}\n").unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    assert_eq!(response, "OK\n");

    write!(client, "{command}\n").unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    response.clear();
    reader.read_to_string(&mut response).unwrap();
    handler.join().unwrap();
    (response, request_rx)
}

fn rejected_control_command(command: &str, args: &[&str]) -> String {
    let (request_tx, request_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel();
    assert!(dispatch_control_command(
        command,
        args,
        &crate::server::connection::TargetedSender::new(&request_tx, None),
        response_tx,
        None,
        false,
        None,
        0, &Default::default(), false, &Default::default(),
    ));
    let response = response_rx.recv().unwrap();
    assert!(response.starts_with("\u{0001}ERR\u{0001}"));
    assert!(matches!(
        request_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty),
    ));
    response
}

#[test]
fn control_mode_rejects_the_complete_indicator_value_without_dispatch() {
    let response = rejected_control_command(
        "set-option",
        &[
            "-gt",
            ":1",
            "pane-border-indicators",
            "arrows",
            "-junk",
        ],
    );
    assert!(response.contains("arrows -junk"));
}

#[test]
fn quiet_control_mode_preserves_indicator_validation_error() {
    let response = rejected_control_command(
        "set-option",
        &["-gq", "pane-border-indicators", "sideways"],
    );
    assert!(response.contains("sideways"));
}

#[test]
fn targeted_indicator_missing_operand_is_rejected_without_dispatch() {
    let response = rejected_control_command(
        "set-option",
        &["-gt", ":1", "pane-border-indicators"],
    );
    assert!(response.contains("empty value"));
}

#[test]
fn window_scoped_indicator_is_rejected_without_dispatch() {
    let response = rejected_control_command(
        "set-window-option",
        &["pane-border-indicators", "arrows"],
    );
    assert!(response.contains("does not support local window overrides"));
}

#[test]
fn control_mode_rejects_append_for_indicator_choices() {
    let response = rejected_control_command(
        "set-option",
        &["-gat", ":1", "pane-border-indicators", "arrows"],
    );
    assert!(response.contains("does not support append"));
}

#[test]
fn direct_validation_accepts_global_window_forms() {
    for (command, args) in [
        ("set-window-option", vec!["-g", "pane-border-indicators", "off"]),
        ("setw", vec!["-g", "pane-border-indicators", "colour"]),
        ("setw", vec!["-s", "pane-border-indicators", "arrows"]),
        ("set-option", vec!["-gw", "pane-border-indicators", "arrows"]),
        ("set-option", vec!["-sw", "pane-border-indicators", "both"]),
        ("set", vec!["-wg", "pane-border-indicators", "both"]),
    ] {
        let parsed = parse_set_option_args(&args);
        let window_command = matches!(command, "set-window-option" | "setw");
        assert!(
            parsed.validate(window_command).is_ok(),
            "{command} {args:?} should be a global window assignment",
        );
    }
}

#[test]
fn direct_validation_rejects_local_window_forms() {
    for (command, args) in [
        ("set-window-option", vec!["pane-border-indicators", "arrows"]),
        ("setw", vec!["-t", ":1", "pane-border-indicators", "arrows"]),
        ("set-option", vec!["-w", "pane-border-indicators", "arrows"]),
        ("set", vec!["-wt", ":1", "pane-border-indicators", "arrows"]),
    ] {
        let parsed = parse_set_option_args(&args);
        let window_command = matches!(command, "set-window-option" | "setw");
        let error = parsed.validate(window_command).unwrap_err();
        assert!(
            error.contains("local window overrides"),
            "{command} {args:?} should reject a local window assignment: {error}",
        );
    }
}

#[test]
fn config_accepts_global_window_forms() {
    for line in [
        "set-window-option -g pane-border-indicators off",
        "setw -g pane-border-indicators colour",
        "setw -s pane-border-indicators arrows",
        "set-option -gw pane-border-indicators arrows",
        "set-option -sw pane-border-indicators both",
        "set -wg pane-border-indicators both",
    ] {
        let mut app = crate::types::AppState::new("indicator-config".to_string());
        crate::config::parse_config_content(&mut app, line);
        assert!(
            app.config_warnings.is_empty(),
            "{line} should be accepted: {:?}",
            app.config_warnings,
        );
        assert!(app.user_options.contains_key("pane-border-indicators"));
    }
}

#[test]
fn config_rejects_local_window_forms() {
    for line in [
        "set-window-option pane-border-indicators arrows",
        "setw -t :1 pane-border-indicators arrows",
        "set-option -w pane-border-indicators arrows",
        "set -wt :1 pane-border-indicators arrows",
    ] {
        let mut app = crate::types::AppState::new("indicator-config".to_string());
        crate::config::parse_config_content(&mut app, line);
        assert!(
            app.config_warnings
                .iter()
                .any(|warning| warning.contains("local window overrides")),
            "{line} should reject a local window assignment: {:?}",
            app.config_warnings,
        );
        assert!(!app.user_options.contains_key("pane-border-indicators"));
    }
}

#[test]
fn config_set_unset_and_only_if_unset_preserve_indicator_state() {
    let mut app = crate::types::AppState::new("indicator-config".to_string());
    crate::config::parse_config_content(
        &mut app,
        "set -g pane-border-indicators arrows\n",
    );
    assert_eq!(
        app.user_options.get("pane-border-indicators").unwrap(),
        "arrows",
    );

    crate::config::parse_config_content(
        &mut app,
        "set -gu pane-border-indicators\n",
    );
    assert!(!app.user_options.contains_key("pane-border-indicators"));
    assert!(!app.user_set_options.contains("pane-border-indicators"));

    crate::config::parse_config_content(
        &mut app,
        "set -go pane-border-indicators both\n",
    );
    assert_eq!(
        app.user_options.get("pane-border-indicators").unwrap(),
        "both",
    );
}

#[test]
fn config_rejects_invalid_and_append_indicator_assignments() {
    let mut app = crate::types::AppState::new("indicator-config".to_string());
    crate::config::parse_config_content(
        &mut app,
        "set -g pane-border-indicators arrows\n\
         set -go pane-border-indicators sideways\n\
         set -goa pane-border-indicators both\n",
    );
    assert_eq!(
        app.user_options.get("pane-border-indicators").unwrap(),
        "arrows",
    );
    assert!(
        app.config_warnings
            .iter()
            .any(|warning| warning.contains("sideways")),
    );
    assert!(
        app.config_warnings
            .iter()
            .any(|warning| warning.contains("does not support append")),
    );
}

#[test]
fn invalid_indicator_assignment_does_not_mutate_state() {
    let mut app = crate::types::AppState::new("indicator-config".to_string());
    assert!(crate::server::options::apply_set_option(
        &mut app,
        "pane-border-indicators",
        "sideways",
        true,
    )
    .is_err());
    assert!(!app.user_options.contains_key("pane-border-indicators"));
}

#[test]
fn simple_tcp_accepts_global_window_forms() {
    for command in [
        "set-window-option -g pane-border-indicators off",
        "setw -g pane-border-indicators colour",
        "setw -s pane-border-indicators arrows",
        "set-option -gw pane-border-indicators arrows",
        "set-option -sw pane-border-indicators both",
        "set -wg pane-border-indicators both",
    ] {
        let expected = command.split_whitespace().last().unwrap();
        let (response, requests) = simple_tcp_command(command);
        assert_eq!(response, "", "{command} should not return an error");
        let request = requests
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("{command} was not dispatched: {error}"));
        assert_indicator_request(request, expected);
    }
}

#[test]
fn simple_tcp_rejects_local_window_forms_before_dispatch() {
    for command in [
        "set-window-option pane-border-indicators arrows",
        "setw -t :1 pane-border-indicators arrows",
        "set-option -w pane-border-indicators arrows",
        "set -wt :1 pane-border-indicators arrows",
    ] {
        let (response, requests) = simple_tcp_command(command);
        assert!(
            response.contains("local window overrides"),
            "{command} should return the local override error: {response:?}",
        );
        assert!(
            matches!(
                requests.try_recv(),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected),
            ),
            "{command} must be rejected before dispatch",
        );
    }
}

#[test]
fn persistent_control_accepts_global_window_forms() {
    for (command, args, expected) in [
        (
            "set-window-option",
            vec!["-g", "pane-border-indicators", "off"],
            "off",
        ),
        (
            "setw",
            vec!["-g", "pane-border-indicators", "colour"],
            "colour",
        ),
        (
            "setw",
            vec!["-s", "pane-border-indicators", "arrows"],
            "arrows",
        ),
        (
            "set-option",
            vec!["-gw", "pane-border-indicators", "arrows"],
            "arrows",
        ),
        (
            "set-option",
            vec!["-sw", "pane-border-indicators", "both"],
            "both",
        ),
        (
            "set",
            vec!["-wg", "pane-border-indicators", "both"],
            "both",
        ),
    ] {
        let (request_tx, request_rx) = mpsc::channel();
        let (response_tx, response_rx) = mpsc::channel();
        assert!(dispatch_control_command(
            command,
            &args,
            &crate::server::connection::TargetedSender::new(&request_tx, None),
            response_tx,
            None,
            false,
            None,
            0, &Default::default(), false, &Default::default(),
        ));
        assert_eq!(response_rx.recv().unwrap(), "");
        assert_indicator_request(request_rx.recv().unwrap(), expected);
    }
}

#[test]
fn persistent_control_rejects_local_window_forms_before_dispatch() {
    for (command, args) in [
        (
            "set-window-option",
            vec!["pane-border-indicators", "arrows"],
        ),
        (
            "setw",
            vec!["-t", ":1", "pane-border-indicators", "arrows"],
        ),
        (
            "set-option",
            vec!["-w", "pane-border-indicators", "arrows"],
        ),
        (
            "set",
            vec!["-wt", ":1", "pane-border-indicators", "arrows"],
        ),
    ] {
        let response = rejected_control_command(command, &args);
        assert!(
            response.contains("local window overrides"),
            "{command} {args:?} should return the local override error: {response:?}",
        );
    }
}
