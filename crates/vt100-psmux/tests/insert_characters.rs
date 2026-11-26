// ICH, `CSI Ps @`, with a count past the right edge inserts up to the edge,
// as tmux does (screen-write.c:1365 at tag 3.8). Each insert moved the rest of
// the row, so a large count cost time and nothing else.

use std::time::{Duration, Instant};
use vt100_psmux as vt100;

fn row(parser: &vt100::Parser) -> String {
    parser.screen().contents_between(0, 0, 0, parser.screen().size().1)
}

#[test]
fn a_count_past_the_edge_blanks_the_rest_of_the_row() {
    for (count, want) in [("3", "ab   cdefg"), ("8", "ab"), ("9", "ab"), ("65535", "ab")] {
        let mut parser = vt100::Parser::new(5, 10, 0);
        parser.process(format!("abcdefghij\x1b[1;3H\x1b[{count}@").as_bytes());
        assert_eq!(row(&parser), want, "CSI {count} @");
        assert_eq!(parser.screen().cursor_position(), (0, 2), "CSI {count} @");
    }
}

#[test]
fn a_count_past_the_edge_takes_no_longer_than_one_to_the_edge() {
    let mut parser = vt100::Parser::new(50, 200, 0);
    parser.process("x".repeat(200).as_bytes());
    // Before, each of these took about two seconds in a release build.
    let bytes = "\x1b[1;1H\x1b[65535@".repeat(20);
    let start = Instant::now();
    parser.process(bytes.as_bytes());
    let took = start.elapsed();
    assert!(took < Duration::from_secs(2), "20 x CSI 65535 @ took {took:?}");
    assert_eq!(row(&parser), "");
}
