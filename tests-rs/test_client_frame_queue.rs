//! PR #882: the attached client's incoming queue must stay bounded while the
//! main loop is stuck on a terminal that stopped reading, without blocking the
//! socket reader (which would get the client dropped by the server's 5 s write
//! timeout) and without changing what the main loop ends up acting on.

use super::*;

fn frame(n: usize) -> String {
    format!("{{\"n\":{},\"pad\":\"{}\"}}\n", n, "x".repeat(1000))
}

fn drain(rx: &FrameReceiver) -> Vec<String> {
    let mut v = Vec::new();
    while let Ok(l) = rx.try_recv() {
        v.push(l);
    }
    v
}

#[test]
fn classify_kinds() {
    assert_eq!(classify("{\"layout\":1}\n"), LineKind::Frame);
    assert_eq!(classify("NC\n"), LineKind::NoChange);
    assert_eq!(classify("NC"), LineKind::NoChange);
    assert_eq!(classify("DETACH\n"), LineKind::Other);
    assert_eq!(classify("DETACH-KILL-PARENT\n"), LineKind::Other);
    assert_eq!(classify("SWITCH other\n"), LineKind::Other);
    assert_eq!(classify("OK\n"), LineKind::Other);
}

#[test]
fn a_stalled_main_loop_keeps_one_frame_not_thousands() {
    let (tx, rx) = frame_queue();
    for n in 0..10_000 {
        assert!(tx.send(frame(n)));
    }
    assert_eq!(tx.queued(), 1);
    let got = drain(&rx);
    assert_eq!(got, vec![frame(9_999)]);
}

#[test]
fn directives_keep_their_place_between_frames() {
    let (tx, rx) = frame_queue();
    for l in [frame(1), frame(2), "SWITCH other\n".to_string(), frame(3), frame(4),
              "DETACH\n".to_string(), frame(5)] {
        assert!(tx.send(l));
    }
    let got = drain(&rx);
    assert_eq!(got, vec![frame(2), "SWITCH other\n".to_string(), frame(4),
                         "DETACH\n".to_string(), frame(5)]);
}

/// What the main loop does with one drain: the last frame wins and every
/// directive is seen in order. Coalescing must not change that outcome.
fn main_loop_outcome(lines: &[String]) -> (Option<String>, Vec<String>, usize) {
    let mut last = None;
    let mut directives = Vec::new();
    let mut nc = 0;
    for l in lines {
        match classify(l) {
            LineKind::Frame => last = Some(l.clone()),
            LineKind::NoChange => nc += 1,
            LineKind::Other => directives.push(l.clone()),
        }
    }
    (last, directives, nc.min(1))
}

#[test]
fn the_main_loop_sees_the_same_outcome_as_with_the_plain_channel() {
    let seqs: Vec<Vec<String>> = vec![
        vec![frame(1), "NC\n".into(), frame(2), "NC\n".into(), "NC\n".into()],
        vec!["NC\n".into(), "NC\n".into(), "DETACH-KILL-PARENT\n".into(), frame(7)],
        vec![frame(1), "SWITCH a\n".into(), "SWITCH b\n".into(), frame(2), frame(3)],
        vec!["OK\n".into(), "OK\n".into(), frame(9)],
    ];
    for seq in seqs {
        let (tx, rx) = frame_queue();
        for l in &seq {
            assert!(tx.send(l.clone()));
        }
        let got = drain(&rx);
        assert_eq!(main_loop_outcome(&got), main_loop_outcome(&seq), "{:?}", seq);
    }
}

#[test]
fn nothing_is_coalesced_once_the_main_loop_has_taken_it() {
    let (tx, rx) = frame_queue();
    assert!(tx.send(frame(1)));
    assert_eq!(rx.try_recv().unwrap(), frame(1));
    assert!(tx.send(frame(2)));
    assert_eq!(rx.try_recv().unwrap(), frame(2));
    assert_eq!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
}

#[test]
fn disconnect_comes_after_the_queued_lines() {
    let (tx, rx) = frame_queue();
    assert!(tx.send(frame(1)));
    assert!(tx.send("DETACH\n".to_string()));
    drop(tx);
    assert_eq!(rx.try_recv().unwrap(), frame(1));
    assert_eq!(rx.try_recv().unwrap(), "DETACH\n");
    assert_eq!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
}

#[test]
fn the_reader_stops_when_the_main_loop_drops_its_end() {
    let (tx, rx) = frame_queue();
    drop(rx);
    assert!(!tx.send(frame(1)));
}

#[test]
fn a_reader_thread_flooding_frames_never_blocks_and_stays_bounded() {
    let (tx, rx) = frame_queue();
    let t = std::thread::spawn(move || {
        let mut max = 0;
        for n in 0..50_000 {
            assert!(tx.send(frame(n)));
            max = max.max(tx.queued());
        }
        assert!(tx.send("DETACH\n".to_string()));
        max
    });
    // The main loop is "stalled": it does not drain until the reader is done.
    let max = t.join().unwrap();
    assert_eq!(max, 1);
    assert_eq!(drain(&rx), vec![frame(49_999), "DETACH\n".to_string()]);
    assert_eq!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
}
