// REP (CSI Ps b), autowrap off (CSI ? 7 l) and insert mode (CSI 4 h), as
// tmux handles them (input.c:1799-1818, :1946, :2043, :2007, :1910 and
// screen-write.c:2695-2707, :2790-2794 at tag 3.8). vt100 ignored all three,
// so on the OpenConsole route, which hands psmux the program's output as
// written, `a ESC[3b` stayed `a`, a line wrapped with autowrap off and insert
// mode overwrote.
//
// Every expectation is what tmux 3.8 kept in a 20 column pane, except REP
// past the right margin: tmux stops there, the console hosts wrap and go on
// as xterm does, and psmux follows the host (Discussion #937).

use vt100_psmux as vt100;

const COLS: u16 = 20;

fn parser(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(5, COLS, 0);
    parser.process(bytes.as_bytes());
    parser
}

/// Rows 0 and 1, and the cursor column.
fn after(bytes: &str) -> (String, String, u16) {
    let parser = parser(bytes);
    let screen = parser.screen();
    let (row0, row1) = (row(screen, 0), row(screen, 1));
    (row0, row1, screen.cursor_position().1)
}

/// One row on its own (`contents` joins a wrapped row to the next).
fn row(screen: &vt100::Screen, n: usize) -> String {
    screen.rows(0, COLS).nth(n).unwrap_or_default().trim_end().to_string()
}

fn want(row0: &str, row1: &str, col: u16) -> (String, String, u16) {
    (row0.to_string(), row1.to_string(), col)
}

const LINE: &str = "0123456789ABCDEFGHIJKL";

#[test]
fn rep_repeats_the_last_character() {
    assert_eq!(after("a\x1b[3bX\x1b[2b|"), want("aaaaXXX|", "", 8));
    // No parameter, or 0, is once.
    assert_eq!(after("a\x1b[bX\x1b[0b|"), want("aaXX|", "", 5));
}

#[test]
fn rep_after_a_control_an_escape_or_a_sequence_repeats_nothing() {
    assert_eq!(after("a\x1b[C\x1b[3b|"), want("a |", "", 3));
    assert_eq!(after("a\x07\x1b[3b|"), want("a|", "", 2));
    assert_eq!(after("a\x1b7\x1b[3b|"), want("a|", "", 2));
    assert_eq!(after("a\x1b]2;t\x07\x1b[3b|"), want("a|", "", 2));
    // A REP ends it too: the second one repeats nothing.
    assert_eq!(after("a\x1b[2b\x1b[2b|"), want("aaa|", "", 4));
}

#[test]
fn rep_with_nothing_printed_does_nothing() {
    assert_eq!(after("\x1b[3bX"), want("X", "", 1));
}

#[test]
fn rep_repeats_a_wide_character() {
    assert_eq!(after("\u{3042}\x1b[2b|"), want("\u{3042}\u{3042}\u{3042}|", "", 7));
}

#[test]
fn rep_wraps_at_the_right_margin_as_the_console_hosts_do() {
    assert_eq!(after("\x1b[18Ga\x1b[9b"), want("                 aaa", "aaaaaaa", 7));
    let parser = parser("\x1b[18Ga\x1b[9b");
    assert!(parser.screen().row_wrapped(0));
}

#[test]
fn rep_takes_the_largest_count() {
    // 1 + 65535 letters: the screen is full, and the last row holds 16.
    let parser = parser("a\x1b[65535b|");
    let screen = parser.screen();
    assert_eq!(screen.cursor_position(), (4, 17));
    assert_eq!(row(screen, 0), "a".repeat(20));
    assert_eq!(row(screen, 4), format!("{}|", "a".repeat(16)));
}

#[test]
fn autowrap_off_overwrites_the_last_column() {
    assert_eq!(after(&format!("\x1b[?7l{}", LINE)), want("0123456789ABCDEFGHIL", "", 19));
    assert!(!parser(&format!("\x1b[?7l{}", LINE)).screen().row_wrapped(0));
}

#[test]
fn autowrap_on_again_wraps() {
    assert_eq!(
        after(&format!("\x1b[?7l{}\x1b[?7hMN", LINE)),
        want("0123456789ABCDEFGHIM", "N", 1)
    );
    // The default wraps.
    assert_eq!(after(LINE), want("0123456789ABCDEFGHIJ", "KL", 2));
}

#[test]
fn autowrap_off_drops_a_wide_character_that_does_not_fit() {
    let text = format!("\x1b[?7l{}\u{3042}|", "a".repeat(19));
    assert_eq!(after(&text), want(&format!("{}|", "a".repeat(19)), "", 19));
}

#[test]
fn autowrap_off_puts_a_wide_character_on_the_last_two_columns() {
    let text = format!("\x1b[?7l{}\u{3042}", "a".repeat(18));
    assert_eq!(after(&text), want(&format!("{}\u{3042}", "a".repeat(18)), "", 19));
}

#[test]
fn ris_turns_autowrap_back_on() {
    assert_eq!(after(&format!("\x1b[?7l\x1bc{}", LINE)), want("0123456789ABCDEFGHIJ", "KL", 2));
}

#[test]
fn insert_mode_moves_the_rest_of_the_line_right() {
    assert_eq!(after("abcdef\r\x1b[4hXY\x1b[4l"), want("XYabcdef", "", 2));
    assert_eq!(after("abcdef\r\x1b[4hXY\x1b[4lZ"), want("XYZbcdef", "", 3));
}

#[test]
fn insert_mode_makes_room_for_a_wide_character() {
    assert_eq!(after("abcdef\r\x1b[4h\u{3042}\x1b[4l"), want("\u{3042}abcdef", "", 2));
}

#[test]
fn insert_mode_loses_what_moves_past_the_margin() {
    assert_eq!(after("0123456789ABCDEFGHIJ\r\x1b[4hX\x1b[4l"), want("X0123456789ABCDEFGHI", "", 1));
}

#[test]
fn ris_ends_insert_mode() {
    assert_eq!(after("\x1b[4h\x1bcabc\rX"), want("Xbc", "", 1));
}

#[test]
fn other_ansi_modes_stay_ignored() {
    // LNM (20) is not insert mode.
    assert_eq!(after("abcdef\r\x1b[20hXY"), want("XYcdef", "", 2));
}
