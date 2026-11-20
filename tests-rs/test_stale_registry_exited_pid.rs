//! A registry whose server was just terminated must be reaped by the next
//! cleanup pass.
//!
//! Until the kernel finishes tearing a terminated process down it is still in
//! the process table under its image name and QueryFullProcessImageNameW still
//! answers for it, so `pid_anchor_verdict` (which judged liveness by name)
//! called the dead server alive and kept its `.port`/`.key`/`.sid`/`.pid`.
//! Measured 2026-11-20: after `Stop-Process -Force` plus Wait-Process, 2 of 90
//! killed servers were in that state (exit code 0xFFFFFFFF, still listed as
//! psmux) and theirs were exactly the registries one cleanup pass left behind;
//! test_startup_stale_port_tax failed on it ("2 stale .port files survived").
//!
//! The teardown window itself is not reproducible on demand, but its two
//! observable facts are: the handle is signaled, and the name lookup still says
//! psmux. A child process that has exited while we still hold its handle gives
//! the first for real; the injected name lookup supplies the second.

use super::*;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> PathBuf {
    let mut p = std::env::temp_dir();
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    p.push(format!("psmux_exited_pid_{}_{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&p);
    p
}

/// A child that has exited, with its handle still held by the returned
/// `Child`, so the pid stays openable and signaled until it is dropped.
#[cfg(windows)]
fn exited_child() -> std::process::Child {
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "exit", "0"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn cmd.exe");
    child.wait().expect("wait for cmd.exe");
    child
}

#[cfg(windows)]
fn write_registry(dir: &PathBuf, base: &str, pid: u32) -> PathBuf {
    let port = dir.join(format!("{base}.port"));
    std::fs::write(&port, "1").unwrap();
    std::fs::write(dir.join(format!("{base}.pid")), format!("{pid}")).unwrap();
    port
}

#[test]
#[cfg(windows)]
fn exited_process_is_reported_exited() {
    let child = exited_child();
    assert!(
        crate::platform::process_has_exited(child.id()),
        "an exited child whose handle we still hold must read as exited"
    );
    drop(child);
}

#[test]
#[cfg(windows)]
fn running_process_is_not_reported_exited() {
    assert!(!crate::platform::process_has_exited(std::process::id()));
}

#[test]
#[cfg(windows)]
fn unopenable_pid_is_not_reported_exited() {
    // Not a pid anyone has: OpenProcess fails, which must mean "unknown", not
    // "dead" (#650: a live server in another session refuses the handle too).
    assert!(!crate::platform::process_has_exited(0xFFFF_FFF0));
}

#[test]
#[cfg(windows)]
fn terminated_server_still_named_psmux_is_dead() {
    let child = exited_child();
    let dir = temp_dir();
    let port = write_registry(&dir, "torn_down", child.id());
    // The teardown window: the name lookup still finds a psmux image.
    let verdict = pid_anchor_verdict_with(&port, |_| Some("psmux".to_string()));
    assert_eq!(
        verdict,
        Some(false),
        "a server whose process object is already signaled is dead even while \
         its image name is still listed"
    );
    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(windows)]
fn live_server_named_psmux_is_still_alive() {
    // Our own process is running and was created before the .pid file was
    // written, so with the same injected name it must stay alive.
    let dir = temp_dir();
    let port = write_registry(&dir, "live", std::process::id());
    let verdict = pid_anchor_verdict_with(&port, |_| Some("psmux".to_string()));
    assert_eq!(verdict, Some(true));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(windows)]
fn cleanup_pass_reaps_a_terminated_server_registry() {
    let child = exited_child();
    let dir = temp_dir();
    let port = write_registry(&dir, "dead_one", child.id());
    std::fs::write(dir.join("dead_one.key"), "k").unwrap();
    std::fs::write(dir.join("dead_one.sid"), "7").unwrap();
    // The network probe must never be reached for a pid-anchored entry.
    cleanup_stale_port_files_in_with(&dir, |_, _| panic!("probe must not run"));
    assert!(!port.exists(), ".port of a terminated server survived cleanup");
    assert!(!dir.join("dead_one.pid").exists());
    assert!(!dir.join("dead_one.key").exists());
    assert!(!dir.join("dead_one.sid").exists());
    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
}
