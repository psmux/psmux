// A sixel drawn over an older one hides it where it sets every pixel.
//
// Windows Terminal, whose sixel parser the pane's OpenConsole runs, writes a
// sixel into each row's `ImageSlice` pixel by pixel and leaves a pixel the
// sixel does not set as it was (`_maybeFlushImageBuffer`), so the older image
// shows only through pixels the newer one leaves unset.  chafa plays a GIF by
// drawing every frame in the same place: the pane kept each frame under the
// next until its cap of 64, and from then on each frame evicted the oldest
// and the client drew the 63 left again, up to 28 MB a frame.
use crate::Parser;

/// A frame at the top left: `ESC P 0 ; 1 q` (transparent background), 40 by
/// 24 pixels, 4 by 2 cells, the second row 4 pixels deep.
fn frame(colour: u8) -> Vec<u8> {
    format!("\x1b[H\x1bP0;1q#{colour};2;100;0;0#{colour}!40~-!40~\x1b\\").into_bytes()
}

fn erased_rows(p: &Parser, i: usize) -> Vec<Vec<bool>> {
    let img = &p.screen().images()[i];
    (0..img.cell_height).map(|r| (0..img.cell_width).map(|c| img.is_erased(r, c)).collect()).collect()
}

#[test]
fn frames_drawn_in_one_place_leave_one_image() {
    let mut p = Parser::new(24, 80, 100);
    for n in 0..70 {
        p.process(&frame(1 + n % 3));
    }
    assert_eq!(p.screen().images().len(), 1, "only the last frame shows");
    assert!(p.screen().images()[0].erased.is_empty(), "the last frame is whole");
}

#[test]
fn rows_no_frame_sets_do_not_keep_old_frames() {
    // chafa's frames: a raster of 720 by 440 and 1:1 pixels, so the last band
    // runs 4 rows past it, and no frame sets those rows.  Here a raster of 40
    // by 22 with the last band setting its top 4 rows only: the second row of
    // cells has rows 20 and 21 set, 22 and 23 unset, in every frame.
    let mut p = Parser::new(24, 80, 100);
    for n in 0..70 {
        let colour = 1 + n % 3;
        p.process(
            format!("\x1b[H\x1bP0;1q\"1;1;40;22#{colour};2;100;0;0#{colour}!40~-!40~-!40~-!40N\x1b\\").as_bytes(),
        );
    }
    assert_eq!(p.screen().images().len(), 1);
    assert_eq!(p.screen().images()[0].px_height, 24);
}

#[test]
fn the_partly_filled_last_row_is_hidden_by_an_image_as_tall() {
    // The second row of cells holds 4 pixel rows of each image: the new one
    // sets all of them, which is all the old one has there.
    let mut p = Parser::new(24, 80, 100);
    p.process(&frame(1));
    p.process(&frame(2));
    assert_eq!(p.screen().images().len(), 1);
}

#[test]
fn an_image_reaching_further_down_keeps_its_cells_under_a_shorter_one() {
    // The old image is 40 by 40 pixels: its second row of cells is full, and
    // the new one, 4 pixels deep there, cannot hide it.
    let mut p = Parser::new(24, 80, 100);
    p.process(b"\x1b[H\x1bP0;1q#1;2;100;0;0#1!40~-!40~-!40~-!40~\x1b\\");
    p.process(&frame(2));
    assert_eq!(p.screen().images().len(), 2);
    assert_eq!(erased_rows(&p, 0)[..2], [vec![true; 4], vec![false; 4]]);
}

#[test]
fn pixels_left_unset_keep_the_image_under_them() {
    // The new image draws only the first 20 columns, and leaves bits unset in
    // its third cell: the old image is hidden in its first two cells only.
    let mut p = Parser::new(24, 80, 100);
    p.process(&frame(1));
    p.process(b"\x1b[H\x1bP0;1q#2;2;0;100;0#2!20~!10N-!20~!10N\x1b\\");
    assert_eq!(p.screen().images().len(), 2);
    assert_eq!(erased_rows(&p, 0), vec![vec![true, true, false, false], vec![true, true, false, false]]);
}

#[test]
fn a_filled_background_hides_what_the_raster_covers() {
    // Background select 0 fills the image, so a sixel that draws one column
    // still hides every cell of its 40 by 24 raster.
    let mut p = Parser::new(24, 80, 100);
    p.process(&frame(1));
    p.process(b"\x1b[H\x1bP0;0q\"1;1;40;24#2;2;0;100;0#2~\x1b\\");
    assert_eq!(p.screen().images().len(), 1);
}

#[test]
fn an_image_beside_another_hides_nothing_of_it() {
    let mut p = Parser::new(24, 80, 100);
    p.process(&frame(1));
    p.process(b"\x1b[1;5H\x1bP0;1q#2;2;0;100;0#2!40~-!40~\x1b\\");
    assert_eq!(p.screen().images().len(), 2);
    assert!(p.screen().images()[0].erased.is_empty());
}

#[test]
fn an_image_on_the_alternate_screen_hides_nothing_on_the_main_one() {
    let mut p = Parser::new(24, 80, 100);
    p.process(&frame(1));
    p.process(b"\x1b[?1049h");
    p.process(&frame(2));
    let main: Vec<_> = p.screen().images().iter().filter(|i| !i.on_alt).collect();
    assert_eq!(main.len(), 1);
    assert!(main[0].erased.is_empty());
}

#[test]
fn a_sixel_too_costly_to_trace_hides_nothing() {
    // A raster with a large aspect makes each sixel many pixel rows tall, and
    // `$` draws the same column again: past the work limit the sixel's pixels
    // are not known, and the layout is the one sixel_layout gives.
    let mut dcs = b"\x1bP0;1q\"80;1;10;10#1;2;100;0;0".to_vec();
    for _ in 0..100_000 {
        dcs.extend_from_slice(b"~$");
    }
    dcs.extend_from_slice(b"\x1b\\");
    let started = std::time::Instant::now();
    let (layout, masks) = crate::perform::sixel_cell_masks(&dcs, 24, 24);
    assert!(masks.is_empty());
    assert_eq!(layout, crate::perform::sixel_layout(&dcs, 24, 24));
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}
