// Issue #559: `set -g monitor-silence N` was stored but functionally dead.
//
// check_window_activity (the only place silence_flag is ever set) ran solely
// from DumpState handling and the server-push path, both of which require a
// client. A detached session therefore never evaluated monitor-silence and
// the flag could not fire. tmux 3.4 (WSL oracle) fires the ~ flag and
// window_silence_flag=1 on EVERY window of a detached session once the
// threshold passes. These tests pin the detection function itself; the 1 Hz
// server-loop call that now drives it detached is covered by the E2E test
// tests/test_issue559_family.ps1.

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

/// An `Instant` roughly `secs` in the past.
///
/// `Instant` cannot be moved back beyond the clock's origin, so on a machine
/// that booted recently `checked_sub` returns `None` for a large interval and
/// the old `.expect(...)` here panicked: the 3600s case failed on any host with
/// under an hour of uptime (a fresh CI runner, or a workstation just rebooted).
/// Every caller only needs "longer ago than the threshold under test", never
/// the exact interval, so halve the request until the clock can represent it.
fn instant_secs_ago(secs: u64) -> std::time::Instant {
    let now = std::time::Instant::now();
    let mut back = secs;
    loop {
        if let Some(past) = now.checked_sub(std::time::Duration::from_secs(back)) {
            return past;
        }
        if back == 0 {
            return now;
        }
        back /= 2;
    }
}

/// App with `n` windows whose last output was `silent_for` seconds ago and
/// whose version counters are settled (so the "new output" reset path does
/// not swallow the silence check).
fn app_silent_for(n: usize, silent_for: u64) -> AppState {
    let mut app = AppState::new("silence559".to_string());
    app.window_base_index = 0;
    let past = instant_secs_ago(silent_for);
    for i in 0..n {
        let mut w = make_window(&format!("w{}", i), i);
        w.last_output_time = past;
        w.last_seen_version = window_data_version(&w);
        app.windows.push(w);
    }
    app.window_indices = (0..n).collect();
    app
}

#[test]
fn detached_session_fires_silence_on_every_window() {
    let mut app = app_silent_for(2, 10);
    app.monitor_silence = 3;
    app.attached_clients = 0; // detached
    app.active_idx = 0;

    let hooks = check_window_activity(&mut app).hooks;

    assert!(
        app.windows[0].silence_flag,
        "BUG (issue #559): the ACTIVE window of a DETACHED session must flag silence (tmux parity)"
    );
    assert!(
        app.windows[1].silence_flag,
        "BUG (issue #559): a non-active silent window must flag silence"
    );
    assert!(
        hooks.contains(&"alert-silence"),
        "alert-silence hook must fire when the flag is first set"
    );
}

#[test]
fn attached_session_clears_active_window_but_flags_others() {
    let mut app = app_silent_for(2, 10);
    app.monitor_silence = 3;
    app.attached_clients = 1; // a client is viewing the session
    app.active_idx = 0;

    check_window_activity(&mut app);

    assert!(
        !app.windows[0].silence_flag,
        "the current window of an ATTACHED session is being watched; no silence alert"
    );
    assert!(
        app.windows[1].silence_flag,
        "non-active windows must still flag silence while a client is attached"
    );
}

#[test]
fn silence_does_not_fire_before_threshold() {
    let mut app = app_silent_for(2, 1); // only 1s of silence
    app.monitor_silence = 30;
    app.attached_clients = 0;

    check_window_activity(&mut app);

    assert!(!app.windows[0].silence_flag);
    assert!(!app.windows[1].silence_flag);
}

#[test]
fn monitor_silence_zero_means_off() {
    let mut app = app_silent_for(2, 3600);
    app.monitor_silence = 0; // default: disabled
    app.attached_clients = 0;

    let hooks = check_window_activity(&mut app).hooks;

    assert!(!app.windows[0].silence_flag, "monitor-silence 0 must never flag");
    assert!(!app.windows[1].silence_flag, "monitor-silence 0 must never flag");
    assert!(!hooks.contains(&"alert-silence"));
}

#[test]
fn silence_flag_latches_and_hook_fires_once() {
    let mut app = app_silent_for(1, 10);
    app.monitor_silence = 3;
    app.attached_clients = 0;

    let first = check_window_activity(&mut app).hooks;
    let second = check_window_activity(&mut app).hooks;

    assert!(app.windows[0].silence_flag, "flag must stay set while silent");
    assert!(first.contains(&"alert-silence"));
    assert!(
        !second.contains(&"alert-silence"),
        "alert-silence must not re-fire every tick while the flag is already set"
    );
}

#[test]
fn new_output_resets_silence_flag() {
    let mut app = app_silent_for(2, 10);
    app.monitor_silence = 3;
    app.attached_clients = 0;
    app.active_idx = 0;

    check_window_activity(&mut app);
    assert!(app.windows[1].silence_flag, "precondition: flag set while silent");

    // Simulate fresh output: perturb the version counter the detector hashes.
    app.windows[1].last_seen_version = app.windows[1].last_seen_version.wrapping_add(1);

    check_window_activity(&mut app);
    assert!(
        !app.windows[1].silence_flag,
        "new output must clear the silence flag (tmux resets the alert)"
    );
}
