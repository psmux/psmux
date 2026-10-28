// The client side of pane sixel images (#431): the overlay bytes and which
// frames write them.  Moved from test_client.rs with the code.

// ── Sixel overlay builder (#431, M6) ─────────────────────────────────
#[test]
fn build_sixel_overlay_emits_decsc_cup_blob_decrc_for_cached_id() {
    use std::collections::HashMap;
    let blob = b"\x1bP0;0;0q#0;2;0;0;0!100~-\x1b\\".to_vec();
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    cache.insert(7, blob.clone());
    let emits = vec![super::SixelEmit { x: 4, y: 2, id: 7, cw: 10, ch: 6 }];
    let out = super::build_sixel_overlay(&emits, &cache);
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
    let emits = vec![super::SixelEmit { x: 0, y: 0, id: 7, cw: 1, ch: 1 }];
    let out = super::build_sixel_overlay(&emits, &cache);
    assert!(out.is_empty(), "uncached id must produce no bytes");
}

#[test]
fn build_sixel_overlay_empty_emits_is_empty() {
    use std::collections::HashMap;
    let mut cache: HashMap<u64, Vec<u8>> = HashMap::new();
    cache.insert(7, b"\x1bPq\x1b\\".to_vec());
    let out = super::build_sixel_overlay(&[], &cache);
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
        super::SixelEmit { x: 0, y: 0, id: 1, cw: 1, ch: 1 },
        super::SixelEmit { x: 9, y: 3, id: 2, cw: 1, ch: 1 },
    ];
    let out = super::build_sixel_overlay(&emits, &cache);
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
    super::SixelEmit { x, y, id, cw: 2, ch: 1 }
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
