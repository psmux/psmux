// The cells of a sixel image that text or an erase is written over after it.
//
// Windows Terminal, whose sixel parser the pane's OpenConsole runs, erases the
// part of an image under anything written there later, cell by cell
// (`TextBuffer::Replace` and `FillRect` call `ImageSlice::EraseCells`).
// OpenConsole leaves the cursor on the row holding the top of the image's last
// band, so a program's next line lands on that row: in Windows Terminal the
// image's partly filled last row is erased there, and the client must leave
// those cells out too or the image is drawn over the text.
use crate::Parser;

/// 40 by 24 pixels: 4 by 2 cells, the second row only 4 pixels deep.
const IMAGE: &[u8] = b"\x1bPq#1;2;100;0;0#1!40~-!40~\x1b\\";

fn parser_with_image() -> Parser {
    let mut p = Parser::new(24, 80, 100);
    p.process(IMAGE);
    p
}

fn erased_rows(p: &Parser) -> Vec<Vec<bool>> {
    let img = &p.screen().images()[0];
    (0..img.cell_height).map(|r| (0..img.cell_width).map(|c| img.is_erased(r, c)).collect()).collect()
}

#[test]
fn the_next_line_written_over_the_last_row_erases_it() {
    let mut p = parser_with_image();
    assert_eq!(p.screen().cursor_position(), (0, 0), "OpenConsole's cursor: the top of the last band");
    p.process(b"\r\nPS C:\\> ");
    assert_eq!(erased_rows(&p), vec![vec![false; 4], vec![true; 4]]);
}

#[test]
fn only_the_cells_written_are_erased() {
    let mut p = parser_with_image();
    p.process(b"\r\n\x1b[2Cab");
    assert_eq!(erased_rows(&p), vec![vec![false; 4], vec![false, false, true, true]]);
}

#[test]
fn erasing_a_line_or_cells_erases_them() {
    let mut p = parser_with_image();
    p.process(b"\r\n\x1b[2K");
    assert_eq!(erased_rows(&p)[1], vec![true; 4], "EL 2");

    let mut p = parser_with_image();
    p.process(b"\x1b[1;3H\x1b[K");
    assert_eq!(erased_rows(&p)[0], vec![false, false, true, true], "EL 0 from the third column");

    let mut p = parser_with_image();
    p.process(b"\x1b[1;2H\x1b[1K");
    assert_eq!(erased_rows(&p)[0], vec![true, true, false, false], "EL 1 up to the second column");

    let mut p = parser_with_image();
    p.process(b"\x1b[1;2H\x1b[2X");
    assert_eq!(erased_rows(&p)[0], vec![false, true, true, false], "ECH 2");
}

#[test]
fn erasing_the_rest_of_the_screen_erases_the_rows_below() {
    let mut p = parser_with_image();
    p.process(b"\x1b[1;4H\x1b[J");
    assert_eq!(erased_rows(&p), vec![vec![false, false, false, true], vec![true; 4]]);

    let mut p = parser_with_image();
    p.process(b"\x1b[2;2H\x1b[1J");
    assert_eq!(erased_rows(&p), vec![vec![true; 4], vec![true, true, false, false]]);
}

#[test]
fn an_image_written_over_everywhere_is_dropped() {
    let mut p = parser_with_image();
    p.process(b"\x1b[1;1Habcd\r\nefgh");
    assert!(p.screen().images().is_empty());
}

#[test]
fn text_beside_the_image_leaves_it_alone() {
    let mut p = parser_with_image();
    p.process(b"\x1b[1;5Hright of it\x1b[3;1Hbelow it");
    assert!(p.screen().images()[0].erased.is_empty());
}

#[test]
fn the_cells_stay_erased_once_the_image_scrolls() {
    let mut p = parser_with_image();
    p.process(b"\r\nPS> ");
    for _ in 0..30 {
        p.process(b"\r\n");
    }
    let img = &p.screen().images()[0];
    assert!(img.is_erased(1, 0));
    assert!(!img.is_erased(0, 0));
}

#[test]
fn the_main_screen_image_is_not_erased_from_the_alternate_screen() {
    let mut p = parser_with_image();
    p.process(b"\x1b[?1049h\x1b[1;1Habcd\r\nefgh\x1b[?1049l");
    assert!(p.screen().images()[0].erased.is_empty());
}
