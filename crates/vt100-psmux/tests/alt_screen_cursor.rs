// The cursor across the alternate screen: DECSET 47 and 1049 keep it where
// it was, as tmux's screen_alternate_on (screen.c at tag 3.8) and both console
// hosts do; 1049 reset puts back the one 1049 saved, and 47 reset, which saves
// none, leaves it where the alternate screen had it. 1047 and 1048 are
// ignored, as both console hosts ignore them (measured through OpenConsole,
// which hands psmux the bytes as written).
//
// Before the fix the alternate grid's own cursor showed: 0,0 after
// `ESC[5;4H AB ESC[?1049h`, where tmux and the hosts have 5,4. 1049 set again
// on the alternate screen does nothing, and 1049 reset puts back a cursor only
// if 1049 saved one, as tmux's screen_alternate_on and screen_alternate_off.

use vt100_psmux as vt100;

const E: &str = "\x1b";

fn run(bytes: &str) -> vt100::Parser {
    let mut p = vt100::Parser::new(10, 40, 0);
    p.process(bytes.as_bytes());
    p
}

/// (col, row), as `#{cursor_x},#{cursor_y}` show it.
fn cursor(p: &vt100::Parser) -> (u16, u16) {
    let (row, col) = p.screen().cursor_position();
    (col, row)
}

fn row(p: &vt100::Parser, r: u16) -> String {
    p.screen().contents_between(r, 0, r, 40).trim_end().to_string()
}

#[test]
fn decset_1049_keeps_the_cursor() {
    let p = run(&format!("{E}[5;4HAB{E}[?1049h"));
    assert!(p.screen().alternate_screen());
    assert_eq!(cursor(&p), (5, 4));
    assert_eq!(row(&p, 4), "", "the alternate screen starts clear");
}

#[test]
fn decset_1049_keeps_the_column_on_the_first_row() {
    let p = run(&format!("{E}[4G{E}[?1049h"));
    assert_eq!(cursor(&p), (3, 0));
}

#[test]
fn decrst_1049_puts_back_the_saved_cursor() {
    let p = run(&format!("{E}[5;4HAB{E}[?1049h{E}[9;20Hx{E}[?1049l"));
    assert!(!p.screen().alternate_screen());
    assert_eq!(cursor(&p), (5, 4));
    assert_eq!(row(&p, 4), "   AB");
    assert_eq!(row(&p, 8), "");
}

#[test]
fn decset_47_keeps_the_cursor_and_clears() {
    let p = run(&format!("{E}[5;4HAB{E}[?47h"));
    assert!(p.screen().alternate_screen());
    assert_eq!(cursor(&p), (5, 4));
    assert_eq!(row(&p, 4), "");
    let p = run(&format!("{E}[4G{E}[?47h"));
    assert_eq!(cursor(&p), (3, 0));
}

#[test]
fn decrst_47_leaves_the_cursor_where_the_alternate_screen_had_it() {
    let p = run(&format!("{E}[5;4HAB{E}[?47h{E}[9;20Hx{E}[?47l"));
    assert!(!p.screen().alternate_screen());
    assert_eq!(cursor(&p), (20, 8));
    assert_eq!(row(&p, 4), "   AB");
}

#[test]
fn a_second_47_session_starts_clear_and_from_the_cursor() {
    let p = run(&format!(
        "{E}[?47hold{E}[?47l{E}[3;7H{E}[?47h"
    ));
    assert_eq!(cursor(&p), (6, 2));
    assert_eq!(row(&p, 0), "", "tmux clears the alternate screen for 47");
}

#[test]
fn the_wrap_pending_column_carries_over() {
    let forty = "0123456789".repeat(4);
    let p = run(&format!("{forty}{E}[?1049h"));
    assert_eq!(cursor(&p), (40, 0));
    let p = run(&format!("{forty}{E}[?1049hX"));
    assert_eq!(cursor(&p), (1, 1), "the pending wrap still wraps");
}

#[test]
fn origin_mode_on_the_alternate_screen_does_not_shift_the_carried_cursor() {
    let p = run(&format!("{E}[3;8r{E}[?6h{E}[6;10H{E}[?1049h"));
    // CUP 6 in origin mode from region top 2 is row 7.
    assert_eq!(cursor(&p), (9, 7));
}

#[test]
fn modes_1047_and_1048_are_ignored() {
    let p = run(&format!("{E}[5;4HAB{E}[?1047h"));
    assert!(!p.screen().alternate_screen());
    assert_eq!(cursor(&p), (5, 4));
    let p = run(&format!("{E}[5;4HAB{E}[?1048h{E}[9;20Hx{E}[?1048l"));
    assert_eq!(cursor(&p), (20, 8));
}

#[test]
fn decset_1049_on_the_alternate_screen_changes_nothing() {
    // tmux 3.8: `ESC[5;10H ESC[?1049h ESC[2;3H X ESC[?1049h` leaves 3,1 and the
    // X (screen_alternate_on returns at once). It used to save again, clear
    // and go to 0,0.
    let p = run(&format!("{E}[5;10H{E}[?1049h{E}[2;3HX{E}[?1049h"));
    assert!(p.screen().alternate_screen());
    assert_eq!(cursor(&p), (3, 1));
    assert_eq!(row(&p, 1), "  X");
    // And the cursor 1049 saved first still comes back.
    let p = run(&format!("{E}[5;10H{E}[?1049h{E}[2;3H{E}[?1049h{E}[?1049l"));
    assert_eq!(cursor(&p), (9, 4));
}

#[test]
fn decrst_1049_puts_back_only_a_cursor_1049_saved() {
    // tmux 3.8: entered with 47, which saves no cursor, 1049 reset leaves it
    // where the alternate screen had it (2,1); it used to go to 0,0.
    let p = run(&format!("{E}[5;10H{E}[?47h{E}[2;3H{E}[?1049l"));
    assert!(!p.screen().alternate_screen());
    assert_eq!(cursor(&p), (2, 1));
    // Off the alternate screen with nothing saved, it does nothing.
    let p = run(&format!("{E}[5;4H{E}[?1049l"));
    assert_eq!(cursor(&p), (3, 4));
    // A cursor an earlier 1049 saved stays saved after it, as tmux's
    // saved_cx: 3,4 in tmux 3.8.
    let p = run(&format!("{E}[5;4H{E}[?1049h{E}[?1049l{E}[2;3H{E}[?47h{E}[9;20H{E}[?1049l"));
    assert_eq!(cursor(&p), (3, 4));
}

#[test]
fn split_at_every_byte() {
    let bytes = format!("{E}[5;4HAB{E}[?1049h{E}[?47l{E}[?47h");
    let whole = cursor(&run(&bytes));
    for k in 0..bytes.len() {
        let mut p = vt100::Parser::new(10, 40, 0);
        p.process(&bytes.as_bytes()[..k]);
        p.process(&bytes.as_bytes()[k..]);
        assert_eq!(cursor(&p), whole, "split at {k}");
    }
}
