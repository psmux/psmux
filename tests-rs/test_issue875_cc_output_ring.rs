// Issue #875: a control client's %output lost pane output when a pane wrote
// more than 64 KiB between two server loop passes, because the pane's output
// ring dropped its oldest bytes to make room.
use super::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn serial() -> std::sync::MutexGuard<'static, ()> {
    TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner())
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

// The server loop stuck while a reader waits on a full ring: on Windows that
// is a cycle, since the loop's AttachConsole and ResizePseudoConsole wait for
// a console host that is itself waiting on the pipe this reader stopped
// reading. No test here calls set_consumed after the start, which is what a
// loop that stopped passing looks like to the reader.
#[test]
fn reader_stops_waiting_for_a_stuck_loop() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    let t0 = Instant::now();
    let writer = std::thread::spawn(move || {
        push(&r2, &vec![b'a'; RING_HIGH + 10]);
    });
    std::thread::sleep(LOOP_STUCK_AFTER / 2);
    assert!(!writer.is_finished(), "a reader must wait for a loop that may still come back");
    while !writer.is_finished() && t0.elapsed() < LOOP_STUCK_AFTER + Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(writer.is_finished(), "reader stays blocked on a stuck loop");
    assert!(t0.elapsed() >= LOOP_STUCK_AFTER, "gave up before the limit");
    assert_eq!(len(&r), RING_HIGH);
    set_consumed(&[], false);
}

#[test]
fn while_the_loop_stays_stuck_more_output_does_not_wait_and_keeps_the_newest() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    std::thread::spawn(move || push(&r2, &vec![b'a'; RING_HIGH + 10])).join().unwrap();
    // The reader already found the loop stuck: these neither wait nor grow.
    let t0 = Instant::now();
    for i in 0..50u8 {
        push(&r, &vec![i; 1000]);
    }
    assert!(t0.elapsed() < LOOP_STUCK_AFTER, "a read must not wait again while the loop is stuck");
    assert_eq!(len(&r), RING_HIGH);
    let kept: Vec<u8> = r.lock().unwrap().iter().copied().collect();
    assert_eq!(kept[kept.len() - 1000..], vec![49u8; 1000][..]);
    assert_eq!(kept[0], b'a', "the oldest bytes are the ones dropped");
    set_consumed(&[], false);
}

// All clients behind: flush_pane_output keeps passing and leaves the ring
// alone on purpose (#875). A timeout alone would turn this hold into dropped
// output after two seconds.
#[test]
fn reader_keeps_waiting_while_the_loop_passes_without_draining() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    let writer = std::thread::spawn(move || {
        push(&r2, &vec![b'b'; RING_HIGH + 10]);
    });
    let t0 = Instant::now();
    while t0.elapsed() < LOOP_STUCK_AFTER + Duration::from_secs(1) {
        set_consumed(&[&r], false);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!writer.is_finished(), "a reader must wait for a live loop however long it holds back");
    assert_eq!(len(&r), RING_HIGH + 10, "nothing is dropped while the loop runs");
    let mut got = drain(&r).len();
    notify_drained();
    writer.join().unwrap();
    got += drain(&r).len();
    assert_eq!(got, RING_HIGH + 10);
    set_consumed(&[], false);
}

#[test]
fn a_loop_that_passes_again_is_waited_for_again() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let r2 = r.clone();
    std::thread::spawn(move || push(&r2, &vec![b'c'; RING_HIGH + 10])).join().unwrap();
    // The loop came back, without draining yet.
    set_consumed(&[&r], false);
    let r2 = r.clone();
    let writer = std::thread::spawn(move || push(&r2, b"more"));
    std::thread::sleep(LOOP_STUCK_AFTER / 2);
    assert!(!writer.is_finished(), "a live loop's full ring holds the reader again");
    assert_eq!(drain(&r).len(), RING_HIGH + 4);
    notify_drained();
    writer.join().unwrap();
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
fn drain_preserves_a_split_utf8_character() {
    let _g = serial();
    let r = ring();
    set_consumed(&[&r], true);
    let s = "box \u{2500}\u{2502} end".as_bytes().to_vec();
    // Cut inside the 3 byte U+2502.
    let cut = s.len() - " end".len() - 1;
    assert!(std::str::from_utf8(&s[..cut]).is_err());
    push(&r, &s[..cut]);
    let first = drain(&r);
    assert_eq!(first, s[..cut]);
    assert_eq!(len(&r), 0);
    push(&r, &s[cut..]);
    let second = drain(&r);
    assert_eq!(second, s[cut..]);
    assert_eq!(len(&r), 0);
    assert_eq!([first, second].concat(), s);
    set_consumed(&[], false);
}

#[test]
fn drain_keeps_arbitrary_bytes_including_incomplete_utf8_tails() {
    let r = ring();
    for bytes in [b"".as_slice(), b"abc", b"\x80\xff", b"\xc2", b"\xe2\x94", b"\xf0\x9f\x98"] {
        r.lock().unwrap().extend(bytes);
        assert_eq!(len(&r), bytes.len());
        assert_eq!(drain(&r), bytes);
        assert_eq!(len(&r), 0);
    }
}

#[test]
fn chunks_preserve_bytes_at_the_payload_limit() {
    for size in [0, 5, OUTPUT_CHUNK_MAX - 1, OUTPUT_CHUNK_MAX, OUTPUT_CHUNK_MAX + 1, 150_000] {
        let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
        let parts = chunks(data.clone());
        assert!(parts.iter().all(|p| p.len() <= OUTPUT_CHUNK_MAX));
        assert_eq!(parts.concat(), data);
        if size > 0 {
            assert!(parts.iter().all(|p| !p.is_empty()));
        }
    }
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
