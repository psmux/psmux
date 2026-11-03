// U+FFFD, the replacement character, is a character of one cell, as in tmux:
// tmux writes it for a byte that is no UTF-8 (input.c:783-793 at tag 3.8,
// input_stop_utf8), and the console hosts hand psmux U+FFFD for a lone 8-bit
// C1 byte, a byte such as 0xFF and a cut off sequence. vt100 dropped it, so
// every later cell of the line was one column short of the console host's.
//
// Every expectation is what tmux 3.8 kept for the same bytes (row text and
// cursor column), except U+0080 to U+009F written as UTF-8: tmux keeps them
// as a zero width character, the console hosts as nothing, and psmux, which
// follows the host, as nothing too.

use vt100_psmux as vt100;

fn after(bytes: &[u8]) -> (String, u16) {
    let mut parser = vt100::Parser::new(5, 80, 0);
    parser.process(bytes);
    let screen = parser.screen();
    let row = screen.contents().lines().next().unwrap_or("").to_string();
    (row, screen.cursor_position().1)
}

/// The same bytes fed one at a time, so every multi byte sequence is split.
fn after_bytewise(bytes: &[u8]) -> (String, u16) {
    let mut parser = vt100::Parser::new(5, 80, 0);
    for byte in bytes {
        parser.process(std::slice::from_ref(byte));
    }
    let screen = parser.screen();
    let row = screen.contents().lines().next().unwrap_or("").to_string();
    (row, screen.cursor_position().1)
}

#[test]
fn a_replacement_character_takes_one_cell() {
    let want = ("a\u{fffd}b|".to_string(), 4);
    assert_eq!(after("a\u{fffd}b|".as_bytes()), want);
    assert_eq!(after_bytewise("a\u{fffd}b|".as_bytes()), want);
    let cell = {
        let mut parser = vt100::Parser::new(5, 80, 0);
        parser.process("a\u{fffd}b".as_bytes());
        parser.screen().cell(0, 1).unwrap().clone()
    };
    assert_eq!(cell.contents(), "\u{fffd}");
    assert!(!cell.is_wide());
}

#[test]
fn bytes_that_are_no_utf8_become_one_replacement_character() {
    // A byte from 0xA0 up that starts nothing, and a sequence cut off by
    // plain text or by an escape: tmux writes one U+FFFD for each.
    let want = ("a\u{fffd}b|".to_string(), 4);
    assert_eq!(after(b"a\xffb|"), want);
    assert_eq!(after(b"a\xe2\x82b|"), want);
    assert_eq!(after(b"a\xe2\x82\x1b[mb|"), want);
    assert_eq!(after_bytewise(b"a\xffb|"), want);
}

#[test]
fn the_row_keeps_its_columns_after_a_replacement_character() {
    // An absolute move after the character lands where the host has it.
    let (row, _) = after("\u{fffd}\x1b[1;3HX".as_bytes());
    assert_eq!(row, "\u{fffd} X");
    let (row, col) = after("x\u{fffd}\u{fffd}y".as_bytes());
    assert_eq!((row.as_str(), col), ("x\u{fffd}\u{fffd}y", 4));
}

#[test]
fn encoded_c1_characters_still_take_no_cell() {
    // U+009B and U+0085 written as UTF-8: the console hosts move no cell,
    // and neither does psmux, whole or split across reads.
    assert_eq!(after(b"a\xc2\x9bb|"), ("ab|".to_string(), 3));
    assert_eq!(after(b"a\xc2\x85b|"), ("ab|".to_string(), 3));
    assert_eq!(after_bytewise(b"a\xc2\x9bb|"), ("ab|".to_string(), 3));
}

#[test]
fn formatted_contents_replay_the_replacement_character() {
    let mut parser = vt100::Parser::new(5, 80, 0);
    parser.process("a\u{fffd}b".as_bytes());
    let mut replay = vt100::Parser::new(5, 80, 0);
    replay.process(&parser.screen().contents_formatted());
    assert_eq!(replay.screen().contents(), parser.screen().contents());
    assert_eq!(replay.screen().cursor_position(), (0, 3));
}
