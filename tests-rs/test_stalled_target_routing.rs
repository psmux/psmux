// A command's -t target travels with the request it applies to.
//
// The defect: the CLI route sent a target as its own request,
// FocusTargetTemp, which switched the server's focus and left it switched
// until "the next request that is not a temp focus", from anyone.  Measured
// on 2358d49 with tests\test_stalled_server_target_routing.ps1: six targeted
// commands fired at once put `send-keys -t s:0.1` keys into pane 0.0 or into
// window 1, and `kill-pane -t s:0.2` killed pane 0.0, with or without a
// server stall; and `send-keys -N 3 -t s:0.1 Q` with nothing else running
// typed one Q into 0.1 and two into the active pane.
//
// The contract now (tmux's, cmd-queue.c cmdq_fire_command + cmd-find.c
// cmd_find_target): ValidateTarget resolves the target read only into stable
// ids, every request of the command carries it (CtrlReq::Targeted, sent by
// TargetedSender), and the server loop focuses, runs and restores within that
// one request.  These tests pin the pieces; the E2E pins the loop.
// Registered from src/server/temp_target.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::connection::TargetedSender;
use crate::types::{CtrlReq, LayoutKind, Node, TempTarget};

fn make_pane(id: usize) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
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

fn make_window(id: usize, name: &str, pane_ids: &[usize]) -> crate::types::Window {
    let root = if pane_ids.len() == 1 {
        Node::Leaf(make_pane(pane_ids[0]))
    } else {
        Node::Split {
            kind: LayoutKind::Vertical,
            sizes: vec![100 / pane_ids.len() as u16; pane_ids.len()],
            children: pane_ids.iter().map(|&p| Node::Leaf(make_pane(p))).collect(),
        }
    };
    crate::types::Window {
        root,
        active_path: if pane_ids.len() == 1 { vec![] } else { vec![0] },
        name: name.to_string(),
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

/// Session `s`: window 0 (@10, "main") with panes %1 %2 %3, window 1
/// (@11, "other") with pane %4.  Active: window 0, pane %1.
fn app() -> AppState {
    let mut app = AppState::new("s".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(make_window(10, "main", &[1, 2, 3]));
    app.windows.push(make_window(11, "other", &[4]));
    app.active_idx = 0;
    app
}

fn active(app: &AppState) -> (usize, usize) {
    let w = &app.windows[app.active_idx];
    (app.active_idx, crate::tree::get_active_pane_id(&w.root, &w.active_path).unwrap())
}

fn pane_index(i: usize) -> TempTarget {
    TempTarget { pane: Some(i), ..Default::default() }
}

fn window_index(i: usize) -> TempTarget {
    TempTarget { win: Some(i), ..Default::default() }
}

#[test]
fn validation_resolves_to_stable_ids() {
    let app = app();
    // s:0.1 -> %2 (the active window's second pane)
    let r = resolve_temp_target(&app, &pane_index(1)).unwrap();
    assert_eq!(r, TempTarget { win: None, win_is_id: false, win_name: None, pane: Some(2), pane_is_id: true });
    assert!(r.is_resolved());
    // s:1 -> @11, no pane part (the window's active pane when it runs)
    let r = resolve_temp_target(&app, &window_index(1)).unwrap();
    assert_eq!(r, TempTarget { win: Some(11), win_is_id: true, win_name: None, pane: None, pane_is_id: false });
    // s:other.0 -> @11 + %4
    let t = TempTarget { win_name: Some("other".into()), pane: Some(0), ..Default::default() };
    let r = resolve_temp_target(&app, &t).unwrap();
    assert_eq!((r.win, r.pane), (Some(11), Some(4)));
    assert!(r.is_resolved());
    // %3 stays %3
    let t = TempTarget { pane: Some(3), pane_is_id: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap().pane, Some(3));
    // Resolution changes nothing.
    assert_eq!(active(&app), (0, 1));
}

#[test]
fn validation_reports_tmux_messages_for_missing_targets() {
    let app = app();
    assert_eq!(resolve_temp_target(&app, &window_index(7)).unwrap_err(), "can't find window: 7");
    assert_eq!(resolve_temp_target(&app, &pane_index(5)).unwrap_err(), "can't find pane: 5");
    let t = TempTarget { pane: Some(99), pane_is_id: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap_err(), "can't find pane: %99");
    let t = TempTarget { win: Some(77), win_is_id: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap_err(), "can't find window: @77");
    let t = TempTarget { win_name: Some("nope".into()), ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap_err(), "can't find window: nope");
}

#[test]
fn apply_focuses_the_target_and_restore_puts_the_real_focus_back() {
    let mut app = app();
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &TempTarget { win: Some(11), win_is_id: true, pane: Some(4), pane_is_id: true, ..Default::default() }).unwrap();
    assert_eq!(active(&app), (1, 4));
    assert_eq!(app.temp_focus_saved_active, Some(0), "#{{window_active}} must still see the real window");
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(active(&app), (0, 1));
    assert_eq!(saved, None);
    assert_eq!(app.temp_focus_saved_active, None);
}

/// The defect in one picture: three requests in one server batch, a
/// targeted one from client X, an untargeted one from client Y (or a pane
/// output wake), and another targeted one from client Z.  Each must act on
/// its own pane: X on %2, Y on the real active pane %1, Z on %4.  Under
/// FocusTargetTemp, X's focus request and its command were two requests, and
/// whatever landed between them ran against %2 or put %1 back before X's
/// command ran.
#[test]
fn interleaved_requests_each_act_on_their_own_target() {
    let mut app = app();
    let mut saved = None;
    let x = resolve_temp_target(&app, &pane_index(1)).unwrap();
    let z = resolve_temp_target(&app, &window_index(1)).unwrap();
    let batch: Vec<(&str, Option<TempTarget>)> = vec![("X", Some(x)), ("Y", None), ("Z", Some(z))];
    let mut acted_on = Vec::new();
    for (who, target) in batch {
        // The server loop: restore, unwrap and apply, run, restore.
        restore_temp_focus(&mut app, &mut saved);
        if let Some(t) = target {
            apply_temp_target(&mut app, &mut saved, &t).unwrap();
        }
        acted_on.push((who, active(&app).1));
        restore_temp_focus(&mut app, &mut saved);
    }
    assert_eq!(acted_on, vec![("X", 2), ("Y", 1), ("Z", 4)]);
    assert_eq!(active(&app), (0, 1));
}

/// A resolved target names the pane, not a position: after the panes of
/// window 0 are reordered (a swap-pane in between), `s:0.1` resolved
/// earlier still means %2, never whatever now sits at index 1.
#[test]
fn a_resolved_target_survives_a_structural_change() {
    let mut app = app();
    let mut saved = None;
    let t = resolve_temp_target(&app, &pane_index(1)).unwrap();
    if let Node::Split { children, .. } = &mut app.windows[0].root {
        children.swap(0, 1); // index 1 is now %1
    }
    app.windows[0].active_path = vec![1]; // still %1 active
    apply_temp_target(&mut app, &mut saved, &t).unwrap();
    assert_eq!(active(&app).1, 2);
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(active(&app).1, 1);
}

/// A target that vanished after validation fails to apply and changes
/// nothing, so the server drops the request instead of running it against
/// the active pane.
#[test]
fn a_vanished_target_is_not_applied() {
    let mut app = app();
    let mut saved = None;
    let t = resolve_temp_target(&app, &window_index(1)).unwrap();
    app.windows.pop();
    assert_eq!(apply_temp_target(&mut app, &mut saved, &t).unwrap_err(), "can't find window: @11");
    assert_eq!(saved, None);
    assert_eq!(active(&app), (0, 1));
}

/// Every request a targeted command sends carries the target, not just the
/// first (send-keys -N 3 sends three), and an untargeted command's requests
/// pass through untouched.
#[test]
fn targeted_sender_wraps_every_request() {
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    let t = TempTarget { pane: Some(2), pane_is_id: true, ..Default::default() };
    let s = TargetedSender::new(&tx, Some(t.clone()));
    for _ in 0..3 {
        s.send(CtrlReq::SendKeys(vec!["Q".into()], true)).unwrap();
    }
    for _ in 0..3 {
        match rx.try_recv().unwrap() {
            CtrlReq::Targeted(got, inner) => {
                assert_eq!(got, t);
                assert!(matches!(*inner, CtrlReq::SendKeys(..)));
            }
            _ => panic!("a request of a targeted command went out without its target"),
        }
    }
    let plain = TargetedSender::new(&tx, None);
    plain.send(CtrlReq::ClearHistory).unwrap();
    assert!(matches!(rx.try_recv().unwrap(), CtrlReq::ClearHistory));
}
