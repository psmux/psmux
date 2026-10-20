// Issue #825: control clients and detach-client / sizing. A detached control
// client gets a bare `%exit` (tmux client.c:416-421) through its own writer
// instead of a dropped connection; `ignore-size` leaves a client out of the
// window size while another attached client has not set it (resize.c
// ignore_client_size); `#{session_attached}` counts attached clients
// (format.c format_cb_session_attached). Registered from src/control.rs.

use super::*;
use std::sync::mpsc::{channel, Receiver};

fn add_control(app: &mut AppState, id: u64, size: Option<(u16, u16)>) -> Receiver<ControlNotification> {
    let (tx, rx) = channel();
    app.control_clients.insert(id, crate::types::ControlClient {
        client_id: id,
        cmd_counter: 0,
        echo_enabled: false,
        notification_tx: tx,
        paused_panes: std::collections::HashSet::new(),
        subscriptions: std::collections::HashMap::new(),
        subscription_values: std::collections::HashMap::new(),
        subscription_last_check: std::collections::HashMap::new(),
        output_gate: Default::default(),
        size,
        window_sizes: std::collections::HashMap::new(),
    });
    app.register_client(id, true);
    rx
}

#[test]
fn detached_is_a_bare_exit() {
    assert_eq!(format_notification(&ControlNotification::Detached), "%exit");
}

#[test]
fn detach_control_client_tells_only_that_client() {
    let mut app = AppState::new("t".to_string());
    let rx1 = add_control(&mut app, 1, None);
    let rx2 = add_control(&mut app, 2, None);
    assert!(detach_control_client(&mut app, 2));
    assert!(matches!(rx2.try_recv(), Ok(ControlNotification::Detached)));
    // Its sender went with it, so its writer ends after the %exit.
    assert!(rx2.try_recv().is_err());
    assert!(!app.control_clients.contains_key(&2));
    // The other control client is untouched and still reachable.
    assert!(rx1.try_recv().is_err());
    assert!(app.control_clients.contains_key(&1));
    emit_notification(&app, ControlNotification::SessionsChanged);
    assert!(matches!(rx1.try_recv(), Ok(ControlNotification::SessionsChanged)));
}

#[test]
fn detach_control_client_refuses_a_non_control_client() {
    let mut app = AppState::new("t".to_string());
    app.register_client(7, false);
    assert!(!detach_control_client(&mut app, 7));
    assert!(!detach_control_client(&mut app, 99));
}

#[test]
fn session_attached_counts_clients() {
    let mut app = AppState::new("t".to_string());
    // Session formats resolve only for a session with a window.
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id: 1,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
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
    });
    assert_eq!(crate::format::expand_var("session_attached", &app, 0), "0");
    let _a = add_control(&mut app, 1, None);
    let _b = add_control(&mut app, 2, None);
    assert_eq!(crate::format::expand_var("session_attached", &app, 0), "2");
}

#[test]
fn ignore_size_client_is_left_out_while_another_client_is_not() {
    let mut app = AppState::new("t".to_string());
    let _a = add_control(&mut app, 1, Some((100, 30)));
    let _b = add_control(&mut app, 2, Some((80, 24)));
    app.ignore_size_clients.insert(2);
    assert!(crate::resize_window::client_size_ignored(&app, 2));
    assert!(!crate::resize_window::client_size_ignored(&app, 1));
    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
    assert_eq!((app.client_area.width, app.client_area.height), (100, 30));
    // Every client ignoring size: none is ignored (resize.c).
    app.ignore_size_clients.insert(1);
    assert!(!crate::resize_window::client_size_ignored(&app, 1));
    assert!(!crate::resize_window::client_size_ignored(&app, 2));
}

#[test]
fn reap_forgets_ignore_size() {
    let mut app = AppState::new("t".to_string());
    let _a = add_control(&mut app, 1, None);
    app.ignore_size_clients.insert(1);
    app.reap_client(1);
    assert!(app.ignore_size_clients.is_empty());
}

#[test]
fn client_flags_lists_ignore_size_after_control_mode() {
    let mut app = AppState::new("t".to_string());
    let _a = add_control(&mut app, 1, None);
    let ci = app.client_registry.get(&1).unwrap();
    assert_eq!(crate::format::client_flags_string_with(ci, true), "attached,control-mode,ignore-size,UTF-8");
    assert_eq!(crate::format::client_flags_string(ci), "attached,control-mode,UTF-8");
}
