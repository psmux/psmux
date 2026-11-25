// Issue #1095: on a console host that does not parse win32 input mode (the
// inbox conhost of Windows 10 19045), the legacy byte of some keys reaches a
// record reader as another key: C-1 as a plain 1, C-Space as Ctrl+Shift+2,
// C-i as Tab, C-j as Ctrl+Enter, C-m as Enter, C-BSpace as Ctrl+H, S-Enter as
// Alt+Enter.  `legacy_host_key_record` names the record those keys must reach
// such a reader as, and it is the same record psmux writes in win32 form on a
// host that parses it.

use super::*;

const CTRL: u32 = LEFT_CTRL_PRESSED;
const SHIFT: u32 = SHIFT_PRESSED;

#[test]
fn ctrl_digits_and_space_carry_their_virtual_key_and_ctrl() {
    assert_eq!(legacy_host_key_record("C-1"), Some((0x31, 0, CTRL)));
    assert_eq!(legacy_host_key_record("C-0"), Some((0x30, 0, CTRL)));
    assert_eq!(legacy_host_key_record("C-9"), Some((0x39, 0, CTRL)));
    assert_eq!(legacy_host_key_record("C-Space"), Some((0x20, 0, CTRL)));
    assert_eq!(legacy_host_key_record("C-S-1"), Some((0x31, 0, CTRL | SHIFT)));
    // Any order and case, as parse_key_string reads them.
    assert_eq!(legacy_host_key_record("S-C-1"), Some((0x31, 0, CTRL | SHIFT)));
    assert_eq!(legacy_host_key_record("c-1"), Some((0x31, 0, CTRL)));
}

#[test]
fn the_digit_record_is_the_one_written_in_win32_form_on_a_parsing_host() {
    for (c, shift) in [('1', false), ('0', false), ('5', true), (' ', false)] {
        let name = format!("{}{}", if shift { "C-S-" } else { "C-" }, if c == ' ' { "Space".to_string() } else { c.to_string() });
        let (vk, uchar, cs) = legacy_host_key_record(&name).expect(&name);
        let scan = crate::platform::mouse_inject::vk_to_scan(vk);
        assert_eq!(Some(win32_input_key_seq(vk, scan, uchar, cs)), ctrl_key_win32_seq(c, shift), "{}", name);
    }
}

#[test]
fn only_the_ctrl_letters_the_host_misreads() {
    assert_eq!(legacy_host_key_record("C-i"), Some((0x49, 0x09, CTRL)));
    assert_eq!(legacy_host_key_record("C-j"), Some((0x4A, 0x0A, CTRL)));
    assert_eq!(legacy_host_key_record("C-m"), Some((0x4D, 0x0D, CTRL)));
    assert_eq!(legacy_host_key_record("C-J"), Some((0x4A, 0x0A, CTRL)));
    // Measured to arrive as their own Ctrl+letter on 19045: the byte stays.
    for c in "abdefghklnopqrstuvwxyz".chars() {
        assert_eq!(legacy_host_key_record(&format!("C-{}", c)), None, "C-{}", c);
    }
    // Ctrl+C keeps its interrupt route.
    assert_eq!(legacy_host_key_record("C-c"), None);
}

#[test]
fn ctrl_backspace_and_shift_enter() {
    // What the 26300 host itself makes of the byte 08 (#610) and what psmux
    // writes in win32 form for Shift+Enter.
    assert_eq!(legacy_host_key_record("C-BSpace"), Some((0x08, 0x7F, CTRL)));
    assert_eq!(legacy_host_key_record("S-Enter"), Some((0x0D, 0x0D, SHIFT)));
}

#[test]
fn every_other_key_keeps_its_legacy_bytes() {
    for name in [
        "1", "a", "Space", "Enter", "C-Enter", "M-Enter", "BSpace", "M-BSpace", "S-BSpace",
        "M-1", "C-M-1", "C-M-j", "M-j", "Tab", "C-Tab", "Up", "C-Up", "F1", "Escape", "C-[",
        " C-1", "C-1 ", "",
    ] {
        assert_eq!(legacy_host_key_record(name), None, "{:?}", name);
    }
}

#[test]
fn only_the_windows_10_inbox_host_gets_the_records() {
    // The host measured to misread the bytes: the inbox conhost before 22000.
    assert!(legacy_host_misreads_key_bytes(Some(19045), false));
    assert!(legacy_host_misreads_key_bytes(Some(WIN32_INPUT_INBOX_MIN_BUILD - 1), false));
    // Windows 11 keeps what it had whatever the stream carried, and so do an
    // unknown build and a host supplied with PSMUX_CONPTY_DIR.
    for build in [Some(WIN32_INPUT_INBOX_MIN_BUILD), Some(22523), Some(26100), Some(26300), None] {
        assert!(!legacy_host_misreads_key_bytes(build, false), "{:?}", build);
    }
    assert!(!legacy_host_misreads_key_bytes(Some(19045), true), "OpenConsole on Windows 10");
}
