//! Which cells of a pane image are visible, and that part cut out and
//! resampled to the terminal's cell size.

use super::decode::SixelPixels;

/// The visible part of an image, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct VisibleCells {
    /// The first visible cell inside the image: columns and rows cut off on
    /// the left and at the top.
    pub i: u16,
    pub j: u16,
    /// Where that cell is inside the pane.
    pub x: u16,
    pub y: u16,
    /// How many cells are visible.
    pub rx: u16,
    pub ry: u16,
}

/// The cells of an image at `row`, `col` (the row can be negative once it
/// has scrolled up), `cw` by `ch` cells, that lie inside a pane of
/// `width` by `height` cells. `None` when none do.
///
/// tmux cuts an image to the pane when it is stored and when it scrolls
/// (`screen_write_sixelimage`, screen-write.c:3084; `image_scroll_up`,
/// image.c:191 in tmux 3.8). psmux keeps the whole image and cuts what is
/// drawn instead, so the part that comes back into view on a resize or in
/// copy mode is still there.
pub(crate) fn visible_cells(row: i32, col: u16, cw: u16, ch: u16, width: u16, height: u16) -> Option<VisibleCells> {
    if col >= width || cw == 0 || ch == 0 {
        return None;
    }
    let top = row.max(0);
    let bottom = (row + i32::from(ch)).min(i32::from(height));
    if top >= bottom {
        return None;
    }
    Some(VisibleCells {
        i: 0,
        j: (top - row) as u16,
        x: col,
        y: top as u16,
        rx: cw.min(width - col),
        ry: (bottom - top) as u16,
    })
}

/// The rectangles of `part` to draw once the cells in `erased`, `[row, col,
/// len]` runs in the image's own cells that text has been written over, are
/// left out. Each row's runs of cells still to draw become rectangles, and
/// rows with the same runs are joined, so an image with nothing written over
/// it is `part` alone.
///
/// A sixel cannot leave a hole: pixels it does not set are filled with
/// colour 0 unless its background select says otherwise. So the cells around
/// a hole are drawn as separate images, cut out like any other part.
pub(crate) fn drawn_rects(part: VisibleCells, erased: &[[u16; 3]]) -> Vec<VisibleCells> {
    if erased.is_empty() {
        return vec![part];
    }
    let runs_of = |row: u16| -> Vec<(u16, u16)> {
        let (left, right) = (part.i, part.i + part.rx);
        let mut keep = vec![true; usize::from(part.rx)];
        for &[_, c, n] in erased.iter().filter(|e| e[0] == row) {
            let (c0, c1) = (c.max(left), c.saturating_add(n).min(right));
            for col in c0..c1.max(c0) {
                keep[usize::from(col - left)] = false;
            }
        }
        let mut runs = Vec::new();
        let mut col = 0usize;
        while col < keep.len() {
            if !keep[col] {
                col += 1;
                continue;
            }
            let start = col;
            while col < keep.len() && keep[col] {
                col += 1;
            }
            runs.push((left + start as u16, (col - start) as u16));
        }
        runs
    };
    let mut rects = Vec::new();
    let mut row = part.j;
    while row < part.j + part.ry {
        let runs = runs_of(row);
        let mut rows = 1;
        while row + rows < part.j + part.ry && runs_of(row + rows) == runs {
            rows += 1;
        }
        for &(col, len) in &runs {
            rects.push(VisibleCells {
                i: col,
                j: row,
                x: part.x + (col - part.i),
                y: part.y + (row - part.j),
                rx: len,
                ry: rows,
            });
        }
        row += rows;
    }
    rects
}

/// The cells `i`, `j`, `rx` by `ry` of `src`, whose cells are `src_cell`
/// pixels, resampled to cells of `dst_cell` pixels. `None` when that part is
/// empty.
///
/// This is tmux's `sixel_scale` (image-sixel.c:411-478 in tmux 3.8): nearest
/// neighbour, with the same integer arithmetic. The result is cut to where
/// the image ends, so a last cell the image only partly fills stays partly
/// filled, as tmux does with the raster size.
pub(crate) fn crop_scale(
    src: &SixelPixels,
    src_cell: (u32, u32),
    part: VisibleCells,
    dst_cell: (u32, u32),
) -> Option<SixelPixels> {
    let (sxp, syp) = src_cell;
    let (dxp, dyp) = dst_cell;
    if sxp == 0 || syp == 0 || dxp == 0 || dyp == 0 {
        return None;
    }
    let pox = u32::from(part.i) * sxp;
    let poy = u32::from(part.j) * syp;
    let psx = u32::from(part.rx) * sxp;
    let psy = u32::from(part.ry) * syp;
    let tsx = u32::from(part.rx) * dxp;
    let tsy = u32::from(part.ry) * dyp;
    // Where the image ends inside the part, in target pixels.
    let width = (src.width.saturating_sub(pox).min(psx) as u64 * u64::from(dxp) / u64::from(sxp)) as u32;
    let height = (src.height.saturating_sub(poy).min(psy) as u64 * u64::from(dyp) / u64::from(syp)) as u32;
    if width == 0 || height == 0 || tsx == 0 || tsy == 0 {
        return None;
    }
    // A cell size the terminal reported far larger than any real one would
    // otherwise make a part of millions of pixels a side; such a part is not
    // drawn rather than allocated.
    if width > super::decode::MAX_SIDE || height > super::decode::MAX_SIDE {
        return None;
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize);
    for y in 0..height {
        let py = poy + (u64::from(y) * u64::from(psy) / u64::from(tsy)) as u32;
        for x in 0..width {
            let px = pox + (u64::from(x) * u64::from(psx) / u64::from(tsx)) as u32;
            pixels.push(src.get(px, py));
        }
    }
    Some(SixelPixels { width, height, p2: src.p2, palette: src.palette.clone(), pixels })
}

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_scale.rs"]
mod test_sixel_scale;
