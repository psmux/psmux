//! What the terminal around the client says about sixel (#431, #928).

/// Whether the terminal's DA1 reply, `CSI ? Ps ; ... c`, lists sixel.
///
/// Read the way tmux reads it (`tty_keys_device_attributes`,
/// tty-keys.c:1519-1528 in tmux 3.8): a first parameter of 61 to 65 names
/// the conformance level and the rest are features, 4 being sixel.  Any other reply says no.  `None` when there
/// is no DA1 reply in `buf`.  Measured on Windows Terminal 1.24:
/// `CSI ? 61;4;6;7;14;21;22;23;24;28;32;42;52 c`.
pub(crate) fn parse_da1_sixel(buf: &[u8]) -> Option<bool> {
    let needle = b"\x1b[?";
    let mut from = 0;
    while let Some(at) = buf[from..].windows(needle.len()).position(|w| w == needle) {
        let start = from + at + needle.len();
        from = start;
        // Other replies in the drain share the opening (`CSI ? 12 ; 1 $ y`,
        // `CSI ? 997 ; 1 n`), so only the one ending in c counts.
        let Some(end) = buf[start..].iter().position(|b| b.is_ascii_alphabetic()) else {
            continue;
        };
        if buf[start + end] != b'c' {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&buf[start..start + end]) else {
            continue;
        };
        let params: Vec<u32> = text
            .split(';')
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let level = params.first().copied().unwrap_or(0);
        return Some((61..=65).contains(&level) && params[1..].contains(&4));
    }
    None
}

/// Whether the terminal said it draws sixel, from its DA1 reply at attach:
/// 0 nobody said, 1 yes, 2 no.
static HOST_SIXEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Record what the terminal's DA1 reply said about sixel.
pub fn note_host_sixel(sixel: Option<bool>) {
    let v = match sixel {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    HOST_SIXEL.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// What the terminal's DA1 reply said about sixel, `None` when it did not
/// answer or was not asked.
pub(crate) fn host_sixel() -> Option<bool> {
    match HOST_SIXEL.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Whether the client writes sixel images to the terminal under `mode`.
pub(crate) fn sixel_draws(mode: crate::types::SixelMode) -> bool {
    sixel_mode_draws(mode, host_sixel())
}

/// The `sixel` option against what the terminal said.
///
/// `auto` is tmux: it writes an image only to a terminal with the sixel
/// feature (`tty_cmd_sixelimage`, tty.c:2191 in tmux 3.8), which a DA1 reply
/// listing 4 turns on (`tty_keys_device_attributes`, tty-keys.c:1528).  So no
/// reply writes no image, which is the case inside psmux, where the query is
/// skipped, and for a stdin that is not a console.  `on` is for those, the
/// way tmux takes `sixel` in `terminal-features` (tty-term.c:619).
pub(crate) fn sixel_mode_draws(mode: crate::types::SixelMode, host: Option<bool>) -> bool {
    use crate::types::SixelMode;
    match mode {
        SixelMode::Off => false,
        SixelMode::On => true,
        SixelMode::Auto => host == Some(true),
    }
}

/// The `sixel` option a frame carries: `sixel_mode` from a server that has
/// the three values, else the older on/off flag, whose on drew unless the
/// terminal said no, which is nearest to auto.
pub(crate) fn frame_sixel_mode(mode: Option<&str>, flag: bool) -> crate::types::SixelMode {
    use crate::types::SixelMode;
    match mode.and_then(SixelMode::parse) {
        Some(mode) => mode,
        None if flag => SixelMode::Auto,
        None => SixelMode::Off,
    }
}

/// The terminal's cell size in pixels from its replies at attach, the width
/// in the high 16 bits and the height in the low ones; 0 when it did not say.
static HOST_CELL_PX: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Record the cell size the terminal reported.
pub(crate) fn note_host_cell_px(px: Option<(u32, u32)>) {
    let v = px
        .filter(|&(w, h)| w > 0 && h > 0 && w <= u32::from(u16::MAX) && h <= u32::from(u16::MAX))
        .map_or(0, |(w, h)| (w << 16) | h);
    HOST_CELL_PX.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The cell size the terminal reported, `None` when it did not say.
pub(crate) fn host_cell_px() -> Option<(u32, u32)> {
    match HOST_CELL_PX.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        v => Some((v >> 16, v & 0xffff)),
    }
}

/// The cell size images are drawn at: the terminal's, or the 10 by 20 pixels
/// a pane lays them out in when it did not say.
///
/// tmux draws a placeholder instead when it knows no cell size
/// (`tty_cmd_sixelimage`, tty.c:2194 in tmux 3.8). psmux draws the image as
/// the pane laid it out, which is what Windows Terminal reports anyway.
pub(crate) fn draw_cell_px() -> (u32, u32) {
    host_cell_px().unwrap_or(vt100::image::DEFAULT_CELL_PX)
}

/// The cell size in pixels from the terminal's XTWINOPS replies: `CSI 6 ; h
/// ; w t` to `CSI 16 t`, or else `CSI 4 ; h ; w t` to `CSI 14 t` divided by
/// the terminal's `cols` by `rows`, which is what tmux reads
/// (`tty_keys_winsz`, tty-keys.c:726-735 in tmux 3.8).  Measured on Windows
/// Terminal 1.24 at 120 by 30: `CSI 6;20;10 t` and `CSI 4;600;1200 t`, the
/// 10 by 20 cell it scales sixel images from, whatever the font.
pub(crate) fn parse_cell_size(buf: &[u8], cols: u16, rows: u16) -> Option<(u32, u32)> {
    let report = |op: &[u8]| -> Option<(u32, u32)> {
        let needle = [b"\x1b[".as_slice(), op, b";"].concat();
        let at = buf.windows(needle.len()).position(|w| w == needle.as_slice())?;
        let rest = &buf[at + needle.len()..];
        let end = rest.iter().position(|&b| b == b't')?;
        let text = std::str::from_utf8(&rest[..end]).ok()?;
        let mut fields = text.split(';').map(|f| f.parse::<u32>().ok());
        let h = fields.next()??;
        let w = fields.next()??;
        if fields.next().is_some() {
            return None;
        }
        Some((w, h))
    };
    if let Some((w, h)) = report(b"6").filter(|&(w, h)| w > 0 && h > 0) {
        return Some((w, h));
    }
    let (w, h) = report(b"4")?;
    let (cols, rows) = (u32::from(cols), u32::from(rows));
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((w / cols, h / rows)).filter(|&(w, h)| w > 0 && h > 0)
}

#[cfg(test)]
#[path = "../../tests-rs/test_issue431_host_sixel.rs"]
mod test_issue431_host_sixel;

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_mode.rs"]
mod test_sixel_mode;

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_cell_size.rs"]
mod test_sixel_cell_size;
