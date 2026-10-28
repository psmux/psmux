// The client side of pane sixel images (#431): the overlay bytes and which
// frames write them.  Moved from test_client.rs with the code.

use std::collections::HashMap;

use super::{build_sixel_overlay, SixelDrawCache, SixelEmit};

/// The overlay at the 10 by 20 pixel cell the pane lays images out in.
fn overlay(emits: &[SixelEmit], blobs: &HashMap<u64, Vec<u8>>) -> Vec<u8> {
    build_sixel_overlay(emits, blobs, &mut SixelDrawCache::default(), vt100::image::DEFAULT_CELL_PX)
}

// ── Sixel overlay builder (#431, M6) ─────────────────────────────────
#[test]
fn build_sixel_overlay_emits_decsc_cup_blob_decrc_for_cached_id() {
    use std::collections::HashMap;
    let blob = b"\x1bP0;0;0q#0;2;0;0;0!100~-\x1b\\".to_vec();
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    cache.insert(7, blob.clone());
    let emits = vec![super::SixelEmit::whole(4, 2, 7, 10, 6)];
    let out = overlay(&emits, &cache);
    // DECSC, then CUP to 1-based (y+1=3 ; x+1=5), then the raw blob, then DECRC.
    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(b"\x1b7");
    expected.extend_from_slice(b"\x1b[3;5H");
    expected.extend_from_slice(&blob);
    expected.extend_from_slice(b"\x1b8");
    assert_eq!(out, expected);
}

#[test]
fn build_sixel_overlay_skips_uncached_id() {
    use std::collections::HashMap;
    let cache: HashMap<u64, Vec<u8>> = HashMap::new(); // id 7 not present
    let emits = vec![super::SixelEmit::whole(0, 0, 7, 1, 1)];
    let out = overlay(&emits, &cache);
    assert!(out.is_empty(), "uncached id must produce no bytes");
}

#[test]
fn build_sixel_overlay_empty_emits_is_empty() {
    use std::collections::HashMap;
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    cache.insert(7, b"\x1bPq\x1b\\".to_vec());
    let out = overlay(&[], &cache);
    assert!(out.is_empty(), "no emits must produce no bytes");
}

#[test]
fn build_sixel_overlay_concatenates_multiple_emits() {
    use std::collections::HashMap;
    let a = b"\x1bPqAAA\x1b\\".to_vec();
    let b = b"\x1bPqBBB\x1b\\".to_vec();
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    cache.insert(1, a.clone());
    cache.insert(2, b.clone());
    let emits = vec![
        super::SixelEmit::whole(0, 0, 1, 1, 1),
        super::SixelEmit::whole(9, 3, 2, 1, 1),
    ];
    let out = overlay(&emits, &cache);
    let mut expected: Vec<u8> = Vec::new();
    expected.extend_from_slice(b"\x1b7\x1b[1;1H");
    expected.extend_from_slice(&a);
    expected.extend_from_slice(b"\x1b8");
    expected.extend_from_slice(b"\x1b7\x1b[4;10H");
    expected.extend_from_slice(&b);
    expected.extend_from_slice(b"\x1b8");
    assert_eq!(out, expected);
}


// #431: the client writes sixel images only when they or the screen size
// change, and covers what a moved or removed image left behind.
fn sixel_at(x: u16, y: u16, id: u64) -> super::SixelEmit {
    super::SixelEmit::whole(x, y, id, 2, 1)
}

#[test]
fn plan_sixel_frame_writes_nothing_while_nothing_changed() {
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let imgs = vec![sixel_at(3, 4, 7)];
    assert_eq!(super::plan_sixel_frame(&imgs, area, &imgs, area), None);
    assert_eq!(super::plan_sixel_frame(&[], area, &[], area), None);
}

#[test]
fn plan_sixel_frame_draws_a_new_image() {
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let now = vec![sixel_at(3, 4, 7)];
    let plan = super::plan_sixel_frame(&[], area, &now, area).expect("a new image is drawn");
    assert!(plan.repaint.is_empty());
    assert_eq!(plan.draw, now);
}

#[test]
fn plan_sixel_frame_covers_where_a_moved_image_was() {
    // Output scrolled the image up a row: the cells it covered are rewritten
    // and it is drawn at its new place.
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let last = vec![sixel_at(3, 4, 7)];
    let now = vec![sixel_at(3, 3, 7)];
    let plan = super::plan_sixel_frame(&last, area, &now, area).expect("a moved image is redrawn");
    assert_eq!(plan.repaint, vec![Rect::new(3, 4, 2, 1)]);
    assert_eq!(plan.draw, now);
}

#[test]
fn plan_sixel_frame_covers_a_removed_image() {
    // The image went away (cleared, or the sixel option turned off).
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let last = vec![sixel_at(78, 23, 7)];
    let plan = super::plan_sixel_frame(&last, area, &[], area).expect("a removed image is covered");
    assert_eq!(plan.repaint, vec![Rect::new(78, 23, 2, 1)]);
    assert!(plan.draw.is_empty());
}

#[test]
fn plan_sixel_frame_redraws_after_a_resize_without_repainting() {
    // ratatui clears the screen on a resize, which takes the images with it.
    use ratatui::layout::Rect;
    let last = vec![sixel_at(3, 4, 7)];
    let plan = super::plan_sixel_frame(&last, Rect::new(0, 0, 80, 24), &last, Rect::new(0, 0, 100, 30))
        .expect("a resize redraws the images");
    assert!(plan.repaint.is_empty());
    assert_eq!(plan.draw, last);
}

// ── The visible part, at the terminal's cell size ────────────────────
// The 914 suite's image as the server stores it: 14 by 24 pixels, 2 by 2
// cells, with the raster command vt100 puts in front.
const STORED: &[u8] = b"\x1bP0q\"2;1;14;24#1;2;100;100;0#2;2;0;100;0#1~~@@vv@@~~@@~~$#2??}}GG}}??}}??-#1@@@@@@@@@@@@@@\x1b\\";

fn blobs() -> HashMap<u64, Vec<u8>> {
    HashMap::from([(7, STORED.to_vec())])
}

/// The sixel an overlay holds between its cursor move and DECRC.
fn sixel_of(out: &[u8]) -> Vec<u8> {
    let start = out.windows(2).position(|w| w == b"\x1bP").expect("a DCS");
    let end = out.len() - 2; // the DECRC
    out[start..end].to_vec()
}

#[test]
fn a_whole_image_at_the_pane_cell_is_the_blob_as_it_came() {
    let out = overlay(&[SixelEmit::whole(4, 2, 7, 2, 2)], &blobs());
    assert_eq!(sixel_of(&out), STORED);
}

#[test]
fn a_whole_image_on_smaller_cells_is_resampled_to_them() {
    let mut cache = SixelDrawCache::default();
    let out = build_sixel_overlay(&[SixelEmit::whole(4, 2, 7, 2, 2)], &blobs(), &mut cache, (8, 16));
    let img = crate::sixel::decode::decode(&sixel_of(&out)).expect("a sixel");
    // The image keeps its 2 by 2 cells; 14 by 24 pixels become 11 by 19.
    assert_eq!((img.width, img.height), (11, 19));
    assert!(sixel_of(&out).starts_with(b"\x1bP9;0q\"1;1;11;19"));
}

#[test]
fn a_cut_image_is_the_visible_cells_only() {
    // Scrolled up a row: only the bottom row of cells is left.
    let emit = SixelEmit { x: 4, y: 0, id: 7, cw: 2, ch: 2, i: 0, j: 1, rx: 2, ry: 1 };
    let out = overlay(&[emit], &blobs());
    assert!(out.starts_with(b"\x1b7\x1b[1;5H"));
    let img = crate::sixel::decode::decode(&sixel_of(&out)).expect("a sixel");
    let whole = crate::sixel::decode::decode(STORED).unwrap();
    assert_eq!((img.width, img.height), (14, 4));
    assert_eq!(img.get(0, 0), whole.get(0, 20));
    assert_eq!(img.get(13, 3), whole.get(13, 23));
}

#[test]
fn a_cut_image_that_does_not_decode_is_not_drawn() {
    let blobs = HashMap::from([(7, b"\x1bP0;0".to_vec())]);
    let cut = SixelEmit { x: 0, y: 0, id: 7, cw: 2, ch: 2, i: 0, j: 0, rx: 1, ry: 2 };
    assert!(overlay(&[cut], &blobs).is_empty());
    // Whole, it is still written as it came.
    let mut cache = SixelDrawCache::default();
    let out = build_sixel_overlay(&[SixelEmit::whole(0, 0, 7, 2, 2)], &blobs, &mut cache, (8, 16));
    assert_eq!(sixel_of_any(&out), b"\x1bP0;0".to_vec());
}

/// The bytes between the cursor move and DECRC, for a blob that is not a DCS.
fn sixel_of_any(out: &[u8]) -> Vec<u8> {
    let h = out.iter().position(|&b| b == b'H').expect("a cursor move");
    out[h + 1..out.len() - 2].to_vec()
}

#[test]
fn a_new_blob_for_an_id_is_drawn_from_the_new_bytes() {
    let mut cache = SixelDrawCache::default();
    let emit = [SixelEmit::whole(0, 0, 7, 2, 2)];
    let first = build_sixel_overlay(&emit, &blobs(), &mut cache, (8, 16));
    // Another server's image with the same id, after switch-client.
    let other = HashMap::from([(7, b"\x1bP9q#1;2;0;0;100#1!10~\x1b\\".to_vec())]);
    cache.forget(7);
    let second = build_sixel_overlay(&emit, &other, &mut cache, (8, 16));
    assert_ne!(first, second);
    let img = crate::sixel::decode::decode(&sixel_of(&second)).unwrap();
    assert_eq!(img.width, 8, "10 pixels at 10 to a cell are 8 at 8");
}

#[test]
fn plan_sixel_frame_covers_only_the_cells_a_cut_image_drew() {
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let last = vec![SixelEmit { x: 3, y: 0, id: 7, cw: 4, ch: 3, i: 0, j: 1, rx: 4, ry: 2 }];
    let plan = super::plan_sixel_frame(&last, area, &[], area).expect("a removed image is covered");
    assert_eq!(plan.repaint, vec![Rect::new(3, 0, 4, 2)]);
}

#[test]
fn plan_sixel_frame_redraws_when_the_visible_part_changes() {
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let last = vec![SixelEmit { x: 3, y: 0, id: 7, cw: 4, ch: 3, i: 0, j: 1, rx: 4, ry: 2 }];
    let now = vec![SixelEmit { x: 3, y: 0, id: 7, cw: 4, ch: 3, i: 0, j: 2, rx: 4, ry: 1 }];
    assert!(super::plan_sixel_frame(&last, area, &now, area).is_some());
}

#[test]
fn plan_sixel_frame_draws_only_the_part_that_changed() {
    // A key typed on the image's last row: the big part above stays, the
    // small part on that row shrinks by a cell.
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 30);
    let top = SixelEmit { x: 0, y: 1, id: 1, cw: 36, ch: 24, i: 0, j: 0, rx: 36, ry: 23 };
    let old_row = SixelEmit { x: 19, y: 24, id: 1, cw: 36, ch: 24, i: 19, j: 23, rx: 17, ry: 1 };
    let new_row = SixelEmit { x: 20, y: 24, id: 1, cw: 36, ch: 24, i: 20, j: 23, rx: 16, ry: 1 };
    let plan = super::plan_sixel_frame(&[top.clone(), old_row], area, &[top, new_row.clone()], area)
        .expect("the row changed");
    assert_eq!(plan.repaint, vec![Rect::new(19, 24, 17, 1)]);
    assert_eq!(plan.draw, vec![new_row], "the part above is left alone");
}

#[test]
fn plan_sixel_frame_redraws_a_part_whose_cells_are_rewritten() {
    // An image that went away overlapped one that stays: rewriting its cells
    // erases the other image there, so that one is drawn again.
    use ratatui::layout::Rect;
    let area = Rect::new(0, 0, 80, 24);
    let stays = SixelEmit::whole(0, 0, 1, 4, 4);
    let goes = SixelEmit::whole(2, 2, 2, 4, 4);
    let apart = SixelEmit::whole(40, 10, 3, 2, 2);
    let plan = super::plan_sixel_frame(
        &[stays.clone(), goes, apart.clone()],
        area,
        &[stays.clone(), apart],
        area,
    )
    .expect("an image went away");
    assert_eq!(plan.repaint, vec![Rect::new(2, 2, 4, 4)]);
    assert_eq!(plan.draw, vec![stays], "the image apart from it is left alone");
}
