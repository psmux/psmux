// ICH, DCH and ECH with the cursor on a half of a wide character. tmux moves
// the cells and leaves the halves apart (grid-view.c:197-225 at tag 3.8); a
// row here never keeps a lone half (#639), so the expectation is tmux's row
// with a lone half as a blank. That is also what both console hosts of build
// 26300 and OpenConsole 1.25 leave, read back with ReadConsoleOutputW, but for
// DCH on a left half, where they drop the character before the cursor.

use vt100_psmux as vt100;

/// The row is `X` in column 0, a wide character in 1 and 2, `y` and `z`.
const ROW: &str = "X\u{3042}yz";

/// Row 0 after `bytes`: `[` and the character for a wide one, `]` for its
/// right half, `_` for a blank, up to the last cell that is not blank.
fn row_after(bytes: &str) -> String {
    let mut parser = vt100::Parser::new(5, 12, 0);
    parser.process(bytes.as_bytes());
    let screen = parser.screen();
    let mut out = String::new();
    for col in 0..12 {
        let cell = screen.cell(0, col).unwrap();
        if cell.is_wide_continuation() {
            out.push(']');
        } else if cell.is_wide() {
            out.push('[');
            out.push_str(cell.contents());
        } else if cell.has_contents() {
            out.push_str(cell.contents());
        } else {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// The row, the cursor to column `col` (1 based), then `op`.
fn on(row: &str, col: u16, op: &str) -> String {
    row_after(&format!("{row}\x1b[1;{col}H{op}"))
}

#[test]
fn the_row() {
    assert_eq!(row_after(ROW), "X[\u{3042}]yz");
}

#[test]
fn insert_on_the_right_half_blanks_both_halves() {
    assert_eq!(on(ROW, 3, "\x1b[@"), "X___yz");
    assert_eq!(on(ROW, 3, "\x1b[2@"), "X____yz");
    // The cursor stays on the first cell inserted.
    assert_eq!(on(ROW, 3, "\x1b[@Q"), "X_Q_yz");
}

#[test]
fn insert_on_the_left_half_moves_the_character_whole() {
    assert_eq!(on(ROW, 2, "\x1b[@"), "X_[\u{3042}]yz");
    assert_eq!(on(ROW, 1, "\x1b[@"), "_X[\u{3042}]yz");
}

#[test]
fn insert_mode_on_the_right_half() {
    assert_eq!(on(ROW, 3, "\x1b[4hQ"), "X_Q_yz");
    assert_eq!(on(ROW, 3, "\x1b[4h\u{3044}"), "X_[\u{3044}]_yz");
    assert_eq!(on(ROW, 2, "\x1b[4hQ"), "XQ[\u{3042}]yz");
}

#[test]
fn insert_pushes_a_wide_character_off_the_edge() {
    // Ten columns of text and the wide character in the last two: one
    // insert leaves its left half in the last column, a blank.
    let row = "0123456789\u{3042}";
    assert_eq!(on(row, 1, "\x1b[@"), "_0123456789");
}

#[test]
fn delete_on_either_half() {
    assert_eq!(on(ROW, 1, "\x1b[P"), "[\u{3042}]yz");
    assert_eq!(on(ROW, 2, "\x1b[P"), "X_yz");
    assert_eq!(on(ROW, 3, "\x1b[P"), "X_yz");
    assert_eq!(on(ROW, 2, "\x1b[2P"), "Xyz");
    assert_eq!(on(ROW, 1, "\x1b[2P"), "_yz");
    assert_eq!(on(ROW, 3, "\x1b[2P"), "X_z");
    assert_eq!(on(ROW, 2, "\x1b[3P"), "Xz");
    assert_eq!(on("X\u{3042}\u{3044}z", 3, "\x1b[P"), "X_[\u{3044}]z");
    assert_eq!(on(ROW, 2, "\x1b[PQ"), "XQyz");
    assert_eq!(on(ROW, 3, "\x1b[PQ"), "X_Qz");
}

#[test]
fn erase_on_either_half() {
    assert_eq!(on(ROW, 2, "\x1b[X"), "X__yz");
    assert_eq!(on(ROW, 3, "\x1b[X"), "X__yz");
}

#[test]
fn the_inserted_cells_take_the_background() {
    let mut parser = vt100::Parser::new(5, 12, 0);
    parser.process(format!("{ROW}\x1b[1;3H\x1b[41m\x1b[2@").as_bytes());
    let screen = parser.screen();
    let red = vt100::Color::Idx(1);
    let bg: Vec<_> = (0..7).map(|c| screen.cell(0, c).unwrap().bgcolor()).collect();
    assert_eq!(
        bg,
        [
            vt100::Color::Default,
            vt100::Color::Default,
            red,
            red,
            vt100::Color::Default,
            vt100::Color::Default,
            vt100::Color::Default,
        ]
    );
}
