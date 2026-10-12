// Issue #192: Sequential operator \; not respected when chaining commands
// Tests that commands separated by \; or ; are all executed, not just the first one.

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

// ─── split_chained_commands_pub tests ───────────────────────────────────────

#[test]
fn split_chained_commands_backslash_semicolon() {
    let result = crate::config::split_chained_commands_pub(r"source-file foo \; display bar");
    assert_eq!(result, vec!["source-file foo", "display bar"]);
}

#[test]
fn split_chained_commands_bare_semicolon() {
    let result = crate::config::split_chained_commands_pub("source-file foo ; display bar");
    assert_eq!(result, vec!["source-file foo", "display bar"]);
}

#[test]
fn split_chained_commands_three_commands() {
    let result = crate::config::split_chained_commands_pub(
        r"new-window \; split-window \; select-pane -D"
    );
    assert_eq!(result, vec!["new-window", "split-window", "select-pane -D"]);
}

#[test]
fn split_chained_commands_single_command() {
    let result = crate::config::split_chained_commands_pub("display hello");
    assert_eq!(result, vec!["display hello"]);
}

// ─── execute_command_string chaining tests ──────────────────────────────────

#[test]
fn execute_command_string_chained_sets_both_options() {
    // Issue #192: chained commands via \; should both execute when
    // passed through execute_command_string (embedded mode path)
    let mut app = mock_app_with_window();

    // Chain two set-option commands with \;
    let chained = r#"set-option status-style "bg=red" \; set-option status-left "TEST""#;
    execute_command_string(&mut app, chained).unwrap();

    // Both options should be set
    assert_eq!(app.status_style, "bg=red",
        "First chained command (set status-style) should have executed");
    assert_eq!(app.status_left, "TEST",
        "Second chained command (set status-left) should have executed");
}

#[test]
fn execute_command_string_chained_rename_then_option() {
    let mut app = mock_app_with_window();
    app.windows[0].name = "old_name".to_string();

    // Chain rename-window and set-option
    let chained = r"rename-window new_name \; set-option status-left CHANGED";
    execute_command_string(&mut app, chained).unwrap();

    assert_eq!(app.windows[0].name, "new_name",
        "First chained command (rename-window) should have executed");
    assert_eq!(app.status_left, "CHANGED",
        "Second chained command (set-option) should have executed");
}

// ─── execute_command_prompt chaining tests ──────────────────────────────────

#[test]
fn execute_command_prompt_chained_commands() {
    // The command prompt should also split on \;
    let mut app = mock_app_with_window();
    app.mode = Mode::CommandPrompt {
        input: r#"set-option status-style "bg=blue" \; set-option status-left "PROMPT""#.to_string(),
        cursor: 0,
    };

    execute_command_prompt(&mut app).unwrap();

    assert_eq!(app.status_style, "bg=blue",
        "First chained command from prompt should execute");
    assert_eq!(app.status_left, "PROMPT",
        "Second chained command from prompt should execute");
}

// ─── select-pane inside a chain ─────────────────────────────────────────────
// A select-pane that has nothing to do on this route (a title, an input flag,
// a target the one-shot path resolves) returns early from its own command; the
// commands after it in the list must still run (bindings, hooks, the prompt).

#[test]
fn execute_command_string_select_pane_does_not_end_the_chain() {
    for chained in [
        r"select-pane -T ttl \; set-option status-left AFTER",
        r"select-pane -t %1 ; set-option status-left AFTER",
        r"set-option status-style bg=red \; select-pane -d \; set-option status-left AFTER",
    ] {
        let mut app = mock_app_with_window();
        execute_command_string(&mut app, chained).unwrap();
        assert_eq!(app.status_left, "AFTER", "command after select-pane must run: {chained}");
    }
}
