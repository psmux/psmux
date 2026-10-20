// Issue #811: the window notifications came from the command's hook and
// named whatever window was current when it fired, so `kill-window -t @2`
// with @1 current announced `%window-close @1`. They now come from a diff of
// the window list around the request. Registered from src/control.rs.

use super::*;

fn snap(ids: &[usize], current: usize) -> WindowSnapshot {
    WindowSnapshot { ids: ids.to_vec(), current: Some(current), pane_modes: Vec::new() }
}

fn lines(before: &WindowSnapshot, after: &WindowSnapshot) -> Vec<String> {
    window_change_notifications(before, after, 5).iter().map(format_notification).collect()
}

#[test]
fn kill_of_another_window_names_that_window() {
    assert_eq!(lines(&snap(&[1, 2], 1), &snap(&[1], 1)), vec!["%window-close @2"]);
}

#[test]
fn kill_of_the_current_window_closes_it_then_changes_current() {
    // session.c session_detach: window-unlinked, then session_last.
    assert_eq!(
        lines(&snap(&[1, 3], 3), &snap(&[1], 1)),
        vec!["%window-close @3", "%session-window-changed $5 @1"]
    );
}

#[test]
fn new_window_changes_current_then_adds() {
    // spawn.c: session_select, then window-linked.
    assert_eq!(
        lines(&snap(&[1], 1), &snap(&[1, 2], 2)),
        vec!["%session-window-changed $5 @2", "%window-add @2"]
    );
}

#[test]
fn detached_new_window_only_adds() {
    assert_eq!(lines(&snap(&[1], 1), &snap(&[1, 4], 1)), vec!["%window-add @4"]);
}

#[test]
fn select_window_and_no_op() {
    assert_eq!(lines(&snap(&[1, 2], 2), &snap(&[1, 2], 1)), vec!["%session-window-changed $5 @1"]);
    // Selecting the current window, or a pane change, announces nothing.
    assert!(lines(&snap(&[1, 2], 1), &snap(&[1, 2], 1)).is_empty());
}

#[test]
fn last_window_gone_announces_only_the_close() {
    let after = WindowSnapshot { ids: vec![], current: None, pane_modes: Vec::new() };
    assert_eq!(lines(&snap(&[7], 7), &after), vec!["%window-close @7"]);
}

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: format!("w{}", id),
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

/// A change made outside any request (a pane that exited, a hook run by the
/// reaper) is announced by the next sync, once, against what clients were
/// last told.
#[test]
fn sync_announces_each_change_once() {
    let mut app = crate::types::AppState::new("s".to_string());
    app.windows = vec![make_window(1), make_window(2)];
    app.active_idx = 1;
    app.session_id = 5;
    let (ntx, nrx) = std::sync::mpsc::channel();
    app.control_clients.insert(1, crate::types::ControlClient {
        client_id: 1,
        cmd_counter: 0,
        echo_enabled: false,
        notification_tx: ntx,
        paused_panes: Default::default(),
        subscriptions: Default::default(),
        subscription_values: Default::default(),
        subscription_last_check: Default::default(),
        output_gate: Default::default(),
        size: None,
        window_sizes: Default::default(),
    });
    let drain = |rx: &std::sync::mpsc::Receiver<ControlNotification>| -> Vec<String> {
        rx.try_iter().map(|n| format_notification(&n)).collect()
    };
    // The first sync only records the state.
    sync_window_changes(&mut app);
    assert!(drain(&nrx).is_empty());
    // The current window @2 goes away on its own.
    app.windows.remove(1);
    app.active_idx = 0;
    sync_window_changes(&mut app);
    assert_eq!(drain(&nrx), vec!["%window-close @2", "%session-window-changed $5 @1"]);
    // Nothing changed since: nothing to say.
    sync_window_changes(&mut app);
    assert!(drain(&nrx).is_empty());
    app.windows.push(make_window(3));
    sync_window_changes(&mut app);
    assert_eq!(drain(&nrx), vec!["%window-add @3"]);
}
