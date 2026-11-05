//! tmux's user keys: `user-keys[N]` names a byte sequence the outer terminal
//! sends, and that sequence arriving is the key `UserN` (options-table.c:
//! 734-743, tty-keys.c:533-540 and key-string.c:206 at tag 3.8; N is 0 to
//! KEYC_NUSER, 1000, tmux.h:164). `bind User0 ...` binds it like any other
//! key, a `UserN` with no binding sends nothing to the pane (input-keys.c:676),
//! and `send-keys User0` sends nothing either.
//!
//! psmux keys are `(KeyCode, KeyModifiers)` pairs, so a user key is stored as
//! a `KeyCode::Char` in a private use area, right after the mouse keys
//! (`key_base::user_base()`, U+10E000 + 3200 = U+10EC80 to U+10F068, clear
//! of the Nerd Fonts icons), so bind, unbind, list-keys and the client's binding sync carry them without
//! a separate store.
//!
//! Matching the sequences needs the bytes the outer terminal sent, which only
//! the VT and pipe input routes have (`ssh_input::VtParser`). On the console
//! route the console host parses VT input before psmux sees it: a known
//! sequence becomes a key record (`ESC[5;30012~` arrives as PageUp with
//! Shift and Alt) and an unknown one is dropped (`ESC[99~`), measured with
//! both the inbox conhost and OpenConsole. The client learns the sequences
//! from the server's render options and publishes them here for its input
//! reader thread.

use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

pub const OPTION: &str = "user-keys";

/// tmux KEYC_NUSER: the highest user key number.
pub const MAX_USER: u32 = 1000;

// The user keys take key_base::USER_SPAN characters: User0 to User1000.
const _: () = assert!(MAX_USER + 1 == crate::key_base::USER_SPAN);

/// The stored character of `UserN`.
pub fn char_for(n: u32) -> Option<char> {
    if n > MAX_USER {
        return None;
    }
    char::from_u32(crate::key_base::user_base() + n)
}

/// True when `c` is the stored form of a user key.
pub fn is_user_char(c: char) -> bool {
    let base = crate::key_base::user_base();
    (base..=base + MAX_USER).contains(&(c as u32))
}

/// True when `key` is a user key, with any modifiers.
pub fn is_user_key(key: &(KeyCode, KeyModifiers)) -> bool {
    matches!(key.0, KeyCode::Char(c) if is_user_char(c))
}

/// `UserN` for a stored user key character.
pub fn name(c: char) -> Option<String> {
    is_user_char(c).then(|| format!("User{}", c as u32 - crate::key_base::user_base()))
}

/// The user key a name without modifiers names: `User0` to `User1000`, read
/// as tmux's `sscanf(string, "User%u", &user)` reads it (key-string.c:206),
/// case sensitive. Anything else, `User1001` included, is not a user key.
pub fn parse_base(name: &str) -> Option<KeyCode> {
    let digits = name.strip_prefix("User")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = digits.parse().ok()?;
    char_for(n).map(KeyCode::Char)
}

/// The user key with its modifiers (`M-User2`), or `None` for any other name.
pub fn parse(name: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut rest = name;
    let mut mods = KeyModifiers::NONE;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            mods |= KeyModifiers::CONTROL;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-") {
            mods |= KeyModifiers::ALT;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("S-") {
            mods |= KeyModifiers::SHIFT;
            rest = r;
        } else {
            break;
        }
    }
    parse_base(rest).map(|code| (code, mods))
}

/// The sequences the client matches, `(N, sequence)`, from the server.
pub type Table = Arc<Vec<(u32, String)>>;

static ANY: AtomicBool = AtomicBool::new(false);
static TABLE: RwLock<Option<Table>> = RwLock::new(None);

/// True for an element the input reader can match: numbered 0 to
/// `MAX_USER` (tmux reads no other, tty-keys.c:533-540), not empty (an empty
/// string adds no key), and holding no U+FFFD.
///
/// U+FFFD is what a byte that is no UTF-8 became (`"\377"` in the config is
/// decoded to it, as psmux keeps option values as text), and on the VT route
/// the console host turns every such byte the terminal sends into U+FFFD
/// before psmux reads it (measured with conhost and OpenConsole: `ESC 0xFF`
/// arrives as ESC, U+FFFD), so the element would match any of them; the pipe
/// route drops such bytes. tmux matches the byte itself; psmux cannot, and
/// leaves the element out rather than match the wrong key.
pub fn matchable(n: u32, seq: &str) -> bool {
    n <= MAX_USER && !seq.is_empty() && !seq.contains('\u{FFFD}')
}

/// Replace the sequences the input reader matches (the client calls this when
/// the server's `user-keys` changes), keeping only the [`matchable`] ones.
pub fn set_sequences(entries: &[(u32, String)]) {
    let kept: Vec<(u32, String)> = entries
        .iter()
        .filter(|(n, s)| matchable(*n, s))
        .cloned()
        .collect();
    let any = !kept.is_empty();
    if let Ok(mut table) = TABLE.write() {
        *table = any.then(|| Arc::new(kept));
    }
    ANY.store(any, Ordering::Release);
}

/// True when there is at least one sequence to match: the input reader's
/// fast gate, so a client with no user keys pays one atomic load per char.
#[inline]
pub fn any() -> bool {
    ANY.load(Ordering::Acquire)
}

/// True when the client should drop `c` as an unbound user key rather than
/// pass it on as text.
///
/// The stored range is also real text: a font may draw a character there,
/// and an IME commit, a paste or the emoji picker delivers such a glyph as a
/// key event for that character. The range (`key_base`) is clear of the
/// Nerd Fonts and SF Symbols glyphs (before it moved, it shared U+F1000 to
/// U+F13E8 with the Nerd Fonts Material Design icons), but another font may
/// still draw there. Only the VT and pipe input reader
/// ever turns a sequence into a user key, and only while `user-keys` has
/// elements, so anywhere else the character is the glyph the user typed and
/// goes on as text: on the console route (`vt_route` false, where the console
/// host parses the sequences before psmux reads them), and on any route while
/// the option is empty. `have_sequences` is [`any`], passed in so the rule is
/// testable.
pub fn drop_as_unbound(c: char, vt_route: bool, have_sequences: bool) -> bool {
    vt_route && have_sequences && is_user_char(c)
}

/// The sequences published by [`set_sequences`], if there are any.
pub fn table() -> Option<Table> {
    if !any() {
        return None;
    }
    TABLE.read().ok().and_then(|t| t.clone())
}

/// What the characters held so far are, against the sequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    /// Exactly one sequence, and no longer one starts with it: `UserN`.
    Key(u32),
    /// A whole sequence that a longer one also starts with: `UserN` if
    /// nothing more arrives in time.
    KeyOrLonger(u32),
    /// The start of at least one sequence: hold and wait for more.
    Prefix,
    /// No sequence starts with it.
    None,
}

/// Look `held` up. When two elements hold the same sequence, the higher index
/// wins: tmux adds them from 0 up and `tty_keys_add` replaces the key of a
/// sequence already in the tree (tty-keys.c:442-448 at tag 3.8), which is
/// also why a user key wins over a built-in key with the same sequence.
pub fn classify_in(table: &[(u32, String)], held: &str) -> Match {
    if held.is_empty() {
        return Match::None;
    }
    let mut exact: Option<u32> = None;
    let mut longer = false;
    for (n, seq) in table {
        if seq == held {
            exact = Some(exact.map_or(*n, |e| e.max(*n)));
        } else if seq.starts_with(held) {
            longer = true;
        }
    }
    match (exact, longer) {
        (Some(n), false) => Match::Key(n),
        (Some(n), true) => Match::KeyOrLonger(n),
        (None, true) => Match::Prefix,
        (None, false) => Match::None,
    }
}

/// `classify_in`, and when that finds nothing, the same lookup after a
/// leading ESC. tmux reads ESC and then a key as that key with Meta
/// (`tty_keys_next`, tty-keys.c at tag 3.8), so ESC and a sequence is
/// `M-UserN`; the second value says the match is that Meta form. A lone ESC
/// is held as its start, since any sequence may follow it.
pub fn classify_meta_in(table: &[(u32, String)], held: &str) -> (Match, bool) {
    let whole = classify_in(table, held);
    if whole != Match::None {
        return (whole, false);
    }
    match held.strip_prefix('\x1b') {
        Some("") if !table.is_empty() => (Match::Prefix, true),
        Some(rest) if !rest.is_empty() => match classify_in(table, rest) {
            Match::None => (Match::None, false),
            m => (m, true),
        },
        _ => (Match::None, false),
    }
}

/// The key event the input reader hands the client for `UserN`.
pub fn key_event(n: u32) -> Option<crossterm::event::Event> {
    key_event_with(n, false)
}

/// The key event for `UserN`, or for `M-UserN` when `meta` is set.
pub fn key_event_with(n: u32, meta: bool) -> Option<crossterm::event::Event> {
    let c = char_for(n)?;
    let mods = if meta { KeyModifiers::ALT } else { KeyModifiers::NONE };
    Some(crossterm::event::Event::Key(crossterm::event::KeyEvent::new(KeyCode::Char(c), mods)))
}

#[cfg(test)]
#[path = "../tests-rs/test_user_keys.rs"]
mod tests_user_keys;
