// Issue #227: pane-died / pane-exited hooks with remain-on-exit
//
// When remain-on-exit is on, prune_exited() marks panes as dead but keeps
// them in the tree. The old hook-firing logic only checked whether the tree
// leaf count decreased (any_pruned), so hooks never fired in the remain-on-exit
// case. The fix adds a newly_dead_count return from prune_exited() and a
// separate any_newly_dead flag in reap_children().
//
// These tests verify:
//   1. fire_hooks dispatches registered hook commands
//   2. set-hook registers hooks correctly (set, append, unset)
//   3. fire_hooks is a no-op when no hooks are registered
//   4. fire_hooks with multiple hooks fires all of them
//   5. set-hook -u removes a hook
//   6. set-hook -a appends to existing hooks
//   7. Chained hook commands work
//   8. fire_hooks via command prompt path
//   9. Hook with display-message sets status
//  10. Multiple hook events (pane-died + pane-exited) coexist

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

fn mock_app_with_windows(names: &[&str]) -> AppState {
    let mut app = mock_app();
    for (i, name) in names.iter().enumerate() {
        app.windows.push(make_window(name, i));
    }
    app
}

// ============================================================================
// Test 1: set-hook registers a hook and fire_hooks dispatches it
// ============================================================================
#[test]
fn set_hook_registers_hook() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-died \"set -g @hook-marker yes\"").unwrap();
    assert!(app.hooks.contains_key("pane-died"), "pane-died hook should be registered");
    let cmds = app.hooks.get("pane-died").unwrap();
    assert_eq!(cmds.len(), 1, "Should have exactly one command");
    assert!(cmds[0].contains("set -g @hook-marker yes"), "Command should match, got: {}", cmds[0]);
}

// ============================================================================
// Test 2: fire_hooks executes registered hook commands
// ============================================================================
#[test]
fn fire_hooks_executes_registered_commands() {
    let mut app = mock_app_with_window();
    // Register a hook that sets a user option
    app.hooks.insert("pane-died".to_string(), vec!["set -g @pane-died-fired yes".to_string()]);
    
    // Fire the hook
    fire_hooks(&mut app, "pane-died");
    
    // Verify the hook command executed: @pane-died-fired should be set
    let val = app.user_options.get("@pane-died-fired");
    assert_eq!(val.map(|s| s.as_str()), Some("yes"), "Hook command should have set user option");
}

// ============================================================================
// Test 3: fire_hooks is a no-op when no hooks are registered
// ============================================================================
#[test]
fn fire_hooks_noop_when_no_hooks() {
    let mut app = mock_app_with_window();
    // No hooks registered; should not panic or error
    fire_hooks(&mut app, "pane-died");
    fire_hooks(&mut app, "pane-exited");
    fire_hooks(&mut app, "nonexistent-event");
    // If we got here without panic, the test passes
}

// ============================================================================
// Test 4: fire_hooks with multiple commands fires all of them
// ============================================================================
#[test]
fn fire_hooks_fires_all_commands() {
    let mut app = mock_app_with_window();
    app.hooks.insert("pane-died".to_string(), vec![
        "set -g @hook-first yes".to_string(),
        "set -g @hook-second yes".to_string(),
    ]);
    
    fire_hooks(&mut app, "pane-died");
    
    assert_eq!(app.user_options.get("@hook-first").map(|s| s.as_str()), Some("yes"),
        "First hook command should execute");
    assert_eq!(app.user_options.get("@hook-second").map(|s| s.as_str()), Some("yes"),
        "Second hook command should execute");
}

// ============================================================================
// Test 5: set-hook -u removes a hook
// ============================================================================
#[test]
fn set_hook_unset_removes_hook() {
    let mut app = mock_app_with_window();
    // Register then unset
    execute_command_string(&mut app, "set-hook pane-died \"display-message test\"").unwrap();
    assert!(app.hooks.contains_key("pane-died"), "Hook should exist before unset");
    
    execute_command_string(&mut app, "set-hook -u pane-died").unwrap();
    assert!(!app.hooks.contains_key("pane-died"), "Hook should be removed after -u");
}

// ============================================================================
// Test 6: set-hook -a appends to existing hooks
// ============================================================================
#[test]
fn set_hook_append_adds_to_existing() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-died \"set -g @first yes\"").unwrap();
    execute_command_string(&mut app, "set-hook -a pane-died \"set -g @second yes\"").unwrap();
    
    let cmds = app.hooks.get("pane-died").unwrap();
    assert_eq!(cmds.len(), 2, "Should have two commands after append");
}

// ============================================================================
// Test 7: set-hook without -a replaces existing hooks
// ============================================================================
#[test]
fn set_hook_replaces_without_append() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-died \"set -g @old yes\"").unwrap();
    execute_command_string(&mut app, "set-hook pane-died \"set -g @new yes\"").unwrap();
    
    let cmds = app.hooks.get("pane-died").unwrap();
    assert_eq!(cmds.len(), 1, "Should have only one command after replacement");
    assert!(cmds[0].contains("@new"), "Should be the new command, got: {}", cmds[0]);
}

// ============================================================================
// Test 8: Both pane-died and pane-exited hooks coexist independently
// ============================================================================
#[test]
fn pane_died_and_pane_exited_hooks_coexist() {
    let mut app = mock_app_with_window();
    app.hooks.insert("pane-died".to_string(), vec!["set -g @died-marker yes".to_string()]);
    app.hooks.insert("pane-exited".to_string(), vec!["set -g @exited-marker yes".to_string()]);
    
    // Fire both (as the server does after reap_children)
    fire_hooks(&mut app, "pane-died");
    fire_hooks(&mut app, "pane-exited");
    
    assert_eq!(app.user_options.get("@died-marker").map(|s| s.as_str()), Some("yes"),
        "pane-died hook should fire");
    assert_eq!(app.user_options.get("@exited-marker").map(|s| s.as_str()), Some("yes"),
        "pane-exited hook should fire");
}

// ============================================================================
// Test 9: fire_hooks only fires for the named event, not others
// ============================================================================
#[test]
fn fire_hooks_only_fires_named_event() {
    let mut app = mock_app_with_window();
    app.hooks.insert("pane-died".to_string(), vec!["set -g @died yes".to_string()]);
    app.hooks.insert("pane-exited".to_string(), vec!["set -g @exited yes".to_string()]);
    
    // Only fire pane-died
    fire_hooks(&mut app, "pane-died");
    
    assert_eq!(app.user_options.get("@died").map(|s| s.as_str()), Some("yes"),
        "pane-died should fire");
    assert!(app.user_options.get("@exited").is_none(),
        "pane-exited should NOT fire when only pane-died is triggered");
}

// ============================================================================
// Test 10: set-hook via command prompt path
// ============================================================================
#[test]
fn set_hook_from_command_prompt() {
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt {
        input: "set-hook pane-died \"set -g @prompt-hook yes\"".to_string(),
        cursor: 0,
    };
    execute_command_prompt(&mut app).unwrap();
    assert!(app.hooks.contains_key("pane-died"),
        "set-hook from command prompt should register the hook");
}

// ============================================================================
// Test 11: Hook that sets a user option proves command execution works
// ============================================================================
#[test]
fn hook_set_user_option_proves_execution() {
    let mut app = mock_app_with_window();
    // Simulate what a real user would do: set a hook, then fire it
    // Note: no surrounding quotes on the command; set-hook stores everything
    // after the hook name verbatim, and fire_hooks passes it to execute_command_string.
    execute_command_string(&mut app, "set-hook pane-died set -g @hook-proof confirmed").unwrap();
    
    // Verify hook is registered
    assert!(app.hooks.contains_key("pane-died"));
    
    // Fire the hook (simulating what reap_children triggers)
    fire_hooks(&mut app, "pane-died");
    
    // The undeniable proof: the user option was set by the hook
    let val = app.user_options.get("@hook-proof");
    assert_eq!(val.map(|s| s.as_str()), Some("confirmed"),
        "Hook command must have executed and set @hook-proof=confirmed");
}

// ============================================================================
// Test 12: Multiple events with multiple commands each
// ============================================================================
#[test]
fn multiple_events_multiple_commands() {
    let mut app = mock_app_with_window();
    app.hooks.insert("pane-died".to_string(), vec![
        "set -g @d1 yes".to_string(),
        "set -g @d2 yes".to_string(),
    ]);
    app.hooks.insert("pane-exited".to_string(), vec![
        "set -g @e1 yes".to_string(),
        "set -g @e2 yes".to_string(),
    ]);
    
    fire_hooks(&mut app, "pane-died");
    fire_hooks(&mut app, "pane-exited");
    
    for key in &["@d1", "@d2", "@e1", "@e2"] {
        assert_eq!(app.user_options.get(*key).map(|s| s.as_str()), Some("yes"),
            "User option {} should be set by hook", key);
    }
}

// ============================================================================
// Test 13: Verify show-hooks output includes registered hooks
// ============================================================================
#[test]
fn show_hooks_includes_pane_died() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-died \"display-message test\"").unwrap();
    // pane-died is a pane hook: tmux lists it under -w (window and pane hooks).
    let output = generate_show_hooks(&app, &["-w"]);
    assert!(output.contains("pane-died"), "show-hooks should list pane-died, got: {}", output);
}

// ============================================================================
// Test 14: set-hook -gu (global unset) removes hook
// ============================================================================
#[test]
fn set_hook_gu_removes_hook() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-exited \"set -g @exited yes\"").unwrap();
    assert!(app.hooks.contains_key("pane-exited"));
    
    execute_command_string(&mut app, "set-hook -gu pane-exited").unwrap();
    assert!(!app.hooks.contains_key("pane-exited"),
        "-gu should remove pane-exited hook");
}

// ============================================================================
// Test 15: set-hook -ga (global append) appends hook command
// ============================================================================
#[test]
fn set_hook_ga_appends_hook() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-hook pane-died set -g @first yes").unwrap();
    execute_command_string(&mut app, "set-hook -ga pane-died set -g @second yes").unwrap();
    
    let cmds = app.hooks.get("pane-died").unwrap();
    assert_eq!(cmds.len(), 2, "-ga should append, not replace");
    
    fire_hooks(&mut app, "pane-died");
    assert_eq!(app.user_options.get("@first").map(|s| s.as_str()), Some("yes"));
    assert_eq!(app.user_options.get("@second").map(|s| s.as_str()), Some("yes"));
}
