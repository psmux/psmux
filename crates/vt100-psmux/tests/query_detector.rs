use vt100_psmux::{Parser, QueryDetector, TerminalQuery};

fn live() -> Parser {
    let mut parser = Parser::default();
    parser.screen_mut().capture_queries(true);
    parser
}

#[test]
fn detector_and_live_parser_share_queries_at_every_split() {
    for (bytes, expected) in [
        (&b"\x1b[c"[..], TerminalQuery::Da1),
        (&b"\x1b[0c"[..], TerminalQuery::Da1),
        (&b"\x1b[>c"[..], TerminalQuery::Da2),
        (&b"\x1b[5n"[..], TerminalQuery::DsrStatus),
        (&b"\x1b[?2026$p"[..], TerminalQuery::DecrqmPrivate(2026)),
        (&b"\x1b[4$p"[..], TerminalQuery::DecrqmAnsi(4)),
        (&b"\x1b]7501;?\x07"[..], TerminalQuery::ProgramStatus),
        (&b"\x1b]7501;?\x1b\\"[..], TerminalQuery::ProgramStatus),
    ] {
        for cut in 0..=bytes.len() {
            let mut detector = QueryDetector::default();
            let mut parser = live();
            let mut queries = Vec::new();
            for batch in [&bytes[..cut], &bytes[cut..]] {
                let early = detector.scan(batch);
                parser.process(batch);
                let captured = parser.screen_mut().take_queries();
                assert_eq!(early, !captured.is_empty(), "{bytes:?}, cut {cut}");
                queries.extend(captured);
            }
            assert_eq!(queries, vec![expected], "{bytes:?}, cut {cut}");
            assert!(!detector.scan(b"plain output"));
            assert!(!detector.scan(b""));
        }
    }
}

#[test]
fn reports_paint_and_unrelated_sequences_do_not_request_release() {
    for bytes in [
        &b"\x1b[?25l\x1b[2J\x1b[38;5;12mhello\x1b[0m"[..],
        &b"\x1b[?2026h\x1b[?2026l"[..],
        &b"\x1b[1c\x1b[>1c\x1b[?1;2c\x1b[>84;0;0c"[..],
        &b"\x1b[?0$p\x1b[0$p\x1b[?996n\x1b[>0q"[..],
        &b"\x1b]7501;state=blocked:app=test\x07"[..],
        &b"\x1b]7501;?;extra\x07"[..],
        &b"\x1b]2;7501;? [c [>c\x07"[..],
        &b"\x1bPignored [c [>c 7501;?\x1b\\"[..],
        &b"\x1b[\x18c"[..],
        &b"\x1b[0 !c"[..],
    ] {
        let mut detector = QueryDetector::default();
        let mut parser = live();
        for byte in bytes {
            assert!(!detector.scan(&[*byte]), "{bytes:?}");
            parser.process(&[*byte]);
            assert!(parser.screen_mut().take_queries().is_empty(), "{bytes:?}");
        }
    }
}

#[test]
fn overflowed_csi_and_large_osc_keep_the_live_parsers_context() {
    let mut bytes = b"\x1b[".to_vec();
    bytes.extend_from_slice(b"0;".repeat(40).as_slice());
    bytes.extend_from_slice(b"0c\x1b]2;");
    bytes.extend_from_slice(&vec![b'x'; 8192]);
    bytes.extend_from_slice(b"\x07\x1b]7501;?\x07\x1b[c");
    let mut detector = QueryDetector::default();
    let mut parser = live();
    let mut queries = Vec::new();
    for batch in bytes.chunks(13) {
        let early = detector.scan(batch);
        parser.process(batch);
        let captured = parser.screen_mut().take_queries();
        assert_eq!(early, !captured.is_empty());
        queries.extend(captured);
    }
    assert_eq!(queries, vec![TerminalQuery::ProgramStatus, TerminalQuery::Da1]);
}

#[test]
fn reports_stay_silent_and_live_queries_are_captured_once() {
    let mut detector = QueryDetector::default();
    let mut parser = live();
    let report = b"\x1b]7501;state=done:app=test\x07";
    assert!(!detector.scan(report));
    assert!(detector.scan(b"\x1b]7501;?\x07\x1b[c"));
    parser.process(report);
    parser.process(b"\x1b]7501;?\x07\x1b[c");
    assert_eq!(parser.screen_mut().take_queries(), vec![TerminalQuery::ProgramStatus, TerminalQuery::Da1]);
    assert_eq!(parser.screen().program_status().get("").unwrap().app.as_deref(), Some("test"));
    assert!(parser.screen_mut().take_queries().is_empty());
}
