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
//! The keys take U+10E000 to U+10F068, near the end of Supplementary Private
//! Use Area-B, far from both. Every key's name, not its character, is what
//! crosses the wire (the binding sync, `list-keys`, the config), so the place
//! can move again without breaking a client or a server of another version.

/// The first character: the first mouse key.
pub const BASE: u32 = 0x10E000;

/// How many characters the mouse keys take: 10 events x 16 buttons x 20
/// locations (`mouse_keys`, which checks this at compile time).
pub const MOUSE_SPAN: u32 = 3200;

/// How many characters the user keys take: `User0` to `User1000`
/// (`user_keys`, which checks this at compile time).
pub const USER_SPAN: u32 = 1001;

/// Every character the keys take, from the first one on.
pub const TOTAL: u32 = MOUSE_SPAN + USER_SPAN;

// Every key stays inside Supplementary Private Use Area-B (U+100000 to
// U+10FFFD), so each one is a valid scalar and none is a noncharacter.
const _: () = assert!(BASE >= 0x100000 && BASE + TOTAL - 1 <= 0x10FFFD);

/// The first mouse key character.
#[inline]
pub const fn mouse_base() -> u32 {
    BASE
}

/// The first user key character, right after the mouse keys.
#[inline]
pub const fn user_base() -> u32 {
    BASE + MOUSE_SPAN
}

#[cfg(test)]
#[path = "../tests-rs/test_key_base.rs"]
mod tests_key_base;
