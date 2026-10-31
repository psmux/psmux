use vt100_psmux::{Parser, QueryDetector, TerminalQuery, MAX_PENDING_QUERIES};

fn live() -> Parser {
    let mut parser = Parser::default();
    parser.screen_mut().capture_queries(true);
    parser.screen_mut().set_program_status_enabled(true);
    parser
}

fn detector() -> QueryDetector {
    let mut detector = QueryDetector::default();
    detector.set_program_status_enabled(true);
    detector
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
        (&b"\x1b[14t"[..], TerminalQuery::WindowReport(14)),
        (&b"\x1b[16t"[..], TerminalQuery::WindowReport(16)),
        (&b"\x1b[18t"[..], TerminalQuery::WindowReport(18)),
        (&b"\x1b[19t"[..], TerminalQuery::WindowReport(19)),
        (&b"\x1b]7501;?\x07"[..], TerminalQuery::ProgramStatus),
        (&b"\x1b]7501;?\x1b\\"[..], TerminalQuery::ProgramStatus),
        (&b"\x1b[>q"[..], TerminalQuery::Xtversion),
        (&b"\x1b[>0q"[..], TerminalQuery::Xtversion),
        (&b"\x1b[>0;0q"[..], TerminalQuery::Xtversion),
        (&b"\x1b[?996n"[..], TerminalQuery::ColourScheme),
        (&b"\x1b]10;?\x07"[..], TerminalQuery::ForegroundColour),
        (&b"\x1b]10;?\x1b\\"[..], TerminalQuery::ForegroundColour),
        (&b"\x1b]11;?\x1b\\"[..], TerminalQuery::BackgroundColour),
        (&b"\x1b]4;0;?\x1b\\"[..], TerminalQuery::PaletteColour { index: 0, own: None }),
        (&b"\x1b]4;15;?\x07"[..], TerminalQuery::PaletteColour { index: 15, own: None }),
    ] {
        for cut in 0..=bytes.len() {
            let mut detector = detector();
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
        &b"\x1b[?0$p\x1b[0$p\x1b[?995n\x1b[996n"[..],
        // `CSI > 1 q` is not a version request, and DECSCUSR shares the
        // final byte without the `>`.
        &b"\x1b[>1q\x1b[5 q\x1b[0 q"[..],
        // Colour sets, a palette index past the sixteen ANSI colours, a `?`
        // after a bad index, and an index with no colour are not queries.
        &b"\x1b]10;#ff0000\x07\x1b]11;rgb:00/00/00\x1b\\"[..],
        &b"\x1b]4;1;rgb:ff/00/00\x07\x1b]4;16;?\x07\x1b]4;256;?\x07"[..],
        &b"\x1b]10;?;?\x07\x1b]4;x;?\x07\x1b]4;x;?;1;?\x07\x1b]4;1\x07"[..],
        &b"\x1b]7501;state=blocked:app=test\x07"[..],
        &b"\x1b]7501;?;extra\x07"[..],
        &b"\x1b]2;7501;? [c [>c\x07"[..],
        &b"\x1bPignored [c [>c 7501;?\x1b\\"[..],
        &b"\x1b[\x18c"[..],
        &b"\x1b[0 !c"[..],
    ] {
        let mut detector = detector();
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
    let mut detector = detector();
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
    let mut detector = detector();
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

#[test]
fn disabled_program_status_is_an_unknown_osc_to_detector_and_parser() {
    // The default: no reply request, no capture, no record, and the device
    // queries around it are untouched.
    let mut detector = QueryDetector::default();
    let mut parser = Parser::default();
    parser.screen_mut().capture_queries(true);
    let bytes = b"]7501;state=blocked:app=test]7501;?[c";
    assert!(detector.scan(bytes), "DA1 still requests a reply");
    parser.process(bytes);
    assert_eq!(parser.screen_mut().take_queries(), vec![TerminalQuery::Da1]);
    assert!(parser.screen().program_status().records().next().is_none());
    let mut quiet = QueryDetector::default();
    assert!(!quiet.scan(b"]7501;?"));
    assert!(!quiet.scan(b"]7501;?\\"));
}

#[test]
fn turning_program_status_off_withdraws_records_and_survives_ris() {
    let mut parser = live();
    parser.process(b"]7501;state=done:app=test");
    assert!(parser.screen().program_status().get("").is_some());
    parser.process(b"c");
    assert!(parser.screen().program_status_enabled(), "RIS keeps the host's switch");
    parser.process(b"]7501;state=done:app=test");
    parser.screen_mut().set_program_status_enabled(false);
    assert!(parser.screen().program_status().records().next().is_none());
    parser.process(b"]7501;state=blocked]7501;?");
    assert!(parser.screen().program_status().records().next().is_none());
    assert!(parser.screen_mut().take_queries().is_empty());
}

#[test]
fn colour_and_version_queries_come_in_stream_order_with_the_others() {
    // One write the way a startup probe sends it, read back in the order the
    // queries were asked, so the host can answer them in that order.
    let mut parser = live();
    parser.process(b"\x1b[c\x1b[>q\x1b]11;?\x1b\\\x1b[?996n\x1b]4;2;?\x07\x1b]4;1;?\x07\x1b[16t\x1b]10;?\x07");
    assert_eq!(
        parser.screen_mut().take_queries(),
        vec![
            TerminalQuery::Da1,
            TerminalQuery::Xtversion,
            TerminalQuery::BackgroundColour,
            TerminalQuery::ColourScheme,
            TerminalQuery::PaletteColour { index: 2, own: None },
            TerminalQuery::PaletteColour { index: 1, own: None },
            TerminalQuery::WindowReport(16),
            TerminalQuery::ForegroundColour,
        ]
    );
}

fn palette(index: u8, own: Option<(u8, u8, u8)>) -> TerminalQuery {
    TerminalQuery::PaletteColour { index, own }
}

#[test]
fn a_palette_query_leaves_the_palette_alone_and_carries_its_entry() {
    let mut parser = live();
    parser.process(b"\x1b]4;1;rgb:12/34/56\x07\x1b]4;1;?\x07");
    assert_eq!(parser.screen_mut().take_queries(), vec![palette(1, Some((0x12, 0x34, 0x56)))]);
    assert_eq!(parser.screen().palette_entry(1), Some((0x12, 0x34, 0x56)));
}

#[test]
fn every_query_pair_of_one_osc_4_is_taken_in_order() {
    // tmux's input_osc_4 reads the pairs in order and answers each `?`
    // (input.c:2935-2958 in tmux 3.8).
    let mut parser = live();
    parser.process(b"\x1b]4;1;?;2;?\x1b\\");
    assert_eq!(parser.screen_mut().take_queries(), vec![palette(1, None), palette(2, None)]);
    // A set and a query mixed: the query reports the set before it.
    parser.process(b"\x1b]4;3;rgb:ff/00/00;3;?;5;?\x07");
    assert_eq!(
        parser.screen_mut().take_queries(),
        vec![palette(3, Some((0xff, 0, 0))), palette(5, None)]
    );
    // Past the sixteen colours a pair is skipped and the rest still read; a
    // bad index ends the OSC, as in tmux.
    parser.process(b"\x1b]4;16;?;2;?;x;?;6;?\x07");
    assert_eq!(parser.screen_mut().take_queries(), vec![palette(2, None)]);
}

#[test]
fn a_palette_query_reports_the_colour_at_that_point_of_the_stream() {
    // Set red, ask, set blue, ask, all in one batch: red then blue, as tmux
    // answers each query when its parser meets it, not the colour the batch
    // ends with.
    let mut parser = live();
    parser.process(b"\x1b]4;1;rgb:ff/00/00\x07\x1b]4;1;?\x07\x1b]4;1;rgb:00/00/ff\x07\x1b]4;1;?\x07");
    assert_eq!(
        parser.screen_mut().take_queries(),
        vec![palette(1, Some((0xff, 0, 0))), palette(1, Some((0, 0, 0xff)))]
    );
}

#[test]
fn the_detector_releases_for_a_query_pair_anywhere_in_an_osc_4() {
    for bytes in [
        &b"\x1b]4;1;?;2;?\x07"[..],
        &b"\x1b]4;1;rgb:ff/00/00;2;?\x07"[..],
        &b"\x1b]4;16;?;2;?\x07"[..],
    ] {
        assert!(detector().scan(bytes), "{bytes:?}");
    }
}

#[test]
fn a_flood_of_queries_is_captured_up_to_the_bound_until_the_next_drain() {
    // Three bytes a query: a 64 KB read of DA1 alone would otherwise ask the
    // host for more than 20000 replies, and colour queries asked for as many.
    let mut parser = live();
    let mut flood = b"\x1b[c".repeat(MAX_PENDING_QUERIES);
    flood.extend_from_slice(&b"\x1b]11;?\x07\x1b[>q".repeat(50));
    parser.process(&flood);
    let queries = parser.screen_mut().take_queries();
    assert_eq!(queries.len(), MAX_PENDING_QUERIES);
    assert!(queries.iter().all(|q| *q == TerminalQuery::Da1), "the first ones asked are the ones kept");
    // The bound is per drain: the next batch is answered again.
    parser.process(b"\x1b[>q\x1b]11;?\x07");
    assert_eq!(
        parser.screen_mut().take_queries(),
        vec![TerminalQuery::Xtversion, TerminalQuery::BackgroundColour]
    );
}

#[test]
fn the_bound_leaves_the_screen_and_the_release_request_alone() {
    // Past the bound a query is only not answered: the text after it is still
    // drawn, and the synchronized update gate still opens for it.
    let mut parser = live();
    let mut detector = detector();
    let mut bytes = b"\x1b[5n".repeat(MAX_PENDING_QUERIES + 10);
    bytes.extend_from_slice(b"after");
    assert!(detector.scan(&bytes));
    parser.process(&bytes);
    assert_eq!(parser.screen_mut().take_queries().len(), MAX_PENDING_QUERIES);
    assert_eq!(parser.screen().contents(), "after");
}
