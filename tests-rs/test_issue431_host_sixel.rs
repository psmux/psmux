// Issue #431: the client writes sixel only to a terminal that can draw it.
//
// tmux turns its sixel feature on from a DA1 reply listing 4 after a
// conformance level of 61 to 65 (tty_keys_device_attributes, tty-keys.c:1528 in tmux 3.8)
// and draws no image without it (tty_cmd_sixelimage, tty.c:2191).  The client
// reads the same reply from the colour query's drain, where DA1 is the
// sentinel that ends it.
use crate::platform::parse_da1_sixel;

#[test]
fn windows_terminal_lists_sixel() {
    // Measured on Windows Terminal 1.24.
    let reply = b"\x1b[?61;4;6;7;14;21;22;23;24;28;32;42;52c";
    assert_eq!(parse_da1_sixel(reply), Some(true));
}

#[test]
fn a_reply_without_4_says_no() {
    assert_eq!(parse_da1_sixel(b"\x1b[?61;6;7;14;21;22;23;24;28;32;42c"), Some(false));
    // A VT100 style reply names no conformance level, so 2 is not a feature.
    assert_eq!(parse_da1_sixel(b"\x1b[?1;2c"), Some(false));
    // 4 as the level is not the sixel feature either.
    assert_eq!(parse_da1_sixel(b"\x1b[?4;6c"), Some(false));
}

#[test]
fn the_other_replies_in_the_drain_are_skipped() {
    // DECRQM and the colour scheme reply open the same way as DA1.
    let buf = b"\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\\x1b[?997;1n\x1b[?12;1$y\x1bP1$r0 q\x1b\\\x1b[?61;4c";
    assert_eq!(parse_da1_sixel(buf), Some(true));
}

#[test]
fn no_da1_reply_is_none() {
    assert_eq!(parse_da1_sixel(b""), None);
    assert_eq!(parse_da1_sixel(b"\x1b[?12;1$y"), None);
    // Cut off before the final byte.
    assert_eq!(parse_da1_sixel(b"\x1b[?61;4"), None);
}

#[test]
fn under_auto_only_a_yes_draws() {
    use crate::rendering::{note_host_sixel, sixel_draws};
    use crate::types::SixelMode;
    note_host_sixel(Some(false));
    assert!(!sixel_draws(SixelMode::Auto), "the terminal said it has no sixel");
    note_host_sixel(Some(true));
    assert!(sixel_draws(SixelMode::Auto));
    // Not asked, or no answer: tmux writes no image, and neither does auto.
    note_host_sixel(None);
    assert!(!sixel_draws(SixelMode::Auto));
    assert!(sixel_draws(SixelMode::On), "on draws without a reply");
}
