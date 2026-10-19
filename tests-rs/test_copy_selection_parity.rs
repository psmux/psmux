// What a copy mode selection copies, against tmux 3.4 and 3.8.
//
// tmux window_copy_get_selection keeps the top left end of a selection and,
// with emacs mode-keys, drops the bottom right one whichever way the
// selection was made; vi keeps it. The last row stops at the end of the
// row's text, so blanks between words at the end of the selection are
// copied (psmux trimmed them), and a line the pane wrapped copies as one
// line. Every expected string is what both tmux versions put in the buffer
// for the same screen and motions (send -X cursor-right and friends).
//
// selection_text is the copy without the buffer and clipboard side effects
// of yank_selection, so nothing here touches the system clipboard.
// Registered from src/copy_mode.rs.

use super::*;
use crate::types::SelectionMode::{Char, Rect};

const ROWS: u16 = 10;
const COLS: u16 = 40;

fn screen(text: &str) -> vt100::Parser {
    let mut p = vt100::Parser::new(ROWS, COLS, 0);
    p.process(text.replace('\n', "\r\n").as_bytes());
    p
}

/// Select from `anchor` to `cursor` (row, col) and copy.
fn copy(p: &mut vt100::Parser, anchor: (u16, u16), cursor: (u16, u16), mode: crate::types::SelectionMode, emacs: bool) -> String {
    selection_text(p, ROWS, COLS, (anchor.0 as i64, anchor.1), (cursor.0 as i64, cursor.1), mode, emacs, 0)
}

const TEXT: &str = "abcdefgh\nfoo   bar   baz\nxy\n";

#[test]
fn emacs_leaves_the_cursor_cell_out_and_vi_keeps_it() {
    let mut p = screen(TEXT);
    // Three cursor-right from column 0.
    assert_eq!(copy(&mut p, (0, 0), (0, 3), Char, true), "abc");
    assert_eq!(copy(&mut p, (0, 0), (0, 3), Char, false), "abcd");
    // Backwards on one row: the right end is the anchor, and emacs drops it.
    assert_eq!(copy(&mut p, (0, 3), (0, 1), Char, true), "bc");
    // Both ends on one cell: emacs selects nothing.
    assert_eq!(copy(&mut p, (0, 2), (0, 2), Char, true), "");
    assert_eq!(copy(&mut p, (0, 2), (0, 2), Char, false), "c");
}

#[test]
fn the_last_row_keeps_blanks_between_words_up_to_the_end() {
    let mut p = screen(TEXT);
    // down, right x4 / x5 / x10 from (0, 0).
    assert_eq!(copy(&mut p, (0, 0), (1, 4), Char, true), "abcdefgh\nfoo ");
    assert_eq!(copy(&mut p, (0, 0), (1, 4), Char, false), "abcdefgh\nfoo  ");
    assert_eq!(copy(&mut p, (0, 0), (1, 5), Char, true), "abcdefgh\nfoo  ");
    assert_eq!(copy(&mut p, (0, 0), (1, 5), Char, false), "abcdefgh\nfoo   ");
    assert_eq!(copy(&mut p, (0, 0), (1, 10), Char, true), "abcdefgh\nfoo   bar ");
    assert_eq!(copy(&mut p, (0, 0), (1, 10), Char, false), "abcdefgh\nfoo   bar  ");
    // Past the text of the last row, the text and nothing more.
    assert_eq!(copy(&mut p, (0, 0), (2, 2), Char, true), "abcdefgh\nfoo   bar   baz\nxy");
    assert_eq!(copy(&mut p, (0, 0), (2, 1), Char, false), "abcdefgh\nfoo   bar   baz\nxy");
    assert_eq!(copy(&mut p, (0, 0), (2, 30), Char, false), "abcdefgh\nfoo   bar   baz\nxy");
    // An upward selection ends on the anchor, which emacs leaves out too.
    assert_eq!(copy(&mut p, (1, 4), (0, 2), Char, true), "cdefgh\nfoo ");
}

#[test]
fn a_selection_that_ends_on_an_empty_row() {
    let mut p = screen("abc\n\nxyz\n");
    // emacs: the empty row's newline is the one tmux drops.
    assert_eq!(copy(&mut p, (0, 0), (1, 0), Char, true), "abc\n");
    // vi: the end is past the (empty) text, so the newline stays.
    assert_eq!(copy(&mut p, (0, 0), (1, 0), Char, false), "abc\n\n");
}

#[test]
fn a_wrapped_line_copies_as_one_line() {
    // Row 3 wraps into row 4: "AAAAAAAAAABBBBBBBBB C...", 40 wide.
    let line = format!("{}{} CCCC", "A".repeat(20), "B".repeat(19));
    let mut p = screen(&format!("abc\n\nxyz\n{line}\nend\n"));
    assert!(p.screen().row_wrapped(3), "the long line wraps");
    // From the start of the line to the second C, and to the end of it.
    assert_eq!(copy(&mut p, (3, 0), (4, 2), Char, true), format!("{}{} CC", "A".repeat(20), "B".repeat(19)));
    assert_eq!(copy(&mut p, (3, 0), (4, 2), Char, false), format!("{}{} CCC", "A".repeat(20), "B".repeat(19)));
    assert_eq!(copy(&mut p, (3, 0), (4, 3), Char, false), line);
    // A selection that only covers whole wrapped rows ends in its text.
    assert_eq!(copy(&mut p, (2, 0), (4, 3), Char, false), format!("xyz\n{line}"));
}

#[test]
fn a_rectangle_leaves_the_cursor_column_out_with_emacs_keys() {
    let mut p = screen("abc\n\nxyz\n");
    // rectangle-toggle, down x2, right x2 from (0, 0).
    assert_eq!(copy(&mut p, (0, 0), (2, 2), Rect, true), "ab\n\nxy");
    assert_eq!(copy(&mut p, (0, 0), (2, 2), Rect, false), "abc\n\nxyz");
    // Cursor left of the anchor: both keep the anchor column.
    assert_eq!(copy(&mut p, (0, 2), (2, 0), Rect, true), "abc\n\nxyz");
}

#[test]
fn a_rectangle_keeps_blanks_between_words() {
    let mut p = screen("a   b\nc   d\n");
    assert_eq!(copy(&mut p, (0, 0), (1, 2), Rect, false), "a  \nc  ");
}

#[test]
fn the_highlight_paints_what_the_copy_takes() {
    // vi: both ends painted.
    assert_eq!(char_selection_highlight((0, 0), (0, 3), false, COLS), Some(((0, 0), (0, 3))));
    // emacs: the end cell is not painted.
    assert_eq!(char_selection_highlight((0, 0), (0, 3), true, COLS), Some(((0, 0), (0, 2))));
    assert_eq!(char_selection_highlight((0, 2), (0, 2), true, COLS), None);
    // emacs, end at column 0 of a later row: that row has nothing selected.
    assert_eq!(char_selection_highlight((0, 1), (2, 0), true, COLS), Some(((0, 1), (1, COLS - 1))));
    assert_eq!(char_selection_highlight((1, 0), (1, 0), true, COLS), None);
}
