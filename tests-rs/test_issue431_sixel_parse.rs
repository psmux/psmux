// Issue #431 (M2 parser DCS handling): the vte DCS hook/put/unhook path must
// capture a sixel graphics DCS (`ESC P ... q ... ST`) into one SixelImage on the
// Screen, leave the surrounding text intact, leave the cursor where the pane's
// console host leaves it, and ignore every NON-sixel DCS (e.g. DECRQSS) so the
// historical no-op behaviour is preserved.
//
// The host is OpenConsole, which lays sixel out with Windows Terminal's
// SixelParser: 10 by 20 pixel cells, a 2:1 pixel aspect ratio unless the
// introducer or a raster command says otherwise, and the cursor left in its
// column on the row holding the top of the last sixel band.
use crate::Parser;

// A real, self-contained sixel: raster `"1;1;24;12` (24x12 px) then two bands.
const SIXEL: &[u8] =
    b"\x1bP0;0;0q\"1;1;24;12#0;2;100;0;0#0!24~$-!24~$-\x1b\\";

#[test]
fn sixel_dcs_becomes_one_image_and_keeps_the_cursor_on_its_last_band() {
    let mut p: Parser = Parser::new(24, 80, 100);

    // Text before the image on its own line.
    p.process(b"BEFORE\r\n");
    let (row_before, col_before) = p.screen().cursor_position();

    // Feed the sixel; it must materialise exactly one image.
    p.process(SIXEL);
    assert_eq!(p.screen().images().len(), 1, "sixel DCS -> one image");

    let img = &p.screen().images()[0];
    // Raw blob round-trips as ESC P ... ST.
    assert!(img.raw.starts_with(b"\x1bP"), "raw starts with ESC P");
    assert!(img.raw.ends_with(b"\x1b\\"), "raw ends with ST");
    // Raster `"1;1;24;12` => 24x12 px, cells ceil(24/10)=3 x ceil(12/20)=1.
    assert_eq!(img.px_width, 24);
    assert_eq!(img.px_height, 12);
    assert_eq!(img.cell_width, 3);
    assert_eq!(img.cell_height, 1);

    // Two 6 pixel bands, then a new line to y=12, still inside the first
    // 20 pixel row: the cursor has not moved.
    assert_eq!(p.screen().cursor_position(), (row_before, col_before));

    // Text after the image still lands and the screen keeps both labels.
    p.process(b"\r\nAFTER");
    let text = p.screen().contents();
    assert!(text.contains("BEFORE"), "BEFORE survived the sixel");
    assert!(text.contains("AFTER"), "AFTER printed below the sixel");
    assert_eq!(p.screen().cursor_position().0, row_before + 1);

    // Still exactly one image after the trailing text.
    assert_eq!(p.screen().images().len(), 1);
}

#[test]
fn decrqss_dcs_produces_no_image_and_no_panic() {
    // DECRQSS: ESC P $ q m ESC \  — final byte 'q' but with the '$' intermediate.
    // Must be ignored entirely (no image, no state change, no panic).
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(b"HELLO");
    p.process(b"\x1bP$qm\x1b\\");
    assert_eq!(
        p.screen().images().len(),
        0,
        "DECRQSS (non-sixel DCS) must not create an image",
    );
    // Surrounding text is unaffected.
    p.process(b"WORLD");
    let text = p.screen().contents();
    assert!(text.contains("HELLO"));
    assert!(text.contains("WORLD"));
}

#[test]
fn sixel_without_raster_falls_back_to_scanned_dims() {
    // No `"` raster command: dimensions come from the data scan. Two bands of
    // 4 sixel columns each at the default 2:1 aspect ratio => width 4, height
    // 24 (2 bands x 12).
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(b"\x1bP0;0;0q#0~~~~$-~~~~\x1b\\");
    assert_eq!(p.screen().images().len(), 1);
    let img = &p.screen().images()[0];
    assert_eq!(img.px_width, 4, "widest band = 4 sixel columns");
    assert_eq!(img.px_height, 24, "2 bands x 12 px");
    // cells: ceil(4/10)=1 x ceil(24/20)=2.
    assert_eq!(img.cell_width, 1);
    assert_eq!(img.cell_height, 2);
}

#[test]
fn the_aspect_ratio_follows_the_first_introducer_parameter() {
    // 7 to 9 mean 1:1, so two bands are 12 pixels tall.
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(b"\x1bP9q~~$-~~\x1b\\");
    assert_eq!(p.screen().images()[0].px_height, 12);
    // An omitted parameter is 2:1, as 0 is.  Drawn in the same place with a
    // filled background, it hides the first image, which is dropped.
    p.process(b"\x1bPq~~$-~~\x1b\\");
    assert_eq!(p.screen().images().last().unwrap().px_height, 24);
}

#[test]
fn the_cursor_stays_in_the_column_the_image_started_in() {
    // What OpenConsole sends for `[Console]::Out.Write(<sample.sixel>)`: the
    // prompt line, the sixel at column 0, then CR LF and the next prompt.
    // Before this followed the host, the next prompt landed a row too low
    // and the host's later cursor moves wrote over it.
    let mut p: Parser = Parser::new(10, 40, 100);
    p.process(b"PS> \x1bPq#1~~@@$#2??}}-#1@@@@\x1b\\");
    assert_eq!(p.screen().cursor_position(), (0, 4), "same row, same column");
    p.process(b"\r\nPS> ");
    assert_eq!(p.screen().cursor_position(), (1, 4));
    let img = &p.screen().images()[0];
    assert_eq!((img.px_width, img.px_height), (4, 24));
    assert_eq!((img.cell_width, img.cell_height), (1, 2));
    assert_eq!(img.anchor_col, 4);
}

#[test]
fn the_stored_blob_bounds_the_background_to_the_image() {
    // Background select 0 with no raster size makes Windows Terminal fill
    // from the image to the bottom right of the page.  The stored blob gets
    // a raster command with the image's own size and the introducer's 2:1
    // ratio, so drawing it again leaves the rows below alone.
    let mut p: Parser = Parser::new(10, 40, 100);
    p.process(b"\x1bPq#1~~@@$#2??}}-#1@@@@\x1b\\");
    let raw = &p.screen().images()[0].raw;
    assert!(
        raw.starts_with(b"\x1bP0q\"2;1;4;24#1~~@@"),
        "{}",
        String::from_utf8_lossy(raw),
    );
    assert!(raw.ends_with(b"#1@@@@\x1b\\"));

    // A raster of the image's own comes after, so its ratio still wins, and
    // the size is the larger of the data and the raster.
    p.process(b"\x1bP9q\"1;1;30;40~~\x1b\\");
    let raw = &p.screen().images().last().unwrap().raw;
    assert!(raw.starts_with(b"\x1bP9q\"1;1;30;40\"1;1;30;40~~"));
}

#[test]
fn a_trailing_new_line_moves_the_cursor_to_the_row_it_reaches() {
    // 1:1 raster, four 6 pixel bands each ended by `-`: the image cursor
    // ends at y=24, which is in the second 20 pixel row.
    let mut p: Parser = Parser::new(10, 40, 100);
    p.process(b"\x1bPq\"1;1;2;24~~-~~-~~-~~-\x1b\\");
    assert_eq!(p.screen().cursor_position(), (1, 0));
    assert_eq!(p.screen().images()[0].px_height, 24);
}

#[test]
fn an_image_at_the_bottom_scrolls_the_text_up_first() {
    // Five rows, cursor on the last one: 20 pixels of room.  Five 1:1 bands
    // need 30.  The fourth new line finds 2 pixels left, scrolls one line
    // and moves on to y=24, so the cursor ends one row below the image's
    // top, which the scroll has moved up a row: back on the last row.
    let mut p: Parser = Parser::new(5, 40, 100);
    p.process(b"TOP\r\n\r\n\r\n\r\n");
    assert_eq!(p.screen().cursor_position(), (4, 0));
    p.process(b"\x1bPq\"1;1;2;30~~-~~-~~-~~-~~\x1b\\");
    assert_eq!(p.screen().cursor_position(), (4, 0));
    assert!(
        !p.screen().contents().contains("TOP"),
        "the first row scrolled into history",
    );
    assert_eq!(p.screen().images().len(), 1);
}

// A sixel DCS is held at most tmux's input-buffer-size default (1 MiB,
// INPUT_BUF_DEFAULT_SIZE) and thrown away past it, as tmux input_input does.
// Before the limit `ESC P q` with no ST grew the server by twice what the pane
// wrote: 64 MiB written took the server from 4 MB to 134 MB.

fn sixel_with_body(len: usize) -> Vec<u8> {
    let mut v = b"\x1bPq#0;2;100;0;0#0".to_vec();
    v.extend(std::iter::repeat(b'~').take(len));
    v.extend_from_slice(b"\x1b\\");
    v
}

#[test]
fn a_sixel_over_the_input_buffer_size_is_discarded() {
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(&sixel_with_body(2 << 20));
    assert_eq!(p.screen().images().len(), 0, "an oversized sixel is not kept");
    p.process(b"AFTER");
    assert!(p.screen().contents().contains("AFTER"), "text after it still lands");
}

#[test]
fn an_unterminated_sixel_does_not_grow_without_bound() {
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(b"\x1bPq#0;2;100;0;0#0");
    let chunk = vec![b'~'; 1 << 20];
    for _ in 0..8 {
        p.process(&chunk);
    }
    // The ESC that ends it, then text.
    p.process(b"\x1b[0mAFTER");
    assert_eq!(p.screen().images().len(), 0);
    assert!(p.screen().contents().contains("AFTER"));
}

#[test]
fn a_sixel_under_the_limit_is_kept_and_the_next_one_after_a_discard_too() {
    let mut p: Parser = Parser::new(24, 80, 100);
    p.process(&sixel_with_body(2 << 20));
    p.process(&sixel_with_body((1 << 20) - 64));
    assert_eq!(p.screen().images().len(), 1, "the image under the limit is kept");
}
