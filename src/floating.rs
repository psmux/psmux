//! Floating-pane geometry: position keywords, movement, and clamping.
//!
//! Pure helpers for the tmux `new-pane` floating-pane feature. The PTY-backed
//! pane itself is created via `popup::create_popup_pane` (floats reuse the full
//! pane infrastructure); this module owns only the placement math so it can be
//! unit tested in isolation, mirroring `border_lines` / `copy_line_numbers`.
//!
//! Coordinates are content-area cells: `(x, y)` is the float's top-left, `(w, h)`
//! its outer size including the 1-cell border. `win_w`/`win_h` are the window
//! content dimensions the float must stay inside.

/// A directional move step for `-U`/`-D`/`-L`/`-R`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDir { Up, Down, Left, Right }

impl MoveDir {
    pub fn parse(flag: &str) -> Option<MoveDir> {
        match flag {
            "-U" => Some(MoveDir::Up),
            "-D" => Some(MoveDir::Down),
            "-L" => Some(MoveDir::Left),
            "-R" => Some(MoveDir::Right),
            _ => None,
        }
    }
}

/// Recognised `-P` position keywords (tmux-style corner/edge/centre anchors).
pub const POSITIONS: &[&str] = &[
    "top-left", "top-right", "bottom-left", "bottom-right",
    "top", "bottom", "left", "right", "centre", "center",
];

/// Resolve a `-P` position keyword to a top-left `(x, y)` for a `w`x`h` float
/// inside a `win_w`x`win_h` content area. Unknown keywords centre the float.
pub fn resolve_position(pos: &str, win_w: u16, win_h: u16, w: u16, h: u16) -> (u16, u16) {
    let max_x = win_w.saturating_sub(w);
    let max_y = win_h.saturating_sub(h);
    let cx = max_x / 2;
    let cy = max_y / 2;
    match pos.trim().to_ascii_lowercase().as_str() {
        "top-left"     => (0, 0),
        "top-right"    => (max_x, 0),
        "bottom-left"  => (0, max_y),
        "bottom-right" => (max_x, max_y),
        "top"          => (cx, 0),
        "bottom"       => (cx, max_y),
        "left"         => (0, cy),
        "right"        => (max_x, cy),
        // "centre", "center", and anything unrecognised.
        _              => (cx, cy),
    }
}

/// Clamp a float so it stays fully within the `win_w`x`win_h` content area.
/// If the float is larger than the area, it is pinned to the top-left.
pub fn clamp_into(x: u16, y: u16, w: u16, h: u16, win_w: u16, win_h: u16) -> (u16, u16) {
    let max_x = win_w.saturating_sub(w);
    let max_y = win_h.saturating_sub(h);
    (x.min(max_x), y.min(max_y))
}

/// Apply a directional step of `step` cells to `(x, y)`, then clamp into bounds.
pub fn move_step(dir: MoveDir, x: u16, y: u16, w: u16, h: u16, win_w: u16, win_h: u16, step: u16) -> (u16, u16) {
    let (nx, ny) = match dir {
        MoveDir::Up    => (x, y.saturating_sub(step)),
        MoveDir::Down  => (x, y.saturating_add(step)),
        MoveDir::Left  => (x.saturating_sub(step), y),
        MoveDir::Right => (x.saturating_add(step), y),
    };
    clamp_into(nx, ny, w, h, win_w, win_h)
}

/// Default floating-pane outer size when `-w`/`-h` are omitted: a proportion of
/// the window, clamped to sane minimums (tmux defaults popups to ~half-ish).
pub fn default_size(win_w: u16, win_h: u16) -> (u16, u16) {
    let w = ((win_w as u32 * 3 / 4) as u16).clamp(10, win_w.max(10));
    let h = ((win_h as u32 * 3 / 4) as u16).clamp(4, win_h.max(4));
    (w, h)
}

/// The outer rectangle a float is drawn at inside a `area_w`x`area_h`
/// content area, relative to that area: a float larger than the area is cut
/// to it and one past its edge is pulled back in, the way
/// `render_float_overlays` draws it. `None` for a float too small to draw.
/// The client's hit test uses this too, so a press lands on what is shown.
pub fn screen_rect(x: u16, y: u16, w: u16, h: u16, area_w: u16, area_h: u16) -> Option<(u16, u16, u16, u16)> {
    if w < 2 || h < 2 {
        return None;
    }
    let w = w.min(area_w);
    let h = h.min(area_h);
    Some((x.min(area_w.saturating_sub(w)), y.min(area_h.saturating_sub(h)), w, h))
}

/// Where a pointer is on a floating pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatHit {
    /// The content: tmux's `KEYC_MOUSE_LOCATION_PANE`.
    Inside,
    /// One of the border cells: tmux's `KEYC_MOUSE_LOCATION_BORDER`.
    Border,
}

/// Which part of the float drawn at `rect` (see [`screen_rect`]) the cell
/// `(px, py)` is, in the same coordinates. tmux's
/// `server_client_check_mouse_in_pane` (server-client.c) gives every border
/// cell of a floating pane the border location and the rest the pane one; a
/// float without a border is all content.
pub fn hit(rect: (u16, u16, u16, u16), bordered: bool, px: u16, py: u16) -> Option<FloatHit> {
    let (x, y, w, h) = rect;
    if px < x || py < y || px >= x + w || py >= y + h {
        return None;
    }
    let on_border = px == x || py == y || px == x + w - 1 || py == y + h - 1;
    Some(if bordered && on_border { FloatHit::Border } else { FloatHit::Inside })
}

/// tmux's smallest pane (`PANE_MINIMUM`, tmux.h).
pub const PANE_MINIMUM: i32 = 1;

/// A floating pane's content area, as tmux keeps it in the pane's layout
/// cell: the border is one cell outside it on every side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geom {
    pub xoff: i32,
    pub yoff: i32,
    pub sx: i32,
    pub sy: i32,
}

impl Geom {
    /// From psmux's outer `(x, y, w, h)`, border included.
    pub fn from_outer(x: u16, y: u16, w: u16, h: u16) -> Geom {
        Geom { xoff: x as i32 + 1, yoff: y as i32 + 1, sx: w as i32 - 2, sy: h as i32 - 2 }
    }

    /// Back to the outer `(x, y, w, h)`.
    pub fn outer(self) -> (i32, i32, i32, i32) {
        (self.xoff - 1, self.yoff - 1, self.sx + 2, self.sy + 2)
    }
}

/// One step of a border drag, tmux's
/// `cmd_resize_pane_mouse_resize_move_floating` (cmd-resize-pane.c): the
/// pointer went from `last` to `cur` (window cells) while the float had the
/// content area `g`. Which part was grabbed is decided from `last` each time,
/// as tmux does:
/// - a corner resizes both ways, a left or top one moving that edge, and the
///   cell next to a corner on the top and bottom rows counts as the corner;
/// - the left, right and bottom edges resize that way only;
/// - the top edge moves the float.
///
/// `None` when nothing changes, including an edge drag that would make the
/// pane smaller than [`PANE_MINIMUM`] (a corner stops at it instead).
pub fn drag_step(g: Geom, last: (i32, i32), cur: (i32, i32)) -> Option<Geom> {
    let (lx, ly) = last;
    let (x, y) = cur;
    let left = g.xoff - 1;
    let right = g.xoff + g.sx;
    let top = g.yoff - 1;
    let bottom = g.yoff + g.sy;
    let at_left = lx == left || lx == left + 1;
    let at_right = lx == right + 1 || lx == right;
    let new = if at_left && ly == top {
        Geom {
            xoff: x + 1,
            yoff: y + 1,
            sx: (g.sx + (lx - x)).max(PANE_MINIMUM),
            sy: (g.sy + (ly - y)).max(PANE_MINIMUM),
        }
    } else if at_right && ly == top {
        Geom { yoff: y + 1, sx: (x - g.xoff).max(PANE_MINIMUM), sy: (g.sy + (ly - y)).max(PANE_MINIMUM), ..g }
    } else if at_left && ly == bottom {
        let sy = y - g.yoff;
        if sy < PANE_MINIMUM {
            return None;
        }
        Geom { xoff: x + 1, sx: (g.sx + (lx - x)).max(PANE_MINIMUM), sy, ..g }
    } else if at_right && ly == bottom {
        Geom { sx: (x - g.xoff).max(PANE_MINIMUM), sy: (y - g.yoff).max(PANE_MINIMUM), ..g }
    } else if lx == right {
        let sx = x - g.xoff;
        if sx < PANE_MINIMUM {
            return None;
        }
        Geom { sx, ..g }
    } else if lx == left {
        let sx = g.sx + (lx - x);
        if sx < PANE_MINIMUM {
            return None;
        }
        Geom { xoff: x + 1, sx, ..g }
    } else if ly == bottom {
        let sy = y - g.yoff;
        if sy < PANE_MINIMUM {
            return None;
        }
        Geom { sy, ..g }
    } else if ly == top {
        Geom { xoff: g.xoff + (x - lx), yoff: y + 1, ..g }
    } else {
        return None;
    };
    (new != g).then_some(new)
}

/// [`drag_step`]'s result `g` (from `prev`) as psmux's outer `(x, y, w, h)`
/// inside a `win_w`x`win_h` window. psmux keeps a float inside the window
/// (it draws it there, see [`screen_rect`]), where tmux lets it hang over
/// the edge: a move stops at the edges, and a resize stops growing at them.
pub fn fit_outer(prev: Geom, g: Geom, win_w: u16, win_h: u16) -> (u16, u16, u16, u16) {
    let (x, y, w, h) = g.outer();
    let (win_w, win_h) = (win_w as i32, win_h as i32);
    if g.sx == prev.sx && g.sy == prev.sy {
        let x = x.clamp(0, (win_w - w).max(0));
        let y = y.clamp(0, (win_h - h).max(0));
        return (x as u16, y as u16, w as u16, h as u16);
    }
    let x = x.clamp(0, (win_w - (PANE_MINIMUM + 2)).max(0));
    let y = y.clamp(0, (win_h - (PANE_MINIMUM + 2)).max(0));
    let w = w.min(win_w - x).max(PANE_MINIMUM + 2);
    let h = h.min(win_h - y).max(PANE_MINIMUM + 2);
    (x as u16, y as u16, w as u16, h as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_anchor_correctly() {
        // 100x30 window, 40x10 float.
        assert_eq!(resolve_position("top-left", 100, 30, 40, 10), (0, 0));
        assert_eq!(resolve_position("top-right", 100, 30, 40, 10), (60, 0));
        assert_eq!(resolve_position("bottom-left", 100, 30, 40, 10), (0, 20));
        assert_eq!(resolve_position("bottom-right", 100, 30, 40, 10), (60, 20));
    }

    #[test]
    fn centre_and_edges() {
        assert_eq!(resolve_position("centre", 100, 30, 40, 10), (30, 10));
        assert_eq!(resolve_position("center", 100, 30, 40, 10), (30, 10));
        assert_eq!(resolve_position("top", 100, 30, 40, 10), (30, 0));
        assert_eq!(resolve_position("bottom", 100, 30, 40, 10), (30, 20));
        assert_eq!(resolve_position("left", 100, 30, 40, 10), (0, 10));
        assert_eq!(resolve_position("right", 100, 30, 40, 10), (60, 10));
        // Unknown -> centre.
        assert_eq!(resolve_position("bogus", 100, 30, 40, 10), (30, 10));
    }

    #[test]
    fn oversized_float_pins_top_left() {
        assert_eq!(resolve_position("bottom-right", 20, 10, 40, 20), (0, 0));
        assert_eq!(clamp_into(50, 50, 40, 20, 20, 10), (0, 0));
    }

    #[test]
    fn move_steps_and_clamp() {
        // Move up from y=5 by 2 -> y=3.
        assert_eq!(move_step(MoveDir::Up, 10, 5, 40, 10, 100, 30, 2), (10, 3));
        // Move up past the top clamps to 0.
        assert_eq!(move_step(MoveDir::Up, 10, 1, 40, 10, 100, 30, 5), (10, 0));
        // Move right past the edge clamps to max_x = 60.
        assert_eq!(move_step(MoveDir::Right, 55, 5, 40, 10, 100, 30, 20), (60, 5));
        // Move down past the bottom clamps to max_y = 20.
        assert_eq!(move_step(MoveDir::Down, 10, 18, 40, 10, 100, 30, 10), (10, 20));
    }

    #[test]
    fn parse_move_dirs() {
        assert_eq!(MoveDir::parse("-U"), Some(MoveDir::Up));
        assert_eq!(MoveDir::parse("-D"), Some(MoveDir::Down));
        assert_eq!(MoveDir::parse("-L"), Some(MoveDir::Left));
        assert_eq!(MoveDir::parse("-R"), Some(MoveDir::Right));
        assert_eq!(MoveDir::parse("-x"), None);
    }

    #[test]
    fn default_size_is_bounded() {
        let (w, h) = default_size(100, 30);
        assert!(w >= 10 && w <= 100 && h >= 4 && h <= 30);
        // Tiny window still yields the minimums.
        let (w2, h2) = default_size(8, 3);
        assert!(w2 >= 10 && h2 >= 4);
    }
}
