//! Live preview helpers for the choose-tree / choose-session pickers.
//!
//! Fetches `capture-pane` output from any reachable session via TCP and
//! caches results briefly so navigation through the picker stays snappy.
//!
//! See issue #257 (preview support like tmux's `screen_write_preview`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::session::{fetch_authed_response_multi, read_session_key};
use crate::util::LayoutSimple;

/// Cache key: "session\twin_id\tpane_id" (pane_id == usize::MAX means
/// "use the active pane of the targeted window").
pub type PreviewCache = HashMap<String, (String, Instant)>;

pub const PREVIEW_TTL: Duration = Duration::from_millis(1500);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(150);
const READ_TIMEOUT: Duration = Duration::from_millis(400);

pub fn cache_key(sess: &str, win_id: usize, pane_id: usize) -> String {
    format!("{}\t{}\t{}", sess, win_id, pane_id)
}

/// Fetch capture-pane text for the given target. Returns None if the
/// session is not reachable or the response is empty.
///
/// `pane_id == usize::MAX` => target the window only (server captures the
/// active pane). Otherwise targets a specific pane id within the window.
pub fn fetch_pane_preview(sess: &str, win_id: usize, pane_id: usize) -> Option<String> {
    let port_path = crate::paths::port_file(sess);
    let port: u16 = std::fs::read_to_string(&port_path).ok()?.trim().parse().ok()?;
    let key = read_session_key(sess).ok()?;
    let target = if pane_id == usize::MAX {
        format!(":@{}", win_id)
    } else {
        format!(":@{}.%{}", win_id, pane_id)
    };
    // Use -e to preserve SGR escape sequences so the preview can show
    // colors and attributes like the real pane (issue #257 follow-up).
    let cmd = format!("capture-pane -e -p -t {}\n", target);
    let resp = fetch_authed_response_multi(
        &format!("127.0.0.1:{}", port),
        &key,
        cmd.as_bytes(),
        CONNECT_TIMEOUT,
        READ_TIMEOUT,
    )?;
    if resp.trim().is_empty() {
        None
    } else {
        Some(resp)
    }
}

/// Get a preview, using the cache if fresh, fetching otherwise.
pub fn get_or_fetch(
    cache: &mut PreviewCache,
    sess: &str,
    win_id: usize,
    pane_id: usize,
) -> Option<String> {
    let key = cache_key(sess, win_id, pane_id);
    if let Some((text, ts)) = cache.get(&key) {
        if ts.elapsed() < PREVIEW_TTL {
            return Some(text.clone());
        }
    }
    let text = fetch_pane_preview(sess, win_id, pane_id)?;
    cache.insert(key, (text.clone(), Instant::now()));
    Some(text)
}

/// Render preview text into a Vec of lines clipped to the given dimensions.
/// Strips trailing whitespace and keeps the most recent (bottom) `height`
/// non-empty lines so the active prompt is visible.
pub fn clip_lines(text: &str, width: u16, height: u16) -> Vec<String> {
    let max_w = width as usize;
    let max_h = height as usize;
    if max_h == 0 || max_w == 0 {
        return Vec::new();
    }
    // Split, trim trailing whitespace, drop the trailing empty noise but
    // keep blank lines that appear between content.
    let raw: Vec<&str> = text.split('\n').collect();
    // Trim trailing empty lines so the last visible line is real content.
    let mut end = raw.len();
    while end > 0 && raw[end - 1].trim_end().is_empty() {
        end -= 1;
    }
    let slice = &raw[..end];
    let start = slice.len().saturating_sub(max_h);
    slice[start..]
        .iter()
        .map(|l| {
            let t = l.trim_end_matches(['\r', ' ', '\t'][..].as_ref());
            // Truncate by characters to avoid splitting on a UTF-8 boundary.
            let mut out = String::new();
            let mut w = 0;
            for ch in t.chars() {
                // Crude width: 1 per char. ratatui will handle wide chars
                // when the Paragraph is rendered.
                if w + 1 > max_w {
                    break;
                }
                out.push(ch);
                w += 1;
            }
            out
        })
        .collect()
}

// ---------------------------------------------------------------------
// ANSI SGR -> ratatui Spans (issue #257 follow-up: faithful preview)
// ---------------------------------------------------------------------

fn sgr_color_from_8bit(n: u8) -> Color {
    match n {
        0 => Color::Black, 1 => Color::Red, 2 => Color::Green, 3 => Color::Yellow,
        4 => Color::Blue, 5 => Color::Magenta, 6 => Color::Cyan, 7 => Color::Gray,
        8 => Color::DarkGray, 9 => Color::LightRed, 10 => Color::LightGreen,
        11 => Color::LightYellow, 12 => Color::LightBlue, 13 => Color::LightMagenta,
        14 => Color::LightCyan, 15 => Color::White,
        n => Color::Indexed(n),
    }
}

fn apply_sgr(style: &mut Style, params: &[u32]) {
    let mut i = 0;
    while i < params.len() {
        let p = params[i];
        match p {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            5 | 6 => *style = style.add_modifier(Modifier::SLOW_BLINK),
            7 => *style = style.add_modifier(Modifier::REVERSED),
            9 => *style = style.add_modifier(Modifier::CROSSED_OUT),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            25 => *style = style.remove_modifier(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
            27 => *style = style.remove_modifier(Modifier::REVERSED),
            29 => *style = style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => *style = style.fg(sgr_color_from_8bit((p - 30) as u8)),
            39 => *style = style.fg(Color::Reset),
            40..=47 => *style = style.bg(sgr_color_from_8bit((p - 40) as u8)),
            49 => *style = style.bg(Color::Reset),
            90..=97 => *style = style.fg(sgr_color_from_8bit((p - 90 + 8) as u8)),
            100..=107 => *style = style.bg(sgr_color_from_8bit((p - 100 + 8) as u8)),
            38 | 48 => {
                if let Some(&kind) = params.get(i + 1) {
                    if kind == 5 {
                        if let Some(&n) = params.get(i + 2) {
                            let col = if n <= 255 { Color::Indexed(n as u8) } else { Color::Reset };
                            *style = if p == 38 { style.fg(col) } else { style.bg(col) };
                            i += 2;
                        }
                    } else if kind == 2 {
                        if let (Some(&r), Some(&g), Some(&b)) =
                            (params.get(i + 2), params.get(i + 3), params.get(i + 4))
                        {
                            let col = Color::Rgb(r as u8, g as u8, b as u8);
                            *style = if p == 38 { style.fg(col) } else { style.bg(col) };
                            i += 4;
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn parse_sgr_line(line: &str, max_width: usize, style: &mut Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut width = 0usize;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                let mut params_buf = String::new();
                let mut final_byte = '\0';
                while let Some(c) = chars.next() {
                    if c.is_ascii_digit() || c == ';' || c == ':' {
                        params_buf.push(c);
                    } else {
                        final_byte = c;
                        break;
                    }
                }
                if final_byte == 'm' {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), *style));
                    }
                    let params: Vec<u32> = if params_buf.is_empty() {
                        vec![0]
                    } else {
                        params_buf
                            .split(|c| c == ';' || c == ':')
                            .map(|s| s.parse::<u32>().unwrap_or(0))
                            .collect()
                    };
                    apply_sgr(style, &params);
                }
            } else {
                let _ = chars.next();
            }
            continue;
        }
        if ch == '\r' { continue; }
        if (ch as u32) < 0x20 { continue; }
        if width + 1 > max_width { break; }
        buf.push(ch);
        width += 1;
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, *style));
    }
    spans
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(c) = chars.next() {
                    if !(c.is_ascii_digit() || c == ';' || c == ':') { break; }
                }
            } else { let _ = chars.next(); }
            continue;
        }
        out.push(ch);
    }
    out
}

/// Parse `capture-pane -e -p` output into ratatui Lines, taking the
/// most recent `height` non-empty lines and clipping each to `width`.
pub fn parse_ansi_lines(text: &str, width: u16, height: u16) -> Vec<Line<'static>> {
    let max_w = width as usize;
    let max_h = height as usize;
    if max_w == 0 || max_h == 0 { return Vec::new(); }
    let raw: Vec<&str> = text.split('\n').collect();
    let mut end = raw.len();
    while end > 0 && strip_ansi(raw[end - 1]).trim_end().is_empty() {
        end -= 1;
    }
    let slice = &raw[..end];
    let start = slice.len().saturating_sub(max_h);
    let mut style = Style::default();
    for l in &slice[..start] {
        let _ = parse_sgr_line(l, usize::MAX, &mut style);
    }
    slice[start..]
        .iter()
        .map(|l| Line::from(parse_sgr_line(l, max_w, &mut style)))
        .collect()
}

// ---------------------------------------------------------------------
// Layout-aware preview (issue #257 follow-up): show every pane in a
// window with its real split layout, mirroring tmux's
// `screen_write_preview` per pane in `window_tree_draw_window`.
// ---------------------------------------------------------------------

/// Cache for window layout JSON keyed by "sess\twin_id".
pub type LayoutCache = HashMap<String, (LayoutSimple, Instant)>;

pub const LAYOUT_TTL: Duration = Duration::from_millis(2500);

/// Fetch the simplified layout for a window in any session via TCP.
pub fn fetch_window_layout(sess: &str, win_id: usize) -> Option<LayoutSimple> {
    let port_path = crate::paths::port_file(sess);
    let port: u16 = std::fs::read_to_string(&port_path).ok()?.trim().parse().ok()?;
    let key = read_session_key(sess).ok()?;
    let cmd = format!("window-layout {}\n", win_id);
    let resp = fetch_authed_response_multi(
        &format!("127.0.0.1:{}", port),
        &key,
        cmd.as_bytes(),
        CONNECT_TIMEOUT,
        READ_TIMEOUT,
    )?;
    let trimmed = resp.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    serde_json::from_str::<LayoutSimple>(trimmed).ok()
}

pub fn get_or_fetch_layout(
    cache: &mut LayoutCache,
    sess: &str,
    win_id: usize,
) -> Option<LayoutSimple> {
    let key = format!("{}\t{}", sess, win_id);
    if let Some((layout, ts)) = cache.get(&key) {
        if ts.elapsed() < LAYOUT_TTL {
            return Some(layout.clone());
        }
    }
    let layout = fetch_window_layout(sess, win_id)?;
    cache.insert(key, (layout.clone(), Instant::now()));
    Some(layout)
}

/// Recursively split a Rect according to the layout tree, returning a
/// list of (pane_id, active, area) tuples for every leaf.
///
/// `Horizontal` => children laid out left/right (columns), `Vertical`
/// => top/bottom (rows). One cell between children is reserved as a
/// separator so the user can see the split structure.
pub fn flatten_layout_to_rects(
    layout: &LayoutSimple,
    area: ratatui::layout::Rect,
) -> Vec<(usize, bool, ratatui::layout::Rect)> {
    use ratatui::layout::Rect;
    let mut out: Vec<(usize, bool, Rect)> = Vec::new();
    fn rec(node: &LayoutSimple, area: Rect, out: &mut Vec<(usize, bool, Rect)>) {
        match node {
            LayoutSimple::Leaf { id, active } => {
                if area.width > 0 && area.height > 0 {
                    out.push((*id, *active, area));
                }
            }
            LayoutSimple::Split { kind, sizes, children } => {
                if children.is_empty() {
                    return;
                }
                let total: u32 = sizes.iter().map(|s| *s as u32).sum::<u32>().max(1);
                let is_horiz = kind.as_str() == "Horizontal";
                let span = if is_horiz { area.width as u32 } else { area.height as u32 };
                let sep_count = children.len().saturating_sub(1) as u32;
                let usable = span.saturating_sub(sep_count);
                let n = children.len();
                let mut alloc: Vec<u32> = sizes.iter().take(n)
                    .map(|s| (*s as u32 * usable) / total)
                    .collect();
                while alloc.len() < n { alloc.push(0); }
                let used: u32 = alloc.iter().sum();
                let mut leftover = usable.saturating_sub(used);
                let alen = alloc.len();
                let mut idx = 0;
                while leftover > 0 && alen > 0 {
                    alloc[idx % alen] += 1;
                    idx += 1;
                    leftover -= 1;
                }
                let mut cursor: u32 = 0;
                for (i, child) in children.iter().enumerate() {
                    let size = alloc[i] as u16;
                    let sub = if is_horiz {
                        Rect { x: area.x + cursor as u16, y: area.y, width: size, height: area.height }
                    } else {
                        Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: size }
                    };
                    rec(child, sub, out);
                    cursor += size as u32;
                    if i + 1 < children.len() {
                        cursor += 1;
                    }
                }
            }
        }
    }
    rec(layout, area, &mut out);
    out
}

/// Compute separator line segments (vertical and horizontal) drawn
/// between children of every Split node, for visual delineation.
/// Returns a list of (Rect, is_vertical) where Rect is a 1-cell wide
/// vertical bar or 1-cell tall horizontal bar.
pub fn layout_separators(
    layout: &LayoutSimple,
    area: ratatui::layout::Rect,
) -> Vec<(ratatui::layout::Rect, bool)> {
    use ratatui::layout::Rect;
    let mut out: Vec<(Rect, bool)> = Vec::new();
    fn rec(node: &LayoutSimple, area: Rect, out: &mut Vec<(Rect, bool)>) {
        if let LayoutSimple::Split { kind, sizes, children } = node {
            if children.is_empty() { return; }
            let total: u32 = sizes.iter().map(|s| *s as u32).sum::<u32>().max(1);
            let is_horiz = kind.as_str() == "Horizontal";
            let span = if is_horiz { area.width as u32 } else { area.height as u32 };
            let sep_count = children.len().saturating_sub(1) as u32;
            let usable = span.saturating_sub(sep_count);
            let n = children.len();
            let mut alloc: Vec<u32> = sizes.iter().take(n)
                .map(|s| (*s as u32 * usable) / total)
                .collect();
            while alloc.len() < n { alloc.push(0); }
            let used: u32 = alloc.iter().sum();
            let mut leftover = usable.saturating_sub(used);
            let alen = alloc.len();
            let mut idx = 0;
            while leftover > 0 && alen > 0 {
                alloc[idx % alen] += 1;
                idx += 1;
                leftover -= 1;
            }
            let mut cursor: u32 = 0;
            for (i, child) in children.iter().enumerate() {
                let size = alloc[i] as u16;
                let sub = if is_horiz {
                    Rect { x: area.x + cursor as u16, y: area.y, width: size, height: area.height }
                } else {
                    Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: size }
                };
                rec(child, sub, out);
                cursor += size as u32;
                if i + 1 < children.len() {
                    if is_horiz {
                        out.push((Rect { x: area.x + cursor as u16, y: area.y, width: 1, height: area.height }, true));
                    } else {
                        out.push((Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: 1 }, false));
                    }
                    cursor += 1;
                }
            }
        }
    }
    rec(layout, area, &mut out);
    out
}

// ---------------------------------------------------------------------
// Full styled preview (issue #257 follow-up): reuse the same rich
// `LayoutJson` (with `rows_v2` cell runs) the main viewport renders,
// instead of replaying `capture-pane -e` per pane and parsing ANSI.
// This fixes two problems at once:
//   1. `capture-pane -t :@W.%P` resolved through transient -t focus and
//      could return the active pane's content for every pane id, so all
//      preview cells showed the same buffer.
//   2. The hand-rolled ANSI pipeline duplicated rendering logic that
//      already exists in the main viewport (border, color, attribute
//      handling). Sharing the structured `LayoutJson` keeps previews
//      visually identical to the real window.
// ---------------------------------------------------------------------

/// Cache for full layout dumps keyed by "sess\twin_id".
pub type DumpCache = HashMap<String, (crate::layout::LayoutJson, Instant)>;

pub const DUMP_TTL: Duration = Duration::from_millis(1500);

/// Fetch the full styled layout (rows_v2) for a window in any session
/// via TCP using the new `window-dump` command.
pub fn fetch_window_dump(sess: &str, win_id: usize) -> Option<crate::layout::LayoutJson> {
    let port_path = crate::paths::port_file(sess);
    let port: u16 = std::fs::read_to_string(&port_path).ok()?.trim().parse().ok()?;
    let key = read_session_key(sess).ok()?;
    let cmd = format!("window-dump {}\n", win_id);
    let resp = fetch_authed_response_multi(
        &format!("127.0.0.1:{}", port),
        &key,
        cmd.as_bytes(),
        CONNECT_TIMEOUT,
        READ_TIMEOUT,
    )?;
    let trimmed = resp.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return None;
    }
    serde_json::from_str::<crate::layout::LayoutJson>(trimmed).ok()
}

pub fn get_or_fetch_dump(
    cache: &mut DumpCache,
    sess: &str,
    win_id: usize,
) -> Option<crate::layout::LayoutJson> {
    let key = format!("{}\t{}", sess, win_id);
    if let Some((layout, ts)) = cache.get(&key) {
        if ts.elapsed() < DUMP_TTL {
            return Some(layout.clone());
        }
    }
    let layout = fetch_window_dump(sess, win_id)?;
    cache.insert(key, (layout.clone(), Instant::now()));
    Some(layout)
}

/// Map a vt100-style color name (as emitted by `crate::util::color_to_name`)
/// plus a `flags` bitfield into a ratatui Style. Mirrors the inline match
/// in the main viewport's render path so previews look identical.
fn run_style(fg: &str, bg: &str, flags: u8) -> Style {
    let mut style = Style::default()
        .fg(crate::style::map_color(fg))
        .bg(crate::style::map_color(bg));
    if flags & 1 != 0 { style = style.add_modifier(Modifier::DIM); }
    if flags & 2 != 0 { style = style.add_modifier(Modifier::BOLD); }
    if flags & 4 != 0 { style = style.add_modifier(Modifier::ITALIC); }
    if flags & 8 != 0 { style = style.add_modifier(Modifier::UNDERLINED); }
    if flags & 16 != 0 { style = style.add_modifier(Modifier::REVERSED); }
    if flags & 32 != 0 { style = style.add_modifier(Modifier::SLOW_BLINK); }
    if flags & 128 != 0 { style = style.add_modifier(Modifier::CROSSED_OUT); }
    style
}

/// Convert one row's run list into ratatui Spans, clipping to `width`.
/// Pads the tail with a space-styled span so the line fills the inside
/// rect (matches the main renderer behavior).
pub fn render_runs_line(
    runs: &[crate::layout::CellRunJson],
    width: u16,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut c: u16 = 0;
    let mut last_bg = Color::Reset;
    for run in runs {
        if c >= width { break; }
        let style = run_style(&run.fg, &run.bg, run.flags);
        last_bg = style.bg.unwrap_or(Color::Reset);
        // Hidden cells render as spaces to match the main view's behavior.
        let text: &str = if run.flags & 64 != 0 {
            " "
        } else if run.text.is_empty() {
            " "
        } else {
            &run.text
        };
        let run_w = run.width.max(1);
        if c + run_w > width {
            // Truncate to the available width.
            let avail = (width - c) as usize;
            let mut truncated = String::new();
            let mut used = 0usize;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                if used + cw > avail { break; }
                used += cw;
                truncated.push(ch);
            }
            if !truncated.is_empty() {
                spans.push(Span::styled(truncated, style));
                c += avail as u16;
            }
            break;
        } else {
            spans.push(Span::styled(text.to_string(), style));
            c += run_w;
        }
    }
    if c < width {
        let pad = " ".repeat((width - c) as usize);
        spans.push(Span::styled(pad, Style::default().bg(last_bg)));
    }
    Line::from(spans)
}

/// Walk a `LayoutJson` tree and produce per-leaf rectangles (same
/// algorithm as `flatten_layout_to_rects` but for the rich tree).
pub fn flatten_dump_rects<'a>(
    layout: &'a crate::layout::LayoutJson,
    area: ratatui::layout::Rect,
) -> Vec<(&'a crate::layout::LayoutJson, ratatui::layout::Rect)> {
    use ratatui::layout::Rect;
    let mut out: Vec<(&crate::layout::LayoutJson, Rect)> = Vec::new();
    fn rec<'b>(
        node: &'b crate::layout::LayoutJson,
        area: Rect,
        out: &mut Vec<(&'b crate::layout::LayoutJson, Rect)>,
    ) {
        match node {
            crate::layout::LayoutJson::Leaf { .. } => {
                if area.width > 0 && area.height > 0 {
                    out.push((node, area));
                }
            }
            crate::layout::LayoutJson::Split { kind, sizes, children } => {
                if children.is_empty() { return; }
                let total: u32 = sizes.iter().map(|s| *s as u32).sum::<u32>().max(1);
                let is_horiz = kind == "Horizontal";
                let span = if is_horiz { area.width as u32 } else { area.height as u32 };
                let sep_count = children.len().saturating_sub(1) as u32;
                let usable = span.saturating_sub(sep_count);
                let n = children.len();
                let mut alloc: Vec<u32> = sizes.iter().take(n)
                    .map(|s| (*s as u32 * usable) / total)
                    .collect();
                while alloc.len() < n { alloc.push(0); }
                let used: u32 = alloc.iter().sum();
                let mut leftover = usable.saturating_sub(used);
                let alen = alloc.len();
                let mut idx = 0;
                while leftover > 0 && alen > 0 {
                    alloc[idx % alen] += 1;
                    idx += 1;
                    leftover -= 1;
                }
                let mut cursor: u32 = 0;
                for (i, child) in children.iter().enumerate() {
                    let size = alloc[i] as u16;
                    let sub = if is_horiz {
                        Rect { x: area.x + cursor as u16, y: area.y, width: size, height: area.height }
                    } else {
                        Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: size }
                    };
                    rec(child, sub, out);
                    cursor += size as u32;
                    if i + 1 < children.len() {
                        cursor += 1;
                    }
                }
            }
        }
    }
    rec(layout, area, &mut out);
    out
}

/// Compute separator segments for a rich `LayoutJson` tree (identical
/// algorithm to `layout_separators` but for the dump tree).
pub fn dump_separators(
    layout: &crate::layout::LayoutJson,
    area: ratatui::layout::Rect,
) -> Vec<(ratatui::layout::Rect, bool)> {
    use ratatui::layout::Rect;
    let mut out: Vec<(Rect, bool)> = Vec::new();
    fn rec(node: &crate::layout::LayoutJson, area: Rect, out: &mut Vec<(Rect, bool)>) {
        if let crate::layout::LayoutJson::Split { kind, sizes, children } = node {
            if children.is_empty() { return; }
            let total: u32 = sizes.iter().map(|s| *s as u32).sum::<u32>().max(1);
            let is_horiz = kind == "Horizontal";
            let span = if is_horiz { area.width as u32 } else { area.height as u32 };
            let sep_count = children.len().saturating_sub(1) as u32;
            let usable = span.saturating_sub(sep_count);
            let n = children.len();
            let mut alloc: Vec<u32> = sizes.iter().take(n)
                .map(|s| (*s as u32 * usable) / total)
                .collect();
            while alloc.len() < n { alloc.push(0); }
            let used: u32 = alloc.iter().sum();
            let mut leftover = usable.saturating_sub(used);
            let alen = alloc.len();
            let mut idx = 0;
            while leftover > 0 && alen > 0 {
                alloc[idx % alen] += 1;
                idx += 1;
                leftover -= 1;
            }
            let mut cursor: u32 = 0;
            for (i, child) in children.iter().enumerate() {
                let size = alloc[i] as u16;
                let sub = if is_horiz {
                    Rect { x: area.x + cursor as u16, y: area.y, width: size, height: area.height }
                } else {
                    Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: size }
                };
                rec(child, sub, out);
                cursor += size as u32;
                if i + 1 < children.len() {
                    if is_horiz {
                        out.push((Rect { x: area.x + cursor as u16, y: area.y, width: 1, height: area.height }, true));
                    } else {
                        out.push((Rect { x: area.x, y: area.y + cursor as u16, width: area.width, height: 1 }, false));
                    }
                    cursor += 1;
                }
            }
        }
    }
    rec(layout, area, &mut out);
    out
}

/// Render the target's pane contents and separators into the preview area.
/// Target window styles are not part of `window-dump`, so cross-session
/// previews use only the colours serialized in the dump.
pub fn render_dump_tree(
    f: &mut ratatui::Frame,
    layout: &crate::layout::LayoutJson,
    area: ratatui::layout::Rect,
    border_fg: Color,
    active_border_fg: Color,
    _highlight_pid: Option<usize>,
) {
    if area.width == 0 || area.height == 0 { return; }
    let active_rect = crate::client::compute_active_rect_json(layout, area);
    let total_panes = layout.count_leaves();
    crate::client::render_layout_json(
        f, layout, area,
        false,            // dim_preds: never dim predictions in preview
        border_fg, active_border_fg,
        false,            // clock_mode off in preview
        Color::Reset,     // clock_colour irrelevant
        active_rect,
        "",               // mode_style_str irrelevant (no copy mode in preview)
        false,            // zoomed: ignore zoom for preview, show real layout
        "off",            // border_status off (no per-pane title bar)
        "",               // border_format irrelevant
        total_panes,
        crate::border_lines::border_chars(crate::border_lines::DEFAULT),
        None,
        crate::client::WindowContentStyles::default(),
    );
    let border_mask = crate::client::border_mask_from_layout(layout, area, f.buffer_mut().area, false);
    crate::rendering::fix_border_intersections(f.buffer_mut(), crate::border_lines::border_chars(crate::border_lines::DEFAULT), &border_mask);
}

#[cfg(test)]
mod tests_ansi {
    use super::*;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn parse_ansi_lines_preserves_red_marker() {
        // Two-line input: a red ABC then a default def.
        let txt = "\x1b[31mABC\x1b[0m\ndef";
        let lines = parse_ansi_lines(txt, 10, 5);
        assert_eq!(lines.len(), 2, "expected 2 lines, got {}", lines.len());
        // First line should contain a span styled with red foreground.
        let first = &lines[0];
        let abc_span = first.spans.iter().find(|s| s.content == "ABC")
            .expect("no ABC span");
        assert_eq!(abc_span.style.fg, Some(Color::Red));
        // Second line def should have default fg.
        let second = &lines[1];
        let def_span = second.spans.iter().find(|s| s.content == "def")
            .expect("no def span");
        assert_ne!(def_span.style.fg, Some(Color::Red));
    }

    #[test]
    fn parse_ansi_lines_clips_to_width() {
        let txt = "ABCDEFGHIJ";
        let lines = parse_ansi_lines(txt, 4, 1);
        assert_eq!(lines.len(), 1);
        let total: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(total, "ABCD");
    }

    #[test]
    fn parse_ansi_lines_handles_bold() {
        let txt = "\x1b[1mBOLD\x1b[0m";
        let lines = parse_ansi_lines(txt, 10, 1);
        let span = lines[0].spans.iter().find(|s| s.content == "BOLD").expect("no BOLD span");
        assert!(span.style.add_modifier.contains(Modifier::BOLD));
    }
}
