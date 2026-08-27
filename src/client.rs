use std::io::{self, Write, BufRead, BufReader};
use std::time::{Duration, Instant};
use std::env;

use chrono::Local;
use crossterm::event::{Event, KeyCode, KeyModifiers, KeyEventKind};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::layout::LayoutJson;
use crate::help;
use crate::util::{WinTree, base64_encode, quote_arg};
use crate::session::read_session_key;
use crate::rendering::{dim_predictions_enabled, map_color, dim_color, centered_rect, fix_border_intersections};
use crate::style::parse_tmux_style_components;
use crate::config::{parse_key_string, normalize_key_for_binding};
use crate::clipboard::{copy_to_system_clipboard, read_from_system_clipboard};
use crate::debug_log::{client_log, client_log_enabled, input_log, input_log_enabled};
use crate::layout::RowRunsJson;
use crate::tree::split_with_gaps;

/// A floating pane (tmux new-pane) as shipped from the server: position, size,
/// border style, focus, title, and the pane's rendered rows.
#[derive(serde::Deserialize, Clone, Default)]
pub(crate) struct FloatJson {
    #[serde(default)] pub x: u16,
    #[serde(default)] pub y: u16,
    #[serde(default)] pub w: u16,
    #[serde(default)] pub h: u16,
    #[serde(default)] pub border: String,
    #[serde(default)] pub focused: bool,
    #[serde(default)] pub title: String,
    #[serde(default)] pub rows: Vec<crate::layout::RowRunsJson>,
}

/// Extract the actual command from a confirm-before argument string.
/// Handles: `confirm-before -p 'prompt text' kill-pane`
/// Returns the command to execute after confirmation (e.g. "kill-pane").
fn extract_confirm_command(args: &str) -> String {
    let parts: Vec<&str> = args.split_whitespace().collect();
    let mut i = 0;
    while i < parts.len() {
        if parts[i] == "-p" {
            i += 1; // skip flag
            // skip the prompt value (may be quoted with single quotes spanning multiple parts)
            if i < parts.len() {
                if parts[i].starts_with('\'') {
                    // scan until closing quote
                    while i < parts.len() && !parts[i].ends_with('\'') {
                        i += 1;
                    }
                } else if parts[i].starts_with('"') {
                    while i < parts.len() && !parts[i].ends_with('"') {
                        i += 1;
                    }
                }
                i += 1; // move past the prompt value
            }
        } else if parts[i].starts_with('-') {
            i += 1; // skip other flags like -b, -y
        } else {
            // Remaining parts form the command
            return parts[i..].join(" ");
        }
    }
    args.to_string()
}

/// Build a send-key name with modifier prefix (e.g. "C-Left", "S-Right", "C-S-Up").
pub(crate) fn modified_key_name(base: &str, mods: KeyModifiers) -> String {
    let mut prefix = String::new();
    if mods.contains(KeyModifiers::CONTROL) { prefix.push_str("C-"); }
    if mods.contains(KeyModifiers::ALT) { prefix.push_str("M-"); }
    if mods.contains(KeyModifiers::SHIFT) { prefix.push_str("S-"); }
    if prefix.is_empty() {
        base.to_lowercase()
    } else {
        format!("{}{}", prefix, base)
    }
}

/// Extract selected text from the layout tree given absolute terminal coordinates.
/// Computes pane areas via the same Layout splitting render_json uses, then reads
/// characters from the run-length-encoded rows_v2 data.
struct PaneLeaf<'a> {
    inner: Rect,
    rows_v2: &'a [RowRunsJson],
}

/// One OSC 8 hyperlink run to overlay after the frame is drawn (#361).
/// `x`/`y` are 0-based absolute terminal coordinates; `text` is the exact
/// glyphs ratatui already drew there; `style` is the run's style so the
/// re-emitted glyphs look identical, just wrapped in OSC 8.
#[derive(Clone)]
pub(crate) struct HyperlinkRun {
    pub x: u16,
    pub y: u16,
    pub text: String,
    pub uri: String,
    pub style: ratatui::style::Style,
}

thread_local! {
    /// Hyperlink runs collected during the current frame's render, drained and
    /// emitted after `terminal.draw()`. Thread-local because the client renders
    /// on a single thread and this avoids threading a collector through the
    /// recursive `render_layout_json`.
    static FRAME_HYPERLINKS: std::cell::RefCell<Vec<HyperlinkRun>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

pub(crate) fn frame_hyperlinks_clear() {
    FRAME_HYPERLINKS.with(|h| h.borrow_mut().clear());
}

pub(crate) fn frame_hyperlinks_push(run: HyperlinkRun) {
    FRAME_HYPERLINKS.with(|h| h.borrow_mut().push(run));
}

pub(crate) fn frame_hyperlinks_take() -> Vec<HyperlinkRun> {
    FRAME_HYPERLINKS.with(|h| std::mem::take(&mut *h.borrow_mut()))
}

/// SGR parameters for a ratatui color as a foreground (`base` 38/foreground)
/// or background (`base` 48). Returns the numeric params without the leading
/// `\x1b[` or trailing `m`.
fn color_sgr(c: ratatui::style::Color, fg: bool) -> String {
    use ratatui::style::Color::*;
    // 30-37 / 40-47 normal, 90-97 / 100-107 bright, 39/49 default.
    let (base, bright_base, def) = if fg { (30u8, 90u8, 39u8) } else { (40u8, 100u8, 49u8) };
    match c {
        Reset => def.to_string(),
        Black => (base).to_string(),
        Red => (base + 1).to_string(),
        Green => (base + 2).to_string(),
        Yellow => (base + 3).to_string(),
        Blue => (base + 4).to_string(),
        Magenta => (base + 5).to_string(),
        Cyan => (base + 6).to_string(),
        Gray => (base + 7).to_string(),
        DarkGray => (bright_base).to_string(),
        LightRed => (bright_base + 1).to_string(),
        LightGreen => (bright_base + 2).to_string(),
        LightYellow => (bright_base + 3).to_string(),
        LightBlue => (bright_base + 4).to_string(),
        LightMagenta => (bright_base + 5).to_string(),
        LightCyan => (bright_base + 6).to_string(),
        White => (bright_base + 7).to_string(),
        Indexed(n) => format!("{};5;{}", if fg { 38 } else { 48 }, n),
        Rgb(r, g, b) => format!("{};2;{};{};{}", if fg { 38 } else { 48 }, r, g, b),
    }
}

/// Build the raw escape sequence that overlays the collected hyperlink runs on
/// top of an already-drawn frame: save cursor, then for each run move to its
/// position, set its style, write the text wrapped in OSC 8, and finally
/// restore the cursor. Pure function so it can be unit-tested.
pub(crate) fn build_osc8_overlay(runs: &[HyperlinkRun]) -> String {
    use ratatui::style::Modifier;
    if runs.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("\x1b7"); // DECSC: save cursor
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        // Move to 1-based (row;col), reset, then set the run's style.
        out.push_str(&format!("\x1b[{};{}H\x1b[0m", run.y + 1, run.x + 1));
        let mut params = vec![color_sgr(run.style.fg.unwrap_or(ratatui::style::Color::Reset), true),
                              color_sgr(run.style.bg.unwrap_or(ratatui::style::Color::Reset), false)];
        let m = run.style.add_modifier;
        if m.contains(Modifier::BOLD) { params.push("1".into()); }
        if m.contains(Modifier::DIM) { params.push("2".into()); }
        if m.contains(Modifier::ITALIC) { params.push("3".into()); }
        if m.contains(Modifier::UNDERLINED) { params.push("4".into()); }
        if m.contains(Modifier::SLOW_BLINK) { params.push("5".into()); }
        if m.contains(Modifier::REVERSED) { params.push("7".into()); }
        if m.contains(Modifier::CROSSED_OUT) { params.push("9".into()); }
        out.push_str(&format!("\x1b[{}m", params.join(";")));
        // OSC 8 open ; text ; OSC 8 close.
        out.push_str(&format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", run.uri, run.text));
    }
    out.push_str("\x1b[0m"); // reset SGR
    out.push_str("\x1b8"); // DECRC: restore cursor
    out
}

/// Content area of a pane after reserving the `pane-border-status` label row.
/// Must match `render_layout_json`'s `inner`; the caret and every screen→cell
/// mouse mapping route through this so they stay aligned with the content (#288).
pub(crate) fn pane_content_inner(area: Rect, border_status: &str, border_format: &str) -> Rect {
    let has_border_label = border_status != "off" && !border_format.is_empty() && area.height > 1;
    if !has_border_label {
        return area;
    }
    if border_status == "top" {
        Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(1))
    } else {
        // "bottom": content keeps its top origin, only the last row is reserved.
        Rect::new(area.x, area.y, area.width, area.height.saturating_sub(1))
    }
}

/// Screen rect of the centred display-popup overlay.
///
/// The draw pass and the post-draw cursor write both need this rect and must
/// agree on it to the cell, or the cursor lands off the popup it belongs to
/// (#507), so both call this instead of recomputing the geometry.
pub(crate) fn popup_overlay_rect(content_chunk: Rect, want_w: u16, want_h: u16) -> Rect {
    let w = want_w.min(content_chunk.width.saturating_sub(2));
    let h = want_h.min(content_chunk.height.saturating_sub(2));
    Rect {
        x: content_chunk.x + (content_chunk.width.saturating_sub(w)) / 2,
        y: content_chunk.y + (content_chunk.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Screen position of a PTY popup's cursor, given the popup-inner cursor cell
/// reported by the server.
///
/// Returns `None` when the cell is not currently on screen (scrolled away, or
/// outside a popup too small to have an interior), in which case the caller
/// leaves the cursor hidden rather than parking it somewhere arbitrary.
pub(crate) fn popup_cursor_screen_pos(
    content_chunk: Rect,
    want_w: u16,
    want_h: u16,
    scroll: u16,
    cursor: (u16, u16),
) -> Option<(u16, u16)> {
    let (cc, cr) = cursor;
    let popup_area = popup_overlay_rect(content_chunk, want_w, want_h);
    let inner_w = popup_area.width.saturating_sub(2);
    let inner_h = popup_area.height.saturating_sub(2);
    if inner_w == 0 || inner_h == 0 {
        return None;
    }
    // Same scroll offset the popup paragraph is drawn with.
    let row = cr.checked_sub(scroll)?;
    if row >= inner_h || cc >= inner_w {
        return None;
    }
    Some((popup_area.x + 1 + cc, popup_area.y + 1 + row))
}

fn collect_leaves<'a>(node: &'a LayoutJson, area: Rect, out: &mut Vec<PaneLeaf<'a>>) {
    match node {
        LayoutJson::Leaf { rows_v2, .. } => {
            out.push(PaneLeaf { inner: area, rows_v2 });
        }
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = kind == "Horizontal";
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter().enumerate() {
                if i < rects.len() {
                    collect_leaves(child, rects[i], out);
                }
            }
        }
    }
}

/// Get the character at a column position within a row's runs.
///
/// `run.text` may be shorter than `run.width` (single repeated char) or
/// multi-char (wide chars); pick the nth char if present.
fn char_at_col(runs: &[crate::layout::CellRunJson], local_col: usize) -> char {
    let mut cursor = 0usize;
    for run in runs {
        let run_width = run.width.max(1) as usize;
        if local_col >= cursor && local_col < cursor + run_width {
            let offset = local_col - cursor;
            return run.text.chars().nth(offset).unwrap_or(' ');
        }
        cursor += run_width;
    }
    ' '
}

/// Expand a row's runs into a dense `Vec<char>` indexed by local column.
/// Used by hot paths (word-boundary scan) that would otherwise call
/// `char_at_col` O(width) times and pay O(width²) total.
fn row_chars(runs: &[crate::layout::CellRunJson], width: usize) -> Vec<char> {
    let mut out = vec![' '; width];
    let mut cursor = 0usize;
    for run in runs {
        let run_width = run.width.max(1) as usize;
        let chars: Vec<char> = run.text.chars().collect();
        for i in 0..run_width {
            let col = cursor + i;
            if col >= width { break; }
            out[col] = chars.get(i).copied().unwrap_or(' ');
        }
        cursor += run_width;
        if cursor >= width { break; }
    }
    out
}

/// Clip a `rows_v2` buffer to fit a smaller preview area without
/// rescaling. Scaling cell-grid content (terminal output) is fundamentally
/// lossy: nearest-neighbour sampling drops characters and reflow word-wrap
/// destroys 2D TUI grids (htop, vim, pstop) by shifting subsequent rows.
/// The honest behaviour, matching tmux's own `choose-tree` preview, is to
/// show the buffer at 1:1 and clip what does not fit.
///
/// Strategy:
///   * Trailing fully-blank rows are dropped so the prompt / cursor of a
///     shell sits at the bottom edge of the preview instead of being
///     scrolled off by empty space.
///   * The bottom `dst_h` remaining rows are returned. For a shell this
///     is the most recent output. For a full-screen TUI (no blank rows)
///     this is the bottom edge of the TUI (status / F-key bar) which
///     preserves the grid intact.
///   * Columns are NOT modified here. The caller's render loop already
///     clips runs that exceed `inner.width`, so column geometry stays
///     pixel-accurate.
pub(crate) fn downscale_rows_v2(
    src: &[crate::layout::RowRunsJson],
    _src_h: u16,
    _src_w: u16,
    dst_h: u16,
    _dst_w: u16,
) -> Vec<crate::layout::RowRunsJson> {
    use crate::layout::RowRunsJson;
    if dst_h == 0 || src.is_empty() {
        return Vec::new();
    }
    // Find the last row that has any non-blank cell (with bg colour or
    // non-space text). Everything after that is empty filler from the
    // viewport.
    let is_blank = |row: &RowRunsJson| -> bool {
        row.runs.iter().all(|run| {
            let blank_text = run.text.is_empty() || run.text.chars().all(|c| c == ' ');
            let no_bg = run.bg.is_empty() || run.bg == "default";
            blank_text && no_bg
        })
    };
    let mut last_used = src.len();
    while last_used > 0 && is_blank(&src[last_used - 1]) {
        last_used -= 1;
    }
    // Keep at least one blank row so the cursor on a fresh prompt line is
    // visible (otherwise we would trim away the line the cursor is on).
    if last_used < src.len() {
        last_used += 1;
    }
    let trimmed = &src[..last_used];
    let start = trimmed.len().saturating_sub(dst_h as usize);
    trimmed[start..].to_vec()
}

/// Normalise a selection (start, end) into reading-order or block-mode bounds.
fn normalize_selection(start: (u16, u16), end: (u16, u16), block: bool) -> (u16, u16, u16, u16) {
    if block {
        (start.1.min(end.1), start.0.min(end.0), start.1.max(end.1), start.0.max(end.0))
    } else if (start.1, start.0) <= (end.1, end.0) {
        (start.1, start.0, end.1, end.0)
    } else {
        (end.1, end.0, start.1, start.0)
    }
}

fn selection_row_cols(
    row: u16,
    r0: u16,
    c0: u16,
    r1: u16,
    c1: u16,
    block: bool,
    term_width: u16,
    pane_clip: Option<Rect>,
) -> Option<(u16, u16)> {
    if term_width == 0 {
        return None;
    }

    let mut col_start = if block || row == r0 { c0 } else { 0 };
    let mut col_end = if block || row == r1 {
        c1
    } else {
        term_width.saturating_sub(1)
    };

    if let Some(clip) = pane_clip {
        if clip.width == 0 || clip.height == 0 {
            return None;
        }

        let clip_bottom = clip.y.saturating_add(clip.height).saturating_sub(1);
        if row < clip.y || row > clip_bottom {
            return None;
        }

        let clip_right = clip.x.saturating_add(clip.width).saturating_sub(1);
        col_start = col_start.max(clip.x);
        col_end = col_end.min(clip_right);
    }

    (col_start <= col_end).then_some((col_start, col_end))
}

fn extract_selection_text(
    layout: &LayoutJson,
    term_width: u16,
    content_height: u16,
    start: (u16, u16),
    end: (u16, u16),
    block: bool,
    pane_clip: Option<Rect>,
    border_status: &str,
    border_format: &str,
) -> String {
    let (r0, c0, r1, c1) = normalize_selection(start, end, block);

    let content_area = Rect {
        x: 0,
        y: 0,
        width: term_width,
        height: content_height,
    };
    let mut leaves: Vec<PaneLeaf> = Vec::new();
    collect_leaves(layout, content_area, &mut leaves);

    let mut result = String::new();
    let mut wrote_line = false;
    for row in r0..=r1 {
        let Some((col_start, col_end)) =
            selection_row_cols(row, r0, c0, r1, c1, block, term_width, pane_clip)
        else {
            continue;
        };

        let mut line = String::new();
        for col in col_start..=col_end {
            let mut ch = ' ';
            for leaf in &leaves {
                let inner = pane_content_inner(leaf.inner, border_status, border_format);
                if col >= inner.x
                    && col < inner.x + inner.width
                    && row >= inner.y
                    && row < inner.y + inner.height
                {
                    let local_row = (row - inner.y) as usize;
                    let local_col = (col - inner.x) as usize;
                    if local_row < leaf.rows_v2.len() {
                        ch = char_at_col(&leaf.rows_v2[local_row].runs, local_col);
                    }
                    break;
                }
            }
            line.push(ch);
        }
        if wrote_line {
            result.push('\n');
        }
        result.push_str(line.trim_end());
        wrote_line = true;
    }

    result
}

/// Check if the active pane is running a fullscreen TUI app (alternate screen).
/// Used to decide whether right-click should paste (shell prompt) or forward
/// as a mouse event to the child (TUI app like htop, Claude Code, etc.).
fn active_pane_in_alt_screen(layout: &LayoutJson) -> bool {
    match layout {
        LayoutJson::Leaf { active, alternate_screen, .. } => *active && *alternate_screen,
        LayoutJson::Split { children, .. } => children.iter().any(|c| active_pane_in_alt_screen(c)),
    }
}

/// Check if the pane with `pane_id` has explicitly enabled a mouse protocol
/// (DECSET 1000/1002/1003 — the server ships this per leaf as `wants_mouse`).
/// Used on left-down to decide whether psmux may start its client-side drag
/// selection: an app that asked for mouse tracking (gaviero, opencode, nvim
/// with `set mouse=a`, …) implements its own selection and must receive the
/// drags, not have psmux swallow them and paint an overlay on top.  This is
/// the per-pane automatic version of the global `mouse-selection off` escape
/// hatch (issue #245).  Alt-screen alone does NOT qualify, so alt-screen
/// apps without mouse support (`less`, …) keep psmux selection.
fn pane_wants_mouse_json(layout: &LayoutJson, pane_id: usize) -> bool {
    match layout {
        LayoutJson::Leaf { id, wants_mouse, .. } => *id == pane_id && *wants_mouse,
        LayoutJson::Split { children, .. } => {
            children.iter().any(|c| pane_wants_mouse_json(c, pane_id))
        }
    }
}

/// Is this batched client command nothing but a bare pointer move?
///
/// The client emits `pane-mouse <id> 35 <col> <row> M` when the pointer moves
/// inside a pane and `mouse-move <col> <row>` when it moves anywhere else.
/// SGR button 35 is the motion bit (32) plus the "no button held" low bits (3),
/// so it is the one mouse verb that carries no user intent: it must not be
/// mistaken for a keystroke and drive the typing fast-poll and an immediate
/// state round trip (#604).
pub(crate) fn is_bare_motion_cmd(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.starts_with("mouse-move ") {
        return true;
    }
    let Some(rest) = cmd.strip_prefix("pane-mouse ") else { return false };
    let mut fields = rest.split_whitespace();
    let _pane_id = fields.next();
    fields.next() == Some("35")
}

fn client_selection_owns_drag(
    mouse_selection: bool,
    mouse_selection_force: bool,
    pane_handles_mouse: bool,
) -> bool {
    mouse_selection && (mouse_selection_force || !pane_handles_mouse)
}

/// Check if the active pane is in server-side copy mode.
/// When true, the client should NOT start its own text selection —
/// the server handles cursor positioning and selection in copy mode.
fn active_pane_in_copy_mode(layout: &LayoutJson) -> bool {
    match layout {
        LayoutJson::Leaf { active, copy_mode, .. } => *active && *copy_mode,
        LayoutJson::Split { children, .. } => children.iter().any(|c| active_pane_in_copy_mode(c)),
    }
}

/// Collect each leaf's live scrollback offset (`view_offset`) by pane id.
/// Nonzero outside copy mode means the pane is direct-scrolled
/// (scroll-enter-copy-mode off, #193) — i.e. there is content below the view.
fn collect_view_offsets(layout: &LayoutJson, out: &mut Vec<(usize, usize)>) {
    match layout {
        LayoutJson::Leaf { id, view_offset, .. } => out.push((*id, *view_offset)),
        LayoutJson::Split { children, .. } => {
            for c in children {
                collect_view_offsets(c, out);
            }
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Find the (start_col, end_col) of the word at `(col, row)` inside the
/// given pane. Returns None when the cell is not a word character.
///
/// `layout` is walked to resolve the clicked leaf's `rows_v2` — the caller
/// already knows `pane_rect`, but it does not have a handle to the raw
/// content, so we do a single targeted descent.
fn word_bounds_at(
    layout: &LayoutJson,
    term_width: u16,
    content_height: u16,
    pane_rect: Rect,
    col: u16,
    row: u16,
    border_status: &str,
    border_format: &str,
) -> Option<(u16, u16)> {
    let content_area = Rect { x: 0, y: 0, width: term_width, height: content_height };
    let mut leaves: Vec<PaneLeaf> = Vec::new();
    collect_leaves(layout, content_area, &mut leaves);

    // `pane_rect` is the outer rect; match on it, then map into the content inner.
    let leaf = leaves.iter().find(|l| l.inner == pane_rect)?;
    let content = pane_content_inner(leaf.inner, border_status, border_format);

    let local_row = row.checked_sub(content.y)? as usize;
    if local_row >= leaf.rows_v2.len() { return None; }
    let width = content.width as usize;
    let chars = row_chars(&leaf.rows_v2[local_row].runs, width);

    let local_col = col.checked_sub(content.x)? as usize;
    if local_col >= width { return None; }
    if !is_word_char(chars[local_col]) { return None; }

    let mut left = local_col;
    while left > 0 && is_word_char(chars[left - 1]) {
        left -= 1;
    }
    let mut right = local_col;
    while right + 1 < width && is_word_char(chars[right + 1]) {
        right += 1;
    }

    Some((content.x + left as u16, content.x + right as u16))
}

/// Check if screen coordinates (x, y) fall on a separator line in the layout.
/// Used to distinguish border-drag (resize) from text selection on left-click.
fn is_on_separator(layout: &LayoutJson, area: Rect, x: u16, y: u16) -> bool {
    match layout {
        LayoutJson::Leaf { .. } => false,
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = kind == "Horizontal";
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);

            // Check if (x, y) is on any separator between children
            for i in 0..children.len().saturating_sub(1) {
                if i >= rects.len() { break; }
                if is_horizontal {
                    let sep_x = rects[i].x + rects[i].width;
                    if x == sep_x && y >= area.y && y < area.y + area.height {
                        return true;
                    }
                } else {
                    let sep_y = rects[i].y + rects[i].height;
                    if y == sep_y && x >= area.x && x < area.x + area.width {
                        return true;
                    }
                }
            }

            // Recurse into children
            for (i, child) in children.iter().enumerate() {
                if i < rects.len() && is_on_separator(child, rects[i], x, y) {
                    return true;
                }
            }

            false
        }
    }
}

/// Collect all leaf pane IDs and their absolute rects from a LayoutJson tree.
fn collect_pane_rects(node: &LayoutJson, area: Rect, out: &mut Vec<(usize, Rect)>) {
    match node {
        LayoutJson::Leaf { id, .. } => {
            out.push((*id, area));
        }
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = kind == "Horizontal";
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter().enumerate() {
                if i < rects.len() {
                    collect_pane_rects(child, rects[i], out);
                }
            }
        }
    }
}

/// Collect all split border positions from a LayoutJson tree.
/// Returns: (tree_path_to_parent, kind, child_index, border_pixel_pos, total_pixels, sizes_snapshot)
fn collect_layout_borders(
    node: &LayoutJson,
    area: Rect,
    path: &mut Vec<usize>,
    out: &mut Vec<(Vec<usize>, String, usize, u16, u16, Vec<u16>, Rect)>,
) {
    if let LayoutJson::Split { kind, sizes, children } = node {
        let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
            sizes.clone()
        } else {
            vec![(100 / children.len().max(1)) as u16; children.len()]
        };
        let is_horizontal = kind == "Horizontal";
        let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
        let total_px = if is_horizontal { area.width } else { area.height };
        for i in 0..children.len().saturating_sub(1) {
            if i < rects.len() {
                let pos = if is_horizontal {
                    rects[i].x + rects[i].width
                } else {
                    rects[i].y + rects[i].height
                };
                out.push((path.clone(), kind.clone(), i, pos, total_px, effective_sizes.clone(), area));
            }
        }
        for (i, child) in children.iter().enumerate() {
            if i < rects.len() {
                path.push(i);
                collect_layout_borders(child, rects[i], path, out);
                path.pop();
            }
        }
    }
}

pub fn border_mask_from_layout(
    node: &LayoutJson,
    area: Rect,
    buf_area: Rect,
    zoomed: bool,
) -> Vec<usize> {
    let mut cells = Vec::new();
    mark_layout_borders(node, area, buf_area, zoomed, &mut cells);
    cells.sort_unstable();
    cells.dedup();
    cells
}

fn mark_layout_borders(
    node: &LayoutJson,
    area: Rect,
    buf_area: Rect,
    zoomed: bool,
    cells: &mut Vec<usize>,
) {
    let LayoutJson::Split {
        kind,
        sizes,
        children,
    } = node
    else {
        return;
    };
    let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
        sizes.clone()
    } else {
        vec![(100 / children.len().max(1)) as u16; children.len()]
    };
    let is_horizontal = kind == "Horizontal";

    if zoomed {
        // Match render_layout_json's zoom early-return exactly: no separator
        // drawn for this split, recurse into the chosen child at the SAME area.
        if let Some(i) = effective_sizes.iter().position(|&s| s != 0) {
            if let Some(child) = children.get(i) {
                mark_layout_borders(child, area, buf_area, zoomed, cells);
            }
        }
        return;
    }

    let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
    let w = buf_area.width as usize;
    for i in 0..children.len().saturating_sub(1) {
        if i >= rects.len() {
            break;
        }
        if is_horizontal {
            // Vertical separator line at column `pos`, spanning this split's own area height.
            let pos = rects[i].x + rects[i].width;
            if pos >= buf_area.x && pos < buf_area.x + buf_area.width {
                for y in area.y..area.y + area.height {
                    if y >= buf_area.y && y < buf_area.y + buf_area.height {
                        cells.push((y - buf_area.y) as usize * w + (pos - buf_area.x) as usize);
                    }
                }
            }
        } else {
            // Horizontal separator line at row `pos`, spanning this split's own area width.
            let pos = rects[i].y + rects[i].height;
            if pos >= buf_area.y && pos < buf_area.y + buf_area.height {
                for x in area.x..area.x + area.width {
                    if x >= buf_area.x && x < buf_area.x + buf_area.width {
                        cells.push((pos - buf_area.y) as usize * w + (x - buf_area.x) as usize);
                    }
                }
            }
        }
    }
    for (i, child) in children.iter().enumerate() {
        if i < rects.len() {
            mark_layout_borders(child, rects[i], buf_area, zoomed, cells);
        }
    }
}

/// Check if any leaf in a LayoutJson subtree is the active pane.
/// Compute the rectangle of the active pane by searching the LayoutJson tree.
pub fn compute_active_rect_json(node: &LayoutJson, area: Rect) -> Option<Rect> {
    match node {
        LayoutJson::Leaf { active, .. } => {
            if *active { Some(area) } else { None }
        }
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = kind == "Horizontal";
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter().enumerate() {
                if i < rects.len() {
                    if let Some(r) = compute_active_rect_json(child, rects[i]) {
                        return Some(r);
                    }
                }
            }
            None
        }
    }
}

/// Compute the active pane rectangle, optionally treating the layout as zoomed.
///
/// When `zoomed` is true, this mirrors `render_layout_json` behavior by walking
/// only the visible (non-zero) split child and passing down the full parent area.
pub(crate) fn compute_active_rect_json_zoom_aware(
    node: &LayoutJson,
    area: Rect,
    zoomed: bool,
) -> Option<Rect> {
    match node {
        LayoutJson::Leaf { active, .. } => {
            if *active { Some(area) } else { None }
        }
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            if zoomed {
                if let Some(i) = effective_sizes.iter().position(|&s| s != 0) {
                    if let Some(child) = children.get(i) {
                        return compute_active_rect_json_zoom_aware(child, area, zoomed);
                    }
                }
                return None;
            }
            let is_horizontal = kind == "Horizontal";
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            for (i, child) in children.iter().enumerate() {
                if i < rects.len() {
                    if let Some(r) = compute_active_rect_json_zoom_aware(child, rects[i], zoomed) {
                        return Some(r);
                    }
                }
            }
            None
        }
    }
}

/// Render a large ASCII clock overlay (tmux clock-mode).
/// Top-level so both the main viewport and the choose-tree/choose-session
/// preview can share one implementation.
pub fn render_clock_overlay(f: &mut Frame, area: Rect, colour: Color) {
    const DIGITS: [&[&str; 5]; 10] = [
        &["###", "# #", "# #", "# #", "###"],
        &["  #", "  #", "  #", "  #", "  #"],
        &["###", "  #", "###", "#  ", "###"],
        &["###", "  #", "###", "  #", "###"],
        &["# #", "# #", "###", "  #", "  #"],
        &["###", "#  ", "###", "  #", "###"],
        &["###", "#  ", "###", "# #", "###"],
        &["###", "  #", "  #", "  #", "  #"],
        &["###", "# #", "###", "# #", "###"],
        &["###", "# #", "###", "  #", "###"],
    ];
    const COLON: [&str; 5] = [" ", "#", " ", "#", " "];
    let now = Local::now();
    let time_str = now.format("%H:%M:%S").to_string();
    let total_w: u16 = time_str.chars().map(|c| if c == ':' { 2 } else { 4 }).sum::<u16>() - 1;
    let total_h: u16 = 5;
    if area.width < total_w || area.height < total_h { return; }
    let start_x = area.x + (area.width.saturating_sub(total_w)) / 2;
    let start_y = area.y + (area.height.saturating_sub(total_h)) / 2;
    let clock_area = Rect::new(start_x.saturating_sub(1), start_y, total_w + 2, total_h);
    f.render_widget(Clear, clock_area);
    for row in 0..5u16 {
        let mut x = start_x;
        for ch in time_str.chars() {
            if ch == ':' {
                let cell_area = Rect::new(x, start_y + row, 1, 1);
                let s = Span::styled(COLON[row as usize], Style::default().fg(colour));
                f.render_widget(Paragraph::new(Line::from(s)), cell_area);
                x += 2;
            } else if let Some(d) = ch.to_digit(10) {
                let pattern = DIGITS[d as usize][row as usize];
                let cell_area = Rect::new(x, start_y + row, 3, 1);
                let s = Span::styled(pattern, Style::default().fg(colour));
                f.render_widget(Paragraph::new(Line::from(s)), cell_area);
                x += 4;
            }
        }
    }
}

/// Render a LayoutJson tree into the given area.  This is the canonical
/// pane renderer used by both the main viewport and the choose-tree/
/// choose-session preview, so a preview is a true miniature of the real
/// window (same separators, same colors, same content rendering).
/// Draw the active window's floating panes (tmux new-pane) as positioned
/// overlays over the tiled layout. Called after `render_layout_json` and before
/// the modal popup overlay, so popups still stack on top. Reuses the popup
/// run-rendering; borders come from `-B` (mapped to ratatui border types),
/// `none` draws no border. The focused float gets a highlighted border.
pub(crate) fn render_float_overlays(f: &mut Frame, content_chunk: Rect, floats: &[FloatJson]) {
    for fl in floats {
        if fl.w < 2 || fl.h < 2 { continue; }
        let w = fl.w.min(content_chunk.width);
        let h = fl.h.min(content_chunk.height);
        let x = content_chunk.x + fl.x.min(content_chunk.width.saturating_sub(w));
        let y = content_chunk.y + fl.y.min(content_chunk.height.saturating_sub(h));
        let area = Rect { x, y, width: w, height: h };
        let border_type = match fl.border.as_str() {
            "double" => Some(BorderType::Double),
            "heavy" => Some(BorderType::Thick),
            "none" => None,
            // single/simple/number/spaces/unknown -> plain line box
            _ => Some(BorderType::Plain),
        };
        let bcol = if fl.focused { Color::Green } else { Color::DarkGray };
        let inner_w = if border_type.is_some() { w.saturating_sub(2) } else { w };
        let mut lines: Vec<Line<'static>> = Vec::new();
        for row_data in &fl.rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut col: u16 = 0;
            for run in &row_data.runs {
                if col >= inner_w { break; }
                let fg = crate::style::map_color(&run.fg);
                let bg = crate::style::map_color(&run.bg);
                let mut style = Style::default().fg(fg).bg(bg);
                if run.flags & 1  != 0 { style = style.add_modifier(Modifier::DIM); }
                if run.flags & 2  != 0 { style = style.add_modifier(Modifier::BOLD); }
                if run.flags & 4  != 0 { style = style.add_modifier(Modifier::ITALIC); }
                if run.flags & 8  != 0 {
                    style = crate::rendering::with_underline(
                        style,
                        if run.ul == 0 { 1 } else { run.ul },
                        run.ulc.as_deref().map(map_color),
                    );
                }
                if run.flags & 16 != 0 { style = style.add_modifier(Modifier::REVERSED); }
                if run.flags & 32 != 0 { style = style.add_modifier(Modifier::SLOW_BLINK); }
                if run.flags & 128 != 0 { style = style.add_modifier(Modifier::CROSSED_OUT); }
                let text: &str = if run.flags & 64 != 0 { " " } else if run.text.is_empty() { " " } else { &run.text };
                let run_w = run.width.max(1);
                if col + run_w > inner_w {
                    let avail = (inner_w - col) as usize;
                    let truncated: String = text.chars().take(avail).collect();
                    if !truncated.is_empty() { spans.push(Span::styled(truncated, style)); }
                    col = inner_w;
                } else {
                    spans.push(Span::styled(text.to_string(), style));
                    col += run_w;
                }
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Clear, area);
        match border_type {
            Some(bt) => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(bt)
                    .border_style(Style::default().fg(bcol))
                    .title(fl.title.clone());
                let inner = block.inner(area);
                f.render_widget(block, area);
                f.render_widget(Paragraph::new(Text::from(lines)), inner);
            }
            None => {
                f.render_widget(Paragraph::new(Text::from(lines)), area);
            }
        }
    }
}

/// Copy-mode line-number rendering config, resolved once per frame from the
/// `copy-mode-line-numbers` option and the active pane's scrollback size.
#[derive(Clone, Copy)]
pub struct CopyLnRender {
    pub mode: crate::copy_line_numbers::CopyLnMode,
    pub hsize: usize,
    pub num_style: Style,
    pub cur_style: Style,
}

pub fn render_layout_json(
    f: &mut Frame,
    node: &LayoutJson,
    area: Rect,
    dim_preds: bool,
    border_fg: Color,
    active_border_fg: Color,
    clock_mode: bool,
    clock_colour: Color,
    active_rect: Option<Rect>,
    mode_style_str: &str,
    zoomed: bool,
    border_status: &str,
    border_format: &str,
    total_panes: usize,
    bchars: Option<crate::border_lines::BorderChars>,
    copy_ln: Option<CopyLnRender>,
) {
    match node {
        LayoutJson::Leaf {
            id,
            rows: src_rows,
            cols: src_cols,
            cursor_row,
            cursor_col,
            alternate_screen,
            wants_mouse: _,
            hide_cursor: _,
            cursor_shape: _,
            active,
            copy_mode,
            scroll_offset,
            view_offset: _,
            sel_start_row,
            sel_start_col,
            sel_end_row,
            sel_end_col,
            sel_mode,
            copy_cursor_row,
            copy_cursor_col,
            content,
            rows_v2,
            title,
        } => {
            // Reserve 1 row for the border label so it doesn't overlap content (#288).
            let has_border_label = border_status != "off" && !border_format.is_empty() && area.height > 1;
            let inner = pane_content_inner(area, border_status, border_format);
            let mut lines: Vec<Line> = Vec::new();
            let use_full_cells = *copy_mode && *active && !content.is_empty();
            // If the source pane is larger than the preview area, reflow
            // (word-wrap) the rows onto preview-width lines instead of
            // dropping characters via nearest-neighbour sampling. The bottom
            // `inner.height` wrapped rows are shown so the cursor stays in
            // view, matching how a terminal scrolls.
            let needs_scale = !use_full_cells
                && !rows_v2.is_empty()
                && *src_rows > 0 && *src_cols > 0
                && inner.height > 0 && inner.width > 0
                && (*src_rows > inner.height || *src_cols > inner.width);
            let scaled_holder: Vec<RowRunsJson>;
            let rows_v2_eff: &[RowRunsJson] = if needs_scale {
                scaled_holder = downscale_rows_v2(rows_v2, *src_rows, *src_cols, inner.height, inner.width);
                &scaled_holder
            } else {
                rows_v2.as_slice()
            };
            if use_full_cells || rows_v2_eff.is_empty() {
                for r in 0..inner.height.min(content.len() as u16) {
                    let mut spans: Vec<Span> = Vec::new();
                    let row = &content[r as usize];
                    let max_c = inner.width.min(row.len() as u16);
                    let mut c: u16 = 0;
                    while c < max_c {
                        let cell = &row[c as usize];
                        let mut fg = map_color(&cell.fg);
                        let bg = map_color(&cell.bg);
                        let in_selection = if *copy_mode && *active {
                            if let (Some(sr), Some(sc), Some(er), Some(ec)) = (sel_start_row, sel_start_col, sel_end_row, sel_end_col) {
                                let mode = sel_mode.as_deref().unwrap_or("char");
                                match mode {
                                    "rect" => r >= *sr && r <= *er && c >= (*sc).min(*ec) && c <= (*sc).max(*ec),
                                    "line" => r >= *sr && r <= *er,
                                    _ => {
                                        if *sr == *er {
                                            r == *sr && c >= (*sc).min(*ec) && c <= (*sc).max(*ec)
                                        } else if r == *sr {
                                            c >= *sc
                                        } else if r == *er {
                                            c <= *ec
                                        } else {
                                            r > *sr && r < *er
                                        }
                                    }
                                }
                            } else { false }
                        } else { false };
                        if *active && dim_preds && !*alternate_screen
                            && (r > *cursor_row || (r == *cursor_row && c >= *cursor_col))
                        {
                            fg = dim_color(fg);
                        }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if in_selection {
                            let ms = crate::rendering::parse_tmux_style(mode_style_str);
                            style = ms;
                        }
                        if cell.inverse { style = style.add_modifier(Modifier::REVERSED); }
                        if cell.dim { style = style.add_modifier(Modifier::DIM); }
                        if cell.bold { style = style.add_modifier(Modifier::BOLD); }
                        if cell.italic { style = style.add_modifier(Modifier::ITALIC); }
                        if cell.underline { style = style.add_modifier(Modifier::UNDERLINED); }
                        if cell.blink { style = style.add_modifier(Modifier::SLOW_BLINK); }
                        if cell.strikethrough { style = style.add_modifier(Modifier::CROSSED_OUT); }
                        let text: &str = if cell.hidden {
                            " "
                        } else if cell.text.is_empty() {
                            " "
                        } else {
                            &cell.text
                        };
                        let char_width = unicode_width::UnicodeWidthStr::width(text) as u16;
                        if char_width >= 2 && c + char_width > max_c {
                            spans.push(Span::styled(" ", style));
                            c += 1;
                        } else {
                            spans.push(Span::styled(text, style));
                            if char_width >= 2 {
                                c += 2;
                            } else {
                                c += 1;
                            }
                        }
                    }
                    if c < inner.width {
                        let last_bg = if !spans.is_empty() {
                            spans.last().unwrap().style.bg.unwrap_or(Color::Reset)
                        } else { Color::Reset };
                        let pad = " ".repeat((inner.width - c) as usize);
                        spans.push(Span::styled(pad, Style::default().bg(last_bg)));
                    }
                    lines.push(Line::from(spans));
                }
            } else {
                for r in 0..inner.height.min(rows_v2_eff.len() as u16) {
                    let mut spans: Vec<Span> = Vec::new();
                    let mut c: u16 = 0;
                    let mut last_bg = Color::Reset;
                    for run in &rows_v2_eff[r as usize].runs {
                        if c >= inner.width { break; }
                        let mut fg = map_color(&run.fg);
                        let bg = map_color(&run.bg);
                        last_bg = bg;
                        if *active && dim_preds && !*alternate_screen
                            && (r > *cursor_row || (r == *cursor_row && c >= *cursor_col))
                        {
                            fg = dim_color(fg);
                        }
                        let mut style = Style::default().fg(fg).bg(bg);
                        if run.flags & 16 != 0 { style = style.add_modifier(Modifier::REVERSED); }
                        if run.flags & 1 != 0 { style = style.add_modifier(Modifier::DIM); }
                        if run.flags & 2 != 0 { style = style.add_modifier(Modifier::BOLD); }
                        if run.flags & 4 != 0 { style = style.add_modifier(Modifier::ITALIC); }
                        if run.flags & 8 != 0 {
                            style = crate::rendering::with_underline(
                                style,
                                if run.ul == 0 { 1 } else { run.ul },
                                run.ulc.as_deref().map(map_color),
                            );
                        }
                        if run.flags & 32 != 0 { style = style.add_modifier(Modifier::SLOW_BLINK); }
                        if run.flags & 128 != 0 { style = style.add_modifier(Modifier::CROSSED_OUT); }
                        let text: &str = if run.flags & 64 != 0 {
                            " "
                        } else if run.text.is_empty() {
                            " "
                        } else {
                            &run.text
                        };
                        let run_w = run.width.max(1);
                        if c + run_w > inner.width {
                            let avail = (inner.width - c) as usize;
                            let mut truncated = String::new();
                            let mut used = 0usize;
                            for ch in text.chars() {
                                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                                if used + cw > avail { break; }
                                used += cw;
                                truncated.push(ch);
                            }
                            if !truncated.is_empty() {
                                if let Some(uri) = &run.link {
                                    frame_hyperlinks_push(HyperlinkRun {
                                        x: inner.x + c, y: inner.y + r,
                                        text: truncated.clone(), uri: uri.clone(), style,
                                    });
                                }
                                spans.push(Span::styled(truncated, style));
                            }
                            c = inner.width;
                        } else {
                            if let Some(uri) = &run.link {
                                frame_hyperlinks_push(HyperlinkRun {
                                    x: inner.x + c, y: inner.y + r,
                                    text: text.to_string(), uri: uri.clone(), style,
                                });
                            }
                            spans.push(Span::styled(text, style));
                            c = c.saturating_add(run_w);
                        }
                    }
                    if c < inner.width {
                        let pad = " ".repeat((inner.width - c) as usize);
                        spans.push(Span::styled(pad, Style::default().bg(last_bg)));
                    }
                    lines.push(Line::from(spans));
                }
            }
            // Copy-mode line-number gutter (#copy-mode-line-numbers). Prepend a
            // right-aligned number column to each visible row; content shifts
            // right and the rightmost columns clip, matching tmux's reduced
            // content width. Only the active copy-mode pane shows the gutter.
            let gutter_w: u16 = if *copy_mode && *active {
                copy_ln.map(|cfg| crate::copy_line_numbers::gutter_width(cfg.mode, cfg.hsize, inner.height as usize) as u16).unwrap_or(0)
            } else { 0 };
            if gutter_w > 0 {
                if let Some(cfg) = copy_ln {
                    let oy = *scroll_offset;
                    let cy = copy_cursor_row.map(|r| r as usize).unwrap_or(usize::MAX);
                    for (r, line) in lines.iter_mut().enumerate() {
                        let txt = crate::copy_line_numbers::gutter_text(cfg.mode, gutter_w as usize, r, oy, cy, cfg.hsize);
                        let sty = if crate::copy_line_numbers::is_current_row(r, cy) { cfg.cur_style } else { cfg.num_style };
                        line.spans.insert(0, Span::styled(txt, sty));
                    }
                }
            }

            f.render_widget(Clear, inner);
            let para = Paragraph::new(Text::from(lines));
            f.render_widget(para, inner);

            if *copy_mode && *active {
                let label = "[copy mode]";
                let lw = label.len() as u16;
                if area.width >= lw {
                    let lx = area.x + area.width.saturating_sub(lw);
                    let la = Rect::new(lx, area.y, lw, 1);
                    let ls = Span::styled(label, Style::default().fg(Color::Black).bg(Color::Yellow));
                    f.render_widget(Paragraph::new(Line::from(ls)), la);
                }
            }

            if *copy_mode && *active && *scroll_offset > 0 {
                let indicator = format!("[{}/{}]", scroll_offset, scroll_offset);
                let indicator_width = indicator.len() as u16;
                if area.width > indicator_width + 2 {
                    let indicator_x = area.x + area.width - indicator_width - 1;
                    let indicator_y = if *copy_mode { area.y + 1 } else { area.y };
                    let indicator_area = Rect::new(indicator_x, indicator_y, indicator_width, 1);
                    let indicator_span = Span::styled(indicator, Style::default().fg(Color::Black).bg(Color::Yellow));
                    f.render_widget(Paragraph::new(Line::from(indicator_span)), indicator_area);
                }
            }

            if *active && !*copy_mode {
                if clock_mode {
                    render_clock_overlay(f, inner, clock_colour);
                }
            }

            if *copy_mode && *active {
                if let (Some(cr), Some(cc)) = (copy_cursor_row, copy_cursor_col) {
                    let cr = (*cr).min(inner.height.saturating_sub(1));
                    // The gutter shifts content right, so the cursor shifts too.
                    let cc = (*cc).min(inner.width.saturating_sub(1).saturating_sub(gutter_w));
                    let cy = inner.y + cr;
                    let cx = inner.x + gutter_w + cc;
                    f.set_cursor_position((cx, cy));
                    let buf = f.buffer_mut();
                    let buf_area = buf.area;
                    if cy >= buf_area.y && cy < buf_area.y + buf_area.height
                        && cx >= buf_area.x && cx < buf_area.x + buf_area.width
                    {
                        let idx = (cy - buf_area.y) as usize * buf_area.width as usize
                            + (cx - buf_area.x) as usize;
                        if idx < buf.content.len() {
                            let cell = &mut buf.content[idx];
                            cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
                        }
                    }
                }
            }

            if has_border_label {
                let pane_title_str = title.as_deref().unwrap_or("");
                let pane_label = border_format
                    .replace("#{pane_title}", pane_title_str)
                    .replace("#{pane_index}", &id.to_string())
                    .replace("#P", &id.to_string());
                let label_width = unicode_width::UnicodeWidthStr::width(pane_label.as_str()) as u16;
                if label_width > 0 && area.width >= label_width {
                    let label_y = if border_status == "bottom" { area.y + area.height.saturating_sub(1) } else { area.y };
                    let label_area = Rect::new(area.x, label_y, label_width.min(area.width), 1);
                    let label_style = Style::default().fg(if *active { active_border_fg } else { border_fg });
                    f.render_widget(Paragraph::new(Line::from(Span::styled(pane_label, label_style))), label_area);
                }
            }
        }
        LayoutJson::Split { kind, sizes, children } => {
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = kind == "Horizontal";

            if zoomed {
                if let Some(i) = effective_sizes.iter().position(|&s| s != 0) {
                    if let Some(child) = children.get(i) {
                        render_layout_json(f, child, area, dim_preds, border_fg, active_border_fg, clock_mode, clock_colour, active_rect, mode_style_str, zoomed, border_status, border_format, total_panes, bchars, copy_ln);
                    }
                }
                return;
            }

            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);

            for (i, child) in children.iter().enumerate() {
                if i < rects.len() {
                    render_layout_json(f, child, rects[i], dim_preds, border_fg, active_border_fg, clock_mode, clock_colour, active_rect, mode_style_str, zoomed, border_status, border_format, total_panes, bchars, copy_ln);
                }
            }
            let border_style = Style::default().fg(border_fg);
            let active_border_style = Style::default().fg(active_border_fg);
            // pane-border-lines none: children are drawn, but no separators.
            let Some(bc) = bchars else { return; };
            let (v_char, h_char) = (bc.vertical, bc.horizontal);
            let buf = f.buffer_mut();
            for i in 0..children.len().saturating_sub(1) {
                if i >= rects.len() { break; }

                let both_leaves = matches!(&children[i], LayoutJson::Leaf { .. })
                    && matches!(children.get(i + 1), Some(LayoutJson::Leaf { .. }));

                if is_horizontal {
                    let sep_x = rects[i].x + rects[i].width;
                    if sep_x < buf.area.x + buf.area.width {
                        if both_leaves && total_panes == 2 {
                            let left_active = matches!(&children[i], LayoutJson::Leaf { active, .. } if *active);
                            let right_active = matches!(children.get(i + 1), Some(LayoutJson::Leaf { active, .. }) if *active);
                            let left_sty = if left_active { active_border_style } else { border_style };
                            let right_sty = if right_active { active_border_style } else { border_style };
                            let mid_y = area.y + area.height / 2;
                            for y in area.y..area.y + area.height {
                                let sty = if y < mid_y { left_sty } else { right_sty };
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize
                                    + (sep_x - buf.area.x) as usize;
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
                                let idx = (y - buf.area.y) as usize * buf.area.width as usize
                                    + (sep_x - buf.area.x) as usize;
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
                        if both_leaves && total_panes == 2 {
                            let top_active = matches!(&children[i], LayoutJson::Leaf { active, .. } if *active);
                            let bot_active = matches!(children.get(i + 1), Some(LayoutJson::Leaf { active, .. }) if *active);
                            let top_sty = if top_active { active_border_style } else { border_style };
                            let bot_sty = if bot_active { active_border_style } else { border_style };
                            let mid_x = area.x + area.width / 2;
                            for x in area.x..area.x + area.width {
                                let sty = if x < mid_x { top_sty } else { bot_sty };
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize
                                    + (x - buf.area.x) as usize;
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
                                let idx = (sep_y - buf.area.y) as usize * buf.area.width as usize
                                    + (x - buf.area.x) as usize;
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

/// Client-side border drag state — tracks an in-progress separator resize.
struct ClientDragState {
    path: Vec<usize>,
    kind: String,
    index: usize,
    start_pos: u16,
    initial_sizes: Vec<u16>,
    total_pixels: u16,
}

/// Connect to the server, authenticate, enter persistent mode, spawn the reader
/// thread, and return a (writer, frame_rx) pair ready for the event loop.
/// Sets a 5-second write timeout so blocked writes never freeze the client.
fn establish_connection(addr: &str, key: &str) -> io::Result<Connection> {
    establish_connection_with_timeout(addr, key, RECONNECT_CONNECT_TIMEOUT)
}

/// Connect budget for a RE-connect: generous, because the server is known to
/// have existed a moment ago and may simply be busy.
const RECONNECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Connect budget for the FIRST attach. A live server on loopback completes the
/// handshake in about a millisecond (the kernel accepts into the listen backlog
/// before the app ever calls accept), so a short budget cannot starve a healthy
/// server. It does cap how long a DEAD port can stall the attach: Windows drops
/// the SYN for an unbound loopback port and retransmits, so a refusal can take
/// upwards of two seconds to surface (issue #605). Waiting that out only to
/// print an OS error wastes the user's time; the liveness verdict is the same
/// either way.
const ATTACH_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);

fn establish_connection_with_timeout(
    addr: &str,
    key: &str,
    connect_timeout: Duration,
) -> io::Result<Connection> {
    // Bounded connect: a wedged/backlogged server otherwise makes `attach` hang
    // until the ~21s Windows SYN-retransmit and surface as `os error 10060`.
    // A bounded timeout turns that into a fast, clear failure the caller can
    // report (and lets try_reconnect back off promptly instead of stalling).
    let sockaddr: std::net::SocketAddr = addr.parse()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, format!("bad server address: {addr}")))?;
    let stream = std::net::TcpStream::connect_timeout(&sockaddr, connect_timeout)?;
    stream.set_nodelay(true)?;
    let mut writer = stream.try_clone()?;
    writer.set_nodelay(true)?;
    writer.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream);

    // Bound the AUTH read too: a wedged server that accepts the socket but never
    // answers would otherwise block the attach forever with no feedback. The
    // persistent read timeout is (re)set to 2s just below, after auth.
    let _ = reader.get_ref().set_read_timeout(Some(Duration::from_secs(3)));
    let _ = writer.write_all(format!("AUTH {}\n", key).as_bytes());
    let _ = writer.flush();
    let mut auth_line = String::new();
    let read_res = reader.read_line(&mut auth_line);
    if std::env::var("PSMUX_AUTH_DEBUG").map(|v| v == "1").unwrap_or(false) {
        eprintln!(
            "[auth-debug pid={}] establish_connection addr={} key={} read_res={:?} got_line={:?}",
            std::process::id(), addr, key, read_res.as_ref().map(|n| *n), auth_line
        );
    }
    read_res?;
    if !auth_line.trim().starts_with("OK") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "auth failed"));
    }

    let _ = writer.write_all(b"PERSISTENT\n");
    let _ = writer.write_all(b"client-attach\n");
    // Report the session this client came FROM, so the server can answer
    // `switch-client -l` from this client's own history rather than from a
    // machine-wide file (issue #566). This MUST follow client-attach: that is
    // what registers the client, and a value arriving before registration has
    // no entry to be recorded on and is silently dropped. Sent only when there
    // is something to say, so a first attach is byte-identical to before.
    if let Ok(prev) = std::env::var("PSMUX_CLIENT_LAST_SESSION") {
        if !prev.trim().is_empty() {
            let _ = writer.write_all(
                format!("client-last-session {}\n",
                    crate::util::quote_arg_if_needed(prev.trim())).as_bytes(),
            );
        }
    }
    // Issue #473: report the host terminal's colors (queried once at client
    // startup) so the server can answer pane color queries (OSC 4/10/11,
    // CSI ?996n) with the real palette instead of the Campbell fallback.
    if let Some(Some(spec)) = crate::types::HOST_COLORS_SPEC.get() {
        let _ = writer.write_all(format!("host-colors {}\n", spec).as_bytes());
    }
    let _ = writer.flush();

    // 2-second read timeout keeps the thread from blocking forever on process exit.
    let _ = reader.get_ref().set_read_timeout(Some(Duration::from_secs(2)));
    let (frame_tx, frame_rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = String::with_capacity(64 * 1024);
        loop {
            buf.clear();
            loop {
                match reader.read_line(&mut buf) {
                    Ok(0) => return,
                    Ok(_) => break,
                    Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut
                        || e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Do NOT clear buf here. read_line appends to whatever is
                        // already in buf, so a partial line from a previous
                        // fill_buf call is preserved across timeouts. Clearing
                        // would break the framing and corrupt the next message.
                        continue;
                    }
                    Err(_) => return,
                }
            }
            let line = std::mem::take(&mut buf);
            buf = String::with_capacity(64 * 1024);
            if frame_tx.send(line).is_err() { return; }
        }
    });

    Ok((writer, frame_rx))
}

/// A live connection: write half + incoming-frame channel.
type Connection = (std::net::TcpStream, std::sync::mpsc::Receiver<String>);

/// Retry connecting up to 5 times: one immediate attempt, then up to 4 more
/// with increasing backoff (500ms, 1s, 1.5s, 2s). Returns None if all fail.
fn try_reconnect(addr: &str, key: &str) -> Option<Connection> {
    for attempt in 0..5u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(attempt * 500));
        }
        if let Ok(result) = establish_connection(addr, key) {
            return Some(result);
        }
    }
    None
}

/// tmux's wording for an attach that cannot reach its session, verbatim:
/// `can't find session: NAME`, which `main` prints as `psmux: <msg>` and exits
/// 1. `-L` namespaces are stored on disk as `<ns>__<session>`; the attach gate
/// publishes the user-facing spelling in `PSMUX_SESSION_DISPLAY_NAME` so this
/// message shows the name that was typed rather than the internal one.
pub(crate) fn no_such_session(name: &str) -> io::Error {
    let shown = std::env::var("PSMUX_SESSION_DISPLAY_NAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.to_string());
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("can't find session: {}", shown),
    )
}

pub fn run_remote(terminal: &mut Terminal<crate::platform::PsmuxBackend>, input: &crate::ssh_input::InputSource) -> io::Result<()> {
    let name = env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string());
    let path = crate::paths::port_file(&name);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok())
        .ok_or_else(|| no_such_session(&name))?;
    let addr = format!("127.0.0.1:{}", port);
    // The .port file can be visible a beat before the .key has readable
    // content (servers before the issue #496 fix wrote .port first, and a
    // just-claimed warm server rewrites its files). Never AUTH with an empty
    // key — that is guaranteed to fail — poll briefly for the credential.
    let mut session_key = read_session_key(&name).unwrap_or_default();
    if session_key.is_empty() {
        for _ in 0..400 {
            std::thread::sleep(Duration::from_millis(5));
            session_key = read_session_key(&name).unwrap_or_default();
            if !session_key.is_empty() { break; }
        }
    }
    let session_key = session_key;
    if std::env::var("PSMUX_AUTH_DEBUG").map(|v| v == "1").unwrap_or(false) {
        eprintln!(
            "[auth-debug pid={}] run_remote name={} port={} key={}",
            std::process::id(), name, port, session_key
        );
    }
    let last_path = crate::paths::psmux_dir_file("last_session");
    if !crate::session::is_warm_session(&name) {
        let _ = std::fs::write(&last_path, &name);
    }
    // Attaching is activity: tmux restamps the session in server-client.c when a
    // client takes it (`server_client_set_session` -> `session_update_activity`).
    // Bare CLI routing ranks by this stamp, so it has to be written here rather
    // than left to the `last_session` file, which only ever names ONE session
    // machine-wide and so cannot express "B is newer than A" (issue #603).
    crate::session::touch_session_activity(&name);
    // The name this client keeps restamping while the user types. `None` for a
    // warm (standby) session, which is internal and never a routing candidate.
    let activity_name: Option<String> =
        if crate::session::is_warm_session(&name) { None } else { Some(name.clone()) };

    // ── Open persistent TCP connection ───────────────────────────────────
    // A failure here is NOT something to report in the operating system's own
    // words. The registry named a server; if it cannot be reached the session
    // is gone, and tmux says so in one line. Letting the raw io::Error escape
    // printed `psmux: No connection could be made because the target machine
    // actively refused it. (os error 10061)` (issue #605), which names neither
    // psmux's own concept (a session) nor anything the user can act on.
    let (mut writer, mut frame_rx) =
        match establish_connection_with_timeout(&addr, &session_key, ATTACH_CONNECT_TIMEOUT) {
            Ok(conn) => conn,
            Err(e) => {
                // Nothing answered on the registered port. Reap the entry so
                // the next `ls`/`attach` is clean, but only once the pid anchor
                // agrees the process is gone: a live server that was merely too
                // slow must keep its registration (it rewrites these files on
                // its own 5s tick anyway).
                if crate::session::registry_pid_anchor_alive(&name) != Some(true) {
                    crate::session::remove_session_registry(&name);
                }
                if std::env::var("PSMUX_AUTH_DEBUG").map(|v| v == "1").unwrap_or(false) {
                    eprintln!(
                        "[auth-debug pid={}] attach connect to {} failed: {} (kind {:?})",
                        std::process::id(), addr, e, e.kind()
                    );
                }
                return Err(no_such_session(&name));
            }
        };
    // Pending background reconnect: Some(rx) while a reconnect thread is running.
    // Kept as None in normal operation. When the channel yields Some(result),
    // the result replaces writer/frame_rx; if it yields None all attempts failed.
    let mut reconnect_pending: Option<std::sync::mpsc::Receiver<Option<Connection>>> = None;

    let mut quit = false;
    // detach-client -P: server sets this via DETACH-KILL-PARENT directive.
    // After we exit, kill the parent shell process for tmux -P parity (issue #275).
    let mut kill_parent_on_exit = false;
    let mut prefix_armed = false;
    let mut prefix_armed_at = Instant::now();
    let mut prefix_repeating = false;
    // Track whether IME was open before we suppressed it for prefix mode (issue #286).
    #[cfg(windows)]
    let mut ime_was_open = false;
    let mut repeat_time_ms: u64 = 500;
    let mut renaming = false;
    let mut session_renaming = false;
    let mut rename_buf = String::new();
    let mut pane_renaming = false;
    let mut pane_title_buf = String::new();
    let mut command_input = false;
    let mut command_buf = String::new();
    let mut command_cursor: usize = 0;
    let mut command_history: Vec<String> = Vec::new();
    let mut command_history_idx: usize = 0;
    // Template for command-prompt -I '#W' 'rename-window "%%"' style bindings.
    // When set, Enter substitutes %% with user input and executes the template.
    let mut command_template: Option<String> = None;
    // Custom prompt label from command-prompt -p 'prompt:'
    let mut command_prompt_label: Option<String> = None;
    // Track active window name from last dump-state for #W expansion
    let mut active_window_name = String::new();
    let mut window_idx_input = false;
    let mut window_idx_buf = String::new();

    let mut tree_chooser = false;
    let mut tree_entries: Vec<(bool, usize, usize, String, String)> = Vec::new();  // (is_win, id, sub_id, label, session_name)
    let mut tree_selected: usize = 0;
    let mut tree_scroll: usize = 0;
    // Digit-jump buffer for the choose-tree / choose-window picker.
    // Same UX as session_num_buffer: digits append, Enter jumps, Backspace
    // edits, Esc clears. Numbered prefix is rendered next to each row so
    // the digit-to-row mapping is visible.
    let mut tree_num_buffer = String::new();
    let mut buffer_chooser = false;
    let mut buffer_entries: Vec<(usize, usize, String)> = Vec::new();  // (index, byte_len, preview)
    let mut buffer_selected: usize = 0;
    let mut buffer_scroll: usize = 0;
    // Digit-jump buffer for the choose-buffer picker.
    let mut buffer_num_buffer = String::new();
    let mut session_chooser = false;
    let mut session_entries: Vec<(String, String)> = Vec::new();
    let mut session_selected: usize = 0;
    let mut session_scroll: usize = 0;
    // Digits typed while the picker is open accumulate here and are consumed
    // when the user presses Enter — "12" + Enter jumps to the 12th session.
    let mut session_num_buffer = String::new();
    let mut session_filter_active = false;
    let mut session_filter = String::new();
    // Digit-jump buffer for the customize-mode picker. Customize lives on
    // the server, so Enter computes a navigate delta and dispatches
    // `customize-navigate <delta>` instead of mutating local state directly.
    let mut customize_num_buffer = String::new();
    // Live preview cache for choose-tree / choose-session pickers (issue #257).
    // Keyed by "session\twin_id\tpane_id"; pane_id == usize::MAX => active pane.
    let mut preview_cache: crate::preview::PreviewCache = std::collections::HashMap::new();
    // Full-styled dump cache: every pane in a window with its own
    // `rows_v2` content, fetched in one round trip via `window-dump`.
    // This is the primary preview source — it sidesteps the per-pane
    // `capture-pane -t` round trips that mis-targeted the active pane,
    // and lets the client reuse the same renderer the main view uses.
    let mut dump_cache: crate::preview::DumpCache = std::collections::HashMap::new();
    // Whether the right-side preview pane is shown. Toggled by `p`
    // while a chooser is open. Persisted across reopens.
    let mut preview_enabled: bool = false;
    // Mirror of the server-side `choose-tree-preview` option. When true,
    // pickers open with `preview_enabled` already set so the user does not
    // need to press `p` each time. Configured via `set -g choose-tree-preview on`.
    let mut choose_tree_preview_default: bool = false;
    // Draggable popup state (shared across pickers). Offset is applied on top
    // of the centered rect; resets when no picker is open.
    let mut popup_offset: (i32, i32) = (0, 0);
    let mut popup_dragging: bool = false;
    let mut popup_drag_anchor: (u16, u16) = (0, 0);
    let mut popup_initial_offset: (i32, i32) = (0, 0);
    let mut popup_rect_last: Option<Rect> = None;
    let mut confirm_cmd: Option<String> = None;  // pending kill confirmation
    let current_session = name.clone();
    let mut last_sent_size: (u16, u16) = (0, 0);
    let mut last_status_lines: u16 = 1; // track server's status_lines for correct client-size height
    let mut last_dump_time = Instant::now() - Duration::from_millis(250);
    let mut force_dump = true;
    let mut last_tree: Vec<WinTree> = Vec::new();
    // Default prefix is Ctrl+B, updated dynamically from server config
    let mut prefix_key: (KeyCode, KeyModifiers) = (KeyCode::Char('b'), KeyModifiers::CONTROL);
    // Precompute the raw control character for the default prefix
    let mut prefix_raw_char: Option<char> = Some('\x02');
    // Secondary prefix key (prefix2), default None
    let mut prefix2_key: Option<(KeyCode, KeyModifiers)> = None;
    let mut prefix2_raw_char: Option<char> = None;
    // Status bar style from server (parsed from tmux status-style format)
    let mut status_fg: Color = Color::Black;
    // #372: terminal default until the server sends a parseable status-style.
    let mut status_bg: Color = Color::Reset;
    let mut status_bold: bool = false;
    let mut custom_status_left: Option<String> = None;
    let mut custom_status_right: Option<String> = None;
    let mut pane_border_fg: Color = Color::DarkGray;
    let mut pane_active_border_fg: Color = Color::Green;
    let mut pane_border_hover_fg: Color = Color::Yellow;
    let mut win_status_fmt: String = "#I:#W#{?window_flags,#{window_flags}, }".to_string();
    let mut win_status_current_fmt: String = "#I:#W#{?window_flags,#{window_flags}, }".to_string();
    let mut win_status_sep: String = " ".to_string();
    let mut win_status_style: Option<(Option<Color>, Option<Color>, bool)> = None;
    let mut win_status_current_style: Option<(Option<Color>, Option<Color>, bool)> = None;
    let mut mode_style_str: String = "bg=yellow,fg=black".to_string();
    let mut status_position_str: String = "bottom".to_string();
    let mut status_justify_str: String = "left".to_string();
    // Synced bindings from server (updated each frame from DumpState)
    let mut synced_bindings: Vec<BindingEntry> = Vec::new();
    let mut defaults_suppressed: bool = false;
    let mut scroll_enter_copy_mode: bool = true;
    // When false, Ctrl+V is forwarded to the child app instead of being
    // intercepted for paste detection.
    #[cfg(windows)]
    let mut paste_detection_enabled: bool = true;

    // ── Windows paste detection state ──────────────────────────────────
    // On Windows, Ctrl+V paste injects individual Key events BEFORE the
    // Ctrl+V Release event arrives (~184ms later).  We buffer ALL printable
    // chars for a short 20ms window.  If ≥3 chars arrive within 20ms, it's
    // almost certainly a paste — hold the buffer until Ctrl+V Release confirms
    // (up to 300ms), then send as a single bracketed paste (send-paste).
    // If <3 chars arrive within 20ms, flush them as normal send-text.
    // Pending chars being examined for paste detection.
    #[cfg(windows)]
    let mut paste_pend: String = String::new();
    // When the first char of the current pending group arrived.
    #[cfg(windows)]
    let mut paste_pend_start: Option<Instant> = None;
    // True once the 20ms window showed ≥3 chars — waiting for Ctrl+V Release.
    #[cfg(windows)]
    let mut paste_stage2: bool = false;
    // Set to true when Ctrl+V Release is seen — confirms the burst was a paste.
    #[cfg(windows)]
    let mut paste_confirmed: bool = false;
    // Buffer size at previous stage2 timeout check — for growth detection.
    #[cfg(windows)]
    let mut paste_stage2_last_len: usize = 0;
    // Suppression window: after right-click copy, discard text key events
    // for a short period to prevent VS Code ConPTY duplicate injection.
    // Declared on every platform because the right-click handler assigns it
    // unconditionally; only the Windows code paths ever read it.
    #[allow(unused_variables)]
    let mut paste_suppress_until: Option<Instant> = None;

    // Track whether a modified Enter Press was already handled this keypress
    // cycle.  WezTerm sends Shift+Enter as Release-only (no Press), so we
    // accept Release events for modified Enter and promote them to Press.
    // Windows Terminal, however, generates a real Press followed by a phantom
    // Release ~80ms later.  This flag suppresses that phantom duplicate.
    #[cfg(windows)]
    let mut modified_enter_press_handled: bool = false;

    // list-keys overlay state (C-b ?)
    let mut keys_viewer = false;
    let mut keys_viewer_lines: Vec<String> = Vec::new();
    let mut keys_viewer_scroll: usize = 0;

    // ── Server-side overlay state (updated each frame) ──
    // Initial values are overwritten on the first render frame; defaults
    // are kept here for safety in case the first state message is delayed.
    #[allow(unused_assignments)]
    let mut srv_popup_active = false;
    #[allow(unused_assignments)]
    let mut srv_popup_command = String::new();
    #[allow(unused_assignments)]
    let mut srv_popup_width: u16 = 80;
    #[allow(unused_assignments)]
    let mut srv_popup_height: u16 = 24;
    #[allow(unused_assignments)]
    let mut srv_popup_lines: Vec<String> = Vec::new();
    #[allow(unused_assignments)]
    let mut srv_popup_rows: Vec<crate::layout::RowRunsJson> = Vec::new();
    #[allow(unused_assignments)]
    let mut srv_floats: Vec<FloatJson> = Vec::new();
    #[allow(unused_assignments)]
    let mut srv_popup_has_pty = false;
    #[allow(unused_assignments)]
    let mut srv_popup_cursor: Option<(u16, u16)> = None; // popup-inner (col, row)
    let mut srv_popup_scroll: u16 = 0;
    #[allow(unused_assignments)]
    let mut srv_confirm_active = false;
    #[allow(unused_assignments)]
    let mut srv_confirm_prompt = String::new();
    #[allow(unused_assignments)]
    let mut srv_menu_active = false;
    #[allow(unused_assignments)]
    let mut srv_menu_title = String::new();
    #[allow(unused_assignments)]
    let mut srv_menu_selected: usize = 0;
    #[allow(unused_assignments)]
    let mut srv_menu_items: Vec<ServerMenuItem> = Vec::new();
    #[allow(unused_assignments)]
    let mut srv_display_panes = false;
    #[allow(unused_assignments)]
    let mut srv_pane_base_index: usize = 0;
    #[allow(unused_assignments)]
    let mut clock_active = false;
    #[allow(unused_assignments)]
    let mut clock_colour_str: Option<String> = None;

    // ── Customize-mode overlay state ──
    #[allow(unused_assignments)]
    let mut srv_customize_active = false;
    #[allow(unused_assignments)]
    let mut srv_customize_selected: usize = 0;
    #[allow(unused_assignments)]
    let mut srv_customize_scroll: usize = 0;
    #[allow(unused_assignments)]
    let mut srv_customize_editing = false;
    #[allow(unused_assignments)]
    let mut srv_customize_cursor: usize = 0;
    let mut srv_customize_edit_buf = String::new();
    let mut srv_customize_filter = String::new();
    #[allow(unused_assignments)]
    let mut srv_customize_options: Vec<CustomizeOption> = Vec::new();

    #[derive(serde::Deserialize, Default)]
    struct WinStatus { id: usize, name: String, active: bool, #[serde(default)] activity: bool, #[serde(default)] bell: bool, #[serde(default)] last: bool, #[serde(default)] tab_text: String, #[serde(default)] idx: usize }
    
    fn default_base_index() -> usize { 1 }
    fn default_prediction_dimming() -> bool { dim_predictions_enabled() }
    fn default_status_left_length() -> usize { 10 }
    fn default_status_right_length() -> usize { 40 }
    fn default_status_lines() -> usize { 1 }
    fn default_status_visible() -> bool { true }
    fn default_repeat_time() -> u64 { 500 }
    fn default_bold_is_bright() -> bool { true }
    fn default_paste_detection() -> bool { true }
    fn default_mouse_selection() -> bool { true }
    fn default_scroll_enter_copy_mode() -> bool { true }

    /// A single key binding synced from the server.
    #[derive(serde::Deserialize, Clone, Debug)]
    struct BindingEntry {
        /// Key table name (e.g. "prefix", "root")
        t: String,
        /// Key string (e.g. "C-a", "-", "F12")
        k: String,
        /// Command string (e.g. "split-window -v")
        c: String,
        /// Whether the binding is repeatable
        #[serde(default)]
        r: bool,
    }

    /// A menu item from server-side MenuMode
    #[derive(serde::Deserialize, Clone, Debug, Default)]
    struct ServerMenuItem {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        sep: bool,
    }

    /// A customize-mode option row from server
    #[derive(serde::Deserialize, Clone, Debug, Default)]
    struct CustomizeOption {
        /// Original index in the full options list
        i: usize,
        /// Option name
        n: String,
        /// Current value
        v: String,
        /// Scope (server/session/window/pane)
        s: String,
    }

    #[derive(serde::Deserialize)]
    struct DumpState {
        layout: LayoutJson,
        windows: Vec<WinStatus>,
        #[serde(default)]
        prefix: Option<String>,
        #[serde(default)]
        prefix2: Option<String>,
        #[serde(default)]
        tree: Vec<WinTree>,
        #[serde(default = "default_base_index")]
        base_index: usize,
        #[serde(default = "default_prediction_dimming")]
        prediction_dimming: bool,
        #[serde(default)]
        status_style: Option<String>,
        #[serde(default)]
        status_left: Option<String>,
        #[serde(default)]
        status_right: Option<String>,
        #[serde(default)]
        pane_border_style: Option<String>,
        #[serde(default)]
        pane_active_border_style: Option<String>,
        #[serde(default)]
        pane_border_hover_style: Option<String>,
        #[serde(default)]
        pane_border_status: Option<String>,
        #[serde(default)]
        pane_border_format: Option<String>,
        #[serde(default)]
        pane_border_lines: Option<String>,
        /// copy-mode-line-numbers option (off/default/absolute/relative/hybrid)
        #[serde(default)]
        copy_mode_line_numbers: Option<String>,
        /// Active pane scrollback size, for absolute/hybrid line numbers.
        #[serde(default)]
        copy_hsize: usize,
        #[serde(default)]
        copy_mode_line_number_style: Option<String>,
        #[serde(default)]
        copy_mode_current_line_number_style: Option<String>,
        /// window-status-format (short key to save bandwidth)
        #[serde(default)]
        wsf: Option<String>,
        /// window-status-current-format
        #[serde(default)]
        wscf: Option<String>,
        /// window-status-separator
        #[serde(default)]
        wss: Option<String>,
        /// window-status-style
        #[serde(default)]
        ws_style: Option<String>,
        /// window-status-current-style
        #[serde(default)]
        wsc_style: Option<String>,
        /// #451: status-left-style (was dropped in modularization)
        #[serde(default)]
        status_left_style: Option<String>,
        /// #451: status-right-style
        #[serde(default)]
        status_right_style: Option<String>,
        /// #451: window-status-activity-style
        #[serde(default)]
        wsa_style: Option<String>,
        /// #451: window-status-bell-style
        #[serde(default)]
        wsb_style: Option<String>,
        /// #451: window-status-last-style
        #[serde(default)]
        wsl_style: Option<String>,
        /// clock-mode active
        #[serde(default)]
        clock_mode: bool,
        /// clock-mode-colour (tmux option)
        #[serde(default)]
        clock_colour: Option<String>,
        /// Dynamic key bindings from server
        #[serde(default)]
        bindings: Vec<BindingEntry>,
        /// When true, hardcoded default keybindings are suppressed (set by unbind-key -a)
        #[serde(default)]
        defaults_suppressed: bool,
        /// scroll-enter-copy-mode option (mirror of server-side AppState field).
        /// When false, root key bindings that enter copy mode (e.g. PageUp ->
        /// copy-mode -u) are skipped so the key reaches the PTY (#284).
        #[serde(default = "default_scroll_enter_copy_mode")]
        scroll_enter_copy_mode: bool,
        /// pwsh-mouse-selection option (mirror of server-side AppState field)
        #[serde(default)]
        pwsh_mouse_selection: bool,
        /// mouse-selection option (mirror of server-side AppState field).
        /// When false, client suppresses its own drag-selection overlay so
        /// in-pane apps (opencode, etc.) can do their own mouse selection.
        #[serde(default = "default_mouse_selection")]
        mouse_selection: bool,
        /// mouse-selection-force option. When true, client-side drag
        /// selection overrides an application's mouse tracking.
        #[serde(default)]
        mouse_selection_force: bool,
        /// paste-detection option (mirror of server-side AppState field)
        #[serde(default = "default_paste_detection")]
        paste_detection: bool,
        /// choose-tree-preview option: when true, choose-session and
        /// choose-tree pickers open with the live preview pane visible.
        #[serde(default)]
        choose_tree_preview: bool,
        /// status-left-length (max display width for left status)
        #[serde(default = "default_status_left_length")]
        status_left_length: usize,
        /// status-right-length (max display width for right status)
        #[serde(default = "default_status_right_length")]
        status_right_length: usize,
        /// Number of status bar lines
        #[serde(default = "default_status_lines")]
        status_lines: usize,
        /// Custom format strings for additional status lines
        #[serde(default)]
        status_format: Vec<String>,
        /// mode-style for copy mode selection highlighting
        #[serde(default)]
        mode_style: Option<String>,
        /// message-style for the status-line message bar (display-message,
        /// command prompt). #372: previously never sent, so the client
        /// hard-coded bg=yellow,fg=black and ignored the user's option.
        #[serde(default)]
        message_style: Option<String>,
        /// status-position: "top" or "bottom"
        #[serde(default)]
        status_position: Option<String>,
        /// status-justify: "left", "centre", or "right"
        #[serde(default)]
        status_justify: Option<String>,
        /// Whether the status bar is visible (true) or hidden (false).
        /// Corresponds to `set-option status on/off`.
        #[serde(default = "default_status_visible")]
        status_visible: bool,
        /// Configured cursor style as DECSCUSR code (0-6) from server.
        /// Used as fallback when no child process has set a cursor shape.
        #[serde(default)]
        cursor_style_code: Option<u8>,
        /// One-shot clipboard text (base64-encoded) for OSC 52 delivery.
        #[serde(default)]
        clipboard_osc52: Option<String>,
        /// One-shot bell flag: server signals client to emit \x07 to the host terminal.
        #[serde(default)]
        bell: bool,
        /// set-titles: server pushes the expanded set-titles-string here when
        /// `set-titles on`.  Client emits OSC 0 to its host terminal whenever
        /// this value changes so external terminal tabs (Windows Terminal,
        /// iTerm2, etc.) follow the active pane / window title.
        #[serde(default)]
        host_title: Option<String>,
        /// Issue #269: OSC 9;4 progress indicator from the active pane,
        /// formatted as "<state>;<value>".  Client emits OSC 9;4 to its host
        /// terminal so apps inside a pane (Copilot CLI, build tools) keep
        /// driving the Windows Terminal taskbar / tab progress indicator.
        #[serde(default)]
        host_progress: Option<String>,
        /// Repeat key timeout in ms (default: 500, synced from server)
        #[serde(default = "default_repeat_time")]
        repeat_time: u64,
        /// bold-is-bright option (issue #425): controls whether the console
        /// writer rewrites crossterm's 256-indexed basic colors to standard SGR.
        /// The writer lives in this client process, so the value is synced from
        /// the server and pushed into the writer's atomic each frame.
        #[serde(default = "default_bold_is_bright")]
        bold_is_bright: bool,
        /// Whether a pane is currently zoomed (borders should be hidden)
        #[serde(default)]
        zoomed: bool,
        // ── Server-side overlay state ──
        /// Popup overlay active
        #[serde(default)]
        popup_active: bool,
        #[serde(default)]
        popup_command: Option<String>,
        #[serde(default)]
        popup_width: Option<u16>,
        #[serde(default)]
        popup_height: Option<u16>,
        #[serde(default)]
        popup_lines: Vec<String>,
        #[serde(default)]
        popup_rows: Vec<crate::layout::RowRunsJson>,
        #[serde(default)]
        popup_has_pty: bool,
        /// Cursor of the process running inside a PTY popup, relative to the
        /// popup's inner (inside-the-border) area.
        #[serde(default)]
        popup_cursor_row: Option<u16>,
        #[serde(default)]
        popup_cursor_col: Option<u16>,
        #[serde(default)]
        popup_hide_cursor: bool,
        /// Floating panes (tmux new-pane) overlaid on the active window.
        #[serde(default)]
        floats: Vec<FloatJson>,
        /// Confirm overlay active
        #[serde(default)]
        confirm_active: bool,
        #[serde(default)]
        confirm_prompt: Option<String>,
        /// Menu overlay active
        #[serde(default)]
        menu_active: bool,
        #[serde(default)]
        menu_title: Option<String>,
        #[serde(default)]
        menu_selected: usize,
        #[serde(default)]
        menu_items: Vec<ServerMenuItem>,
        /// Display-panes overlay active
        #[serde(default)]
        display_panes: bool,
        /// Pane base index for display-panes numbering
        #[serde(default)]
        pane_base_index: usize,
        /// Status bar message from display-message (without -p)
        #[serde(default)]
        status_message: Option<String>,
        /// Customize-mode overlay active
        #[serde(default)]
        customize_active: bool,
        #[serde(default)]
        customize_selected: usize,
        #[serde(default)]
        customize_scroll: usize,
        #[serde(default)]
        customize_editing: bool,
        #[serde(default)]
        customize_cursor: usize,
        #[serde(default)]
        customize_edit_buf: Option<String>,
        #[serde(default)]
        customize_filter: Option<String>,
        #[serde(default)]
        customize_options: Vec<CustomizeOption>,
    }

    let mut cmd_batch: Vec<String> = Vec::new();
    let mut dump_buf = String::new();
    let mut prev_dump_buf = String::new();
    let mut last_key_send_time: Option<Instant> = None;
    let mut dump_in_flight = false;
    let mut dump_flight_start: Instant = Instant::now();

    // Diagnostic latency log: set PSMUX_LATENCY_LOG=1 to enable
    let latency_log_enabled = env::var("PSMUX_LATENCY_LOG").unwrap_or_default() == "1";
    let mut latency_log: Option<std::fs::File> = if latency_log_enabled {
        let path = crate::paths::psmux_dir_file("latency.log");
        std::fs::File::create(&path).ok()
    } else { None };
    let mut loop_count: u64 = 0;
    let mut _last_key_char: Option<char> = None;
    let mut key_send_instant: Option<Instant> = None; // when the key was SENT to server

    // Text selection state (client-side only, left-click drag like pwsh)
    let mut rsel_start: Option<(u16, u16)> = None;  // (col, row) in terminal coords
    let mut rsel_end: Option<(u16, u16)> = None;
    let mut rsel_pane_rect: Option<Rect> = None;    // clip bounds of the originating pane
    let mut rsel_pane_id: Option<usize> = None;     // pane the selection started in
    let mut rsel_dragged = false;
    // Server-side copy-mode drag tracking.  The pane the drag started in is
    // remembered so drag/release events keep reaching the server after the
    // pointer leaves the pane rect (out-of-range rows drive edge
    // auto-scroll), and a 50ms repeat re-sends the last drag while the
    // pointer dwells at/past the pane's first or last row so the view keeps
    // scrolling without mouse movement (tmux drag-scroll parity).
    let mut copy_drag_pane: Option<(usize, Rect)> = None; // (pane_id, rect at drag start)
    let mut copy_drag_last: Option<(i16, i16)> = None;    // last pane-relative (col, row) sent
    let mut copy_drag_repeat_at: Option<Instant> = None;  // next repeat due (armed only at an edge)
    const COPY_DRAG_REPEAT: Duration = Duration::from_millis(50);
    // A click withheld from a mouse-aware pane while psmux determines whether
    // the gesture is a click or a selection drag: (pane_id, col, row).
    let mut deferred_left_click: Option<(usize, i16, i16)> = None;
    // Multi-click tracking for word/line selection.
    let mut last_click: Option<(Instant, (u16, u16))> = None;
    let mut click_count: u32 = 0;
    // When true, the current rsel selection uses rectangular (block) mode
    // instead of reading-order. Triggered by Alt held on MouseDown.
    let mut rsel_block: bool = false;
    let mut selection_changed = false; // forces redraw for selection overlay
    let mut border_drag = false; // true when dragging a pane separator (resize)
    // Track rendered tab positions for accurate mouse click detection.
    // (row, window_display_idx, x_start, x_end) — one entry per clickable tab
    // span, on WHICHEVER status row it renders (#593: multi-row bars carry
    // clickable ranges on every line, matching tmux status_get_range).
    let mut client_tab_positions: Vec<(u16, usize, u16, u16)> = Vec::new();
    let mut client_status_row: u16 = u16::MAX; // first status bar row
    let mut client_status_rows: u16 = 0; // status bar height in rows (0 = hidden)
    // Display indices of windows that actually exist, so a click on a
    // #[range=window|N] naming a nonexistent window is ignored like tmux
    // (server-client.c returns KEYC_UNKNOWN when winlink_find_by_index fails).
    let mut client_window_indices: Vec<usize> = Vec::new();
    let mut client_pane_rects: Vec<(usize, Rect)> = Vec::new();
    // Per-pane live scrollback offset from the last dump (see view_offset).
    let mut client_pane_view_offsets: Vec<(usize, usize)> = Vec::new();
    let mut client_borders: Vec<(Vec<usize>, String, usize, u16, u16, Vec<u16>, Rect)> = Vec::new();
    let mut client_content_area: Rect = Rect::default();
    // Border status/format from the last draw, for cursor/mouse inner-rect calc.
    let mut client_border_status: String = "off".to_string();
    let mut client_border_format: String = String::new();
    let mut client_copy_mode: bool = false;
    let mut client_pwsh_selection: bool = false;
    let mut client_mouse_selection: bool = true;
    let mut client_mouse_selection_force: bool = false;
    let mut client_zoomed: bool = false;
    let mut client_drag: Option<ClientDragState> = None;
    // Border hover highlight: (position, kind, area) of the border under the cursor.
    let mut hovered_border: Option<(u16, String, Rect)> = None;
    // Buffered OSC 52 clipboard text — written AFTER terminal.draw() to
    // avoid corrupting ratatui's output buffer.
    let mut pending_osc52: Option<String> = None;
    let mut pending_bell = false;
    // Last OSC 0 (host terminal title) value emitted to the host terminal.
    // Tracked across iterations so we only re-emit when the title changes.
    let mut last_emitted_host_title: Option<String> = None;
    // Issue #269: last OSC 9;4 (host terminal progress) value emitted.
    // Same debounce pattern as host_title.
    let mut last_emitted_host_progress: Option<String> = None;
    // VT input mode (SSH / JediTerm / WezTerm): used to pick the Win32
    // caret path below.  The periodic mouse-enable refresh keyed off
    // last_mouse_enable runs in EVERY input mode via send_mouse_keepalive —
    // Windows Terminal drops even a local client's mouse registration.
    let is_ssh_mode = crate::ssh_input::needs_vt_input();
    let mut last_mouse_enable = Instant::now();
    // Raw VT pipe: cadence for the XTWINOPS size poll.
    let mut last_pipe_size_query = Instant::now();
    // ── Cursor blink stabilisation ──────────────────────────────────
    // Cache the last-sent DECSCUSR code so we only write it when it
    // actually changes (avoids resetting WT's blink timer every frame).
    let mut last_cursor_style: u8 = 255;
    // Trap Ctrl+Break (and stray Ctrl+C) console signals so they interrupt the
    // pane's foreground program instead of terminating this client and
    // detaching the still-running session (issue #454).  The signal is drained
    // into cmd_batch as `send-key C-Break` at the top of each iteration.
    crate::platform::install_client_console_ctrl_handler();
    loop {
        // ── Poll background reconnect result (non-blocking) ──────────────────
        // If a background reconnect thread has finished, apply its result here
        // before any other step so the rest of the loop sees the fresh channels.
        if let Some(ref rx) = reconnect_pending {
            if let Ok(result) = rx.try_recv() {
                reconnect_pending = None;
                if let Some((new_writer, new_rx)) = result {
                    if !quit {
                        // Normal reconnect: apply fresh writer + frame channel.
                        writer = new_writer;
                        frame_rx = new_rx;
                        force_dump = true;
                        dump_in_flight = false;
                    }
                    // If quit is already set (user detached while reconnect was
                    // in-flight), let new_writer drop here so the TcpStream
                    // closes immediately — server-side Guard fires right away
                    // rather than waiting for the 5 s write timeout.
                } else {
                    quit = true;
                }
            }
        }
        // Expire stale key_send_instant after 30ms — ConPTY echo should
        // have arrived by then; stop force-dumping to save CPU.
        if let Some(ks) = key_send_instant {
            if ks.elapsed().as_millis() > 30 { key_send_instant = None; }
        }
        // Safety valve: if dump_in_flight is stuck for >500ms (e.g. server
        // did not respond), release it so the client doesn't spin at 1ms.
        if dump_in_flight && dump_flight_start.elapsed().as_millis() > 500 {
            dump_in_flight = false;
        }
        // ── STEP 0: Receive latest frame from reader thread (non-blocking) ──
        // Drain channel, keeping only the most recent frame.
        let mut got_frame = false;
        let mut _nc_count = 0u32;
        loop {
            match frame_rx.try_recv() {
                Ok(line) => {
                    if line.trim() == "NC" {
                        _nc_count += 1;
                        // Server says nothing changed — release dump_in_flight
                        // without touching dump_buf (saves 50-100KB clone + parse).
                        dump_in_flight = false;
                        last_dump_time = Instant::now();
                        // If we're waiting for a key echo, force an
                        // immediate dump-state re-request (~1ms TCP RTT)
                        // instead of waiting the full 10ms typing interval.
                        if key_send_instant.is_some() {
                            force_dump = true;
                        }
                    } else if line.trim().starts_with("SWITCH ") {
                        // Server is telling us to switch to another session
                        let target_session = line.trim().strip_prefix("SWITCH ").unwrap_or("").to_string();
                        if !target_session.is_empty() {
                            env::set_var("PSMUX_SWITCH_TO", &target_session);
                            let _ = writer.write_all(b"client-detach\n");
                            let _ = writer.flush();
                            quit = true;
                        }
                    } else if line.trim() == "DETACH" {
                        // Server-initiated clean shutdown (session ended, kill-session, etc.)
                        // Set quit before the TCP connection closes so the Disconnected
                        // handler does not spawn a reconnect thread.
                        quit = true;
                    } else if line.trim() == "DETACH-KILL-PARENT" {
                        // detach-client -P: detach this client AND kill the
                        // parent shell on exit (tmux -P parity, issue #275).
                        kill_parent_on_exit = true;
                        quit = true;
                    } else {
                        if client_log_enabled() {
                            client_log("frame", &format!("received {} bytes", line.len()));
                        }
                        dump_buf = line; got_frame = true; dump_in_flight = false;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // TCP connection dropped. If the server already removed its port
                    // file (all clean-shutdown paths do this before calling
                    // shutdown_persistent_streams), treat the disconnect as intentional
                    // and quit immediately instead of burning 5 s of reconnect backoff.
                    if !quit && !std::path::Path::new(&path).exists() {
                        quit = true;
                    } else if reconnect_pending.is_none() && !quit {
                        let addr_c = addr.clone();
                        let key_c = session_key.clone();
                        let (rtx, rrx) = std::sync::mpsc::channel();
                        reconnect_pending = Some(rrx);
                        std::thread::spawn(move || {
                            let _ = rtx.send(try_reconnect(&addr_c, &key_c));
                        });
                    }
                    break;
                }
            }
        }
        if quit && !got_frame { break; }

        // ── STEP 1: Poll events with adaptive timeout ────────────────────
        let since_dump = last_dump_time.elapsed().as_millis() as u64;
        // Expire typing timer after 100ms of no new keys
        if let Some(kt) = last_key_send_time {
            if kt.elapsed().as_millis() > 100 { last_key_send_time = None; }
        }
        let typing_active = last_key_send_time.is_some();
        // When typing: cap at ~100fps to avoid flooding the server with
        // dump-state requests (each one is ~50-100KB of JSON over TCP).
        // When idle: 50ms refresh (20fps) saves CPU.
        // Use fast poll when paste chars are pending (need timely detection)
        #[cfg(windows)]
        let paste_pend_active = !paste_pend.is_empty();
        #[cfg(not(windows))]
        let paste_pend_active = false;

        let poll_ms = if paste_pend_active { 1 }
            else if got_frame { 0 }
            else if dump_in_flight { 5 }
            else if force_dump { 0 }
            else if typing_active {
                // Rate-limit to ~100fps (10ms) when typing.  The snapshot-
                // based serialisation in dump_layout_json_fast now holds
                // the parser mutex for only ~1ms (cell snapshot), so
                // polling at 10ms no longer starves the ConPTY reader
                // thread.  10ms is notably shorter than ConPTY's ~16ms
                // render interval, avoiding systematic alignment delays.
                let remaining = 10u64.saturating_sub(since_dump);
                remaining
            }
            else {
                // Server pushes frames proactively via auto-push —
                // no need for fast idle polling.  16ms (~60fps) ensures
                // pushed frames render within one vsync while using
                // negligible CPU (vs 50ms poll + dump-state roundtrip).
                16
            };

        cmd_batch.clear();

        // ── Ctrl+Break signal → forward to pane (issue #454) ───────────
        // A trapped Ctrl+Break console signal can't come through the key loop,
        // so drain it here and relay it to the active pane's foreground process.
        // Write straight to the server rather than via cmd_batch: cmd_batch is
        // cleared every iteration and some event branches `continue` before the
        // batch flush, which could silently drop the break.
        if crate::platform::take_client_ctrl_break() {
            let _ = writer.write_all(b"send-key C-Break\n");
            let _ = writer.flush();
        }

        // ── Windows paste pending-buffer management ────────────────────
        // Flush or promote chars based on how long they've been buffered.
        #[cfg(windows)]
        {
            if let Some(start) = paste_pend_start {
                let elapsed = start.elapsed();
                if paste_confirmed {
                    // Ctrl+V Release already seen — send as paste now
                    if !paste_pend.is_empty() {
                        if input_log_enabled() {
                            input_log("paste", &format!("paste CONFIRMED (top), sending {} chars as send-paste: {:?}",
                                paste_pend.len(), &paste_pend.chars().take(200).collect::<String>()));
                        }
                        let encoded = base64_encode(&paste_pend);
                        cmd_batch.push(format!("send-paste {}\n", encoded));
                        // Suppress clipboard-read fallback
                        paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
                    }
                    paste_pend.clear();
                    paste_pend_start = None;
                    paste_stage2 = false;
                    paste_confirmed = false;
                } else if !paste_stage2 && elapsed > Duration::from_millis(20) {
                    // 20ms window expired
                    let has_non_ascii = paste_pend.chars().any(|c| !c.is_ascii());
                    if paste_pend.len() >= 3 && !has_non_ascii {
                        // ≥3 ASCII chars in 20ms → likely paste, enter stage 2.
                        // Non-ASCII chars (IME composition, CJK input) are excluded
                        // because IME routinely generates 3+ chars in <20ms and would
                        // trigger a false-positive 300ms delay (fixes #91).
                        paste_stage2 = true;
                        paste_stage2_last_len = paste_pend.len();
                        if input_log_enabled() {
                            input_log("paste", &format!("stage2: {} chars in 20ms, waiting for Ctrl+V Release", paste_pend.len()));
                        }
                    } else if paste_pend.len() >= 20 && has_non_ascii {
                        // ≥20 non-ASCII chars in 20ms — almost certainly a paste
                        // containing Unicode content (em-dashes, CJK, etc.), not
                        // IME composition (which rarely exceeds a few chars).
                        paste_stage2 = true;
                        paste_stage2_last_len = paste_pend.len();
                        if input_log_enabled() {
                            input_log("paste", &format!("stage2 (large non-ASCII): {} chars in 20ms", paste_pend.len()));
                        }
                    } else if paste_pend.len() >= 3 && has_non_ascii {
                        // ≥3 chars but contains non-ASCII (IME input) — flush
                        // immediately as normal text to avoid 300ms delay.
                        if input_log_enabled() {
                            input_log("paste", &format!("flush {} chars as normal (non-ASCII / IME detected)", paste_pend.len()));
                        }
                        for c in paste_pend.chars() {
                            match c {
                                '\n' => { cmd_batch.push("send-key enter\n".into()); }
                                '\t' => { cmd_batch.push("send-key tab\n".into()); }
                                ' '  => { cmd_batch.push("send-key space\n".into()); }
                                _ => {
                                    let escaped = match c {
                                        '"' => "\\\"".to_string(),
                                        '\\' => "\\\\".to_string(),
                                        _ => c.to_string(),
                                    };
                                    cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                                }
                            }
                        }
                        paste_pend.clear();
                        paste_pend_start = None;
                    } else {
                        // <3 chars → normal typing, flush as send-text
                        if input_log_enabled() {
                            input_log("paste", &format!("flush {} chars as normal (< 3 in 20ms)", paste_pend.len()));
                        }
                        for c in paste_pend.chars() {
                            match c {
                                '\n' => { cmd_batch.push("send-key enter\n".into()); }
                                '\t' => { cmd_batch.push("send-key tab\n".into()); }
                                ' '  => { cmd_batch.push("send-key space\n".into()); }
                                _ => {
                                    let escaped = match c {
                                        '"' => "\\\"".to_string(),
                                        '\\' => "\\\\".to_string(),
                                        _ => c.to_string(),
                                    };
                                    cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                                }
                            }
                        }
                        paste_pend.clear();
                        paste_pend_start = None;
                    }
                } else if paste_stage2 && elapsed > Duration::from_millis(300) {
                    // Stage 2 timeout — no Ctrl+V Release arrived.
                    // Growth detection: if the buffer grew since last check,
                    // ConPTY is still injecting characters (large paste).
                    // Extend the window instead of splitting the paste.
                    if paste_pend.len() > paste_stage2_last_len {
                        paste_stage2_last_len = paste_pend.len();
                        paste_pend_start = Some(Instant::now() - Duration::from_millis(280));
                    } else {
                        // Buffer stopped growing — send accumulated chars as
                        // send-paste so the server wraps in bracketed paste.
                        if input_log_enabled() {
                            input_log("paste", &format!("stage2 timeout, sending {} chars as send-paste", paste_pend.len()));
                        }
                        let encoded = base64_encode(&paste_pend);
                        cmd_batch.push(format!("send-paste {}\n", encoded));
                        paste_pend.clear();
                        paste_pend_start = None;
                        paste_stage2 = false;
                        paste_stage2_last_len = 0;
                        // Suppress the clipboard-read fallback that fires
                        // when Ctrl+V Release arrives later (the paste was
                        // already sent via stage2).
                        paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
                    }
                }
            }
        }

        {
            let mut _pending_evt = input.read_timeout(Duration::from_millis(poll_ms))?;
            while let Some(_cur_evt) = _pending_evt {
                // Input debug: log every raw event BEFORE filtering
                if input_log_enabled() {
                    match &_cur_evt {
                        Event::Key(key) => {
                            input_log("event", &format!(
                                "Key code={:?} mods={:?} kind={:?} state={:?}",
                                key.code, key.modifiers, key.kind, key.state
                            ));
                        }
                        Event::Mouse(me) => {
                            input_log("event", &format!("Mouse {:?}", me.kind));
                        }
                        Event::Resize(w, h) => {
                            input_log("event", &format!("Resize {}x{}", w, h));
                        }
                        Event::Paste(d) => {
                            input_log("event", &format!("Paste ({} bytes)", d.len()));
                        }
                        other => {
                            input_log("event", &format!("Other {:?}", other));
                        }
                    }
                }
                // Real input from a real client is what tmux counts as session
                // activity (server-client.c `server_client_handle_key` restamps
                // `activity_time` before dispatching, and mouse arrives on the
                // same path as a key). Resize is deliberately excluded: it is
                // the terminal talking, not the user. Bare CLI commands rank
                // sessions by this stamp, so typing in the session you are
                // sitting in is what makes it win (issue #603).
                if let Some(ref sess) = activity_name {
                    if matches!(_cur_evt, Event::Key(_) | Event::Mouse(_) | Event::Paste(_)) {
                        crate::session::touch_session_activity_throttled(sess);
                    }
                }
                match _cur_evt {
                    // ── Windows Ctrl+V paste interception ────────────────
                    // ── Suppress phantom modified-Enter Release (Windows Terminal) ──
                    // Windows Terminal fires a real Press then a phantom Release
                    // ~80ms later for Shift+Enter.  If we already handled the
                    // Press, drop the Release so it does not trigger the WezTerm
                    // Release-only acceptance path and produce a double newline.
                    #[cfg(windows)]
                    Event::Key(key) if key.kind == KeyEventKind::Release
                        && matches!(key.code, KeyCode::Enter)
                        && modified_enter_press_handled =>
                    {
                        // drop the phantom Release
                    }
                    // On Windows, Windows Terminal intercepts Ctrl+V Press,
                    // reads the clipboard, and injects the paste content as
                    // a byte stream into the ConPTY input pipe — bypassing
                    // the console input buffer that crossterm reads via
                    // ReadConsoleInputW.  Only the Ctrl+V *Release* event
                    // leaks through.  We use that Release as a trigger to
                    // read the clipboard ourselves and forward the content
                    // as a bracketed-paste so child apps (Claude CLI, etc.)
                    // can distinguish paste from typed input.
                    // Skipped when paste-detection is off.
                    #[cfg(windows)]
                    Event::Key(key) if key.kind == KeyEventKind::Release
                        && matches!(key.code, KeyCode::Char('v'))
                        && key.modifiers == KeyModifiers::CONTROL
                        && paste_detection_enabled =>
                    {
                        if input_log_enabled() {
                            input_log("paste", &format!("Ctrl+V Release detected, paste_pend len={}", paste_pend.len()));
                        }
                        paste_confirmed = true;
                    }
                    // ── WezTerm: Shift+Enter arrives as Release-only ──
                    // WezTerm generates only KeyEventKind::Release for Shift+Enter
                    // (no Press, no Repeat).  Accept and promote to Press.
                    #[cfg(windows)]
                    Event::Key(mut key) if key.kind == KeyEventKind::Release
                        && matches!(key.code, KeyCode::Enter)
                        && !key.modifiers.is_empty() =>
                    {
                        key.kind = KeyEventKind::Press;
                        crate::platform::augment_enter_shift(&mut key);
                        modified_enter_press_handled = true;
                        // Skip paste buffering — forward directly like a Press.
                        let is_prefix = (key.code, key.modifiers) == prefix_key
                            || prefix_raw_char.map_or(false, |c| matches!(key.code, KeyCode::Char(ch) if ch == c))
                            || prefix2_key.map_or(false, |p2| (key.code, key.modifiers) == p2)
                            || prefix2_raw_char.map_or(false, |c| matches!(key.code, KeyCode::Char(ch) if ch == c));
                        if !is_prefix {
                            if let Some(encoded) = crate::input::encode_key_event(&key) {
                                cmd_batch.push(format!("send-key-raw {}\n",
                                    encoded.iter().map(|b| format!("{:02x}", b)).collect::<String>()));
                            }
                        }
                    }
                    Event::Key(mut key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
                        // Fold NUL onto C-Space before anything looks at the key.
                        // The console reports Ctrl+Space, Ctrl+2, Ctrl+Shift+2 and a
                        // NUL byte sent by a ConPTY-hosted terminal identically, and
                        // all of them ARE NUL.  Folding here (not just in the binding
                        // lookup) is what lets the raw `is_prefix` comparison below
                        // match a `C-Space` prefix, and makes the byte forwarded to
                        // the pane 0x00 instead of 0x12 (issue #504).
                        {
                            let folded = crate::config::fold_nul_to_ctrl_space((key.code, key.modifiers));
                            key.code = folded.0;
                            key.modifiers = folded.1;
                        }
                        // On Windows, VS Code's xterm.js sends ESC+CR for
                        // Shift+Enter.  ConPTY interprets the ESC as Alt, so
                        // crossterm reports Alt+Enter.  Poll the physical
                        // keyboard to detect the real modifier.
                        #[cfg(windows)]
                        crate::platform::augment_enter_shift(&mut key);
                        // Clear the WezTerm dedup flag on any non-Enter key; set
                        // it when a modified Enter Press is being processed.
                        #[cfg(windows)]
                        {
                            if matches!(key.code, KeyCode::Enter) && !key.modifiers.is_empty() {
                                modified_enter_press_handled = true;
                            } else {
                                modified_enter_press_handled = false;
                            }
                        }
                        // Flush pending paste buffer before processing any non-bufferable key.
                        // Bufferable keys are: plain Char, Space, Enter (if pend non-empty), Tab (if pend non-empty).
                        #[cfg(windows)]
                        {
                            if !paste_pend.is_empty() {
                                let is_bufferable = match key.code {
                                    KeyCode::Char(' ') => true,
                                    KeyCode::Char(c) => {
                                        // AltGr on Windows is reported as Ctrl+Alt.
                                        // Non-letter chars with Ctrl+Alt are AltGr-produced
                                        // (e.g. \ @ { } on German/Czech keyboards) and
                                        // should be bufferable like normal text.
                                        let is_altgr = key.modifiers.contains(KeyModifiers::CONTROL)
                                            && key.modifiers.contains(KeyModifiers::ALT)
                                            && !c.is_ascii_lowercase();
                                        is_altgr || (!key.modifiers.contains(KeyModifiers::CONTROL)
                                                  && !key.modifiers.contains(KeyModifiers::ALT))
                                    }
                                    KeyCode::Enter | KeyCode::Tab => true, // buffered when pend non-empty
                                    _ => false,
                                };
                                if !is_bufferable {
                                    flush_paste_pend_as_text(&mut paste_pend, &mut paste_pend_start, &mut paste_stage2, &mut cmd_batch);
                                }
                            }
                        }
                        // Dynamic prefix key check (default: Ctrl+B, configurable via .psmux.conf)
                        let is_prefix = (key.code, key.modifiers) == prefix_key
                            || prefix_raw_char.map_or(false, |c| matches!(key.code, KeyCode::Char(ch) if ch == c))
                            || prefix2_key.map_or(false, |p2| (key.code, key.modifiers) == p2)
                            || prefix2_raw_char.map_or(false, |c| matches!(key.code, KeyCode::Char(ch) if ch == c));

                        // Expire repeat-mode prefix if repeat-time has elapsed.
                        // This ensures keys are forwarded to the PTY rather than
                        // being interpreted as prefix bindings (tmux parity).
                        if prefix_armed && prefix_repeating
                            && prefix_armed_at.elapsed().as_millis() >= repeat_time_ms as u128
                        {
                            prefix_armed = false;
                            prefix_repeating = false;
                            #[cfg(windows)]
                            if ime_was_open { crate::platform::ime_restore(); ime_was_open = false; }
                            cmd_batch.push("prefix-end\n".into());
                        }

                        // Overlay Esc must be checked BEFORE selection-Esc so that
                        // pressing Esc always closes the active overlay first.
                        // ── Server-side overlay key handling ─────────────────
                        // When a server overlay is active, intercept ALL keys and
                        // forward them to the server via overlay-specific commands.
                        if srv_popup_active {
                            if srv_popup_has_pty {
                                // PTY popup: forward all keys to server
                                match key.code {
                                    KeyCode::Esc => {
                                        // Forward Esc to the popup PTY, not to the overlay (#471).
                                        // A PTY popup runs a real interactive app (nvim, fzf, a
                                        // shell); Esc belongs to that app. tmux forwards it too and
                                        // only closes the popup when its child process exits (e.g.
                                        // `:q` in nvim), which close-on-exit already handles. The
                                        // previous overlay-close swallowed Esc and killed the popup.
                                        let encoded = crate::util::base64_encode("\x1b");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Char(c) => {
                                        let bytes = if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                                            vec![(c as u8) & 0x1F]
                                        } else {
                                            let mut buf = [0u8; 4];
                                            let s = c.encode_utf8(&mut buf);
                                            s.as_bytes().to_vec()
                                        };
                                        let encoded = crate::util::base64_encode(std::str::from_utf8(&bytes).unwrap_or(""));
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Enter => {
                                        let encoded = crate::util::base64_encode("\r");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Backspace => {
                                        let encoded = crate::util::base64_encode("\x7f");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Tab => {
                                        let encoded = crate::util::base64_encode("\t");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Up => {
                                        let encoded = crate::util::base64_encode("\x1b[A");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Down => {
                                        let encoded = crate::util::base64_encode("\x1b[B");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Right => {
                                        let encoded = crate::util::base64_encode("\x1b[C");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Left => {
                                        let encoded = crate::util::base64_encode("\x1b[D");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Home => {
                                        let encoded = crate::util::base64_encode("\x1b[H");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::End => {
                                        let encoded = crate::util::base64_encode("\x1b[F");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::PageUp => {
                                        let encoded = crate::util::base64_encode("\x1b[5~");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::PageDown => {
                                        let encoded = crate::util::base64_encode("\x1b[6~");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    KeyCode::Delete => {
                                        let encoded = crate::util::base64_encode("\x1b[3~");
                                        cmd_batch.push(format!("popup-input {}\n", encoded));
                                    }
                                    _ => {}
                                }
                            } else {
                                // Static (non-PTY) popup: handle scroll locally, q/Esc close
                                let total_lines = srv_popup_lines.len() as u16;
                                match key.code {
                                    KeyCode::Esc | KeyCode::Char('q') => {
                                        cmd_batch.push("overlay-close\n".into());
                                        srv_popup_scroll = 0;
                                    }
                                    KeyCode::Up | KeyCode::Char('k') => {
                                        srv_popup_scroll = srv_popup_scroll.saturating_sub(1);
                                    }
                                    KeyCode::Down | KeyCode::Char('j') => {
                                        if srv_popup_scroll < total_lines.saturating_sub(1) {
                                            srv_popup_scroll += 1;
                                        }
                                    }
                                    KeyCode::PageUp => {
                                        srv_popup_scroll = srv_popup_scroll.saturating_sub(10);
                                    }
                                    KeyCode::PageDown => {
                                        srv_popup_scroll = (srv_popup_scroll + 10).min(total_lines.saturating_sub(1));
                                    }
                                    KeyCode::Home | KeyCode::Char('g') => {
                                        srv_popup_scroll = 0;
                                    }
                                    KeyCode::End | KeyCode::Char('G') => {
                                        srv_popup_scroll = total_lines.saturating_sub(1);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        else if srv_confirm_active {
                            match key.code {
                                KeyCode::Char('y') | KeyCode::Char('Y') => {
                                    cmd_batch.push("confirm-respond y\n".into());
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                    cmd_batch.push("confirm-respond n\n".into());
                                }
                                _ => {} // Ignore other keys during confirm
                            }
                        }
                        else if srv_menu_active {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => { cmd_batch.push("menu-navigate -1\n".into()); }
                                KeyCode::Down | KeyCode::Char('j') => { cmd_batch.push("menu-navigate 1\n".into()); }
                                KeyCode::Enter => {
                                    cmd_batch.push(format!("menu-select {}\n", srv_menu_selected));
                                }
                                KeyCode::Esc | KeyCode::Char('q') => { cmd_batch.push("overlay-close\n".into()); }
                                KeyCode::Char(c) => {
                                    // Shortcut key: find menu item with matching key
                                    if let Some(idx) = srv_menu_items.iter().position(|item| {
                                        item.key.as_ref().map(|k| k.len() == 1 && k.chars().next() == Some(c)).unwrap_or(false)
                                    }) {
                                        cmd_batch.push(format!("menu-select {}\n", idx));
                                    }
                                }
                                _ => {}
                            }
                        }
                        else if srv_display_panes {
                            match key.code {
                                KeyCode::Char(d) if d.is_ascii_digit() => {
                                    let digit = d.to_digit(10).unwrap() as usize;
                                    cmd_batch.push(format!("display-panes-select {}\n", digit));
                                }
                                _ => { cmd_batch.push("overlay-close\n".into()); }
                            }
                        }
                        else if srv_customize_active {
                            if srv_customize_editing {
                                match key.code {
                                    KeyCode::Esc => { cmd_batch.push("customize-edit-cancel\n".into()); }
                                    KeyCode::Enter => { cmd_batch.push("customize-edit-confirm\n".into()); }
                                    KeyCode::Backspace => {
                                        if srv_customize_cursor > 0 {
                                            let mut buf = srv_customize_edit_buf.clone();
                                            buf.remove(srv_customize_cursor - 1);
                                            cmd_batch.push(format!("customize-edit-update {}\n", buf));
                                        }
                                    }
                                    KeyCode::Char(c) => {
                                        let mut buf = srv_customize_edit_buf.clone();
                                        buf.insert(srv_customize_cursor, c);
                                        cmd_batch.push(format!("customize-edit-update {}\n", buf));
                                    }
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc | KeyCode::Char('q') => {
                                        customize_num_buffer.clear();
                                        cmd_batch.push("overlay-close\n".into());
                                    }
                                    KeyCode::Up | KeyCode::Char('k') => { cmd_batch.push("customize-navigate -1\n".into()); }
                                    KeyCode::Down | KeyCode::Char('j') => { cmd_batch.push("customize-navigate 1\n".into()); }
                                    // hjkl parity with tmux mode-tree (issue #259): h = up, l = down for flat lists
                                    KeyCode::Char('h') => { cmd_batch.push("customize-navigate -1\n".into()); }
                                    KeyCode::Char('l') => { cmd_batch.push("customize-navigate 1\n".into()); }
                                    KeyCode::PageUp => { cmd_batch.push("customize-navigate -20\n".into()); }
                                    KeyCode::PageDown => { cmd_batch.push("customize-navigate 20\n".into()); }
                                    KeyCode::Home | KeyCode::Char('g') => { cmd_batch.push("customize-navigate -9999\n".into()); }
                                    KeyCode::End | KeyCode::Char('G') => { cmd_batch.push("customize-navigate 9999\n".into()); }
                                    KeyCode::Backspace => { customize_num_buffer.pop(); }
                                    KeyCode::Enter => {
                                        // Digit-jump: number+Enter navigates to the Nth visible
                                        // option (1-based). Empty buffer falls back to the
                                        // existing edit-on-Enter behavior.
                                        if customize_num_buffer.is_empty() {
                                            cmd_batch.push("customize-edit\n".into());
                                        } else {
                                            let want_pos = customize_num_buffer.parse::<usize>().ok()
                                                .filter(|n| *n >= 1 && *n <= srv_customize_options.len());
                                            if let Some(pos) = want_pos {
                                                // current_pos = index of the highlighted opt within the
                                                // visible (filtered) option list.
                                                let cur_pos = srv_customize_options.iter()
                                                    .position(|o| o.i == srv_customize_selected)
                                                    .unwrap_or(0);
                                                let target_pos = pos - 1;
                                                let delta = target_pos as i64 - cur_pos as i64;
                                                if delta != 0 {
                                                    cmd_batch.push(format!("customize-navigate {}\n", delta));
                                                }
                                                customize_num_buffer.clear();
                                            }
                                            // unparseable / out-of-range -> keep buffer
                                        }
                                    }
                                    KeyCode::Char('d') => { cmd_batch.push("customize-reset-default\n".into()); }
                                    KeyCode::Char('/') => {
                                        // Toggle filter: if filter active, clear it
                                        if !srv_customize_filter.is_empty() {
                                            cmd_batch.push("customize-filter \n".into());
                                        }
                                        // For entering a new filter, we would need a mini prompt.
                                        // For now, users type filter text via subsequent keystrokes.
                                    }
                                    KeyCode::Char(c) if c.is_ascii_digit() => {
                                        if customize_num_buffer.len() < 6 {
                                            customize_num_buffer.push(c);
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        else if matches!(key.code, KeyCode::Esc) && (command_input || renaming || pane_renaming || tree_chooser || buffer_chooser || session_chooser || confirm_cmd.is_some() || keys_viewer) {
                            if session_chooser && !session_filter.is_empty() {
                                let selected_entry = session_filter_escape_selection(
                                    &session_entries,
                                    &session_filter,
                                    session_selected,
                                );
                                session_filter.clear();
                                session_filter_active = false;
                                session_selected = selected_entry.unwrap_or(0);
                                session_scroll = 0;
                                session_num_buffer.clear();
                            } else {
                                command_input = false;
                                command_cursor = 0;
                                renaming = false;
                                pane_renaming = false;
                                tree_chooser = false;
                                buffer_chooser = false;
                                session_chooser = false;
                                session_filter_active = false;
                                session_filter.clear();
                                keys_viewer = false;
                                confirm_cmd = None;
                                // Drop any pending digit-jump buffers when the
                                // pickers are dismissed via Esc.
                                tree_num_buffer.clear();
                                buffer_num_buffer.clear();
                                session_num_buffer.clear();
                                // Also clear any lingering selection
                                rsel_start = None;
                                rsel_end = None;
                                selection_changed = true;
                            }
                        }
                        else if rsel_start.is_some() && matches!(key.code, KeyCode::Esc) {
                            // Escape clears any active text selection
                            rsel_start = None;
                            rsel_end = None;
                            selection_changed = true;
                        }
                        else if is_prefix && !prefix_armed {
                            // Suppress IME while in prefix mode so command keys
                            // are not intercepted by the input method (issue #286).
                            #[cfg(windows)]
                            { ime_was_open = crate::platform::ime_disable(); }
                            prefix_armed = true; prefix_armed_at = Instant::now(); prefix_repeating = false; cmd_batch.push("prefix-begin\n".into());
                        }
                        // NOTE: when the prefix key is pressed while the prefix is
                        // ALREADY armed we deliberately do NOT re-arm here. Falling
                        // through lets the prefix-table dispatch below handle it, so
                        // `bind -T prefix C-b send-prefix` fires and a literal prefix
                        // byte is forwarded to the active pane. This is what lets a
                        // nested multiplexer (e.g. tmux over SSH) receive its own
                        // prefix via prefix+prefix (discussion #310).
                        // Check root-table bindings (bind-key -n / bind-key -T root)
                        // These fire without prefix, before keys are forwarded to PTY.
                        // Skip them entirely when the prefix is armed so the prefix
                        // table takes exclusive priority, matching tmux: once the prefix
                        // is pressed a key bound in both tables must fire its prefix
                        // binding, not its root binding (issue #472).
                        else if !prefix_armed && !command_input && !renaming && !pane_renaming && !tree_chooser && !buffer_chooser && !session_chooser && !keys_viewer && confirm_cmd.is_none() && {
                            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
                            synced_bindings.iter().any(|b| {
                                b.t == "root" && parse_key_string(&b.k).map_or(false, |k| normalize_key_for_binding(k) == key_tuple)
                                // Skip scroll-triggered copy mode bindings when option is off (#284)
                                && !(b.c.starts_with("copy-mode") && b.c.contains("-u") && !scroll_enter_copy_mode)
                            })
                        } {
                            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
                            if let Some(entry) = synced_bindings.iter().find(|b| {
                                b.t == "root" && parse_key_string(&b.k).map_or(false, |k| normalize_key_for_binding(k) == key_tuple)
                                && !(b.c.starts_with("copy-mode") && b.c.contains("-u") && !scroll_enter_copy_mode)
                            }) {
                                if entry.c == "detach-client" || entry.c == "detach" {
                                    quit = true;
                                } else {
                                    // Split on \; to support command chaining (issue #192)
                                    let sub_cmds = crate::config::split_chained_commands_pub(&entry.c);
                                    for sub in &sub_cmds {
                                        cmd_batch.push(format!("{}\n", sub));
                                    }
                                }
                            }
                        }
                        else if prefix_armed {
                            // Pending flags for complex client-side UI commands
                            // (shared between synced_bindings dispatch and pre-sync hardcoded fallback)
                            let mut do_choose_tree = false;
                            let mut do_choose_session = false;
                            let mut do_choose_buffer = false;
                            let mut do_session_nav: Option<bool> = None; // Some(true)=next, Some(false)=prev

                            // Check synced bindings from server (includes defaults from PREFIX_DEFAULTS)
                            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
                            let user_binding = synced_bindings.iter().find(|b| {
                                b.t == "prefix" && parse_key_string(&b.k).map_or(false, |k| normalize_key_for_binding(k) == key_tuple)
                            });
                            if let Some(entry) = user_binding {
                                // Dispatch binding (handles both defaults and user overrides).
                                // Client-side UI commands need special handling here since
                                // they set local overlay state rather than sending to server.
                                let cmd = &entry.c;
                                if cmd == "detach-client" || cmd == "detach" {
                                    quit = true;
                                } else if cmd.starts_with("confirm-before") {
                                    // Extract the actual command from confirm-before wrapper
                                    let inner = cmd.strip_prefix("confirm-before").unwrap_or(cmd).trim();
                                    // Skip -p 'prompt' flags to get the actual command
                                    let actual_cmd = extract_confirm_command(inner);
                                    confirm_cmd = Some(actual_cmd);
                                } else if cmd == "kill-pane" || cmd == "kill-window" || cmd == "kill-session" {
                                    // Direct kill without confirmation (user explicitly bound without confirm-before)
                                    cmd_batch.push(format!("{}\n", cmd));
                                } else if cmd == "rename-window" {
                                    renaming = true; rename_buf.clear();
                                } else if cmd == "rename-session" {
                                    renaming = true; rename_buf.clear(); session_renaming = true;
                                } else if cmd == "command-prompt" || cmd.starts_with("command-prompt ") {
                                    command_input = true;
                                    command_cursor = 0;
                                    command_history_idx = command_history.len();
                                    command_template = None;
                                    command_prompt_label = None;
                                    if cmd.starts_with("command-prompt ") {
                                        // Parse -I initial_text, -p prompt, and template argument
                                        let cp_args = &cmd["command-prompt ".len()..];
                                        let tokens = crate::config::shell_words(cp_args);
                                        let mut initial = String::new();
                                        let mut prompt_text: Option<String> = None;
                                        let mut positional: Vec<String> = Vec::new();
                                        let mut i = 0;
                                        while i < tokens.len() {
                                            if tokens[i] == "-I" && i + 1 < tokens.len() {
                                                initial = tokens[i + 1].clone();
                                                i += 2;
                                            } else if tokens[i] == "-p" && i + 1 < tokens.len() {
                                                prompt_text = Some(tokens[i + 1].clone());
                                                i += 2;
                                            } else if tokens[i] == "-1" || tokens[i] == "-N" || tokens[i] == "-W" {
                                                i += 1; // skip flags
                                            } else if tokens[i].starts_with('-') {
                                                i += 1; // skip unknown flags
                                            } else {
                                                positional.push(tokens[i].clone());
                                                i += 1;
                                            }
                                        }
                                        // Expand format variables in initial text
                                        let initial = initial
                                            .replace("#W", &active_window_name)
                                            .replace("#{window_name}", &active_window_name)
                                            .replace("#S", &current_session)
                                            .replace("#{session_name}", &current_session);
                                        command_buf = initial.clone();
                                        command_cursor = command_buf.len();
                                        // Join all positional args to form the template
                                        // e.g. 'rename-window "%%"' → single arg, or
                                        //       rename-window %%    → two args joined
                                        if !positional.is_empty() {
                                            command_template = Some(positional.join(" "));
                                        }
                                        command_prompt_label = prompt_text;
                                    } else {
                                        command_buf.clear();
                                    }
                                } else if cmd == "list-keys" {
                                    keys_viewer_scroll = 0;
                                    let user_binds: Vec<(bool, String, String, String)> = synced_bindings
                                        .iter()
                                        .map(|b| (b.r, b.t.clone(), b.k.clone(), b.c.clone()))
                                        .collect();
                                    keys_viewer_lines = help::build_overlay_lines(&user_binds, defaults_suppressed);
                                    keys_viewer = true;
                                } else if cmd == "select-window-index" {
                                    window_idx_input = true; window_idx_buf.clear();
                                } else if cmd == "choose-tree" || cmd == "choose-window" {
                                    do_choose_tree = true;
                                } else if cmd == "choose-buffer" || cmd == "chooseb" {
                                    do_choose_buffer = true;
                                } else if cmd == "choose-session" {
                                    do_choose_session = true;
                                } else if cmd.starts_with("switch-client") {
                                    do_session_nav = Some(cmd.contains("-n"));
                                } else {
                                    // Generic: split on \; for command chaining (issue #192)
                                    let sub_cmds = crate::config::split_chained_commands_pub(&entry.c);
                                    for sub in &sub_cmds {
                                        cmd_batch.push(format!("{}\n", sub));
                                    }
                                }
                            } else if synced_bindings.is_empty() {
                            // Pre-sync hardcoded fallback (only used before first server state sync)
                            match key.code {
                                KeyCode::Char('c') => { cmd_batch.push("new-window\n".into()); }
                                KeyCode::Char('%') => { cmd_batch.push("split-window -h\n".into()); }
                                KeyCode::Char('"') => { cmd_batch.push("split-window -v\n".into()); }
                                KeyCode::Char('x') => { confirm_cmd = Some("kill-pane".into()); }
                                KeyCode::Char('&') => { confirm_cmd = Some("kill-window".into()); }
                                KeyCode::Char('z') => { cmd_batch.push("zoom-pane\n".into()); }
                                KeyCode::Char('[') => { cmd_batch.push("copy-enter\n".into()); }
                                KeyCode::Char(']') => { cmd_batch.push("paste-buffer\n".into()); }
                                KeyCode::Char('{') => { cmd_batch.push("swap-pane -U\n".into()); }
                                KeyCode::Char('}') => { cmd_batch.push("swap-pane -D\n".into()); }
                                KeyCode::Char('n') => { cmd_batch.push("next-window\n".into()); }
                                KeyCode::Char('p') => { cmd_batch.push("previous-window\n".into()); }
                                KeyCode::Char('l') => { cmd_batch.push("last-window\n".into()); }
                                KeyCode::Char(';') => { cmd_batch.push("last-pane\n".into()); }
                                KeyCode::Char(' ') => { cmd_batch.push("next-layout\n".into()); }
                                KeyCode::Char('!') => { cmd_batch.push("break-pane\n".into()); }
                                KeyCode::Char(d) if d.is_ascii_digit() => {
                                    let idx = d.to_digit(10).unwrap() as usize;
                                    cmd_batch.push(format!("select-window {}\n", idx));
                                }
                                KeyCode::Char('o') => { cmd_batch.push("select-pane -t :.+\n".into()); }
                                // Alt+Arrow: resize pane by 5 (must be before plain Arrow)
                                KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("resize-pane -U 5\n".into()); }
                                KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("resize-pane -D 5\n".into()); }
                                KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("resize-pane -L 5\n".into()); }
                                KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("resize-pane -R 5\n".into()); }
                                // Ctrl+Arrow: resize pane by 1
                                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => { cmd_batch.push("resize-pane -U 1\n".into()); }
                                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => { cmd_batch.push("resize-pane -D 1\n".into()); }
                                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => { cmd_batch.push("resize-pane -L 1\n".into()); }
                                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => { cmd_batch.push("resize-pane -R 1\n".into()); }
                                // Plain Arrow: select pane
                                KeyCode::Up => { cmd_batch.push("select-pane -U\n".into()); }
                                KeyCode::Down => { cmd_batch.push("select-pane -D\n".into()); }
                                KeyCode::Left => { cmd_batch.push("select-pane -L\n".into()); }
                                KeyCode::Right => { cmd_batch.push("select-pane -R\n".into()); }
                                KeyCode::Char('d') => { quit = true; }
                                KeyCode::Char(',') => { renaming = true; rename_buf.clear(); }
                                KeyCode::Char('$') => {
                                    // Rename session — reuse rename overlay
                                    renaming = true;
                                    rename_buf.clear();
                                    // Mark that we're renaming the session, not a window
                                    // We'll detect this by checking if pane_renaming is used as a flag
                                    session_renaming = true;
                                }
                                KeyCode::Char('?') => {
                                    // Build comprehensive help overlay from help.rs
                                    keys_viewer_scroll = 0;
                                    let user_binds: Vec<(bool, String, String, String)> = synced_bindings
                                        .iter()
                                        .map(|b| (b.r, b.t.clone(), b.k.clone(), b.c.clone()))
                                        .collect();
                                    keys_viewer_lines = help::build_overlay_lines(&user_binds, defaults_suppressed);
                                    keys_viewer = true;
                                }
                                KeyCode::Char('t') => { cmd_batch.push("clock-mode\n".into()); }
                                KeyCode::Char('=') => { do_choose_buffer = true; }
                                KeyCode::Char('#') => { cmd_batch.push("list-buffers\n".into()); }
                                KeyCode::Char(':') => { command_input = true; command_buf.clear(); command_cursor = 0; command_history_idx = command_history.len(); }
                                KeyCode::Char('\'') => { window_idx_input = true; window_idx_buf.clear(); }
                                KeyCode::Char('w') => { do_choose_tree = true; }
                                KeyCode::Char('s') => { do_choose_session = true; }
                                KeyCode::Char('q') => { cmd_batch.push("display-panes\n".into()); }
                                KeyCode::Char('v') => { cmd_batch.push("rectangle-toggle\n".into()); }
                                KeyCode::Char('y') => { cmd_batch.push("copy-yank\n".into()); }
                                // Session navigation (like tmux prefix+( and prefix+))
                                KeyCode::Char('(') | KeyCode::Char(')') => {
                                    do_session_nav = Some(key.code == KeyCode::Char(')'));
                                }
                                // Meta+1..5 preset layouts (like tmux)
                                KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("select-layout even-horizontal\n".into()); }
                                KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("select-layout even-vertical\n".into()); }
                                KeyCode::Char('3') if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("select-layout main-horizontal\n".into()); }
                                KeyCode::Char('4') if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("select-layout main-vertical\n".into()); }
                                KeyCode::Char('5') if key.modifiers.contains(KeyModifiers::ALT) => { cmd_batch.push("select-layout tiled\n".into()); }
                                // Display pane info
                                KeyCode::Char('i') => { cmd_batch.push("display-message\n".into()); }
                                _ => {
                                    // No default binding for this key (user bindings already checked above)
                                }
                            }
                            } // end of else (no user binding override)

                            // Dispatch pending flags for complex client-side UI commands.
                            // These are shared between synced_bindings dispatch and pre-sync fallback.
                            if do_choose_tree {
                                tree_chooser = true;
                                tree_entries.clear();
                                tree_selected = 0;
                                tree_scroll = 0;
                                tree_num_buffer.clear();
                                popup_offset = (0, 0);
                                popup_dragging = false;
                                popup_rect_last = None;
                                if choose_tree_preview_default { preview_enabled = true; }
                                // Query ALL sessions (like tmux choose-tree)
                                let dir = crate::paths::psmux_dir();
                                if let Ok(entries) = std::fs::read_dir(&dir) {
                                    let mut sessions: Vec<(String, Vec<(usize, String, bool, Vec<(usize, String)>, usize)>)> = Vec::new();
                                    for e in entries.flatten() {
                                        if let Some(fname) = e.file_name().to_str().map(|s| s.to_string()) {
                                            if let Some((base, ext)) = fname.rsplit_once('.') {
                                                if ext == "port" {
                                                    if crate::session::is_warm_session(base) { continue; }
                                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                                        if let Ok(p) = port_str.trim().parse::<u16>() {
                                                            let sess_addr = format!("127.0.0.1:{}", p);
                                                            let sess_key = read_session_key(base).unwrap_or_default();
                                                            // Centralized AUTH+command helper handles the OK ack race
                                                            // (issue #250) and bounds the response size.
                                                            if let Some(tree_line) = crate::session::fetch_authed_response_multi(
                                                                &sess_addr,
                                                                &sess_key,
                                                                b"list-tree\n",
                                                                Duration::from_millis(50),
                                                                Duration::from_millis(100),
                                                            ) {
                                                                if let Ok(wins) = serde_json::from_str::<Vec<WinTree>>(tree_line.trim()) {
                                                                    let mut win_data = Vec::new();
                                                                    for w in &wins {
                                                                        let panes: Vec<(usize, String)> = w.panes.iter().map(|p| (p.id, p.title.clone())).collect();
                                                                        win_data.push((w.id, w.name.clone(), w.active, panes, w.idx));
                                                                    }
                                                                    sessions.push((base.to_string(), win_data));
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    sessions.sort_by(|a, b| {
                                        if a.0 == current_session { std::cmp::Ordering::Less }
                                        else if b.0 == current_session { std::cmp::Ordering::Greater }
                                        else { a.0.cmp(&b.0) }
                                    });
                                    // Row index of the current session's active window, so the
                                    // chooser opens its cursor on it (issue #373) instead of the
                                    // hardcoded top row. The active window is also marked with `*`,
                                    // glyph-adjacent, matching list_windows_tmux and real tmux.
                                    let mut active_tree_row: Option<usize> = None;
                                    for (sess_name, wins) in &sessions {
                                        let is_current = sess_name == &current_session;
                                        let attached = if is_current { " (attached)" } else { "" };
                                        let nw = wins.len();
                                        tree_entries.push((true, usize::MAX, 0,
                                            format!("{}: {} windows{}", sess_name, nw, attached),
                                            sess_name.clone()));
                                        if is_current {
                                            for (wid, wname, active, panes, disp) in wins.iter() {
                                                let marker = if *active { "*" } else { "" };
                                                if *active { active_tree_row = Some(tree_entries.len()); }
                                                tree_entries.push((true, *wid, 0,
                                                    format!("  {}: {}{} ({} panes)", disp, wname, marker, panes.len()),
                                                    sess_name.clone()));
                                                for (pid, ptitle) in panes {
                                                    tree_entries.push((false, *wid, *pid,
                                                        format!("    {}", ptitle),
                                                        sess_name.clone()));
                                                }
                                            }
                                        } else {
                                            for (wid, wname, active, panes, disp) in wins.iter() {
                                                let marker = if *active { "*" } else { "" };
                                                tree_entries.push((true, *wid, 0,
                                                    format!("  {}: {}{} ({} panes)", disp, wname, marker, panes.len()),
                                                    sess_name.clone()));
                                            }
                                        }
                                    }
                                    if let Some(r) = active_tree_row { tree_selected = r; }
                                }
                                if tree_entries.is_empty() {
                                    for wi in &last_tree {
                                        let marker = if wi.active { "*" } else { "" };
                                        if wi.active { tree_selected = tree_entries.len(); }
                                        tree_entries.push((true, wi.id, 0, format!("{}{}", wi.name, marker), current_session.clone()));
                                        for pi in &wi.panes {
                                            tree_entries.push((false, wi.id, pi.id, pi.title.clone(), current_session.clone()));
                                        }
                                    }
                                }
                            }
                            if do_choose_session {
                                session_chooser = true;
                                session_entries.clear();
                                session_selected = 0;
                                session_scroll = 0;
                                session_num_buffer.clear();
                                session_filter_active = false;
                                session_filter.clear();
                                popup_offset = (0, 0);
                                popup_dragging = false;
                                popup_rect_last = None;
                                if choose_tree_preview_default { preview_enabled = true; }
                                let dir = crate::paths::psmux_dir();
                                // Collect (label, addr, key) for every port file, then run ONE
                                // bounded liveness probe per session in parallel. This both lists
                                // and prunes: total wall time is ~one probe window regardless of
                                // how many sessions exist, so the picker stays responsive (no
                                // sequential O(N * timeout) cleanup pass).
                                let mut targets: Vec<(String, String, String)> = Vec::new();
                                if let Ok(entries) = std::fs::read_dir(&dir) {
                                    for e in entries.flatten() {
                                        if let Some(fname) = e.file_name().to_str() {
                                            if let Some((base, ext)) = fname.rsplit_once('.') {
                                                if ext == "port" {
                                                    if crate::session::is_warm_session(base) { continue; }
                                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                                        if let Ok(p) = port_str.trim().parse::<u16>() {
                                                            let sess_addr = format!("127.0.0.1:{}", p);
                                                            let sess_key = read_session_key(base).unwrap_or_default();
                                                            targets.push((base.to_string(), sess_addr, sess_key));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                // Issue #259 (batch D): sort by label so the picker's order is
                                // deterministic, matching the tree_chooser's established
                                // convention (session.rs::list_all_sessions_tree sorts by name)
                                // and the g/G Home/End contract. Without this, order followed
                                // raw directory-enumeration order, which the Win32 API
                                // explicitly does not guarantee to be stable or alphabetical.
                                targets.sort_by(|a, b| a.0.cmp(&b.0));
                                let verdicts = crate::session::classify_sessions_parallel(
                                    targets,
                                    Duration::from_millis(50),
                                    Duration::from_millis(250),
                                );
                                for (label, liveness) in verdicts {
                                    match liveness {
                                        crate::session::SessionLiveness::Alive(info) => {
                                            session_entries.push((label, info));
                                        }
                                        crate::session::SessionLiveness::Dead => {
                                            // Server gone (crashed, killed by a reboot, or its
                                            // port reused by another server). Reap it so it never
                                            // shows as a "(not responding)" zombie. A live-but-slow
                                            // server self-heals on its next 5s registry tick.
                                            if crate::debug_log::session_log_enabled() {
                                                crate::debug_log::session_log("picker",
                                                    &format!("reaping dead session '{}' from chooser", label));
                                            }
                                            crate::session::remove_session_registry(&label);
                                        }
                                        crate::session::SessionLiveness::Unreachable => {
                                            // Could not connect (transient, non-refused). Keep it
                                            // and show the honest "(not responding)" rather than
                                            // deleting on an ambiguous failure.
                                            session_entries.push((label.clone(), format!("{}: (not responding)", label)));
                                        }
                                    }
                                }
                                if session_entries.is_empty() {
                                    session_entries.push((current_session.clone(), format!("{}: (current)", current_session)));
                                }
                                for (i, (sname, _)) in session_entries.iter().enumerate() {
                                    if sname == &current_session { session_selected = i; break; }
                                }
                            }
                            if do_choose_buffer {
                                buffer_chooser = true;
                                buffer_entries.clear();
                                buffer_selected = 0;
                                buffer_scroll = 0;
                                buffer_num_buffer.clear();
                                // Fetch buffer list from server via TCP
                                let port_file = crate::paths::port_file(&current_session);
                                if let Ok(port_str) = std::fs::read_to_string(&port_file) {
                                    if let Ok(p) = port_str.trim().parse::<u16>() {
                                        let sess_key = read_session_key(&current_session).unwrap_or_default();
                                        let addr = format!("127.0.0.1:{}", p);
                                        if let Some(buf_line) = crate::session::fetch_authed_response_multi(
                                            &addr,
                                            &sess_key,
                                            b"choose-buffer\n",
                                            Duration::from_millis(100),
                                            Duration::from_millis(200),
                                        ) {
                                            {
                                                // Parse "buffer0: 17 bytes: "content"\nbuffer1: ..."
                                                for line in buf_line.trim().split('\n') {
                                                    let line = line.trim();
                                                    if line.is_empty() { continue; }
                                                    // Format: "bufferN: M bytes: "preview""
                                                    if let Some(rest) = line.strip_prefix("buffer") {
                                                        if let Some(colon_pos) = rest.find(':') {
                                                            if let Ok(idx) = rest[..colon_pos].parse::<usize>() {
                                                                let after_colon = &rest[colon_pos+1..].trim_start();
                                                                let byte_len = after_colon.split_whitespace().next()
                                                                    .and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                                                                // Extract preview (after "bytes: ")
                                                                let preview = if let Some(bp) = after_colon.find('"') {
                                                                    let p = &after_colon[bp+1..];
                                                                    p.trim_end_matches('"').to_string()
                                                                } else {
                                                                    after_colon.to_string()
                                                                };
                                                                buffer_entries.push((idx, byte_len, preview));
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                if buffer_entries.is_empty() {
                                    // No buffers — don't show chooser
                                    buffer_chooser = false;
                                }
                            }
                            if let Some(dir_next) = do_session_nav {
                                let dir = crate::paths::psmux_dir();
                                let mut names: Vec<String> = Vec::new();
                                if let Ok(entries) = std::fs::read_dir(&dir) {
                                    for e in entries.flatten() {
                                        if let Some(fname) = e.file_name().to_str() {
                                            if let Some((base, ext)) = fname.rsplit_once('.') {
                                                if ext == "port" {
                                                    if crate::session::is_warm_session(base) { continue; }
                                                    if let Ok(ps) = std::fs::read_to_string(e.path()) {
                                                        if let Ok(p) = ps.trim().parse::<u16>() {
                                                            let a = format!("127.0.0.1:{}", p);
                                                            if std::net::TcpStream::connect_timeout(
                                                                &a.parse().unwrap(), Duration::from_millis(25)
                                                            ).is_ok() {
                                                                names.push(base.to_string());
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                names.sort();
                                if names.len() > 1 {
                                    if let Some(cur_pos) = names.iter().position(|n| *n == current_session) {
                                        let next_pos = if dir_next {
                                            (cur_pos + 1) % names.len()
                                        } else {
                                            (cur_pos + names.len() - 1) % names.len()
                                        };
                                        let next_name = names[next_pos].clone();
                                        cmd_batch.push("client-detach\n".into());
                                        env::set_var("PSMUX_SWITCH_TO", &next_name);
                                        quit = true;
                                    }
                                }
                            }

                            // Arrow keys are repeatable by default (tmux -r flag).
                            // User-defined bindings also respect the repeat flag.
                            let is_repeatable_default = matches!(key.code,
                                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                            );
                            let is_user_repeat = user_binding.map_or(false, |e| e.r);
                            if is_repeatable_default || is_user_repeat {
                                prefix_armed_at = Instant::now();
                                prefix_repeating = true;
                            } else {
                                prefix_armed = false;
                                prefix_repeating = false;
                                #[cfg(windows)]
                                if ime_was_open { crate::platform::ime_restore(); ime_was_open = false; }
                                cmd_batch.push("prefix-end\n".into());
                            }
                        } else {
                            // True briefly after an Event::Paste consumed by an
                            // overlay (issue #290).  On Windows, crossterm emits
                            // Event::Paste AND per-char Event::Key for one
                            // Ctrl+V — this gates the duplicate Char inserts
                            // into the overlay buffers below.
                            #[cfg(windows)]
                            let paste_burst_active =
                                paste_suppress_until.map_or(false, |t| Instant::now() < t);
                            #[cfg(not(windows))]
                            let paste_burst_active = false;
                            match key.code {
                                KeyCode::Up if session_chooser => { if session_selected > 0 { session_selected -= 1; } }
                                KeyCode::Down if session_chooser => {
                                    let visible_len = session_filtered_indices(&session_entries, &session_filter).len();
                                    if session_selected + 1 < visible_len { session_selected += 1; }
                                }
                                KeyCode::Backspace if session_chooser && session_filter_active => {
                                    session_filter.pop();
                                    session_selected = 0;
                                    session_scroll = 0;
                                    session_num_buffer.clear();
                                }
                                KeyCode::Char(c) if session_chooser && session_filter_active => {
                                    if session_filter.len() < 256 {
                                        session_filter.push(c);
                                        session_selected = 0;
                                        session_scroll = 0;
                                        session_num_buffer.clear();
                                    }
                                }
                                // hjkl parity with tmux mode-tree (issue #259): for flat lists
                                // tmux treats h/k as up and j/l as down. g/G map to Home/End.
                                KeyCode::Char('k') if session_chooser => { if session_selected > 0 { session_selected -= 1; } }
                                KeyCode::Char('j') if session_chooser => {
                                    let visible_len = session_filtered_indices(&session_entries, &session_filter).len();
                                    if session_selected + 1 < visible_len { session_selected += 1; }
                                }
                                KeyCode::Char('h') if session_chooser => { if session_selected > 0 { session_selected -= 1; } }
                                KeyCode::Char('l') if session_chooser => {
                                    let visible_len = session_filtered_indices(&session_entries, &session_filter).len();
                                    if session_selected + 1 < visible_len { session_selected += 1; }
                                }
                                KeyCode::Char('g') if session_chooser => { session_selected = 0; }
                                KeyCode::Char('G') if session_chooser => {
                                    session_selected = session_filtered_indices(&session_entries, &session_filter).len().saturating_sub(1);
                                }
                                KeyCode::PageUp if session_chooser => { session_selected = session_selected.saturating_sub(10); }
                                KeyCode::PageDown if session_chooser => {
                                    let last = session_filtered_indices(&session_entries, &session_filter).len().saturating_sub(1);
                                    session_selected = (session_selected + 10).min(last);
                                }
                                KeyCode::Home if session_chooser => { session_selected = 0; }
                                KeyCode::End if session_chooser => {
                                    session_selected = session_filtered_indices(&session_entries, &session_filter).len().saturating_sub(1);
                                }
                                KeyCode::Enter if session_chooser => {
                                    let filtered_indices = session_filtered_indices(&session_entries, &session_filter);
                                    // If the user has typed a number, that wins over the arrow cursor.
                                    // Buffer is 1-based: "1" → first entry, "12" → twelfth. Out-of-range
                                    // or unparseable → do nothing (keep buffer so user can Backspace).
                                    let target_idx: Option<usize> = if session_num_buffer.is_empty() {
                                        Some(session_selected)
                                    } else {
                                        match session_num_buffer.parse::<usize>() {
                                            Ok(n) if n >= 1 && n <= filtered_indices.len() => Some(n - 1),
                                            _ => None,
                                        }
                                    };
                                    if let Some(idx) = target_idx.and_then(|i| filtered_indices.get(i).copied()) {
                                        if let Some((sname, _)) = session_entries.get(idx) {
                                            if sname != &current_session {
                                                cmd_batch.push("client-detach\n".into());
                                                env::set_var("PSMUX_SWITCH_TO", sname);
                                                quit = true;
                                            }
                                            session_chooser = false;
                                            session_num_buffer.clear();
                                            session_filter_active = false;
                                            session_filter.clear();
                                        }
                                    }
                                }
                                KeyCode::Backspace if session_chooser => {
                                    session_num_buffer.pop();
                                }
                                KeyCode::Char('x') if session_chooser => {
                                    // Kill the selected session (like tmux session chooser)
                                    let selected_entry = session_filtered_indices(&session_entries, &session_filter)
                                        .get(session_selected)
                                        .copied();
                                    if let Some((sname, _)) = selected_entry.and_then(|i| session_entries.get(i)) {
                                        let sname = sname.clone();
                                        if sname == current_session {
                                            // Killing current session — exit after kill
                                            cmd_batch.push("kill-session\n".into());
                                            session_chooser = false;
                                            quit = true;
                                        } else {
                                            // Kill another session by connecting to it
                                            let port_path = crate::paths::port_file(&sname);
                                            let key_path = crate::paths::key_file(&sname);
                                            if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                                                if let Ok(port) = port_str.trim().parse::<u16>() {
                                                    let addr = format!("127.0.0.1:{}", port);
                                                    let sess_key = std::fs::read_to_string(&key_path).unwrap_or_default();
                                                    if let Ok(mut ss) = std::net::TcpStream::connect_timeout(
                                                        &addr.parse().unwrap(), Duration::from_millis(100)
                                                    ) {
                                                        let _ = write!(ss, "AUTH {}\n", sess_key.trim());
                                                        let _ = ss.write_all(b"kill-session\n");
                                                    }
                                                }
                                            }
                                            // Remove the killed session from the list
                                            if let Some(idx) = selected_entry {
                                                session_entries.remove(idx);
                                            }
                                            let visible_len = session_filtered_indices(&session_entries, &session_filter).len();
                                            if session_selected >= visible_len && session_selected > 0 {
                                                session_selected -= 1;
                                            }
                                            if session_entries.is_empty() {
                                                session_chooser = false;
                                            }
                                            // Indexes shifted; drop any pending jump buffer.
                                            session_num_buffer.clear();
                                        }
                                    }
                                }
                                KeyCode::Char('f') if session_chooser => {
                                    session_filter_active = true;
                                    session_filter.clear();
                                    session_selected = 0;
                                    session_scroll = 0;
                                    session_num_buffer.clear();
                                }
                                KeyCode::Char(c) if session_chooser && c.is_ascii_digit() => {
                                    // Accumulate into the jump buffer — Enter consumes it.
                                    // Cap length so extremely long inputs can't grow unbounded.
                                    if session_num_buffer.len() < 6 {
                                        session_num_buffer.push(c);
                                    }
                                }
                                // 'p' toggles the live preview pane in choose-session
                                KeyCode::Char('p') if session_chooser => {
                                    preview_enabled = !preview_enabled;
                                }
                                // Absorb any other char while the session picker is open so
                                // it cannot leak through to the focused pane's PTY.
                                KeyCode::Char(_) if session_chooser => {}
                                KeyCode::Up if tree_chooser => { if tree_selected > 0 { tree_selected -= 1; } }
                                KeyCode::Down if tree_chooser => { if tree_selected + 1 < tree_entries.len() { tree_selected += 1; } }
                                // hjkl parity with tmux mode-tree (issue #259): h/k = up, j/l = down
                                // for flat lists. g/G map to Home/End.
                                KeyCode::Char('k') if tree_chooser => { if tree_selected > 0 { tree_selected -= 1; } }
                                KeyCode::Char('j') if tree_chooser => { if tree_selected + 1 < tree_entries.len() { tree_selected += 1; } }
                                KeyCode::Char('h') if tree_chooser => { if tree_selected > 0 { tree_selected -= 1; } }
                                KeyCode::Char('l') if tree_chooser => { if tree_selected + 1 < tree_entries.len() { tree_selected += 1; } }
                                KeyCode::Char('g') if tree_chooser => { tree_selected = 0; }
                                KeyCode::Char('G') if tree_chooser => { tree_selected = tree_entries.len().saturating_sub(1); }
                                KeyCode::PageUp if tree_chooser => { tree_selected = tree_selected.saturating_sub(10); }
                                KeyCode::PageDown if tree_chooser => { tree_selected = (tree_selected + 10).min(tree_entries.len().saturating_sub(1)); }
                                KeyCode::Home if tree_chooser => { tree_selected = 0; }
                                KeyCode::End if tree_chooser => { tree_selected = tree_entries.len().saturating_sub(1); }
                                KeyCode::Enter if tree_chooser => {
                                    // Digit-jump: if a number was typed, prefer it over the
                                    // arrow cursor. Buffer is 1-based: "1" -> first row,
                                    // "12" -> twelfth. Out-of-range or unparseable -> no-op
                                    // (keep buffer so user can Backspace and fix).
                                    let target_idx: Option<usize> = if tree_num_buffer.is_empty() {
                                        Some(tree_selected)
                                    } else {
                                        match tree_num_buffer.parse::<usize>() {
                                            Ok(n) if n >= 1 && n <= tree_entries.len() => Some(n - 1),
                                            _ => None,
                                        }
                                    };
                                    if let Some(sel_idx) = target_idx {
                                        if let Some((is_win, wid, pid, _label, sess_name)) = tree_entries.get(sel_idx) {
                                            if *wid == usize::MAX {
                                                // Session header — switch to that session
                                                if *sess_name != current_session {
                                                    cmd_batch.push("client-detach\n".into());
                                                    env::set_var("PSMUX_SWITCH_TO", sess_name);
                                                    quit = true;
                                                }
                                                tree_chooser = false;
                                                tree_num_buffer.clear();
                                            } else if *sess_name != current_session {
                                                // Window/pane in another session — switch to that session
                                                cmd_batch.push("client-detach\n".into());
                                                env::set_var("PSMUX_SWITCH_TO", sess_name);
                                                quit = true;
                                                tree_chooser = false;
                                                tree_num_buffer.clear();
                                            } else if *is_win {
                                                cmd_batch.push(format!("focus-window {}\n", wid));
                                                tree_chooser = false;
                                                tree_num_buffer.clear();
                                            } else {
                                                cmd_batch.push(format!("focus-pane {}\n", pid));
                                                tree_chooser = false;
                                                tree_num_buffer.clear();
                                            }
                                        }
                                    }
                                }
                                KeyCode::Esc if tree_chooser => { tree_chooser = false; tree_num_buffer.clear(); }
                                KeyCode::Backspace if tree_chooser => { tree_num_buffer.pop(); }
                                // 'p' toggles the live preview pane in choose-tree
                                KeyCode::Char('p') if tree_chooser => {
                                    preview_enabled = !preview_enabled;
                                }
                                // 'x' kills the highlighted window (mirrors the session
                                // chooser's `x`). Scoped to a window row in the current
                                // session: focus the window, then kill it. The digit-jump
                                // buffer wins over the arrow cursor, same as Enter.
                                KeyCode::Char('x') if tree_chooser => {
                                    let target_idx: Option<usize> = if tree_num_buffer.is_empty() {
                                        Some(tree_selected)
                                    } else {
                                        match tree_num_buffer.parse::<usize>() {
                                            Ok(n) if n >= 1 && n <= tree_entries.len() => Some(n - 1),
                                            _ => None,
                                        }
                                    };
                                    if let Some(sel_idx) = target_idx {
                                        // Copy out the fields we need so the immutable borrow
                                        // ends before we mutate tree_entries below.
                                        if let Some((is_win, wid, sess_name)) = tree_entries
                                            .get(sel_idx)
                                            .map(|(iw, w, _p, _l, s)| (*iw, *w, s.clone()))
                                        {
                                            if is_win && wid != usize::MAX && sess_name == current_session {
                                                cmd_batch.push(format!("focus-window {}\n", wid));
                                                cmd_batch.push("kill-window\n".into());
                                                // Drop the killed row and clamp the cursor so
                                                // the picker stays open for further kills.
                                                tree_entries.remove(sel_idx);
                                                if tree_selected >= tree_entries.len() && tree_selected > 0 {
                                                    tree_selected -= 1;
                                                }
                                                if tree_entries.is_empty() {
                                                    tree_chooser = false;
                                                }
                                                tree_num_buffer.clear();
                                            }
                                        }
                                    }
                                }
                                KeyCode::Char(c) if tree_chooser && c.is_ascii_digit() => {
                                    // Append to the digit-jump buffer; Enter consumes it.
                                    if tree_num_buffer.len() < 6 {
                                        tree_num_buffer.push(c);
                                    }
                                }
                                // Absorb any other char while the tree picker is open so
                                // it cannot leak through to the focused pane's PTY.
                                KeyCode::Char(_) if tree_chooser => {}
                                // --- buffer chooser (C-b =) ---
                                KeyCode::Up | KeyCode::Char('k') if buffer_chooser => {
                                    if buffer_selected > 0 { buffer_selected -= 1; }
                                }
                                KeyCode::Down | KeyCode::Char('j') if buffer_chooser => {
                                    if buffer_selected + 1 < buffer_entries.len() { buffer_selected += 1; }
                                }
                                // hjkl parity with tmux mode-tree (issue #259): h = up, l = down for flat lists
                                KeyCode::Char('h') if buffer_chooser => { if buffer_selected > 0 { buffer_selected -= 1; } }
                                KeyCode::Char('l') if buffer_chooser => { if buffer_selected + 1 < buffer_entries.len() { buffer_selected += 1; } }
                                KeyCode::Char('g') if buffer_chooser => { buffer_selected = 0; }
                                KeyCode::Char('G') if buffer_chooser => { buffer_selected = buffer_entries.len().saturating_sub(1); }
                                KeyCode::PageUp if buffer_chooser => { buffer_selected = buffer_selected.saturating_sub(10); }
                                KeyCode::PageDown if buffer_chooser => { buffer_selected = (buffer_selected + 10).min(buffer_entries.len().saturating_sub(1)); }
                                KeyCode::Home if buffer_chooser => { buffer_selected = 0; }
                                KeyCode::End if buffer_chooser => { buffer_selected = buffer_entries.len().saturating_sub(1); }
                                KeyCode::Enter if buffer_chooser => {
                                    // Digit-jump: number+Enter selects the Nth visible buffer
                                    // (1-based). Empty buffer falls back to arrow cursor.
                                    let target_idx: Option<usize> = if buffer_num_buffer.is_empty() {
                                        Some(buffer_selected)
                                    } else {
                                        match buffer_num_buffer.parse::<usize>() {
                                            Ok(n) if n >= 1 && n <= buffer_entries.len() => Some(n - 1),
                                            _ => None,
                                        }
                                    };
                                    if let Some(sel) = target_idx {
                                        if sel < buffer_entries.len() {
                                            let (idx, _, _) = &buffer_entries[sel];
                                            cmd_batch.push(format!("paste-buffer-at {}\n", idx));
                                            buffer_chooser = false;
                                            buffer_num_buffer.clear();
                                        }
                                    }
                                }
                                KeyCode::Char('d') | KeyCode::Delete if buffer_chooser => {
                                    // Delete selected buffer
                                    if buffer_selected < buffer_entries.len() {
                                        let (idx, _, _) = &buffer_entries[buffer_selected];
                                        cmd_batch.push(format!("delete-buffer-at {}\n", idx));
                                        buffer_entries.remove(buffer_selected);
                                        // Re-index remaining entries
                                        for (i, entry) in buffer_entries.iter_mut().enumerate() {
                                            entry.0 = i;
                                        }
                                        if buffer_selected >= buffer_entries.len() && buffer_selected > 0 {
                                            buffer_selected -= 1;
                                        }
                                        if buffer_entries.is_empty() {
                                            buffer_chooser = false;
                                        }
                                        // Indexes shifted; drop any pending jump buffer.
                                        buffer_num_buffer.clear();
                                    }
                                }
                                KeyCode::Esc | KeyCode::Char('q') if buffer_chooser => { buffer_chooser = false; buffer_num_buffer.clear(); }
                                KeyCode::Backspace if buffer_chooser => { buffer_num_buffer.pop(); }
                                KeyCode::Char(c) if buffer_chooser && c.is_ascii_digit() => {
                                    if buffer_num_buffer.len() < 6 {
                                        buffer_num_buffer.push(c);
                                    }
                                }
                                // Absorb any other char while the buffer picker is open so
                                // it cannot leak through to the focused pane's PTY.
                                KeyCode::Char(_) if buffer_chooser => {}
                                // --- list-keys viewer (C-b ?) ---
                                KeyCode::Up if keys_viewer => { if keys_viewer_scroll > 0 { keys_viewer_scroll -= 1; } }
                                KeyCode::Down if keys_viewer => { keys_viewer_scroll += 1; }
                                KeyCode::PageUp if keys_viewer => { keys_viewer_scroll = keys_viewer_scroll.saturating_sub(20); }
                                KeyCode::PageDown if keys_viewer => { keys_viewer_scroll += 20; }
                                KeyCode::Home if keys_viewer => { keys_viewer_scroll = 0; }
                                KeyCode::End if keys_viewer => { keys_viewer_scroll = keys_viewer_lines.len().saturating_sub(1); }
                                KeyCode::Char('q') if keys_viewer => { keys_viewer = false; }
                                KeyCode::Esc if keys_viewer => { keys_viewer = false; }
                                KeyCode::Char('k') if keys_viewer => { if keys_viewer_scroll > 0 { keys_viewer_scroll -= 1; } }
                                KeyCode::Char('j') if keys_viewer => { keys_viewer_scroll += 1; }
                                // hjkl parity with tmux mode-tree (issue #259): h = up, l = down, g/G = home/end
                                KeyCode::Char('h') if keys_viewer => { if keys_viewer_scroll > 0 { keys_viewer_scroll -= 1; } }
                                KeyCode::Char('l') if keys_viewer => { keys_viewer_scroll += 1; }
                                KeyCode::Char('g') if keys_viewer => { keys_viewer_scroll = 0; }
                                KeyCode::Char('G') if keys_viewer => { keys_viewer_scroll = keys_viewer_lines.len().saturating_sub(1); }
                                // --- kill confirmation: y/Y/Enter confirms, n/N/Esc cancels ---
                                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter if confirm_cmd.is_some() => {
                                    if let Some(cmd) = confirm_cmd.take() {
                                        cmd_batch.push(format!("{}\n", cmd));
                                    }
                                }
                                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc if confirm_cmd.is_some() => {
                                    confirm_cmd = None;
                                }
                                KeyCode::Char(c) if renaming && !key.modifiers.contains(KeyModifiers::CONTROL) && !paste_burst_active => { rename_buf.push(c); }
                                KeyCode::Char(c) if pane_renaming && !key.modifiers.contains(KeyModifiers::CONTROL) && !paste_burst_active => { pane_title_buf.push(c); }
                                KeyCode::Char(c) if window_idx_input && c.is_ascii_digit() && !paste_burst_active => { window_idx_buf.push(c); }
                                KeyCode::Char(c) if command_input && !key.modifiers.contains(KeyModifiers::CONTROL) && !paste_burst_active => { command_buf.insert(command_cursor, c); command_cursor += c.len_utf8(); }
                                KeyCode::Backspace if renaming => { let _ = rename_buf.pop(); }
                                KeyCode::Backspace if pane_renaming => { let _ = pane_title_buf.pop(); }
                                KeyCode::Backspace if window_idx_input => { let _ = window_idx_buf.pop(); }
                                KeyCode::Backspace if command_input => {
                                    if command_cursor > 0 {
                                        // Walk back to the previous char boundary so we never split a UTF-8 sequence.
                                        let prev_len = command_buf[..command_cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(1);
                                        let new_cursor = command_cursor - prev_len;
                                        command_buf.replace_range(new_cursor..command_cursor, "");
                                        command_cursor = new_cursor;
                                    }
                                }
                                KeyCode::Enter if renaming => {
                                    if session_renaming {
                                        cmd_batch.push(format!("rename-session {}\n", quote_arg(&rename_buf)));
                                        session_renaming = false;
                                    } else {
                                        cmd_batch.push(format!("rename-window {}\n", quote_arg(&rename_buf)));
                                    }
                                    renaming = false;
                                }
                                KeyCode::Enter if pane_renaming => { cmd_batch.push(format!("set-pane-title {}\n", quote_arg(&pane_title_buf))); pane_renaming = false; }
                                KeyCode::Enter if window_idx_input => {
                                    if !window_idx_buf.is_empty() {
                                        cmd_batch.push(format!("select-window -t :{}\n", window_idx_buf));
                                    }
                                    window_idx_input = false;
                                }
                                KeyCode::Enter if command_input => {
                                    let trimmed = command_buf.trim().to_string();
                                    if !trimmed.is_empty() || command_template.is_some() {
                                        if !trimmed.is_empty() {
                                            command_history.push(trimmed.clone());
                                            command_history_idx = command_history.len();
                                        }
                                        // If we have a template (from command-prompt -I ... 'cmd "%%"'),
                                        // substitute %% with user input and send that command instead.
                                        let final_cmd = if let Some(ref tmpl) = command_template {
                                            tmpl.replace("%%", &trimmed)
                                        } else {
                                            trimmed.clone()
                                        };
                                        // Intercept client-side UI commands from command prompt
                                        let first_word = final_cmd.split_whitespace().next().unwrap_or("");
                                        if first_word == "choose-buffer" || first_word == "chooseb" {
                                            // Open interactive buffer chooser instead of sending to server
                                            buffer_chooser = true;
                                            buffer_entries.clear();
                                            buffer_selected = 0;
                                            buffer_scroll = 0;
                                            buffer_num_buffer.clear();
                                            let port_file = crate::paths::port_file(&current_session);
                                            if let Ok(port_str) = std::fs::read_to_string(&port_file) {
                                                if let Ok(p) = port_str.trim().parse::<u16>() {
                                                    let sess_key = read_session_key(&current_session).unwrap_or_default();
                                                    let addr = format!("127.0.0.1:{}", p);
                                                    if let Some(buf_line) = crate::session::fetch_authed_response_multi(
                                                        &addr,
                                                        &sess_key,
                                                        b"choose-buffer\n",
                                                        Duration::from_millis(100),
                                                        Duration::from_millis(200),
                                                    ) {
                                                        {
                                                            for line in buf_line.trim().split('\n') {
                                                                let line = line.trim();
                                                                if line.is_empty() { continue; }
                                                                if let Some(rest) = line.strip_prefix("buffer") {
                                                                    if let Some(colon_pos) = rest.find(':') {
                                                                        if let Ok(idx) = rest[..colon_pos].parse::<usize>() {
                                                                            let after_colon = &rest[colon_pos+1..].trim_start();
                                                                            let byte_len = after_colon.split_whitespace().next()
                                                                                .and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                                                                            let preview = if let Some(bp) = after_colon.find('"') {
                                                                                let p = &after_colon[bp+1..];
                                                                                p.trim_end_matches('"').to_string()
                                                                            } else {
                                                                                after_colon.to_string()
                                                                            };
                                                                            buffer_entries.push((idx, byte_len, preview));
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            if buffer_entries.is_empty() { buffer_chooser = false; }
                                        } else {
                                            // Split on \; or ; to support command chaining (issue #192)
                                            let sub_cmds = crate::config::split_chained_commands_pub(&final_cmd);
                                            // detach-client typed at the prompt must also quit
                                            // THIS client unless `-a` (detach others) or
                                            // `-t %<id>`/`-t <tty>` (target someone else) is
                                            // specified.  Mirrors how prefix+d quits at the
                                            // keybinding dispatch level (issue #275).
                                            let mut quit_on_detach = false;
                                            for sub in &sub_cmds {
                                                let parts: Vec<&str> = sub.split_whitespace().collect();
                                                if parts.first().map_or(false, |w| *w == "detach-client" || *w == "detach") {
                                                    let detach_others_only = parts.iter().any(|p| *p == "-a");
                                                    let has_target = parts.windows(2).any(|w| w[0] == "-t");
                                                    if !detach_others_only && !has_target {
                                                        quit_on_detach = true;
                                                    }
                                                }
                                                cmd_batch.push(format!("{}\n", sub));
                                            }
                                            if quit_on_detach {
                                                quit = true;
                                            }
                                        }
                                    }
                                    command_input = false;
                                    command_cursor = 0;
                                }
                                KeyCode::Esc if renaming => { renaming = false; session_renaming = false; }
                                KeyCode::Esc if pane_renaming => { pane_renaming = false; }
                                KeyCode::Esc if window_idx_input => { window_idx_input = false; }
                                KeyCode::Esc if command_input => { command_input = false; command_cursor = 0; }

                                // Command prompt: cursor movement, history, and editing keys
                                KeyCode::Left if command_input => {
                                    if command_cursor > 0 {
                                        let prev_len = command_buf[..command_cursor].chars().next_back().map(|c| c.len_utf8()).unwrap_or(1);
                                        command_cursor -= prev_len;
                                    }
                                }
                                KeyCode::Right if command_input => {
                                    if command_cursor < command_buf.len() {
                                        let next_len = command_buf[command_cursor..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                                        command_cursor += next_len;
                                    }
                                }
                                KeyCode::Home if command_input => { command_cursor = 0; }
                                KeyCode::End if command_input => { command_cursor = command_buf.len(); }
                                KeyCode::Delete if command_input => { if command_cursor < command_buf.len() { command_buf.remove(command_cursor); } }
                                KeyCode::Up if command_input => {
                                    if command_history_idx > 0 {
                                        command_history_idx -= 1;
                                        command_buf = command_history[command_history_idx].clone();
                                        command_cursor = command_buf.len();
                                    }
                                }
                                KeyCode::Down if command_input => {
                                    if command_history_idx < command_history.len() {
                                        command_history_idx += 1;
                                        command_buf = if command_history_idx < command_history.len() {
                                            command_history[command_history_idx].clone()
                                        } else {
                                            String::new()
                                        };
                                        command_cursor = command_buf.len();
                                    }
                                }
                                KeyCode::Char('a') if command_input && key.modifiers.contains(KeyModifiers::CONTROL) => { command_cursor = 0; }
                                KeyCode::Char('e') if command_input && key.modifiers.contains(KeyModifiers::CONTROL) => { command_cursor = command_buf.len(); }
                                KeyCode::Char('u') if command_input && key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    command_buf.drain(..command_cursor);
                                    command_cursor = 0;
                                }
                                KeyCode::Char('k') if command_input && key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    command_buf.truncate(command_cursor);
                                }
                                KeyCode::Char('w') if command_input && key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let mut pos = command_cursor;
                                    while pos > 0 && command_buf.as_bytes().get(pos - 1) == Some(&b' ') { pos -= 1; }
                                    while pos > 0 && command_buf.as_bytes().get(pos - 1) != Some(&b' ') { pos -= 1; }
                                    command_buf.drain(pos..command_cursor);
                                    command_cursor = pos;
                                }

                                KeyCode::Char(' ') => {
                                    #[cfg(windows)]
                                    {
                                        paste_pend.push(' ');
                                        if paste_pend_start.is_none() {
                                            paste_pend_start = Some(Instant::now());
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        cmd_batch.push("send-key space\n".into());
                                    }
                                }
                                // AltGr detection: On Windows, AltGr is reported as
                                // Ctrl+Alt.  Non-lowercase-letter chars with Ctrl+Alt
                                // are AltGr-produced (e.g. \ @ { } [ ] | ~ on
                                // German/Czech keyboards) — treat as plain text.
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.modifiers.contains(KeyModifiers::ALT)
                                    && !c.is_ascii_lowercase() => {
                                    #[cfg(windows)]
                                    {
                                        paste_pend.push(c);
                                        if paste_pend_start.is_none() {
                                            paste_pend_start = Some(Instant::now());
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        let escaped = match c {
                                            '"' => "\\\"".to_string(),
                                            '\\' => "\\\\".to_string(),
                                            _ => c.to_string(),
                                        };
                                        cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                                    }
                                }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::ALT) => {
                                    cmd_batch.push(format!("send-key C-M-{}\n", c.to_ascii_lowercase()));
                                }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
                                    cmd_batch.push(format!("send-key M-{}\n", c));
                                }
                                // pwsh-mouse-selection: Ctrl+Shift+C / Ctrl+Shift+V
                                // explicit copy/paste regardless of selection state.
                                KeyCode::Char('C') if client_pwsh_selection
                                    && key.kind == KeyEventKind::Press
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
                                {
                                    if let (Some(s), Some(e)) = (rsel_start, rsel_end) {
                                        if rsel_dragged {
                                            if let Ok(state) = serde_json::from_str::<DumpState>(&prev_dump_buf) {
                                                let text = extract_selection_text(
                                                    &state.layout,
                                                    last_sent_size.0,
                                                    last_sent_size.1,
                                                    s, e,
                                                    rsel_block,
                                                    rsel_pane_rect, &client_border_status, &client_border_format,
                                                );
                                                if !text.is_empty() {
                                                    copy_to_system_clipboard(&text);
                                                    pending_osc52 = Some(text);
                                                }
                                            }
                                        }
                                    }
                                    rsel_start = None;
                                    rsel_end = None;
                                    rsel_pane_rect = None;
                                    rsel_pane_id = None;
                                    rsel_block = false;
                                    rsel_dragged = false;
                                    selection_changed = true;
                                }
                                KeyCode::Char('V') if client_pwsh_selection
                                    && key.kind == KeyEventKind::Press
                                    && key.modifiers.contains(KeyModifiers::CONTROL)
                                    && key.modifiers.contains(KeyModifiers::SHIFT) =>
                                {
                                    if let Some(text) = read_from_system_clipboard() {
                                        if !text.is_empty() {
                                            let encoded = base64_encode(&text);
                                            cmd_batch.push(format!("send-paste {}\n", encoded));
                                        }
                                    }
                                }
                                // Ctrl+C smart: when a selection is active in
                                // pwsh-mouse-selection mode, copy and clear.
                                // Otherwise fall through to the generic Ctrl handler
                                // which sends SIGINT to the shell.
                                KeyCode::Char('c') if client_pwsh_selection
                                    && key.kind == KeyEventKind::Press
                                    && key.modifiers == KeyModifiers::CONTROL
                                    && rsel_dragged
                                    && rsel_start.is_some() =>
                                {
                                    if let (Some(s), Some(e)) = (rsel_start, rsel_end) {
                                        if let Ok(state) = serde_json::from_str::<DumpState>(&prev_dump_buf) {
                                            let text = extract_selection_text(
                                                &state.layout,
                                                last_sent_size.0,
                                                last_sent_size.1,
                                                s, e,
                                                rsel_block,
                                                rsel_pane_rect, &client_border_status, &client_border_format,
                                            );
                                            if !text.is_empty() {
                                                copy_to_system_clipboard(&text);
                                                pending_osc52 = Some(text);
                                            }
                                        }
                                    }
                                    rsel_start = None;
                                    rsel_end = None;
                                    rsel_pane_rect = None;
                                    rsel_pane_id = None;
                                    rsel_block = false;
                                    rsel_dragged = false;
                                    selection_changed = true;
                                }
                                // On Windows, suppress Ctrl+V Press when paste-detection
                                // is enabled — the console host already injected clipboard
                                // content as character events and the paste mechanism
                                // handles them.  When paste-detection is off, forward C-v
                                // to the child app (e.g. neovim visual block mode).
                                #[cfg(windows)]
                                KeyCode::Char('v') if key.modifiers == KeyModifiers::CONTROL && paste_detection_enabled => {}
                                #[cfg(windows)]
                                KeyCode::Char('v') if key.modifiers == KeyModifiers::CONTROL && !paste_detection_enabled => {
                                    cmd_batch.push("send-key C-v\n".to_string());
                                }
                                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    // Preserve a held Shift so Ctrl+Shift+<letter> is not
                                    // silently collapsed to a plain Ctrl+<letter> on the wire
                                    // (issue #368).  tmux-style name: C-S-x.  The server
                                    // injects a native KEY_EVENT carrying both modifiers so
                                    // console-input apps can tell the two apart.
                                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                                        cmd_batch.push(format!("send-key C-S-{}\n", c.to_ascii_lowercase()));
                                    } else {
                                        cmd_batch.push(format!("send-key C-{}\n", c.to_ascii_lowercase()));
                                    }
                                }
                                KeyCode::Char(c) if (c as u32) >= 0x01 && (c as u32) <= 0x1A => {
                                    let ctrl_letter = ((c as u8) + b'a' - 1) as char;
                                    cmd_batch.push(format!("send-key C-{}\n", ctrl_letter));
                                }
                                KeyCode::Char(c) => {
                                    #[cfg(windows)]
                                    {
                                        // Suppress text key events during the post-copy
                                        // suppression window (VS Code ConPTY duplicate).
                                        let suppressed = paste_suppress_until
                                            .map_or(false, |t| Instant::now() < t);
                                        if suppressed {
                                            if input_log_enabled() {
                                                input_log("paste", &format!("suppressed char '{}' during paste suppress window", c));
                                            }
                                        } else {
                                            paste_suppress_until = None;
                                            paste_pend.push(c);
                                            if paste_pend_start.is_none() {
                                                paste_pend_start = Some(Instant::now());
                                            }
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    {
                                        let escaped = match c {
                                            '"' => "\\\"".to_string(),
                                            '\\' => "\\\\".to_string(),
                                            _ => c.to_string(),
                                        };
                                        cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                                    }
                                }
                                KeyCode::Enter => {
                                    #[cfg(windows)]
                                    {
                                        // If paste detection is enabled, treat plain Enter as
                                        // bufferable even when it's the first key in the burst.
                                        // This prevents a leading pasted newline from being
                                        // forwarded as an immediate submit before the rest of
                                        // the clipboard payload arrives.
                                        if should_buffer_leading_paste_control(
                                            &key.code,
                                            key.modifiers,
                                            paste_detection_enabled,
                                            !paste_pend.is_empty(),
                                        ) {
                                            paste_pend.push('\n');
                                            if paste_pend_start.is_none() {
                                                paste_pend_start = Some(Instant::now());
                                            }
                                        } else {
                                            cmd_batch.push(format!("send-key {}\n", modified_key_name("Enter", key.modifiers)));
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    { cmd_batch.push(format!("send-key {}\n", modified_key_name("Enter", key.modifiers))); }
                                }
                                KeyCode::Tab => {
                                    #[cfg(windows)]
                                    {
                                        if should_buffer_leading_paste_control(
                                            &key.code,
                                            key.modifiers,
                                            paste_detection_enabled,
                                            !paste_pend.is_empty(),
                                        ) {
                                            paste_pend.push('\t');
                                            if paste_pend_start.is_none() {
                                                paste_pend_start = Some(Instant::now());
                                            }
                                        } else {
                                            cmd_batch.push("send-key tab\n".into());
                                        }
                                    }
                                    #[cfg(not(windows))]
                                    { cmd_batch.push("send-key tab\n".into()); }
                                }
                                KeyCode::BackTab => { cmd_batch.push("send-key btab\n".into()); }
                                // Backspace has to carry its modifiers like every
                                // other special key here (issue #610).  Naming it
                                // "backspace" unconditionally threw Ctrl away on the
                                // wire, so the server could only ever write a plain
                                // 0x7f and no bind-key C-BSpace could match.  The
                                // unmodified case still lowercases to "backspace",
                                // exactly as before.
                                KeyCode::Backspace => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Backspace", key.modifiers))); }
                                KeyCode::Delete => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Delete", key.modifiers))); }
                                KeyCode::Esc => { cmd_batch.push("send-key esc\n".into()); }
                                KeyCode::Left => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Left", key.modifiers))); }
                                KeyCode::Right => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Right", key.modifiers))); }
                                KeyCode::Up => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Up", key.modifiers))); }
                                KeyCode::Down => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Down", key.modifiers))); }
                                KeyCode::PageUp => { cmd_batch.push(format!("send-key {}\n", modified_key_name("PageUp", key.modifiers))); }
                                KeyCode::PageDown => { cmd_batch.push(format!("send-key {}\n", modified_key_name("PageDown", key.modifiers))); }
                                KeyCode::Home => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Home", key.modifiers))); }
                                KeyCode::End => { cmd_batch.push(format!("send-key {}\n", modified_key_name("End", key.modifiers))); }
                                KeyCode::Insert => { cmd_batch.push(format!("send-key {}\n", modified_key_name("Insert", key.modifiers))); }
                                KeyCode::F(n) => { cmd_batch.push(format!("send-key {}\n", modified_key_name(&format!("F{}", n), key.modifiers))); }
                                _ => {}
                            }
                        }
                    }
                    Event::Paste(data) => {
                        // Route paste into the active client-side text overlay
                        // (issue #290) so it does not leak past the command
                        // prompt / rename prompts into the underlying pane.
                        let consumed = route_paste_to_overlay(
                            &data,
                            command_input, &mut command_buf, &mut command_cursor,
                            renaming, &mut rename_buf,
                            pane_renaming, &mut pane_title_buf,
                            window_idx_input, &mut window_idx_buf,
                        );
                        if !consumed {
                            let encoded = base64_encode(&data);
                            cmd_batch.push(format!("send-paste {}\n", encoded));
                        }
                        // On Windows, crossterm with EnableBracketedPaste may
                        // emit Event::Paste AND individual Event::Key events
                        // for the same Ctrl+V paste.  Suppress the duplicate
                        // Key events by clearing any partially accumulated
                        // paste_pend chars and blocking accumulation briefly.
                        // The same paste_suppress_until window also gates the
                        // overlay Char arms so the duplicate Key events do
                        // not double-insert when an overlay consumed the paste.
                        #[cfg(windows)]
                        {
                            paste_pend.clear();
                            paste_pend_start = None;
                            paste_stage2 = false;
                            paste_confirmed = false;
                            paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
                        }
                    }
                    Event::Mouse(me) => {
                        use crossterm::event::{MouseEventKind, MouseButton};
                        // Intercept mouse events while a draggable picker is open
                        // so the user can move the popup by dragging its border
                        // and so clicks behind the popup don't leak through to
                        // the underlying panes (issue #257).
                        if tree_chooser || session_chooser {
                            let on_top_border = popup_rect_last.map_or(false, |r| {
                                me.row == r.y && me.column >= r.x && me.column < r.x + r.width
                            });
                            match me.kind {
                                MouseEventKind::Down(MouseButton::Left) => {
                                    if on_top_border {
                                        popup_dragging = true;
                                        popup_drag_anchor = (me.column, me.row);
                                        popup_initial_offset = popup_offset;
                                    }
                                }
                                MouseEventKind::Drag(MouseButton::Left) => {
                                    if popup_dragging {
                                        let dx = me.column as i32 - popup_drag_anchor.0 as i32;
                                        let dy = me.row as i32 - popup_drag_anchor.1 as i32;
                                        popup_offset = (
                                            popup_initial_offset.0 + dx,
                                            popup_initial_offset.1 + dy,
                                        );
                                    }
                                }
                                MouseEventKind::Up(MouseButton::Left) => {
                                    popup_dragging = false;
                                }
                                MouseEventKind::ScrollUp => {
                                    if tree_chooser && tree_selected > 0 { tree_selected -= 1; }
                                    if session_chooser && session_selected > 0 { session_selected -= 1; }
                                }
                                MouseEventKind::ScrollDown => {
                                    if tree_chooser && tree_selected + 1 < tree_entries.len() { tree_selected += 1; }
                                    if session_chooser {
                                        let visible_len = session_filtered_indices(&session_entries, &session_filter).len();
                                        if session_selected + 1 < visible_len { session_selected += 1; }
                                    }
                                }
                                _ => {}
                            }
                            // Advance to the next pending event without falling
                            // through to the underlying-pane mouse handler.
                            _pending_evt = input.try_read()?;
                            continue;
                        }
                        match me.kind {
                            MouseEventKind::Down(MouseButton::Left) => {
                                // Status bar tab click. Every status row is
                                // hit-tested (#593) — tmux server-client.c
                                // accepts y anywhere in [statusat,
                                // statusat+statuslines) and resolves the
                                // clicked row's own ranges. A click on status
                                // rows never falls through to pane handling.
                                if client_status_rows > 0
                                    && me.row >= client_status_row
                                    && me.row < client_status_row.saturating_add(client_status_rows) {
                                    let mut clicked_tab: Option<usize> = None;
                                    for &(row, win_idx, x_start, x_end) in &client_tab_positions {
                                        if me.row == row && me.column >= x_start && me.column < x_end {
                                            clicked_tab = Some(win_idx);
                                            break;
                                        }
                                    }
                                    if let Some(idx) = clicked_tab {
                                        // client_tab_positions stores the true display
                                        // index (honors gaps from renumber-windows off).
                                        // A range naming a window that does not exist
                                        // is ignored, matching tmux.
                                        if client_window_indices.contains(&idx) {
                                            cmd_batch.push(format!("select-window -t :{}\n", idx));
                                        }
                                    }
                                } else {
                                    // Border detection
                                    let mut on_border = false;
                                    if !client_zoomed {
                                        let tol = 0u16;
                                        for (bpath, bkind, bidx, bpos, btotal, bsizes, barea) in &client_borders {
                                            let hit = if bkind == "Horizontal" {
                                                me.column >= bpos.saturating_sub(tol) && me.column <= bpos + tol
                                                && me.row >= barea.y && me.row < barea.y + barea.height
                                            } else {
                                                me.row >= bpos.saturating_sub(tol) && me.row <= bpos + tol
                                                && me.column >= barea.x && me.column < barea.x + barea.width
                                            };
                                            if hit {
                                                client_drag = Some(ClientDragState {
                                                    path: bpath.clone(),
                                                    kind: bkind.clone(),
                                                    index: *bidx,
                                                    start_pos: if bkind == "Horizontal" { me.column } else { me.row },
                                                    initial_sizes: bsizes.clone(),
                                                    total_pixels: *btotal,
                                                });
                                                border_drag = true;
                                                on_border = true;
                                                rsel_start = None;
                                                rsel_end = None;
                                                selection_changed = true;
                                                break;
                                            }
                                        }
                                    }

                                    if !on_border {
                                        let clicked_pane = client_pane_rects.iter().find(|(_, rect)| {
                                            rect.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                        });

                                        if let Some(&(pane_id, pane_rect)) = clicked_pane {
                                            cmd_batch.push(format!("select-pane -t %{}\n", pane_id));
                                            let rel_col = me.column as i16 - pane_rect.x as i16;
                                            let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);

                                            if client_copy_mode {
                                                deferred_left_click = None;
                                                cmd_batch.push(format!("pane-mouse {} 0 {} {} M\n",
                                                    pane_id, rel_col, rel_row));
                                                // Remember the pane so a drag that leaves its
                                                // rect still reaches the server (edge auto-scroll).
                                                copy_drag_pane = Some((pane_id, pane_rect));
                                                copy_drag_last = None;
                                                copy_drag_repeat_at = None;
                                                rsel_start = None;
                                                rsel_end = None;
                                                rsel_pane_rect = None;
                                                rsel_pane_id = None;
                                                rsel_block = false;
                                                selection_changed = true;
                                            } else {
                                                border_drag = false;

                                                // mouse-selection off: do not start any client-side
                                                // drag selection.  In-pane apps (opencode, nvim, etc.)
                                                // can implement their own mouse selection without
                                                // psmux drawing on top.  (issue #245)
                                                //
                                                // Per-pane: even with mouse-selection on, normally
                                                // yield to an app that enabled a mouse protocol. The
                                                // force option instead withholds the press until
                                                // release, so clicks can be replayed while drags stay
                                                // available for psmux selection.
                                                let pane_handles_mouse =
                                                    serde_json::from_str::<DumpState>(&prev_dump_buf)
                                                        .map(|s| pane_wants_mouse_json(&s.layout, pane_id))
                                                        .unwrap_or(false);
                                                let force_client_selection =
                                                    client_selection_owns_drag(
                                                        client_mouse_selection,
                                                        client_mouse_selection_force,
                                                        pane_handles_mouse,
                                                    ) && pane_handles_mouse;
                                                if force_client_selection {
                                                    deferred_left_click = Some((pane_id, rel_col, rel_row));
                                                } else {
                                                    deferred_left_click = None;
                                                    cmd_batch.push(format!("pane-mouse {} 0 {} {} M\n",
                                                        pane_id, rel_col, rel_row));
                                                }
                                                if !client_selection_owns_drag(
                                                    client_mouse_selection,
                                                    client_mouse_selection_force,
                                                    pane_handles_mouse,
                                                ) {
                                                    rsel_start = None;
                                                    rsel_end = None;
                                                    rsel_pane_rect = None;
                                                    rsel_pane_id = None;
                                                    rsel_block = false;
                                                    rsel_dragged = false;
                                                    selection_changed = true;
                                                } else {
                                                // Ctrl+click extends an existing selection to the click
                                                // position without starting a new one. (Shift+click
                                                // cannot be used on Windows Terminal — it is reserved
                                                // for WT's native selection override.) Only active
                                                // when pwsh-mouse-selection is on and a selection
                                                // already exists in the same pane.
                                                let ctrl_extend = client_pwsh_selection
                                                    && me.modifiers.contains(KeyModifiers::CONTROL)
                                                    && rsel_start.is_some()
                                                    && rsel_pane_rect == Some(pane_rect);

                                                if ctrl_extend {
                                                    let r = pane_rect;
                                                    let col = me.column.clamp(r.x, r.x + r.width.saturating_sub(1));
                                                    let row = me.row.clamp(r.y, r.y + r.height.saturating_sub(1));
                                                    rsel_end = Some((col, row));
                                                    rsel_dragged = true;
                                                    selection_changed = true;
                                                } else if client_pwsh_selection {
                                                    rsel_block = me.modifiers.contains(KeyModifiers::ALT);
                                                    rsel_pane_rect = Some(pane_rect);
                                                    rsel_pane_id = Some(pane_id);
                                                    rsel_dragged = false;
                                                    selection_changed = true;

                                                    let now = Instant::now();
                                                    let is_multi = last_click.map_or(false, |(t, (c, r))| {
                                                        now.duration_since(t) < Duration::from_millis(400)
                                                            && c == me.column && r == me.row
                                                    });
                                                    click_count = if is_multi { click_count + 1 } else { 1 };
                                                    last_click = Some((now, (me.column, me.row)));

                                                    let word = if click_count == 2 {
                                                        serde_json::from_str::<DumpState>(&prev_dump_buf).ok()
                                                            .and_then(|s| word_bounds_at(
                                                                &s.layout,
                                                                last_sent_size.0,
                                                                last_sent_size.1,
                                                                pane_rect,
                                                                me.column, me.row,
                                                                &client_border_status, &client_border_format,
                                                            ))
                                                    } else {
                                                        None
                                                    };

                                                    if let Some((ws, we)) = word {
                                                        rsel_start = Some((ws, me.row));
                                                        rsel_end = Some((we, me.row));
                                                        rsel_dragged = true;
                                                    } else if click_count >= 3 {
                                                        let left = pane_rect.x;
                                                        let right = pane_rect.x + pane_rect.width.saturating_sub(1);
                                                        rsel_start = Some((left, me.row));
                                                        rsel_end = Some((right, me.row));
                                                        rsel_dragged = true;
                                                    } else {
                                                        rsel_start = Some((me.column, me.row));
                                                        rsel_end = None;
                                                    }
                                                } else {
                                                    // Legacy: start == end for 1-cell hint.
                                                    rsel_start = Some((me.column, me.row));
                                                    rsel_end = Some((me.column, me.row));
                                                    rsel_pane_rect = Some(pane_rect);
                                                    rsel_pane_id = Some(pane_id);
                                                    rsel_dragged = false;
                                                    selection_changed = true;
                                                }
                                                } // end client-side selection gate (mouse-selection + per-pane wants_mouse)
                                            }
                                        } else {
                                            deferred_left_click = None;
                                            cmd_batch.push(format!("mouse-down {} {}\n", me.column, me.row));
                                        }
                                    }
                                }
                            }
                            MouseEventKind::Down(MouseButton::Right) => {
                                // Check if active pane is running a TUI app (alternate screen).
                                // TUI apps (htop, Claude Code, etc.) expect right-click as a
                                // mouse event, NOT clipboard paste.
                                let tui_active = if !prev_dump_buf.is_empty() {
                                    serde_json::from_str::<DumpState>(&prev_dump_buf)
                                        .map(|s| active_pane_in_alt_screen(&s.layout))
                                        .unwrap_or(false)
                                } else { false };

                                if tui_active {
                                    // Forward right-click as pane-relative mouse event
                                    if let Some(&(pane_id, pane_rect)) = client_pane_rects.iter().find(|(_, r)| {
                                        r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                    }) {
                                        let rel_col = me.column as i16 - pane_rect.x as i16;
                                        let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);
                                        cmd_batch.push(format!("pane-mouse {} 2 {} {} M\n",
                                            pane_id, rel_col, rel_row));
                                    }
                                    rsel_start = None;
                                    rsel_end = None;
                                    selection_changed = true;
                                } else if rsel_start.is_some() && rsel_dragged {
                                    // pwsh-style: right-click with active selection → copy + clear
                                    if let (Some(s), Some(e)) = (rsel_start, rsel_end) {
                                        if let Ok(state) = serde_json::from_str::<DumpState>(&prev_dump_buf) {
                                            let text = extract_selection_text(
                                                &state.layout,
                                                last_sent_size.0,
                                                last_sent_size.1,
                                                s, e,
                                                rsel_block,
                                                rsel_pane_rect, &client_border_status, &client_border_format,
                                            );
                                            if !text.is_empty() {
                                                copy_to_system_clipboard(&text);
                                                pending_osc52 = Some(text);
                                            }
                                        }
                                    }
                                    rsel_start = None;
                                    rsel_end = None;
                                    rsel_pane_rect = None;
                                    rsel_pane_id = None;
                                    rsel_block = false;
                                    rsel_dragged = false;
                                    selection_changed = true;
                                    // Suppress text key events that VS Code's ConPTY
                                    // injects after a right-click copy action.
                                    paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
                                } else {
                                    // No selection, no TUI — paste from clipboard (pwsh-style)
                                    rsel_start = None;
                                    rsel_end = None;
                                    selection_changed = true;
                                    if let Some(text) = read_from_system_clipboard() {
                                        if !text.is_empty() {
                                            let encoded = base64_encode(&text);
                                            cmd_batch.push(format!("send-paste {}\n", encoded));
                                        }
                                    }
                                }
                            }
                            MouseEventKind::Down(MouseButton::Middle) => {
                                if let Some(&(pane_id, pane_rect)) = client_pane_rects.iter().find(|(_, r)| {
                                    r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                }) {
                                    let rel_col = me.column as i16 - pane_rect.x as i16;
                                    let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);
                                    cmd_batch.push(format!("pane-mouse {} 1 {} {} M\n",
                                        pane_id, rel_col, rel_row));
                                } else {
                                    cmd_batch.push(format!("mouse-down-middle {} {}\n", me.column, me.row));
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                if border_drag {
                                    if let Some(ref d) = client_drag {
                                        let current_pos = if d.kind == "Horizontal" { me.column } else { me.row };
                                        let pixel_delta = current_pos as i32 - d.start_pos as i32;
                                        let total_pct: i32 = d.initial_sizes.iter().map(|&s| s as i32).sum();
                                        let total_px = d.total_pixels.max(1) as i32;
                                        let pct_delta = (pixel_delta * total_pct) / total_px;
                                        let min_pct = 5i32;

                                        let mut new_sizes = d.initial_sizes.clone();
                                        let left = (d.initial_sizes[d.index] as i32 + pct_delta)
                                            .clamp(min_pct, d.initial_sizes[d.index] as i32 + d.initial_sizes[d.index + 1] as i32 - min_pct) as u16;
                                        let right = d.initial_sizes[d.index] + d.initial_sizes[d.index + 1] - left;
                                        new_sizes[d.index] = left;
                                        new_sizes[d.index + 1] = right;

                                        let path_str = if d.path.is_empty() { "_".to_string() } else { d.path.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(".") };
                                        let sizes_str = new_sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
                                        cmd_batch.push(format!("split-sizes {} {}\n", path_str, sizes_str));
                                    }
                                } else if rsel_start.is_none() || !client_mouse_selection {
                                    if client_copy_mode {
                                        // Route the drag to the pane it started in (falling
                                        // back to the pane under the pointer) and send the
                                        // row UNCLAMPED: an out-of-range row tells the server
                                        // the pointer crossed a pane edge, which auto-scrolls
                                        // the copy-mode view (tmux drag-scroll parity).
                                        if copy_drag_pane.is_none() {
                                            copy_drag_pane = client_pane_rects.iter().find(|(_, r)| {
                                                r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                            }).map(|&(id, r)| (id, r));
                                        }
                                        if let Some((pane_id, pane_rect)) = copy_drag_pane {
                                            let inner = pane_content_inner(pane_rect, &client_border_status, &client_border_format);
                                            let rel_col = me.column as i16 - pane_rect.x as i16;
                                            let rel_row = me.row as i16 - inner.y as i16;
                                            cmd_batch.push(format!("pane-mouse {} 32 {} {} M\n",
                                                pane_id, rel_col, rel_row));
                                            copy_drag_last = Some((rel_col, rel_row));
                                            // Arm the repeat while the pointer dwells on/past
                                            // the pane's first or last row: the pre-flush tick
                                            // re-sends this drag every 50ms so the view keeps
                                            // scrolling without further mouse movement.
                                            let at_edge = rel_row <= 0
                                                || rel_row >= inner.height.saturating_sub(1) as i16;
                                            copy_drag_repeat_at = if at_edge {
                                                Some(Instant::now() + COPY_DRAG_REPEAT)
                                            } else {
                                                None
                                            };
                                        }
                                    } else {
                                        cmd_batch.push(format!("mouse-drag {} {}\n", me.column, me.row));
                                    }
                                } else {
                                    if let Some(start) = rsel_start {
                                        // tmux parity: a real drag reaching the pane's top
                                        // row has run out of visible screen — hand the
                                        // selection off to server-side copy mode so it
                                        // continues into scrollback (with edge auto-scroll
                                        // from here on).  The bottom row does the same, but
                                        // only when the view is scrolled back (direct
                                        // scrollback with scroll-enter-copy-mode off, #193)
                                        // — at the live bottom there is nothing below to
                                        // select.  Same-cell micro-jitter is excluded so a
                                        // sloppy click on an edge row stays a click (#199
                                        // parity).
                                        let handoff = rsel_pane_rect.zip(rsel_pane_id)
                                            .and_then(|(pane_rect, pane_id)| {
                                                let inner = pane_content_inner(pane_rect, &client_border_status, &client_border_format);
                                                let moved = rsel_dragged || (me.column, me.row) != start;
                                                let at_top = me.row <= inner.y;
                                                let scrolled_back = client_pane_view_offsets.iter()
                                                    .find(|(id, _)| *id == pane_id)
                                                    .map_or(false, |(_, off)| *off > 0);
                                                let at_bottom = scrolled_back
                                                    && me.row >= inner.y + inner.height.saturating_sub(1);
                                                ((at_top || at_bottom) && moved)
                                                    .then_some((pane_rect, pane_id, inner))
                                            });
                                        if let Some((pane_rect, pane_id, inner)) = handoff {
                                            let a_col = start.0 as i16 - pane_rect.x as i16;
                                            let a_row = start.1 as i16 - inner.y as i16;
                                            let cur_col = me.column as i16 - pane_rect.x as i16;
                                            let cur_row = me.row as i16 - inner.y as i16;
                                            cmd_batch.push(format!("copy-drag-begin {} {} {} {} {}{}\n",
                                                pane_id, a_col, a_row, cur_col, cur_row,
                                                if rsel_block { " b" } else { "" }));
                                            // The gesture is definitively a selection drag,
                                            // not a click — never replay a withheld press
                                            // (mouse-selection-force) on release.
                                            deferred_left_click = None;
                                            // The server is in copy mode from here on; keep
                                            // the local flag in sync until the next dump
                                            // confirms it so follow-up drags take the
                                            // copy-mode path above.
                                            client_copy_mode = true;
                                            copy_drag_pane = Some((pane_id, pane_rect));
                                            copy_drag_last = Some((cur_col, cur_row));
                                            copy_drag_repeat_at = Some(Instant::now() + COPY_DRAG_REPEAT);
                                            rsel_start = None;
                                            rsel_end = None;
                                            rsel_pane_rect = None;
                                            rsel_pane_id = None;
                                            rsel_block = false;
                                            rsel_dragged = false;
                                            selection_changed = true;
                                        } else {
                                            let (col, row) = if client_pwsh_selection {
                                                if let Some(r) = rsel_pane_rect {
                                                    (
                                                        me.column.clamp(r.x, r.x + r.width.saturating_sub(1)),
                                                        me.row.clamp(r.y, r.y + r.height.saturating_sub(1)),
                                                    )
                                                } else {
                                                    (me.column, me.row)
                                                }
                                            } else {
                                                (me.column, me.row)
                                            };
                                            // Ignore micro-drags that stay on the
                                            // initial click cell (#199 parity).
                                            if (col, row) == start && !rsel_dragged {
                                                // no-op
                                            } else {
                                                rsel_end = Some((col, row));
                                                rsel_dragged = true;
                                                selection_changed = true;
                                            }
                                        }
                                    }
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Right) => {}
                            MouseEventKind::Up(MouseButton::Left) => {
                                if border_drag {
                                    deferred_left_click = None;
                                    cmd_batch.push(format!("split-resize-done\n"));
                                    border_drag = false;
                                    client_drag = None;
                                } else if rsel_dragged {
                                    if client_pwsh_selection {
                                        // tmux-like copy-on-release for
                                        // pwsh-mouse-selection: use the drag
                                        // endpoint tracked by MouseDrag so
                                        // double/triple-click bounds remain
                                        // intact, then clear transient state.
                                        if let (Some(s), Some(e)) = (rsel_start, rsel_end) {
                                            if let Ok(state) = serde_json::from_str::<DumpState>(&prev_dump_buf) {
                                                let text = extract_selection_text(
                                                    &state.layout,
                                                    last_sent_size.0,
                                                    last_sent_size.1,
                                                    s, e,
                                                    rsel_block,
                                                    rsel_pane_rect, &client_border_status, &client_border_format,
                                                );
                                                if !text.is_empty() {
                                                    copy_to_system_clipboard(&text);
                                                    pending_osc52 = Some(text);
                                                }
                                            }
                                        }
                                        rsel_start = None;
                                        rsel_end = None;
                                        rsel_pane_rect = None;
                                        rsel_pane_id = None;
                                        rsel_block = false;
                                        rsel_dragged = false;
                                        selection_changed = true;
                                    } else {
                                        // Legacy: copy-on-release.
                                        rsel_end = Some((me.column, me.row));
                                        if let (Some(s), Some(e)) = (rsel_start, rsel_end) {
                                            if let Ok(state) = serde_json::from_str::<DumpState>(&prev_dump_buf) {
                                                let text = extract_selection_text(
                                                    &state.layout,
                                                    last_sent_size.0,
                                                    last_sent_size.1,
                                                    s, e,
                                                    false,
                                                    rsel_pane_rect, &client_border_status, &client_border_format,
                                                );
                                                if !text.is_empty() {
                                                    copy_to_system_clipboard(&text);
                                                    pending_osc52 = Some(text);
                                                }
                                            }
                                        }
                                        rsel_start = None;
                                        rsel_end = None;
                                        rsel_pane_rect = None;
                                        rsel_pane_id = None;
                                        rsel_block = false;
                                        rsel_dragged = false;
                                        selection_changed = true;
                                    }
                                    deferred_left_click = None;
                                } else {
                                    rsel_start = None;
                                    rsel_end = None;
                                    rsel_pane_rect = None;
                                    rsel_pane_id = None;
                                    rsel_block = false;
                                    selection_changed = true;
                                    if let Some((pane_id, down_col, down_row)) = deferred_left_click.take() {
                                        let (up_col, up_row) = client_pane_rects.iter()
                                            .find(|(id, _)| *id == pane_id)
                                            .map(|(_, pane_rect)| {
                                                let inner = pane_content_inner(
                                                    *pane_rect,
                                                    &client_border_status,
                                                    &client_border_format,
                                                );
                                                (
                                                    (me.column as i16 - pane_rect.x as i16)
                                                        .max(0)
                                                        .min(pane_rect.width.saturating_sub(1) as i16),
                                                    (me.row as i16 - inner.y as i16)
                                                        .max(0)
                                                        .min(inner.height.saturating_sub(1) as i16),
                                                )
                                            })
                                            .unwrap_or((down_col, down_row));
                                        cmd_batch.push(format!("pane-mouse {} 0 {} {} M\n",
                                            pane_id, down_col, down_row));
                                        cmd_batch.push(format!("pane-mouse {} 0 {} {} m\n",
                                            pane_id, up_col, up_row));
                                    } else if client_copy_mode {
                                        // Send the release to the pane the drag started in,
                                        // even when the pointer ends outside its rect — the
                                        // server clamps and finalizes the yank.
                                        let target = copy_drag_pane.or_else(|| {
                                            client_pane_rects.iter().find(|(_, r)| {
                                                r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                            }).map(|&(id, r)| (id, r))
                                        });
                                        if let Some((pane_id, pane_rect)) = target {
                                            let rel_col = me.column as i16 - pane_rect.x as i16;
                                            let rel_row = me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16;
                                            cmd_batch.push(format!("pane-mouse {} 0 {} {} m\n",
                                                pane_id, rel_col, rel_row));
                                        }
                                    } else {
                                        cmd_batch.push(format!("mouse-up {} {}\n", me.column, me.row));
                                    }
                                }
                                // The drag gesture is over in every branch above.
                                copy_drag_pane = None;
                                copy_drag_last = None;
                                copy_drag_repeat_at = None;
                            }
                            MouseEventKind::Up(MouseButton::Right) => {}
                            MouseEventKind::Up(MouseButton::Middle) => {}
                            MouseEventKind::Moved => {
                                // A bare motion event means the left button is no longer
                                // held.  If a copy-mode drag was being tracked, its release
                                // was swallowed (e.g. it happened outside the terminal
                                // window): finalize the gesture with a synthetic release so
                                // the selection yanks, and stop the auto-scroll repeat.
                                if copy_drag_pane.is_some() {
                                    if client_copy_mode {
                                        if let (Some((pane_id, _)), Some((rc, rr))) = (copy_drag_pane, copy_drag_last) {
                                            cmd_batch.push(format!("pane-mouse {} 0 {} {} m\n",
                                                pane_id, rc, rr));
                                        }
                                    }
                                    copy_drag_pane = None;
                                    copy_drag_last = None;
                                    copy_drag_repeat_at = None;
                                }
                                // Detect border hover for visual preview
                                let mut new_hover: Option<(u16, String, Rect)> = None;
                                if !client_zoomed {
                                    let tol = 0u16;
                                    for (_, bkind, _, bpos, _, _, barea) in &client_borders {
                                        let hit = if bkind == "Horizontal" {
                                            me.column >= bpos.saturating_sub(tol) && me.column <= bpos + tol
                                            && me.row >= barea.y && me.row < barea.y + barea.height
                                        } else {
                                            me.row >= bpos.saturating_sub(tol) && me.row <= bpos + tol
                                            && me.column >= barea.x && me.column < barea.x + barea.width
                                        };
                                        if hit {
                                            new_hover = Some((*bpos, bkind.clone(), *barea));
                                            break;
                                        }
                                    }
                                }
                                if new_hover != hovered_border {
                                    hovered_border = new_hover;
                                    selection_changed = true; // trigger redraw
                                }
                                // Forward hover to PTY
                                if let Some(&(pane_id, pane_rect)) = client_pane_rects.iter().find(|(_, r)| {
                                    r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                }) {
                                    let rel_col = me.column as i16 - pane_rect.x as i16;
                                    let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);
                                    cmd_batch.push(format!("pane-mouse {} 35 {} {} M\n",
                                        pane_id, rel_col, rel_row));
                                } else {
                                    cmd_batch.push(format!("mouse-move {} {}\n", me.column, me.row));
                                }
                            }
                            MouseEventKind::ScrollUp => {
                                rsel_start = None;
                                rsel_end = None;
                                rsel_dragged = false;
                                selection_changed = true;
                                if let Some(&(pane_id, pane_rect)) = client_pane_rects.iter().find(|(_, r)| {
                                    r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                }) {
                                    let rel_col = me.column as i16 - pane_rect.x as i16;
                                    let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);
                                    cmd_batch.push(format!("pane-scroll {} up {} {}\n", pane_id, rel_col, rel_row));
                                } else {
                                    cmd_batch.push(format!("scroll-up {} {}\n", me.column, me.row));
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                rsel_start = None;
                                rsel_end = None;
                                rsel_dragged = false;
                                selection_changed = true;
                                if let Some(&(pane_id, pane_rect)) = client_pane_rects.iter().find(|(_, r)| {
                                    r.contains(ratatui::layout::Position { x: me.column, y: me.row })
                                }) {
                                    let rel_col = me.column as i16 - pane_rect.x as i16;
                                    let rel_row = (me.row as i16 - pane_content_inner(pane_rect, &client_border_status, &client_border_format).y as i16).max(0);
                                    cmd_batch.push(format!("pane-scroll {} down {} {}\n", pane_id, rel_col, rel_row));
                                } else {
                                    cmd_batch.push(format!("scroll-down {} {}\n", me.column, me.row));
                                }
                            }
                            _ => {}
                        }
                    }
                    Event::FocusGained => {
                        cmd_batch.push("focus-in\n".into());
                    }
                    Event::FocusLost => {
                        cmd_batch.push("focus-out\n".into());
                        // No more mouse events will arrive; don't keep
                        // auto-scrolling a copy-mode drag on the timer alone.
                        copy_drag_repeat_at = None;
                    }
                    _ => {}
                }
                if quit { break; }
                _pending_evt = input.try_read()?;
            }
        }
        if quit { break; }

        // ── Windows zero-latency typing flush (post-event) ─────────────
        // After exhausting all available events, if paste_pend has 1-2
        // chars and no paste sequence is in progress, flush immediately
        // as send-text.  This eliminates the 20ms detection window delay
        // for normal typing while preserving paste detection:
        //   • ConPTY clipboard injection writes all chars atomically, so
        //     paste_pend will already have 3+ chars after the event batch.
        //   • 1-2 char clipboard pastes already flush as send-text in the
        //     20ms path — early flush produces identical behaviour.
        //   • Stage2 / paste_confirmed states block this path.
        //
        // When paste-detection is OFF, flush ALL pending chars immediately
        // regardless of count.  This bypasses the 20ms/300ms staging that
        // would otherwise wrap clipboard-injected characters in bracketed
        // paste even though the user explicitly disabled paste detection.
        #[cfg(windows)]
        {
            if should_zero_latency_flush_paste_pend(
                &paste_pend,
                paste_detection_enabled,
                paste_confirmed,
                paste_stage2,
            ) {
                if input_log_enabled() {
                    input_log("paste", &format!(
                        "zero-latency flush {} char(s) as typing",
                        paste_pend.len()));
                }
                for c in paste_pend.chars() {
                    match c {
                        '\n' => { cmd_batch.push("send-key enter\n".into()); }
                        '\t' => { cmd_batch.push("send-key tab\n".into()); }
                        ' '  => { cmd_batch.push("send-key space\n".into()); }
                        _ => {
                            let escaped = match c {
                                '"' => "\\\"".to_string(),
                                '\\' => "\\\\".to_string(),
                                _ => c.to_string(),
                            };
                            cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                        }
                    }
                }
                paste_pend.clear();
                paste_pend_start = None;
            }
        }

        // ── Windows paste buffer flush (post-event) ────────────────────
        // If Ctrl+V Release was seen in this iteration AND we have pending
        // chars, immediately send as send-paste (don't wait for top-of-loop).
        #[cfg(windows)]
        {
            if paste_confirmed && !paste_pend.is_empty() {
                if input_log_enabled() {
                    input_log("paste", &format!("paste CONFIRMED (post-event), sending {} chars as send-paste: {:?}",
                        paste_pend.len(), &paste_pend.chars().take(200).collect::<String>()));
                }
                let encoded = base64_encode(&paste_pend);
                cmd_batch.push(format!("send-paste {}\n", encoded));
                paste_pend.clear();
                paste_pend_start = None;
                paste_stage2 = false;
                paste_confirmed = false;
                // Suppress subsequent char accumulation and clipboard-read
                // fallback — the paste was already delivered.
                paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
            } else if paste_confirmed && paste_pend.is_empty() {
                // Ctrl+V Release with no buffered chars.  If paste was
                // already sent via stage2 timeout or Event::Paste, the
                // suppress window prevents a redundant clipboard read.
                let suppressed = paste_suppress_until
                    .map_or(false, |t| Instant::now() < t);
                if !suppressed {
                    // No recent paste — read clipboard as fallback
                    if let Some(text) = read_from_system_clipboard() {
                        if !text.is_empty() {
                            if input_log_enabled() {
                                input_log("paste", &format!("paste CONFIRMED (no buffer), clipboard read len={}", text.len()));
                            }
                            let encoded = base64_encode(&text);
                            cmd_batch.push(format!("send-paste {}\n", encoded));
                            // Suppress subsequent char accumulation — the
                            // clipboard chars may arrive later (async inject)
                            // and would cause a duplicate paste via stage2.
                            paste_suppress_until = Some(Instant::now() + Duration::from_millis(200));
                        }
                    }
                }
                paste_confirmed = false;
            }
        }

        // ── STEP 2: Send commands immediately, refresh screen at capped rate ──
        // Send client-size if changed
        let mut size_changed = false;
        {
            let ts = terminal.size()?;
            let new_size = (ts.width, ts.height.saturating_sub(last_status_lines));
            if new_size != last_sent_size {
                last_sent_size = new_size;
                size_changed = true;
                if writer.write_all(format!("client-size {} {}\n", new_size.0, new_size.1).as_bytes()).is_err() {
                    break; // Connection lost
                }
                // Re-send mouse-enable on resize — the terminal may reset
                // mouse reporting after a window size change.  A resize is
                // the known trigger for Windows Terminal dropping a local
                // client's mouse registration, so this runs in every input
                // mode (the keepalive routes per mode), not just SSH.
                crate::ssh_input::send_mouse_keepalive();
                last_mouse_enable = Instant::now();
            }
        }

        // Timer-driven repeat-window expiry (issue #371).
        // A repeatable (-r) prefix binding arms `prefix_repeating` but does NOT
        // send `prefix-end` — it stays armed so a follow-up repeat key is caught.
        // The key-event path (above) only expires this on the NEXT keystroke, so
        // with no further key the prefix-active state (and #{client_prefix}) would
        // stay stuck on indefinitely. This runs every poll tick, so once
        // repeat-time elapses with no repeat key we disarm and emit prefix-end,
        // matching tmux where the indicator clears when the repeat window closes.
        if prefix_armed && prefix_repeating
            && prefix_armed_at.elapsed().as_millis() >= repeat_time_ms as u128
        {
            prefix_armed = false;
            prefix_repeating = false;
            #[cfg(windows)]
            if ime_was_open { crate::platform::ime_restore(); ime_was_open = false; }
            cmd_batch.push("prefix-end\n".into());
        }

        // ── Copy-mode drag auto-scroll repeat ──────────────────────────
        // While the left button is held with the pointer dwelling at/past
        // the top or bottom row of the copy-mode pane, re-send the last drag
        // position every 50ms so the server keeps scrolling the view (tmux
        // repeats its drag timer at the same cadence).  The state clears on
        // mouse-up / bare motion / focus loss; gating on client_copy_mode
        // also pauses the repeat harmlessly if a stale dump briefly flips
        // the flag right after a normal-mode handoff.
        if client_copy_mode {
            if let (Some((pane_id, _)), Some((rc, rr)), Some(due)) =
                (copy_drag_pane, copy_drag_last, copy_drag_repeat_at)
            {
                if Instant::now() >= due {
                    cmd_batch.push(format!("pane-mouse {} 32 {} {} M\n", pane_id, rc, rr));
                    copy_drag_repeat_at = Some(Instant::now() + COPY_DRAG_REPEAT);
                }
            }
        }

        // Send all batched commands immediately — keys reach the server
        // without waiting for a dump-state round-trip
        let sent_keys_this_iter = !cmd_batch.is_empty();
        if sent_keys_this_iter {
            if input_log_enabled() {
                for cmd in &cmd_batch {
                    input_log("send", &format!("→ {}", cmd.trim()));
                }
            }
            for cmd in &cmd_batch {
                if writer.write_all(cmd.as_bytes()).is_err() {
                    break; // Connection lost
                }
            }
            let _ = writer.flush(); // push keys to server NOW
            // #604: a batch that is nothing but bare pointer motion is not a
            // keystroke.  It produces no echo to chase, and when no pane is
            // tracking motion it changes no server state at all, so neither the
            // typing fast-poll nor an immediate dump-state is warranted.
            // Treating every pointer sample as a keystroke pulled a full state
            // frame (~12 KB) and repainted the whole screen at pointer-motion
            // rate, which is what the reporter saw as the cursor flickering
            // while the mouse merely moved.  If the motion DOES reach an app,
            // the server pushes the resulting output frame on its own (see the
            // "Server auto-pushes frames when state changes" note below).
            let only_bare_motion = cmd_batch.iter().all(|c| is_bare_motion_cmd(c));
            if !only_bare_motion {
                last_key_send_time = Some(Instant::now());
                key_send_instant = Some(Instant::now());
                // Force immediate dump-state so we start the echo-detection
                // polling chain right away (eliminates 0-10ms initial wait).
                force_dump = true;
            }
        }

        // ── STEP 2b: Request screen update (non-blocking) ────────────────
        // Rate-limit dump-state requests to avoid flooding the server.
        // dump_in_flight prevents >1 concurrent request; the interval check
        // ensures we don't re-request faster than ~100fps when typing.
        let overlays_active = command_input || renaming || pane_renaming || tree_chooser || buffer_chooser || session_chooser || keys_viewer || confirm_cmd.is_some() || srv_popup_active || srv_confirm_active || srv_menu_active || srv_display_panes || clock_active;
        let should_dump = if force_dump || size_changed {
            true
        } else if typing_active {
            since_dump >= 10  // ~100fps cap when typing (matches poll_ms)
        } else {
            // Server auto-pushes frames when state changes (PTY output,
            // new window, etc.) — no idle dump-state polling needed.
            // This saves CPU + bandwidth: no 50-100KB JSON roundtrips
            // when the client is just sitting idle.
            false
        };
        if should_dump && !dump_in_flight {
            if writer.write_all(b"dump-state\n").is_err() { break; }
            if writer.flush().is_err() { break; }
            dump_in_flight = true;
            dump_flight_start = Instant::now();
        }

        // ── STEP 3: Render if we have a frame ────────────────────────────
        // Also render if selection changed (for highlight overlay) even without new frame
        // Always render when overlays are active (command prompt, rename, choosers)
        if !got_frame && !selection_changed && !overlays_active {
            continue;
        }

        // Skip parse + render when the raw JSON is identical to the previous
        // frame AND selection hasn't changed AND no overlays are active.
        if dump_buf == prev_dump_buf && !selection_changed && !overlays_active {
            last_dump_time = Instant::now();
            continue;
        }

        // Parse the frame (use prev_dump_buf for selection-only redraws)
        let frame_to_parse = if got_frame && dump_buf != prev_dump_buf { &dump_buf } else { &prev_dump_buf };
        let _t_parse = Instant::now();
        let state: DumpState = match serde_json::from_str(frame_to_parse) {
            Ok(s) => s,
            Err(_e) => {
                client_log("parse", &format!("JSON parse error: {} (len={})", _e, frame_to_parse.len()));
                force_dump = true;
                selection_changed = false;
                continue;
            }
        };
        let _parse_us = _t_parse.elapsed().as_micros();
        if client_log_enabled() {
            client_log("parse", &format!("OK in {}us, {} windows", _parse_us, state.windows.len()));
        }

        let root = state.layout;
        let windows = state.windows;
        // Track the active window name for command-prompt -I '#W' expansion
        if let Some(aw) = windows.iter().find(|w| w.active) {
            active_window_name = aw.name.clone();
        }
        last_tree = state.tree;
        let base_index = state.base_index;
        client_copy_mode = active_pane_in_copy_mode(&root);
        client_pwsh_selection = state.pwsh_mouse_selection;
        client_mouse_selection = state.mouse_selection;
        client_mouse_selection_force = state.mouse_selection_force;
        #[cfg(windows)]
        { paste_detection_enabled = state.paste_detection; }
        choose_tree_preview_default = state.choose_tree_preview;
        client_zoomed = state.zoomed;
        let dim_preds = state.prediction_dimming;
        clock_active = state.clock_mode;
        clock_colour_str = state.clock_colour;
        let state_cursor_style_code = state.cursor_style_code;
        // Server-side overlay state (update persistent variables)
        srv_floats = state.floats;
        srv_popup_active = state.popup_active;
        srv_popup_command = state.popup_command.unwrap_or_default();
        srv_popup_width = state.popup_width.unwrap_or(80);
        srv_popup_height = state.popup_height.unwrap_or(24);
        srv_popup_lines = state.popup_lines;
        let srv_popup_rows_new = state.popup_rows;
        srv_popup_rows = srv_popup_rows_new;
        let new_popup_has_pty = state.popup_has_pty;
        if !srv_popup_active || new_popup_has_pty != srv_popup_has_pty {
            srv_popup_scroll = 0;
        }
        srv_popup_has_pty = new_popup_has_pty;
        // A PTY popup owns the cursor while it is up; a static popup has no
        // editable cursor and must not steal it from the pane underneath.
        srv_popup_cursor = match (srv_popup_active && new_popup_has_pty && !state.popup_hide_cursor,
                                  state.popup_cursor_col, state.popup_cursor_row) {
            (true, Some(cc), Some(cr)) => Some((cc, cr)),
            _ => None,
        };
        srv_confirm_active = state.confirm_active;
        srv_confirm_prompt = state.confirm_prompt.unwrap_or_default();
        srv_menu_active = state.menu_active;
        srv_menu_title = state.menu_title.unwrap_or_default();
        srv_menu_selected = state.menu_selected;
        srv_menu_items = state.menu_items;
        srv_display_panes = state.display_panes;
        srv_pane_base_index = state.pane_base_index;
        srv_customize_active = state.customize_active;
        srv_customize_selected = state.customize_selected;
        srv_customize_scroll = state.customize_scroll;
        srv_customize_editing = state.customize_editing;
        srv_customize_cursor = state.customize_cursor;
        srv_customize_edit_buf = state.customize_edit_buf.unwrap_or_default();
        srv_customize_filter = state.customize_filter.unwrap_or_default();
        srv_customize_options = state.customize_options;
        // Drop any pending digit-jump buffer when the picker is closed,
        // or while the user is mid-edit on an option (digits there are
        // edits to the value, not jumps to a row).
        if !srv_customize_active || srv_customize_editing {
            customize_num_buffer.clear();
        }

        // ── Extract active pane's cursor state ──────────────────────
        // We collect cursor info here but DON'T use
        // f.set_cursor_position() inside the draw callback for the
        // normal (non-copy-mode) active pane.  Instead we write
        // cursor show/hide + position + style as ONE atomic write
        // after terminal.draw().  This prevents ratatui's separate
        // execute!(..., Show/Hide) flushes from creating intermediate
        // states visible to Windows Terminal between vsync frames,
        // which causes rapid cursor flicker during high-frequency
        // output (e.g. opencode streaming).
        let mut post_draw_cursor: Option<(u16, u16)> = None; // pane-local (col, row)
        {
            fn active_cursor_info(node: &LayoutJson) -> Option<(bool, u16, u16, bool)> {
                match node {
                    LayoutJson::Leaf { active, hide_cursor, cursor_row, cursor_col, copy_mode, .. } => {
                        if *active { Some((*hide_cursor, *cursor_row, *cursor_col, *copy_mode)) } else { None }
                    }
                    LayoutJson::Split { children, .. } => {
                        children.iter().find_map(active_cursor_info)
                    }
                }
            }
            if let Some((hide, cr, cc, copy)) = active_cursor_info(&root) {
                if !hide && !clock_active && !copy {
                    post_draw_cursor = Some((cc, cr));
                }
            }
        }

        // ── OSC 52: propagate server-side clipboard to local terminal ────
        // When the server copies text (yank_selection / copy mode),
        // it includes a one-shot clipboard_osc52 field in the dump.
        // Buffer for emission after terminal.draw() to avoid corrupting
        // ratatui's output.
        if let Some(ref clip_b64) = state.clipboard_osc52 {
            if let Some(clip_text) = crate::util::base64_decode(clip_b64) {
                // Also set the local Win32 clipboard for non-SSH scenarios
                copy_to_system_clipboard(&clip_text);
                pending_osc52 = Some(clip_text);
            }
        }

        // ── Audible bell: forward BEL to host terminal ──────────────
        if state.bell {
            pending_bell = true;
        }

        // ── set-titles: capture host title for post-draw OSC 0 emit ──
        // The server has already expanded set-titles-string, so we
        // just compare against the last value we emitted and write a
        // new OSC 0 sequence if it has changed.  Stored as a local so
        // it survives `state` being moved into its other fields below.
        let host_title_this_frame: Option<String> = state.host_title.clone();
        // Issue #269: capture host_progress for post-draw OSC 9;4 emit.
        let host_progress_this_frame: Option<String> = state.host_progress.clone();

        // Update prefix key from server config (if provided)
        if let Some(ref prefix_str) = state.prefix {
            if let Some((kc, km)) = parse_key_string(prefix_str) {
                if (kc, km) != prefix_key {
                    prefix_key = (kc, km);
                    // Compute raw control character for Ctrl+<letter> prefix
                    prefix_raw_char = if km.contains(KeyModifiers::CONTROL) {
                        if let KeyCode::Char(c) = kc {
                            Some((c as u8 & 0x1f) as char)
                        } else { None }
                    } else { None };
                }
            }
        }

        // Update prefix2 key from server config (if provided)
        if let Some(ref prefix2_str) = state.prefix2 {
            if !prefix2_str.is_empty() {
                if let Some((kc, km)) = parse_key_string(prefix2_str) {
                    prefix2_key = Some((kc, km));
                    prefix2_raw_char = if km.contains(KeyModifiers::CONTROL) {
                        if let KeyCode::Char(c) = kc {
                            Some((c as u8 & 0x1f) as char)
                        } else { None }
                    } else { None };
                }
            } else {
                prefix2_key = None;
                prefix2_raw_char = None;
            }
        }

        // Update status-style from server config (if provided)
        if let Some(ref ss) = state.status_style {
            if !ss.is_empty() {
                let (fg, bg, bold) = parse_tmux_style_components(ss);
                status_fg = fg.unwrap_or(Color::Black);
                // #372: fall back to the terminal default rather than bright
                // green when a bg fails to parse, so any future parse miss is
                // invisible instead of alarming.
                status_bg = bg.unwrap_or(Color::Reset);
                status_bold = bold;
            }
        }

        // Sync key bindings from server
        if !state.bindings.is_empty() || !synced_bindings.is_empty() {
            synced_bindings = state.bindings;
        }
        defaults_suppressed = state.defaults_suppressed;
        scroll_enter_copy_mode = state.scroll_enter_copy_mode;
        // Sync repeat-time from server
        repeat_time_ms = state.repeat_time;
        // Sync bold-is-bright (issue #425) into the console writer's atomic.
        // The writer lives in this client process, so the server-side option
        // value must be pushed here for the rewrite toggle to take effect.
        crate::platform::set_bold_is_bright(state.bold_is_bright);
        // Update status-left / status-right from server (already format-expanded)
        if let Some(sl) = state.status_left {
            // Pass full string — visual truncation is handled by ratatui
            // when rendering into the allocated status bar area.
            // Do NOT naively truncate by char count as that can split
            // inside #[...] style directives, causing parse failures.
            // Allow empty values so conditionals like #{?client_prefix,...,}
            // can clear the status area when the condition becomes false.
            custom_status_left = if sl.is_empty() { None } else { Some(sl) };
        }
        if let Some(sr) = state.status_right {
            custom_status_right = if sr.is_empty() { None } else { Some(sr) };
        }
        let status_lines = if state.status_visible { state.status_lines } else { 0 };
        // If server's status_lines changed, re-send client-size with the
        // correct content-area height so the server's pane rects match the
        // client's render area exactly.
        let new_sl = (status_lines as u16).max(1);
        if new_sl != last_status_lines {
            last_status_lines = new_sl;
            // Force a client-size re-send on the next iteration
            last_sent_size = (0, 0);
        }
        let status_format = state.status_format;
        // Update pane border styles
        if let Some(ref pbs) = state.pane_border_style {
            if !pbs.is_empty() {
                let (fg, _bg, _bold) = parse_tmux_style_components(pbs);
                if let Some(c) = fg { pane_border_fg = c; }
            }
        }
        if let Some(ref pabs) = state.pane_active_border_style {
            if !pabs.is_empty() {
                let (fg, _bg, _bold) = parse_tmux_style_components(pabs);
                if let Some(c) = fg { pane_active_border_fg = c; }
            }
        }
        if let Some(ref pbhs) = state.pane_border_hover_style {
            if !pbhs.is_empty() {
                let (fg, _bg, _bold) = parse_tmux_style_components(pbhs);
                if let Some(c) = fg { pane_border_hover_fg = c; }
            }
        }
        // Update window-status-format strings
        if let Some(ref f) = state.wsf { if !f.is_empty() { win_status_fmt = f.clone(); } }
        if let Some(ref f) = state.wscf { if !f.is_empty() { win_status_current_fmt = f.clone(); } }
        if let Some(ref s) = state.wss { win_status_sep = s.clone(); }
        // Update window-status styles
        if let Some(ref s) = state.ws_style {
            if !s.is_empty() {
                win_status_style = Some(parse_tmux_style_components(s));
            }
        }
        if let Some(ref s) = state.wsc_style {
            if !s.is_empty() {
                win_status_current_style = Some(parse_tmux_style_components(s));
            }
        }
        // Update mode-style, status-position, status-justify from server
        if let Some(ref ms) = state.mode_style {
            if !ms.is_empty() { mode_style_str = ms.clone(); }
        }
        if let Some(ref sp) = state.status_position {
            if !sp.is_empty() { status_position_str = sp.clone(); }
        }
        if let Some(ref sj) = state.status_justify {
            if !sj.is_empty() { status_justify_str = sj.clone(); }
        }

        // ── STEP 3: Render ───────────────────────────────────────────────
        let sel_s = rsel_start;
        let sel_e = rsel_end;
        let sel_rect = rsel_pane_rect;
        let sel_pwsh = client_pwsh_selection;
        let sel_block = rsel_block;
        let status_at_top = status_position_str == "top";
        if client_log_enabled() {
            let sz = terminal.size().unwrap_or_default();
            client_log("draw", &format!("pre-draw terminal_size={}x{}", sz.width, sz.height));
        }
        // Reset OSC 8 hyperlink runs collected during this frame's render (#361).
        frame_hyperlinks_clear();
        terminal.draw(|f| {
            let area = f.area();
            let constraints = if status_at_top {
                vec![Constraint::Length(status_lines as u16), Constraint::Min(1)]
            } else {
                vec![Constraint::Min(1), Constraint::Length(status_lines as u16)]
            };
            let chunks = Layout::default().direction(Direction::Vertical)
                .constraints(constraints).split(area);
            let (content_chunk, status_chunk) = if status_at_top {
                (chunks[1], chunks[0])
            } else {
                (chunks[0], chunks[1])
            };

            client_content_area = content_chunk;
            client_pane_rects.clear();
            collect_pane_rects(&root, content_chunk, &mut client_pane_rects);
            client_pane_view_offsets.clear();
            collect_view_offsets(&root, &mut client_pane_view_offsets);
            client_borders.clear();
            let mut border_path = Vec::new();
            collect_layout_borders(&root, content_chunk, &mut border_path, &mut client_borders);

            let active_rect = compute_active_rect_json_zoom_aware(&root, content_chunk, state.zoomed);
            let clock_col = clock_colour_str.as_deref().map(|s| map_color(s)).unwrap_or(Color::Cyan);
            let border_status = state.pane_border_status.as_deref().unwrap_or("off");
            // tmux defaults pane-border-format to a non-empty value, so enabling
            // pane-border-status alone renders the pane title. Match that default
            // when unset/empty (else the label gate skips and the border is blank) (#414).
            let border_format = match state.pane_border_format.as_deref() {
                Some(s) if !s.is_empty() => s,
                _ => "#{pane_index} \"#{pane_title}\"",
            };
            // Publish for the post-draw cursor and next frame's mouse handlers.
            client_border_status = border_status.to_string();
            client_border_format = border_format.to_string();
            // O(N) per frame but pane counts are small in practice (typically < 20).
            let total_panes = if state.zoomed { 1 } else { root.count_leaves() };
            let bchars = crate::border_lines::border_chars(
                state.pane_border_lines.as_deref().unwrap_or(crate::border_lines::DEFAULT));
            // copy-mode-line-numbers: build the gutter render config from the
            // option value + active pane scrollback size shipped in state.
            let copy_ln = {
                let mode = crate::copy_line_numbers::CopyLnMode::parse(
                    state.copy_mode_line_numbers.as_deref().unwrap_or(crate::copy_line_numbers::DEFAULT));
                if mode.is_active() {
                    let num_style = state.copy_mode_line_number_style.as_deref()
                        .map(crate::style::parse_tmux_style).unwrap_or_else(|| Style::default().fg(Color::DarkGray));
                    let cur_style = state.copy_mode_current_line_number_style.as_deref()
                        .map(crate::style::parse_tmux_style).unwrap_or_else(|| Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
                    Some(CopyLnRender { mode, hsize: state.copy_hsize, num_style, cur_style })
                } else { None }
            };
            render_layout_json(f, &root, content_chunk, dim_preds, pane_border_fg, pane_active_border_fg, clock_active, clock_col, active_rect, &mode_style_str, state.zoomed, border_status, border_format, total_panes, bchars, copy_ln);
            let border_mask = border_mask_from_layout(&root, content_chunk, f.buffer_mut().area, state.zoomed);
            fix_border_intersections(f.buffer_mut(), bchars, &border_mask);
            // render_json and fix_border_intersections can leave inconsistent styles
            // at intersections and along edges shared by nested splits.
            if let Some(ar) = active_rect {
                let buf = f.buffer_mut();
                let w = buf.area.width as usize;
                let border_style = Style::default().fg(pane_border_fg);
                let active_style = Style::default().fg(pane_active_border_fg);
                for &idx in &border_mask {
                    if idx >= buf.content.len() { continue; }
                    let ch = buf.content[idx].symbol().chars().next().unwrap_or(' ');
                    // Only re-color junction characters. The straight │ and ─ separators
                    // are now already colored correctly per-cell by render_layout_json based
                    // on adjacency, so re-coloring them here would clobber that work for
                    // 3+ pane layouts where a separator borders both active and inactive panes.
                    if !matches!(ch, '┼' | '├' | '┤' | '┬' | '┴') { continue; }
                    let x = buf.area.x + (idx % w) as u16;
                    let y = buf.area.y + (idx / w) as u16;
                    let adj = (x + 1 == ar.x && y >= ar.y && y < ar.y + ar.height)
                        || (x == ar.x + ar.width && y >= ar.y && y < ar.y + ar.height)
                        || (y + 1 == ar.y && x >= ar.x && x < ar.x + ar.width)
                        || (y == ar.y + ar.height && x >= ar.x && x < ar.x + ar.width)
                        || ((x + 1 == ar.x || x == ar.x + ar.width) && (y + 1 == ar.y || y == ar.y + ar.height));
                    buf.content[idx].set_style(if adj { active_style } else { border_style });
                }
            }

            // Highlight the border under the cursor to preview what a drag would move.
            if let Some((hpos, ref hkind, harea)) = hovered_border {
                let buf = f.buffer_mut();
                let w = buf.area.width as usize;
                let hover_style = Style::default().fg(pane_border_hover_fg);
                if hkind == "Horizontal" {
                    // Vertical separator line at column hpos, spanning harea's height
                    let col = hpos as usize;
                    if col >= buf.area.x as usize && col < (buf.area.x + buf.area.width) as usize {
                        for y in harea.y..harea.y + harea.height {
                            let idx = (y - buf.area.y) as usize * w + (col - buf.area.x as usize);
                            if idx < buf.content.len() {
                                buf.content[idx].set_style(hover_style);
                            }
                        }
                    }
                } else {
                    // Horizontal separator line at row hpos, spanning harea's width
                    let row = hpos as usize;
                    if row >= buf.area.y as usize && row < (buf.area.y + buf.area.height) as usize {
                        for x in harea.x..harea.x + harea.width {
                            let idx = (row - buf.area.y as usize) * w + (x - buf.area.x) as usize;
                            if idx < buf.content.len() {
                                buf.content[idx].set_style(hover_style);
                            }
                        }
                    }
                }
            }

            // ── Left-click drag text selection overlay ────────────────
            // Suppress the client-side blue selection overlay when the
            // server is in copy mode – the server draws its own themed
            // selection and the blue overlay would hide everything.
            if let (Some(s), Some(e)) = (sel_s, sel_e) {
            if !active_pane_in_copy_mode(&root) {
                let (r0, c0, r1, c1) = normalize_selection(s, e, sel_block);
                // pwsh-mouse-selection: clip intermediate rows to the
                // originating pane so they never bleed into neighbours.
                // Legacy mode: full terminal width on intermediate rows.
                let (pane_left, pane_right) = if sel_pwsh {
                    if let Some(r) = sel_rect {
                        (r.x, r.x + r.width.saturating_sub(1))
                    } else {
                        (0, area.width.saturating_sub(1))
                    }
                } else {
                    (0, area.width.saturating_sub(1))
                };
                let buf = f.buffer_mut();
                let buf_area = buf.area;
                for row in r0..=r1 {
                    let col_start = if sel_block {
                        c0.max(pane_left)
                    } else if row == r0 { c0.max(pane_left) } else { pane_left };
                    let col_end = if sel_block {
                        c1.min(pane_right)
                    } else if row == r1 { c1.min(pane_right) } else { pane_right };
                    if col_start > col_end { continue; }
                    for col in col_start..=col_end {
                        if row < buf_area.height && col < buf_area.width {
                            let idx = (row - buf_area.y) as usize * buf_area.width as usize
                                + (col - buf_area.x) as usize;
                            if idx < buf.content.len() {
                                let style = if sel_pwsh {
                                    Style::default().fg(Color::Black).bg(Color::White)
                                } else {
                                    Style::default().fg(Color::Black).bg(Color::LightCyan)
                                };
                                buf.content[idx].set_style(style);
                            }
                        }
                    }
                }
            } // !active_pane_in_copy_mode
            } // if let sel_s, sel_e

            if session_chooser {
                let sel_style = crate::rendering::parse_tmux_style(&mode_style_str);
                let filtered_indices = session_filtered_indices(&session_entries, &session_filter);
                // Popup size: when preview is OFF use the original
                // pre-#257 dynamic sizing (compact, list-only). When preview
                // is ON expand to 85x75% so the right-side preview has room.
                let jump_rows: u16 = if session_num_buffer.is_empty() { 0 } else { 2 };
                let filter_rows: u16 = if session_filter_active { 2 } else { 0 };
                let buffer_rows = jump_rows.saturating_add(filter_rows);
                let avail_w = content_chunk.width;
                let avail_h = content_chunk.height;
                let (popup_w, popup_h) = if preview_enabled {
                    let want_w = ((avail_w as u32 * 85) / 100) as u16;
                    let want_h = ((avail_h as u32 * 75) / 100) as u16;
                    (want_w.max(40).min(avail_w), want_h.max(10).min(avail_h))
                } else {
                    let sess_h = (filtered_indices.len() as u16)
                        .saturating_add(2)
                        .saturating_add(buffer_rows)
                        .max(5)
                        .min(content_chunk.height.saturating_sub(2));
                    let pw = ((avail_w as u32 * 70) / 100) as u16;
                    (pw.max(20).min(avail_w), sess_h)
                };
                let base_x = content_chunk.x + (avail_w.saturating_sub(popup_w)) / 2;
                let base_y = content_chunk.y + (avail_h.saturating_sub(popup_h)) / 2;
                let max_dx = (avail_w.saturating_sub(popup_w)) as i32 / 2;
                let max_dy = (avail_h.saturating_sub(popup_h)) as i32 / 2;
                let dx = popup_offset.0.clamp(-max_dx, max_dx);
                let dy = popup_offset.1.clamp(-max_dy, max_dy);
                let oa = Rect {
                    x: ((base_x as i32) + dx).max(content_chunk.x as i32) as u16,
                    y: ((base_y as i32) + dy).max(content_chunk.y as i32) as u16,
                    width: popup_w,
                    height: popup_h,
                };
                popup_rect_last = Some(oa);
                let title = if preview_enabled {
                    " choose-session (f=filter, digits+enter=jump, enter=switch, x=kill, p=preview, esc=clear/close, drag border to move) "
                } else {
                    " choose-session (f=filter, digits+enter=jump, enter=switch, x=kill, p=preview, esc=clear/close) "
                };
                let overlay = Block::default().borders(Borders::ALL).title(title).border_style(sel_style);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);

                // Split inner area: list on the left, preview on the right.
                // `p` toggles the preview pane off — when off the list takes
                // the full inner width.
                let list_w = if !preview_enabled {
                    inner.width
                } else if inner.width >= 60 {
                    (inner.width * 40 / 100).max(30).min(inner.width.saturating_sub(30))
                } else {
                    inner.width
                };
                let list_area = Rect { x: inner.x, y: inner.y, width: list_w, height: inner.height };
                let preview_area = if preview_enabled && inner.width > list_w + 1 {
                    Some(Rect {
                        x: inner.x + list_w + 1,
                        y: inner.y,
                        width: inner.width - list_w - 1,
                        height: inner.height,
                    })
                } else { None };

                // Reserve rows for the active jump and filter indicators.
                let reserved = buffer_rows as usize;
                let visible_h = (list_area.height as usize).saturating_sub(reserved);
                if visible_h > 0 && session_selected >= session_scroll + visible_h {
                    session_scroll = session_selected.saturating_sub(visible_h - 1);
                }
                if session_selected < session_scroll {
                    session_scroll = session_selected;
                }
                let num_width = filtered_indices.len().max(1).to_string().len();
                let mut lines: Vec<Line> = Vec::new();
                for (visible_idx, entry_idx) in filtered_indices.iter().copied().enumerate().skip(session_scroll).take(visible_h) {
                    let (sname, info) = &session_entries[entry_idx];
                    let marker = if sname == &current_session { "*" } else { " " };
                    let row = format!("{:>w$}. {} {}", visible_idx + 1, marker, info, w = num_width);
                    let line = if visible_idx == session_selected {
                        Line::from(Span::styled(row, sel_style))
                    } else {
                        Line::from(row)
                    };
                    lines.push(line);
                }
                if filtered_indices.is_empty() && visible_h > 0 {
                    lines.push(Line::from("(no matching sessions)"));
                }
                if !session_num_buffer.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("go to {}", session_num_buffer),
                        sel_style,
                    )));
                }
                if session_filter_active {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("Filter: {}", session_filter),
                        sel_style,
                    )));
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, list_area);

                if let Some(parea) = preview_area {
                    let sep_x = inner.x + list_w;
                    for yy in inner.y..(inner.y + inner.height) {
                        let sep = Paragraph::new(Span::styled("│", Style::default().fg(Color::DarkGray)));
                        f.render_widget(sep, Rect { x: sep_x, y: yy, width: 1, height: 1 });
                    }
                    // Issue #257 follow-up: render the first window of the
                    // highlighted session with its full split layout.
                    let mut rendered = false;
                    if let Some((sname, _info)) = filtered_indices
                        .get(session_selected)
                        .and_then(|idx| session_entries.get(*idx))
                    {
                        // Resolve first window id via cached list-tree fetch.
                        let lt_key = format!("__lt__\t{}", sname);
                        let win_id = if let Some((cached, ts)) = preview_cache.get(&lt_key) {
                            if ts.elapsed() < crate::preview::PREVIEW_TTL {
                                cached.parse::<usize>().ok()
                            } else { None }
                        } else { None };
                        let win_id = win_id.or_else(|| {
                            let port_path = crate::paths::port_file(sname);
                            let port: u16 = std::fs::read_to_string(&port_path).ok()?.trim().parse().ok()?;
                            let key = crate::session::read_session_key(sname).ok()?;
                            let resp = crate::session::fetch_authed_response_multi(
                                &format!("127.0.0.1:{}", port),
                                &key,
                                b"list-tree\n",
                                Duration::from_millis(150),
                                Duration::from_millis(300),
                            )?;
                            let wins: Vec<WinTree> = serde_json::from_str(resp.trim()).ok()?;
                            let first = wins.first()?;
                            preview_cache.insert(lt_key, (first.id.to_string(), Instant::now()));
                            Some(first.id)
                        });

                        if let Some(wid) = win_id {
                            if let Some(layout) = crate::preview::get_or_fetch_dump(
                                &mut dump_cache, sname, wid,
                            ) {
                                crate::preview::render_dump_tree(
                                    f,
                                    &layout,
                                    parea,
                                    pane_border_fg,
                                    pane_active_border_fg,
                                    None,
                                );
                                rendered = true;
                            }
                        }
                    }

                    if !rendered {
                        // Fallback to single-pane preview if the layout
                        // endpoint is unavailable.
                        let preview_text: Option<String> = filtered_indices
                            .get(session_selected)
                            .and_then(|idx| session_entries.get(*idx))
                            .and_then(|(sname, _info)| {
                                let lt_key = format!("__lt__\t{}", sname);
                                let win_id = if let Some((cached, ts)) = preview_cache.get(&lt_key) {
                                    if ts.elapsed() < crate::preview::PREVIEW_TTL {
                                        cached.parse::<usize>().ok()
                                    } else { None }
                                } else { None };
                                let wid = win_id?;
                                crate::preview::get_or_fetch(&mut preview_cache, sname, wid, usize::MAX)
                            });
                        let pv: Vec<Line> = match preview_text {
                            Some(t) => crate::preview::parse_ansi_lines(&t, parea.width, parea.height),
                            None => vec![Line::from("(no preview available)")],
                        };
                        let pv_para = Paragraph::new(Text::from(pv));
                        f.render_widget(pv_para, parea);
                    }
                }
                // Scroll position indicator (when content overflows)
                if filtered_indices.len() > visible_h {
                    let max_scroll = filtered_indices.len().saturating_sub(visible_h);
                    let pct = if max_scroll > 0 { session_scroll * 100 / max_scroll } else { 0 };
                    let indicator = if session_scroll == 0 {
                        "Top".to_string()
                    } else if session_scroll >= max_scroll {
                        "Bot".to_string()
                    } else {
                        format!("{}%", pct)
                    };
                    let ind_len = indicator.len() as u16;
                    if oa.width > ind_len + 2 {
                        let ind_x = oa.x + oa.width - ind_len - 2;
                        let ind_y = oa.y + oa.height - 1;
                        let ind_rect = Rect::new(ind_x, ind_y, ind_len, 1);
                        let ind_para = Paragraph::new(Span::styled(indicator, Style::default().fg(Color::DarkGray)));
                        f.render_widget(ind_para, ind_rect);
                    }
                }
            }
            if tree_chooser {
                let sel_style = crate::rendering::parse_tmux_style(&mode_style_str);
                // Popup size: when preview is OFF use the original
                // pre-#257 dynamic sizing (compact, list-only). When preview
                // is ON expand to 85x75% so the right-side preview has room.
                let buffer_rows: u16 = if tree_num_buffer.is_empty() { 0 } else { 2 };
                let avail_w = content_chunk.width;
                let avail_h = content_chunk.height;
                let (popup_w, popup_h) = if preview_enabled {
                    let want_w = ((avail_w as u32 * 85) / 100) as u16;
                    let want_h = ((avail_h as u32 * 75) / 100) as u16;
                    (want_w.max(40).min(avail_w), want_h.max(10).min(avail_h))
                } else {
                    let tree_h = ((tree_entries.len() as u16).saturating_add(2).saturating_add(buffer_rows))
                        .max(5)
                        .min(content_chunk.height.saturating_sub(2));
                    let pw = ((avail_w as u32 * 60) / 100) as u16;
                    (pw.max(20).min(avail_w), tree_h)
                };
                let base_x = content_chunk.x + (avail_w.saturating_sub(popup_w)) / 2;
                let base_y = content_chunk.y + (avail_h.saturating_sub(popup_h)) / 2;
                // Apply drag offset, clamped so the popup stays fully on-screen.
                let max_dx = (avail_w.saturating_sub(popup_w)) as i32 / 2;
                let max_dy = (avail_h.saturating_sub(popup_h)) as i32 / 2;
                let dx = popup_offset.0.clamp(-max_dx, max_dx);
                let dy = popup_offset.1.clamp(-max_dy, max_dy);
                let oa = Rect {
                    x: ((base_x as i32) + dx).max(content_chunk.x as i32) as u16,
                    y: ((base_y as i32) + dy).max(content_chunk.y as i32) as u16,
                    width: popup_w,
                    height: popup_h,
                };
                popup_rect_last = Some(oa);
                let title = if preview_enabled {
                    " choose-tree (digits+enter=jump  Enter=switch  p=preview  Esc=close  drag border to move) "
                } else {
                    " choose-tree (digits+enter=jump  Enter=switch  p=preview  Esc=close) "
                };
                let overlay = Block::default().borders(Borders::ALL).title(title).border_style(sel_style);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);

                // Split inner area: list on the left, preview on the right.
                // When preview is toggled off (`p`), use full width for the list.
                let list_w = if !preview_enabled {
                    inner.width
                } else if inner.width >= 60 {
                    (inner.width * 40 / 100).max(28).min(inner.width.saturating_sub(30))
                } else {
                    inner.width
                };
                let list_area = Rect { x: inner.x, y: inner.y, width: list_w, height: inner.height };
                let preview_area = if preview_enabled && inner.width > list_w + 1 {
                    Some(Rect {
                        x: inner.x + list_w + 1,
                        y: inner.y,
                        width: inner.width - list_w - 1,
                        height: inner.height,
                    })
                } else { None };

                let visible_h = (list_area.height as usize).saturating_sub(buffer_rows as usize);
                if visible_h > 0 && tree_selected >= tree_scroll + visible_h {
                    tree_scroll = tree_selected.saturating_sub(visible_h - 1);
                }
                if tree_selected < tree_scroll {
                    tree_scroll = tree_selected;
                }
                let num_width = tree_entries.len().to_string().len();
                let mut lines: Vec<Line> = Vec::new();
                for (i, (is_win, wid, _pid, label, _sess)) in tree_entries.iter().enumerate().skip(tree_scroll).take(visible_h) {
                    // Right-aligned 1-based row number so the digit-jump
                    // mapping is visible without trial and error.
                    let row = format!("{:>w$}. {}", i + 1, label, w = num_width);
                    let line = if i == tree_selected {
                        Line::from(Span::styled(row, sel_style))
                    } else if *is_win && *wid == usize::MAX {
                        // Session header — bold
                        Line::from(Span::styled(row, Style::default().add_modifier(Modifier::BOLD)))
                    } else {
                        Line::from(row)
                    };
                    lines.push(line);
                }
                if !tree_num_buffer.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("go to {}", tree_num_buffer),
                        sel_style,
                    )));
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, list_area);

                // Vertical separator + preview pane
                if let Some(parea) = preview_area {
                    // Draw vertical separator at column inner.x + list_w
                    let sep_x = inner.x + list_w;
                    for yy in inner.y..(inner.y + inner.height) {
                        let sep = Paragraph::new(Span::styled("│", Style::default().fg(Color::DarkGray)));
                        f.render_widget(sep, Rect { x: sep_x, y: yy, width: 1, height: 1 });
                    }
                    // Determine target session/window/pane for the preview.
                    // Issue #257 follow-up: if the highlighted entry is a
                    // window (or a session header), render the *whole*
                    // window with its real split layout, mirroring tmux's
                    // window_tree_draw_window. Pane-level entries still
                    // render the single pane.
                    let sel = tree_entries.get(tree_selected).cloned();
                    let mut rendered = false;
                    if let Some((is_win, wid, pid, _label, sess)) = sel {
                        // Resolve target window id: session header => first window
                        // in that session from tree_entries.
                        let target_win: Option<usize> = if is_win && wid == usize::MAX {
                            tree_entries.iter()
                                .find(|(iw, w, _p, _l, s)| *iw && *w != usize::MAX && s == &sess)
                                .map(|(_, w, _p, _l, _s)| *w)
                        } else if is_win {
                            Some(wid)
                        } else {
                            // pane entry: still render the whole window
                            Some(wid)
                        };

                        if let Some(twid) = target_win {
                            if let Some(layout) = crate::preview::get_or_fetch_dump(
                                &mut dump_cache, &sess, twid,
                            ) {
                                // Highlight which pane the user is hovering on
                                // (for pane-level entries). Active pane gets a
                                // brighter separator anyway.
                                let highlight_pid = if !is_win { Some(pid) } else { None };
                                crate::preview::render_dump_tree(
                                    f,
                                    &layout,
                                    parea,
                                    pane_border_fg,
                                    pane_active_border_fg,
                                    highlight_pid,
                                );
                                rendered = true;
                            }
                        }

                        if !rendered {
                            // Fallback: single-pane preview (session not
                            // reachable, or no layout returned).
                            let preview_text: Option<String> = if is_win && wid == usize::MAX {
                                tree_entries.iter()
                                    .find(|(iw, w, _p, _l, s)| *iw && *w != usize::MAX && s == &sess)
                                    .and_then(|(_, w, _p, _l, s)| crate::preview::get_or_fetch(&mut preview_cache, s, *w, usize::MAX))
                            } else if is_win {
                                crate::preview::get_or_fetch(&mut preview_cache, &sess, wid, usize::MAX)
                            } else {
                                crate::preview::get_or_fetch(&mut preview_cache, &sess, wid, pid)
                            };
                            let pv: Vec<Line> = match preview_text {
                                Some(t) => crate::preview::parse_ansi_lines(&t, parea.width, parea.height),
                                None => vec![Line::from("(no preview available)")],
                            };
                            let pv_para = Paragraph::new(Text::from(pv));
                            f.render_widget(pv_para, parea);
                        }
                    }
                }
                // Scroll position indicator (when content overflows)
                if tree_entries.len() > visible_h {
                    let max_scroll = tree_entries.len().saturating_sub(visible_h);
                    let pct = if max_scroll > 0 { tree_scroll * 100 / max_scroll } else { 0 };
                    let indicator = if tree_scroll == 0 {
                        "Top".to_string()
                    } else if tree_scroll >= max_scroll {
                        "Bot".to_string()
                    } else {
                        format!("{}%", pct)
                    };
                    let ind_len = indicator.len() as u16;
                    if oa.width > ind_len + 2 {
                        let ind_x = oa.x + oa.width - ind_len - 2;
                        let ind_y = oa.y + oa.height - 1;
                        let ind_rect = Rect::new(ind_x, ind_y, ind_len, 1);
                        let ind_para = Paragraph::new(Span::styled(indicator, Style::default().fg(Color::DarkGray)));
                        f.render_widget(ind_para, ind_rect);
                    }
                }
            }
            if buffer_chooser {
                let sel_style = crate::rendering::parse_tmux_style(&mode_style_str);
                let overlay = Block::default().borders(Borders::ALL)
                    .title(" choose-buffer (digits+enter=jump, Enter=paste, d=delete, q/Esc=close) ")
                    .border_style(sel_style);
                let buffer_rows: u16 = if buffer_num_buffer.is_empty() { 0 } else { 2 };
                let buf_h = ((buffer_entries.len() as u16).saturating_add(2).saturating_add(buffer_rows))
                    .max(5)
                    .min(content_chunk.height.saturating_sub(2));
                let oa = centered_rect(70, buf_h, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);
                let visible_h = (inner.height as usize).saturating_sub(buffer_rows as usize);
                if visible_h > 0 && buffer_selected >= buffer_scroll + visible_h {
                    buffer_scroll = buffer_selected.saturating_sub(visible_h - 1);
                }
                if buffer_selected < buffer_scroll {
                    buffer_scroll = buffer_selected;
                }
                let num_width = buffer_entries.len().to_string().len();
                let mut lines: Vec<Line> = Vec::new();
                for (i, (idx, byte_len, preview)) in buffer_entries.iter().enumerate().skip(buffer_scroll).take(visible_h) {
                    // 1-based jump-row number on the left, then the existing
                    // tmux-style "bufferN: M bytes: ..." label.
                    let label = format!("{:>w$}. buffer{}: {} bytes: \"{}\"",
                        i + 1, idx, byte_len, preview, w = num_width);
                    let line = if i == buffer_selected {
                        Line::from(Span::styled(label, sel_style))
                    } else {
                        Line::from(label)
                    };
                    lines.push(line);
                }
                if !buffer_num_buffer.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("go to {}", buffer_num_buffer),
                        sel_style,
                    )));
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, inner);
                // Scroll position indicator (when content overflows)
                if buffer_entries.len() > visible_h {
                    let max_scroll = buffer_entries.len().saturating_sub(visible_h);
                    let pct = if max_scroll > 0 { buffer_scroll * 100 / max_scroll } else { 0 };
                    let indicator = if buffer_scroll == 0 {
                        "Top".to_string()
                    } else if buffer_scroll >= max_scroll {
                        "Bot".to_string()
                    } else {
                        format!("{}%", pct)
                    };
                    let ind_len = indicator.len() as u16;
                    if oa.width > ind_len + 2 {
                        let ind_x = oa.x + oa.width - ind_len - 2;
                        let ind_y = oa.y + oa.height - 1;
                        let ind_rect = Rect::new(ind_x, ind_y, ind_len, 1);
                        let ind_para = Paragraph::new(Span::styled(indicator, Style::default().fg(Color::DarkGray)));
                        f.render_widget(ind_para, ind_rect);
                    }
                }
            }
            if keys_viewer {
                // Proportional overlay: 90% width, up to 80% height
                let avail_h = content_chunk.height;
                let overlay_h = (avail_h * 80 / 100).max(5).min(avail_h.saturating_sub(2));
                let overlay = Block::default().borders(Borders::ALL)
                    .title(" list-keys (q/Esc=close, Up/Down/PgUp/PgDn=scroll) ");
                let oa = centered_rect(90, overlay_h, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);
                let visible_h = inner.height as usize;
                // Clamp scroll so we don't scroll past the end
                let max_scroll = keys_viewer_lines.len().saturating_sub(visible_h);
                if keys_viewer_scroll > max_scroll { keys_viewer_scroll = max_scroll; }
                let mut lines: Vec<Line> = Vec::new();
                for (_i, entry) in keys_viewer_lines.iter().enumerate().skip(keys_viewer_scroll).take(visible_h) {
                    // Highlight section headers, "bind-key" keyword, and plain text differently
                    if entry.starts_with("──") || entry.starts_with("── ") {
                        lines.push(Line::from(Span::styled(entry.clone(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
                    } else if let Some(rest) = entry.strip_prefix("bind-key") {
                        lines.push(Line::from(vec![
                            Span::styled("bind-key", Style::default().fg(Color::Green)),
                            Span::raw(rest.to_string()),
                        ]));
                    } else {
                        lines.push(Line::from(entry.clone()));
                    }
                }
                // Show scroll indicator in bottom-right
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, inner);
                // Scroll position indicator
                if keys_viewer_lines.len() > visible_h {
                    let pct = if max_scroll == 0 { 100 } else { keys_viewer_scroll * 100 / max_scroll };
                    let indicator = if keys_viewer_scroll == 0 {
                        "Top".to_string()
                    } else if keys_viewer_scroll >= max_scroll {
                        "Bot".to_string()
                    } else {
                        format!("{}%", pct)
                    };
                    let ind_len = indicator.len() as u16;
                    if oa.width > ind_len + 2 {
                        let ind_x = oa.x + oa.width - ind_len - 2;
                        let ind_y = oa.y + oa.height - 1;
                        let ind_rect = Rect::new(ind_x, ind_y, ind_len, 1);
                        let ind_para = Paragraph::new(Span::styled(indicator, Style::default().fg(Color::DarkGray)));
                        f.render_widget(ind_para, ind_rect);
                    }
                }
            }
            let sb_fg = status_fg;
            let sb_bg = status_bg;
            let sb_base = if status_bold {
                Style::default().fg(sb_fg).bg(sb_bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(sb_fg).bg(sb_bg)
            };
            // ── Build three separate span groups: left, tabs, right ──
            use unicode_width::UnicodeWidthStr;
            // If status_format[0] is set, use it for line 0 instead of the default 3-part layout
            let use_status_format_0 = status_format.len() > 0 && !status_format[0].is_empty();
            // Left portion: custom status_left or default [session] prefix
            let left_prefix = match custom_status_left {
                Some(ref sl) => sl.clone(),
                None => format!("[{}] ", name),
            };
            if client_log_enabled() {
                client_log("status", &format!("parsing left_prefix ({} chars): [{}]",
                    left_prefix.len(), left_prefix.chars().take(100).collect::<String>()));
            }
            // #451: status-left-style is layered over the base status style so
            // inline #[...] overrides in status-left still win. Dropped in the
            // app.rs->client.rs modularization; restored here.
            let left_base = match state.status_left_style.as_deref() {
                Some(s) if !s.is_empty() => sb_base.patch(crate::rendering::parse_tmux_style(s)),
                _ => sb_base,
            };
            let mut left_spans: Vec<Span> = crate::rendering::parse_inline_styles(&left_prefix, left_base);

            // Window tabs (the window list)
            let mut tab_spans_all: Vec<Span> = Vec::new();
            let mut tab_rel_positions: Vec<(usize, u16, u16)> = Vec::new();
            let mut tab_cursor: u16 = 0;
            for (i, w) in windows.iter().enumerate() {
                let tab_text = if !w.tab_text.is_empty() {
                    w.tab_text.clone()
                } else {
                    let display_idx = i + base_index;
                    let fmt = if w.active { &win_status_current_fmt } else { &win_status_fmt };
                    fmt.replace("#I", &display_idx.to_string())
                       .replace("#W", &w.name)
                       .replace("#F", if w.active { "*" } else { "" })
                };
                if i > 0 {
                    // Parse inline styles in separator (e.g. "#[fg=#44475a]|")
                    let sep_spans = crate::rendering::parse_inline_styles(&win_status_sep, sb_base);
                    let sep_w: u16 = sep_spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16).sum();
                    tab_spans_all.extend(sep_spans);
                    tab_cursor += sep_w;
                }
                // Normal (non-current) window-status-style, used as the base for
                // flagged windows and the fallback for plain ones.
                let normal_style = if let Some((fg, bg, bold)) = win_status_style {
                    let mut s = Style::default();
                    if let Some(c) = fg { s = s.fg(c); }
                    if let Some(c) = bg { s = s.bg(c); }
                    if bold { s = s.add_modifier(Modifier::BOLD); }
                    s
                } else {
                    sb_base
                };
                // #451: restore the tmux flag-style priority that the monolithic
                // app.rs renderer had (current > bell > activity > last > normal).
                // The modularization dropped bell/last entirely and hard-coded the
                // activity colour, so window-status-{activity,bell,last}-style had
                // no effect. Each flagged style is layered over the normal style so
                // an option that only sets fg keeps the normal bg, and full
                // attributes like `reverse` (the activity/bell default) work.
                let flag_style = |raw: &Option<String>| -> Option<Style> {
                    match raw.as_deref() {
                        Some(s) if !s.is_empty() => Some(normal_style.patch(crate::rendering::parse_tmux_style(s))),
                        _ => None,
                    }
                };
                let fallback_style = if w.active {
                    if let Some((fg, bg, bold)) = win_status_current_style {
                        let mut s = Style::default();
                        if let Some(c) = fg { s = s.fg(c); }
                        if let Some(c) = bg { s = s.bg(c); }
                        if bold { s = s.add_modifier(Modifier::BOLD); }
                        s
                    } else {
                        sb_base
                    }
                } else if w.bell {
                    flag_style(&state.wsb_style).unwrap_or(normal_style)
                } else if w.activity {
                    flag_style(&state.wsa_style).unwrap_or(normal_style)
                } else if w.last {
                    flag_style(&state.wsl_style).unwrap_or(normal_style)
                } else {
                    normal_style
                };
                let parsed = crate::rendering::parse_inline_styles(&tab_text, fallback_style);
                let tab_start = tab_cursor;
                let tab_w: u16 = parsed.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref()) as u16).sum();
                tab_cursor += tab_w;
                tab_rel_positions.push((w.idx, tab_start, tab_cursor));
                tab_spans_all.extend(parsed);
            }

            // Right portion
            let right_text = custom_status_right.as_deref().unwrap_or("").to_string();
            if client_log_enabled() {
                client_log("status", &format!("parsing right_text ({} chars): [{}]",
                    right_text.len(), right_text.chars().take(100).collect::<String>()));
            }
            // #451: status-right-style, layered like status-left-style above.
            let right_base = match state.status_right_style.as_deref() {
                Some(s) if !s.is_empty() => sb_base.patch(crate::rendering::parse_tmux_style(s)),
                _ => sb_base,
            };
            let mut right_spans = crate::rendering::parse_inline_styles(&right_text, right_base);

            // Enforce status-left-length / status-right-length truncation (tmux parity)
            crate::style::truncate_spans_to_width(&mut left_spans, state.status_left_length);
            crate::style::truncate_spans_to_width(&mut right_spans, state.status_right_length);

            // Measure widths using Unicode display width
            let left_w: usize = left_spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
            let tabs_w: usize = tab_spans_all.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
            let right_w: usize = right_spans.iter().map(|s| UnicodeWidthStr::width(s.content.as_ref())).sum();
            let total_width = status_chunk.width as usize;

            // Assemble final spans based on status-justify
            let mut status_spans: Vec<Span> = Vec::new();
            match status_justify_str.as_str() {
                "centre" | "center" => {
                    // Centre: [left] [pad1] [tabs] [pad2] [right]
                    // Tabs are centred in the space between left and right.
                    let avail = total_width.saturating_sub(left_w).saturating_sub(right_w);
                    let pad_before = avail.saturating_sub(tabs_w) / 2;
                    let pad_after = avail.saturating_sub(tabs_w).saturating_sub(pad_before);
                    status_spans.extend(left_spans);
                    if pad_before > 0 { status_spans.push(Span::styled(" ".repeat(pad_before), sb_base)); }
                    status_spans.extend(tab_spans_all);
                    if pad_after > 0 { status_spans.push(Span::styled(" ".repeat(pad_after), sb_base)); }
                    status_spans.extend(right_spans);
                }
                "absolute-centre" | "absolute-center" => {
                    // Absolute-centre: tabs centred on the total terminal width
                    let tabs_start = total_width.saturating_sub(tabs_w) / 2;
                    status_spans.extend(left_spans);
                    let pad_before = tabs_start.saturating_sub(left_w);
                    if pad_before > 0 { status_spans.push(Span::styled(" ".repeat(pad_before), sb_base)); }
                    status_spans.extend(tab_spans_all);
                    let used = left_w + pad_before + tabs_w;
                    let pad_after = total_width.saturating_sub(used).saturating_sub(right_w);
                    if pad_after > 0 { status_spans.push(Span::styled(" ".repeat(pad_after), sb_base)); }
                    status_spans.extend(right_spans);
                }
                "right" => {
                    // Right: [left] [pad] [tabs] [right]
                    status_spans.extend(left_spans);
                    let used = left_w + tabs_w + right_w;
                    let pad = total_width.saturating_sub(used);
                    if pad > 0 { status_spans.push(Span::styled(" ".repeat(pad), sb_base)); }
                    status_spans.extend(tab_spans_all);
                    status_spans.extend(right_spans);
                }
                _ => {
                    // Left (default): [left] [tabs] [pad] [right]
                    status_spans.extend(left_spans);
                    status_spans.extend(tab_spans_all);
                    let used = left_w + tabs_w + right_w;
                    let pad = total_width.saturating_sub(used);
                    if pad > 0 { status_spans.push(Span::styled(" ".repeat(pad), sb_base)); }
                    status_spans.extend(right_spans);
                }
            }
            // Compute absolute tab positions based on status-justify layout
            let tabs_x_offset: u16 = status_chunk.x + match status_justify_str.as_str() {
                "centre" | "center" => {
                    let avail = total_width.saturating_sub(left_w).saturating_sub(right_w);
                    let pad_before = avail.saturating_sub(tabs_w) / 2;
                    (left_w + pad_before) as u16
                }
                "absolute-centre" | "absolute-center" => {
                    let tabs_start = total_width.saturating_sub(tabs_w) / 2;
                    tabs_start as u16
                }
                "right" => {
                    let used = left_w + tabs_w + right_w;
                    let pad = total_width.saturating_sub(used);
                    (left_w + pad) as u16
                }
                _ => left_w as u16, // "left" default
            };
            client_tab_positions = tab_rel_positions.iter()
                .map(|&(idx, s, e)| (status_chunk.y, idx, s + tabs_x_offset, e + tabs_x_offset))
                .collect();
            client_status_row = status_chunk.y;
            client_status_rows = status_chunk.height;
            client_window_indices = windows.iter().map(|w| w.idx).collect();
            // Truncate overall status line to fit the available width
            crate::style::truncate_spans_to_width(&mut status_spans, total_width);
            // If a display-message is active, show it on the status bar
            // instead of the normal status content (tmux parity).
            // Uses message-style (default: bg=yellow,fg=black) matching tmux.
            let status_bar = if let Some(ref msg) = state.status_message {
                // #372: honor the configured message-style (sent expanded by the
                // server) instead of a hard-coded colour. Falls back to tmux's
                // default bg=yellow,fg=black when unset/empty.
                let msg_style = crate::rendering::parse_tmux_style(
                    match state.message_style.as_deref() {
                        Some(s) if !s.is_empty() => s,
                        _ => "bg=yellow,fg=black",
                    }
                );
                let padded = if msg.len() < status_chunk.width as usize {
                    format!("{}{}", msg, " ".repeat(status_chunk.width as usize - msg.len()))
                } else {
                    msg.chars().take(status_chunk.width as usize).collect()
                };
                Paragraph::new(Line::from(Span::styled(padded, msg_style))).style(msg_style)
            } else {
                Paragraph::new(Line::from(status_spans)).style(sb_base)
            };
            f.render_widget(Clear, status_chunk);
            // Render the first status line (line 0)
            let line0_area = Rect { x: status_chunk.x, y: status_chunk.y, width: status_chunk.width, height: 1.min(status_chunk.height) };
            if use_status_format_0 && state.status_message.is_none() {
                // status-format[0] overrides the default left+tabs+right layout.
                // Use the layout engine to handle #[align], #[fill], #[list], #[range].
                let layout = crate::style::layout_format_line(
                    &status_format[0], total_width, sb_base,
                );
                // Update tab positions from range info so mouse clicks work
                // with custom status-format layouts. tmux parity (#593): the
                // range argument IS the window index — tmux passes it raw to
                // winlink_find_by_index (style.c strtonum, server-client.c
                // STYLE_RANGE_WINDOW) with no base-index arithmetic, so
                // `#[range=window|#{window_index}]` works at any base-index.
                client_tab_positions = layout.ranges.iter().filter_map(|(rt, s, e)| {
                    match rt {
                        crate::style::StatusRangeType::Window(idx) => {
                            Some((status_chunk.y, *idx, *s + status_chunk.x, *e + status_chunk.x))
                        }
                    }
                }).collect();
                let fmt0_widget = Paragraph::new(Line::from(layout.spans)).style(sb_base);
                f.render_widget(fmt0_widget, line0_area);
            } else {
                f.render_widget(status_bar, line0_area);
            }
            // Render additional status lines (index 1+) from status_format
            for line_idx in 1..status_lines {
                let line_y = status_chunk.y + line_idx as u16;
                if line_y >= status_chunk.y + status_chunk.height { break; }
                let line_area = Rect { x: status_chunk.x, y: line_y, width: status_chunk.width, height: 1 };
                let text = if line_idx < status_format.len() && !status_format[line_idx].is_empty() {
                    status_format[line_idx].clone()
                } else {
                    String::new()
                };
                // Use the layout engine for #[align], #[fill], #[list], #[range] support
                let layout = crate::style::layout_format_line(&text, line_area.width as usize, sb_base);
                // Harvest this line's clickable ranges too (#593): tmux keeps
                // one style_range list per status line (status.c status_redraw
                // fills sle->ranges for every i in 0..lines) so a window list
                // on status-format[1] is just as clickable as on line 0.
                for (rt, s, e) in &layout.ranges {
                    match rt {
                        crate::style::StatusRangeType::Window(idx) => {
                            client_tab_positions.push((line_y, *idx, *s + line_area.x, *e + line_area.x));
                        }
                    }
                }
                let line_widget = Paragraph::new(Line::from(layout.spans)).style(sb_base);
                f.render_widget(line_widget, line_area);
            }
            if renaming {
                let title = if session_renaming { "rename session" } else { "rename window" };
                let overlay = Block::default().borders(Borders::ALL).title(title);
                let oa = centered_rect(60, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("name: {}", rename_buf));
                f.render_widget(para, overlay.inner(oa));
            }
            if pane_renaming {
                let overlay = Block::default().borders(Borders::ALL).title("set pane title");
                let oa = centered_rect(60, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("title: {}", pane_title_buf));
                f.render_widget(para, overlay.inner(oa));
            }
            if command_input {
                let title = command_prompt_label.as_deref().unwrap_or("command");
                let overlay = Block::default().borders(Borders::ALL).title(title);
                let oa = centered_rect(60, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);
                let para = Paragraph::new(format!(": {}", command_buf));
                f.render_widget(para, inner);
                // Show cursor at the correct position within the prompt
                let cx = inner.x + 2 + command_cursor as u16; // +2 for ": "
                f.set_cursor_position((cx, inner.y));
            }
            if window_idx_input {
                let overlay = Block::default().borders(Borders::ALL).title("select window");
                let oa = centered_rect(50, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);
                let para = Paragraph::new(format!("index: {}", window_idx_buf));
                f.render_widget(para, inner);
                let cx = inner.x + 7 + window_idx_buf.len() as u16;
                f.set_cursor_position((cx, inner.y));
            }
            if let Some(ref cmd) = confirm_cmd {
                let overlay = Block::default().borders(Borders::ALL).title("confirm");
                let oa = centered_rect(50, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(format!("{}? (y/n)", cmd));
                f.render_widget(para, overlay.inner(oa));
            }

            // ── Server-side overlay rendering ────────────────────────
            // Floating panes (tmux new-pane) draw above the tiled layout but
            // below modal popups so a popup still stacks on top.
            if !srv_floats.is_empty() {
                render_float_overlays(f, content_chunk, &srv_floats);
            }
            if srv_popup_active {
                let popup_area = popup_overlay_rect(content_chunk, srv_popup_width, srv_popup_height);
                let w = popup_area.width;
                let title = if srv_popup_command.is_empty() { "Popup".to_string() } else { let max_title = (w as usize).saturating_sub(4); if srv_popup_command.len() > max_title { format!("{}...", &srv_popup_command[..max_title.saturating_sub(3)]) } else { srv_popup_command.clone() } };
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow))
                    .title(title);
                let inner_w = w.saturating_sub(2);
                let mut lines: Vec<Line<'static>> = Vec::new();
                if !srv_popup_rows.is_empty() {
                    // Render with full color/style data from popup_rows (#154)
                    for row_data in &srv_popup_rows {
                        let mut spans: Vec<Span<'static>> = Vec::new();
                        let mut col: u16 = 0;
                        for run in &row_data.runs {
                            if col >= inner_w { break; }
                            let fg = crate::style::map_color(&run.fg);
                            let bg = crate::style::map_color(&run.bg);
                            let mut style = Style::default().fg(fg).bg(bg);
                            if run.flags & 1  != 0 { style = style.add_modifier(Modifier::DIM); }
                            if run.flags & 2  != 0 { style = style.add_modifier(Modifier::BOLD); }
                            if run.flags & 4  != 0 { style = style.add_modifier(Modifier::ITALIC); }
                            if run.flags & 8  != 0 {
                    style = crate::rendering::with_underline(
                        style,
                        if run.ul == 0 { 1 } else { run.ul },
                        run.ulc.as_deref().map(map_color),
                    );
                }
                            if run.flags & 16 != 0 { style = style.add_modifier(Modifier::REVERSED); }
                            if run.flags & 32 != 0 { style = style.add_modifier(Modifier::SLOW_BLINK); }
                            if run.flags & 128 != 0 { style = style.add_modifier(Modifier::CROSSED_OUT); }
                            // ratatui-crossterm omits SGR 8 (HIDDEN), render as spaces
                            let text: &str = if run.flags & 64 != 0 {
                                " "
                            } else if run.text.is_empty() {
                                " "
                            } else {
                                &run.text
                            };
                            let run_w = run.width.max(1);
                            if col + run_w > inner_w {
                                let avail = (inner_w - col) as usize;
                                let truncated: String = text.chars().take(avail).collect();
                                if !truncated.is_empty() {
                                    spans.push(Span::styled(truncated, style));
                                }
                                col = inner_w;
                            } else {
                                spans.push(Span::styled(text.to_string(), style));
                                col += run_w;
                            }
                        }
                        lines.push(Line::from(spans));
                    }
                } else {
                    // Fallback: plain text lines for non-PTY popups
                    for line_str in &srv_popup_lines {
                        lines.push(Line::from(line_str.clone()));
                    }
                }
                let para = Paragraph::new(Text::from(lines)).block(block).scroll((srv_popup_scroll, 0));
                f.render_widget(Clear, popup_area);
                f.render_widget(para, popup_area);
            }
            if srv_confirm_active {
                let overlay = Block::default().borders(Borders::ALL).title("confirm");
                let oa = centered_rect(60, 3, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let para = Paragraph::new(srv_confirm_prompt.clone());
                f.render_widget(para, overlay.inner(oa));
            }
            if srv_menu_active {
                let sel_style = crate::rendering::parse_tmux_style(&mode_style_str);
                let title_str = if srv_menu_title.is_empty() { "Menu".to_string() } else { srv_menu_title.clone() };
                let overlay = Block::default().borders(Borders::ALL).title(title_str).border_style(sel_style);
                let item_count = srv_menu_items.len();
                let menu_h = ((item_count as u16).saturating_add(2)).max(3).min(content_chunk.height.saturating_sub(2));
                let oa = centered_rect(50, menu_h, content_chunk);
                f.render_widget(Clear, oa);
                f.render_widget(&overlay, oa);
                let inner = overlay.inner(oa);
                let mut lines: Vec<Line<'static>> = Vec::new();
                for (i, item) in srv_menu_items.iter().enumerate() {
                    if item.sep {
                        lines.push(Line::from("─".repeat(inner.width as usize)));
                    } else {
                        let name = item.name.clone().unwrap_or_default();
                        let key_str = item.key.clone().unwrap_or_default();
                        let label = if key_str.is_empty() { name } else { format!("{} ({})", name, key_str) };
                        if i == srv_menu_selected {
                            lines.push(Line::from(Span::styled(label, sel_style)));
                        } else {
                            lines.push(Line::from(label));
                        }
                    }
                }
                let para = Paragraph::new(Text::from(lines));
                f.render_widget(para, inner);
            }
            if srv_customize_active {
                // Full-screen overlay for customize-mode
                let area = content_chunk;
                let overlay = Rect {
                    x: area.x + 2,
                    y: area.y + 1,
                    width: area.width.saturating_sub(4).min(100),
                    height: area.height.saturating_sub(2),
                };
                f.render_widget(Clear, overlay);
                let header_style = Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD);
                let header = if srv_customize_filter.is_empty() {
                    " Customize Mode  [q:exit  /:filter  digits+Enter:jump  Enter:edit  d:reset default] "
                } else {
                    " Customize Mode  [q:exit  /:clear filter  digits+Enter:jump  Enter:edit  d:reset] "
                };
                if overlay.height > 0 {
                    let header_area = Rect { x: overlay.x, y: overlay.y, width: overlay.width, height: 1 };
                    let hdr = Paragraph::new(Line::from(Span::styled(
                        format!("{:<width$}", header, width = overlay.width as usize),
                        header_style,
                    )));
                    f.render_widget(hdr, header_area);
                }
                // Filter indicator
                let body_start = overlay.y + 1;
                if !srv_customize_filter.is_empty() && overlay.height > 1 {
                    let filter_area = Rect { x: overlay.x, y: body_start, width: overlay.width, height: 1 };
                    let filter_style = Style::default().fg(Color::Yellow).bg(Color::DarkGray);
                    let ftxt = format!(" Filter: {} ", srv_customize_filter);
                    f.render_widget(Paragraph::new(Line::from(Span::styled(
                        format!("{:<width$}", ftxt, width = overlay.width as usize), filter_style,
                    ))), filter_area);
                }
                let list_start = if srv_customize_filter.is_empty() { body_start } else { body_start + 1 };
                let list_height = overlay.y.saturating_add(overlay.height).saturating_sub(list_start) as usize;
                // Column header
                if list_height > 0 {
                    let col_hdr_area = Rect { x: overlay.x, y: list_start, width: overlay.width, height: 1 };
                    let col_style = Style::default().fg(Color::White).bg(Color::DarkGray).add_modifier(Modifier::BOLD);
                    let name_w = (overlay.width as usize / 2).max(20);
                    let col_text = format!(" {:<nw$} {}", "Option", "Value", nw = name_w.saturating_sub(2));
                    f.render_widget(Paragraph::new(Line::from(Span::styled(
                        format!("{:<width$}", col_text, width = overlay.width as usize), col_style,
                    ))), col_hdr_area);
                }
                let rows_start = list_start + 1;
                let rows_height = overlay.y.saturating_add(overlay.height).saturating_sub(rows_start) as usize;
                // Render visible option rows
                let visible_opts: Vec<&CustomizeOption> = srv_customize_options.iter()
                    .skip(srv_customize_scroll)
                    .take(rows_height)
                    .collect();
                let total_opts = srv_customize_options.len();
                let num_width = total_opts.to_string().len();
                for (row_idx, opt) in visible_opts.iter().enumerate() {
                    if rows_start + row_idx as u16 >= overlay.y + overlay.height { break; }
                    let row_area = Rect {
                        x: overlay.x,
                        y: rows_start + row_idx as u16,
                        width: overlay.width,
                        height: 1,
                    };
                    let is_selected = opt.i == srv_customize_selected;
                    let name_w = (overlay.width as usize / 2).max(20);
                    let scope_prefix = match opt.s.as_str() {
                        "server" => "[S] ",
                        "session" => "[s] ",
                        "window" => "[w] ",
                        "pane" => "[p] ",
                        _ => "    ",
                    };
                    // 1-based jump-row number prefix so the digit-jump
                    // mapping is visible.
                    let visible_pos = srv_customize_scroll + row_idx + 1;
                    let name_display = format!("{:>w$}. {}{}", visible_pos, scope_prefix, opt.n, w = num_width);
                    let value_display = if is_selected && srv_customize_editing {
                        let buf = &srv_customize_edit_buf;
                        format!("{}|", buf)
                    } else {
                        opt.v.clone()
                    };
                    let line_text = format!(" {:<nw$} {}", name_display, value_display, nw = name_w.saturating_sub(2));
                    let style = if is_selected {
                        if srv_customize_editing {
                            Style::default().fg(Color::Black).bg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::Black).bg(Color::White)
                        }
                    } else {
                        Style::default().fg(Color::White).bg(Color::Reset)
                    };
                    f.render_widget(Paragraph::new(Line::from(Span::styled(
                        format!("{:<width$}", line_text, width = overlay.width as usize), style,
                    ))), row_area);
                }
                // Digit-jump buffer indicator at the bottom of the overlay.
                if !customize_num_buffer.is_empty() && overlay.height >= 2 {
                    let ind_y = overlay.y + overlay.height.saturating_sub(1);
                    let ind_area = Rect { x: overlay.x, y: ind_y, width: overlay.width, height: 1 };
                    let ind_text = format!(" go to {} ", customize_num_buffer);
                    let ind_style = Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD);
                    f.render_widget(Paragraph::new(Line::from(Span::styled(
                        format!("{:<width$}", ind_text, width = overlay.width as usize), ind_style,
                    ))), ind_area);
                }
            }
            if srv_display_panes {
                // Render pane numbers overlay (like tmux display-panes)
                fn collect_leaf_rects(node: &LayoutJson, area: Rect, out: &mut Vec<Rect>) {
                    match node {
                        LayoutJson::Leaf { .. } => { out.push(area); }
                        LayoutJson::Split { kind, sizes, children } => {
                            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                                sizes.clone()
                            } else {
                                vec![(100 / children.len().max(1)) as u16; children.len()]
                            };
                            let is_horizontal = kind == "Horizontal";
                            let rects = crate::tree::split_with_gaps(is_horizontal, &effective_sizes, area);
                            for (i, child) in children.iter().enumerate() {
                                if i < rects.len() { collect_leaf_rects(child, rects[i], out); }
                            }
                        }
                    }
                }
                let mut leaf_rects = Vec::new();
                collect_leaf_rects(&root, content_chunk, &mut leaf_rects);
                for (idx, prect) in leaf_rects.iter().enumerate() {
                    if prect.width >= 7 && prect.height >= 3 {
                        let bw = 7u16; let bh = 3u16;
                        let bx = prect.x + prect.width.saturating_sub(bw) / 2;
                        let by = prect.y + prect.height.saturating_sub(bh) / 2;
                        let b = Rect { x: bx, y: by, width: bw, height: bh };
                        let pane_sel_style = Style::default().fg(Color::Yellow).bg(Color::Black).add_modifier(Modifier::BOLD);
                        let block = Block::default().borders(Borders::ALL).style(pane_sel_style);
                        let inner = block.inner(b);
                        let disp = ((idx + srv_pane_base_index) % 10).to_string();
                        let para = Paragraph::new(Line::from(Span::styled(
                            format!(" {} ", disp),
                            pane_sel_style,
                        ))).alignment(Alignment::Center);
                        f.render_widget(Clear, b);
                        f.render_widget(block, b);
                        f.render_widget(para, inner);
                    }
                }
            }

        })?;
        if client_log_enabled() {
            client_log("draw", &format!("draw OK, render={}us overlays: popup={} confirm={} menu={} display_panes={}",
                _t_parse.elapsed().as_micros().saturating_sub(_parse_us as u128),
                srv_popup_active, srv_confirm_active, srv_menu_active, srv_display_panes
            ));
        }

        // ── Post-draw: overlay OSC 8 hyperlinks (#361) ───────────────
        // ratatui's Buffer/Cell can't carry OSC 8, so after the frame is
        // drawn we re-emit just the hyperlinked runs wrapped in OSC 8 at
        // their screen positions, with cursor save/restore. This is a no-op
        // for the common case (no links), so it does not touch the hot path.
        {
            let runs = frame_hyperlinks_take();
            if !runs.is_empty() {
                let overlay = build_osc8_overlay(&runs);
                let mut out = std::io::stdout().lock();
                let _ = std::io::Write::write_all(&mut out, overlay.as_bytes());
                let _ = std::io::Write::flush(&mut out);
            }
        }

        // ── Post-draw: emit buffered OSC 52 clipboard ────────────────
        // Written AFTER terminal.draw() so it doesn't interfere with
        // ratatui's VT output buffer.
        if let Some(clip_text) = pending_osc52.take() {
            crate::copy_mode::emit_osc52(&mut std::io::stdout(), &clip_text);
        }

        // ── Post-draw: emit audible bell ─────────────────────────────
        if pending_bell {
            pending_bell = false;
            let _ = std::io::Write::write_all(&mut std::io::stdout(), b"\x07");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }

        // ── Post-draw: forward host terminal title (set-titles) ──────
        // OSC 0 = "set both icon name and window title" — broadly
        // supported by Windows Terminal, iTerm2, GNOME Terminal,
        // Konsole, xterm, and matches what tmux's `tsl` capability
        // emits on xterm-class terminals.  Only emit when the
        // expanded title actually changed to avoid flooding the host
        // terminal every frame.
        if host_title_this_frame != last_emitted_host_title {
            if let Some(ref title) = host_title_this_frame {
                use std::io::Write;
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(b"\x1b]0;");
                let _ = out.write_all(title.as_bytes());
                let _ = out.write_all(b"\x07");
                let _ = out.flush();
            }
            last_emitted_host_title = host_title_this_frame;
        }

        // ── Post-draw: forward OSC 9;4 progress (issue #269) ─────────
        // host_progress is "<state>;<value>" e.g. "1;50" (default, 50%) or
        // "0;0" (hide).  Re-emit ESC ] 9 ; 4 ; <state> ; <value> ESC \ to
        // the host terminal so Windows Terminal / iTerm2 / kitty render
        // the progress indicator that the pane app intended.
        if host_progress_this_frame != last_emitted_host_progress {
            if let Some(ref prog) = host_progress_this_frame {
                if let Some((s, v)) = prog.split_once(';') {
                    if !s.is_empty() && !v.is_empty()
                        && s.bytes().all(|b| b.is_ascii_digit())
                        && v.bytes().all(|b| b.is_ascii_digit())
                    {
                        use std::io::Write;
                        let mut out = std::io::stdout().lock();
                        let _ = out.write_all(b"\x1b]9;4;");
                        let _ = out.write_all(s.as_bytes());
                        let _ = out.write_all(b";");
                        let _ = out.write_all(v.as_bytes());
                        let _ = out.write_all(b"\x1b\\");
                        let _ = out.flush();
                    }
                }
            }
            last_emitted_host_progress = host_progress_this_frame;
        }

        // ── Periodic mouse-enable refresh (ALL input modes) ──────────
        // ConPTY or terminal resize can silently disable mouse reporting,
        // and Windows Terminal additionally drops a LOCAL client's mouse
        // registration outright (mouse dead, keys fine) until the DECSET
        // bytes are re-sent.  This used to run only in SSH mode, leaving
        // local sessions mouse-dead until detach/reattach; the keepalive
        // routes per input mode so it is safe everywhere.
        if last_mouse_enable.elapsed().as_secs() >= 30 {
            crate::ssh_input::send_mouse_keepalive();
            last_mouse_enable = Instant::now();
        }

        // ── Raw VT pipe: periodic terminal-size poll ───────────────
        // A pipe carries no resize notifications, so ask the terminal for
        // its text-area size (XTWINOPS). The reply flows through the VT
        // reader, which updates the backend size override; an unchanged
        // size is a no-op for terminal.autoresize().
        if crate::ssh_input::pipe_mode_active()
            && last_pipe_size_query.elapsed().as_millis() >= 1000
        {
            crate::ssh_input::request_pipe_terminal_size();
            last_pipe_size_query = Instant::now();
        }

        // ── Post-draw: atomic cursor write ──────────────────────────
        // Write cursor visibility + position + style as ONE batch to
        // avoid the separate execute!() flushes that ratatui's normal
        // show_cursor()/set_cursor_position() would produce.  Multiple
        // separate console writes create intermediate states visible
        // to WT between vsync frames, causing rapid cursor flicker.
        {
            use std::io::Write;
            fn find_active_cursor_shape(node: &LayoutJson) -> Option<u8> {
                match node {
                    LayoutJson::Leaf { active, cursor_shape, .. } => {
                        if *active && *cursor_shape >= 1 && *cursor_shape <= 6 { Some(*cursor_shape) } else { None }
                    }
                    LayoutJson::Split { children, .. } => {
                        children.iter().find_map(find_active_cursor_shape)
                    }
                }
            }
            let effective = find_active_cursor_shape(&root)
                .unwrap_or_else(|| state_cursor_style_code.unwrap_or_else(crate::rendering::configured_cursor_code));
            let content_chunk = {
                let sz = terminal.size().unwrap_or_default();
                let constraints = if status_at_top {
                    vec![Constraint::Length(status_lines as u16), Constraint::Min(1)]
                } else {
                    vec![Constraint::Min(1), Constraint::Length(status_lines as u16)]
                };
                let chunks = Layout::default().direction(Direction::Vertical)
                    .constraints(constraints).split(sz.into());
                if status_at_top { chunks[1] } else { chunks[0] }
            };
            let active_pane_area: Option<Rect> =
                compute_active_rect_json_zoom_aware(&root, content_chunk, client_zoomed);
            // Compute screen-global cursor position from pane-local coords.
            //
            // A PTY popup is modal and takes the keystrokes, so while it is up
            // the cursor belongs to the process inside it, not to the pane
            // underneath (tmux does the same in server_client_reset_state by
            // taking both the cursor and the cursor mode from the overlay).
            // Leaving it on the pane underneath is what made the popup look
            // like it had no cursor at all (#507).
            let cursor_visible = if srv_popup_active && srv_popup_has_pty {
                srv_popup_cursor.and_then(|c| {
                    popup_cursor_screen_pos(content_chunk, srv_popup_width, srv_popup_height, srv_popup_scroll, c)
                })
            } else if let (Some((cc, cr)), Some(outer)) = (post_draw_cursor, active_pane_area) {
                // Content lives inside the border-label reservation; use the render's inner rect.
                let inner = pane_content_inner(outer, &client_border_status, &client_border_format);
                let cy = inner.y + cr.min(inner.height.saturating_sub(1));
                let cx = inner.x + cc.min(inner.width.saturating_sub(1));
                Some((cx, cy))
            } else {
                None
            };
            // Build a single VT string with: ?25h + CUP + DECSCUSR
            // ratatui's draw() always emits ?25l (since we never call
            // f.set_cursor_position), so we must re-emit ?25h + CUP
            // every frame when the cursor should be visible.
            let mut buf = String::with_capacity(32);
            if let Some((cx, cy)) = cursor_visible {
                buf.push_str("\x1b[?25h");
                use std::fmt::Write as FmtWrite;
                let _ = write!(buf, "\x1b[{};{}H", cy + 1, cx + 1);
            }
            // DECSCUSR only when style actually changes (avoids blink
            // timer resets in WT).
            if effective != last_cursor_style {
                last_cursor_style = effective;
                use std::fmt::Write as FmtWrite;
                let _ = write!(buf, "\x1b[{} q", effective);
            }
            if !buf.is_empty() {
                let mut out = std::io::stdout().lock();
                let _ = out.write_all(buf.as_bytes());
                let _ = out.flush();
            }

            // Update Win32 system caret for accessibility / speech-to-text
            // tools (e.g. Wispr Flow).  Skip for SSH sessions — no local
            // console window.
            if !is_ssh_mode {
                if let Some((cx, cy)) = cursor_visible {
                    crate::platform::caret::update(cx, cy);
                }
            }
        }

        let _render_us = _t_parse.elapsed().as_micros().saturating_sub(_parse_us as u128);
        last_dump_time = Instant::now();
        // Latency log: measure full cycle from key-send to render-complete
        if let (Some(ref mut log), Some(ks)) = (&mut latency_log, key_send_instant) {
            let elapsed_ms = ks.elapsed().as_millis();
            loop_count += 1;
            use std::io::Write;
            let _ = writeln!(log, "L{}: key->render {}ms  parse={}us  render={}us  json_len={}  since_dump={}",
                loop_count, elapsed_ms, _parse_us, _render_us, dump_buf.len(), since_dump);
            // Only clear after we rendered a DIFFERENT frame (echo arrived)
            if got_frame && dump_buf != prev_dump_buf {
                let _ = writeln!(log, "L{}: ECHO VISIBLE after {}ms  (parse={}us render={}us)",
                    loop_count, elapsed_ms, _parse_us, _render_us);
                key_send_instant = None;
            }
        }
        selection_changed = false;
        // Cache this frame so we can skip identical re-renders.
        // Only update cache when we got a genuinely new frame (not selection-only redraw)
        if got_frame && dump_buf != prev_dump_buf {
            std::mem::swap(&mut prev_dump_buf, &mut dump_buf);
        }
        // DON'T clear last_key_send_time — keep fast-dumping for 100ms
        // after last keystroke so we catch the ConPTY echo promptly.
        // The timer expires naturally in the poll_ms calculation above.
        // Clear key_send_instant once echo arrives (frame differs).
        if got_frame && dump_buf != prev_dump_buf {
            key_send_instant = None;
        }
        force_dump = false;
    }

    // Clean disconnect on persistent connection
    let _ = writer.write_all(b"client-detach\n");
    let _ = writer.flush();
    // detach-client -P parity (issue #275): kill the parent shell so the host
    // terminal closes when the user explicitly requested it.
    if kill_parent_on_exit {
        #[cfg(windows)]
        {
            let _ = crate::platform::process_kill::kill_parent_process();
        }
    }
    Ok(())
}

fn session_filtered_indices(entries: &[(String, String)], filter: &str) -> Vec<usize> {
    if filter.is_empty() {
        return (0..entries.len()).collect();
    }

    let filter = filter.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter_map(|(idx, (name, _))| name.to_lowercase().contains(&filter).then_some(idx))
        .collect()
}

fn session_filter_escape_selection(
    entries: &[(String, String)],
    filter: &str,
    selected: usize,
) -> Option<usize> {
    if filter.is_empty() {
        return None;
    }

    Some(
        session_filtered_indices(entries, filter)
            .get(selected)
            .copied()
            .unwrap_or(0),
    )
}

/// Flush the paste-pending buffer as individual send-text / send-key commands.
/// Called when a non-bufferable key (Backspace, Delete, Esc, BackTab) interrupts
/// a potential paste burst, so we emit whatever we had as normal keystrokes.
#[cfg(windows)]
fn flush_paste_pend_as_text(
    paste_pend: &mut String,
    paste_pend_start: &mut Option<Instant>,
    paste_stage2: &mut bool,
    cmd_batch: &mut Vec<String>,
) {
    if paste_pend.is_empty() {
        return;
    }
    // If we accumulated enough ASCII chars that stage2 was entered, this
    // is almost certainly pasted content — send as send-paste so the server
    // wraps it in bracketed paste sequences (fixes nvim autoindent).
    // Non-ASCII buffers (IME input) are always flushed as normal text to
    // avoid the 300ms delay (fixes #91).
    let has_non_ascii = paste_pend.chars().any(|c| !c.is_ascii());
    if (*paste_stage2 || paste_pend.len() >= 3) && !has_non_ascii {
        let encoded = crate::util::base64_encode(paste_pend);
        cmd_batch.push(format!("send-paste {}\n", encoded));
    } else {
        for c in paste_pend.chars() {
            match c {
                '\n' => { cmd_batch.push("send-key enter\n".into()); }
                '\t' => { cmd_batch.push("send-key tab\n".into()); }
                ' '  => { cmd_batch.push("send-key space\n".into()); }
                _ => {
                    let escaped = match c {
                        '"' => "\\\"".to_string(),
                        '\\' => "\\\\".to_string(),
                        _ => c.to_string(),
                    };
                    cmd_batch.push(format!("send-text \"{}\"\n", escaped));
                }
            }
        }
    }
    paste_pend.clear();
    *paste_pend_start = None;
    *paste_stage2 = false;
}

#[cfg(windows)]
fn should_buffer_leading_paste_control(
    code: &KeyCode,
    modifiers: KeyModifiers,
    paste_detection_enabled: bool,
    paste_pend_non_empty: bool,
) -> bool {
    if !matches!(code, KeyCode::Enter | KeyCode::Tab) {
        return false;
    }
    if paste_pend_non_empty {
        return true;
    }
    if !paste_detection_enabled || !modifiers.is_empty() {
        return false;
    }
    true
}

#[cfg(windows)]
fn should_zero_latency_flush_paste_pend(
    paste_pend: &str,
    paste_detection_enabled: bool,
    paste_confirmed: bool,
    paste_stage2: bool,
) -> bool {
    if paste_confirmed || paste_stage2 || paste_pend.is_empty() {
        return false;
    }
    if !paste_detection_enabled {
        return true;
    }
    if paste_pend.starts_with('\n') || paste_pend.starts_with('\t') {
        return false;
    }
    paste_pend.len() <= 2
}

/// Returns true if the buffer contains any non-ASCII characters (IME / CJK input).
/// Used by the paste detection heuristic to skip Stage 2 for IME input (fixes #91).
#[cfg(windows)]
fn paste_buffer_has_non_ascii(buf: &str) -> bool {
    buf.chars().any(|c| !c.is_ascii())
}

/// Route a clipboard paste into the active client-side text overlay
/// (command prompt, rename prompt, pane title, window-index prompt).
/// Returns `true` when an overlay consumed the paste — callers must skip
/// the `send-paste` forwarding in that case so the text does not also leak
/// into the underlying pane (fixes issue #290).
fn route_paste_to_overlay(
    data: &str,
    command_input: bool,
    command_buf: &mut String,
    command_cursor: &mut usize,
    renaming: bool,
    rename_buf: &mut String,
    pane_renaming: bool,
    pane_title_buf: &mut String,
    window_idx_input: bool,
    window_idx_buf: &mut String,
) -> bool {
    if command_input {
        command_buf.insert_str(*command_cursor, data);
        *command_cursor += data.len();
        true
    } else if renaming {
        rename_buf.push_str(data);
        true
    } else if pane_renaming {
        pane_title_buf.push_str(data);
        true
    } else if window_idx_input {
        for c in data.chars() {
            if c.is_ascii_digit() { window_idx_buf.push(c); }
        }
        true
    } else {
        false
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_client.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_zoom_bleed.rs"]
mod test_zoom_bleed;

#[cfg(test)]
#[path = "../tests-rs/test_zoom_cursor_rect.rs"]
mod test_zoom_cursor_rect;

#[cfg(test)]
#[path = "../tests-rs/test_pane_border_status_cursor.rs"]
mod test_pane_border_status_cursor;

#[cfg(test)]
#[path = "../tests-rs/test_issue345_command_prompt_utf8.rs"]
mod test_issue345_command_prompt_utf8;

#[cfg(test)]
#[path = "../tests-rs/test_issue361_osc8_overlay.rs"]
mod test_issue361_osc8_overlay;

#[cfg(test)]
#[path = "../tests-rs/test_pane_wants_mouse_selection.rs"]
mod test_pane_wants_mouse_selection;

#[cfg(test)]
#[path = "../tests-rs/test_issue507_popup_cursor.rs"]
mod test_issue507_popup_cursor;

#[cfg(test)]
#[path = "../tests-rs/test_issue605_stale_port_attach.rs"]
mod test_issue605_stale_port_attach;
