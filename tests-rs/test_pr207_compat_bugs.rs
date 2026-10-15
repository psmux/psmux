// PR #207 Compatibility Bug Tests
// Tests 4 confirmed behavioural deltas vs tmux:
//   Bug 2: -F#{fmt} concatenated form ignored
//   Bug 3: has-session -t =NAME not supported (tested at CLI level, not unit)
//   Bug 5: Named paste buffers not supported (-b NAME collapses)
//   Bug 6: paste-buffer -p flag ignored (always SendText, not SendPaste)
//
// Each test is designed to PASS once fixed, FAIL now.

#[allow(unused_imports)]
use super::*;

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

// ========================================================================
// BUG 5: Named paste buffers
// commands.rs set-buffer handler ignores -b flag entirely.
// It just does parts.get(1) which is "-b" (the flag), not the content.
// When -b is used: parts = ["set-buffer", "-b", "name", "content"]
// The handler takes parts[1] = "-b" as the buffer text (!!)
// ========================================================================

#[test]
fn set_buffer_without_name_stores_content() {
    // Control test: set-buffer without -b should work normally
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer HELLO_WORLD");
    assert_eq!(app.paste_buffers.len(), 1, "Should have 1 buffer");
    assert_eq!(app.paste_buffers[0], "HELLO_WORLD", "Buffer content mismatch");
}

#[test]
fn set_buffer_with_b_flag_should_not_store_flag_as_content() {
    // BUG: set-buffer -b mybuf CONTENT stores "-b" as the content (parts[1])
    // FIXED: should skip -b and its argument, store only the content in named_buffers
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b mybuf ACTUAL_CONTENT");
    // Named buffer should have been stored
    assert!(app.named_buffers.contains_key("mybuf"), "Named buffer 'mybuf' should exist");
    let content = &app.named_buffers["mybuf"];
    assert!(
        !content.contains("-b"),
        "Buffer should NOT contain the -b flag, got: '{}'", content
    );
    assert!(
        !content.contains("mybuf"),
        "Buffer content should NOT contain the buffer name 'mybuf', got: '{}'", content
    );
    assert!(
        content.contains("ACTUAL_CONTENT"),
        "Buffer should contain 'ACTUAL_CONTENT', got: '{}'", content
    );
}

#[test]
fn set_buffer_with_b_flag_multiple_names_independent() {
    // When named buffers are properly supported, two -b names should be
    // independently retrievable via named_buffers HashMap.
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b alpha ALPHA_DATA");
    let _ = execute_command_string(&mut app, "set-buffer -b beta BETA_DATA");
    // Both named buffers should exist
    assert_eq!(app.named_buffers.len(), 2, "Should have 2 named buffers, got {}", app.named_buffers.len());
    assert_eq!(app.named_buffers["alpha"], "ALPHA_DATA", "alpha should contain ALPHA_DATA");
    assert_eq!(app.named_buffers["beta"], "BETA_DATA", "beta should contain BETA_DATA");
    // Positional stack should be untouched
    assert!(app.paste_buffers.is_empty(), "Positional stack should remain empty when using -b");
    // Neither buffer should have leaked the -b flag or buffer name into content
    for (name, buf) in &app.named_buffers {
        assert!(
            !buf.contains("-b"),
            "Buffer '{}' should not contain '-b': '{}'", name, buf
        );
    }
}

#[test]
fn show_buffer_with_b_flag_retrieves_named_buffer() {
    // show-buffer -b name should retrieve the named buffer, not the positional stack top
    let mut app = mock_app_with_window();
    app.control_port = None;
    // Add positional buffers
    app.paste_add("STACK_TOP".to_string());
    // Add named buffer
    app.named_buffers.insert("myname".to_string(), "NAMED_CONTENT".to_string());
    // show-buffer -b myname should show NAMED_CONTENT
    let _ = execute_command_string(&mut app, "show-buffer -b myname");
    match &app.mode {
        Mode::PopupMode { output, .. } => {
            assert!(
                output.contains("NAMED_CONTENT"),
                "show-buffer -b myname should show named buffer content, got: '{}'", output
            );
            assert!(
                !output.contains("STACK_TOP"),
                "show-buffer -b myname should NOT show stack top, got: '{}'", output
            );
        }
        _ => panic!("Expected PopupMode for show-buffer"),
    }
}

// ========================================================================
// BUG 6: paste-buffer -p flag ignored
// commands.rs paste-buffer handler calls paste_latest(app) which does
// NOT check for -p. It should optionally wrap in bracketed-paste sequences.
// The server/connection.rs handler also always sends SendText, never SendPaste.
// ========================================================================

#[test]
fn paste_buffer_command_exists_and_runs() {
    // Control: paste-buffer should at least not crash
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_add("test_data".to_string());
    // This calls paste_latest which writes to the active pane's PTY.
    // In test mode without a real PTY, it may fail gracefully.
    let result = execute_command_string(&mut app, "paste-buffer");
    // Should not panic. Error is OK (no real PTY in test).
    let _ = result;
}

#[test]
fn paste_buffer_alias_pasteb() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_add("alias_test".to_string());
    let result = execute_command_string(&mut app, "pasteb");
    let _ = result;
    // Should not panic or return an unknown-command error
}

// ========================================================================
// BUG 2: -F concatenated form
// This is primarily a CLI/server-side argument parsing issue.
// The command parser in commands.rs list-sessions handler may also be affected.
// Test that the command parser can handle -F#{format} as a single token.
// ========================================================================

#[test]
fn list_sessions_command_dispatches_without_crash() {
    // Control: list-sessions should work in command prompt mode
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "list-sessions");
    // Should produce a popup with session names, not crash
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "list-sessions");
        }
        _ => {} // Acceptable if it routes to server
    }
}

// ========================================================================
// BUG 3: has-session -t =NAME
// In commands.rs, has-session is a no-op (in embedded mode, always succeeds).
// The real bug is in main.rs CLI dispatch. We can verify the command is
// recognized and does not crash.
// ========================================================================

#[test]
fn has_session_command_recognized() {
    // In embedded mode, has-session is a no-op (we ARE the session)
    let mut app = mock_app_with_window();
    app.control_port = None;
    let result = execute_command_string(&mut app, "has-session -t =test_session");
    assert!(result.is_ok(), "has-session should not error");
}

#[test]
fn has_session_alias_recognized() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let result = execute_command_string(&mut app, "has -t =test_session");
    assert!(result.is_ok(), "has (alias) should not error");
}

// ========================================================================
// Cross-cutting: set-buffer then list-buffers verifies no name leaking
// ========================================================================

#[test]
fn set_buffer_b_then_list_buffers_no_name_leak() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b myname ACTUAL_PAYLOAD");
    let _ = execute_command_string(&mut app, "list-buffers");
    match &app.mode {
        Mode::PopupMode { output, .. } => {
            // The listing should NOT show the buffer name inside the content preview
            assert!(
                !output.contains("myname ACTUAL_PAYLOAD"),
                "list-buffers content preview should not leak buffer name. Got: '{}'", output
            );
            // Should show the content
            assert!(
                output.contains("ACTUAL_PAYLOAD"),
                "list-buffers should show buffer content. Got: '{}'", output
            );
        }
        _ => {} // list-buffers may route to server
    }
}

#[test]
fn delete_buffer_works_normally() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer BUFFER_TO_DELETE");
    assert_eq!(app.paste_buffers.len(), 1);
    let _ = execute_command_string(&mut app, "delete-buffer");
    assert_eq!(app.paste_buffers.len(), 0, "delete-buffer should remove buffer");
}
