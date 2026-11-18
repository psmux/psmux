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
