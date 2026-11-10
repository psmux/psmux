// DECALN (ESC # 8), DECSTBM (CSI Ps ; Ps r) and VPA (CSI Ps d) in origin mode,
// as tmux 3.8 handles them (screen-write.c:1316 and :1775, input.c:1439,
// :1620 and :1870 at tag 3.8), and as the console hosts do where they differ.
// vt100 ignored DECALN, read VPA from the top of the screen in origin mode, and
// sent the cursor to the top of the region on DECSTBM, falling back to the
// whole screen for a region it could not set.
//
// Every expectation is what tmux 3.8 showed for the same bytes in a 40x10
// pane (#{cursor_x}, #{cursor_y}, #{scroll_region_upper},
// #{scroll_region_lower}, #{origin_flag}), except the five marked "hosts":
// there the inbox conhost and OpenConsole agree with each other and not with
// tmux, and psmux follows them so that its screen stays where the host's is.

use vt100_psmux as vt100;

const COLS: u16 = 40;
const ROWS: u16 = 10;

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(bytes.as_bytes());
    parser
}

/// (x, y, upper, lower, origin) after `bytes`.
fn after(bytes: &str) -> (u16, u16, u16, u16, bool) {
    let parser = parse(bytes);
    let screen = parser.screen();
    let (y, x) = screen.cursor_position();
    let (upper, lower) = screen.scroll_region();
    (x, y, upper, lower, screen.origin_mode())
}

fn row(parser: &vt100::Parser, n: u16) -> String {
    parser.screen().rows(0, COLS).nth(usize::from(n)).unwrap()
}

const ALL_E: &str = "EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE";

#[test]
fn the_default_region_is_the_whole_screen() {
    assert_eq!(after(""), (0, 0, 0, 9, false));
}

#[test]
fn decaln_fills_the_screen_with_e() {
    let parser = parse("\x1b#8");
    for n in 0..ROWS {
        assert_eq!(row(&parser, n), ALL_E, "row {n}");
    }
    assert_eq!(after("\x1b#8"), (0, 0, 0, 9, false));
}

#[test]
fn decaln_resets_the_region_and_homes_the_cursor() {
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b#8"), (0, 0, 0, 9, false));
    let parser = parse("\x1b[5;1Hhello\x1b#8");
    assert_eq!(row(&parser, 4), ALL_E);
}

#[test]
fn decaln_turns_origin_mode_off() {
    // hosts: tmux keeps origin mode, (0, 0, 0, 9, true), and the next CUP 1;1
    // after a region of 3-6 then lands on row 2 instead of 0.
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b#8"), (0, 0, 0, 9, false));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b#8\x1b[2;2H"), (1, 1, 0, 9, false));
    assert_eq!(after("\x1b[?6h\x1b#8\x1b[3;6r\x1b[1;1H"), (0, 0, 2, 5, false));
}

#[test]
fn decaln_fills_with_the_default_attributes_and_keeps_the_current_ones() {
    let parser = parse("\x1b[41m\x1b#8abc");
    let screen = parser.screen();
    assert_eq!(row(&parser, 0), format!("abc{}", &ALL_E[3..]));
    assert_eq!(screen.cell(0, 0).unwrap().bgcolor(), vt100::Color::Idx(1));
    assert_eq!(screen.cell(0, 3).unwrap().bgcolor(), vt100::Color::Default);
    assert_eq!(screen.cell(9, 39).unwrap().bgcolor(), vt100::Color::Default);
    assert_eq!(screen.cursor_position(), (0, 3));
}

#[test]
fn decstbm_sets_the_region_and_homes_the_cursor() {
    assert_eq!(after("\x1b[5;10H\x1b[3;6r"), (0, 0, 2, 5, false));
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b[r"), (0, 0, 0, 9, false));
    assert_eq!(after("\x1b[5;10H\x1b[;6r"), (0, 0, 0, 5, false));
    assert_eq!(after("\x1b[5;10H\x1b[3r"), (0, 0, 2, 9, false));
}

#[test]
fn decstbm_in_origin_mode_homes_to_the_top_of_the_region() {
    // hosts: tmux homes to the top left of the screen, (0, 0, 2, 5, true).
    assert_eq!(after("\x1b[?6h\x1b[3;6r"), (0, 2, 2, 5, true));
}

#[test]
fn decstbm_ignores_a_region_it_cannot_set() {
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b[6;3r"), (9, 4, 2, 5, false));
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b[5;5r"), (9, 4, 2, 5, false));
    assert_eq!(after("\x1b[5;10H\x1b[20;30r"), (9, 4, 0, 9, false));
}

#[test]
fn decstbm_ignores_a_bottom_past_the_screen() {
    // hosts: tmux cuts it to the last row, (0, 0, 2, 9, false).
    assert_eq!(after("\x1b[5;10H\x1b[3;20r"), (9, 4, 0, 9, false));
}

#[test]
fn decstbm_zero_zero_is_the_whole_screen() {
    // hosts: tmux reads it as a one line region and ignores it, (9, 4, 2, 5).
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b[0;0r"), (0, 0, 0, 9, false));
}

#[test]
fn vpa_counts_from_the_top_of_the_region_in_origin_mode() {
    assert_eq!(after("\x1b[3;6r\x1b[2d"), (0, 1, 2, 5, false));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[2d"), (0, 3, 2, 5, true));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[9d"), (0, 5, 2, 5, true));
}

#[test]
fn vpa_keeps_the_column() {
    assert_eq!(after("\x1b[5;10H\x1b[3d"), (9, 2, 0, 9, false));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[1;10H\x1b[2d"), (9, 3, 2, 5, true));
}

#[test]
fn cup_in_origin_mode_is_unchanged() {
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[2;4H"), (3, 3, 2, 5, true));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[9;4H"), (3, 5, 2, 5, true));
}

#[test]
fn decom_homes_the_cursor_either_way() {
    assert_eq!(after("\x1b[3;6r\x1b[5;10H\x1b[?6h"), (0, 2, 2, 5, true));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b[2;5H\x1b[?6l"), (0, 0, 2, 5, false));
    assert_eq!(after("\x1b[3;6r\x1b[?6h\x1b7\x1b[?6l\x1b8"), (0, 2, 2, 5, true));
}

#[test]
fn a_split_sequence_still_works() {
    let bytes = "\x1b[3;6r\x1b[?6h\x1b[2d\x1b#8\x1b[5;10H";
    for at in 0..=bytes.len() {
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        parser.process(&bytes.as_bytes()[..at]);
        parser.process(&bytes.as_bytes()[at..]);
        let screen = parser.screen();
        assert_eq!(screen.cursor_position(), (4, 9), "split at {at}");
        assert_eq!(screen.scroll_region(), (0, 9), "split at {at}");
        assert_eq!(row(&parser, 0), ALL_E, "split at {at}");
    }
}
