//! TUI rendering — pane tree rendering, separator drawing, cursor positioning.
//!
//! Style/color parsing is in `style.rs`; this module re-exports it for
//! backward compatibility so `use crate::rendering::*` still works.

use std::io::{self, Write};
use std::env;
use ratatui::prelude::*;
use ratatui::widgets::*;
use ratatui::style::{Style, Modifier};
use unicode_width::UnicodeWidthStr;
use crossterm::style::Print;
use crossterm::execute;
use portable_pty::PtySize;

use crate::types::{AppState, Mode, Node, LayoutKind};
use crate::tree::split_with_gaps;

// Re-export style utilities so existing `use crate::rendering::*` still works.
pub use crate::style::{
    map_color, parse_tmux_style, parse_inline_styles,
};

// ─── Extended underline styles (issue #589) ─────────────────────────────────

/// ratatui's `Modifier` bitflags define only bits 0 through 8 (`BOLD` through
/// `CROSSED_OUT`) and carry no notion of double, curly, dotted or dashed
/// underscores, which tmux models as `GRID_ATTR_UNDERSCORE_2` through
/// `GRID_ATTR_UNDERSCORE_5` (tmux `grid.h`).  Rather than fork ratatui, psmux
/// smuggles the SGR 4 subparameter (0 through 5) through three unused high
/// bits of the modifier.  `PsmuxBackend::draw` masks them off before anything
/// ratatui owns sees them, and emits the matching `CSI 4:N m` itself.
pub const UL_STYLE_SHIFT: u16 = 12;

/// Mask covering the three smuggled underline-style bits.
pub const UL_STYLE_MASK: u16 = 0b0111_0000_0000_0000;

/// Reads the smuggled underline style back out of a modifier.  Returns the
/// SGR 4 subparameter: 0 none/plain, 1 single, 2 double, 3 curly, 4 dotted,
/// 5 dashed.
#[must_use]
pub fn ul_style_of(m: Modifier) -> u8 {
    ((m.bits() & UL_STYLE_MASK) >> UL_STYLE_SHIFT) as u8
}

/// Strips the smuggled bits so only modifiers ratatui itself defines remain.
#[must_use]
pub fn strip_ul_style(m: Modifier) -> Modifier {
    Modifier::from_bits_truncate(m.bits() & !UL_STYLE_MASK)
}

/// Adds the underline attributes for one cell to a style: the plain
/// `UNDERLINED` modifier (so anything that only knows about SGR 4 still draws
/// a line), the smuggled extended style, and the SGR 58 underline colour.
#[must_use]
pub fn with_underline(mut style: Style, ul: u8, ulcolor: Option<Color>) -> Style {
    if ul == 0 {
        return style;
    }
    style = style.add_modifier(Modifier::UNDERLINED);
    if ul > 1 {
        let bits = (u16::from(ul) << UL_STYLE_SHIFT) & UL_STYLE_MASK;
        style = style.add_modifier(Modifier::from_bits_retain(bits));
    }
    if let Some(c) = ulcolor {
        style = style.underline_color(c);
    }
    style
}

// ─── VT color helpers ───────────────────────────────────────────────────────

pub fn vt_to_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        // Map the 16 standard colors to ratatui named variants so that
        // dim_color() can distinguish individual hues when dimming
        // prediction text.  Note: crossterm 0.29 serialises ALL named
        // colors as 38;5;N (256-color indexed), so the outer terminal
        // sees the same bytes as Color::Indexed(n).
        vt100::Color::Idx(0) => Color::Black,
        vt100::Color::Idx(1) => Color::Red,
        vt100::Color::Idx(2) => Color::Green,
        vt100::Color::Idx(3) => Color::Yellow,
        vt100::Color::Idx(4) => Color::Blue,
        vt100::Color::Idx(5) => Color::Magenta,
        vt100::Color::Idx(6) => Color::Cyan,
        vt100::Color::Idx(7) => Color::Gray,       // index 7 = light gray (SGR 37)
        vt100::Color::Idx(8) => Color::DarkGray,
        vt100::Color::Idx(9) => Color::LightRed,
        vt100::Color::Idx(10) => Color::LightGreen,
        vt100::Color::Idx(11) => Color::LightYellow,
        vt100::Color::Idx(12) => Color::LightBlue,
        vt100::Color::Idx(13) => Color::LightMagenta,
        vt100::Color::Idx(14) => Color::LightCyan,
        vt100::Color::Idx(15) => Color::White,     // index 15 = bright white (SGR 97)
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn dim_color(c: Color) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb((r as u16 * 2 / 5) as u8, (g as u16 * 2 / 5) as u8, (b as u16 * 2 / 5) as u8),
        Color::Black => Color::Rgb(40, 40, 40),
        Color::White | Color::Gray | Color::DarkGray => Color::Rgb(100, 100, 100),
        Color::LightRed => Color::Rgb(150, 80, 80),
        Color::LightGreen => Color::Rgb(80, 150, 80),
        Color::LightYellow => Color::Rgb(150, 150, 80),
        Color::LightBlue => Color::Rgb(80, 120, 180),
        Color::LightMagenta => Color::Rgb(150, 80, 150),
        Color::LightCyan => Color::Rgb(80, 150, 150),
        _ => Color::Rgb(80, 80, 80),
    }
}

pub fn dim_predictions_enabled() -> bool {
    std::env::var("PSMUX_DIM_PREDICTIONS").map(|v| v == "1" || v.to_lowercase() == "true").unwrap_or(false)
}

// ─── Cursor ─────────────────────────────────────────────────────────────────

/// Returns `true` when ConPTY passthrough mode is available (Windows 11 22H2+,
/// build ≥ 22621).  Cached after the first call.
///
/// On Windows 10 (classic ConPTY without passthrough), the child's CSI ?25h
/// (show cursor) is often lost or delayed by the translation layer, which
/// makes the vt100 parser's `hide_cursor` flag unreliable — it gets stuck on
/// `true`.  We only trust `hide_cursor` when passthrough mode is active.
pub fn has_conpty_passthrough() -> bool {
    use std::sync::OnceLock;
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        crate::ssh_input::windows_build_number()
            .map(|b| b >= 22621)
            .unwrap_or(false)
    })
}

/// Resolve the DECSCUSR code (0-6) from the PSMUX_CURSOR_STYLE / PSMUX_CURSOR_BLINK
/// configuration.  Returns 0 ("default") when no explicit style is configured.
///
/// Used as the fallback cursor shape when ConPTY doesn't forward DECSCUSR from
/// the child process (Windows 10 without passthrough mode).
pub fn configured_cursor_code() -> u8 {
    let style = env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string());
    let blink = env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0";
    match style.as_str() {
        "block" => if blink { 1 } else { 2 },
        "underline" => if blink { 3 } else { 4 },
        "bar" | "beam" => if blink { 5 } else { 6 },
        "default" => 0,
        _ => 0,
    }
}

pub fn apply_cursor_style<W: Write>(out: &mut W) -> io::Result<()> {
    let code = configured_cursor_code();
    execute!(out, Print(format!("\x1b[{} q", code)))?;
    Ok(())
}

// ─── Pane tree rendering ────────────────────────────────────────────────────

pub fn render_window(f: &mut Frame, app: &mut AppState, area: Rect) {
    let dim_preds = app.prediction_dimming;
    let border_style = parse_tmux_style(&app.pane_border_style);
    let active_border_style = parse_tmux_style(&app.pane_active_border_style);
    let copy_cursor = if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) { app.copy_pos } else { None };
    // Same resolver as the live client renderer (src/client.rs), so the two
    // paths agree on the tmux `tty_default_colours` fallback (#619).
    let window_styles = crate::client::WindowContentStyles {
        inactive: app.user_options.get("window-style")
            .filter(|s| !s.is_empty())
            .map(|s| parse_tmux_style(s)),
        active: app.user_options.get("window-active-style")
            .filter(|s| !s.is_empty())
            .map(|s| parse_tmux_style(s)),
        inactive_dim: app.user_options.get("window-style")
            .map(|s| crate::style::parse_style_dim(s)).unwrap_or(0),
        active_dim: app.user_options.get("window-active-style")
            .map(|s| crate::style::parse_style_dim(s)).unwrap_or(0),
    };
    let border_status = app.user_options.get("pane-border-status").cloned().unwrap_or_else(|| "off".to_string());
    // tmux ships a non-empty default for pane-border-format, so enabling
    // `pane-border-status top` alone shows the pane title on the border. psmux
    // stored it empty, which made the label gate (below) skip rendering, so the
    // border drew blank. Fall back to the tmux default when unset/empty (#414).
    let border_format = app.user_options.get("pane-border-format")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "#{pane_index} \"#{pane_title}\"".to_string());
    // pane-border-lines (#pane-border-lines): choose the glyph set for the
    // separators drawn between panes. `none` suppresses them entirely.
    let border_lines_name = app.user_options.get("pane-border-lines")
        .cloned()
        .unwrap_or_else(|| crate::border_lines::DEFAULT.to_string());
    let bchars = crate::border_lines::border_chars(&border_lines_name);
    let win = &mut app.windows[app.active_idx];
    let active_rect = compute_active_rect(&win.root, &win.active_path, area);
    render_node(f, &mut win.root, &win.active_path, &mut Vec::new(), area, dim_preds, border_style, active_border_style, copy_cursor, active_rect, window_styles, &border_status, &border_format, &mut 0, bchars);
    let buf_area = f.buffer_mut().area;
    let borders = border_geometry_from_node(&win.root, area, buf_area);
    fix_border_intersections(f.buffer_mut(), bchars, &borders);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BorderOrientation {
    Horizontal,
    Vertical,
}

/// Final separator orientation at each drawn buffer cell.
#[derive(Default)]
pub struct BorderGeometry {
    cells: std::collections::BTreeMap<usize, BorderOrientation>,
}

impl BorderGeometry {
    pub(crate) fn set_vertical(&mut self, idx: usize) {
        self.cells.insert(idx, BorderOrientation::Vertical);
    }

    pub(crate) fn set_horizontal(&mut self, idx: usize) {
        self.cells.insert(idx, BorderOrientation::Horizontal);
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, idx: &usize) -> bool {
        self.cells.contains_key(idx)
    }

    fn has_orientation(&self, idx: usize, orientation: BorderOrientation) -> bool {
        self.cells.get(&idx) == Some(&orientation)
    }
}

/// Replace straight separator glyphs at oriented crossings.
///
/// The returned indices identify every junction, including space-mode
/// junctions whose glyph does not change.
pub fn fix_border_intersections(
    buf: &mut Buffer,
    bchars: Option<crate::border_lines::BorderChars>,
    borders: &BorderGeometry,
) -> Vec<usize> {
    let Some(bc) = bchars else { return Vec::new(); };
    let width = buf.area.width as usize;
    if width == 0 { return Vec::new(); }
    let mut junctions = Vec::new();

    for (&idx, &orientation) in &borders.cells {
        if orientation != BorderOrientation::Vertical {
            continue;
        }
        if idx >= buf.content.len() { continue; }
        let col = idx % width;
        let left = col > 0
            && borders.has_orientation(idx - 1, BorderOrientation::Horizontal);
        let right = col + 1 < width
            && borders.has_orientation(idx + 1, BorderOrientation::Horizontal);
        let glyph = match (left, right) {
            (true, true) => Some(bc.cross),
            (true, false) => Some(bc.right_tee),
            (false, true) => Some(bc.left_tee),
            (false, false) => None,
        };
        if let Some(glyph) = glyph {
            junctions.push(idx);
            if bc.has_distinct_junction_glyphs {
                buf.content[idx].set_char(glyph);
            }
        }
    }

    for (&idx, &orientation) in &borders.cells {
        if orientation != BorderOrientation::Horizontal {
            continue;
        }
        if idx >= buf.content.len() { continue; }
        let row = idx / width;
        let up = row > 0
            && borders.has_orientation(idx - width, BorderOrientation::Vertical);
        let down = row + 1 < buf.area.height as usize
            && borders.has_orientation(idx + width, BorderOrientation::Vertical);
        let glyph = match (up, down) {
            (true, true) => Some(bc.cross),
            (true, false) => Some(bc.bottom_tee),
            (false, true) => Some(bc.top_tee),
            (false, false) => None,
        };
        if let Some(glyph) = glyph {
            junctions.push(idx);
            if bc.has_distinct_junction_glyphs {
                buf.content[idx].set_char(glyph);
            }
        }
    }
    junctions.sort_unstable();
    junctions
}

/// Collect the buffer-cell indices where psmux draws a pane separator,
/// mirroring the separator geometry in [`render_node`]'s `Node::Split` arm
/// (`split_with_gaps` → `rects[i].x + width` / `rects[i].y + height`, spanning
/// the split's own `area`).
///
/// Indices match `Buffer::content` (`(y - buf_area.y) * buf_area.width +
/// (x - buf_area.x)`). Child geometry is recorded before parent separators so
/// the final orientation at an overwritten cell matches rendering order.
pub fn border_geometry_from_node(
    root: &Node,
    area: Rect,
    buf_area: Rect,
) -> BorderGeometry {
    let mut borders = BorderGeometry::default();
    mark_node_borders(root, area, buf_area, &mut borders);
    borders
}

fn mark_node_borders(
    node: &Node,
    area: Rect,
    buf_area: Rect,
    borders: &mut BorderGeometry,
) {
    let Node::Split { kind, sizes, children } = node else { return; };
    let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
        sizes.clone()
    } else { vec![100 / children.len().max(1) as u16; children.len()] };
    let is_horizontal = *kind == LayoutKind::Horizontal;
    let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
    for (i, child) in children.iter().enumerate() {
        if i < rects.len() {
            mark_node_borders(child, rects[i], buf_area, borders);
        }
    }

    let w = buf_area.width as usize;
    for i in 0..children.len().saturating_sub(1) {
        if i >= rects.len() { break; }
        if is_horizontal {
            let sep_x = rects[i].x + rects[i].width;
            if sep_x < buf_area.x + buf_area.width && sep_x >= buf_area.x {
                for y in area.y..area.y + area.height {
                    if y >= buf_area.y && y < buf_area.y + buf_area.height {
                        // Guards above bound y/x to buf_area, so idx < w*h.
                        borders.set_vertical(
                            (y - buf_area.y) as usize * w
                                + (sep_x - buf_area.x) as usize,
                        );
                    }
                }
            }
        } else {
            let sep_y = rects[i].y + rects[i].height;
            if sep_y < buf_area.y + buf_area.height && sep_y >= buf_area.y {
                for x in area.x..area.x + area.width {
                    if x >= buf_area.x && x < buf_area.x + buf_area.width {
                        borders.set_horizontal(
                            (sep_y - buf_area.y) as usize * w
                                + (x - buf_area.x) as usize,
                        );
                    }
                }
            }
        }
    }
}

pub fn render_node(
    f: &mut Frame,
    node: &mut Node,
    active_path: &Vec<usize>,
    cur_path: &mut Vec<usize>,
    area: Rect,
    dim_preds: bool,
    border_style: Style,
    active_border_style: Style,
    copy_cursor: Option<(u16, u16)>,
    active_rect: Option<Rect>,
    window_styles: crate::client::WindowContentStyles,
    border_status: &str,
    border_format: &str,
    pane_idx: &mut usize,
    bchars: Option<crate::border_lines::BorderChars>,
) {
    match node {
        Node::Leaf(pane) => {
            let is_active = *cur_path == *active_path;
            // window-style / window-active-style for this pane, with the tmux
            // per attribute fallback applied for the active pane (#619).
            let ws = window_styles.for_pane(is_active);
            let ws_dim = window_styles.dim_for_pane(is_active);
            // When pane-border-status is enabled, reserve 1 row for the
            // border label so it doesn't overlap pane content (#288).
            let has_border_label = border_status != "off" && !border_format.is_empty() && area.height > 1;
            let inner = if has_border_label {
                if border_status == "top" {
                    Rect::new(area.x, area.y + 1, area.width, area.height - 1)
                } else {
                    Rect::new(area.x, area.y, area.width, area.height - 1)
                }
            } else {
                area
            };
            let target_rows = inner.height.max(1);
            let target_cols = inner.width.max(1);
            if pane.last_rows != target_rows || pane.last_cols != target_cols {
                let _ = pane.master.resize(PtySize { rows: target_rows, cols: target_cols, pixel_width: 0, pixel_height: 0 });
                if let Ok(mut parser) = pane.term.lock() {
                    parser.screen_mut().set_size(target_rows, target_cols);
                }
                pane.last_rows = target_rows;
                pane.last_cols = target_cols;
            }
            let parser_guard = pane.term.lock();
            let Ok(parser) = parser_guard else { return; };
            let screen = parser.screen();
            let (cur_r, cur_c) = screen.cursor_position();
            let mut lines: Vec<Line> = Vec::with_capacity(target_rows as usize);
            for r in 0..target_rows {
                let mut spans: Vec<Span> = Vec::with_capacity(target_cols as usize);
                let mut c = 0;
                while c < target_cols {
                    if let Some(cell) = screen.cell(r, c) {
                        let mut fg = vt_to_color(cell.fgcolor());
                        let mut bg = vt_to_color(cell.bgcolor());
                        // Apply window-style / window-active-style defaults for unset colors
                        if let Some(ws) = ws {
                            if fg == Color::Reset { if let Some(wfg) = ws.fg { fg = wfg; } }
                            if bg == Color::Reset { if let Some(wbg) = ws.bg { bg = wbg; } }
                        }
                        // tmux `tty_attributes` dims the resolved colours by the
                        // window style's `dim=N` percentage (#619 item 4).
                        if ws_dim != 0 {
                            fg = crate::style::dim_colour_percent(fg, ws_dim);
                            bg = crate::style::dim_colour_percent(bg, ws_dim);
                        }
                        if dim_preds && !screen.alternate_screen()
                            && (r > cur_r || (r == cur_r && c >= cur_c))
                        {
                            fg = dim_color(fg);
                        }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if cell.dim() { style = style.add_modifier(Modifier::DIM); }
                        if cell.bold() { style = style.add_modifier(Modifier::BOLD); }
                        if cell.italic() { style = style.add_modifier(Modifier::ITALIC); }
                        style = with_underline(
                            style,
                            cell.underline_style().sgr_subparam(),
                            match cell.underline_color() {
                                vt100::Color::Default => None,
                                c => Some(vt_to_color(c)),
                            },
                        );
                        if cell.inverse() { style = style.add_modifier(Modifier::REVERSED); }
                        if cell.blink() { style = style.add_modifier(Modifier::SLOW_BLINK); }
                        if cell.strikethrough() { style = style.add_modifier(Modifier::CROSSED_OUT); }
                        // ratatui-crossterm 0.1.0 omits SGR 8 from
                        // ModifierDiff, so Modifier::HIDDEN never
                        // reaches the terminal.  Render hidden cells
                        // as spaces instead.
                        let text = if cell.hidden() {
                            " ".to_string()
                        } else {
                            cell.contents().to_string()
                        };
                        let w = UnicodeWidthStr::width(text.as_str()) as u16;
                        if w == 0 {
                            spans.push(Span::styled(" ", style));
                            c += 1;
                        } else if w >= 2 {
                            // Wide char at the last column would overflow the pane boundary
                            if c + w > target_cols {
                                spans.push(Span::styled(" ", style));
                                c += 1;
                            } else {
                                spans.push(Span::styled(text, style));
                                c += 2;
                            }
                        } else {
                            spans.push(Span::styled(text, style));
                            c += 1;
                        }
                    } else {
                        spans.push(Span::raw(" "));
                        c += 1;
                    }
                }
                lines.push(Line::from(spans));
            }
            f.render_widget(Clear, inner);
            let para = Paragraph::new(Text::from(lines));
            f.render_widget(para, inner);
            if is_active {
                let (cr, cc) = copy_cursor.unwrap_or_else(|| screen.cursor_position());
                let cr = cr.min(target_rows.saturating_sub(1));
                let cc = cc.min(target_cols.saturating_sub(1));
                let cx = inner.x + cc;
                let cy = inner.y + cr;
                // Respect the child's cursor-visibility state.
                // TUI apps like Claude draw their own cursor via cell
                // inverse-video and hide the real terminal cursor —
                // honour that so we don't place a stray cursor at
                // ConPTY's parking position.
                if !screen.hide_cursor() {
                    f.set_cursor_position((cx, cy));
                }
            }
            // Pane border format/status overlay
            if has_border_label {
                let pane_label = border_format.replace("#{pane_index}", &pane_idx.to_string())
                    .replace("#P", &pane_idx.to_string())
                    .replace("#{pane_title}", &pane.title);
                let label_width = UnicodeWidthStr::width(pane_label.as_str()) as u16;
                if label_width > 0 && area.width >= label_width {
                    let label_y = if border_status == "bottom" { area.y + area.height.saturating_sub(1) } else { area.y };
                    let label_area = Rect::new(area.x, label_y, label_width.min(area.width), 1);
                    let label_style = if is_active { active_border_style } else { border_style };
                    f.render_widget(Paragraph::new(Line::from(Span::styled(pane_label, label_style))), label_area);
                }
            }
            *pane_idx += 1;
        }
        Node::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else { vec![100 / children.len().max(1) as u16; children.len()] };
            let is_horizontal = *kind == LayoutKind::Horizontal;
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter_mut().enumerate() {
                cur_path.push(i);
                if i < rects.len() {
                    render_node(f, child, active_path, cur_path, rects[i], dim_preds, border_style, active_border_style, copy_cursor, active_rect, window_styles, border_status, border_format, pane_idx, bchars);
                }
                cur_path.pop();
            }
            // Draw separator lines. `pane-border-lines none` (bchars == None)
            // suppresses them entirely.
            let Some(bc) = bchars else { return; };
            let (v_char, h_char) = (bc.vertical, bc.horizontal);
            let buf = f.buffer_mut();
            for i in 0..children.len().saturating_sub(1) {
                if i >= rects.len() { break; }
                let both_leaves = matches!(&children[i], Node::Leaf(_))
                    && matches!(children.get(i + 1), Some(Node::Leaf(_)));

                if is_horizontal {
                    let sep_x = rects[i].x + rects[i].width;
                    if sep_x < buf.area.x + buf.area.width {
                        if both_leaves {
                            let left_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i;
                            let right_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i + 1;
                            let left_sty = if left_active { active_border_style } else { border_style };
                            let right_sty = if right_active { active_border_style } else { border_style };
                            let mid_y = area.y + area.height / 2;
                            for y in area.y..area.y + area.height {
                                let sty = if y < mid_y { left_sty } else { right_sty };
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize + (sep_x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char(v_char);
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        } else {
                            for y in area.y..area.y + area.height {
                                let active = active_rect.map_or(false, |ar| {
                                    y >= ar.y && y < ar.y + ar.height
                                    && (sep_x == ar.x + ar.width || sep_x + 1 == ar.x)
                                });
                                let sty = if active { active_border_style } else { border_style };
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize + (sep_x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char(v_char);
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        }
                    }
                } else {
                    let sep_y = rects[i].y + rects[i].height;
                    if sep_y < buf.area.y + buf.area.height {
                        if both_leaves {
                            let top_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i;
                            let bot_active = cur_path.len() < active_path.len()
                                && active_path[..cur_path.len()] == cur_path[..]
                                && active_path[cur_path.len()] == i + 1;
                            let top_sty = if top_active { active_border_style } else { border_style };
                            let bot_sty = if bot_active { active_border_style } else { border_style };
                            let mid_x = area.x + area.width / 2;
                            for x in area.x..area.x + area.width {
                                let sty = if x < mid_x { top_sty } else { bot_sty };
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize + (x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char(h_char);
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        } else {
                            for x in area.x..area.x + area.width {
                                let active = active_rect.map_or(false, |ar| {
                                    x >= ar.x && x < ar.x + ar.width
                                    && (sep_y == ar.y + ar.height || sep_y + 1 == ar.y)
                                });
                                let sty = if active { active_border_style } else { border_style };
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize + (x - buf.area.x) as usize;
                                if idx < buf.content.len() {
                                    buf.content[idx].set_char(h_char);
                                    buf.content[idx].set_style(sty);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ─── Layout helpers ─────────────────────────────────────────────────────────

/// Compute the rectangle of the active pane by following the active_path through the tree.
fn compute_active_rect(node: &Node, active_path: &[usize], area: Rect) -> Option<Rect> {
    compute_active_rect_pub(node, active_path, area)
}

/// Public version of `compute_active_rect` for use outside the rendering module
/// (e.g. accessibility caret updates).
pub fn compute_active_rect_pub(node: &Node, active_path: &[usize], area: Rect) -> Option<Rect> {
    match node {
        Node::Leaf(_) => Some(area),
        Node::Split { kind, sizes, children } => {
            if active_path.is_empty() || children.is_empty() { return None; }
            let idx = active_path[0];
            if idx >= children.len() { return None; }
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![100 / children.len().max(1) as u16; children.len()]
            };
            let is_horizontal = *kind == LayoutKind::Horizontal;
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            if idx < rects.len() {
                compute_active_rect(&children[idx], &active_path[1..], rects[idx])
            } else {
                None
            }
        }
    }
}

// ─── Status bar convenience wrappers (delegate to style.rs) ─────────────────

/// Expand simple status variables using AppState context.
pub fn expand_status(fmt: &str, app: &AppState, time_str: &str) -> String {
    let window = &app.windows[app.active_idx];
    let win_idx = app.win_display_index(app.active_idx);
    crate::style::expand_status(fmt, &app.session_name, &window.name, win_idx, time_str)
}

/// Parse a status format string with AppState context into styled spans.
pub fn parse_status(fmt: &str, app: &AppState, time_str: &str) -> Vec<Span<'static>> {
    let window = &app.windows[app.active_idx];
    let win_idx = app.win_display_index(app.active_idx);
    crate::style::parse_status(fmt, &app.session_name, &window.name, win_idx, time_str)
}

// ─── UI layout helpers ──────────────────────────────────────────────────────

pub fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    // Clamp requested height to the available area so we never
    // produce a Rect that extends beyond the buffer.
    let clamped_h = height.min(r.height);
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Length(clamped_h),
            Constraint::Percentage(50),
        ])
        .split(r);
    let middle = popup_layout[1];
    let width = (middle.width * percent_x) / 100;
    let x = middle.x + (middle.width - width) / 2;
    // Use the Layout-allocated height, not the raw parameter,
    // to guarantee the rect stays within the parent area.
    let final_h = middle.height.min(clamped_h);
    Rect { x, y: middle.y, width, height: final_h }
}
