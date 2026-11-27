// DECRQM (`CSI ? Ps $ p`, `CSI Ps $ p`) answered with each mode as it stood
// where the query was met, as tmux answers it (INPUT_CSI_QUERY_PRIVATE,
// input.c:1651-1722 at tag 3.8). The expected values are tmux 3.8's,
// measured; the console host streams are OpenConsole 1.25's and the inbox
// conhost's on build 26300, captured with PSMUX_PANE_RAW.

use vt100_psmux::{Parser, TerminalQuery};

fn parser() -> Parser {
    let mut parser = Parser::default();
    parser.screen_mut().capture_queries(true);
    parser
}

/// The value each DECRQM carries, in order, for output read in the batches
/// given. Other queries are left out.
fn answers_in(batches: &[&[u8]]) -> Vec<Option<u8>> {
    let mut parser = parser();
    for batch in batches {
        parser.process(batch);
    }
    parser
        .screen_mut()
        .take_queries()
        .into_iter()
        .filter_map(|q| match q {
            TerminalQuery::DecrqmPrivate { value, .. } => Some(value),
            TerminalQuery::DecrqmAnsi { value, .. } => Some(Some(value)),
            _ => None,
        })
        .collect()
}

fn answers(bytes: &[u8]) -> Vec<Option<u8>> {
    answers_in(&[bytes])
}

#[test]
fn a_mode_is_answered_as_it_stood_at_the_query() {
    // Set, asked and reset in one write: tmux answers the set mode. The
    // answer used to be taken from the screen after the whole batch.
    for (mode, ansi) in [(1, false), (25, false), (7, false), (2004, false), (1000, false), (1049, false), (4, true)] {
        let q = if ansi { "" } else { "?" };
        let (first, second) = if mode == 25 || mode == 7 { ("l", "h") } else { ("h", "l") };
        let bytes = format!("\x1b[{q}{mode}{first}\x1b[{q}{mode}$p\x1b[{q}{mode}{second}");
        let want = if first == "h" { 1 } else { 2 };
        assert_eq!(answers(bytes.as_bytes()), vec![Some(want)], "{bytes:?}");
    }
    assert_eq!(answers(b"\x1b[?1$p\x1b[?1h\x1b[?1$p"), vec![Some(2), Some(1)]);
}

#[test]
fn origin_mode_6() {
    assert_eq!(answers(b"\x1b[?6$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?6h\x1b[?6$p"), vec![Some(1)]);
    assert_eq!(answers(b"\x1b[?6h\x1b[?6l\x1b[?6$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?6h\x1bc\x1b[?6$p"), vec![Some(2)]);
}

#[test]
fn cursor_blink_12() {
    // None: the program set neither a shape nor a blink, so the host answers
    // from its cursor-style option.
    for (bytes, want) in [
        (&b""[..], None),
        (b"\x1b[?12h", Some(1)),
        (b"\x1b[?12l", Some(2)),
        (b"\x1b[1 q", Some(1)),
        (b"\x1b[2 q", Some(2)),
        (b"\x1b[6 q\x1b[?12h", Some(1)),
        // DECSCUSR 0 forgets the blink a `?12` set (input.c:1880-1883).
        (b"\x1b[?12h\x1b[0 q", None),
        (b"\x1b[?12l\x1b[0 q", None),
        (b"\x1b[2 q\x1b[0 q", None),
        // An RIS keeps the shape and drops the blink, a mode.
        (b"\x1b[5 q\x1bc", Some(2)),
        (b"\x1b[?12l\x1bc", None),
        (b"\x1b[?12h\x1b[?1049h", Some(1)),
        // DECSCUSR 7 is ignored.
        (b"\x1b[?12h\x1b[7 q", Some(1)),
    ] {
        let bytes = [bytes, b"\x1b[?12$p"].concat();
        assert_eq!(answers(&bytes), vec![want], "{bytes:?}");
    }
}

#[test]
fn the_blink_does_not_change_the_decrqss_style_without_a_shape() {
    let mut parser = parser();
    parser.process(b"\x1b[?12h");
    assert_eq!(parser.screen().cursor_style(), 0);
    parser.process(b"\x1b[3 q\x1b[?12l");
    assert_eq!(parser.screen().cursor_style(), 4);
}

#[test]
fn focus_reporting_1004_from_a_program() {
    assert_eq!(answers(b"\x1b[?1004$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?1004h\x1b[?1004$p"), vec![Some(1)]);
    assert_eq!(answers(b"\x1b[?1004h\x1b[?1004l\x1b[?1004$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?1004h\x1bc\x1b[?1004$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?1000;1004h\x1b[?1004$p"), vec![Some(1)]);
}

#[test]
fn the_console_hosts_own_1004_does_not_count() {
    for (stream, want) in [
        // A pane starting: OpenConsole 1.25, then the inbox conhost.
        (&b"\x1b[1t\x1b[c\x1b[?1004h\x1b[?9001h\x1b[?1004$p"[..], vec![Some(2)]),
        (b"\x1b[?9001h\x1b[?1004h\x1b[?25l\x1b[2J\x1b[?1004$p", vec![Some(2)]),
        // What OpenConsole passed on for a program's writes: its own
        // `?1004h` right after each `?1004l`, and with `?9001h` after an RIS.
        (b"\x1b[?1004h\x1b[?1004$p\x1b[?1004l\x1b[?1004h\x1b[?1004$p", vec![Some(1), Some(2)]),
        (b"\x1b[?1004h\x1b[?1004l\x1b[?1004h\x1b[?1004$p", vec![Some(2)]),
        (b"\x1b[?1004h\x1bc\x1b[?1004h\x1b[?9001h\x1b[?1004$p", vec![Some(2)]),
        (b"\x1b[?1004l\x1b[?1004h\x1b[?1004h\x1b[?1004$p", vec![Some(1)]),
        (b"\x1b[?1004h\x1b[?1004h\x1b[?1004$p\x1b[?1004l\x1b[?1004h", vec![Some(1)]),
        (b"\x1b[?1004h\x1b[?1004l\x1b[?1004h\x1b[?1004l\x1b[?1004h\x1b[?1004$p", vec![Some(2)]),
        (b"\x1b[?1000;1004h\x1b[?1004;1000l\x1b[?1004h\x1b[?1004$p", vec![Some(2)]),
        (b"\x1b[?1004h\x1b[?1004;1000l\x1b[?1004hxy\x1b[?1004$p", vec![Some(2)]),
        // A program's later `?1004h` comes after the host's, so it counts.
        (b"\x1b[?1004l\x1b[?1004hxy\x1b[?1004h\x1b[?1004$p", vec![Some(1)]),
        (b"\x1b[?1004l\x1b[?1004h\x1b[?25h\x1b[?1004h\x1b[?1004$p", vec![Some(1)]),
    ] {
        assert_eq!(answers(stream), want, "{stream:?}");
    }
}

#[test]
fn the_hosts_1004_is_told_apart_across_reads() {
    assert_eq!(answers_in(&[b"\x1b[?1004h\x1b[?1004l", b"\x1b[?1004h", b"\x1b[?1004$p"]), vec![Some(2)]);
    assert_eq!(answers_in(&[b"\x1b[?1004h", b"\x1b[?9001h\x1b[?1004$p"]), vec![Some(2)]);
    assert_eq!(answers_in(&[b"\x1b[?9001", b"h\x1b[?1004h\x1b[?1004$p"]), vec![Some(2)]);
}

#[test]
fn a_program_writing_9001_beside_its_1004_is_taken_for_the_host() {
    // The one stream the rule reads wrongly (tmux answers 1). The host then
    // turns win32 input mode on and hands the program its reply as key
    // records, so the program cannot read the answer anyway.
    assert_eq!(answers(b"\x1b[?1004h\x1b[?9001h\x1b[?1004$p"), vec![Some(2)]);
    // Only private mode CSIs part them, so that no other CSI pays for the
    // check; the hosts write nothing else there.
    assert_eq!(answers(b"\x1b[?1004h\x1b[0m\x1b[?9001h\x1b[?1004$p"), vec![Some(2)]);
    assert_eq!(answers(b"\x1b[?1004h\x1b[?25h\x1b[?9001h\x1b[?1004$p"), vec![Some(1)]);
}

#[test]
fn focus_reporting_is_readable() {
    let mut parser = parser();
    parser.process(b"\x1b[?9001h\x1b[?1004h");
    assert!(!parser.screen().focus_reporting());
    parser.process(b"\x1b[?1004h");
    assert!(parser.screen().focus_reporting());
}

#[test]
fn theme_updates_2031_stay_unknown() {
    // psmux sends no theme change notification, so it does not claim the
    // mode tmux answers 1 or 2 for.
    assert_eq!(answers(b"\x1b[?2031$p"), vec![Some(0)]);
    assert_eq!(answers(b"\x1b[?2031h\x1b[?2031$p"), vec![Some(0)]);
}

#[test]
fn synchronized_output_2026_is_left_to_the_host() {
    assert_eq!(answers(b"\x1b[?2026h\x1b[?2026$p"), vec![None]);
}

#[test]
fn unknown_and_permanent_modes() {
    assert_eq!(answers(b"\x1b[?3$p"), vec![Some(4)]);
    assert_eq!(answers(b"\x1b[?31337$p"), vec![Some(0)]);
    assert_eq!(answers(b"\x1b[20$p"), vec![Some(0)]);
}

#[test]
fn a_respawn_resets_the_modes_and_keeps_the_shape() {
    let mut parser = parser();
    parser.process(b"\x1b[?6h\x1b[?1004h\x1b[?12h");
    parser.screen_mut().reinit_keep_history();
    parser.screen_mut().capture_queries(true);
    parser.process(b"\x1b[?6$p\x1b[?1004$p\x1b[?12$p");
    let got: Vec<_> = parser.screen_mut().take_queries();
    assert_eq!(
        got,
        vec![
            TerminalQuery::DecrqmPrivate { mode: 6, value: Some(2) },
            TerminalQuery::DecrqmPrivate { mode: 1004, value: Some(2) },
            TerminalQuery::DecrqmPrivate { mode: 12, value: None },
        ]
    );
    let mut parser = Parser::default();
    parser.process(b"\x1b[5 q");
    parser.screen_mut().reinit_keep_history();
    parser.screen_mut().capture_queries(true);
    parser.process(b"\x1b[?12$p");
    assert_eq!(parser.screen_mut().take_queries(), vec![TerminalQuery::DecrqmPrivate { mode: 12, value: Some(2) }]);
}
