// A window or a pane that goes away takes its own mode with it, and the pane
// that gets the focus keeps its own. tmux keeps a mode on its window_pane, so
// both hold there by construction; measured on tmux 3.8 for kill-window,
// exit, kill-window -a, unlink-window -k and break-pane -d. psmux parks the
// modes of the panes without the focus (copy in Pane::copy_state, clock in
// AppState::clock_panes), and the paths that remove a window or reap a pane
// moved the focus without handing them over.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Mode, Node};

fn make_pane(id: usize, alive: bool) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: if alive { crate::util::StubChild::running() } else { crate::util::StubChild::exited() },
        term: Arc::new(Mutex::new(vt100::Parser::new(10, 40, 0))),
        last_rows: 10,
        last_cols: 40,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        dead_info: None,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false, input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

/// A window of the panes `(id, alive)`, the first one focused.
fn make_window(id: usize, panes: &[(usize, bool)]) -> crate::types::Window {
    let root = if panes.len() == 1 {
        Node::Leaf(make_pane(panes[0].0, panes[0].1))
    } else {
        Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![100 / panes.len() as u16; panes.len()],
            children: panes.iter().map(|&(p, a)| Node::Leaf(make_pane(p, a))).collect(),
        }
    };
    crate::types::Window {
        root,
        active_path: if panes.len() == 1 { vec![] } else { vec![0] },
        name: format!("w{id}"),
        id,
        area: ratatui::layout::Rect::new(0, 0, 40, 10),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
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

fn app_of(windows: Vec<crate::types::Window>) -> AppState {
    let mut app = AppState::new("wcm".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows = windows;
    app.active_idx = 0;
    app
}

fn mode_of(app: &AppState, pane_id: usize) -> String {
    crate::format::expand_format_for_pane_by_id("#{?pane_in_mode,#{pane_mode},none}", app, pane_id)
}

/// Window 0 (`%1`, focused) and window 1 (`%4`), `%4` in `mode` and parked.
fn next_window_in(mode: &str, first_alive: bool) -> AppState {
    let mut app = app_of(vec![make_window(10, &[(1, first_alive)]), make_window(11, &[(4, true)])]);
    switch_with_copy_save(&mut app, |a| a.active_idx = 1);
    match mode {
        "clock" => app.mode = Mode::ClockMode,
        _ => enter_copy_mode(&mut app),
    }
    switch_with_copy_save(&mut app, |a| a.active_idx = 0);
    assert!(matches!(app.mode, Mode::Passthrough));
    assert_eq!(active_pane_id(&app), Some(1));
    app
}

#[test]
fn kill_window_hands_the_next_window_its_own_clock() {
    let mut app = next_window_in("clock", true);
    crate::commands::execute_command_string(&mut app, "kill-window").unwrap();
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::ClockMode), "%4's clock is live again, so a key reaches it");
    assert!(app.clock_panes.is_empty());
    assert_eq!(mode_of(&app, 4), "clock-mode");
}

#[test]
fn kill_window_hands_the_next_window_its_own_copy_mode() {
    let mut app = next_window_in("copy", true);
    crate::commands::execute_command_string(&mut app, "kill-window").unwrap();
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(mode_of(&app, 4), "copy-mode");
}

#[test]
fn a_killed_window_takes_its_own_mode_with_it() {
    for mode in ["clock", "copy"] {
        let mut app = app_of(vec![make_window(10, &[(1, true)]), make_window(11, &[(4, true)])]);
        match mode {
            "clock" => app.mode = Mode::ClockMode,
            _ => enter_copy_mode(&mut app),
        }
        crate::commands::execute_command_string(&mut app, "kill-window").unwrap();
        assert_eq!(active_pane_id(&app), Some(4));
        assert!(matches!(app.mode, Mode::Passthrough), "{mode}: %4 never had a mode");
        assert_eq!(mode_of(&app, 4), "none", "{mode}");
        assert!(app.clock_panes.is_empty(), "{mode}: the dead %1 is not parked");
    }
}

#[test]
fn kill_window_all_hands_the_kept_window_its_clock() {
    let mut app = next_window_in("clock", true);
    crate::commands::kill_windows_except(&mut app, 1);
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn unlink_window_hands_the_next_window_its_clock() {
    let mut app = next_window_in("clock", true);
    crate::commands::execute_command_string(&mut app, "unlink-window -k").unwrap();
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn a_window_that_exits_hands_the_next_one_its_clock() {
    // Window 0's only pane has exited: the reaper removes the window.
    let mut app = next_window_in("clock", false);
    let (empty, pruned, _) = crate::tree::reap_children(&mut app).unwrap();
    assert!(!empty && pruned);
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::ClockMode));
    assert_eq!(mode_of(&app, 4), "clock-mode");
}

#[test]
fn a_window_that_exits_in_clock_mode_takes_the_clock_with_it() {
    let mut app = app_of(vec![make_window(10, &[(1, false)]), make_window(11, &[(4, true)])]);
    app.mode = Mode::ClockMode;
    crate::tree::reap_children(&mut app).unwrap();
    assert_eq!(active_pane_id(&app), Some(4));
    assert!(matches!(app.mode, Mode::Passthrough));
    assert!(app.clock_panes.is_empty());
}

#[test]
fn a_pane_that_exits_hands_the_other_pane_its_modes() {
    for mode in ["clock", "copy"] {
        // %1 left, alive and in `mode`; %2 right, focused, and exited.
        let mut app = app_of(vec![make_window(10, &[(1, true), (2, false)])]);
        match mode {
            "clock" => app.mode = Mode::ClockMode,
            _ => enter_copy_mode(&mut app),
        }
        switch_with_copy_save(&mut app, |a| a.windows[0].active_path = vec![1]);
        assert_eq!(active_pane_id(&app), Some(2));
        assert!(matches!(app.mode, Mode::Passthrough));
        crate::tree::reap_children(&mut app).unwrap();
        assert_eq!(active_pane_id(&app), Some(1));
        assert_eq!(mode_of(&app, 1), format!("{mode}-mode"));
        assert!(!matches!(app.mode, Mode::Passthrough), "{mode}: the live mode is %1's again");
    }
}

#[test]
fn break_pane_detached_hands_the_pane_left_its_clock() {
    let mut app = app_of(vec![make_window(10, &[(1, true), (2, true)])]);
    app.mode = Mode::ClockMode;
    switch_with_copy_save(&mut app, |a| a.windows[0].active_path = vec![1]);
    assert_eq!(active_pane_id(&app), Some(2));
    let req = crate::window_ops::BreakPaneRequest { detach: true, ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).unwrap();
    assert_eq!(active_pane_id(&app), Some(1));
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn removing_a_window_without_the_focus_changes_no_mode() {
    let mut app = next_window_in("clock", true);
    // Kill window 0's neighbour from window 0: the focus stays on %1.
    crate::commands::execute_command_string(&mut app, "kill-window -t :1").unwrap();
    assert_eq!(active_pane_id(&app), Some(1));
    assert!(matches!(app.mode, Mode::Passthrough));
    assert!(app.clock_panes.is_empty(), "the killed %4's parked clock goes with it");
}
