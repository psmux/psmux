// Issue #766: a client driven through ConPTY (node-pty) wrote
// `ESC[200~ text ESC[201~` and then a lone CR 150 ms later. The console host
// strips the paste markers and hands the client key records, so the client
// held the text in stage 2 for its 300 ms window and appended the CR to it:
// the pane read `ESC[200~ text CR ESC[201~`, a pasted newline instead of a
// submit. Separately, a first character that needs Shift was handed over in a
// console write of its own and went out as typing in front of `ESC[200~`.
use super::*;
use std::time::Duration;

#[test]
fn a_key_after_a_quiet_held_paste_ends_the_paste_first() {
    // The reporter's shape: the paste was complete about 25 ms after it was
    // written, the CR came 150 ms after it, so 125 ms of quiet.
    assert!(stage2_paste_ended_by_new_key(true, true, Duration::from_millis(125)));
    assert!(stage2_paste_ended_by_new_key(true, true, PASTE_STAGE2_QUIET));
}

#[test]
fn a_fragment_of_the_same_paste_keeps_accumulating() {
    // sshd's ConPTY hands a fragmented paste over 20 to 60 ms apart (#598):
    // those fragments are one paste and must not be cut at a gap that short.
    for gap in [0u64, 1, 20, 60, 99] {
        assert!(
            !stage2_paste_ended_by_new_key(true, true, Duration::from_millis(gap)),
            "a {} ms gap ended the paste", gap
        );
    }
}

#[test]
fn nothing_is_ended_outside_stage_two() {
    // Not yet a paste (inside the 20 ms window), or nothing held.
    assert!(!stage2_paste_ended_by_new_key(false, true, Duration::from_millis(500)));
    assert!(!stage2_paste_ended_by_new_key(true, false, Duration::from_millis(500)));
}

#[test]
fn the_key_rule_and_the_timer_rule_share_one_quiet_threshold() {
    // The timer still ends a paste nobody follows; the key rule only moves the
    // same decision to the moment the next key proves the paste was over.
    let quiet = PASTE_STAGE2_QUIET;
    assert!(stage2_paste_is_over(Duration::from_millis(301), quiet));
    assert!(stage2_paste_ended_by_new_key(true, true, quiet));
    let short = PASTE_STAGE2_QUIET - Duration::from_millis(1);
    assert!(!stage2_paste_is_over(Duration::from_millis(301), short));
    assert!(!stage2_paste_ended_by_new_key(true, true, short));
}

fn shifted(held_ms: u64) -> PasteHeadEvidence<'static> {
    PasteHeadEvidence {
        gesture_open: false,
        clip_head: None,
        held_for: Duration::from_millis(held_ms),
        shifted_head: true,
    }
}

#[test]
fn a_lone_shifted_head_is_held_for_the_short_window() {
    // `Y` arrived alone; the rest of the paste came about 1 ms later.
    assert!(!should_zero_latency_flush_paste_pend("Y", true, false, false, shifted(0)));
    assert!(!should_zero_latency_flush_paste_pend("(", true, false, false, shifted(2)));
}

#[test]
fn a_typed_shifted_character_goes_out_after_the_short_window() {
    // Nothing followed: it was a keystroke, committed 3 ms late, not 20.
    assert!(should_zero_latency_flush_paste_pend("Y", true, false, false, shifted(3)));
    assert!(should_zero_latency_flush_paste_pend("Y", true, false, false, shifted(19)));
}

#[test]
fn an_unshifted_character_is_still_flushed_at_once() {
    let ev = PasteHeadEvidence { shifted_head: false, ..shifted(0) };
    assert!(should_zero_latency_flush_paste_pend("y", true, false, false, ev));
}

#[test]
fn the_shifted_rule_covers_one_character_only() {
    // Two characters is a burst shape that the clipboard and gesture rules
    // already judge; the shifted head says nothing about the second one.
    assert!(should_zero_latency_flush_paste_pend("Yo", true, false, false, shifted(0)));
}

#[test]
fn paste_detection_off_never_holds_a_shifted_head() {
    assert!(should_zero_latency_flush_paste_pend("Y", false, false, false, shifted(0)));
}
