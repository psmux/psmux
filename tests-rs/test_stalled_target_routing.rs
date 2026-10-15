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
pub(super) fn app() -> AppState {
    let mut app = AppState::new("s".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(make_window(10, "main", &[1, 2, 3]));
    app.windows.push(make_window(11, "other", &[4]));
    app.active_idx = 0;
    app
}

pub(super) fn active(app: &AppState) -> (usize, usize) {
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
    assert_eq!(r, TempTarget { win: None, win_is_id: false, win_name: None, pane: Some(2), pane_is_id: true, session: None, pane_check_only: false });
    assert!(r.is_resolved());
    // s:1 -> @11, no pane part (the window's active pane when it runs)
    let r = resolve_temp_target(&app, &window_index(1)).unwrap();
    assert_eq!(r, TempTarget { win: Some(11), win_is_id: true, win_name: None, pane: None, pane_is_id: false, session: None, pane_check_only: false });
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

// Issue #793: a target's session part is a claim about which server the
// connection reached. Over the raw TCP protocol the reporter's script read the
// first `<ns>*.port` file, which is the namespace's warm standby
// (`<ns>____warm__` sorts before `<ns>__t`), and the standby ran
// `send-keys -t t:@1.%1 C-c` and `respawn-pane -k -t t:@1.%1` on ITS OWN %1 and
// answered OK. tmux answers "can't find session: t" (cmd-find.c).

fn named(name: &str, ns: Option<&str>, id: usize) -> AppState {
    let mut a = app();
    a.session_name = name.to_string();
    a.socket_name = ns.map(|s| s.to_string());
    a.session_id = id;
    a
}

fn session_only(s: &str) -> TempTarget {
    TempTarget { session: Some(s.to_string()), ..Default::default() }
}

#[test]
fn issue793_foreign_session_is_cant_find_session() {
    let warm = named("__warm__", Some("ns793unit"), 4);
    let t = TempTarget {
        win: Some(10), win_is_id: true, pane: Some(1), pane_is_id: true,
        session: Some("t".into()), ..Default::default()
    };
    assert_eq!(resolve_temp_target(&warm, &t).unwrap_err(), "can't find session: t");
    assert_eq!(resolve_temp_target(&warm, &session_only("t")).unwrap_err(), "can't find session: t");
    // Checked before the window and pane, so the message names the session.
    let t = TempTarget { pane: Some(99), pane_is_id: true, session: Some("bogus".into()), ..Default::default() };
    assert_eq!(resolve_temp_target(&warm, &t).unwrap_err(), "can't find session: bogus");
    // Nothing moved.
    assert_eq!(active(&warm), (0, 1));
}

#[test]
fn issue793_own_session_in_every_spelling_resolves() {
    let a = named("work", Some("ns793unit"), 7);
    for s in ["work", "=work", "ns793unit__work", "$7", "wo"] {
        assert!(session_names_this_server(&a, s), "{s} names this server");
        let t = TempTarget { pane: Some(2), pane_is_id: true, session: Some(s.into()), ..Default::default() };
        let r = resolve_temp_target(&a, &t).unwrap();
        assert_eq!((r.pane, r.session.as_deref()), (Some(2), None), "{s}");
    }
    // A session-only target resolves to no window and no pane.
    let r = resolve_temp_target(&a, &session_only("work")).unwrap();
    assert_eq!((r.win, r.pane), (None, None));
    for s in ["Work", "worker", "$8", "other", "ns793unit__other", "__warm__"] {
        assert!(!session_names_this_server(&a, s), "{s} is another session");
    }
}

#[test]
fn issue793_which_targets_carry_a_session_to_check() {
    use crate::server::connection::explicit_target_session;
    let t = Some("t:@1.%1");
    assert_eq!(explicit_target_session("send-keys", t).as_deref(), Some("t"));
    assert_eq!(explicit_target_session("respawn-pane", t).as_deref(), Some("t"));
    assert_eq!(explicit_target_session("respawn-window", Some("t:@1")).as_deref(), Some("t"));
    assert_eq!(explicit_target_session("kill-window", Some("=t:@1")).as_deref(), Some("t"));
    assert_eq!(explicit_target_session("select-pane", Some("t:0.1")).as_deref(), Some("t"));
    // No explicit session: pane ids, window forms and bare words are unchanged.
    for raw in ["%1", "@1.%1", ":1", ":.+", "0", "t", "main"] {
        assert_eq!(explicit_target_session("send-keys", Some(raw)), None, "{raw}");
    }
    assert_eq!(explicit_target_session("send-keys", None), None);
    // Targets that name another session or a client on purpose.
    for cmd in ["switch-client", "detach-client", "kill-session", "has-session", "new-session",
                "move-window", "link-window", "join-pane", "move-pane", "swap-window", "swap-pane",
                "set-option", "refresh-client", "display-panes"] {
        assert_eq!(explicit_target_session(cmd, t), None, "{cmd}");
    }
}

// Issue #793 follow up: commands that resolve the pane of their -t themselves
// (display-message, capture-pane and kill-pane by %id, select-pane) skipped
// validation, so a pane that does not exist gave the ACTIVE pane's values
// (`display-message -p -t %99` printed `%1`), an empty capture, or nothing,
// all at exit 0 over TCP and control mode. tmux: "can't find pane: %99".

fn check_only(pane: usize, is_id: bool) -> TempTarget {
    TempTarget { pane: Some(pane), pane_is_id: is_id, pane_check_only: true, ..Default::default() }
}

fn add_float(app: &mut AppState, win: usize, id: usize) {
    app.windows[win].floating.push(crate::types::FloatingPane {
        pane: make_pane(id),
        x: 0, y: 0, w: 10, h: 5,
        border: String::new(),
        id,
        title: String::new(),
        position: None,
        styles: Default::default(),
    });
}

#[test]
fn issue793_check_only_pane_must_exist() {
    let mut app = app();
    add_float(&mut app, 1, 9);
    assert_eq!(resolve_temp_target(&app, &check_only(99, true)).unwrap_err(), "can't find pane: %99");
    assert_eq!(resolve_temp_target(&app, &check_only(7, false)).unwrap_err(), "can't find pane: 7");
    // A tiled pane in another window and a floating pane both exist; the
    // pane is not part of the resolved target (the command resolves it).
    for t in [check_only(4, true), check_only(9, true), check_only(2, false)] {
        let r = resolve_temp_target(&app, &t).unwrap();
        assert_eq!((r.win, r.pane, r.pane_check_only), (None, None, false), "{t:?}");
    }
    // A floating pane's index follows the tiled ones, as #{pane_index} does:
    // window 1 has %4 at 0 and the float %9 at 1.
    let t = TempTarget { win: Some(1), pane: Some(1), pane_check_only: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap().win, Some(11));
    let t = TempTarget { win: Some(1), pane: Some(2), pane_check_only: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap_err(), "can't find pane: 2");
    // The window part is still resolved and still fails first.
    let t = TempTarget { win: Some(9), pane: Some(0), pane_check_only: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap_err(), "can't find window: 9");
    assert_eq!(active(&app), (0, 1));
}

#[test]
fn issue793_which_commands_check_or_can_fail() {
    use crate::server::connection::{pane_checked_not_focused, target_can_fail};
    assert!(pane_checked_not_focused("display-message", false));
    assert!(pane_checked_not_focused("display", true));
    assert!(pane_checked_not_focused("capture-pane", true));
    assert!(pane_checked_not_focused("kill-pane", true));
    assert!(pane_checked_not_focused("killp", true));
    // By index those two take the ordinary focused route.
    assert!(!pane_checked_not_focused("capture-pane", false));
    assert!(!pane_checked_not_focused("kill-pane", false));
    // swap-pane resolves and reports its own target.
    assert!(!pane_checked_not_focused("swap-pane", true));
    // tmux CMD_FIND_CANFAIL commands that run without a target.
    for c in ["run-shell", "run", "if-shell", "if", "source-file", "source"] {
        assert!(target_can_fail(c), "{c}");
    }
    for c in ["display-message", "send-keys", "set-option", "kill-pane"] {
        assert!(!target_can_fail(c), "{c}");
    }
}

// Issue #793 follow up: a floating pane is a pane of its window for every
// by-id request. It resolves, a -t naming it runs the request with it focused
// (`floating_focus`, which input follows first), and a -t naming a TILED pane
// takes that focus away for the request so a focused float cannot swallow it.

#[test]
fn issue793_floating_target_resolves_and_focuses_for_one_request() {
    let mut app = app();
    add_float(&mut app, 1, 9);
    let t = TempTarget { pane: Some(9), pane_is_id: true, ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap().pane, Some(9));
    // By index: window 1 has %4 at 0, the float at 1.
    let t = TempTarget { win: Some(1), pane: Some(1), ..Default::default() };
    assert_eq!(resolve_temp_target(&app, &t).unwrap().pane, Some(9));
    let mut saved = None;
    let t = TempTarget { pane: Some(9), pane_is_id: true, ..Default::default() };
    apply_temp_target(&mut app, &mut saved, &t).unwrap();
    assert_eq!((app.active_idx, app.windows[1].floating_focus), (1, Some(0)));
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!((app.active_idx, app.windows[1].floating_focus), (0, None));
    assert_eq!(active(&app), (0, 1));
}

#[test]
fn issue793_tiled_target_is_not_swallowed_by_a_focused_float() {
    let mut app = app();
    add_float(&mut app, 0, 8);
    app.windows[0].floating_focus = Some(0);
    let mut saved = None;
    let t = TempTarget { pane: Some(2), pane_is_id: true, ..Default::default() };
    apply_temp_target(&mut app, &mut saved, &t).unwrap();
    assert_eq!(app.windows[0].floating_focus, None, "the request runs on %2, not on the float");
    assert_eq!(active(&app), (0, 2));
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(app.windows[0].floating_focus, Some(0), "the float gets its focus back");
    assert_eq!(active(&app), (0, 1));
}

#[test]
fn issue793_focus_pane_by_id_moves_floating_focus() {
    let mut app = app();
    add_float(&mut app, 1, 9);
    crate::tree::focus_pane_by_id(&mut app, 9);
    assert_eq!((app.active_idx, app.windows[1].floating_focus), (1, Some(0)));
    crate::tree::focus_pane_by_id(&mut app, 4);
    assert_eq!((app.active_idx, app.windows[1].floating_focus), (1, None));
}

#[test]
fn issue793_swap_pane_trades_a_float_with_a_tiled_pane() {
    let mut app = app();
    add_float(&mut app, 0, 8);
    assert_eq!(crate::window_ops::swap_pane_by_spec(&mut app, Some("%8"), "%2", true), Ok(true));
    assert_eq!(app.windows[0].floating[0].id, 2);
    assert_eq!(app.windows[0].floating[0].pane.id, 2);
    assert!(crate::tree::find_pane_by_id_global(&app, 8).is_some(), "%8 is tiled now");
    assert!(crate::tree::find_pane_by_id_global(&app, 2).is_none());
    // And two floats in different windows trade places too.
    add_float(&mut app, 1, 9);
    assert_eq!(crate::window_ops::swap_pane_by_spec(&mut app, Some("%2"), "%9", true), Ok(true));
    assert_eq!((app.windows[0].floating[0].id, app.windows[1].floating[0].id), (9, 2));
}