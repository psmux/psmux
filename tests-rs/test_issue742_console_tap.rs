// Issue #742: the console input route dropped every non BMP character, and
// every control character delivered with vk = 0.
//
// These run the tap's decision over the exact record sequences the console
// produces, the way `take_head` feeds it one record at a time from the head of
// the buffer. A record the tap returns NotOurs for is the one crossterm reads.

use super::*;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

fn down(u: u16) -> Option<KeyRec> {
    Some(KeyRec { key_down: true, vk: 0, u_char: u, ctrl_state: 0 })
}
fn up(u: u16) -> Option<KeyRec> {
    Some(KeyRec { key_down: false, vk: 0, u_char: u, ctrl_state: 0 })
}
fn keyed(down_: bool, vk: u16, u: u16, ctrl_state: u32) -> Option<KeyRec> {
    Some(KeyRec { key_down: down_, vk, u_char: u, ctrl_state })
}
fn ch(c: char) -> Event {
    Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() })
}
fn ctrl(c: char) -> Event {
    Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::CONTROL, kind: KeyEventKind::Press, state: KeyEventState::empty() })
}

/// Feed records through one tap. Returns the events the tap produced and the
/// records it left for crossterm, in order.
fn run(tap: &mut ConsoleTap, recs: &[Option<KeyRec>]) -> (Vec<Event>, Vec<Option<KeyRec>>) {
    let mut out = Vec::new();
    let mut left = Vec::new();
    for r in recs {
        match tap.classify(*r) {
            Verdict::NotOurs => left.push(*r),
            Verdict::Consumed(Some(e)) => out.push(e),
            Verdict::Consumed(None) => {}
        }
    }
    (out, left)
}

#[test]
fn the_measured_four_records_give_one_emoji() {
    // U+1F60A exactly as the console hands it over for a paste, an IME commit
    // or the emoji panel: one down and one up per code unit, vk = 0.
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[down(0xD83D), up(0xD83D), down(0xDE0A), up(0xDE0A)]);
    assert_eq!(ev, vec![ch('\u{1F60A}')]);
    assert!(left.is_empty(), "no surrogate half may reach crossterm: {:?}", left);
}

#[test]
fn jis2004_kanji_between_ascii() {
    // a, U+20B9F, b: the ASCII halves stay crossterm's, the kanji is decoded.
    let a = keyed(true, 0x41, 'a' as u16, 0);
    let a_up = keyed(false, 0x41, 'a' as u16, 0);
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[a, a_up, down(0xD842), up(0xD842), down(0xDF9F), up(0xDF9F), a]);
    assert_eq!(ev, vec![ch('\u{20B9F}')]);
    assert_eq!(left, vec![a, a_up, a]);
}

#[test]
fn two_emoji_back_to_back() {
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[
        down(0xD83D), up(0xD83D), down(0xDE0A), up(0xDE0A),
        down(0xD83D), up(0xD83D), down(0xDE00), up(0xDE00),
    ]);
    assert_eq!(ev, vec![ch('\u{1F60A}'), ch('\u{1F600}')]);
}

#[test]
fn a_pair_split_across_two_reads_still_decodes() {
    // The state lives in the tap, not in one batch: the high half in one
    // ReadConsoleInputW, the low half in the next.
    let mut tap = ConsoleTap::new();
    let (first, _) = run(&mut tap, &[down(0xD83D), up(0xD83D)]);
    assert!(first.is_empty());
    let (second, _) = run(&mut tap, &[down(0xDE0A), up(0xDE0A)]);
    assert_eq!(second, vec![ch('\u{1F60A}')]);
}

#[test]
fn a_lone_high_surrogate_emits_nothing_and_does_not_poison_the_next_pair() {
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[down(0xD83D), up(0xD83D)]);
    assert!(ev.is_empty());
    // A character key in between breaks the dangling half.
    let x = keyed(true, 0x58, 'x' as u16, 0);
    let (ev, left) = run(&mut tap, &[x, down(0xDE0A), up(0xDE0A)]);
    assert!(ev.is_empty(), "an orphan low after a broken pair must not decode: {:?}", ev);
    assert_eq!(left, vec![x]);
    // And a fresh pair after that decodes normally.
    let (ev, _) = run(&mut tap, &[down(0xD83D), down(0xDE0A)]);
    assert_eq!(ev, vec![ch('\u{1F60A}')]);
}

#[test]
fn a_second_high_replaces_the_first() {
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[down(0xD83D), down(0xD842), down(0xDF9F)]);
    assert_eq!(ev, vec![ch('\u{20B9F}')]);
}

#[test]
fn a_lone_low_surrogate_emits_nothing() {
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[down(0xDE0A), up(0xDE0A)]);
    assert!(ev.is_empty());
    assert!(left.is_empty(), "the orphan is consumed, never handed to crossterm");
}

#[test]
fn a_key_up_only_sequence_emits_nothing() {
    // Only the releases: nothing was pressed, so nothing is typed, and the
    // releases must not be paired with each other the way crossterm pairs them.
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[up(0xD83D), up(0xDE0A)]);
    assert!(ev.is_empty());
    assert!(left.is_empty());
    // They left no state behind.
    let (ev, _) = run(&mut tap, &[down(0xDE0A)]);
    assert!(ev.is_empty());
}

#[test]
fn crossterms_pairing_of_the_same_records_is_what_lost_them() {
    // Model of crossterm 0.29 handle_surrogate: pair consecutive surrogate
    // values regardless of bKeyDown. Over the measured four records it decodes
    // nothing, which is the bug; the tap over the same records decodes one.
    let units = [0xD83Du16, 0xD83D, 0xDE0A, 0xDE0A];
    let mut buf: Option<u16> = None;
    let mut crossterm_out = String::new();
    for u in units {
        match buf.take() {
            Some(b) => {
                if let Some(Ok(c)) = std::char::decode_utf16([b, u]).next() { crossterm_out.push(c); }
            }
            None => buf = Some(u),
        }
    }
    assert_eq!(crossterm_out, "");
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[down(0xD83D), up(0xD83D), down(0xDE0A), up(0xDE0A)]);
    assert_eq!(ev.len(), 1);
}

#[test]
fn modifiers_on_the_low_half_carry_like_crossterms_char_path() {
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[keyed(true, 0, 0xD83D, 0x0010), keyed(true, 0, 0xDE0A, 0x0010)]);
    assert_eq!(ev, vec![Event::Key(KeyEvent {
        code: KeyCode::Char('\u{1F60A}'),
        modifiers: KeyModifiers::SHIFT,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    })]);
}

#[test]
fn an_alt_code_release_still_counts() {
    // crossterm's one exception: an Alt key UP carrying a uChar is an Alt code.
    let mut tap = ConsoleTap::new();
    let (ev, _) = run(&mut tap, &[keyed(false, 0x12, 0xD83D, 0), keyed(false, 0x12, 0xDE0A, 0)]);
    assert_eq!(ev, vec![ch('\u{1F60A}')]);
}

#[test]
fn non_key_records_are_crossterms_and_keep_a_pending_half() {
    // A mouse or resize record between the halves (None here) goes to
    // crossterm and does not break the pair.
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[down(0xD83D), None, down(0xDE0A)]);
    assert_eq!(ev, vec![ch('\u{1F60A}')]);
    assert_eq!(left, vec![None]);
}

#[test]
fn ordinary_keys_are_never_taken() {
    let mut tap = ConsoleTap::new();
    let recs = [
        keyed(true, 0x42, 0x02, 0x0008),  // a real Ctrl+B: vk set, crossterm's
        keyed(true, 0x0D, 0x0D, 0),       // Enter
        keyed(true, 0x1B, 0x1B, 0),       // Escape key
        keyed(true, 0x10, 0, 0x0010),     // Shift
        keyed(true, 0x31, 0, 0x0008),     // Ctrl+1 (no uChar, the #623 Far key)
        keyed(true, 0x00, 0x5F45, 0),     // a BMP kanji with vk = 0
        keyed(false, 0x00, 0x5F45, 0),
        keyed(true, 0x00, 0x00, 0),       // vk = 0 and no character
    ];
    let (ev, left) = run(&mut tap, &recs);
    assert!(ev.is_empty());
    assert_eq!(left, recs.to_vec());
}

#[test]
fn vk0_control_characters_become_the_keys_the_vt_route_reports() {
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[down(0x02), up(0x02)]);
    assert_eq!(ev, vec![ctrl('b')]);
    assert!(left.is_empty());

    let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() });
    let tab = Event::Key(KeyEvent { code: KeyCode::Tab, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() });
    let (ev, _) = run(&mut tap, &[down(0x0D), down(0x09), down(0x0A), down(0x08), down(0x00), down(0x1C), down(0x1F)]);
    // vk = 0 with no character is not ours, so 0x00 is left alone.
    assert_eq!(ev, vec![enter, tab, ctrl('j'), ctrl('h'), ctrl('\\'), ctrl('_')]);
}

// Issue #860: crossterm drops a vk = 0 ESC, which is exactly what a Windows
// Terminal `sendInput` "\u001b" (Ctrl+[ remapped to Escape) delivers. The tap
// now turns it into the Escape key and swallows its key up.
#[test]
fn vk0_escape_is_the_escape_key() {
    let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() });
    let mut tap = ConsoleTap::new();
    let (ev, left) = run(&mut tap, &[down(0x1B), up(0x1B)]);
    assert_eq!(ev, vec![esc]);
    assert!(left.is_empty(), "nothing of the ESC may reach crossterm: {:?}", left);
}

// The sendInput shape of an extended key: ESC is the tap's, every printable
// byte behind it stays crossterm's, so EscCoalesce sees Esc then `[` and
// recover_csi_u still gets the whole `[32;5u`.
#[test]
fn vk0_escape_leaves_the_csi_u_body_to_crossterm() {
    let mut tap = ConsoleTap::new();
    let mut recs = vec![down(0x1B), up(0x1B)];
    for c in "[32;5u".chars() {
        recs.push(down(c as u16));
        recs.push(up(c as u16));
    }
    let (ev, left) = run(&mut tap, &recs);
    assert_eq!(ev.len(), 1);
    assert_eq!(left.len(), 12, "the six body characters, down and up: {:?}", left);
}

// A real Escape key (vk = VK_ESCAPE) is still crossterm's.
#[test]
fn keyed_escape_is_still_crossterms() {
    let mut tap = ConsoleTap::new();
    let k = keyed(true, 0x1B, 0x1B, 0);
    let (ev, left) = run(&mut tap, &[k]);
    assert!(ev.is_empty());
    assert_eq!(left, vec![k]);
}
