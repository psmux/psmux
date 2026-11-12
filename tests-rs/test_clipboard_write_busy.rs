// set-buffer -w and load-buffer -w from the command line wrote the Windows
// clipboard with about 10 ms of retries and then gave up without a word, so a
// clipboard history listener, a VNC or RDP clipboard bridge or a phone link app
// holding the clipboard open a little longer silently left the old text in
// place (full sweeps 2026-11-11 and 11-12: test_issue298_set_buffer_clipboard
// "set-buffer -w with spaces in content: got 'SENTINEL'"). The command line
// write now waits for a busy clipboard up to CLIPBOARD_WRITE_TIMEOUT and warns
// on stderr when it still could not write.

use super::*;
use std::cell::Cell;

#[test]
fn a_busy_clipboard_is_waited_for_and_then_written() {
    let calls = Cell::new(0u32);
    let pauses = Cell::new(0u32);
    let got = retry_while_busy(
        Duration::from_secs(1),
        || {
            calls.set(calls.get() + 1);
            // Busy for the first 10 attempts: about 100 ms of a real holder.
            if calls.get() <= 10 { Err(ClipboardBusy) } else { Ok(true) }
        },
        || pauses.set(pauses.get() + 1),
    );
    assert_eq!(got, Ok(true));
    assert_eq!(calls.get(), 11);
    assert_eq!(pauses.get(), 10);
}

#[test]
fn a_free_clipboard_is_written_at_once() {
    let pauses = Cell::new(0u32);
    let got = retry_while_busy(Duration::from_secs(1), || Ok(true), || pauses.set(pauses.get() + 1));
    assert_eq!(got, Ok(true));
    assert_eq!(pauses.get(), 0);
}

#[test]
fn a_refused_write_is_not_retried() {
    let calls = Cell::new(0u32);
    let got = retry_while_busy(
        Duration::from_secs(1),
        || { calls.set(calls.get() + 1); Ok(false) },
        || {},
    );
    assert_eq!(got, Ok(false));
    assert_eq!(calls.get(), 1, "an opened clipboard that refused the data is an answer");
}

#[test]
fn a_clipboard_held_past_the_timeout_gives_up() {
    let start = std::time::Instant::now();
    let got: Result<bool, ClipboardBusy> = retry_while_busy(
        Duration::from_millis(60),
        || Err(ClipboardBusy),
        || std::thread::sleep(Duration::from_millis(5)),
    );
    assert_eq!(got, Err(ClipboardBusy));
    let took = start.elapsed();
    assert!(took >= Duration::from_millis(60) && took < Duration::from_secs(2), "took {:?}", took);
}

#[test]
fn only_a_failed_write_warns() {
    let t = CLIPBOARD_WRITE_TIMEOUT;
    assert_eq!(clipboard_write_warning(Ok(true), t), None);
    let busy = clipboard_write_warning(Err(ClipboardBusy), t).unwrap();
    assert!(busy.starts_with("psmux: could not write the system clipboard"), "{busy}");
    assert!(busy.contains("1000 ms"), "{busy}");
    assert!(clipboard_write_warning(Ok(false), t).is_some());
}

#[test]
fn the_write_wait_is_one_second() {
    assert_eq!(CLIPBOARD_WRITE_TIMEOUT, Duration::from_secs(1));
}

/// The real Win32 path: another process holds the clipboard open for 400 ms
/// (far past the 10 ms the plain write retries), then lets go. The waiting
/// write must land; the plain write must report the clipboard busy.
#[cfg(windows)]
#[test]
fn a_write_waits_for_a_clipboard_another_process_holds() {
    use std::process::{Command, Stdio};
    use std::io::{BufRead, BufReader};

    let _g = TEST_CLIPBOARD_CONTENTS.lock().unwrap_or_else(|e| e.into_inner());
    let saved = read_from_system_clipboard_waiting(Duration::from_secs(2));

    // A PowerShell child opens the clipboard with a real window, says READY,
    // holds it 400 ms, and closes it. A NULL owner would block nobody.
    let script = r#"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices; public static class H { [DllImport("user32.dll")] public static extern bool OpenClipboard(IntPtr h); [DllImport("user32.dll")] public static extern bool CloseClipboard(); }'
$f = New-Object System.Windows.Forms.Form
$ok = $false
for ($i = 0; $i -lt 400 -and -not $ok; $i++) { $ok = [H]::OpenClipboard($f.Handle); if (-not $ok) { Start-Sleep -Milliseconds 5 } }
if (-not $ok) { [Console]::Out.WriteLine('NOHOLD'); exit 1 }
[Console]::Out.WriteLine('READY'); [Console]::Out.Flush()
Start-Sleep -Milliseconds 400
[H]::CloseClipboard() | Out-Null
"#;
    let hold = |script: &str| {
        let mut child = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn powershell");
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        (child, line.trim().to_string())
    };

    // Plain write while held: busy.
    let (mut child, ready) = hold(script);
    if ready != "READY" {
        let _ = child.wait();
        eprintln!("the desktop refused the holder the clipboard; nothing can be learned");
        return;
    }
    assert_eq!(try_copy_to_system_clipboard("PSMUX_WRITE_BUSY_PLAIN"), Err(ClipboardBusy));
    let _ = child.wait();

    // Waiting write while held: lands once the holder lets go.
    let (mut child, ready) = hold(script);
    if ready != "READY" {
        let _ = child.wait();
        return;
    }
    let start = std::time::Instant::now();
    let got = copy_to_system_clipboard_waiting("PSMUX_WRITE_BUSY_WAITED", CLIPBOARD_WRITE_TIMEOUT);
    let took = start.elapsed();
    let _ = child.wait();
    assert_eq!(got, Ok(true), "the waiting write gave up after {:?}", took);
    assert!(took >= Duration::from_millis(100), "it cannot have written while the clipboard was held: {:?}", took);
    assert_eq!(
        read_from_system_clipboard_waiting(Duration::from_secs(2)).as_deref(),
        Some("PSMUX_WRITE_BUSY_WAITED")
    );

    if let Some(text) = saved {
        let _ = copy_to_system_clipboard_waiting(&text, Duration::from_secs(2));
    }
}
