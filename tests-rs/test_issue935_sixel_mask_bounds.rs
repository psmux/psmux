// Bounds on the per cell pixel masks a sixel gets (#935).
//
// The masks are built from the image's size, and a raster command sets that
// size from a few bytes of pane output: `"1;1;4096;4096` asks for 410 by 205
// cells whatever the pane's size.  Masks sized that way cost 2.7 MB for a 34
// byte sixel, 172 MB for 64 of them in one pane, and 20 sixels of 33 bytes
// drawn over 63 such images took 19 s of the parser's time.
use crate::Parser;

const ESC: u8 = 0x1b;

/// `ESC [ H` then the sixel `ESC P <body> ESC \`.
fn sixel_at_home(body: &str) -> Vec<u8> {
    let mut v = vec![ESC, b'[', b'H', ESC, b'P'];
    v.extend_from_slice(body.as_bytes());
    v.extend_from_slice(&[ESC, b'\\']);
    v
}

fn mask_bytes(p: &Parser) -> usize {
    p.screen()
        .images()
        .iter()
        .map(|img| img.cell_masks.len() * std::mem::size_of::<crate::image::CellMask>())
        .sum()
}

#[test]
fn a_raster_far_past_the_pane_gets_no_masks() {
    let mut p = Parser::new(30, 120, 0);
    // An opaque 4096 by 4096 raster with one sixel drawn.
    p.process(&sixel_at_home("0;0;0q\"1;1;4096;4096#0~"));
    assert_eq!(p.screen().images().len(), 1);
    assert_eq!(mask_bytes(&p), 0, "masks for a few bytes of sixel");
}

#[test]
fn masks_stay_within_the_pane_for_many_images() {
    // 64 images, each one row lower, each with a huge raster: the masks of
    // every image together stay within what the pane's own cells need.
    let mut p = Parser::new(30, 120, 1000);
    for _ in 0..64 {
        let mut s = sixel_at_home("0;0;0q\"1;1;4096;4096#0~");
        s.drain(..3);
        s.extend_from_slice(b"\r\n");
        p.process(&s);
    }
    let per_pane = 30 * 120 * std::mem::size_of::<crate::image::CellMask>();
    assert!(mask_bytes(&p) <= 64 * per_pane, "{} bytes of masks", mask_bytes(&p));
}

#[test]
fn drawing_over_images_without_masks_is_cheap() {
    // 63 images too big to trace (no mask), then a run of transparent sixels
    // over them that set one pixel each: every one of them compares itself
    // with all 63.
    let mut p = Parser::new(30, 120, 0);
    for _ in 0..63 {
        p.process(&sixel_at_home("0;0;0q\"1;1;4097;4097#0~"));
    }
    let started = std::time::Instant::now();
    for _ in 0..20 {
        p.process(&sixel_at_home("0;1;0q\"1;1;4096;4096#0@"));
    }
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(2), "20 sixels of 33 bytes took {took:?}");
}

#[test]
fn drawing_over_pane_sized_images_without_masks_is_cheap() {
    // The same with images as big as the pane, which are compared cell by
    // cell: an old image with no mask is taken to set every pixel it reaches.
    let mut p = Parser::new(30, 120, 0);
    let (cw, ch) = crate::image::cell_dims_from_px(1200, 600);
    for _ in 0..63 {
        p.screen_mut().add_image(crate::image::SixelImage {
            id: 0,
            raw: sixel_at_home("0;0;0q#0~"),
            px_width: 1200,
            px_height: 600,
            cell_width: cw,
            cell_height: ch,
            anchor_line: 0,
            anchor_col: 0,
            on_alt: false,
            erased: Vec::new(),
            cell_masks: Vec::new(),
            hid: Vec::new(),
        });
    }
    let started = std::time::Instant::now();
    for _ in 0..20 {
        p.process(&sixel_at_home("0;1;0q\"1;1;1200;600#0@"));
    }
    let took = started.elapsed();
    assert!(p.screen().images().len() >= 63, "a pixel hides none of them");
    assert!(took < std::time::Duration::from_secs(2), "20 sixels of 33 bytes took {took:?}");
}

#[test]
fn pane_sized_frames_still_hide_each_other() {
    // A frame as big as the pane (120 by 29 cells) still gets masks.
    let mut p = Parser::new(30, 120, 0);
    for n in 0..70 {
        let c = 1 + n % 3;
        p.process(&sixel_at_home(&format!("0;1;0q\"1;1;1200;580#{c};2;100;0;0#{c}!1200~")));
    }
    assert_eq!(p.screen().images().len(), 1);
    assert!(mask_bytes(&p) > 0);
}

#[test]
fn transparent_masks_match_the_pixels_drawn() {
    // A 25 by 12 pixel sixel with every pixel set over the first two bands
    // except column 13 of the second band, transparent: the masks are traced
    // ten bits a cell from the painted rows.
    let mut p = Parser::new(30, 120, 0);
    p.process(&sixel_at_home("0;1;0q\"1;1;25;12#1;2;100;0;0#1!25~-!13~?!11~"));
    let img = &p.screen().images()[0];
    assert_eq!((img.cell_width, img.cell_height), (3, 1));
    let bit = |m: &crate::image::CellMask, x: u32, y: u32| {
        let b = usize::try_from(y * 10 + x).unwrap();
        (m[b / 64] >> (b % 64)) & 1 == 1
    };
    for y in 0..20 {
        for x in 0..25u32 {
            let (cell, cx) = (usize::try_from(x / 10).unwrap(), x % 10);
            let want = y < 12 && !(x == 13 && y >= 6);
            assert_eq!(bit(&img.cell_masks[cell], cx, y), want, "pixel {x},{y}");
        }
    }
}

#[test]
fn an_image_names_the_images_it_hid_and_nothing_else() {
    // The client draws a frame straight over the ones it hid, without
    // rewriting their cells, so `hid` must name only those: an image ED 2J
    // took away is not hidden by the next one, which may leave pixels unset.
    let frame = |c: u8| {
        sixel_at_home(&format!(
            "0;1;0q\"1;1;40;40#{c};2;100;0;0#{c}!40~-!40~-!40~-!40~-!40~-!40~-!40~"
        ))
    };
    let mut p = Parser::new(30, 120, 0);
    p.process(&frame(1));
    let first = p.screen().images()[0].id;
    p.process(&frame(2));
    assert_eq!(p.screen().images().len(), 1);
    let second = p.screen().images()[0].id;
    assert_eq!(p.screen().images()[0].hid, vec![first]);

    let mut cleared = vec![ESC, b'[', b'2', b'J'];
    cleared.extend_from_slice(&sixel_at_home("0;1;0q\"1;1;40;40#3;2;0;100;0#3!20~"));
    p.process(&cleared);
    assert_eq!(p.screen().images().len(), 1);
    assert_ne!(p.screen().images()[0].id, second);
    assert!(p.screen().images()[0].hid.is_empty(), "ED 2J took the old frame, not the new image");
}
