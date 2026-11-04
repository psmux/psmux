use super::*;
use crate::types::LayoutKind;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app
}

#[test]
fn test_literal_modifier() {
    let app = mock_app();
    assert_eq!(expand_expression("l:hello", &app, 0), "hello");
}

#[test]
fn test_trim_modifier() {
    let app = mock_app();
    let result = expand_expression("=3:session_name", &app, 0);
    assert_eq!(result, "tes");
}

#[test]
fn test_trim_negative() {
    let app = mock_app();
    let result = expand_expression("=-3:session_name", &app, 0);
    assert_eq!(result, "ion");
}

#[test]
fn test_basename() {
    let app = mock_app();
    let val = apply_modifier(&Modifier::Basename, "/usr/src/tmux", &app, 0);
    assert_eq!(val, "tmux");
}

#[test]
fn test_dirname() {
    let app = mock_app();
    let val = apply_modifier(&Modifier::Dirname, "/usr/src/tmux", &app, 0);
    assert_eq!(val, "/usr/src");
}

#[test]
fn test_pad() {
    let app = mock_app();
    let val = apply_modifier(&Modifier::Pad(10), "foo", &app, 0);
    assert_eq!(val, "foo       ");
    let val = apply_modifier(&Modifier::Pad(-10), "foo", &app, 0);
    assert_eq!(val, "       foo");
}

#[test]
fn test_substitute() {
    let app = mock_app();
    let val = apply_modifier(
        &Modifier::Substitute { pattern: "foo".into(), replacement: "bar".into(), case_insensitive: false },
        "foobar", &app, 0
    );
    assert_eq!(val, "barbar");
}

#[test]
fn test_math_add() {
    let app = mock_app();
    let val = apply_modifier(
        &Modifier::MathExpr { op: '+', floating: false, decimals: 0 },
        "3,5", &app, 0
    );
    assert_eq!(val, "8");
}

#[test]
fn test_math_float_div() {
    let app = mock_app();
    let val = apply_modifier(
        &Modifier::MathExpr { op: '/', floating: true, decimals: 4 },
        "10,3", &app, 0
    );
    assert_eq!(val, "3.3333");
}

#[test]
fn test_boolean_or() {
    let app = mock_app();
    assert_eq!(expand_expression("||:1,0", &app, 0), "1");
    assert_eq!(expand_expression("||:0,0", &app, 0), "0");
}

#[test]
fn test_boolean_and() {
    let app = mock_app();
    assert_eq!(expand_expression("&&:1,1", &app, 0), "1");
    assert_eq!(expand_expression("&&:1,0", &app, 0), "0");
}

#[test]
fn test_comparison_eq() {
    let app = mock_app();
    assert_eq!(expand_expression("==:version,version", &app, 0), "1");
}

#[test]
fn test_glob_match_fn() {
    assert!(glob_match("*foo*", "barfoobar", false));
    assert!(!glob_match("*foo*", "barbaz", false));
    assert!(glob_match("*FOO*", "barfoobar", true));
}

#[test]
fn test_quote() {
    let app = mock_app();
    let val = apply_modifier(&Modifier::Quote, "(hello)", &app, 0);
    assert_eq!(val, "\\(hello\\)");
}

// ── Window flags tests ─────────────────────────────────────────

fn mock_window(name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
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
    }
}

#[test]
fn test_window_flags_active() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.active_idx = 0;
    assert_eq!(expand_var("window_flags", &app, 0), "*");
}

#[test]
fn test_window_flags_last() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.windows.push(mock_window("win1"));
    app.active_idx = 1;
    app.last_window_idx = 0;
    assert_eq!(expand_var("window_flags", &app, 0), "-");
}

#[test]
fn test_window_flags_bell() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.bell_flag = true;
    app.windows.push(win);
    app.windows.push(mock_window("win1"));
    app.active_idx = 1;
    app.last_window_idx = 1; // same as active so "-" won't appear
    assert_eq!(expand_var("window_flags", &app, 0), "!");
}

#[test]
fn test_window_flags_silence() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.silence_flag = true;
    app.windows.push(win);
    app.windows.push(mock_window("win1"));
    app.active_idx = 1;
    app.last_window_idx = 1;
    assert_eq!(expand_var("window_flags", &app, 0), "~");
}

#[test]
fn test_window_flags_activity() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.activity_flag = true;
    app.windows.push(win);
    app.windows.push(mock_window("win1"));
    app.active_idx = 1;
    app.last_window_idx = 1;
    assert_eq!(expand_var("window_flags", &app, 0), "#");
}

#[test]
fn test_window_flags_bell_and_activity() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.bell_flag = true;
    win.activity_flag = true;
    app.windows.push(win);
    app.windows.push(mock_window("win1"));
    app.active_idx = 1;
    app.last_window_idx = 1;
    assert_eq!(expand_var("window_flags", &app, 0), "#!");
}

#[test]
fn test_window_activity_flag_var() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.activity_flag = true;
    app.windows.push(win);
    assert_eq!(expand_var("window_activity_flag", &app, 0), "1");
}

#[test]
fn test_window_activity_flag_var_off() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("window_activity_flag", &app, 0), "0");
}

// ── AppState defaults tests ─────────────────────────────────────

#[test]
fn test_appstate_defaults_allow_rename() {
    let app = mock_app();
    assert!(app.allow_rename);
}

// ── Per-window zoom flag tests (issue #125 follow-up) ──────────────

#[test]
fn test_window_zoomed_flag_default_no_zoom() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("window_zoomed_flag", &app, 0), "0");
}

#[test]
fn test_window_zoomed_flag_set_on_zoomed_window() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    app.windows.push(win);
    app.active_idx = 0;
    // The zoomed window should report flag=1
    assert_eq!(expand_var("window_zoomed_flag", &app, 0), "1");
}

#[test]
fn test_window_zoomed_flag_per_window_not_global() {
    // Simulates: zoom in window 0, check that window 1 does NOT show zoomed
    let mut app = mock_app();
    let mut win0 = mock_window("win0");
    win0.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    app.windows.push(win0);
    app.windows.push(mock_window("win1"));
    app.active_idx = 0;
    // Window 0 is zoomed → flag=1
    assert_eq!(expand_var("window_zoomed_flag", &app, 0), "1");
    // Window 1 is NOT zoomed → flag=0
    assert_eq!(expand_var("window_zoomed_flag", &app, 1), "0");
}

#[test]
fn test_window_zoomed_flag_stays_on_original_window_after_switch() {
    // Simulates: zoom in window 0, then switch to window 1
    // Window 0 should still show zoomed, window 1 should not
    let mut app = mock_app();
    let mut win0 = mock_window("win0");
    win0.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    app.windows.push(win0);
    app.windows.push(mock_window("win1"));
    // Switch to window 1
    app.active_idx = 1;
    // Window 0 still zoomed → flag=1 (even though it's not the active window)
    assert_eq!(expand_var("window_zoomed_flag", &app, 0), "1");
    // Window 1 is NOT zoomed → flag=0
    assert_eq!(expand_var("window_zoomed_flag", &app, 1), "0");
}

#[test]
fn test_window_zoomed_flag_multiple_windows_zoomed() {
    // In tmux, multiple windows can each have a zoomed pane independently
    let mut app = mock_app();
    let mut win0 = mock_window("win0");
    win0.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    let mut win1 = mock_window("win1");
    win1.zoom_saved = Some(vec![(vec![0], vec![30, 70])]);
    app.windows.push(win0);
    app.windows.push(win1);
    app.windows.push(mock_window("win2"));
    app.active_idx = 2;
    // Both window 0 and 1 are zoomed
    assert_eq!(expand_var("window_zoomed_flag", &app, 0), "1");
    assert_eq!(expand_var("window_zoomed_flag", &app, 1), "1");
    // Window 2 is not zoomed
    assert_eq!(expand_var("window_zoomed_flag", &app, 2), "0");
}

#[test]
fn test_window_flags_include_z_when_zoomed() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    app.windows.push(win);
    app.active_idx = 0;
    let flags = expand_var("window_flags", &app, 0);
    assert!(flags.contains('Z'), "window_flags should contain Z when zoomed, got: {}", flags);
    assert!(flags.contains('*'), "window_flags should contain * for active window, got: {}", flags);
}

#[test]
fn test_window_flags_no_z_when_not_zoomed() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.active_idx = 0;
    let flags = expand_var("window_flags", &app, 0);
    assert!(!flags.contains('Z'), "window_flags should not contain Z when not zoomed, got: {}", flags);
}

#[test]
fn test_conditional_window_zoomed_flag_per_window() {
    let mut app = mock_app();
    let mut win0 = mock_window("win0");
    win0.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    app.windows.push(win0);
    app.windows.push(mock_window("win1"));
    app.active_idx = 1; // active is window 1, but window 0 is zoomed
    // Conditional format should show ZOOMED for window 0
    let result0 = expand_format_for_window("#{?window_zoomed_flag,ZOOMED,normal}", &app, 0);
    assert_eq!(result0, "ZOOMED");
    // Conditional format should show normal for window 1
    let result1 = expand_format_for_window("#{?window_zoomed_flag,ZOOMED,normal}", &app, 1);
    assert_eq!(result1, "normal");
}

#[test]
fn test_appstate_defaults_bell_action() {
    let app = mock_app();
    assert_eq!(app.bell_action, "any");
}

#[test]
fn test_appstate_defaults_bell_forward() {
    let app = mock_app();
    assert!(!app.bell_forward, "bell_forward must default to false");
}

#[test]
fn test_appstate_defaults_activity_action() {
    let app = mock_app();
    assert_eq!(app.activity_action, "other");
}

#[test]
fn test_appstate_defaults_silence_action() {
    let app = mock_app();
    assert_eq!(app.silence_action, "other");
}

#[test]
fn test_appstate_defaults_monitor_silence() {
    let app = mock_app();
    assert_eq!(app.monitor_silence, 0);
}

#[test]
fn test_appstate_defaults_update_environment() {
    let app = mock_app();
    assert!(app.update_environment.values().contains(&"DISPLAY".to_string()));
    assert!(app.update_environment.values().contains(&"SSH_AUTH_SOCK".to_string()));
    assert!(app.update_environment.values().contains(&"SSH_AGENT_PID".to_string()));
}

// ── Session group format variable tests ─────────────────────────

#[test]
fn test_session_group_empty_by_default() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("session_group", &app, 0), "");
}

#[test]
fn test_session_group_returns_group_name() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("mygroup".to_string());
    assert_eq!(expand_var("session_group", &app, 0), "mygroup");
}

#[test]
fn test_session_group_list_returns_group_name() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("mygroup".to_string());
    assert_eq!(expand_var("session_group_list", &app, 0), "mygroup");
}

#[test]
fn test_session_grouped_false_by_default() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("session_grouped", &app, 0), "0");
}

#[test]
fn test_session_grouped_true_when_in_group() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("grp".to_string());
    assert_eq!(expand_var("session_grouped", &app, 0), "1");
}

#[test]
fn test_session_group_attached_when_grouped_and_attached() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("grp".to_string());
    app.attached_clients = 1;
    assert_eq!(expand_var("session_group_attached", &app, 0), "1");
}

#[test]
fn test_session_group_attached_zero_when_not_grouped() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.attached_clients = 1;
    assert_eq!(expand_var("session_group_attached", &app, 0), "0");
}

#[test]
fn test_session_group_attached_zero_when_no_clients() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("grp".to_string());
    app.attached_clients = 0;
    assert_eq!(expand_var("session_group_attached", &app, 0), "0");
}

#[test]
fn test_session_group_size_when_grouped() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.session_group = Some("grp".to_string());
    assert_eq!(expand_var("session_group_size", &app, 0), "1");
}

#[test]
fn test_session_group_size_zero_when_not_grouped() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("session_group_size", &app, 0), "0");
}

// ── Window linked format variable tests ─────────────────────────

#[test]
fn test_window_linked_false_by_default() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("window_linked", &app, 0), "0");
}

#[test]
fn test_window_linked_true_when_linked() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.linked_from = Some(42);
    app.windows.push(win);
    assert_eq!(expand_var("window_linked", &app, 0), "1");
}

#[test]
fn test_window_linked_sessions_mirrors_linked() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.linked_from = Some(5);
    app.windows.push(win);
    assert_eq!(expand_var("window_linked_sessions", &app, 0), "1");
}

#[test]
fn test_window_linked_sessions_list_empty() {
    let mut app = mock_app();
    let mut win = mock_window("win0");
    win.linked_from = Some(5);
    app.windows.push(win);
    assert_eq!(expand_var("window_linked_sessions_list", &app, 0), "");
}

// ── Pane fg/bg default tests ────────────────────────────────────
// Without a real PTY pane, pane_fg and pane_bg should return "default"

#[test]
fn test_pane_fg_default_without_real_pane() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    // No panes in the split node, so target_pane() returns None -> "default"
    assert_eq!(expand_var("pane_fg", &app, 0), "default");
}

#[test]
fn test_pane_bg_default_without_real_pane() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("pane_bg", &app, 0), "default");
}

// ── Mouse position format variable tests ────────────────────────

// tmux's format_cb_mouse_x / _y return nothing without a mouse event (#988).
#[test]
fn test_mouse_x_empty_without_mouse_event() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("mouse_x", &app, 0), "");
}

#[test]
fn test_mouse_y_empty_without_mouse_event() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    assert_eq!(expand_var("mouse_y", &app, 0), "");
}

#[test]
fn test_mouse_x_tracks_last_position() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.mouse_fmt = Some(crate::types::MouseFormat { x: 42, ..Default::default() });
    assert_eq!(expand_var("mouse_x", &app, 0), "42");
}

#[test]
fn test_mouse_y_tracks_last_position() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.mouse_fmt = Some(crate::types::MouseFormat { y: 17, ..Default::default() });
    assert_eq!(expand_var("mouse_y", &app, 0), "17");
}

// ── Session many_attached format variable tests ─────────────────

#[test]
fn test_session_many_attached_zero_single_client() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.attached_clients = 1;
    assert_eq!(expand_var("session_many_attached", &app, 0), "0");
}

#[test]
fn test_session_many_attached_one_when_multiple() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.attached_clients = 3;
    assert_eq!(expand_var("session_many_attached", &app, 0), "1");
}

// ── Session format var (alias for session_many_attached) ────────

#[test]
fn test_session_format_alias() {
    let mut app = mock_app();
    app.windows.push(mock_window("win0"));
    app.attached_clients = 2;
    assert_eq!(expand_var("session_format", &app, 0), "1");
}

// ── Issue #164: expand_format must preserve #[style] directives ──

#[test]
fn test_expand_format_preserves_style_directives() {
    let app = mock_app();
    // #[fg=red] should pass through expand_format unchanged
    let result = expand_format("#[fg=red]Custom Line 2", &app);
    assert_eq!(result, "#[fg=red]Custom Line 2",
        "expand_format must not eat #[fg=red] directive");
}

#[test]
fn test_expand_format_preserves_align_directive() {
    let app = mock_app();
    let result = expand_format("#[align=left]Custom Line 1", &app);
    assert_eq!(result, "#[align=left]Custom Line 1",
        "expand_format must not eat #[align=left] directive");
}

#[test]
fn test_expand_format_mixed_variables_and_styles() {
    let mut app = mock_app();
    app.session_name = "main".to_string();
    // Mix of style directive and variable expansion
    let result = expand_format("#[fg=red]session: #S", &app);
    assert_eq!(result, "#[fg=red]session: main",
        "Style directives preserved and variables expanded");
}

#[test]
fn test_expand_format_multiple_style_blocks() {
    let app = mock_app();
    let result = expand_format("#[fg=red]Hello #[fg=green]World", &app);
    assert_eq!(result, "#[fg=red]Hello #[fg=green]World",
        "Multiple style blocks must all be preserved");
}

#[test]
fn test_expand_format_complex_style() {
    let app = mock_app();
    let result = expand_format("#[fg=yellow,bg=blue,bold]Styled Text", &app);
    assert_eq!(result, "#[fg=yellow,bg=blue,bold]Styled Text",
        "Complex style directives must be preserved");
}

#[test]
fn test_session_path_is_server_cwd() {
    // tmux: #{session_path} is the working directory of the session. psmux has
    // no per-session cwd, so it resolves to the server's current directory --
    // the same source #{pane_current_path} falls back to. It must NOT be the
    // user's home directory.
    let app = mock_app();
    let expected = std::env::current_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();
    assert_eq!(expand_format("#{session_path}", &app), expected,
        "#{{session_path}} must resolve to the session (server) working directory");
}
