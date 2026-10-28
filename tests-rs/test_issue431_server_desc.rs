//! Issue #431 (M3) unit check for `visible_pane_images` — the server-side
//! descriptor serialization.  Runs entirely in-process (no ConPTY / no live
//! terminal), so it deterministically proves that a parsed sixel becomes a
//! viewport-relative leaf descriptor with the right id/pos/pixel/cell fields,
//! and that images scrolled out of the viewport are culled.
//!
//! (The PowerShell TCP test `tests/test_issue431_server_desc.ps1` proves the
//! JSON schema is present in every leaf end-to-end; live rasterisation through
//! ConPTY passthrough is an M8 / interactive-Windows-Terminal concern per the
//! design's verification boundary.)

use super::visible_pane_images;

/// A minimal but valid sixel DCS: `ESC P q "Pan;Pad;Ph;Pv <data> ST`.
/// Raster attrs `"1;1;20;40` => 20x40 px => ceil against DEFAULT_CELL_PX(10,20)
/// => 2x2 cells.
const SIXEL_20X40: &[u8] = b"\x1bPq\"1;1;20;40#0;2;0;0;0#0~~~~~~\x1b\\";

#[test]
fn sixel_becomes_viewport_descriptor() {
    let mut p = vt100::Parser::new(29, 80, 2000);
    p.process(SIXEL_20X40);
    let screen = p.screen();
    assert_eq!(screen.images().len(), 1, "one image stored on the screen");

    let descs = visible_pane_images(screen, 29, 0);
    assert_eq!(descs.len(), 1, "one visible descriptor emitted");
    let d = &descs[0];
    assert_eq!(d.pw, 20, "pixel width from raster attrs");
    assert_eq!(d.ph, 40, "pixel height from raster attrs");
    assert_eq!(d.cw, 2, "cell width = ceil(20/10)");
    assert_eq!(d.ch, 2, "cell height = ceil(40/20)");
    assert_eq!(d.col, 0, "anchored at column 0 of a fresh grid");
    assert_eq!(d.row, 0, "anchored at viewport row 0 of a fresh grid");
    assert!(d.id >= 1, "id is the monotonic Screen id (>= 1)");
}

#[test]
fn image_scrolled_above_viewport_is_culled() {
    // 5-row grid with generous scrollback: the image stays retained (so it
    // survives scrolling) but must not be emitted once it leaves the viewport.
    let mut p = vt100::Parser::new(5, 80, 2000);
    p.process(SIXEL_20X40);
    // Below the image first: text written over it erases its cells.
    p.process(b"\r\n\r\n");
    for _ in 0..60 {
        p.process(b"line\r\n");
    }
    // Image is still retained in scrollback...
    assert_eq!(p.screen().images().len(), 1, "image retained in scrollback");
    // ...but culled from the visible viewport (row-span no longer intersects).
    let descs = visible_pane_images(p.screen(), 5, 0);
    assert!(
        descs.is_empty(),
        "off-viewport image must be culled, got {}",
        descs.len()
    );
}

#[test]
fn scrolling_back_reveals_the_image_again() {
    // Emit a top-anchored image, then scroll it out of a 5-row viewport. A
    // scrollback offset brings it back into view, and the descriptor's row is a
    // function of the offset (higher offset => image sits lower in the viewport).
    let mut p = vt100::Parser::new(5, 80, 2000);
    p.process(SIXEL_20X40);
    // Below the image first: text written over it erases its cells.
    p.process(b"\r\n\r\n");
    for _ in 0..20 {
        p.process(b"line\r\n");
    }
    assert!(
        visible_pane_images(p.screen(), 5, 0).is_empty(),
        "image is above the viewport at offset 0"
    );

    // At the maximum scrollback offset the top-anchored image is at viewport
    // row 0 (the viewport top is scrolled all the way back to the oldest line).
    let max_off = p.screen().scrollback_filled();
    assert!(max_off > 0, "content scrolled into scrollback");
    let full = visible_pane_images(p.screen(), 5, max_off);
    assert_eq!(full.len(), 1, "scrolling all the way back reveals the image");
    assert_eq!(full[0].row, 0, "top-anchored image sits at viewport row 0");

    // One row less of scrollback lifts the image one row above the top edge
    // (row -1); it still straddles the viewport so it is still emitted.
    let less = visible_pane_images(p.screen(), 5, max_off - 1);
    assert_eq!(less.len(), 1, "image still partly visible one row up");
    assert_eq!(less[0].row, -1, "less scrollback raises the image by one row");
}
