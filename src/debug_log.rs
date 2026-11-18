//! Centralized debug logging for psmux.
//!
//! All logs write to `~/.psmux/` and are gated by environment variables.
//! Nothing is stored in the repo or source tree — only in the user's
//! home directory under `.psmux/`.
//!
//! ## Environment Variables
//!
//! | Variable               | Log file                          | Description                          |
//! |------------------------|-----------------------------------|--------------------------------------|
//! | `PSMUX_CLIENT_DEBUG=1` | `~/.psmux/client_debug.log`       | Client TUI rendering, draw, status   |
//! | `PSMUX_CLIENT_DEBUG=1` | `~/.psmux/client_reconnect.log`   | Client reconnect attempts (appended) |
//! | `PSMUX_STYLE_DEBUG=1`  | `~/.psmux/style_debug.log`        | Style/theme parsing, inline styles   |/// | `PSMUX_INPUT_DEBUG=1`  | `~/.psmux/input_debug.log`        | Every crossterm event + console mode |//! | `PSMUX_MOUSE_DEBUG=1`  | `~/.psmux/mouse_debug.log`        | Mouse injection (existing)           |
//! | `PSMUX_SSH_DEBUG=1`    | `~/.psmux/ssh_input.log`          | SSH input handling (existing)        |
//! | `PSMUX_LATENCY_LOG=1`  | `~/.psmux/latency.log`            | Keypress-to-render latency (existing)|
//! | `PSMUX_SESSION_DEBUG=1`| `~/.psmux/session_debug.log`      | Session-registry stale-port cleanup  |
//! | `PSMUX_PANE_RAW=1`    | `~/.psmux/pane_raw.bin`           | Raw pre-parse pane byte stream        |
//! | `PSMUX_AUTORENAME_DEBUG=1` | `~/.psmux/autorename.log`     | Process-tree walk for automatic-rename |
//! | `PSMUX_PTY_TRACE=<path>` | `<path>.<pid>`                  | QPC-stamped keystroke path, see src/pty_trace.rs |
//! | `PSMUX_STARTUP_TRACE=<path>` | `<path>.<pid>`              | QPC-stamped launch-to-prompt hops, see src/startup_trace.rs |
//!
//! All loggers are:
//! - **Off by default** — zero overhead when disabled (one atomic load per call)
//! - **Capped** — auto-stop after N entries to prevent disk fill
//! - **Thread-safe** — use `LazyLock<Mutex<Option<File>>>`
//! - **Timestamped** — `[HH:MM:SS.mmm]` prefix on every line
//! - **Truncated on startup** — fresh log each session (no stale data), EXCEPT
//!   for the files more than one psmux process writes.  `input_debug.log` is
//!   written by the client and by the server, so it is opened in append mode
//!   with a banner per process; see [`open_shared_log`].

use std::io::Write;
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};

/// Resolve the psmux data directory (`~/.psmux/`).
fn psmux_dir() -> String {
    crate::paths::psmux_dir()
}

/// How a log file is opened when a process takes it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LogMode {
    /// One writer: start clean so the file holds this session only.
    Truncate,
    /// Several processes write the same file: every one of them appends.
    ///
    /// `truncate(true)` is wrong the moment a second process can open the same
    /// path.  `input_debug.log` is exactly that: the client and the server both
    /// write it, each through its own `LazyLock` that opens on its first line,
    /// and each keeps its own file offset.  gabri-ns hit both halves of that on
    /// #684: the server's first `[paste]` line truncated the file the client had
    /// been writing since startup, wiping every client `[paste]` line before it,
    /// and the client's next write landed at its old offset and re-extended the
    /// file over the hole, leaving 106,367 NUL bytes between the server's four
    /// lines and the rest of the client's.
    Append,
}

/// Open a log file in the psmux data directory, creating the directory if needed.
/// Returns `None` if the file cannot be created.
fn open_log(filename: &str) -> Option<std::fs::File> {
    open_log_in(&psmux_dir(), filename, LogMode::Truncate)
}

/// Open a log file that more than one psmux process writes.
///
/// Appending, rather than a per process file name: the client's and the
/// server's lines are two halves of one story (the client decides a burst is a
/// paste, the server decides which channel carries it), and reading them
/// interleaved in one file, in one timeline, is what made #684 diagnosable at
/// all.  Two files would have to be correlated by timestamp by hand, and every
/// existing instruction, test and doc would have to learn a new name.  Each
/// process writes a banner naming itself when it opens the file, so the halves
/// are still attributable, and `PSMUX_INPUT_DEBUG` is opt in with a 10,000 line
/// cap per process, so the file cannot grow behind anyone's back.
fn open_shared_log(filename: &str) -> Option<std::fs::File> {
    let mut file = open_log_in(&psmux_dir(), filename, LogMode::Append)?;
    let _ = writeln!(
        file,
        "[{}][log] === {} pid {} opened {} (append) ===",
        crate::strftime::now_string("%H:%M:%S%.3f"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "psmux".to_string()),
        std::process::id(),
        filename
    );
    let _ = file.flush();
    Some(file)
}

/// The open itself, with the directory and the mode passed in so it can be
/// exercised against a temporary directory in a unit test.
pub(crate) fn open_log_in(dir: &str, filename: &str, mode: LogMode) -> Option<std::fs::File> {
    let _ = std::fs::create_dir_all(dir);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true);
    match mode {
        LogMode::Truncate => { opts.truncate(true); }
        LogMode::Append => { opts.append(true); }
    }
    opts.open(format!("{}/{}", dir, filename)).ok()
}

/// Check if an env var is set to a truthy value ("1" or "true").
fn env_enabled(var: &str) -> bool {
    std::env::var(var).map_or(false, |v| v == "1" || v.eq_ignore_ascii_case("true"))
}

// ─── Client debug log ───────────────────────────────────────────────────────

/// Client debug log file, gated by `PSMUX_CLIENT_DEBUG=1`.
/// Covers: frame receive, JSON parse, draw lifecycle, status bar rendering.
static CLIENT_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_CLIENT_DEBUG") { return Mutex::new(None); }
    Mutex::new(open_log("client_debug.log"))
});

static CLIENT_LOG_COUNT: AtomicU32 = AtomicU32::new(0);

/// Maximum log entries per session to prevent disk fill.
const CLIENT_LOG_CAP: u32 = 5000;

/// Log a client debug message. No-op unless `PSMUX_CLIENT_DEBUG=1`.
///
/// # Arguments
/// * `component` — short tag like `"frame"`, `"draw"`, `"status"`, `"parse"`
/// * `msg` — the log message (should not contain newlines)
pub fn client_log(component: &str, msg: &str) {
    let n = CLIENT_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= CLIENT_LOG_CAP {
        if n == CLIENT_LOG_CAP {
            // Log one final "cap reached" message
            if let Ok(mut guard) = CLIENT_LOG.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = writeln!(f, "[{}][log] --- log cap reached ({} entries), further logging suppressed ---",
                        crate::strftime::now_string("%H:%M:%S%.3f"), CLIENT_LOG_CAP);
                    let _ = f.flush();
                }
            }
        }
        return;
    }
    if let Ok(mut guard) = CLIENT_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][{}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), component, msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if client debug logging is active.
pub fn client_log_enabled() -> bool {
    CLIENT_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Client reconnect log ───────────────────────────────────────────────────

/// Client reconnect trace, gated by the same `PSMUX_CLIENT_DEBUG=1`.
///
/// Separate from `client_debug.log`, and **appended** rather than truncated,
/// because a reconnect is exactly the event a truncating log cannot record.
/// `client_debug.log` is opened fresh by every attach client, so the client
/// whose reconnect is under investigation has its trace erased by the next
/// client to attach. Issue #434's own suite proves the point: the client that
/// survives a writer teardown in Test 1 is followed by thirty churn clients in
/// Test 3, and by the time anyone reads the log nothing of Test 1 is left.
///
/// Every line carries the pid, so interleaved clients stay tellable apart.
static RECONNECT_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_CLIENT_DEBUG") { return Mutex::new(None); }
    let dir = psmux_dir();
    let _ = std::fs::create_dir_all(&dir);
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}/client_reconnect.log", dir))
        .ok();
    Mutex::new(f)
});

static RECONNECT_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
const RECONNECT_LOG_CAP: u32 = 2000;

/// Log a client reconnect message. No-op unless `PSMUX_CLIENT_DEBUG=1`.
pub fn reconnect_log(msg: &str) {
    let n = RECONNECT_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= RECONNECT_LOG_CAP {
        return;
    }
    if let Ok(mut guard) = RECONNECT_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][reconnect][pid {}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), std::process::id(), msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if client reconnect logging is active.
pub fn reconnect_log_enabled() -> bool {
    RECONNECT_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Raw pane byte log ──────────────────────────────────────────────────────

/// Raw pane output log, gated by `PSMUX_PANE_RAW=1`.
///
/// Appends the EXACT bytes each pane's ConPTY hands to the vt100 parser, before
/// any parsing. This is the only artifact that can settle "who emitted this
/// escape sequence" questions: the pane grid and `capture-pane -e` show the
/// RESULT, not the input, so a stuck colour looks identical whether the child
/// emitted it or psmux mis-parsed something (issue #502).
///
/// Written verbatim with no framing so the file can be replayed straight back
/// through the parser. Capped at 8 MB, which is far more than any interactive
/// session needs and keeps a runaway `yes` loop from filling the disk.
static PANE_RAW_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_PANE_RAW") { return Mutex::new(None); }
    Mutex::new(open_log("pane_raw.bin"))
});

static PANE_RAW_BYTES: AtomicU32 = AtomicU32::new(0);

/// Cheap gate so the pane hot path pays one relaxed load, not a mutex, when
/// logging is off. `LazyLock` on the file handle alone would still lock.
static PANE_RAW_ON: LazyLock<bool> = LazyLock::new(|| env_enabled("PSMUX_PANE_RAW"));

/// Maximum bytes captured per session (8 MB).
const PANE_RAW_CAP: u32 = 8 * 1024 * 1024;

/// Append raw pane bytes. No-op unless `PSMUX_PANE_RAW=1`.
pub fn pane_raw(bytes: &[u8]) {
    if !*PANE_RAW_ON || bytes.is_empty() {
        return;
    }
    let n = PANE_RAW_BYTES.fetch_add(bytes.len() as u32, Ordering::Relaxed);
    if n >= PANE_RAW_CAP {
        return;
    }
    if let Ok(mut guard) = PANE_RAW_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = f.write_all(bytes);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if raw pane logging is active (lets hot paths skip the call).
pub fn pane_raw_enabled() -> bool {
    PANE_RAW_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Style debug log ────────────────────────────────────────────────────────

/// Style/theme parsing debug log, gated by `PSMUX_STYLE_DEBUG=1`.
/// Covers: inline style parsing, unclosed directives, color mapping.
static STYLE_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_STYLE_DEBUG") { return Mutex::new(None); }
    Mutex::new(open_log("style_debug.log"))
});

static STYLE_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
const STYLE_LOG_CAP: u32 = 2000;

/// Log a style debug message. No-op unless `PSMUX_STYLE_DEBUG=1`.
pub fn style_log(component: &str, msg: &str) {
    let n = STYLE_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= STYLE_LOG_CAP {
        if n == STYLE_LOG_CAP {
            if let Ok(mut guard) = STYLE_LOG.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = writeln!(f, "[{}][log] --- log cap reached ---",
                        crate::strftime::now_string("%H:%M:%S%.3f"));
                    let _ = f.flush();
                }
            }
        }
        return;
    }
    if let Ok(mut guard) = STYLE_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][{}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), component, msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if style debug logging is active.
pub fn style_log_enabled() -> bool {
    STYLE_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Input debug log ────────────────────────────────────────────────────────

/// Input event debug log, gated by `PSMUX_INPUT_DEBUG=1`.
/// Traces every crossterm event + console input mode at startup.
///
/// Opened in APPEND mode: the client and the server both write this file (the
/// client's paste decision and the server's channel decision are the two halves
/// of one paste), so whichever opened it second used to truncate the other's
/// lines away.  See [`open_shared_log`].
static INPUT_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_INPUT_DEBUG") { return Mutex::new(None); }
    Mutex::new(open_shared_log("input_debug.log"))
});

static INPUT_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
const INPUT_LOG_CAP: u32 = 10000;

/// Log an input debug message. No-op unless `PSMUX_INPUT_DEBUG=1`.
pub fn input_log(component: &str, msg: &str) {
    let n = INPUT_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= INPUT_LOG_CAP {
        if n == INPUT_LOG_CAP {
            if let Ok(mut guard) = INPUT_LOG.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = writeln!(f, "[{}][log] --- log cap reached ---",
                        crate::strftime::now_string("%H:%M:%S%.3f"));
                    let _ = f.flush();
                }
            }
        }
        return;
    }
    if let Ok(mut guard) = INPUT_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][{}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), component, msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if input debug logging is active.
pub fn input_log_enabled() -> bool {
    INPUT_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Server debug log ───────────────────────────────────────────────────────

/// Server debug log, gated by `PSMUX_SERVER_DEBUG=1`.
/// Traces active_idx changes, command dispatch, etc.
static SERVER_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_SERVER_DEBUG") { return Mutex::new(None); }
    Mutex::new(open_log("server_debug.log"))
});

static SERVER_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
const SERVER_LOG_CAP: u32 = 10000;

/// Log a server debug message. No-op unless `PSMUX_SERVER_DEBUG=1`.
pub fn server_log(component: &str, msg: &str) {
    let n = SERVER_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= SERVER_LOG_CAP {
        if n == SERVER_LOG_CAP {
            if let Ok(mut guard) = SERVER_LOG.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = writeln!(f, "[{}][log] --- log cap reached ---",
                        crate::strftime::now_string("%H:%M:%S%.3f"));
                    let _ = f.flush();
                }
            }
        }
        return;
    }
    if let Ok(mut guard) = SERVER_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][{}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), component, msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if server debug logging is active.
pub fn server_log_enabled() -> bool {
    SERVER_LOG.lock().ok().map_or(false, |g| g.is_some())
}

// ─── Session-registry debug log ─────────────────────────────────────────────

/// Session-registry lifecycle log, gated by `PSMUX_SESSION_DEBUG=1`.
/// Covers stale-port cleanup decisions: boot-time reaps, auth-rejected
/// (reused-port) reaps, unparseable port files, and live/inconclusive
/// probe verdicts — exactly the path that decided whether a session shows
/// up as a `(not responding)` zombie.
///
/// Unlike the other loggers this **appends** rather than truncates, because
/// registry cleanup runs in many short-lived CLI processes (every `psmux`
/// invocation calls it at startup); truncating on open would clobber the
/// log before it could be read.
static SESSION_LOG: LazyLock<Mutex<Option<std::fs::File>>> = LazyLock::new(|| {
    if !env_enabled("PSMUX_SESSION_DEBUG") { return Mutex::new(None); }
    // Every short lived psmux CLI process runs the registry cleanup, so this
    // one has always appended; it is the same reason input_debug.log now does.
    Mutex::new(open_log_in(&psmux_dir(), "session_debug.log", LogMode::Append))
});

static SESSION_LOG_COUNT: AtomicU32 = AtomicU32::new(0);
const SESSION_LOG_CAP: u32 = 5000;

/// Log a session-registry message. No-op unless `PSMUX_SESSION_DEBUG=1`.
pub fn session_log(component: &str, msg: &str) {
    let n = SESSION_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= SESSION_LOG_CAP {
        if n == SESSION_LOG_CAP {
            if let Ok(mut guard) = SESSION_LOG.lock() {
                if let Some(ref mut f) = *guard {
                    let _ = writeln!(f, "[{}][log] --- log cap reached ---",
                        crate::strftime::now_string("%H:%M:%S%.3f"));
                    let _ = f.flush();
                }
            }
        }
        return;
    }
    if let Ok(mut guard) = SESSION_LOG.lock() {
        if let Some(ref mut f) = *guard {
            let _ = writeln!(f, "[{}][{}] {}",
                crate::strftime::now_string("%H:%M:%S%.3f"), component, msg);
            let _ = f.flush();
        }
    }
}

/// Returns `true` if session-registry debug logging is active.
pub fn session_log_enabled() -> bool {
    SESSION_LOG.lock().ok().map_or(false, |g| g.is_some())
}

#[cfg(test)]
#[path = "../tests-rs/test_issue684_input_log_append.rs"]
mod tests_issue684_input_log_append;

