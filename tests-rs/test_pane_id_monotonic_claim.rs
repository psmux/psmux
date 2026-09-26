//! Pane ids must never walk backwards, including when the pool ran dry.
//!
//! tmux allocates a pane id when the pane is created and the sequence never
//! goes backwards, so scripts address panes as %1, %2, %3 on that basis. A
//! psmux spare's id is allocated when it is SPAWNED, because it is planted in
//! the shell as TMUX_PANE and a child's environment cannot be rewritten, so the
//! claim order is the visible id order.
//!
//! `WarmPool::push` already refuses a spare whose id is below the floor, and
//! the cold spawn path raised that floor before taking `next_pane_id` for
//! itself. What nothing raised was the floor for an id handed out FROM the
//! pool. That normally cannot matter, because the pool is sorted and a claim
//! takes its front, but refills are spawned concurrently and land in whatever
//! order the OS finishes them: when the pool ran dry, the claim that waited for
//! an in flight spare got the first one to LAND, which can be a higher id than
//! the ones still on their way. Those lower ids then landed, passed `push`
//! because the floor had never moved, and were handed to the next creation.
//!
//! Measured on the full sweep of 2026-09-19 and reproduced in 2 of the first 3
//! reruns, a burst of five `new-window` with no waiting came out
//! `%2 %3 %4 %11 %9`, `%2 %3 %4 %11 %7` and `%2 %3 %4 %10 %6`. A run that kept
//! its order came out `%2 %3 %4 %6 %11`: gaps are fine, going backwards is not.

use super::*;

/// A spare with no real shell behind it. The pool only inspects `pane_id`,
/// `spawned_at`, `ready` and the child's liveness.
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
        host_colors: None,
    }
}

fn pool() -> crate::types::WarmPool {
    crate::types::WarmPool::default()
}

/// The exact shape of the sweep failure: the pool ran dry, an in flight spare
/// landed out of order and was handed out, and a lower id arrived afterwards.
#[test]
fn a_spare_landing_below_an_id_already_handed_out_is_refused() {
    let mut p = pool();
    // The pool ran dry and the first refill to land is a high one.
    p.push(spare(11));
    let (claimed, _) = p.claim();
    assert_eq!(claimed.map(|w| w.pane_id), Some(11), "the only spare is the one handed out");

    // The lower ids the concurrent refills reserved now land.
    p.push(spare(9));
    p.push(spare(7));
    assert_eq!(
        p.len(),
        0,
        "a spare whose id is below one already handed out must be refused, not pooled"
    );

    let (after, _) = p.claim();
    assert!(
        after.is_none(),
        "the next creation must not be handed an id below %11; it cold spawns or waits instead"
    );
}

/// The ordinary path must be untouched: a sorted pool still hands out every one
/// of its spares, lowest first.
#[test]
fn an_in_order_pool_still_hands_out_every_spare() {
    let mut p = pool();
    for id in [2usize, 3, 4] {
        p.push(spare(id));
    }
    let mut got = Vec::new();
    while let (Some(w), _) = p.claim() {
        got.push(w.pane_id);
    }
    assert_eq!(got, vec![2, 3, 4], "lowest id first, none refused");
}

/// A spare above the last id handed out is still perfectly good.
#[test]
fn a_spare_landing_above_the_last_id_handed_out_is_kept() {
    let mut p = pool();
    p.push(spare(11));
    let _ = p.claim();
    p.push(spare(12));
    assert_eq!(p.len(), 1, "12 is ahead of 11, so it is still usable");
    let (next, _) = p.claim();
    assert_eq!(next.map(|w| w.pane_id), Some(12));
}

/// Claiming must move the floor the same way the cold spawn path does, so the
/// two ways an id can be issued cannot disagree.
#[test]
fn claiming_raises_the_floor_like_a_cold_spawn_does() {
    let mut p = pool();
    p.push(spare(11));
    assert_eq!(p.issued_floor(), 0, "nothing issued yet");
    let _ = p.claim();
    assert_eq!(p.issued_floor(), 11, "handing out %11 puts every lower id in the past");
    // And an explicit floor raise still wins when it is higher.
    p.set_issued_floor(20);
    assert_eq!(p.issued_floor(), 20);
    let _ = p.claim();
    assert_eq!(p.issued_floor(), 20, "an empty claim cannot lower the floor");
}
