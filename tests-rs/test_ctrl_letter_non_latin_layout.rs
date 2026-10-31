// Ctrl + a letter key on a non Latin keyboard layout (Russian, Ukrainian).
//
// crossterm 0.29 reads a Ctrl+letter record's control character in uChar and
// maps it back to a letter with ToUnicodeEx on the active layout, so on
// Russian Ctrl+V (vk 0x56, uChar 0x16) comes out as Char('м') with CONTROL.
// The client has no control code for that and the key never reached the
// pane. The tap now decodes such a record as Ctrl plus the Latin letter the
// virtual key names, and leaves every record a Latin layout types to
// crossterm. These tests cover `ConsoleTap::classify_with` only; the layout
// lookup is passed in.

use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

const LCTRL: u32 = 0x0008;
const RCTRL: u32 = 0x0004;
const LALT: u32 = 0x0002;
const RALT: u32 = 0x0001;
const SHIFT: u32 = 0x0010;
const CAPS: u32 = 0x0080;

fn rec(down: bool, vk: u16, u: u16, ctrl_state: u32) -> Option<KeyRec> {
    Some(KeyRec { key_down: down, vk, u_char: u, ctrl_state })
}

fn key(c: char, modifiers: KeyModifiers, kind: KeyEventKind) -> Verdict {
    Verdict::Consumed(Some(Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers, kind, state: KeyEventState::empty() })))
}

/// The Russian layout's letter on the keys these tests use.
fn russian(r: &KeyRec) -> Option<char> {
    match r.vk {
        0x42 => Some('и'),
        0x43 => Some('с'),
        0x49 => Some('ш'),
        0x56 => Some('м'),
        _ => None,
    }
}

fn latin(r: &KeyRec) -> Option<char> {
    ((0x41..=0x5A).contains(&r.vk)).then(|| (r.vk as u8 as char).to_ascii_lowercase())
}

#[test]
fn russian_ctrl_v_is_ctrl_v_press_and_release() {
    let mut tap = ConsoleTap::new();
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL), russian), key('v', KeyModifiers::CONTROL, KeyEventKind::Press));
    assert_eq!(tap.classify_with(rec(false, 0x56, 0x16, LCTRL), russian), key('v', KeyModifiers::CONTROL, KeyEventKind::Release));
    // No uChar at all, right Ctrl.
    assert_eq!(tap.classify_with(rec(true, 0x42, 0, RCTRL), russian), key('b', KeyModifiers::CONTROL, KeyEventKind::Press));
}

#[test]
fn a_cyrillic_uchar_with_ctrl_is_the_latin_letter_on_any_lookup() {
    let mut tap = ConsoleTap::new();
    assert_eq!(tap.classify_with(rec(true, 0x43, 'с' as u16, LCTRL), |_| None), key('c', KeyModifiers::CONTROL, KeyEventKind::Press));
}

#[test]
fn shift_xor_caps_lock_gives_the_upper_case() {
    let mut tap = ConsoleTap::new();
    let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL | SHIFT), russian), key('V', cs, KeyEventKind::Press));
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL | CAPS), russian), key('V', KeyModifiers::CONTROL, KeyEventKind::Press));
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL | SHIFT | CAPS), russian), key('v', cs, KeyEventKind::Press));
}

#[test]
fn a_latin_layout_stays_crossterms() {
    let mut tap = ConsoleTap::new();
    for r in [rec(true, 0x56, 0x16, LCTRL), rec(true, 0x49, 0x09, LCTRL), rec(true, 0x42, 0x02, LCTRL), rec(true, 0x56, b'v' as u16, LCTRL)] {
        assert_eq!(tap.classify_with(r, latin), Verdict::NotOurs, "{r:?}");
    }
}

#[test]
fn not_ours_without_ctrl_with_alt_or_on_other_keys() {
    let mut tap = ConsoleTap::new();
    // Typed Cyrillic letter, no Ctrl.
    assert_eq!(tap.classify_with(rec(true, 0x56, 'м' as u16, 0), russian), Verdict::NotOurs);
    // Ctrl+Alt chords and AltGr keep their own rules.
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL | LALT), russian), Verdict::NotOurs);
    assert_eq!(tap.classify_with(rec(true, 0x56, 'м' as u16, LCTRL | RALT), russian), Verdict::NotOurs);
    // A digit or punctuation key is not a letter key.
    assert_eq!(tap.classify_with(rec(true, 0x32, 0x00, LCTRL), russian), Verdict::NotOurs);
    assert_eq!(tap.classify_with(rec(true, 0xDB, 0x1B, LCTRL), |_| Some('х')), Verdict::NotOurs);
}

#[test]
fn a_pending_high_surrogate_is_dropped_by_the_ctrl_key() {
    let mut tap = ConsoleTap::new();
    assert_eq!(tap.classify_with(rec(true, 0, 0xD83D, 0), russian), Verdict::Consumed(None));
    assert_eq!(tap.classify_with(rec(true, 0x56, 0x16, LCTRL), russian), key('v', KeyModifiers::CONTROL, KeyEventKind::Press));
    // The orphan low half no longer pairs with the dropped high half.
    assert_eq!(tap.classify_with(rec(true, 0, 0xDE0A, 0), russian), Verdict::Consumed(None));
}
