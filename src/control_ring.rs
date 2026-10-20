//! The raw pane output that control clients are sent as `%output` (#875).
//!
//! Each pane's PTY reader thread appends what it reads to the pane's
//! `output_ring`, and the server loop takes it from there for the control
//! clients. The ring used to keep only the newest 64 KiB and throw away the
//! oldest bytes when a read did not fit, so a pane that wrote more than that
//! between two passes of the loop lost output for every control client
//! without a word, while its screen and history had everything.
//!
//! tmux never drops a control client's output silently: a pane's output
//! stays buffered until every client has consumed it, and when the only
//! attached clients are control clients that cannot take more it stops
//! reading from the pane (server-client.c server_client_check_pane_buffer),
//! so the program blocks on its write. A client that falls behind is told,
//! with `%pause` (pause-after) or `%exit too far behind` (control.c
//! control_check_age).
//!
//! So a ring the server loop drains for control clients ("consumed") never
//! drops: past `RING_HIGH` the reader waits for the loop to take the bytes,
//! which holds the pane's program on its write the way tmux does. A ring
//! nobody consumes (no control client, a popup, a floating pane, a warm
//! spare) keeps the old bounded behaviour and never blocks its reader, and a
//! ring is emptied when it starts being consumed, so a control client that
//! attaches is sent only output written after it attached, as in tmux.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub type Ring = Arc<Mutex<VecDeque<u8>>>;

/// Bytes a ring nobody consumes keeps (the old cap).
pub const RING_UNCONSUMED_MAX: usize = 65536;
/// A consumed ring holding this much makes its reader wait for the loop.
pub const RING_HIGH: usize = 1 << 20;
/// Largest `%output` payload sent in one line. The old ring never held more
/// than 64 KiB, so no line was ever longer; keep it that way.
pub const OUTPUT_CHUNK_MAX: usize = 65536;

/// The rings the server loop currently drains for control clients, by
/// address. Rebuilt by the loop on every pass from the panes in the window
/// trees, so a ring of a closed pane leaves the set at once.
static CONSUMED: Mutex<Option<HashSet<usize>>> = Mutex::new(None);
/// Signalled after the loop drains rings, for readers waiting on a full one.
static DRAINED: Condvar = Condvar::new();
/// True while the server has control clients: their output needs the loop
/// woken for it even when no TUI client is watching (`wake_server_loop`).
pub static CONTROL_CLIENTS_PRESENT: AtomicBool = AtomicBool::new(false);

fn key(ring: &Ring) -> usize {
    Arc::as_ptr(ring) as *const () as usize
}

fn consumed_set() -> std::sync::MutexGuard<'static, Option<HashSet<usize>>> {
    CONSUMED.lock().unwrap_or_else(|e| e.into_inner())
}

fn is_consumed(set: &Option<HashSet<usize>>, k: usize) -> bool {
    set.as_ref().map_or(false, |s| s.contains(&k))
}

/// Reader side: append freshly read pane output. A consumed ring never drops
/// bytes; once it holds `RING_HIGH` this waits until the server loop drained
/// it (or stopped consuming it, or the pane is gone).
pub fn push(ring: &Ring, data: &[u8]) {
    let k = key(ring);
    let consumed = is_consumed(&consumed_set(), k);
    {
        let mut buf = ring.lock().unwrap_or_else(|e| e.into_inner());
        if !consumed {
            let space = RING_UNCONSUMED_MAX.saturating_sub(buf.len());
            if data.len() > space {
                let drop_count = (data.len() - space).min(buf.len());
                buf.drain(..drop_count);
            }
            let skip = data.len().saturating_sub(RING_UNCONSUMED_MAX);
            buf.extend(&data[skip..]);
            return;
        }
        buf.extend(data);
        if buf.len() < RING_HIGH {
            return;
        }
    }
    // Full: hold this pane's reader (and so its program) until the loop has
    // taken the bytes, as tmux stops reading a pane its clients cannot take.
    loop {
        crate::types::PTY_DATA_READY.store(true, Ordering::Release);
        crate::types::wake_server_loop();
        let set = consumed_set();
        // The pane was closed (only this reader still holds the ring), or
        // the loop stopped draining it: nobody will make room.
        if !is_consumed(&set, k) || Arc::strong_count(ring) <= 1 {
            return;
        }
        if ring.lock().map_or(0, |b| b.len()) < RING_HIGH {
            return;
        }
        let _ = DRAINED.wait_timeout(set, Duration::from_millis(10));
    }
}

/// The length of the longest prefix of `bytes` that does not end inside a
/// UTF-8 sequence, so a character split across two reads is sent whole in
/// the later `%output` instead of as two replacement characters.
pub fn complete_utf8_len(bytes: &[u8]) -> usize {
    let n = bytes.len();
    // Look back at most 3 bytes for the lead byte of a trailing sequence.
    for back in 1..=3.min(n) {
        let b = bytes[n - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte, keep looking for the lead
        }
        let need = if b & 0xE0 == 0xC0 {
            2
        } else if b & 0xF0 == 0xE0 {
            3
        } else if b & 0xF8 == 0xF0 {
            4
        } else {
            return n; // ASCII or invalid: nothing to hold back
        };
        return if back < need { n - back } else { n };
    }
    n
}

/// Server side: take what the ring holds for the control clients, leaving
/// only an unfinished UTF-8 character for the next pass.
pub fn drain(ring: &Ring) -> Vec<u8> {
    let mut buf = ring.lock().unwrap_or_else(|e| e.into_inner());
    if buf.is_empty() {
        return Vec::new();
    }
    let all: Vec<u8> = buf.drain(..).collect();
    let keep = complete_utf8_len(&all);
    buf.extend(&all[keep..]);
    let mut all = all;
    all.truncate(keep);
    all
}

/// Wake readers waiting on a full ring after a drain pass.
pub fn notify_drained() {
    DRAINED.notify_all();
}

/// Server side, once per pass: the rings drained for control clients from now
/// on. Pass an empty list when no control client is left. When `attach` is
/// set (a control client just registered) and no ring was being consumed
/// before, every ring is emptied first: what they hold was written before any
/// control client was there, and tmux sends a client only output written
/// after it attached. A pane created later joins the set without being
/// emptied, so its first output (the shell's prompt) is kept.
pub fn set_consumed(rings: &[&Ring], attach: bool) {
    let mut set = consumed_set();
    let old = set.take().unwrap_or_default();
    let first = attach && old.is_empty();
    let mut new = HashSet::with_capacity(rings.len());
    for r in rings {
        if first {
            if let Ok(mut b) = r.lock() {
                b.clear();
            }
        }
        new.insert(key(r));
    }
    let changed = old != new;
    *set = if new.is_empty() { None } else { Some(new) };
    drop(set);
    if changed {
        notify_drained();
    }
}

/// Split pane output into `%output` payloads of at most `OUTPUT_CHUNK_MAX`
/// bytes, on character boundaries.
pub fn chunks(data: String) -> Vec<String> {
    if data.len() <= OUTPUT_CHUNK_MAX {
        return vec![data];
    }
    let mut out = Vec::new();
    let mut rest = data.as_str();
    while rest.len() > OUTPUT_CHUNK_MAX {
        let mut cut = OUTPUT_CHUNK_MAX;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        out.push(rest[..cut].to_string());
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

#[cfg(test)]
#[path = "../tests-rs/test_issue875_cc_output_ring.rs"]
mod tests_issue875_cc_output_ring;
