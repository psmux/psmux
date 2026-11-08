// Unit tests for the spare shell pool: depth accounting, refill scheduling
// arithmetic, and the `warm-pool-size` option.
//
// The bug these pin: the pool used to be a bare `Option<WarmPane>`, a depth of
// one. Claim the single spare, refill, and the refill is a shell that started
// milliseconds ago -- so the NEXT creation claims a newborn and pays its whole
// startup. Sequential creations therefore alternated fast, slow, fast, slow.
//
// What is testable without a real shell is the accounting that drives it:
// `deficit()` (how many spawns the loop should hand to the background spawner
// this tick, counting the ones already in flight), FIFO claim order, and the
// option that sets the target. The end to end timings live in
// tests/test_pane_startup_perf.ps1, which asserts p90, max and bimodality.

use super::*;
use crate::types::{AppState, WarmPool, WARM_POOL_SIZE_DEFAULT, WARM_POOL_SIZE_MAX, WARM_POOL_SURGE_MAX};

// ── deficit(): the refill scheduler's only input ───────────────────

#[test]
fn empty_pool_asks_for_its_whole_target() {
    let pool = WarmPool::new(2);
    assert_eq!(pool.deficit(), 2, "an empty pool of target 2 needs 2 spawns");
}

#[test]
fn inflight_spawns_count_towards_the_target() {
    // Without this the server loop would queue one spawn per tick while the
    // first was still running: a 1ms loop would fire hundreds of shells.
    let mut pool = WarmPool::new(2);
    pool.inflight = 1;
    assert_eq!(pool.deficit(), 1);
    pool.inflight = 2;
    assert_eq!(pool.deficit(), 0);
    pool.inflight = 5;
    assert_eq!(pool.deficit(), 0, "over-supply must not underflow");
}

#[test]
fn target_zero_never_asks_for_a_spawn() {
    // `warm-pool-size 0` and `set -g warm off` both land here. A pool that
    // still refills after being switched off is an opt-out that does not opt
    // out.
    let mut pool = WarmPool::new(0);
    assert_eq!(pool.deficit(), 0);
    pool.inflight = 0;
    assert_eq!(pool.deficit(), 0);
}

// ── claim order ────────────────────────────────────────────────────

#[test]
fn claims_are_served_oldest_first() {
    // A spare is worth exactly as much as the amount of shell startup it has
    // already done, so the oldest spare is always the right one to hand out.
    // Ordering is what makes depth pay off; LIFO would hand the caller the
    // newest shell and reproduce the original bug at any depth.
    let mut pool = WarmPool::new(3);
    for id in [10usize, 11, 12] {
        pool.push(fake_spare(id));
    }
    assert_eq!(pool.take().map(|w| w.pane_id), Some(10));
    assert_eq!(pool.take().map(|w| w.pane_id), Some(11));
    assert_eq!(pool.take().map(|w| w.pane_id), Some(12));
    assert!(pool.take().is_none());
}

#[test]
fn claiming_creates_exactly_one_unit_of_deficit() {
    let mut pool = WarmPool::new(2);
    pool.push(fake_spare(1));
    pool.push(fake_spare(2));
    assert_eq!(pool.deficit(), 0, "a full pool schedules nothing");
    let _ = pool.take();
    assert_eq!(pool.deficit(), 1, "a claim must schedule its refill immediately");
    let _ = pool.take();
    assert_eq!(pool.deficit(), 2);
}

#[test]
fn kill_all_empties_the_pool_not_just_its_head() {
    // Every "the pool is stale now" site (resize, set-option, kill-server)
    // goes through this. Killing only the head would leave orphan shells.
    let mut pool = WarmPool::new(3);
    pool.push(fake_spare(1));
    pool.push(fake_spare(2));
    pool.push(fake_spare(3));
    pool.kill_all();
    assert!(pool.is_empty());
    assert_eq!(pool.len(), 0);
    assert_eq!(pool.deficit(), 3, "and the deficit is what refills it");
}

// ── the warm-pool-size option ──────────────────────────────────────

#[test]
fn option_sets_the_target_and_clamps_to_the_maximum() {
    let mut app = AppState::new("pooltest".to_string());
    crate::server::options::set_warm_pool_size(&mut app, "4");
    assert_eq!(app.warm_pane.target, 4);
    crate::server::options::set_warm_pool_size(&mut app, "999");
    assert_eq!(
        app.warm_pane.target, WARM_POOL_SIZE_MAX,
        "each spare is a real process, so the depth has to have a ceiling"
    );
}

#[test]
fn option_zero_disables_the_pool() {
    let mut app = AppState::new("pooltest".to_string());
    app.warm_pane.push(fake_spare(1));
    crate::server::options::set_warm_pool_size(&mut app, "0");
    assert_eq!(app.warm_pane.target, 0);
    assert!(app.warm_pane.is_empty(), "existing spares are released too");
    assert_eq!(app.warm_pane.deficit(), 0);
}

#[test]
fn shrinking_the_target_releases_the_surplus_now() {
    let mut app = AppState::new("pooltest".to_string());
    for id in 1..=4 {
        app.warm_pane.push(fake_spare(id));
    }
    crate::server::options::set_warm_pool_size(&mut app, "2");
    assert_eq!(app.warm_pane.len(), 2, "memory asked for back is given back");
    assert_eq!(app.warm_pane.deficit(), 0);
}

#[test]
fn non_numeric_value_leaves_the_pool_alone() {
    let mut app = AppState::new("pooltest".to_string());
    let before = app.warm_pane.target;
    crate::server::options::set_warm_pool_size(&mut app, "banana");
    assert_eq!(app.warm_pane.target, before);
}

#[test]
fn default_depth_is_greater_than_one() {
    // The whole point. Depth one is what produced the alternating
    // fast/slow window creation; a default of one would reintroduce it.
    assert!(
        WARM_POOL_SIZE_DEFAULT >= 2,
        "a pool of depth one cannot serve two creations in a row"
    );
    assert!(WARM_POOL_SIZE_DEFAULT <= WARM_POOL_SIZE_MAX);
}

#[test]
fn default_depth_serves_a_quick_run_after_a_cold_launch() {
    // 2026-10-08: at depth two, five new-windows 300 ms apart after a cold
    // launch stalled the 3rd and 4th (p50 173 and 791 ms); depth three kept
    // all five at 3 ms. See WARM_POOL_SIZE_DEFAULT for the measurement.
    assert_eq!(WARM_POOL_SIZE_DEFAULT, 3);
    let app = AppState::new("pooldefault".to_string());
    // The env override is how the measurement was taken; without it the
    // compiled default is what a fresh server gets.
    if std::env::var("PSMUX_WARM_POOL_SIZE").is_err() && std::env::var("PSMUX_NO_WARM").is_err() {
        assert_eq!(app.warm_pane.target, 3);
    }
    let def = crate::server::option_catalog::OPTION_CATALOG
        .iter()
        .find(|d| d.name == "warm-pool-size")
        .expect("warm-pool-size is in the catalog");
    assert_eq!(def.default, WARM_POOL_SIZE_DEFAULT.to_string(), "catalog default drifted from the compiled one");
}

// ── readiness: the thing depth alone did not fix ───────────────────
//
// Depth-N with async refill removed the fast/slow ALTERNATION and left a
// slow creation every third one. The trace said why: a spare becomes a pool
// member ~25ms after it is asked for (CreateProcess plus a ConPTY) but its pwsh
// needs ~400ms more to put a prompt up. Claims were being served spares 30 and
// 40ms old, which cost the caller the whole remaining startup -- measured at
// 376ms and 395ms against 15ms for a settled spare. Counting spares by
// existence rather than by readiness was the real defect.

#[test]
fn an_unready_spare_is_not_counted_as_ready() {
    let mut pool = WarmPool::new(2);
    pool.push(fake_spare(1));           // lands, but its shell is still starting
    assert_eq!(pool.len(), 1, "it is in the pool");
    assert_eq!(pool.ready_len(), 0, "but it is not ready");
    let (got, was_ready) = pool.claim();
    assert!(got.is_some(), "it is still the best thing available");
    assert!(!was_ready, "and the claim is reported as a miss");
}

#[test]
fn a_claim_still_takes_a_warming_spare_over_nothing() {
    // Readiness picks WHICH spare, never whether one is handed out. A spare
    // part way through its startup beats a cold spawn, which is 0ms through
    // one: refusing it made a cold `new-session` 330ms slower, because the one
    // spare such a server has is newborn by definition.
    let mut pool = WarmPool::new(2);
    pool.push(fake_spare(1));
    let (got, was_ready) = pool.claim();
    assert_eq!(got.map(|w| w.pane_id), Some(1), "the warming spare is used");
    assert!(!was_ready, "and the miss is reported, which is what opens a surge");
}

#[test]
fn a_claim_takes_the_lowest_id_even_when_a_later_spare_is_ready_first() {
    // Pane ids MUST come out in creation order, because a spare's id is
    // allocated when it is spawned (it is planted in the shell as TMUX_PANE, and
    // a child's environment cannot be rewritten afterwards). Preferring the
    // first READY spare reordered them: spares are spawned concurrently, so
    // they become ready in whatever order the OS finishes them, and ten splits
    // came out %2 %3 %4 %5 %12 %9 %11 %6 %7 %8.
    //
    // Taking the lowest id costs nothing in latency: the lowest id is also the
    // earliest spawned, so it is the spare furthest through its startup.
    let mut pool = WarmPool::new(3);
    pool.push(fake_spare(1));           // earliest, still starting
    pool.push(ready_spare(2));
    let (got, was_ready) = pool.claim();
    assert_eq!(got.map(|w| w.pane_id), Some(1), "creation order wins");
    assert!(!was_ready, "and the miss is reported so the pool surges");
}

#[test]
fn a_claim_on_an_empty_pool_reports_a_miss() {
    let mut pool = WarmPool::new(2);
    let (got, was_ready) = pool.claim();
    assert!(got.is_none(), "nothing to hand out, the caller cold spawns");
    assert!(!was_ready);
}

#[test]
fn a_ready_spare_is_handed_out() {
    let mut pool = WarmPool::new(2);
    pool.push(ready_spare(7));
    let (got, was_ready) = pool.claim();
    assert_eq!(got.map(|w| w.pane_id), Some(7));
    assert!(was_ready);
}

#[test]
fn consecutive_claims_hand_out_strictly_increasing_ids() {
    // The contract scripts depend on: the Nth creation gets the Nth id. tmux
    // allocates at creation and never goes backwards, and a pool of pre spawned
    // spares has to present the same sequence.
    let mut pool = WarmPool::new(8);
    // Landing order deliberately scrambled: this is what concurrent refills do.
    for id in [5usize, 2, 7, 3, 6, 4] {
        pool.push(if id % 2 == 0 { ready_spare(id) } else { fake_spare(id) });
    }
    let mut out = Vec::new();
    while let (Some(wp), _) = pool.claim() {
        out.push(wp.pane_id);
    }
    assert_eq!(out, vec![2, 3, 4, 5, 6, 7], "claims must come out in id order");
}

#[test]
fn a_spare_whose_id_is_already_in_the_past_is_refused() {
    // The burst case: four creations drain the pool, the fifth finds it empty
    // and takes a fresh id above every id the in flight refills reserved. Those
    // refills must not then be handed out, or the sequence reads %2 %3 %12 %4.
    let mut pool = WarmPool::new(8);
    pool.push(ready_spare(6));
    pool.push(ready_spare(7));
    let discarded = pool.set_issued_floor(12);
    assert_eq!(discarded, 2, "pooled spares below the floor are retired at once");
    assert!(pool.is_empty());
    // And one that was still in flight when the floor rose is refused on arrival.
    pool.push(ready_spare(8));
    assert!(pool.is_empty(), "a late spare below the floor is dropped, not pooled");
    pool.push(ready_spare(13));
    assert_eq!(pool.len(), 1, "ids above the floor are still welcome");
    assert_eq!(pool.claim().0.map(|w| w.pane_id), Some(13));
}

#[test]
fn the_id_floor_never_goes_backwards() {
    let mut pool = WarmPool::new(4);
    pool.set_issued_floor(10);
    assert_eq!(pool.set_issued_floor(4), 0, "a lower floor is ignored");
    assert_eq!(pool.issued_floor(), 10);
}

#[test]
fn a_spare_that_lands_late_still_keeps_its_place_in_the_sequence() {
    // Refill for id 3 finishes after the refill for id 4. It must still be
    // handed out first, or the visible ids go 4 then 3.
    let mut pool = WarmPool::new(4);
    pool.push(ready_spare(4));
    pool.push(ready_spare(3));
    assert_eq!(pool.claim().0.map(|w| w.pane_id), Some(3));
    assert_eq!(pool.claim().0.map(|w| w.pane_id), Some(4));
}

#[test]
fn readiness_needs_visible_text() {
    let mut wp = fake_spare(1);
    let t0 = std::time::Instant::now();
    // No output at all: not ready however long we wait (short of the backstop).
    assert!(!wp.refresh_ready(t0 + std::time::Duration::from_millis(600)));
    // The prompt arrives: ready at once.
    shell_writes(&wp.data_version, &wp.term, b"PS C:\\> ");
    assert!(wp.refresh_ready(t0 + std::time::Duration::from_millis(601)));
    assert!(!wp.ready_via_backstop);
}

#[test]
fn a_blank_quiet_spare_is_not_ready_issue661() {
    // #661: a loading pwsh writes ConPTY's setup sequences, which draw
    // nothing, and then stays silent until its prompt. On a busy machine that
    // silence outlasted WARM_READY_QUIET and blank spares counted as ready.
    let mut wp = fake_spare(1);
    let t0 = wp.spawned_at;
    shell_writes(&wp.data_version, &wp.term, b"\x1b[?25l\x1b[2J\x1b[H\x1b[?25h");
    assert!(!wp.refresh_ready(ms(t0, 20)));
    assert!(!wp.refresh_ready(ms(t0, 20) + crate::types::WARM_READY_QUIET), "blank and quiet is still booting");
    assert!(!wp.refresh_ready(ms(t0, 900)), "blank and quiet is still booting");
    assert!(!wp.shows_text);
    shell_writes(&wp.data_version, &wp.term, b"PS C:\\> ");
    assert!(wp.refresh_ready(ms(t0, 950)));
    assert!(wp.shows_text && !wp.ready_via_backstop);
}

#[test]
fn blank_spare_still_has_the_backstop_issue661() {
    let mut wp = fake_spare(1);
    shell_writes(&wp.data_version, &wp.term, b"\x1b[?25l\x1b[H");
    assert!(!wp.refresh_ready(wp.spawned_at + std::time::Duration::from_millis(10)));
    assert!(wp.refresh_ready(wp.spawned_at + crate::types::WARM_READY_MAX_WAIT));
    assert!(wp.ready_via_backstop);
}

#[test]
fn readiness_has_a_backstop_for_a_silent_shell() {
    // A `default-shell` that prints nothing would otherwise never be handed
    // out and every creation would cold spawn for ever. Past the backstop the
    // pool behaves as it did before readiness existed.
    let mut wp = fake_spare(1);
    let late = wp.spawned_at + crate::types::WARM_READY_MAX_WAIT;
    assert!(wp.refresh_ready(late), "a silent shell must eventually count");
}

#[test]
fn readiness_is_sticky() {
    let mut wp = fake_spare(1);
    shell_writes(&wp.data_version, &wp.term, b"PS C:\\> ");
    let t = wp.spawned_at + std::time::Duration::from_millis(10);
    wp.refresh_ready(t);
    wp.refresh_ready(t + crate::types::WARM_READY_QUIET);
    assert!(wp.ready);
    // More output afterwards (the user's shell is running something) must not
    // un-ready a spare; it is already startable.
    wp.data_version.store(99, std::sync::atomic::Ordering::Relaxed);
    assert!(wp.refresh_ready(t + crate::types::WARM_READY_QUIET + std::time::Duration::from_millis(1)));
}

// ── surge: a run of creations outruns any fixed depth ──────────────

#[test]
fn a_satisfied_claim_does_not_surge() {
    // Opening one window must not cost eight shell spawns.
    let mut pool = WarmPool::new(2);
    pool.note_claim(true);
    assert!(!pool.is_surging());
    assert_eq!(pool.effective_target(false), 2);
}

#[test]
fn one_lone_miss_does_not_surge() {
    // A cold `new-session` misses by definition: the only spare it has was
    // born moments earlier. Surging there fired eight shell spawns beside the
    // session's own starting shell and cost ~100ms of startup, measured.
    let mut pool = WarmPool::new(2);
    pool.note_claim(false);
    assert!(!pool.is_surging(), "an isolated creation is not a run of creations");
    assert_eq!(pool.effective_target(false), 2);
}

#[test]
fn a_miss_following_another_claim_surges_to_the_cap() {
    // Two claims close together, the second finding nothing ready: that is a
    // run outpacing the pool. Widening the batch is what lets a run pay ONE
    // shell startup between them all instead of one each, because the spawns
    // go out concurrently.
    let mut pool = WarmPool::new(2);
    pool.note_claim(true);      // first creation, served
    pool.note_claim(false);     // second, nothing ready
    assert!(pool.is_surging());
    assert_eq!(pool.effective_target(false), WARM_POOL_SURGE_MAX);
    assert_eq!(pool.deficit_for(pool.effective_target(false)), WARM_POOL_SURGE_MAX);
}

#[test]
fn a_surge_respects_a_deliberately_small_target() {
    // Someone who set `warm-pool-size 1` to save memory must not be handed
    // eight shells by a burst.
    let mut pool = WarmPool::new(1);
    pool.note_claim(true);
    pool.note_claim(false);
    assert_eq!(pool.effective_target(false), crate::types::WARM_POOL_SURGE_FACTOR);
}

#[test]
fn a_disabled_pool_never_surges() {
    let mut pool = WarmPool::new(0);
    pool.note_claim(true);
    pool.note_claim(false);
    assert_eq!(pool.effective_target(false), 0);
    assert_eq!(pool.deficit_for(pool.effective_target(false)), 0);
}

#[test]
fn a_standby_is_held_at_one_spare_and_never_surges() {
    // A `__warm__` helper creates no windows of its own, so spares beyond the
    // one its claimant wants first are idle memory in a process that may sit
    // around for days.
    let mut pool = WarmPool::new(5);
    assert_eq!(pool.effective_target(true), 1);
    pool.note_claim(true);
    pool.note_claim(false);
    assert_eq!(pool.effective_target(true), 1, "not even a burst deepens a standby");
}

#[test]
fn surplus_from_a_finished_surge_is_given_back() {
    // Without this a single burst would leave the pool permanently deep: eight
    // idle shells for a user who configured two.
    let mut pool = WarmPool::new(2);
    for id in 1..=6 {
        pool.push(ready_spare(id));
    }
    pool.note_claim(true);
    pool.note_claim(false);
    assert_eq!(pool.trim_surplus(), 0, "nothing is released while the surge is live");
    assert_eq!(pool.len(), 6);
    pool.end_surge_for_test();
    assert_eq!(pool.trim_surplus(), 4);
    assert_eq!(pool.len(), 2, "back to the configured depth");
}

#[test]
fn trimming_keeps_the_oldest_spares() {
    // The oldest spares are the ones whose startup is furthest along, so they
    // are the ones worth keeping; dropping them would throw away the readiness
    // the pool just spent 400ms acquiring.
    let mut pool = WarmPool::new(2);
    for id in 1..=5 {
        pool.push(ready_spare(id));
    }
    pool.end_surge_for_test();
    pool.trim_surplus();
    assert_eq!(pool.claim().0.map(|w| w.pane_id), Some(1));
    assert_eq!(pool.claim().0.map(|w| w.pane_id), Some(2));
}

// ── helper: a spare with no shell behind it ────────────────────────
//
// The pool only ever inspects `pane_id`, `rows`/`cols`, `spawned_at`, `ready`
// and the child's liveness, so a stub stands in fine.
//
// The stub must say RUNNING: claims reap dead spares (#450), so a spare that
// reported an exit would be reaped out from under the ordering assertions.
// This used to be a `cmd /c pause` process under a real pseudoconsole, which
// blocks on stdin for ever and does NOT reliably exit when the PTY master
// drops: a spare moved out of the pool by `claim()` and then dropped, or one
// alive when an assertion panics, kept running with no console host, and on
// Windows 11 the orphan surfaced as a Windows Terminal tab waiting at "Press
// any key". Six `cargo test` runs in one day left 121 of them on the desktop,
// so every test here held a guard that ended the recorded pids. A stub has no
// pid to leak and no guard to hold.
//
// `reap_dead` is deliberately not tested here: it depends on when the OS reaps
// a real child, which is a race, and it is covered end to end by
// tests/test_issue450_dead_warm_pane.ps1.
fn fake_spare(pane_id: usize) -> crate::types::WarmPane {
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
        ready: false,
        last_dv: 0,
        last_change: now,
        trace_settled: false,
        ready_via_backstop: false,
        shows_text: false,
        spawn_cwd: None,
        host_colors: None,
    }
}

/// A spare already past its shell startup, which is the only kind a caller is
/// ever handed.
fn ready_spare(pane_id: usize) -> crate::types::WarmPane {
    let mut wp = fake_spare(pane_id);
    wp.ready = true;
    // Settled: its prompt has been up for a while.
    wp.shows_text = true;
    wp.last_change = wp.spawned_at - std::time::Duration::from_secs(1);
    wp
}

// ── boot hold: background spawns wait for shells somebody waits on ──

type Dv = std::sync::Arc<std::sync::atomic::AtomicU64>;
type Term = std::sync::Arc<std::sync::Mutex<vt100::Parser>>;

fn held_shell() -> (Dv, Term) {
    (
        std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        std::sync::Arc::new(std::sync::Mutex::new(vt100::Parser::new(30, 120, 0))),
    )
}

/// What the pane reader does: feed the parser, then bump the version.
fn shell_writes(dv: &Dv, term: &Term, bytes: &[u8]) {
    term.lock().unwrap().process(bytes);
    dv.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn ms(t0: std::time::Instant, n: u64) -> std::time::Instant {
    t0 + std::time::Duration::from_millis(n)
}

#[test]
fn boot_hold_releases_at_the_first_visible_text() {
    // ConPTY writes its own startup sequences first; they draw nothing, so
    // the shell is still booting. The prompt is the first visible text.
    let t0 = std::time::Instant::now();
    let mut h = crate::types::BootHold::new(t0);
    let (dv, term) = held_shell();
    h.watch(1, dv.clone(), term.clone(), t0);
    assert!(!h.released(ms(t0, 50)), "no output yet");
    shell_writes(&dv, &term, b"\x1b[?9001h\x1b[?1004h\x1b[H\x1b[2J");
    assert!(!h.released(ms(t0, 60)), "ConPTY setup is not a started shell");
    assert!(!h.released(ms(t0, 200)), "and quiet after it is not either, under the quiet window");
    shell_writes(&dv, &term, b"PS C:\\> ");
    assert!(h.released(ms(t0, 210)), "the prompt releases at once, no quiet window");
}

#[test]
fn boot_hold_waits_for_a_window_created_during_it() {
    // The first new-window of a launch arrives while the hold is on, finds
    // the pool empty and cold spawns. Releasing the pool beside that spawn
    // made the first window 1.7 to 1.9 s; the hold keeps watching it.
    let t0 = std::time::Instant::now();
    let mut h = crate::types::BootHold::new(t0);
    let (dv1, term1) = held_shell();
    h.watch(1, dv1.clone(), term1.clone(), t0);
    let (dv2, term2) = held_shell();
    assert!(h.watch(2, dv2.clone(), term2.clone(), ms(t0, 100)));
    assert!(!h.watch(2, dv2.clone(), term2.clone(), ms(t0, 110)), "a pane is watched once");
    shell_writes(&dv1, &term1, b"PS C:\\> ");
    assert!(!h.released(ms(t0, 400)), "the first shell is up, the window's is not");
    shell_writes(&dv2, &term2, b"PS C:\\> ");
    assert!(h.released(ms(t0, 700)), "both up");
}

#[test]
fn boot_hold_counts_a_quiet_shell_that_draws_nothing() {
    let t0 = std::time::Instant::now();
    let mut h = crate::types::BootHold::new(t0);
    let (dv, term) = held_shell();
    h.watch(1, dv.clone(), term.clone(), t0);
    shell_writes(&dv, &term, b"\x1b[H\x1b[2J");
    assert!(!h.released(ms(t0, 10)));
    assert!(!h.released(ms(t0, 10) + crate::types::WARM_READY_QUIET - std::time::Duration::from_millis(1)));
    assert!(h.released(ms(t0, 10) + crate::types::WARM_READY_QUIET), "written, then quiet");
}

#[test]
fn boot_hold_has_a_backstop_from_the_last_watched_pane() {
    // A shell that never draws and never stops writing must not keep the
    // pool and the standby away for ever; the backstop restarts when a new
    // pane joins, since somebody is now waiting on that one.
    let t0 = std::time::Instant::now();
    let mut h = crate::types::BootHold::new(t0);
    let (dv1, term1) = held_shell();
    h.watch(1, dv1.clone(), term1.clone(), t0);
    let (dv2, term2) = held_shell();
    h.watch(2, dv2, term2, ms(t0, 1000));
    let mut t = t0;
    while t < ms(t0, 1000) + crate::types::WARM_READY_MAX_WAIT {
        shell_writes(&dv1, &term1, b"\x1b[H");
        assert!(!h.released(t), "chatty and invisible: held until the backstop");
        t += std::time::Duration::from_millis(50);
    }
    assert!(h.released(ms(t0, 1000) + crate::types::WARM_READY_MAX_WAIT));
}

#[test]
fn boot_hold_release_trickles_the_pool_one_spare_at_a_time() {
    // Right after the hold, the next creation will wait on whatever spare is
    // booting. Two spares and the standby booting together made that wait the
    // slowest part of the first window, so the pool starts one at a time
    // until one is ready.
    let mut trickle = true;
    let mut pool = WarmPool::new(2);
    assert_eq!(crate::pane::trickle_deficit(&mut trickle, &pool, 2), 1, "one at a time");
    pool.inflight = 1;
    assert_eq!(crate::pane::trickle_deficit(&mut trickle, &pool, 1), 0, "one already booting");
    pool.inflight = 0;
    pool.push(fake_spare(2));
    assert_eq!(crate::pane::trickle_deficit(&mut trickle, &pool, 1), 0, "landed but still warming");
    assert!(trickle);
    pool.push(ready_spare(3));
    assert_eq!(crate::pane::trickle_deficit(&mut trickle, &pool, 1), 1, "a ready spare ends the trickle");
    assert!(!trickle);
    // And once off it stays off: the pool refills at full width again.
    let empty = WarmPool::new(2);
    assert_eq!(crate::pane::trickle_deficit(&mut trickle, &empty, 2), 2);
}

#[test]
fn forgetting_claims_keeps_the_first_new_window_out_of_a_surge() {
    // The server's own first window goes through the claim path. Counting it
    // made the user's first new-window the second claim of a "burst" and
    // fired eight shells beside it.
    let mut pool = WarmPool::new(2);
    pool.note_claim(false);
    pool.forget_claims();
    pool.note_claim(false);
    assert!(!pool.is_surging(), "one real claim is not a burst");
    pool.note_claim(false);
    assert!(pool.is_surging(), "two real claims that missed still are");
}

