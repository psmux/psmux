// Issue #860 side finding: `send-keys M-C-a` wrote ESC C (1B 43) where tmux
// writes ESC ^A (1B 01).
//
// tmux reads C-, M- and S- in any order and case (key-string.c:213
// key_string_get_modifiers) and writes a modified character in the standard
// mode as the meta prefix plus the standard_map byte, ignoring Shift
// (input-keys.c input_key_vt10x).  The send-keys arms matched fixed prefixes
// and took the character at a fixed offset, so every other order was wrong.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

fn canon(t: &str) -> Option<String> {
    canonical_modified_char_name(t)
}

#[test]
fn every_order_of_ctrl_meta_is_one_key() {
    for t in ["C-M-a", "M-C-a", "c-m-a", "m-c-a", "C-m-a", "M-c-a", "C-M-A", "M-C-A"] {
        assert_eq!(canon(t).as_deref(), Some("C-M-a"), "{t}");
    }
}

#[test]
fn shift_is_ignored_on_a_character_as_in_vt10x() {
    for t in ["C-M-S-a", "M-C-S-a", "S-M-C-a", "C-S-M-a", "S-C-M-a", "M-S-C-a"] {
        assert_eq!(canon(t).as_deref(), Some("C-M-a"), "{t}");
    }
    assert_eq!(canon("M-S-a").as_deref(), Some("M-a"));
    assert_eq!(canon("S-M-a").as_deref(), Some("M-a"));
    assert_eq!(canon("C-S-a").as_deref(), Some("C-a"), "was Ctrl+S (0x13)");
    assert_eq!(canon("S-C-a").as_deref(), Some("C-a"));
    assert_eq!(canon("S-a").as_deref(), Some("a"), "was typed as the text S-a");
}

#[test]
fn meta_keeps_the_case_of_a_letter() {
    // tmux: M-A is ESC A, M-a is ESC a.
    assert_eq!(canon("M-A").as_deref(), Some("M-A"));
    assert_eq!(canon("M-a").as_deref(), Some("M-a"));
}

#[test]
fn ctrl_shift_punctuation_keeps_its_record_arm() {
    // #394 / #623: C-S-<digit or punctuation> has its own arm.
    assert_eq!(canon("C-S-1").as_deref(), Some("C-S-1"));
    assert_eq!(canon("S-C-1").as_deref(), Some("C-S-1"));
    assert_eq!(canon("S-C--").as_deref(), Some("C-S--"));
}

#[test]
fn space_and_escape_in_every_order() {
    for t in ["M-C-Space", "C-M-Space", "C-S-M-Space", "M-C-S-Space", "m-c-space"] {
        assert_eq!(canon(t).as_deref(), Some("C-M-Space"), "{t}");
    }
    assert_eq!(canon("M-Space").as_deref(), Some("M-Space"), "was ESC S");
    assert_eq!(canon("C-Space").as_deref(), Some("C-Space"));
    assert_eq!(canon("S-C-Space").as_deref(), Some("C-Space"));
    assert_eq!(canon("M-Escape").as_deref(), Some("M-Escape"), "was ESC E");
}

#[test]
fn ctrl_meta_punctuation() {
    assert_eq!(canon("M-C-[").as_deref(), Some("C-M-["));
    assert_eq!(canon("M-C-1").as_deref(), Some("C-M-1"));
    assert_eq!(canon("M-C-/").as_deref(), Some("C-M-/"));
}

#[test]
fn not_a_modified_character_key() {
    for t in ["a", "Enter", "hello", "S-Mart", "C-Left", "M-Up", "S-Enter", "C-F5", "M-BSpace", "C-", "-", "C-M-ab"] {
        assert_eq!(canon(t), None, "{t}");
    }
}

#[test]
fn canonical_spellings_give_tmux_vt10x_bytes() {
    // The bytes the canonical arm writes, through the same standard_map port
    // the arms use, against tmux input_key_vt10x.
    let vt10x = |name: &str| -> Vec<u8> {
        let (code, mods) = crate::config::parse_key_string(name).unwrap();
        let KeyCode::Char(c) = code else { panic!("{name}") };
        let mut out = Vec::new();
        if mods.contains(KeyModifiers::ALT) { out.push(0x1b); }
        if mods.contains(KeyModifiers::CONTROL) {
            out.push(ctrl_char_send_keys_byte(c).unwrap());
        } else {
            out.push(c as u8);
        }
        out
    };
    assert_eq!(vt10x(&canon("M-C-a").unwrap()), vec![0x1b, 0x01]);
    assert_eq!(vt10x(&canon("M-C-[").unwrap()), vec![0x1b, 0x1b]);
    assert_eq!(vt10x(&canon("M-C-1").unwrap()), vec![0x1b, b'1']);
    assert_eq!(vt10x(&canon("M-S-a").unwrap()), vec![0x1b, b'a']);
    assert_eq!(vt10x(&canon("C-S-a").unwrap()), vec![0x01]);
}

#[test]
fn names_are_never_altgr_text_in_extended_mode() {
    use crate::extended_keys::{encode, encode_name};
    let cm = KeyModifiers::CONTROL | KeyModifiers::ALT;
    assert_eq!(encode(2, true, KeyCode::Char('['), cm), None, "a key EVENT Ctrl+Alt+[ is AltGr text");
    assert_eq!(encode_name(2, true, KeyCode::Char('['), cm), Some(b"\x1b[27;7;91~".to_vec()), "send-keys C-M-[");
    // tmux reports a NAMED Shift+character extended in both modes.
    assert_eq!(encode_name(2, true, KeyCode::Char('a'), KeyModifiers::SHIFT), Some(b"\x1b[27;2;97~".to_vec()));
    assert_eq!(encode_name(1, true, KeyCode::Char('a'), KeyModifiers::SHIFT), Some(b"\x1b[27;2;97~".to_vec()));
    assert_eq!(encode(2, true, KeyCode::Char('A'), KeyModifiers::SHIFT), None, "typed A stays text");
    assert_eq!(encode_name(0, true, KeyCode::Char('a'), KeyModifiers::SHIFT), None);
}

#[cfg(windows)]
#[test]
fn meta_keys_conhost_loses_become_records() {
    // M-C-Space: Ctrl+Alt+Space, no character.
    assert_eq!(meta_key_record(b"\x1b\x00"), Some((0x20, 0, LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED)));
    // M-Escape / C-M-[: Escape, written twice.
    assert_eq!(meta_key_record(b"\x1b\x1b"), Some((0x1B, 0x1b, 0)));
    // M-[ and M-O: Alt + the character.
    assert_eq!(meta_key_record(b"\x1b["), Some((0xDB, b'[' as u16, LEFT_ALT_PRESSED)));
    assert_eq!(meta_key_record(b"\x1bO"), Some((0x4F, b'O' as u16, LEFT_ALT_PRESSED | SHIFT_PRESSED)));
    // Everything else keeps its bytes.
    for b in [&b"\x1ba"[..], b"\x1b\r", b"\x1b\x7f", b"\x1b", b"\x1b[A", b"ab"] {
        assert_eq!(meta_key_record(b), None, "{b:?}");
    }
}

#[cfg(windows)]
#[test]
fn only_the_nul_needs_a_record_on_an_unlatched_conpty() {
    assert!(meta_key_needs_record(b"\x1b\x00", false));
    assert!(meta_key_needs_record(b"\x1b\x00", true));
    for b in [&b"\x1b["[..], b"\x1bO", b"\x1b\x1b"] {
        assert!(!meta_key_needs_record(b, false), "{b:?} unlatched keeps its bytes");
        assert!(meta_key_needs_record(b, true), "{b:?} latched needs the record");
    }
    assert!(!meta_key_needs_record(b"\x1ba", true));
}
