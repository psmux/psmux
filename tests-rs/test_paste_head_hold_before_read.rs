// The paste head hold is measured before the clipboard is read.
//
// Full sweep 2026-11-17_18-39-01: test_altgr_letter_paste got 'bi' in front of
// ESC[200~ and test_issue684_paste_head made 40 zero-latency flushes during a
// dripped paste. Since clipboard.rs opens the clipboard with a window, the
// first clipboard read on the client's input thread creates that window and
// takes a few milliseconds. The held time of the lone first character was
// taken after that read, came out above PASTE_HEAD_PREFIX_HOLD (3 ms), and
// the head of the paste was released as typing before the rest of it could
// be read.

use super::*;

/// A read slower than the whole hold does not count against it.
#[test]
fn a_slow_read_does_not_eat_the_hold() {
    let start = Some(Instant::now());
    let (held_for, head) = held_for_then_read(start, || {
        std::thread::sleep(Duration::from_millis(15));
        Some("bir sonraki")
    });
    assert!(held_for < PASTE_HEAD_PREFIX_HOLD, "held_for {held_for:?} includes the read");
    assert_eq!(head, Some("bir sonraki"));
}

/// The first character of a paste whose clipboard read was slow is still
/// held as the head of a paste.
#[test]
fn the_first_character_is_held_after_a_slow_first_read() {
    let start = Some(Instant::now());
    let (held_for, clip_head) = held_for_then_read(start, || {
        std::thread::sleep(Duration::from_millis(15));
        Some("bir sonraki Eski")
    });
    let evidence = PasteHeadEvidence {
        gesture_open: false,
        clip_head,
        clip_busy: false,
        held_for,
        shifted_head: false,
        head_released: true,
    };
    assert!(evidence.head_of_paste("b"), "the head 'b' was released as typing");
}

/// Time already spent waiting before the read still ends the hold: a typed
/// character is not held longer than before.
#[test]
fn time_waited_before_the_read_still_counts() {
    let start = Some(Instant::now() - Duration::from_millis(10));
    let (held_for, clip_head) = held_for_then_read(start, || Some("bir sonraki"));
    let evidence = PasteHeadEvidence {
        gesture_open: false,
        clip_head,
        clip_busy: false,
        held_for,
        shifted_head: false,
        head_released: true,
    };
    assert!(!evidence.head_of_paste("b"));
}

/// No pending start means nothing has waited.
#[test]
fn no_start_is_zero() {
    let (held_for, v) = held_for_then_read(None, || 7);
    assert_eq!(held_for, Duration::ZERO);
    assert_eq!(v, 7);
}

// A clipboard another window holds open is unknown, not empty.
//
// Sweep 6 (2026-11-19_11-38-54): test_altgr_letter_paste got 'iyi bi' in
// front of ESC[200~. The client's input log showed "clipboard busy, head not
// cached" then "zero-latency flush 1 char(s) as typing" for 'b' and again for
// 'i': a busy read gave no head, the prefix check said no, and the held head
// went out as typing. With a second process holding the clipboard 40 ms in
// every 100 it failed 3 of 5 runs, 0 of 8 without.

fn busy_evidence(held_ms: u64, released: bool) -> PasteHeadEvidence<'static> {
    PasteHeadEvidence {
        gesture_open: false,
        clip_head: None,
        clip_busy: true,
        held_for: Duration::from_millis(held_ms),
        shifted_head: false,
        head_released: released,
    }
}

/// A host written first character is held while the clipboard is busy.
#[test]
fn busy_clipboard_holds_the_head_of_a_paste() {
    assert!(busy_evidence(0, true).head_of_paste("b"), "'b' released as typing on a busy clipboard");
    assert!(busy_evidence(19, true).head_of_paste("bi"), "two characters are a burst and keep the window");
}

/// The busy hold is no longer than the prefix hold.
#[test]
fn busy_clipboard_hold_is_short() {
    let ms = PASTE_HEAD_PREFIX_HOLD.as_millis() as u64;
    assert!(!busy_evidence(ms, true).head_of_paste("b"));
}

/// A typed key (its release not read yet) is never held for a busy clipboard.
#[test]
fn busy_clipboard_does_not_hold_typing() {
    assert!(!busy_evidence(0, false).head_of_paste("b"));
}

/// The zero latency flush leaves the head pending on a busy clipboard.
#[test]
fn busy_clipboard_is_not_flushed_as_typing() {
    assert!(!should_zero_latency_flush_paste_pend("b", true, false, false, busy_evidence(0, true)));
    assert!(should_zero_latency_flush_paste_pend("b", true, false, false, busy_evidence(0, false)));
}

/// A known clipboard that does not start with the character still flushes it.
#[test]
fn known_clipboard_that_does_not_match_still_flushes() {
    let ev = PasteHeadEvidence { clip_head: Some("xyz"), clip_busy: false, ..busy_evidence(0, true) };
    assert!(should_zero_latency_flush_paste_pend("b", true, false, false, ev));
}

/// The cache reports a busy read, and clears it once a read succeeds.
#[test]
fn cache_reports_busy_until_a_read_succeeds() {
    let mut cache = ClipboardHeadCache::default();
    assert_eq!(cache.state_with(4, || Err(crate::clipboard::ClipboardBusy)), (None, true));
    assert_eq!(cache.state_with(4, || Err(crate::clipboard::ClipboardBusy)), (None, true));
    assert_eq!(cache.state_with(4, || Ok(Some("abc".into()))), (Some("abc"), false));
    assert_eq!(cache.state_with(5, || Ok(None)), (None, false));
}
/// The time a clipboard read blocked the loop is taken off the pending
/// buffer's age, so two busy reads (14 and 10 ms measured) no longer use up
/// the 20 ms window before the rest of the paste is read.
#[test]
fn read_time_is_not_counted_against_the_window() {
    let start = Instant::now() - Duration::from_millis(25);
    let shifted = shift_pend_start_past_read(Some(start), Duration::from_millis(24)).unwrap();
    assert!(shifted.elapsed() < Duration::from_millis(20), "window still used up: {:?}", shifted.elapsed());
}

/// The start never moves into the future, and nothing pending stays nothing.
#[test]
fn shifted_start_is_never_in_the_future() {
    let start = Instant::now();
    let shifted = shift_pend_start_past_read(Some(start), Duration::from_secs(5)).unwrap();
    assert!(shifted <= Instant::now());
    assert_eq!(shift_pend_start_past_read(None, Duration::from_millis(9)), None);
}