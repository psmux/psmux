// Issue #431 (M1 foundation): sixel image storage on Screen + Grid absolute-line
// anchoring.  Verifies add_image assigns ids/anchors, that images are pruned when
// their anchor scrolls off retained scrollback, that alt-screen images are
// ephemeral while main-grid images survive, and that CSI 2J/3J drop images.
use crate::Parser;
use crate::image::{SixelImage, cell_dims_from_px};

/// Builds a bare image; add_image overwrites id/anchor_line/anchor_col/on_alt.
fn mk_image(px_w: u32, px_h: u32) -> SixelImage {
    let (cw, ch) = cell_dims_from_px(px_w, px_h);
    SixelImage {
        id: 0,
        raw: b"\x1bP0;0;0q#0~~\x1b\\".to_vec(),
        px_width: px_w,
        px_height: px_h,
        cell_width: cw,
        cell_height: ch,
        anchor_line: 0,
        anchor_col: 0,
        on_alt: false,
        erased: Vec::new(),
        cell_masks: Vec::new(),
    }
}

#[test]
fn cell_dims_ceil_div_defaults() {
    // DEFAULT_CELL_PX = (10, 20).
    assert_eq!(cell_dims_from_px(320, 240), (32, 12));
    assert_eq!(cell_dims_from_px(1, 1), (1, 1)); // never zero cells
    assert_eq!(cell_dims_from_px(11, 21), (2, 2)); // ceil, not floor
}

#[test]
fn add_image_sets_anchor_and_id() {
    let mut p: Parser = Parser::new(24, 80, 0);
    // Move cursor to row 5, col 3 (1-indexed) => pos.row=4, col=2.
    p.process(b"\x1b[5;3H");
    let id = p.screen_mut().add_image(mk_image(100, 40));
    assert_ne!(id, 0, "0 is never an image id");
    let imgs = p.screen().images();
    assert_eq!(imgs.len(), 1);
    let img = &imgs[0];
    assert_eq!(img.id, id);
    assert_eq!(img.anchor_line, 4, "no scrollback => abs line == cursor row");
    assert_eq!(img.anchor_col, 2);
    assert!(!img.on_alt, "added on the main grid");

    // Second image gets a later id.
    let id2 = p.screen_mut().add_image(mk_image(10, 20));
    assert!(id2 > id, "ids increase: {id} then {id2}");
    assert_eq!(p.screen().images().len(), 2);
}

#[test]
fn image_pruned_when_anchor_scrolls_off_scrollback() {
    // 3 visible rows, scrollback capped at 2 rows.
    let mut p: Parser = Parser::new(3, 20, 2);
    // Cursor at top; anchor an image at absolute line 0 with cell_height 1.
    p.process(b"\x1b[1;1H");
    let id1 = p.screen_mut().add_image(mk_image(10, 20)); // 1 cell tall
    assert_eq!(p.screen().images().len(), 1);
    assert_eq!(p.screen().images()[0].anchor_line, 0);

    // Scroll well past the scrollback cap so first_line advances beyond
    // anchor_line + cell_height (== 1).
    for _ in 0..12 {
        p.process(b"X\r\n");
    }
    // Adding a new image runs prune_images, which must drop the scrolled-off id1.
    let id2 = p.screen_mut().add_image(mk_image(10, 20));
    assert_ne!(id2, id1);
    let imgs = p.screen().images();
    assert_eq!(imgs.len(), 1, "scrolled-off image pruned, only new one remains");
    assert_eq!(imgs[0].id, id2, "the surviving image is the newly added one");
}

#[test]
fn alt_screen_image_dropped_on_exit_main_survives() {
    let mut p: Parser = Parser::new(10, 40, 100);
    // Main-grid image.
    p.process(b"\x1b[1;1H");
    let main_id = p.screen_mut().add_image(mk_image(30, 40));
    assert_eq!(p.screen().images().len(), 1);

    // Enter the alternate screen and add an alt-grid image.
    p.process(b"\x1b[?1049h");
    let alt_id = p.screen_mut().add_image(mk_image(30, 40));
    {
        let imgs = p.screen().images();
        assert_eq!(imgs.len(), 2, "both images present while alt is active");
        let alt = imgs.iter().find(|i| i.id == alt_id).unwrap();
        assert!(alt.on_alt, "alt-added image tagged on_alt");
        let main = imgs.iter().find(|i| i.id == main_id).unwrap();
        assert!(!main.on_alt, "main image still not on_alt");
    }

    // Exit the alternate screen: alt image dropped, main image survives.
    p.process(b"\x1b[?1049l");
    let imgs = p.screen().images();
    assert_eq!(imgs.len(), 1, "alt image dropped on exit");
    assert_eq!(imgs[0].id, main_id, "surviving image is the main-grid one");
    assert!(!imgs[0].on_alt);
}

#[test]
fn ed_2_and_3_drop_current_grid_images() {
    // CSI 2J drops current-grid images.
    let mut p: Parser = Parser::new(10, 40, 100);
    p.process(b"\x1b[1;1H");
    p.screen_mut().add_image(mk_image(30, 40));
    assert_eq!(p.screen().images().len(), 1);
    p.process(b"\x1b[2J");
    assert_eq!(p.screen().images().len(), 0, "CSI 2J drops the image");

    // CSI 3J (clear scrollback) also drops current-grid images.
    let mut p2: Parser = Parser::new(10, 40, 100);
    p2.process(b"\x1b[1;1H");
    p2.screen_mut().add_image(mk_image(30, 40));
    assert_eq!(p2.screen().images().len(), 1);
    p2.process(b"\x1b[3J");
    assert_eq!(p2.screen().images().len(), 0, "CSI 3J drops the image");
}

#[test]
fn clear_scrollback_keeps_cursor_absolute_line_stable() {
    // Regression for the centralised first_line accounting: clearing scrollback
    // must not shift the cursor's absolute line (the visible content stays put),
    // so an image anchored at the cursor keeps its anchor across a 3J that only
    // clears scrollback rows below/above the visible screen. We assert via a
    // fresh image before and after having identical anchor for the same cursor.
    let mut p: Parser = Parser::new(3, 20, 100);
    for _ in 0..10 {
        p.process(b"X\r\n"); // build some scrollback
    }
    p.process(b"\x1b[1;1H");
    let before = p.screen_mut().add_image(mk_image(10, 20));
    let anchor_before = p.screen().images().iter().find(|i| i.id == before).unwrap().anchor_line;
    // 3J drops the image, but the absolute line of the (unchanged) cursor must
    // be identical for a freshly added image at the same cursor position.
    p.process(b"\x1b[3J");
    p.process(b"\x1b[1;1H");
    let after = p.screen_mut().add_image(mk_image(10, 20));
    let anchor_after = p.screen().images().iter().find(|i| i.id == after).unwrap().anchor_line;
    assert_eq!(anchor_before, anchor_after, "cursor absolute line stable across 3J");
}

#[test]
fn two_panes_never_share_an_image_id() {
    // The server ships each blob once per id and the client caches by id, so
    // two panes must not both hand out the same id.  With a counter per
    // screen, both panes' first image was id 1 and only one blob was shipped.
    let mut a: Parser = Parser::new(10, 40, 100);
    let mut b: Parser = Parser::new(10, 40, 100);
    let ia = a.screen_mut().add_image(mk_image(10, 20));
    let ib = b.screen_mut().add_image(mk_image(10, 20));
    assert_ne!(ia, ib, "pane A and pane B got the same image id");
}

#[test]
fn ris_does_not_reuse_an_image_id() {
    // RIS rebuilds the screen.  The image id must not restart, or the client
    // would draw the blob it cached for the image from before the reset.
    let mut p: Parser = Parser::new(10, 40, 100);
    let before = p.screen_mut().add_image(mk_image(10, 20));
    p.process(b"\x1bc");
    assert!(p.screen().images().is_empty(), "RIS drops the image");
    let after = p.screen_mut().add_image(mk_image(10, 20));
    assert_ne!(before, after, "RIS restarted the image ids");
}
