//! `terminal-overrides` (issue #700).
//!
//! tmux keeps `terminal-overrides` as a server scope ARRAY option
//! (options-table.c: `OPTIONS_TABLE_IS_ARRAY`, `.separator = ","`), and so
//! does psmux, on crate::option_array with tmux's default `linux*:AX@`. Every
//! element is `pattern:cap[@|=value]:cap...`, the first field is matched with
//! `fnmatch(3)` against the TERM of the attaching client, and the rest is
//! applied in order on top of that client's terminfo entry
//! (tty-term.c: `tty_term_apply_overrides`, `tty_term_override_next`,
//! `tty_term_apply`). A `::` inside a field is a literal colon.
//!
//! psmux has no terminfo database: the attached client writes VT sequences
//! directly. The only capabilities that change what the client emits are
//! therefore the ones psmux itself would otherwise send unconditionally, and
//! of those it honours `smcup` and `rmcup`, the pair that enters and leaves
//! the host terminal's alternate screen. Every other capability is parsed
//! (so the entries round trip through `show-options` verbatim) and ignored.
//!
//! TERM on Windows: a native console usually has no TERM at all. tmux's own
//! client sends an empty name in that case (client.c: `if ((termname =
//! getenv("TERM")) == NULL) termname = "";`), so psmux does the same: an unset
//! TERM is matched as the empty string. `*` matches it, `xterm*` does not.

use std::sync::atomic::{AtomicU8, Ordering};

/// Split one element into its `:` separated fields, `::` being a literal
/// colon (tmux `tty_term_override_next`).
pub fn split_fields(entry: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = entry.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' {
            if chars.peek() == Some(&':') {
                chars.next();
                cur.push(':');
            } else {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// What one capability field does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapOp {
    /// `name@`: the capability is removed.
    Remove,
    /// `name=value` (value already unescaped) or a bare `name` (empty value).
    Set(String),
}

/// A parsed element: the TERM pattern and its capability operations in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideEntry {
    pub pattern: String,
    pub caps: Vec<(String, CapOp)>,
}

/// Parse one array element. Returns None for an element with no pattern
/// field, which tmux skips the same way (`first == NULL`).
pub fn parse_entry(entry: &str) -> Option<OverrideEntry> {
    let fields = split_fields(entry);
    let mut it = fields.into_iter();
    let pattern = it.next()?;
    let mut caps = Vec::new();
    for field in it {
        if field.is_empty() {
            continue;
        }
        if let Some(eq) = field.find('=') {
            let name = field[..eq].to_string();
            caps.push((name, CapOp::Set(unescape(&field[eq + 1..]))));
        } else if let Some(name) = field.strip_suffix('@') {
            caps.push((name.to_string(), CapOp::Remove));
        } else {
            caps.push((field, CapOp::Set(String::new())));
        }
    }
    Some(OverrideEntry { pattern, caps })
}

/// The subset of `strunvis(3)` escapes that terminfo style values use:
/// `\E` / `\e` (ESC), `\\`, `\n`, `\r`, `\t`, `\a`, `\b`, and octal `\NNN`.
/// Anything else keeps its backslash, as tmux falls back to the raw value when
/// `strunvis` fails.
pub fn unescape(s: &str) -> String {
    let b: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != '\\' || i + 1 >= b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        let n = b[i + 1];
        match n {
            'E' | 'e' => { out.push('\x1b'); i += 2; }
            '\\' => { out.push('\\'); i += 2; }
            'n' => { out.push('\n'); i += 2; }
            'r' => { out.push('\r'); i += 2; }
            't' => { out.push('\t'); i += 2; }
            'a' => { out.push('\x07'); i += 2; }
            'b' => { out.push('\x08'); i += 2; }
            '0'..='7' => {
                let mut v: u32 = 0;
                let mut j = i + 1;
                while j < b.len() && j < i + 4 && ('0'..='7').contains(&b[j]) {
                    v = v * 8 + (b[j] as u32 - '0' as u32);
                    j += 1;
                }
                if let Some(c) = char::from_u32(v) { out.push(c); }
                i = j;
            }
            _ => { out.push('\\'); out.push(n); i += 2; }
        }
    }
    out
}

/// `fnmatch(pattern, name, 0)`: `*`, `?`, bracket expressions (with `!` or
/// `^` negation and ranges), and backslash quoting.
pub fn fnmatch(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = name.chars().collect();
    fnmatch_at(&p, 0, &t, 0)
}

fn fnmatch_at(p: &[char], mut pi: usize, t: &[char], mut ti: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                while pi < p.len() && p[pi] == '*' { pi += 1; }
                if pi == p.len() { return true; }
                for k in ti..=t.len() {
                    if fnmatch_at(p, pi, t, k) { return true; }
                }
                return false;
            }
            '?' => {
                if ti >= t.len() { return false; }
                pi += 1; ti += 1;
            }
            '[' => {
                if ti >= t.len() { return false; }
                match bracket(p, pi, t[ti]) {
                    Some((matched, next)) => {
                        if !matched { return false; }
                        pi = next; ti += 1;
                    }
                    // An unterminated `[` is an ordinary character.
                    None => {
                        if t[ti] != '[' { return false; }
                        pi += 1; ti += 1;
                    }
                }
            }
            '\\' if pi + 1 < p.len() => {
                if ti >= t.len() || t[ti] != p[pi + 1] { return false; }
                pi += 2; ti += 1;
            }
            c => {
                if ti >= t.len() || t[ti] != c { return false; }
                pi += 1; ti += 1;
            }
        }
    }
    ti == t.len()
}

/// Match `c` against the bracket expression starting at `p[start] == '['`.
/// Returns (matched, index after the closing `]`), or None if unterminated.
fn bracket(p: &[char], start: usize, c: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = i < p.len() && (p[i] == '!' || p[i] == '^');
    if negate { i += 1; }
    let mut matched = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        let mut lo = p[i];
        if lo == '\\' && i + 1 < p.len() { i += 1; lo = p[i]; }
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let mut hi = p[i + 2];
            let mut skip = 3;
            if hi == '\\' && i + 3 < p.len() { hi = p[i + 3]; skip = 4; }
            if lo <= c && c <= hi { matched = true; }
            i += skip;
        } else {
            if lo == c { matched = true; }
            i += 1;
        }
    }
    None
}

/// Whether `smcup` and `rmcup` survive the overrides for this TERM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AltScreenCaps {
    /// Enter the alternate screen on attach (`smcup`).
    pub smcup: bool,
    /// Leave the alternate screen on exit (`rmcup`).
    pub rmcup: bool,
}

/// Apply every element whose pattern matches `term`, in array order, and
/// report whether `smcup` / `rmcup` are still present. A capability set to an
/// empty string counts as absent: tmux would emit nothing for it.
pub fn alt_screen_caps<S: AsRef<str>>(overrides: &[S], term: &str) -> AltScreenCaps {
    let mut caps = AltScreenCaps { smcup: true, rmcup: true };
    for raw in overrides {
        let Some(entry) = parse_entry(raw.as_ref()) else { continue };
        if !fnmatch(&entry.pattern, term) {
            continue;
        }
        for (name, op) in &entry.caps {
            let present = match op {
                CapOp::Remove => false,
                CapOp::Set(v) => !v.is_empty(),
            };
            match name.as_str() {
                "smcup" => caps.smcup = present,
                "rmcup" => caps.rmcup = present,
                _ => {}
            }
        }
    }
    caps
}

/// The TERM the client matches against: the environment value, or the empty
/// string when unset (tmux client.c does the same).
pub fn client_term() -> String {
    std::env::var("TERM").unwrap_or_default()
}

// ── Client screen state ────────────────────────────────────────────────────
//
// The client used to enter the alternate screen before it had spoken to the
// server. The decision now waits for the first frame, which carries the
// server's `terminal-overrides`, and is taken once per client process so a
// session switch never flips screens mid attach.

const SCREEN_IDLE: u8 = 0;
const SCREEN_PENDING: u8 = 1;
const SCREEN_ALT: u8 = 2;
const SCREEN_MAIN: u8 = 3;

static SCREEN_STATE: AtomicU8 = AtomicU8::new(SCREEN_IDLE);
static RMCUP: AtomicU8 = AtomicU8::new(1);

/// Called by the attach path where it used to enter the alternate screen.
pub fn arm_client_screen() {
    SCREEN_STATE.store(SCREEN_PENDING, Ordering::SeqCst);
}

/// Called before every client draw; acts only on the first one after
/// [`arm_client_screen`]. Emits `ESC[?1049h` unless `smcup` is overridden
/// away, in which case the main screen is cleared instead, as tmux does
/// (tty_start_tty sends `clear` after `smcup`), so the first frame is drawn
/// onto a blank screen.
pub fn client_screen_start<W: std::io::Write>(out: &mut W, overrides: &[String]) {
    if SCREEN_STATE
        .compare_exchange(SCREEN_PENDING, SCREEN_IDLE, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let caps = alt_screen_caps(overrides, &client_term());
    RMCUP.store(caps.rmcup as u8, Ordering::SeqCst);
    if caps.smcup {
        let _ = crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen);
        SCREEN_STATE.store(SCREEN_ALT, Ordering::SeqCst);
    } else {
        let _ = out.write_all(b"\x1b[H\x1b[2J");
        let _ = out.flush();
        SCREEN_STATE.store(SCREEN_MAIN, Ordering::SeqCst);
    }
}

/// Called once on client exit. With `rmcup` present this leaves the alternate
/// screen exactly as before. With `rmcup@` it clears the screen and homes the
/// cursor instead, which is what tmux's tty_stop_tty leaves behind. Nothing is
/// emitted if the client never drew a frame.
pub fn client_screen_stop<W: std::io::Write>(out: &mut W) {
    let state = SCREEN_STATE.swap(SCREEN_IDLE, Ordering::SeqCst);
    if state != SCREEN_ALT && state != SCREEN_MAIN {
        return;
    }
    if RMCUP.load(Ordering::SeqCst) != 0 {
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
    } else {
        let _ = out.write_all(b"\x1b[H\x1b[2J");
        let _ = out.flush();
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue700_terminal_overrides.rs"]
mod tests_issue700_terminal_overrides;
