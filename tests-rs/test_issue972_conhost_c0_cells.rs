// Issue #972: the inbox Windows console host draws a C0 control that sits in a
// buffer cell as that raw byte and counts it as one printed column.
//
// A console cell can hold a C0 control: WriteConsoleOutputCharacterW puts one
// there, and so does a plain write while ENABLE_PROCESSED_OUTPUT is off. The
// inbox conhost (measured on Windows 10 19045 and Windows 11 26100, with and
// without the passthrough flag) then writes that cell to the ConPTY pipe as the
// bare byte (0x0F for SI) and moves its own idea of the terminal cursor one
// column right. Its next relative move (a BS to step back onto the cell) then
// lands one column short in a terminal that runs SI as a control. That is the
// stale `❯a` in Claude Code's prompt on Windows 10: the cell after `❯` held an
// SI, conhost sent `SI BS a` to put `a` in column 3, and psmux put it in
// column 2 where nothing ever erased it again.
//
// With processed output on, conhost turns the same controls into the code page
// 437 glyphs (SI becomes ☼), and a NuGet OpenConsole draws the cell as that
// glyph too. psmux does the same for the inbox host's stream, so its grid stays
// in step with the console host's.
use crate::Parser;

/// The capture from #972: Claude Code 2.1.322 in a psmux pane on Windows 10
/// 19045 with the inbox conhost, typing `a`, Backspace, `b`. The home folder
/// name in it is replaced with one of the same length.
const CLAUDE_W10: &[u8] = include_bytes!("fixtures/issue972-claude-w10-inbox-conhost.bin");

fn conhost_parser(rows: u16, cols: u16) -> Parser {
    let mut p = Parser::new(rows, cols, 0);
    p.set_conhost_c0_cells(true);
    p
}

fn row(p: &Parser, r: u16, cols: u16) -> String {
    p.screen().rows(0, cols).nth(r as usize).unwrap()
}

#[test]
fn si_cell_then_backspace_puts_the_letter_where_conhost_meant() {
    // The exact bytes conhost sent for the keystroke, after drawing
    // `❯ Try` and parking the cursor in column 3.
    let mut p = conhost_parser(5, 40);
    p.process("\u{276f}\u{a0}Try\x1b[1;3H".as_bytes());
    p.process(b"\x0f\x1b[25l\x08a\x1b[K\x1b[1;4H");
    assert_eq!(row(&p, 0, 40), "\u{276f}\u{a0}a");
    assert_eq!(p.screen().cursor_position(), (0, 3));
}

#[test]
fn claude_code_capture_from_windows_10_leaves_no_ghost() {
    let mut p = conhost_parser(20, 100);
    p.process(CLAUDE_W10);
    // After `a`, Backspace, `b` the prompt holds just `b`.
    assert_eq!(row(&p, 16, 100), "\u{276f}\u{a0}b");
    assert_eq!(p.screen().cursor_position(), (16, 3));
}

#[test]
fn the_capture_shows_the_reported_ghost_without_the_conhost_rule() {
    // What #972 reported: the first letter stays and the next one lands
    // after it.
    let mut p = Parser::new(20, 100, 0);
    p.process(CLAUDE_W10);
    assert_eq!(row(&p, 16, 100), "\u{276f}ab");
}

#[test]
fn every_cell_control_is_one_column_with_the_conhost_glyph() {
    let cases: &[(u8, char)] = &[
        (0x01, '\u{263a}'), (0x02, '\u{263b}'), (0x03, '\u{2665}'), (0x04, '\u{2666}'),
        (0x05, '\u{2663}'), (0x06, '\u{2660}'), (0x09, '\u{25cb}'), (0x0b, '\u{2642}'),
        (0x0c, '\u{2640}'), (0x0e, '\u{266b}'), (0x0f, '\u{263c}'), (0x10, '\u{25ba}'),
        (0x11, '\u{25c4}'), (0x12, '\u{2195}'), (0x13, '\u{203c}'), (0x14, '\u{00b6}'),
        (0x15, '\u{00a7}'), (0x16, '\u{25ac}'), (0x17, '\u{21a8}'), (0x18, '\u{2191}'),
        (0x19, '\u{2193}'), (0x1a, '\u{2192}'), (0x1c, '\u{221f}'), (0x1d, '\u{2194}'),
        (0x1e, '\u{25b2}'), (0x1f, '\u{25bc}'),
    ];
    for &(b, glyph) in cases {
        let mut p = conhost_parser(3, 20);
        p.process(&[b'x', b, b'y']);
        assert_eq!(row(&p, 0, 20), format!("x{glyph}y"), "byte {b:#04x}");
        assert_eq!(p.screen().cursor_position(), (0, 3), "byte {b:#04x} is one column");
    }
}

#[test]
fn bell_backspace_return_and_line_feed_keep_their_meaning() {
    // conhost itself sends these as controls: BEL for a real bell (it does not
    // advance its cursor for one), BS, CR and LF for its own cursor moves.
    let mut p = conhost_parser(4, 20);
    p.process(b"ab\x07c\x08d\r\x1b[1;5He\nf");
    assert_eq!(row(&p, 0, 20), "abd e");
    assert_eq!(row(&p, 1, 20), "     f");
    assert_eq!(p.screen().audible_bell_count, 1);
}

#[test]
fn escape_sequences_still_parse_around_cell_controls() {
    let mut p = conhost_parser(3, 20);
    p.process(b"\x1b[31m\x0f\x1b[1;5Hz");
    assert_eq!(row(&p, 0, 20), "\u{263c}   z");
    assert_eq!(p.screen().cell(0, 0).unwrap().fgcolor(), crate::Color::Idx(1));
}

#[test]
fn without_the_conhost_rule_controls_are_vt_controls() {
    // A supplied OpenConsole forwards the application's own bytes, where a tab
    // is a tab and SI selects G0, so the rule is off unless asked for.
    let mut p = Parser::new(3, 20, 0);
    assert!(!p.conhost_c0_cells());
    p.process(b"a\tb\x1b)0\x0eq\x0fq");
    assert_eq!(row(&p, 0, 20), "a       b\u{2500}q");
}
