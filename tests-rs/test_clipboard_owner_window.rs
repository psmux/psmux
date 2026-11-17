// psmux opens the clipboard with a window, so another process cannot share it.
//
// Full sweep 2026-11-16_19-30-29: set-buffer -w warned "could not write the
// system clipboard: Windows refused the text" and left the old text; paste and
// refresh-client -l (test_issue428_busy_clipboard_paste,
// test_refresh_client_l_server_route) stored nothing.  OpenClipboard(NULL)
// does not lock other processes out: a process that also opens with no
// window (pwsh Get-Clipboard is one) gets the clipboard at the same time, and
// its CloseClipboard closes it under psmux, whose EmptyClipboard then fails
// (ERROR_CLIPBOARD_NOT_OPEN) or whose GetClipboardData returns nothing.  With
// a pwsh Get-Clipboard loop running, master lost 67 of 200 set-buffer -w
// writes and 21 of 100 refresh-client -l reads.
//
// These tests run that other process for real: a child PowerShell opens and
// closes the clipboard with no window in a tight loop while this process
// writes and reads it.

use super::*;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// A child process that opens the clipboard with no window and closes it
/// again, over and over, until `stop` exists (or 60 s pass).
#[cfg(windows)]
struct NoWindowOpener {
    child: Child,
    stop: PathBuf,
    dir: PathBuf,
}

#[cfg(windows)]
impl NoWindowOpener {
    fn start(tag: &str) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!("psmux_clipown_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        let ready = dir.join("ready");
        let stop = dir.join("stop");
        let script = format!(
            "Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class C {{ \
             [DllImport(\"user32.dll\")] public static extern bool OpenClipboard(IntPtr h); \
             [DllImport(\"user32.dll\")] public static extern bool CloseClipboard(); \
             [DllImport(\"user32.dll\")] public static extern IntPtr GetClipboardData(uint f); \
             public static int Spin(string stop, int ms) {{ int n = 0; var end = DateTime.Now.AddMilliseconds(ms); \
             while (DateTime.Now < end && !System.IO.File.Exists(stop)) {{ \
             if (OpenClipboard(IntPtr.Zero)) {{ n++; GetClipboardData(13); CloseClipboard(); }} }} return n; }} }}'; \
             Set-Content -LiteralPath '{}' 'go'; [C]::Spin('{}', 60000)",
            ready.display(),
            stop.display()
        );
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut me = NoWindowOpener { child, stop, dir };
        let t = std::time::Instant::now();
        while !ready.exists() {
            if t.elapsed() > Duration::from_secs(30) {
                me.finish();
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Let the loop get going.
        std::thread::sleep(Duration::from_millis(200));
        Some(me)
    }

    fn finish(&mut self) {
        let _ = std::fs::write(&self.stop, "stop");
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(windows)]
impl Drop for NoWindowOpener {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Every write either lands or reports a busy clipboard (which the waiting
/// callers retry); none is refused half way because another process closed
/// the clipboard under it.
#[cfg(windows)]
#[test]
fn writes_are_not_broken_by_a_process_that_opens_the_clipboard_with_no_window() {
    let _g = TEST_CLIPBOARD_CONTENTS.lock().unwrap_or_else(|e| e.into_inner());
    let before = read_from_system_clipboard_waiting(Duration::from_secs(2));
    let Some(mut opener) = NoWindowOpener::start("w") else {
        eprintln!("skipped: could not start the child PowerShell");
        return;
    };
    let mut refused = 0;
    let mut stored = 0;
    for i in 0..200 {
        match copy_to_system_clipboard_waiting(&format!("PSMUX_OWNWIN_W{i}"), Duration::from_secs(1)) {
            Ok(true) => stored += 1,
            Ok(false) => refused += 1,
            Err(ClipboardBusy) => {}
        }
    }
    opener.finish();
    if let Some(prev) = before {
        let _ = copy_to_system_clipboard_waiting(&prev, Duration::from_secs(2));
    }
    assert_eq!(refused, 0, "{refused} of 200 writes were refused ({stored} stored)");
    assert!(stored > 150, "only {stored} of 200 writes got the clipboard");
}

/// Every read that opens the clipboard sees the text on it; a read never
/// answers "no text" because another process closed the clipboard under it.
#[cfg(windows)]
#[test]
fn reads_are_not_emptied_by_a_process_that_opens_the_clipboard_with_no_window() {
    let _g = TEST_CLIPBOARD_CONTENTS.lock().unwrap_or_else(|e| e.into_inner());
    let before = read_from_system_clipboard_waiting(Duration::from_secs(2));
    let want = "PSMUX_OWNWIN_READ";
    if copy_to_system_clipboard_waiting(want, Duration::from_secs(2)) != Ok(true) {
        eprintln!("skipped: could not seed the clipboard");
        return;
    }
    let Some(mut opener) = NoWindowOpener::start("r") else {
        eprintln!("skipped: could not start the child PowerShell");
        return;
    };
    let mut wrong = Vec::new();
    let mut read = 0;
    for _ in 0..400 {
        match try_read_from_system_clipboard() {
            Ok(Some(t)) if t == want => read += 1,
            Ok(other) => wrong.push(other),
            Err(ClipboardBusy) => {}
        }
    }
    opener.finish();
    if let Some(prev) = before {
        let _ = copy_to_system_clipboard_waiting(&prev, Duration::from_secs(2));
    }
    assert!(wrong.is_empty(), "{} of 400 reads saw {:?} ({read} read the text)", wrong.len(), wrong.first());
    assert!(read > 100, "only {read} of 400 reads got the clipboard");
}

/// The text stays on the clipboard after the window psmux opened it with is
/// destroyed.
#[cfg(windows)]
#[test]
fn the_text_outlives_the_owner_window() {
    let _g = TEST_CLIPBOARD_CONTENTS.lock().unwrap_or_else(|e| e.into_inner());
    let before = read_from_system_clipboard_waiting(Duration::from_secs(2));
    let wrote = copy_to_system_clipboard_waiting("PSMUX_OWNWIN_KEEP", Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(100));
    let back = read_from_system_clipboard_waiting(Duration::from_secs(2));
    if let Some(prev) = before {
        let _ = copy_to_system_clipboard_waiting(&prev, Duration::from_secs(2));
    }
    assert_eq!(wrote, Ok(true));
    assert_eq!(back.as_deref(), Some("PSMUX_OWNWIN_KEEP"));
}
