//! The client side of pane sixel images (#431): which images a frame draws,
//! and the bytes that draw them over the frame ratatui has written.

use ratatui::layout::Rect;

/// A single sixel image to re-emit after the frame is drawn (#431). `x`/`y`
/// are OUTER-terminal 0-based cell coordinates of the image's top-left corner;
/// `id` keys into the session-lifetime blob cache; `cw`/`ch` are the cell
/// footprint, which `plan_sixel_frame` repaints when the image moves away.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SixelEmit {
    pub x: u16,
    pub y: u16,
    pub id: u64,
    pub cw: u16,
    pub ch: u16,
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

/// Decode the blobs a frame ships in `image_blobs` into the client's cache.
///
/// A server ships a blob once per id, again only after a resync, so whatever
/// arrives replaces what the cache holds for that id: the client may have
/// cached that id from another server (after `switch-client`), whose ids start
/// at 1 as well.  Malformed ids and undecodable base64 are skipped.
pub(crate) fn merge_image_blobs(
    cache: &mut std::collections::HashMap<u64, Vec<u8>>,
    blobs: &std::collections::HashMap<String, String>,
) {
    use base64::Engine;
    for (id_str, b64) in blobs {
        let Ok(id) = id_str.parse::<u64>() else { continue };
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
            cache.insert(id, bytes);
        }
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
/// and the screen size are what the last frame left (`None`).  Otherwise
/// every image is drawn again, and the cells under the last frame's images
/// are rewritten first, since an image that moved or went away leaves its
/// pixels behind on cells whose text did not change.  After a resize ratatui
/// has cleared the whole screen, so there is nothing to rewrite.
pub(crate) fn plan_sixel_frame(
    last: &[SixelEmit],
    last_area: Rect,
    now: &[SixelEmit],
    area: Rect,
) -> Option<SixelFramePlan> {
    if now == last && area == last_area {
        return None;
    }
    let repaint = if area == last_area {
        last.iter()
            .map(|e| Rect::new(e.x, e.y, e.cw, e.ch).intersection(area))
            .filter(|r| r.width > 0 && r.height > 0)
            .collect()
    } else {
        Vec::new()
    };
    Some(SixelFramePlan { repaint, draw: now.to_vec() })
}

/// Build the raw byte stream that overlays the collected sixel images on top of
/// an already-drawn frame (#431). For each emit whose blob id is present in the
/// cache: DECSC (save cursor); CUP to 1-based (y+1, x+1); the raw ESC P … ST
/// sixel bytes verbatim; DECRC (restore cursor). Returns EMPTY when there is
/// nothing to draw or no emit's id is cached — the caller gates emission on the
/// `sixel` option separately. Bytes (not String) because the blob is arbitrary
/// binary. Pure function so it can be unit-tested.
pub(crate) fn build_sixel_overlay(
    emits: &[SixelEmit],
    cache: &std::collections::HashMap<u64, Vec<u8>>,
) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for emit in emits {
        let Some(blob) = cache.get(&emit.id) else {
            continue; // blob not shipped/decoded yet; skip this frame
        };
        out.extend_from_slice(b"\x1b7"); // DECSC: save cursor
        // CUP to 1-based (row;col).
        out.extend_from_slice(format!("\x1b[{};{}H", emit.y as u32 + 1, emit.x as u32 + 1).as_bytes());
        out.extend_from_slice(blob); // raw ESC P … ST sixel bytes verbatim
        out.extend_from_slice(b"\x1b8"); // DECRC: restore cursor
    }
    out
}

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_draw.rs"]
mod test_sixel_draw;
