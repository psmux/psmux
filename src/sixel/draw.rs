//! The client side of pane sixel images (#431): which images a frame draws,
//! and the bytes that draw them over the frame ratatui has written.

use std::collections::HashMap;

use ratatui::layout::Rect;

use super::decode::{decode, SixelPixels};
use super::encode::encode;
use super::scale::{crop_scale, VisibleCells};

/// A single sixel image to re-emit after the frame is drawn (#431). `x`/`y`
/// are OUTER-terminal 0-based cell coordinates of the top-left corner of the
/// part that is visible; `id` keys into the session-lifetime blob cache;
/// `cw`/`ch` are the whole image's cells; `i`/`j` the first visible cell
/// inside it and `rx`/`ry` how many are visible, the footprint
/// `plan_sixel_frame` repaints when the image moves away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SixelEmit {
    pub x: u16,
    pub y: u16,
    pub id: u64,
    pub cw: u16,
    pub ch: u16,
    pub i: u16,
    pub j: u16,
    pub rx: u16,
    pub ry: u16,
}

impl SixelEmit {
    /// An image drawn whole at `x`, `y`.
    pub(crate) fn whole(x: u16, y: u16, id: u64, cw: u16, ch: u16) -> Self {
        Self { x, y, id, cw, ch, i: 0, j: 0, rx: cw, ry: ch }
    }

    fn is_whole(&self) -> bool {
        self.i == 0 && self.j == 0 && self.rx == self.cw && self.ry == self.ch
    }

    fn part(&self) -> VisibleCells {
        VisibleCells { i: self.i, j: self.j, x: 0, y: 0, rx: self.rx, ry: self.ry }
    }
}

thread_local! {
    /// Sixel image emits collected during the current frame's render, drained
    /// and emitted (gated) after `terminal.draw()`. Mirrors `FRAME_HYPERLINKS`:
    /// thread-local so the recursive `render_layout_json` need not thread a
    /// collector, and so no outer state (the blob cache) is required at render
    /// time — the cache-presence check happens at emit time where it is in scope.
    static FRAME_SIXELS: std::cell::RefCell<Vec<SixelEmit>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn frame_sixels_clear() {
    FRAME_SIXELS.with(|h| h.borrow_mut().clear());
}

pub(crate) fn frame_sixels_push(emit: SixelEmit) {
    FRAME_SIXELS.with(|h| h.borrow_mut().push(emit));
}

pub(crate) fn frame_sixels_take() -> Vec<SixelEmit> {
    FRAME_SIXELS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

/// Decode the blobs a frame ships in `image_blobs` into the client's cache,
/// and return the ids that arrived.
///
/// A server ships a blob once per id, again only after a resync, so whatever
/// arrives replaces what the cache holds for that id: the client may have
/// cached that id from another server (after `switch-client`), whose ids start
/// at 1 as well.  Malformed ids and undecodable base64 are skipped.
pub(crate) fn merge_image_blobs(
    cache: &mut HashMap<u64, Vec<u8>>,
    blobs: &HashMap<String, String>,
) -> Vec<u64> {
    use base64::Engine;
    let mut arrived = Vec::new();
    for (id_str, b64) in blobs {
        let Ok(id) = id_str.parse::<u64>() else { continue };
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
            cache.insert(id, bytes);
            arrived.push(id);
        }
    }
    arrived
}

/// Images decoded from their blobs, and the bytes drawn for a part of one at
/// a cell size, kept so a frame that draws the same again does not decode
/// and encode it again.
#[derive(Default)]
pub(crate) struct SixelDrawCache {
    decoded: HashMap<u64, Option<SixelPixels>>,
    drawn: HashMap<(u64, VisibleCells, (u32, u32)), Vec<u8>>,
}

/// Entries kept before a cache starts over.
const DRAW_CACHE_MAX: usize = 64;

/// Decoded pixels kept before the decoded images start over: two images of
/// the largest size, 64 MiB.
const DECODED_PIXELS_MAX: usize = 2 * (super::decode::MAX_SIDE as usize) * (super::decode::MAX_SIDE as usize);

impl SixelDrawCache {
    /// Drop what was made from `id`'s blob, which a new blob replaced.
    pub(crate) fn forget(&mut self, id: u64) {
        self.decoded.remove(&id);
        self.drawn.retain(|key, _| key.0 != id);
    }

    /// The sixel for the part of `emit`'s image it shows, at `cell` pixels a
    /// cell. `None` when the blob does not decode or the part is empty.
    fn redrawn(&mut self, emit: &SixelEmit, blob: &[u8], cell: (u32, u32)) -> Option<&[u8]> {
        let key = (emit.id, emit.part(), cell);
        if !self.drawn.contains_key(&key) {
            if !self.decoded.contains_key(&emit.id) {
                // Each decoded image can be up to MAX_SIDE pixels a side, 32
                // MiB, so the cache is held to a number of pixels as well as
                // a number of images.
                let pixels: usize = self.decoded.values().flatten().map(|p| p.pixels.len()).sum();
                if self.decoded.len() >= DRAW_CACHE_MAX || pixels >= DECODED_PIXELS_MAX {
                    self.decoded.clear();
                }
            }
            let image = self.decoded.entry(emit.id).or_insert_with(|| decode(blob)).as_ref()?;
            let part = crop_scale(image, vt100::image::DEFAULT_CELL_PX, emit.part(), cell)?;
            if self.drawn.len() >= DRAW_CACHE_MAX {
                self.drawn.clear();
            }
            self.drawn.insert(key, encode(&part));
        }
        self.drawn.get(&key).map(Vec::as_slice)
    }
}

/// What a frame writes for sixel images (#431), from [`plan_sixel_frame`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SixelFramePlan {
    /// Screen cells the images of the last frame covered, to rewrite from the
    /// frame just drawn so the pixels left there are covered.
    pub repaint: Vec<Rect>,
    /// The images to draw, on top of the frame.
    pub draw: Vec<SixelEmit>,
}

/// Decide what this frame writes for sixel images (#431).
///
/// ratatui writes only the cells that changed, so an image stays on the
/// outer terminal until something is written over it, and writing it again
/// every frame only made it flicker.  Nothing is written while the images
/// and the screen size are what the last frame left (`None`).
///
/// Otherwise only what changed is written.  The cells under the parts that
/// went away are rewritten from the frame just drawn, since a part that
/// moved or went away leaves its pixels behind on cells whose text did not
/// change.  Then the new parts are drawn, and any part still in place whose
/// cells that rewrite covered, because the terminal erases an image under
/// text written over it.  A line typed beside an image's partly erased last
/// row changes one small part each key, and the rest of the image is left
/// alone.  After a resize ratatui has cleared the whole screen, so every
/// part is drawn and nothing is rewritten.
///
/// The part of an image that is gone, inside a new part of a newer image
/// (ids only grow), is not rewritten: the newer image is drawn over it, as
/// the pane's console host draws a newer sixel over an older one.  The pane
/// drops an image under a newer one where the newer one sets every pixel the
/// older one set, so nothing of it is left showing.  Rewriting its cells
/// first blanked the image until the new one was drawn, and a GIF chafa
/// plays in one place flickered on every frame.  A part of an image still
/// there, moved or cut, is rewritten as before.
pub(crate) fn plan_sixel_frame(
    last: &[SixelEmit],
    last_area: Rect,
    now: &[SixelEmit],
    area: Rect,
) -> Option<SixelFramePlan> {
    if now == last && area == last_area {
        return None;
    }
    if area != last_area {
        return Some(SixelFramePlan { repaint: Vec::new(), draw: now.to_vec() });
    }
    let rect = |e: &SixelEmit| Rect::new(e.x, e.y, e.rx, e.ry);
    let inside = |r: Rect, outer: Rect| {
        r.x >= outer.x && r.y >= outer.y && r.right() <= outer.right() && r.bottom() <= outer.bottom()
    };
    let drawn_over = |gone: &SixelEmit| {
        !now.iter().any(|e| e.id == gone.id)
            && now.iter().any(|e| e.id > gone.id && !last.contains(e) && inside(rect(gone), rect(e)))
    };
    let repaint: Vec<Rect> = last
        .iter()
        .filter(|e| !now.contains(e) && !drawn_over(e))
        .map(rect)
        .map(|r| r.intersection(area))
        .filter(|r| r.width > 0 && r.height > 0)
        .collect();
    let draw: Vec<SixelEmit> = now
        .iter()
        .filter(|e| !last.contains(e) || repaint.iter().any(|r| r.intersects(rect(e))))
        .cloned()
        .collect();
    Some(SixelFramePlan { repaint, draw })
}

/// Build the raw byte stream that overlays the collected sixel images on top of
/// an already-drawn frame (#431). For each emit whose blob id is present in the
/// cache: DECSC (save cursor); CUP to 1-based (y+1, x+1); the sixel; DECRC
/// (restore cursor). Returns EMPTY when there is nothing to draw or no emit's
/// id is cached — the caller gates emission on the `sixel` option separately.
/// Bytes (not String) because the blob is arbitrary binary.
///
/// A whole image at the 10 by 20 pixel cell the pane laid it out in is the
/// blob verbatim. Anything else is the visible part cut out and resampled to
/// `cell`, the terminal's cell size, as tmux's `tty_cmd_sixelimage` draws
/// every image (tty.c:2180-2227 in tmux 3.8): the image keeps its cells and
/// only the pixels in a cell change.
pub(crate) fn build_sixel_overlay(
    emits: &[SixelEmit],
    blobs: &HashMap<u64, Vec<u8>>,
    cache: &mut SixelDrawCache,
    cell: (u32, u32),
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for emit in emits {
        let Some(blob) = blobs.get(&emit.id) else {
            continue; // blob not shipped/decoded yet; skip this frame
        };
        let as_laid_out = cell == vt100::image::DEFAULT_CELL_PX && emit.is_whole();
        let sixel: &[u8] = if as_laid_out {
            blob
        } else {
            match cache.redrawn(emit, blob, cell) {
                Some(bytes) => bytes,
                // A whole image that will not decode is still drawn as it came.
                None if emit.is_whole() => blob,
                None => continue,
            }
        };
        out.extend_from_slice(b"\x1b7"); // DECSC: save cursor
        // CUP to 1-based (row;col).
        out.extend_from_slice(format!("\x1b[{};{}H", emit.y as u32 + 1, emit.x as u32 + 1).as_bytes());
        out.extend_from_slice(sixel);
        out.extend_from_slice(b"\x1b8"); // DECRC: restore cursor
    }
    out
}

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_draw.rs"]
mod test_sixel_draw;
