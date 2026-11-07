// A palette query past the sixteen ANSI colours, and how a colour query ended.
//
// tmux's input_osc_4 answers every index from 0 to 255 from the pane's own
// palette, the OSC 4 entries over the pane-colours defaults
// (colour_palette_get, input.c:2947 at tag 3.8), and asks the outer terminal
// only when the pane has none (input_add_request, input.c:2956). It ends each
// colour reply the way the query ended, BEL or ST (input_osc_colour_reply
// with ictx->input_end). psmux took a query only for the sixteen ANSI colours,
// whose outer terminal values its client reports, and kept no terminator.

use vt100_psmux as vt100;
use vt100::{Color, ColourPalette, Parser, QueryDetector, TerminalQuery};

fn live() -> Parser {
    let mut parser = Parser::new(24, 80, 0);
    parser.screen_mut().capture_queries(true);
    parser
}

fn queries(parser: &mut Parser, bytes: &[u8]) -> Vec<TerminalQuery> {
    parser.process(bytes);
    parser.screen_mut().take_queries()
}

fn palette(index: u8, own: Option<(u8, u8, u8)>, bel: bool) -> TerminalQuery {
    TerminalQuery::PaletteColour { index, own, bel }
}

#[test]
fn an_index_past_sixteen_set_by_osc_4_is_a_query() {
    let mut p = live();
    assert_eq!(
        queries(&mut p, b"\x1b]4;20;rgb:11/22/33\x1b\\\x1b]4;20;?\x07"),
        vec![palette(20, Some((0x11, 0x22, 0x33)), true)]
    );
    assert_eq!(
        queries(&mut p, b"\x1b]4;255;#abcdef;255;?\x1b\\"),
        vec![palette(255, Some((0xab, 0xcd, 0xef)), false)]
    );
}

#[test]
fn an_index_past_sixteen_with_no_entry_is_not() {
    // tmux asks the outer terminal for it; psmux does not yet.
    let mut p = live();
    assert!(queries(&mut p, b"\x1b]4;16;?\x07\x1b]4;20;?\x1b\\\x1b]4;255;?\x07").is_empty());
    // Reset with OSC 104, it has none again.
    queries(&mut p, b"\x1b]4;20;rgb:11/22/33\x07\x1b]104;20\x07");
    assert!(queries(&mut p, b"\x1b]4;20;?\x07").is_empty());
}

#[test]
fn an_index_past_sixteen_from_pane_colours_is_a_query() {
    let mut p = live();
    let mut defaults = ColourPalette::default();
    defaults.set_colour(20, Some(Color::Rgb(0x44, 0x55, 0x66)));
    assert!(p.screen_mut().set_pane_colours(Some(defaults)));
    assert_eq!(queries(&mut p, b"\x1b]4;20;?\x07"), vec![palette(20, Some((0x44, 0x55, 0x66)), true)]);
    // An OSC 4 entry wins over it, and OSC 104 shows it again.
    assert_eq!(
        queries(&mut p, b"\x1b]4;20;rgb:11/22/33\x07\x1b]4;20;?\x07\x1b]104;20\x07\x1b]4;20;?\x07"),
        vec![palette(20, Some((0x11, 0x22, 0x33)), true), palette(20, Some((0x44, 0x55, 0x66)), true)]
    );
}

#[test]
fn the_pairs_of_one_osc_4_mix_the_low_and_high_indexes_in_order() {
    let mut p = live();
    assert_eq!(
        queries(&mut p, b"\x1b]4;20;rgb:11/22/33;1;?;20;?;21;?;2;?\x1b\\"),
        vec![palette(1, None, false), palette(20, Some((0x11, 0x22, 0x33)), false), palette(2, None, false)]
    );
}

#[test]
fn a_colour_query_keeps_how_it_ended() {
    let mut p = live();
    assert_eq!(
        queries(&mut p, b"\x1b]10;?\x07\x1b]10;?\x1b\\\x1b]11;?\x07\x1b]11;?\x1b\\\x1b]4;1;?\x07\x1b]4;1;?\x1b\\"),
        vec![
            TerminalQuery::ForegroundColour { bel: true },
            TerminalQuery::ForegroundColour { bel: false },
            TerminalQuery::BackgroundColour { bel: true },
            TerminalQuery::BackgroundColour { bel: false },
            palette(1, None, true),
            palette(1, None, false),
        ]
    );
}

#[test]
fn the_detector_releases_held_output_for_any_palette_query() {
    // It has no palette, so it cannot tell an index the pane has an entry
    // for; one it flags and the screen leaves unanswered only releases
    // synchronized output early.
    for bytes in [&b"\x1b]4;20;?\x07"[..], &b"\x1b]4;255;?\x1b\\"[..], &b"\x1b]4;20;#112233;20;?\x07"[..]] {
        assert!(QueryDetector::default().scan(bytes), "{bytes:?}");
    }
    assert!(!QueryDetector::default().scan(b"\x1b]4;20;#112233\x07\x1b]4;256;?\x07"));
}
