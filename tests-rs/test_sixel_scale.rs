// Which cells of a pane image are visible, and that part cut out and
// resampled the way tmux's `sixel_scale` does (image-sixel.c:411-478 in
// tmux 3.8).
use super::{crop_scale, drawn_rects, visible_cells, VisibleCells};
use crate::sixel::decode::SixelPixels;

fn cells(i: u16, j: u16, x: u16, y: u16, rx: u16, ry: u16) -> Option<VisibleCells> {
    Some(VisibleCells { i, j, x, y, rx, ry })
}

#[test]
fn an_image_inside_the_pane_is_visible_whole() {
    assert_eq!(visible_cells(3, 4, 5, 2, 80, 24), cells(0, 0, 4, 3, 5, 2));
    // Exactly to the edges.
    assert_eq!(visible_cells(22, 75, 5, 2, 80, 24), cells(0, 0, 75, 22, 5, 2));
}

#[test]
fn the_right_and_bottom_are_cut_at_the_pane() {
    assert_eq!(visible_cells(3, 78, 5, 2, 80, 24), cells(0, 0, 78, 3, 2, 2));
    assert_eq!(visible_cells(23, 4, 5, 2, 80, 24), cells(0, 0, 4, 23, 5, 1));
    assert_eq!(visible_cells(23, 78, 5, 2, 80, 24), cells(0, 0, 78, 23, 2, 1));
}

#[test]
fn rows_scrolled_up_past_the_top_are_cut() {
    assert_eq!(visible_cells(-1, 4, 5, 3, 80, 24), cells(0, 1, 4, 0, 5, 2));
    // Taller than the pane on both sides.
    assert_eq!(visible_cells(-2, 0, 5, 30, 80, 24), cells(0, 2, 0, 0, 5, 24));
    // And cut on the right as well.
    assert_eq!(visible_cells(-1, 78, 5, 3, 80, 24), cells(0, 1, 78, 0, 2, 2));
}

#[test]
fn an_image_wholly_outside_is_not_visible() {
    assert_eq!(visible_cells(-3, 4, 5, 3, 80, 24), None);
    assert_eq!(visible_cells(24, 4, 5, 3, 80, 24), None);
    assert_eq!(visible_cells(3, 80, 5, 3, 80, 24), None);
    assert_eq!(visible_cells(3, 4, 0, 3, 80, 24), None);
}

/// A `w` by `h` image whose pixel at x, y is `(x + 100 * y) % 60000 + 1`.
fn ramp(w: u32, h: u32) -> SixelPixels {
    let pixels = (0..h).flat_map(|y| (0..w).map(move |x| ((x + 100 * y) % 60000 + 1) as u16)).collect();
    SixelPixels { width: w, height: h, p2: 0, palette: vec![], pixels }
}

fn whole(rx: u16, ry: u16) -> VisibleCells {
    VisibleCells { i: 0, j: 0, x: 0, y: 0, rx, ry }
}

#[test]
fn the_same_cell_size_and_the_whole_image_is_the_image() {
    let src = ramp(20, 40);
    assert_eq!(crop_scale(&src, (10, 20), whole(2, 2), (10, 20)), Some(src));
}

#[test]
fn a_part_starts_at_its_first_cell() {
    let src = ramp(30, 40);
    let part = VisibleCells { i: 1, j: 1, x: 0, y: 0, rx: 2, ry: 1 };
    let out = crop_scale(&src, (10, 20), part, (10, 20)).unwrap();
    assert_eq!((out.width, out.height), (20, 20));
    assert_eq!(out.get(0, 0), src.get(10, 20));
    assert_eq!(out.get(19, 19), src.get(29, 39));
}

#[test]
fn smaller_cells_take_every_other_pixel() {
    // 10x20 to 5x10: x maps to 2x and y to 2y, nearest neighbour.
    let src = ramp(20, 40);
    let out = crop_scale(&src, (10, 20), whole(2, 2), (5, 10)).unwrap();
    assert_eq!((out.width, out.height), (10, 20));
    for (x, y) in [(0, 0), (3, 7), (9, 19)] {
        assert_eq!(out.get(x, y), src.get(2 * x, 2 * y), "{x},{y}");
    }
}

#[test]
fn larger_cells_repeat_pixels() {
    // 10x20 to 15x30: x maps to x * 10 / 15, truncated, as in tmux.
    let src = ramp(10, 20);
    let out = crop_scale(&src, (10, 20), whole(1, 1), (15, 30)).unwrap();
    assert_eq!((out.width, out.height), (15, 30));
    for (x, y) in [(0, 0), (1, 1), (2, 2), (14, 29)] {
        assert_eq!(out.get(x, y), src.get(x * 10 / 15, y * 20 / 30), "{x},{y}");
    }
}

#[test]
fn a_last_cell_the_image_only_partly_fills_stays_partly_filled() {
    // 14 by 24 pixels is 2 by 2 cells; at 8x16 the image ends 11 by 19 pixels in.
    let src = ramp(14, 24);
    let out = crop_scale(&src, (10, 20), whole(2, 2), (8, 16)).unwrap();
    assert_eq!((out.width, out.height), (14 * 8 / 10, 24 * 16 / 20));
}

#[test]
fn an_empty_part_is_none() {
    let src = ramp(10, 20);
    let past = VisibleCells { i: 1, j: 0, x: 0, y: 0, rx: 1, ry: 1 };
    assert_eq!(crop_scale(&src, (10, 20), past, (10, 20)), None);
    assert_eq!(crop_scale(&src, (10, 20), whole(1, 1), (0, 20)), None);
}

// ── The cells text has been written over are left out ───────────────────

#[test]
fn with_nothing_written_over_the_part_is_drawn_as_it_is() {
    let part = VisibleCells { i: 0, j: 0, x: 5, y: 2, rx: 4, ry: 2 };
    assert_eq!(drawn_rects(part, &[]), vec![part]);
}

#[test]
fn a_last_row_written_over_leaves_the_rows_above() {
    let part = VisibleCells { i: 0, j: 0, x: 5, y: 2, rx: 4, ry: 2 };
    assert_eq!(drawn_rects(part, &[[1, 0, 4]]), vec![VisibleCells { i: 0, j: 0, x: 5, y: 2, rx: 4, ry: 1 }]);
}

#[test]
fn a_hole_splits_a_row_and_rows_with_the_same_cells_join() {
    // 4 by 3 cells; the middle two cells of row 1 are written over.
    let part = VisibleCells { i: 0, j: 0, x: 0, y: 0, rx: 4, ry: 3 };
    assert_eq!(
        drawn_rects(part, &[[1, 1, 2]]),
        vec![
            VisibleCells { i: 0, j: 0, x: 0, y: 0, rx: 4, ry: 1 },
            VisibleCells { i: 0, j: 1, x: 0, y: 1, rx: 1, ry: 1 },
            VisibleCells { i: 3, j: 1, x: 3, y: 1, rx: 1, ry: 1 },
            VisibleCells { i: 0, j: 2, x: 0, y: 2, rx: 4, ry: 1 },
        ]
    );
    assert_eq!(
        drawn_rects(part, &[[0, 3, 1], [1, 3, 1]]),
        vec![
            VisibleCells { i: 0, j: 0, x: 0, y: 0, rx: 3, ry: 2 },
            VisibleCells { i: 0, j: 2, x: 0, y: 2, rx: 4, ry: 1 },
        ]
    );
}

#[test]
fn erased_cells_outside_the_visible_part_change_nothing() {
    // Scrolled up a row and cut to 2 columns: rows 1 and 2, columns 0 and 1.
    let part = VisibleCells { i: 0, j: 1, x: 7, y: 0, rx: 2, ry: 2 };
    assert_eq!(drawn_rects(part, &[[0, 0, 4], [1, 2, 2]]), vec![part]);
    assert_eq!(
        drawn_rects(part, &[[2, 1, 3]]),
        vec![
            VisibleCells { i: 0, j: 1, x: 7, y: 0, rx: 2, ry: 1 },
            VisibleCells { i: 0, j: 2, x: 7, y: 1, rx: 1, ry: 1 },
        ]
    );
}

// A terminal that reports an absurd cell would make the part millions of
// pixels a side; it is not drawn rather than allocated.
#[test]
fn a_part_larger_than_the_largest_side_is_not_made() {
    let src = SixelPixels { width: 400, height: 200, p2: 0, palette: vec![None], pixels: vec![1; 400 * 200] };
    let whole = VisibleCells { i: 0, j: 0, x: 0, y: 0, rx: 40, ry: 10 };
    assert!(crop_scale(&src, (10, 20), whole, (8, 16)).is_some());
    assert_eq!(crop_scale(&src, (10, 20), whole, (60000, 60000)), None);
}
