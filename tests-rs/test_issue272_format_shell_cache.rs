// Issue #272 + async #() format jobs.
//
// run_shell_command is non-blocking: it spawns the subprocess on a background
// thread, returns the last cached value (empty on the first call), and the
// server loop drains completed results to refresh the cache. These tests prove
// the async contract: the call never blocks, one worker runs per command per
// TTL / in-flight window (dup guard + cache), TTL expiry re-spawns, distinct
// commands are independent, the value reaches the caller after a drain, and the
// cached value is the stdout (not the command text).
//
// Spawn detection: the helper appends a line to a unique counter file each time
// it actually runs, so line count == real subprocess spawns, independent of
// what expand_format returns. `mock_app` preinitializes the format-job channel
// (as the server loop does) and `drain`/`wait_for_drain` replicate the loop's
// non-blocking result drain.

use super::*;
use std::time::{Duration, Instant};

/// Expand in ASYNC mode — the mode the periodic status bar uses (PR #477).
/// These tests assert the non-blocking spawn-once-per-TTL caching contract,
/// which is now opt-in; one-shot callers (display-message -p) expand `#()`
/// synchronously by default.
fn expand_async(fmt: &str, app: &AppState) -> String {
    let _g = AsyncFormatGuard::new();
    super::expand_format(fmt, app)
}

fn mock_app(interval_secs: u64) -> AppState {
    let mut app = AppState::new("issue272".to_string());
    app.window_base_index = 0;
    app.status_interval = interval_secs;
    // Preinitialize the format-job channel exactly as the server loop does, so
    // the async path can spawn workers and deliver results.
    let (tx, rx) = std::sync::mpsc::channel();
    app.format_job_tx = Some(tx);
    app.format_job_rx = Some(rx);
    app
}

fn counter_path(test_name: &str) -> std::path::PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("psmux_issue272_{}_{}_{}.count", test_name, pid, nanos))
}

/// A `#(...)` inner command that appends one line to `counter` when it runs.
/// Its stdout is empty, so `#()` expands to "".
fn tracer_cmd(counter: &std::path::Path) -> String {
    let p = counter.display().to_string().replace('\\', "/");
    format!("echo x>>{}", p)
}

/// Like `tracer_cmd` but waits ~1s first, so a synchronous spawn would visibly
/// block. `ping -n 2` is the classic cmd sleep and reads no stdin.
fn slow_tracer_cmd(counter: &std::path::Path) -> String {
    let p = counter.display().to_string().replace('\\', "/");
    if cfg!(windows) {
        format!("ping -n 2 127.0.0.1 >nul & echo x>>{}", p)
    } else {
        format!("sleep 1; echo x>>{}", p)
    }
}

fn line_count(p: &std::path::Path) -> usize {
    match std::fs::read_to_string(p) {
        Ok(s) => s.lines().count(),
        Err(_) => 0,
    }
}

/// Ceiling for "the worker has run" waits. Every wait below polls and returns
/// as soon as its condition holds, so this costs nothing when spawning is
/// quick; it only bounds how long a starved spawn may take.
///
/// It was 5 s, and that is not a property of the code under test but of the
/// harness: unit tests run in ONE process, and on Windows process creation
/// inside a process is serialized (std holds a lock across CreateProcessW).
/// In `cargo test -- shell` some twenty tests spawn PowerShell at the same
/// time, and Store PowerShell's app alias activation makes each creation
/// slow, so the worker's `cmd` spawn() itself was measured blocking 5.8 s
/// (instrumented 2026-10-09: spawn 5.83 s, the command 20 ms) while the
/// assertions here are about COUNTS and VALUES, never latency. Latency of
/// expand_format itself is still asserted, in expand_does_not_block_on_the_subprocess.
const SPAWN_WAIT: Duration = Duration::from_secs(30);
fn cleanup(p: &std::path::Path) {
    let _ = std::fs::remove_file(p);
}

/// Replicate the server loop's drain: pull completed `#()` results and refresh
/// the cache so the next `expand_format` returns the fresh value.
fn drain(app: &AppState) -> usize {
    let mut n = 0;
    if let Some(rx) = app.format_job_rx.as_ref() {
        while let Ok((cmd, output)) = rx.try_recv() {
            if let Ok(mut g) = app.format_shell_cache.lock() {
                let now = Instant::now();
                g.insert(cmd, crate::types::ShellEntry { at: now, value: output, running: false });
            }
            n += 1;
        }
    }
    n
}

fn wait_for_drain(app: &AppState, timeout: Duration) -> usize {
    let deadline = Instant::now() + timeout;
    loop {
        let n = drain(app);
        if n > 0 || Instant::now() >= deadline {
            return n;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_spawns(counter: &std::path::Path, expected: usize, timeout: Duration) -> usize {
    let deadline = Instant::now() + timeout;
    loop {
        let c = line_count(counter);
        if c >= expected || Instant::now() >= deadline {
            return c;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ───────────────────────── tests ─────────────────────────

#[test]
fn expand_does_not_block_on_the_subprocess() {
    let counter = counter_path("nonblock");
    cleanup(&counter);
    let app = mock_app(15);
    let fmt = format!("#({})", slow_tracer_cmd(&counter));

    let t0 = Instant::now();
    let out = expand_async(&fmt, &app);
    let elapsed = t0.elapsed();

    // The helper waits ~1s; a synchronous spawn would block here. The async
    // path must return immediately with empty output.
    assert!(
        elapsed < Duration::from_millis(300),
        "expand_format blocked {:?} on a ~1s helper (issue #272 residual is back)",
        elapsed
    );
    assert_eq!(out, "", "first render shows empty #() before the worker completes; got {:?}", out);

    // Let the worker finish so it doesn't outlive the test.
    wait_for_spawns(&counter, 1, SPAWN_WAIT);
    cleanup(&counter);
}

#[test]
fn one_spawn_per_window_and_value_after_drain() {
    let counter = counter_path("value");
    cleanup(&counter);
    let app = mock_app(60);
    let p = counter.display().to_string().replace('\\', "/");
    let cmd = if cfg!(windows) {
        format!("echo TOKEN-272 & echo x>>{}", p)
    } else {
        format!("echo TOKEN-272; echo x>>{}", p)
    };
    let fmt = format!("[#({})]", cmd);

    // First render: worker spawned, value not ready yet -> empty.
    assert_eq!(expand_async(&fmt, &app), "[]", "empty before the worker completes");
    // 50 rapid calls in the same window must not double-spawn (cache is fresh).
    for _ in 0..50 {
        let _ = expand_async(&fmt, &app);
    }

    let drained = wait_for_drain(&app, SPAWN_WAIT);
    assert_eq!(drained, 1, "exactly one result should drain");

    let second = expand_async(&fmt, &app);
    let third = expand_async(&fmt, &app);
    let spawns = line_count(&counter);
    cleanup(&counter);

    assert!(second.contains("TOKEN-272"), "value must reach the caller after drain; got {:?}", second);
    assert_eq!(second, third, "cached value stays stable within TTL");
    assert_eq!(spawns, 1, "one spawn total within the TTL window; got {}", spawns);
}

#[test]
fn respawns_after_ttl_expiry() {
    let counter = counter_path("ttl");
    cleanup(&counter);
    let app = mock_app(1); // TTL = 1s
    let fmt = format!("#({})", tracer_cmd(&counter));

    let _ = expand_async(&fmt, &app);
    wait_for_drain(&app, SPAWN_WAIT);
    assert_eq!(line_count(&counter), 1, "first call spawns");

    for _ in 0..10 {
        let _ = expand_async(&fmt, &app);
    }
    assert_eq!(line_count(&counter), 1, "within TTL: no respawn");

    std::thread::sleep(Duration::from_millis(1100));
    let _ = expand_async(&fmt, &app);
    let spawns = wait_for_spawns(&counter, 2, SPAWN_WAIT);
    cleanup(&counter);
    assert_eq!(spawns, 2, "respawn after TTL expiry; got {}", spawns);
}

#[test]
fn in_flight_worker_blocks_a_second_spawn_after_ttl() {
    // Dup guard: if the entry is TTL-expired but a worker is still running, do
    // NOT spawn a second one. A ~1s helper with a 1s TTL exercises the window
    // where `at` is expired but `running` is still true.
    let counter = counter_path("dupguard");
    cleanup(&counter);
    let app = mock_app(1); // TTL = 1s
    let fmt = format!("#({})", slow_tracer_cmd(&counter));

    let _ = expand_async(&fmt, &app); // spawn worker (waits ~1s)
    // Hammer just past the 1s TTL while the worker is still in flight (and we
    // never drain, so `running` stays true).
    std::thread::sleep(Duration::from_millis(1050));
    for _ in 0..10 {
        let _ = expand_async(&fmt, &app);
    }

    let spawns = wait_for_spawns(&counter, 1, SPAWN_WAIT);
    std::thread::sleep(Duration::from_millis(200));
    let after = line_count(&counter);
    cleanup(&counter);
    assert_eq!(spawns, 1, "in-flight worker present; got {} spawns", spawns);
    assert_eq!(after, 1, "no second worker while one was in flight; got {}", after);
}

#[test]
fn distinct_commands_are_independent() {
    let ca = counter_path("indep_a");
    let cb = counter_path("indep_b");
    cleanup(&ca);
    cleanup(&cb);
    let app = mock_app(15);
    let fa = format!("#({})", tracer_cmd(&ca));
    let fb = format!("#({})", tracer_cmd(&cb));

    for _ in 0..20 {
        let _ = expand_async(&fa, &app);
        let _ = expand_async(&fb, &app);
    }

    wait_for_spawns(&ca, 1, SPAWN_WAIT);
    wait_for_spawns(&cb, 1, SPAWN_WAIT);
    std::thread::sleep(Duration::from_millis(150));
    let a = line_count(&ca);
    let b = line_count(&cb);
    cleanup(&ca);
    cleanup(&cb);
    assert_eq!(a, 1, "command A spawns exactly once; got {}", a);
    assert_eq!(b, 1, "command B spawns exactly once; got {}", b);
}

#[test]
fn status_interval_zero_uses_one_second_floor() {
    let counter = counter_path("zero");
    cleanup(&counter);
    // The .max(1) floor keeps status-interval=0 from re-spawning on every push.
    let app = mock_app(0);
    let fmt = format!("#({})", tracer_cmd(&counter));

    for _ in 0..50 {
        let _ = expand_async(&fmt, &app);
    }
    wait_for_spawns(&counter, 1, SPAWN_WAIT);
    std::thread::sleep(Duration::from_millis(150));
    let spawns = line_count(&counter);
    cleanup(&counter);
    assert_eq!(spawns, 1, "interval=0 floor keeps 50 rapid calls to one spawn; got {}", spawns);
}

#[test]
fn value_is_stdout_not_command_text() {
    let counter = counter_path("noleak");
    cleanup(&counter);
    let app = mock_app(15);
    let p = counter.display().to_string().replace('\\', "/");
    let cmd = if cfg!(windows) {
        format!("echo SAFE_OUT & echo x>>{}", p)
    } else {
        format!("echo SAFE_OUT; echo x>>{}", p)
    };
    let fmt = format!("#({})", cmd);

    let _ = expand_async(&fmt, &app); // spawn
    wait_for_drain(&app, SPAWN_WAIT);
    let out = expand_async(&fmt, &app); // cached value
    cleanup(&counter);

    assert!(out.contains("SAFE_OUT"), "value should be the helper stdout; got {:?}", out);
    assert!(
        !out.contains("echo SAFE_OUT"),
        "value must be stdout, not the raw command text; got {:?}",
        out
    );
}
