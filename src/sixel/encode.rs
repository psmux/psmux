//! Indexed pixels written back out as a sixel DCS, in the shape tmux's
//! `sixel_print` writes (image-sixel.c:566-650 in tmux 3.8).

use super::decode::SixelPixels;

/// The sixel for `image`: pixel aspect 1:1 (`ESC P 9 ; p2 q`), a raster
/// command with its size so a terminal that fills the background stops at
/// the image, the colours it defines, then each six pixel band colour by
/// colour.
pub(crate) fn encode(image: &SixelPixels) -> Vec<u8> {
    let mut out = format!("\x1bP9;{}q\"1;1;{};{}", image.p2, image.width, image.height).into_bytes();
    for (register, colour) in image.palette.iter().enumerate() {
        if let Some(c) = colour {
            out.extend_from_slice(format!("#{};{};{};{};{}", register, c.kind, c.a, c.b, c.c).as_bytes());
        }
    }

    let width = image.width as usize;
    let mut band_top = 0u32;
    while band_top < image.height {
        // Each colour's sixels across this band, the colours in the order
        // they first appear.
        let mut used: Vec<u16> = Vec::new();
        let mut rows: std::collections::HashMap<u16, Vec<u8>> = std::collections::HashMap::new();
        for bit in 0..6u32 {
            let y = band_top + bit;
            if y >= image.height {
                break;
            }
            let at = y as usize * width;
            for (x, &v) in image.pixels[at..at + width].iter().enumerate() {
                if v == 0 {
                    continue;
                }
                let sixels = rows.entry(v).or_insert_with(|| {
                    used.push(v);
                    vec![0; width]
                });
                sixels[x] |= 1 << bit;
            }
        }
        if band_top > 0 {
            out.push(b'-');
        }
        for (n, value) in used.iter().enumerate() {
            if n > 0 {
                out.push(b'$');
            }
            out.extend_from_slice(format!("#{}", value - 1).as_bytes());
            write_runs(&mut out, &rows[value]);
        }
        band_top += 6;
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// One colour's sixels for a band: runs of more than three become `!n`, as
/// tmux's `sixel_print_repeat` writes them, and the empty tail is dropped.
fn write_runs(out: &mut Vec<u8>, sixels: &[u8]) {
    let end = sixels.iter().rposition(|&s| s != 0).map_or(0, |p| p + 1);
    let mut x = 0;
    while x < end {
        let s = sixels[x];
        let run = sixels[x..end].iter().take_while(|&&t| t == s).count();
        let ch = 0x3f + s;
        if run > 3 {
            out.extend_from_slice(format!("!{}", run).as_bytes());
            out.push(ch);
        } else {
            out.extend(std::iter::repeat_n(ch, run));
        }
        x += run;
    }
}

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_encode.rs"]
mod test_sixel_encode;
