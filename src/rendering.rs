//! Shared terminal styling, cursor, pane-border, and layout helpers.

use std::io::{self, Write};
use std::env;
use ratatui::prelude::*;
use ratatui::style::{Style, Modifier};
use crossterm::style::Print;
use crossterm::execute;

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

/// The DECSCUSR code this client last asserted on the real terminal, 0 for
/// "none asserted, the terminal's own shape stands".
///
/// tmux keeps the same thing per tty, `tty->cstyle`, and reads it when it
/// stops the tty: `tty_stop_tty` (tty.c) sends the reset only when the style
/// it last sent was not SCREEN_CURSOR_DEFAULT. Teardown here asks the same
/// question, and the two places that write a shape, the attach below and the
/// client's frame loop, both record it.
static LAST_CURSOR_CODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// What this client asserted through DEC private mode 12: 0 nothing, 1 on,
/// 2 off. Mode 12 has no "back to the terminal's default" the way DECSCUSR 0
/// is for the shape, only the two states, so putting it back needs to know
/// which one was there first (`HOST_BLINK_BEFORE`).
static BLINK_ASSERTED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// What mode 12 was before this client touched it, as the terminal answered
/// DECRQM at attach: 0 nobody said, 1 set, 2 reset.
static HOST_BLINK_BEFORE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// What the cursor's shape was before this client touched it, as the terminal
/// answered DECRQSS at attach, or 255 when it did not say. 0 is a real answer:
/// it is the terminal's own default, which is what most terminals report.
static HOST_SHAPE_BEFORE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(255);

/// Record what the terminal said its cursor shape was before this client
/// touched it.
pub fn note_host_shape_before(code: Option<u8>) {
    HOST_SHAPE_BEFORE.store(code.unwrap_or(255), std::sync::atomic::Ordering::Relaxed);
}

/// The DECSCUSR code teardown should write to put the shape back, `None` when
/// this client never asserted one.
///
/// The terminal is asked at attach what it was drawing, so the usual answer is
/// that shape. A terminal that will not say leaves 0, the reset to its own
/// default, which is what tmux settles for: `tty_stop_tty` (tty.c) sends `Se`,
/// defined as `ESC [ 2 q` by the cstyle feature, or `Ss 0` for a terminal
/// without it, neither of which is where the user was.
pub(crate) fn shape_restore() -> Option<u8> {
    if !SHAPE_EVER_ASSERTED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let before = HOST_SHAPE_BEFORE.load(std::sync::atomic::Ordering::Relaxed);
    Some(if before == 255 { 0 } else { before })
}

/// Whether this client ever asserted a cursor SHAPE, which is a different
/// question from whether it is holding one now (`LAST_CURSOR_CODE`).
///
/// A shape is a DECSCUSR code, and every code says whether it blinks, so
/// writing one moves the terminal's blinking whether or not that was the
/// point. The reset that takes the shape back, `ESC [ 0 q`, moves it again: it
/// returns the terminal to ITS default, which on Windows Terminal blinks, and
/// a user who had the blinking off is not back where they started. So once a
/// shape has gone out, the blink has to be put back explicitly, even after the
/// shape has been let go of again.
static SHAPE_EVER_ASSERTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Record that this client wrote DEC private mode 12.
pub(crate) fn note_blink_asserted(on: bool) {
    BLINK_ASSERTED.store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::Relaxed);
}

/// Record what the terminal said mode 12 was before this client touched it.
pub fn note_host_blink_before(state: Option<bool>) {
    let v = match state { Some(true) => 1, Some(false) => 2, None => 0 };
    HOST_BLINK_BEFORE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The mode 12 write that puts the terminal's blinking back where this client
/// found it, `None` when it never moved it.
///
/// Two things move it: mode 12 itself, and any DECSCUSR shape, since a shape's
/// code says whether it blinks and the reset that takes the shape back returns
/// the terminal to its own default rather than to what the user had. So either
/// one owes this.
///
/// The terminal is asked at attach what mode 12 was, so the usual answer is
/// the state it had. A terminal that does not answer DECRQM leaves that
/// unknown: for a blink this client set, the opposite of what it set undoes
/// it, and for a shape there is nothing honest left to say, so it says
/// nothing.
pub(crate) fn blink_restore() -> Option<bool> {
    let asserted = BLINK_ASSERTED.load(std::sync::atomic::Ordering::Relaxed);
    let shape = SHAPE_EVER_ASSERTED.load(std::sync::atomic::Ordering::Relaxed);
    if asserted == 0 && !shape {
        return None;
    }
    match HOST_BLINK_BEFORE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ if asserted != 0 => Some(asserted != 1),
        _ => None,
    }
}

/// The blink `cursor-blink` asks for on its own, for a `cursor-style` that
/// names no shape, and `None` when the shape carries it or nobody asked.
///
/// A shape goes out as a DECSCUSR code that already says whether it blinks.
/// Without a shape there is no code to carry it, and DEC private mode 12 is
/// the only thing that can, so an explicit `cursor-blink` is sent that way and
/// an unset one is sent not at all.
pub fn blink_only_mode() -> Option<bool> {
    if configured_cursor_code() != 0 {
        return None;
    }
    cursor_blink_option()
}

/// Write the blink on its own, as DEC private mode 12. `None` writes nothing.
pub fn apply_blink_only<W: Write>(out: &mut W, blink: Option<bool>) -> io::Result<()> {
    let Some(on) = blink else { return Ok(()) };
    execute!(out, Print(if on { "\x1b[?12h" } else { "\x1b[?12l" }))?;
    note_blink_asserted(on);
    Ok(())
}

/// Record a DECSCUSR code this client just wrote to the terminal.
pub(crate) fn note_cursor_code(code: u8) {
    LAST_CURSOR_CODE.store(code, std::sync::atomic::Ordering::Relaxed);
    if code != 0 {
        SHAPE_EVER_ASSERTED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Forget what this client asserted. Only the tests need it: a client asserts
/// over one attach and tears down once, while the tests share one process and
/// one pair of statics.
#[cfg(test)]
pub(crate) fn forget_cursor_state_for_test() {
    LAST_CURSOR_CODE.store(0, std::sync::atomic::Ordering::Relaxed);
    BLINK_ASSERTED.store(0, std::sync::atomic::Ordering::Relaxed);
    HOST_BLINK_BEFORE.store(0, std::sync::atomic::Ordering::Relaxed);
    SHAPE_EVER_ASSERTED.store(false, std::sync::atomic::Ordering::Relaxed);
    HOST_SHAPE_BEFORE.store(255, std::sync::atomic::Ordering::Relaxed);
}

/// Whether `cursor-blink` is on, or `None` when nobody set it.
///
/// `set -g cursor-blink <value>` takes the words the option is documented
/// with and stores `1` or `0` in `PSMUX_CURSOR_BLINK` (config.rs,
/// server/options.rs). The variable is read back the way that value was
/// parsed, so a shell that exports the word rather than the digit,
/// `PSMUX_CURSOR_BLINK=off`, does not come out meaning the opposite of what it
/// says. Everything that is not a yes is a no, which is what the option's own
/// parser does.
pub(crate) fn cursor_blink_option() -> Option<bool> {
    env::var("PSMUX_CURSOR_BLINK")
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "on" | "true"))
}

/// Whether an unset `cursor-blink` blinks, mirroring its catalog default.
///
/// It is `off`, which is what makes a bare `block`, `underline` or `bar` mean
/// the steady shape tmux means by those words. Named rather than inlined so
/// the fallback below reads as two separate reasons, and so a test can hold it
/// against the catalog.
pub(crate) const CURSOR_BLINK_DEFAULT_BLINKS: bool = false;

/// Resolve the DECSCUSR code (1-6) from the PSMUX_CURSOR_STYLE /
/// PSMUX_CURSOR_BLINK configuration, or 0 for "no opinion".
///
/// 0 is a state, not a shape: it is what `cursor-style default` means, it is
/// the default of the option, and it is also what an unset or unrecognised
/// value resolves to. tmux spells the same state `SCREEN_CURSOR_DEFAULT`
/// (tmux.h) and treats it as a third value beside block, underline and bar.
///
/// Used as the fallback cursor shape when ConPTY doesn't forward DECSCUSR from
/// the child process (Windows 10 without passthrough mode).
pub fn configured_cursor_code() -> u8 {
    let style = env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "default".to_string());
    // tmux has no `cursor-blink`: it spells the blink into the value, as
    // `blinking-block`, `blinking-underline` and `blinking-bar`
    // (options-table.c, tmux.1). psmux has carried the blink as its own
    // option since its first commit and users have it in their configs, so
    // both spellings have to work. The prefix names the SHAPE; who decides the
    // blink depends on whether the option was set at all.
    let named_blink = style.strip_prefix("blinking-");
    let shape = named_blink.unwrap_or(style.as_str());
    let blink = match cursor_blink_option() {
        // Set: it decides. It is the setting that speaks about nothing but
        // blinking, so `cursor-style blinking-bar` with `cursor-blink off` is
        // a steady bar.
        Some(on) => on,
        // Not set: a value that names the blink supplies it, and otherwise
        // `cursor-blink`'s own default stands. With that default off, a bare
        // `block` is the steady block tmux means by the word, and
        // `blinking-block` is the blinking one.
        None => named_blink.is_some() || CURSOR_BLINK_DEFAULT_BLINKS,
    };
    match shape {
        "block" => if blink { 1 } else { 2 },
        "underline" => if blink { 3 } else { 4 },
        "bar" | "beam" => if blink { 5 } else { 6 },
        "default" => 0,
        _ => 0,
    }
}

/// Assert the configured cursor shape on the real terminal, once, at attach.
///
/// Writes nothing when nothing asked for a shape. tmux never sends one of its
/// own either: `tty_start_tty` (tty.c) sends the alternate screen, the keypad
/// mode, a clear, `cnorm`, the mouse mode resets and bracketed paste, and no
/// DECSCUSR at all. A terminal keeps the cursor its user configured because
/// nothing overwrote it, which cannot be had by sending a reset instead.
pub fn apply_cursor_style<W: Write>(out: &mut W) -> io::Result<()> {
    let code = configured_cursor_code();
    if code == 0 {
        // No shape. An explicit `cursor-blink` still has something to say,
        // and mode 12 is the only way to say it without naming a shape.
        return apply_blink_only(out, blink_only_mode());
    }
    execute!(out, Print(format!("\x1b[{} q", code)))?;
    note_cursor_code(code);
    Ok(())
}

// ─── Pane border intersections ─────────────────────────────────────────────

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

    pub(crate) fn contains_vertical(&self, idx: usize) -> bool {
        self.has_orientation(idx, BorderOrientation::Vertical)
    }

    pub(crate) fn contains_horizontal(&self, idx: usize) -> bool {
        self.has_orientation(idx, BorderOrientation::Horizontal)
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

#[cfg(test)]
#[path = "../tests-rs/test_issue735_cursor_style_default.rs"]
mod test_issue735_cursor_style_default;
