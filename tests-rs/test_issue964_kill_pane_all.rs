// Issue #964: kill-pane -a killed the target instead of every other pane.
// tmux 3.8 cmd_kill_pane_all (cmd-kill-pane.c:72-93) unzooms the target's
// window, removes every pane of it but the target and redraws; a one-pane
// window is left alone, and with no -t the target is the current pane.
// psmux also counts floats as panes of the window (#767). These drive the real
// kill_panes_except and the in server route over PTY-less stub panes.
// Registered from src/commands.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn make_pane(id: usize) -> crate::types::Pane {
    let (rows, cols) = (30u16, 40u16);
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
        dead_info: None,
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

fn make_float(id: usize) -> crate::types::FloatingPane {
    crate::types::FloatingPane {
        pane: make_pane(id),
        x: 2,
        y: 2,
        w: 20,
        h: 10,
        border: String::new(),
        id,
        title: String::new(),
        position: None,
        styles: crate::types::FloatStyles { style: String::new(), active_border: String::new(), border: String::new() },
    }
}

/// A window whose tiled panes `ids` sit in one horizontal row, the first active.
fn make_window(id: usize, ids: &[usize]) -> crate::types::Window {
    let root = if ids.len() == 1 {
        Node::Leaf(make_pane(ids[0]))
    } else {
        Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![(100 / ids.len()) as u16; ids.len()],
            children: ids.iter().map(|&p| Node::Leaf(make_pane(p))).collect(),
        }
    };
    crate::types::Window {
        root,
        active_path: if ids.len() == 1 { vec![] } else { vec![0] },
        name: format!("w{id}"),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
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

/// Window 0 holds %1 %2 %3 (%2 active), window 1 holds %4 %5 (%4 active),
/// window 0 current.
fn two_windows() -> AppState {
    let mut app = AppState::new("A".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows = vec![make_window(10, &[1, 2, 3]), make_window(11, &[4, 5])];
    app.windows[0].active_path = vec![1];
    app.active_idx = 0;
    app
}

fn ids(app: &AppState, wi: usize) -> Vec<usize> {
    crate::tree::collect_pane_ids(&app.windows[wi].root)
}

fn active(app: &AppState, wi: usize) -> Option<usize> {
    crate::tree::get_active_pane_id(&app.windows[wi].root, &app.windows[wi].active_path)
}

#[test]
fn in_server_kill_pane_a_keeps_the_active_pane() {
    let mut app = two_windows();
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert_eq!(ids(&app, 0), vec![2], "only the active pane %2 is left");
    assert_eq!(active(&app, 0), Some(2));
    assert_eq!(ids(&app, 1), vec![4, 5], "another window is not touched");
    assert_eq!(app.active_idx, 0);
}

#[test]
fn in_server_killp_a_keeps_the_active_pane() {
    let mut app = two_windows();
    execute_command_string(&mut app, "killp -a").unwrap();
    assert_eq!(ids(&app, 0), vec![2]);
}

#[test]
fn plain_kill_pane_still_kills_the_active_pane() {
    let mut app = two_windows();
    execute_command_string(&mut app, "kill-pane").unwrap();
    assert_eq!(ids(&app, 0), vec![1, 3], "without -a the target goes");
}

#[test]
fn keep_by_id_keeps_that_pane() {
    // kill-pane -a -t %3: the server resolves %3 and keeps it, whatever is active.
    let mut app = two_windows();
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 3), 2);
    assert_eq!(ids(&app, 0), vec![3]);
    assert_eq!(active(&app, 0), Some(3), "the kept pane is the window's active pane");
}

#[test]
fn keep_in_another_window_leaves_the_current_window_alone() {
    // kill-pane -a -t :1.1 (window.pane form): the target window is not current.
    let mut app = two_windows();
    assert_eq!(crate::pane::kill_panes_except(&mut app, 1, 5), 1);
    assert_eq!(ids(&app, 1), vec![5]);
    assert_eq!(ids(&app, 0), vec![1, 2, 3], "the current window keeps its panes");
    assert_eq!(app.active_idx, 0, "the current window does not change");
    assert_eq!(active(&app, 0), Some(2));
}

#[test]
fn one_pane_window_is_left_alone() {
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window(10, &[7])];
    app.active_idx = 0;
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 7), 0);
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert_eq!(ids(&app, 0), vec![7], "the last pane is not killed");
}

#[test]
fn unknown_pane_kills_nothing() {
    let mut app = two_windows();
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 99), 0);
    assert_eq!(ids(&app, 0), vec![1, 2, 3]);
}

#[test]
fn zoomed_window_is_unzoomed() {
    let mut app = two_windows();
    crate::window_ops::toggle_zoom(&mut app);
    assert!(app.windows[0].zoom_saved.is_some());
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 2), 2);
    assert!(app.windows[0].zoom_saved.is_none(), "server_unzoom_window runs first");
    assert_eq!(ids(&app, 0), vec![2]);
}

#[test]
fn tiled_target_closes_the_window_floats() {
    // Decision: floats are panes of the window (#767), so they go too.
    let mut app = two_windows();
    app.windows[0].floating = vec![make_float(8), make_float(9)];
    app.windows[0].floating_focus = Some(1);
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 2), 4);
    assert!(app.windows[0].floating.is_empty());
    assert_eq!(app.windows[0].floating_focus, None);
    assert_eq!(ids(&app, 0), vec![2]);
}

#[test]
fn floating_target_closes_the_other_floats_only() {
    // Decision: the tiled tree cannot be emptied, so a kept float leaves the
    // tiled panes alone and closes the other floats.
    let mut app = two_windows();
    app.windows[0].floating = vec![make_float(8), make_float(9)];
    app.windows[0].floating_focus = Some(1);
    assert_eq!(crate::pane::kill_panes_except(&mut app, 0, 9), 1);
    let floats: Vec<usize> = app.windows[0].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![9]);
    assert_eq!(app.windows[0].floating_focus, Some(0), "the focused kept float keeps focus");
    assert_eq!(ids(&app, 0), vec![1, 2, 3]);
}

#[test]
fn unfocused_floating_target_does_not_take_focus() {
    let mut app = two_windows();
    app.windows[0].floating = vec![make_float(8), make_float(9)];
    app.windows[0].floating_focus = Some(1);
    crate::pane::kill_panes_except(&mut app, 0, 8);
    assert_eq!(app.windows[0].floating.len(), 1);
    assert_eq!(app.windows[0].floating_focus, None);
}

#[test]
fn clustered_at_is_split() {
    assert_eq!(
        crate::cli::split_command_flag_clusters("kill-pane", &["-at", "%4"]),
        Some(vec!["-a".to_string(), "-t".to_string(), "%4".to_string()])
    );
    assert_eq!(
        crate::cli::split_command_flag_clusters("killp", &["-at", "%4"]),
        Some(vec!["-a".to_string(), "-t".to_string(), "%4".to_string()])
    );
}

#[test]
fn a_is_read_by_the_getopt_walk() {
    assert!(own_flag_set("kill-pane", &["-a"], 'a'));
    assert!(own_flag_set("kill-pane", &["-at", "%4"], 'a'));
    assert!(own_flag_set("kill-pane", &["-t", "%4", "-a"], 'a'));
    assert!(!own_flag_set("kill-pane", &["-t", "%4"], 'a'));
    assert!(!own_flag_set("kill-pane", &["-t", "-a"], 'a'), "-a as the value of -t is not the flag");
}

/// A listener standing in for the server's control port: a line forwarded to
/// the server shows up as a connection.
fn control_port(app: &mut AppState) -> std::net::TcpListener {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    app.control_port = Some(listener.local_addr().unwrap().port());
    listener
}

#[test]
fn in_server_route_runs_here_against_the_current_focus() {
    // A hook's line runs while the hook's target is the current window. Sent
    // to the server, it ran after that focus was put back, in another window.
    let mut app = two_windows();
    app.active_idx = 1;
    let listener = control_port(&mut app);
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert!(listener.accept().is_err(), "the line is not forwarded to the server");
    assert_eq!(ids(&app, 1), vec![4], "window 1's active pane %4 is kept");
    assert_eq!(ids(&app, 0), vec![1, 2, 3], "window 0 is not touched");
}

#[test]
fn in_server_route_fires_its_hooks_once() {
    // A binding or the command prompt runs kill-pane -a here, and tmux fires
    // its before- and after-kill-pane hooks. The hook's own kill-pane -a fires
    // none (CMDQ_STATE_NOHOOKS), so an after-kill-pane hook running it ends.
    let mut app = two_windows();
    let listener = control_port(&mut app);
    app.hooks.insert("after-kill-pane".to_string(), vec!["kill-pane -a -t %5".to_string()]);
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert!(listener.accept().is_err(), "nothing is queued on the server");
    assert_eq!(ids(&app, 0), vec![2]);
    assert_eq!(ids(&app, 1), vec![5], "after-kill-pane ran");
    assert_eq!(app.hook_depth, 0);
}

#[test]
fn in_server_route_fires_before_kill_pane() {
    let mut app = two_windows();
    app.hooks.insert("before-kill-pane".to_string(), vec!["kill-pane -a -t %5".to_string()]);
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert_eq!(ids(&app, 1), vec![5], "before-kill-pane ran");
    assert_eq!(ids(&app, 0), vec![2]);
}

#[test]
fn inside_a_hook_no_hook_fires() {
    let mut app = two_windows();
    app.hooks.insert("after-kill-pane".to_string(), vec!["kill-pane -a -t %5".to_string()]);
    run_hook_commands(&mut app, &["kill-pane -a".to_string()]);
    assert_eq!(ids(&app, 0), vec![2]);
    assert_eq!(ids(&app, 1), vec![4, 5], "a hook's own command fires no after-kill-pane");
    assert_eq!(app.hook_depth, 0);
}

#[test]
fn window_target_keeps_its_focused_float() {
    // `-t :1` is window 1's current pane, its focused float, as the server's
    // temporary focus reads it; the tiled-only resolver kept %4.
    let mut app = two_windows();
    app.windows[1].floating = vec![make_float(8), make_float(9)];
    app.windows[1].floating_focus = Some(1);
    execute_command_string(&mut app, "kill-pane -a -t :1").unwrap();
    let floats: Vec<usize> = app.windows[1].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![9], "the focused float is the target");
    assert_eq!(ids(&app, 0), vec![1, 2, 3]);
}

#[test]
fn bare_window_name_keeps_its_focused_float() {
    let mut app = two_windows();
    app.windows[1].floating = vec![make_float(8)];
    app.windows[1].floating_focus = Some(0);
    execute_command_string(&mut app, "kill-pane -a -t w11").unwrap();
    let floats: Vec<usize> = app.windows[1].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![8]);
}

#[test]
fn pane_index_counts_the_floats() {
    // Window 1 is %4 %5 then float %8: index 2 is the float (#767).
    let mut app = two_windows();
    app.windows[1].floating = vec![make_float(8), make_float(9)];
    execute_command_string(&mut app, "kill-pane -a -t :1.2").unwrap();
    let floats: Vec<usize> = app.windows[1].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![8], "pane 2 of window 1 is the float %8");
    assert!(app.status_message.is_none(), "{:?}", app.status_message);
    // A bare index is a pane of the current window: 3 is its float %6.
    app.windows[0].floating = vec![make_float(6), make_float(7)];
    execute_command_string(&mut app, "kill-pane -a -t 3").unwrap();
    let floats: Vec<usize> = app.windows[0].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![6]);
    assert!(app.status_message.is_none(), "{:?}", app.status_message);
}

#[test]
fn tiled_index_target_still_names_the_tiled_pane() {
    let mut app = two_windows();
    app.windows[1].floating = vec![make_float(8)];
    app.windows[1].floating_focus = Some(0);
    execute_command_string(&mut app, "kill-pane -a -t :1.1").unwrap();
    assert_eq!(ids(&app, 1), vec![5]);
    assert!(app.windows[1].floating.is_empty());
}

#[test]
fn unzooming_a_one_pane_window_is_announced() {
    // A one-pane window can be zoomed here; kill-pane -a kills nothing but
    // still unzooms it, and window_zoomed_flag and the raw flags change.
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window(10, &[7])];
    app.active_idx = 0;
    crate::window_ops::toggle_zoom(&mut app);
    assert!(app.windows[0].zoom_saved.is_some());
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
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    assert!(app.windows[0].zoom_saved.is_none());
    let sent: Vec<String> = nrx.try_iter().map(|n| crate::control::format_notification(&n)).collect();
    assert!(sent.iter().any(|n| n.starts_with("%layout-change @10")), "{:?}", sent);
}

#[test]
fn in_server_route_reads_its_target() {
    let mut app = two_windows();
    let listener = control_port(&mut app);
    execute_command_string(&mut app, "kill-pane -a -t %5").unwrap();
    assert!(listener.accept().is_err());
    assert_eq!(ids(&app, 1), vec![5]);
    assert_eq!(ids(&app, 0), vec![1, 2, 3]);
    execute_command_string(&mut app, "kill-pane -a -t :0.2").unwrap();
    assert_eq!(ids(&app, 0), vec![3], "a window.pane target");
}

#[test]
fn in_server_route_target_naming_nothing_kills_nothing() {
    let mut app = two_windows();
    execute_command_string(&mut app, "kill-pane -a -t %99").unwrap();
    assert_eq!(ids(&app, 0), vec![1, 2, 3]);
    assert_eq!(ids(&app, 1), vec![4, 5]);
    assert!(app.status_message.as_ref().is_some_and(|(m, ..)| m.contains("can't find pane")));
}

#[test]
fn in_server_route_keeps_a_focused_float() {
    let mut app = two_windows();
    app.windows[0].floating = vec![make_float(8), make_float(9)];
    app.windows[0].floating_focus = Some(0);
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    let floats: Vec<usize> = app.windows[0].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![8], "the focused float is the target");
}

#[test]
fn command_prompt_reads_the_target() {
    let mut app = two_windows();
    app.mode = Mode::CommandPrompt { input: "kill-pane -a -t %5".to_string(), cursor: 0 };
    execute_command_prompt(&mut app).unwrap();
    assert_eq!(ids(&app, 1), vec![5], "window 1 keeps %5");
    assert_eq!(ids(&app, 0), vec![1, 2, 3], "the current window is not touched");
}

#[test]
fn command_prompt_keeps_a_focused_float() {
    let mut app = two_windows();
    app.windows[0].floating = vec![make_float(8), make_float(9)];
    app.windows[0].floating_focus = Some(1);
    app.mode = Mode::CommandPrompt { input: "kill-pane -a".to_string(), cursor: 0 };
    execute_command_prompt(&mut app).unwrap();
    let floats: Vec<usize> = app.windows[0].floating.iter().map(|f| f.id).collect();
    assert_eq!(floats, vec![9], "the focused float survives");
}

fn pane_cols(app: &AppState, wi: usize, id: usize) -> u16 {
    let root = &app.windows[wi].root;
    let path = crate::tree::find_path_by_id(root, id).unwrap();
    crate::tree::active_pane(root, &path).unwrap().last_cols
}

#[test]
fn background_window_pty_takes_its_new_size() {
    // %5 in window 1 (not current) was a 40 column PTY in a 120 column window.
    let mut app = two_windows();
    assert_eq!(pane_cols(&app, 1, 5), 40);
    crate::pane::kill_panes_except(&mut app, 1, 5);
    assert!(pane_cols(&app, 1, 5) > 100, "the kept pane fills its window");
}

#[test]
fn a_after_the_option_terminator_is_an_operand() {
    assert!(!own_flag_set("kill-pane", &["--", "-a"], 'a'));
    assert!(!own_flag_set("kill-pane", &["x", "-a"], 'a'));
}

#[test]
fn targeted_hooks_run_in_the_target_window() {
    // A binding runs kill-pane -a -t %5 while window 0 is current. tmux gives
    // the hooks the target's state, so a bare kill-pane -a in them acts on
    // window 1; it killed window 0's other panes.
    for event in ["before-kill-pane", "after-kill-pane"] {
        let mut app = two_windows();
        app.hooks.insert(event.to_string(), vec!["kill-pane -a".to_string()]);
        execute_command_string(&mut app, "kill-pane -a -t %5").unwrap();
        assert_eq!(ids(&app, 0), vec![1, 2, 3], "{event}: the current window is not touched");
        assert_eq!(ids(&app, 1), vec![5], "{event}");
        assert_eq!(app.active_idx, 0, "{event}: the focus goes back");
        assert_eq!(active(&app, 0), Some(2), "{event}");
    }
}

#[test]
fn every_removed_pane_is_a_layout_change() {
    // tmux notifies window-layout-changed from window_unzoom and from each
    // layout_close_pane: zoomed, with two panes going, that is three lines.
    let mut app = two_windows();
    crate::window_ops::toggle_zoom(&mut app);
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
    execute_command_string(&mut app, "kill-pane -a").unwrap();
    let sent: Vec<String> = nrx.try_iter().map(|n| crate::control::format_notification(&n)).collect();
    let layouts = sent.iter().filter(|n| n.starts_with("%layout-change @10")).count();
    assert_eq!(layouts, 3, "{:?}", sent);
}

#[test]
fn unzoom_alone_resizes_the_ptys() {
    // A kept float with no other float: nothing is removed, but the unzoom
    // gives the tiled panes back their sizes, so their PTYs must follow.
    let mut app = two_windows();
    app.active_idx = 1;
    crate::window_ops::toggle_zoom(&mut app);
    let area = app.windows[1].area;
    crate::tree::resize_window_panes(&mut app, 1, area);
    assert!(pane_cols(&app, 1, 4) > 100, "zoomed %4 fills its window");
    app.windows[1].floating = vec![make_float(9)];
    app.windows[1].floating_focus = Some(0);
    app.active_idx = 0;
    assert_eq!(crate::pane::kill_panes_except(&mut app, 1, 9), 0);
    assert!(app.windows[1].zoom_saved.is_none());
    assert!(pane_cols(&app, 1, 4) < 100, "%4 is back to its share of the window");
}
