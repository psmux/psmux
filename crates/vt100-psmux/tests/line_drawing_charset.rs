// The DEC Special Graphics set, the line drawing set: `ESC ( 0` into G0,
// `ESC ) 0` into G1, `ESC ( B` / `ESC ) B` for ASCII, SI and SO to choose
// (input.c:241-249, :1372-1377, :1442-1453 at tag 3.8). vt100 ignored them,
// so on the OpenConsole route `ESC(0 lqqk` stayed `lqqk` while the console
// host held `┌──┐`.
//
// Every expectation is what the Windows console hosts hold for the same
// bytes, measured on the inbox conhost and in OpenConsole's own buffer. tmux
// keeps the letter and draws the box character only on output, and its table
// also turns `+ , - . 0` into arrows and a block and `` ` `` into ◆; the
// hosts keep `+ , - . 0`, give ♦ for `` ` `` and a blank for `_`, and psmux
// follows the host (Discussion #937).

use vt100_psmux as vt100;

/// Row 0 and the cursor column.
fn after(bytes: &str) -> (String, u16) {
    let mut parser = vt100::Parser::new(5, 40, 0);
    parser.process(bytes.as_bytes());
    let screen = parser.screen();
    let row = screen.rows(0, 40).next().unwrap_or_default().trim_end().to_string();
    (row, screen.cursor_position().1)
}

fn want(row: &str, col: u16) -> (String, u16) {
    (row.to_string(), col)
}

#[test]
fn g0_draws_lines_until_ascii_again() {
    assert_eq!(after("\x1b(0lqqk\x1b(B lqqk"), want("\u{250C}\u{2500}\u{2500}\u{2510} lqqk", 9));
}

#[test]
fn the_whole_set_is_the_console_hosts_table() {
    assert_eq!(
        after("\x1b(0`abcdefghijklm\x1b(B"),
        want("\u{2666}\u{2592}\u{2409}\u{240C}\u{240D}\u{240A}\u{00B0}\u{00B1}\u{2424}\u{240B}\u{2518}\u{2510}\u{250C}\u{2514}", 14)
    );
    assert_eq!(
        after("\x1b(0nopqrstuvwxyz{|}~\x1b(B"),
        want("\u{253C}\u{23BA}\u{23BB}\u{2500}\u{23BC}\u{23BD}\u{251C}\u{2524}\u{2534}\u{252C}\u{2502}\u{2264}\u{2265}\u{03C0}\u{2260}\u{00A3}\u{00B7}", 17)
    );
    // `_` is a blank.
    assert_eq!(after("\x1b(0a_a\x1b(B"), want("\u{2592} \u{2592}", 3));
}

#[test]
fn what_is_not_in_the_set_stays() {
    // The hosts keep these, where tmux's table has arrows and a block.
    assert_eq!(after("\x1b(0+,-.0ABC^]\x1b(B"), want("+,-.0ABC^]", 10));
    // A wide character goes through.
    assert_eq!(after("\x1b(0q\u{3042}q\x1b(B"), want("\u{2500}\u{3042}\u{2500}", 4));
}

#[test]
fn g1_with_shift_out_and_shift_in() {
    assert_eq!(after("\x1b)0\x0elqqk\x0flqqk"), want("\u{250C}\u{2500}\u{2500}\u{2510}lqqk", 8));
    // With the line drawing set in G0, SO chooses G1, ASCII, and SI G0.
    assert_eq!(after("\x1b(0\x0eq\x0fq\x1b(B"), want("q\u{2500}", 2));
    assert_eq!(after("\x1b)0\x1b)B\x0eq"), want("q", 1));
}

#[test]
fn rep_repeats_the_letter_in_the_set_in_use() {
    assert_eq!(after("\x1b(0q\x1b[3b\x1b(B|"), want("\u{2500}\u{2500}\u{2500}\u{2500}|", 5));
}

#[test]
fn decsc_saves_the_sets_and_decrc_restores_them() {
    assert_eq!(after("\x1b(0\x1b7\x1b(B\x1b8q"), want("\u{2500}", 1));
    assert_eq!(after("\x1b7\x1b(0\x1b8q"), want("q", 1));
    assert_eq!(after("\x1b)0\x0e\x1b7\x0f\x1b8q"), want("\u{2500}", 1));
}

#[test]
fn the_alternate_screen_keeps_the_main_screens_sets() {
    // DECSET 1049 saves the cursor with the sets and DECRST 1049 restores
    // them, as the hosts show.
    assert_eq!(after("\x1b(0\x1b[?1049h\x1b(B\x1b[?1049lq"), want("\u{2500}", 1));
}

#[test]
fn ris_goes_back_to_ascii() {
    assert_eq!(after("\x1b(0\x1bcq"), want("q", 1));
    assert_eq!(after("\x1b)0\x0e\x1bcq"), want("q", 1));
}

#[test]
fn other_sets_stay_unhandled() {
    // The UK set: the hosts give `£` for `#`, tmux keeps `#`, so does psmux.
    assert_eq!(after("\x1b(A#\x1b(B"), want("#", 1));
    // And it leaves the line drawing set alone.
    assert_eq!(after("\x1b(0\x1b(Aq\x1b(B"), want("\u{2500}", 1));
}
