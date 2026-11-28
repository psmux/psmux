// Regression tests for the stale-port startup tax.
//
// Root cause proven by measurement: every CLI invocation ran
// cleanup_stale_port_files(), which TCP-probed each .port file serially
// (3 attempts x 100ms connect timeout). On Windows hosts where a dead
// loopback port never sends RST (stealth firewall), each probe attempt
// burned its full connect timeout AND classified as Inconclusive, so the
// stale files were never reaped: ~350-400ms per stale file on EVERY psmux
// command, forever. Six stale files made `psmux new-session` take ~2.4s
// (and cold start ~4.9s, since the spawned server pays the tax again).
//
// The fix consults the .pid sentinel (issue #448) first: a dead PID is a
// definitive, microsecond-cheap stale verdict with no network round-trip.

use super::*;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_psmux_dir(test_name: &str) -> PathBuf {
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir()
        .join(format!("psmux_{test_name}_{}_{}", std::process::id(), n))
        .join(".psmux");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_registry(dir: &std::path::Path, session: &str, port: &str) -> (PathBuf, PathBuf, PathBuf) {
    let port_path = dir.join(format!("{session}.port"));
    let key_path = dir.join(format!("{session}.key"));
    let sid_path = dir.join(format!("{session}.sid"));
    fs::write(&port_path, port).unwrap();
    fs::write(&key_path, "test-key").unwrap();
    fs::write(&sid_path, "1").unwrap();
    (port_path, key_path, sid_path)
}

/// Spawn a short-lived real process and return its PID after it has exited.
/// This produces a PID that is genuinely dead (not fabricated), matching what
/// a hard-killed session server leaves behind in its .pid file.
#[cfg(windows)]
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("cmd")
        .args(["/c", "exit"])
        .spawn()
        .expect("spawn cmd");
    let pid = child.id();
    let _ = child.wait();
    pid
}

#[cfg(windows)]
#[test]
fn dead_pid_anchor_reaps_registry_without_any_network_probe() {
    let dir = temp_psmux_dir("pid_anchor_dead");
    let (port_path, key_path, sid_path) = write_registry(&dir, "crashed", "54329");
    let pid_path = dir.join("crashed.pid");
    fs::write(&pid_path, dead_pid().to_string()).unwrap();

    // The probe must never run: a dead .pid anchor is definitive on its own.
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        panic!("network probe must not run when the .pid anchor says dead");
    });

    assert!(!port_path.exists(), ".port of a dead-PID session must be reaped");
    assert!(!key_path.exists(), ".key of a dead-PID session must be reaped");
    assert!(!sid_path.exists(), ".sid of a dead-PID session must be reaped");
    assert!(!pid_path.exists(), ".pid sentinel itself must be reaped");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[cfg(windows)]
#[test]
fn pid_recycled_by_unrelated_image_is_reaped_without_probe() {
    // The current test process is alive but its image is not `psmux.exe`
    // (cargo test binaries are named psmux-<hash>), which models a PID that
    // was recycled by an unrelated application after the server died.
    let dir = temp_psmux_dir("pid_anchor_recycled");
    let (port_path, _, _) = write_registry(&dir, "recycled", "54330");
    fs::write(dir.join("recycled.pid"), std::process::id().to_string()).unwrap();

    cleanup_stale_port_files_in_with(&dir, |_, _| {
        panic!("network probe must not run when the PID belongs to another image");
    });

    assert!(!port_path.exists(), "recycled-PID registry must be reaped");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn missing_pid_anchor_falls_back_to_network_probe() {
    // Pre-#448 registries have no .pid file; behavior must be unchanged:
    // the probe runs, and Inconclusive keeps the files.
    let dir = temp_psmux_dir("pid_anchor_missing");
    let (port_path, key_path, sid_path) = write_registry(&dir, "legacy", "54331");

    let mut probe_ran = false;
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        probe_ran = true;
        PortProbeResult::Inconclusive
    });

    assert!(probe_ran, "without a .pid anchor the network probe must still run");
    assert!(port_path.exists(), "inconclusive probe must keep .port");
    assert!(key_path.exists(), "inconclusive probe must keep .key");
    assert!(sid_path.exists(), "inconclusive probe must keep .sid");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

#[test]
fn unparseable_pid_anchor_falls_back_to_network_probe() {
    let dir = temp_psmux_dir("pid_anchor_garbage");
    let (port_path, _, _) = write_registry(&dir, "garbled", "54332");
    fs::write(dir.join("garbled.pid"), "not-a-pid").unwrap();

    let mut probe_ran = false;
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        probe_ran = true;
        PortProbeResult::Inconclusive
    });

    assert!(probe_ran, "garbage .pid must fall back to the network probe");
    assert!(port_path.exists(), "inconclusive fallback must keep files");
    let _ = fs::remove_dir_all(dir.parent().unwrap());
}

/// Writes `STALE_COUNT` registries (with a `.pid` holding `pid` when given),
/// runs one cleanup with a probe that waits `probe_cost` and answers Stale,
/// and returns how long the cleanup took and how many probes it made. Every
/// registry must be reaped either way.
#[cfg(windows)]
fn timed_cleanup(pid: Option<u32>, probe_cost: Duration) -> (Duration, usize) {
    const STALE_COUNT: usize = 6;
    let dir = temp_psmux_dir("pid_anchor_speed");
    for i in 0..STALE_COUNT {
        write_registry(&dir, &format!("dead{i}"), &format!("5430{i}"));
        if let Some(pid) = pid {
            fs::write(dir.join(format!("dead{i}.pid")), pid.to_string()).unwrap();
        }
    }

    let mut probes = 0usize;
    let start = Instant::now();
    cleanup_stale_port_files_in_with(&dir, |_, _| {
        probes += 1;
        if !probe_cost.is_zero() {
            std::thread::sleep(probe_cost);
        }
        PortProbeResult::Stale
    });
    let elapsed = start.elapsed();

    for i in 0..STALE_COUNT {
        assert!(!dir.join(format!("dead{i}.port")).exists(), "dead{i}.port must be reaped");
    }
    let _ = fs::remove_dir_all(dir.parent().unwrap());
    (elapsed, probes)
}

#[cfg(windows)]
#[test]
fn cleanup_with_many_dead_pid_registries_is_fast() {
    // The measured defect: 6 stale files cost ~2.4s, because each one paid a
    // serial network probe (3 attempts x 100ms connect timeout) even though
    // its server was dead. With the PID anchor a dead registry is reaped
    // without any probe.
    //
    // 1. The deterministic guard: a dead .pid anchor makes no probe at all.
    // 2. The cost guard is relative, not a fixed wall clock bound, so a busy
    //    host does not fail it (a fixed 250ms failed 20 of 20 runs with the
    //    CPU saturated, on a build where the anchor works). The dead-PID path
    //    is compared with a baseline measured in this same process: the same
    //    six registries with no .pid, reaped by an instant Stale probe (the
    //    file work), plus six of the anchor's own pid lookups. That is all a
    //    dead registry should cost, with no network. Best of several runs
    //    each, so a scheduling stall in one run does not decide it.
    // 3. The budget must still catch the original slow path under the load
    //    of this run: the same six registries with a probe that costs what
    //    one real dead-port probe cost (3 x 100ms) must blow the budget.
    const TRIALS: usize = 5;
    let dead = dead_pid();

    let mut dead_best = Duration::MAX;
    for _ in 0..TRIALS {
        let (elapsed, probes) = timed_cleanup(Some(dead), Duration::ZERO);
        assert_eq!(probes, 0, "a dead .pid anchor must reap without a network probe; the stale-port tax is back");
        dead_best = dead_best.min(elapsed);
    }

    let mut files_best = Duration::MAX;
    for _ in 0..TRIALS {
        let (elapsed, probes) = timed_cleanup(None, Duration::ZERO);
        assert_eq!(probes, 6, "without a .pid anchor every registry is probed");
        files_best = files_best.min(elapsed);
    }

    // The anchor's own price for a dead PID, paid once per registry: the
    // handle check, then the process table snapshot that #650 needs when the
    // handle cannot be opened. It grows with the number of processes on the
    // host, which is why it is measured here and not assumed.
    let mut lookups_best = Duration::MAX;
    for _ in 0..TRIALS {
        let start = Instant::now();
        for _ in 0..6 {
            let _ = crate::platform::process_has_exited(dead);
            let _ = crate::platform::process_info::get_process_name_or_snapshot(dead);
        }
        lookups_best = lookups_best.min(start.elapsed());
    }

    let baseline = files_best + lookups_best;
    let budget = baseline * 2 + Duration::from_millis(100);
    assert!(
        dead_best <= budget,
        "cleanup of 6 dead registries took {dead_best:?} at best, over the {budget:?} budget \
         (file work {files_best:?} + pid lookups {lookups_best:?}); the stale-port tax is back"
    );

    let (slow, _) = timed_cleanup(None, Duration::from_millis(300));
    eprintln!(
        "stale-port tax: dead {dead_best:?}, files {files_best:?}, lookups {lookups_best:?}, \
         budget {budget:?}, old slow path {slow:?}"
    );
    assert!(
        slow > budget,
        "the budget {budget:?} no longer catches the original slow path ({slow:?} for 6 \
         probed registries); the host is too loaded for this check to mean anything"
    );
}

#[test]
fn filetime_conversion_is_monotonic_and_anchored() {
    // 1601->1970 offset must be present and ordering preserved.
    let now = std::time::SystemTime::now();
    let later = now + Duration::from_secs(10);
    let a = system_time_to_filetime_ticks(now).unwrap();
    let b = system_time_to_filetime_ticks(later).unwrap();
    assert!(b > a, "later SystemTime must map to larger FILETIME ticks");
    assert_eq!(b - a, 10 * 10_000_000, "10s must be exactly 10^8 ticks");
    assert!(a > 116_444_736_000_000_000, "ticks must include the 1601 epoch offset");
}
