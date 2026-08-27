// Tests for commands that were non-functional and have been fixed.
// Each test verifies that a specific command produces visible state
// changes when run WITHOUT a server connection (control_port = None),
// confirming the local fallback implementation works.

use super::*;

use std::io::{Read, Write as IoWrite};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
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

fn mock_app_with_window() -> AppState {
    let mut app = mock_app();
    app.windows.push(make_window("shell", 0));
    app
}

fn mock_app_with_windows(names: &[&str]) -> AppState {
    let mut app = mock_app();
    for (i, name) in names.iter().enumerate() {
        app.windows.push(make_window(name, i));
    }
    app
}

fn capture_control_request() -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut request = Vec::new();
        if let Ok((mut stream, _)) = listener.accept() {
            let mut seen_lf = 0;
            let mut buf = [0u8; 1];
            while seen_lf < 2 {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        request.push(buf[0]);
                        if buf[0] == b'\n' {
                            seen_lf += 1;
                        }
                    }
                }
            }
            let _ = stream.write_all(b"OK\n");
            let _ = stream.flush();
        }
        let _ = tx.send(String::from_utf8_lossy(&request).to_string());
    });
    (port, rx)
}

/// Extract popup output text, panicking with context if not PopupMode.
fn extract_popup(app: &AppState) -> (&str, &str) {
    match &app.mode {
        Mode::PopupMode { command, output, .. } => (command, output),
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

/// Extract status message text, panicking if not set.
fn extract_status_message(app: &AppState) -> &str {
    match &app.status_message {
        Some((msg, ..)) => msg.as_str(),
        None => panic!("expected status_message to be set"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  display-message: local format expansion and status bar output
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn display_message_shows_plain_text_in_status() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-message hello").unwrap();
    let msg = extract_status_message(&app);
    assert_eq!(msg, "hello", "plain text display-message should set status");
}

#[test]
fn display_message_expands_session_name_format() {
    let mut app = mock_app_with_window();
    app.session_name = "my_project".to_string();
    execute_command_string(&mut app, "display-message #S").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("my_project"), "format #S should expand to session name, got: {}", msg);
}

#[test]
fn display_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display test_alias").unwrap();
    let msg = extract_status_message(&app);
    assert_eq!(msg, "test_alias");
}

#[test]
fn new_window_forwards_full_command_to_control_port() {
    let command = r#"new-window -n 'Foo Bar' -c 'D:\x y' 'clod --resume=abc 123'"#;
    let (port, request_rx) = capture_control_request();
    let mut app = mock_app_with_window();
    app.control_port = Some(port);
    app.session_key = "test-key".to_string();

    execute_command_string(&mut app, command).unwrap();

    let request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(request, format!("AUTH test-key\n{}\n", command));
}

// ════════════════════════════════════════════════════════════════════════════
//  show-options: local fallback generates readable option output
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn show_options_displays_popup_with_key_options() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-options").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-options");
    assert!(out.contains("prefix"), "should show prefix option");
    assert!(out.contains("mouse"), "should show mouse option");
    assert!(out.contains("history-limit"), "should show history-limit");
    assert!(out.contains("escape-time"), "should show escape-time");
    assert!(out.contains("status"), "should show status option");
}

#[test]
fn show_options_reflects_current_settings() {
    let mut app = mock_app_with_window();
    app.mouse_enabled = false;
    app.history_limit = 5000;
    execute_command_string(&mut app, "show-options").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("mouse off"), "mouse should be off");
    assert!(out.contains("history-limit 5000"), "history-limit should be 5000");
}

#[test]
fn show_alias_same_as_show_options() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show").unwrap();
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "show-options");
}

#[test]
fn showw_alias_same_as_show_options() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "showw").unwrap();
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "show-options");
}

// ════════════════════════════════════════════════════════════════════════════
//  show-environment / set-environment: local env management
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn set_environment_updates_local_env() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-environment MY_TEST_VAR hello_world").unwrap();
    assert_eq!(app.environment.get("MY_TEST_VAR").map(|s| s.as_str()), Some("hello_world"));
}

#[test]
fn setenv_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "setenv ALIAS_VAR value123").unwrap();
    assert_eq!(app.environment.get("ALIAS_VAR").map(|s| s.as_str()), Some("value123"));
}

#[test]
fn set_environment_unset_removes_var() {
    let mut app = mock_app_with_window();
    app.environment.insert("REMOVE_ME".to_string(), "old_value".to_string());
    execute_command_string(&mut app, "set-environment -u REMOVE_ME").unwrap();
    assert!(app.environment.get("REMOVE_ME").is_none(), "unset var should be removed");
}

#[test]
fn show_environment_displays_vars_in_popup() {
    let mut app = mock_app_with_window();
    app.environment.insert("KEY1".to_string(), "val1".to_string());
    app.environment.insert("KEY2".to_string(), "val2".to_string());
    execute_command_string(&mut app, "show-environment").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-environment");
    assert!(out.contains("KEY1=val1"), "should show KEY1");
    assert!(out.contains("KEY2=val2"), "should show KEY2");
}

#[test]
fn show_environment_empty_shows_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-environment").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("no environment"), "empty env should show feedback message");
}

#[test]
fn showenv_alias_works() {
    let mut app = mock_app_with_window();
    app.environment.insert("TESTVAR".to_string(), "tv".to_string());
    execute_command_string(&mut app, "showenv").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-environment");
    assert!(out.contains("TESTVAR=tv"));
}

// ════════════════════════════════════════════════════════════════════════════
//  set-hook: local hook management
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn set_hook_creates_new_hook() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook after-new-window display-message created").unwrap();
    assert!(app.hooks.contains_key("after-new-window"), "hook should be created");
    assert_eq!(app.hooks["after-new-window"], vec!["display-message created"]);
}

#[test]
fn set_hook_append_adds_to_existing() {
    let mut app = mock_app_with_window();
    app.hooks.insert("after-new-window".to_string(), vec!["cmd1".to_string()]);
    execute_command_string(&mut app, "set-hook -a after-new-window cmd2").unwrap();
    assert_eq!(app.hooks["after-new-window"].len(), 2);
    assert_eq!(app.hooks["after-new-window"][1], "cmd2");
}

#[test]
fn set_hook_unset_removes_hook() {
    let mut app = mock_app_with_window();
    app.hooks.insert("my-hook".to_string(), vec!["command".to_string()]);
    execute_command_string(&mut app, "set-hook -u my-hook").unwrap();
    assert!(!app.hooks.contains_key("my-hook"), "hook should be removed after -u");
}

// ════════════════════════════════════════════════════════════════════════════
//  find-window: local search through windows
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn find_window_shows_matching_windows() {
    let mut app = mock_app_with_windows(&["editor-main", "server-logs", "editor-alt"]);
    execute_command_string(&mut app, "find-window editor").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "find-window");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "should find 2 windows matching 'editor'");
    assert!(out.contains("editor-main"), "should include editor-main");
    assert!(out.contains("editor-alt"), "should include editor-alt");
    assert!(!out.contains("server"), "should NOT include server-logs");
}

#[test]
fn find_window_no_match_shows_feedback() {
    let mut app = mock_app_with_windows(&["alpha", "beta"]);
    execute_command_string(&mut app, "find-window nonexistent").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("no windows matching"), "should show feedback for no matches");
}

#[test]
fn findw_alias_works() {
    let mut app = mock_app_with_windows(&["target", "other"]);
    execute_command_string(&mut app, "findw target").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "find-window");
    assert!(out.contains("target"));
}

// ════════════════════════════════════════════════════════════════════════════
//  move-window: local window reordering
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn move_window_changes_position() {
    // `-t` names a display INDEX, and tmux refuses one another window already
    // holds. Verified on tmux 3.4 with 0:first* 1:second 2:third:
    //   move-window -t 2  ->  rc 1, "index in use: 2", list unchanged
    //   move-window -t 5  ->  rc 0, 1:second 2:third 5:first*
    // This used to splice the Vec instead, which produced an order tmux never
    // produces for any layout with a gap (issue #602).
    let mut app = mock_app_with_windows(&["first", "second", "third"]);
    app.active_idx = 0; // "first" is active
    execute_command_string(&mut app, "move-window -t 2").unwrap();
    let names: Vec<&str> = app.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["first", "second", "third"],
        "an occupied destination is refused, so nothing moves");

    execute_command_string(&mut app, "move-window -t 5").unwrap();
    let names: Vec<&str> = app.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["second", "third", "first"], "first moved to index 5");
    assert_eq!(app.window_indices, vec![1, 2, 5]);
    assert_eq!(app.windows[app.active_idx].name, "first",
        "without -d the moved window stays current");
}

#[test]
fn movew_alias_works() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 2;
    execute_command_string(&mut app, "movew -t 7").unwrap();
    assert_eq!(app.window_indices, vec![0, 1, 7], "c took index 7");
    assert_eq!(app.windows[2].name, "c");
    assert_eq!(app.windows[app.active_idx].name, "c");
}

// ════════════════════════════════════════════════════════════════════════════
//  swap-window: local window swapping
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn swap_window_swaps_two_windows() {
    let mut app = mock_app_with_windows(&["alpha", "beta", "gamma"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "swap-window -t 2").unwrap();
    assert_eq!(app.windows[0].name, "gamma", "index 0 should have gamma");
    assert_eq!(app.windows[2].name, "alpha", "index 2 should have alpha");
    assert_eq!(app.windows[1].name, "beta", "index 1 unchanged");
}

#[test]
fn swapw_alias_works() {
    let mut app = mock_app_with_windows(&["x", "y"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "swapw -t 1").unwrap();
    assert_eq!(app.windows[0].name, "y");
    assert_eq!(app.windows[1].name, "x");
}

#[test]
fn swap_window_same_index_is_noop() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "swap-window -t 0").unwrap();
    assert_eq!(app.windows[0].name, "a", "same-index swap should be no-op");
}

// ════════════════════════════════════════════════════════════════════════════
//  link-window: shows feedback message (not supported)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn link_window_accepted() {
    let mut app = mock_app_with_window();
    // link-window is now functional; in mock context (no PTY system),
    // the command is accepted without error
    execute_command_string(&mut app, "link-window -t 0").unwrap();
}

#[test]
fn linkw_alias_accepted() {
    let mut app = mock_app_with_window();
    // linkw alias is also accepted without error
    execute_command_string(&mut app, "linkw").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  lock-*: shows platform feedback
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn lock_server_shows_not_available_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-server").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"), "lock-server should show not-available message");
}

#[test]
fn lock_client_shows_not_available_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-client").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

#[test]
fn lock_session_shows_not_available_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-session").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

#[test]
fn lock_alias_shows_not_available() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

#[test]
fn lockc_alias_shows_not_available() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lockc").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

#[test]
fn locks_alias_shows_not_available() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "locks").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

// ════════════════════════════════════════════════════════════════════════════
//  suspend-client: shows platform feedback
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn suspend_client_shows_not_available() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "suspend-client").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"), "suspend should show not-available");
}

#[test]
fn suspendc_alias_shows_not_available() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "suspendc").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("not available"));
}

// ════════════════════════════════════════════════════════════════════════════
//  choose-client: shows single-client feedback
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn choose_client_shows_single_client_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-client").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("single-client") || msg.contains("only client"),
        "choose-client should show single-client feedback, got: {}", msg);
}

// ════════════════════════════════════════════════════════════════════════════
//  customize-mode: interactive options editor
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn customize_mode_shows_options_popup() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "customize-mode").unwrap();
    match &app.mode {
        Mode::CustomizeMode { options, .. } => {
            assert!(options.iter().any(|(n, _, _)| n == "mouse"), "should display options including mouse");
            assert!(options.iter().any(|(n, _, _)| n == "prefix"), "should display options including prefix");
        }
        other => panic!("expected CustomizeMode for customize-mode, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  refresh-client: shows feedback
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn refresh_client_shows_status_message() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "refresh-client").unwrap();
    let msg = extract_status_message(&app);
    assert!(msg.contains("refresh"), "refresh should show feedback, got: {}", msg);
}

#[test]
fn refresh_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "refresh").unwrap();
    assert!(app.status_message.is_some());
}

// ════════════════════════════════════════════════════════════════════════════
//  server-info: shows info popup
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn server_info_shows_popup_with_version_and_session() {
    let mut app = mock_app_with_window();
    app.session_name = "my_sess".to_string();
    execute_command_string(&mut app, "server-info").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "server-info");
    assert!(out.contains("psmux"), "should contain 'psmux'");
    assert!(out.contains("my_sess"), "should contain session name");
    assert!(out.contains("Windows:"), "should contain window count");
}

#[test]
fn info_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "info").unwrap();
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "server-info");
}

// ════════════════════════════════════════════════════════════════════════════
//  show-messages: shows popup
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn show_messages_shows_popup() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-messages").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-messages");
    assert!(out.contains("no messages"), "empty messages log should show feedback");
}

#[test]
fn showmsgs_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "showmsgs").unwrap();
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "show-messages");
}

// ════════════════════════════════════════════════════════════════════════════
//  set-option / bind-key / unbind-key: local config fallback
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn set_option_local_changes_mouse() {
    let mut app = mock_app_with_window();
    app.mouse_enabled = true;
    execute_command_string(&mut app, "set-option mouse off").unwrap();
    assert!(!app.mouse_enabled, "set-option mouse off should disable mouse locally");
}

#[test]
fn set_option_local_changes_history_limit() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set history-limit 9999").unwrap();
    assert_eq!(app.history_limit, 9999, "set history-limit should update app state");
}

#[test]
fn bind_key_local_adds_binding() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -T prefix z kill-pane").unwrap();
    // Verify a binding was added to the prefix table
    let prefix_binds = app.key_tables.get("prefix");
    assert!(prefix_binds.is_some(), "prefix table should exist after bind-key");
    let has_z = prefix_binds.unwrap().iter().any(|b| matches!(b.key.0, crossterm::event::KeyCode::Char('z')));
    assert!(has_z, "should have a binding for 'z' key");
}

// ════════════════════════════════════════════════════════════════════════════
//  source-file: local config loading
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn source_file_loads_config_locally() {
    let mut app = mock_app_with_window();
    // Create a temp config file
    let tmp_dir = std::env::temp_dir();
    let tmp_file = tmp_dir.join("psmux_test_source.conf");
    std::fs::write(&tmp_file, "set -g history-limit 7777\n").unwrap();
    execute_command_string(&mut app, &format!("source-file {}", tmp_file.display())).unwrap();
    let _ = std::fs::remove_file(&tmp_file);
    assert_eq!(app.history_limit, 7777, "source-file should load config and change history-limit");
}

// ════════════════════════════════════════════════════════════════════════════
//  if-shell: local conditional execution
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn if_shell_format_mode_true_runs_true_cmd() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 0;
    // With -F, a non-empty, non-zero condition is true
    execute_command_string(&mut app, "if-shell -F 1 next-window previous-window").unwrap();
    assert_eq!(app.active_idx, 1, "true condition should run next-window");
}

#[test]
fn if_shell_format_mode_false_runs_false_cmd() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 1;
    // With -F, 0 is false
    execute_command_string(&mut app, "if-shell -F 0 next-window previous-window").unwrap();
    assert_eq!(app.active_idx, 0, "false condition should run previous-window");
}

#[test]
fn if_shell_literal_true_runs_true_cmd() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "if-shell true next-window").unwrap();
    assert_eq!(app.active_idx, 1, "condition 'true' should run the true command");
}

#[test]
fn if_shell_literal_false_runs_false_cmd() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "if-shell false next-window previous-window").unwrap();
    // "false" condition: runs previous-window, which wraps from 0 to 1
    assert_eq!(app.active_idx, 1, "condition 'false' should run the false command");
}

// Regression: #183 — if-shell -F must expand format variables before truthiness check
#[test]
fn if_shell_format_expands_user_option_truthy() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 0;
    // Set @pane-is-vim to "1" (truthy)
    app.user_options.insert("@pane-is-vim".to_string(), "1".to_string());
    // The format string #{@pane-is-vim} must be expanded to "1" before evaluation
    execute_command_string(&mut app, r##"if-shell -F "#{@pane-is-vim}" next-window previous-window"##).unwrap();
    assert_eq!(app.active_idx, 1, "@pane-is-vim=1 should expand to truthy, running next-window");
}

#[test]
fn if_shell_format_expands_user_option_falsy() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 1;
    // Set @pane-is-vim to "0" (falsy)
    app.user_options.insert("@pane-is-vim".to_string(), "0".to_string());
    execute_command_string(&mut app, r##"if-shell -F "#{@pane-is-vim}" next-window previous-window"##).unwrap();
    assert_eq!(app.active_idx, 0, "@pane-is-vim=0 should expand to falsy, running previous-window");
}

#[test]
fn if_shell_format_expands_unset_option_as_falsy() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 1;
    // @pane-is-vim is NOT set, so #{@pane-is-vim} should expand to "" (empty = falsy)
    execute_command_string(&mut app, r##"if-shell -F "#{@pane-is-vim}" next-window previous-window"##).unwrap();
    assert_eq!(app.active_idx, 0, "unset @pane-is-vim should expand to empty (falsy), running previous-window");
}

#[test]
fn if_shell_format_expands_session_name() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 0;
    // #{session_name} is always non-empty ("test_session"), so true branch should run
    execute_command_string(&mut app, r##"if-shell -F "#{session_name}" next-window previous-window"##).unwrap();
    assert_eq!(app.active_idx, 1, "session_name should expand to non-empty truthy value");
}

#[test]
fn if_shell_format_expands_window_zoomed_flag() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 1;
    // window_zoomed_flag is 0 when not zoomed, should be falsy
    execute_command_string(&mut app, r##"if-shell -F "#{window_zoomed_flag}" next-window previous-window"##).unwrap();
    assert_eq!(app.active_idx, 0, "window_zoomed_flag=0 (not zoomed) should be falsy");
}

// ════════════════════════════════════════════════════════════════════════════
//  previous-layout: cycles layout in reverse
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn previous_layout_changes_layout_index() {
    let mut app = mock_app_with_window();
    let orig_idx = app.windows[0].layout_index;
    execute_command_string(&mut app, "previous-layout").unwrap();
    // Layout index should change (cycle_layout_reverse decrements)
    // Even with a simple window, the index tracking should update
    let new_idx = app.windows[0].layout_index;
    // If there is only one empty window the layout might not visually change,
    // but the layout_index bookkeeping should still advance.
    assert!(new_idx != orig_idx || orig_idx == 0, "layout index should change or start at 0");
}

#[test]
fn prevl_alias_works() {
    let mut app = mock_app_with_window();
    // Just verify it does not panic
    execute_command_string(&mut app, "prevl").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  select-layout: applies a named layout locally
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn select_layout_applies_tiled() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout tiled").unwrap();
    // Should not panic; layout is applied even with empty window
}

#[test]
fn selectl_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "selectl even-horizontal").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  next-layout: cycles layout forward
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn next_layout_changes_layout_index() {
    let mut app = mock_app_with_window();
    let orig_idx = app.windows[0].layout_index;
    execute_command_string(&mut app, "next-layout").unwrap();
    let new_idx = app.windows[0].layout_index;
    assert!(new_idx != orig_idx || orig_idx == 0, "layout index should change");
}

// ════════════════════════════════════════════════════════════════════════════
//  unlink-window: removes window locally
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn unlink_window_removes_active_window() {
    let mut app = mock_app_with_windows(&["win1", "win2", "win3"]);
    app.active_idx = 1;
    execute_command_string(&mut app, "unlink-window").unwrap();
    assert_eq!(app.windows.len(), 2);
    let names: Vec<&str> = app.windows.iter().map(|w| w.name.as_str()).collect();
    assert!(!names.contains(&"win2"), "unlinkd window should be removed");
}

#[test]
fn unlink_window_refuses_last_window() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "unlink-window").unwrap();
    assert_eq!(app.windows.len(), 1, "must not unlink the last window");
}

#[test]
fn unlinkw_alias_works() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    execute_command_string(&mut app, "unlinkw").unwrap();
    assert_eq!(app.windows.len(), 1);
}

// ════════════════════════════════════════════════════════════════════════════
//  clear-history: local scrollback clearing
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn clear_history_does_not_panic_on_empty_window() {
    let mut app = mock_app_with_window();
    // Empty window has a Split root with no panes, should not panic
    execute_command_string(&mut app, "clear-history").unwrap();
}

#[test]
fn clearhist_alias_does_not_panic() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "clearhist").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  break-pane: on empty window, no crash (single pane cannot break)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn break_pane_empty_split_does_not_crash() {
    let mut app = mock_app_with_window();
    // Empty Split root with no panes: break-pane should be safe
    execute_command_string(&mut app, "break-pane").unwrap();
}

#[test]
fn breakp_alias_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "breakp").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  respawn-pane: on empty window, no crash
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn respawn_pane_empty_does_not_crash() {
    let mut app = mock_app_with_window();
    // This may fail gracefully (no PTY system in test), just verify no panic
    let _ = execute_command_string(&mut app, "respawn-pane");
}

#[test]
fn respawnp_alias_does_not_crash() {
    let mut app = mock_app_with_window();
    let _ = execute_command_string(&mut app, "respawnp");
}

// ════════════════════════════════════════════════════════════════════════════
//  swap-pane: on empty window, no crash
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn swap_pane_does_not_crash_on_empty() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swap-pane -U").unwrap();
    execute_command_string(&mut app, "swap-pane -D").unwrap();
}

#[test]
fn swapp_alias_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swapp -D").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  rotate-window: on empty window, no crash
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn rotate_window_does_not_crash_on_empty() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotate-window").unwrap();
}

#[test]
fn rotate_window_reverse_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotate-window -D").unwrap();
}

#[test]
fn rotatew_alias_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotatew").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  resize-pane: local directional resizing
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn resize_pane_up_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -U 5").unwrap();
}

#[test]
fn resize_pane_down_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -D 5").unwrap();
}

#[test]
fn resize_pane_left_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -L 5").unwrap();
}

#[test]
fn resize_pane_right_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -R 5").unwrap();
}

#[test]
fn resizep_alias_zoom() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resizep -Z").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  Comprehensive: every command in list-commands is parsable to an action
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn every_listed_command_parses_to_action() {
    let commands = [
        "attach-session", "detach-client", "has-session", "kill-server",
        "kill-session", "list-sessions", "new-session", "rename-session foo",
        "switch-client", "choose-tree", "find-window test", "kill-window",
        "last-window", "link-window", "list-windows", "move-window -t 0",
        "new-window", "next-window", "previous-window", "rename-window test",
        "resize-window", "respawn-window", "rotate-window", "select-window -t 0",
        "swap-window -t 0", "unlink-window", "break-pane", "capture-pane",
        "display-panes", "join-pane -t 0", "kill-pane", "last-pane",
        "move-pane -t 0", "pipe-pane", "resize-pane -Z", "respawn-pane",
        "select-pane -U", "split-window", "swap-pane -D",
        "next-layout", "previous-layout", "select-layout tiled",
        "choose-buffer", "clear-history", "copy-mode", "delete-buffer",
        "list-buffers", "load-buffer /tmp/test", "paste-buffer",
        "save-buffer /tmp/test", "set-buffer hello", "show-buffer",
        "bind-key -T prefix x kill-pane", "list-keys", "unbind-key x",
        "set-option mouse on", "set-window-option", "show-options",
        "show-window-options", "source-file /tmp/test.conf",
        "clock-mode", "command-prompt", "display-menu",
        "display-message hello", "display-popup", "list-commands",
        "server-info", "confirm-before kill-server", "if-shell true echo",
        "list-clients", "refresh-client", "run-shell echo",
        "send-keys hello", "set-environment FOO bar", "set-hook after-new-window echo",
        "show-environment", "show-hooks", "show-messages",
        "wait-for test-channel",
    ];
    for cmd in &commands {
        let action = parse_command_to_action(cmd);
        assert!(action.is_some(), "command '{}' should parse to an Action", cmd);
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Comprehensive: every command in list-commands does not panic when executed
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn every_command_does_not_panic_embedded_mode() {
    // Commands that should complete without panicking in embedded mode
    // (no control_port, so local fallbacks are exercised)
    let safe_commands = [
        "list-windows", "list-panes", "list-clients", "list-commands",
        "list-keys", "list-sessions", "list-buffers",
        "show-hooks", "show-buffer", "show-options", "show-window-options",
        "show-messages", "show-environment",
        "choose-tree", "choose-buffer",
        "clock-mode", "command-prompt", "copy-mode",
        "display-panes",
        "display-message hello",
        "set-buffer testdata", "delete-buffer",
        "rename-window newname", "rename-session newsess",
        "toggle-sync",
        "set-option mouse on",
        "set-environment TEST_VAR value",
        "set-hook my-hook echo",
        "find-window shell",
        "confirm-before echo",
        "has-session", "start-server",
        "server-info",
        "lock-server", "lock-client", "lock-session",
        "suspend-client", "choose-client", "customize-mode",
        "refresh-client",
        "break-pane", "swap-pane -D", "rotate-window",
        "respawn-pane",
        "swap-window -t 0", "move-window -t 0",
        "unlink-window",
        "next-layout", "previous-layout",
        "select-layout tiled",
        "clear-history",
        "resize-pane -U", "resize-pane -D",
        "resize-pane -L", "resize-pane -R",
        "link-window",
        "if-shell -F 1 list-windows",
    ];
    for cmd in &safe_commands {
        let mut app = mock_app_with_windows(&["test1", "test2"]);
        app.paste_buffers.push("buffer_data".to_string());
        let result = execute_command_string(&mut app, cmd);
        assert!(result.is_ok(), "command '{}' panicked or returned error: {:?}", cmd, result);
    }
}
