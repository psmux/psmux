// ESC D (IND) and ESC E (NEL), which tmux handles in input_esc_dispatch
// (input.c:1413-1418 in tmux 3.8): IND moves down a line and scrolls at the
// bottom margin as a line feed does, NEL does a carriage return first.  vt100
// ignored both.  chafa makes room for an image with a run of IND and then moves
// back up, so its image landed over the line it was started from.
use crate::Parser;

#[test]
fn ind_moves_down_a_line_in_the_same_column() {
    let mut p = Parser::new(24, 80, 0);
    p.process(b"\x1b[3;5H\x1bD");
    assert_eq!(p.screen().cursor_position(), (3, 4));
}

#[test]
fn ind_scrolls_at_the_bottom_margin() {
    let mut p = Parser::new(5, 20, 10);
    p.process(b"top\x1b[5;1Hbottom\x1bD");
    assert_eq!(p.screen().cursor_position(), (4, 6));
    let rows: Vec<String> = p.screen().rows(0, 20).collect();
    assert_eq!(rows, vec!["", "", "", "bottom", ""], "everything moved up a row");
}

#[test]
fn ind_scrolls_inside_the_scroll_region() {
    let mut p = Parser::new(10, 20, 0);
    p.process(b"\x1b[2;4r\x1b[2;1Ha\x1b[3;1Hb\x1b[4;1Hc\x1b[9;1Hkeep\x1b[4;1H\x1bD");
    let rows: Vec<String> = p.screen().rows(0, 20).collect();
    assert_eq!(rows[1], "b");
    assert_eq!(rows[2], "c");
    assert_eq!(rows[3], "");
    assert_eq!(rows[8], "keep");
}

#[test]
fn nel_goes_to_the_start_of_the_next_line() {
    let mut p = Parser::new(24, 80, 0);
    p.process(b"\x1b[3;5H\x1bE");
    assert_eq!(p.screen().cursor_position(), (3, 0));
}

#[test]
fn chafa_makes_room_for_its_image_below_the_command() {
    // What OpenConsole passes on when chafa starts an animation under a line
    // of output: room for 22 rows, back up, save the place, then the frame.
    let mut p = Parser::new(30, 120, 100);
    p.process(b"PS> chafa\r\n");
    let mut room = b"\x1b[?8452l".to_vec();
    for _ in 0..22 {
        room.extend_from_slice(b"\x1bD");
    }
    room.extend_from_slice(b"\x1b[22A\x1b[s");
    p.process(&room);
    assert_eq!(p.screen().cursor_position(), (1, 0), "the row below the command");
    p.process(b"\x1bP0;1;0q\"1;1;40;40#1;2;100;0;0#1!40~\x1b\\\x1b[u");
    let img = &p.screen().images()[0];
    assert_eq!(img.anchor_line, 1);
    assert_eq!(p.screen().rows(0, 120).next().unwrap(), "PS> chafa");
}
