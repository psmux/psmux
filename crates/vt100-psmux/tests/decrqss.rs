// DECRQSS, `DCS $ q Pt ST`, and the cursor style it reports.
//
// tmux answers `Pt` of `SP q` with the DECSCUSR style the program set, its
// shape with the blink of `CSI ? 12 h`, and with the `cursor-style` option
// when it set none (input_handle_decrqss, input.c:2554-2617 at tag 3.8). Any
// other setting gets `DCS 0 $ r ST`. Each is answered at the ST, in order with
// the other queries. psmux dropped every DCS but sixel, and kept no cursor
// style, so neither reached the host.

use vt100_psmux as vt100;
use vt100::{Parser, QueryDetector, TerminalQuery};

fn live() -> Parser {
    let mut parser = Parser::new(24, 80, 0);
    parser.screen_mut().capture_queries(true);
    parser
}

fn queries(parser: &mut Parser, bytes: &[u8]) -> Vec<TerminalQuery> {
    parser.process(bytes);
    parser.screen_mut().take_queries()
}

fn style(style: u8) -> TerminalQuery {
    TerminalQuery::CursorStyle { style }
}

const ASK: &[u8] = b"\x1bP$q q\x1b\\";

fn asked_after(setup: &[u8]) -> Vec<TerminalQuery> {
    let mut p = live();
    queries(&mut p, &[setup, ASK].concat())
}

#[test]
fn the_style_the_program_set() {
    // The cases measured on tmux 3.8, where the reply carried these numbers.
    let cases: &[(&[u8], u8)] = &[
        (b"", 0),
        (b"\x1b[1 q", 1),
        (b"\x1b[2 q", 2),
        (b"\x1b[3 q", 3),
        (b"\x1b[4 q", 4),
        (b"\x1b[5 q", 5),
        (b"\x1b[6 q", 6),
        (b"\x1b[4 q\x1b[0 q", 0),
        (b"\x1b[4 q\x1b[ q", 0),
        // Past 6 changes nothing (screen_set_cursor_style, screen.c:221).
        (b"\x1b[4 q\x1b[7 q", 4),
        // A colon makes input_get give -1, which DECSCUSR ignores.
        (b"\x1b[4 q\x1b[1:2 q", 4),
        // The blink of mode 12 counts only with a shape.
        (b"\x1b[?12h", 0),
        (b"\x1b[2 q\x1b[?12h", 1),
        (b"\x1b[1 q\x1b[?12l", 2),
        (b"\x1b[6 q\x1b[?12h", 5),
        (b"\x1b[?12h\x1b[2 q\x1b[0 q", 0),
        // RIS keeps the shape and drops the blink, a mode.
        (b"\x1b[1 q\x1bc", 2),
        (b"\x1b[5 q\x1bc", 6),
        // The alternate screen shares it.
        (b"\x1b[4 q\x1b[?1049h", 4),
        (b"\x1b[?1049h\x1b[3 q\x1b[?1049l", 3),
    ];
    for &(setup, want) in cases {
        assert_eq!(asked_after(setup), vec![style(want)], "{setup:?}");
    }
}

#[test]
fn the_screen_reports_it_too() {
    let mut p = Parser::new(24, 80, 0);
    assert_eq!(p.screen().cursor_style(), 0);
    p.process(b"\x1b[3 q");
    assert_eq!(p.screen().cursor_style(), 3);
    p.process(b"\x1b[?12l");
    assert_eq!(p.screen().cursor_style(), 4);
}

#[test]
fn what_else_follows_and_what_comes_before_change_nothing() {
    // input_handle_decrqss reads only the first two bytes of Pt, and the
    // parameter before the `$` is not read at all.
    assert_eq!(asked_after(b"\x1b[6 q\x1bP$q qx\x1b\\"), vec![style(6), style(6)]);
    let mut p = live();
    assert_eq!(queries(&mut p, b"\x1bP1$q q\x1b\\"), vec![style(0)]);
}

#[test]
fn any_other_setting_is_unknown() {
    let mut p = live();
    for q in [&b"\x1bP$qm\x1b\\"[..], b"\x1bP$qr\x1b\\", b"\x1bP$q\"q\x1b\\", b"\x1bP$q\x1b\\", b"\x1bP$qq\x1b\\", b"\x1bP$q  q\x1b\\"] {
        assert_eq!(queries(&mut p, q), vec![TerminalQuery::UnknownSetting], "{q:?}");
    }
}

#[test]
fn other_dcs_strings_ask_nothing() {
    let mut p = live();
    for q in [&b"\x1bP$p q\x1b\\"[..], b"\x1bPq q\x1b\\", b"\x1bP$$q q\x1b\\", b"\x1bP+q q\x1b\\"] {
        assert_eq!(queries(&mut p, q), vec![], "{q:?}");
    }
}

#[test]
fn the_string_goes_on_to_the_st() {
    // tmux reads a CAN, a SUB, an ESC and what follows it as part of the
    // string (input.c:683-702), answers at the ST, and answers nothing the
    // string holds. Measured on tmux 3.8, each case alone.
    let cases: &[(&[u8], Vec<TerminalQuery>)] = &[
        (b"\x1bP$q q\x18\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1a\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x18abc\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1b[m\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1b[c\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1b]0;x\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1b]11;?\x07\x1b\\", vec![style(0)]),
        (b"\x1bP$q q\x1bP$qm\x1b\\", vec![style(0)]),
        // The CAN cuts `Pt` short of `SP q`.
        (b"\x1bP$q \x18q\x1b\\", vec![TerminalQuery::UnknownSetting]),
        // Before the final byte it ends the DCS, as anywhere in tmux.
        (b"\x1bP$\x18q q\x1b\\", vec![]),
        // No ST, no reply, and the DA1 is part of the string.
        (b"\x1bP$q q\x1b[c", vec![]),
    ];
    for (bytes, want) in cases {
        let mut p = live();
        assert_eq!(&queries(&mut p, bytes), want, "{bytes:?}");
    }
}

#[test]
fn the_queries_after_the_st_are_answered() {
    let mut p = live();
    assert_eq!(
        queries(&mut p, b"\x1bP$q q\x1b[c\x1b\\\x1b[c"),
        vec![style(0), TerminalQuery::Da1]
    );
}

#[test]
fn in_order_with_the_other_queries() {
    assert_eq!(
        asked_after(b"\x1b[5 q\x1bP$q q\x1b\\\x1b[c\x1b[3 q"),
        vec![style(5), TerminalQuery::Da1, style(3)]
    );
}

#[test]
fn split_anywhere() {
    let bytes: &[u8] = b"\x1b[5 q\x1bP$q q\x1b\\\x1b[?12l\x1bP$qm\x1b\\\x1bP$q q\x1b\\";
    let want = vec![style(5), TerminalQuery::UnknownSetting, style(6)];
    for cut in 0..=bytes.len() {
        let mut p = live();
        let mut got = queries(&mut p, &bytes[..cut]);
        got.extend(queries(&mut p, &bytes[cut..]));
        assert_eq!(got, want, "cut at {cut}");
        let mut d = QueryDetector::default();
        let (a, b) = (d.scan(&bytes[..cut]), d.scan(&bytes[cut..]));
        assert!(a || b, "detector, cut at {cut}");
    }
}

#[test]
fn a_string_as_long_as_the_input_buffer_is_dropped() {
    // tmux discards a string that outgrows input-buffer-size (INPUT_DISCARD,
    // input.c:2641), here at its default and minimum.
    let size = vt100::DEFAULT_INPUT_BUFFER_SIZE;
    let ask = |len: usize| {
        let mut s = b"\x1bP$q q".to_vec();
        s.resize(s.len() + len - 2, b'x');
        s.extend_from_slice(b"\x1b\\");
        s
    };
    let mut p = live();
    assert_eq!(queries(&mut p, &ask(size - 1)), vec![style(0)]);
    assert_eq!(queries(&mut p, &ask(size)), vec![]);
}

#[test]
fn the_detector_sees_the_query() {
    let mut d = QueryDetector::default();
    assert!(d.scan(ASK));
    assert!(d.scan(b"\x1bP$qm\x1b\\"));
    assert!(!d.scan(b"\x1bP$p q\x1b\\"));
    assert!(!d.scan(b"\x1b[4 q"));
    // Open until the ST, which holds back the DA1 inside it.
    assert!(!d.scan(b"\x1bP$q q\x18\x1b[c"));
    assert!(d.scan(b"\x1b\\"));
    assert!(d.scan(b"\x1b[c"));
}

#[test]
fn a_respawn_keeps_the_shape_and_not_the_blink() {
    // screen_reinit resets the modes and keeps s->cstyle (screen.c:117).
    let mut p = Parser::new(24, 80, 0);
    p.process(b"\x1b[3 q");
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().cursor_style(), 4);
}

#[test]
fn clear_history_keeps_it() {
    // tmux's clear-history frees only the history and keeps the style.
    let mut p = Parser::new(24, 80, 0);
    p.process(b"\x1b[2 q\x1b[?12h");
    p.screen_mut().clear_history();
    assert_eq!(p.screen().cursor_style(), 1);
}
