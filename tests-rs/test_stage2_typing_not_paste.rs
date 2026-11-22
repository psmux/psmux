// Sweep 2026-11-22: typing at 12 ms per key froze for 1.4 s and the line
// appeared at once. A busy machine left keys queued, the client read five
// together, the 3 in 20 ms test took them for a paste, and stage 2 held every
// later key until input went quiet for 100 ms, which steady typing never does.
// Keys that keep arriving one at a time end that hold as typing.
use super::*;
use std::time::Duration;

#[test]
fn keys_a_typist_spaces_out_arrive_alone() {
    // 15 ms per key is about 66 keys a second; key repeat is about 30 ms.
    for gap in [5u64, 8, 12, 15, 30, 60, 99] {
        assert!(stage2_key_arrived_alone(Duration::from_millis(gap)), "{} ms", gap);
    }
}

#[test]
fn characters_of_one_paste_do_not_arrive_alone() {
    // A paste, or one sshd fragment of it, comes in microseconds apart.
    for us in [0u64, 50, 500, 2000, 4999] {
        assert!(!stage2_key_arrived_alone(Duration::from_micros(us)), "{} us", us);
    }
}

#[test]
fn a_single_fragment_head_is_not_enough_to_call_it_typing() {
    // The first character of each #598 fragment arrives alone after a 20 to
    // 60 ms gap, the rest of the fragment with it. One key alone, then
    // several together, never reaches the streak.
    let mut streak = 0u32;
    for gap_us in [40_000u64, 10, 10, 10, 30_000, 10, 10, 60_000, 10] {
        if stage2_key_arrived_alone(Duration::from_micros(gap_us)) { streak += 1 } else { streak = 0 }
        assert!(streak < PASTE_STAGE2_TYPED_STREAK);
    }
}

#[test]
fn held_keys_go_out_as_the_keys_they_were() {
    let mut batch = Vec::new();
    push_pend_as_keys("a b\"\\\t\n", &mut batch);
    assert_eq!(batch, vec![
        "send-text \"a\"\n", "send-key space\n", "send-text \"b\"\n",
        "send-text \"\\\"\"\n", "send-text \"\\\\\"\n",
        "send-key tab\n", "send-key enter\n",
    ]);
}
