// Live-client style options must share one server-side render-state contract:
//   1. append_extra_style_json() emits status and pane-content style fields.
//   2. list_windows_json_with_tabs() must carry per-window bell/last/activity
//      flags so the client can pick the right style.

use super::*;

fn mk_app() -> AppState {
    let mut app = AppState::new("live_client_styles".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn mk_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
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

// ── 1. The render-state JSON carries live-client status styles ──
#[test]
fn append_extra_style_json_emits_status_styles() {
    let mut app = mk_app();
    app.status_left_style = "fg=colour201".to_string();
    app.status_right_style = "bg=colour21".to_string();
    app.window_status_activity_style = "reverse".to_string();
    app.window_status_bell_style = "bg=blue".to_string();
    app.window_status_last_style = "fg=green".to_string();

    let mut buf = String::from("{\"status_style\":\"x\"}");
    append_extra_style_json(&mut buf, &app);

    assert!(buf.contains("\"status_left_style\":\"fg=colour201\""), "status-left-style missing: {buf}");
    assert!(buf.contains("\"status_right_style\":\"bg=colour21\""), "status-right-style missing: {buf}");
    assert!(buf.contains("\"wsa_style\":\"reverse\""), "activity-style missing: {buf}");
    assert!(buf.contains("\"wsb_style\":\"bg=blue\""), "bell-style missing: {buf}");
    assert!(buf.contains("\"wsl_style\":\"fg=green\""), "last-style missing: {buf}");

    // Result must remain a single valid JSON object.
    assert!(buf.ends_with('}'));
    let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(parsed["wsa_style"], "reverse");
    assert_eq!(parsed["status_left_style"], "fg=colour201");
}

#[test]
fn append_extra_style_json_emits_global_window_content_styles() {
    let mut app = mk_app();
    app.user_options.insert(
        "window-style".to_string(),
        "fg=colour245,bg=colour236".to_string(),
    );
    app.user_options.insert(
        "window-active-style".to_string(),
        "fg=colour250,bg=black".to_string(),
    );

    let mut buf = String::from("{\"layout\":{}}");
    append_extra_style_json(&mut buf, &app);

    let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(parsed["window_style"], "fg=colour245,bg=colour236");
    assert_eq!(parsed["window_active_style"], "fg=colour250,bg=black");
}

#[test]
fn append_extra_style_json_emits_empty_global_window_content_styles_when_unset() {
    let app = mk_app();
    let mut buf = String::from("{\"layout\":{}}");
    append_extra_style_json(&mut buf, &app);

    let parsed: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(parsed["window_style"], "");
    assert_eq!(parsed["window_active_style"], "");
}

// Guard the injection contract: never touch a buffer that is not a closed object.
#[test]
fn append_extra_style_json_noop_when_not_object() {
    let mut app = mk_app();
    app.window_status_activity_style = "reverse".to_string();
    let mut buf = String::from("[1,2,3]"); // not ending in '}'
    append_extra_style_json(&mut buf, &app);
    assert_eq!(buf, "[1,2,3]", "must not corrupt a non-object buffer");
}

// ── 2. Per-window flags must be plumbed so the client can style tabs ──
#[test]
fn list_windows_json_carries_bell_activity_last_flags() {
    let mut app = mk_app();
    app.windows.push(mk_window("w0", 0));
    app.windows.push(mk_window("w1", 1));
    app.active_idx = 1;        // w1 is current
    app.last_window_idx = 0;   // w0 is the last-active window
    app.windows[0].activity_flag = true;
    app.windows[0].bell_flag = true;

    let json = crate::server::helpers::list_windows_json_with_tabs(&app).unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(arr.len(), 2);

    // w0: not active, but bell + activity + last flags all set.
    assert_eq!(arr[0]["active"], false);
    assert_eq!(arr[0]["bell"], true, "bell flag not plumbed: {json}");
    assert_eq!(arr[0]["activity"], true, "activity flag not plumbed: {json}");
    assert_eq!(arr[0]["last"], true, "last flag not plumbed: {json}");

    // w1: active, and definitely not the last window.
    assert_eq!(arr[1]["active"], true);
    assert_eq!(arr[1]["last"], false);
    assert_eq!(arr[1]["bell"], false);
}
