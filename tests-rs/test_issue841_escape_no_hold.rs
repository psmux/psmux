//! Issue #841 follow up: a typed Escape is not held.
//!
//! Every bare Escape used to wait out a fixed 50 ms window in `EscCoalesce`
//! (added for #611) before it reached the pane, on every route, whatever
//! `escape-time` said.  Measured with a real pseudoconsole in front of an
//! attached client and a pane child stamping each byte on the same clock:
//! letters, Alt+x and Up arrived in 1 to 2 ms, Escape in 50.5 to 50.8 ms.
//!
//! What the console hands over tells the two kinds of Escape apart:
//!
//! ```text
//!   win32-input-mode key press (Windows Terminal), classic console window:
//!     DOWN vk=0x1B scan=0x01          ... UP arrives when the key is let go
//!   a VT byte 1b the console parsed (a host writing 1b 0d, #611):
//!     DOWN vk=0x1B scan=0x01, UP vk=0x1B scan=0x01   in the same read
//! ```
//!
//! So a DOWN with nothing queued behind it is a key press, which nothing can
//! be merged into: it goes out at once.  Anything already queued keeps the old
//! path, and the hold that remains follows `escape-time` (capped at 50 ms),
//! the way tmux's `tty_keys_next` waits `escape-time` only for a partial key.

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::{escape_wait_for, EscCoalesce, ESC_COALESCE_MS};

fn press(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent { code, modifiers: mods, kind: KeyEventKind::Press, state: KeyEventState::empty() })
}

fn release(code: KeyCode) -> Event {
    Event::Key(KeyEvent { code, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Release, state: KeyEventState::empty() })
}

fn key_of(ev: &Event) -> KeyEvent {
    match ev {
        Event::Key(k) => *k,
        other => panic!("expected a key event, got {other:?}"),
    }
}

/// A pull that hands out `events` in order, then reports nothing queued.
fn queued(events: Vec<Event>) -> impl FnMut() -> Option<Event> {
    let mut it = events.into_iter();
    move || it.next()
}

#[test]
fn a_key_press_with_nothing_queued_goes_straight_through() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let out = c.feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, || None);
    let k = key_of(&out.expect("a typed Escape must not be held"));
    assert_eq!((k.code, k.modifiers, k.kind), (KeyCode::Esc, KeyModifiers::NONE, KeyEventKind::Press));
    assert!(c.deadline_ms(t0).is_none(), "nothing may be left waiting");
    assert!(c.pop().is_none());
}

#[test]
fn its_release_arriving_later_changes_nothing() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, || None).is_some());
    let up = c.feed(release(KeyCode::Esc), t0 + Duration::from_millis(90)).unwrap();
    assert_eq!(key_of(&up).kind, KeyEventKind::Release);
    assert!(c.deadline_ms(t0).is_none());
}

#[test]
fn a_held_key_repeating_is_never_held_either() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let mut rep = press(KeyCode::Esc, KeyModifiers::NONE);
    if let Event::Key(k) = &mut rep { k.kind = KeyEventKind::Repeat; }
    assert!(c.feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, || None).is_some());
    assert!(c.feed_with(rep, t0 + Duration::from_millis(30), || None).is_some());
}

#[test]
fn a_parsed_byte_pair_is_still_held_for_its_window() {
    // DOWN and UP in one read: the console made this Escape from a byte, so
    // a CR may still be on its way (#611).
    let mut c = EscCoalesce::with_window(true, Duration::from_millis(10));
    let t0 = Instant::now();
    assert!(c
        .feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, queued(vec![release(KeyCode::Esc)]))
        .is_none());
    assert_eq!(c.deadline_ms(t0), Some(10));
    assert!(c.expire(t0 + Duration::from_millis(9)).is_none());
    let out = c.expire(t0 + Duration::from_millis(10)).expect("released when the window is up");
    assert_eq!(key_of(&out).code, KeyCode::Esc);
}

#[test]
fn esc_cr_written_in_one_call_merges_without_waiting() {
    // The #611 measurement: ESC+CR in ONE WriteConsoleInput.  The Enter is
    // already queued, so the merge needs no timer at all.
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let mut pull = queued(vec![release(KeyCode::Esc), press(KeyCode::Enter, KeyModifiers::NONE)]);
    assert!(c.feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, &mut pull).is_none());
    // The client's read loop hands it the queued Enter next.
    let next = pull().unwrap();
    let k = key_of(&c.feed_with(next, t0, || None).unwrap());
    assert_eq!((k.code, k.modifiers), (KeyCode::Enter, KeyModifiers::ALT));
    assert!(c.deadline_ms(t0).is_none());
}

#[test]
fn an_escape_with_its_cr_right_behind_it_merges_at_once() {
    // A pair whose records arrive DOWN, DOWN with no release in between.
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let out = c
        .feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, queued(vec![press(KeyCode::Enter, KeyModifiers::NONE)]))
        .unwrap();
    let k = key_of(&out);
    assert_eq!((k.code, k.modifiers), (KeyCode::Enter, KeyModifiers::ALT));
}

#[test]
fn an_escape_with_a_letter_queued_behind_comes_out_first_and_in_order() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let out = c
        .feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, queued(vec![press(KeyCode::Char('j'), KeyModifiers::NONE)]))
        .unwrap();
    assert_eq!(key_of(&out).code, KeyCode::Esc);
    assert_eq!(key_of(&c.pop().unwrap()).code, KeyCode::Char('j'));
    assert!(c.deadline_ms(t0).is_none());
}

#[test]
fn an_unparsed_csi_u_behind_a_surviving_escape_is_still_one_key() {
    // conhost does not know CSI u and flushes `ESC [ 1 3 ; 2 u` as one record
    // per byte, all in one read.  The Escape must go into the sequence, not
    // out on its own.
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let mut pull = queued(
        "[13;2u".chars().map(|ch| press(KeyCode::Char(ch), KeyModifiers::NONE)).collect(),
    );
    let out = c.feed_with(press(KeyCode::Esc, KeyModifiers::NONE), t0, &mut pull).unwrap();
    let k = key_of(&out);
    assert_eq!((k.code, k.modifiers), (KeyCode::Enter, KeyModifiers::SHIFT));
    assert!(c.pop().is_none());
    assert!(c.deadline_ms(t0).is_none());
}

#[test]
fn the_hold_follows_escape_time_like_tmux() {
    // tmux: `delay = escape-time; if (delay == 0) delay = 1` (tty-keys.c).
    assert_eq!(escape_wait_for(0), 1);
    assert_eq!(escape_wait_for(10), 10);
    assert_eq!(escape_wait_for(25), 25);
    // A large escape-time (tmux 3.4 defaulted to 500) is never held that long.
    assert_eq!(escape_wait_for(500), ESC_COALESCE_MS);
    assert_eq!(escape_wait_for(u64::MAX), ESC_COALESCE_MS);
}
