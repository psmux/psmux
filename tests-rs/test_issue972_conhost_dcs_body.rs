// Issue #972 follow up: `tmux;` left in Claude Code's prompt on Windows 10.
//
// Inside psmux, Claude Code sees TMUX and wraps its OSC 11 background colour
// query in a tmux passthrough: `ESC P tmux; ESC ESC ] 11;? BEL ESC \`. The
// inbox console host of Windows 10 (19045, conhost 10.0.19041) has no DCS
// parser. It runs `ESC P` as an escape it ignores, prints the body up to the
// first ESC (`tmux;`) into its buffer at the cursor, and forwards the inner OSC
// it does not know as it cached it, with both escapes: `ESC ESC ] 11;? BEL`.
// On its next frame it paints the five cells, so psmux received
// `ESC ESC ]11;? BEL tmux;` and drew `❯ tmux;refactor ...` over the placeholder.
//
// The application never wrote those cells as text, so psmux leaves them as the
// application drew them and only moves the cursor past them, the way conhost
// counted them. tmux drops the whole DCS while allow-passthrough is off
// (input_dcs_dispatch), and psmux does too when the DCS reaches it intact
// (Windows 11 and OpenConsole).
use crate::Parser;

/// Claude Code 2.1.323 with theme `auto` starting in a psmux pane (100x30) on
/// Windows 10 19045 with the inbox conhost, as PSMUX_PANE_RAW recorded it.
const CLAUDE_W10_DCS: &[u8] = include_bytes!("fixtures/issue972-claude-w10-dcs-tmux.bin");

fn conhost_parser(rows: u16, cols: u16) -> Parser {
    let mut p = Parser::new(rows, cols, 0);
    p.set_conhost_c0_cells(true);
    p
}

fn row(p: &Parser, r: u16, cols: u16) -> String {
    p.screen().rows(0, cols).nth(r as usize).unwrap()
}

#[test]
fn claude_code_start_on_windows_10_leaves_no_tmux_text() {
    let mut p = conhost_parser(30, 100);
    p.process(CLAUDE_W10_DCS);
    assert_eq!(row(&p, 26, 100), "\u{276f}\u{a0}Try \"refactor <filepath>\"");
    assert_eq!(p.screen().cursor_position(), (26, 2));
}

#[test]
fn the_capture_shows_the_reported_text_without_the_conhost_rule() {
    // What the screenshot in #972 shows.
    let mut p = Parser::new(30, 100, 0);
    p.process(CLAUDE_W10_DCS);
    assert_eq!(row(&p, 26, 100), "\u{276f}\u{a0}tmux;refactor <filepath>\"");
}

#[test]
fn the_cursor_moves_past_the_cells_conhost_counted() {
    // conhost's cursor is after its five cells; its next relative move starts
    // there.
    let mut p = conhost_parser(3, 30);
    p.process("\u{276f} Try it\x1b[1;3H".as_bytes());
    p.process(b"\x1b\x1b]11;?\x07tmux;\x1b[2Cx");
    assert_eq!(row(&p, 0, 30), "\u{276f} Try it x");
    assert_eq!(p.screen().cursor_position(), (0, 10));
}

#[test]
fn split_across_reads() {
    let mut p = conhost_parser(3, 30);
    p.process(b"abcdefgh\x1b[1;2H");
    for chunk in [&b"\x1b"[..], b"\x1b]11", b";?\x07t", b"mu", b"x;Z"] {
        p.process(chunk);
    }
    assert_eq!(row(&p, 0, 30), "abcdefZh");
}

#[test]
fn other_text_after_the_marker_is_drawn() {
    let mut p = conhost_parser(3, 30);
    p.process(b"\x1b\x1b]11;?\x07tmix;");
    assert_eq!(row(&p, 0, 30), "tmix;");
    // The marker is spent by the first text after it.
    let mut p = conhost_parser(3, 30);
    p.process(b"\x1b\x1b]11;?\x07a tmux;");
    assert_eq!(row(&p, 0, 30), "a tmux;");
}

#[test]
fn tmux_text_without_the_marker_is_drawn() {
    let mut p = conhost_parser(3, 30);
    p.process(b"\x1b]11;?\x07tmux;");
    assert_eq!(row(&p, 0, 30), "tmux;");
}

#[test]
fn plain_vt_keeps_the_text() {
    // A supplied OpenConsole hands psmux the application's bytes, where the
    // DCS arrives whole; the conhost rule is off there.
    let mut p = Parser::new(3, 30, 0);
    p.process(b"\x1b\x1b]11;?\x07tmux;");
    assert_eq!(row(&p, 0, 30), "tmux;");
}

#[test]
fn an_intact_passthrough_is_dropped_in_both_modes() {
    for conhost in [false, true] {
        let mut p = Parser::new(3, 30, 0);
        p.set_conhost_c0_cells(conhost);
        p.process(b"ab\x1bPtmux;\x1b\x1b]11;?\x07\x1b\\cd");
        assert_eq!(row(&p, 0, 30), "abcd", "conhost rule {conhost}");
    }
}
