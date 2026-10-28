//! In-memory model for sixel graphics captured from a pane's DCS stream.
//!
//! The parser reconstructs each sixel DCS sequence into a [`SixelImage`] and
//! hands it to [`crate::Screen::add_image`].  The image blob is stored verbatim
//! so the client can re-emit it to a sixel-capable outer terminal; the cell and
//! anchor fields let the server compute a viewport-relative draw position that
//! survives scrolling.  See the sixel design (issue #431) for the full pipeline.

/// Default outer-terminal cell size in pixels, `(width, height)`.
///
/// psmux does not know the host terminal's true cell size today, so v1 uses the
/// xterm / tmux fallback of 10x20.  This only affects cursor-advance and
/// clipping math (how many cells the image occupies), NOT the rasterized pixels:
/// the outer terminal draws the raw bytes at its own real cell size.  A wrong
/// constant costs at most a slightly-off cursor-below position and a
/// conservative clip.
pub const DEFAULT_CELL_PX: (u32, u32) = (10, 20);

/// Computes an image's cell footprint from its pixel size using ceil-division
/// against [`DEFAULT_CELL_PX`].  A non-empty image always occupies at least one
/// cell in each axis; the result is clamped into `u16`.
#[must_use]
pub fn cell_dims_from_px(px_width: u32, px_height: u32) -> (u16, u16) {
    let (cell_px_w, cell_px_h) = DEFAULT_CELL_PX;
    let cells_w = px_width.div_ceil(cell_px_w.max(1)).max(1);
    let cells_h = px_height.div_ceil(cell_px_h.max(1)).max(1);
    (
        u16::try_from(cells_w).unwrap_or(u16::MAX),
        u16::try_from(cells_h).unwrap_or(u16::MAX),
    )
}

/// Source of image ids for every pane in the process.  The server ships each
/// blob once per id and the client caches blobs by id, so an id must not be
/// handed out twice: a counter per screen started every pane, and every RIS,
/// at 1 again, and pane B's first image was then never shipped because pane
/// A's had been.
static NEXT_IMAGE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Takes the next unused image id (never 0).
pub(crate) fn next_image_id() -> u64 {
    NEXT_IMAGE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A single sixel image anchored at a logical position on a pane's grid.
#[derive(Clone, Debug)]
pub struct SixelImage {
    /// Id unique in the process (see [`next_image_id`]).  The blob is shipped
    /// to the client once and referenced by id thereafter.
    pub id: u64,
    /// Re-emit bytes: `ESC P` + params + intermediates + `q` + a raster
    /// command bounding the background to the image + payload + `ST`.
    /// Writable verbatim to a sixel-capable terminal after a cursor move.
    pub raw: Vec<u8>,
    /// Image width in pixels: the widest band, or the raster width if larger.
    pub px_width: u32,
    /// Image height in pixels at the image's aspect ratio: the bottom of the
    /// last band that draws, or the raster height if larger.
    pub px_height: u32,
    /// Cell width = `ceil(px_width / cell_px_w)` (see [`DEFAULT_CELL_PX`]).
    pub cell_width: u16,
    /// Cell height = `ceil(px_height / cell_px_h)` (see [`DEFAULT_CELL_PX`]).
    pub cell_height: u16,
    /// Absolute logical line of the top-left cell; survives scrolling and is
    /// pruned when it falls off retained scrollback.
    pub anchor_line: u64,
    /// Grid column of the top-left cell at emit time.
    pub anchor_col: u16,
    /// Grid ownership: images tagged `on_alt` belong to the alternate screen
    /// and are dropped when it exits.
    pub on_alt: bool,
    /// The image's cells that text or an erase has since been written over,
    /// row major, `cell_width * cell_height`; empty while there are none.
    pub erased: Vec<bool>,
}

impl SixelImage {
    /// Mark the cells `col0..col1` of the image's row `row` as written over.
    ///
    /// Windows Terminal, whose sixel parser the pane's `OpenConsole` runs,
    /// erases the part of an image under any text or erase written after it,
    /// cell by cell (`TextBuffer::Replace` and `FillRect` call
    /// `ImageSlice::EraseCells`, src/buffer/out/textBuffer.cpp), so the
    /// program's next line can sit on the image's last, partly filled row.
    pub fn erase_cells(&mut self, row: u16, col0: u16, col1: u16) {
        let (w, h) = (usize::from(self.cell_width), usize::from(self.cell_height));
        let (row, col0, col1) = (usize::from(row), usize::from(col0), usize::from(col1).min(w));
        if row >= h || col0 >= col1 {
            return;
        }
        if self.erased.is_empty() {
            self.erased = vec![false; w * h];
        }
        self.erased[row * w + col0..row * w + col1].fill(true);
    }

    /// Whether the cell at `row`, `col` of the image has been written over.
    #[must_use]
    pub fn is_erased(&self, row: u16, col: u16) -> bool {
        let (row, col) = (usize::from(row), usize::from(col));
        col < usize::from(self.cell_width)
            && self.erased.get(row * usize::from(self.cell_width) + col).copied().unwrap_or(false)
    }

    /// Whether every cell of the image has been written over.
    #[must_use]
    pub fn is_all_erased(&self) -> bool {
        !self.erased.is_empty() && self.erased.iter().all(|&e| e)
    }
}
