// Issue #766 (found while reproducing it): an em dash written into the
// client's pseudoconsole by node-pty never reached the pane. The console hands
// a character no key on the layout produces over as an Alt code, and these are
// the records it produced for `a—b` (raw ReadConsoleInputW dump, build 26200,
// identical with and without ENABLE_VIRTUAL_TERMINAL_INPUT apart from the key
// down records of plain characters).
use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

fn rec(key_down: bool, vk: u16, u_char: u16, ctrl_state: u32) -> Option<KeyRec> {
    Some(KeyRec { key_down, vk, u_char, ctrl_state })
}

fn press_char(c: char) -> Event {
    Event::Key(KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    })
}

#[test]
fn the_alt_up_that_carries_an_em_dash_becomes_its_key_press() {
    let mut tap = ConsoleTap::new();
    let records = [
        rec(true, 0x12, 0, 0x0002),       // Alt down
        rec(true, 0x64, 0, 0x0002),       // numpad 4 down
        rec(false, 0x64, 0, 0x0002),      // numpad 4 up
        rec(true, 0x65, 0, 0x0002),       // numpad 5 down
        rec(false, 0x65, 0, 0x0002),      // numpad 5 up
        rec(false, 0x12, 0x2014, 0x0000), // Alt up carrying U+2014
    ];
    let mut produced = Vec::new();
    for r in records {
        if let Verdict::Consumed(Some(e)) = tap.classify(r) {
            produced.push(e);
        }
    }
    assert_eq!(produced, vec![press_char('\u{2014}')]);
}

#[test]
fn ordinary_key_up_records_are_still_left_alone() {
    let mut tap = ConsoleTap::new();
    // A plain `b` key up and a bare Alt up (no character) are crossterm's.
    assert_eq!(tap.classify(rec(false, 0x42, 0x62, 0)), Verdict::NotOurs);
    assert_eq!(tap.classify(rec(false, 0x12, 0, 0)), Verdict::NotOurs);
}

#[test]
fn the_vt_route_keeps_exactly_the_alt_code_release() {
    assert!(alt_code_release_char(false, 0x12, 0x2014));
    assert!(alt_code_release_char(false, 0x12, 0x00E9));
    // Key down records, other keys' releases, a bare Alt up, control
    // characters and surrogate halves are not this record.
    assert!(!alt_code_release_char(true, 0x12, 0x2014));
    assert!(!alt_code_release_char(false, 0x42, 0x62));
    assert!(!alt_code_release_char(false, 0x12, 0));
    assert!(!alt_code_release_char(false, 0x12, 0x0D));
    assert!(!alt_code_release_char(false, 0x12, 0xD83D));
}
