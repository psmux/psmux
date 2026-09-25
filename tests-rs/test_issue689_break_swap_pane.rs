// Issue #689: break-pane ignored -d, silently accepted -s and broke the
// ACTIVE pane instead, and swap-pane did nothing when -s and -t named panes
// in two different windows.
//
// Three separate defects, one shape: the command line stopped carrying
// information at the dispatch boundary.
//
//   * `break-pane` reached the server as a payload free CtrlReq::BreakPane,
//     and break_pane_to_window always took app.windows[active_idx]
//     .active_path and always ended with app.active_idx = len - 1.
//   * `swap-pane` resolved BOTH halves inside app.windows[app.active_idx], so
//     a cross window pair resolved to the same pane twice and exited 0.
//
// These tests drive the REAL parser and the REAL operations on a pane tree
// backed by real PTYs, the way tests-rs/test_issue442_swap_pane_source.rs
// does. Registered from src/window_ops.rs.
//
// tmux references: cmd-break-pane.c (flag set :37, -d :186, -s :42,
// -t :43, index in use :148, -a/-b :119, -n :169, -P/-F :29 and :199),
// cmd-swap-pane.c (cross window splice :143 to :155, focus :164 to :177),
// cmd-find.c (session before window :348, no pane in an index target :1153).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use crate::window_ops::BreakPaneRequest;
use ratatui::layout::Rect;

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = crate::util::spawn_pty_child(&*pair.slave, cmd).expect("spawn dummy");
    let writer = pair.master.take_writer().expect("writer");
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: pair.master,
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
        mouse_input_cache: None, win32_input_latched: false,
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

/// A session of `windows.len()` windows, window i holding the pane ids given.
/// Window display indices are 0, 1, 2 ... and pane-base-index is 0.
fn app_with_windows(windows: &[&[usize]]) -> AppState {
    let mut app = AppState::new("issue689".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    app.client_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    let mut next_id = 1usize;
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
        next_id = next_id.max(w + 1);
    }
    app.next_win_id = next_id;
    app.active_idx = 0;
    app
}

fn ids_in(app: &AppState, w: usize) -> Vec<usize> {
    crate::tree::collect_pane_ids(&app.windows[w].root)
}
fn id_at(app: &AppState, w: usize, i: usize) -> usize {
    crate::tree::get_nth_pane(&app.windows[w].root, i).map(|p| p.id).unwrap_or(usize::MAX)
}
fn active_id_in(app: &AppState, w: usize) -> usize {
    crate::tree::get_active_pane_id(&app.windows[w].root, &app.windows[w].active_path).unwrap()
}

// ─────────────────────────────────────────────────────────────────────────
// FLAG PARSING (src/server/connection.rs::parse_break_pane_args)
//
// The parser is what did not exist at all: every one of these used to be
// dropped on the floor before the request left the connection thread.
// ─────────────────────────────────────────────────────────────────────────

use crate::server::connection::{parse_break_pane_args, BREAK_PANE_TEMPLATE};

#[test]
fn parse_bare_break_pane_has_no_flags_set() {
    let (req, print) = parse_break_pane_args(&[], None);
    assert!(!req.detach, "bare break-pane switches to the new window");
    assert!(req.src.is_none() && req.dst.is_none() && req.name.is_none());
    assert!(!req.after && !req.before);
    assert!(print.is_none(), "no -P means no printed target");
}

#[test]
fn parse_detach_flag() {
    let (req, _) = parse_break_pane_args(&["-d"], None);
    assert!(req.detach, "-d must survive the parse (issue #689 part one)");
}

#[test]
fn parse_source_flag_takes_its_value() {
    let (req, _) = parse_break_pane_args(&["-s", "sess:0.2"], None);
    assert_eq!(req.src.as_deref(), Some("sess:0.2"), "-s names the pane to break");
}

#[test]
fn parse_source_and_detach_together() {
    let (req, _) = parse_break_pane_args(&["-d", "-s", "%7"], None);
    assert!(req.detach);
    assert_eq!(req.src.as_deref(), Some("%7"));
}

#[test]
fn parse_target_flag_is_the_destination_window() {
    let (req, _) = parse_break_pane_args(&["-t", "3"], None);
    assert_eq!(req.dst.as_deref(), Some("3"));
}

#[test]
fn parse_outer_target_becomes_the_destination() {
    // The generic -t parser peels the outer target off the command line and
    // hands it over separately; for break-pane it is a destination window.
    let (req, _) = parse_break_pane_args(&["-d"], Some("sess:2"));
    assert_eq!(req.dst.as_deref(), Some("sess:2"));
    assert!(req.detach);
}

#[test]
fn parse_inline_target_overrides_the_outer_one() {
    let (req, _) = parse_break_pane_args(&["-t", "9"], Some("sess"));
    assert_eq!(req.dst.as_deref(), Some("9"));
}

#[test]
fn parse_name_flag() {
    let (req, _) = parse_break_pane_args(&["-n", "logs"], None);
    assert_eq!(req.name.as_deref(), Some("logs"));
}

#[test]
fn parse_after_and_before_flags() {
    let (a, _) = parse_break_pane_args(&["-a"], None);
    assert!(a.after && !a.before);
    let (b, _) = parse_break_pane_args(&["-b"], None);
    assert!(b.before && !b.after);
}

#[test]
fn parse_print_uses_tmux_default_template() {
    let (_, print) = parse_break_pane_args(&["-P"], None);
    assert_eq!(print.as_deref(), Some(BREAK_PANE_TEMPLATE),
        "-P without -F prints tmux's BREAK_PANE_TEMPLATE (cmd-break-pane.c:29)");
}

#[test]
fn parse_print_with_explicit_format() {
    let (_, print) = parse_break_pane_args(&["-P", "-F", "#{pane_id}"], None);
    assert_eq!(print.as_deref(), Some("#{pane_id}"));
}

#[test]
fn parse_format_without_print_prints_nothing() {
    // tmux only prints when -P is present; -F alone just supplies a template.
    let (_, print) = parse_break_pane_args(&["-F", "#{pane_id}"], None);
    assert!(print.is_none());
}

#[test]
fn parse_every_flag_at_once() {
    let (req, print) = parse_break_pane_args(
        &["-d", "-a", "-P", "-F", "#{window_index}", "-n", "side", "-s", "%3", "-t", "5"], None);
    assert!(req.detach && req.after);
    assert_eq!(req.src.as_deref(), Some("%3"));
    assert_eq!(req.dst.as_deref(), Some("5"));
    assert_eq!(req.name.as_deref(), Some("side"));
    assert_eq!(print.as_deref(), Some("#{window_index}"));
}

#[test]
fn parse_strips_quotes_from_values() {
    let (req, _) = parse_break_pane_args(&["-n", "\"my win\""], None);
    assert_eq!(req.name.as_deref(), Some("my win"));
}

// ─────────────────────────────────────────────────────────────────────────
// PANE TARGET RESOLUTION (window_ops::resolve_pane_spec)
// tmux CMD_FIND_PANE searches the whole session, not the active window.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn resolve_pane_by_id_finds_it_in_another_window() {
    let app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let (w, p) = crate::window_ops::resolve_pane_spec(&app, "%4").expect("%4 resolves");
    assert_eq!(w, 1, "pane 4 lives in window 1, not the active window 0");
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w].root, &p), Some(4));
}

#[test]
fn resolve_window_and_pane_index() {
    let app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let (w, p) = crate::window_ops::resolve_pane_spec(&app, "sess:1.1").expect("resolves");
    assert_eq!(w, 1);
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w].root, &p), Some(4));
}

#[test]
fn resolve_window_name_and_pane_index() {
    let app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let (w, p) = crate::window_ops::resolve_pane_spec(&app, "sess:win1.0").expect("resolves");
    assert_eq!(w, 1);
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w].root, &p), Some(3));
}

#[test]
fn resolve_leading_token_is_a_window_when_it_is_not_this_session() {
    // psmux's parse_target files the leading token of `win1.0` / `1.0` as a
    // SESSION name; tmux splits on '.' and reads it as a window. For a pane
    // target, fall back to a window in this session the way tmux does
    // (cmd-find.c:348), so `-s 1.1` means window 1 pane 1 and not the active
    // window's pane 1.
    let app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let (w, p) = crate::window_ops::resolve_pane_spec(&app, "1.1").expect("resolves");
    assert_eq!(w, 1, "window 1, not the active window 0");
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w].root, &p), Some(4));

    let (w2, p2) = crate::window_ops::resolve_pane_spec(&app, "win1.0").expect("resolves");
    assert_eq!(w2, 1);
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w2].root, &p2), Some(3));
}

#[test]
fn resolve_this_sessions_own_name_is_not_a_window() {
    // `issue689:1.0` still means "this session, window 1, pane 0".
    let app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let (w, _) = crate::window_ops::resolve_pane_spec(&app, "issue689:1.0").expect("resolves");
    assert_eq!(w, 1);
}

#[test]
fn resolve_bare_pane_index_uses_the_active_window() {
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    app.active_idx = 1;
    let (w, p) = crate::window_ops::resolve_pane_spec(&app, ".1").expect("resolves");
    assert_eq!(w, 1);
    assert_eq!(crate::tree::get_active_pane_id(&app.windows[w].root, &p), Some(4));
}

#[test]
fn resolve_missing_pane_is_a_tmux_shaped_error() {
    let app = app_with_windows(&[&[1, 2]]);
    let err = crate::window_ops::resolve_pane_spec(&app, "%99").unwrap_err();
    assert_eq!(err, "can't find pane: %99");
}

#[test]
fn resolve_missing_window_is_a_tmux_shaped_error() {
    let app = app_with_windows(&[&[1, 2]]);
    let err = crate::window_ops::resolve_pane_spec(&app, "sess:7.0").unwrap_err();
    assert_eq!(err, "can't find window: 7");
}

#[test]
fn resolve_pane_index_past_the_end_is_an_error() {
    let app = app_with_windows(&[&[1, 2]]);
    let err = crate::window_ops::resolve_pane_spec(&app, ".9").unwrap_err();
    assert_eq!(err, "can't find pane: .9");
}

// ─────────────────────────────────────────────────────────────────────────
// break-pane OPERATION (window_ops::break_pane)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn bare_break_pane_moves_the_active_pane_and_switches() {
    let mut app = app_with_windows(&[&[1, 2, 3]]);
    app.windows[0].active_path = vec![1]; // active pane is id 2
    let out = crate::window_ops::break_pane(&mut app, &BreakPaneRequest::default()).expect("breaks");

    assert_eq!(app.windows.len(), 2);
    assert_eq!(ids_in(&app, 0), vec![1, 3], "the active pane left window 0");
    assert_eq!(ids_in(&app, 1), vec![2], "and is alone in the new window");
    assert_eq!(app.active_idx, 1, "without -d the session switches to it");
    assert_eq!(out.pane_id, Some(2));
}

#[test]
fn detach_keeps_the_current_window() {
    // Issue #689 part one, the reporter's exact repro: a two pane window, then
    // `break-pane -d`, then #{window_index} must still be 0.
    let mut app = app_with_windows(&[&[1, 2]]);
    app.windows[0].active_path = vec![1];
    let req = BreakPaneRequest { detach: true, ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(app.windows.len(), 2);
    assert_eq!(ids_in(&app, 1), vec![2], "the pane still moved out");
    assert_eq!(app.active_idx, 0, "-d must NOT switch to the new window");
    assert_eq!(app.win_display_index(app.active_idx), 0, "#{{window_index}} stays 0");
}

#[test]
fn source_flag_breaks_the_named_pane_not_the_active_one() {
    // Issue #689 part two: the reporter's Claude Code teammate backend moves
    // the active pane right after each spawn, so -s naming pane 2 must move
    // pane 2 even though pane 0 is active.
    let mut app = app_with_windows(&[&[1, 2, 3]]);
    app.windows[0].active_path = vec![0]; // active is id 1
    let req = BreakPaneRequest { src: Some("%3".to_string()), ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(ids_in(&app, 1), vec![3], "-s %3 moved pane 3");
    assert_eq!(ids_in(&app, 0), vec![1, 2], "the active pane stayed put");
}

#[test]
fn source_flag_reaches_into_another_window() {
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let req = BreakPaneRequest { src: Some("sess:1.1".to_string()), detach: true, ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(ids_in(&app, 0), vec![1, 2], "window 0 untouched");
    assert_eq!(ids_in(&app, 1), vec![3], "pane 4 left window 1");
    assert_eq!(ids_in(&app, 2), vec![4], "and is alone in the new window");
    assert_eq!(app.active_idx, 0, "-d kept us where we were");
}

#[test]
fn source_and_detach_together_move_the_named_pane_and_stay() {
    let mut app = app_with_windows(&[&[1, 2, 3]]);
    app.windows[0].active_path = vec![0];
    let req = BreakPaneRequest { src: Some(".2".to_string()), detach: true, ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(ids_in(&app, 1), vec![3]);
    assert_eq!(app.active_idx, 0);
    assert_eq!(active_id_in(&app, 0), 1, "the active pane is still pane 1");
}

#[test]
fn an_unresolvable_source_errors_and_changes_nothing() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { src: Some("%42".to_string()), ..Default::default() };
    let err = crate::window_ops::break_pane(&mut app, &req).unwrap_err();

    assert_eq!(err, "can't find pane: %42");
    assert_eq!(app.windows.len(), 1, "a refused break must not create a window");
    assert_eq!(ids_in(&app, 0), vec![1, 2], "and must not move a pane");
}

#[test]
fn name_flag_names_the_new_window_and_pins_it() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { name: Some("logs".to_string()), ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(app.windows[1].name, "logs");
    assert!(app.windows[1].manual_rename,
        "tmux clears automatic-rename when -n named the window (cmd-break-pane.c:173)");
}

#[test]
fn an_empty_name_is_refused() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { name: Some("  ".to_string()), ..Default::default() };
    let err = crate::window_ops::break_pane(&mut app, &req).unwrap_err();
    assert!(err.starts_with("invalid window name:"), "got {err}");
    assert_eq!(app.windows.len(), 1);
}

#[test]
fn target_flag_places_the_new_window_at_that_index() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { dst: Some(":6".to_string()), detach: true, ..Default::default() };
    let out = crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(app.win_display_index(out.win_pos), 6, "-t 6 puts the new window at index 6");
    assert_eq!(ids_in(&app, out.win_pos), vec![out.pane_id.unwrap()]);
}

#[test]
fn a_pane_in_the_target_is_refused_the_way_tmux_refuses_it() {
    // tmux cmd-find.c:1152 to 1156: "No pane is allowed if want an index."
    // break-pane's -t is a DESTINATION window, so naming a pane in it is an
    // error; -s is the flag that names a pane.
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { dst: Some("sess:0.1".to_string()), ..Default::default() };
    let err = crate::window_ops::break_pane(&mut app, &req).unwrap_err();
    assert_eq!(err, "can't specify pane here");
    assert_eq!(app.windows.len(), 1, "the refusal changed nothing");
}

#[test]
fn a_target_naming_only_this_session_takes_the_next_free_index() {
    // `break-pane -t <this session>` names no window, so tmux leaves the
    // destination index at -1 and the window lands at the next free slot
    // (cmd-find.c:351). The existing suites all spell -t that way.
    let mut app = app_with_windows(&[&[1, 2]]);
    let req = BreakPaneRequest { dst: Some("issue689".to_string()), detach: true, ..Default::default() };
    let out = crate::window_ops::break_pane(&mut app, &req).expect("breaks");
    assert_eq!(app.win_display_index(out.win_pos), 1);
}

#[test]
fn an_index_already_in_use_is_refused_before_anything_moves() {
    // tmux cmd-break-pane.c:148 to 151, "index in use: N" at exit 1.
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    let req = BreakPaneRequest { dst: Some(":1".to_string()), ..Default::default() };
    let err = crate::window_ops::break_pane(&mut app, &req).unwrap_err();

    assert_eq!(err, "index in use: 1");
    assert_eq!(app.windows.len(), 2, "the refusal left the session alone");
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
}

#[test]
fn after_flag_inserts_right_after_the_current_window() {
    // -a shuffles every index at or above (current + 1) up by one and lands
    // there (winlink_shuffle_up, cmd-break-pane.c:119 to 127).
    let mut app = app_with_windows(&[&[1, 2], &[3], &[4]]);
    app.active_idx = 0;
    let req = BreakPaneRequest { after: true, detach: true, ..Default::default() };
    let out = crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(app.win_display_index(out.win_pos), 1, "the broken pane took index 1");
    let old = app.windows.iter().position(|w| crate::tree::collect_pane_ids(&w.root) == vec![3]).unwrap();
    assert_eq!(app.win_display_index(old), 2, "the window that was 1 moved up to 2");
}

#[test]
fn before_flag_inserts_at_the_current_index() {
    let mut app = app_with_windows(&[&[1, 2], &[3]]);
    app.active_idx = 0;
    let req = BreakPaneRequest { before: true, detach: true, ..Default::default() };
    let out = crate::window_ops::break_pane(&mut app, &req).expect("breaks");

    assert_eq!(app.win_display_index(out.win_pos), 0, "-b takes the current index");
    let src = app.windows.iter().position(|w| crate::tree::collect_pane_ids(&w.root).contains(&2)).unwrap();
    assert_eq!(app.win_display_index(src), 1, "the source window shifted up");
}

#[test]
fn breaking_the_last_pane_of_a_window_does_not_lose_it() {
    let mut app = app_with_windows(&[&[1], &[2, 3]]);
    app.active_idx = 0;
    crate::window_ops::break_pane(&mut app, &BreakPaneRequest::default()).expect("breaks");

    let all: Vec<usize> = (0..app.windows.len()).flat_map(|w| ids_in(&app, w)).collect();
    assert!(all.contains(&1), "the only pane of window 0 survives the break");
    assert_eq!(all.len(), 3, "no pane was duplicated or dropped");
    assert!(app.active_idx < app.windows.len());
}

// ─────────────────────────────────────────────────────────────────────────
// swap-pane ACROSS WINDOWS (window_ops::swap_pane_across_windows / by_spec)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn cross_window_swap_exchanges_the_two_panes() {
    // Issue #689 part three: this used to resolve to the same pane twice,
    // return false and exit 0 with nothing changed.
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let ok = crate::window_ops::swap_pane_by_spec(&mut app, Some("sess:0.0"), "sess:1.0", false)
        .expect("both targets resolve");

    assert!(ok, "a cross window swap must actually swap");
    assert_eq!(id_at(&app, 0, 0), 3, "window 0 slot 0 now holds the -t pane");
    assert_eq!(id_at(&app, 1, 0), 1, "window 1 slot 0 now holds the -s pane");
    assert_eq!(id_at(&app, 0, 1), 2, "the other panes are untouched");
    assert_eq!(id_at(&app, 1, 1), 4);
}

#[test]
fn cross_window_swap_by_pane_id() {
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    crate::window_ops::swap_pane_by_spec(&mut app, Some("%2"), "%3", false).expect("resolves");
    assert_eq!(id_at(&app, 0, 1), 3);
    assert_eq!(id_at(&app, 1, 0), 2);
}

#[test]
fn cross_window_swap_moves_each_windows_active_pane_to_the_arrival() {
    // cmd-swap-pane.c:165 to 167: without -d each window activates the pane
    // that arrived in it.
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    app.windows[0].active_path = vec![0];
    app.windows[1].active_path = vec![1];
    crate::window_ops::swap_pane_by_spec(&mut app, Some("sess:0.0"), "sess:1.0", false).expect("resolves");

    assert_eq!(active_id_in(&app, 0), 3, "window 0 activates the pane that arrived");
    assert_eq!(active_id_in(&app, 1), 1, "window 1 activates the pane that arrived");
}

#[test]
fn cross_window_swap_with_detach_leaves_a_third_party_active_pane_alone() {
    // cmd-swap-pane.c:172 to 177: with -d a window only follows when the pane
    // that left WAS its active one.
    let mut app = app_with_windows(&[&[1, 2, 5], &[3, 4]]);
    app.windows[0].active_path = vec![2]; // active is pane 5, not the swapped one
    app.windows[1].active_path = vec![1]; // active is pane 4, not the swapped one
    crate::window_ops::swap_pane_by_spec(&mut app, Some("sess:0.0"), "sess:1.0", true).expect("resolves");

    assert_eq!(active_id_in(&app, 0), 5, "-d left the third party active pane alone");
    assert_eq!(active_id_in(&app, 1), 4);
    assert_eq!(id_at(&app, 0, 0), 3, "the panes still traded places");
    assert_eq!(id_at(&app, 1, 0), 1);
}

#[test]
fn cross_window_swap_moves_the_ids_between_the_two_mru_lists() {
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    crate::window_ops::swap_pane_by_spec(&mut app, Some("%1"), "%3", false).expect("resolves");

    assert!(app.windows[0].pane_mru.contains(&3) && !app.windows[0].pane_mru.contains(&1),
        "window 0's MRU follows the pane that arrived, got {:?}", app.windows[0].pane_mru);
    assert!(app.windows[1].pane_mru.contains(&1) && !app.windows[1].pane_mru.contains(&3),
        "window 1's MRU follows the pane that arrived, got {:?}", app.windows[1].pane_mru);
}

#[test]
fn swapping_a_single_pane_window_with_a_pane_elsewhere_works() {
    // The single pane window's root IS the leaf (an empty path), which the
    // same window swap_nodes refuses by design; the cross window path allows it.
    let mut app = app_with_windows(&[&[1], &[2, 3]]);
    crate::window_ops::swap_pane_by_spec(&mut app, Some("sess:0.0"), "sess:1.1", false).expect("resolves");

    assert_eq!(ids_in(&app, 0), vec![3]);
    assert_eq!(ids_in(&app, 1), vec![2, 1]);
}

#[test]
fn same_window_swap_still_behaves_as_before() {
    // The #442 contract is unchanged: without -d the -t pane becomes active,
    // and it sits in the src slot after the exchange.
    let mut app = app_with_windows(&[&[1, 2, 3, 4]]);
    app.windows[0].active_path = vec![1];
    crate::window_ops::swap_pane_by_spec(&mut app, Some(".0"), ".3", false).expect("resolves");

    assert_eq!(id_at(&app, 0, 0), 4);
    assert_eq!(id_at(&app, 0, 3), 1);
    assert_eq!(active_id_in(&app, 0), 4, "active follows the -t pane");
}

#[test]
fn swap_without_source_uses_the_current_pane() {
    // tmux's default source is the current pane (cmd-swap-pane.c:38).
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    app.active_idx = 0;
    app.windows[0].active_path = vec![1]; // current pane is 2
    crate::window_ops::swap_pane_by_spec(&mut app, None, "sess:1.1", false).expect("resolves");

    assert_eq!(id_at(&app, 0, 1), 4);
    assert_eq!(id_at(&app, 1, 1), 2);
}

#[test]
fn swapping_a_pane_with_itself_is_a_no_op() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let ok = crate::window_ops::swap_pane_by_spec(&mut app, Some("%1"), "%1", false).expect("resolves");
    assert!(!ok, "src == dst swaps nothing");
    assert_eq!(ids_in(&app, 0), vec![1, 2]);
}

#[test]
fn an_unresolvable_swap_target_errors_instead_of_exiting_zero() {
    let mut app = app_with_windows(&[&[1, 2], &[3, 4]]);
    let err = crate::window_ops::swap_pane_by_spec(&mut app, Some("%1"), "%77", false).unwrap_err();
    assert_eq!(err, "can't find pane: %77");
    assert_eq!(ids_in(&app, 0), vec![1, 2], "nothing moved");
    assert_eq!(ids_in(&app, 1), vec![3, 4]);
}

#[test]
fn an_unresolvable_swap_source_errors_too() {
    let mut app = app_with_windows(&[&[1, 2]]);
    let err = crate::window_ops::swap_pane_by_spec(&mut app, Some("sess:4.0"), "%2", false).unwrap_err();
    assert_eq!(err, "can't find window: 4");
}
