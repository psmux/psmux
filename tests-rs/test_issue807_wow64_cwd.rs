// `#{pane_current_path}` for a 32-bit (WOW64) foreground process (issue #807).
//
// A WOW64 process has a 64-bit and a 32-bit PEB, and only the 32-bit one
// tracks its current directory; the 64-bit copy reads `C:\Windows\`. Every
// scoop shim is x86, so any program launched through `~/scoop/shims` made the
// pane report `C:\Windows`.
//
// The 64-bit copy is only overwritten once WOW64 has initialised: read straight
// after CreateProcess it still holds the directory the process was started
// with, so the old code passed a naive test. These tests start a real cmd.exe
// (no psmux server) parked on `/k` with a piped stdin, wait for its prompt,
// which the 32-bit image only prints once WOW64 is up, read its cwd through
// `get_process_cwd`, and kill it.

use super::*;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct Parked(Child);

impl Drop for Parked {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unique_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("psmux_i807_{}_{}", tag, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn park_cmd(exe: &Path, cwd: &Path) -> Parked {
    let child = Command::new(exe)
        .args(["/d", "/k"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn {}: {}", exe.display(), e));
    let mut parked = Parked(child);
    let mut stdout = parked.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut first = [0u8; 1];
        let _ = tx.send(stdout.read_exact(&mut first));
    });
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(Ok(())) => parked,
        Ok(Err(e)) => panic!("{} printed nothing: {}", exe.display(), e),
        Err(_) => panic!("{} printed no prompt within 30s", exe.display()),
    }
}

fn assert_cwd(exe: &Path, tag: &str) {
    let dir = unique_dir(tag);
    let parked = park_cmd(exe, &dir);
    let got = get_process_cwd(parked.0.id());
    drop(parked);
    let _ = std::fs::remove_dir(&dir);

    let want = dir.to_string_lossy();
    let got = got.unwrap_or_else(|| panic!("no cwd read for {}", exe.display()));
    assert!(
        crate::util::same_dir(&got, &want),
        "{} started in {:?} reported cwd {:?}",
        exe.display(),
        want,
        got
    );
}

fn system_root() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
}

#[test]
fn wow64_process_reports_its_own_cwd_not_windows() {
    let exe = system_root().join("SysWOW64").join("cmd.exe");
    if !exe.exists() {
        eprintln!("skipping: {} not present (no WOW64 on this machine)", exe.display());
        return;
    }
    assert_cwd(&exe, "wow64");
}

#[test]
fn native_process_still_reports_its_cwd() {
    assert_cwd(&system_root().join("System32").join("cmd.exe"), "native");
}
