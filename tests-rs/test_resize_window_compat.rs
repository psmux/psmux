// Regression coverage for tmux-compatible window sizing.
//
// tmex sends `resize-window -t @id -x cols -y rows` to its Windows psmux
// backend. psmux used to accept that command while leaving the window and
// ConPTY unchanged. These tests exercise the production parser and state
// transitions behind both normal and control-mode command dispatch without
// starting a psmux server (see AGENTS.md).

use super::*;
use crate::types::{ControlClient, LayoutKind, Node, Window};

fn empty_window(id: usize, name: &str, width: u16, height: u16) -> Window {
    Window {
        root: Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: Vec::new(),
            children: Vec::new(),
        },
        active_path: Vec::new(),
        name: name.to_string(),
        id,
        area: Rect::new(0, 0, width, height),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: Vec::new(),
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

fn app_with_two_windows() -> AppState {
    let mut app = AppState::new("test".to_string());
    app.windows.push(empty_window(3, "first", 80, 24));
    app.windows.push(empty_window(7, "second", 80, 24));
    app.window_indices = vec![0, 1];
    app.client_area = Rect::new(0, 0, 80, 24);
    app.last_window_area = app.client_area;
    app
}

fn control_client(
    client_id: u64,
    size: Option<(u16, u16)>,
    window_sizes: &[(usize, (u16, u16))],
) -> ControlClient {
    let (notification_tx, _notification_rx) = std::sync::mpsc::channel();
    ControlClient {
        client_id,
        cmd_counter: 0,
        echo_enabled: false,
        notification_tx,
        paused_panes: std::collections::HashSet::new(),
        subscriptions: std::collections::HashMap::new(),
        subscription_values: std::collections::HashMap::new(),
        subscription_last_check: std::collections::HashMap::new(),
        output_gate: Default::default(),
        size,
        window_sizes: window_sizes.iter().copied().collect(),
    }
}

#[test]
fn parses_combined_absolute_dimensions_and_target() {
    let request = parse_resize_window(&["-x120", "-y", "40"], Some("@7")).unwrap();
    assert_eq!(request.target, WindowTarget::Id(7));
    assert_eq!(request.width, Some(120));
    assert_eq!(request.height, Some(40));

    let attached_target = parse_resize_window(&["-t@7", "-x=130"], None).unwrap();
    assert_eq!(attached_target.target, WindowTarget::Id(7));
    assert_eq!(attached_target.width, Some(130));
}

#[test]
fn parses_session_qualified_window_id() {
    let request = parse_resize_window(&["-t", "work:@7", "-x", "120", "-y", "40"], None).unwrap();

    assert_eq!(request.target, WindowTarget::Id(7));
    assert_eq!(request.width, Some(120));
    assert_eq!(request.height, Some(40));
}

#[test]
fn parses_tmux_adjustment_flags_with_priority() {
    let request = parse_resize_window(&["-DR", "5"], None).unwrap();
    assert_eq!(request.direction, Some(ResizeDirection::Right));
    assert_eq!(request.adjustment, 5);
}

#[test]
fn parses_largest_before_smallest_like_tmux() {
    let request = parse_resize_window(&["-aA"], None).unwrap();
    assert_eq!(request.client_size, Some(ClientSizeChoice::Largest));
}

#[test]
fn rejects_dimensions_unsafe_for_conpty() {
    assert_eq!(
        parse_resize_window(&["-x", "1"], None),
        Err("width too small".to_string())
    );
    assert_eq!(
        parse_resize_window(&["-y10001"], None),
        Err("height too large".to_string())
    );
    assert_eq!(
        parse_resize_window(&["-R", "2147483648"], None),
        Err("adjustment too large".to_string())
    );
}

#[test]
fn parses_control_default_and_per_window_sizes() {
    assert_eq!(
        parse_control_client_size("80,24").unwrap(),
        (None, Some((80, 24)))
    );
    assert_eq!(
        parse_control_client_size("@3:120x40").unwrap(),
        (Some(3), Some((120, 40)))
    );
    assert_eq!(parse_control_client_size("@3:").unwrap(), (Some(3), None));
    assert!(parse_control_client_size("@3").is_err());
    assert!(parse_control_client_size("80").is_err());
    assert!(parse_control_client_size("@3:1x24").is_err());
}

#[test]
fn tmex_resize_command_updates_only_the_target_window() {
    let mut app = app_with_two_windows();
    let request = parse_resize_window(&["-t", "@7", "-x", "120", "-y", "40"], None).unwrap();
    let result = apply_resize_window(&mut app, &request).unwrap();

    assert_eq!(result.window_index, 1);
    assert_eq!(app.windows[1].area, Rect::new(0, 0, 120, 40));
    assert_eq!(app.windows[1].window_size.as_deref(), Some("manual"));
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 80, 24));
    assert_eq!(app.client_area, Rect::new(0, 0, 80, 24));
    assert_eq!(app.last_window_area, Rect::new(0, 0, 80, 24));
}

#[test]
fn relative_resize_uses_tmux_direction_priority() {
    let mut app = app_with_two_windows();
    let request = parse_resize_window(&["-DR", "5"], Some("@3")).unwrap();

    apply_resize_window(&mut app, &request).unwrap();

    assert_eq!(app.windows[0].area, Rect::new(0, 0, 85, 24));
    assert_eq!(app.manual_window_sizes.get(&3), Some(&(85, 24)));
}

#[test]
fn client_resize_preserves_manual_window() {
    let mut app = app_with_two_windows();
    let request = parse_resize_window(&["-x", "120", "-y", "40"], Some("@7")).unwrap();
    apply_resize_window(&mut app, &request).unwrap();

    app.client_sizes.insert(11, (100, 32));
    app.latest_client_id = Some(11);
    refresh_dynamic_window_sizes(&mut app);

    assert_eq!(app.windows[0].area, Rect::new(0, 0, 100, 32));
    assert_eq!(app.windows[1].area, Rect::new(0, 0, 120, 40));
    assert_eq!(app.last_window_area, Rect::new(0, 0, 100, 32));
}

#[test]
fn largest_and_smallest_use_all_client_dimensions() {
    let mut app = app_with_two_windows();
    app.client_sizes.insert(1, (90, 50));
    app.client_sizes.insert(2, (120, 30));

    let largest = parse_resize_window(&["-A"], Some("@3")).unwrap();
    apply_resize_window(&mut app, &largest).unwrap();
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 120, 50));

    let smallest = parse_resize_window(&["-a"], Some("@7")).unwrap();
    apply_resize_window(&mut app, &smallest).unwrap();
    assert_eq!(app.windows[1].area, Rect::new(0, 0, 90, 30));
}

#[test]
fn largest_without_clients_uses_the_default_geometry() {
    let mut app = app_with_two_windows();
    let largest = parse_resize_window(&["-A"], Some("@3")).unwrap();

    apply_resize_window(&mut app, &largest).unwrap();

    assert_eq!(app.windows[0].area, Rect::new(0, 0, 80, 24));
    assert_eq!(app.windows[0].window_size.as_deref(), Some("manual"));
}

#[test]
fn per_window_client_size_clamps_manual_geometry_until_cleared() {
    let mut app = app_with_two_windows();
    app.control_clients
        .insert(11, control_client(11, Some((200, 100)), &[(3, (80, 24))]));
    let request = parse_resize_window(&["-x", "120", "-y", "40"], Some("@3")).unwrap();

    apply_resize_window(&mut app, &request).unwrap();
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 80, 24));
    assert_eq!(app.manual_window_sizes.get(&3), Some(&(120, 40)));

    app.control_clients
        .get_mut(&11)
        .unwrap()
        .window_sizes
        .remove(&3);
    refresh_dynamic_window_sizes(&mut app);
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 120, 40));
}

#[test]
fn explicit_window_size_caps_largest_across_other_clients() {
    let mut app = app_with_two_windows();
    app.control_clients
        .insert(11, control_client(11, Some((200, 100)), &[(3, (80, 24))]));
    app.control_clients
        .insert(12, control_client(12, Some((160, 80)), &[]));
    let largest = parse_resize_window(&["-A"], Some("@3")).unwrap();

    apply_resize_window(&mut app, &largest).unwrap();

    assert_eq!(app.windows[0].area, Rect::new(0, 0, 80, 24));
    assert_eq!(app.manual_window_sizes.get(&3), Some(&(160, 80)));
}

#[test]
fn window_size_option_can_leave_manual_mode() {
    let mut app = app_with_two_windows();
    app.client_sizes.insert(1, (100, 32));
    app.latest_client_id = Some(1);
    let request = parse_resize_window(&["-x", "120", "-y", "40"], Some("@3")).unwrap();
    apply_resize_window(&mut app, &request).unwrap();

    set_active_window_size_mode(&mut app, Some("latest".to_string())).unwrap();

    assert_eq!(app.windows[0].window_size.as_deref(), Some("latest"));
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 100, 32));
}

#[test]
fn leaving_manual_mode_without_clients_preserves_geometry() {
    let mut app = app_with_two_windows();
    let request = parse_resize_window(&["-x", "112", "-y", "36"], Some("@3")).unwrap();
    apply_resize_window(&mut app, &request).unwrap();

    set_active_window_size_mode(&mut app, Some("latest".to_string())).unwrap();

    assert_eq!(app.windows[0].window_size.as_deref(), Some("latest"));
    assert_eq!(app.windows[0].area, Rect::new(0, 0, 112, 36));
}


// ── `window-size latest` follows the client the user is active in ──
//
// Regression: a phone client (SSH/Terminus) narrowed the window when it
// attached and the desktop never got it back. Typing and clicking did not
// refresh the size choice at all, and the stale `latest_size_client_id`
// (written only by `client-size`, i.e. a real resize) kept winning over the
// client the user had gone back to. tmux keeps the most recently active
// client at the head of `clients->latest`; `note_client_activity` is the
// psmux equivalent of that.

fn activity_app(window_size: &str) -> AppState {
    let mut app = AppState::new("test".to_string());
    app.windows.push(empty_window(3, "only", 120, 40));
    app.window_indices = vec![0];
    app.window_size = window_size.to_string();
    // client 1 = desktop, client 2 = phone; the phone resized last
    app.client_sizes.insert(1, (200, 50));
    app.client_sizes.insert(2, (80, 24));
    app.latest_client_id = Some(2);
    app.latest_size_client_id = Some(2);
    app.client_area = Rect::new(0, 0, 80, 24);
    app.last_window_area = app.client_area;
    assert!(refresh_dynamic_window_sizes(&mut app), "the first client size must reach the window");
    app
}

#[test]
fn latest_window_size_follows_the_active_client() {
    let mut app = activity_app("latest");
    assert!(note_client_activity(&mut app, 1), "activity from the desktop must resize");
    assert_eq!((app.windows[0].area.width, app.windows[0].area.height), (200, 50));
    assert_eq!(app.client_area.width, 200);
    assert_eq!(app.latest_client_id, Some(1));
    assert_eq!(app.latest_size_client_id, Some(1));
}

#[test]
fn repeated_activity_from_the_same_client_is_a_no_op() {
    let mut app = activity_app("latest");
    assert!(note_client_activity(&mut app, 1));
    assert!(!note_client_activity(&mut app, 1), "same client must not resize again");
}

#[test]
fn activity_from_a_client_without_a_size_is_ignored() {
    let mut app = activity_app("latest");
    // One-shot CLI clients (`psmux ls`) and CONTROL clients send requests
    // constantly and never report a size: they must not drive the geometry.
    assert!(!note_client_activity(&mut app, 99));
    assert_eq!(app.latest_size_client_id, Some(2));
    assert_eq!(app.windows[0].area.width, 80);
}

#[test]
fn largest_and_smallest_ignore_which_client_is_active() {
    let mut app = activity_app("largest");
    assert_eq!(app.windows[0].area.width, 200);
    assert!(!note_client_activity(&mut app, 2), "largest keeps the largest size");

    let mut app = activity_app("smallest");
    assert_eq!(app.windows[0].area.width, 80);
    assert!(!note_client_activity(&mut app, 1), "smallest keeps the smallest size");
}

#[test]
fn phone_going_away_without_a_resize_still_restores_the_desktop() {
    // The phone detaches (size entry removed) and the desktop types a key:
    // the window must come back to the desktop size.
    let mut app = activity_app("latest");
    app.client_sizes.remove(&2);
    app.windows[0].area = Rect::new(0, 0, 80, 24);
    assert!(note_client_activity(&mut app, 1));
    assert_eq!((app.windows[0].area.width, app.windows[0].area.height), (200, 50));
}

// ── `window-size latest` must not follow a client's idle poll ──
//
// Regression: the server pinged `ClientActivity` for every command line a
// persistent client sent, `dump-state` included. Two clients of different
// sizes poll once a second each, so every poll moved "latest" to the *other*
// client, `refresh_dynamic_window_sizes` resized every pane on the flip, and
// the pane's full-screen program repainted its whole screen. The user saw it
// as the pane flickering whenever the program was idle and a second
// (SSH/Termius) client was attached; with only one client attached its own
// poll left "latest" where it already was and nothing resized, which is why
// disconnecting the phone made it stop. `is_client_poll_cmd` is what the two
// activity pings in `server::connection` consult, next to the #604 exclusion
// for bare pointer motion.

#[test]
fn the_frame_poll_is_not_user_activity() {
    use crate::client::is_client_poll_cmd;
    assert!(is_client_poll_cmd("dump-state"));
    assert!(is_client_poll_cmd("dump-state\n"));
    assert!(is_client_poll_cmd("  dump-state  "));
}

#[test]
fn real_user_requests_still_count_as_activity() {
    use crate::client::is_client_poll_cmd;
    for cmd in [
        "send-key a\n",
        "send-text 35\n",
        "pane-mouse 4 0 12 3 M\n",  // click
        "pane-mouse 4 32 12 3 M\n", // drag
        "pane-mouse 4 35 12 3 M\n", // hover: excluded by #604, not here
        "client-size 200 50\n",     // a real resize
        "copy-mode\n",
        "refresh-client\n",
    ] {
        assert!(!is_client_poll_cmd(cmd), "{cmd:?} is user intent");
    }
}

// ── losing focus is the one key tmux refuses to count ──
//
// tmux updates the latest client at the `out:` label of its key callback under
// `key != KEYC_FOCUS_OUT` (server-client.c:1653): focus-IN is activity,
// focus-OUT is filtered out by name. psmux's client reports both as ordinary
// command lines, so `focus-out` pinged ClientActivity and the terminal the
// user had just alt-tabbed AWAY from took the window. Measured with a 120x40
// and a 60x20 client under `window-size latest`: typing in the 120x40 client
// sized the window 120x40, then `focus-out` from the idle 60x20 client pulled
// it to 60x20 and resized every pane. Same visible cost as the idle poll,
// different trigger.

#[test]
fn losing_focus_is_not_user_activity() {
    use crate::client::is_focus_loss_cmd;
    assert!(is_focus_loss_cmd("focus-out"));
    assert!(is_focus_loss_cmd("focus-out\n"));
    assert!(is_focus_loss_cmd("  focus-out  "));
}

#[test]
fn gaining_focus_and_real_requests_still_count_as_activity() {
    use crate::client::is_focus_loss_cmd;
    for cmd in [
        "focus-in\n", // tmux DOES make this client the latest
        "send-key a\n",
        "pane-mouse 4 0 12 3 M\n",
        "client-size 200 50\n",
        "dump-state\n", // a poll, excluded by is_client_poll_cmd instead
        "refresh-client\n",
    ] {
        assert!(!is_focus_loss_cmd(cmd), "{cmd:?} must not be read as focus loss");
    }
}

/// The two exclusions together are exactly what the activity ping in
/// `server::connection` asks, so assert the composed predicate rather than
/// leaving the call sites untested.
#[test]
fn the_activity_ping_excludes_polls_and_focus_loss_only() {
    use crate::client::{is_bare_motion_cmd, is_client_poll_cmd, is_focus_loss_cmd};
    let pings = |line: &str| {
        !is_bare_motion_cmd(line) && !is_client_poll_cmd(line) && !is_focus_loss_cmd(line)
    };
    for silent in ["dump-state\n", "focus-out\n", "pane-mouse 4 35 12 3 M\n"] {
        assert!(!pings(silent), "{silent:?} must not move `window-size latest`");
    }
    for intent in [
        "send-key a\n",
        "send-text 35\n",
        "focus-in\n",
        "client-size 200 50\n",
        "pane-mouse 4 0 12 3 M\n",
        "copy-mode\n",
        "new-window\n",
    ] {
        assert!(pings(intent), "{intent:?} must move `window-size latest`");
    }
}
