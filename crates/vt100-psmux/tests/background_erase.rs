// Background colour erase: what an inserted or deleted line or cell, a line
// scrolled in by LF, IND, NEL, RI, SU or SD, and an erase (ED, EL, ECH)
// leave behind is the blank cell with the current background and nothing
// else (`grid_clear_cell`, grid.c:264 at tag 3.8, called with
// `ictx->cell.cell.bg` from input.c:1365-1425 and 1613-1852). vt100 left the
// first group in the default colours and gave the erases every attribute, so
// an underline or reverse ran across the erased cells. Every expectation is
// what tmux 3.8 and both console hosts showed for the same bytes in a 40x10
// pane, unless a comment says the console hosts are followed instead.

use vt100_psmux as vt100;

const COLS: u16 = 40;
const ROWS: u16 = 10;

/// Rows 0-9 hold `aaaa...` to `jjjj...`, 20 letters each, so shifts show.
fn fill() -> String {
    (0..ROWS)
        .map(|r| format!("\x1b[{};1H{}", r + 1, char::from(b'a' + r as u8).to_string().repeat(20)))
        .collect()
}

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(ROWS, COLS, 100);
    parser.process(fill().as_bytes());
    parser.process(bytes.as_bytes());
    parser
}

/// One letter a cell: `.` default, `R` red background and nothing else, `U`
/// underline, `V` reverse, `F` red foreground, `?` anything else.
fn code(cell: &vt100::Cell) -> char {
    let red = cell.bgcolor() == vt100::Color::Idx(1);
    let plain_bg = cell.bgcolor() == vt100::Color::Default;
    let rest_default = cell.fgcolor() == vt100::Color::Default
        && !cell.underline()
        && !cell.inverse()
        && cell.hyperlink_id() == 0;
    match (rest_default, plain_bg, red) {
        (true, true, _) => '.',
        (true, false, true) => 'R',
        _ if cell.underline() => 'U',
        _ if cell.inverse() => 'V',
        _ if cell.fgcolor() == vt100::Color::Idx(1) => 'F',
        _ => '?',
    }
}

/// The rows that are not all default, as `row:codes` with the default tail
/// dropped, the form the E2E suite prints.
fn attrs(parser: &vt100::Parser) -> String {
    let mut out = Vec::new();
    for row in 0..ROWS {
        let codes: String = (0..COLS)
            .map(|col| code(parser.screen().cell(row, col).unwrap()))
            .collect();
        let trimmed = codes.trim_end_matches('.');
        if !trimmed.is_empty() {
            out.push(format!("{row}:{trimmed}"));
        }
    }
    if out.is_empty() { "none".to_string() } else { out.join(" ") }
}

fn text(parser: &vt100::Parser, row: u16) -> String {
    parser.screen().rows(0, COLS).nth(usize::from(row)).unwrap().trim_end().to_string()
}

fn red(n: usize) -> String {
    "R".repeat(n)
}

fn row_red(row: u16) -> String {
    format!("{row}:{}", red(40))
}

#[test]
fn line_feed_at_the_bottom_scrolls_in_the_background() {
    for (name, op) in [("LF", "\n"), ("IND", "\x1bD"), ("NEL", "\x1bE")] {
        let p = parse(&format!("\x1b[10;1H\x1b[41m{op}"));
        assert_eq!(attrs(&p), row_red(9), "{name}");
        assert_eq!(text(&p, 0), "b".repeat(20), "{name}");
        assert_eq!(p.screen().cursor_position(), (9, 0), "{name}");
    }
}

#[test]
fn reverse_index_at_the_top_scrolls_in_the_background() {
    let p = parse("\x1b[1;1H\x1b[41m\x1bM");
    assert_eq!(attrs(&p), row_red(0));
    assert_eq!(text(&p, 1), "a".repeat(20));
}

#[test]
fn scroll_up_and_down() {
    let p = parse("\x1b[41m\x1b[2S");
    assert_eq!(attrs(&p), format!("{} {}", row_red(8), row_red(9)));
    let p = parse("\x1b[41m\x1b[2T");
    assert_eq!(attrs(&p), format!("{} {}", row_red(0), row_red(1)));
}

#[test]
fn insert_and_delete_lines() {
    let p = parse("\x1b[5;1H\x1b[41m\x1b[2L");
    assert_eq!(attrs(&p), format!("{} {}", row_red(4), row_red(5)));
    assert_eq!(text(&p, 6), "e".repeat(20));
    let p = parse("\x1b[5;1H\x1b[41m\x1b[2M");
    assert_eq!(attrs(&p), format!("{} {}", row_red(8), row_red(9)));
    assert_eq!(text(&p, 4), "g".repeat(20));
}

#[test]
fn insert_and_delete_characters() {
    let p = parse("\x1b[5;11H\x1b[41m\x1b[3@");
    assert_eq!(attrs(&p), format!("4:{}RRR", ".".repeat(10)));
    assert_eq!(text(&p, 4), format!("{}   {}", "e".repeat(10), "e".repeat(10)));
    let p = parse("\x1b[5;11H\x1b[41m\x1b[3P");
    assert_eq!(attrs(&p), format!("4:{}RRR", ".".repeat(37)));
    assert_eq!(text(&p, 4), "e".repeat(17));
}

#[test]
fn delete_characters_past_the_end_fill_the_rest() {
    let p = parse("\x1b[5;31H\x1b[41m\x1b[20P");
    assert_eq!(attrs(&p), format!("4:{}{}", ".".repeat(30), red(10)));
}

#[test]
fn inside_a_scroll_region() {
    let p = parse("\x1b[3;6r\x1b[6;1H\x1b[41m\n");
    assert_eq!(attrs(&p), row_red(5));
    let p = parse("\x1b[3;6r\x1b[3;1H\x1b[41m\x1bM");
    assert_eq!(attrs(&p), row_red(2));
    let p = parse("\x1b[3;6r\x1b[41m\x1b[S");
    assert_eq!(attrs(&p), row_red(5));
    let p = parse("\x1b[3;6r\x1b[4;1H\x1b[41m\x1b[L");
    assert_eq!(attrs(&p), row_red(3));
    assert_eq!(text(&p, 6), "g".repeat(20));
}

#[test]
fn erases_keep_the_background() {
    let p = parse("\x1b[5;11H\x1b[41m\x1b[3X");
    assert_eq!(attrs(&p), format!("4:{}RRR", ".".repeat(10)));
    let p = parse("\x1b[5;11H\x1b[41m\x1b[K");
    assert_eq!(attrs(&p), format!("4:{}{}", ".".repeat(10), red(30)));
    let p = parse("\x1b[5;11H\x1b[41m\x1b[1K");
    assert_eq!(attrs(&p), format!("4:{}", red(11)));
    let p = parse("\x1b[5;11H\x1b[41m\x1b[2K");
    assert_eq!(attrs(&p), row_red(4));
    let p = parse("\x1b[5;11H\x1b[41m\x1b[1J");
    let mut want: Vec<String> = (0..4).map(row_red).collect();
    want.push(format!("4:{}", red(11)));
    assert_eq!(attrs(&p), want.join(" "));
}

#[test]
fn erases_drop_every_attribute_but_the_background() {
    for (name, sgr) in [("underline", "4"), ("reverse", "7"), ("foreground", "31")] {
        let p = parse(&format!("\x1b[5;11H\x1b[{sgr}m\x1b[K"));
        assert_eq!(attrs(&p), "none", "{name} EL");
        let p = parse(&format!("\x1b[5;11H\x1b[{sgr}m\x1b[3X"));
        assert_eq!(attrs(&p), "none", "{name} ECH");
        let p = parse(&format!("\x1b[5;11H\x1b[{sgr}m\x1b[2J"));
        assert_eq!(attrs(&p), "none", "{name} ED");
        let p = parse(&format!("\x1b[5;1H\x1b[{sgr}m\x1b[L"));
        assert_eq!(attrs(&p), "none", "{name} IL");
        let p = parse(&format!("\x1b[10;1H\x1b[{sgr}m\n"));
        assert_eq!(attrs(&p), "none", "{name} LF");
    }
    let p = parse("\x1b[5;11H\x1b[4;41m\x1b[K");
    assert_eq!(attrs(&p), format!("4:{}{}", ".".repeat(10), red(30)));
}

#[test]
fn erases_drop_the_link() {
    let p = parse("\x1b]8;;https://example.com\x1b\\\x1b[5;11H\x1b[K\x1b[5;1H\x1b[L");
    for col in 0..COLS {
        assert_eq!(p.screen().cell(4, col).unwrap().hyperlink_id(), 0, "IL col {col}");
        assert_eq!(p.screen().cell(5, col).unwrap().hyperlink_id(), 0, "EL col {col}");
    }
}

#[test]
fn the_bright_256_and_rgb_backgrounds_carry_over() {
    for (sgr, want) in [
        ("101", vt100::Color::Idx(9)),
        ("48;5;1", vt100::Color::Idx(1)),
        ("48;5;200", vt100::Color::Idx(200)),
        ("48;2;10;20;30", vt100::Color::Rgb(10, 20, 30)),
    ] {
        let p = parse(&format!("\x1b[5;1H\x1b[{sgr}m\x1b[L"));
        for col in 0..COLS {
            assert_eq!(p.screen().cell(4, col).unwrap().bgcolor(), want, "{sgr} col {col}");
        }
    }
}

// The hosts, not tmux: a wrap at the bottom scrolls in a line in the current
// background on both console hosts; tmux leaves it default
// (screen-write.c:2709), so only the character written there is red.
#[test]
fn a_wrap_at_the_bottom_scrolls_in_the_background_as_the_hosts_do() {
    let p = parse(&format!("\x1b[10;1H\x1b[41m{}", "x".repeat(41)));
    assert_eq!(attrs(&p), format!("{} {}", row_red(8), row_red(9)));
    assert_eq!(text(&p, 9), "x");
    assert_eq!(p.screen().cursor_position(), (9, 1));
}

#[test]
fn the_default_background_stays_the_default() {
    for op in ["\x1b[10;1H\n", "\x1b[5;1H\x1b[2L", "\x1b[5;11H\x1b[3P", "\x1b[2T"] {
        let p = parse(&format!("\x1b[41m\x1b[m{op}"));
        assert_eq!(attrs(&p), "none", "{op:?}");
    }
}

#[test]
fn the_line_scrolled_off_keeps_its_own_cells() {
    let mut p = parse("\x1b[1;1H\x1b[41m\x1b[2K\x1b[m\x1b[10;1H\n");
    p.screen_mut().set_scrollback(1);
    assert_eq!(attrs(&p), row_red(0));
    p.screen_mut().set_scrollback(0);
    assert_eq!(attrs(&p), "none");
}

#[test]
fn a_wide_character_pushed_past_the_edge_by_insert() {
    // Both console hosts drop it, and so does vt100; the inserted cell takes
    // the background.
    let p = parse("\x1b[5;1H\x1b[2K\x1b[5;39H\u{3042}\x1b[5;1H\x1b[41m\x1b[@");
    assert_eq!(text(&p, 4), "");
    assert_eq!(attrs(&p), "4:R");
}
