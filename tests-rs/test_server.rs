use super::should_spawn_warm_server;
use super::helpers::{combined_data_version, list_windows_json_with_tabs};
use crate::types::AppState;

// ── Hook set/replace/unset tests (issue #133) ───────────────────

#[test]
fn set_hook_replaces_existing_hook() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message first'");
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message second'");
    let cmds = app.hooks.get("client-attached").unwrap();
    assert_eq!(cmds.len(), 1, "hook should be replaced, not appended");
    assert_eq!(cmds[0], "display-message second");
}

#[test]
fn set_hook_unset_removes_hook() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message hello'");
    assert!(app.hooks.contains_key("client-attached"));
    crate::config::parse_config_line(&mut app, "set-hook -gu client-attached");
    assert!(!app.hooks.contains_key("client-attached"), "hook should be removed by -gu");
}

#[test]
fn set_hook_different_hooks_coexist() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message a'");
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window 'display-message b'");
    assert_eq!(app.hooks.len(), 2);
    assert_eq!(app.hooks["client-attached"][0], "display-message a");
    assert_eq!(app.hooks["after-new-window"][0], "display-message b");
}

#[test]
fn set_hook_replace_preserves_other_hooks() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'cmd-a'");
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window 'cmd-b'");
    // Replace client-attached — after-new-window should be untouched
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'cmd-c'");
    assert_eq!(app.hooks["client-attached"], vec!["cmd-c"]);
    assert_eq!(app.hooks["after-new-window"], vec!["cmd-b"]);
}

#[test]
fn set_hook_unset_with_u_flag() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'hello'");
    crate::config::parse_config_line(&mut app, "set-hook -u client-attached");
    assert!(!app.hooks.contains_key("client-attached"), "hook should be removed by -u");
}

// ── Hook -ga (append) tests (issue #133 follow-up) ─────────────

#[test]
fn set_hook_ga_appends_to_existing() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'display-message first'");
    crate::config::parse_config_line(&mut app, "set-hook -ga client-attached 'display-message second'");
    let cmds = app.hooks.get("client-attached").unwrap();
    assert_eq!(cmds.len(), 2, "-ga should append, giving 2 handlers");
    assert_eq!(cmds[0], "display-message first");
    assert_eq!(cmds[1], "display-message second");
}

#[test]
fn set_hook_ga_creates_if_missing() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -ga client-attached 'display-message only'");
    let cmds = app.hooks.get("client-attached").unwrap();
    assert_eq!(cmds.len(), 1, "-ga on missing hook should create it");
    assert_eq!(cmds[0], "display-message only");
}

#[test]
fn set_hook_g_replaces_appended_hooks() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'cmd-a'");
    crate::config::parse_config_line(&mut app, "set-hook -ga client-attached 'cmd-b'");
    crate::config::parse_config_line(&mut app, "set-hook -ga client-attached 'cmd-c'");
    assert_eq!(app.hooks["client-attached"].len(), 3);
    // Now -g (without -a) should replace all of them
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'cmd-new'");
    let cmds = app.hooks.get("client-attached").unwrap();
    assert_eq!(cmds.len(), 1, "-g should replace entire list");
    assert_eq!(cmds[0], "cmd-new");
}

#[test]
fn set_hook_gu_removes_all_appended_hooks() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook -g client-attached 'cmd-a'");
    crate::config::parse_config_line(&mut app, "set-hook -ga client-attached 'cmd-b'");
    assert_eq!(app.hooks["client-attached"].len(), 2);
    crate::config::parse_config_line(&mut app, "set-hook -gu client-attached");
    assert!(!app.hooks.contains_key("client-attached"), "-gu should remove all handlers");
}

#[test]
fn set_hook_a_flag_without_g() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set-hook client-attached 'cmd-a'");
    crate::config::parse_config_line(&mut app, "set-hook -a client-attached 'cmd-b'");
    let cmds = app.hooks.get("client-attached").unwrap();
    assert_eq!(cmds.len(), 2, "-a without -g should also append");
}

#[test]
fn warm_server_is_disabled_for_destroy_unattached_sessions() {
    let mut app = AppState::new("demo".to_string());
    app.destroy_unattached = true;
    assert!(!should_spawn_warm_server(&app));
}

#[test]
fn warm_server_is_disabled_for_warm_session_itself() {
    let app = AppState::new("__warm__".to_string());
    assert!(!should_spawn_warm_server(&app));
}

#[test]
fn warm_server_is_disabled_when_warm_enabled_is_false() {
    let mut app = AppState::new("demo".to_string());
    app.warm_enabled = false;
    assert!(!should_spawn_warm_server(&app));
}

#[test]
fn warm_server_is_allowed_for_normal_sessions() {
    let app = AppState::new("demo".to_string());
    assert!(should_spawn_warm_server(&app));
}

// ── Options get/set tests ───────────────────────────────────────

#[test]
fn get_option_allow_rename() {
    let app = AppState::new("test".to_string());
    let val = super::options::get_option_value(&app, "allow-rename");
    assert_eq!(val, "on");
}

#[test]
fn warm_server_does_not_run_status_interval_timer() {
    // Guards the double-fire fix; see AppState::should_run_status_interval_timer.
    let mut app = AppState::new("__warm__".to_string());
    app.status_interval = 5;
    assert!(
        !app.should_run_status_interval_timer(),
        "warm server must not run the status-interval timer"
    );

    app.session_name = "main".to_string();
    assert!(
        app.should_run_status_interval_timer(),
        "a real session must run the status-interval timer"
    );

    app.status_interval = 0;
    assert!(
        !app.should_run_status_interval_timer(),
        "status-interval=0 disables the timer"
    );
}

#[test]
fn is_warm_server_tracks_the_reserved_name() {
    // Guards the double-fire fix; see AppState::is_warm_server.
    let mut app = AppState::new("__warm__".to_string());
    assert!(app.is_warm_server(), "the __warm__ pre-spawn server is warm");

    // Claiming a warm server renames it to the real session; it is no longer warm.
    app.session_name = "main".to_string();
    assert!(!app.is_warm_server(), "a claimed/real session is not warm");
}

#[test]
fn get_option_bell_action() {
    let app = AppState::new("test".to_string());
    let val = super::options::get_option_value(&app, "bell-action");
    assert_eq!(val, "any");
}

#[test]
fn get_option_activity_action() {
    let app = AppState::new("test".to_string());
    let val = super::options::get_option_value(&app, "activity-action");
    assert_eq!(val, "other");
}

#[test]
fn get_option_silence_action() {
    let app = AppState::new("test".to_string());
    let val = super::options::get_option_value(&app, "silence-action");
    assert_eq!(val, "other");
}

#[test]
fn get_option_update_environment() {
    let app = AppState::new("test".to_string());
    let val = super::options::get_option_value(&app, "update-environment");
    assert!(val.contains("DISPLAY"));
    assert!(val.contains("SSH_AUTH_SOCK"));
}

#[test]
fn set_option_allow_rename_off() {
    let mut app = AppState::new("test".to_string());
    super::options::apply_set_option(&mut app, "allow-rename", "off", false);
    assert!(!app.allow_rename);
}

#[test]
fn set_option_activity_action() {
    let mut app = AppState::new("test".to_string());
    super::options::apply_set_option(&mut app, "activity-action", "any", false);
    assert_eq!(app.activity_action, "any");
}

#[test]
fn set_option_silence_action() {
    let mut app = AppState::new("test".to_string());
    super::options::apply_set_option(&mut app, "silence-action", "none", false);
    assert_eq!(app.silence_action, "none");
}

// ── Root table binding tests (discussion #130: vim-style C-hjkl nav) ────

use crossterm::event::{KeyCode, KeyModifiers};
use crate::config::{normalize_key_for_binding, parse_bind_key};
use crate::types::{Action, FocusDir};

#[test]
fn bind_key_n_creates_root_binding() {
    let mut app = AppState::new("test".to_string());
    parse_bind_key(&mut app, "bind-key -n C-h select-pane -L");
    let root = app.key_tables.get("root").expect("root table should exist");
    assert_eq!(root.len(), 1, "root table should have one binding");
    let bind = &root[0];
    assert!(matches!(bind.action, Action::MoveFocus(FocusDir::Left)),
        "C-h should be bound to select-pane -L");
}

#[test]
fn bind_key_n_all_vim_directions() {
    let mut app = AppState::new("test".to_string());
    parse_bind_key(&mut app, "bind-key -n C-h select-pane -L");
    parse_bind_key(&mut app, "bind-key -n C-j select-pane -D");
    parse_bind_key(&mut app, "bind-key -n C-k select-pane -U");
    parse_bind_key(&mut app, "bind-key -n C-l select-pane -R");
    let root = app.key_tables.get("root").expect("root table should exist");
    assert_eq!(root.len(), 4, "root table should have four bindings");

    let expected = [
        ('h', FocusDir::Left),
        ('j', FocusDir::Down),
        ('k', FocusDir::Up),
        ('l', FocusDir::Right),
    ];
    for (ch, dir) in expected {
        let key = normalize_key_for_binding((KeyCode::Char(ch), KeyModifiers::CONTROL));
        let bind = root.iter().find(|b| b.key == key)
            .unwrap_or_else(|| panic!("binding for C-{} should exist", ch));
        assert!(matches!(&bind.action, Action::MoveFocus(d) if *d == dir),
            "C-{} should be bound to {:?}", ch, dir);
    }
}

#[test]
fn ctrl_h_binding_matches_windows_key_event() {
    // On Windows, Ctrl+H is reported as Char('h') + CONTROL by crossterm
    let mut app = AppState::new("test".to_string());
    parse_bind_key(&mut app, "bind-key -n C-h select-pane -L");
    let root = app.key_tables.get("root").unwrap();

    let win_key = normalize_key_for_binding((KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert!(root.iter().any(|b| b.key == win_key),
        "C-h binding must match Char('h')+CONTROL key event");
}

#[test]
fn backspace_and_ctrl_h_are_distinct_on_windows() {
    // On Windows, Backspace and Ctrl+H are distinct keys — they must NOT alias
    let backspace = normalize_key_for_binding((KeyCode::Backspace, KeyModifiers::empty()));
    let ctrl_h = normalize_key_for_binding((KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert_ne!(backspace, ctrl_h,
        "Backspace and C-h must be distinct on Windows (no Unix aliasing)");
}

#[test]
fn tab_and_ctrl_i_are_distinct_on_windows() {
    let tab = normalize_key_for_binding((KeyCode::Tab, KeyModifiers::empty()));
    let ctrl_i = normalize_key_for_binding((KeyCode::Char('i'), KeyModifiers::CONTROL));
    assert_ne!(tab, ctrl_i,
        "Tab and C-i must be distinct on Windows");
}

#[test]
fn enter_and_ctrl_m_are_distinct_on_windows() {
    let enter = normalize_key_for_binding((KeyCode::Enter, KeyModifiers::empty()));
    let ctrl_m = normalize_key_for_binding((KeyCode::Char('m'), KeyModifiers::CONTROL));
    assert_ne!(enter, ctrl_m,
        "Enter and C-m must be distinct on Windows");
}

#[test]
fn normalize_only_strips_shift_from_char() {
    // Regular keys: SHIFT stripped from Char events
    let shifted = normalize_key_for_binding((KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert_eq!(shifted, (KeyCode::Char('A'), KeyModifiers::empty()));

    // Non-Char keys: modifiers preserved
    let ctrl_l = normalize_key_for_binding((KeyCode::Char('l'), KeyModifiers::CONTROL));
    assert_eq!(ctrl_l, (KeyCode::Char('l'), KeyModifiers::CONTROL));

    let shift_bs = normalize_key_for_binding((KeyCode::Backspace, KeyModifiers::SHIFT));
    assert_eq!(shift_bs, (KeyCode::Backspace, KeyModifiers::SHIFT));
}

#[test]
fn bind_key_select_pane_z_stays_as_command() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_bind_key(&mut app, "bind-key -n C-h select-pane -Z -L");
    let root = app.key_tables.get("root").expect("root table should exist");
    let bind = &root[0];
    assert!(matches!(&bind.action, crate::types::Action::Command(cmd) if cmd == "select-pane -Z -L"));
}

#[test]
fn parse_config_line_select_pane_z_stays_as_command() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "bind-key -r h select-pane -Z -L");
    let prefix = app.key_tables.get("prefix").expect("prefix table should exist");
    let bind = prefix.iter().find(|b| matches!(b.key.0, KeyCode::Char('h'))).expect("h binding should exist");
    assert!(matches!(&bind.action, crate::types::Action::Command(cmd) if cmd == "select-pane -Z -L"));
}

#[test]
fn serialized_bindings_preserve_select_pane_z_command() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "bind-key -r h select-pane -Z -L");
    let json = crate::server::helpers::serialize_bindings_json(&app);
    assert!(json.contains("select-pane -Z -L"));
}

// ── combined_data_version includes copy mode state (issue #152) ──

#[test]
fn combined_data_version_changes_on_copy_pos() {
    let mut app = AppState::new("test".to_string());
    let v1 = combined_data_version(&app);

    app.copy_pos = Some((5, 10));
    let v2 = combined_data_version(&app);
    assert_ne!(v1, v2, "version must change when copy_pos is set");

    app.copy_pos = Some((5, 11));
    let v3 = combined_data_version(&app);
    assert_ne!(v2, v3, "version must change when copy cursor column changes");

    app.copy_pos = Some((6, 11));
    let v4 = combined_data_version(&app);
    assert_ne!(v3, v4, "version must change when copy cursor row changes");
}

#[test]
fn combined_data_version_changes_on_scroll_offset() {
    let mut app = AppState::new("test".to_string());
    let v1 = combined_data_version(&app);

    app.copy_scroll_offset = 5;
    let v2 = combined_data_version(&app);
    assert_ne!(v1, v2, "version must change when copy_scroll_offset changes");

    app.copy_scroll_offset = 6;
    let v3 = combined_data_version(&app);
    assert_ne!(v2, v3, "version must change on each scroll offset increment");
}

#[test]
fn combined_data_version_changes_on_copy_anchor() {
    let mut app = AppState::new("test".to_string());
    let v1 = combined_data_version(&app);

    app.copy_anchor = Some((3, 7));
    let v2 = combined_data_version(&app);
    assert_ne!(v1, v2, "version must change when copy_anchor is set");
}

#[test]
fn combined_data_version_stable_when_copy_state_unchanged() {
    let mut app = AppState::new("test".to_string());
    app.copy_pos = Some((2, 3));
    app.copy_scroll_offset = 10;
    app.copy_anchor = Some((1, 0));

    let v1 = combined_data_version(&app);
    let v2 = combined_data_version(&app);
    assert_eq!(v1, v2, "version must be stable when nothing changes");
}

// ── Bell forwarding tests ───────────────────────────────────────

#[test]
fn bell_forward_defaults_to_false() {
    let app = AppState::new("test".to_string());
    assert!(!app.bell_forward, "bell_forward must default to false");
}

#[test]
fn bell_action_none_suppresses_bell_forward() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set -g bell-action none");
    assert_eq!(app.bell_action, "none");
    // With bell-action none, check_window_activity should never set bell_forward
    // (no panes to trigger, but verify the option is accepted)
    let hooks = super::helpers::check_window_activity(&mut app).hooks;
    assert!(!app.bell_forward, "bell_forward must stay false with bell-action none");
    assert!(hooks.is_empty());
}

#[test]
fn bell_action_set_to_any_via_config() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set -g bell-action any");
    assert_eq!(app.bell_action, "any");
}

#[test]
fn bell_action_set_to_current_via_config() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set -g bell-action current");
    assert_eq!(app.bell_action, "current");
}

#[test]
fn bell_action_set_to_other_via_config() {
    let mut app = AppState::new("test".to_string());
    crate::config::parse_config_line(&mut app, "set -g bell-action other");
    assert_eq!(app.bell_action, "other");
}

// ── Issue #125: window_zoomed_flag status bar caching ───────────

fn mock_window_for_server(name: &str) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
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
fn list_windows_tab_text_reflects_zoom_flag() {
    // Simulates the core issue #125 bug: after zoom toggle, the
    // server must re-expand window-status-format so that
    // #{?window_zoomed_flag,+, } updates in the status bar.
    // If list_windows_json_with_tabs is not called (because meta_dirty
    // is not set), the client receives stale tab_text.
    let mut app = AppState::new("test".to_string());
    app.window_status_current_format = "#W #{?window_zoomed_flag,+, }".to_string();
    app.window_status_format = "#W #{?window_zoomed_flag,+, }".to_string();
    let mut win0 = mock_window_for_server("editor");
    win0.id = 0;
    app.windows.push(win0);
    app.active_idx = 0;

    // Before zoom: tab_text should NOT contain +
    let json_before = list_windows_json_with_tabs(&app).unwrap();
    assert!(json_before.contains("editor  ") || !json_before.contains("editor +"),
        "before zoom, tab_text should not show +, got: {}", json_before);

    // Simulate zoom toggle
    app.windows[0].zoom_saved = Some(vec![(vec![], vec![50, 50])]);

    // After zoom: tab_text MUST contain + (this only happens if
    // list_windows_json_with_tabs is actually re-called, which
    // requires meta_dirty = true in the server loop)
    let json_after = list_windows_json_with_tabs(&app).unwrap();
    assert!(json_after.contains("editor +"),
        "after zoom, tab_text must show +, got: {}", json_after);
}

#[test]
fn list_windows_tab_text_per_window_zoom() {
    // Multi-window scenario from issue #125 follow-up:
    // zoom window 0, switch to window 1 — window 0 must keep +
    let mut app = AppState::new("test".to_string());
    app.window_status_current_format = "#I #W #{?window_zoomed_flag,+, }".to_string();
    app.window_status_format = "#I #W #{?window_zoomed_flag,+, }".to_string();
    let mut win0 = mock_window_for_server("editor");
    win0.id = 0;
    win0.zoom_saved = Some(vec![(vec![], vec![50, 50])]);
    let mut win1 = mock_window_for_server("shell");
    win1.id = 1;
    app.windows.push(win0);
    app.windows.push(win1);
    // Active window is 1 (user switched away from zoomed window 0)
    app.active_idx = 1;

    let json = list_windows_json_with_tabs(&app).unwrap();
    // Window 0 (zoomed) should show +
    assert!(json.contains("editor +"), "zoomed window 0 must show +, got: {}", json);
    // Window 1 (not zoomed) should show space, not +
    assert!(!json.contains("shell +"), "non-zoomed window 1 must not show +, got: {}", json);
}
