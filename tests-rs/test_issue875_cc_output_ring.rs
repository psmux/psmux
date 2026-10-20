// Issue #875: a control client's %output lost pane output when a pane wrote
// more than 64 KiB between two server loop passes, because the pane's output
// ring dropped its oldest bytes to make room.
use super::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// set_consumed replaces the whole consumed set, so tests that use it run
// one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn ring() -> Ring {
    Arc::new(Mutex::new(std::collections::VecDeque::new()))
}

fn len(r: &Ring) -> usize {
    r.lock().unwrap().len()
}

#[test]
fn unconsumed_ring_stays_bounded_and_never_blocks() {
    let _g = serial();
    set_consumed(&[], false);
    let r = ring();
    let chunk = vec![b'a'; 40_000];
    for _ in 0..10 {
        push(&r, &chunk);
    }
    assert_eq!(len(&r), RING_UNCONSUMED_MAX);
    // One read larger than the cap keeps its newest bytes.
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    push(&r, &big);
    let kept: Vec<u8> = r.lock().unwrap().iter().copied().collect();
    assert_eq!(kept, big[big.len() - RING_UNCONSUMED_MAX..].to_vec());
}

#[test]
fn consumed_ring_keeps_every_byte_past_the_old_cap() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    // 900 KiB in 64 KiB reads: the old ring kept only the last 64 KiB.
    let mut sent = Vec::new();
    for i in 0..14u32 {
        let chunk: Vec<u8> = (0..65536u32).map(|j| ((i * 7 + j) % 200) as u8 + 32).collect();
        push(&r, &chunk);
        sent.extend_from_slice(&chunk);
    }
    assert_eq!(len(&r), sent.len());
    assert_eq!(drain(&r), sent);
    set_consumed(&[], false);
}

#[test]
fn full_consumed_ring_holds_the_reader_until_drained() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    let total = RING_HIGH + 3 * 65536;
    let writer = std::thread::spawn(move || {
        let chunk = vec![b'x'; 65536];
        let mut n = 0;
        while n < total {
            push(&r2, &chunk);
            n += chunk.len();
        }
    });
    // The reader stops at RING_HIGH instead of dropping or growing.
    let t0 = Instant::now();
    while len(&r) < RING_HIGH && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(2));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(!writer.is_finished(), "reader must wait on a full ring");
    assert!(len(&r) <= RING_HIGH + 65536);
    // Draining lets it go on; every byte arrives.
    let mut got = 0;
    let t0 = Instant::now();
    while got < total && t0.elapsed() < Duration::from_secs(10) {
        got += drain(&r).len();
        notify_drained();
        std::thread::sleep(Duration::from_millis(1));
    }
    writer.join().unwrap();
    got += drain(&r).len();
    assert_eq!(got, total);
    set_consumed(&[], false);
}

#[test]
fn reader_waiting_on_a_full_ring_is_released_when_consuming_stops() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    let writer = std::thread::spawn(move || {
        push(&r2, &vec![b'y'; RING_HIGH + 10]);
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(!writer.is_finished());
    set_consumed(&[], false); // the last control client left
    let t0 = Instant::now();
    while !writer.is_finished() && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(writer.is_finished(), "reader stays blocked after consuming stopped");
}

#[test]
fn reader_waiting_on_a_closed_pane_is_released() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    let writer = std::thread::spawn(move || {
        push(&r2, &vec![b'z'; RING_HIGH + 10]);
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(!writer.is_finished());
    // The pane is dropped; only the reader still holds the ring.
    let weak = Arc::downgrade(&r);
    drop(r);
    let t0 = Instant::now();
    while !writer.is_finished() && t0.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(writer.is_finished(), "reader stays blocked after its pane closed");
    drop(weak);
    set_consumed(&[], false);
}

#[test]
fn first_attach_empties_the_rings_but_a_later_pane_keeps_its_output() {
    let _g = serial();
    set_consumed(&[], false);
    let old = ring();
    push(&old, b"written before any control client");
    set_consumed(&[&old], true);
    assert_eq!(len(&old), 0, "a new control client is not sent old output");
    push(&old, b"after");
    // A pane created later joins without being emptied.
    let newer = ring();
    push(&newer, b"prompt> ");
    set_consumed(&[&old, &newer], false);
    assert_eq!(drain(&newer), b"prompt> ".to_vec());
    // A second control client attaching does not take the first one's bytes.
    set_consumed(&[&old, &newer], true);
    assert_eq!(drain(&old), b"after".to_vec());
    set_consumed(&[], false);
}

#[test]
fn drain_holds_back_a_split_utf8_character() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let s = "box \u{2500}\u{2502} end".as_bytes().to_vec();
    // Cut inside the 3 byte U+2502.
    let cut = s.len() - " end".len() - 1;
    push(&r, &s[..cut]);
    let first = drain(&r);
    push(&r, &s[cut..]);
    let second = drain(&r);
    assert!(std::str::from_utf8(&first).is_ok());
    assert!(std::str::from_utf8(&second).is_ok());
    assert_eq!([first, second].concat(), s);
    set_consumed(&[], false);
}

#[test]
fn complete_utf8_len_cases() {
    assert_eq!(complete_utf8_len(b""), 0);
    assert_eq!(complete_utf8_len(b"abc"), 3);
    let e = "\u{e9}".as_bytes(); // 2 bytes
    assert_eq!(complete_utf8_len(&[b'a', e[0]]), 1);
    assert_eq!(complete_utf8_len(&[b'a', e[0], e[1]]), 3);
    let w = "\u{1F600}".as_bytes(); // 4 bytes
    assert_eq!(complete_utf8_len(&w[..3]), 0);
    assert_eq!(complete_utf8_len(w), 4);
    // Invalid bytes are passed on, not held forever.
    assert_eq!(complete_utf8_len(&[0x80, 0x80, 0x80, 0x80]), 4);
    assert_eq!(complete_utf8_len(&[b'a', 0xFF]), 2);
}

#[test]
fn chunks_split_on_character_boundaries() {
    assert_eq!(chunks("short".into()), vec!["short".to_string()]);
    let s: String = "\u{2500}".repeat(50_000); // 150 000 bytes
    let parts = chunks(s.clone());
    assert!(parts.len() >= 3);
    assert!(parts.iter().all(|p| p.len() <= OUTPUT_CHUNK_MAX && !p.is_empty()));
    assert_eq!(parts.concat(), s);
}

#[test]
fn gate_counts_queued_bytes_and_tells_a_client_past_the_ceiling() {
    use crate::control::{ControlOutputGate, GateEnqueue, CONTROL_QUEUE_MAX};
    let t0 = Instant::now();
    let g = ControlOutputGate::new(None);
    let GateEnqueue::Send(a) = g.enqueue_sized(1, t0, 1000) else { panic!() };
    assert_eq!(g.queued_bytes(), 1000);
    g.written(1000);
    assert_eq!(g.queued_bytes(), 0);
    let GateEnqueue::Send(b) = g.enqueue_sized(1, t0, 500) else { panic!() };
    g.unqueue_sized(1, b, 500);
    assert_eq!(g.queued_bytes(), 0);
    let _ = a;
    // Without pause-after, past the ceiling the client is too far behind.
    let GateEnqueue::Send(_) = g.enqueue_sized(1, t0, CONTROL_QUEUE_MAX - 10) else { panic!() };
    assert_eq!(g.enqueue_sized(1, t0, 100), GateEnqueue::TooFarBehind);
    assert!(g.is_too_far_behind());
    // With pause-after, the pane is paused instead (a %pause line is due).
    let g = ControlOutputGate::new(None);
    g.set_pause_after(Some(60_000));
    let GateEnqueue::Send(_) = g.enqueue_sized(2, t0, CONTROL_QUEUE_MAX - 10) else { panic!() };
    assert_eq!(g.enqueue_sized(2, t0, 100), GateEnqueue::Paused);
    assert_eq!(g.take_pending_pauses(), vec![2]);
}
