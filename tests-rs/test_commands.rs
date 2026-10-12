// Original commands.rs inline tests, moved to separate file for #146.

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

#[test]
fn test_generate_list_clients() {
    let mut app = mock_app();
    let win = crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "shell".to_string(),
        id: 0,
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
    };
    app.windows.push(win);
    // One row per registered client, tmux's LIST_CLIENTS_TEMPLATE (#724); the
    // made up `/dev/pts/0` row is gone (discussion #776).
    app.register_client(1, false);
    let output = generate_list_clients(&app);
    assert!(output.contains("test_session"), "should contain session name");
    assert!(output.starts_with("/dev/pts/1:"), "row names the registered client: {output:?}");
    assert!(!output.contains("/dev/pts/0:"), "no made up client: {output:?}");
}

#[test]
fn test_generate_show_hooks_empty() {
    let app = mock_app();
    let output = generate_show_hooks(&app);
    assert_eq!(output, "(no hooks)\n");
}

#[test]
fn test_generate_show_hooks_with_hooks() {
    let mut app = mock_app();
    app.hooks.insert("after-new-window".to_string(), vec!["run-shell 'echo hello'".to_string()]);
    let output = generate_show_hooks(&app);
    assert!(output.contains("after-new-window"), "should contain hook name");
    assert!(output.contains("run-shell"), "should contain hook command");
}

#[test]
fn test_generate_list_commands() {
    let output = generate_list_commands();
    assert!(output.contains("list-windows"), "should list list-windows command");
    assert!(output.contains("show-hooks"), "should list show-hooks command");
    assert!(output.contains("list-commands"), "should list list-commands command");
    assert!(output.contains("list-clients"), "should list list-clients command");
}

#[test]
fn test_show_output_popup_sets_mode() {
    let mut app = mock_app();
    show_output_popup(&mut app, "test-cmd", "line1\nline2\nline3".to_string());
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "test-cmd");
            assert!(output.contains("line1"));
            assert!(output.contains("line3"));
        }
        _ => panic!("expected PopupMode"),
    }
}

fn mock_app_with_window() -> AppState {
    let mut app = mock_app();
    let win = crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "shell".to_string(),
        id: 0,
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
    };
    app.windows.push(win);

    app
}

// ── Issue #146: list commands from command prompt must set PopupMode ──

#[test]
fn test_list_windows_command_prompt_sets_popup() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "list-windows".to_string(), cursor: 12 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "list-windows");
            assert!(!output.is_empty(), "list-windows output must not be empty");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_list_panes_command_prompt_sets_popup() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "list-panes".to_string(), cursor: 10 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "list-panes");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_list_clients_command_prompt_sets_popup() {
    let mut app = mock_app_with_window();
    app.register_client(1, false);
    app.mode = Mode::CommandPrompt { input: "list-clients".to_string(), cursor: 12 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "list-clients");
            assert!(output.contains("test_session"), "should contain session name");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_list_commands_command_prompt_sets_popup() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "list-commands".to_string(), cursor: 13 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "list-commands");
            assert!(output.contains("list-windows"), "should list known commands");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_show_hooks_command_prompt_sets_popup() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "show-hooks".to_string(), cursor: 10 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "show-hooks");
            assert!(output.contains("no hooks"), "empty hooks should show (no hooks)");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_list_panes_alias_lsp_command_prompt() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "lsp".to_string(), cursor: 3 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "list-panes");
        }
        other => panic!("expected PopupMode for lsp alias, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_list_windows_alias_lsw_command_prompt() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "lsw".to_string(), cursor: 3 };
    execute_command_prompt(&mut app).unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "list-windows");
        }
        other => panic!("expected PopupMode for lsw alias, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn test_popup_dimensions_reasonable() {
    let mut app = mock_app_with_window();
    show_output_popup(&mut app, "test", "a\nb\nc\nd\ne\nf\ng\nh\ni\nj".to_string());
    match &app.mode {
        Mode::PopupMode { width, height, .. } => {
            assert!(*width >= 20, "popup width must be at least 20");
            assert!(*width <= 120, "popup width must be at most 120");
            assert!(*height >= 5, "popup height must be at least 5");
            assert!(*height <= 40, "popup height must be at most 40");
        }
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn test_command_prompt_unknown_cmd_stays_passthrough() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt { input: "some-unknown-cmd".to_string(), cursor: 16 };
    execute_command_prompt(&mut app).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough), "unknown command should leave mode as Passthrough");
}
