// DECSET 2026 synchronized output: output between `ESC[?2026h` and
// `ESC[?2026l` is held back and released to the parser as one batch.
use super::*;

const B: &[u8] = b"\x1b[?2026h";
const E: &[u8] = b"\x1b[?2026l";

fn cat(parts: &[&[u8]]) -> Vec<u8> { parts.concat() }

#[test]
fn sync_gate_passes_plain_output_through() {
    let mut g = SyncGate::new();
    assert_eq!(g.feed(b"hello".to_vec(), Instant::now(), false), b"hello");
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_releases_a_complete_frame_in_one_batch() {
    let mut g = SyncGate::new();
    let frame = cat(&[B, b"frame", E]);
    assert_eq!(g.feed(frame.clone(), Instant::now(), false), frame);
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_holds_an_open_frame_and_passes_what_came_before() {
    let mut g = SyncGate::new();
    let out = g.feed(cat(&[b"before", B, b"half"]), Instant::now(), false);
    assert_eq!(out, b"before");
    assert!(g.time_left(Instant::now()).is_some());
}

#[test]
fn sync_gate_releases_the_whole_frame_when_the_end_arrives() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"one "]), Instant::now(), false).is_empty());
    assert!(g.feed(b"two ".to_vec(), Instant::now(), false).is_empty());
    let out = g.feed(cat(&[b"three", E, b"after"]), Instant::now(), false);
    assert_eq!(out, cat(&[B, b"one two three", E, b"after"]));
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_finds_an_end_marker_split_across_reads() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"x", &E[..4]]), Instant::now(), false).is_empty());
    let out = g.feed(E[4..].to_vec(), Instant::now(), false);
    assert_eq!(out, cat(&[B, b"x", E]));
}

#[test]
fn sync_gate_times_out_an_unterminated_frame() {
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(cat(&[B, b"stuck"]), t0, false).is_empty());
    assert!(!g.time_left(t0).unwrap().is_zero());
    let later = t0 + SYNC_TIMEOUT + Duration::from_millis(1);
    assert!(g.time_left(later).unwrap().is_zero());
    assert_eq!(g.feed(Vec::new(), later, false), cat(&[B, b"stuck"]));
    assert!(g.time_left(later).is_none());
}

#[test]
fn sync_gate_holds_a_second_frame_that_opens_after_a_closed_one() {
    let mut g = SyncGate::new();
    let out = g.feed(cat(&[B, b"a", E, B, b"b"]), Instant::now(), false);
    assert_eq!(out, cat(&[B, b"a", E]));
    let out = g.feed(E.to_vec(), Instant::now(), false);
    assert_eq!(out, cat(&[B, b"b", E]));
}

#[test]
fn sync_gate_take_all_returns_what_is_held() {
    let mut g = SyncGate::new();
    assert!(g.feed(cat(&[B, b"x"]), Instant::now(), false).is_empty());
    assert_eq!(g.take_all(), cat(&[B, b"x"]));
    assert!(g.time_left(Instant::now()).is_none());
}

#[test]
fn sync_gate_flood_inside_an_open_frame_stays_linear() {
    // An open update that floods megabytes before its end (or never ends)
    // must not rescan everything it holds on every read: that is quadratic
    // and stalls the parser thread for the whole pane.
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(B.to_vec(), t0, false).is_empty());
    let chunk = vec![b'x'; 16 * 1024];
    let start = Instant::now();
    let mut released = 0usize;
    for _ in 0..((SYNC_MAX_HELD / chunk.len()) - 1) {
        released += g.feed(chunk.clone(), t0, false).len();
    }
    let took = start.elapsed();
    eprintln!("flood of {} bytes held in {:?}, released {}", SYNC_MAX_HELD, took, released);
    assert_eq!(released, 0, "nothing may be released before the cap");
    assert!(took < Duration::from_millis(500), "holding a 4 MiB flood took {:?}", took);
}

#[test]
fn sync_gate_flush_releases_but_keeps_the_update_open() {
    // A query inside an open update (the program is blocked on its reply)
    // releases what is held at once; the rest of the frame is still held.
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(cat(&[B, b"top"]), t0, false).is_empty());
    let out = g.feed(b"\x1b[c".to_vec(), t0, true);
    assert_eq!(out, cat(&[B, b"top", b"\x1b[c"]));
    assert!(g.is_open(), "the update is still open after a query");
    assert!(g.feed(b"rest".to_vec(), t0, false).is_empty());
    assert_eq!(g.feed(E.to_vec(), t0, false), cat(&[b"rest", E]));
    assert!(!g.is_open());
}

#[test]
fn sync_gate_flush_without_an_open_update_is_a_pass_through() {
    let mut g = SyncGate::new();
    assert_eq!(g.feed(b"\x1b[6n".to_vec(), Instant::now(), true), b"\x1b[6n");
    assert!(!g.is_open());
}

#[test]
fn sync_gate_open_with_nothing_held_still_times_out() {
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert_eq!(g.feed(cat(&[B, b"\x1b[c"]), t0, true), cat(&[B, b"\x1b[c"]));
    assert!(g.is_open());
    let later = t0 + SYNC_TIMEOUT;
    assert!(g.time_left(later).unwrap().is_zero());
    assert!(g.feed(Vec::new(), later, false).is_empty());
    assert!(!g.is_open());
    assert!(g.time_left(later).is_none());
}

#[test]
fn sync_gate_holds_a_new_update_that_arrives_with_the_timeout() {
    // The expired update is released, and a fresh begin in the same batch
    // starts a new held update instead of being released half drawn.
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(cat(&[B, b"old"]), t0, false).is_empty());
    let later = t0 + SYNC_TIMEOUT + Duration::from_millis(5);
    let out = g.feed(cat(&[b"tail", B, b"new"]), later, false);
    assert_eq!(out, cat(&[B, b"old", b"tail"]));
    assert!(g.is_open());
    assert_eq!(g.feed(E.to_vec(), later, false), cat(&[B, b"new", E]));
}

#[test]
fn sync_gate_releases_an_update_over_the_cap() {
    let mut g = SyncGate::new();
    let t0 = Instant::now();
    assert!(g.feed(B.to_vec(), t0, false).is_empty());
    let big = vec![b'z'; SYNC_MAX_HELD];
    let out = g.feed(big, t0, false);
    assert_eq!(out.len(), SYNC_MAX_HELD + B.len());
    assert!(!g.is_open(), "an update over the cap ends, like the timeout");
    assert_eq!(g.feed(b"next".to_vec(), t0, false), b"next");
}

#[test]
fn sync_gate_finds_an_end_split_at_every_offset() {
    for cut in 1..E.len() {
        let mut g = SyncGate::new();
        let t0 = Instant::now();
        assert!(g.feed(cat(&[B, b"body", &E[..cut]]), t0, false).is_empty(), "cut {}", cut);
        let out = g.feed(cat(&[&E[cut..], b"x"]), t0, false);
        assert_eq!(out, cat(&[B, b"body", E, b"x"]), "cut {}", cut);
        assert!(!g.is_open(), "cut {}", cut);
    }
}

#[test]
fn sync_gate_ignores_other_private_modes_that_share_the_prefix() {
    let mut g = SyncGate::new();
    let other = b"\x1b[?20260h\x1b[?2026$p".to_vec();
    assert_eq!(g.feed(other.clone(), Instant::now(), false), other);
    assert!(!g.is_open());
}
