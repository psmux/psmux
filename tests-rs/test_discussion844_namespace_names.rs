// Discussion #844: building and reading registry bases for names with `__`.
//
// A session named `nsq__x` under -L nsq is stored at `nsq__nsq__x`, but a -t of
// `nsq__x` was read as that base already and looked up `nsq__x`. And a default
// namespace name `nsq__bb` shares its registry files with -L nsq's `bb`.

use super::*;

fn none(_: &str) -> bool { false }

#[test]
fn a_name_is_always_stored_with_the_prefix() {
    assert_eq!(session_name_base(Some("nsq"), "x"), "nsq__x");
    assert_eq!(session_name_base(Some("nsq"), "nsq__x"), "nsq__nsq__x");
    assert_eq!(session_name_base(Some("nsq"), "a__b"), "nsq__a__b");
    assert_eq!(session_name_base(None, "x"), "x");
}

#[test]
fn a_name_that_starts_with_the_namespace_finds_its_own_session() {
    let reg = |b: &str| b == "nsq__nsq__x";
    assert_eq!(namespaced_session_base_with(Some("nsq"), "nsq__x", reg), "nsq__nsq__x");
}

#[test]
fn a_base_with_no_session_of_that_name_is_still_a_base() {
    // `$N` resolution and PSMUX_TARGET_SESSION hand over the base itself.
    assert_eq!(namespaced_session_base_with(Some("nsq"), "nsq__x", none), "nsq__x");
    let reg = |b: &str| b == "nsq__x";
    assert_eq!(namespaced_session_base_with(Some("nsq"), "nsq__x", reg), "nsq__x");
}

#[test]
fn a_short_name_is_prefixed_whatever_the_registry_holds() {
    assert_eq!(namespaced_session_base_with(Some("nsq"), "x", none), "nsq__x");
    assert_eq!(namespaced_session_base_with(Some("nsq"), "nsr__v", none), "nsq__nsr__v");
}

#[test]
fn the_default_namespace_never_prefixes() {
    let all = |_: &str| true;
    assert_eq!(namespaced_session_base_with(None, "nsq__bb", all), "nsq__bb");
    assert_eq!(namespaced_session_base_with(Some(""), "x", all), "x");
}

#[test]
fn the_stored_base_and_the_lookup_agree_once_the_session_exists() {
    for name in ["x", "nsq__x", "nsq__nsq__y", "a__b", "_z"] {
        let stored = session_name_base(Some("nsq"), name);
        let reg = |b: &str| b == stored;
        assert_eq!(namespaced_session_base_with(Some("nsq"), name, reg), stored, "name {}", name);
    }
}

#[test]
fn the_stored_base_reads_back_as_its_namespace() {
    for name in ["x", "nsq__x", "a__b", "_z"] {
        let stored = session_name_base(Some("nsq"), name);
        assert_eq!(session_namespace(&stored), Some("nsq"), "name {}", name);
        assert!(session_visible_from(&stored, Some("nsq")), "name {}", name);
        assert!(!session_visible_from(&stored, None), "name {}", name);
    }
}

#[test]
fn the_default_namespace_refuses_a_name_with_a_double_underscore() {
    for name in ["nsq__bb", "__x", "x__", "a__b__c"] {
        let msg = reserved_session_name_error(None, name).expect(name);
        assert!(msg.starts_with(&format!("invalid session: {}", name)), "{}", msg);
        assert!(!msg.contains('\u{2014}'));
    }
    assert!(reserved_session_name_error(Some(""), "a__b").is_some());
}

#[test]
fn the_default_namespace_accepts_every_other_name() {
    for name in ["bb", "a_b", "a_", "_a", "0", "my-session", "x.y"] {
        assert_eq!(reserved_session_name_error(None, name), None, "{}", name);
    }
}

#[test]
fn an_l_namespace_accepts_a_double_underscore() {
    for name in ["nsq__x", "a__b", "__x"] {
        assert_eq!(reserved_session_name_error(Some("nsq"), name), None, "{}", name);
    }
}

#[test]
fn every_name_the_default_namespace_accepts_reads_back_as_default() {
    for name in ["bb", "a_b", "a_", "_a", "0"] {
        assert!(reserved_session_name_error(None, name).is_none());
        let stored = session_name_base(None, name);
        assert_eq!(session_namespace(&stored), None, "{}", name);
        assert!(session_visible_from(&stored, None), "{}", name);
    }
}
