// Issue #610: Ctrl+Backspace lost its modifier end to end, so no word delete.
//
// Two independent defects, both reproduced with real measurements before the
// fix was written:
//
//   1. src/client.rs pushed a bare "send-key backspace" for EVERY Backspace,
//      whatever modifiers were held.  The client's own input_debug.log showed
//      `Key code=Backspace mods=KeyModifiers(CONTROL)` followed by
//      `[send] -> send-key backspace`, next to `Char('h') mods=CONTROL` which
//      correctly became `send-key C-h`.  The modifier died on the wire, so no
//      server side bind-key could ever see a modified Backspace.
//
//   2. parse_modified_special_key had no Backspace arm.  `send-keys C-BSpace`
//      therefore fell through to the generic "C-" handler, which takes the
//      character at index 2 of the uppercased name "C-BSPACE", i.e. 'B', and
//      wrote Ctrl+B (0x02) into the pane.  Measured: a byte logging pane
//      received `02` for C-BSpace and `1b 42` (ESC 'B') for M-BSpace.
//
// The byte values asserted here are not a guess.  They were measured in both
// directions against a real pseudoconsole, the same thing psmux hands a pane:
//
//   ConPTY input parser, bytes in -> INPUT_RECORD out:
//     7f            -> vk=0x08 uChar=0x0008 ctrl=0x0000
//     08            -> vk=0x08 uChar=0x007F ctrl=0x0008 (LEFT_CTRL_PRESSED)
//     1b 7f         -> vk=0x08 uChar=0x0008 ctrl=0x0002 (LEFT_ALT_PRESSED)
//     1b 08         -> vk=0x08 uChar=0x007F ctrl=0x000A (LEFT_CTRL|LEFT_ALT)
//     ESC [ 127;5 u -> NOTHING, the CSI u form is discarded
//     ESC [ 27;5;127 ~ -> NOTHING
//
//   conhost record -> VT encoder, the native contract a terminal delivers:
//     Backspace       -> 7f
//     Ctrl+Backspace  -> 08
//     Shift+Backspace -> 7f
//     Alt+Backspace   -> 1b 7f
//
// tmux 3.4 for comparison (measured in WSL with a raw tty byte logger):
//   send-keys BSpace   -> 7f        send-keys M-BSpace -> 1b 7f
//   send-keys C-BSpace -> nothing at all (tmux has no encoding for it)
// psmux matches tmux where tmux has an answer, and uses the native Windows
// encoding where tmux has none, which is the only value that makes a record
// reading app such as PSReadLine run BackwardKillWord.

use crate::input::parse_modified_special_key;
use crate::client::modified_key_name;
use crate::config::{parse_key_string, normalize_key_for_binding};
use crossterm::event::{KeyCode, KeyModifiers};

// ── The output encoding ──────────────────────────────────────────────────────

#[test]
fn ctrl_backspace_is_the_single_byte_0x08() {
    assert_eq!(parse_modified_special_key("C-BSpace"), Some("\u{8}".to_string()));
    assert_eq!(parse_modified_special_key("C-Backspace"), Some("\u{8}".to_string()));
    // The name arrives lowercased on some routes.
    assert_eq!(parse_modified_special_key("c-bspace"), Some("\u{8}".to_string()));
}

#[test]
fn alt_backspace_is_esc_del_matching_tmux() {
    assert_eq!(parse_modified_special_key("M-BSpace"), Some("\u{1b}\u{7f}".to_string()));
    assert_eq!(parse_modified_special_key("M-Backspace"), Some("\u{1b}\u{7f}".to_string()));
}

#[test]
fn shift_backspace_collapses_onto_del() {
    // conhost sends a plain 0x7f for Shift+Backspace: there is no distinct
    // encoding on either side, so S- must not invent one.
    assert_eq!(parse_modified_special_key("S-BSpace"), Some("\u{7f}".to_string()));
}

#[test]
fn ctrl_alt_backspace_is_esc_0x08() {
    assert_eq!(parse_modified_special_key("C-M-BSpace"), Some("\u{1b}\u{8}".to_string()));
    // Ctrl wins over Shift for the base byte, and Alt still prefixes.
    assert_eq!(parse_modified_special_key("C-S-BSpace"), Some("\u{8}".to_string()));
    assert_eq!(parse_modified_special_key("M-S-BSpace"), Some("\u{1b}\u{7f}".to_string()));
}

#[test]
fn unmodified_backspace_is_not_claimed_by_this_function() {
    // Plain Backspace keeps its existing dedicated route (0x7f); this helper
    // only ever handles MODIFIED keys, so it must decline the bare name.
    assert_eq!(parse_modified_special_key("BSpace"), None);
    assert_eq!(parse_modified_special_key("Backspace"), None);
    assert_eq!(parse_modified_special_key("backspace"), None);
}

#[test]
fn other_modified_special_keys_are_unchanged() {
    // Guard the neighbours: the Backspace arm must not disturb the CSI forms.
    assert_eq!(parse_modified_special_key("C-Left"), Some("\u{1b}[1;5D".to_string()));
    assert_eq!(parse_modified_special_key("S-Right"), Some("\u{1b}[1;2C".to_string()));
    assert_eq!(parse_modified_special_key("C-Delete"), Some("\u{1b}[3;5~".to_string()));
    // Ctrl+Enter is LF on Windows (#409) and CSI 13;5~ elsewhere; either way
    // the Backspace arm must not touch it.
    #[cfg(windows)]
    assert_eq!(parse_modified_special_key("C-Enter"), Some("\n".to_string()));
    #[cfg(not(windows))]
    assert_eq!(parse_modified_special_key("C-Enter"), Some("\u{1b}[13;5~".to_string()));
}

// ── The client side key NAME ─────────────────────────────────────────────────

#[test]
fn client_names_backspace_with_its_modifiers() {
    // The regression: this used to be the constant "backspace" for every case.
    assert_eq!(modified_key_name("Backspace", KeyModifiers::NONE), "backspace");
    assert_eq!(modified_key_name("Backspace", KeyModifiers::CONTROL), "C-Backspace");
    assert_eq!(modified_key_name("Backspace", KeyModifiers::ALT), "M-Backspace");
    assert_eq!(modified_key_name("Backspace", KeyModifiers::SHIFT), "S-Backspace");
    assert_eq!(
        modified_key_name("Backspace", KeyModifiers::CONTROL | KeyModifiers::ALT),
        "C-M-Backspace"
    );
}

#[test]
fn client_name_and_output_encoding_agree_end_to_end() {
    // The whole chain in one assertion: what the client puts on the wire for a
    // Ctrl+Backspace keystroke must be a name the pane writer encodes as 0x08.
    let wire = modified_key_name("Backspace", KeyModifiers::CONTROL);
    assert_eq!(parse_modified_special_key(&wire), Some("\u{8}".to_string()));

    let wire_alt = modified_key_name("Backspace", KeyModifiers::ALT);
    assert_eq!(parse_modified_special_key(&wire_alt), Some("\u{1b}\u{7f}".to_string()));

    // And the unmodified name still means plain Backspace, which this helper
    // declines so the dedicated 0x7f route keeps handling it.
    let wire_plain = modified_key_name("Backspace", KeyModifiers::NONE);
    assert_eq!(wire_plain, "backspace");
    assert_eq!(parse_modified_special_key(&wire_plain), None);
}

// ── bind-key C-BSpace must parse and must stay distinct ──────────────────────

#[test]
fn bind_key_parses_the_modified_backspace_names() {
    assert_eq!(
        parse_key_string("C-BSpace"),
        Some((KeyCode::Backspace, KeyModifiers::CONTROL))
    );
    assert_eq!(
        parse_key_string("C-Backspace"),
        Some((KeyCode::Backspace, KeyModifiers::CONTROL))
    );
    assert_eq!(
        parse_key_string("M-BSpace"),
        Some((KeyCode::Backspace, KeyModifiers::ALT))
    );
    assert_eq!(
        parse_key_string("S-BSpace"),
        Some((KeyCode::Backspace, KeyModifiers::SHIFT))
    );
    assert_eq!(
        parse_key_string("BSpace"),
        Some((KeyCode::Backspace, KeyModifiers::NONE))
    );
}

#[test]
fn bound_ctrl_backspace_matches_a_real_ctrl_backspace_event() {
    // A binding matches when the normalised binding tuple equals the normalised
    // tuple of the incoming key event, which is exactly what the client does.
    let bound = normalize_key_for_binding(parse_key_string("C-BSpace").unwrap());
    let pressed = normalize_key_for_binding((KeyCode::Backspace, KeyModifiers::CONTROL));
    assert_eq!(bound, pressed, "bind-key C-BSpace must match a Ctrl+Backspace press");

    // ... and must NOT swallow a plain Backspace, nor alias Ctrl+H.
    let plain = normalize_key_for_binding((KeyCode::Backspace, KeyModifiers::NONE));
    assert_ne!(bound, plain, "C-BSpace must not match a bare Backspace");
    let ctrl_h = normalize_key_for_binding((KeyCode::Char('h'), KeyModifiers::CONTROL));
    assert_ne!(bound, ctrl_h, "C-BSpace and C-h are different keys on Windows");
}

// ── The VT input path: SSH, WezTerm and JetBrains clients ────────────────────
//
// needs_vt_input() routes SSH sessions, WezTerm and JetBrains terminals through
// the VT parser instead of the console reader, and that parser carried the SAME
// modifier-blind bug: a special case mapped the incoming byte 0x08 to an
// UNMODIFIED Backspace.  Measured on a real attached client with a byte logging
// pane, injecting real keystrokes (a marker digit, then the key):
//
//   console path             1 7F  2 08  3 08  4 17  5 1B 7F   correct
//   SSH_CONNECTION set       1 7F  2 7F  3 7F  4 17  5 1B 7F   broken
//   TERM_PROGRAM=WezTerm     1 7F  2 7F  3 7F  4 17  5 1B 7F   broken
//   markers: 1 Backspace, 2 Ctrl+Backspace, 3 Ctrl+H, 4 Ctrl+W, 5 Alt+Backspace
//
// So on those clients Ctrl+Backspace lost its word delete, and Ctrl+H was
// additionally corrupted from 0x08 into 0x7f, which no interpretation allows.
//
// 0x08 must decode to C-h.  That is not a preference: writing a raw 0x08 into a
// real tmux 3.4 client pty, with both candidates bound in the root table, fired
// `bind-key -n C-h` and never `bind-key -n C-BSpace`.  Both candidate decodes
// would put the same 0x08 byte on the pane, so PSReadLine kills a word either
// way; tmux settles only which NAME a binding sees.

use crate::ssh_input::test_support::decode_vt_byte;

#[test]
fn vt_input_decodes_0x08_as_ctrl_h_like_tmux() {
    assert_eq!(
        decode_vt_byte(0x08),
        Some((KeyCode::Char('h'), KeyModifiers::CONTROL)),
        "0x08 must decode to C-h, matching what a raw 0x08 fires in tmux 3.4"
    );
}

#[test]
fn vt_input_keeps_plain_backspace_on_0x7f() {
    // Terminals send 0x7f for the Backspace key; that must not move.
    assert_eq!(
        decode_vt_byte(0x7f),
        Some((KeyCode::Backspace, KeyModifiers::NONE)),
        "0x7f is the plain Backspace key and must stay unmodified"
    );
}

#[test]
fn vt_input_0x08_no_longer_collapses_onto_bare_backspace() {
    // The exact regression: 0x08 used to come out as an unmodified Backspace,
    // indistinguishable from 0x7f, which is what erased the modifier.
    assert_ne!(
        decode_vt_byte(0x08),
        decode_vt_byte(0x7f),
        "0x08 and 0x7f are different keys and must not decode to the same event"
    );
}

#[test]
fn vt_input_other_control_bytes_are_unchanged() {
    // Guard the neighbours of the arm that was removed.
    assert_eq!(decode_vt_byte(0x17), Some((KeyCode::Char('w'), KeyModifiers::CONTROL)));
    assert_eq!(decode_vt_byte(0x01), Some((KeyCode::Char('a'), KeyModifiers::CONTROL)));
    assert_eq!(decode_vt_byte(0x1a), Some((KeyCode::Char('z'), KeyModifiers::CONTROL)));
    assert_eq!(decode_vt_byte(0x00), Some((KeyCode::Char(' '), KeyModifiers::CONTROL)));
    assert_eq!(decode_vt_byte(0x09), Some((KeyCode::Tab, KeyModifiers::NONE)));
    assert_eq!(decode_vt_byte(0x0d), Some((KeyCode::Enter, KeyModifiers::NONE)));
}

#[test]
fn vt_input_ctrl_h_round_trips_back_to_0x08_on_the_pane() {
    // End to end on this path: the decoded key must be one the pane writer
    // encodes back into the byte the terminal sent, so 0x08 in means 0x08 out.
    let (code, mods) = decode_vt_byte(0x08).expect("0x08 must decode to something");
    assert_eq!(code, KeyCode::Char('h'));
    assert!(mods.contains(KeyModifiers::CONTROL));
    assert_eq!(
        crate::input::ctrl_char_send_keys_byte('h'),
        Some(0x08),
        "C-h must be written to the pane as 0x08, closing the round trip"
    );
}
