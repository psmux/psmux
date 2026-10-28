//! What the terminal around the client says about sixel (#431, #928).

/// Whether the terminal's DA1 reply, `CSI ? Ps ; ... c`, lists sixel.
///
/// Read the way tmux reads it (`tty_keys_device_attributes`, tty-keys.c:1496):
/// a first parameter of 61 to 65 names the conformance level and the rest
/// are features, 4 being sixel.  Any other reply says no.  `None` when there
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
fn host_sixel() -> Option<bool> {
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

#[cfg(test)]
#[path = "../../tests-rs/test_issue431_host_sixel.rs"]
mod test_issue431_host_sixel;

#[cfg(test)]
#[path = "../../tests-rs/test_sixel_mode.rs"]
mod test_sixel_mode;
