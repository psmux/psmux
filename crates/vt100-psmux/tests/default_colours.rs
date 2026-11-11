// The default colours: `OSC 10 ; colour` and `OSC 11 ; colour` set the
// pane's own foreground and background, `OSC 110` and `OSC 111` take them
// back, and `OSC 10 ; ?` and `OSC 11 ; ?` ask for them (input_osc_10,
// input_osc_110, input_osc_11 and input_osc_111, input.c:3070-3160 at tag
// 3.8). A query carries the colour as it stood, the styles' and the
// program's in tmux's order (tty_default_colours, tty.c:3164), and none
// leaves the reply to the outer terminal's. Every expectation is what tmux
// 3.8 answered for the same bytes.

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

fn fg(colour: Option<(u8, u8, u8)>, bel: bool) -> vt100::TerminalQuery {
    vt100::TerminalQuery::ForegroundColour { colour, bel }
}

fn bg(colour: Option<(u8, u8, u8)>, bel: bool) -> vt100::TerminalQuery {
    vt100::TerminalQuery::BackgroundColour { colour, bel }
}

const RED: (u8, u8, u8) = (0xff, 0, 0);
const GREEN: (u8, u8, u8) = (0, 0xff, 0);

#[test]
fn nothing_set_is_asked_with_no_colour() {
    let mut p = parse("\x1b]11;?\x1b\\\x1b]10;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(None, false), fg(None, false)]);
    assert_eq!(p.screen().program_default_colours(), (None, None));
}

#[test]
fn a_colour_set_is_what_the_query_after_it_reports() {
    for (spec, rgb) in [
        ("#ff0000", RED),
        ("rgb:12/34/56", (0x12, 0x34, 0x56)),
        ("12,34,56", (12, 34, 56)),
    ] {
        let mut p = parse(&format!("\x1b]11;{spec}\x1b\\\x1b]11;?\x1b\\"));
        assert_eq!(queries(&mut p), vec![bg(Some(rgb), false)], "{spec}");
        assert_eq!(p.screen().program_default_colours(), (None, Some(rgb)), "{spec}");
    }
    let mut p = parse("\x1b]10;#00ff00\x07\x1b]10;?\x07");
    assert_eq!(queries(&mut p), vec![fg(Some(GREEN), true)]);
    assert_eq!(p.screen().program_default_colours(), (Some(GREEN), None));
}

#[test]
fn osc_110_and_111_take_them_back_only_with_nothing_after_them() {
    // Measured: `111;x` is ignored, `110;` resets.
    let mut p = parse("\x1b]11;#ff0000\x1b\\\x1b]111;x\x1b\\\x1b]11;?\x1b\\\x1b]111\x1b\\\x1b]11;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(Some(RED), false), bg(None, false)]);
    let mut p = parse("\x1b]10;#ff0000\x1b\\\x1b]110;\x1b\\\x1b]10;?\x1b\\");
    assert_eq!(queries(&mut p), vec![fg(None, false)]);
}

#[test]
fn an_unreadable_colour_changes_nothing() {
    // Measured: red;blue, colour1 and bad are ignored. psmux has no table of
    // X11 names, so red is ignored too where tmux takes it.
    for spec in ["red;blue", "colour1", "bad", ""] {
        let mut p = parse(&format!("\x1b]11;#010203\x1b\\\x1b]11;{spec}\x1b\\\x1b]11;?\x1b\\"));
        assert_eq!(queries(&mut p), vec![bg(Some((1, 2, 3)), false)], "{spec:?}");
    }
}

#[test]
fn ris_and_a_bare_osc_104_take_them_back_and_the_alternate_screen_keeps_them() {
    // tmux's RIS and OSC 104 clear the palette they live in (input.c:1408,
    // 3454); measured, the next style change shows them gone.
    for reset in ["\x1bc", "\x1b]104\x1b\\", "\x1b]104;\x1b\\"] {
        let p = parse(&format!("\x1b]11;#ff0000\x1b\\\x1b]10;#ff0000\x1b\\{reset}"));
        assert_eq!(p.screen().program_default_colours(), (None, None), "{reset:?}");
    }
    // Measured: OSC 104 with an index keeps them.
    let p = parse("\x1b]11;#ff0000\x1b\\\x1b]104;1\x1b\\");
    assert_eq!(p.screen().program_default_colours(), (None, Some(RED)));
    let mut p = parse("\x1b]11;#ff0000\x1b\\\x1b[?1049h\x1b]11;?\x1b\\\x1b[?1049l\x1b]11;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(Some(RED), false), bg(Some(RED), false)]);
}

#[test]
fn the_styles_go_over_and_under_the_program_colour() {
    let blue = (0, 0, 0x80);
    let green = (0, 0x80, 0);
    let mut p = parse("");
    p.screen_mut().set_style_colours(
        vt100::StyleColour::default(),
        vt100::StyleColour { over: None, under: Some(green) },
    );
    p.process(b"\x1b]11;?\x1b\\\x1b]11;#ff0000\x1b\\\x1b]11;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(Some(green), false), bg(Some(RED), false)]);
    p.screen_mut().set_style_colours(
        vt100::StyleColour::default(),
        vt100::StyleColour { over: Some(blue), under: Some(green) },
    );
    p.process(b"\x1b]11;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(Some(blue), false)]);
    // RIS drops the program's colour, not the styles'.
    p.process(b"\x1bc");
    p.screen_mut().set_style_colours(
        vt100::StyleColour::default(),
        vt100::StyleColour { over: None, under: Some(green) },
    );
    assert_eq!(p.screen().default_background(), Some(green));
}

#[test]
fn a_host_that_drops_the_resets_has_the_program_colours_ignored() {
    let mut p = parse("\x1b]11;#ff0000\x1b\\");
    p.screen_mut().set_program_default_colours_enabled(false);
    assert_eq!(p.screen().program_default_colours(), (None, None));
    p.process(b"\x1b]11;#00ff00\x1b\\\x1b]10;#00ff00\x1b\\\x1b]11;?\x1b\\");
    assert_eq!(queries(&mut p), vec![bg(None, false)]);
    p.screen_mut().set_program_default_colours_enabled(true);
    p.process(b"\x1b]11;#00ff00\x1b\\");
    assert_eq!(p.screen().program_default_colours(), (None, Some(GREEN)));
}

#[test]
fn a_respawn_and_a_new_screen_keep_them() {
    let mut p = parse("\x1b]11;#ff0000\x1b\\");
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().program_default_colours(), (None, Some(RED)));
    let mut fresh = vt100::Parser::new(10, 40, 0);
    fresh.screen_mut().keep_colours(p.screen());
    assert_eq!(fresh.screen().program_default_colours(), (None, Some(RED)));
}

#[test]
fn a_query_split_at_every_byte() {
    let bytes = b"\x1b]11;#ff0000\x1b\\\x1b]11;?\x1b\\";
    let mut p = vt100::Parser::new(10, 40, 0);
    p.screen_mut().capture_queries(true);
    for b in bytes {
        p.process(std::slice::from_ref(b));
    }
    assert_eq!(queries(&mut p), vec![bg(Some(RED), false)]);
}
