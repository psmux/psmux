// The cursor-colour option and the cursor colour's replies and writes. Every
// expectation is what tmux 3.8 did (options-table.c:354, options.c:1205,
// format.c:1296, input.c:2898, tty.c:770 at tag 3.8), measured where it says
// so.

use super::*;

#[test]
fn the_option_takes_empty_a_format_or_a_colour() {
    for value in ["", "blue", "#00ff00", "colour200", "default", "brightred", "3", "#{?1,red,blue}"] {
        assert_eq!(validate(value), Ok(()), "{value:?}");
    }
    // Measured: `set -w cursor-colour bad` says `invalid colour: bad`.
    assert_eq!(validate("bad"), Err("invalid colour: bad".to_string()));
    assert_eq!(validate("#12345"), Err("invalid colour: #12345".to_string()));
}

#[test]
fn a_colour_is_drawn_as_its_xterm_value() {
    // Measured: `cursor-colour blue` answers OSC 12 ; ? with rgb:0000/0000/8080.
    assert_eq!(rgb("blue"), Some((0, 0, 0x80)));
    assert_eq!(rgb("brightred"), Some((0xff, 0, 0)));
    assert_eq!(rgb("3"), Some((0x80, 0x80, 0)));
    assert_eq!(rgb("colour200"), Some(vt100::xterm_rgb(200)));
    assert_eq!(rgb("#00FF00"), Some((0, 0xff, 0)));
    for none in ["", "default", "terminal", "#{?1,red,blue}", "bad"] {
        assert_eq!(rgb(none), None, "{none:?}");
    }
}

#[test]
fn the_format_is_the_program_colour_then_the_option_by_name() {
    // Measured: #{cursor_colour} after OSC 12 ; red is #ff0000, after
    // `cursor-colour 3` is yellow, and with neither is none.
    assert_eq!(format_value(Some((0xff, 0, 0)), "blue"), "#ff0000");
    assert_eq!(format_value(None, "blue"), "blue");
    assert_eq!(format_value(None, "3"), "yellow");
    assert_eq!(format_value(None, "#00FF00"), "#00ff00");
    assert_eq!(format_value(None, "colour200"), "colour200");
    assert_eq!(format_value(None, "default"), "default");
    assert_eq!(format_value(None, ""), "none");
}

#[test]
fn the_reply_is_tmux_form_ended_as_asked() {
    // Measured: OSC 12 ; #00ff00 BEL then OSC 12 ; ? BEL answers
    // ESC ] 12 ; rgb:0000/ffff/0000 BEL, and the ST form answers with ST.
    assert_eq!(crate::pane::cursor_colour_reply((0, 0xff, 0), true), "\x1b]12;rgb:0000/ffff/0000\x07");
    assert_eq!(crate::pane::cursor_colour_reply((0x12, 0x34, 0x56), false), "\x1b]12;rgb:1212/3434/5656\x1b\\");
}

#[test]
fn a_query_with_no_colour_has_no_reply() {
    let st = crate::pane::DecrqmState::default();
    let colours = crate::types::HostColors::campbell();
    let mut da1_open = false;
    let none = vt100::TerminalQuery::CursorColour { colour: None, bel: false };
    assert!(crate::pane::pane_replies(&[none], &st, &colours, &mut da1_open).is_empty());
    let some = vt100::TerminalQuery::CursorColour { colour: Some((0xff, 0, 0)), bel: false };
    let replies = crate::pane::pane_replies(&[some], &st, &colours, &mut da1_open);
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].text, "\x1b]12;rgb:ffff/0000/0000\x1b\\");
}

#[test]
fn the_terminal_is_told_as_tmux_tells_it() {
    // Measured from an attached tmux 3.8 client: ESC ] 12 ; rgb:00/ff/00 BEL,
    // then ESC ] 112 BEL.
    assert_eq!(crate::rendering::cursor_colour_set("#00ff00").as_deref(), Some("\x1b]12;rgb:00/ff/00\x07"));
    assert_eq!(crate::rendering::cursor_colour_set("#ABCDEF").as_deref(), Some("\x1b]12;rgb:ab/cd/ef\x07"));
    assert_eq!(crate::rendering::cursor_colour_set("blue"), None);
    assert_eq!(crate::rendering::CURSOR_COLOUR_RESET, "\x1b]112\x07");
}
