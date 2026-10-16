//! Issue #659 follow up: a pool respawn must also refuse the spares that were
//! still being spawned when it was requested.
//!
//! `WarmPaneSync::Respawn` is what a warm claim applies after adopting the
//! claiming client's environment (#659), and what a `default-shell` change, a
//! client resize and a host colour change apply. `kill_all` only reaches the
//! spares already in the pool. A refill issued before the respawn captured the
//! old environment block the moment its `CreateProcessW` began and lands in
//! the pool afterwards, and `new-window` hands it out.
//!
//! Measured with `tests/test_issue659_warm_claim_environment.ps1`: a standby
//! trickles its pool one spare at a time after its boot hold, each spawn 75 to
//! 270 ms inside `CreateProcessW`, and a claim arriving inside that window gave
//! the next window the standby's environment in 3 of 10 runs on dd695eaf.
//!
//! Every spare issued so far carries an id below `ids.next_pane`, so raising
//! the pool's issued floor there makes `WarmPool::push` refuse each late
//! arrival, the gate a cold creation already uses (ebce952).

use super::*;

fn spare(pane_id: usize) -> crate::types::WarmPane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    });
    let child = crate::util::StubChild::running();
    let now = std::time::Instant::now();
    crate::types::WarmPane {
        master,
        writer,
        child,
        term: std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(40, 120, 100))),
        data_version: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        cursor_shape: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0)),
        bell_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        cpr_pending: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        color_query_pending: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
        child_pid: None,
        pane_id,
        rows: 40,
        cols: 120,
        output_ring: std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        spawned_at: now,
        ready: true,
        last_dv: 0,
        last_change: now,
        trace_settled: false,
        ready_via_backstop: false,
        spawn_cwd: None,
        host_colors: None,
    }
}

/// An `AppState` whose pool looks like a standby mid trickle: one spare
/// pooled (%2), one being spawned (%3, issued, not landed), the next id %4.
fn app_mid_trickle() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.warm_pane = crate::types::WarmPool::default();
    app.warm_pane.push(spare(2));
    app.warm_pane.inflight = 1;
    app.ids.set_next_pane(4);
    app
}

fn respawn_now(app: &mut AppState) {
    let pty_system = portable_pty::native_pty_system();
    apply(app, &*pty_system, WarmPaneSync::Respawn("test: claim adopted the client environment"));
}

#[test]
fn the_pooled_spare_is_killed_by_the_respawn() {
    let mut app = app_mid_trickle();
    respawn_now(&mut app);
    assert_eq!(app.warm_pane.len(), 0, "the pooled spare carries the old environment and must go");
}

#[test]
fn the_in_flight_spare_is_refused_when_it_lands_after_the_respawn() {
    // The exact shape of the #659 flake: %3 was issued (and began its
    // CreateProcessW, capturing the standby's environment) before the claim,
    // and lands after the respawn killed the pool.
    let mut app = app_mid_trickle();
    respawn_now(&mut app);
    app.warm_pane.push(spare(3));
    assert_eq!(
        app.warm_pane.len(),
        0,
        "a spare issued before the respawn landed with the old environment and must be refused, not pooled"
    );
    let (claimed, _) = app.warm_pane.claim();
    assert!(claimed.is_none(), "new-window must not be handed the stale spare");
}

#[test]
fn a_spare_issued_after_the_respawn_is_pooled() {
    let mut app = app_mid_trickle();
    respawn_now(&mut app);
    // The refill the server loop issues for the deficit takes the next id.
    let id = app.ids.alloc_pane();
    app.warm_pane.push(spare(id));
    assert_eq!(app.warm_pane.len(), 1, "a spare spawned after the respawn has the new environment and is kept");
    let (claimed, _) = app.warm_pane.claim();
    assert_eq!(claimed.map(|w| w.pane_id), Some(id));
}

#[test]
fn the_floor_is_the_next_pane_id_so_ids_keep_going_forward() {
    let mut app = app_mid_trickle();
    respawn_now(&mut app);
    assert_eq!(app.warm_pane.issued_floor(), 4, "the floor is raised to next_pane_id, nothing beyond it");
    assert_eq!(app.ids.peek_pane(), 4, "the respawn allocates no id of its own");
}

#[test]
fn a_respawn_with_nothing_in_flight_still_refuses_nothing_new() {
    // No in flight spawn, no pooled spare: the respawn must not break the
    // ordinary refill that follows.
    let mut app = AppState::new("test_session".to_string());
    app.warm_pane = crate::types::WarmPool::default();
    app.ids.set_next_pane(7);
    respawn_now(&mut app);
    app.warm_pane.push(spare(7));
    assert_eq!(app.warm_pane.len(), 1);
}

#[test]
fn a_client_resize_keeps_the_in_flight_spare() {
    // A resize is reconciled by the transplant (`need_resize`), and a host
    // colour report by `land_spare`; both fire right after an attached launch
    // has issued its first trickled spare, so that spare must be kept.
    let mut app = app_mid_trickle();
    let pty_system = portable_pty::native_pty_system();
    apply(&mut app, &*pty_system, WarmPaneSync::RespawnKeepInflight("client resized"));
    assert_eq!(app.warm_pane.len(), 0, "the pooled spare is still replaced");
    app.warm_pane.push(spare(3));
    assert_eq!(app.warm_pane.len(), 1, "the refill that was in flight lands and is kept");
    assert_eq!(app.warm_pane.issued_floor(), 0, "no floor is raised for a keep in flight respawn");
}

#[test]
fn the_resize_and_host_colour_decisions_keep_in_flight_spares() {
    // for_resize on an empty pool asks for a respawn (the refill must happen
    // at the new size), and for_host_colors_change on a pool whose spare
    // carries other colours does too; both must be the keep in flight kind.
    let mut app = AppState::new("test_session".to_string());
    app.warm_pane = crate::types::WarmPool::default();
    assert!(matches!(for_resize(&app, 24, 80), WarmPaneSync::RespawnKeepInflight(_)));
    app.warm_pane.push(spare(2));
    app.host_colors = Some(crate::types::HostColors { fg: Some((1, 2, 3)), bg: None, palette: [None; 16], dark: None });
    assert!(matches!(for_host_colors_change(&app), WarmPaneSync::RespawnKeepInflight(_)));
}
