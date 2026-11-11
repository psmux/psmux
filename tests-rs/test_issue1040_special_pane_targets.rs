// Issue #1040: `send-keys -t - 'echo a' Enter` after `split-window -d`.
//
// In tmux a pane target's special tokens are read against the current window
// first (cmd-find.c cmd_find_get_pane_with_window, :599 to :640, with the
// cmd_find_pane_table at :59):
//   -  / {previous}   the pane before the active one by number, wrapping
//   +  / {next}       the pane after it
//   -N / +N           N panes back / on
//   !  / {last}       the window's last pane
//   {up-of} ...       the active pane's neighbour, wrapping
// and only when that names nothing is the token tried as a window.
//
// psmux read a bare `-` as the previous WINDOW (so in a one window session it
// was the active pane, and send-keys typed into the pane the user was in),
// the dotted forms `.-` / `:.+` as no pane at all, and on the CLI a bare
// `select-pane -t -` or `0.{previous}` as a session name ("no server running
// on session '-'"). Registered from src/commands.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

fn make_pane(id: usize) -> crate::types::Pane {
    let (master, _sink) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer: Box::new(std::io::sink()),
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(vt100::Parser::new(10, 40, 100))),
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
        win32_input_latched: false,
        input_off: false,
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

fn make_window(id: usize, root: Node) -> crate::types::Window {
    crate::types::Window {
        root,
        active_path: vec![0],
        name: format!("w{id}"),
        id,
        area: Rect::new(0, 0, 120, 30),
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

/// Window 0 (@10): %1 top left, %3 top right, %2 across the bottom, the
/// layout `split-window -d` then `split-window -h -d -t 0.0` makes, so the
/// pane numbers are 0 = %1, 1 = %3, 2 = %2. Window 1 (@11): %4 over %5.
/// Active: window 0, pane %1.
fn fixture() -> AppState {
    let top = Node::Split {
        kind: LayoutKind::Horizontal,
        sizes: vec![50, 50],
        children: vec![Node::Leaf(make_pane(1)), Node::Leaf(make_pane(3))],
    };
    let w0 = make_window(10, Node::Split {
        kind: LayoutKind::Vertical,
        sizes: vec![50, 50],
        children: vec![top, Node::Leaf(make_pane(2))],
    });
    let mut w0 = w0;
    w0.active_path = vec![0, 0];
    let w1 = make_window(11, Node::Split {
        kind: LayoutKind::Vertical,
        sizes: vec![50, 50],
        children: vec![Node::Leaf(make_pane(4)), Node::Leaf(make_pane(5))],
    });
    let mut app = AppState::new("s1040".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(w0);
    app.windows.push(w1);
    app.active_idx = 0;
    app.last_window_area = Rect::new(0, 0, 120, 30);
    app
}

fn pane(app: &AppState, t: &str) -> Result<Option<String>, String> {
    resolve_in_server_target(app, InServerTarget::Pane, t)
}

fn id(s: &str) -> Result<Option<String>, String> {
    Ok(Some(s.to_string()))
}

#[test]
fn special_pane_token_matches_tmux_pane_table() {
    for t in ["!", "+", "-", "+2", "-10", "{last}", "{next}", "{previous}",
              "{up-of}", "{down-of}", "{left-of}", "{right-of}",
              "{top}", "{bottom}", "{left}", "{right}",
              "{top-left}", "{top-right}", "{bottom-left}", "{bottom-right}"] {
        assert!(crate::cli::special_pane_token(t), "{t} is a pane token");
    }
    for t in ["", "0", "1", "%1", "@1", "-x", "+1a", "{marked}", "{mouse}", "{start}", "{end}", "work", "~", "="] {
        assert!(!crate::cli::special_pane_token(t), "{t} is not a pane token");
    }
}

#[test]
fn split_special_pane_target_follows_cmd_find_separators() {
    use crate::cli::split_special_pane_target as split;
    assert_eq!(split(".-"), Some(("", "-")));
    assert_eq!(split(":.+"), Some((":", "+")));
    assert_eq!(split("s1040:1.!"), Some(("s1040:1", "!")));
    assert_eq!(split("0.{previous}"), Some(("0", "{previous}")));
    assert_eq!(split("=s1040:0.{up-of}"), Some(("s1040:0", "{up-of}")));
    // Not a special pane half: keep the old reading.
    assert_eq!(split("-"), None);
    assert_eq!(split("0.1"), None);
    assert_eq!(split("my.session"), None);
    assert_eq!(split("%3"), None);
}

#[test]
fn bare_minus_is_the_previous_pane_not_the_previous_window() {
    let app = fixture();
    // The issue's repro: from pane 0, `-` wraps to the last pane by number.
    // master answered %4, window 1's active pane (the "previous window").
    assert_eq!(pane(&app, "-"), id("%2"));
    assert_eq!(pane(&app, "{previous}"), id("%2"));
    assert_eq!(pane(&app, "+"), id("%3"));
    assert_eq!(pane(&app, "{next}"), id("%3"));
    assert_eq!(pane(&app, "+2"), id("%2"));
    assert_eq!(pane(&app, "-2"), id("%3"));
    assert_eq!(pane(&app, "+3"), id("%1"));
}

#[test]
fn issue_repro_two_panes_after_split_window_d() {
    // `split-window -d` in a one window session: pane 0 active, pane 1 new.
    let mut app = fixture();
    app.windows.truncate(1);
    app.windows[0].root = Node::Split {
        kind: LayoutKind::Vertical,
        sizes: vec![50, 50],
        children: vec![Node::Leaf(make_pane(1)), Node::Leaf(make_pane(2))],
    };
    app.windows[0].active_path = vec![0];
    assert_eq!(pane(&app, "-"), id("%2"));
    assert_eq!(pane(&app, "+"), id("%2"));
}

#[test]
fn directional_tokens_name_the_active_panes_neighbour() {
    let app = fixture();
    assert_eq!(pane(&app, "{right-of}"), id("%3"));
    assert_eq!(pane(&app, "{down-of}"), id("%2"));
    // tmux's window_pane_find_up wraps round the top edge.
    assert_eq!(pane(&app, "{up-of}"), id("%2"));
    assert_eq!(pane(&app, "{bottom}"), id("%2"));
    assert_eq!(pane(&app, "{top-right}"), id("%3"));
}

#[test]
fn last_token_is_the_windows_last_pane() {
    let mut app = fixture();
    app.last_pane_path = vec![1];
    assert_eq!(pane(&app, "!"), id("%2"));
    assert_eq!(pane(&app, "{last}"), id("%2"));
    assert_eq!(pane(&app, ":.!"), id("%2"));
}

#[test]
fn dotted_forms_resolve_in_the_named_window() {
    let app = fixture();
    assert_eq!(pane(&app, ".-"), id("%2"));
    assert_eq!(pane(&app, ":.+"), id("%3"));
    assert_eq!(pane(&app, "s1040:0.+"), id("%3"));
    // Window 1: %4 active, so both offsets land on %5.
    assert_eq!(pane(&app, ":1.-"), id("%5"));
    assert_eq!(pane(&app, "1.{next}"), id("%5"));
    assert_eq!(pane(&app, "s1040:1.{previous}"), id("%5"));
    // Another session's target is left for that session to resolve.
    assert_eq!(pane(&app, "other:0.-"), Ok(None));
}

#[test]
fn window_half_tokens_name_a_window_not_a_window_called_plus() {
    let app = fixture();
    // From window 0: `+` is window 1 (its active pane %4), `-` wraps to 1 too.
    assert_eq!(pane(&app, "s1040:+"), id("%4"));
    assert_eq!(pane(&app, ":-"), id("%4"));
    assert_eq!(pane(&app, ":{next}"), id("%4"));
    assert_eq!(pane(&app, ":$"), id("%4"));
    assert_eq!(pane(&app, ":^"), id("%1"));
    assert_eq!(pane(&app, "s1040:+.1"), id("%5"));
    assert_eq!(pane(&app, ":+.-"), id("%5"));
    assert!(crate::cli::has_special_window_part("s:+"));
    assert!(crate::cli::has_special_window_part(":{end}.0"));
    assert!(!crate::cli::has_special_window_part("s:work"));
    assert!(!crate::cli::has_special_window_part("+"));
}

#[test]
fn the_cli_and_server_routes_resolve_these_targets_live() {
    assert!(target_resolved_in_server(InServerTarget::Pane, "-"));
    assert!(target_resolved_in_server(InServerTarget::Pane, ".-"));
    assert!(target_resolved_in_server(InServerTarget::Pane, ":.+"));
    assert!(target_resolved_in_server(InServerTarget::Pane, "s:1.!"));
    assert!(!target_resolved_in_server(InServerTarget::Pane, "%3"));
    assert!(!target_resolved_in_server(InServerTarget::Pane, "s:1.2"));
    // A window command keeps its own reading of a dotted target.
    assert!(!target_resolved_in_server(InServerTarget::Window, ":.+"));
    // select-pane's -t is CMD_FIND_PANE, like send-keys'.
    assert_eq!(in_server_target_kind("select-pane"), Some(InServerTarget::Pane));
    assert_eq!(in_server_target_kind("selectp"), Some(InServerTarget::Pane));
}
