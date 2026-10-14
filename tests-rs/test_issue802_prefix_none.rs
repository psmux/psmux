// Issue #802: `set prefix None` was accepted and silently ignored, so the old
// prefix kept working and the nested session toggle could not release it.
//
// tmux types prefix and prefix2 OPTIONS_TABLE_KEY. "None" (any case) is a
// real key there, KEYC_NONE (key-string.c:254), which never matches typed
// input, so it turns the prefix off; show prints it back as "None"
// (key-string.c:355). Anything else key_string_lookup_string cannot read is
// refused with "bad key: X" (options.c:1305) and the option is left alone.
//
// psmux stored the prefix as a plain (KeyCode, KeyModifiers), had no way to
// say "no prefix", and both setters skipped the assignment when the parse
// failed, without a word.

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

const CTRL: KeyModifiers = KeyModifiers::CONTROL;

fn app() -> AppState {
    AppState::new("issue802".to_string())
}

fn ctrl(c: char) -> Option<(KeyCode, KeyModifiers)> {
    Some((KeyCode::Char(c), CTRL))
}

#[test]
fn none_in_any_case_parses_to_no_key() {
    for v in ["None", "none", "NONE", " None "] {
        assert_eq!(parse_option_key(v, false), Ok(None), "command route: {v:?}");
        assert_eq!(parse_option_key(v, true), Ok(None), "config route: {v:?}");
    }
}

#[test]
fn unparsable_key_is_a_bad_key_error() {
    assert_eq!(parse_option_key("NotAKey", false), Err("bad key: NotAKey".to_string()));
    assert_eq!(parse_option_key("NotAKey", true), Err("bad key: NotAKey".to_string()));
    assert_eq!(parse_option_key("C-a", false), Ok(ctrl('a')));
    assert_eq!(parse_option_key("C-a", true), Ok(ctrl('a')));
}

#[test]
fn format_prints_none_like_tmux() {
    assert_eq!(format_option_key(&None), "None");
    assert_eq!(format_option_key(&ctrl('a')), "C-a");
}

#[test]
fn set_prefix_none_turns_the_prefix_off() {
    let mut a = app();
    crate::server::options::apply_set_option(&mut a, "prefix", "C-a", false).unwrap();
    assert_eq!(a.prefix_key, ctrl('a'));
    crate::server::options::apply_set_option(&mut a, "prefix", "None", false).unwrap();
    assert_eq!(a.prefix_key, None);
    assert_eq!(crate::server::options::get_option_value(&a, "prefix"), "None");
    assert_eq!(crate::format::lookup_option_pub("prefix", &a).as_deref(), Some("None"));
    crate::server::options::apply_set_option(&mut a, "prefix", "none", false).unwrap();
    assert_eq!(a.prefix_key, None);
}

#[test]
fn bad_prefix_is_refused_and_keeps_the_value() {
    let mut a = app();
    crate::server::options::apply_set_option(&mut a, "prefix", "C-a", false).unwrap();
    let err = crate::server::options::apply_set_option(&mut a, "prefix", "NotAKey", false).unwrap_err();
    assert_eq!(err, "bad key: NotAKey");
    assert_eq!(a.prefix_key, ctrl('a'));

    crate::server::options::apply_set_option(&mut a, "prefix2", "C-x", false).unwrap();
    let err = crate::server::options::apply_set_option(&mut a, "prefix2", "Bogus-Key", false).unwrap_err();
    assert_eq!(err, "bad key: Bogus-Key");
    assert_eq!(a.prefix2_key, ctrl('x'));
    crate::server::options::apply_set_option(&mut a, "prefix2", "None", false).unwrap();
    assert_eq!(a.prefix2_key, None);
    assert_eq!(crate::server::options::get_option_value(&a, "prefix2"), "None");
}

#[test]
fn catalog_validation_rejects_bad_keys_only() {
    use crate::server::option_catalog::validate_option_value;
    assert!(validate_option_value("prefix", "None").is_ok());
    assert!(validate_option_value("prefix", "C-Space").is_ok());
    assert!(validate_option_value("prefix", "F12").is_ok());
    assert_eq!(validate_option_value("prefix", "Nope"), Err("bad key: Nope".to_string()));
    assert_eq!(validate_option_value("prefix2", "Nope"), Err("bad key: Nope".to_string()));
    // prefix2 has always taken an empty value as "no key".
    assert!(validate_option_value("prefix2", "").is_ok());
}

#[test]
fn unset_after_none_restores_c_b() {
    let mut a = app();
    crate::server::options::apply_set_option(&mut a, "prefix", "None", false).unwrap();
    crate::server::options::reset_option_to_default(&mut a, "prefix");
    assert_eq!(a.prefix_key, ctrl('b'));
}

#[test]
fn config_file_prefix_none_and_bad_key() {
    let mut a = app();
    parse_config_content(&mut a, "set -g prefix C-a\nset -g prefix None\n");
    assert_eq!(a.prefix_key, None);

    let mut a = app();
    parse_config_content(&mut a, "set -g prefix C-a\nset -g prefix NoSuchKey\nset -g @after 1\n");
    assert_eq!(a.prefix_key, ctrl('a'), "a bad key must not change the prefix");
    assert!(
        a.config_warnings.iter().any(|w| w.contains("bad key: NoSuchKey")),
        "the bad key must be reported: {:?}",
        a.config_warnings
    );
    assert_eq!(a.user_options.get("@after").map(String::as_str), Some("1"), "later lines still run");
}

#[test]
fn no_prefix_means_no_send_prefix_self_binding() {
    let mut a = app();
    a.key_tables.clear();
    a.prefix_key = None;
    ensure_prefix_self_binding(&mut a);
    assert!(a.key_tables.get("prefix").map_or(true, |t| t.is_empty()));
}

fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

/// Bind C-b and the raw 0x02 byte in the root table, so a key that is NOT
/// taken as the prefix is consumed by a binding instead of being written to a
/// pane this bare AppState does not have.
fn bind_root_catchers(a: &mut AppState) {
    let root = a.key_tables.entry("root".to_string()).or_default();
    for key in [(KeyCode::Char('b'), CTRL), (KeyCode::Char('\u{0002}'), KeyModifiers::NONE)] {
        root.push(crate::types::Bind {
            key,
            action: crate::types::Action::Command("set -g @hit 1".to_string()),
            repeat: false,
            note: None,
        });
    }
}

#[test]
fn server_side_key_path_does_not_arm_a_none_prefix() {
    let mut a = app();
    a.prefix_key = None;
    bind_root_catchers(&mut a);
    // Neither the old C-b nor its raw 0x02 alias may enter the prefix table.
    let _ = crate::input::handle_key(&mut a, press(KeyCode::Char('b'), CTRL));
    assert!(!matches!(a.mode, crate::types::Mode::Prefix { .. }), "C-b armed a None prefix");
    let _ = crate::input::handle_key(&mut a, press(KeyCode::Char('\u{0002}'), KeyModifiers::NONE));
    assert!(!matches!(a.mode, crate::types::Mode::Prefix { .. }), "0x02 armed a None prefix");
}

#[test]
fn raw_ctrl_b_alias_follows_the_prefix() {
    // With prefix C-a the raw 0x02 byte is just C-b, not the prefix.
    let mut a = app();
    a.prefix_key = ctrl('a');
    bind_root_catchers(&mut a);
    let _ = crate::input::handle_key(&mut a, press(KeyCode::Char('\u{0002}'), KeyModifiers::NONE));
    assert!(!matches!(a.mode, crate::types::Mode::Prefix { .. }));
    // With the default prefix it still arms, as before.
    let mut a = app();
    let _ = crate::input::handle_key(&mut a, press(KeyCode::Char('\u{0002}'), KeyModifiers::NONE));
    assert!(matches!(a.mode, crate::types::Mode::Prefix { .. }));
}
