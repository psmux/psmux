//! Per pane colour palette, the OSC 4 / OSC 104 store (issue #685).
//!
//! A pane child on `ConPTY` announces its console colour table as OSC 4 palette
//! sets: conhost pushes all 256 entries and then paints with *indexed* SGR
//! (`ESC[38;5;14m`, `ESC[48;5;6m`).  Without somewhere to keep those entries,
//! the indexes mean whatever the outer terminal's own scheme says they mean,
//! so Far Manager's classic `#000080` panels came out Windows Terminal's
//! Campbell `#0037DA`.
//!
//! tmux solves this with a palette per pane (`struct colour_palette` in
//! `tmux.h:764`, held by `struct window_pane` at `tmux.h:1379`), filled by
//! `input_osc_4` (`input.c:2927`) through `colour_palette_set`
//! (`colour.c:1272`), emptied by `input_osc_104` (`input.c:3446`) and by RIS
//! (`input.c:1407`), and consulted on the way to the terminal by
//! `tty_check_fg` / `tty_check_bg` / `tty_check_us` (`tty.c:2822`, `2892`,
//! `2945`), each of which replaces an indexed colour with the palette's RGB
//! *before* the bytes go out.  psmux does the same, except that the
//! substitution happens where a cell is serialised for the client, which is
//! psmux's equivalent of "on the way to the tty".
//!
//! Deliberately NOT done: forwarding OSC 4 to the outer terminal.  Two panes
//! with different palettes would fight over one terminal, which is exactly why
//! tmux resolves per pane.

use crate::attrs::Color;

/// 256 optional overrides, one per indexed colour.
///
/// Mirrors tmux's `p->palette`, which is likewise allocated only once an entry
/// is actually set (`colour.c:1280`) and whose "unset" marker is `-1`.  An
/// OSC 4 entry is always an RGB colour; an entry of the `pane-colours` default
/// palette (issue #980, tmux's `p->default_palette`) can also be another
/// index or the default colour, as `colour_palette_from_option`
/// (`colour.c:1293`) stores any colour.  `Option<Color>` is the same four
/// bytes the RGB triple took, so the table did not grow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColourPalette {
    entries: [Option<Color>; 256],
}

impl Default for ColourPalette {
    fn default() -> Self {
        Self {
            entries: [None; 256],
        }
    }
}

impl ColourPalette {
    /// The override for `idx` as RGB, if one has been set: an RGB entry as
    /// it is, an indexed one through the xterm table, as tmux's
    /// `colour_force_rgb` (`colour.c:189`) turns a palette colour into the
    /// RGB an OSC 4 query reports.  A default colour entry has no RGB.
    #[must_use]
    pub fn get(&self, idx: u8) -> Option<(u8, u8, u8)> {
        match self.entries[idx as usize]? {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            Color::Idx(i) => Some(xterm_rgb(i)),
            Color::Default => None,
        }
    }

    /// The override for `idx` as it is stored.
    #[must_use]
    pub fn entry(&self, idx: u8) -> Option<Color> {
        self.entries[idx as usize]
    }

    /// Set (or, with `None`, unset) one entry.  Returns `true` when the value
    /// actually changed, which is tmux's `colour_palette_set` return and what
    /// drives its `screen_write_fullredraw`.
    pub fn set(&mut self, idx: u8, rgb: Option<(u8, u8, u8)>) -> bool {
        self.set_colour(idx, rgb.map(|(r, g, b)| Color::Rgb(r, g, b)))
    }

    /// [`set`](Self::set) for any colour, which a `pane-colours` entry can be.
    pub fn set_colour(&mut self, idx: u8, colour: Option<Color>) -> bool {
        let slot = &mut self.entries[idx as usize];
        if *slot == colour {
            return false;
        }
        *slot = colour;
        true
    }

    /// Drop every entry (`colour_palette_clear`, `colour.c:1227`).  Returns
    /// `true` when something was actually dropped.
    pub fn clear(&mut self) -> bool {
        if self.is_empty() {
            return false;
        }
        self.entries = [None; 256];
        true
    }

    /// True when no entry is set, so the pane renders exactly as it did before
    /// any OSC 4 arrived.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Option::is_none)
    }

    /// Replace an indexed colour with this palette's entry, leaving `Default`
    /// and already-RGB colours alone.  This is the `tty_check_fg` substitution
    /// (`tty.c:2836`), done once: an entry that is itself an index is drawn as
    /// that index, as in tmux.
    #[inline]
    #[must_use]
    pub fn resolve(&self, c: Color) -> Color {
        match c {
            Color::Idx(i) => self.entries[i as usize].unwrap_or(c),
            _ => c,
        }
    }
}

/// The xterm 256 colour table, tmux's `colour_256toRGB` (`colour.c:466`).
#[must_use]
pub fn xterm_rgb(idx: u8) -> (u8, u8, u8) {
    const SYSTEM: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00), (0x80, 0x00, 0x00), (0x00, 0x80, 0x00), (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80), (0x80, 0x00, 0x80), (0x00, 0x80, 0x80), (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80), (0xff, 0x00, 0x00), (0x00, 0xff, 0x00), (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff), (0xff, 0x00, 0xff), (0x00, 0xff, 0xff), (0xff, 0xff, 0xff),
    ];
    const LEVELS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    match idx {
        0..=15 => SYSTEM[idx as usize],
        16..=231 => {
            let n = idx - 16;
            (
                LEVELS[(n / 36) as usize],
                LEVELS[((n / 6) % 6) as usize],
                LEVELS[(n % 6) as usize],
            )
        }
        _ => {
            let v = 8 + (idx - 232) * 10;
            (v, v, v)
        }
    }
}

/// Parse the colour specification of an OSC 4 pair.
///
/// Accepts what tmux's `colour_parseX11` (`colour.c:1176`) accepts of the
/// forms a terminal actually emits:
///
/// * `rgb:<r>/<g>/<b>` with 1 to 4 hex digits per channel, scaled to 8 bits
///   (`rgb:00/00/80`, `rgb:0000/0000/8080`)
/// * `#RGB`, `#RRGGBB`, `#RRRGGGBBB`, `#RRRRGGGGBBBB`
/// * `<r>,<g>,<b>` decimal
///
/// Named X11 colours are not accepted: psmux has no X11 colour table and no
/// terminal emits a name here.  Anything unrecognised returns `None`, which
/// makes the caller skip that pair exactly as tmux does when `colour_parseX11`
/// returns -1 (`input.c:2960`).
#[must_use]
pub fn parse_x11_colour(spec: &[u8]) -> Option<(u8, u8, u8)> {
    let spec = trim_ascii(spec);
    if spec.is_empty() {
        return None;
    }
    if let Some(body) = spec.strip_prefix(b"#") {
        if body.len() % 3 != 0 {
            return None;
        }
        let per = body.len() / 3;
        if per == 0 || per > 4 {
            return None;
        }
        let r = scale_hex(&body[..per])?;
        let g = scale_hex(&body[per..per * 2])?;
        let b = scale_hex(&body[per * 2..])?;
        return Some((r, g, b));
    }
    if let Some(body) = strip_prefix_ci(spec, b"rgb:") {
        let mut parts = body.split(|&b| b == b'/');
        let r = scale_hex(parts.next()?)?;
        let g = scale_hex(parts.next()?)?;
        let b = scale_hex(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        return Some((r, g, b));
    }
    // Decimal `r,g,b`, which tmux takes through the same sscanf.
    let mut parts = spec.split(|&b| b == b',');
    let r = decimal_u8(parts.next()?)?;
    let g = decimal_u8(parts.next()?)?;
    let b = decimal_u8(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((r, g, b))
}

/// Parse the index half of an OSC 4 / OSC 104 pair.  tmux uses `strtol` and
/// rejects anything outside 0..=255 (`input.c:2942`).
#[must_use]
pub fn parse_palette_index(raw: &[u8]) -> Option<u8> {
    let raw = trim_ascii(raw);
    if raw.is_empty() || raw.len() > 3 || !raw.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n: u32 = 0;
    for &b in raw {
        n = n * 10 + u32::from(b - b'0');
    }
    u8::try_from(n).ok()
}

fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

fn strip_prefix_ci<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.len() < prefix.len() {
        return None;
    }
    let (head, tail) = s.split_at(prefix.len());
    if head.eq_ignore_ascii_case(prefix) {
        Some(tail)
    } else {
        None
    }
}

/// Scale a 1..=4 digit hex channel to 8 bits, the way xterm does: the value is
/// taken as a fraction of its own maximum, so `8` is `0x88` and `8080` is
/// `0x80`.
fn scale_hex(digits: &[u8]) -> Option<u8> {
    if digits.is_empty() || digits.len() > 4 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let mut v: u32 = 0;
    let mut max: u32 = 0;
    for &d in digits {
        v = v * 16 + u32::from(hex_val(d)?);
        max = max * 16 + 15;
    }
    Some(u8::try_from((v * 255 + max / 2) / max).unwrap_or(255))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decimal_u8(raw: &[u8]) -> Option<u8> {
    let raw = trim_ascii(raw);
    if raw.is_empty() || raw.len() > 3 || !raw.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut n: u32 = 0;
    for &b in raw {
        n = n * 10 + u32::from(b - b'0');
    }
    u8::try_from(n).ok()
}
