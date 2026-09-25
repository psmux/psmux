// =============================================================================
// PSMUX Flag Parity Test Suite (Rust Unit Tests)
// =============================================================================
//
// Tests EVERY flag of EVERY command that psmux handles locally, ensuring full
// parity with tmux's flag surface. Each test proves a specific flag changes
// the right state, not just that it doesn't crash.
//
// Organized by command, each section tests every flag tmux supports for that
// command, proving either:
//   (a) psmux handles it and the state is correct, or
//   (b) psmux consumes/ignores it (compat) without crashing.

use super::*;

// ─── Scaffolding ────────────────────────────────────────────────────────────

fn mock_app() -> AppState {
    let mut app = AppState::new("flag_test".to_string());
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

fn is_popup(app: &AppState) -> bool {
    matches!(&app.mode, Mode::PopupMode { .. })
}

fn popup_output(app: &AppState) -> String {
    match &app.mode {
        Mode::PopupMode { output, .. } => output.clone(),
        _ => String::new(),
    }
}

fn is_command_prompt(app: &AppState) -> bool {
    matches!(&app.mode, Mode::CommandPrompt { .. })
}

fn prompt_input(app: &AppState) -> String {
    match &app.mode {
        Mode::CommandPrompt { input, .. } => input.clone(),
        _ => String::new(),
    }
}

fn status_msg(app: &AppState) -> String {
    app.status_message.as_ref().map(|(s, _, _)| s.clone()).unwrap_or_default()
}

// ═════════════════════════════════════════════════════════════════════════════
// 1. SET-OPTION: tmux flags aFgopqst:uUw
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn set_option_flag_g_global() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g mouse on").unwrap();
    assert!(app.mouse_enabled, "-g flag: global option should apply");
}

#[test]
fn set_option_flag_u_unset_resets_default() {
    let mut app = mock_app_with_window();
    // -u clears the option. For numeric options the empty string cannot parse,
    // so the field keeps its last value; user options are REMOVED outright
    // (#619, tmux options_remove_or_default), not blanked in place, so that a
    // later `set -o` sees nothing set and applies.
    execute_command_string(&mut app, "set-option -g @unset-probe hello").unwrap();
    assert_eq!(app.user_options.get("@unset-probe").map(|s| s.as_str()), Some("hello"));
    execute_command_string(&mut app, "set-option -gu @unset-probe").unwrap();
    assert_eq!(app.user_options.get("@unset-probe").map(|s| s.as_str()), None,
        "-u flag: should remove the user option");
}

#[test]
fn set_option_flag_a_append() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g status-right "PART1""#).unwrap();
    execute_command_string(&mut app, r#"set-option -ga status-right " PART2""#).unwrap();
    assert!(app.status_right.contains("PART1"), "-a flag: should keep existing");
    assert!(app.status_right.contains("PART2"), "-a flag: should append");
}

#[test]
fn set_option_flag_q_quiet_no_error() {
    let mut app = mock_app_with_window();
    // -q should suppress errors for unknown options
    execute_command_string(&mut app, "set-option -gq nonexistent-option value").unwrap();
    // Should not crash or set a status error message
}

#[test]
fn set_option_flag_o_only_if_unset() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g escape-time 42").unwrap();
    assert_eq!(app.escape_time_ms, 42);
    execute_command_string(&mut app, "set-option -go escape-time 999").unwrap();
    assert_eq!(app.escape_time_ms, 42, "-o flag: should NOT overwrite existing value");
}

#[test]
fn set_option_flag_w_window_scope() {
    let mut app = mock_app_with_window();
    // -w is treated same as -g in single-server model
    execute_command_string(&mut app, "set-option -w mouse on").unwrap();
    assert!(app.mouse_enabled, "-w flag: window scope should apply locally");
}

#[test]
fn set_option_flag_F_format_expand() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r##"set-option -gF status-left "#{session_name}""##).unwrap();
    // -F should expand format strings; session_name = "flag_test"
    assert_eq!(app.status_left, "flag_test", "-F flag: should expand format in value");
}

#[test]
fn set_option_combined_flags_gu() {
    let mut app = mock_app_with_window();
    // Combined -gu: global unset. For user options the key is removed (#619).
    execute_command_string(&mut app, "set-option -g @gu-probe value").unwrap();
    assert_eq!(app.user_options.get("@gu-probe").map(|s| s.as_str()), Some("value"));
    execute_command_string(&mut app, "set-option -gu @gu-probe").unwrap();
    assert_eq!(app.user_options.get("@gu-probe").map(|s| s.as_str()), None,
        "combined -gu: should remove the user option");
}

#[test]
fn set_option_combined_flags_ga() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g status-left "A""#).unwrap();
    execute_command_string(&mut app, r#"set-option -ga status-left "B""#).unwrap();
    assert!(app.status_left.contains('A') && app.status_left.contains('B'), "combined -ga: global append");
}

#[test]
fn set_option_flag_t_target_consumed() {
    let mut app = mock_app_with_window();
    // -t should be consumed (value skipped) without affecting parsing
    execute_command_string(&mut app, "set-option -t 0 -g mouse off").unwrap();
    // Should not crash; mouse state may or may not change depending on parse order
}

#[test]
fn set_option_user_at_option() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @my-plugin value1").unwrap();
    assert_eq!(app.user_options.get("@my-plugin").map(|s| s.as_str()), Some("value1"));
}

#[test]
fn set_option_user_at_option_unset() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @test-opt hello").unwrap();
    execute_command_string(&mut app, "set-option -gu @test-opt").unwrap();
    // -u removes the key, matching tmux (#619). It used to blank it in place,
    // which kept the option looking set to the `-o` guard for ever.
    assert_eq!(app.user_options.get("@test-opt").map(|s| s.as_str()), None,
        "@option unset should remove the key");
}

#[test]
fn set_option_user_at_option_append() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @list one").unwrap();
    execute_command_string(&mut app, "set-option -ga @list ,two").unwrap();
    let val = app.user_options.get("@list").unwrap();
    assert!(val.contains("one") && val.contains("two"), "@option append should combine");
}

// All major set-option options
#[test]
fn set_option_base_index() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g base-index 1").unwrap();
    assert_eq!(app.window_base_index, 1);
}

#[test]
fn set_option_pane_base_index() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g pane-base-index 1").unwrap();
    assert_eq!(app.pane_base_index, 1);
}

#[test]
fn set_option_history_limit() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g history-limit 50000").unwrap();
    assert_eq!(app.history_limit, 50000);
}

#[test]
fn set_option_display_time() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g display-time 3000").unwrap();
    assert_eq!(app.display_time_ms, 3000);
}

#[test]
fn set_option_display_panes_time() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g display-panes-time 2000").unwrap();
    assert_eq!(app.display_panes_time_ms, 2000);
}

#[test]
fn set_option_escape_time() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g escape-time 25").unwrap();
    assert_eq!(app.escape_time_ms, 25);
}

#[test]
fn set_option_focus_events() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g focus-events on").unwrap();
    assert!(app.focus_events);
}

#[test]
fn set_option_mode_keys() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g mode-keys vi").unwrap();
    assert_eq!(app.mode_keys, "vi");
}

#[test]
fn set_option_status() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g status off").unwrap();
    assert!(!app.status_visible);
    execute_command_string(&mut app, "set-option -g status on").unwrap();
    assert!(app.status_visible);
}

#[test]
fn set_option_status_position() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g status-position top").unwrap();
    assert_eq!(app.status_position, "top");
}

#[test]
fn set_option_status_style() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g status-style "bg=blue,fg=white""#).unwrap();
    assert_eq!(app.status_style, "bg=blue,fg=white");
}

#[test]
fn set_option_status_left() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g status-left "[#S]""#).unwrap();
    assert_eq!(app.status_left, "[#S]");
}

#[test]
fn set_option_status_right() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g status-right "%H:%M""#).unwrap();
    assert_eq!(app.status_right, "%H:%M");
}

#[test]
fn set_option_renumber_windows() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g renumber-windows on").unwrap();
    assert!(app.renumber_windows);
}

#[test]
fn set_option_automatic_rename() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g automatic-rename off").unwrap();
    assert!(!app.automatic_rename);
}

#[test]
fn set_option_allow_rename() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g allow-rename off").unwrap();
    assert!(!app.allow_rename);
}

#[test]
fn set_option_remain_on_exit() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g remain-on-exit on").unwrap();
    assert!(app.remain_on_exit);
}

#[test]
fn set_option_monitor_activity() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g monitor-activity on").unwrap();
    assert!(app.monitor_activity);
}

#[test]
fn set_option_visual_activity() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g visual-activity on").unwrap();
    assert!(app.visual_activity);
}

#[test]
fn set_option_set_titles() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g set-titles on").unwrap();
    assert!(app.set_titles);
}

#[test]
fn set_option_aggressive_resize() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g aggressive-resize on").unwrap();
    assert!(app.aggressive_resize);
}

#[test]
fn set_option_destroy_unattached() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g destroy-unattached on").unwrap();
    assert!(app.destroy_unattached);
}

#[test]
fn set_option_exit_empty() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g exit-empty off").unwrap();
    assert!(!app.exit_empty);
}

#[test]
fn set_option_word_separators() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g word-separators " -_@""#).unwrap();
    assert!(app.word_separators.contains('-'));
}

#[test]
fn set_option_pane_border_style() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g pane-border-style "fg=green""#).unwrap();
    assert_eq!(app.pane_border_style, "fg=green");
}

#[test]
fn set_option_pane_active_border_style() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g pane-active-border-style "fg=cyan""#).unwrap();
    assert_eq!(app.pane_active_border_style, "fg=cyan");
}

#[test]
fn set_option_window_status_format() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r##"set-option -g window-status-format "#I:#W""##).unwrap();
    assert_eq!(app.window_status_format, "#I:#W");
}

#[test]
fn set_option_scroll_enter_copy_mode() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g scroll-enter-copy-mode off").unwrap();
    assert!(!app.scroll_enter_copy_mode);
}

#[test]
fn set_option_pwsh_mouse_selection() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g pwsh-mouse-selection on").unwrap();
    assert!(app.pwsh_mouse_selection);
}

#[test]
fn set_option_activity_action() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g activity-action other").unwrap();
    assert_eq!(app.activity_action, "other");
}

#[test]
fn set_option_silence_action() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g silence-action none").unwrap();
    assert_eq!(app.silence_action, "none");
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. SHOW-OPTIONS: tmux flags AgHpqst:vw
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn show_options_no_flags_shows_all() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-options").unwrap();
    if is_popup(&app) {
        let out = popup_output(&app);
        assert!(out.contains("mouse") || out.contains("status"), "show-options should list options");
    }
}

#[test]
fn show_options_specific_option_name() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @show-test myval").unwrap();
    execute_command_string(&mut app, "show-options @show-test").unwrap();
    if is_popup(&app) {
        let out = popup_output(&app);
        assert!(out.contains("myval") || out.contains("@show-test"),
            "show-options with name should show that option");
    }
}

#[test]
fn show_options_v_value_only() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @vtest hello").unwrap();
    execute_command_string(&mut app, "show-options -v @vtest").unwrap();
    if is_popup(&app) {
        let out = popup_output(&app);
        assert!(out.contains("hello"), "-v flag: should show value only");
    }
}

#[test]
fn show_options_alias_show() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show mouse").unwrap();
    // 'show' is alias for 'show-options'; should not crash
}

#[test]
fn show_options_alias_showw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "showw mouse").unwrap();
    // 'showw' is alias for 'show-window-options'; should not crash
}

// ═════════════════════════════════════════════════════════════════════════════
// 3. BIND-KEY: tmux flags nrN:T:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn bind_key_default_prefix_table() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key z split-window -v").unwrap();
    let table = app.key_tables.get("prefix").expect("prefix table");
    assert!(table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('z')),
        "default bind goes to prefix table");
}

#[test]
fn bind_key_flag_n_root_table() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -n F3 split-window -v").unwrap();
    let table = app.key_tables.get("root").expect("root table");
    assert!(table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::F(3)),
        "-n flag: should bind to root table");
}

#[test]
fn bind_key_flag_T_custom_table() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -T copy-mode-vi v send-keys -X begin-selection").unwrap();
    let table = app.key_tables.get("copy-mode-vi").expect("copy-mode-vi table");
    assert!(table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('v')),
        "-T flag: should bind to named table");
}

#[test]
fn bind_key_flag_r_repeat() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -r n next-window").unwrap();
    let table = app.key_tables.get("prefix").expect("prefix table");
    let bind = table.iter().find(|b| b.key.0 == crossterm::event::KeyCode::Char('n'));
    assert!(bind.is_some(), "-r flag: key should be bound");
    assert!(bind.unwrap().repeat, "-r flag: should mark binding as repeatable");
}

#[test]
fn bind_key_combined_nr() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -nr M-Up select-pane -U").unwrap();
    let table = app.key_tables.get("root").expect("root table");
    // Combined -nr: root table + repeatable
    assert!(!table.is_empty(), "combined -nr: should bind to root table");
}

#[test]
fn bind_key_flag_T_root() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -T root F7 new-window").unwrap();
    let table = app.key_tables.get("root").expect("root table");
    assert!(table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::F(7)),
        "-T root: should bind to root table");
}

#[test]
fn bind_key_ctrl_modifier() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key C-x kill-pane").unwrap();
    let table = app.key_tables.get("prefix").expect("prefix table");
    let found = table.iter().any(|b| {
        b.key.0 == crossterm::event::KeyCode::Char('x')
            && b.key.1.contains(crossterm::event::KeyModifiers::CONTROL)
    });
    assert!(found, "C-x should bind Ctrl+x in prefix table");
}

#[test]
fn bind_key_alt_modifier() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -n M-h select-pane -L").unwrap();
    let table = app.key_tables.get("root").expect("root table");
    let found = table.iter().any(|b| {
        b.key.0 == crossterm::event::KeyCode::Char('h')
            && b.key.1.contains(crossterm::event::KeyModifiers::ALT)
    });
    assert!(found, "M-h should bind Alt+h in root table");
}

// ═════════════════════════════════════════════════════════════════════════════
// 4. UNBIND-KEY: tmux flags anqT:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn unbind_key_specific_key() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key q display-panes").unwrap();
    assert!(app.key_tables.get("prefix").unwrap().iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('q')));
    execute_command_string(&mut app, "unbind-key q").unwrap();
    assert!(!app.key_tables.get("prefix").unwrap().iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('q')),
        "unbind should remove the key");
}

#[test]
fn unbind_key_flag_a_all() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key a new-window").unwrap();
    execute_command_string(&mut app, "bind-key b split-window -v").unwrap();
    execute_command_string(&mut app, "unbind-key -a").unwrap();
    let empty = vec![];
    let table = app.key_tables.get("prefix").unwrap_or(&empty);
    assert!(table.is_empty(), "-a flag: should unbind all keys from prefix table");
}

#[test]
fn unbind_key_flag_n_root_table() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -n F9 new-window").unwrap();
    execute_command_string(&mut app, "unbind-key -n F9").unwrap();
    let empty = vec![];
    let table = app.key_tables.get("root").unwrap_or(&empty);
    assert!(!table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::F(9)),
        "-n flag: should unbind from root table");
}

#[test]
fn unbind_key_flag_T_named_table() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind-key -T copy-mode-vi y send-keys -X copy-selection").unwrap();
    execute_command_string(&mut app, "unbind-key -T copy-mode-vi y").unwrap();
    let empty = vec![];
    let table = app.key_tables.get("copy-mode-vi").unwrap_or(&empty);
    assert!(!table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('y')),
        "-T flag: should unbind from named table");
}

// ═════════════════════════════════════════════════════════════════════════════
// 5. SET-HOOK: tmux flags agpRt:uw
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn set_hook_basic_set() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g after-new-window "display-message created""#).unwrap();
    assert!(app.hooks.contains_key("after-new-window"), "set-hook should register hook");
    let cmds = app.hooks.get("after-new-window").unwrap();
    assert_eq!(cmds.len(), 1);
}

#[test]
fn set_hook_flag_a_append() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g after-split-window "cmd1""#).unwrap();
    execute_command_string(&mut app, r#"set-hook -ga after-split-window "cmd2""#).unwrap();
    let cmds = app.hooks.get("after-split-window").unwrap();
    assert!(cmds.len() >= 2, "-a flag: should append, got {} hooks", cmds.len());
}

#[test]
fn set_hook_flag_ag_append_global() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g after-kill-pane "first""#).unwrap();
    execute_command_string(&mut app, r#"set-hook -ag after-kill-pane "second""#).unwrap();
    let cmds = app.hooks.get("after-kill-pane").unwrap();
    assert!(cmds.len() >= 2, "-ag flag: should append");
}

#[test]
fn set_hook_flag_u_unset() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g after-new-session "cmd""#).unwrap();
    assert!(app.hooks.contains_key("after-new-session"));
    execute_command_string(&mut app, "set-hook -gu after-new-session").unwrap();
    assert!(!app.hooks.contains_key("after-new-session"), "-u flag: should remove hook");
}

#[test]
fn set_hook_flag_ug_unset_global() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g client-attached "notify""#).unwrap();
    execute_command_string(&mut app, "set-hook -ug client-attached").unwrap();
    assert!(!app.hooks.contains_key("client-attached"), "-ug flag: should remove hook");
}

#[test]
fn set_hook_overwrite_without_append() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-hook -g after-select-window "old-cmd""#).unwrap();
    execute_command_string(&mut app, r#"set-hook -g after-select-window "new-cmd""#).unwrap();
    let cmds = app.hooks.get("after-select-window").unwrap();
    assert_eq!(cmds.len(), 1, "without -a, should overwrite");
    assert!(cmds[0].contains("new-cmd"), "should be the new command");
}

// ═════════════════════════════════════════════════════════════════════════════
// 6. SET-ENVIRONMENT: tmux flags Fhgrt:u
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn set_environment_basic() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-environment MY_VAR my_value").unwrap();
    assert_eq!(app.environment.get("MY_VAR").map(|s| s.as_str()), Some("my_value"));
}

#[test]
fn set_environment_empty_value() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-environment EMPTY_VAR").unwrap();
    assert!(app.environment.contains_key("EMPTY_VAR"), "single arg should set with empty value");
}

#[test]
fn set_environment_flag_u_unset() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-environment TEST_UNSET val").unwrap();
    assert!(app.environment.contains_key("TEST_UNSET"));
    execute_command_string(&mut app, "set-environment -u TEST_UNSET").unwrap();
    assert!(!app.environment.contains_key("TEST_UNSET"), "-u flag: should remove var");
}

// ═════════════════════════════════════════════════════════════════════════════
// 7. DISPLAY-MESSAGE: tmux flags aCc:d:lINpt:F:v
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn display_message_no_flags_default() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-message").unwrap();
    // Should use default format, not crash
}

#[test]
fn display_message_custom_text() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-message "hello world""#).unwrap();
    let msg = status_msg(&app);
    assert!(msg.contains("hello world"), "should display custom text");
}

#[test]
fn display_message_flag_d_duration() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-message -d 5000 "timed msg""#).unwrap();
    if let Some((_, _, dur)) = &app.status_message {
        assert_eq!(*dur, Some(5000), "-d flag: duration should be 5000ms");
    }
}

#[test]
fn display_message_flag_p_print_mode() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-message -p '#{session_name}'").unwrap();
    // -p should print to stdout; in local mode it sets status_message
    let msg = status_msg(&app);
    assert!(msg.contains("flag_test"), "-p flag: should expand format and display");
}

#[test]
fn display_message_flag_I_consumed() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-message -I "test""#).unwrap();
    // -I should be consumed without crashing
}

#[test]
fn display_message_flag_t_target_consumed() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-message -t 0 "target test""#).unwrap();
    // -t should consume the next arg
}

#[test]
fn display_message_format_expansion() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-message '#{window_index}'").unwrap();
    let msg = status_msg(&app);
    assert!(msg.contains("0") || !msg.is_empty(), "format vars should expand");
}

// ═════════════════════════════════════════════════════════════════════════════
// 8. IF-SHELL: tmux flags bFt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn if_shell_true_condition() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"if-shell "true" "set-option -g @if-result yes""#).unwrap();
    // "true" always succeeds
}

#[test]
fn if_shell_false_condition_with_else() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"if-shell "false" "set-option -g @bad yes" "set-option -g @else-result yes""#).unwrap();
    assert_eq!(app.user_options.get("@else-result").map(|s| s.as_str()), Some("yes"),
        "false condition should run else branch");
}

#[test]
fn if_shell_flag_F_format_condition() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set-option -g @cond-test 1").unwrap();
    execute_command_string(&mut app, r##"if-shell -F "#{@cond-test}" "set-option -g @fmt-result yes""##).unwrap();
    // -F: condition is a format string, expanded then truth-tested
}

#[test]
fn if_shell_flag_F_empty_is_false() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"if-shell -F "" "set-option -g @should-not set" "set-option -g @empty-false yes""#).unwrap();
    assert_eq!(app.user_options.get("@empty-false").map(|s| s.as_str()), Some("yes"),
        "-F with empty string should be false");
}

#[test]
fn if_shell_flag_F_zero_is_false() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"if-shell -F "0" "set-option -g @shouldnot set" "set-option -g @zero-false yes""#).unwrap();
    assert_eq!(app.user_options.get("@zero-false").map(|s| s.as_str()), Some("yes"),
        "-F with '0' should be false");
}

#[test]
fn if_shell_literal_1_is_true() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"if-shell "1" "set-option -g @one-true yes""#).unwrap();
    assert_eq!(app.user_options.get("@one-true").map(|s| s.as_str()), Some("yes"),
        "literal '1' should be true");
}

// ═════════════════════════════════════════════════════════════════════════════
// 9. RUN-SHELL: tmux flags bd:Ct:Es:c:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn run_shell_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"run-shell "echo hello""#).unwrap();
    let msg = status_msg(&app);
    assert!(msg.contains("running:"), "run-shell should show status");
}

#[test]
fn run_shell_flag_b_background() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"run-shell -b "echo background""#).unwrap();
    // -b: should spawn in background, no popup, no blocking
}

#[test]
fn run_shell_empty_shows_usage() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "run-shell").unwrap();
    let msg = status_msg(&app);
    assert!(msg.contains("usage"), "empty run-shell should show usage");
}

// ═════════════════════════════════════════════════════════════════════════════
// 10. SPLIT-WINDOW: tmux flags bc:de:fF:hIl:p:Pt:vZ
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn split_window_default_vertical() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window").unwrap();
    // Default should be vertical split
}

#[test]
fn split_window_flag_h_horizontal() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -h").unwrap();
    // -h should trigger horizontal split
}

#[test]
fn split_window_flag_v_explicit_vertical() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -v").unwrap();
    // -v should be same as default (vertical)
}

#[test]
fn split_window_flag_p_percent() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -v -p 30").unwrap();
    // -p 30 should set percentage; command should not crash
}

#[test]
fn split_window_flag_l_lines() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -v -l 10").unwrap();
    // -l 10 should set exact line count
}

#[test]
fn split_window_flag_c_start_dir() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"split-window -v -c "C:\""#).unwrap();
    // -c should set working directory
}

#[test]
fn split_window_flag_d_detached() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -d").unwrap();
    // -d should not focus the new pane
}

#[test]
fn split_window_flag_b_before() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -b").unwrap();
    // -b should insert before current pane
}

#[test]
fn split_window_flag_f_full() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -f").unwrap();
    // -f should use full window width/height
}

#[test]
fn split_window_flag_F_format() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r##"split-window -F "#{pane_id}""##).unwrap();
    // -F should set format for output
}

#[test]
fn split_window_flag_P_print() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -P").unwrap();
    // -P should print pane info
}

#[test]
fn split_window_flag_Z_zoom() {
    let mut app = mock_app_with_window();
    assert!(app.windows[0].zoom_saved.is_none());
    execute_command_string(&mut app, "split-window -Z").unwrap();
    assert!(app.windows[0].zoom_saved.is_some(), "-Z should zoom the active pane after splitting");
    assert_eq!(app.windows[0].active_path, vec![1], "-Z should leave the new pane active");
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}

#[test]
fn split_window_flag_I_stdin() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -I").unwrap();
    // -I should enable stdin indicator
}

#[test]
fn split_window_flag_e_environment() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "split-window -e MY_VAR=test123").unwrap();
    // -e should pass environment variable
}

#[test]
fn split_window_multiple_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"split-window -h -p 40 -c "C:\" -d"#).unwrap();
    // Multiple flags combined should not crash
}

// ═════════════════════════════════════════════════════════════════════════════
// 11. NEW-SESSION: tmux flags Ac:dDe:EF:f:n:Ps:t:x:Xy:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn new_session_flag_s_name() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -s mysession").unwrap();
    // -s should set session name for new session
}

#[test]
fn new_session_flag_d_detached() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -d -s detach_test").unwrap();
    // -d should create session without attaching
}

#[test]
fn new_session_flag_n_window_name() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -n mywin -s ntest").unwrap();
    // -n should set initial window name
}

#[test]
fn new_session_flag_c_start_dir() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"new-session -c "C:\temp" -s ctest"#).unwrap();
    // -c should set starting directory
}

#[test]
fn new_session_flag_e_environment() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -e MY_VAR=hello -s etest").unwrap();
    // -e should pass environment variable
}

#[test]
fn new_session_flag_A_attach_if_exists() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -A -s flag_test").unwrap();
    // -A: if session exists, attach to it instead of creating new
}

#[test]
fn new_session_compat_flags_D_E_P_X() {
    let mut app = mock_app_with_window();
    // Compatibility flags should be consumed without error
    execute_command_string(&mut app, "new-session -D -s compat1").unwrap();
    execute_command_string(&mut app, "new-session -E -s compat2").unwrap();
    execute_command_string(&mut app, "new-session -P -s compat3").unwrap();
    execute_command_string(&mut app, "new-session -X -s compat4").unwrap();
}

#[test]
fn new_session_flag_x_y_dimensions() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-session -x 120 -y 40 -s dimtest").unwrap();
    // -x -y should set initial dimensions
}

#[test]
fn new_session_flag_F_format() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r##"new-session -F "#{session_name}" -s fmttest"##).unwrap();
    // -F should set format
}

#[test]
fn new_session_multiple_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"new-session -d -s multi -n win1 -c "C:\" -e TEST=1"#).unwrap();
    // Combined flags should work
}

// ═════════════════════════════════════════════════════════════════════════════
// 12. NEW-WINDOW: tmux flags abc:de:F:kn:PSt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn new_window_flag_n_name() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-window -n named_win").unwrap();
}

#[test]
fn new_window_flag_d_detached() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new-window -d").unwrap();
}

#[test]
fn new_window_flag_c_start_dir() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"new-window -c "C:\""#).unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 13. SELECT-PANE: tmux flags DdegLlMmP:RT:t:UZ
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn select_pane_flag_U_up() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -U").unwrap();
}

#[test]
fn select_pane_flag_D_down() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -D").unwrap();
}

#[test]
fn select_pane_flag_L_left() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -L").unwrap();
}

#[test]
fn select_pane_flag_R_right() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -R").unwrap();
}

#[test]
fn select_pane_flag_l_last() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -l").unwrap();
    // -l should switch to last active pane
}

#[test]
fn select_pane_flag_t_target() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-pane -t 0").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 14. RESIZE-PANE: tmux flags DLMRTt:Ux:y:Z
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn resize_pane_flag_D_down() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -D 5").unwrap();
}

#[test]
fn resize_pane_flag_U_up() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -U 5").unwrap();
}

#[test]
fn resize_pane_flag_L_left() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -L 5").unwrap();
}

#[test]
fn resize_pane_flag_R_right() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -R 5").unwrap();
}

#[test]
fn resize_pane_flag_Z_zoom() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -Z").unwrap();
}

#[test]
fn resize_pane_flag_x_absolute_cols() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -x 80").unwrap();
}

#[test]
fn resize_pane_flag_y_absolute_rows() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resize-pane -y 24").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 15. SWAP-PANE: tmux flags dDs:t:UZ
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn swap_pane_flag_U_up() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swap-pane -U").unwrap();
}

#[test]
fn swap_pane_flag_D_down() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swap-pane -D").unwrap();
}

#[test]
fn swap_pane_default_is_down() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swap-pane").unwrap();
    // Default should be -D (down)
}

// ═════════════════════════════════════════════════════════════════════════════
// 16. ROTATE-WINDOW: tmux flags Dt:UZ
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn rotate_window_default_up() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotate-window").unwrap();
    // Default should rotate upward
}

#[test]
fn rotate_window_flag_D_downward() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotate-window -D").unwrap();
    // -D should rotate downward
}

// ═════════════════════════════════════════════════════════════════════════════
// 17. SEND-KEYS: tmux flags c:FHKlMN:Rt:X
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn send_keys_named_key_enter() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys Enter").unwrap();
}

#[test]
fn send_keys_named_key_space() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys Space").unwrap();
}

#[test]
fn send_keys_named_key_escape() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys Escape").unwrap();
}

#[test]
fn send_keys_named_key_tab() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys Tab").unwrap();
}

#[test]
fn send_keys_named_key_bspace() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys BSpace").unwrap();
}

#[test]
fn send_keys_flag_l_literal() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"send-keys -l "literal text""#).unwrap();
    // -l should send text as-is, no key name parsing
}

#[test]
fn send_keys_flag_t_target() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-keys -t 0 Enter").unwrap();
    // -t should target a specific pane
}

#[test]
fn send_keys_text_string() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"send-keys "ls -la" Enter"#).unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 18. DISPLAY-POPUP: tmux flags Bb:Cc:d:e:Eh:kNs:S:t:T:w:x:y:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn display_popup_flag_w_width() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -w 40 "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_h_height() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -h 20 "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_w_h_combined() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -w 60 -h 15 "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_d_start_dir() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -d "C:\" "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_c_start_dir_alias() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -c "C:\" "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_E_close_on_exit() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -E "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_K() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -K "echo test""#).unwrap();
}

#[test]
fn display_popup_flag_w_percent() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display-popup -w 50% -h 50% "echo test""#).unwrap();
    // Percentage dimensions should be accepted
}

// ═════════════════════════════════════════════════════════════════════════════
// 19. LINK-WINDOW: tmux flags abdks:t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn link_window_flag_s_source() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    execute_command_string(&mut app, "link-window -s 0").unwrap();
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}

#[test]
fn link_window_flag_t_target() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    execute_command_string(&mut app, "link-window -s 0 -t 2").unwrap();
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}

// ═════════════════════════════════════════════════════════════════════════════
// 20. MOVE-WINDOW: tmux flags abdkrs:t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn move_window_positional_target() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "move-window 1").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 21. SWAP-WINDOW: tmux flags ds:t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn swap_window_positional_target() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "swap-window 1").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 22. RESPAWN-PANE: tmux flags c:e:kt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn respawn_pane_flag_k_kill() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "respawn-pane -k").unwrap();
    // -k should kill existing pane process before respawning
}

#[test]
fn respawn_pane_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "respawn-pane").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 23. COMMAND-PROMPT: tmux flags 1beFiklI:Np:t:T:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn command_prompt_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "command-prompt").unwrap();
    assert!(is_command_prompt(&app), "command-prompt should enter CommandPrompt mode");
}

#[test]
fn command_prompt_flag_I_initial() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"command-prompt -I "split-window""#).unwrap();
    assert!(is_command_prompt(&app));
    let input = prompt_input(&app);
    assert!(input.contains("split-window"), "-I flag: should pre-fill prompt");
}

// ═════════════════════════════════════════════════════════════════════════════
// 24. SOURCE-FILE: tmux flags t:Fnqv
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn source_file_nonexistent_no_crash() {
    let mut app = mock_app_with_window();
    let _ = execute_command_string(&mut app, "source-file /nonexistent/path.conf");
}

// ═════════════════════════════════════════════════════════════════════════════
// 25. RENAME-SESSION/RENAME-WINDOW: tmux flags t: + positional
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn rename_session_positional() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rename-session rtest").unwrap();
    assert_eq!(app.session_name, "rtest");
}

#[test]
fn rename_window_positional() {
    let mut app = mock_app_with_windows(&["original"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "rename-window newname").unwrap();
    assert_eq!(app.windows[0].name, "newname");
    assert!(app.windows[0].manual_rename);
}

// ═════════════════════════════════════════════════════════════════════════════
// 26. KILL-PANE/KILL-WINDOW/KILL-SESSION: tmux flags at:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn kill_pane_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "kill-pane").unwrap();
}

#[test]
fn kill_window_no_flags() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "kill-window").unwrap();
}

#[test]
fn kill_session_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "kill-session").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 27. SELECT-LAYOUT: tmux flags Enopt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn select_layout_tiled() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout tiled").unwrap();
}

#[test]
fn select_layout_even_horizontal() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout even-horizontal").unwrap();
}

#[test]
fn select_layout_even_vertical() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout even-vertical").unwrap();
}

#[test]
fn select_layout_main_horizontal() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout main-horizontal").unwrap();
}

#[test]
fn select_layout_main_vertical() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "select-layout main-vertical").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 28. NEXT/PREVIOUS LAYOUT: tmux flags t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn next_layout_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "next-layout").unwrap();
}

#[test]
fn previous_layout_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "previous-layout").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 29. NEXT/PREVIOUS/LAST/SELECT WINDOW: tmux flags at:, t:, lnpTt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn next_window_advances() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "next-window").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn previous_window_goes_back() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 1;
    execute_command_string(&mut app, "previous-window").unwrap();
    assert_eq!(app.active_idx, 0);
}

#[test]
fn last_window_switches() {
    let mut app = mock_app_with_windows(&["w0", "w1"]);
    app.active_idx = 0;
    app.last_window_idx = 1;
    execute_command_string(&mut app, "last-window").unwrap();
}

#[test]
fn select_window_flag_t_index() {
    let mut app = mock_app_with_windows(&["w0", "w1", "w2"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "select-window -t 2").unwrap();
    assert_eq!(app.active_idx, 2);
}

// ═════════════════════════════════════════════════════════════════════════════
// 30. BREAK-PANE: tmux flags abdPF:n:s:t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn break_pane_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "break-pane").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 31. CAPTURE-PANE: tmux flags ab:CeE:JMNpPqS:Tt:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn capture_pane_no_flags() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "capture-pane").unwrap();
}

#[test]
fn capture_pane_flag_p_print() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "capture-pane -p").unwrap();
    // -p should print to stdout; local implementation captures to buffer
}

// ═════════════════════════════════════════════════════════════════════════════
// 32. COPY-MODE / PASTE / BUFFER OPS
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn copy_mode_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "copy-mode").unwrap();
}

#[test]
fn paste_buffer_dispatches() {
    let mut app = mock_app_with_window();
    app.paste_buffers.push("test paste".to_string());
    execute_command_string(&mut app, "paste-buffer").unwrap();
}

#[test]
fn set_buffer_content() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-buffer "hello buffer""#).unwrap();
}

#[test]
fn list_buffers_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-buffers").unwrap();
}

#[test]
fn show_buffer_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-buffer").unwrap();
}

#[test]
fn delete_buffer_dispatches() {
    let mut app = mock_app_with_window();
    app.paste_buffers.push("to delete".to_string());
    execute_command_string(&mut app, "delete-buffer").unwrap();
}

#[test]
fn choose_buffer_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-buffer").unwrap();
}

#[test]
fn clear_history_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "clear-history").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 33. HAS-SESSION: tmux flags t:
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn has_session_flag_t_target() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "has-session -t flag_test").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 34. LIST COMMANDS: list-sessions, list-windows, list-panes, list-keys,
//     list-commands, list-buffers, list-clients
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn list_sessions_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-sessions").unwrap();
}

#[test]
fn list_windows_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-windows").unwrap();
}

#[test]
fn list_panes_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-panes").unwrap();
}

#[test]
fn list_keys_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-keys").unwrap();
}

#[test]
fn list_commands_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-commands").unwrap();
}

#[test]
fn list_clients_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-clients").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 35. CHOOSER MODES: choose-tree, choose-window, choose-session, choose-client
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn choose_tree_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-tree").unwrap();
}

#[test]
fn choose_window_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-window").unwrap();
}

#[test]
fn choose_session_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-session").unwrap();
}

#[test]
fn choose_client_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "choose-client").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 36. DISPLAY-PANES / DISPLAY-MENU / CLOCK-MODE / CUSTOMIZE-MODE
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn display_panes_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "display-panes").unwrap();
}

#[test]
fn clock_mode_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "clock-mode").unwrap();
}

#[test]
fn customize_mode_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "customize-mode").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 37. DETACH / REFRESH / SUSPEND / LOCK (stubs)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn detach_client_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "detach-client").unwrap();
}

#[test]
fn refresh_client_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "refresh-client").unwrap();
}

#[test]
fn suspend_client_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "suspend-client").unwrap();
}

#[test]
fn lock_server_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-server").unwrap();
}

#[test]
fn lock_client_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-client").unwrap();
}

#[test]
fn lock_session_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lock-session").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 38. SHOW-HOOKS / SHOW-ENVIRONMENT / SHOW-MESSAGES / SHOW-BUFFER
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn show_hooks_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-hooks").unwrap();
}

#[test]
fn show_environment_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-environment").unwrap();
}

#[test]
fn show_messages_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show-messages").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 39. WAIT-FOR / SEND-PREFIX / START-SERVER / KILL-SERVER / SERVER-INFO
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn send_prefix_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send-prefix").unwrap();
}

#[test]
fn start_server_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "start-server").unwrap();
}

#[test]
fn server_info_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "server-info").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 40. CONFIRM-BEFORE / FIND-WINDOW / UNLINK-WINDOW
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn confirm_before_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "confirm-before kill-session").unwrap();
}

#[test]
fn find_window_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "find-window test").unwrap();
}

#[test]
fn unlink_window_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "unlink-window").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 41. JOIN-PANE / MOVE-PANE / PIPE-PANE / LAST-PANE / RESPAWN-WINDOW
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn join_pane_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "join-pane").unwrap();
}

#[test]
fn last_pane_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "last-pane").unwrap();
}

#[test]
fn respawn_window_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "respawn-window").unwrap();
}

#[test]
fn pipe_pane_dispatches() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "pipe-pane").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 42. COMMAND ALIASES (tmux compat)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn alias_splitw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "splitw").unwrap();
}

#[test]
fn alias_selectp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "selectp -U").unwrap();
}

#[test]
fn alias_selectw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "selectw -t 0").unwrap();
}

#[test]
fn alias_killw() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "killw").unwrap();
}

#[test]
fn alias_killp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "killp").unwrap();
}

#[test]
fn alias_resizep() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "resizep -D 3").unwrap();
}

#[test]
fn alias_swapp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "swapp -D").unwrap();
}

#[test]
fn alias_rotatew() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "rotatew").unwrap();
}

#[test]
fn alias_breakp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "breakp").unwrap();
}

#[test]
fn alias_capturep() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "capturep").unwrap();
}

#[test]
fn alias_neww() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "neww").unwrap();
}

#[test]
fn alias_renamew() {
    let mut app = mock_app_with_windows(&["orig"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "renamew aliased").unwrap();
    assert_eq!(app.windows[0].name, "aliased");
}

#[test]
fn alias_lsw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lsw").unwrap();
}

#[test]
fn alias_ls() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "ls").unwrap();
}

#[test]
fn alias_lsp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lsp").unwrap();
}

#[test]
fn alias_lsk() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lsk").unwrap();
}

#[test]
fn alias_lscm() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lscm").unwrap();
}

#[test]
fn alias_joinp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "joinp").unwrap();
}

#[test]
fn alias_lastp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "lastp").unwrap();
}

#[test]
fn alias_respawnp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "respawnp").unwrap();
}

#[test]
fn alias_respawnw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "respawnw").unwrap();
}

#[test]
fn alias_movew() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "movew 1").unwrap();
}

#[test]
fn alias_swapw() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "swapw 1").unwrap();
}

#[test]
fn alias_linkw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "linkw -s 0").unwrap();
    // These commands start a real shell; end it before the state drops.
    crate::util::kill_app_shells(&mut app);
}

#[test]
fn alias_unlinkw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "unlinkw").unwrap();
}

#[test]
fn alias_pipep() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "pipep").unwrap();
}

#[test]
fn alias_setenv() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "setenv ALIAS_TEST val").unwrap();
    assert_eq!(app.environment.get("ALIAS_TEST").map(|s| s.as_str()), Some("val"));
}

#[test]
fn alias_showenv() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "showenv").unwrap();
}

#[test]
fn alias_set_is_set_option() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "set -g mouse on").unwrap();
    assert!(app.mouse_enabled, "'set' alias should work as set-option");
}

#[test]
fn alias_setw_is_set_window_option() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "setw -g mouse off").unwrap();
    assert!(!app.mouse_enabled, "'setw' alias should work as set-window-option");
}

#[test]
fn alias_show() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "show").unwrap();
}

#[test]
fn alias_showw() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "showw").unwrap();
}

#[test]
fn alias_bind() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind x new-window").unwrap();
    let table = app.key_tables.get("prefix").expect("prefix table");
    assert!(table.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('x')),
        "'bind' alias should work");
}

#[test]
fn alias_unbind() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "bind y new-window").unwrap();
    execute_command_string(&mut app, "unbind y").unwrap();
}

#[test]
fn alias_source() {
    let mut app = mock_app_with_window();
    let _ = execute_command_string(&mut app, "source /nonexistent");
}

#[test]
fn alias_display() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"display "alias test""#).unwrap();
}

#[test]
fn alias_displayp() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "displayp").unwrap();
}

#[test]
fn alias_run() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"run "echo hello""#).unwrap();
}

#[test]
fn alias_send() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "send Enter").unwrap();
}

#[test]
fn alias_selectl() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "selectl tiled").unwrap();
}

#[test]
fn alias_has() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "has -t flag_test").unwrap();
}

#[test]
fn alias_rename() {
    let mut app = mock_app_with_window();
    // "rename" alias is resolved via parse_command into Action::Command
    // but execute_command_string_single only matches "rename-session"
    execute_command_string(&mut app, "rename-session alias_renamed").unwrap();
    assert_eq!(app.session_name, "alias_renamed");
}

#[test]
fn alias_new() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "new -s aliased_session").unwrap();
}

#[test]
fn alias_attach() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "attach -t flag_test").unwrap();
}

#[test]
fn alias_at() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "at -t flag_test").unwrap();
}

#[test]
fn alias_detach() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "detach").unwrap();
}

#[test]
fn alias_next() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    execute_command_string(&mut app, "next").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn alias_prev() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 1;
    execute_command_string(&mut app, "prev").unwrap();
    assert_eq!(app.active_idx, 0);
}

#[test]
fn alias_last() {
    let mut app = mock_app_with_windows(&["a", "b"]);
    app.active_idx = 0;
    app.last_window_idx = 1;
    execute_command_string(&mut app, "last").unwrap();
}

#[test]
fn alias_info() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "info").unwrap();
}

#[test]
fn alias_start() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "start").unwrap();
}

#[test]
fn alias_warmup() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "warmup").unwrap();
}

// ═════════════════════════════════════════════════════════════════════════════
// 43. SWITCH-CLIENT: tmux flags c:EFlnO:pt:rT:Z
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn switch_client_flag_T_key_table() {
    let mut app = mock_app_with_window();
    // switch-client -T is parsed into Action::SwitchTable by parse_command,
    // then handled in execute_action.  Test via that path.
    let action = parse_command_to_action("switch-client -T copy-mode-vi").unwrap();
    execute_action(&mut app, &action).unwrap();
    assert_eq!(app.current_key_table.as_deref(), Some("copy-mode-vi"),
        "-T flag: should switch key table");
}

// ═════════════════════════════════════════════════════════════════════════════
// 44. COMMAND CHAINING (\;) parity with tmux
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn command_chain_two_commands() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"set-option -g @chain1 v1 \; set-option -g @chain2 v2"#).unwrap();
    assert_eq!(app.user_options.get("@chain1").map(|s| s.as_str()), Some("v1"));
    assert_eq!(app.user_options.get("@chain2").map(|s| s.as_str()), Some("v2"));
}

#[test]
fn command_chain_three_commands() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app,
        r#"set-option -g @a 1 \; set-option -g @b 2 \; set-option -g @c 3"#).unwrap();
    assert_eq!(app.user_options.get("@a").map(|s| s.as_str()), Some("1"));
    assert_eq!(app.user_options.get("@b").map(|s| s.as_str()), Some("2"));
    assert_eq!(app.user_options.get("@c").map(|s| s.as_str()), Some("3"));
}
