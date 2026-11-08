// `scroll-on-clear` (options-table.c:1723 at tag 3.8, default on): a clear of
// the whole screen sends what it showed to history first, through
// grid_view_clear_history. tmux does that for CSI 2 J (screen-write.c:2093),
// CSI 0 J from the top left corner (screen-write.c:1941) and RIS
// (screen-write.c:435), never in the alternate screen. vt100 only blanked the
// screen, and RIS threw the history away as well. Every expectation is what
// tmux 3.8 showed for the same bytes in a 40x10 pane, unless a comment says
// the console hosts are followed instead.

use vt100_psmux as vt100;

const COLS: u16 = 40;
const ROWS: u16 = 10;

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line{}\r\n", i)).collect()
}

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(ROWS, COLS, 100);
    parser.process(bytes.as_bytes());
    parser
}

fn off(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(ROWS, COLS, 100);
    parser.screen_mut().set_scroll_on_clear(false);
    parser.process(bytes.as_bytes());
    parser
}

/// The history, oldest first, then the view back at the live screen.
fn history(parser: &mut vt100::Parser) -> Vec<String> {
    let filled = parser.screen().scrollback_filled();
    let mut out = Vec::new();
    for offset in (1..=filled).rev() {
        parser.screen_mut().set_scrollback(offset);
        out.push(parser.screen().rows(0, COLS).next().unwrap_or_default().trim_end().to_string());
    }
    parser.screen_mut().set_scrollback(0);
    out
}

/// The screen's rows, trailing blank rows dropped.
fn screen(parser: &vt100::Parser) -> Vec<String> {
    let mut rows: Vec<String> =
        parser.screen().rows(0, COLS).map(|r| r.trim_end().to_string()).collect();
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows
}

fn want(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("line{}", i)).collect()
}

#[test]
fn erase_display_2_sends_the_used_rows_to_history() {
    let mut p = parse(&format!("{}\x1b[2J", lines(5)));
    assert_eq!(history(&mut p), want(5));
    assert!(screen(&p).is_empty());
    // The cursor stays where it was.
    assert_eq!(p.screen().cursor_position(), (5, 0));
}

#[test]
fn history_already_there_stays_below_the_screen() {
    let mut p = parse(&format!("{}\x1b[2J", lines(15)));
    assert_eq!(history(&mut p), want(15));
    assert_eq!(p.screen().cursor_position(), (9, 0));
}

#[test]
fn erase_display_0_from_the_top_left_corner_does_too() {
    for form in ["\x1b[H\x1b[J", "\x1b[H\x1b[0J", "\x1b[1;1H\x1b[J"] {
        let mut p = parse(&format!("{}{}", lines(5), form));
        assert_eq!(history(&mut p), want(5), "{:?}", form);
        assert!(screen(&p).is_empty());
        assert_eq!(p.screen().cursor_position(), (0, 0));
    }
}

#[test]
fn a_partial_erase_sends_nothing() {
    let mut p = parse(&format!("{}\x1b[3;1H\x1b[J", lines(5)));
    assert_eq!(history(&mut p), Vec::<String>::new());
    assert_eq!(screen(&p), want(2));

    let mut p = parse(&format!("{}\x1b[1;3H\x1b[J", lines(5)));
    assert_eq!(history(&mut p), Vec::<String>::new());
    assert_eq!(screen(&p), vec!["li"]);

    let mut p = parse(&format!("{}\x1b[1J", lines(5)));
    assert_eq!(history(&mut p), Vec::<String>::new());
    assert!(screen(&p).is_empty());
}

#[test]
fn a_screen_with_no_text_sends_nothing() {
    for bytes in [
        "\x1b[2J".to_string(),
        format!("{}\x1b[2J\x1b[2J", lines(5)),
        "\r\n\r\n\r\n\x1b[2J".to_string(),
        // A background painted with erase is not text.
        "\x1b[41m\x1b[K\r\n\x1b[K\x1b[0m\x1b[2J".to_string(),
    ] {
        let p = parse(&bytes);
        let expected = if bytes.starts_with("line") { 5 } else { 0 };
        assert_eq!(p.screen().scrollback_filled(), expected, "{:?}", bytes);
    }
}

#[test]
fn blank_rows_between_used_ones_go_too_and_written_spaces_count() {
    let mut p = parse("a\r\n\r\n\r\nb\r\n\x1b[2J");
    assert_eq!(history(&mut p), vec!["a", "", "", "b"]);
    let p = parse("     \r\n     \r\n\x1b[2J");
    assert_eq!(p.screen().scrollback_filled(), 2);
}

#[test]
fn a_wrapped_row_and_its_colours_go_as_they_were() {
    let mut p = parse(&format!("{}\r\n\x1b[2J", "x".repeat(50)));
    assert_eq!(p.screen().scrollback_filled(), 2);
    p.screen_mut().set_scrollback(2);
    assert!(p.screen().row_wrapped(0));
    assert!(!p.screen().row_wrapped(1));

    let mut p = parse("\x1b[31mred\x1b[0m\r\n\x1b[2J");
    p.screen_mut().set_scrollback(1);
    assert_eq!(p.screen().cell(0, 0).unwrap().fgcolor(), vt100::Color::Idx(1));
}

#[test]
fn the_screen_is_cleared_with_the_current_background() {
    let p = parse("text\x1b[41m\x1b[2J");
    assert_eq!(p.screen().cell(0, 0).unwrap().bgcolor(), vt100::Color::Idx(1));
    assert_eq!(p.screen().cell(9, 39).unwrap().bgcolor(), vt100::Color::Idx(1));
}

#[test]
fn the_scroll_region_plays_no_part() {
    let mut p = parse(&format!("{}\x1b[2;4r\x1b[2J", lines(5)));
    assert_eq!(history(&mut p), want(5));
}

#[test]
fn erase_display_3_clears_the_history_after_it() {
    let p = parse(&format!("{}\x1b[2J\x1b[3J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert!(screen(&p).is_empty());
    // `clear`, on a terminal with E3.
    let p = parse(&format!("{}\x1b[H\x1b[2J\x1b[3J", lines(15)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    let p = parse(&format!("{}\x1b[H\x1b[J\x1b[3J", lines(15)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    // Alone it leaves the screen.
    let p = parse(&format!("{}\x1b[3J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert_eq!(screen(&p), want(5));
}

#[test]
fn ris_keeps_the_history_and_sends_the_screen_to_it() {
    let mut p = parse(&format!("{}\x1bc", lines(5)));
    assert_eq!(history(&mut p), want(5));
    assert!(screen(&p).is_empty());
    assert_eq!(p.screen().cursor_position(), (0, 0));

    let mut p = parse(&format!("{}\x1bc", lines(15)));
    assert_eq!(history(&mut p), want(15));
}

#[test]
fn the_history_a_ris_keeps_keeps_its_hyperlinks() {
    let mut p = parse("\x1b]8;;https://example.com/a\x07link\x1b]8;;\x07\r\n\x1bc");
    assert_eq!(p.screen().scrollback_filled(), 1);
    p.screen_mut().set_scrollback(1);
    let id = p.screen().cell(0, 0).unwrap().hyperlink_id();
    assert_ne!(id, 0);
    assert_eq!(p.screen().hyperlink_uri(id), Some("https://example.com/a"));
}

#[test]
fn off_sends_nothing_and_a_ris_still_keeps_the_history() {
    for bytes in [
        format!("{}\x1b[2J", lines(5)),
        format!("{}\x1b[H\x1b[J", lines(5)),
        format!("{}\x1bc", lines(5)),
    ] {
        let p = off(&bytes);
        assert_eq!(p.screen().scrollback_filled(), 0, "{:?}", bytes);
        assert!(screen(&p).is_empty());
    }
    let mut p = off(&format!("{}\x1bc", lines(15)));
    assert_eq!(history(&mut p), want(6));
    assert!(screen(&p).is_empty());
}

#[test]
fn a_ris_keeps_the_option() {
    let p = off(&format!("\x1bc{}\x1b[2J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
}

#[test]
fn it_can_be_turned_on_again() {
    let mut p = off(&format!("{}\x1b[2J", lines(5)));
    p.screen_mut().set_scroll_on_clear(true);
    p.process(format!("{}\x1b[2J", lines(1)).as_bytes());
    assert_eq!(p.screen().scrollback_filled(), 6);
}

#[test]
fn the_alternate_screen_sends_nothing() {
    let p = parse(&format!("\x1b[?1049h{}\x1b[2J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert!(p.screen().alternate_screen());

    let p = parse(&format!("\x1b[?1049h{}\x1b[H\x1b[J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);

    let p = parse(&format!("{}\x1b[?1049h{}\x1b[2J\x1b[?1049l", lines(2), lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert_eq!(screen(&p), want(2));
}

#[test]
fn a_ris_in_the_alternate_screen_leaves_it_and_keeps_the_normal_rows() {
    // tmux stays in the alternate screen and keeps the normal one. Both
    // console hosts leave it and blank the normal one, so psmux follows them,
    // and the rows would be lost if they did not go to history.
    let mut p = parse(&format!("{}\x1b[?1049hinalt\r\n\x1bc", lines(5)));
    assert!(!p.screen().alternate_screen());
    assert_eq!(history(&mut p), want(5));
    assert!(screen(&p).is_empty());
}

#[test]
fn selective_erase_sends_nothing() {
    let p = parse(&format!("{}\x1b[?2J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    let p = parse(&format!("{}\x1b[H\x1b[?J", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
}

#[test]
fn deccolm_leaves_the_screen_as_the_console_hosts_do() {
    // tmux clears the screen for CSI ? 3 h and l. The inbox conhost neither
    // passes it on nor changes its screen, so this keeps in step with it.
    let p = parse(&format!("{}\x1b[?3h", lines(5)));
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert_eq!(screen(&p), want(5));
}

#[test]
fn the_history_limit_holds() {
    let mut p = vt100::Parser::new(ROWS, COLS, 3);
    p.process(format!("{}\x1b[2J", lines(5)).as_bytes());
    assert_eq!(history(&mut p), vec!["line3", "line4", "line5"]);

    let mut p = vt100::Parser::new(ROWS, COLS, 0);
    p.process(format!("{}\x1b[2J", lines(5)).as_bytes());
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert!(screen(&p).is_empty());
    assert_eq!(p.screen().cursor_position(), (5, 0));
}

#[test]
fn a_frozen_view_stays_on_what_it_showed() {
    // Copy mode keeps its place while rows join the history under it.
    let mut p = parse(&lines(5));
    p.screen_mut().set_frozen(true);
    p.process(b"\x1b[2J");
    assert_eq!(p.screen().scrollback(), 5);
    assert_eq!(screen(&p), want(5));

    let mut p = parse(&lines(15));
    p.screen_mut().set_frozen(true);
    p.process(b"\x1bc");
    assert_eq!(p.screen().scrollback(), 9);
    assert_eq!(screen(&p).first().map(String::as_str), Some("line7"));
}

/// An image two cells wide and one high.
const IMAGE: &str = "\x1bPq#0;2;100;0;0#0~~~~~~~~~~~~~~~~~~~~\x1b\\";

// Only the text goes to history. A clear leaves no image on the screen, which is
// what tmux does too (image_free_all, screen-write.c at tag 3.8).
#[test]
fn an_image_on_the_screen_does_not_go_with_the_text() {
    for clear in ["\x1b[2J", "\x1b[H\x1b[J", "\x1bc"] {
        let mut p = parse(&format!("before\r\n{}\r\nafter\r\n", IMAGE));
        assert_eq!(p.screen().images().len(), 1, "{:?}", clear);
        p.process(clear.as_bytes());
        assert!(p.screen().images().is_empty(), "{:?}", clear);
        let kept = history(&mut p);
        assert_eq!(kept.first().map(String::as_str), Some("before"), "{:?}", clear);
        assert!(kept.iter().any(|row| row == "after"), "{:?}", clear);
    }
}

#[test]
fn a_screen_holding_only_an_image_sends_nothing() {
    for clear in ["\x1b[2J", "\x1b[H\x1b[J", "\x1bc"] {
        let mut p = parse(IMAGE);
        p.process(clear.as_bytes());
        assert_eq!(p.screen().scrollback_filled(), 0, "{:?}", clear);
    }
}

// An image already in history: CSI 2 J and RIS drop it as they always have, and
// a clear from the corner touches only the screen's rows.
#[test]
fn an_image_in_history_goes_with_a_full_clear_but_not_with_one_from_the_corner() {
    for (clear, left) in [("\x1b[2J", 0), ("\x1bc", 0), ("\x1b[H\x1b[J", 1)] {
        let mut p = parse(&format!("{}\r\n{}", IMAGE, lines(15)));
        assert_eq!(p.screen().images().len(), 1, "{:?}", clear);
        let anchor = p.screen().images()[0].anchor_line;
        p.process(clear.as_bytes());
        assert_eq!(p.screen().images().len(), left, "{:?}", clear);
        if left == 1 {
            assert_eq!(p.screen().images()[0].anchor_line, anchor, "{:?}", clear);
        }
    }
}

#[test]
fn text_after_the_clear_lands_where_the_cursor_is() {
    let p = parse(&format!("{}\x1b[2Jafter", lines(5)));
    assert_eq!(screen(&p), vec!["", "", "", "", "", "after"]);
    assert_eq!(p.screen().cursor_position(), (5, 5));
    let p = parse(&format!("{}\x1b[H\x1b[Jafter", lines(5)));
    assert_eq!(screen(&p), vec!["after"]);
}
