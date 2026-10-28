// A pane sixel decoded into indexed pixels.
//
// The size must come out as the pane laid the image out, since the client
// cuts and rescales by the cells the server reports for it: vt100's
// `sixel_layout` follows Windows Terminal's parser (a band is six rows of
// `aspect` pixels, 2:1 unless the introducer or a raster command says
// otherwise), and so does `decode`.
use super::{decode, SixelColour, MAX_SIDE};

// The two colour image of the 914 suite: 14 by 2 bands at 2:1.
const SAMPLE: &[u8] = b"\x1bPq#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@\x1b\\";

#[test]
fn the_sample_decodes_to_its_pixels() {
    let img = decode(SAMPLE).expect("a sixel");
    assert_eq!((img.width, img.height), (14, 24));
    assert_eq!(img.p2, 0);
    assert_eq!(
        img.palette,
        vec![
            None,
            Some(SixelColour { kind: 2, a: 100, b: 100, c: 0 }),
            Some(SixelColour { kind: 2, a: 0, b: 100, c: 0 }),
        ]
    );
    // `~` fills all six rows of the band, each row two pixels tall.
    assert_eq!(img.get(0, 0), 2);
    assert_eq!(img.get(0, 11), 2);
    // `@` is the top row only; the second colour's `}` fills the rest.
    assert_eq!(img.get(2, 0), 2);
    assert_eq!(img.get(2, 1), 2);
    assert_eq!(img.get(2, 2), 3);
    assert_eq!(img.get(2, 11), 3);
    // The second band: the top row only.
    assert_eq!(img.get(0, 12), 2);
    assert_eq!(img.get(0, 13), 2);
    assert_eq!(img.get(0, 14), 0);
    assert_eq!(img.get(13, 23), 0);
    assert_eq!(img.get(14, 0), 0, "outside the image");
}

/// The size vt100 gives the image the pane wrote, and the stored blob, which
/// is what the client receives and decodes.
fn laid_out(dcs: &[u8]) -> ((u32, u32), Vec<u8>) {
    let mut parser = vt100::Parser::new(30, 120, 0);
    parser.process(dcs);
    let img = &parser.screen().images()[0];
    ((img.px_width, img.px_height), img.raw.clone())
}

#[test]
fn the_size_is_the_size_the_pane_laid_out() {
    for dcs in [
        SAMPLE.to_vec(),
        // Aspect 1:1 from the introducer.
        b"\x1bP9q#1;2;100;0;0#1~~~~-~~\x1b\\".to_vec(),
        // A raster command that changes the aspect and is larger than the data.
        b"\x1bPq\"3;1;40;30#1;2;0;0;100#1!7~\x1b\\".to_vec(),
        // Repeats, carriage returns and a data band that draws nothing.
        b"\x1bP0;1q#3;2;50;50;50#3!20~$!5@-??-#3~\x1b\\".to_vec(),
    ] {
        let (size, stored) = laid_out(&dcs);
        let mine = decode(&dcs).expect("a sixel");
        assert_eq!((mine.width, mine.height), size, "{:?}", String::from_utf8_lossy(&dcs));
        let blob = decode(&stored).expect("the stored blob");
        assert_eq!((blob.width, blob.height), size, "stored {:?}", String::from_utf8_lossy(&stored));
        assert_eq!(blob.pixels, mine.pixels, "the stored blob draws the same pixels");
    }
}

#[test]
fn a_raster_command_sets_the_aspect() {
    // "3;1: each sixel row is three pixels tall.
    let img = decode(b"\x1bPq\"3;1#1;2;0;0;100#1@\x1b\\").unwrap();
    assert_eq!((img.width, img.height), (1, 18));
    assert_eq!((img.get(0, 0), img.get(0, 2), img.get(0, 3)), (2, 2, 0));
}

#[test]
fn the_background_select_is_kept() {
    assert_eq!(decode(b"\x1bP0;1q#1~\x1b\\").unwrap().p2, 1);
    assert_eq!(decode(b"\x1bP;2q#1~\x1b\\").unwrap().p2, 2);
}

#[test]
fn a_colour_selected_but_not_defined_is_a_register_without_a_definition() {
    let img = decode(b"\x1bPq#5~\x1b\\").unwrap();
    assert_eq!(img.palette.len(), 6);
    assert!(img.palette.iter().all(Option::is_none));
    assert_eq!(img.get(0, 0), 6);
}

#[test]
fn pixels_before_any_colour_use_register_0() {
    // Windows Terminal draws with register 0 until a `#` selects another.
    let img = decode(b"\x1bP9q~\x1b\\").unwrap();
    assert_eq!(img.get(0, 0), 1);
}

#[test]
fn a_later_column_overwrites_an_earlier_one() {
    let img = decode(b"\x1bP9q#1;2;100;0;0#2;2;0;100;0#1~~$#2@\x1b\\").unwrap();
    assert_eq!((img.get(0, 0), img.get(0, 1), img.get(1, 0)), (3, 2, 2));
}

#[test]
fn what_is_not_a_sixel_is_none() {
    assert_eq!(decode(b""), None);
    assert_eq!(decode(b"q~~"), None);
    assert_eq!(decode(b"\x1bP0;0"), None);
}

#[test]
fn a_huge_repeat_is_held_to_the_largest_side() {
    let img = decode(b"\x1bP9q#1!99999~\x1b\\").unwrap();
    assert_eq!(img.width, MAX_SIDE);
    assert_eq!(img.get(MAX_SIDE - 1, 0), 2);
}
