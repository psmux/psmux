//! Env gated hop trace of launch to prompt, `PSMUX_STARTUP_TRACE=<file>`.
//!
//! With the variable unset every probe is one relaxed atomic load and returns,
//! so the shipped startup path is unchanged. It exists to answer one question
//! with timestamps rather than inference: of the time between a user typing
//! `psmux new-session <cmd>` and the pane's shell reaching its first prompt,
//! which hop owns which milliseconds.
//!
//! Timestamps are raw `QueryPerformanceCounter` ticks, which are system wide on
//! Windows, so a line written by the client process, a line written by the
//! server process and a `Stopwatch` timestamp taken by a test harness can all
//! be compared directly. The header line records the frequency.
//!
//! Every process that has the variable set writes its own `<path>.<pid>`, so
//! the client's and the server's lines never interleave; merge and sort by the
//! first column to read the whole path.
//!
//! THREE PROCESSES WRITE, AND THEIR FILES MUST NOT BE MERGED BY LABEL: the
//! foreground CLI, the session server, and the warm standby server the session
//! server spawns just before its loop. All three run `main`, so all three write
//! `cli.entry`, and merging by label produces a timeline that runs backwards.
//! The client is the pid the launcher started; the session server is the one
//! that writes `srv.child.spawned`; anything else is the standby.
//!
//! Labels, in path order (`cli.` is the foreground CLI, `srv.` the server):
//!   `cli.entry`        first line of `main`
//!   `cli.dispatch`     argv parsed, about to act on the subcommand
//!   `cli.warm.claimed` a warm standby server was claimed (fast path)
//!   `cli.server.spawn` `spawn_server_hidden` returned (cold path)
//!   `cli.ready`        the readiness gate accepted the server
//!   `cli.cfgwarn`      the client read `config-warnings.log`, with what it
//!                      found. It has to land after `srv.cfgwarn` or the
//!                      warnings are lost (#706)
//!   `cli.attach`       the attach/TUI path is entered
//!   `cli.connected`    the client's socket to the server is up
//!   `cli.frame`        the client's reader received a frame line, with its size
//!   `cli.draw`         the client started drawing a frame
//!   `srv.boot.spawned` the first pane's process, started on its own thread
//!                      right after `srv.listen`, is in its pseudoconsole
//!   `srv.boot.respawn` the config changed that early pane's spawn inputs, with
//!                      which one; the window is created from scratch instead
//!   `srv.boot.envdiff` for an `environment` respawn, the names that differ
//!   `srv.entry`        first line of `run_server`
//!   `srv.appstate`     `AppState::new` returned
//!   `srv.priority`     scheduling class claimed (#608)
//!   `srv.mutex`        the single-server-per-name guard is held
//!   `srv.listen`       `TcpListener::bind` returned, the port is known
//!   `srv.keyfile`      the `.key` file is on disk (it precedes `.port`, #496)
//!   `srv.reg.pid`      `.sid` and `.pid` written
//!   `srv.reg.instance` the namespace instance token is established (#509)
//!   `srv.reg.marker`   this process is claimed for the data dir (#510)
//!   `srv.bound`        `.port` written: the readiness beacon is visible
//!   `srv.config`       `load_config` returned
//!   `srv.cfgwarn`      `config-warnings.log` written, with how many
//!   `srv.pty.open`     `openpty` called
//!   `srv.pty.ready`    `CreatePseudoConsole` returned
//!   `srv.child.argv`   the pane child's argv, about to be spawned
//!   `srv.child.spawned` `CreateProcessW` into the pseudoconsole returned
//!   `srv.window`       the initial window exists
//!   `srv.prewarm`      the spare pane pool has been filled
//!   `srv.loop`         the main request loop is about to run
//!   `srv.boot.watch`   the boot hold started watching a pane (the first one,
//!                      or a window or split created during the hold)
//!   `srv.boot.output`  a watched pane wrote output, with its data version
//!                      and whether it now shows visible text (`types::BootHold`)
//!   `srv.boot.release` the boot hold released: the pool trickles from now on
//!                      and the standby follows once the pool has its spares
//!
//! The `srv.listen` .. `srv.bound` span is the session registry: six small
//! files, and on a machine with realtime AV scanning each create costs several
//! milliseconds, so the span is worth far more than its line count suggests.
//! Its order is load bearing (see #496 and #509) and is not free to shuffle.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

/// 0 = not looked up yet, 1 = on, 2 = off.
static GATE: AtomicU8 = AtomicU8::new(0);
static SINK: OnceLock<Option<Mutex<BufWriter<File>>>> = OnceLock::new();

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn QueryPerformanceCounter(out: *mut i64) -> i32;
    fn QueryPerformanceFrequency(out: *mut i64) -> i32;
}

#[cfg(windows)]
fn qpc() -> i64 {
    let mut v: i64 = 0;
    // SAFETY: writes one i64 through a valid pointer to a local.
    unsafe { QueryPerformanceCounter(&mut v) };
    v
}

#[cfg(windows)]
fn qpf() -> i64 {
    let mut v: i64 = 1;
    // SAFETY: writes one i64 through a valid pointer to a local.
    unsafe { QueryPerformanceFrequency(&mut v) };
    v
}

#[cfg(not(windows))]
fn qpc() -> i64 {
    0
}
#[cfg(not(windows))]
fn qpf() -> i64 {
    1
}

/// Whether tracing is on. One relaxed load in the steady state.
#[inline]
pub fn on() -> bool {
    match GATE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let want = std::env::var_os("PSMUX_STARTUP_TRACE").is_some();
            GATE.store(if want { 1 } else { 2 }, Ordering::Relaxed);
            want
        }
    }
}

fn sink() -> Option<&'static Mutex<BufWriter<File>>> {
    SINK.get_or_init(|| {
        let path = format!(
            "{}.{}",
            std::env::var("PSMUX_STARTUP_TRACE").ok()?,
            std::process::id()
        );
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok()?;
        let mut w = BufWriter::new(f);
        let _ = writeln!(w, "# qpf {} pid {}", qpf(), std::process::id());
        let _ = w.flush();
        Some(Mutex::new(w))
    })
    .as_ref()
}

/// Record one startup hop.
pub fn mark(label: &str) {
    if !on() {
        return;
    }
    let t = qpc();
    if let Some(m) = sink() {
        if let Ok(mut w) = m.lock() {
            let _ = writeln!(w, "{} {}", t, label);
            let _ = w.flush();
        }
    }
}

/// Record one startup hop with a short detail field (a command line, a count).
pub fn mark_detail(label: &str, detail: &str) {
    if !on() {
        return;
    }
    let t = qpc();
    if let Some(m) = sink() {
        if let Ok(mut w) = m.lock() {
            let shown: String = detail.chars().take(200).collect();
            let _ = writeln!(w, "{} {} {}", t, label, shown);
            let _ = w.flush();
        }
    }
}
