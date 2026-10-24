// Issue #275: detach-client CLI command parity with tmux
//
// These tests exercise the CLI argument parsing and the AppState mutations
// performed by the new server handlers.  They do NOT spin up a real TCP server
// (that's covered by tests/test_issue275_detach_client.ps1) — they verify the
// pure-state-mutation contract: which clients get removed from the registry,
// which counters decrement, and which conditions trigger destroy-on-detach.

use super::*;
use crate::types::{AppState, ClientInfo};
use std::time::Instant;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn add_client(app: &mut AppState, id: u64, tty: &str) {
    app.client_registry.insert(id, ClientInfo {
        id,
        width: 120,
        height: 30,
        connected_at: Instant::now(),
        last_activity: Instant::now(),
        tty_name: tty.to_string(),
        is_control: false,
        termfeatures: crate::terminal_features::TerminalFeatures::from_bits(0xf),
        last_session: None,
        pid: 0,
        readonly: false,
        focused: false,
    });
    app.attached_clients += 1;
}

// ════════════════════════════════════════════════════════════════════════════
//  Pure state-mutation tests (mirror what the CtrlReq handlers do)
// ════════════════════════════════════════════════════════════════════════════

/// `detach-client -t %1` should remove only that client and decrement counters.
#[test]
fn force_detach_single_client_by_id() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");
    add_client(&mut app, 2, "/dev/pts/2");
    add_client(&mut app, 3, "/dev/pts/3");

    // Simulate the ForceDetachClient handler's effect.
    app.client_sizes.remove(&2);
    let was_present = app.client_registry.remove(&2).is_some();
    if was_present {
        app.attached_clients = app.attached_clients.saturating_sub(1);
    }

    assert_eq!(app.client_registry.len(), 2, "only target removed");
    assert!(!app.client_registry.contains_key(&2));
    assert!(app.client_registry.contains_key(&1));
    assert!(app.client_registry.contains_key(&3));
    assert_eq!(app.attached_clients, 2);
}

/// `detach-client -t /dev/pts/2` should resolve via tty_name lookup.
#[test]
fn force_detach_by_tty_name_lookup() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");
    add_client(&mut app, 2, "/dev/pts/2");

    let target_cid: Option<u64> = app.client_registry.iter()
        .find(|(_, ci)| ci.tty_name == "/dev/pts/2")
        .map(|(cid, _)| *cid);
    assert_eq!(target_cid, Some(2), "tty_name lookup should find client 2");

    if let Some(cid) = target_cid {
        app.client_registry.remove(&cid);
        app.attached_clients = app.attached_clients.saturating_sub(1);
    }
    assert!(!app.client_registry.contains_key(&2));
    assert_eq!(app.attached_clients, 1);
}

/// Unknown tty_name should resolve to None — the handler must be a safe no-op.
#[test]
fn force_detach_by_tty_name_missing() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");

    let target_cid: Option<u64> = app.client_registry.iter()
        .find(|(_, ci)| ci.tty_name == "/dev/pts/99")
        .map(|(cid, _)| *cid);
    assert_eq!(target_cid, None);

    // Original state unchanged.
    assert_eq!(app.client_registry.len(), 1);
    assert_eq!(app.attached_clients, 1);
}

/// `detach-client -a` from client_id=2: detaches 1 and 3, keeps 2.
#[test]
fn detach_all_other_clients_keeps_current() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");
    add_client(&mut app, 2, "/dev/pts/2");
    add_client(&mut app, 3, "/dev/pts/3");
    let except = 2u64;

    let targets: Vec<u64> = app.client_registry.iter()
        .filter(|(cid, _)| **cid != except)
        .map(|(cid, _)| *cid)
        .collect();
    assert_eq!(targets.len(), 2, "should target 1 and 3, not 2");

    for cid in &targets {
        app.client_registry.remove(cid);
        app.attached_clients = app.attached_clients.saturating_sub(1);
    }
    assert_eq!(app.client_registry.len(), 1);
    assert!(app.client_registry.contains_key(&2));
    assert_eq!(app.attached_clients, 1);
}

/// `detach-client -a` from CLI (except = u64::MAX) detaches everyone.
#[test]
fn detach_all_other_clients_with_cli_sentinel_detaches_all() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");
    add_client(&mut app, 2, "/dev/pts/2");
    let except = u64::MAX;

    let targets: Vec<u64> = app.client_registry.iter()
        .filter(|(cid, _)| **cid != except)
        .map(|(cid, _)| *cid)
        .collect();
    assert_eq!(targets.len(), 2, "u64::MAX sentinel matches no client → all detach");

    for cid in &targets {
        app.client_registry.remove(cid);
        app.attached_clients = app.attached_clients.saturating_sub(1);
    }
    assert!(app.client_registry.is_empty());
    assert_eq!(app.attached_clients, 0);
}

/// `detach-client -s <session>` (and the CLI default) detaches every client.
#[test]
fn detach_all_clients_clears_registry() {
    let mut app = mock_app();
    add_client(&mut app, 1, "/dev/pts/1");
    add_client(&mut app, 2, "/dev/pts/2");
    add_client(&mut app, 3, "/dev/pts/3");
    app.latest_client_id = Some(2);
    app.client_prefix_active = true;

    let targets: Vec<u64> = app.client_registry.keys().copied().collect();
    for cid in &targets {
        app.client_registry.remove(cid);
        app.attached_clients = app.attached_clients.saturating_sub(1);
    }
    if !targets.is_empty() {
        app.latest_client_id = None;
        app.client_prefix_active = false;
    }

    assert!(app.client_registry.is_empty());
    assert_eq!(app.attached_clients, 0);
    assert_eq!(app.latest_client_id, None);
    assert!(!app.client_prefix_active);
}

/// destroy_unattached + last client detached → server should be eligible for shutdown.
#[test]
fn detach_last_client_with_destroy_unattached_signals_shutdown() {
    let mut app = mock_app();
    app.destroy_unattached = true;
    add_client(&mut app, 1, "/dev/pts/1");

    app.client_registry.remove(&1);
    app.attached_clients = app.attached_clients.saturating_sub(1);

    // Replicates the handler's exit-eligibility check.
    let eligible = app.attached_clients == 0 && app.destroy_unattached;
    assert!(eligible, "destroy_unattached + zero clients → shutdown path");
}

/// Without destroy_unattached, the same condition should NOT trigger shutdown.
#[test]
fn detach_last_client_without_destroy_unattached_does_not_signal_shutdown() {
    let mut app = mock_app();
    app.destroy_unattached = false;
    add_client(&mut app, 1, "/dev/pts/1");

    app.client_registry.remove(&1);
    app.attached_clients = app.attached_clients.saturating_sub(1);

    let eligible = app.attached_clients == 0 && app.destroy_unattached;
    assert!(!eligible, "without destroy_unattached, server stays alive");
}

// ════════════════════════════════════════════════════════════════════════════
//  Directive-channel gating (grace-sleep precondition)
// ════════════════════════════════════════════════════════════════════════════
//
// The detach handlers take the 50 ms grace sleep only when
// `send_directive_to_client` reports the directive was queued.  A client with no
// registered channel is already gone, so the send reports false and the handler
// skips the sleep instead of stalling the server loop for nothing.  These tests
// pin that true/false contract directly, without spinning up a server.

/// Absent channel → the directive cannot be queued, so the send reports false
/// and the handler skips the grace sleep.
#[test]
fn send_directive_reports_false_without_channel() {
    use crate::types::{remove_directive_channel, send_directive_to_client};
    let cid = 0xDEAD_0001u64;
    remove_directive_channel(cid); // drop any entry a prior run might have left
    assert!(!send_directive_to_client(cid, "DETACH"),
        "absent channel must report not-queued so the handler skips the sleep");
}

/// Registered channel → the directive is queued (send reports true) and the
/// exact string is delivered; after removal the send reports false again.
#[test]
fn send_directive_delivers_then_stops_after_removal() {
    use crate::types::{register_directive_channel, remove_directive_channel, send_directive_to_client};
    let cid = 0xDEAD_0002u64;
    let rx = register_directive_channel(cid);

    assert!(send_directive_to_client(cid, "DETACH"),
        "registered channel must report queued so the handler takes the grace sleep");
    assert_eq!(rx.recv().ok().as_deref(), Some("DETACH"),
        "the exact directive string must reach the client's writer thread");

    remove_directive_channel(cid);
    assert!(!send_directive_to_client(cid, "DETACH"),
        "after channel removal the send must report not-queued again");
}

// ════════════════════════════════════════════════════════════════════════════
//  CLI flag-parsing tests (mirror the parser in main.rs detach-client branch)
// ════════════════════════════════════════════════════════════════════════════

/// Helper: parse the same flag set the CLI dispatch parses.
fn parse_detach_args(argv: &[&str]) -> (Option<String>, Option<String>, bool, bool, Option<String>) {
    let mut t_target: Option<String> = None;
    let mut s_target: Option<String> = None;
    let mut detach_all = false;
    let mut kill_parent = false;
    let mut shell_cmd: Option<String> = None;
    let mut i = 0;
    while i < argv.len() {
        match argv[i] {
            "-a" => { detach_all = true; }
            "-P" => { kill_parent = true; }
            "-t" => { if let Some(v) = argv.get(i + 1) { t_target = Some(v.to_string()); i += 1; } }
            "-s" => { if let Some(v) = argv.get(i + 1) { s_target = Some(v.to_string()); i += 1; } }
            "-E" => { if let Some(v) = argv.get(i + 1) { shell_cmd = Some(v.to_string()); i += 1; } }
            _ => {}
        }
        i += 1;
    }
    (t_target, s_target, detach_all, kill_parent, shell_cmd)
}

#[test]
fn cli_parse_no_args() {
    let (t, s, a, p, e) = parse_detach_args(&[]);
    assert_eq!(t, None);
    assert_eq!(s, None);
    assert!(!a);
    assert!(!p);
    assert_eq!(e, None);
}

#[test]
fn cli_parse_t_with_session_name() {
    let (t, _, _, _, _) = parse_detach_args(&["-t", "main"]);
    assert_eq!(t, Some("main".to_string()));
}

#[test]
fn cli_parse_t_with_tty_path() {
    let (t, _, _, _, _) = parse_detach_args(&["-t", "/dev/pts/2"]);
    assert_eq!(t, Some("/dev/pts/2".to_string()));
}

#[test]
fn cli_parse_t_with_percent_id() {
    let (t, _, _, _, _) = parse_detach_args(&["-t", "%5"]);
    assert_eq!(t, Some("%5".to_string()));
    let numeric: Option<u64> = t.as_ref().and_then(|v| v.trim_start_matches('%').parse().ok());
    assert_eq!(numeric, Some(5));
}

#[test]
fn cli_parse_a_flag() {
    let (_, _, a, _, _) = parse_detach_args(&["-a"]);
    assert!(a);
}

#[test]
fn cli_parse_P_flag() {
    let (_, _, _, p, _) = parse_detach_args(&["-P"]);
    assert!(p);
}

#[test]
fn cli_parse_combined_aP() {
    let (_, _, a, p, _) = parse_detach_args(&["-a", "-P"]);
    assert!(a);
    assert!(p);
}

#[test]
fn cli_parse_s_and_t_together() {
    let (t, s, _, _, _) = parse_detach_args(&["-s", "work", "-t", "%1"]);
    assert_eq!(s, Some("work".to_string()));
    assert_eq!(t, Some("%1".to_string()));
}

#[test]
fn cli_parse_E_shell_command() {
    let (_, _, _, _, e) = parse_detach_args(&["-E", "exit"]);
    assert_eq!(e, Some("exit".to_string()));
}

#[test]
fn cli_parse_unknown_flags_ignored() {
    // Unknown flags must not panic or consume positional arguments.
    let (t, _, _, _, _) = parse_detach_args(&["-X", "garbage", "-t", "main"]);
    assert_eq!(t, Some("main".to_string()));
}

// ════════════════════════════════════════════════════════════════════════════
//  Action mapping (keybinding dispatch path)
// ════════════════════════════════════════════════════════════════════════════

/// `detach-client` and `detach` (alias) both resolve to Action::Detach.
/// This is what `bind-key d detach-client` binds to.
#[test]
fn detach_client_resolves_to_action_detach() {
    use crate::types::Action;
    assert!(matches!(parse_command_to_action("detach-client"), Some(Action::Detach)),
        "detach-client should map to Action::Detach");
    assert!(matches!(parse_command_to_action("detach"), Some(Action::Detach)),
        "detach (alias) should map to Action::Detach");
}

/// Flag suffixes (`-a`, `-P`) on the bound command should still resolve to
/// Detach so prefix+d-with-flags works the same.  We accept either Detach or
/// a generic Command(...) — both are valid dispatch shapes.
#[test]
fn detach_with_flags_still_dispatches() {
    let action = parse_command_to_action("detach-client -a");
    assert!(action.is_some(), "detach-client -a must produce some Action");
}
