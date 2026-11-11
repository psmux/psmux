// The cursor colour: `OSC 12 ; colour` sets it, `OSC 112` takes it back, and
// `OSC 12 ; ?` asks for it (input_osc_12 and input_osc_112, input.c:3164 and
// 3189 at tag 3.8). A query is answered with the program's colour, then the
// default the pane's `cursor-colour` option gives, and not at all with
// neither. Every expectation is what tmux 3.8 answered for the same bytes.

use vt100_psmux as vt100;

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(10, 40, 0);
    parser.screen_mut().capture_queries(true);
    parser.process(bytes.as_bytes());
    parser
}

fn queries(parser: &mut vt100::Parser) -> Vec<vt100::TerminalQuery> {
    parser.screen_mut().take_queries()
}

fn asked(colour: Option<(u8, u8, u8)>, bel: bool) -> vt100::TerminalQuery {
    vt100::TerminalQuery::CursorColour { colour, bel }
}

#[test]
fn nothing_set_is_asked_with_no_colour() {
    let mut p = parse("\x1b]12;?\x1b\\");
    assert_eq!(queries(&mut p), vec![asked(None, false)]);
    assert_eq!(p.screen().cursor_colour(), None);
}

#[test]
fn a_colour_set_is_what_the_query_after_it_reports() {
    for (spec, rgb) in [
        ("#ff0000", (0xff, 0, 0)),
        ("rgb:12/34/56", (0x12, 0x34, 0x56)),
        ("rgb:ffff/0000/8080", (0xff, 0, 0x80)),
        ("12,34,56", (12, 34, 56)),
    ] {
        let mut p = parse(&format!("\x1b]12;{spec}\x1b\\\x1b]12;?\x1b\\"));
        assert_eq!(queries(&mut p), vec![asked(Some(rgb), false)], "{spec}");
        assert_eq!(p.screen().cursor_colour(), Some(rgb), "{spec}");
        assert_eq!(p.screen().program_cursor_colour(), Some(rgb), "{spec}");
    }
}

#[test]
fn a_query_ended_in_bel_says_so() {
    let mut p = parse("\x1b]12;#00ff00\x07\x1b]12;?\x07");
    assert_eq!(queries(&mut p), vec![asked(Some((0, 0xff, 0)), true)]);
}

#[test]
fn the_colour_at_each_query_is_the_one_then() {
    let mut p = parse("\x1b]12;?\x1b\\\x1b]12;#010203\x1b\\\x1b]12;?\x1b\\\x1b]112\x1b\\\x1b]12;?\x1b\\");
    assert_eq!(
        queries(&mut p),
        vec![asked(None, false), asked(Some((1, 2, 3)), false), asked(None, false)]
    );
}

#[test]
fn osc_112_takes_it_back_only_with_nothing_after_it() {
    let p = parse("\x1b]12;#ff0000\x1b\\\x1b]112\x1b\\");
    assert_eq!(p.screen().cursor_colour(), None);
    let p = parse("\x1b]12;#ff0000\x1b\\\x1b]112;\x1b\\");
    assert_eq!(p.screen().cursor_colour(), None);
    let p = parse("\x1b]12;#ff0000\x1b\\\x1b]112;x\x1b\\");
    assert_eq!(p.screen().cursor_colour(), Some((0xff, 0, 0)));
}

#[test]
fn an_unreadable_colour_changes_nothing() {
    for spec in ["notacolour", "#ff00", "rgb:12/34", "red;blue"] {
        let p = parse(&format!("\x1b]12;#010203\x1b\\\x1b]12;{spec}\x1b\\"));
        assert_eq!(p.screen().cursor_colour(), Some((1, 2, 3)), "{spec}");
    }
}

#[test]
fn ris_and_the_alternate_screen_keep_it() {
    let p = parse("\x1b]12;#ff0000\x1b\\\x1bc");
    assert_eq!(p.screen().cursor_colour(), Some((0xff, 0, 0)));
    let p = parse("\x1b]12;#ff0000\x1b\\\x1b[?1049h");
    assert_eq!(p.screen().cursor_colour(), Some((0xff, 0, 0)));
    let mut p = parse("\x1b]12;#ff0000\x1b\\\x1b[?1049h\x1b[?1049l");
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().cursor_colour(), Some((0xff, 0, 0)));
}

#[test]
fn the_default_is_under_the_program_colour() {
    let mut p = vt100::Parser::new(10, 40, 0);
    p.screen_mut().capture_queries(true);
    assert!(p.screen_mut().set_default_cursor_colour(Some((0, 0, 0x80))));
    p.process(b"\x1b]12;?\x1b\\");
    assert_eq!(queries(&mut p), vec![asked(Some((0, 0, 0x80)), false)]);
    p.process(b"\x1b]12;#ff0000\x1b\\\x1b]12;?\x1b\\");
    assert_eq!(queries(&mut p), vec![asked(Some((0xff, 0, 0)), false)]);
    assert!(!p.screen_mut().set_default_cursor_colour(Some((0, 0x80, 0))), "the program's colour still wins");
    p.process(b"\x1b]112\x1b\\\x1b]12;?\x1b\\");
    assert_eq!(queries(&mut p), vec![asked(Some((0, 0x80, 0)), false)]);
    assert_eq!(p.screen().program_cursor_colour(), None);
    p.process(b"\x1bc");
    assert_eq!(p.screen().cursor_colour(), Some((0, 0x80, 0)), "RIS keeps the default");
}

#[test]
fn a_host_that_drops_the_reset_has_the_program_colour_ignored() {
    let mut p = vt100::Parser::new(10, 40, 0);
    p.screen_mut().capture_queries(true);
    p.screen_mut().set_program_cursor_colour_enabled(false);
    p.screen_mut().set_default_cursor_colour(Some((0, 0, 0x80)));
    p.process(b"\x1b]12;#ff0000\x1b\\\x1b]12;?\x1b\\");
    assert_eq!(p.screen().program_cursor_colour(), None);
    assert_eq!(queries(&mut p), vec![asked(Some((0, 0, 0x80)), false)]);
}

#[test]
fn a_query_split_at_every_byte() {
    let bytes = b"\x1b]12;#ff0000\x1b\\\x1b]12;?\x07";
    for cut in 1..bytes.len() {
        let mut p = vt100::Parser::new(10, 40, 0);
        p.screen_mut().capture_queries(true);
        p.process(&bytes[..cut]);
        p.process(&bytes[cut..]);
        assert_eq!(queries(&mut p), vec![asked(Some((0xff, 0, 0)), true)], "cut at {cut}");
    }
}

#[test]
fn the_detector_sees_the_query() {
    let mut d = vt100::QueryDetector::default();
    assert!(d.scan(b"\x1b]12;?\x1b\\"));
    assert!(!d.scan(b"\x1b]12;#ff0000\x1b\\"));
}

#[test]
fn a_new_screen_can_keep_both_colours() {
    // clear-history starts the screen again; tmux keeps both colours there.
    let mut old = parse("\x1b]12;#010203\x1b\\");
    old.screen_mut().set_default_cursor_colour(Some((4, 5, 6)));
    let mut fresh = vt100::Parser::new(10, 40, 0);
    fresh.screen_mut().keep_colours(old.screen());
    assert_eq!(fresh.screen().program_cursor_colour(), Some((1, 2, 3)));
    fresh.process(b"\x1b]112\x1b\\");
    assert_eq!(fresh.screen().cursor_colour(), Some((4, 5, 6)));
}
