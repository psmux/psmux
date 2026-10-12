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
    // Linear means a chunk fed near the cap costs what one fed early costs,
    // so the last quarter of the flood is timed against the first. A rescan
    // of everything held makes the last quarter about seven times dearer.
    // A wall clock budget alone failed on a loaded CI runner (548 ms against
    // 500 ms, 170 ms on a quiet machine) with no change in the algorithm.
    let chunks = (SYNC_MAX_HELD / chunk.len()) - 1;
    let quarter = chunks / 4;
    let mut released = 0usize;
    let mut first = Duration::ZERO;
    let mut last = Duration::ZERO;
    let start = Instant::now();
    for n in 0..chunks {
        let s = Instant::now();
        released += g.feed(chunk.clone(), t0, false).len();
        if n < quarter { first += s.elapsed(); }
        if n >= chunks - quarter { last += s.elapsed(); }
    }
    let took = start.elapsed();
    eprintln!("flood of {} bytes held in {:?} (first quarter {:?}, last quarter {:?}), released {}",
        SYNC_MAX_HELD, took, first, last, released);
    assert_eq!(released, 0, "nothing may be released before the cap");
    assert!(last < first * 3 + Duration::from_millis(30),
        "the last quarter of the flood took {:?} against {:?} for the first: holding is not linear", last, first);
    assert!(took < Duration::from_secs(5), "holding a 4 MiB flood took {:?}", took);
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
fn sync_gate_shared_detection_releases_every_query_split() {
    for (query, expected) in [
        (&b"\x1b[c"[..], DeviceQuery::Da1),
        (&b"\x1b[>c"[..], DeviceQuery::Da2),
        (&b"\x1b[5n"[..], DeviceQuery::DsrStatus),
        (&b"\x1b[?2026$p"[..], DeviceQuery::DecrqmPrivate(2026)),
        (&b"\x1b[4$p"[..], DeviceQuery::DecrqmAnsi(4)),
        (&b"\x1b]7501;?\x07"[..], DeviceQuery::ProgramStatus),
        (&b"\x1b]7501;?\x1b\\"[..], DeviceQuery::ProgramStatus),
    ] {
        for cut in 0..=query.len() {
            let mut detector = vt100::QueryDetector::default();
            let mut gate = SyncGate::new();
            let mut parser = vt100::Parser::default();
            parser.screen_mut().capture_queries(true);
            let t0 = Instant::now();
            let held = cat(&[B, b"held\x1b]7501;state=blocked:app=test\x07"]);
            assert!(!detector.scan(&held));
            assert!(gate.feed(held, t0, false).is_empty());
            assert!(parser.screen().program_status().get("").is_none());
            let mut captured = Vec::new();
            for batch in [&query[..cut], &query[cut..]] {
                let flush = detector.scan(batch);
                let released = gate.feed(batch.to_vec(), t0, flush);
                parser.process(&released);
                captured.extend(parser.screen_mut().take_queries());
            }
            assert_eq!(captured, vec![expected], "{query:?}, cut {cut}");
            assert!(gate.is_open(), "a query must not close the update");
            assert_eq!(parser.screen().contents(), "held");
            assert_eq!(parser.screen().program_status().get("").unwrap().state, vt100::ProgramState::Blocked);
            assert!(gate.feed(b"rest".to_vec(), t0, detector.scan(b"rest")).is_empty());
            let released = gate.feed(E.to_vec(), t0, detector.scan(E));
            parser.process(&released);
            assert_eq!(parser.screen().contents(), "heldrest");
            assert!(parser.screen_mut().take_queries().is_empty());
            assert!(!gate.is_open());
        }
    }
}

#[test]
fn sync_gate_keeps_paint_and_reports_held_until_a_real_query() {
    let mut detector = vt100::QueryDetector::default();
    let mut gate = SyncGate::new();
    let mut parser = vt100::Parser::default();
    parser.screen_mut().capture_queries(true);
    let t0 = Instant::now();
    for batch in [B, b"paint\x1b[31m", b"\x1b]2;7501;? [c\x07", b"\x1b]7501;state=done\x07"] {
        assert!(!detector.scan(batch));
        assert!(gate.feed(batch.to_vec(), t0, false).is_empty());
    }
    assert_eq!(parser.screen().contents(), "");
    assert!(parser.screen().program_status().get("").is_none());
    let query = b"\x1b[?2004h\x1b]7501;?\x07\x1b[c\x1b[?2026$p";
    assert!(detector.scan(query));
    let released = gate.feed(query.to_vec(), t0, true);
    parser.process(&released);
    let queries = parser.screen_mut().take_queries();
    assert_eq!(queries, vec![DeviceQuery::ProgramStatus, DeviceQuery::Da1, DeviceQuery::DecrqmPrivate(2026)]);
    let mut state = DecrqmState::from_screen(parser.screen());
    state.synchronized_output = gate.is_open();
    assert!(state.bracketed_paste);
    let replies: String = queries.into_iter().map(|q| device_reply(q, &state)).collect();
    assert_eq!(replies, format!("{}\x1b[?1;2c\x1b[?2026;1$y", vt100::PROGRAM_STATUS_REPLY));
    assert_eq!(parser.screen().program_status().get("").unwrap().state, vt100::ProgramState::Done);
    assert!(!detector.scan(b""));
    assert!(parser.screen_mut().take_queries().is_empty());
}

#[test]
fn sync_gate_exit_drains_reports_before_expiring_transient_state() {
    let mut gate = SyncGate::new();
    let mut parser = vt100::Parser::default();
    let reports = cat(&[B, b"\x1b]7501;state=done\x07\x1b]7501;state=blocked:id=child\x07"]);
    assert!(gate.feed(reports, Instant::now(), false).is_empty());
    assert!(parser.screen().program_status().get("").is_none());
    parser.process(&gate.take_all());
    assert!(parser.screen().program_status().get("child").is_some());
    parser.screen_mut().program_status_mut().expire();
    assert_eq!(parser.screen().program_status().get("").unwrap().state, vt100::ProgramState::Done);
    assert!(parser.screen().program_status().get("child").is_none());
}

#[test]
fn parser_thread_answers_queries_before_an_open_update_times_out() {
    struct Output {
        batches: std::sync::mpsc::Receiver<Vec<u8>>,
        current: std::io::Cursor<Vec<u8>>,
    }
    impl std::io::Read for Output {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.current.position() as usize == self.current.get_ref().len() {
                match self.batches.recv() {
                    Ok(batch) => self.current = std::io::Cursor::new(batch),
                    Err(_) => return Ok(0),
                }
            }
            std::io::Read::read(&mut self.current, buf)
        }
    }
    let pane_id = usize::MAX - 7501;
    assert!(crate::types::take_device_replies(pane_id).is_none());
    let (send, receive) = std::sync::mpsc::channel();
    let parser = Arc::new(Mutex::new(vt100::Parser::default()));
    spawn_reader_thread(
        Box::new(Output { batches: receive, current: std::io::Cursor::new(Vec::new()) }),
        parser.clone(), Default::default(), Default::default(),
        Default::default(), Default::default(), Default::default(),
        Default::default(), pane_id, None,
    );
    let start = Instant::now();
    send.send(cat(&[B, b"paint\x1b]7501;state=blocked\x07\x1b]7501;?\x07\x1b[c\x1b[?2026$p"])).unwrap();
    let mut reply = Vec::new();
    let expected = format!("{}\x1b[?1;2c\x1b[?2026;1$y", vt100::PROGRAM_STATUS_REPLY).into_bytes();
    while reply.len() < expected.len() && start.elapsed() < SYNC_TIMEOUT / 2 {
        if let Some(bytes) = crate::types::take_device_replies(pane_id) { reply.extend(bytes); }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(reply, expected, "a program waiting on replies must not wait for the sync timeout");
    assert_eq!(parser.lock().unwrap().screen().contents(), "paint");
    assert_eq!(parser.lock().unwrap().screen().program_status().get("").unwrap().state, vt100::ProgramState::Blocked);
    drop(send);
    let exit = Instant::now();
    while parser.lock().unwrap().screen().program_status().get("").is_some() && exit.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(parser.lock().unwrap().screen().program_status().get("").is_none());
    assert!(crate::types::take_device_replies(pane_id).is_none());
}

#[test]
fn sync_gate_shared_detection_flood_stays_linear() {
    let mut detector = vt100::QueryDetector::default();
    let mut gate = SyncGate::new();
    let t0 = Instant::now();
    assert!(!detector.scan(B));
    assert!(gate.feed(B.to_vec(), t0, false).is_empty());
    let chunk = vec![b'x'; 16 * 1024];
    let chunks = (SYNC_MAX_HELD / chunk.len()) - 1;
    let quarter = chunks / 4;
    let mut first = Duration::ZERO;
    let mut last = Duration::ZERO;
    let start = Instant::now();
    for n in 0..chunks {
        let s = Instant::now();
        assert!(!detector.scan(&chunk));
        assert!(gate.feed(chunk.clone(), t0, false).is_empty());
        if n < quarter { first += s.elapsed(); }
        if n >= chunks - quarter { last += s.elapsed(); }
    }
    let took = start.elapsed();
    eprintln!("shared detector + gate held {} bytes in {:?} (first {:?}, last {:?})", chunks * chunk.len(), took, first, last);
    assert!(last < first * 3 + Duration::from_millis(30), "holding must remain linear: first {first:?}, last {last:?}");
    assert!(took < Duration::from_secs(5), "shared detection and holding took {took:?}");
}

#[test]
fn sync_gate_ignores_other_private_modes_that_share_the_prefix() {
    let mut g = SyncGate::new();
    let other = b"\x1b[?20260h\x1b[?2026$p".to_vec();
    assert_eq!(g.feed(other.clone(), Instant::now(), false), other);
    assert!(!g.is_open());
}
