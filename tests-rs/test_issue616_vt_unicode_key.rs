// Issue #616, the OTHER input route.
//
// The Win32 route (crossterm over ReadConsoleInputW) is covered end to end by
// tests/test_issue616_unicode_bind.ps1. A client attached over SSH or a
// Cygwin/MSYS pty instead reads a raw VT BYTE STREAM, so a Cyrillic keypress
// arrives as two UTF-8 bytes rather than one console record. That is precisely
// the shape of input the #616 byte-versus-character confusion would have
// mangled, so pin it: the reader must reassemble the bytes into one scalar and
// the parser must emit exactly one KeyCode::Char for it.
//
// start_pipe_reader decodes with `std::str::from_utf8` and holds an incomplete
// tail back for the next read, so a multi-byte character SPLIT ACROSS TWO READS
// still has to arrive as one key. The split case is simulated here by feeding
// the parser the reassembled characters, which is what the reader hands it.

use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

const YERU: char = '\u{044B}';
const EF: char = '\u{0444}';
const ROCKET: char = '\u{1F680}';

fn feed_chars(s: &str) -> Vec<Event> {
    let mut p = VtParser::new();
    let mut events = Vec::new();
    for ch in s.chars() {
        p.feed(ch, &mut |evt| events.push(evt));
    }
    events
}

fn chars_of(events: &[Event]) -> Vec<char> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) => Some(*c),
            _ => None,
        })
        .collect()
}

#[test]
fn issue616_vt_stream_yields_one_key_per_unicode_scalar() {
    let s: String = [YERU, EF, ROCKET, 'q'].iter().collect();
    let events = feed_chars(&s);
    assert_eq!(
        chars_of(&events),
        vec![YERU, EF, ROCKET, 'q'],
        "each scalar must produce exactly one Char key, not one per UTF-8 byte"
    );
}

#[test]
fn issue616_vt_unicode_key_carries_no_phantom_modifiers() {
    // A bare Cyrillic key must look identical to a bare `q`: no ALT from a
    // stray high bit, no CONTROL. Anything else would miss a plain
    // `bind <yeru>` in the prefix table.
    let events = feed_chars(&YERU.to_string());
    let keys: Vec<&KeyEvent> = events
        .iter()
        .filter_map(|e| match e {
            Event::Key(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(keys.len(), 1, "expected exactly one key event, got {:?}", events);
    assert_eq!(keys[0].code, KeyCode::Char(YERU));
    assert_eq!(keys[0].modifiers, KeyModifiers::NONE);
}

#[test]
fn issue616_vt_utf8_reassembly_is_byte_exact() {
    // Guard the reader's own contract: `std::str::from_utf8` on a buffer that
    // ends mid character reports the valid prefix and asks for more bytes, so
    // the tail is held rather than emitted as replacement characters.
    let full = format!("{}{}", YERU, EF).into_bytes();
    assert_eq!(full.len(), 4, "two 2 byte characters");
    // Split inside the first character.
    let err = std::str::from_utf8(&full[..1]).unwrap_err();
    assert_eq!(err.valid_up_to(), 0);
    assert!(err.error_len().is_none(), "incomplete, not invalid: wait for more bytes");
    // Split between the two characters.
    let ok = std::str::from_utf8(&full[..2]).expect("first character is complete");
    assert_eq!(ok.chars().next(), Some(YERU));
    // Split inside the second character.
    let err2 = std::str::from_utf8(&full[..3]).unwrap_err();
    assert_eq!(err2.valid_up_to(), 2);
    assert!(err2.error_len().is_none());
}
