use super::*;
use crate::types::{AppState, ClientInfo};

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

// ════════════════════════════════════════════════════════════════════════════
//  Hook System Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn hook_before_new_window_found_in_hooks_map() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-new-window 'display-message creating'");
    assert!(app.hooks.contains_key("before-new-window"));
    assert_eq!(app.hooks["before-new-window"][0], "display-message creating");
}

#[test]
fn hook_before_split_window_found_in_hooks_map() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-split-window 'display-message splitting'");
    assert!(app.hooks.contains_key("before-split-window"));
    assert_eq!(app.hooks["before-split-window"][0], "display-message splitting");
}

#[test]
fn hook_before_kill_pane_found_in_hooks_map() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-kill-pane 'display-message killing'");
    assert!(app.hooks.contains_key("before-kill-pane"));
}

#[test]
fn hook_before_select_window_found_in_hooks_map() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-select-window 'display-message switching'");
    assert!(app.hooks.contains_key("before-select-window"));
}

#[test]
fn hook_before_rename_window_found_in_hooks_map() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-rename-window 'display-message renaming'");
    assert!(app.hooks.contains_key("before-rename-window"));
}

#[test]
fn hook_after_new_window_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window 'display-message created'");
    assert!(app.hooks.contains_key("after-new-window"));
    assert_eq!(app.hooks["after-new-window"][0], "display-message created");
}

#[test]
fn hook_after_split_window_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-split-window 'display-message split'");
    assert!(app.hooks.contains_key("after-split-window"));
}

#[test]
fn hook_after_kill_pane_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-kill-pane 'display-message killed'");
    assert!(app.hooks.contains_key("after-kill-pane"));
}

#[test]
fn hook_after_select_window_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-select-window 'display-message switched'");
    assert!(app.hooks.contains_key("after-select-window"));
}

#[test]
fn hook_after_resize_pane_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-resize-pane 'display-message resized'");
    assert!(app.hooks.contains_key("after-resize-pane"));
}

#[test]
fn hook_client_attached_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message hi'");
    assert!(app.hooks.contains_key("client-attached"));
}

#[test]
fn hook_session_created_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g session-created 'display-message new'");
    assert!(app.hooks.contains_key("session-created"));
}

#[test]
fn hook_pane_set_clipboard_still_works() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g pane-set-clipboard 'run-shell clip'");
    assert!(app.hooks.contains_key("pane-set-clipboard"));
}

#[test]
fn hook_multiple_commands_via_append() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window 'display-message first'");
    crate::config::parse_config_line(&mut app, "set-hook -ga after-new-window 'display-message second'");
    crate::config::parse_config_line(&mut app, "set-hook -ga after-new-window 'display-message third'");
    let cmds = app.hooks.get("after-new-window").unwrap();
    assert_eq!(cmds.len(), 3);
    assert_eq!(cmds[0], "display-message first");
    assert_eq!(cmds[1], "display-message second");
    assert_eq!(cmds[2], "display-message third");
}

#[test]
fn hook_before_and_after_coexist() {
    let mut app = mock_app();
    crate::config::parse_config_line(&mut app, "set-hook -g before-new-window 'display-message before'");
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window 'display-message after'");
    assert!(app.hooks.contains_key("before-new-window"));
    assert!(app.hooks.contains_key("after-new-window"));
    assert_eq!(app.hooks.len(), 2);
}

#[test]
fn hook_empty_hooks_map_by_default() {
    let app = mock_app();
    assert!(app.hooks.is_empty());
}

// ════════════════════════════════════════════════════════════════════════════
//  Client Registry Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn client_info_creation() {
    let info = ClientInfo {
        id: 1,
        width: 120,
        height: 30,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: "/dev/pts/0".to_string(),
        is_control: false,
        last_session: None,
    };
    assert_eq!(info.id, 1);
    assert_eq!(info.width, 120);
    assert_eq!(info.height, 30);
    assert_eq!(info.tty_name, "/dev/pts/0");
    assert!(!info.is_control);
}

#[test]
fn client_info_control_mode() {
    let info = ClientInfo {
        id: 5,
        width: 80,
        height: 24,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: "/dev/pts/3".to_string(),
        is_control: true,
        last_session: None,
    };
    assert!(info.is_control);
}

#[test]
fn client_registry_empty_by_default() {
    let app = mock_app();
    assert!(app.client_registry.is_empty());
}

#[test]
fn client_registry_add_client() {
    let mut app = mock_app();
    let info = ClientInfo {
        id: 1,
        width: 120,
        height: 30,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: "/dev/pts/0".to_string(),
        is_control: false,
        last_session: None,
    };
    app.client_registry.insert(1, info);
    assert_eq!(app.client_registry.len(), 1);
    assert!(app.client_registry.contains_key(&1));
}

#[test]
fn client_registry_add_multiple_clients() {
    let mut app = mock_app();
    for i in 0..5 {
        app.client_registry.insert(i, ClientInfo {
            id: i,
            width: 120,
            height: 30,
            connected_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            tty_name: format!("/dev/pts/{}", i),
            is_control: false,
            last_session: None,
        });
    }
    assert_eq!(app.client_registry.len(), 5);
}

#[test]
fn client_registry_remove_client() {
    let mut app = mock_app();
    app.client_registry.insert(1, ClientInfo {
        id: 1,
        width: 120,
        height: 30,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: "/dev/pts/0".to_string(),
        is_control: false,
        last_session: None,
    });
    app.client_registry.insert(2, ClientInfo {
        id: 2,
        width: 80,
        height: 24,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: "/dev/pts/1".to_string(),
        is_control: false,
        last_session: None,
    });
    assert_eq!(app.client_registry.len(), 2);
    app.client_registry.remove(&1);
    assert_eq!(app.client_registry.len(), 1);
    assert!(!app.client_registry.contains_key(&1));
    assert!(app.client_registry.contains_key(&2));
}

#[test]
fn attached_clients_initial_zero() {
    let app = mock_app();
    assert_eq!(app.attached_clients, 0);
}

#[test]
fn attached_clients_tracking() {
    let mut app = mock_app();
    app.attached_clients = 1;
    assert_eq!(app.attached_clients, 1);
    app.attached_clients += 1;
    assert_eq!(app.attached_clients, 2);
    app.attached_clients -= 1;
    assert_eq!(app.attached_clients, 1);
}

#[test]
fn client_sizes_tracks_per_client_dimensions() {
    let mut app = mock_app();
    app.client_sizes.insert(1, (120, 30));
    app.client_sizes.insert(2, (80, 24));
    assert_eq!(app.client_sizes[&1], (120, 30));
    assert_eq!(app.client_sizes[&2], (80, 24));
}

// ════════════════════════════════════════════════════════════════════════════
//  Option Catalog Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn option_catalog_build_returns_entries() {
    let app = mock_app();
    let options = crate::server::option_catalog::build_option_list(&app);
    assert!(!options.is_empty(), "option catalog should return entries");
}

#[test]
fn option_catalog_contains_common_options() {
    let app = mock_app();
    let options = crate::server::option_catalog::build_option_list(&app);
    let names: Vec<&str> = options.iter().map(|(n, _, _)| n.as_str()).collect();
    assert!(names.contains(&"escape-time"), "catalog should contain escape-time");
    assert!(names.contains(&"mouse"), "catalog should contain mouse");
    assert!(names.contains(&"prefix"), "catalog should contain prefix");
    assert!(names.contains(&"status"), "catalog should contain status");
    assert!(names.contains(&"base-index"), "catalog should contain base-index");
    assert!(names.contains(&"mode-keys"), "catalog should contain mode-keys");
}

#[test]
fn option_catalog_entries_have_scope() {
    let app = mock_app();
    let options = crate::server::option_catalog::build_option_list(&app);
    let scopes: Vec<&str> = options.iter().map(|(_, _, s)| s.as_str()).collect();
    assert!(scopes.contains(&"server"), "catalog should have server scope entries");
    assert!(scopes.contains(&"session"), "catalog should have session scope entries");
    assert!(scopes.contains(&"window"), "catalog should have window scope entries");
}

#[test]
fn option_catalog_default_for_escape_time() {
    let def = crate::server::option_catalog::default_for("escape-time");
    assert_eq!(def, Some("500"));
}

#[test]
fn option_catalog_default_for_mouse() {
    // This asserted "off" (tmux's default) while psmux actually starts with
    // `mouse_enabled: true`, so the catalog was advertising a default the
    // product does not have. Because customize-mode writes the catalog default
    // verbatim when you reset an option, that mismatch meant resetting `mouse`
    // silently turned the mouse OFF in a session the user never reconfigured.
    // psmux deliberately diverges from tmux here and enables the mouse by
    // default; the catalog has to mirror the runtime, not upstream tmux.
    // Enforced across every option by
    // `tests-rs/test_option_default_parity.rs`.
    let def = crate::server::option_catalog::default_for("mouse");
    assert_eq!(def, Some("on"));
}

#[test]
fn option_catalog_default_for_status() {
    let def = crate::server::option_catalog::default_for("status");
    assert_eq!(def, Some("on"));
}

#[test]
fn option_catalog_default_for_mode_keys() {
    let def = crate::server::option_catalog::default_for("mode-keys");
    assert_eq!(def, Some("emacs"));
}

#[test]
fn option_catalog_default_for_unknown_returns_none() {
    let def = crate::server::option_catalog::default_for("nonexistent-option");
    assert_eq!(def, None);
}

#[test]
fn option_catalog_all_entries_have_valid_types() {
    let valid_types = ["number", "boolean", "choice", "string"];
    for def in crate::server::option_catalog::OPTION_CATALOG {
        assert!(
            valid_types.contains(&def.option_type),
            "option '{}' has invalid type '{}' (expected one of {:?})",
            def.name, def.option_type, valid_types
        );
    }
}

#[test]
fn option_catalog_all_entries_have_valid_scopes() {
    let valid_scopes = ["server", "session", "window", "pane"];
    for def in crate::server::option_catalog::OPTION_CATALOG {
        assert!(
            valid_scopes.contains(&def.scope),
            "option '{}' has invalid scope '{}' (expected one of {:?})",
            def.name, def.scope, valid_scopes
        );
    }
}

#[test]
fn option_catalog_no_duplicate_names() {
    let mut seen = std::collections::HashSet::new();
    for def in crate::server::option_catalog::OPTION_CATALOG {
        assert!(
            seen.insert(def.name),
            "duplicate option name in catalog: '{}'", def.name
        );
    }
}

#[test]
fn option_catalog_default_for_base_index() {
    assert_eq!(crate::server::option_catalog::default_for("base-index"), Some("0"));
}

#[test]
fn option_catalog_default_for_history_limit() {
    assert_eq!(crate::server::option_catalog::default_for("history-limit"), Some("2000"));
}

#[test]
fn option_catalog_default_for_remain_on_exit() {
    assert_eq!(crate::server::option_catalog::default_for("remain-on-exit"), Some("off"));
}

// ════════════════════════════════════════════════════════════════════════════
//  Prompt History Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn prompt_history_initially_empty() {
    let app = mock_app();
    assert!(app.command_history.is_empty());
    assert_eq!(app.command_history_idx, 0);
}

#[test]
fn prompt_history_add_entries() {
    let mut app = mock_app();
    app.command_history.push("split-window".to_string());
    app.command_history.push("new-window".to_string());
    assert_eq!(app.command_history.len(), 2);
    assert_eq!(app.command_history[0], "split-window");
    assert_eq!(app.command_history[1], "new-window");
}

#[test]
fn prompt_history_index_navigation() {
    let mut app = mock_app();
    app.command_history.push("cmd1".to_string());
    app.command_history.push("cmd2".to_string());
    app.command_history.push("cmd3".to_string());
    // Simulate navigating up (towards older entries)
    app.command_history_idx = app.command_history.len();
    // Go up once
    app.command_history_idx -= 1;
    assert_eq!(app.command_history[app.command_history_idx], "cmd3");
    // Go up again
    app.command_history_idx -= 1;
    assert_eq!(app.command_history[app.command_history_idx], "cmd2");
    // Go up again
    app.command_history_idx -= 1;
    assert_eq!(app.command_history[app.command_history_idx], "cmd1");
    // Go down
    app.command_history_idx += 1;
    assert_eq!(app.command_history[app.command_history_idx], "cmd2");
}

#[test]
fn prompt_history_capped_at_100() {
    let mut app = mock_app();
    for i in 0..150 {
        app.command_history.push(format!("command-{}", i));
        if app.command_history.len() > 100 {
            app.command_history.remove(0);
        }
    }
    assert_eq!(app.command_history.len(), 100);
    // Oldest surviving entry should be command-50
    assert_eq!(app.command_history[0], "command-50");
    assert_eq!(app.command_history[99], "command-149");
}

#[test]
fn prompt_history_vi_mode_default_insert() {
    let app = mock_app();
    assert!(!app.command_vi_normal, "command prompt should start in insert mode");
}

// ════════════════════════════════════════════════════════════════════════════
//  Wrap-search Tests (copy mode search_next / search_prev)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn search_next_wraps_by_default() {
    let mut app = mock_app();
    // Manually populate search state
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 2; // at last match
    crate::copy_mode::search_next(&mut app);
    // Should wrap to index 0
    assert_eq!(app.copy_search_idx, 0);
    // Where the cursor lands is no longer a plain copy of the match tuple: the
    // match carries an ABSOLUTE line number and copy_mode::scroll_to_abs_line
    // maps it back onto the viewport, scrolling if the line is off screen
    // (#612). That needs a real grid, so it is pinned in
    // tests-rs/test_issue612_copy_search_scrollback.rs instead.
}

#[test]
fn search_next_does_not_wrap_when_off() {
    let mut app = mock_app();
    app.user_options.insert("wrap-search".to_string(), "off".to_string());
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 2; // at last match
    crate::copy_mode::search_next(&mut app);
    // Should NOT wrap; stays at index 2
    assert_eq!(app.copy_search_idx, 2);
}

#[test]
fn search_next_advances_normally() {
    let mut app = mock_app();
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 0;
    crate::copy_mode::search_next(&mut app);
    assert_eq!(app.copy_search_idx, 1);
}

#[test]
fn search_prev_wraps_by_default() {
    let mut app = mock_app();
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 0; // at first match
    crate::copy_mode::search_prev(&mut app);
    // Should wrap to last index
    assert_eq!(app.copy_search_idx, 2);
}

#[test]
fn search_prev_does_not_wrap_when_off() {
    let mut app = mock_app();
    app.user_options.insert("wrap-search".to_string(), "off".to_string());
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 0; // at first match
    crate::copy_mode::search_prev(&mut app);
    // Should NOT wrap; stays at index 0
    assert_eq!(app.copy_search_idx, 0);
}

#[test]
fn search_prev_retreats_normally() {
    let mut app = mock_app();
    app.copy_search_matches = vec![(0, 5, 8), (1, 10, 13), (2, 0, 3)];
    app.copy_search_idx = 2;
    crate::copy_mode::search_prev(&mut app);
    assert_eq!(app.copy_search_idx, 1);
}

#[test]
fn search_next_no_op_on_empty_matches() {
    let mut app = mock_app();
    app.copy_search_matches = vec![];
    app.copy_search_idx = 0;
    crate::copy_mode::search_next(&mut app);
    assert_eq!(app.copy_search_idx, 0);
    assert_eq!(app.copy_pos, None);
}

#[test]
fn search_prev_no_op_on_empty_matches() {
    let mut app = mock_app();
    app.copy_search_matches = vec![];
    app.copy_search_idx = 0;
    crate::copy_mode::search_prev(&mut app);
    assert_eq!(app.copy_search_idx, 0);
    assert_eq!(app.copy_pos, None);
}

#[test]
fn search_next_single_match_wraps_to_self() {
    let mut app = mock_app();
    app.copy_search_matches = vec![(5, 10, 15)];
    app.copy_search_idx = 0;
    crate::copy_mode::search_next(&mut app);
    // Only one match, wraps to itself
    assert_eq!(app.copy_search_idx, 0);
}

#[test]
fn search_prev_single_match_wraps_to_self() {
    let mut app = mock_app();
    app.copy_search_matches = vec![(5, 10, 15)];
    app.copy_search_idx = 0;
    crate::copy_mode::search_prev(&mut app);
    // Only one match, wraps to itself (last index = 0)
    assert_eq!(app.copy_search_idx, 0);
}

// ════════════════════════════════════════════════════════════════════════════
//  Session Group State Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn session_group_none_by_default() {
    let app = mock_app();
    assert!(app.session_group.is_none());
}

#[test]
fn session_group_can_be_set() {
    let mut app = mock_app();
    app.session_group = Some("work".to_string());
    assert_eq!(app.session_group.as_deref(), Some("work"));
}

// ════════════════════════════════════════════════════════════════════════════
//  Miscellaneous State Defaults Tests
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn session_id_is_unique() {
    let app1 = AppState::new("s1".to_string());
    let app2 = AppState::new("s2".to_string());
    assert_ne!(app1.session_id, app2.session_id, "each AppState should get a unique session_id");
}

#[test]
fn paste_buffers_initially_empty() {
    let app = mock_app();
    assert!(app.paste_buffers.is_empty());
}

#[test]
fn named_registers_initially_empty() {
    let app = mock_app();
    assert!(app.named_registers.is_empty());
}

#[test]
fn wait_channels_initially_empty() {
    let app = mock_app();
    assert!(app.wait_channels.is_empty());
}

#[test]
fn pipe_panes_initially_empty() {
    let app = mock_app();
    assert!(app.pipe_panes.is_empty());
}

#[test]
fn environment_initially_empty() {
    let app = mock_app();
    assert!(app.environment.is_empty());
}

#[test]
fn user_options_initially_empty() {
    let app = mock_app();
    assert!(app.user_options.is_empty());
}

#[test]
fn command_aliases_initially_empty() {
    let app = mock_app();
    assert!(app.command_aliases.is_empty());
}

#[test]
fn control_clients_initially_empty() {
    let app = mock_app();
    assert!(app.control_clients.is_empty());
}

#[test]
fn port_file_base_without_socket_name() {
    let app = AppState::new("mysession".to_string());
    assert_eq!(app.port_file_base(), "mysession");
}

#[test]
fn port_file_base_with_socket_name() {
    let mut app = AppState::new("mysession".to_string());
    app.socket_name = Some("custom".to_string());
    assert_eq!(app.port_file_base(), "custom__mysession");
}
