// Issue #473: OSC 4/10/11 terminal color queries and CSI ?996n are not
// answered inside psmux, so pane applications (GitHub Copilot CLI) cannot
// detect the terminal palette and fall back to wrong themes.
//
// These tests cover the detection (the pane's vt100 parser), the HostColors
// model/wire format (types.rs), the host-reply parser (platform.rs), and the
// response builder (server/helpers.rs, exercised through the pipe-fallback
// path by passing child_pid=None).

use super::*;
use crate::types::{HostColors, COLOR_QUERY_BG, COLOR_QUERY_FG, COLOR_QUERY_SCHEME};

// ── Detection (the pane's vt100 parser, in stream order) ─────────────────

use vt100::TerminalQuery as Q;

#[test]
fn scan_detects_each_palette_index() {
    for i in 0..16u8 {
        let q = format!("\x1b]4;{};?\x1b\\", i);
        assert_eq!(scan_device_queries(q.as_bytes()), vec![Q::PaletteColour { index: i, own: None }], "index {}", i);
    }
}

#[test]
fn scan_detects_fg_bg_and_scheme() {
    assert_eq!(scan_device_queries(b"\x1b]10;?\x1b\\"), vec![Q::ForegroundColour]);
    assert_eq!(scan_device_queries(b"\x1b]11;?\x1b\\"), vec![Q::BackgroundColour]);
    assert_eq!(scan_device_queries(b"\x1b[?996n"), vec![Q::ColourScheme]);
}

#[test]
fn scan_detects_copilot_burst() {
    // The exact batch Copilot CLI sends at startup (issue #473 probe), every
    // query in the order it was asked.
    let mut burst = String::from("\x1b[?996n\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    for i in 0..16 {
        burst.push_str(&format!("\x1b]4;{};?\x1b\\", i));
    }
    let mut expected = vec![Q::ColourScheme, Q::ForegroundColour, Q::BackgroundColour];
    expected.extend((0..16u8).map(|index| Q::PaletteColour { index, own: None }));
    assert_eq!(scan_device_queries(burst.as_bytes()), expected);
}

#[test]
fn scan_ignores_out_of_range_index_and_non_queries() {
    // Index 255 is outside the sixteen colours psmux answers.
    assert!(scan_device_queries(b"\x1b]4;255;?\x1b\\").is_empty());
    // A color SET (no `?`) is not a query.
    assert!(scan_device_queries(b"\x1b]4;1;rgb:ff/00/00\x1b\\").is_empty());
    assert!(scan_device_queries(b"\x1b]11;rgb:ff/00/00\x1b\\").is_empty());
    // Plain output with escapes.
    assert!(scan_device_queries(b"\x1b[31mred\x1b[0m").is_empty());
    assert!(scan_device_queries(b"no escapes at all").is_empty());
}

#[test]
fn scanner_detects_query_split_across_batches() {
    // The parser thread hands the parser coalesced batches; a query split
    // across two of them is found once, when its last byte arrives.
    let q = b"\x1b]4;7;?\x07";
    for split in 1..q.len() {
        let mut s = DeviceQueryScanner::new();
        assert!(s.scan(&q[..split]).is_empty(), "split at {}", split);
        assert_eq!(s.scan(&q[split..]), vec![Q::PaletteColour { index: 7, own: None }], "split at {}", split);
    }
}

#[test]
fn scanner_does_not_double_count_completed_query() {
    // A query answered in batch N must not come back in batch N+1, which
    // would answer it twice.
    let mut s = DeviceQueryScanner::new();
    assert_eq!(s.scan(b"\x1b]4;3;?\x07"), vec![Q::PaletteColour { index: 3, own: None }]);
    assert!(s.scan(b"more output").is_empty());
}

#[test]
fn scanner_split_scheme_query() {
    let q = b"\x1b[?996n";
    for split in 1..q.len() {
        let mut s = DeviceQueryScanner::new();
        let mut found = s.scan(&q[..split]);
        found.extend(s.scan(&q[split..]));
        assert_eq!(found, vec![Q::ColourScheme], "split at {}", split);
    }
}

// ── HostColors model and wire format ─────────────────────────────────────

#[test]
fn host_colors_spec_roundtrip() {
    let mut hc = HostColors::empty();
    hc.fg = Some((0x65, 0x7B, 0x83));
    hc.bg = Some((0xFD, 0xF6, 0xE3));
    hc.palette[0] = Some((0x07, 0x36, 0x42));
    hc.palette[15] = Some((0xFD, 0xF6, 0xE3));
    hc.dark = Some(false);
    let spec = hc.to_spec();
    let parsed = HostColors::from_spec(&spec);
    assert_eq!(parsed, hc, "to_spec -> from_spec must roundtrip");
}

#[test]
fn host_colors_is_dark_prefers_explicit_flag_then_luminance() {
    let mut hc = HostColors::empty();
    hc.bg = Some((0xFD, 0xF6, 0xE3)); // Solarized Light base3 — light
    assert!(!hc.is_dark(), "light bg must classify as light");
    hc.bg = Some((0x0C, 0x0C, 0x0C));
    assert!(hc.is_dark(), "dark bg must classify as dark");
    hc.dark = Some(false);
    assert!(!hc.is_dark(), "explicit host report wins over luminance");
    let empty = HostColors::empty();
    assert!(empty.is_dark(), "unknown bg defaults to dark");
}

#[test]
fn parse_x11_color_accepts_common_forms() {
    assert_eq!(crate::types::parse_x11_color("rgb:fd/f6/e3"), Some((0xFD, 0xF6, 0xE3)));
    assert_eq!(crate::types::parse_x11_color("rgb:fdfd/f6f6/e3e3"), Some((0xFD, 0xF6, 0xE3)));
    assert_eq!(crate::types::parse_x11_color("#657b83"), Some((0x65, 0x7B, 0x83)));
    assert_eq!(crate::types::parse_x11_color("rgb:f/f/f"), Some((0xFF, 0xFF, 0xFF)));
    assert_eq!(crate::types::parse_x11_color("bogus"), None);
    assert_eq!(crate::types::parse_x11_color("rgb:zz/00/00"), None);
    assert_eq!(crate::types::parse_x11_color("rgb:00/00"), None);
    assert_eq!(crate::types::parse_x11_color("rgb:00/00/00/00"), None);
}

#[test]
fn parse_host_replies_windows_terminal_style() {
    // Replies as Windows Terminal sends them (ST-terminated), plus the
    // dark/light report and the DA1 sentinel that ends the read loop.
    let mut blob = String::new();
    blob.push_str("\x1b]10;rgb:6565/7b7b/8383\x1b\\");
    blob.push_str("\x1b]11;rgb:fdfd/f6f6/e3e3\x07"); // BEL-terminated variant
    for i in 0..16 {
        blob.push_str(&format!("\x1b]4;{};rgb:{:02x}{:02x}/00/ff\x1b\\", i, i, i));
    }
    blob.push_str("\x1b[?997;2n");
    blob.push_str("\x1b[?1;0c");
    let hc = crate::platform::parse_host_color_replies(blob.as_bytes());
    assert_eq!(hc.fg, Some((0x65, 0x7B, 0x83)));
    assert_eq!(hc.bg, Some((0xFD, 0xF6, 0xE3)));
    for i in 0..16 {
        // 4-digit channel "iiii" (e.g. 0101) scales to the 8-bit value i.
        assert_eq!(hc.palette[i], Some((i as u8, 0x00, 0xFF)), "palette {}", i);
    }
    assert_eq!(hc.dark, Some(false));
}

// ── Response builder (pipe-fallback path, child_pid = None) ──────────────

fn responses_for(bits: u32, colors: &HostColors) -> String {
    let mut out: Vec<u8> = Vec::new();
    crate::server::helpers::answer_color_queries(bits, &mut out, None, colors);
    String::from_utf8(out).unwrap()
}

#[test]
fn responder_answers_scheme_query() {
    let campbell = HostColors::campbell();
    let out = responses_for(COLOR_QUERY_SCHEME, &campbell);
    assert_eq!(out, "\x1b[?997;1n", "campbell (dark) must report dark");

    let mut light = HostColors::campbell();
    light.dark = Some(false);
    let out = responses_for(COLOR_QUERY_SCHEME, &light);
    assert_eq!(out, "\x1b[?997;2n", "light host must report light");
}

#[test]
fn responder_answers_single_palette_index_without_extras() {
    let campbell = HostColors::campbell();
    let out = responses_for(1 << 5, &campbell);
    assert_eq!(out, "\x1b]4;5;rgb:8888/1717/9898\x1b\\",
        "a single non-zero index query gets exactly one reply, no unsolicited fg/bg");
}

#[test]
fn responder_full_burst_answers_all_18_colors_and_scheme() {
    // The Copilot CLI startup burst: ?996n + OSC10 + OSC11 + OSC4 0..15.
    let bits = 0xFFFF | COLOR_QUERY_FG | COLOR_QUERY_BG | COLOR_QUERY_SCHEME;
    let campbell = HostColors::campbell();
    let out = responses_for(bits, &campbell);
    assert!(out.starts_with("\x1b[?997;1n"), "scheme reply present");
    assert!(out.contains("\x1b]10;rgb:cccc/cccc/cccc\x1b\\"), "fg reply present");
    assert!(out.contains("\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\"), "bg reply present");
    for i in 0..16 {
        assert!(out.contains(&format!("\x1b]4;{};rgb:", i)), "palette {} reply present", i);
    }
}

#[test]
fn responder_palette_burst_includes_fg_bg_even_when_conpty_ate_those_queries() {
    // On Win11 26200 ConPTY consumes the OSC 10;?/11;? queries before psmux
    // sees them, while the OSC 4 burst passes through.  When index 0 is
    // queried (burst marker) the fg/bg replies the app is simultaneously
    // waiting for must be included.
    let campbell = HostColors::campbell();
    let out = responses_for(0xFFFF, &campbell);
    assert!(out.contains("\x1b]10;"), "burst must include fg reply");
    assert!(out.contains("\x1b]11;"), "burst must include bg reply");
}

#[test]
fn responder_writes_nothing_for_empty_bits() {
    let campbell = HostColors::campbell();
    assert_eq!(responses_for(0, &campbell), "");
}

#[test]
fn responder_uses_host_reported_colors() {
    // Solarized Light host — exact values from issue #473's environment.
    let mut hc = HostColors::empty();
    hc.fg = Some((0x65, 0x7B, 0x83));
    hc.bg = Some((0xFD, 0xF6, 0xE3));
    for i in 0..16 { hc.palette[i] = Some((i as u8, i as u8, i as u8)); }
    let out = responses_for(0xFFFF | COLOR_QUERY_SCHEME, &hc);
    assert!(out.starts_with("\x1b[?997;2n"), "solarized light bg must classify light");
    assert!(out.contains("\x1b]10;rgb:6565/7b7b/8383\x1b\\"));
    assert!(out.contains("\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\"));
    assert!(out.contains("\x1b]4;3;rgb:0303/0303/0303\x1b\\"));
}

#[test]
fn campbell_fallback_has_complete_palette() {
    let c = HostColors::campbell();
    assert!(c.fg.is_some() && c.bg.is_some());
    assert!(c.palette.iter().all(|p| p.is_some()), "fallback must answer all 16 entries");
    assert!(c.is_dark());
}
