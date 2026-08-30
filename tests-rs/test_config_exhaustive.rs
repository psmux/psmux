// Exhaustive configuration tests for psmux.
//
// Tests every config option through multiple channels:
// 1. parse_config_content (config file parsing path)
// 2. parse_config_line (direct line parsing)
// 3. execute_command_string (CLI / command path)
//
// Also tests config parsing features:
// - Continuation lines (\)
// - Conditional blocks (%if / %elif / %else / %endif)
// - %hidden variables and $NAME expansion
// - Comments and empty lines
// - UTF-8 BOM handling
// - setw / set-window-option aliases
// - Option flags: -g, -u, -a, -q, -o, -w, -F, combined
// - Default values for every option

use crate::types::AppState;
use crate::config::{parse_config_content, parse_config_line};
use crate::commands::execute_command_string;
use std::sync::Mutex;

static ENV_MUTEX: Mutex<()> = Mutex::new(());

fn mock_app() -> AppState {
    AppState::new("config-test".to_string())
}

// ============================================================
// SECTION 1: Default values for every option
// ============================================================

#[test]
fn default_escape_time() {
    let app = mock_app();
    assert_eq!(app.escape_time_ms, 500);
}

#[test]
fn default_mouse() {
    let app = mock_app();
    assert!(app.mouse_enabled);
}

#[test]
fn default_status_visible() {
    let app = mock_app();
    assert!(app.status_visible);
}

#[test]
fn default_status_position() {
    let app = mock_app();
    assert_eq!(app.status_position, "bottom");
}

#[test]
fn default_status_style() {
    let app = mock_app();
    assert_eq!(app.status_style, "bg=green,fg=black");
}

#[test]
fn default_status_left() {
    let app = mock_app();
    assert_eq!(app.status_left, "[#S] ");
}

#[test]
fn default_status_right() {
    let app = mock_app();
    assert!(app.status_right.contains("pane_title"));
}

#[test]
fn default_status_interval() {
    let app = mock_app();
    assert_eq!(app.status_interval, 15);
}

#[test]
fn default_status_justify() {
    let app = mock_app();
    assert_eq!(app.status_justify, "left");
}

#[test]
fn default_status_left_length() {
    let app = mock_app();
    assert_eq!(app.status_left_length, 10);
}

#[test]
fn default_status_right_length() {
    let app = mock_app();
    assert_eq!(app.status_right_length, 40);
}

#[test]
fn default_status_lines() {
    let app = mock_app();
    assert_eq!(app.status_lines, 1);
}

#[test]
fn default_window_base_index() {
    let app = mock_app();
    assert_eq!(app.window_base_index, 0);
}

#[test]
fn default_pane_base_index() {
    let app = mock_app();
    assert_eq!(app.pane_base_index, 0);
}

#[test]
fn default_history_limit() {
    let app = mock_app();
    assert_eq!(app.history_limit, 2000);
}

#[test]
fn default_display_time() {
    let app = mock_app();
    assert_eq!(app.display_time_ms, 750);
}

#[test]
fn default_display_panes_time() {
    let app = mock_app();
    assert_eq!(app.display_panes_time_ms, 1000);
}

#[test]
fn default_focus_events() {
    let app = mock_app();
    assert!(!app.focus_events);
}

#[test]
fn default_mode_keys() {
    let app = mock_app();
    assert_eq!(app.mode_keys, "emacs");
}

#[test]
fn default_word_separators() {
    let app = mock_app();
    assert_eq!(app.word_separators, " -_@");
}

#[test]
fn default_renumber_windows() {
    let app = mock_app();
    assert!(!app.renumber_windows);
}

#[test]
fn default_automatic_rename() {
    let app = mock_app();
    assert!(app.automatic_rename);
}

#[test]
fn default_allow_rename() {
    let app = mock_app();
    assert!(app.allow_rename);
}

#[test]
fn default_monitor_activity() {
    let app = mock_app();
    assert!(!app.monitor_activity);
}

#[test]
fn default_visual_activity() {
    let app = mock_app();
    assert!(!app.visual_activity);
}

#[test]
fn default_remain_on_exit() {
    let app = mock_app();
    assert!(!app.remain_on_exit);
}

#[test]
fn default_destroy_unattached() {
    let app = mock_app();
    assert!(!app.destroy_unattached);
}

#[test]
fn default_exit_empty() {
    let app = mock_app();
    assert!(app.exit_empty);
}

#[test]
fn default_aggressive_resize() {
    let app = mock_app();
    assert!(!app.aggressive_resize);
}

#[test]
fn default_set_titles() {
    let app = mock_app();
    assert!(!app.set_titles);
}

#[test]
fn default_set_titles_string() {
    let app = mock_app();
    assert_eq!(app.set_titles_string, "");
}

#[test]
fn default_activity_action() {
    let app = mock_app();
    assert_eq!(app.activity_action, "other");
}

#[test]
fn default_silence_action() {
    let app = mock_app();
    assert_eq!(app.silence_action, "other");
}

#[test]
fn default_bell_action() {
    let app = mock_app();
    assert_eq!(app.bell_action, "any");
}

#[test]
fn default_visual_bell() {
    let app = mock_app();
    assert!(!app.visual_bell);
}

#[test]
fn default_monitor_silence() {
    let app = mock_app();
    assert_eq!(app.monitor_silence, 0);
}

#[test]
fn default_scroll_enter_copy_mode() {
    let app = mock_app();
    assert!(app.scroll_enter_copy_mode);
}

#[test]
fn default_pwsh_mouse_selection() {
    let app = mock_app();
    assert!(!app.pwsh_mouse_selection);
}

#[test]
fn default_mouse_selection_force() {
    let app = mock_app();
    assert!(!app.mouse_selection_force);
}

#[test]
fn default_sync_input() {
    let app = mock_app();
    assert!(!app.sync_input);
}

#[test]
fn default_pane_border_style() {
    let app = mock_app();
    assert_eq!(app.pane_border_style, "");
}

#[test]
fn default_pane_active_border_style() {
    let app = mock_app();
    assert_eq!(app.pane_active_border_style, "fg=green");
}

#[test]
fn default_pane_border_hover_style() {
    let app = mock_app();
    assert_eq!(app.pane_border_hover_style, "fg=yellow");
}

#[test]
fn default_window_status_format() {
    let app = mock_app();
    assert!(app.window_status_format.contains("#I:#W"));
}

#[test]
fn default_window_status_current_format() {
    let app = mock_app();
    assert!(app.window_status_current_format.contains("#I:#W"));
}

#[test]
fn default_window_status_separator() {
    let app = mock_app();
    assert_eq!(app.window_status_separator, " ");
}

#[test]
fn default_window_status_style() {
    let app = mock_app();
    assert_eq!(app.window_status_style, "");
}

#[test]
fn default_window_status_current_style() {
    let app = mock_app();
    assert_eq!(app.window_status_current_style, "");
}

#[test]
fn default_window_status_activity_style() {
    let app = mock_app();
    assert_eq!(app.window_status_activity_style, "reverse");
}

#[test]
fn default_window_status_bell_style() {
    let app = mock_app();
    assert_eq!(app.window_status_bell_style, "reverse");
}

#[test]
fn default_window_status_last_style() {
    let app = mock_app();
    assert_eq!(app.window_status_last_style, "");
}

#[test]
fn default_message_style() {
    let app = mock_app();
    assert_eq!(app.message_style, "bg=yellow,fg=black");
}

#[test]
fn default_message_command_style() {
    let app = mock_app();
    assert_eq!(app.message_command_style, "bg=black,fg=yellow");
}

#[test]
fn default_mode_style() {
    let app = mock_app();
    assert_eq!(app.mode_style, "bg=yellow,fg=black");
}

#[test]
fn default_status_left_style() {
    let app = mock_app();
    assert_eq!(app.status_left_style, "");
}

#[test]
fn default_status_right_style() {
    let app = mock_app();
    assert_eq!(app.status_right_style, "");
}

#[test]
fn default_main_pane_width() {
    let app = mock_app();
    assert_eq!(app.main_pane_width, 0);
}

#[test]
fn default_main_pane_height() {
    let app = mock_app();
    assert_eq!(app.main_pane_height, 0);
}

#[test]
fn default_window_size() {
    let app = mock_app();
    assert_eq!(app.window_size, "latest");
}

#[test]
fn default_allow_passthrough() {
    let app = mock_app();
    assert_eq!(app.allow_passthrough, "off");
}

#[test]
fn default_copy_command() {
    let app = mock_app();
    assert_eq!(app.copy_command, "");
}

#[test]
fn default_set_clipboard() {
    let app = mock_app();
    assert_eq!(app.set_clipboard, "on");
}

#[test]
fn default_env_shim() {
    let app = mock_app();
    assert!(app.env_shim);
}

#[test]
fn default_claude_code_fix_tty() {
    let app = mock_app();
    assert!(app.claude_code_fix_tty);
}

#[test]
fn default_claude_code_force_interactive() {
    let app = mock_app();
    assert!(app.claude_code_force_interactive);
}

#[test]
fn default_default_shell() {
    let app = mock_app();
    assert_eq!(app.default_shell, "");
}

#[test]
fn default_prediction_dimming() {
    let app = mock_app();
    // Default depends on PSMUX_DIM_PREDICTIONS env var, but typically false
    // Just verify it's a boolean that was set
    let _ = app.prediction_dimming;
}

#[test]
fn default_allow_predictions() {
    let app = mock_app();
    assert!(!app.allow_predictions);
}

#[test]
fn default_update_environment() {
    let app = mock_app();
    assert!(app.update_environment.contains(&"DISPLAY".to_string()));
    assert!(app.update_environment.contains(&"SSH_AUTH_SOCK".to_string()));
}

// ============================================================
// SECTION 2: Every option via parse_config_content (config file)
// ============================================================

#[test]
fn config_file_mouse_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mouse on\n");
    assert!(app.mouse_enabled);
}

#[test]
fn config_file_mouse_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_file_escape_time() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 50\n");
    assert_eq!(app.escape_time_ms, 50);
}

#[test]
fn config_file_status_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status on\n");
    assert!(app.status_visible);
}

#[test]
fn config_file_status_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status off\n");
    assert!(!app.status_visible);
}

#[test]
fn config_file_status_numeric_0() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status 0\n");
    assert!(!app.status_visible);
}

#[test]
fn config_file_status_numeric_2_multi_line() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status 2\n");
    assert!(app.status_visible);
    assert_eq!(app.status_lines, 2);
}

#[test]
fn config_file_status_numeric_5_multi_line() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status 5\n");
    assert!(app.status_visible);
    assert_eq!(app.status_lines, 5);
}

#[test]
fn config_file_status_position_top() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-position top\n");
    assert_eq!(app.status_position, "top");
}

#[test]
fn config_file_status_position_bottom() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-position bottom\n");
    assert_eq!(app.status_position, "bottom");
}

#[test]
fn config_file_status_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-style 'bg=blue,fg=white'\n");
    assert_eq!(app.status_style, "bg=blue,fg=white");
}

#[test]
fn config_file_status_left() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left \"[#S] \"\n");
    assert_eq!(app.status_left, "[#S] ");
}

#[test]
fn config_file_status_right() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-right \"hello\"\n");
    assert_eq!(app.status_right, "hello");
}

#[test]
fn config_file_status_interval() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-interval 5\n");
    assert_eq!(app.status_interval, 5);
}

#[test]
fn config_file_status_justify_centre() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-justify centre\n");
    assert_eq!(app.status_justify, "centre");
}

#[test]
fn config_file_status_justify_right() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-justify right\n");
    assert_eq!(app.status_justify, "right");
}

#[test]
fn config_file_status_left_length() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left-length 50\n");
    assert_eq!(app.status_left_length, 50);
}

#[test]
fn config_file_status_right_length() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-right-length 80\n");
    assert_eq!(app.status_right_length, 80);
}

#[test]
fn config_file_status_left_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left-style 'fg=cyan'\n");
    assert_eq!(app.status_left_style, "fg=cyan");
}

#[test]
fn config_file_status_right_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-right-style 'fg=magenta'\n");
    assert_eq!(app.status_right_style, "fg=magenta");
}

#[test]
fn config_file_base_index() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g base-index 1\n");
    assert_eq!(app.window_base_index, 1);
}

#[test]
fn config_file_pane_base_index() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-base-index 1\n");
    assert_eq!(app.pane_base_index, 1);
}

#[test]
fn config_file_history_limit() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g history-limit 50000\n");
    assert_eq!(app.history_limit, 50000);
}

#[test]
fn config_file_display_time() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g display-time 2000\n");
    assert_eq!(app.display_time_ms, 2000);
}

#[test]
fn config_file_display_panes_time() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g display-panes-time 3000\n");
    assert_eq!(app.display_panes_time_ms, 3000);
}

#[test]
fn config_file_focus_events_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g focus-events on\n");
    assert!(app.focus_events);
}

#[test]
fn config_file_focus_events_off() {
    let mut app = mock_app();
    app.focus_events = true;
    parse_config_content(&mut app, "set -g focus-events off\n");
    assert!(!app.focus_events);
}

#[test]
fn config_file_mode_keys_vi() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mode-keys vi\n");
    assert_eq!(app.mode_keys, "vi");
}

#[test]
fn config_file_mode_keys_emacs() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mode-keys emacs\n");
    assert_eq!(app.mode_keys, "emacs");
}

#[test]
fn config_file_word_separators() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g word-separators \" -_@./\"\n");
    assert_eq!(app.word_separators, " -_@./");
}

#[test]
fn config_file_renumber_windows_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g renumber-windows on\n");
    assert!(app.renumber_windows);
}

#[test]
fn config_file_renumber_windows_off() {
    let mut app = mock_app();
    app.renumber_windows = true;
    parse_config_content(&mut app, "set -g renumber-windows off\n");
    assert!(!app.renumber_windows);
}

#[test]
fn config_file_automatic_rename_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g automatic-rename on\n");
    assert!(app.automatic_rename);
}

#[test]
fn config_file_automatic_rename_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g automatic-rename off\n");
    assert!(!app.automatic_rename);
}

#[test]
fn config_file_allow_rename_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g allow-rename on\n");
    assert!(app.allow_rename);
}

#[test]
fn config_file_allow_rename_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g allow-rename off\n");
    assert!(!app.allow_rename);
}

#[test]
fn config_file_monitor_activity_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g monitor-activity on\n");
    assert!(app.monitor_activity);
}

#[test]
fn config_file_visual_activity_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g visual-activity on\n");
    assert!(app.visual_activity);
}

#[test]
fn config_file_remain_on_exit_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g remain-on-exit on\n");
    assert!(app.remain_on_exit);
}

#[test]
fn config_file_destroy_unattached_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g destroy-unattached on\n");
    assert!(app.destroy_unattached);
}

#[test]
fn config_file_exit_empty_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g exit-empty off\n");
    assert!(!app.exit_empty);
}

#[test]
fn config_file_aggressive_resize_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g aggressive-resize on\n");
    assert!(app.aggressive_resize);
}

#[test]
fn config_file_set_titles_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g set-titles on\n");
    assert!(app.set_titles);
}

#[test]
fn config_file_set_titles_string() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g set-titles-string \"#S:#W\"\n");
    assert_eq!(app.set_titles_string, "#S:#W");
}

#[test]
fn config_file_activity_action() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g activity-action any\n");
    assert_eq!(app.activity_action, "any");
}

#[test]
fn config_file_silence_action() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g silence-action none\n");
    assert_eq!(app.silence_action, "none");
}

#[test]
fn config_file_bell_action() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g bell-action none\n");
    assert_eq!(app.bell_action, "none");
}

#[test]
fn config_file_visual_bell_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g visual-bell on\n");
    assert!(app.visual_bell);
}

#[test]
fn config_file_monitor_silence() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g monitor-silence 30\n");
    assert_eq!(app.monitor_silence, 30);
}

#[test]
fn config_file_scroll_enter_copy_mode_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g scroll-enter-copy-mode off\n");
    assert!(!app.scroll_enter_copy_mode);
}

#[test]
fn config_file_pwsh_mouse_selection_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pwsh-mouse-selection on\n");
    assert!(app.pwsh_mouse_selection);
}

#[test]
fn config_file_synchronize_panes_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g synchronize-panes on\n");
    assert!(app.sync_input);
}

#[test]
fn config_file_synchronize_panes_off() {
    let mut app = mock_app();
    app.sync_input = true;
    parse_config_content(&mut app, "set -g synchronize-panes off\n");
    assert!(!app.sync_input);
}

#[test]
fn config_file_pane_border_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-border-style 'fg=colour245'\n");
    assert_eq!(app.pane_border_style, "fg=colour245");
}

#[test]
fn config_file_pane_active_border_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-active-border-style 'fg=cyan'\n");
    assert_eq!(app.pane_active_border_style, "fg=cyan");
}

#[test]
fn config_file_pane_border_hover_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-border-hover-style 'fg=red'\n");
    assert_eq!(app.pane_border_hover_style, "fg=red");
}

#[test]
fn config_file_window_status_format() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-format '#I:#W'\n");
    assert_eq!(app.window_status_format, "#I:#W");
}

#[test]
fn config_file_window_status_current_format() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-current-format '#[bold]#I:#W'\n");
    assert_eq!(app.window_status_current_format, "#[bold]#I:#W");
}

#[test]
fn config_file_window_status_separator() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-separator '|'\n");
    assert_eq!(app.window_status_separator, "|");
}

#[test]
fn config_file_window_status_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-style 'fg=white'\n");
    assert_eq!(app.window_status_style, "fg=white");
}

#[test]
fn config_file_window_status_current_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-current-style 'fg=yellow,bold'\n");
    assert_eq!(app.window_status_current_style, "fg=yellow,bold");
}

#[test]
fn config_file_window_status_activity_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-activity-style 'underscore'\n");
    assert_eq!(app.window_status_activity_style, "underscore");
}

#[test]
fn config_file_window_status_bell_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-bell-style 'blink'\n");
    assert_eq!(app.window_status_bell_style, "blink");
}

#[test]
fn config_file_window_status_last_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-status-last-style 'dim'\n");
    assert_eq!(app.window_status_last_style, "dim");
}

#[test]
fn config_file_message_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g message-style 'fg=red,bg=black'\n");
    assert_eq!(app.message_style, "fg=red,bg=black");
}

#[test]
fn config_file_message_command_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g message-command-style 'fg=blue'\n");
    assert_eq!(app.message_command_style, "fg=blue");
}

#[test]
fn config_file_mode_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mode-style 'bg=red,fg=white'\n");
    assert_eq!(app.mode_style, "bg=red,fg=white");
}

#[test]
fn config_file_main_pane_width() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g main-pane-width 80\n");
    assert_eq!(app.main_pane_width, 80);
}

#[test]
fn config_file_main_pane_height() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g main-pane-height 40\n");
    assert_eq!(app.main_pane_height, 40);
}

#[test]
fn config_file_window_size() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-size smallest\n");
    assert_eq!(app.window_size, "smallest");
}

#[test]
fn config_file_allow_passthrough_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g allow-passthrough on\n");
    assert_eq!(app.allow_passthrough, "on");
}

#[test]
fn config_file_allow_passthrough_all() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g allow-passthrough all\n");
    assert_eq!(app.allow_passthrough, "all");
}

#[test]
fn config_file_copy_command() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g copy-command 'clip.exe'\n");
    assert_eq!(app.copy_command, "clip.exe");
}

#[test]
fn config_file_set_clipboard_external() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g set-clipboard external\n");
    assert_eq!(app.set_clipboard, "external");
}

#[test]
fn config_file_set_clipboard_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g set-clipboard off\n");
    assert_eq!(app.set_clipboard, "off");
}

#[test]
fn config_file_env_shim_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g env-shim off\n");
    assert!(!app.env_shim);
}

#[test]
fn config_file_claude_code_fix_tty_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g claude-code-fix-tty off\n");
    assert!(!app.claude_code_fix_tty);
}

#[test]
fn config_file_claude_code_force_interactive_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g claude-code-force-interactive off\n");
    assert!(!app.claude_code_force_interactive);
}

#[test]
fn config_file_warm_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g warm off\n");
    assert!(!app.warm_enabled);
}

#[test]
fn config_file_warm_on() {
    let mut app = mock_app();
    app.warm_enabled = false;
    parse_config_content(&mut app, "set -g warm on\n");
    assert!(app.warm_enabled);
}

#[test]
fn config_file_default_shell() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g default-shell pwsh\n");
    assert_eq!(app.default_shell, "pwsh");
}

#[test]
fn config_file_default_command() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g default-command \"pwsh -NoProfile\"\n");
    assert_eq!(app.default_shell, "pwsh -NoProfile");
}

#[test]
fn config_file_prediction_dimming_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g prediction-dimming on\n");
    assert!(app.prediction_dimming);
}

#[test]
fn config_file_dim_predictions_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g dim-predictions on\n");
    assert!(app.prediction_dimming);
}

#[test]
fn config_file_allow_predictions_on() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g allow-predictions on\n");
    assert!(app.allow_predictions);
}

#[test]
fn config_file_update_environment() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g update-environment \"FOO BAR BAZ\"\n");
    assert_eq!(app.update_environment, vec!["FOO", "BAR", "BAZ"]);
}

#[test]
fn config_file_default_terminal() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g default-terminal xterm-256color\n");
    assert_eq!(app.environment.get("TERM").unwrap(), "xterm-256color");
}

#[test]
fn config_file_terminal_overrides_accepted() {
    // terminal-overrides is a no-op on Windows but should not error
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g terminal-overrides ',xterm*:Tc'\n");
    // No crash, no error
}

#[test]
fn config_file_command_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g command-alias split-pane=split-window\n");
    assert_eq!(app.command_aliases.get("split-pane").unwrap(), "split-window");
}

#[test]
fn config_file_status_format_indexed() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-format[0] 'line zero'\n");
    assert_eq!(app.status_format[0], "line zero");
}

#[test]
fn config_file_status_format_indexed_1() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-format[1] 'line one'\n");
    assert!(app.status_format.len() >= 2);
    assert_eq!(app.status_format[1], "line one");
}

#[test]
fn config_file_user_option_at_prefix() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @catppuccin_flavor mocha\n");
    assert_eq!(app.user_options.get("@catppuccin_flavor").unwrap(), "mocha");
}

#[test]
fn config_file_user_option_stored_in_user_options() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @my-plugin-opt value\n");
    // Should be in user_options, NOT in environment (issue #105)
    assert!(app.user_options.contains_key("@my-plugin-opt"));
    assert!(!app.environment.contains_key("@my-plugin-opt"));
}

#[test]
fn config_file_hyphenated_option_stored_in_user_options() {
    // Options with hyphens that are not recognized go to user_options, not environment (#137)
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g clock-mode-colour red\n");
    assert!(app.user_options.contains_key("clock-mode-colour"));
    assert!(!app.environment.contains_key("clock-mode-colour"));
}

#[test]
fn config_file_popup_style_stored_in_user_options() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g popup-style 'fg=white'\n");
    assert_eq!(app.user_options.get("popup-style").unwrap(), "fg=white");
}

#[test]
fn config_file_popup_border_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g popup-border-style 'fg=yellow'\n");
    assert_eq!(app.user_options.get("popup-border-style").unwrap(), "fg=yellow");
}

#[test]
fn config_file_popup_border_lines() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g popup-border-lines rounded\n");
    assert_eq!(app.user_options.get("popup-border-lines").unwrap(), "rounded");
}

#[test]
fn config_file_window_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-style 'bg=black'\n");
    assert_eq!(app.user_options.get("window-style").unwrap(), "bg=black");
}

#[test]
fn config_file_window_active_style() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g window-active-style 'bg=colour235'\n");
    assert_eq!(app.user_options.get("window-active-style").unwrap(), "bg=colour235");
}

#[test]
fn config_only_if_unset_sets_window_style_after_unset() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "set -g window-style 'bg=black'\n\
         set -gu window-style\n\
         set -go window-style 'bg=colour235'\n",
    );
    assert_eq!(app.user_options.get("window-style").unwrap(), "bg=colour235");
}

#[test]
fn config_file_wrap_search() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g wrap-search on\n");
    assert_eq!(app.user_options.get("wrap-search").unwrap(), "on");
}

#[test]
fn config_file_lock_options() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g lock-after-time 300\n");
    assert_eq!(app.user_options.get("lock-after-time").unwrap(), "300");
}

#[test]
fn config_file_pane_border_format() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-border-format '#{pane_index}'\n");
    assert_eq!(app.user_options.get("pane-border-format").unwrap(), "#{pane_index}");
}

#[test]
fn config_file_pane_border_status() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g pane-border-status top\n");
    assert_eq!(app.user_options.get("pane-border-status").unwrap(), "top");
}

// ============================================================
// SECTION 3: set-option / set aliases (set, set-option, setw, set-window-option)
// ============================================================

#[test]
fn config_set_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_set_option_full() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-option -g mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_setw_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "setw -g mode-keys vi\n");
    assert_eq!(app.mode_keys, "vi");
}

#[test]
fn config_set_window_option_full() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-window-option -g monitor-activity on\n");
    assert!(app.monitor_activity);
}

#[test]
fn config_set_without_g_flag() {
    // set without -g should still work (treated as global in single-server model)
    let mut app = mock_app();
    parse_config_content(&mut app, "set mouse off\n");
    assert!(!app.mouse_enabled);
}

// ============================================================
// SECTION 4: Option flags (-g, -u, -a, -q, -o, -w, -F, combined)
// via parse_config_content
// ============================================================

#[test]
fn config_flag_g_global() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 100\n");
    assert_eq!(app.escape_time_ms, 100);
}

#[test]
fn config_flag_u_unset_user_option() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @test hello\nset -gu @test\n");
    // -gu sets @user option to empty string
    assert_eq!(app.user_options.get("@test").unwrap(), "");
}

#[test]
fn config_flag_u_unset_numeric_option() {
    // -u on numeric options: tries to parse empty string as number, silently fails
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 100\nset -gu escape-time\n");
    // escape-time stays at 100 because "".parse::<u64>() fails
    assert_eq!(app.escape_time_ms, 100);
}

#[test]
fn config_flag_u_unset_string_option() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left HELLO\nset -gu status-left\n");
    // -u on string option sets to empty string
    assert_eq!(app.status_left, "");
}

#[test]
fn config_flag_a_append_string() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-right AAA\nset -ga status-right BBB\n");
    assert_eq!(app.status_right, "AAABBB");
}

#[test]
fn config_flag_a_append_user_option() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @list one\nset -ga @list ,two\n");
    assert_eq!(app.user_options.get("@list").unwrap(), "one,two");
}

#[test]
fn config_flag_q_quiet() {
    // -q should silently accept unknown options
    let mut app = mock_app();
    parse_config_content(&mut app, "set -gq nonexistent-unknown value\n");
    // No crash, option stored in user_options because it has hyphens
    assert!(app.user_options.contains_key("nonexistent-unknown"));
}

#[test]
fn config_flag_o_only_if_unset_already_set() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 100\nset -go escape-time 999\n");
    // -o should not overwrite because escape-time is already set
    assert_eq!(app.escape_time_ms, 100);
}

#[test]
fn config_flag_o_only_if_unset_not_set() {
    let mut app = mock_app();
    // @my-new-opt is not set yet
    parse_config_content(&mut app, "set -go @my-new-opt first\n");
    assert_eq!(app.user_options.get("@my-new-opt").unwrap(), "first");
}

#[test]
fn config_flag_o_does_not_overwrite() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @myopt AAA\nset -go @myopt BBB\n");
    // Second set should NOT overwrite
    assert_eq!(app.user_options.get("@myopt").unwrap(), "AAA");
}

#[test]
fn config_flag_w_window_scope() {
    // -w is treated same as -g in our single-server model
    let mut app = mock_app();
    parse_config_content(&mut app, "set -w mouse on\n");
    assert!(app.mouse_enabled);
}

#[test]
fn config_flag_F_format_expand() {
    let mut app = mock_app();
    app.session_name = "mysession".to_string();
    parse_config_content(&mut app, "set -gF status-left '#{session_name}'\n");
    assert_eq!(app.status_left, "mysession");
}

#[test]
fn config_combined_flags_gu() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @x hello\nset -gu @x\n");
    assert_eq!(app.user_options.get("@x").unwrap(), "");
}

#[test]
fn config_combined_flags_ga() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left A\nset -ga status-left B\n");
    assert_eq!(app.status_left, "AB");
}

#[test]
fn config_combined_flags_go() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 42\nset -go escape-time 999\n");
    assert_eq!(app.escape_time_ms, 42);
}

#[test]
fn config_flag_t_target_consumed() {
    let mut app = mock_app();
    // -t <target> should be consumed and not treated as option name
    parse_config_content(&mut app, "set -t 0 -g mouse off\n");
    assert!(!app.mouse_enabled);
}

// ============================================================
// SECTION 5: Every option via execute_command_string (CLI path)
// ============================================================

#[test]
fn cli_set_mouse() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g mouse off").unwrap();
    assert!(!app.mouse_enabled);
}

#[test]
fn cli_set_escape_time() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g escape-time 200").unwrap();
    assert_eq!(app.escape_time_ms, 200);
}

#[test]
fn cli_set_status_off() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g status off").unwrap();
    assert!(!app.status_visible);
}

#[test]
fn cli_set_status_position() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g status-position top").unwrap();
    assert_eq!(app.status_position, "top");
}

#[test]
fn cli_set_status_style() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g status-style "bg=red""#).unwrap();
    assert_eq!(app.status_style, "bg=red");
}

#[test]
fn cli_set_base_index() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g base-index 1").unwrap();
    assert_eq!(app.window_base_index, 1);
}

#[test]
fn cli_set_history_limit() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g history-limit 10000").unwrap();
    assert_eq!(app.history_limit, 10000);
}

#[test]
fn cli_set_focus_events() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g focus-events on").unwrap();
    assert!(app.focus_events);
}

#[test]
fn cli_set_mode_keys() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g mode-keys vi").unwrap();
    assert_eq!(app.mode_keys, "vi");
}

#[test]
fn cli_set_renumber_windows() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g renumber-windows on").unwrap();
    assert!(app.renumber_windows);
}

#[test]
fn cli_set_pane_border_style() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g pane-border-style "fg=grey""#).unwrap();
    assert_eq!(app.pane_border_style, "fg=grey");
}

#[test]
fn cli_set_window_status_format() {
    let mut app = mock_app();
    execute_command_string(&mut app, r##"set-option -g window-status-format "#I""##).unwrap();
    assert_eq!(app.window_status_format, "#I");
}

#[test]
fn cli_set_message_style() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g message-style "fg=white""#).unwrap();
    assert_eq!(app.message_style, "fg=white");
}

#[test]
fn cli_set_mode_style() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g mode-style "bg=blue""#).unwrap();
    assert_eq!(app.mode_style, "bg=blue");
}

#[test]
fn cli_set_status_interval() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g status-interval 1").unwrap();
    assert_eq!(app.status_interval, 1);
}

#[test]
fn cli_set_status_justify() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g status-justify centre").unwrap();
    assert_eq!(app.status_justify, "centre");
}

#[test]
fn cli_set_main_pane_width() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g main-pane-width 60").unwrap();
    assert_eq!(app.main_pane_width, 60);
}

#[test]
fn cli_set_main_pane_height() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g main-pane-height 30").unwrap();
    assert_eq!(app.main_pane_height, 30);
}

#[test]
fn cli_set_display_time() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g display-time 5000").unwrap();
    assert_eq!(app.display_time_ms, 5000);
}

#[test]
fn cli_set_window_size() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g window-size largest").unwrap();
    assert_eq!(app.window_size, "largest");
}

#[test]
fn cli_set_copy_command() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g copy-command "pbcopy""#).unwrap();
    assert_eq!(app.copy_command, "pbcopy");
}

#[test]
fn cli_set_allow_passthrough() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g allow-passthrough on").unwrap();
    assert_eq!(app.allow_passthrough, "on");
}

#[test]
fn cli_set_command_alias() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g command-alias sp=split-window").unwrap();
    assert_eq!(app.command_aliases.get("sp").unwrap(), "split-window");
}

#[test]
fn cli_set_env_shim() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g env-shim off").unwrap();
    assert!(!app.env_shim);
}

#[test]
fn cli_set_warm() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g warm off").unwrap();
    assert!(!app.warm_enabled);
}

#[test]
fn cli_set_user_option() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g @theme-color blue").unwrap();
    assert_eq!(app.user_options.get("@theme-color").unwrap(), "blue");
}

// ============================================================
// SECTION 6: Every option via parse_config_line (direct path)
// ============================================================

#[test]
fn direct_set_mouse() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g mouse off");
    assert!(!app.mouse_enabled);
}

#[test]
fn direct_set_escape_time() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g escape-time 75");
    assert_eq!(app.escape_time_ms, 75);
}

#[test]
fn direct_set_status() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g status off");
    assert!(!app.status_visible);
}

#[test]
fn direct_bind_key() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind-key x kill-pane");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('x')));
}

#[test]
fn direct_unbind_key() {
    let mut app = mock_app();
    parse_config_line(&mut app, "bind-key y kill-pane");
    parse_config_line(&mut app, "unbind-key y");
    let empty = vec![];
    let prefix = app.key_tables.get("prefix").unwrap_or(&empty);
    assert!(!prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('y')));
}

#[test]
fn direct_set_hook() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set-hook -g after-new-window 'run echo hi'");
    assert!(app.hooks.contains_key("after-new-window"));
}

#[test]
fn direct_set_environment() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set-environment MY_VAR myvalue");
    assert_eq!(app.environment.get("MY_VAR").unwrap(), "myvalue");
}

#[test]
fn direct_setenv_alias() {
    let mut app = mock_app();
    parse_config_line(&mut app, "setenv MY_VAR2 myvalue2");
    assert_eq!(app.environment.get("MY_VAR2").unwrap(), "myvalue2");
}

// ============================================================
// SECTION 7: Config parsing features
// ============================================================

// --- Comments and empty lines ---

#[test]
fn config_skips_comments() {
    let mut app = mock_app();
    parse_config_content(&mut app, "# This is a comment\nset -g mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_skips_empty_lines() {
    let mut app = mock_app();
    parse_config_content(&mut app, "\n\n\nset -g mouse off\n\n\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_comment_after_not_parsed() {
    // Comments must be at start of line; inline comments are part of the value in tmux
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left hello\n# comment\n");
    assert_eq!(app.status_left, "hello");
}

// --- Continuation lines (backslash at end) ---

#[test]
fn config_continuation_line_basic() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g \\\nstatus-left \\\nHELLO\n");
    assert_eq!(app.status_left, "HELLO");
}

#[test]
fn config_continuation_line_bind() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key \\\nx \\\nkill-pane\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('x')));
}

#[test]
fn config_continuation_line_three_lines() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set \\\n-g \\\nescape-time \\\n100\n");
    assert_eq!(app.escape_time_ms, 100);
}

#[test]
fn config_continuation_at_eof() {
    // If file ends with a continuation, the partial line should still be processed
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g \\\nmouse off");
    assert!(!app.mouse_enabled);
}

// --- %if / %elif / %else / %endif conditional blocks ---

#[test]
fn config_if_true_executes() {
    let mut app = mock_app();
    app.session_name = "test".to_string();
    // A non-empty, non-zero condition is truthy
    parse_config_content(&mut app, "%if \"1\"\nset -g mouse off\n%endif\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_if_false_skips() {
    let mut app = mock_app();
    // Empty or "0" is falsy
    parse_config_content(&mut app, "%if \"0\"\nset -g mouse off\n%endif\n");
    // mouse should remain default (on)
    assert!(app.mouse_enabled);
}

#[test]
fn config_if_empty_is_false() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"\"\nset -g mouse off\n%endif\n");
    assert!(app.mouse_enabled);
}

#[test]
fn config_if_else_true_branch() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"1\"\nset -g escape-time 111\n%else\nset -g escape-time 222\n%endif\n");
    assert_eq!(app.escape_time_ms, 111);
}

#[test]
fn config_if_else_false_branch() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\nset -g escape-time 111\n%else\nset -g escape-time 222\n%endif\n");
    assert_eq!(app.escape_time_ms, 222);
}

#[test]
fn config_elif_first_true() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"1\"\nset -g escape-time 100\n%elif \"1\"\nset -g escape-time 200\n%else\nset -g escape-time 300\n%endif\n");
    assert_eq!(app.escape_time_ms, 100);
}

#[test]
fn config_elif_second_true() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\nset -g escape-time 100\n%elif \"1\"\nset -g escape-time 200\n%else\nset -g escape-time 300\n%endif\n");
    assert_eq!(app.escape_time_ms, 200);
}

#[test]
fn config_elif_else_branch() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\nset -g escape-time 100\n%elif \"0\"\nset -g escape-time 200\n%else\nset -g escape-time 300\n%endif\n");
    assert_eq!(app.escape_time_ms, 300);
}

#[test]
fn config_nested_if() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"1\"\n%if \"1\"\nset -g escape-time 999\n%endif\n%endif\n");
    assert_eq!(app.escape_time_ms, 999);
}

#[test]
fn config_nested_if_outer_false() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\n%if \"1\"\nset -g escape-time 999\n%endif\n%endif\n");
    // Should remain default because outer %if is false
    assert_eq!(app.escape_time_ms, 500);
}

#[test]
fn config_nested_if_inner_false() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"1\"\n%if \"0\"\nset -g escape-time 999\n%endif\nset -g escape-time 111\n%endif\n");
    assert_eq!(app.escape_time_ms, 111);
}

#[test]
fn config_if_with_format_condition() {
    let mut app = mock_app();
    app.session_name = "mysess".to_string();
    // #{session_name} expands to "mysess" which is truthy
    parse_config_content(&mut app, "%if \"#{session_name}\"\nset -g escape-time 777\n%endif\n");
    assert_eq!(app.escape_time_ms, 777);
}

#[test]
fn config_if_after_endif_still_active() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\nset -g escape-time 111\n%endif\nset -g escape-time 222\n");
    // Lines after %endif should be active
    assert_eq!(app.escape_time_ms, 222);
}

// --- %hidden variables and $NAME expansion ---

#[test]
fn config_hidden_basic() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%hidden MY_COLOR=blue\nset -g status-style $MY_COLOR\n");
    assert_eq!(app.status_style, "blue");
}

#[test]
fn config_hidden_in_environment() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%hidden THEME=dark\n");
    assert_eq!(app.environment.get("THEME").unwrap(), "dark");
}

#[test]
fn config_hidden_dollar_brace_syntax() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%hidden COLOR=red\nset -g status-style ${COLOR}\n");
    assert_eq!(app.status_style, "red");
}

#[test]
fn config_hidden_multiple_vars() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%hidden FG=white\n%hidden BG=black\nset -g status-style fg=$FG,bg=$BG\n");
    assert_eq!(app.status_style, "fg=white,bg=black");
}

#[test]
fn config_hidden_undefined_var_literal() {
    // Undefined $VAR should remain literal
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-style $UNDEFINED_VAR_XYZ\n");
    assert_eq!(app.status_style, "$UNDEFINED_VAR_XYZ");
}

#[test]
fn config_hidden_inside_if_false() {
    // %hidden in a false %if block should NOT be defined
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"0\"\n%hidden NOPE=yes\n%endif\n");
    assert!(!app.environment.contains_key("NOPE"));
}

#[test]
fn config_hidden_inside_if_true() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%if \"1\"\n%hidden YES=yep\n%endif\n");
    assert_eq!(app.environment.get("YES").unwrap(), "yep");
}

#[test]
fn config_hidden_quoted_value() {
    let mut app = mock_app();
    parse_config_content(&mut app, "%hidden GREETING=\"hello world\"\n");
    assert_eq!(app.environment.get("GREETING").unwrap(), "hello world");
}

// --- UTF-8 BOM handling ---

#[test]
fn config_utf8_bom_stripped() {
    let mut app = mock_app();
    parse_config_content(&mut app, "\u{FEFF}set -g mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_utf8_bom_first_line_works() {
    let mut app = mock_app();
    parse_config_content(&mut app, "\u{FEFF}set -g escape-time 42\n");
    assert_eq!(app.escape_time_ms, 42);
}

// --- bind-key via config file ---

#[test]
fn config_bind_key_basic() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key r source-file\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('r')));
}

#[test]
fn config_bind_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind s choose-tree\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('s')));
}

#[test]
fn config_bind_n_root_table() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key -n F5 kill-pane\n");
    let root = app.key_tables.get("root").unwrap();
    assert!(root.iter().any(|b| b.key.0 == crossterm::event::KeyCode::F(5)));
}

#[test]
fn config_bind_T_custom_table() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key -T mymenu x kill-pane\n");
    let tab = app.key_tables.get("mymenu").unwrap();
    assert!(tab.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('x')));
}

#[test]
fn config_bind_r_repeat() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key -r Up select-pane -U\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    let b = prefix.iter().find(|b| b.key.0 == crossterm::event::KeyCode::Up).unwrap();
    assert!(b.repeat);
}

#[test]
fn config_bind_command_chain() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key x split-window \\; select-pane -D\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    let b = prefix.iter().find(|b| b.key.0 == crossterm::event::KeyCode::Char('x')).unwrap();
    match &b.action {
        crate::types::Action::CommandChain(cmds) => {
            assert_eq!(cmds.len(), 2);
        }
        _ => panic!("Expected CommandChain"),
    }
}

#[test]
fn config_bind_ctrl_modifier() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key C-a send-prefix\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| {
        b.key.0 == crossterm::event::KeyCode::Char('a')
            && b.key.1.contains(crossterm::event::KeyModifiers::CONTROL)
    }));
}

#[test]
fn config_bind_alt_modifier() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key M-h select-pane -L\n");
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| {
        b.key.0 == crossterm::event::KeyCode::Char('h')
            && b.key.1.contains(crossterm::event::KeyModifiers::ALT)
    }));
}

// --- unbind-key via config file ---

#[test]
fn config_unbind_specific_key() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key z kill-pane\nunbind-key z\n");
    let empty = vec![];
    let prefix = app.key_tables.get("prefix").unwrap_or(&empty);
    assert!(!prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('z')));
}

#[test]
fn config_unbind_all() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key a kill-pane\nbind-key b kill-pane\nunbind-key -a\n");
    assert!(app.key_tables.is_empty() || app.key_tables.values().all(|v| v.is_empty()));
}

#[test]
fn config_unbind_n_root() {
    let mut app = mock_app();
    parse_config_content(&mut app, "bind-key -n F5 kill-pane\nunbind-key -n F5\n");
    let root = app.key_tables.get("root").unwrap();
    assert!(!root.iter().any(|b| b.key.0 == crossterm::event::KeyCode::F(5)));
}

// --- set-hook via config file ---

#[test]
fn config_set_hook_basic() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-hook -g after-new-session 'run echo hello'\n");
    assert!(app.hooks.contains_key("after-new-session"));
    assert_eq!(app.hooks["after-new-session"][0], "run echo hello");
}

#[test]
fn config_set_hook_append() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-hook -g after-new-session 'run echo a'\nset-hook -ga after-new-session 'run echo b'\n");
    assert_eq!(app.hooks["after-new-session"].len(), 2);
}

#[test]
fn config_set_hook_unset() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-hook -g after-new-session 'run echo a'\nset-hook -gu after-new-session\n");
    assert!(!app.hooks.contains_key("after-new-session"));
}

#[test]
fn config_set_hook_replace_no_duplicates() {
    // Without -a, set-hook should replace (not append) to prevent duplicates on reload
    let mut app = mock_app();
    parse_config_content(&mut app, "set-hook -g my-hook 'cmd1'\nset-hook -g my-hook 'cmd2'\n");
    assert_eq!(app.hooks["my-hook"].len(), 1);
    assert_eq!(app.hooks["my-hook"][0], "cmd2");
}

// --- set-environment via config file ---

#[test]
fn config_set_environment_basic() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-environment MY_VAR hello\n");
    assert_eq!(app.environment.get("MY_VAR").unwrap(), "hello");
}

#[test]
fn config_setenv_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "setenv FOO bar\n");
    assert_eq!(app.environment.get("FOO").unwrap(), "bar");
}

#[test]
fn config_set_environment_with_flags() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set-environment -g GLOBAL_VAR gval\n");
    assert_eq!(app.environment.get("GLOBAL_VAR").unwrap(), "gval");
}

// --- source-file via config file ---

#[test]
fn config_source_file_via_config_line() {
    // source-file in a config should be recognized as a command
    let mut app = mock_app();
    // Source a nonexistent file should not crash
    parse_config_content(&mut app, "source-file /nonexistent/path/xyz.conf\n");
    // No crash
}

#[test]
fn config_source_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "source /nonexistent/path/abc.conf\n");
    // No crash
}

// --- run-shell via config file ---

#[test]
fn config_run_shell_recognized() {
    let mut app = mock_app();
    // run-shell is recognized but may not execute in test context
    parse_config_content(&mut app, "run-shell 'echo test'\n");
    // No crash
}

#[test]
fn config_run_alias() {
    let mut app = mock_app();
    parse_config_content(&mut app, "run 'echo test'\n");
    // No crash
}

// --- if-shell via config file ---

#[test]
fn config_if_shell_recognized() {
    let mut app = mock_app();
    // if-shell in config file context
    parse_config_content(&mut app, "if-shell 'true' 'set -g mouse off'\n");
    // The if-shell command is dispatched; in config context it may or may not execute
    // depending on shell availability, but it should not crash
}

// ============================================================
// SECTION 8: Multi-line config files (realistic configs)
// ============================================================

#[test]
fn config_realistic_minimal() {
    let mut app = mock_app();
    let config = r#"
# Minimal config
set -g mouse on
set -g escape-time 50
set -g base-index 1
set -g pane-base-index 1
set -g status-position top
set -g history-limit 10000
"#;
    parse_config_content(&mut app, config);
    assert!(app.mouse_enabled);
    assert_eq!(app.escape_time_ms, 50);
    assert_eq!(app.window_base_index, 1);
    assert_eq!(app.pane_base_index, 1);
    assert_eq!(app.status_position, "top");
    assert_eq!(app.history_limit, 10000);
}

#[test]
fn config_realistic_with_bindings() {
    let mut app = mock_app();
    let config = r#"
# Prefix + bindings
set -g mouse on
set -g prefix C-a
bind-key r source-file
bind-key | split-window -h
bind-key - split-window -v
bind-key -r Up select-pane -U
bind-key -r Down select-pane -D
"#;
    parse_config_content(&mut app, config);
    assert!(app.mouse_enabled);
    assert_eq!(app.prefix_key.0, crossterm::event::KeyCode::Char('a'));
    assert!(app.prefix_key.1.contains(crossterm::event::KeyModifiers::CONTROL));
    let prefix = app.key_tables.get("prefix").unwrap();
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('|')));
    assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('-')));
    // Repeatable bindings
    let up = prefix.iter().find(|b| b.key.0 == crossterm::event::KeyCode::Up).unwrap();
    assert!(up.repeat);
}

#[test]
fn config_realistic_with_styles() {
    let mut app = mock_app();
    let config = r#"
set -g status-style 'bg=#1e1e2e,fg=#cdd6f4'
set -g pane-border-style 'fg=#45475a'
set -g pane-active-border-style 'fg=#89b4fa'
set -g message-style 'bg=#313244,fg=#cdd6f4'
set -g mode-style 'bg=#45475a,fg=#cdd6f4'
set -g window-status-current-style 'fg=#89b4fa,bold'
set -g window-status-style 'fg=#6c7086'
"#;
    parse_config_content(&mut app, config);
    assert!(app.status_style.contains("bg=#1e1e2e"));
    assert!(app.pane_border_style.contains("fg=#45475a"));
    assert!(app.pane_active_border_style.contains("fg=#89b4fa"));
    assert!(app.message_style.contains("bg=#313244"));
    assert!(app.mode_style.contains("bg=#45475a"));
    assert!(app.window_status_current_style.contains("fg=#89b4fa"));
    assert!(app.window_status_style.contains("fg=#6c7086"));
}

#[test]
fn config_realistic_with_conditionals() {
    let mut app = mock_app();
    let config = r#"
%hidden MY_ESCAPE=50
set -g escape-time $MY_ESCAPE
%if "1"
set -g mouse on
%else
set -g mouse off
%endif
"#;
    parse_config_content(&mut app, config);
    assert_eq!(app.escape_time_ms, 50);
    assert!(app.mouse_enabled);
}

#[test]
fn config_realistic_plugin_option() {
    let mut app = mock_app();
    let config = r##"
# Theme plugin
set -g @catppuccin_flavor mocha
set -g @catppuccin_status_modules_right "directory user host session"
set -g @catppuccin_window_default_text "#W"
set -g @catppuccin_window_current_text "#W"
"##;
    parse_config_content(&mut app, config);
    assert_eq!(app.user_options["@catppuccin_flavor"], "mocha");
    assert!(app.user_options["@catppuccin_status_modules_right"].contains("directory"));
    assert_eq!(app.user_options["@catppuccin_window_default_text"], "#W");
    assert_eq!(app.user_options["@catppuccin_window_current_text"], "#W");
}

#[test]
fn config_realistic_with_continuations() {
    let mut app = mock_app();
    let config = "set -g \\\nstatus-right \\\n\"#H %R\"\n";
    parse_config_content(&mut app, config);
    assert_eq!(app.status_right, "#H %R");
}

// ============================================================
// SECTION 9: Edge cases
// ============================================================

#[test]
fn config_empty_content() {
    let mut app = mock_app();
    parse_config_content(&mut app, "");
    // Should not crash, defaults preserved
    assert!(app.mouse_enabled);
}

#[test]
fn config_only_comments() {
    let mut app = mock_app();
    parse_config_content(&mut app, "# comment 1\n# comment 2\n# comment 3\n");
    // Should not crash, defaults preserved
    assert!(app.mouse_enabled);
}

#[test]
fn config_only_whitespace() {
    let mut app = mock_app();
    parse_config_content(&mut app, "   \n   \n   \n");
    assert!(app.mouse_enabled);
}

#[test]
fn config_duplicate_option_last_wins() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 100\nset -g escape-time 200\nset -g escape-time 300\n");
    assert_eq!(app.escape_time_ms, 300);
}

#[test]
fn config_invalid_numeric_ignored() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time notanumber\n");
    // Should remain default
    assert_eq!(app.escape_time_ms, 500);
}

#[test]
fn config_boolean_true_variants() {
    let mut app = mock_app();
    app.mouse_enabled = false;
    parse_config_content(&mut app, "set -g mouse true\n");
    assert!(app.mouse_enabled);

    app.mouse_enabled = false;
    parse_config_content(&mut app, "set -g mouse 1\n");
    assert!(app.mouse_enabled);

    app.mouse_enabled = false;
    parse_config_content(&mut app, "set -g mouse on\n");
    assert!(app.mouse_enabled);
}

#[test]
fn config_boolean_false_variants() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g mouse false\n");
    assert!(!app.mouse_enabled);

    app.mouse_enabled = true;
    parse_config_content(&mut app, "set -g mouse 0\n");
    assert!(!app.mouse_enabled);

    app.mouse_enabled = true;
    parse_config_content(&mut app, "set -g mouse off\n");
    assert!(!app.mouse_enabled);
}

#[test]
fn config_quoted_value_double() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left \"hello world\"\n");
    assert_eq!(app.status_left, "hello world");
}

#[test]
fn config_quoted_value_single() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left 'hello world'\n");
    assert_eq!(app.status_left, "hello world");
}

#[test]
fn config_unquoted_value() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left hello\n");
    assert_eq!(app.status_left, "hello");
}

#[test]
fn config_prefix_c_a() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g prefix C-a\n");
    assert_eq!(app.prefix_key.0, crossterm::event::KeyCode::Char('a'));
    assert!(app.prefix_key.1.contains(crossterm::event::KeyModifiers::CONTROL));
}

#[test]
fn config_prefix2() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g prefix2 C-s\n");
    let p2 = app.prefix2_key.unwrap();
    assert_eq!(p2.0, crossterm::event::KeyCode::Char('s'));
    assert!(p2.1.contains(crossterm::event::KeyModifiers::CONTROL));
}

#[test]
fn config_prefix2_none() {
    let mut app = mock_app();
    app.prefix2_key = Some((crossterm::event::KeyCode::Char('s'), crossterm::event::KeyModifiers::CONTROL));
    parse_config_content(&mut app, "set -g prefix2 none\n");
    assert!(app.prefix2_key.is_none());
}

#[test]
fn config_status_format_0_and_1() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-format[0] 'zero'\nset -g status-format[1] 'one'\n");
    assert_eq!(app.status_format[0], "zero");
    assert_eq!(app.status_format[1], "one");
}

#[test]
fn config_multiple_command_aliases() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g command-alias sp=split-window\nset -g command-alias nw=new-window\n");
    assert_eq!(app.command_aliases["sp"], "split-window");
    assert_eq!(app.command_aliases["nw"], "new-window");
}

// ============================================================
// SECTION 10: Cursor options (env-var based)
// ============================================================

#[test]
fn config_cursor_style_sets_env() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g cursor-style block\n");
    assert_eq!(std::env::var("PSMUX_CURSOR_STYLE").unwrap(), "block");
    // Clean up
    std::env::remove_var("PSMUX_CURSOR_STYLE");
}

#[test]
fn config_cursor_blink_on() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g cursor-blink on\n");
    assert_eq!(std::env::var("PSMUX_CURSOR_BLINK").unwrap(), "1");
    std::env::remove_var("PSMUX_CURSOR_BLINK");
}

#[test]
fn config_cursor_blink_off() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g cursor-blink off\n");
    assert_eq!(std::env::var("PSMUX_CURSOR_BLINK").unwrap(), "0");
    std::env::remove_var("PSMUX_CURSOR_BLINK");
}

// ============================================================
// SECTION 11: Cross-channel consistency (same option, all paths)
// ============================================================

#[test]
fn cross_channel_mouse_config_vs_cli() {
    // Config file path
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g mouse off\n");

    // CLI path
    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-option -g mouse off").unwrap();

    // Direct path
    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g mouse off");

    assert_eq!(app1.mouse_enabled, app2.mouse_enabled);
    assert_eq!(app2.mouse_enabled, app3.mouse_enabled);
    assert!(!app1.mouse_enabled);
}

#[test]
fn cross_channel_escape_time_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g escape-time 42\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-option -g escape-time 42").unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g escape-time 42");

    assert_eq!(app1.escape_time_ms, 42);
    assert_eq!(app2.escape_time_ms, 42);
    assert_eq!(app3.escape_time_ms, 42);
}

#[test]
fn cross_channel_status_style_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g status-style 'bg=red'\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, r#"set-option -g status-style "bg=red""#).unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g status-style 'bg=red'");

    assert_eq!(app1.status_style, "bg=red");
    assert_eq!(app2.status_style, "bg=red");
    assert_eq!(app3.status_style, "bg=red");
}

#[test]
fn cross_channel_base_index_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g base-index 1\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-option -g base-index 1").unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g base-index 1");

    assert_eq!(app1.window_base_index, 1);
    assert_eq!(app2.window_base_index, 1);
    assert_eq!(app3.window_base_index, 1);
}

#[test]
fn cross_channel_focus_events_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g focus-events on\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-option -g focus-events on").unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g focus-events on");

    assert!(app1.focus_events);
    assert!(app2.focus_events);
    assert!(app3.focus_events);
}

#[test]
fn cross_channel_user_option_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g @myopt val\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-option -g @myopt val").unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g @myopt val");

    assert_eq!(app1.user_options["@myopt"], "val");
    assert_eq!(app2.user_options["@myopt"], "val");
    assert_eq!(app3.user_options["@myopt"], "val");
}

#[test]
fn cross_channel_pane_border_style_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g pane-border-style 'fg=grey'\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, r#"set-option -g pane-border-style "fg=grey""#).unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g pane-border-style 'fg=grey'");

    assert_eq!(app1.pane_border_style, "fg=grey");
    assert_eq!(app2.pane_border_style, "fg=grey");
    assert_eq!(app3.pane_border_style, "fg=grey");
}

#[test]
fn cross_channel_window_status_format_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set -g window-status-format '#I'\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, r##"set-option -g window-status-format "#I""##).unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set -g window-status-format '#I'");

    assert_eq!(app1.window_status_format, "#I");
    assert_eq!(app2.window_status_format, "#I");
    assert_eq!(app3.window_status_format, "#I");
}

#[test]
fn cross_channel_bind_key_all_paths() {
    // Config file path
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "bind-key q kill-pane\n");

    // CLI path
    let mut app2 = mock_app();
    execute_command_string(&mut app2, "bind-key q kill-pane").unwrap();

    // Direct path
    let mut app3 = mock_app();
    parse_config_line(&mut app3, "bind-key q kill-pane");

    for app in [&app1, &app2, &app3] {
        let prefix = app.key_tables.get("prefix").unwrap();
        assert!(prefix.iter().any(|b| b.key.0 == crossterm::event::KeyCode::Char('q')));
    }
}

#[test]
fn cross_channel_hook_all_paths() {
    let mut app1 = mock_app();
    parse_config_content(&mut app1, "set-hook -g after-new-window 'run echo a'\n");

    let mut app2 = mock_app();
    execute_command_string(&mut app2, "set-hook -g after-new-window 'run echo a'").unwrap();

    let mut app3 = mock_app();
    parse_config_line(&mut app3, "set-hook -g after-new-window 'run echo a'");

    for app in [&app1, &app2, &app3] {
        assert!(app.hooks.contains_key("after-new-window"));
    }
}

// ============================================================
// SECTION 12: Flag combinations across paths
// ============================================================

#[test]
fn flag_append_via_config() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left A\nset -ga status-left B\nset -ga status-left C\n");
    assert_eq!(app.status_left, "ABC");
}

#[test]
fn flag_append_via_cli() {
    let mut app = mock_app();
    execute_command_string(&mut app, r#"set-option -g status-left "X""#).unwrap();
    execute_command_string(&mut app, r#"set-option -ga status-left "Y""#).unwrap();
    assert_eq!(app.status_left, "XY");
}

#[test]
fn flag_unset_then_set_via_config() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left HELLO\nset -gu status-left\nset -g status-left WORLD\n");
    assert_eq!(app.status_left, "WORLD");
}

#[test]
fn flag_format_via_config() {
    let mut app = mock_app();
    app.session_name = "sess123".to_string();
    parse_config_content(&mut app, "set -gF status-left '#{session_name}'\n");
    assert_eq!(app.status_left, "sess123");
}

#[test]
fn flag_format_via_cli() {
    let mut app = mock_app();
    app.session_name = "sess456".to_string();
    execute_command_string(&mut app, r##"set-option -gF status-left "#{session_name}""##).unwrap();
    assert_eq!(app.status_left, "sess456");
}

#[test]
fn flag_only_if_unset_via_config() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @opt1 first\nset -go @opt1 second\n");
    assert_eq!(app.user_options["@opt1"], "first");
}

#[test]
fn flag_only_if_unset_via_cli() {
    let mut app = mock_app();
    execute_command_string(&mut app, "set-option -g @opt2 first").unwrap();
    execute_command_string(&mut app, "set-option -go @opt2 second").unwrap();
    assert_eq!(app.user_options["@opt2"], "first");
}
