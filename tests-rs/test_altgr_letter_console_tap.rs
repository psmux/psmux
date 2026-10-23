// An ASCII lowercase letter a layout produces with AltGr.
//
// Windows Terminal pastes by writing the clipboard into the client's console
// input, and the console turns every character into the key that types it on
// the active layout (VkKeyScanW). On Turkish Q that key for `i` is AltGr+I,
// not the physical `i` key (VK_OEM_7): measured on Windows 11 26200, the
// records for a pasted `i` are
//
//   DOWN vk=0x12 scan=0x38 uChar=0x0000 ctrl=0x0109   (right Alt, AltGr)
//   DOWN vk=0x49 scan=0x17 uChar=0x0069 ctrl=0x0009   (LEFT_CTRL | RIGHT_ALT)
//   up   vk=0x49 scan=0x17 uChar=0x0069 ctrl=0x0009
//   up   vk=0x12 scan=0x38 uChar=0x0000 ctrl=0x0100
//
// crossterm reports that as Char('i') with CONTROL | ALT, and every AltGr rule
// in the client treats Ctrl+Alt on a lowercase ASCII letter as a real chord:
// the pending paste was flushed as typing and the pane got `send-key C-M-i`,
// a Tab. The tap's policy: a key record with the AltGr signature (LEFT_CTRL
// and RIGHT_ALT held, LEFT_ALT not held) and a lowercase ASCII letter in uChar
// is text and goes out as that letter. Shift and the lock bits do not enter the
// decision. Any other Ctrl+Alt shape (left Ctrl + left Alt, both Alts, right
// Ctrl + right Alt) stays crossterm's, whatever its uChar. What was measured:
// the Turkish Q paste records above (ctrl=0x0009), and physical Ctrl+Left
// Alt+I and +A, which carry LEFT_CTRL | LEFT_ALT with Caps Lock and Num Lock
// on (ctrl=0x00AA) and uChar 0 (psmux's injected C-M-i record carries 0x09).
// These tests cover `ConsoleTap::classify` only, not crossterm, key bindings
// or delivery to a pane.

use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

fn keyed(down_: bool, vk: u16, u: u16, ctrl_state: u32) -> Option<KeyRec> {
    Some(KeyRec { key_down: down_, vk, u_char: u, ctrl_state })
}
fn ch(c: char) -> Event {
    Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() })
}

fn run(tap: &mut ConsoleTap, recs: &[Option<KeyRec>]) -> (Vec<Event>, Vec<Option<KeyRec>>) {
    let mut out = Vec::new();
    let mut left = Vec::new();
    for r in recs {
        match tap.classify(*r) {
            Verdict::NotOurs => left.push(*r),
            Verdict::Consumed(Some(Event::Key(k))) if k.kind == KeyEventKind::Release => {}
            Verdict::Consumed(Some(e)) => out.push(e),
            Verdict::Consumed(None) => {}
        }
    }
    (out, left)
}

// One ordered stream: what the tap turned into text and what it left to
// crossterm, in the order the records arrived.
#[derive(Debug, PartialEq)]
enum Step {
    Text(Event),
    Crossterm(Option<KeyRec>),
}

fn trace(tap: &mut ConsoleTap, recs: &[Option<KeyRec>]) -> Vec<Step> {
    let mut out = Vec::new();
    for r in recs {
        match tap.classify(*r) {
            Verdict::NotOurs => out.push(Step::Crossterm(*r)),
            Verdict::Consumed(Some(Event::Key(k))) if k.kind == KeyEventKind::Release => {}
            Verdict::Consumed(Some(e)) => out.push(Step::Text(e)),
            Verdict::Consumed(None) => {}
        }
    }
    out
}

const ALTGR_DOWN: Option<KeyRec> = Some(KeyRec { key_down: true, vk: 0x12, u_char: 0, ctrl_state: 0x0109 });
const ALTGR_UP: Option<KeyRec> = Some(KeyRec { key_down: false, vk: 0x12, u_char: 0, ctrl_state: 0x0100 });

#[test]
fn the_measured_turkish_q_paste_of_i_is_the_letter_i() {
    let i_down = keyed(true, 0x49, 0x0069, 0x0009);
    let i_up = keyed(false, 0x49, 0x0069, 0x0009);
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[ALTGR_DOWN, i_down, i_up, ALTGR_UP]);
    assert_eq!(ev, vec![ch('i')], "the letter, with no modifiers");
    // The AltGr key itself stays crossterm's, which reports no event for it.
    assert_eq!(left, vec![ALTGR_DOWN, ALTGR_UP]);
}

#[test]
fn the_letter_between_ordinary_keys_keeps_their_order() {
    // `bir`: b and r are crossterm's, the i between them is the tap's. One
    // stream, so the position of the text among the records is asserted.
    let b = keyed(true, 0x42, 'b' as u16, 0);
    let b_up = keyed(false, 0x42, 'b' as u16, 0);
    let r = keyed(true, 0x52, 'r' as u16, 0);
    let mut tap = ConsoleTap::new();
    let steps = trace(&mut tap, &[b, b_up, ALTGR_DOWN, keyed(true, 0x49, 0x69, 0x0009), keyed(false, 0x49, 0x69, 0x0009), ALTGR_UP, r]);
    assert_eq!(steps, vec![
        Step::Crossterm(b),
        Step::Crossterm(b_up),
        Step::Crossterm(ALTGR_DOWN),
        Step::Text(ch('i')),
        Step::Crossterm(ALTGR_UP),
        Step::Crossterm(r),
    ]);
}

#[test]
fn turkish_capital_dotted_i_is_left_as_it_is_today() {
    // `İ` arrives with Shift on top of AltGr (ctrl=0x0019). It is not an ASCII
    // lowercase letter, so the client's AltGr rule already reads it as text.
    let k = keyed(true, 0x49, 0x0130, 0x0019);
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[k]);
    assert!(ev.is_empty());
    assert_eq!(left, vec![k]);
    assert!(crate::client::is_altgr_char('\u{130}', KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT));
}

#[test]
fn altgr_at_sign_is_left_as_it_is_today() {
    let k = keyed(true, 0x51, 0x0040, 0x0009);
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[k]);
    assert!(ev.is_empty());
    assert_eq!(left, vec![k]);
    assert!(crate::client::is_altgr_char('@', KeyModifiers::CONTROL | KeyModifiers::ALT));
}

#[test]
fn ctrl_alt_records_without_a_lowercase_letter_are_not_taken_by_the_tap() {
    // Measured on US: Ctrl+Alt+I has uChar 0; the record psmux injects for
    // C-M-i carries 0x09 (a control character). The tap leaves these to
    // crossterm, as before. Only the tap boundary is tested here, not what
    // crossterm or the client then make of the record.
    let mut tap = ConsoleTap::new();
    for k in [keyed(true, 0x49, 0x0009, 0x000A), keyed(true, 0x49, 0x0000, 0x000A), keyed(true, 0x49, 0x0009, 0x0009)] {
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "{:?} must not become text", k);
        assert_eq!(left, vec![k]);
    }
    assert!(!crate::client::is_altgr_char('i', KeyModifiers::CONTROL | KeyModifiers::ALT));
}

#[test]
fn ctrl_alone_or_alt_alone_is_not_altgr() {
    // Alt+i (uChar 'i', LEFT_ALT) is M-i and Ctrl+I is C-i: one modifier is
    // never AltGr.
    let mut tap = ConsoleTap::new();
    for k in [keyed(true, 0x49, 0x0069, 0x0002), keyed(true, 0x49, 0x0009, 0x0008), keyed(true, 0x49, 0x0069, 0x0008)] {
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "{:?}", k);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn left_ctrl_left_alt_with_a_letter_stays_a_chord() {
    // Left Ctrl + left Alt is how a chord is typed. Even with a letter in
    // uChar it is not the AltGr signature, so the tap leaves it to crossterm
    // (Char('i') with CONTROL | ALT, which the client sends as C-M-i). Also the
    // measured physical shape with Caps Lock and Num Lock on (0x00AA).
    let mut tap = ConsoleTap::new();
    for k in [
        keyed(true, 0x49, 0x0069, 0x000A),
        keyed(true, 0x49, 0x0069, 0x00AA),
        keyed(true, 0x49, 0x0000, 0x00AA),
        keyed(false, 0x49, 0x0069, 0x000A),
    ] {
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "{:?} must stay a chord", k);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn both_alts_held_is_a_chord() {
    // RIGHT_ALT is there, but so is LEFT_ALT: not the AltGr signature.
    let mut tap = ConsoleTap::new();
    for k in [keyed(true, 0x49, 0x0069, 0x000B), keyed(true, 0x49, 0x0069, 0x000F)] {
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "{:?}", k);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn unmeasured_ctrl_alt_shapes_stay_crossterms() {
    // Right Ctrl + right Alt, and right Alt with no Ctrl at all, were never
    // measured carrying AltGr text, so they are not taken. (LEFT_CTRL is what
    // the signature needs; a RIGHT_CTRL held on top of it does not matter.)
    let mut tap = ConsoleTap::new();
    for k in [keyed(true, 0x49, 0x0069, 0x0005), keyed(true, 0x49, 0x0069, 0x0001)] {
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "{:?}", k);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn both_ctrls_with_right_alt_is_the_signature() {
    // 0x000D = LEFT_CTRL | RIGHT_CTRL | RIGHT_ALT: LEFT_CTRL and RIGHT_ALT are
    // held and LEFT_ALT is not, so it is the signature; RIGHT_CTRL on top of it
    // does not matter.
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[keyed(true, 0x49, 0x0069, 0x000D), keyed(false, 0x49, 0x0069, 0x000D)]);
    assert_eq!(ev, vec![ch('i')]);
    assert!(left.is_empty(), "{:?}", left);
}

#[test]
fn the_letter_range_is_a_to_z_and_nothing_next_to_it() {
    // a and z are the ends of the range and are taken.
    for (vk, c) in [(0x41u16, 'a'), (0x5A, 'z')] {
        let mut tap = ConsoleTap::new();
        let (ev, left) = run(&mut tap, &[keyed(true, vk, c as u16, 0x0009)]);
        assert_eq!(ev, vec![ch(c)], "{}", c);
        assert!(left.is_empty());
    }
    // The ASCII characters on either side of the range, '`' (0x60) and
    // '{' (0x7B), keep their previous handling with the same signature.
    for (vk, u) in [(0xC0u16, 0x0060u16), (0xDB, 0x007B)] {
        let mut tap = ConsoleTap::new();
        let k = keyed(true, vk, u, 0x0009);
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "U+{:04X} must not be taken", u);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn shift_and_lock_bits_do_not_change_the_decision() {
    // AltGr signature plus Shift (0x10), Num Lock (0x20), Scroll Lock (0x40),
    // Caps Lock (0x80), alone and together: still the letter, no modifiers.
    for extra in [0x0010u32, 0x0020, 0x0040, 0x0080, 0x00F0] {
        let mut tap = ConsoleTap::new();
        let (ev, left) = run(&mut tap, &[keyed(true, 0x49, 0x0069, 0x0009 | extra), keyed(false, 0x49, 0x0069, 0x0009 | extra)]);
        assert_eq!(ev, vec![ch('i')], "ctrl=0x{:04X}", 0x0009 | extra);
        assert!(left.is_empty(), "ctrl=0x{:04X}: {:?}", 0x0009 | extra, left);
    }
    // And the same bits on a chord shape leave it a chord.
    for extra in [0x0010u32, 0x0080, 0x00F0] {
        let mut tap = ConsoleTap::new();
        let k = keyed(true, 0x49, 0x0069, 0x000A | extra);
        let (ev, left) = run(&mut tap, &[k]);
        assert!(ev.is_empty(), "ctrl=0x{:04X}", 0x000A | extra);
        assert_eq!(left, vec![k]);
    }
}

#[test]
fn a_pending_surrogate_half_is_broken_by_the_letter() {
    // Same as any other character key between two halves.
    let mut tap = ConsoleTap::new();
    let hi = keyed(true, 0, 0xD83D, 0);
    let lo = keyed(true, 0, 0xDE0A, 0);
    let (ev, _) = run(&mut tap, &[hi, keyed(true, 0x49, 0x69, 0x0009), lo]);
    assert_eq!(ev, vec![ch('i')]);
}

#[test]
fn the_key_up_goes_on_as_the_plain_letters_release() {
    // The paste head rule reads a host written pair by its release, so the
    // AltGr letter's key up is handed on as a release of the plain letter.
    let mut tap = ConsoleTap::new();
    let up = tap.classify(keyed(false, 0x49, 0x0069, 0x0009));
    match up {
        Verdict::Consumed(Some(Event::Key(k))) => {
            assert_eq!(k.code, KeyCode::Char('i'));
            assert_eq!(k.modifiers, KeyModifiers::empty());
            assert_eq!(k.kind, KeyEventKind::Release);
        }
        _ => panic!("the key up must be a release of the plain letter"),
    }
}