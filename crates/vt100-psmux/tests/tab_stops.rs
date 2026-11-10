// Tab stops: HT (input.c:1331), ESC H / HTS (input.c:1420), CSI Ps g / TBC
// (input.c:1854), CSI Ps Z / CBT (input.c:1491) and CSI Ps ` / HPA
// (input.c:1784) at tag 3.8, and the table they share, which tmux keeps per
// screen and puts back to every eighth column when the width changes, on RIS
// and on a respawn (screen.c:190-202, :373, screen-write.c:427). vt100 kept no
// table: HT went to the next multiple of 8, and HTS, TBC, CBT and HPA did
// nothing. Every expectation is what tmux 3.8 showed for the same bytes in a
// 40x10 pane, as #{cursor_x} and #{pane_tabs}.

use vt100_psmux as vt100;

const COLS: u16 = 40;
const ROWS: u16 = 10;

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(ROWS, COLS, 0);
    parser.process(bytes.as_bytes());
    parser
}

fn stops(parser: &vt100::Parser) -> Vec<u16> {
    parser.screen().tab_stops().collect()
}

/// #{cursor_x} and #{pane_tabs} after `bytes`.
fn after(bytes: &str) -> (u16, Vec<u16>) {
    let parser = parse(bytes);
    (parser.screen().cursor_position().1, stops(&parser))
}

const DEFAULT: [u16; 4] = [8, 16, 24, 32];
const FORTY: &str = "0123456789012345678901234567890123456789";

#[test]
fn the_default_is_every_eighth_column() {
    assert_eq!(after(""), (0, DEFAULT.to_vec()));
    assert_eq!(stops(&vt100::Parser::new(ROWS, 8, 0)), Vec::<u16>::new());
    assert_eq!(stops(&vt100::Parser::new(ROWS, 9, 0)), vec![8]);
}

#[test]
fn ht_goes_to_the_next_stop_or_the_last_column() {
    assert_eq!(after("\t").0, 8);
    assert_eq!(after("\t\t\t").0, 24);
    assert_eq!(after("\x1b[39G\t").0, 39);
    assert_eq!(after("\x1b[38G\t").0, 39);
    // From the last column, nowhere.
    assert_eq!(after("\x1b[40G\t").0, 39);
}

#[test]
fn ht_from_past_the_last_column_stays() {
    // 40 characters leave the cursor waiting past the last column, which
    // tmux reports as 40; HT does not move it (cx >= sx - 1).
    assert_eq!(after(FORTY).0, 40);
    assert_eq!(after(&format!("{FORTY}\t")).0, 40);
}

#[test]
fn hts_sets_a_stop_at_the_cursor() {
    assert_eq!(after("\x1b[4G\x1bH"), (3, vec![3, 8, 16, 24, 32]));
    assert_eq!(after("\x1b[4G\x1bH\r\t"), (3, vec![3, 8, 16, 24, 32]));
    assert_eq!(after("\x1b[1G\x1bH"), (0, vec![0, 8, 16, 24, 32]));
    assert_eq!(after("\x1b[40G\x1bH"), (39, vec![8, 16, 24, 32, 39]));
}

#[test]
fn hts_waiting_past_the_last_column_sets_nothing() {
    assert_eq!(after(&format!("{FORTY}\x1bH")), (40, DEFAULT.to_vec()));
}

#[test]
fn tbc_clears_one_stop_or_all() {
    assert_eq!(after("\x1b[9G\x1b[g"), (8, vec![16, 24, 32]));
    assert_eq!(after("\x1b[17G\x1b[0g"), (16, vec![8, 24, 32]));
    assert_eq!(after("\x1b[6G\x1b[0g"), (5, DEFAULT.to_vec()));
    assert_eq!(after("\x1b[3g"), (0, vec![]));
    assert_eq!(after("\x1b[3g\r\t"), (39, vec![]));
}

#[test]
fn tbc_ignores_other_values() {
    assert_eq!(after("\x1b[2g"), (0, DEFAULT.to_vec()));
    assert_eq!(after("\x1b[9G\x1b[1g"), (8, DEFAULT.to_vec()));
}

#[test]
fn cbt_goes_back_count_stops() {
    assert_eq!(after("\x1b[21G\x1b[Z").0, 16);
    assert_eq!(after("\x1b[17G\x1b[Z").0, 8);
    assert_eq!(after("\x1b[21G\x1b[2Z").0, 8);
    // 0 counts as 1.
    assert_eq!(after("\x1b[21G\x1b[0Z").0, 16);
    assert_eq!(after("\x1b[21G\x1b[9Z").0, 0);
    assert_eq!(after("\x1b[1G\x1b[Z").0, 0);
    assert_eq!(after("\x1b[6G\x1b[Z").0, 0);
    assert_eq!(after("\x1b[40G\x1b[Z").0, 32);
    assert_eq!(after("\x1b[21G\x1b[65535Z").0, 0);
}

#[test]
fn cbt_from_past_the_last_column_starts_from_it() {
    assert_eq!(after(&format!("{FORTY}\x1b[Z")).0, 32);
}

#[test]
fn cbt_follows_the_table() {
    assert_eq!(after("\x1b[4G\x1bH\x1b[8G\x1b[Z"), (3, vec![3, 8, 16, 24, 32]));
    assert_eq!(after("\x1b[3g\x1b[21G\x1b[Z"), (0, vec![]));
}

#[test]
fn hpa_is_cha() {
    assert_eq!(after("\x1b[10`").0, 9);
    assert_eq!(after("\x1b[20G\x1b[`").0, 0);
    assert_eq!(after("\x1b[20G\x1b[0`").0, 0);
    assert_eq!(after("\x1b[200`").0, 39);
    let parser = parse("\x1b[5;3H\x1b[10`");
    assert_eq!(parser.screen().cursor_position(), (4, 9));
    assert_eq!(after("\x1b[10G").0, 9);
}

#[test]
fn ris_puts_the_default_back() {
    assert_eq!(after("\x1b[4G\x1bH\x1bc"), (0, DEFAULT.to_vec()));
}

#[test]
fn both_screens_share_the_table() {
    // Only the table here: where the cursor lands in the alternate screen is
    // another matter (tmux keeps column 3, vt100 starts at 0).
    assert_eq!(after("\x1b[4G\x1bH\x1b[?1049h").1, vec![3, 8, 16, 24, 32]);
    assert_eq!(after("\x1b[?1049h\x1b[6G\x1bH\x1b[?1049l"), (0, vec![5, 8, 16, 24, 32]));
}

#[test]
fn a_soft_reset_keeps_the_table() {
    // tmux has no DECSTR; CSI ! p changes nothing.
    assert_eq!(after("\x1b[4G\x1bH\x1b[!p").1, vec![3, 8, 16, 24, 32]);
}

#[test]
fn a_change_of_width_puts_the_default_back_and_height_alone_does_not() {
    let mut parser = parse("\x1b[4G\x1bH");
    parser.screen_mut().set_size(8, COLS);
    assert_eq!(stops(&parser), vec![3, 8, 16, 24, 32]);
    parser.screen_mut().set_size(8, 30);
    assert_eq!(stops(&parser), vec![8, 16, 24]);
}

#[test]
fn a_respawn_puts_the_default_back() {
    let mut parser = parse("\x1b[4G\x1bH");
    parser.screen_mut().reinit_keep_history();
    assert_eq!(stops(&parser), DEFAULT.to_vec());
}

#[test]
fn a_split_sequence_still_works() {
    let bytes = "\x1b[4G\x1bH\x1b[21G\x1b[2Z\x1b[3g";
    for at in 0..=bytes.len() {
        let mut parser = vt100::Parser::new(ROWS, COLS, 0);
        parser.process(&bytes.as_bytes()[..at]);
        parser.process(&bytes.as_bytes()[at..]);
        // HTS at 3, then from 20 back two stops, 16 and 8, then no stops.
        assert_eq!((parser.screen().cursor_position().1, stops(&parser)), (8, vec![]), "split at {at}");
    }
}
