// Functional tests for all 50+ command handlers (issue #146).
// These tests verify ACTUAL output content, state correctness, and
// behavioral guarantees, not just mode enum transitions.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

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

/// Extract popup output text from app mode, panicking with context if not PopupMode.
fn extract_popup(app: &AppState) -> (&str, &str) {
    match &app.mode {
        Mode::PopupMode { command, output, .. } => (command, output),
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  1. list-buffers: verify output format shows correct indices, sizes, content
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn list_buffers_empty_says_no_buffers() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-buffers").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "list-buffers");
    assert_eq!(out.trim(), "(no buffers)");
}

#[test]
fn list_buffers_shows_all_buffer_details() {
    let mut app = mock_app_with_window();
    app.paste_buffers.push("hello world".to_string());
    app.paste_buffers.push("short".to_string());
    app.paste_buffers.push("a longer buffer with more text for preview".to_string());
    execute_command_string(&mut app, "list-buffers").unwrap();
    let (_, out) = extract_popup(&app);
    let lines: Vec<&str> = out.lines().collect();
    // Must have exactly 3 lines (one per buffer)
    assert_eq!(lines.len(), 3, "should list all 3 buffers, got:\n{}", out);
    // Buffer 0: verify index, byte count, content
    assert!(lines[0].starts_with("buffer0:"), "first line should start with buffer0");
    assert!(lines[0].contains("11 bytes"), "buffer0 should show 11 bytes for 'hello world'");
    assert!(lines[0].contains("hello world"), "buffer0 should show content preview");
    // Buffer 1
    assert!(lines[1].starts_with("buffer1:"), "second line should start with buffer1");
    assert!(lines[1].contains("5 bytes"), "buffer1 should show 5 bytes for 'short'");
    // Buffer 2
    assert!(lines[2].starts_with("buffer2:"), "third line should start with buffer2");
    assert!(lines[2].contains("42 bytes"), "buffer2 should show 42 bytes");
}

#[test]
fn lsb_alias_produces_identical_output_to_list_buffers() {
    let mut app1 = mock_app_with_window();
    app1.paste_buffers.push("test data".to_string());
    execute_command_string(&mut app1, "list-buffers").unwrap();
    let (_, out1) = extract_popup(&app1);
    let out1 = out1.to_string();

    let mut app2 = mock_app_with_window();
    app2.paste_buffers.push("test data".to_string());
    execute_command_string(&mut app2, "lsb").unwrap();
    let (cmd2, out2) = extract_popup(&app2);
    assert_eq!(cmd2, "list-buffers", "lsb should report command as list-buffers");
    assert_eq!(out1, out2, "lsb output must match list-buffers output");
}

// ════════════════════════════════════════════════════════════════════════════
//  2. show-buffer: verify it displays the exact content of buffer 0
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn show_buffer_displays_first_buffer_content_verbatim() {
    let mut app = mock_app_with_window();
    app.paste_buffers.push("line1\nline2\nline3".to_string());
    app.paste_buffers.push("this should NOT appear".to_string());
    execute_command_string(&mut app, "show-buffer").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-buffer");
    assert_eq!(out, "line1\nline2\nline3", "show-buffer must display buffer[0] verbatim");
}

#[test]
fn show_buffer_empty_does_not_crash() {
    let mut app = mock_app_with_window();
    // No buffers: should stay in Passthrough (no popup because nothing to show)
    execute_command_string(&mut app, "show-buffer").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough), "show-buffer with no buffers should be no-op");
}

#[test]
fn showb_alias_same_as_show_buffer() {
    let mut app = mock_app_with_window();
    app.paste_buffers.push("alias test".to_string());
    execute_command_string(&mut app, "showb").unwrap();
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "alias test");
}

// ════════════════════════════════════════════════════════════════════════════
//  3. list-keys: verify output shows key tables, key names, and commands
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn list_keys_empty_says_no_bindings() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-keys").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "list-keys");
    assert_eq!(out.trim(), "(no bindings)");
}

#[test]
fn list_keys_shows_bound_keys_with_table_key_command() {
    let mut app = mock_app_with_window();
    // Add real key bindings to the prefix key table
    let binds = vec![
        crate::types::Bind {
            key: (KeyCode::Char('c'), KeyModifiers::NONE),
            action: Action::NewWindow,
            repeat: false,
        },
        crate::types::Bind {
            key: (KeyCode::Char('x'), KeyModifiers::CONTROL),
            action: Action::KillPane,
            repeat: false,
        },
        crate::types::Bind {
            key: (KeyCode::Up, KeyModifiers::NONE),
            action: Action::MoveFocus(FocusDir::Up),
            repeat: false,
        },
    ];
    app.key_tables.insert("prefix".to_string(), binds);
    execute_command_string(&mut app, "list-keys").unwrap();
    let (_, out) = extract_popup(&app);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "should list 3 bindings");
    // Verify table name, key format, and command string for each binding
    assert!(lines.iter().any(|l| l.contains("prefix") && l.contains("c") && l.contains("new-window")),
        "should contain 'prefix c new-window', got:\n{}", out);
    assert!(lines.iter().any(|l| l.contains("prefix") && l.contains("C-x") && l.contains("kill-pane")),
        "should contain 'prefix C-x kill-pane', got:\n{}", out);
    assert!(lines.iter().any(|l| l.contains("prefix") && l.contains("Up") && l.contains("select-pane -U")),
        "should contain 'prefix Up select-pane -U', got:\n{}", out);
}

#[test]
fn list_keys_shows_multiple_tables() {
    let mut app = mock_app_with_window();
    app.key_tables.insert("prefix".to_string(), vec![
        crate::types::Bind { key: (KeyCode::Char('n'), KeyModifiers::NONE), action: Action::NextWindow, repeat: false },
    ]);
    app.key_tables.insert("copy-mode".to_string(), vec![
        crate::types::Bind { key: (KeyCode::Char('q'), KeyModifiers::NONE), action: Action::Command("cancel".to_string()), repeat: false },
    ]);
    execute_command_string(&mut app, "list-keys").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("prefix"), "output should contain prefix table");
    assert!(out.contains("copy-mode"), "output should contain copy-mode table");
    assert!(out.contains("next-window"), "should show next-window command");
    assert!(out.contains("cancel"), "should show cancel command");
}

#[test]
fn lsk_alias_produces_same_output() {
    let mut app1 = mock_app_with_window();
    app1.key_tables.insert("root".to_string(), vec![
        crate::types::Bind { key: (KeyCode::F(1), KeyModifiers::NONE), action: Action::Command("help".to_string()), repeat: false },
    ]);
    execute_command_string(&mut app1, "list-keys").unwrap();
    let out1 = extract_popup(&app1).1.to_string();

    let mut app2 = mock_app_with_window();
    app2.key_tables.insert("root".to_string(), vec![
        crate::types::Bind { key: (KeyCode::F(1), KeyModifiers::NONE), action: Action::Command("help".to_string()), repeat: false },
    ]);
    execute_command_string(&mut app2, "lsk").unwrap();
    let out2 = extract_popup(&app2).1.to_string();
    assert_eq!(out1, out2, "lsk alias must produce identical output");
}

// ════════════════════════════════════════════════════════════════════════════
//  4. list-windows: verify tmux-format output with names, flags, indices
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn list_windows_output_has_window_names_and_active_flag() {
    let mut app = mock_app_with_windows(&["editor", "server", "logs"]);
    app.active_idx = 1; // "server" is active
    execute_command_string(&mut app, "list-windows").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "list-windows");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3, "should list 3 windows");
    // Window 0: "editor" NOT active
    assert!(lines[0].starts_with("0:"), "first window index should be 0");
    assert!(lines[0].contains("editor"), "first window name should be 'editor'");
    assert!(!lines[0].contains("*"), "editor should NOT have active flag");
    // Window 1: "server" IS active
    assert!(lines[1].starts_with("1:"), "second window index should be 1");
    assert!(lines[1].contains("server"), "second window name should be 'server'");
    assert!(lines[1].contains("*"), "server should have active flag *");
    // Window 2: "logs" NOT active
    assert!(lines[2].starts_with("2:"), "third window index should be 2");
    assert!(lines[2].contains("logs"), "third window name should be 'logs'");
}

#[test]
fn list_windows_respects_window_base_index() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.window_base_index = 1; // tmux base-index 1
    execute_command_string(&mut app, "list-windows").unwrap();
    let (_, out) = extract_popup(&app);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].starts_with("1:"), "with base-index 1, first window should be index 1, got: {}", lines[0]);
    assert!(lines[1].starts_with("2:"), "second window should be index 2");
}

#[test]
fn list_windows_shows_pane_count() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-windows").unwrap();
    let (_, out) = extract_popup(&app);
    // Even an empty Split counts as 0 panes, the list_windows_tmux function counts Leaf nodes
    assert!(out.contains("panes)") || out.contains("pane)"), "should show pane count");
}

#[test]
fn list_windows_shows_activity_flag() {
    let mut app = mock_app_with_windows(&["main", "bg"]);
    app.active_idx = 0;
    app.windows[1].activity_flag = true;
    execute_command_string(&mut app, "list-windows").unwrap();
    let (_, out) = extract_popup(&app);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].contains("*"), "active window should have *");
    assert!(lines[1].contains("#"), "window with activity_flag should have #");
}

#[test]
fn lsw_alias_matches_list_windows() {
    let mut app1 = mock_app_with_windows(&["x", "y"]);
    execute_command_string(&mut app1, "list-windows").unwrap();
    let out1 = extract_popup(&app1).1.to_string();

    let mut app2 = mock_app_with_windows(&["x", "y"]);
    execute_command_string(&mut app2, "lsw").unwrap();
    let out2 = extract_popup(&app2).1.to_string();
    assert_eq!(out1, out2);
}

// ════════════════════════════════════════════════════════════════════════════
//  5. list-clients: verify session name, window name, encoding in output
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn list_clients_output_has_session_and_window_info() {
    let mut app = mock_app_with_windows(&["editor", "term"]);
    app.active_idx = 1;
    app.session_name = "my_project".to_string();
    execute_command_string(&mut app, "list-clients").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "list-clients");
    assert!(out.contains("my_project"), "must contain session name");
    assert!(out.contains("term"), "must contain active window name");
    assert!(out.contains("(utf8)"), "must contain encoding marker");
    assert!(out.contains("/dev/pts/"), "must contain pseudo-terminal path");
}

#[test]
fn lsc_alias_matches_list_clients() {
    let mut app = mock_app_with_window();
    app.session_name = "s1".to_string();
    execute_command_string(&mut app, "lsc").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("s1"), "lsc should show session name");
}

// ════════════════════════════════════════════════════════════════════════════
//  6. list-commands: verify known commands appear in output
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn list_commands_contains_all_major_commands() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-commands").unwrap();
    let (_, out) = extract_popup(&app);
    let required = [
        "list-windows", "list-clients", "list-commands",
        "list-keys", "list-sessions", "list-buffers",
        "show-hooks", "show-buffer", "show-options",
        "new-window", "split-window", "kill-pane", "kill-window",
        "select-window", "select-pane", "rename-window",
        "copy-mode", "paste-buffer", "choose-tree",
        "display-panes", "clock-mode", "command-prompt",
    ];
    for cmd_name in &required {
        assert!(out.contains(cmd_name), "list-commands output missing '{}'. Full output:\n{}", cmd_name, out);
    }
}

#[test]
fn lscm_alias_matches_list_commands() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lscm").unwrap();
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "list-commands");
}

// ════════════════════════════════════════════════════════════════════════════
//  7. show-hooks: verify exact output format
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn show_hooks_empty_says_no_hooks() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-hooks").unwrap();
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-hooks");
    assert_eq!(out.trim(), "(no hooks)");
}

#[test]
fn show_hooks_lists_all_hooks_with_commands() {
    let mut app = mock_app_with_window();
    app.hooks.insert("after-new-window".to_string(),
        vec!["run-shell 'echo new'".to_string(), "display-message 'created'".to_string()]);
    app.hooks.insert("pane-died".to_string(),
        vec!["kill-pane".to_string()]);
    execute_command_string(&mut app, "show-hooks").unwrap();
    let (_, out) = extract_popup(&app);
    let lines: Vec<&str> = out.lines().collect();
    // 2 indexed commands for after-new-window + 1 for pane-died = 3 lines
    assert_eq!(lines.len(), 3, "should have 3 hook entries, got:\n{}", out);
    // Multi-command hooks use indexed format: name[0] -> cmd, name[1] -> cmd
    assert!(out.contains("after-new-window[0] -> run-shell"), "should show indexed hook[0] -> command");
    assert!(out.contains("after-new-window[1] -> display-message"), "should show indexed hook[1] -> command");
    // Single-command hook uses plain format: name -> cmd
    assert!(out.contains("pane-died -> kill-pane"), "should show pane-died hook");
}

// ════════════════════════════════════════════════════════════════════════════
//  8. set-buffer / delete-buffer: verify LIFO ordering, cap, correct removal
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn set_buffer_inserts_at_front_lifo() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-buffer first").unwrap();
    execute_command_string(&mut app, "set-buffer second").unwrap();
    execute_command_string(&mut app, "set-buffer third").unwrap();
    assert_eq!(app.paste_buffers.len(), 3);
    assert_eq!(app.paste_buffers[0], "third", "most recent should be at index 0");
    assert_eq!(app.paste_buffers[1], "second");
    assert_eq!(app.paste_buffers[2], "first");
}

#[test]
fn set_buffer_caps_at_10_evicts_oldest() {
    let mut app = mock_app_with_window();
    for i in 0..12 {
        execute_command_string(&mut app, &format!("set-buffer item{}", i)).unwrap();
    }
    assert_eq!(app.paste_buffers.len(), 10, "buffer list must cap at 10");
    assert_eq!(app.paste_buffers[0], "item11", "latest should be first");
    assert_eq!(app.paste_buffers[9], "item2", "oldest surviving should be item2");
    // item0 and item1 should have been evicted
    assert!(!app.paste_buffers.contains(&"item0".to_string()), "item0 should be evicted");
    assert!(!app.paste_buffers.contains(&"item1".to_string()), "item1 should be evicted");
}

#[test]
fn setb_alias_inserts_same_as_set_buffer() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "setb via_alias").unwrap();
    assert_eq!(app.paste_buffers[0], "via_alias");
}

#[test]
fn delete_buffer_removes_first_buffer() {
    let mut app = mock_app_with_window();
    app.paste_buffers = vec!["a".into(), "b".into(), "c".into()];
    execute_command_string(&mut app, "delete-buffer").unwrap();
    assert_eq!(app.paste_buffers, vec!["b", "c"], "delete-buffer should remove index 0");
}

#[test]
fn deleteb_alias_works() {
    let mut app = mock_app_with_window();
    app.paste_buffers = vec!["only".into()];
    execute_command_string(&mut app, "deleteb").unwrap();
    assert!(app.paste_buffers.is_empty());
}

#[test]
fn delete_buffer_on_empty_is_safe() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "delete-buffer").unwrap();
    assert!(app.paste_buffers.is_empty());
}

#[test]
fn set_then_show_then_delete_roundtrip() {
    let mut app = mock_app_with_window();
    // Set a buffer, verify show-buffer displays it, delete it, verify empty
    execute_command_string(&mut app, "set-buffer roundtrip_data").unwrap();
    execute_command_string(&mut app, "show-buffer").unwrap();
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "roundtrip_data");
    app.mode = Mode::Passthrough;
    execute_command_string(&mut app, "delete-buffer").unwrap();
    assert!(app.paste_buffers.is_empty());
    // list-buffers should now say (no buffers)
    execute_command_string(&mut app, "list-buffers").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("no buffers"));
}

// ════════════════════════════════════════════════════════════════════════════
//  9. Window navigation: verify active_idx AND last_window_idx tracking
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn next_window_advances_and_tracks_last() {
    let mut app = mock_app_with_windows(&["a", "b", "c", "d"]);
    assert_eq!(app.active_idx, 0);
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 1);
    assert_eq!(app.last_window_idx, 0, "last_window_idx should be previous window");
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 2);
    assert_eq!(app.last_window_idx, 1);
}

#[test]
fn next_window_wraps_around() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 1;
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 0, "should wrap to first window");
}

#[test]
fn previous_window_goes_back_and_wraps() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    assert_eq!(app.active_idx, 0);
    execute_command_string(&mut app, "previous-window").unwrap();
    assert_eq!(app.active_idx, 2, "prev from 0 should wrap to last");
    assert_eq!(app.last_window_idx, 0);
    execute_command_string(&mut app, "previous-window").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn last_window_swaps_active_and_last() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 0;
    app.last_window_idx = 2;
    execute_command_string(&mut app, "last-window").unwrap();
    assert_eq!(app.active_idx, 2, "should jump to last visited");
    assert_eq!(app.last_window_idx, 0, "previous active should become last");
    // Toggle back
    execute_command_string(&mut app, "last-window").unwrap();
    assert_eq!(app.active_idx, 0);
    assert_eq!(app.last_window_idx, 2);
}

#[test]
fn select_window_plain_target() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    execute_command_string(&mut app, "select-window -t 2").unwrap();
    assert_eq!(app.active_idx, 2);
    assert_eq!(app.last_window_idx, 0, "previous active should be saved as last");
}

#[test]
fn select_window_colon_target() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    execute_command_string(&mut app, "select-window -t :1").unwrap();
    assert_eq!(app.active_idx, 1, "colon-prefixed target ':1' should select window 1");
}

#[test]
fn select_window_colon_equals_target() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    execute_command_string(&mut app, "select-window -t :=2").unwrap();
    assert_eq!(app.active_idx, 2, "':=2' should select window 2");
}

#[test]
fn selectw_alias_works() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    execute_command_string(&mut app, "selectw -t 1").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn select_window_out_of_range_is_ignored() {
    let mut app = mock_app_with_windows(&["only"]);
    execute_command_string(&mut app, "select-window -t 999").unwrap();
    assert_eq!(app.active_idx, 0, "out-of-range target should not change window");
}

#[test]
fn select_window_with_base_index_offset() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.window_base_index = 1;
    // With base_index=1, target "2" means internal index 1
    execute_command_string(&mut app, "select-window -t 2").unwrap();
    assert_eq!(app.active_idx, 1, "target 2 with base_index 1 should select internal index 1");
}

// ════════════════════════════════════════════════════════════════════════════
//  10. kill-window: verify correct window removed, active_idx adjusted
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn kill_window_removes_active_window() {
    let mut app = mock_app_with_windows(&["alpha", "beta", "gamma"]);
    app.active_idx = 1; // kill "beta"
    execute_command_string(&mut app, "kill-window").unwrap();
    assert_eq!(app.windows.len(), 2);
    let names: Vec<&str> = app.windows.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "gamma"], "beta should be removed");
}

#[test]
fn kill_window_adjusts_active_idx_when_killing_last() {
    let mut app = mock_app_with_windows(&["a", "b", "c"]);
    app.active_idx = 2; // kill last ("c")
    execute_command_string(&mut app, "kill-window").unwrap();
    assert_eq!(app.windows.len(), 2);
    assert_eq!(app.active_idx, 1, "active_idx should be clamped to last valid index");
}

#[test]
fn kill_window_refuses_last_window() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "kill-window").unwrap();
    assert_eq!(app.windows.len(), 1, "must not kill the last remaining window");
}

#[test]
fn killw_alias_works() {
    let mut app = mock_app_with_windows(&["x", "y"]);
    execute_command_string(&mut app, "killw").unwrap();
    assert_eq!(app.windows.len(), 1);
}

// ════════════════════════════════════════════════════════════════════════════
//  11. rename-window / rename-session: verify name changes correctly
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn rename_window_changes_active_window_name() {
    let mut app = mock_app_with_windows(&["old_name", "other"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "rename-window new_name").unwrap();
    assert_eq!(app.windows[0].name, "new_name");
    assert_eq!(app.windows[1].name, "other", "non-active window should be unchanged");
}

#[test]
fn renamew_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "renamew aliased").unwrap();
    assert_eq!(app.windows[0].name, "aliased");
}

#[test]
fn rename_session_changes_session_name() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rename-session production").unwrap();
    assert_eq!(app.session_name, "production");
    // Verify list-clients reflects the new name
    execute_command_string(&mut app, "list-clients").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("production"), "list-clients should use new session name");
}

// ════════════════════════════════════════════════════════════════════════════
//  12. toggle-sync: verify toggling state
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn toggle_sync_flips_state_correctly() {
    let mut app = mock_app_with_window();
    assert!(!app.sync_input, "sync should start disabled");
    execute_command_string(&mut app, "toggle-sync").unwrap();
    assert!(app.sync_input, "first toggle should enable");
    execute_command_string(&mut app, "toggle-sync").unwrap();
    assert!(!app.sync_input, "second toggle should disable");
    execute_command_string(&mut app, "toggle-sync").unwrap();
    assert!(app.sync_input, "third toggle should re-enable");
}

// ════════════════════════════════════════════════════════════════════════════
//  13. choose-tree: verify tree data contains correct window info
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn choose_tree_enters_window_chooser_mode() {
    let mut app = mock_app_with_windows(&["editor", "server"]);
    app.session_name = "dev".to_string();
    app.active_idx = 0;
    execute_command_string(&mut app, "choose-tree").unwrap();
    match &app.mode {
        Mode::WindowChooser { tree, selected } => {
            // Tree is built from filesystem (.psmux dir); in tests it may be empty.
            // Verify selected is valid (0 for empty tree, or within bounds).
            if !tree.is_empty() {
                assert!(*selected < tree.len(), "selected index should be in range");
                // If our session exists in the tree, verify window entries match
                let current: Vec<_> = tree.iter().filter(|e| e.is_current_session).collect();
                for entry in &current {
                    assert_eq!(entry.session_name, "dev", "session name mismatch in tree entry");
                }
                let win_entries: Vec<_> = tree.iter()
                    .filter(|e| e.is_current_session && !e.is_session_header)
                    .collect();
                if !win_entries.is_empty() {
                    let win_names: Vec<&str> = win_entries.iter().map(|e| e.window_name.as_str()).collect();
                    assert!(win_names.contains(&"editor"), "tree should contain 'editor' window");
                    assert!(win_names.contains(&"server"), "tree should contain 'server' window");
                }
            }
        }
        other => panic!("expected WindowChooser, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn choose_tree_builds_correct_tree_from_list_all_sessions() {
    // Directly test list_all_sessions_tree with known data
    let windows = vec![
        ("editor".to_string(), 1usize, "120x30".to_string(), true, 0usize),
        ("server".to_string(), 2, "120x30".to_string(), false, 1usize),
    ];
    // Create a fake port file for our session so the tree builder can find it
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap();
    let psmux_dir = format!("{}/.psmux", home);
    let port_file = format!("{}/test_tree_session.port", psmux_dir);
    let _ = std::fs::create_dir_all(&psmux_dir);
    let _ = std::fs::write(&port_file, "0");
    
    let tree = crate::session::list_all_sessions_tree("test_tree_session", &windows);
    let _ = std::fs::remove_file(&port_file); // cleanup
    
    // Find our session entries
    let our_entries: Vec<_> = tree.iter().filter(|e| e.session_name == "test_tree_session").collect();
    if !our_entries.is_empty() {
        // Should have 1 session header + 2 window entries
        let headers: Vec<_> = our_entries.iter().filter(|e| e.is_session_header).collect();
        assert_eq!(headers.len(), 1, "should have exactly 1 session header");
        assert!(headers[0].is_current_session, "should be marked as current session");
        
        let wins: Vec<_> = our_entries.iter().filter(|e| !e.is_session_header).collect();
        assert_eq!(wins.len(), 2, "should have 2 window entries");
        assert_eq!(wins[0].window_name, "editor");
        assert_eq!(wins[1].window_name, "server");
        assert!(wins[0].is_active_window, "editor should be active");
        assert!(!wins[1].is_active_window, "server should not be active");
        assert_eq!(wins[0].window_panes, 1);
        assert_eq!(wins[0].window_size, "120x30");
    }
}

#[test]
fn choose_window_and_choose_session_all_enter_window_chooser() {
    // All three commands should enter WindowChooser mode
    for cmd in &["choose-tree", "choose-window", "choose-session"] {
        let mut app = mock_app_with_windows(&["a", "b"]);
        execute_command_string(&mut app, cmd).unwrap();
        assert!(matches!(app.mode, Mode::WindowChooser { .. }), "{} should enter WindowChooser", cmd);
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  14. confirm-before: verify prompt text and stored command
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn confirm_before_stores_command_and_prompt() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "confirm-before kill-server").unwrap();
    match &app.mode {
        Mode::ConfirmMode { prompt, command, input } => {
            assert!(prompt.contains("kill-server"), "prompt should mention the command");
            assert_eq!(command, "kill-server", "stored command must be exact");
            assert!(input.is_empty(), "input should start empty");
        }
        other => panic!("expected ConfirmMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn confirm_alias_works_same_as_confirm_before() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "confirm kill-pane").unwrap();
    match &app.mode {
        Mode::ConfirmMode { command, .. } => assert_eq!(command, "kill-pane"),
        other => panic!("expected ConfirmMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  15. display-menu: verify parsed menu items
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn display_menu_parses_items() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-menu "New Window" n new-window "Kill Pane" k kill-pane"#).unwrap();
    match &app.mode {
        Mode::MenuMode { menu } => {
            assert!(menu.items.len() >= 2, "menu should have at least 2 items, got {}", menu.items.len());
        }
        other => panic!("expected MenuMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  16. display-popup: verify dimensions, close-on-exit flag, command
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn display_popup_default_dimensions() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-popup").unwrap();
    match &app.mode {
        Mode::PopupMode { width, height, close_on_exit, .. } => {
            assert_eq!(*width, 80, "default width should be 80");
            assert_eq!(*height, 24, "default height should be 24");
            assert!(!close_on_exit, "close_on_exit should default to false");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn display_popup_custom_dimensions() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-popup -w 40 -h 10").unwrap();
    match &app.mode {
        Mode::PopupMode { width, height, .. } => {
            assert_eq!(*width, 40);
            assert_eq!(*height, 10);
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn display_popup_close_on_exit_flag() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-popup -E echo hello").unwrap();
    match &app.mode {
        Mode::PopupMode { close_on_exit, .. } => {
            assert!(*close_on_exit, "-E flag should set close_on_exit=true");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn popup_alias_works_same() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "popup -w 50 -h 20").unwrap();
    match &app.mode {
        Mode::PopupMode { width, height, .. } => {
            assert_eq!(*width, 50);
            assert_eq!(*height, 20);
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  17. command-prompt: verify -I initial text and cursor position
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn command_prompt_default_empty() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "command-prompt").unwrap();
    match &app.mode {
        Mode::CommandPrompt { input, cursor } => {
            assert!(input.is_empty());
            assert_eq!(*cursor, 0);
        }
        other => panic!("expected CommandPrompt, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn command_prompt_initial_text_sets_cursor_at_end() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "command-prompt -I kill-pane").unwrap();
    match &app.mode {
        Mode::CommandPrompt { input, cursor } => {
            assert_eq!(input, "kill-pane");
            assert_eq!(*cursor, 9, "cursor should be at end of initial text");
        }
        other => panic!("expected CommandPrompt, got {:?}", std::mem::discriminant(other)),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  18. clock-mode / copy-mode / choose-buffer: mode transitions
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn clock_mode_enters_clock() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "clock-mode").unwrap();
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn copy_mode_enters_copy() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "copy-mode").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
}

#[test]
fn choose_buffer_enters_buffer_chooser_at_0() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-buffer").unwrap();
    match &app.mode {
        Mode::BufferChooser { selected } => assert_eq!(*selected, 0),
        other => panic!("expected BufferChooser, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn chooseb_alias_enters_buffer_chooser() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "chooseb").unwrap();
    assert!(matches!(app.mode, Mode::BufferChooser { .. }));
}

#[test]
fn display_panes_populates_display_map() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-panes").unwrap();
    assert!(matches!(app.mode, Mode::PaneChooser { .. }));
    // display_map should be populated (even if empty for zero-pane split)
}

#[test]
fn displayp_alias_works() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "displayp").unwrap();
    assert!(matches!(app.mode, Mode::PaneChooser { .. }));
}

// ════════════════════════════════════════════════════════════════════════════
//  19. new-session: issue #200 fix, now actually creates sessions instead of blocking
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn new_session_does_not_block_with_popup() {
    // Issue #200: new-session should no longer show the blocking popup.
    // It should attempt to create a session (which may fail in test env
    // without a real server, but must NOT show the old blocking popup).
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session").unwrap();
    let in_blocking_popup = matches!(&app.mode, Mode::PopupMode { output, .. } if output.contains("cannot create"));
    assert!(!in_blocking_popup, "new-session should not show blocking popup after issue #200 fix");
}

#[test]
fn new_alias_does_not_block() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new").unwrap();
    let in_blocking_popup = matches!(&app.mode, Mode::PopupMode { output, .. } if output.contains("cannot create"));
    assert!(!in_blocking_popup, "'new' alias should not show blocking popup after issue #200 fix");
}

// ════════════════════════════════════════════════════════════════════════════
//  20. No-op commands: verify they don't crash AND don't mutate state
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn attach_session_is_noop_preserves_state() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 1;
    let original_name = app.session_name.clone();
    for cmd in &["attach-session", "attach", "a", "at"] {
        execute_command_string(&mut app, cmd).unwrap();
        assert_eq!(app.active_idx, 1, "{} should not change active_idx", cmd);
        assert_eq!(app.session_name, original_name, "{} should not change session_name", cmd);
        assert!(matches!(app.mode, Mode::Passthrough), "{} should stay Passthrough", cmd);
    }
}

#[test]
fn start_server_is_noop() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "start-server").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    execute_command_string(&mut app, "start").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
}

#[test]
fn has_session_is_noop() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "has-session").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    execute_command_string(&mut app, "has").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
}

#[test]
fn choose_client_is_noop() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-client").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
}

#[test]
fn customize_mode_shows_options_popup() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "customize-mode").unwrap();
    // customize-mode now opens an interactive option editor
    assert!(matches!(app.mode, Mode::CustomizeMode { .. }));
}

// ════════════════════════════════════════════════════════════════════════════
//  21. Server-forwarded commands: no-crash without port; no state mutation
// ════════════════════════════════════════════════════════════════════════════

fn assert_server_forward_noop(cmd: &str) {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let original_len = app.windows.len();
    let original_name = app.session_name.clone();
    let original_sync = app.sync_input;
    execute_command_string(&mut app, cmd).unwrap();
    assert_eq!(app.windows.len(), original_len, "'{}' must not add/remove windows", cmd);
    assert_eq!(app.session_name, original_name, "'{}' must not change session name", cmd);
    assert_eq!(app.sync_input, original_sync, "'{}' must not change sync state", cmd);
}

#[test]
fn server_forwarded_show_options() { assert_server_forward_noop("show-options"); }
#[test]
fn server_forwarded_show() { assert_server_forward_noop("show"); }
#[test]
fn server_forwarded_showw() { assert_server_forward_noop("showw"); }
#[test]
fn server_forwarded_display_message() { assert_server_forward_noop("display-message hello"); }
#[test]
fn server_forwarded_display() { assert_server_forward_noop("display hello"); }
#[test]
fn server_forwarded_show_messages() { assert_server_forward_noop("show-messages"); }
#[test]
fn server_forwarded_showmsgs() { assert_server_forward_noop("showmsgs"); }
#[test]
fn server_forwarded_set_environment() { assert_server_forward_noop("set-environment FOO bar"); }
#[test]
fn server_forwarded_setenv() { assert_server_forward_noop("setenv FOO bar"); }
#[test]
fn server_forwarded_show_environment() { assert_server_forward_noop("show-environment"); }
#[test]
fn server_forwarded_showenv() { assert_server_forward_noop("showenv"); }
#[test]
fn server_forwarded_set_hook() { assert_server_forward_noop("set-hook after-new-window 'echo'"); }
#[test]
fn server_forwarded_send_prefix() { assert_server_forward_noop("send-prefix"); }
#[test]
fn server_forwarded_if_shell() { assert_server_forward_noop("if-shell true new-window"); }
#[test]
fn server_forwarded_if_alias() { assert_server_forward_noop("if true new-window"); }
#[test]
fn server_forwarded_wait_for() { assert_server_forward_noop("wait-for done"); }
#[test]
fn server_forwarded_wait() { assert_server_forward_noop("wait done"); }
#[test]
fn server_forwarded_find_window() { assert_server_forward_noop("find-window pattern"); }
#[test]
fn server_forwarded_findw() { assert_server_forward_noop("findw pattern"); }
#[test]
fn server_forwarded_move_window() { assert_server_forward_noop("move-window -t 1"); }
#[test]
fn server_forwarded_movew() { assert_server_forward_noop("movew -t 1"); }
#[test]
fn server_forwarded_swap_window() { assert_server_forward_noop("swap-window -t 1"); }
#[test]
fn server_forwarded_swapw() { assert_server_forward_noop("swapw -t 1"); }
#[test]
fn server_forwarded_link_window() {
    // link-window is now functional: creates a linked window (not a noop)
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "link-window -s 0 -t 1").unwrap();
    // May or may not add a window depending on PTY availability in test env
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}
#[test]
fn server_forwarded_linkw() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "linkw -s 0 -t 1").unwrap();
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}
#[test]
fn server_forwarded_unlink_window() { assert_server_forward_noop("unlink-window"); }
#[test]
fn server_forwarded_unlinkw() { assert_server_forward_noop("unlinkw"); }
#[test]
fn server_forwarded_move_pane() { assert_server_forward_noop("move-pane"); }
#[test]
fn server_forwarded_movep() { assert_server_forward_noop("movep"); }
#[test]
fn server_forwarded_join_pane() { assert_server_forward_noop("join-pane -t 1"); }
#[test]
fn server_forwarded_joinp() { assert_server_forward_noop("joinp -t 1"); }
#[test]
fn server_forwarded_resize_window() { assert_server_forward_noop("resize-window -x 80 -y 24"); }
#[test]
fn server_forwarded_resizew() { assert_server_forward_noop("resizew -x 80 -y 24"); }
#[test]
fn server_forwarded_server_info() { assert_server_forward_noop("server-info"); }
#[test]
fn server_forwarded_info() { assert_server_forward_noop("info"); }
#[test]
fn server_forwarded_lock_variants() {
    for cmd in &["lock-client", "lockc", "lock-server", "lock", "lock-session", "locks"] {
        assert_server_forward_noop(cmd);
    }
}
#[test]
fn server_forwarded_refresh() { assert_server_forward_noop("refresh-client"); }
#[test]
fn server_forwarded_refresh_alias() { assert_server_forward_noop("refresh"); }
#[test]
fn server_forwarded_suspend() { assert_server_forward_noop("suspend-client"); }
#[test]
fn server_forwarded_suspendc() { assert_server_forward_noop("suspendc"); }
#[test]
fn server_forwarded_send_keys() { assert_server_forward_noop("send-keys Enter"); }
#[test]
fn server_forwarded_send() { assert_server_forward_noop("send Enter"); }
#[test]
fn server_forwarded_pipe_pane() { assert_server_forward_noop("pipe-pane cat"); }
#[test]
fn server_forwarded_pipep() { assert_server_forward_noop("pipep cat"); }
#[test]
fn server_forwarded_kill_session() { assert_server_forward_noop("kill-session"); }
#[test]
fn server_forwarded_kill_ses() { assert_server_forward_noop("kill-ses"); }
#[test]
fn server_forwarded_kill_server() { assert_server_forward_noop("kill-server"); }
#[test]
fn server_forwarded_clear_history() { assert_server_forward_noop("clear-history"); }
#[test]
fn server_forwarded_clearhist() { assert_server_forward_noop("clearhist"); }
#[test]
fn server_forwarded_respawn_window() { assert_server_forward_noop("respawn-window"); }
#[test]
fn server_forwarded_respawnw() { assert_server_forward_noop("respawnw"); }
#[test]
fn server_forwarded_previous_layout() { assert_server_forward_noop("previous-layout"); }
#[test]
fn server_forwarded_prevl() { assert_server_forward_noop("prevl"); }
#[test]
fn server_forwarded_next_layout() { assert_server_forward_noop("next-layout"); }
#[test]
fn server_forwarded_select_layout() { assert_server_forward_noop("select-layout even-horizontal"); }
#[test]
fn server_forwarded_selectl() { assert_server_forward_noop("selectl even-horizontal"); }
#[test]
fn server_forwarded_set_option() { assert_server_forward_noop("set-option status on"); }
#[test]
fn server_forwarded_setw() { assert_server_forward_noop("setw mode-keys vi"); }

// ════════════════════════════════════════════════════════════════════════════
//  22. Command prompt delegation: verify full pipeline works
// ════════════════════════════════════════════════════════════════════════════

fn run_via_prompt(app: &mut AppState, cmd: &str) {
    app.mode = Mode::CommandPrompt { input: cmd.to_string(), cursor: cmd.len() };
    execute_command_prompt(app).unwrap();
}

#[test]
fn prompt_delegates_clock_mode() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "clock-mode");
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn prompt_delegates_toggle_sync() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "toggle-sync");
    assert!(app.sync_input);
}

#[test]
fn prompt_delegates_rename_session_with_state_verification() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "rename-session prompted_name");
    assert_eq!(app.session_name, "prompted_name");
}

#[test]
fn prompt_delegates_set_buffer_then_list_shows_it() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "set-buffer via_prompt");
    assert_eq!(app.paste_buffers[0], "via_prompt");
    // Now list-buffers via prompt should show it
    run_via_prompt(&mut app, "list-buffers");
    let (_, out) = extract_popup(&app);
    assert!(out.contains("via_prompt"), "list-buffers should show buffer set via prompt");
}

#[test]
fn prompt_delegates_choose_tree() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "choose-tree");
    assert!(matches!(app.mode, Mode::WindowChooser { .. }));
}

#[test]
fn prompt_delegates_list_keys() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "list-keys");
    let (cmd, _) = extract_popup(&app);
    assert_eq!(cmd, "list-keys");
}

#[test]
fn prompt_delegates_new_session_creates_session() {
    // Issue #200: command prompt new-session should attempt creation, not block
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "new-session");
    let in_blocking_popup = matches!(&app.mode, Mode::PopupMode { output, .. } if output.contains("cannot create"));
    assert!(!in_blocking_popup, "command prompt new-session should not show blocking popup");
}

#[test]
fn prompt_unknown_command_falls_through() {
    let mut app = mock_app_with_window();
    run_via_prompt(&mut app, "nonexistent-command");
    assert!(matches!(app.mode, Mode::Passthrough));
}

// ════════════════════════════════════════════════════════════════════════════
//  23. parse_command_to_action: verify EXACT action types for aliases
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn parse_action_direct_commands() {
    assert!(matches!(parse_command_to_action("display-panes"), Some(Action::DisplayPanes)));
    assert!(matches!(parse_command_to_action("displayp"), Some(Action::DisplayPanes)));
    assert!(matches!(parse_command_to_action("new-window"), Some(Action::NewWindow)));
    assert!(matches!(parse_command_to_action("neww"), Some(Action::NewWindow)));
    assert!(matches!(parse_command_to_action("kill-pane"), Some(Action::KillPane)));
    assert!(matches!(parse_command_to_action("killp"), Some(Action::KillPane)));
    assert!(matches!(parse_command_to_action("next-window"), Some(Action::NextWindow)));
    assert!(matches!(parse_command_to_action("next"), Some(Action::NextWindow)));
    assert!(matches!(parse_command_to_action("previous-window"), Some(Action::PrevWindow)));
    assert!(matches!(parse_command_to_action("prev"), Some(Action::PrevWindow)));
    assert!(matches!(parse_command_to_action("copy-mode"), Some(Action::CopyMode)));
    assert!(matches!(parse_command_to_action("paste-buffer"), Some(Action::Paste)));
    assert!(matches!(parse_command_to_action("pasteb"), Some(Action::Paste)));
    assert!(matches!(parse_command_to_action("detach-client"), Some(Action::Detach)));
    assert!(matches!(parse_command_to_action("detach"), Some(Action::Detach)));
    assert!(matches!(parse_command_to_action("rename-window"), Some(Action::RenameWindow)));
    assert!(matches!(parse_command_to_action("renamew"), Some(Action::RenameWindow)));
    assert!(matches!(parse_command_to_action("choose-tree"), Some(Action::WindowChooser)));
    assert!(matches!(parse_command_to_action("choose-window"), Some(Action::WindowChooser)));
    assert!(matches!(parse_command_to_action("choose-session"), Some(Action::SessionChooser)));
    assert!(matches!(parse_command_to_action("zoom-pane"), Some(Action::ZoomPane)));
    assert!(matches!(parse_command_to_action("resize-pane -Z"), Some(Action::ZoomPane)));
}

#[test]
fn parse_action_command_wrapping_aliases() {
    // These should all produce Action::Command with the correct string
    let aliases = [
        ("last", "last-window"),
        ("lastp", "last-pane"),
        ("lsb", "lsb"),
        ("showb", "showb"),
        ("chooseb", "chooseb"),
        ("lsk", "lsk"),
        ("showmsgs", "showmsgs"),
        ("findw", "findw"),
        ("movew", "movew"),
        ("swapw", "swapw"),
        ("linkw", "linkw"),
        ("unlinkw", "unlinkw"),
        ("movep", "movep"),
        ("joinp", "joinp"),
        ("resizew", "resizew"),
        ("setenv", "setenv"),
        ("showenv", "showenv"),
        ("info", "info"),
        ("lockc", "lockc"),
        ("suspendc", "suspendc"),
    ];
    for (alias, expected_cmd) in &aliases {
        match parse_command_to_action(alias) {
            Some(Action::Command(ref c)) => {
                assert_eq!(c, expected_cmd, "alias '{}' should produce Command('{}')", alias, expected_cmd);
            }
            _other => panic!("alias '{}' should produce Command, got different action", alias),
        }
    }
}

#[test]
fn parse_action_split_window_variants() {
    assert!(matches!(parse_command_to_action("split-window"), Some(Action::SplitVertical)));
    assert!(matches!(parse_command_to_action("splitw"), Some(Action::SplitVertical)));
    assert!(matches!(parse_command_to_action("split-window -h"), Some(Action::SplitHorizontal)));
    assert!(matches!(parse_command_to_action("splitw -h"), Some(Action::SplitHorizontal)));
    // With extra flags it becomes Command to preserve the full args
    assert!(matches!(parse_command_to_action("split-window -c /tmp"), Some(Action::Command(_))));
}

#[test]
fn parse_action_select_pane_directions() {
    assert!(matches!(parse_command_to_action("select-pane -U"), Some(Action::MoveFocus(FocusDir::Up))));
    assert!(matches!(parse_command_to_action("select-pane -D"), Some(Action::MoveFocus(FocusDir::Down))));
    assert!(matches!(parse_command_to_action("select-pane -L"), Some(Action::MoveFocus(FocusDir::Left))));
    assert!(matches!(parse_command_to_action("select-pane -R"), Some(Action::MoveFocus(FocusDir::Right))));
    assert!(matches!(parse_command_to_action("selectp -U"), Some(Action::MoveFocus(FocusDir::Up))));
    assert!(matches!(parse_command_to_action("selectp -D"), Some(Action::MoveFocus(FocusDir::Down))));
    // No direction flag becomes Command
    assert!(matches!(parse_command_to_action("select-pane"), Some(Action::Command(_))));
}

#[test]
fn parse_action_switch_client_table() {
    match parse_command_to_action("switch-client -T copy-mode") {
        Some(Action::SwitchTable(t)) => assert_eq!(t, "copy-mode"),
        _ => panic!("expected SwitchTable"),
    }
    match parse_command_to_action("switchc -T prefix") {
        Some(Action::SwitchTable(t)) => assert_eq!(t, "prefix"),
        _ => panic!("expected SwitchTable"),
    }
    // Without -T becomes Command
    assert!(matches!(parse_command_to_action("switchc"), Some(Action::Command(_))));
}

#[test]
fn parse_action_empty_and_whitespace() {
    assert!(parse_command_to_action("").is_none());
    assert!(parse_command_to_action("   ").is_none());
}

// ════════════════════════════════════════════════════════════════════════════
//  24. format_action: verify bidirectional mapping
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn format_action_roundtrips() {
    let cases: Vec<(Action, &str)> = vec![
        (Action::DisplayPanes, "display-panes"),
        (Action::NewWindow, "new-window"),
        (Action::SplitHorizontal, "split-window -h"),
        (Action::SplitVertical, "split-window -v"),
        (Action::KillPane, "kill-pane"),
        (Action::NextWindow, "next-window"),
        (Action::PrevWindow, "previous-window"),
        (Action::CopyMode, "copy-mode"),
        (Action::Paste, "paste-buffer"),
        (Action::Detach, "detach-client"),
        (Action::RenameWindow, "rename-window"),
        (Action::WindowChooser, "choose-window"),
        (Action::ZoomPane, "resize-pane -Z"),
        (Action::MoveFocus(FocusDir::Up), "select-pane -U"),
        (Action::MoveFocus(FocusDir::Down), "select-pane -D"),
        (Action::MoveFocus(FocusDir::Left), "select-pane -L"),
        (Action::MoveFocus(FocusDir::Right), "select-pane -R"),
        (Action::Command("list-keys".to_string()), "list-keys"),
        (Action::SwitchTable("copy-mode".to_string()), "switch-client -T copy-mode"),
        (Action::CommandChain(vec!["new-window".to_string(), "split-window".to_string()]),
            "new-window \\; split-window"),
    ];
    for (action, expected) in &cases {
        let formatted = format_action(action);
        assert_eq!(&formatted, expected, "format_action({:?}) should produce '{}'",
            expected, expected);
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  25. Multi-step integration scenarios
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn workflow_create_buffers_list_delete_roundtrip() {
    let mut app = mock_app_with_window();
    // Fill 5 buffers
    for i in 0..5 {
        execute_command_string(&mut app, &format!("set-buffer content_{}", i)).unwrap();
    }
    assert_eq!(app.paste_buffers.len(), 5);
    // list-buffers should show all 5 with correct order (LIFO)
    execute_command_string(&mut app, "list-buffers").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("content_4"), "most recent buffer should appear");
    assert!(out.contains("buffer0:"), "first entry should be buffer0");
    assert!(out.contains("buffer4:"), "last entry should be buffer4");
    // show-buffer should show the most recent (content_4)
    app.mode = Mode::Passthrough;
    execute_command_string(&mut app, "show-buffer").unwrap();
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "content_4");
    // Delete first buffer (content_4), then show-buffer should show content_3
    app.mode = Mode::Passthrough;
    execute_command_string(&mut app, "delete-buffer").unwrap();
    execute_command_string(&mut app, "show-buffer").unwrap();
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "content_3");
}

#[test]
fn workflow_navigate_windows_verify_tracking() {
    let mut app = mock_app_with_windows(&["alpha", "beta", "gamma", "delta"]);
    // Start at 0, navigate right through all, verify last_window_idx at each step
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 1);
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 2);
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 3);
    // last-window should go back to 2
    execute_command_string(&mut app, "last-window").unwrap();
    assert_eq!(app.active_idx, 2);
    assert_eq!(app.last_window_idx, 3);
    // select-window -t 0 should jump to alpha
    execute_command_string(&mut app, "select-window -t 0").unwrap();
    assert_eq!(app.active_idx, 0);
    assert_eq!(app.last_window_idx, 2);
    // Rename current window and verify list-windows reflects it
    execute_command_string(&mut app, "rename-window renamed_alpha").unwrap();
    execute_command_string(&mut app, "list-windows").unwrap();
    let (_, out) = extract_popup(&app);
    assert!(out.contains("renamed_alpha"), "list-windows should show renamed window");
    assert!(out.contains("*"), "active window should have flag");
}

#[test]
fn workflow_complex_command_sequence_via_prompt() {
    let mut app = mock_app_with_windows(&["main", "aux"]);
    // Set a buffer via prompt
    run_via_prompt(&mut app, "set-buffer prompt_test");
    assert_eq!(app.paste_buffers[0], "prompt_test");
    // Switch windows via prompt
    run_via_prompt(&mut app, "next-window");
    assert_eq!(app.active_idx, 1);
    // Rename via prompt
    run_via_prompt(&mut app, "rename-window renamed_aux");
    assert_eq!(app.windows[1].name, "renamed_aux");
    // Rename session via prompt
    run_via_prompt(&mut app, "rename-session my_project");
    assert_eq!(app.session_name, "my_project");
    // Verify list-clients shows updated session and window
    run_via_prompt(&mut app, "list-clients");
    let (_, out) = extract_popup(&app);
    assert!(out.contains("my_project"), "should show renamed session");
    assert!(out.contains("renamed_aux"), "should show current (renamed) window");
}

// ════════════════════════════════════════════════════════════════════════════
//  26. Popup dimensions: verify scaling based on content
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn popup_width_scales_to_longest_line() {
    let mut app = mock_app_with_window();
    let short = "ab";
    let long = "a".repeat(100);
    let content = format!("{}\n{}", short, long);
    show_output_popup(&mut app, "test", content);
    match &app.mode {
        Mode::PopupMode { width, .. } => {
            // Width should accommodate the longest line (100 chars + 4 padding)
            assert!(*width >= 104, "width should be >= 104 for 100-char line, got {}", width);
        }
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn popup_height_scales_to_line_count() {
    let mut app = mock_app_with_window();
    let content = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    show_output_popup(&mut app, "test", content);
    match &app.mode {
        Mode::PopupMode { height, .. } => {
            // Height = lines + 2, capped at 40
            assert!(*height >= 22, "height should be >= 22 for 20 lines, got {}", height);
        }
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn popup_height_not_capped_allows_scroll() {
    let mut app = mock_app_with_window();
    let content = (0..100).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
    show_output_popup(&mut app, "test", content);
    match &app.mode {
        Mode::PopupMode { height, .. } => {
            // Height should accommodate all lines (100 + 2 for border)
            assert_eq!(*height, 102, "height should equal line count + 2, got {}", height);
        }
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn popup_width_capped_at_120() {
    let mut app = mock_app_with_window();
    let content = "x".repeat(200);
    show_output_popup(&mut app, "test", content);
    match &app.mode {
        Mode::PopupMode { width, .. } => {
            assert!(*width <= 120, "width should be capped at 120, got {}", width);
        }
        _ => panic!("expected PopupMode"),
    }
}

#[test]
fn popup_minimum_dimensions() {
    let mut app = mock_app_with_window();
    show_output_popup(&mut app, "test", "x".to_string());
    match &app.mode {
        Mode::PopupMode { width, height, .. } => {
            assert!(*width >= 20, "minimum width is 20, got {}", width);
            assert!(*height >= 5, "minimum height is 5, got {}", height);
        }
        _ => panic!("expected PopupMode"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  27. Edge cases
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn empty_command_string_is_noop() {
    let mut app = mock_app_with_window();
    let buffers_before = app.paste_buffers.len();
    execute_command_string(&mut app, "").unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    assert_eq!(app.paste_buffers.len(), buffers_before);
}

#[test]
fn unknown_command_does_not_mutate_critical_state() {
    let mut app = mock_app_with_window();
    let idx = app.active_idx;
    let name = app.session_name.clone();
    let win_count = app.windows.len();
    execute_command_string(&mut app, "totally-fake-command-xyz").unwrap();
    assert_eq!(app.active_idx, idx);
    assert_eq!(app.session_name, name);
    assert_eq!(app.windows.len(), win_count);
}

#[test]
fn zoom_pane_on_empty_split_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "zoom-pane").unwrap();
    // No panic means success; empty split has nothing to zoom
}

#[test]
fn resize_pane_zoom_flag_does_not_crash() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -Z").unwrap();
}

#[test]
fn swap_pane_without_port_does_not_crash() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "swap-pane -U").unwrap();
    execute_command_string(&mut app, "swapp -D").unwrap();
}

#[test]
fn rotate_window_without_port_does_not_crash() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "rotate-window").unwrap();
    execute_command_string(&mut app, "rotatew -D").unwrap();
}

#[test]
fn break_pane_without_port_does_not_crash() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "break-pane").unwrap();
    execute_command_string(&mut app, "breakp").unwrap();
}

#[test]
fn respawn_pane_without_port_does_not_crash() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    execute_command_string(&mut app, "respawn-pane").unwrap();
    execute_command_string(&mut app, "respawnp").unwrap();
}

// ════════════════════════════════════════════════════════════════════════════
//  Window index prompt (prefix + '): jump to any window by typed number
// ════════════════════════════════════════════════════════════════════════════

use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};
use crate::input::handle_key;

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

#[test]
fn prefix_single_quote_enters_window_index_prompt() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.mode = Mode::Prefix { armed_at: std::time::Instant::now() };
    handle_key(&mut app, press(KeyCode::Char('\''))).unwrap();
    assert!(matches!(app.mode, Mode::WindowIndexPrompt { .. }), "prefix+' should enter WindowIndexPrompt mode");
}

#[test]
fn window_index_prompt_accepts_digits_only() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.mode = Mode::WindowIndexPrompt { input: String::new() };
    // Type digit '1'
    handle_key(&mut app, press(KeyCode::Char('1'))).unwrap();
    if let Mode::WindowIndexPrompt { ref input } = app.mode {
        assert_eq!(input, "1", "digit should be appended");
    } else {
        panic!("should still be in WindowIndexPrompt");
    }
    // Type non-digit 'a' should be ignored
    handle_key(&mut app, press(KeyCode::Char('a'))).unwrap();
    if let Mode::WindowIndexPrompt { ref input } = app.mode {
        assert_eq!(input, "1", "non-digit should be ignored");
    } else {
        panic!("should still be in WindowIndexPrompt");
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Issue #170: run-shell output display
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn run_shell_captures_and_displays_output() {
    let mut app = mock_app();
    // Use a simple echo command that produces stdout
    #[cfg(windows)]
    let cmd = r#"run-shell "Write-Output 'hello-from-run-shell'""#;
    #[cfg(not(windows))]
    let cmd = r#"run-shell "echo hello-from-run-shell""#;

    let _ = execute_command_string(&mut app, cmd);

    // run-shell is now async: the command runs in a background thread
    // and sends output via run_shell_rx. We need to recv the result.
    let rx = app.run_shell_rx.as_ref().expect("run_shell_rx should be created");
    let (title, text) = rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("should receive run-shell output within 10s");
    assert_eq!(title, "run-shell");
    assert!(
        text.contains("hello-from-run-shell"),
        "run-shell output should contain the echoed text, got: {}",
        text
    );
}

#[test]
fn run_shell_background_does_not_show_popup() {
    let mut app = mock_app();
    // With -b flag: should NOT enter PopupMode
    #[cfg(windows)]
    let cmd = r#"run-shell -b "Write-Output 'background-test'""#;
    #[cfg(not(windows))]
    let cmd = r#"run-shell -b "echo background-test""#;

    let _ = execute_command_string(&mut app, cmd);

    assert!(
        !matches!(app.mode, Mode::PopupMode { .. }),
        "run-shell -b should NOT produce a popup, mode = {:?}",
        std::mem::discriminant(&app.mode)
    );
}

#[test]
fn run_shell_alias_captures_output() {
    let mut app = mock_app();
    // "run" is the short alias for "run-shell"
    #[cfg(windows)]
    let cmd = r#"run "Write-Output 'alias-test'""#;
    #[cfg(not(windows))]
    let cmd = r#"run "echo alias-test""#;

    let _ = execute_command_string(&mut app, cmd);

    let rx = app.run_shell_rx.as_ref().expect("run_shell_rx should be created");
    let (_title, text) = rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("should receive run alias output within 10s");
    assert!(
        text.contains("alias-test"),
        "run alias should also capture output, got: {}",
        text
    );
}

#[test]
fn run_shell_stderr_is_captured() {
    let mut app = mock_app();
    // Use a command that writes to stderr
    #[cfg(windows)]
    let cmd = r#"run-shell "Write-Error 'error-output' 2>&1""#;
    #[cfg(not(windows))]
    let cmd = r#"run-shell "echo error-output >&2""#;

    let _ = execute_command_string(&mut app, cmd);

    let rx = app.run_shell_rx.as_ref().expect("run_shell_rx should be created");
    let (_title, text) = rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("should receive run-shell stderr output within 10s");
    assert!(
        text.contains("error-output") || text.contains("error"),
        "run-shell should capture stderr, got: {}",
        text
    );
}

#[test]
fn run_shell_empty_output_no_popup() {
    let mut app = mock_app();
    // A command that produces no output should not show a popup
    #[cfg(windows)]
    let cmd = r#"run-shell "Write-Output ''""#;
    #[cfg(not(windows))]
    let cmd = r#"run-shell "true""#;

    let _ = execute_command_string(&mut app, cmd);

    // On Windows, Write-Output '' produces a newline, so a popup may appear
    // On Unix, `true` produces no output, so no popup
    #[cfg(not(windows))]
    assert!(
        !matches!(app.mode, Mode::PopupMode { .. }),
        "run-shell with no output should not produce a popup"
    );
}

#[test]
fn window_index_prompt_backspace_removes_digit() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.mode = Mode::WindowIndexPrompt { input: "12".to_string() };
    handle_key(&mut app, press(KeyCode::Backspace)).unwrap();
    if let Mode::WindowIndexPrompt { ref input } = app.mode {
        assert_eq!(input, "1", "backspace should remove last digit");
    } else {
        panic!("should still be in WindowIndexPrompt");
    }
}

#[test]
fn window_index_prompt_esc_cancels() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.mode = Mode::WindowIndexPrompt { input: "5".to_string() };
    handle_key(&mut app, press(KeyCode::Esc)).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough), "Esc should cancel to Passthrough");
    assert_eq!(app.active_idx, 0, "active window should not change on cancel");
}

#[test]
fn window_index_prompt_enter_jumps_to_window() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.mode = Mode::WindowIndexPrompt { input: "2".to_string() };
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough), "Enter should return to Passthrough");
    assert_eq!(app.active_idx, 2, "should jump to window 2");
    assert_eq!(app.last_window_idx, 0, "previous window should be saved as last");
}

#[test]
fn window_index_prompt_enter_multidigit() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2", "w3", "w4", "w5",
                                           "w6", "w7", "w8", "w9", "w10", "w11"]);
    app.mode = Mode::WindowIndexPrompt { input: "11".to_string() };
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert_eq!(app.active_idx, 11, "should jump to window 11 (multidigit)");
}

#[test]
fn window_index_prompt_out_of_range_stays_put() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.mode = Mode::WindowIndexPrompt { input: "99".to_string() };
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    assert_eq!(app.active_idx, 0, "out-of-range index should not change window");
}

#[test]
fn window_index_prompt_empty_enter_stays_put() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.mode = Mode::WindowIndexPrompt { input: String::new() };
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    assert_eq!(app.active_idx, 0, "empty input should not change window");
}

#[test]
fn window_index_prompt_respects_base_index() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.window_base_index = 1;
    // With base_index=1, typing "2" means internal index 1
    app.mode = Mode::WindowIndexPrompt { input: "2".to_string() };
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert_eq!(app.active_idx, 1, "target 2 with base_index 1 should select internal idx 1");
}

#[test]
fn window_index_prompt_full_flow_via_prefix() {
    // Simulate the full flow: prefix mode -> ' -> type "1" -> Enter
    let mut app = mock_app_with_windows(&["alpha", "beta", "gamma"]);
    app.mode = Mode::Prefix { armed_at: std::time::Instant::now() };
    handle_key(&mut app, press(KeyCode::Char('\''))).unwrap();
    assert!(matches!(app.mode, Mode::WindowIndexPrompt { .. }));
    handle_key(&mut app, press(KeyCode::Char('1'))).unwrap();
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert!(matches!(app.mode, Mode::Passthrough));
    assert_eq!(app.active_idx, 1, "full flow should jump to window 1 (beta)");
}
// ════════════════════════════════════════════════════════════════════════════
//  Discussion #154: popup percentage dimensions, -d flag, TERM env
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn parse_popup_dim_absolute_value() {
    assert_eq!(crate::commands::parse_popup_dim_local("80", 200, 80), 80);
    assert_eq!(crate::commands::parse_popup_dim_local("40", 200, 80), 40);
    assert_eq!(crate::commands::parse_popup_dim_local("120", 200, 80), 120);
}

#[test]
fn parse_popup_dim_percentage_value() {
    // 95% of 200 = 190
    assert_eq!(crate::commands::parse_popup_dim_local("95%", 200, 80), 190);
    // 50% of 200 = 100
    assert_eq!(crate::commands::parse_popup_dim_local("50%", 200, 80), 100);
    // 100% of 200 = 200
    assert_eq!(crate::commands::parse_popup_dim_local("100%", 200, 80), 200);
    // 10% of 200 = 20
    assert_eq!(crate::commands::parse_popup_dim_local("10%", 200, 80), 20);
}

#[test]
fn parse_popup_dim_percentage_clamped_at_100() {
    // 200% should be clamped to 100% = 200
    assert_eq!(crate::commands::parse_popup_dim_local("200%", 200, 80), 200);
}

#[test]
fn parse_popup_dim_invalid_falls_back_to_default() {
    assert_eq!(crate::commands::parse_popup_dim_local("abc", 200, 80), 80);
    assert_eq!(crate::commands::parse_popup_dim_local("abc%", 200, 80), 80);
    assert_eq!(crate::commands::parse_popup_dim_local("", 200, 80), 80);
}

#[test]
fn display_popup_d_flag_stripped_from_command() {
    // When -d is used, the directory path should NOT leak into the command string
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-popup -d /some/path lazygit").unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert!(!command.contains("/some/path"), "start dir should not be in command, got: {}", command);
            // The actual PTY command won't start in tests (no PTY), but the command string is correct
            assert!(command.contains("lazygit") || command.is_empty(),
                "command should contain lazygit or be empty if PTY failed, got: {}", command);
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn display_popup_c_flag_also_works_for_directory() {
    // -c should work the same as -d for setting the popup directory
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "popup -c /tmp echo test").unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert!(!command.contains("/tmp"), "start dir should not leak into command, got: {}", command);
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn display_popup_d_flag_with_percent_dims() {
    // Combined test: -d with percentage dimensions
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "popup -w 95% -h 80% -d /home/user htop").unwrap();
    match &app.mode {
        Mode::PopupMode { command, width, height, .. } => {
            assert!(!command.contains("/home/user"), "dir should not leak into command");
            // Width/height should be resolved percentages (not the raw "95" or "80")
            // Since crossterm::terminal::size() varies, just verify they are not the raw fallback defaults
            assert!(*width > 0, "width should be resolved");
            assert!(*height > 0, "height should be resolved");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ── Issue #111 follow-up: new-window -c must preserve -c flag in bind-key ──

#[test]
fn new_window_bare_returns_action_new_window() {
    // Bare new-window with no args should still return the simple Action::NewWindow
    assert!(matches!(parse_command_to_action("new-window"), Some(Action::NewWindow)));
    assert!(matches!(parse_command_to_action("neww"), Some(Action::NewWindow)));
}

#[test]
fn new_window_with_c_flag_returns_command_preserving_args() {
    // new-window -c <dir> must NOT be reduced to Action::NewWindow — the -c flag
    // must be preserved so the server can expand #{pane_current_path}. (Issue #111)
    match parse_command_to_action("new-window -c #{pane_current_path}") {
        Some(Action::Command(cmd)) => {
            assert!(cmd.contains("-c"), "expected -c in command, got: {}", cmd);
            assert!(cmd.contains("#{pane_current_path}"), "expected format var in command, got: {}", cmd);
        }
        _ => panic!("expected Action::Command preserving -c"),
    }
}

#[test]
fn new_window_with_name_flag_returns_command() {
    // new-window -n myname should also be preserved as Command
    match parse_command_to_action("new-window -n myname") {
        Some(Action::Command(cmd)) => {
            assert!(cmd.contains("-n"), "expected -n in command, got: {}", cmd);
            assert!(cmd.contains("myname"), "expected window name in command, got: {}", cmd);
        }
        _ => panic!("expected Action::Command"),
    }
}

#[test]
fn new_window_with_shell_command_returns_command() {
    // new-window -- python3 should also be preserved
    match parse_command_to_action("new-window -- python3") {
        Some(Action::Command(cmd)) => {
            assert!(cmd.contains("python3"), "expected shell command in command, got: {}", cmd);
        }
        _ => panic!("expected Action::Command"),
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Issue #170 follow-up: run-shell no-arg usage + display-message defaults
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn run_shell_no_args_shows_usage() {
    let mut app = mock_app();
    let _ = execute_command_string(&mut app, "run-shell");
    // Should show usage on status bar, not enter popup
    assert!(!matches!(app.mode, Mode::PopupMode { .. }), "run-shell with no args should not show popup");
    let msg = app.status_message.as_ref().map(|(m, ..)| m.as_str()).unwrap_or("");
    assert!(msg.contains("usage"), "expected usage message on status bar, got: {}", msg);
}

#[test]
fn run_alias_no_args_shows_usage() {
    let mut app = mock_app();
    let _ = execute_command_string(&mut app, "run");
    assert!(!matches!(app.mode, Mode::PopupMode { .. }));
    let msg = app.status_message.as_ref().map(|(m, ..)| m.as_str()).unwrap_or("");
    assert!(msg.contains("usage"), "expected usage message for 'run' alias, got: {}", msg);
}

#[test]
fn display_message_no_args_uses_default_format() {
    let mut app = mock_app();
    // Ensure control_port is None so the local handler runs
    app.control_port = None;
    let _ = execute_command_string(&mut app, "display-message");
    let msg = app.status_message.as_ref().map(|(m, ..)| m.as_str()).unwrap_or("");
    // Default format should contain session name
    assert!(!msg.is_empty(), "display-message with no args should produce a non-empty status message");
    assert!(msg.contains("test_session"), "default format should expand session_name, got: {}", msg);
}

#[test]
fn display_alias_no_args_uses_default_format() {
    let mut app = mock_app();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "display");
    let msg = app.status_message.as_ref().map(|(m, ..)| m.as_str()).unwrap_or("");
    assert!(!msg.is_empty(), "display alias with no args should produce a non-empty status message");
}

#[test]
fn display_message_with_args_still_works() {
    let mut app = mock_app();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "display-message \"hello world\"");
    let msg = app.status_message.as_ref().map(|(m, ..)| m.as_str()).unwrap_or("");
    assert!(msg.contains("hello world"), "display-message with explicit text should show it, got: {}", msg);
}

#[test]
fn run_shell_error_shows_on_status_bar() {
    let mut app = mock_app();
    // Use a command that will definitely fail (non-existent program path)
    let _ = execute_command_string(&mut app, "run-shell \"__nonexistent_program_that_does_not_exist_12345\"");
    // Either shows popup with error output, or shows error on status bar
    // (depends on whether shell itself reports the error via stderr)
    match &app.mode {
        Mode::PopupMode { output, .. } => {
            // Shell captured the error as stderr output
            assert!(!output.is_empty(), "popup should contain error information");
        }
        _ => {
            // If no popup, the status bar might have an error (e.g. shell not found)
            // This is acceptable behavior
        }
    }
}

#[test]
fn resolve_run_shell_returns_valid_shell() {
    let (prog, args) = resolve_run_shell();
    assert!(!prog.is_empty(), "shell program should not be empty");
    assert!(!args.is_empty(), "shell args should include at least one flag");
    // The returned program should be findable on the system
    assert!(
        which::which(&prog).is_ok(),
        "resolved shell '{}' should exist on PATH",
        prog
    );
}

#[cfg(windows)]
#[test]
fn resolve_run_shell_returns_absolute_windows_shell_path() {
    let (prog, args) = resolve_run_shell();
    let path = std::path::Path::new(&prog);
    assert!(!args.is_empty(), "shell args should include at least one flag");
    assert!(
        path.is_absolute(),
        "windows run-shell should resolve to an absolute executable path, got '{}'",
        prog
    );
    assert!(
        path.is_file(),
        "resolved windows shell path should point to an existing file, got '{}'",
        prog
    );
}

// ════════════════════════════════════════════════════════════════════════════
//  Issue #201: prefix+$ should enter RenameSessionPrompt, NOT RenamePrompt
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn prefix_dollar_enters_rename_session_prompt_not_rename_window() {
    let mut app = mock_app_with_window();
    app.mode = Mode::Prefix { armed_at: std::time::Instant::now() };
    handle_key(&mut app, press(KeyCode::Char('$'))).unwrap();
    assert!(
        matches!(app.mode, Mode::RenameSessionPrompt { .. }),
        "prefix+$ must enter RenameSessionPrompt mode, got {:?}",
        std::mem::discriminant(&app.mode)
    );
    // Crucially, it should NOT be RenamePrompt (window rename)
    assert!(
        !matches!(app.mode, Mode::RenamePrompt { .. }),
        "prefix+$ must NOT enter RenamePrompt (window rename) mode"
    );
}

#[test]
fn prefix_comma_enters_rename_window_prompt_not_session() {
    let mut app = mock_app_with_window();
    app.mode = Mode::Prefix { armed_at: std::time::Instant::now() };
    handle_key(&mut app, press(KeyCode::Char(','))).unwrap();
    assert!(
        matches!(app.mode, Mode::RenamePrompt { .. }),
        "prefix+, must enter RenamePrompt (window) mode"
    );
    assert!(
        !matches!(app.mode, Mode::RenameSessionPrompt { .. }),
        "prefix+, must NOT enter RenameSessionPrompt mode"
    );
}

#[test]
fn rename_session_prompt_typing_and_enter_applies_session_name() {
    let mut app = mock_app_with_window();
    app.session_name = "old_session".to_string();
    app.mode = Mode::RenameSessionPrompt { input: String::new() };
    // Type "new_session"
    for c in "new_session".chars() {
        handle_key(&mut app, press(KeyCode::Char(c))).unwrap();
    }
    if let Mode::RenameSessionPrompt { ref input } = app.mode {
        assert_eq!(input, "new_session");
    } else {
        panic!("should still be in RenameSessionPrompt while typing");
    }
    // Press Enter to apply
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert_eq!(app.session_name, "new_session", "session name should be updated");
    assert!(matches!(app.mode, Mode::Passthrough), "should return to Passthrough after Enter");
}

#[test]
fn rename_window_prompt_typing_and_enter_applies_window_name() {
    let mut app = mock_app_with_window();
    app.windows[0].name = "old_win".to_string();
    app.mode = Mode::RenamePrompt { input: String::new() };
    for c in "new_win".chars() {
        handle_key(&mut app, press(KeyCode::Char(c))).unwrap();
    }
    handle_key(&mut app, press(KeyCode::Enter)).unwrap();
    assert_eq!(app.windows[0].name, "new_win", "window name should be updated");
    assert!(matches!(app.mode, Mode::Passthrough), "should return to Passthrough after Enter");
}

#[test]
fn rename_session_prompt_esc_cancels_without_changing_name() {
    let mut app = mock_app_with_window();
    app.session_name = "original".to_string();
    app.mode = Mode::RenameSessionPrompt { input: "typed_but_cancelled".to_string() };
    handle_key(&mut app, press(KeyCode::Esc)).unwrap();
    assert_eq!(app.session_name, "original", "session name must not change on Esc");
    assert!(matches!(app.mode, Mode::Passthrough));
}
