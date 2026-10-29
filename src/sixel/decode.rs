//! A sixel DCS decoded into indexed pixels, so the client can cut out and
//! rescale the part of an image a pane shows.
//!
//! The pixels are the ones the pane's console host laid out. OpenConsole runs
//! Windows Terminal's sixel parser, and `vt100`'s `sixel_layout` sizes an
//! image the way that parser does, so this reads the data by the same rules
//! and gets the same size: a band is six sixel rows of `aspect` pixels each,
//! the aspect comes from the first introducer parameter (2:1 when it is 0 or
//! omitted) and a raster command `"Pan;Pad;Ph;Pv` replaces it with
//! `ceil(Pan / Pad)` and makes the image at least `Ph` by `Pv`.
//!
//! Colours are kept as the image defines them, by register, the way tmux
//! keeps them (`sixel_parse_colour`, image-sixel.c:190 in tmux 3.8), so the
//! terminal converts them exactly as it would have for the original bytes.

/// One colour register definition as the sixel wrote it: `kind` 1 is HLS and
/// 2 is RGB, `a`, `b` and `c` the three values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SixelColour {
    pub kind: u8,
    pub a: u16,
    pub b: u16,
    pub c: u16,
}

/// An image as indexed pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SixelPixels {
    pub width: u32,
    pub height: u32,
    /// The background select, the second introducer parameter: 1 leaves
    /// pixels never drawn transparent, 0 and 2 fill them with colour 0.
    pub p2: u32,
    /// Definitions by register; `None` for a register that is selected but
    /// never defined, which the terminal takes from its own palette.
    pub palette: Vec<Option<SixelColour>>,
    /// Row major, `width * height` entries: the register plus one, or 0 for a
    /// pixel never drawn.
    pub pixels: Vec<u16>,
}

impl SixelPixels {
    /// The pixel at `x`, `y`, 0 outside the image (tmux `sixel_get_pixel`).
    pub(crate) fn get(&self, x: u32, y: u32) -> u16 {
        if x < self.width && y < self.height {
            self.pixels[(y as usize) * (self.width as usize) + x as usize]
        } else {
            0
        }
    }
}

/// The largest width or height decoded. A 1 MiB DCS of repeats could ask for
/// far more than any pane shows; pixels past this are dropped.
pub(crate) const MAX_SIDE: u32 = 4096;

/// The highest register kept. Higher ones are drawn with this one.
const MAX_REGISTER: usize = 4095;

/// The most pixel writes one decode does. A raster command can make every
/// sixel up to `MAX_SIDE` pixels tall, and `$` lets a DCS draw the same
/// columns again and again, so without a limit a few kilobytes written by a
/// pane program kept the client busy for seconds, and a 1 MiB DCS for
/// minutes, before the frame was drawn. Four times the largest image is far
/// more than any image a program means to draw ever writes.
pub(crate) const MAX_WORK: u64 = 4 * (MAX_SIDE as u64) * (MAX_SIDE as u64);

/// The pixel aspect ratio a sixel introducer's first parameter selects, as
/// `sixel_layout` reads it.
fn intro_aspect(p1: u32) -> u32 {
    match p1 {
        0 | 1 | 5 | 6 => 2,
        2 => 5,
        3 | 4 => 3,
        _ => 1,
    }
}

/// Decode `raw`, a whole `ESC P ... q ... ESC \` sequence. `None` when it is
/// not a sixel DCS.
pub(crate) fn decode(raw: &[u8]) -> Option<SixelPixels> {
    let body = raw.strip_prefix(b"\x1bP")?;
    let q = body.iter().position(|&b| b == b'q')?;
    let intro: Vec<u32> = std::str::from_utf8(&body[..q])
        .ok()?
        .split(';')
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let p1 = intro.first().copied().unwrap_or(0);
    let p2 = intro.get(1).copied().unwrap_or(0);
    let payload = &body[q + 1..];

    let mut aspect = intro_aspect(p1);
    let mut rows: Vec<Vec<u16>> = Vec::new();
    let mut palette: Vec<Option<SixelColour>> = Vec::new();
    let mut colour: usize = 0;
    let mut x: u32 = 0;
    let mut y: u32 = 0;
    let mut max_x: u32 = 0;
    let mut drawn_bottom: u32 = 0;
    let mut raster = (0u32, 0u32);

    let read_fields = |i: &mut usize| -> Vec<u32> {
        let mut fields = vec![0u32];
        while *i < payload.len() {
            match payload[*i] {
                b @ b'0'..=b'9' => {
                    let last = fields.last_mut().expect("never empty");
                    *last = last.saturating_mul(10).saturating_add(u32::from(b - b'0'));
                }
                b';' => fields.push(0),
                _ => break,
            }
            *i += 1;
        }
        fields
    };
    // Six bits of one sixel, `n` columns wide, at the current position. Adds
    // the pixels it writes, and at least one per pixel row it visits, to
    // `work`.
    let draw = |rows: &mut Vec<Vec<u16>>, work: &mut u64, x: u32, y: u32, aspect: u32, bits: u8, n: u32, value: u16| {
        if bits == 0 {
            return;
        }
        for bit in 0..6u32 {
            if bits & (1 << bit) == 0 {
                continue;
            }
            for dy in 0..aspect {
                let py = y + bit * aspect + dy;
                if py >= MAX_SIDE {
                    return;
                }
                if rows.len() <= py as usize {
                    rows.resize_with(py as usize + 1, Vec::new);
                }
                let row = &mut rows[py as usize];
                let end = (x.saturating_add(n)).min(MAX_SIDE);
                *work += u64::from(end.saturating_sub(x).max(1));
                if x >= end {
                    continue;
                }
                if row.len() < end as usize {
                    row.resize(end as usize, 0);
                }
                for px in x..end {
                    row[px as usize] = value;
                }
            }
        }
    };

    let mut work: u64 = 0;
    let mut i = 0usize;
    while i < payload.len() && work <= MAX_WORK {
        match payload[i] {
            b'"' => {
                i += 1;
                let f = read_fields(&mut i);
                let (pan, pad) = (f.first().copied().unwrap_or(0), f.get(1).copied().unwrap_or(0));
                if pad > 0 {
                    aspect = pan.div_ceil(pad).clamp(1, MAX_SIDE / 6);
                }
                raster = (
                    f.get(2).copied().filter(|&w| w > 0).unwrap_or(raster.0),
                    f.get(3).copied().filter(|&h| h > 0).unwrap_or(raster.1),
                );
                max_x = max_x.max(x);
                x = 0;
                continue;
            }
            b'#' => {
                i += 1;
                let f = read_fields(&mut i);
                colour = (f[0] as usize).min(MAX_REGISTER);
                if palette.len() <= colour {
                    palette.resize(colour + 1, None);
                }
                if f.len() >= 5 && matches!(f[1], 1 | 2) {
                    let v = |n: u32| n.min(u32::from(u16::MAX)) as u16;
                    palette[colour] = Some(SixelColour { kind: f[1] as u8, a: v(f[2]), b: v(f[3]), c: v(f[4]) });
                }
                continue;
            }
            b'!' => {
                i += 1;
                let n = read_fields(&mut i).first().copied().unwrap_or(0).max(1);
                if i < payload.len() && (0x3f..=0x7e).contains(&payload[i]) {
                    if palette.len() <= colour {
                        palette.resize(colour + 1, None);
                    }
                    draw(&mut rows, &mut work, x, y, aspect, payload[i] - 0x3f, n, colour as u16 + 1);
                    x = x.saturating_add(n);
                    drawn_bottom = drawn_bottom.max(y.saturating_add(6 * aspect));
                    i += 1;
                }
                continue;
            }
            b'$' => {
                max_x = max_x.max(x);
                x = 0;
            }
            b'-' => {
                max_x = max_x.max(x);
                x = 0;
                y = y.saturating_add(6 * aspect);
            }
            c @ 0x3f..=0x7e => {
                if palette.len() <= colour {
                    palette.resize(colour + 1, None);
                }
                draw(&mut rows, &mut work, x, y, aspect, c - 0x3f, 1, colour as u16 + 1);
                x = x.saturating_add(1);
                drawn_bottom = drawn_bottom.max(y.saturating_add(6 * aspect));
            }
            0x1b => break, // ESC of the ST terminator
            _ => {}
        }
        i += 1;
    }
    max_x = max_x.max(x);

    let width = max_x.max(raster.0).clamp(1, MAX_SIDE);
    let height = drawn_bottom.max(raster.1).clamp(1, MAX_SIDE);
    let mut pixels = vec![0u16; width as usize * height as usize];
    for (py, row) in rows.iter().enumerate().take(height as usize) {
        let n = row.len().min(width as usize);
        let at = py * width as usize;
        pixels[at..at + n].copy_from_slice(&row[..n]);
    }
    Some(SixelPixels { width, height, p2, palette, pixels })
}

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_decode.rs"]
mod test_sixel_decode;
