// Issue #876: a bell, activity or silence flag set on a window that is not
// the current one must reach the status line of an attached client at once,
// as in tmux, where alerts_check_bell, alerts_check_activity and
// alerts_check_silence call server_status_session when they set the flag
// (alerts.c:202-204).
//
// The tabs a client draws come from the server's cached window list (each
// window's expanded window-status-format and its flags), which is rebuilt
// only when meta_dirty is set. check_window_activity used to change the flags
// without saying so, and the frame kept the old tabs until something else
// rebuilt the list. These tests pin the report the server loop now reads;
// the frame itself is covered by tests/test_issue876_bell_status_refresh.ps1.

use super::*;

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
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

/// App with `n` settled windows, the first one current.
fn app_with(n: usize) -> AppState {
    let mut app = AppState::new("bellstatus".to_string());
    app.window_base_index = 0;
    for i in 0..n {
        let mut w = make_window(&format!("w{}", i), i);
        w.last_seen_version = window_data_version(&w);
        app.windows.push(w);
    }
    app.window_indices = (0..n).collect();
    app.active_idx = 0;
    app
}

/// Make window `i` look as if its panes printed since the last pass.
fn new_output(app: &mut AppState, i: usize) {
    app.windows[i].last_seen_version = app.windows[i].last_seen_version.wrapping_add(1);
}

/// The tab text of window `i` in the window list a frame carries.
fn tab_text(app: &AppState, i: usize) -> String {
    let json = list_windows_json_with_tabs(app).expect("window list");
    let v: serde_json::Value = serde_json::from_str(&json).expect("window list json");
    v[i]["tab_text"].as_str().expect("tab_text").to_string()
}

#[test]
fn nothing_new_reports_no_change() {
    let mut app = app_with(2);
    app.monitor_activity = true;

    let alerts = check_window_activity(&mut app);

    assert!(!alerts.flags_changed);
    assert!(alerts.hooks.is_empty());
}

#[test]
fn activity_in_another_window_reports_a_change() {
    let mut app = app_with(2);
    app.monitor_activity = true;
    app.attached_clients = 1;
    new_output(&mut app, 1);

    let alerts = check_window_activity(&mut app);

    assert!(app.windows[1].activity_flag);
    assert!(alerts.flags_changed, "a newly set activity flag must be reported so the tabs are rebuilt");
    assert!(alerts.hooks.contains(&"alert-activity"));
}

#[test]
fn a_flag_already_set_reports_no_change() {
    let mut app = app_with(2);
    app.monitor_activity = true;
    app.attached_clients = 1;
    new_output(&mut app, 1);
    let _ = check_window_activity(&mut app);
    new_output(&mut app, 1);

    let alerts = check_window_activity(&mut app);

    assert!(app.windows[1].activity_flag);
    assert!(!alerts.flags_changed, "a flag that stays set must not rebuild the list on every pass");
}

#[test]
fn silence_in_another_window_reports_a_change() {
    let mut app = app_with(2);
    app.monitor_silence = 3;
    app.attached_clients = 1;
    // Five seconds is within the clock's range on any host that has been up
    // that long, unlike the hour test_issue559 had to work around.
    app.windows[1].last_output_time = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(5))
        .expect("an instant five seconds ago");

    let alerts = check_window_activity(&mut app);

    assert!(app.windows[1].silence_flag);
    assert!(alerts.flags_changed, "a newly set silence flag must be reported so the tabs are rebuilt");
    assert!(alerts.hooks.contains(&"alert-silence"));
}

#[test]
fn clearing_the_current_window_reports_a_change() {
    let mut app = app_with(2);
    app.attached_clients = 1;
    app.windows[0].bell_flag = true;

    let alerts = check_window_activity(&mut app);

    assert!(!app.windows[0].bell_flag, "the current window of an attached session drops its flags");
    assert!(alerts.flags_changed, "a cleared flag changes the tab as much as a set one");
}

#[test]
fn rebuilt_list_shows_the_new_flag() {
    let mut app = app_with(2);
    app.monitor_activity = true;
    app.attached_clients = 1;
    let before = tab_text(&app, 1);
    new_output(&mut app, 1);

    let alerts = check_window_activity(&mut app);

    assert!(alerts.flags_changed);
    let after = tab_text(&app, 1);
    assert!(!before.contains('#'), "tab before the output: {before:?}");
    assert!(after.contains('#'), "tab after the output must carry the activity flag: {after:?}");
}
