// Indexed pixels written back out as a sixel, in the shape tmux's
// `sixel_print` writes (image-sixel.c:566-650 in tmux 3.8).
use super::encode;
use crate::sixel::decode::{decode, SixelColour, SixelPixels};

fn image(width: u32, height: u32, pixels: Vec<u16>) -> SixelPixels {
    SixelPixels {
        width,
        height,
        p2: 0,
        palette: vec![
            Some(SixelColour { kind: 2, a: 100, b: 0, c: 0 }),
            Some(SixelColour { kind: 1, a: 120, b: 50, c: 100 }),
        ],
        pixels,
    }
}

#[test]
fn one_colour_band() {
    let img = image(2, 6, vec![1; 12]);
    assert_eq!(
        encode(&img),
        b"\x1bP9;0q\"1;1;2;6#0;2;100;0;0#1;1;120;50;100#0~~\x1b\\".to_vec()
    );
}

#[test]
fn runs_of_more_than_three_repeat_and_the_empty_tail_goes() {
    let mut px = vec![0u16; 8];
    px[..5].fill(1);
    let img = image(8, 1, px);
    let out = String::from_utf8(encode(&img)).unwrap();
    assert!(out.ends_with("#0!5@\x1b\\"), "{:?}", out);

    let img = image(4, 1, vec![1, 1, 1, 0]);
    let out = String::from_utf8(encode(&img)).unwrap();
    assert!(out.ends_with("#0@@@\x1b\\"), "{:?}", out);
}

#[test]
fn colours_in_a_band_are_separated_by_carriage_returns_and_bands_by_new_lines() {
    // Row 0: colour 0 then colour 1; row 6 (the second band): colour 1.
    let mut px = vec![0u16; 2 * 7];
    px[0] = 1;
    px[1] = 2;
    px[12] = 2;
    let img = image(2, 7, px);
    let out = String::from_utf8(encode(&img)).unwrap();
    assert!(out.ends_with("#0@$#1?@-#1@\x1b\\"), "{:?}", out);
}

#[test]
fn the_background_select_is_written() {
    let mut img = image(1, 1, vec![1]);
    img.p2 = 1;
    assert!(encode(&img).starts_with(b"\x1bP9;1q\"1;1;1;1"));
}

#[test]
fn an_undefined_register_is_selected_but_not_defined() {
    let img = SixelPixels { width: 1, height: 1, p2: 0, palette: vec![None, None, None], pixels: vec![3] };
    assert_eq!(encode(&img), b"\x1bP9;0q\"1;1;1;1#2@\x1b\\".to_vec());
}

#[test]
fn decoding_what_was_encoded_gives_the_same_image() {
    let sample = b"\x1bPq#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@\x1b\\";
    let img = decode(sample).unwrap();
    let again = decode(&encode(&img)).expect("the encoded image decodes");
    assert_eq!(again, img);
}
