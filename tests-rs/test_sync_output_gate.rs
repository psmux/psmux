// DECSET 2026 synchronized output: output between `ESC[?2026h` and
// `ESC[?2026l` is held back and released to the parser as one batch.
use super::*;

const B: &[u8] = b"\x1b[?2026h";
const E: &[u8] = b"\x1b[?2026l";

fn cat(parts: &[&[u8]]) -> Vec<u8> { parts.concat() }

#[test]
fn sync_gate_passes_plain_output_through() {
    let mut g = SyncGate::new();
    assert_eq!(g.feed(b"hello".to_vec(), Instant::now()), b"hello");
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_releases_a_complete_frame_in_one_batch() {
    let mut g = SyncGate::new();
    let frame = cat(&[B, b"frame", E]);
    assert_eq!(g.feed(frame.clone(), Instant::now()), frame);
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_holds_an_open_frame_and_passes_what_came_before() {
    let mut g = SyncGate::new();
    let out = g.feed(cat(&[b"before", B, b"half"]), Instant::now());
    assert_eq!(out, b"before");
    assert!(g.time_left(Instant::now()).is_some());
}

#[test]
fn sync_gate_releases_the_whole_frame_when_the_end_arrives() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"one "]), Instant::now()).is_empty());
    assert!(g.feed(b"two ".to_vec(), Instant::now()).is_empty());
    let out = g.feed(cat(&[b"three", E, b"after"]), Instant::now());
    assert_eq!(out, cat(&[B, b"one two three", E, b"after"]));
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_finds_an_end_marker_split_across_reads() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"x", &E[..4]]), Instant::now()).is_empty());
    let out = g.feed(E[4..].to_vec(), Instant::now());
    assert_eq!(out, cat(&[B, b"x", E]));
}

#[test]
fn sync_gate_times_out_an_unterminated_frame() {
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(cat(&[B, b"stuck"]), t0).is_empty());
    assert!(!g.time_left(t0).unwrap().is_zero());
    let later = t0 + SYNC_TIMEOUT + Duration::from_millis(1);
    assert!(g.time_left(later).unwrap().is_zero());
    assert_eq!(g.feed(Vec::new(), later), cat(&[B, b"stuck"]));
    assert!(g.time_left(later).is_none());
}

#[test]
fn sync_gate_holds_a_second_frame_that_opens_after_a_closed_one() {
    let mut g = SyncGate::new();
    let out = g.feed(cat(&[B, b"a", E, B, b"b"]), Instant::now());
    assert_eq!(out, cat(&[B, b"a", E]));
    let out = g.feed(E.to_vec(), Instant::now());
    assert_eq!(out, cat(&[B, b"b", E]));
}

#[test]
fn sync_gate_take_all_returns_what_is_held() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"x"]), Instant::now()).is_empty());
    assert_eq!(g.take_all(), cat(&[B, b"x"]));
    assert!(g.time_left(Instant::now()).is_none());
}
