// capture-pane -J joins wrapped rows into one line, on the grid.
//
// psmux used to capture row by row and then, for -J, only trim each line, so
// a line the pane wrapped still came back in pieces and the spaces at the wrap
// point were lost. tmux (cmd-capture-pane.c cmd_capture_pane_history) leaves
// GRID_STRING_TRIM_SPACES and GRID_STRING_EMPTY_CELLS off for -J: each row is
// the cells it used, spaces the program printed included, and a row flagged
// GRID_LINE_WRAPPED gets no newline. Every expected string here is what tmux
// 3.4 and 3.8 print for the same bytes in a 40 column pane.
//
// Registered from src/copy_mode.rs; the pane scaffolding (stub PTY, no psmux
// server) is shared with test_capture_pane_fidelity.rs.

use super::*;
use super::tests_capture_pane_fidelity::{app_showing, make_pane, make_window};
use crate::types::Node;
use std::sync::{Arc, Mutex};

const J: CaptureOpts = CaptureOpts { preserve_trailing: false, join: true };
const JN: CaptureOpts = CaptureOpts { preserve_trailing: true, join: true };

fn x(n: usize) -> String {
    "x".repeat(n)
}

#[test]
fn join_runs_a_wrapped_line_together_and_keeps_its_spaces() {
    // 38 x, two spaces in the last columns, then "yy" on the next row.
    let mut app = app_showing(format!("{}  yy\r\nnext\r\n", x(38)).as_bytes());
    let plain = capture_active_pane_text(&mut app, None, false).unwrap().unwrap();
    assert_eq!(plain, format!("{}\nyy\nnext\n", x(38)), "without -J every row is its own line");
    let joined = capture_active_pane_text(&mut app, None, J).unwrap().unwrap();
    assert_eq!(joined, format!("{}  yy\nnext\n", x(38)), "-J joins the rows and keeps the spaces at the wrap");
    assert_eq!(capture_active_pane_text(&mut app, None, JN).unwrap().unwrap(), joined, "-JN is -J");
}

#[test]
fn join_keeps_trailing_spaces_the_program_printed_but_not_cleared_cells() {
    // "ab  " was printed (spaces are cells it used); "cd" then EL leaves the
    // rest of its row cleared, which tmux does not count.
    let mut app = app_showing(b"ab  \r\ncd\x1b[K\r\n");
    let joined = capture_active_pane_text(&mut app, None, J).unwrap().unwrap();
    assert_eq!(joined, "ab  \ncd\n");
    let plain = capture_active_pane_text(&mut app, None, false).unwrap().unwrap();
    assert_eq!(plain, "ab\ncd\n", "the default capture still trims");
}

#[test]
fn join_handles_a_wide_character_at_the_wrap_point() {
    // 38 x then two wide glyphs: the first fills columns 38 and 39, the
    // second wraps, and the join puts them side by side with nothing between.
    //
    // Not covered: 39 x then a wide glyph, which does not fit in column 39.
    // tmux flags that row wrapped (screen_write_linefeed with wrapped set),
    // the vt100 parser does not, so -J leaves that line in two pieces.
    let w = "\u{4e16}";
    let mut app = app_showing(format!("{}{w}{w}X\r\nnext\r\n", x(38)).as_bytes());
    let joined = capture_active_pane_text(&mut app, None, J).unwrap().unwrap();
    assert_eq!(joined, format!("{}{w}{w}X\nnext\n", x(38)));
}

#[test]
fn join_respects_an_explicit_range() {
    let mut app = app_showing(format!("{}CCCCC  \r\nnext\r\n", x(40)).as_bytes());
    // The range ends inside the wrapped line: tmux prints the first row with
    // no newline after it (capture-pane -p adds the one it ends with).
    let first = capture_active_pane_range(&mut app, Some(0), Some(0), None, J).unwrap().unwrap();
    assert_eq!(first, x(40));
    // The range starts on the continuation row.
    let second = capture_active_pane_range(&mut app, Some(1), Some(1), None, J).unwrap().unwrap();
    assert_eq!(second, "CCCCC  \n");
    let both = capture_active_pane_range(&mut app, Some(0), Some(2), None, J).unwrap().unwrap();
    assert_eq!(both, format!("{}CCCCC  \nnext\n", x(40)));
}

#[test]
fn join_reads_wrapped_lines_in_the_scrollback() {
    // A pane with history: the wrapped line scrolls off the top.
    let pane = {
        let mut p = make_pane(0, 6, 40);
        p.term = Arc::new(Mutex::new(vt100::Parser::new(6, 40, 100)));
        p
    };
    let mut bytes = format!("{}tail\r\n", x(40));
    for i in 0..10 {
        bytes.push_str(&format!("line{i}\r\n"));
    }
    pane.term.lock().unwrap().process(bytes.as_bytes());
    let mut app = app_showing(b"");
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    app.windows[0] = win;
    let all = capture_active_pane_range(&mut app, Some(i32::MIN), None, None, J).unwrap().unwrap();
    assert!(all.starts_with(&format!("{}tail\nline0\n", x(40))), "got {all:?}");
}

#[test]
fn styled_join_carries_the_style_across_the_wrap() {
    // tmux -Je prints a red run that wraps as one line with one style
    // sequence, then the default style for the rest.
    let mut app = app_showing(format!("\x1b[31m{}\x1b[0mDX\r\n", "R".repeat(40)).as_bytes());
    let out = capture_active_pane_styled(&mut app, None, None, None, J).unwrap().unwrap();
    let first = out.lines().next().unwrap();
    assert_eq!(first, format!("\x1b[0;31m{}\x1b[0mDX\x1b[0m", "R".repeat(40)));
    // A run that is still red at the end of the joined line is reset there.
    let mut app = app_showing(format!("\x1b[31m{}RR\x1b[0m\r\n", "R".repeat(40)).as_bytes());
    let out = capture_active_pane_styled(&mut app, None, None, None, J).unwrap().unwrap();
    assert_eq!(out.lines().next().unwrap(), format!("\x1b[0;31m{}\x1b[0m", "R".repeat(42)));
}
