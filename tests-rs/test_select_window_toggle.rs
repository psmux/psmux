// select-window -T (tmux 3.8, cmd-select-window.c:131-138): a target that is
// already the current window selects the last window instead, and with no
// last window is the error "no last window". psmux ignored -T everywhere.
//
// Pinned here: the request the server routes build (select_window_requests,
// shared by one shot TCP and control mode), the destination rule, and the
// server's own command path (execute_command_string: hooks, key bindings,
// the command prompt). End to end in tests/test_select_window_toggle.ps1.

use super::*;
use crate::types::{CtrlReq, NO_LAST_WINDOW};

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
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

/// Three windows, window 2 current and window 1 last.
fn app3() -> AppState {
    let mut app = AppState::new("swt".to_string());
    for i in 0..3 {
        app.windows.push(make_window(&format!("w{}", i), i));
    }
    app.active_idx = 2;
    app.last_window_idx = 1;
    app
}

fn toggle_of(reqs: &[CtrlReq]) -> Option<(String, bool)> {
    match reqs {
        [CtrlReq::SelectWindowSpec { spec, toggle, .. }] => Some((spec.clone(), *toggle)),
        _ => None,
    }
}

#[test]
fn requests_carry_toggle_for_every_target_form() {
    let (reqs, resp) = crate::server::connection::select_window_requests(&["-T", "-t", "sess:2"], Some("sess:2"), Some(2), false, None);
    assert_eq!(toggle_of(&reqs), Some(("sess:2".to_string(), true)));
    assert!(resp.is_some(), "-T must be able to report no last window");
    let (reqs, _) = crate::server::connection::select_window_requests(&["-T"], None, None, false, None);
    assert_eq!(toggle_of(&reqs), Some((String::new(), true)), "no -t is the current window");
    let (reqs, _) = crate::server::connection::select_window_requests(&["-T", "-t", "@4"], Some("@4"), Some(4), true, None);
    assert_eq!(toggle_of(&reqs), Some(("@4".to_string(), true)));
    let (reqs, _) = crate::server::connection::select_window_requests(&["-T", "-t", "editor"], None, None, false, Some("editor"));
    assert_eq!(toggle_of(&reqs), Some(("editor".to_string(), true)));
}

#[test]
fn requests_without_t_flag_do_not_toggle() {
    let (reqs, _) = crate::server::connection::select_window_requests(&["-t", "sess:2"], Some("sess:2"), Some(2), false, None);
    assert_eq!(toggle_of(&reqs), Some(("sess:2".to_string(), false)));
}

#[test]
fn relative_flags_ignore_toggle() {
    // -n/-p/-l run before tmux ever looks at -T (cmd-select-window.c:104-125).
    let (reqs, _) = crate::server::connection::select_window_requests(&["-T", "-n"], None, None, false, None);
    assert!(matches!(reqs.as_slice(), [CtrlReq::NextWindow]));
    let (reqs, _) = crate::server::connection::select_window_requests(&["-T", "-l"], None, None, false, None);
    assert!(matches!(reqs.as_slice(), [CtrlReq::LastWindow { .. }]));
}

#[test]
fn destination_rule() {
    let mut app = app3();
    use crate::server::temp_target::{select_window_destination, select_window_target_pos};
    assert_eq!(select_window_destination(&app, 2, true), Ok(1), "current + -T: last window");
    assert_eq!(select_window_destination(&app, 0, true), Ok(0), "another window + -T: that window");
    assert_eq!(select_window_destination(&app, 2, false), Ok(2), "no -T: the target");
    assert_eq!(select_window_target_pos(&app, "", None), Ok(2), "no target: the current window");
    app.last_window_idx = NO_LAST_WINDOW;
    assert_eq!(select_window_destination(&app, 2, true), Err("no last window".to_string()));
}

#[test]
fn in_process_toggle_on_current_window_selects_last() {
    let mut app = app3();
    execute_command_string(&mut app, "select-window -T -t :2").unwrap();
    assert_eq!((app.active_idx, app.last_window_idx), (1, 2));
}

#[test]
fn in_process_toggle_without_target_selects_last() {
    let mut app = app3();
    execute_command_string(&mut app, "select-window -T").unwrap();
    assert_eq!((app.active_idx, app.last_window_idx), (1, 2));
    // And again: back where it started.
    execute_command_string(&mut app, "select-window -T").unwrap();
    assert_eq!((app.active_idx, app.last_window_idx), (2, 1));
}

#[test]
fn in_process_toggle_on_another_window_selects_it() {
    let mut app = app3();
    execute_command_string(&mut app, "select-window -T -t :0").unwrap();
    assert_eq!((app.active_idx, app.last_window_idx), (0, 2));
}

#[test]
fn in_process_without_toggle_current_window_stays() {
    let mut app = app3();
    execute_command_string(&mut app, "select-window -t :2").unwrap();
    assert_eq!((app.active_idx, app.last_window_idx), (2, 1));
}

#[test]
fn in_process_toggle_with_no_last_window_is_an_error() {
    let mut app = app3();
    app.last_window_idx = NO_LAST_WINDOW;
    execute_command_string(&mut app, "select-window -T").unwrap();
    assert_eq!(app.active_idx, 2);
    let msg = app.status_message.as_ref().map(|m| m.0.clone()).unwrap_or_default();
    assert_eq!(msg, "no last window");
}
