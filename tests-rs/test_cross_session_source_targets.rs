// Source and target specs that name a pane by %id or another session.
//
// 1. The server read join-pane `-s %N` as pane INDEX N of the active window,
//    so `join-pane -s %3 -t sess:0` with %3 already in window 0 did nothing
//    at exit 0 (tmux 3.x: `can't join a pane to its own window`), and a
//    `-s %N` in another window moved whatever pane sat at index N.
//    commands::join_pane_request resolves both ends strictly first.
// 2. swap-pane, link-window and move-window dropped the session in -s and
//    acted on this session's pane or window of the same number.
//    commands::foreign_session_in_spec spots a spec naming another live
//    session and the commands refuse it.
// 3. The cross session join sent no target and parsed `1.0` as session "1"
//    pane 0. cross_session::window_pane_half builds the spec both pane
//    forward commands read.
//
// Registered from src/commands.rs.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node, TempTarget};
use ratatui::layout::Rect;

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    let child = crate::util::StubChild::exited();
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
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
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize, name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: Rect::new(0, 0, 160, 40),
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

/// Window i holds the pane ids given; display indices 0, 1, 2 and so on.
fn app_with_windows(windows: &[&[usize]]) -> AppState {
    let mut app = AppState::new("xsrc".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    app.client_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    for (w, ids) in windows.iter().enumerate() {
        let mut win = make_window(w, &format!("win{w}"));
        let n = ids.len().max(1);
        if ids.len() == 1 {
            win.root = Node::Leaf(make_pane(ids[0], 40, 160));
            win.active_path = vec![];
        } else {
            let children: Vec<Node> = ids.iter()
                .map(|&id| Node::Leaf(make_pane(id, 40, 160 / n as u16)))
                .collect();
            win.root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![(100 / n) as u16; n], children };
            win.active_path = vec![0];
        }
        win.pane_mru = ids.to_vec();
        app.windows.push(win);
        app.window_indices.push(w);
    }
    app.next_win_id = windows.len();
    app.active_idx = 0;
    app
}

fn ids_in(app: &AppState, w: usize) -> Vec<usize> {
    crate::tree::collect_pane_ids(&app.windows[w].root)
}

fn pane_id(id: usize) -> TempTarget {
    TempTarget { pane: Some(id), pane_is_id: true, ..Default::default() }
}

fn win(w: usize) -> TempTarget {
    TempTarget { win: Some(w), ..Default::default() }
}

fn win_pane(w: usize, p: usize) -> TempTarget {
    TempTarget { win: Some(w), pane: Some(p), ..Default::default() }
}

// ---- 1. join-pane -s %id ------------------------------------------------

#[test]
fn source_pane_id_in_the_target_window_is_refused() {
    // `join-pane -s %3 -t sess:0` with %3 in window 0.
    let mut app = app_with_windows(&[&[1, 2, 3], &[4]]);
    let r = crate::commands::join_pane_request(&mut app, Some("%3"), &pane_id(3), &win(0), false, false, false);
    assert_eq!(r, Err("can't join a pane to its own window".to_string()));
    assert_eq!(ids_in(&app, 0), vec![1, 2, 3]);
    assert_eq!(ids_in(&app, 1), vec![4]);
}

#[test]
fn source_pane_id_is_an_id_not_an_index() {
    // Active window is 1. `-s %3` names pane id 3 in window 0. The old reading
    // (pane index 3 of the active window) found nothing and moved nothing, or,
    // with enough panes, moved the wrong one.
    let mut app = app_with_windows(&[&[1, 3], &[2, 4, 5, 6]]);
    app.active_idx = 1;
    let r = crate::commands::join_pane_request(&mut app, Some("%3"), &pane_id(3), &win(1), false, false, false);
    assert_eq!(r, Ok(()));
    assert_eq!(ids_in(&app, 0), vec![1]);
    assert!(ids_in(&app, 1).contains(&3), "pane 3 joined window 1: {:?}", ids_in(&app, 1));
    assert_eq!(ids_in(&app, 1).len(), 5);
}

#[test]
fn source_pane_id_that_does_not_exist_is_refused() {
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    let r = crate::commands::join_pane_request(&mut app, Some("%99"), &pane_id(99), &win(1), false, false, false);
    assert_eq!(r, Err("can't find pane: %99".to_string()));
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
}

#[test]
fn target_pane_id_puts_the_pane_right_after_it() {
    // `join-pane -s :1 -t %1`: window 0 is 1 2 3 with 1 active... make 3 active
    // so landing beside the active pane would be visible.
    let mut app = app_with_windows(&[&[1, 2, 3], &[4]]);
    app.windows[0].active_path = vec![2];
    let r = crate::commands::join_pane_request(&mut app, Some(":1"), &win(1), &pane_id(1), false, false, false);
    assert_eq!(r, Ok(()));
    let order = ids_in(&app, 0);
    let at = order.iter().position(|&i| i == 4).expect("pane 4 joined window 0");
    assert_eq!(order[at - 1], 1, "the joined pane follows its target: {:?}", order);
}

#[test]
fn colon_specs_in_the_same_window_are_still_refused() {
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    let r = crate::commands::join_pane_request(&mut app, Some(":0.1"), &win_pane(0, 1), &win(0), false, false, false);
    assert_eq!(r, Err("can't join a pane to its own window".to_string()));
}

#[test]
fn resolve_join_end_reports_tmux_messages() {
    let app = app_with_windows(&[&[1, 2], &[3]]);
    assert_eq!(crate::commands::resolve_join_end(&app, &win(7)), Err("can't find window: 7".to_string()));
    assert_eq!(crate::commands::resolve_join_end(&app, &win_pane(0, 5)), Err("can't find pane: 5".to_string()));
    assert_eq!(crate::commands::resolve_join_end(&app, &pane_id(3)), Ok((1, Some(0))));
    assert_eq!(crate::commands::resolve_join_end(&app, &win_pane(0, 1)), Ok((0, Some(1))));
    // No window and no pane: the active window, its active pane.
    assert_eq!(crate::commands::resolve_join_end(&app, &TempTarget::default()), Ok((0, None)));
}

// ---- 2. a spec that names another session --------------------------------

#[test]
fn foreign_session_is_only_a_live_other_session() {
    let _g = crate::util::lock_test_env();
    let dir = std::env::temp_dir().join(format!("psmux_xsrc_unit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let saved = std::env::var_os("PSMUX_DATA_DIR");
    std::env::set_var("PSMUX_DATA_DIR", &dir);

    let mut app = app_with_windows(&[&[1]]);
    app.session_name = "sa".to_string();
    // A live server for sb in the default namespace and for ns__sb under -L.
    std::fs::write(dir.join("sb.port"), "1").unwrap();
    std::fs::write(dir.join("ns__sb.port"), "1").unwrap();

    let check = |app: &AppState| {
        (
            crate::commands::foreign_session_in_spec(app, "sb:0.0"),
            crate::commands::foreign_session_in_spec(app, "=sb:0"),
            crate::commands::foreign_session_in_spec(app, "sa:0.1"),
            crate::commands::foreign_session_in_spec(app, ":0.1"),
            crate::commands::foreign_session_in_spec(app, "%3"),
            crate::commands::foreign_session_in_spec(app, "nosuch:0"),
        )
    };
    let none = None::<String>;
    assert_eq!(check(&app), (Some("sb".to_string()), Some("sb".to_string()), none.clone(), none.clone(), none.clone(), none.clone()));

    // Under -L the session is `ns__sa` on disk while the user types `sa`.
    app.socket_name = Some("ns".to_string());
    assert_eq!(check(&app), (Some("sb".to_string()), Some("sb".to_string()), none.clone(), none.clone(), none.clone(), none.clone()));
    assert_eq!(crate::commands::foreign_session_in_spec(&app, "ns__sa:0"), None);

    match saved { Some(v) => std::env::set_var("PSMUX_DATA_DIR", v), None => std::env::remove_var("PSMUX_DATA_DIR") }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cross_session_refusals_name_the_command_and_session() {
    for cmd in ["swap-pane", "link-window", "move-window"] {
        let m = crate::commands::cross_session_refusal(cmd, "sb");
        assert!(m.starts_with(&format!("{}: source is in session sb", cmd)), "{}", m);
        assert!(m.contains(&format!("cross-session {} is not supported", cmd)), "{}", m);
    }
}

// ---- 3. the window/pane halves the pane forward commands read -----------

#[test]
fn window_pane_half_keeps_window_and_pane() {
    use crate::cross_session::window_pane_half;
    assert_eq!(window_pane_half("sb:1.0"), ":1.0");
    assert_eq!(window_pane_half("sa:0"), ":0");
    assert_eq!(window_pane_half("sa:"), ":");
    assert_eq!(window_pane_half(""), ":");
    assert_eq!(window_pane_half("sa:%3"), "%3");
    assert_eq!(window_pane_half("%3"), "%3");
    assert_eq!(window_pane_half("sa:@2"), "@2");
    // `1.0` read without its colon is session "1" pane 0; with it, window 1.
    let pt = crate::cli::parse_target(&window_pane_half("sb:1.0"));
    assert_eq!((pt.window, pt.pane, pt.session), (Some(1), Some(0), None));
    let pt = crate::cli::parse_target(&window_pane_half("sa:%3"));
    assert_eq!((pt.pane, pt.pane_is_id), (Some(3), true));
}
