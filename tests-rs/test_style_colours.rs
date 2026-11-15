// What window-style and window-active-style give a pane's default colours,
// for the OSC 10 and OSC 11 replies and for drawing. Every expectation is
// what tmux 3.8 did, measured with `OSC 11 ; ?` from a pane (tty.c:3164 and
// window.c:2699 at tag 3.8).

use super::*;

fn colour(over: Option<(u8, u8, u8)>, under: Option<(u8, u8, u8)>) -> vt100::StyleColour {
    vt100::StyleColour { over, under }
}

const GREEN: (u8, u8, u8) = (0, 0x80, 0);
const BLUE: (u8, u8, u8) = (0, 0, 0x80);

#[test]
fn a_style_names_its_colours_as_their_xterm_values() {
    // Measured: window-style bg=green answers rgb:0000/8080/0000, fg=colour196
    // rgb:ffff/0000/0000, bg=#123456 rgb:1212/3434/5656, fg=brightred
    // rgb:ffff/0000/0000.
    assert_eq!(Colours::of("bg=green"), Colours { fg: None, bg: Some(GREEN) });
    assert_eq!(Colours::of("fg=colour196").fg, Some((0xff, 0, 0)));
    assert_eq!(Colours::of("bg=#123456,fg=brightred"), Colours { fg: Some((0xff, 0, 0)), bg: Some((0x12, 0x34, 0x56)) });
    // Measured: bg=default leaves the program's colour to answer.
    assert_eq!(Colours::of("bg=default"), Colours::default());
    assert_eq!(Colours::of(""), Colours::default());
    assert_eq!(Colours::of("dim=50"), Colours::default());
}

#[test]
fn the_active_pane_has_the_active_style_over_the_program_and_the_style_under() {
    // Measured on the active pane: window-active-style bg=blue answers blue
    // before and after OSC 11 ; #ff0000; window-style bg=green alone answers
    // green, and red once the program set it.
    let style = Colours { fg: None, bg: Some(GREEN) };
    let active = Colours { fg: None, bg: Some(BLUE) };
    assert_eq!(for_pane(style, active, true).1, colour(Some(BLUE), Some(GREEN)));
    assert_eq!(for_pane(style, Colours::default(), true).1, colour(None, Some(GREEN)));
}

#[test]
fn any_other_pane_has_the_style_over_the_program() {
    // Measured on an inactive pane: window-style bg=green answers green before
    // and after OSC 11 ; #ff0000, window-active-style plays no part, and with
    // neither style the program's red answers.
    let style = Colours { fg: None, bg: Some(GREEN) };
    let active = Colours { fg: None, bg: Some(BLUE) };
    assert_eq!(for_pane(style, active, false).1, colour(Some(GREEN), None));
    assert_eq!(for_pane(Colours::default(), active, false).1, colour(None, None));
}

#[test]
fn a_screen_answers_in_that_order() {
    let mut parser = vt100::Parser::new(5, 20, 0);
    let screen = parser.screen_mut();
    let style = Colours { fg: None, bg: Some(GREEN) };
    let (fg, bg) = for_pane(style, Colours::default(), false);
    screen.set_style_colours(fg, bg);
    assert_eq!(parser.screen().default_background(), Some(GREEN));
    parser.process(b"\x1b]11;#ff0000\x1b\\");
    assert_eq!(parser.screen().default_background(), Some(GREEN));
    let (fg, bg) = for_pane(style, Colours::default(), true);
    parser.screen_mut().set_style_colours(fg, bg);
    assert_eq!(parser.screen().default_background(), Some((0xff, 0, 0)));
    assert_eq!(parser.screen().default_foreground(), None);
}
