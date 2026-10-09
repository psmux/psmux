// Issue #686, half two: `kill-server` leaked the spares that were still in
// flight.
//
// The pool can only kill what it owns. A surge hands eight spawns to
// background threads, and each of those is a real `CreateProcessW` some
// milliseconds before its `WarmPane` reaches the server loop. `kill-server`
// landing in that window killed the windows and the pooled spares and then
// called `process::exit`, which stops the spawner threads where they stand:
// the shells they had just created stayed alive, parented to a dead psmux,
// each holding a conhost, idle at a prompt. Measured on the pre fix build:
// six orphan `pwsh` over ten rounds of "new-session, six new-window,
// kill-server".
//
// These tests cover the bookkeeping that fixes it: a spawn is tracked from the
// moment it is ISSUED, gains its pid as soon as `CreateProcessW` returns one,
// is released when the server takes ownership, and is drained by the teardown
// reaper. The end to end proof is tests/test_issue686_pool_surge_and_reap.ps1.

use super::inflight;

/// These tests drive process-global statics, so they must not overlap each
/// other. `cargo test` runs the test binary multi threaded.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The teardown flag these tests raise is process wide: while it is up, EVERY
/// spare spawn in the test binary is refused ("server is shutting down; spare
/// killed on arrival"). So they also hold the shared test lock that the tests
/// spawning real warm panes take (test_issue450_dead_warm_pane and friends),
/// which a parallel run otherwise hit about one run in two.
fn guard() -> (std::sync::MutexGuard<'static, ()>, std::sync::MutexGuard<'static, ()>) {
    let env = crate::util::lock_test_env();
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    inflight::reset_for_test();
    (env, g)
}

#[test]
fn an_issued_spawn_is_pending_until_it_is_released() {
    let _g = guard();
    inflight::issue(7);
    assert_eq!(inflight::pending(), vec![7], "issue must register the pane id");
    // No pid yet: the spawner has not reached CreateProcessW.
    assert!(inflight::pids().is_empty());
    inflight::release(7);
    assert!(inflight::pending().is_empty(), "release must drop the entry");
    inflight::reset_for_test();
}

#[test]
fn recording_a_pid_makes_the_spawn_reapable() {
    let _g = guard();
    inflight::issue(3);
    assert!(inflight::record_pid(3, Some(4242)));
    assert_eq!(inflight::pids(), vec![4242]);
    let reaped = inflight::reap(std::time::Duration::from_millis(50), |_| {});
    assert_eq!(reaped, vec![4242], "teardown must hand the pid to the killer");
    assert!(inflight::pending().is_empty(), "reap drains what it returns");
    inflight::reset_for_test();
}

#[test]
fn a_spare_the_server_already_owns_is_not_reaped_twice() {
    let _g = guard();
    // This is the normal life of a spare: issued, spawned, landed in AppState.
    // From the moment it lands, `WarmPool::kill_all` owns it, so the reaper
    // must not also chase its pid (the pid could be recycled by then).
    inflight::issue(11);
    assert!(inflight::record_pid(11, Some(9001)));
    inflight::release(11);
    let reaped = inflight::reap(std::time::Duration::from_millis(50), |_| {});
    assert!(reaped.is_empty(), "a landed spare belongs to the pool, not the reaper");
    inflight::reset_for_test();
}

#[test]
fn after_teardown_the_spawner_is_told_to_kill_its_own_child() {
    let _g = guard();
    inflight::issue(5);
    // Teardown runs while pane 5 is still inside CreateProcessW.
    let reaped = inflight::reap(std::time::Duration::from_millis(20), |_| {});
    assert!(reaped.is_empty(), "nothing had a pid yet");
    assert!(inflight::is_tearing_down());
    // The spawn finishes afterwards. Its pid must NOT be adopted: the answer
    // is false, which is what makes `spawn_warm_pane_from` kill the child it
    // just created instead of posting it to a loop that will never read it.
    assert!(
        !inflight::record_pid(5, Some(1234)),
        "a spawn that lands after teardown must be told to kill its own child"
    );
    // The entry stays until the spawner has actually killed its child and
    // released it, so a reaper still polling keeps waiting for that kill.
    assert_eq!(inflight::pending(), vec![5]);
    inflight::release(5);
    assert!(inflight::pending().is_empty());
    inflight::reset_for_test();
}

#[test]
fn reap_does_not_return_before_a_self_killing_spawner_has_released() {
    let _g = guard();
    // The spawner's CreateProcessW returns while the reaper is polling: it is
    // told to kill its own child. Until that kill has run, the reaper must not
    // report the registry empty, because its caller exits the process next and
    // that would end the spawner thread before the kill (#686).
    inflight::issue(31);
    let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let k = killed.clone();
    let t = std::thread::spawn(move || {
        while !inflight::is_tearing_down() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!inflight::record_pid(31, Some(888)));
        // The kill takes a while (snapshot, identity check, TerminateProcess).
        std::thread::sleep(std::time::Duration::from_millis(40));
        k.store(true, std::sync::atomic::Ordering::SeqCst);
        inflight::release(31);
    });
    let reaped = inflight::reap(std::time::Duration::from_millis(2000), std::thread::sleep);
    assert!(
        killed.load(std::sync::atomic::Ordering::SeqCst),
        "reap returned before the spawner's own kill had run"
    );
    assert!(reaped.is_empty(), "the spawner owned that kill, got {reaped:?}");
    t.join().unwrap();
    inflight::reset_for_test();
}

#[test]
fn the_teardown_budget_outlasts_a_slow_create_process() {
    // A sweep measured a median CreateProcessW of 736 ms for the Store pwsh,
    // and the old 400 ms budget (about 900 ms over both reaper passes) left
    // three suspended shells outside their jobs. The kill-server socket must
    // also stay open longer than the reaper may wait, or the caller's
    // force-kill lands inside that wait.
    assert!(super::INFLIGHT_REAP_BUDGET >= std::time::Duration::from_millis(3000));
    assert!(super::KILL_SERVER_HOLD > super::INFLIGHT_REAP_BUDGET);
}

#[test]
fn reap_waits_for_a_pid_that_is_still_on_its_way() {
    let _g = guard();
    inflight::issue(21);
    // A spawner thread still inside CreateProcessW: its pid appears a few
    // milliseconds after the teardown began. The reaper has to wait for it,
    // otherwise exactly the shell the bug is about walks free.
    let t = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(30));
        // The real spawner kills its own child when this answers false; here
        // the point is only that the pid becomes visible to the reaper before
        // it gives up, whichever of the two paths takes it.
        if inflight::record_pid(21, Some(777)) {
            // adopted, nothing more to do
        } else {
            inflight::release(21);
        }
    });
    let reaped = inflight::reap(std::time::Duration::from_millis(500), std::thread::sleep);
    t.join().unwrap();
    // Either the reaper collected the pid, or the spawner took responsibility
    // for it. What must never happen is the registry still holding a pane whose
    // shell nobody killed.
    assert!(
        reaped == vec![777] || inflight::pending().is_empty(),
        "an in flight spawn must end up either reaped or self killed, got reaped={reaped:?} pending={:?}",
        inflight::pending()
    );
    inflight::reset_for_test();
}

#[test]
fn reap_gives_up_inside_its_budget_when_a_spawn_wedges() {
    let _g = guard();
    // A shutdown must never hang on a spawner stuck inside CreateProcessW.
    inflight::issue(99);
    let t0 = std::time::Instant::now();
    let reaped = inflight::reap(std::time::Duration::from_millis(60), std::thread::sleep);
    let waited = t0.elapsed();
    assert!(reaped.is_empty());
    assert!(
        waited < std::time::Duration::from_millis(1000),
        "reap must respect its budget, waited {waited:?}"
    );
    inflight::reset_for_test();
}

#[test]
fn several_in_flight_spares_are_all_reaped() {
    let _g = guard();
    // The surge case: eight spawns issued at once, all of them with a pid by
    // the time kill-server arrives.
    for (i, pid) in (2usize..10).zip(500u32..508) {
        inflight::issue(i);
        assert!(inflight::record_pid(i, Some(pid)));
    }
    assert_eq!(inflight::pending().len(), 8);
    let mut reaped = inflight::reap(std::time::Duration::from_millis(50), |_| {});
    reaped.sort_unstable();
    assert_eq!(reaped, (500u32..508).collect::<Vec<_>>());
    assert!(inflight::pending().is_empty());
    inflight::reset_for_test();
}
