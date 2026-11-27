// `Screen::clear_history` and `Screen::reset_hyperlinks`, what psmux's
// clear-history and clear-history -H do: tmux's cmd-capture-pane.c frees the
// pane's history with grid_clear_history (grid.c, history only) and with -H
// calls screen_reset_hyperlinks (screen.c, hyperlinks_reset in hyperlinks.c),
// all at tag 3.8.

use vt100_psmux as vt100;

fn lines(n: usize) -> String {
    (1..=n).map(|i| format!("line{}\r\n", i)).collect()
}

fn parse(bytes: &str) -> vt100::Parser {
    let mut parser = vt100::Parser::new(10, 40, 100);
    parser.process(bytes.as_bytes());
    parser
}

/// An image two cells wide and one high.
const IMAGE: &str = "\x1bPq#0;2;100;0;0#0~~~~~~~~~~~~~~~~~~~~\x1b\\";

#[test]
fn only_the_history_goes() {
    let mut p = parse(&lines(30));
    let screen = p.screen().contents();
    let cursor = p.screen().cursor_position();
    assert!(p.screen().scrollback_filled() > 0);
    p.screen_mut().clear_history();
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert_eq!(p.screen().contents(), screen);
    assert_eq!(p.screen().cursor_position(), cursor);
}

#[test]
fn on_the_alternate_screen_it_is_the_normal_screens_history() {
    let mut p = parse(&format!("{}\x1b[?1049h\x1b[HALT\x1b[3;8r\x1b[5;10H", lines(30)));
    assert!(p.screen().scrollback_filled() > 0);
    p.screen_mut().clear_history();
    assert_eq!(p.screen().scrollback_filled(), 0);
    assert!(p.screen().alternate_screen());
    assert_eq!(p.screen().contents().trim_end(), "ALT");
    assert_eq!(p.screen().cursor_position(), (4, 9));
    assert_eq!(p.screen().scroll_region(), (2, 7));
    // Back on the normal screen, its visible rows are still there.
    p.process(b"\x1b[?1049l");
    assert!(p.screen().contents().contains("line30"));
    assert_eq!(p.screen().scrollback_filled(), 0);
}

#[test]
fn an_image_in_the_freed_history_goes_and_one_on_the_screen_stays() {
    let mut p = parse(&format!("{}\r\n{}{}\r\n", IMAGE, lines(15), IMAGE));
    assert_eq!(p.screen().images().len(), 2);
    let on_screen = p.screen().images()[1].anchor_line;
    p.screen_mut().clear_history();
    assert_eq!(p.screen().images().len(), 1);
    assert_eq!(p.screen().images()[0].anchor_line, on_screen);
}

#[test]
fn reset_hyperlinks_forgets_every_link_and_never_reuses_an_id() {
    let mut p = parse("\x1b]8;;https://example.com/a\x1b\\a\x1b]8;;\x1b\\ \x1b]8;;https://example.com/b\x1b\\b\x1b]8;;\x1b\\");
    let a = p.screen().cell(0, 0).expect("cell").hyperlink_id();
    let b = p.screen().cell(0, 2).expect("cell").hyperlink_id();
    assert_eq!(p.screen().hyperlink_uri(a), Some("https://example.com/a"));
    p.screen_mut().reset_hyperlinks();
    assert_eq!(p.screen().hyperlink_uri(a), None);
    assert_eq!(p.screen().hyperlink_uri(b), None);
    assert_eq!(p.screen().cell(0, 0).expect("cell").contents(), "a");
    p.process(b"\r\n\x1b]8;;https://example.com/a\x1b\\again\x1b]8;;\x1b\\");
    let again = p.screen().cell(1, 0).expect("cell").hyperlink_id();
    assert!(again != a && again != b, "a new id: {again}");
    assert_eq!(p.screen().hyperlink_uri(again), Some("https://example.com/a"));
    assert_eq!(p.screen().hyperlink_uri(a), None);
}

#[test]
fn clear_history_alone_keeps_the_links() {
    let mut p = parse(&format!("{}\x1b]8;;https://example.com/a\x1b\\a\x1b]8;;\x1b\\", lines(30)));
    let (row, _) = p.screen().cursor_position();
    let id = p.screen().cell(row, 0).expect("cell").hyperlink_id();
    p.screen_mut().clear_history();
    assert_eq!(p.screen().hyperlink_uri(id), Some("https://example.com/a"));
}
