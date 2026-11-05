//! Where psmux keeps the keys it stores as characters: the tmux mouse keys
//! (`mouse_keys`, `WheelUpPane`, `MouseDown1Pane`, ...) and then the user keys
//! (`user_keys`, `User0` to `User1000`).
//!
//! psmux keys are `(KeyCode, KeyModifiers)` pairs, and those two kinds of key
//! have no `KeyCode` of their own, so each is a `KeyCode::Char` in a private
//! use area. The characters are real text too: a font may draw them, and an
//! IME commit, a paste or an emoji picker delivers such a glyph as a key
//! event for that character. A glyph equal to a bound mouse key or user key
//! runs that binding instead of reaching the pane. So the keys sit where the
//! common icon fonts do not:
//!
//! - Nerd Fonts 3.5.1 (`glyphnames.json`): 4092 icons in the BMP private use
//!   area and 6896 Material Design icons at U+F0001 to U+F1AF0, in
//!   Supplementary Private Use Area-A, where these keys were before. None in
//!   Supplementary Private Use Area-B.
//! - Apple's SF Symbols 5.1: 5359 symbols packed from U+100000 to U+1022CF,
//!   in Supplementary Private Use Area-B.
//!
//! The keys start at U+10E000 in Supplementary Private Use Area-B by default,
//! far from both, and `PSMUX_KEY_BASE` (hex) moves them for a font that does
//! use that spot. Every key's name, not its character, is what crosses the
//! wire (the binding sync, `list-keys`, the config), so a client and a server
//! that read different values still agree on every binding.

use std::sync::OnceLock;

/// The environment variable that moves the keys: the first character, in
/// hex (`10E000`, `0x10E000` or `U+10E000`).
pub const ENV: &str = "PSMUX_KEY_BASE";

/// The first character by default.
pub const DEFAULT: u32 = 0x10E000;

/// How many characters the mouse keys take: 10 events x 16 buttons x 20
/// locations (`mouse_keys`, which checks this at compile time).
pub const MOUSE_SPAN: u32 = 3200;

/// How many characters the user keys take: `User0` to `User1000`
/// (`user_keys`, which checks this at compile time).
pub const USER_SPAN: u32 = 1001;

/// Every character the keys take, from the first one on.
pub const TOTAL: u32 = MOUSE_SPAN + USER_SPAN;

/// The private use areas a whole block of keys may sit in.
const AREAS: [(u32, u32); 2] = [
    (0xF0000, 0xFFFFD),   // Supplementary Private Use Area-A
    (0x100000, 0x10FFFD), // Supplementary Private Use Area-B
];

/// The first character `value` (the variable's text, `None` when unset)
/// asks for. Every one of the `TOTAL` characters from there must sit inside
/// one supplementary private use area; anything else is refused with the
/// reason, and the caller keeps `DEFAULT`.
pub fn parse(value: Option<&str>) -> Result<u32, String> {
    let Some(raw) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(DEFAULT);
    };
    let hex = raw
        .strip_prefix("U+")
        .or_else(|| raw.strip_prefix("u+"))
        .or_else(|| raw.strip_prefix("0x"))
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw);
    let first = u32::from_str_radix(hex, 16)
        .map_err(|_| format!("{}={}: not a hex code point; the keys stay at U+{:X}", ENV, raw, DEFAULT))?;
    let last = first.saturating_add(TOTAL - 1);
    if AREAS.iter().any(|&(lo, hi)| first >= lo && last <= hi) {
        Ok(first)
    } else {
        Err(format!(
            "{}={}: U+{:X} to U+{:X} is not inside one supplementary private use area \
             (U+F0000 to U+FFFFD, or U+100000 to U+10FFFD); the keys stay at U+{:X}",
            ENV, raw, first, last, DEFAULT
        ))
    }
}

fn resolved() -> &'static (u32, Option<String>) {
    static BASE: OnceLock<(u32, Option<String>)> = OnceLock::new();
    BASE.get_or_init(|| match parse(std::env::var(ENV).ok().as_deref()) {
        Ok(first) => (first, None),
        Err(why) => (DEFAULT, Some(why)),
    })
}

/// The first character of the keys for this process, read once.
#[inline]
pub fn base() -> u32 {
    resolved().0
}

/// Why `PSMUX_KEY_BASE` was refused, when it was: the server lists it with
/// the config warnings at startup.
pub fn refused() -> Option<&'static str> {
    resolved().1.as_deref()
}

/// The first mouse key character.
#[inline]
pub fn mouse_base() -> u32 {
    base()
}

/// The first user key character, right after the mouse keys.
#[inline]
pub fn user_base() -> u32 {
    base() + MOUSE_SPAN
}
