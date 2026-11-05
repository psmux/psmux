// Where the mouse keys and the user keys are stored (crate::key_base): U+10E000
// in Supplementary Private Use Area-B, clear of the icon fonts. The font
// ranges are the ones counted from Nerd Fonts 3.5.1's glyphnames.json and
// from an SF Symbols 5.1 export.

use super::*;

/// Nerd Fonts 3.5.1: 4092 icons in the BMP private use area and its 6896
/// Material Design icons in Supplementary Private Use Area-A.
const NERD_FONTS: [(u32, u32); 2] = [(0xE000, 0xF8FF), (0xF0001, 0xF1AF0)];
/// SF Symbols 5.1: 5359 symbols packed from the start of Area-B.
const SF_SYMBOLS: (u32, u32) = (0x100000, 0x1022CF);

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 <= b.1 && b.0 <= a.1
}

#[test]
fn the_range_is_clear_of_the_icon_fonts() {
    let keys = (BASE, BASE + TOTAL - 1);
    assert_eq!(keys, (0x10E000, 0x10F068));
    for font in NERD_FONTS.iter().copied().chain([SF_SYMBOLS]) {
        assert!(!overlaps(keys, font), "U+{:X}..U+{:X} meets U+{:X}..U+{:X}", keys.0, keys.1, font.0, font.1);
    }
    assert!(keys.1 <= 0x10FFFD, "the keys stay inside Area-B");
}

#[test]
fn mouse_keys_come_first_and_user_keys_right_after() {
    assert_eq!(mouse_base(), BASE);
    assert_eq!(user_base(), BASE + MOUSE_SPAN);
    // The two ranges meet and do not overlap.
    let last_mouse = char::from_u32(user_base() - 1).unwrap();
    let first_user = char::from_u32(user_base()).unwrap();
    assert!(crate::mouse_keys::is_mouse_char(last_mouse));
    assert!(!crate::user_keys::is_user_char(last_mouse));
    assert!(crate::user_keys::is_user_char(first_user));
    assert!(!crate::mouse_keys::is_mouse_char(first_user));
    assert_eq!(crate::user_keys::char_for(0), Some(first_user));
    assert_eq!(crate::user_keys::char_for(1000).map(|c| c as u32), Some(BASE + TOTAL - 1));
}

#[test]
fn the_old_ranges_are_text_now() {
    // Before the move the mouse keys were U+F0000 up and the user keys U+F1000
    // to U+F13E8, inside the Nerd Fonts Material Design range. Those
    // characters are no keys now.
    for v in [0xF0000u32, 0xF0010, 0xF08C0, 0xF1000, 0xF13E8, 0xF1AF0] {
        let c = char::from_u32(v).unwrap();
        assert!(!crate::mouse_keys::is_mouse_char(c), "U+{v:X}");
        assert!(!crate::user_keys::is_user_char(c), "U+{v:X}");
    }
    // A name still round trips through its new character.
    for name in ["WheelUpPane", "MouseDown1Pane", "MouseMoveControl9"] {
        let key = crate::mouse_keys::parse(name).unwrap();
        assert_eq!(crate::config::format_key_binding(&key), name);
    }
    assert_eq!(crate::user_keys::name(crate::user_keys::char_for(0).unwrap()).as_deref(), Some("User0"));
}
