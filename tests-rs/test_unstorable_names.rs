// Session and -L names with a character Windows cannot keep in a file name.
//
// `new-session -s a:b` wrote its registry as alternate data streams of a file
// named `a`, so nothing found the server and it was orphaned. Such names are
// now refused before anything is spawned or written.

use super::*;

const BAD: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

#[test]
fn every_reserved_character_is_unstorable() {
    for c in BAD {
        let name = format!("a{}b", c);
        assert_eq!(unstorable_name_char(&name), Some(c), "{name}");
    }
}

#[test]
fn every_control_character_is_unstorable() {
    for b in 0u8..0x20 {
        let name = format!("a{}b", b as char);
        assert_eq!(unstorable_name_char(&name), Some(b as char), "0x{b:02x}");
    }
}

#[test]
fn names_that_work_today_are_kept() {
    for name in [
        "work", "0", "my-session", "my_session", "my.session", "with space",
        "trailing.", "trailing ", "CON", "nul", "COM1", "café", "日本語", "a__b",
        "#{x}", "a%b", "a;b", "a=b", "a,b", "a'b", "~", "\u{7f}",
    ] {
        assert_eq!(unstorable_name_char(name), None, "{name:?}");
    }
}

#[test]
fn a_session_name_error_uses_the_tmux_wording() {
    assert_eq!(
        reserved_session_name_error(None, "a:b").as_deref(),
        Some("invalid session name: a:b")
    );
    assert_eq!(
        reserved_session_name_error(Some("ns"), "a|b").as_deref(),
        Some("invalid session name: a|b")
    );
}

#[test]
fn a_control_character_is_shown_escaped() {
    assert_eq!(
        reserved_session_name_error(Some("ns"), "a\x1bb").as_deref(),
        Some("invalid session name: a\\033b")
    );
}

#[test]
fn a_good_name_is_still_accepted_in_both_namespaces() {
    assert_eq!(reserved_session_name_error(None, "work"), None);
    assert_eq!(reserved_session_name_error(Some("ns"), "my.session"), None);
    assert_eq!(reserved_session_name_error(Some("ns"), "a__b"), None);
}

#[test]
fn the_844_rule_still_applies_after_the_new_one() {
    let msg = reserved_session_name_error(None, "a__b").unwrap();
    assert!(msg.starts_with("invalid session: a__b"), "{msg}");
}

#[test]
fn a_socket_name_is_checked_the_same_way() {
    assert_eq!(invalid_socket_name_error("x:y").as_deref(), Some("invalid socket name: x:y"));
    assert_eq!(invalid_socket_name_error("a/b").as_deref(), Some("invalid socket name: a/b"));
    assert_eq!(invalid_socket_name_error("work"), None);
    assert_eq!(invalid_socket_name_error("my.ns-1_x"), None);
    assert_eq!(invalid_socket_name_error(""), None);
}
