// #661: the pool's depth must hold for as long as the user keeps creating
// panes, not for five seconds after the one claim that happened to miss.
//
// WHAT WAS MEASURED (40 `split-window -v`, one live session, one attached
// client, ~265ms apart, PSMUX_WARM_TRACE):
//
//   [12736.402] claim(split): pane=31 spare_age=2046.2ms pool_left=7 ready_left=6
//   [12839.661] pool: surge over, trimmed 6 surplus spare(s) back to target=2
//   [12840.356] pool: sample depth=2 ready=2 inflight=0 target=2 eff=2
//   [13002.912] claim(split): pane=32 spare_age=2056.4ms pool_left=1 ready_left=1
//   [13267.629] claim(split): pane=33 spare_age=2055.7ms pool_left=1 ready_left=0
//
// Six ready spares were killed 103ms after a split and 160ms before the next
// one, in the middle of a run of splits. `surge_until` was only ever written by
// a claim that MISSED, so a run the surge was serving perfectly renewed
// nothing, and `WARM_SURGE_HOLD` counted down from a miss six seconds in the
// past. After the trim the pool alternated one ready spare with none and the
// visible split latency went from ~20ms to 120-580ms.
//
// The decisions under test live in `WarmPool::note_claim` and are pure: what
// opens the surge window, what holds it open, and what still gives the memory
// back. The end to end numbers are in tests/test_issue661_warm_pool_depth_holds.ps1.

use super::*;
use crate::types::{WarmPool, WARM_POOL_SURGE_MAX};

// ── what OPENS the window ──────────────────────────────────────────

#[test]
fn issue661_a_claim_that_takes_the_last_ready_spare_surges() {
    let _dummies = DummyGuard661;
    // The leading edge. This claim was served, so the old rule saw nothing
    // wrong; but the pool it leaves behind has no ready spare, so the NEXT
    // creation of this run is certain to miss, and a spare needs ~400ms to
    // become useful. Reacting on the miss is reacting one creation too late.
    let mut pool = WarmPool::new(2);
    pool.push(ready_spare661(1));
    pool.push(ready_spare661(2));
    let (first, ready) = pool.claim();
    assert!(ready, "the pool starts with two settled spares");
    drop(first);
    pool.note_claim(true);
    assert!(!pool.is_surging(), "one creation is not a run");
    let (second, ready) = pool.claim();
    assert!(ready);
    drop(second);
    pool.note_claim(true);
    assert!(
        pool.is_surging(),
        "the run just took the last ready spare: the next creation misses unless the pool widens now"
    );
    assert_eq!(pool.effective_target(false), WARM_POOL_SURGE_MAX);
}

#[test]
fn issue661_a_lone_claim_that_empties_the_pool_does_not_surge() {
    let _dummies = DummyGuard661;
    // The regression this must not cause: a cold `new-session` claims its only
    // spare, leaving the pool empty and unready. Surging there fires eight
    // shell spawns beside the session's own starting shell, measured at ~100ms
    // of extra startup, for a user who created exactly one thing.
    let mut pool = WarmPool::new(2);
    pool.push(ready_spare661(1));
    let _ = pool.claim();
    pool.note_claim(true);
    assert!(!pool.is_surging(), "one creation is still not a run");
    assert_eq!(pool.effective_target(false), 2);
}

// ── what HOLDS the window open ─────────────────────────────────────

#[test]
fn issue661_a_run_the_surge_is_serving_holds_the_window_open() {
    let _dummies = DummyGuard661;
    // The defect itself. Two claims open the surge; the pool fills to eight and
    // every later claim in the run is served from it, so under the old rule
    // nothing ever wrote `surge_until` again and the window expired under a run
    // that was still going.
    let mut pool = WarmPool::new(2);
    pool.note_claim(true);
    pool.note_claim(false);
    assert!(pool.is_surging());
    // The pool is now deep and every claim is satisfied with ready spares left
    // over, which is exactly the state that renewed nothing before.
    for id in 1..=8 {
        pool.push(ready_spare661(id));
    }
    for _ in 0..5 {
        let (wp, ready) = pool.claim();
        assert!(ready, "a surged pool serves its run from settled spares");
        drop(wp);
        pool.note_claim(true);
        assert!(
            pool.is_surging(),
            "the run is still going, so the depth that is serving it must not be released"
        );
        assert_eq!(pool.effective_target(false), WARM_POOL_SURGE_MAX);
    }
}

#[test]
fn issue661_a_gap_longer_than_the_burst_window_stops_renewing() {
    let _dummies = DummyGuard661;
    // The other half of the contract: renewal is driven by the run continuing,
    // so a creation that is NOT part of a run renews nothing and the surplus is
    // on its way back. Without this the window would be held open by any
    // creation for ever.
    let mut pool = WarmPool::new(2);
    pool.note_claim(true);
    pool.note_claim(false);
    assert!(pool.is_surging());
    pool.end_surge_for_test();
    pool.forget_last_claim_for_test();
    for id in 1..=6 {
        pool.push(ready_spare661(id));
    }
    let _ = pool.claim();
    pool.note_claim(true);
    assert!(
        !pool.is_surging(),
        "an isolated creation long after the run must not reopen a surge"
    );
}

// ── what still gives the memory BACK ───────────────────────────────

#[test]
fn issue661_surplus_still_comes_back_once_the_run_stops() {
    let _dummies = DummyGuard661;
    // Holding the depth for the length of the run must not turn into holding it
    // for ever: eight idle shells for a user who configured two is the leak the
    // trim exists to prevent.
    let mut pool = WarmPool::new(2);
    pool.note_claim(true);
    pool.note_claim(false);
    for id in 1..=8 {
        pool.push(ready_spare661(id));
    }
    assert_eq!(pool.trim_surplus(), 0, "nothing is released while the run is live");
    pool.end_surge_for_test();
    assert_eq!(pool.trim_surplus(), 6);
    assert_eq!(pool.len(), 2, "back to the configured depth once the user stops");
    assert_eq!(pool.effective_target(false), 2);
}

#[test]
fn issue661_a_standby_never_deepens_however_long_the_run() {
    let _dummies = DummyGuard661;
    // A `__warm__` helper creates no windows of its own and can sit around for
    // days. Whatever the renewal rule does, its cap stays at one spare.
    let mut pool = WarmPool::new(4);
    for _ in 0..6 {
        pool.note_claim(true);
        pool.note_claim(false);
        assert_eq!(pool.effective_target(true), 1);
    }
}

#[test]
fn issue661_a_disabled_pool_never_surges_however_long_the_run() {
    let _dummies = DummyGuard661;
    let mut pool = WarmPool::new(0);
    for _ in 0..6 {
        pool.note_claim(true);
        pool.note_claim(false);
        assert_eq!(pool.effective_target(false), 0);
        assert_eq!(pool.deficit_for(pool.effective_target(false)), 0);
    }
}

// ── dummy lifetime guard ───────────────────────────────────────────
//
// Same rule as test_warm_pool_depth.rs: the stand in spares are `cmd /c pause`
// children under a pseudoconsole, they do not reliably exit when the master
// drops, and an orphan surfaces as a Windows Terminal tab waiting at "Press any
// key". Every pid is recorded at spawn and ended BY PID when the guard drops,
// including on the panic path. Never by image name.
thread_local! {
    static DUMMY_PIDS_661: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

struct DummyGuard661;

impl Drop for DummyGuard661 {
    fn drop(&mut self) {
        DUMMY_PIDS_661.with(|pids| {
            for pid in pids.borrow_mut().drain(..) {
                let _ = std::process::Command::new("taskkill")
                    .args(["/pid", &pid.to_string(), "/t", "/f"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
    }
}

/// A spare with no shell behind it. The pool only inspects `pane_id`,
/// `spawned_at`, `ready` and the child's liveness.
fn fake_spare661(pane_id: usize) -> crate::types::WarmPane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("pause");
    let child = crate::util::spawn_pty_child(&*pair.slave, cmd).expect("spawn dummy");
    if let Some(pid) = child.process_id() {
        DUMMY_PIDS_661.with(|pids| pids.borrow_mut().push(pid));
    }
    let writer = pair.master.take_writer().expect("writer");
    let now = std::time::Instant::now();
    crate::types::WarmPane {
        master: pair.master,
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
        ready: false,
        last_dv: 0,
        last_change: now,
        trace_settled: false,
        host_colors: None,
    }
}

/// A spare already past its shell startup, which is the only kind worth having.
fn ready_spare661(pane_id: usize) -> crate::types::WarmPane {
    let mut wp = fake_spare661(pane_id);
    wp.ready = true;
    wp
}
