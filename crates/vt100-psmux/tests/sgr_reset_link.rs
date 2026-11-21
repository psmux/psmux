// SGR 0 resets the graphic rendition and leaves an OSC 8 link alone.
//
// A link is not a rendition: OSC 8 starts and ends it, and a full reset
// (RIS) drops it. tmux keeps the link through SGR 0 (input.c:2415-2418 at
// tag 3.8, from 9e14c1f8 "SGR 0 should not end hyperlink"), and so do both
// console hosts. vt100 cleared the whole pen, link included, so the text of a
// link after a reset inside it lost the link. tmux 3.8 still drops the link
// for a bare `CSI m` (input.c:2387-2390); vte hands that over as `0` and it
// keeps the link here, as on both console hosts.

use vt100_psmux as vt100;

const OPEN: &[u8] = b"\x1b]8;;http://x/\x1b\\";
const CLOSE: &[u8] = b"\x1b]8;;\x1b\\";

fn parser_after(parts: &[&[u8]]) -> vt100::Parser {
    let mut parser = vt100::Parser::new(5, 80, 0);
    for part in parts {
        parser.process(part);
    }
    parser
}

/// The URI of the cell at column `col` of the first row, if any.
fn link_at(parser: &vt100::Parser, col: u16) -> Option<String> {
    let screen = parser.screen();
    let id = screen.cell(0, col).unwrap().hyperlink_id();
    if id == 0 {
        None
    } else {
        screen.hyperlink_uri(id).map(str::to_string)
    }
}

/// Links of the first `n` cells, `L` for linked and `.` for not.
fn links(parser: &vt100::Parser, n: u16) -> String {
    (0..n)
        .map(|col| if link_at(parser, col).is_some() { 'L' } else { '.' })
        .collect()
}

#[test]
fn every_form_of_sgr_0_keeps_the_link() {
    for reset in [
        &b"\x1b[0m"[..],
        b"\x1b[m",
        b"\x1b[;m",
        b"\x1b[00m",
        b"\x1b[1;0m",
        b"\x1b[0;1m",
        b"\x1b[0;0m",
    ] {
        let p = parser_after(&[OPEN, b"x", reset, b"y", CLOSE, b"z"]);
        assert_eq!(links(&p, 3), "LL.", "reset {reset:?}");
        assert_eq!(link_at(&p, 1).as_deref(), Some("http://x/"), "reset {reset:?}");
    }
}

#[test]
fn sgr_0_still_resets_every_rendition() {
    let p = parser_after(&[
        OPEN,
        b"\x1b[1;2;3;4;5;7;8;9;53;31;42;58;5;3mx\x1b[0my",
        CLOSE,
    ]);
    let x = p.screen().cell(0, 0).unwrap().clone();
    let y = p.screen().cell(0, 1).unwrap().clone();
    assert!(x.bold() && x.dim() && x.italic() && x.underline() && x.inverse());
    assert!(x.overline());
    assert!(!y.bold() && !y.dim() && !y.italic() && !y.underline());
    assert!(!y.blink() && !y.inverse() && !y.hidden() && !y.strikethrough());
    assert!(!y.overline());
    assert_eq!(y.fgcolor(), vt100::Color::Default);
    assert_eq!(y.bgcolor(), vt100::Color::Default);
    assert_eq!(y.underline_color(), vt100::Color::Default);
    assert_eq!(link_at(&p, 1).as_deref(), Some("http://x/"));
}

#[test]
fn sgr_0_followed_by_a_rendition_in_one_sequence() {
    let p = parser_after(&[OPEN, b"x\x1b[0;1my", CLOSE, b"z"]);
    assert_eq!(links(&p, 3), "LL.");
    assert!(p.screen().cell(0, 1).unwrap().bold());
    assert!(p.screen().cell(0, 2).unwrap().bold(), "OSC 8 leaves SGR alone too");
}

#[test]
fn bold_before_the_link_then_sgr_0() {
    let p = parser_after(&[b"\x1b[1m", OPEN, b"x\x1b[0my", CLOSE, b"z"]);
    assert_eq!(links(&p, 3), "LL.");
    assert!(p.screen().cell(0, 0).unwrap().bold());
    assert!(!p.screen().cell(0, 1).unwrap().bold());
}

#[test]
fn sgr_0_split_across_reads() {
    let p = parser_after(&[OPEN, b"x\x1b[", b"0", b"my", CLOSE, b"z"]);
    assert_eq!(links(&p, 3), "LL.");
    let p = parser_after(&[OPEN, b"x\x1b", b"[my", CLOSE, b"z"]);
    assert_eq!(links(&p, 3), "LL.");
}

#[test]
fn only_osc_8_or_ris_ends_the_link() {
    // OSC 8 with an empty URI, after an SGR 0.
    let p = parser_after(&[OPEN, b"x\x1b[0m", CLOSE, b"y"]);
    assert_eq!(links(&p, 2), "L.");
    // A new link replaces the old one through an SGR 0.
    let p = parser_after(&[OPEN, b"x\x1b[0m\x1b]8;;http://y/\x1b\\y\x1b[my", CLOSE]);
    assert_eq!(link_at(&p, 0).as_deref(), Some("http://x/"));
    assert_eq!(link_at(&p, 1).as_deref(), Some("http://y/"));
    assert_eq!(link_at(&p, 2).as_deref(), Some("http://y/"));
    // RIS drops it (input_reset_cell from INPUT_ESC_RIS, input.c:1409).
    let p = parser_after(&[OPEN, b"x\x1bcy"]);
    assert_eq!(links(&p, 1), ".");
}

#[test]
fn erase_after_sgr_0_in_a_link_leaves_no_link() {
    // An erase fills with the background alone (`Attrs::erased`).
    let p = parser_after(&[OPEN, b"xyz\x1b[0m\x1b[1;2H\x1b[K", CLOSE]);
    assert_eq!(links(&p, 3), "L..");
}

#[test]
fn saved_cursor_and_alternate_screen_keep_the_link_they_saved() {
    // DECSC saves the link with the pen, and DECRC brings it back after an
    // SGR 0 and an OSC 8 that ended it (input_save_state and
    // input_restore_state, input.c:849-871).
    let p = parser_after(&[OPEN, b"\x1b7x\x1b[0m", CLOSE, b"\x1b8y", CLOSE, b"z"]);
    assert_eq!(links(&p, 2), "L.");
    // Leaving the alternate screen brings back the pen it was entered with.
    let p = parser_after(&[OPEN, b"x\x1b[?1049h\x1b[0m", CLOSE, b"\x1b[?1049ly", CLOSE, b"z"]);
    assert_eq!(links(&p, 3), "LL.");
}
