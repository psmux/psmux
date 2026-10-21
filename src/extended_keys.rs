//! Extended key encoding for a pane that asked for it (issue #860).
//!
//! tmux keeps an extended key mode per pane (`MODE_KEYS_EXTENDED` and
//! `MODE_KEYS_EXTENDED_2`, tmux.h), set when the application writes
//! `CSI > 4 ; 1 m` or `CSI > 4 ; 2 m` while the `extended-keys` option is on
//! (input.c INPUT_CSI_MODSET), or forced to mode 1 by `extended-keys always`.
//! In those modes a key with no builtin sequence goes out in the extended
//! form, `CSI 27 ; m ; k ~` (`extended-keys-format xterm`, the default) or
//! `CSI k ; m u` (`csi-u`), instead of its legacy byte (input-keys.c
//! `input_key`, `input_key_mode1`, `input_key_extended`).
//!
//! That is the only way Ctrl+Space can reach Neovim: its legacy byte is NUL,
//! which Neovim on Windows never reads as `<C-Space>`.  Neovim 0.12 asks for
//! mode 2 (`CSI > 4 ; 2 m`) at startup.
//!
//! The vt100 parser tracks the mode ([`vt100::Screen::extended_keys_mode`]);
//! this module is the pure encoder.  It returns `None` for every key whose
//! bytes the mode does not change, so the caller keeps its legacy path, byte
//! for byte, for those.

use crossterm::event::{KeyCode, KeyModifiers};

/// tmux's modifier bits in the order of the xterm parameter: the parameter
/// is 1 + Shift + 2 Meta + 4 Ctrl.
const SHIFT: u8 = 1;
const META: u8 = 2;
const CTRL: u8 = 4;

/// The extended bytes for `code` + `mods` in extended key `mode` (0, 1 or 2),
/// or `None` when tmux would write the same bytes as the standard mode, in
/// which case the caller's legacy path is the right one.
///
/// `xterm` selects `extended-keys-format xterm` (`CSI 27;m;k~`) over `csi-u`
/// (`CSI k;mu`).
///
/// This is the KEY EVENT form: a Ctrl+Alt punctuation character is what
/// Windows reports for AltGr, so it is text.  A key NAME is never AltGr and
/// goes through [`encode_name`].
pub fn encode(mode: u8, xterm: bool, code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    encode_inner(mode, xterm, code, mods, true)
}

/// [`encode`] for a key NAME (`send-keys C-M-[`): `C-M-[` is the chord tmux
/// names, so it is reported extended like any other (input-keys.c
/// input_key), where the key event form would take it for AltGr text.
pub fn encode_name(mode: u8, xterm: bool, code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    encode_inner(mode, xterm, code, mods, false)
}

fn encode_inner(mode: u8, xterm: bool, code: KeyCode, mods: KeyModifiers, key_event: bool) -> Option<Vec<u8>> {
    if mode == 0 {
        return None;
    }
    let (key, bits) = tmux_key(mode, code, mods, key_event)?;
    if bits == 0 {
        // input_key_extended returns -1 for an unmodified key; such a key
        // never gets this far in tmux (it is the trivial case).
        return None;
    }
    if mode == 1 && mode1_is_standard(key, bits) {
        return None;
    }
    let m = 1 + bits;
    Some(if xterm {
        format!("\x1b[27;{};{}~", m, key).into_bytes()
    } else {
        format!("\x1b[{};{}u", key, m).into_bytes()
    })
}

/// The key as tmux sees it at the extended stage of `input_key`: its code
/// point and its modifier bits.  `None` for keys that have a builtin sequence
/// (arrows, the editing block, function keys: tmux writes those as `CSI 1;m x`
/// and `CSI n;m ~` in every mode) or that are plain text.
fn tmux_key(mode: u8, code: KeyCode, mods: KeyModifiers, key_event: bool) -> Option<(u32, u8)> {
    let mut bits = 0u8;
    if mods.contains(KeyModifiers::SHIFT) { bits |= SHIFT; }
    if mods.contains(KeyModifiers::ALT) { bits |= META; }
    if mods.contains(KeyModifiers::CONTROL) { bits |= CTRL; }
    match code {
        KeyCode::Char(c) => {
            let cp = c as u32;
            // A raw control character with no modifiers, the second way
            // crossterm reports Ctrl+<letter> (and NUL for Ctrl+Space).
            if bits == 0 && cp == 0 {
                return Some((' ' as u32, CTRL));
            }
            if bits & (CTRL | META) == 0 && (0x01..=0x1A).contains(&cp) {
                return Some((cp + 0x60, CTRL));
            }
            // AltGr arrives as Ctrl+Alt with the character it produced
            // (`\`, `@`, `{`, a Euro sign): that is text, not a chord.
            if key_event && bits & (CTRL | META) == CTRL | META && !c.is_ascii_alphanumeric() && c != ' ' {
                return None;
            }
            // Shift alone on a typed character is the shifted character
            // itself (crossterm reports `A` with SHIFT).  A NAME such as
            // `S-a` is the key 'a'|KEYC_SHIFT, which tmux reports extended
            // in both modes (input_key_mode1 returns -1 for it).
            if key_event && bits & (CTRL | META) == 0 {
                return None;
            }
            Some((cp, bits))
        }
        // tmux keys C0_CR, C0_HT, C0_ESC and the `backspace` option's \177:
        // unmodified they are the trivial case, modified they have no
        // builtin sequence.
        KeyCode::Enter => (bits != 0).then_some((0x0D, bits)),
        KeyCode::Tab => (bits != 0).then_some((0x09, bits)),
        KeyCode::Esc => (bits != 0).then_some((0x1B, bits)),
        KeyCode::Backspace => (bits != 0).then_some((0x7F, bits)),
        // KEYC_BTAB: in mode 2 tmux remaps it into S-Tab and reports it
        // extended; otherwise it drops the modifiers and writes `CSI Z`.
        KeyCode::BackTab if mode == 2 => Some((0x09, bits | SHIFT)),
        _ => None,
    }
}

/// tmux `input_key_mode1`: in mode 1 a Meta (no Ctrl) key, and a Ctrl key
/// whose legacy byte is one of the classic control characters, keep the
/// standard (VT10x) encoding "to maintain compatibility with applications
/// unaware of extended keys" (input-keys.c).
fn mode1_is_standard(key: u32, bits: u8) -> bool {
    if bits & (CTRL | META) == META {
        return true;
    }
    if bits & CTRL != 0 {
        let Some(c) = char::from_u32(key) else { return false };
        return c == ' '
            || c == '/'
            || c == '@'
            || c == '^'
            || ('2'..='8').contains(&c)
            || ('@'..='~').contains(&c);
    }
    false
}

/// tmux `#{pane_key_mode}` text for a mode number.
pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        1 => "Ext 1",
        2 => "Ext 2",
        _ => "VT10x",
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue860_extended_keys_mode.rs"]
mod tests_issue860_extended_keys_mode;
