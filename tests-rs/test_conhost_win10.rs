// The inbox console host of Windows 10 (19045) writes two things the newer
// hosts do not (measured with PSMUX_PANE_RAW against its own buffer, read with
// ReadConsoleOutputCharacterW):
//
// - A C1 control a program wrote as UTF-8 (`C2 85` for U+0085) stays in its
//   buffer as a cell of one column, and the host hands psmux the two bytes as
//   written. psmux kept no cell for it, so the rest of the line, and every
//   later relative move, sat one column left of the host's. The newer hosts
//   keep no cell and send nothing. The cell is kept here as U+FFFD: written as
//   itself, U+0085 would be a NEL to the outer terminal.
// - It hides its cursor for a redraw with `CSI 25 l`, the `?` left out, and
//   shows it again with `CSI ? 25 h` (the #972 capture has five such pairs).
//   psmux read mode 25 as an ANSI mode it does not know and left the cursor
//   on through the redraw.
use crate::Parser;

const CLAUDE_W10: &[u8] = include_bytes!("fixtures/issue972-claude-w10-inbox-conhost.bin");

/// A parser for a pane on the Windows 10 inbox conhost.
fn win10(rows: u16, cols: u16) -> Parser {
    let mut p = Parser::new(rows, cols, 0);
    p.set_conhost_c0_cells(true);
    p.set_conhost_win10(true);
    p
}

fn row(p: &Parser, r: u16, cols: u16) -> String {
    p.screen().rows(0, cols).nth(r as usize).unwrap()
}

#[test]
fn a_c1_control_as_utf8_is_one_cell() {
    // Measured on 19045: `a C2 85 b |` leaves the host's cursor in column 4.
    let mut p = win10(5, 20);
    p.process(b"a\xc2\x85b|");
    assert_eq!(row(&p, 0, 20), "a\u{fffd}b|");
    assert_eq!(p.screen().cursor_position(), (0, 4));
}

#[test]
fn every_c1_control_is_one_cell() {
    for c in 0x80u32..=0x9f {
        let ch = char::from_u32(c).unwrap();
        let mut p = win10(5, 20);
        p.process(format!("x{ch}y").as_bytes());
        assert_eq!(row(&p, 0, 20), "x\u{fffd}y", "U+{c:04X}");
        assert_eq!(p.screen().cursor_position(), (0, 3), "U+{c:04X}");
    }
}

#[test]
fn a_c1_control_split_across_reads_is_one_cell() {
    let bytes: &[u8] = b"a\xc2\x85b|";
    for k in 0..=bytes.len() {
        let mut p = win10(5, 20);
        p.process(&bytes[..k]);
        p.process(&bytes[k..]);
        assert_eq!(row(&p, 0, 20), "a\u{fffd}b|", "split at {k}");
        assert_eq!(p.screen().cursor_position(), (0, 4), "split at {k}");
    }
}

#[test]
fn elsewhere_a_c1_control_is_still_no_cell() {
    // The newer hosts keep no cell and send nothing; plain VT and the newer
    // inbox host's cell rule keep ignoring one as before.
    for c0_cells in [false, true] {
        let mut p = Parser::new(5, 20, 0);
        p.set_conhost_c0_cells(c0_cells);
        p.process(b"a\xc2\x85b|");
        assert_eq!(row(&p, 0, 20), "ab|");
        assert_eq!(p.screen().cursor_position(), (0, 3));
    }
}

#[test]
fn csi_25_l_and_h_hide_and_show_the_cursor() {
    let mut p = win10(5, 20);
    p.process(b"\x1b[25l");
    assert!(p.screen().hide_cursor());
    p.process(b"\x1b[25h");
    assert!(!p.screen().hide_cursor());
    // With insert mode in the same sequence, both apply.
    p.process(b"\x1b[4;25l");
    assert!(p.screen().hide_cursor());
    p.process(b"\x1b[4;25h");
    assert!(!p.screen().hide_cursor());
    p.process(b"ab\rX");
    assert_eq!(row(&p, 0, 20), "Xab", "insert mode is on");
}

#[test]
fn elsewhere_csi_25_l_changes_nothing() {
    let mut p = Parser::new(5, 20, 0);
    p.set_conhost_c0_cells(true);
    p.process(b"\x1b[25l");
    assert!(!p.screen().hide_cursor());
}

#[test]
fn the_972_capture_hides_the_cursor_for_each_redraw() {
    // Every `CSI 25 l` in the capture is followed by a `CSI ? 25 h`, so the
    // cursor is hidden between them and shown at the end; the grid is the one
    // #972 fixed.
    let mut p = win10(20, 100);
    let at = CLAUDE_W10.windows(5).position(|w| w == b"\x1b[25l").unwrap() + 5;
    p.process(&CLAUDE_W10[..at]);
    assert!(p.screen().hide_cursor(), "after the first CSI 25 l");
    p.process(&CLAUDE_W10[at..]);
    assert!(!p.screen().hide_cursor());
    assert_eq!(row(&p, 16, 100), "\u{276f}\u{a0}b");
    assert_eq!(p.screen().cursor_position(), (16, 3));
}
