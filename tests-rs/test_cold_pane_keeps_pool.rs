//! A pane that cannot use a spare must not throw the whole pool away.
//!
//! `new-window <cmd>`, `split-window <cmd>`, `-e` and a dynamic default-shell
//! bypass the spare pool and cold spawn. They used to take `next_pane_id`,
//! which is above every id the pooled spares had reserved (a spare's id is its
//! shell's TMUX_PANE, fixed at spawn), so the floor rule killed EVERY spare and
//! the refill booted three fresh pwsh beside the command the user was waiting
//! on. Measured on a quiet machine: `new-window cmd /c <pwsh marker>` reached
//! its marker in ~560 ms with the pool on and ~360 ms with it off.
//!
//! The cold pane now takes the lowest reserved id and retires only the spare
//! that held it, so the ids still come out in creation order and the rest of
//! the pool stays warm.

use super::*;

fn spare(pane_id: usize) -> crate::types::WarmPane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: 40,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    });
    let now = std::time::Instant::now();
    crate::types::WarmPane {
        master,
        writer,
        child: crate::util::StubChild::running(),
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
        shows_text: false,
        spawn_cwd: None,
        host_colors: None,
    }
}

/// An app whose pool holds spares %2 %3 %4, so the next fresh id is %5.
fn app_with_pool() -> AppState {
    let mut app = AppState::new("coldpool".to_string());
    app.warm_pane = crate::types::WarmPool::new(3);
    for id in [2usize, 3, 4] {
        app.warm_pane.push(spare(id));
    }
    app.next_pane_id = 5;
    app
}

#[test]
fn a_cold_pane_takes_the_lowest_reserved_id_and_keeps_the_other_spares() {
    let mut app = app_with_pool();
    let id = cold_spawn_pane_id(&mut app);
    assert_eq!(id, 2, "the cold pane takes the pool front's id, the lowest outstanding one");
    assert_eq!(app.warm_pane.len(), 2, "only the spare that held %2 is retired, %3 and %4 stay warm");
    assert_eq!(app.next_pane_id, 5, "no fresh id was allocated, so the counter does not move");
    assert_eq!(app.warm_pane.issued_floor(), 2);
}

#[test]
fn ids_still_come_out_in_creation_order_after_a_cold_pane() {
    let mut app = app_with_pool();
    let mut order = vec![cold_spawn_pane_id(&mut app)];
    while let (Some(w), _) = app.warm_pane.claim() {
        order.push(w.pane_id);
    }
    order.push(cold_spawn_pane_id(&mut app));
    assert_eq!(order, vec![2, 3, 4, 5], "cold, warm, warm, cold: never backwards, no spare wasted");
}

#[test]
fn with_a_refill_in_flight_the_cold_pane_falls_back_to_a_fresh_id() {
    // A refill still inside CreateProcessW may hold an id below the pool
    // front, so the front cannot be handed out: the old rule applies.
    let mut app = app_with_pool();
    app.warm_pane.inflight = 1;
    let id = cold_spawn_pane_id(&mut app);
    assert_eq!(id, 5, "a fresh id above every reservation");
    assert_eq!(app.warm_pane.len(), 0, "every spare below the fresh id is retired, as before");
    assert_eq!(app.warm_pane.issued_floor(), 5);
}

#[test]
fn an_empty_pool_gives_a_fresh_id() {
    let mut app = AppState::new("coldpool_empty".to_string());
    app.warm_pane = crate::types::WarmPool::new(3);
    app.next_pane_id = 7;
    assert_eq!(cold_spawn_pane_id(&mut app), 7);
}
