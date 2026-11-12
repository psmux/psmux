//! Windows system clipboard read/write.
//!
//! The Win32 clipboard helpers live in their own module so that any code
//! that needs to interact with the user's clipboard - the server-side copy
//! mode UI, the client-side `load-buffer -w` path, the `iterm-cc-compat`
//! paste integration - can call them without dragging in the rest of
//! `copy_mode`.
//!
//! On non-Windows targets the functions are no-ops returning empty / `None`
//! so callers don't need their own `#[cfg]` gates.

#[cfg(windows)]
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use windows_sys::Win32::Foundation::GlobalFree;
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

/// Every OpenClipboard..CloseClipboard section in this process runs under this
/// lock. The Win32 clipboard lock only keeps processes apart: with no owner
/// window, `OpenClipboard` on a second thread of the same process succeeds
/// while the first still has it open, so one thread's `EmptyClipboard` could
/// free the handle another thread was reading in `GetClipboardData` (a double
/// free in USER32, STATUS_HEAP_CORRUPTION). The server reads the clipboard on
/// connection threads while its main loop writes it.
#[cfg(windows)]
static CLIPBOARD_SECTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(windows)]
fn clipboard_section() -> std::sync::MutexGuard<'static, ()> {
    CLIPBOARD_SECTION.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tests that assert what the clipboard holds take this for their whole body,
/// so one test's write cannot land between another's write and read.
#[cfg(test)]
pub(crate) static TEST_CLIPBOARD_CONTENTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Write `text` to the Windows system clipboard as CF_UNICODETEXT.
///
/// Other processes may briefly hold the clipboard open, so this retries for
/// about 10 ms before giving up silently. That suits the server main loop and
/// the client input loop, which must not stall; a command the user ran to
/// write the clipboard uses [`copy_to_system_clipboard_for_command`] instead.
///
/// * If `GlobalAlloc` / `GlobalLock` / `OpenClipboard` / `EmptyClipboard`
///   fails, the user's existing clipboard contents are left untouched.
/// * There is a narrow inherent race between `EmptyClipboard` and
///   `SetClipboardData`: if `SetClipboardData` fails after `EmptyClipboard`
///   has already succeeded, the clipboard is left empty. The Win32
///   clipboard API offers no atomic "replace" primitive, so this window
///   cannot be closed entirely.
pub fn copy_to_system_clipboard(text: &str) {
    let _ = try_copy_to_system_clipboard(text);
}

/// [`copy_to_system_clipboard`], telling a clipboard another window held open
/// through every retry (`Err`) apart from one that was opened (`Ok(true)` when
/// the text was stored, `Ok(false)` when Windows refused the data itself).
#[cfg(windows)]
pub fn try_copy_to_system_clipboard(text: &str) -> Result<bool, ClipboardBusy> {
    const CF_UNICODETEXT: u32 = 13;

    // Prepare the HGLOBAL BEFORE touching the clipboard. If allocation or
    // locking fails the user's existing clipboard is untouched, and we
    // hold the global clipboard lock for the shortest possible window.
    let mut utf16: Vec<u16> = text.encode_utf16().collect();
    utf16.push(0); // null terminator required by CF_UNICODETEXT
    let size_bytes = utf16.len() * std::mem::size_of::<u16>();

    let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, size_bytes) };
    if hmem.is_null() {
        return Ok(false);
    }

    unsafe {
        let dst = GlobalLock(hmem) as *mut u16;
        if dst.is_null() {
            let _ = GlobalFree(hmem);
            return Ok(false);
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
        GlobalUnlock(hmem);
    }

    // Buffer is ready. Open the clipboard only for the minimal Win32 dance.
    // Other processes can briefly hold the clipboard open; retry a few times.
    let mut result = Err(ClipboardBusy);
    for _ in 0..5 {
        let _section = clipboard_section();
        let opened = unsafe { OpenClipboard(std::ptr::null_mut()) };
        if opened == 0 {
            drop(_section);
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        unsafe {
            // EmptyClipboard immediately followed by SetClipboardData is the
            // documented Win32 pattern for replacing contents. The window
            // between these calls is the unavoidable race described above.
            // Ownership of hmem passes to the OS only when SetClipboardData
            // succeeds; otherwise it is freed below.
            let stored = EmptyClipboard() != 0
                && !SetClipboardData(CF_UNICODETEXT, hmem).is_null();
            let _ = CloseClipboard();
            result = Ok(stored);
        }
        break;
    }

    if result != Ok(true) {
        unsafe {
            let _ = GlobalFree(hmem);
        }
    }
    result
}

#[cfg(not(windows))]
pub fn try_copy_to_system_clipboard(_text: &str) -> Result<bool, ClipboardBusy> { Ok(true) }

/// How long `set-buffer -w` and `load-buffer -w`, run from the command line,
/// wait for a clipboard another window holds open before they give up.
///
/// tmux hands the text to the terminal as OSC 52 and never waits.  On Windows
/// psmux writes the clipboard itself, and a clipboard history listener, a VNC
/// or RDP clipboard bridge or a phone link app opens it for a few to a few
/// hundred milliseconds after every change.  The ~10 ms of retries inside
/// [`try_copy_to_system_clipboard`] lost to such a holder and silently left
/// the old clipboard in place.  One second covers those holders and costs
/// nothing when the clipboard is free.
pub const CLIPBOARD_WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// [`copy_to_system_clipboard`] for a command line process that asked to
/// write the clipboard and can wait: it keeps trying while another window
/// holds the clipboard open, up to `timeout`, and returns as soon as the
/// clipboard was opened.  `Err` means it was held open the whole time.
///
/// Never call this from the server main loop or the client input loop.
pub fn copy_to_system_clipboard_waiting(text: &str, timeout: Duration) -> Result<bool, ClipboardBusy> {
    retry_while_busy(timeout, || try_copy_to_system_clipboard(text), || {
        std::thread::sleep(Duration::from_millis(10))
    })
}

/// The warning a command line `-w` prints when the clipboard could not be
/// written, or `None` when it was.  tmux cannot know whether the terminal took
/// its OSC 52, so the command still succeeds; this only stops the failure
/// from being silent.
pub fn clipboard_write_warning(result: Result<bool, ClipboardBusy>, timeout: Duration) -> Option<String> {
    match result {
        Ok(true) => None,
        Ok(false) => Some("psmux: could not write the system clipboard: Windows refused the text".to_string()),
        Err(ClipboardBusy) => Some(format!(
            "psmux: could not write the system clipboard: another program held it open for {} ms",
            timeout.as_millis()
        )),
    }
}

/// The command line side of `set-buffer -w` and `load-buffer -w`: write the
/// clipboard, waiting for a busy one, and warn on stderr if it could not be
/// written.
pub fn copy_to_system_clipboard_for_command(text: &str) {
    let result = copy_to_system_clipboard_waiting(text, CLIPBOARD_WRITE_TIMEOUT);
    if let Some(warning) = clipboard_write_warning(result, CLIPBOARD_WRITE_TIMEOUT) {
        eprintln!("{}", warning);
    }
}

/// Read text from the Windows system clipboard.
///
/// Returns `None` if the clipboard cannot be opened, has no
/// `CF_UNICODETEXT` data, or the data is malformed (no null terminator
/// within the allocation). The scan is bounded by the actual `HGLOBAL`
/// size via `GlobalSize` so a malformed payload cannot trigger an
/// out-of-bounds read.
pub fn read_from_system_clipboard() -> Option<String> {
    try_read_from_system_clipboard().ok().flatten()
}

/// Another window held the clipboard open through every retry, so nothing is
/// known about its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardBusy;

/// [`read_from_system_clipboard`], telling a clipboard that could not be
/// opened (`Err`) apart from one that holds no text (`Ok(None)`).  A caller
/// that caches the answer must not cache the first: it is a moment in another
/// process, not a property of the clipboard (issue #684).
#[cfg(windows)]
pub fn try_read_from_system_clipboard() -> Result<Option<String>, ClipboardBusy> {
    const CF_UNICODETEXT: u32 = 13;
    for _ in 0..5 {
        let _section = clipboard_section();
        let opened = unsafe { OpenClipboard(std::ptr::null_mut()) };
        if opened == 0 {
            drop(_section);
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let result = unsafe {
            let hmem = GetClipboardData(CF_UNICODETEXT);
            let ptr = if !hmem.is_null() {
                GlobalLock(hmem) as *const u16
            } else {
                std::ptr::null()
            };

            let text = if !ptr.is_null() {
                // Bound the scan by actual allocation size so unterminated
                // CF_UNICODETEXT from a misbehaving clipboard provider
                // cannot trigger an OOB read. Also cap at 1M u16s as a
                // latency guard against pathologically huge clipboards.
                let alloc_bytes = GlobalSize(hmem) as usize;
                let max_u16s =
                    (alloc_bytes / std::mem::size_of::<u16>()).min(1_000_000);

                // If no terminator is found within the allocation, treat
                // the payload as malformed and fail closed by returning
                // None - better than returning truncated garbage.
                let found = (0..max_u16s).position(|i| *ptr.add(i) == 0).map(|len| {
                    let slice = std::slice::from_raw_parts(ptr, len);
                    // Normalize Windows CRLF to LF - ConPTY expands LF to
                    // CRLF on output, so keeping \r\n would produce
                    // double-spaced text in the pane.
                    String::from_utf16_lossy(slice).replace("\r\n", "\n")
                });
                GlobalUnlock(hmem);
                found
            } else {
                None
            };
            let _ = CloseClipboard();
            text
        };
        return Ok(result);
    }
    Err(ClipboardBusy)
}

#[cfg(not(windows))]
pub fn try_read_from_system_clipboard() -> Result<Option<String>, ClipboardBusy> { Ok(None) }

/// How long `refresh-client -l` waits for a clipboard another window holds
/// open: tmux waits this long for the terminal's OSC 52 answer before it
/// gives up on the query (tty.c TTY_QUERY_TIMEOUT, tty_clipboard_query).
pub const CLIPBOARD_QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a paste the user asked for in the attached client (Ctrl+V whose
/// release reads the clipboard, Ctrl+Shift+V, right click) waits for a
/// clipboard another window holds open.  tmux has no client side paste; this
/// is the Windows clipboard's lock.  The client input loop waits with it, so
/// keys typed meanwhile stay behind the paste in order, and it is short
/// enough that a clipboard held for good costs one second, not five.
pub const PASTE_GESTURE_TIMEOUT: Duration = Duration::from_secs(1);

/// [`read_from_system_clipboard`] for a caller that asked for the clipboard
/// and can wait for it, such as `refresh-client -l` on a connection thread.
///
/// The retries inside [`try_read_from_system_clipboard`] cover about 10 ms.
/// A clipboard history listener, a remote desktop clipboard bridge or an
/// Office app routinely holds the clipboard open for tens to hundreds of
/// milliseconds after a change, and the plain read then answered "no text",
/// so `refresh-client -l` silently stored nothing.  This keeps trying until
/// the clipboard is free or `timeout` passes, returning as soon as it reads.
pub fn read_from_system_clipboard_waiting(timeout: Duration) -> Option<String> {
    wait_for_clipboard(timeout, try_read_from_system_clipboard, || {
        std::thread::sleep(Duration::from_millis(10))
    })
}

/// The loop of [`read_from_system_clipboard_waiting`], with the read and the
/// pause passed in so the timing rules can be tested without a desktop.
fn wait_for_clipboard(
    timeout: Duration,
    read: impl FnMut() -> Result<Option<String>, ClipboardBusy>,
    pause: impl FnMut(),
) -> Option<String> {
    retry_while_busy(timeout, read, pause).ok().flatten()
}

/// Run `op` until it gets the clipboard or `timeout` passes, pausing between
/// busy attempts.  Shared by the waiting read and the waiting write.
fn retry_while_busy<T>(
    timeout: Duration,
    mut op: impl FnMut() -> Result<T, ClipboardBusy>,
    mut pause: impl FnMut(),
) -> Result<T, ClipboardBusy> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(ClipboardBusy) if std::time::Instant::now() < deadline => pause(),
            Err(ClipboardBusy) => return Err(ClipboardBusy),
        }
    }
}

/// The system clipboard's change counter.
///
/// `GetClipboardSequenceNumber` is a read of one shared value: it does not open
/// the clipboard, cannot fail and cannot block behind another process that is
/// holding it.  That is what makes it usable from the client's input loop,
/// where an `OpenClipboard` per keystroke would not be: the head of the
/// clipboard can be cached and only re-read when this number moves.
#[cfg(windows)]
pub fn clipboard_sequence_number() -> u32 {
    unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() }
}

#[cfg(not(windows))]
pub fn clipboard_sequence_number() -> u32 { 0 }

#[cfg(test)]
#[path = "../tests-rs/test_refresh_client_l_busy_clipboard.rs"]
mod tests_refresh_client_l_busy_clipboard;

#[cfg(test)]
#[path = "../tests-rs/test_clipboard_write_busy.rs"]
mod tests_clipboard_write_busy;

#[cfg(all(test, windows))]
#[path = "../tests-rs/test_clipboard_thread_lock.rs"]
mod tests_clipboard_thread_lock;
