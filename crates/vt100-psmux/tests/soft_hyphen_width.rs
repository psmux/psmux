// U+00AD SOFT HYPHEN occupies one column in the emulator, as Neovim and tmux
// draw it. unicode-width gives it zero, and when the emulator followed that the
// text after it sat one column left of where the program put it, so every
// later cell on the row (including colour bands such as colorcolumn) landed in
// the wrong place. Reproduced with the cp1251 decoding of a GBK header.

fn row_text(parser: &vt100_psmux::Parser, row: u16) -> Vec<String> {
    let screen = parser.screen();
    let (_, cols) = screen.size();
    (0..cols)
        .map(|c| screen.cell(row, c).map(|cell| cell.contents().to_string()).unwrap_or_default())
        .collect()
}

#[test]
fn soft_hyphen_keeps_the_following_text_in_place() {
    let mut parser = vt100_psmux::Parser::new(2, 10, 0);
    parser.process("a\u{AD}bcd".as_bytes());
    let row = row_text(&parser, 0);
    assert_eq!(&row[..5], ["a", "\u{AD}", "b", "c", "d"]);
    assert_eq!(parser.screen().cursor_position(), (0, 5));
}

#[test]
fn soft_hyphen_is_erased_like_any_one_column_cell() {
    let mut parser = vt100_psmux::Parser::new(2, 10, 0);
    parser.process("a\u{AD}b\r\x1b[2Kz".as_bytes());
    let row = row_text(&parser, 0);
    assert_eq!(row[0], "z");
    assert!(row[1..].iter().all(|c| c.is_empty() || c == " "));
}
