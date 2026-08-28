// Issue #616: `bind` silently ignored non-ASCII key names.
//
// `parse_key_name` gated its single-character branches on `str::len()`, which is
// a BYTE count. Every key outside ASCII is two or more UTF-8 bytes, so
// `bind <cyrillic yeru>` fell straight through to `None`, the config line was
// dropped without a warning, and `list-keys` never mentioned it. The modifier
// branch had the same gate, so `M-<ef>` and `C-<yeru>` died the same way.
// `parse_key_string`, the parser behind the CLI / server route, repeated the
// mistake in its own single-character arm.
//
// tmux (key-string.c, key_string_lookup_string) takes the ASCII fast path only
// when `string[1] == '\0' && string[0] <= 127`, and decodes anything else as
// UTF-8 before OR-ing in the modifiers, so all of these are legal key names
// there. `key_string_lookup_key` prints the raw UTF-8 back, which is what makes
// `list-keys` show the character as typed.
//
// The end to end proof (config boot, CLI, command prompt, and real keystrokes
// injected into an attached client) lives in tests/test_issue616_unicode_bind.ps1.

use super::*;

fn app() -> AppState {
    AppState::new("i616_test".to_string())
}

// The reporter's key and its friends, by codepoint so this file stays readable
// no matter what an editor does to it.
const YERU: char = '\u{044B}'; // CYRILLIC SMALL LETTER YERU, 2 bytes UTF-8
const EF: char = '\u{0444}'; // CYRILLIC SMALL LETTER EF, 2 bytes
const EACUTE: char = '\u{00E9}'; // LATIN SMALL LETTER E WITH ACUTE, 2 bytes
const SHARP_S: char = '\u{00DF}'; // LATIN SMALL LETTER SHARP S, 2 bytes
const SNOWMAN: char = '\u{2603}'; // SNOWMAN, 3 bytes
const ROCKET: char = '\u{1F680}'; // ROCKET, 4 bytes, one scalar

// ─── parse_key_name: bare Unicode keys ──────────────────────────────────────

#[test]
fn issue616_bare_cyrillic_key_parses() {
    assert_eq!(
        parse_key_name(&YERU.to_string()),
        Some((KeyCode::Char(YERU), KeyModifiers::NONE)),
        "a 2 byte key name must parse as one character, not be rejected by byte length"
    );
}

#[test]
fn issue616_every_multibyte_width_parses() {
    // 2, 3 and 4 byte UTF-8 all have to work: the old gate was `len() == 1`,
    // which only ever admitted 1 byte names.
    for c in [YERU, EF, EACUTE, SHARP_S, SNOWMAN, ROCKET] {
        assert_eq!(
            parse_key_name(&c.to_string()),
            Some((KeyCode::Char(c), KeyModifiers::NONE)),
            "U+{:04X} ({} UTF-8 bytes) should parse as itself",
            c as u32,
            c.len_utf8()
        );
    }
}

#[test]
fn issue616_ascii_keys_are_unchanged() {
    assert_eq!(parse_key_name("s"), Some((KeyCode::Char('s'), KeyModifiers::NONE)));
    // Case is preserved for a bare key: tmux and psmux both treat `T` and `t`
    // as different bindings (issue #157).
    assert_eq!(parse_key_name("T"), Some((KeyCode::Char('T'), KeyModifiers::NONE)));
    assert_eq!(parse_key_name("|"), Some((KeyCode::Char('|'), KeyModifiers::NONE)));
}

#[test]
fn issue616_multi_character_name_is_still_rejected() {
    // The fix must widen "one byte" to "one character", NOT to "any string".
    // A two character ASCII name is not a key and must stay None so the
    // unknown-key diagnostic still fires.
    assert_eq!(parse_key_name("ab"), None);
    assert_eq!(parse_key_name("NotARealKeyName"), None);
    // Two Cyrillic characters is just as invalid as two Latin ones. Under the
    // old byte gate this was 4 bytes and under a naive `len() >= 1` fix it
    // would have been accepted as garbage.
    let two = format!("{}{}", YERU, EF);
    assert_eq!(parse_key_name(&two), None);
}

// ─── parse_key_name: modifier forms ─────────────────────────────────────────

#[test]
fn issue616_meta_cyrillic_parses() {
    assert_eq!(
        parse_key_name(&format!("M-{}", EF)),
        Some((KeyCode::Char(EF), KeyModifiers::ALT)),
        "M-<U+0444> is the reporter's second example"
    );
}

#[test]
fn issue616_ctrl_cyrillic_parses() {
    assert_eq!(
        parse_key_name(&format!("C-{}", YERU)),
        Some((KeyCode::Char(YERU), KeyModifiers::CONTROL))
    );
}

#[test]
fn issue616_shift_cyrillic_names_the_shifted_character() {
    // psmux resolves `S-<x>` to the SHIFTED character and drops the SHIFT bit,
    // because `normalize_key_for_binding` strips SHIFT from every Char and the
    // two bindings would otherwise collapse onto each other. That rule was
    // ASCII only (`to_ascii_uppercase`), so it had to become Unicode aware at
    // the same time as the length gate.
    assert_eq!(
        parse_key_name(&format!("S-{}", YERU)),
        Some((KeyCode::Char('\u{042B}'), KeyModifiers::NONE)),
        "S-<U+044B> is the capital U+042B"
    );
    // ASCII behaviour is untouched.
    assert_eq!(parse_key_name("S-a"), Some((KeyCode::Char('A'), KeyModifiers::NONE)));
}

#[test]
fn issue616_shift_of_a_character_with_no_single_scalar_uppercase() {
    // U+00DF uppercases to "SS", two scalars. A key name is one character, so
    // the original character is kept rather than producing a two character key
    // or dropping the binding.
    assert_eq!(
        parse_key_name(&format!("S-{}", SHARP_S)),
        Some((KeyCode::Char(SHARP_S), KeyModifiers::NONE))
    );
}

#[test]
fn issue616_combined_modifiers_on_a_unicode_key() {
    assert_eq!(
        parse_key_name(&format!("C-M-{}", YERU)),
        Some((KeyCode::Char(YERU), KeyModifiers::CONTROL | KeyModifiers::ALT))
    );
}

// ─── parse_key_string: the CLI / server route ───────────────────────────────

#[test]
fn issue616_server_parser_takes_unicode_too() {
    // `CtrlReq::BindKey` (every `psmux bind-key ...` from the command line and
    // from the TUI command prompt) goes through this second parser, which had
    // its own byte-length arm.
    assert_eq!(
        parse_key_string(&YERU.to_string()),
        Some((KeyCode::Char(YERU), KeyModifiers::empty()))
    );
    assert_eq!(
        parse_key_string(&format!("M-{}", EF)),
        Some((KeyCode::Char(EF), KeyModifiers::ALT))
    );
    assert_eq!(parse_key_string("NotARealKeyName"), None);
}

// ─── the whole config path ──────────────────────────────────────────────────

fn prefix_keys(a: &AppState) -> Vec<(KeyCode, KeyModifiers)> {
    a.key_tables
        .get("prefix")
        .map(|t| t.iter().map(|b| b.key).collect())
        .unwrap_or_default()
}

#[test]
fn issue616_config_bind_lands_in_the_prefix_table() {
    let mut a = app();
    let conf = format!(
        "bind s display-message \"latin works\"\nbind {} display-message \"cyrillic test\"\n",
        YERU
    );
    crate::config::parse_config_content(&mut a, &conf);
    let keys = prefix_keys(&a);
    assert!(
        keys.contains(&(KeyCode::Char('s'), KeyModifiers::NONE)),
        "the control binding must be there or the test proves nothing"
    );
    assert!(
        keys.contains(&(KeyCode::Char(YERU), KeyModifiers::NONE)),
        "prefix table is missing the Cyrillic key: {:?}",
        keys
    );
}

#[test]
fn issue616_config_modifier_binds_land_too() {
    let mut a = app();
    let conf = format!(
        "bind M-{} display-message meta\nbind C-{} display-message ctrl\n",
        EF, YERU
    );
    crate::config::parse_config_content(&mut a, &conf);
    let keys = prefix_keys(&a);
    assert!(keys.contains(&(KeyCode::Char(EF), KeyModifiers::ALT)), "{:?}", keys);
    assert!(keys.contains(&(KeyCode::Char(YERU), KeyModifiers::CONTROL)), "{:?}", keys);
}

#[test]
fn issue616_list_keys_renders_the_bare_character() {
    // tmux key_string_lookup_key prints the raw UTF-8 for a Unicode key and
    // keeps the C- / M- prefix on the modifier forms; `list-keys` must show the
    // character the user typed, not an escape or a number.
    assert_eq!(
        format_key_binding(&(KeyCode::Char(YERU), KeyModifiers::NONE)),
        YERU.to_string()
    );
    assert_eq!(
        format_key_binding(&(KeyCode::Char(EF), KeyModifiers::ALT)),
        format!("M-{}", EF)
    );
}

#[test]
fn issue616_unbind_finds_the_unicode_key_again() {
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        &format!("bind {} display-message x\nbind M-{} display-message y\n", YERU, EF),
    );
    assert!(prefix_keys(&a).contains(&(KeyCode::Char(YERU), KeyModifiers::NONE)));
    crate::config::parse_config_content(&mut a, &format!("unbind {}\n", YERU));
    let keys = prefix_keys(&a);
    assert!(!keys.contains(&(KeyCode::Char(YERU), KeyModifiers::NONE)), "{:?}", keys);
    assert!(
        keys.contains(&(KeyCode::Char(EF), KeyModifiers::ALT)),
        "unbind must remove one key, not clear the table: {:?}",
        keys
    );
}

// ─── the diagnostic ─────────────────────────────────────────────────────────

#[test]
fn issue616_unparseable_key_warns_instead_of_vanishing() {
    // The silence is half the bug: with no warning and nothing in list-keys the
    // reporter had no way to tell a rejected key from a working one. tmux says
    // `unknown key: <name>` (cmd-bind-key.c).
    let mut a = app();
    crate::config::parse_config_content(&mut a, "bind NotARealKeyName display-message hi\n");
    assert!(
        a.config_warnings.iter().any(|w| w.contains("unknown key: NotARealKeyName")),
        "expected an unknown-key warning, got {:?}",
        a.config_warnings
    );
}

#[test]
fn issue616_valid_unicode_key_warns_about_nothing() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, &format!("bind {} display-message hi\n", YERU));
    assert!(
        !a.config_warnings.iter().any(|w| w.contains("unknown key")),
        "a legal Unicode key must not warn: {:?}",
        a.config_warnings
    );
}

#[test]
fn issue616_mouse_key_names_are_not_typos() {
    // The new diagnostic must not fire on the names tmux accepts and psmux
    // simply does not model. `bind -n WheelUpPane ...` is ordinary in a ported
    // config (psmux's own FAQ quotes tmux's default binding for it) and has
    // always been taken quietly; turning that into a warning at boot and a
    // non-zero exit from the CLI would be a regression, not tmux parity.
    for name in [
        "WheelUpPane", "WheelDownPane", "MouseDown1Pane", "MouseUp1Status",
        "MouseDrag1Border", "MouseDragEnd1Pane", "DoubleClick1Pane",
        "TripleClick1StatusLeft", "SecondClick1Pane", "MouseDown3ScrollbarSlider",
        "MouseDown1StatusDefault", "MouseDown1Empty",
        "C-WheelUpPane", "M-MouseDown1Pane",
        "Any", "None", "FocusIn", "FocusOut", "PasteStart", "MouseMovePane",
        "User0", "User12",
    ] {
        assert!(
            is_unmodelled_tmux_key_name(name),
            "{} is a real tmux key name and must not be reported as unknown",
            name
        );
    }
    // ... while a genuine typo is still caught, including near misses.
    for name in [
        "NotARealKeyName", "WheelUpPain", "MouseDownPane", "MouseDown1Panel",
        "Wheel", "User", "Userx", "ab",
    ] {
        assert!(
            !is_unmodelled_tmux_key_name(name),
            "{} is not a tmux key name and must still be diagnosed",
            name
        );
    }
}

#[test]
fn issue616_mouse_binding_in_a_config_stays_silent() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, "bind -n WheelUpPane select-pane\n");
    assert!(
        !a.config_warnings.iter().any(|w| w.contains("unknown key")),
        "a mouse binding must not warn: {:?}",
        a.config_warnings
    );
}

#[test]
fn issue616_unbind_of_an_unparseable_key_warns() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, "unbind NotARealKeyName\n");
    assert!(
        a.config_warnings.iter().any(|w| w.contains("unknown key: NotARealKeyName")),
        "got {:?}",
        a.config_warnings
    );
}
