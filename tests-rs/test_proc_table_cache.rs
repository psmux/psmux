//! The render path must not walk the whole process table per frame.
//!
//! `#{pane_current_command}` and `#{pane_current_path}` resolve a pane's
//! foreground process, and on Windows that means
//! `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` — an enumeration of every
//! process on the machine (~340 on a normal desktop). Both variables are
//! expanded on the server's per-output render path, so a status bar or window
//! title referencing them took two full system walks per repaint, on the same
//! thread that delivers keystrokes to ConPTY.
//!
//! These tests assert the property that matters: the number of real walks is
//! bounded by time, not by how many times the render path asks. They count
//! actual enumerations via the thread-local `PROC_TABLE_WALKS` rather than
//! timing anything, so they do not get flaky on a loaded machine.
//!
//! Concurrency note: the *cache* is process-wide, and several other test
//! modules reach it through `#{pane_current_command}`. So a foreign thread can
//! populate the cache and turn one of our expected walks into a hit — it can
//! never cause an extra walk on our thread, because the counter is
//! thread-local. Assertions are written as upper bounds wherever a foreign
//! cache fill is possible, and as exact counts only where it is not.
//!
//! The freshness split is deliberate and is also pinned here: render-path
//! callers reuse a recent table, while the Ctrl+C and mouse-injection routers
//! always enumerate fresh, because serving them a stale process tree would
//! misroute a real keypress or click.

use super::*;
use std::time::Duration;

/// Serialise this module's own tests: they share the process-wide cache slot
/// and each starts by invalidating it.
static CACHE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    CACHE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn walks() -> u64 {
    PROC_TABLE_WALKS.with(|c| c.get())
}

/// Force the next render-path call to miss the cache, so a test starts from a
/// known state regardless of what ran before it. Recovers a poisoned lock (same
/// as `lock()` below) so an earlier panicking test cannot leave a populated
/// cache behind and make these order-dependent.
fn invalidate() {
    let mut g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *g = None;
}

#[test]
fn render_path_calls_share_one_walk_within_the_ttl() {
    let _g = lock();
    invalidate();
    let before = walks();

    // 200 expansions, as a fast-drawing pane would produce.
    for _ in 0..200 {
        let _ = process_table(RENDER_PATH_TTL);
    }

    let taken = walks() - before;
    assert!(
        taken <= 1,
        "200 render-path lookups inside one TTL took {} process walks; they \
         must share at most one — the snapshot cache is not being consulted",
        taken
    );
}

#[test]
fn pane_current_command_and_path_share_a_walk() {
    // The concrete pairing from the reported bug: a status-right with
    // #{pane_current_path} plus a set-titles-string with
    // #{pane_current_command}, expanded in the same frame, used to cost two
    // full walks. They must now cost at most one.
    let _g = lock();
    invalidate();
    let before = walks();

    let _ = get_foreground_process_name(std::process::id());
    let _ = get_foreground_cwd(std::process::id());

    let taken = walks() - before;
    assert!(
        taken <= 1,
        "pane_current_command + pane_current_path in one frame took {} process \
         walks; they must share one",
        taken
    );
}

#[test]
fn an_expired_entry_is_re_walked_but_not_on_the_caller() {
    // A cache that never expires would pass every other test here and silently
    // freeze the window title on whatever was running when the pane opened, so
    // expiry must still trigger a real walk.
    //
    // It must not be this thread's walk. A render-path caller is the server's
    // event loop, the same thread that writes keystrokes into ConPTY, and the
    // walk costs 9-11ms on a normal desktop. Paying it inline stalled the
    // keystroke path once a second per pane. The refresh therefore runs on a
    // background thread and the caller is served the entry it already had.
    let _g = lock();
    let short = Duration::from_millis(1);
    let _ = process_table(Duration::ZERO); // seed a known-fresh entry
    let seeded_at = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(30));

    let before = walks();
    let served = process_table(short).expect("a stale entry is still served");
    assert_eq!(
        walks() - before,
        0,
        "an expired entry must be refreshed off the calling thread; this thread \
         walked {} times and so would stall a frame",
        walks() - before
    );
    assert!(
        served.len() > 10,
        "the stale entry served while the refresh runs must still be the real \
         table; got {} entries",
        served.len()
    );

    // And the refresh must actually land. Generous deadline: this waits on a
    // thread spawn plus a whole-system enumeration.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut refreshed = false;
    while std::time::Instant::now() < deadline {
        let at = {
            let g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            g.as_ref().map(|(at, _)| *at)
        };
        if at.is_some_and(|at| at > seeded_at) {
            refreshed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        refreshed,
        "the background refresh never replaced the expired entry — the window \
         title would freeze on whatever was running when the pane opened"
    );
}

#[test]
fn a_very_old_entry_is_still_served_without_a_caller_walk() {
    // The tempting rule is "past some age, walk inline after all". It is wrong,
    // and it is wrong in the most visible place: the oldest entry is the one
    // found by the first keystroke after a pause, which is exactly the keystroke
    // whose latency a user notices. An earlier revision of this code bounded the
    // staleness at 2s and the measured result was a single 17ms stall on the
    // first character typed after an idle window.
    //
    // So age must never promote the caller to a walker. It only decides whether
    // a refresh is kicked off behind the answer.
    let _g = lock();
    {
        let mut g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let ancient = std::time::Instant::now() - Duration::from_secs(600);
        let table = std::sync::Arc::new(ProcSnapshot::from(vec![(1u32, 0u32, "ancient.exe".to_string())]));
        *g = Some((ancient, table));
    }
    let before = walks();
    let served = process_table(RENDER_PATH_TTL).expect("a cached entry is always served");
    assert_eq!(
        walks() - before,
        0,
        "a ten minute old entry still must not make the caller walk; it did, and \
         the first keystroke after an idle pause pays for it"
    );
    assert_eq!(
        served.len(),
        1,
        "the caller should have been handed the entry that was cached, not a \
         fresh walk"
    );

    // The refresh still has to land, or the table would be frozen.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut fresh = false;
    while std::time::Instant::now() < deadline {
        let n = {
            let g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
            g.as_ref().map(|(_, t)| t.len()).unwrap_or(0)
        };
        if n > 10 {
            fresh = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        fresh,
        "the background refresh never replaced the ancient entry, so the table \
         is now frozen"
    );
}

#[test]
fn a_cold_cache_walks_inline() {
    // With nothing cached there is nothing to serve, so the caller must walk
    // rather than hand back an empty table and make every format expand to
    // nothing on the first frame.
    //
    // The cache is global, so a background refresh another test started can
    // refill it between invalidate() and the call (CI saw 0 walks once): that
    // call was warm, not cold. Retry until a call really starts cold. Every
    // call, warm or cold, must hand back a full table.
    let _g = lock();
    let mut cold_walked = false;
    for _ in 0..20 {
        invalidate();
        let before = walks();
        let served = process_table(RENDER_PATH_TTL).expect("snapshot should succeed");
        let taken = walks() - before;
        assert!(taken <= 1, "one lookup took {} walks", taken);
        assert!(
            served.len() > 10,
            "a lookup after invalidate returned only {} entries",
            served.len()
        );
        if taken == 1 {
            cold_walked = true;
            break;
        }
    }
    assert!(cold_walked, "a cold cache must be filled inline");
}

#[test]
fn the_render_path_ttl_is_bounded_and_non_zero() {
    // Zero would silently disable the cache and restore the per-frame walk;
    // an over-long bound would freeze the window title. Neither is a change
    // anyone should make without noticing.
    assert!(
        !RENDER_PATH_TTL.is_zero(),
        "RENDER_PATH_TTL of zero disables the cache — that is the bug, restored"
    );
    assert!(
        RENDER_PATH_TTL <= Duration::from_millis(500),
        "RENDER_PATH_TTL of {:?} is long enough to make the window title look \
         stuck",
        RENDER_PATH_TTL
    );
}

#[test]
fn zero_max_age_always_walks() {
    // Ctrl+C routing (foreground_is_shell) and mouse-transport selection
    // (has_vt_bridge_descendant) pass Duration::ZERO. They fire on deliberate
    // user input, not per frame, and must never act on a stale process tree.
    // A foreign thread cannot suppress these, so the count is exact.
    let _g = lock();
    let before = walks();

    for _ in 0..5 {
        let _ = process_table(Duration::ZERO);
    }

    let taken = walks() - before;
    assert_eq!(
        taken, 5,
        "Duration::ZERO must bypass the cache every time; got {} walks for 5 \
         calls — an interrupt or click could be routed off a stale tree",
        taken
    );
}

#[test]
fn a_fresh_walk_refreshes_the_shared_cache() {
    // A ZERO-max_age caller still paid for the walk, so a render-path caller
    // immediately after should reuse it rather than walking again.
    let _g = lock();
    let _ = process_table(Duration::ZERO);
    let before = walks();
    let _ = process_table(RENDER_PATH_TTL);

    assert_eq!(
        walks() - before,
        0,
        "a render-path lookup right after a fresh walk should reuse it"
    );
}

#[test]
fn the_table_is_plausibly_populated() {
    // Guard against the caching layer succeeding at returning nothing: every
    // assertion above would still pass on a permanently empty table.
    let _g = lock();
    let table = process_table(Duration::ZERO).expect("process snapshot should succeed");
    assert!(
        table.len() > 10,
        "process table has only {} entries — enumeration is broken, and the \
         cache tests above would pass anyway",
        table.len()
    );
    let me = std::process::id();
    assert!(
        table.iter().any(|(pid, _, _)| *pid == me),
        "the test process itself should appear in its own process table"
    );
}

#[test]
fn foreground_is_shell_classifies_live_processes() {
    use std::process::{Command, Stdio};

    let _g = lock();
    let classify = |pid, expected| {
        // Generous on purpose: this waits on the OS to spawn a process and
        // publish it in the process table, which has no bound under load. The
        // loop breaks the instant the verdict is right, so a healthy machine
        // never pays for the headroom; a loaded one stops reporting a product
        // failure it does not have. Seen at 3s while a full sweep was running.
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut verdict = None;
        loop {
            let present = process_table(Duration::ZERO)
                .is_some_and(|table| table.iter().any(|(entry_pid, _, _)| *entry_pid == pid));
            if present {
                verdict = foreground_is_shell(pid);
                if verdict == Some(expected) {
                    break verdict;
                }
            }
            if std::time::Instant::now() >= deadline {
                break verdict;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    };

    let mut shell = Command::new("cmd.exe")
        .args(["/D", "/Q", "/K"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd");
    let shell_verdict = classify(shell.id(), true);
    let _ = shell.kill();
    let _ = shell.wait();
    assert_eq!(shell_verdict, Some(true), "cmd must classify as a shell");

    let mut ping = Command::new("ping.exe")
        .args(["-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ping");
    let ping_verdict = classify(ping.id(), false);
    let _ = ping.kill();
    let _ = ping.wait();
    assert_eq!(
        ping_verdict,
        Some(false),
        "ping must classify as a non-shell"
    );
}

// ---------------------------------------------------------------------------
// The other half of the policy: an EXPLICIT QUERY must not be one refresh
// behind.
//
// `#{pane_current_command}` is expanded both on the render path and as the
// whole answer to a one-shot `display-message -p` / `list-panes -F`. Serving
// the one-shot route a stale table made it report the PREVIOUS foreground
// process: `pwsh` two seconds after a command started, and the command two
// seconds after it exited, with a second query 300ms later always correct.
// tmux reads the tty's foreground process group at query time and is never
// stale, so the query route walks inline when the entry has expired.
// ---------------------------------------------------------------------------

#[test]
fn an_explicit_query_walks_inline_when_the_entry_expired() {
    let _g = lock();
    {
        let mut g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let ancient = std::time::Instant::now() - Duration::from_secs(600);
        let table = std::sync::Arc::new(ProcSnapshot::from(vec![(1u32, 0u32, "ancient.exe".to_string())]));
        *g = Some((ancient, table));
    }
    let before = walks();
    let served = process_table_bounded(RENDER_PATH_TTL)
        .expect("an explicit query must still get a table");
    assert_eq!(
        walks() - before,
        1,
        "an explicit query found an expired entry and did NOT walk; it would be \
         served the previous foreground process, which is the whole bug"
    );
    assert!(
        served.len() > 10,
        "the query was handed the ancient placeholder ({} entries) instead of a \
         fresh enumeration",
        served.len()
    );
}

#[test]
fn an_explicit_query_reuses_an_entry_inside_the_ttl() {
    // Freshness is bounded, not unconditional: a command reply may reuse a
    // snapshot younger than the TTL. That bound is what keeps the worst case at
    // one inline walk per TTL no matter how often the format is asked, so a
    // `list-panes -F '#{pane_current_command}'` in a loop cannot turn into a
    // walk per invocation.
    let _g = lock();
    let _ = process_table(Duration::ZERO); // seed a known-fresh entry
    let before = walks();
    let _ = process_table_bounded(RENDER_PATH_TTL).expect("a fresh entry is served");
    assert_eq!(
        walks() - before,
        0,
        "an entry younger than the TTL must be reused by the query route too"
    );
}

#[test]
fn the_render_path_still_never_walks_on_the_caller() {
    // The companion assertion to the two above, stated against the pair so a
    // future "just make them both fresh" simplification fails here: adding the
    // inline walk back to the render path is the keystroke-latency regression
    // this cache exists to prevent.
    let _g = lock();
    {
        let mut g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let ancient = std::time::Instant::now() - Duration::from_secs(600);
        let table = std::sync::Arc::new(ProcSnapshot::from(vec![(1u32, 0u32, "ancient.exe".to_string())]));
        *g = Some((ancient, table));
    }
    let before = walks();
    let served = process_table(RENDER_PATH_TTL).expect("a cached entry is always served");
    assert_eq!(
        walks() - before,
        0,
        "the render path walked inline on an expired entry; that is the 9-11ms \
         stall on the keystroke path"
    );
    assert_eq!(
        served.len(),
        1,
        "the render path should have been served the cached entry as is"
    );
}

#[test]
fn pane_current_command_query_route_sees_a_process_the_cache_does_not() {
    // End-to-end on the real resolver that `#{pane_current_command}` calls,
    // with the staleness made deterministic instead of timing-dependent: the
    // cache is seeded with an ancient table that does NOT contain the live
    // child, so the stale route cannot possibly name it and the fresh route
    // must.
    use std::process::{Command, Stdio};

    let _g = lock();
    let mut root = Command::new("cmd.exe")
        .args(["/c", "ping.exe", "-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn cmd");
    let root_pid = root.id();

    // Wait for the descendant to exist, off a fresh enumeration.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut have_child = false;
    while std::time::Instant::now() < deadline {
        let t = process_table(Duration::ZERO).expect("snapshot");
        if t.iter().any(|(_, ppid, name)| *ppid == root_pid && name.starts_with("ping")) {
            have_child = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(have_child, "cmd never spawned its ping child; cannot run the scenario");

    // An ancient table that knows the root but none of its children.
    {
        let mut g = PROC_TABLE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let ancient = std::time::Instant::now() - Duration::from_secs(600);
        let table = std::sync::Arc::new(ProcSnapshot::from(vec![(root_pid, 0u32, "cmd.exe".to_string())]));
        *g = Some((ancient, table));
    }
    let stale = get_deepest_foreground_process_name(root_pid);
    let fresh = get_deepest_foreground_process_name_fresh(root_pid);

    let _ = root.kill();
    let _ = root.wait();

    assert_eq!(
        stale, None,
        "the render path is defined to answer off the snapshot it has; it \
         invented {stale:?} instead"
    );
    let fresh = fresh.expect(
        "the query route returned nothing while a ping child was live — it was \
         served the stale table, which is exactly the one-refresh-behind bug",
    );
    assert!(
        fresh.to_ascii_lowercase().starts_with("ping"),
        "the query route must name the live descendant; got {fresh:?}"
    );
}

/// One Ctrl+C routing decision pins a single fresh walk: the dozen live-tree
/// questions the router asks are answered from it instead of each walking the
/// system again (measured: nine walks, 94-103ms, on the keystroke thread).
#[test]
fn a_pinned_table_serves_every_zero_age_question_from_one_walk() {
    let _g = lock();
    let me = std::process::id();
    let before = walks();
    {
        let _pin = pin_fresh();
        let _ = foreground_is_shell(me);
        let _ = foreground_leaf_pid(me);
        let _ = foreground_is_vt_bridge(me);
        let _ = has_vt_bridge_descendant(me);
        let _ = foreground_fell_back_to_root(me);
        let _ = recently_started_vt_bridge(Duration::from_secs(5));
        let _ = console_has_plain_native_app(&[me]);
        let _ = console_broadcast_hits_bridge(&[me]);
        let _ = classify_console_member(me);
        // Every pid here is in the pinned table: no second walk.
        repin_unless_known(&[me]);
    }
    assert_eq!(walks() - before, 1, "one pinned walk for the whole decision");

    // The pin is gone with its guard: a ZERO caller walks live again.
    let before = walks();
    let _ = process_table(Duration::ZERO);
    assert_eq!(walks() - before, 1);
}

/// A console member the pinned table has never seen (it joined after the pin)
/// forces exactly one re-walk, so the bridge checks still see it.
#[test]
fn a_member_missing_from_the_pin_forces_one_re_walk() {
    let _g = lock();
    let before = walks();
    let _pin = pin_fresh();
    // pid 3 is never a real process id on Windows (ids are multiples of 4).
    repin_unless_known(&[std::process::id(), 3]);
    assert_eq!(walks() - before, 2);
}

/// Without a pin, `repin_unless_known` must not walk at all.
#[test]
fn repin_without_a_pin_is_a_no_op() {
    let _g = lock();
    let before = walks();
    repin_unless_known(&[3]);
    assert_eq!(walks() - before, 0);
}
