// Issue #434: Ghost-client leak / attached_clients over-decrement.
//
// Two confirmed code-level defects:
//   A. CtrlReq::ClientDetach decremented `attached_clients` UNCONDITIONALLY
//      (before removing the registry entry), so a duplicate detach for one
//      `cid` over-decremented the counter -> `session_attached` could read 0
//      while `list-clients` still showed a live client (registry/counter
//      desync), and could even fire the destroy-unattached exit with a real
//      client still attached.
//   B. The writer thread's Guard::drop tore the socket down without reaping
//      the registry entry, so a writer-only teardown could orphan a ClientInfo
//      (ghost) when the reader never fired ClientDetach.
//
// Fix: AppState::reap_client(cid) -> bool removes the registry entry and
// updates the counter/prefix/latest-client bookkeeping ONLY when the entry was
// actually present. Idempotent: a second reap is a safe no-op. Both the
// ClientDetach handler and the writer Guard route through it.
//
// These tests prove the invariant `attached_clients == client_registry.len()`
// holds across duplicate detaches, which the old unguarded path violated.

use super::*;

fn client_info(id: u64, control: bool) -> ClientInfo {
    ClientInfo {
        id,
        width: 80,
        height: 24,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: format!("/dev/pts/{}", id),
        is_control: control,
        termfeatures: crate::terminal_features::TerminalFeatures::from_bits(0xf),
        last_session: None,
        pid: 0,
        readonly: false,
        focused: false,
    }
}

fn app_with_clients(ids: &[u64]) -> AppState {
    let mut app = AppState::new("test434".to_string());
    for &id in ids {
        app.client_registry.insert(id, client_info(id, false));
        app.client_sizes.insert(id, (80, 24));
        app.attached_clients = app.attached_clients.saturating_add(1);
        app.latest_client_id = Some(id);
    }
    app
}

// ---- Demonstrate the ORIGINAL bug premise (claim A), reproduced literally ----
// This models exactly what the old ClientDetach handler did: decrement first,
// then remove, with no guard. A duplicate detach for the same cid desyncs the
// counter below the true registry size.
#[test]
fn old_unguarded_double_detach_desyncs_counter() {
    let mut app = app_with_clients(&[10, 20]); // two attached clients
    assert_eq!(app.attached_clients, 2);
    assert_eq!(app.client_registry.len(), 2);

    // Simulate the OLD unguarded handler firing twice for cid=10.
    let old_detach = |app: &mut AppState, cid: u64| {
        app.attached_clients = app.attached_clients.saturating_sub(1); // unconditional
        app.client_sizes.remove(&cid);
        app.client_registry.remove(&cid);
    };
    old_detach(&mut app, 10);
    old_detach(&mut app, 10); // duplicate for the SAME cid

    // BUG: counter reached 0 while client 20 is still registered -> desync.
    assert_eq!(app.attached_clients, 0, "old path over-decrements to 0");
    assert_eq!(app.client_registry.len(), 1, "client 20 still present");
    assert_ne!(
        app.attached_clients,
        app.client_registry.len(),
        "old path leaves attached_clients desynced from registry (the reported bug)"
    );
}

// ---- Prove the FIX (reap_client) keeps counter == registry ----
#[test]
fn reap_client_double_detach_keeps_counter_in_sync() {
    let mut app = app_with_clients(&[10, 20]);
    assert_eq!(app.attached_clients, 2);

    let first = app.reap_client(10);
    assert!(first, "first reap of a present client returns true");
    assert_eq!(app.attached_clients, 1);
    assert_eq!(app.client_registry.len(), 1);

    // Duplicate detach for the SAME cid -> no-op, counter untouched.
    let second = app.reap_client(10);
    assert!(!second, "second reap of the same cid is a no-op");
    assert_eq!(app.attached_clients, 1, "counter NOT over-decremented");
    assert_eq!(app.client_registry.len(), 1);
    assert_eq!(
        app.attached_clients,
        app.client_registry.len(),
        "attached_clients stays in lock-step with the registry"
    );
    assert!(app.client_registry.contains_key(&20), "live client 20 survives");
}

#[test]
fn reap_client_is_idempotent_on_unknown_cid() {
    let mut app = app_with_clients(&[7]);
    assert_eq!(app.attached_clients, 1);
    // Reaping a cid that was never registered must not touch the counter.
    assert!(!app.reap_client(999));
    assert_eq!(app.attached_clients, 1);
    assert_eq!(app.client_registry.len(), 1);
}

#[test]
fn reap_client_updates_latest_client_id_to_remaining_max() {
    let mut app = app_with_clients(&[3, 9, 5]);
    app.latest_client_id = Some(9);
    assert!(app.reap_client(9));
    // latest should fall back to the max remaining registered id (5), not None.
    assert_eq!(app.latest_client_id, Some(5));
    // Reaping a non-latest client leaves latest_client_id alone.
    assert!(app.reap_client(3));
    assert_eq!(app.latest_client_id, Some(5));
}

#[test]
fn reap_client_clears_sizes_and_prefix() {
    let mut app = app_with_clients(&[42]);
    app.client_prefix_active = true;
    assert!(app.client_sizes.contains_key(&42));
    assert!(app.reap_client(42));
    assert!(!app.client_sizes.contains_key(&42), "client size removed on reap");
    assert!(!app.client_prefix_active, "stale prefix state cleared on reap");
    assert_eq!(app.attached_clients, 0);
}

// Reaping the LAST client drives the counter to exactly 0 (so destroy-unattached
// fires once, correctly), and a duplicate never re-triggers it.
#[test]
fn reap_last_client_reaches_zero_once() {
    let mut app = app_with_clients(&[1]);
    assert!(app.reap_client(1));
    assert_eq!(app.attached_clients, 0, "reaches zero exactly once");
    // A stray duplicate must NOT drive it negative / re-fire teardown logic.
    assert!(!app.reap_client(1));
    assert_eq!(app.attached_clients, 0, "stays zero, no spurious re-trigger");
}

// A persistent connection may send both its implicit `client-attach` and an
// explicit `attach-session`. Both map to ClientAttach for the same server-side
// id. Before register_client(), the counter incremented twice while the registry
// contained one row; the single disconnect then left attached_clients == 1
// forever even though list-clients was empty.
#[test]
fn duplicate_attach_then_single_detach_does_not_leave_ghost_attached() {
    let mut app = AppState::new("test434_duplicate_attach".to_string());

    assert!(app.register_client(42, false), "first attach registers the client");
    assert!(!app.register_client(42, false), "duplicate attach is a no-op");
    assert_eq!(app.attached_clients, 1);
    assert_eq!(app.client_registry.len(), 1);

    assert!(app.reap_client(42), "one detach reaps the one client");
    assert_eq!(app.attached_clients, 0, "session is no longer ghost-attached");
    assert!(app.client_registry.is_empty(), "list-clients has no ghost row");
}

#[test]
fn duplicate_control_register_preserves_counter_registry_invariant() {
    let mut app = AppState::new("test434_duplicate_control".to_string());
    app.latest_client_id = Some(12);

    assert!(app.register_client(77, true));
    assert!(!app.register_client(77, true));
    assert_eq!(app.attached_clients, app.client_registry.len());
    assert!(app.client_registry.get(&77).unwrap().is_control);
    assert_eq!(app.latest_client_id, Some(12), "control clients do not replace the latest interactive client");
}
