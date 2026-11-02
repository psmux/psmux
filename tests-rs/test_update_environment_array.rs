// update-environment as tmux's array option (options-table.c:1210-1220 at tag
// 3.8: a SESSION option, OPTIONS_TABLE_IS_ARRAY, no separator so a value is
// split on space and `,`, and 12 default patterns, tmux 3.8 less MSYSTEM). Each pattern keeps its own
// index, `-a` takes the lowest free index, `-u update-environment[N]` removes
// one, show prints a line per element, and environ_update walks the elements
// in key order (environ.c:186-213).
//
// Without -g the session's own array starts empty (the session table has no
// entry until the first write), so an element or `-a` alone is the whole
// session array; `-u` drops it and the global array applies again; `-g`
// writes reach the global array the session shadows. Every expected value
// below is what tmux 3.8 (`-f /dev/null`, WSL) printed for the same commands.

use super::*;
use crate::types::AppState;

const OPTION: &str = "update-environment";

const DEFAULT: [&str; 12] = [
    "DISPLAY", "KRB5CCNAME", "SSH_ASKPASS", "SSH_AUTH_SOCK",
    "SSH_AGENT_PID", "SSH_CONNECTION", "WAYLAND_DISPLAY", "WINDOWID",
    "XAUTHORITY", "XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "XDG_SESSION_TYPE",
];

fn app() -> AppState {
    AppState::new("test_ue_array".to_string())
}

fn items(a: &AppState) -> Vec<(String, String)> {
    a.update_environment.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

fn item(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

fn show(a: &AppState, global: bool, mark_inherited: bool) -> String {
    array_option_show_lines_scoped(a, OPTION, false, global, mark_inherited)
}

fn set_session(a: &mut AppState, name: &str, value: &str, append: bool) {
    assert_eq!(apply_set_session_option(a, name, value, false, append, false, false), "");
}

#[test]
fn the_default_is_tmux_38s_thirteen_patterns_in_order() {
    let a = app();
    assert_eq!(a.update_environment.values(), DEFAULT.to_vec());
    let lines: Vec<String> = DEFAULT
        .iter()
        .enumerate()
        .map(|(index, name)| format!("update-environment[{}] {}\n", index, name))
        .collect();
    assert_eq!(array_option_show_lines(&a, OPTION, false), lines.concat());
    assert_eq!(get_option_value(&a, OPTION), DEFAULT.join(" "));
    assert_eq!(
        crate::server::option_catalog::default_for(OPTION),
        Some(DEFAULT.join(" ").as_str()),
    );
}

#[test]
fn a_value_splits_on_space_and_comma() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, "set -g update-environment 'A B,C'\n");
    assert_eq!(items(&a), vec![item("0", "A"), item("1", "B"), item("2", "C")]);
}

#[test]
fn append_index_and_unset_follow_tmux() {
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -ga update-environment FOO\n\
         set -g update-environment[20] BAR\n\
         set -gu update-environment[0]\n",
    );
    let values = a.update_environment.values();
    assert_eq!(values.len(), 13);
    assert!(!values.contains(&"DISPLAY".to_string()));
    assert_eq!(a.update_environment.get_index(12), Some("FOO"));
    assert_eq!(a.update_environment.get_index(20), Some("BAR"));
    assert_eq!(array_option_show_lines(&a, "update-environment[20]", false), "update-environment[20] BAR\n");
    assert_eq!(array_option_value(&a, "update-environment[20]"), "BAR");

    crate::config::parse_config_content(&mut a, "set -gu update-environment\n");
    assert_eq!(a.update_environment.values(), DEFAULT.to_vec());
}

#[test]
fn the_runtime_setter_reaches_the_array_too() {
    let mut a = app();
    set_array_option(&mut a, OPTION, "ONE TWO", false).unwrap();
    set_array_option(&mut a, OPTION, "THREE", true).unwrap();
    apply_set_option(&mut a, "update-environment[7]", "SEVEN", false).unwrap();
    assert_eq!(items(&a), vec![item("0", "ONE"), item("1", "TWO"), item("2", "THREE"), item("7", "SEVEN")]);
}

#[test]
fn environ_update_uses_every_element_in_key_order() {
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -g update-environment 'UE_A'\n\
         set -g update-environment[20] 'UE_B*'\n",
    );
    let patterns = a.update_environment.values();
    let mut env = std::collections::HashMap::new();
    let mut scopes = crate::environ::EnvScopes::default();
    let src = vec![
        ("UE_A".to_string(), "a".to_string()),
        ("UE_B1".to_string(), "b1".to_string()),
        ("UE_C".to_string(), "c".to_string()),
    ];
    let written = crate::environ::update_environment(&mut env, &mut scopes, &patterns, &src, false);
    assert_eq!(written, vec!["UE_A".to_string(), "UE_B1".to_string()]);
    assert_eq!(env.get("UE_A").map(String::as_str), Some("a"));
    assert_eq!(env.get("UE_B1").map(String::as_str), Some("b1"));
    assert!(!env.contains_key("UE_C"));
}

#[test]
fn a_session_element_alone_is_the_whole_session_array() {
    let mut a = app();
    set_session(&mut a, "update-environment[20]", "X", false);
    assert_eq!(items(&a), vec![item("20", "X")]);
    assert_eq!(show(&a, false, false), "update-environment[20] X\n");
    // The global array is untouched and still has its 12 patterns.
    let global = show(&a, true, false);
    assert_eq!(global.lines().count(), 12);
    assert!(global.starts_with("update-environment[0] DISPLAY\n"));
}

#[test]
fn session_append_with_nothing_before_starts_empty() {
    let mut a = app();
    set_session(&mut a, OPTION, "FOO", true);
    assert_eq!(items(&a), vec![item("0", "FOO")]);
}

#[test]
fn session_set_then_append_then_unset_one() {
    let mut a = app();
    set_session(&mut a, OPTION, "A B", false);
    set_session(&mut a, OPTION, "C", true);
    assert_eq!(items(&a), vec![item("0", "A"), item("1", "B"), item("2", "C")]);
    assert_eq!(apply_set_session_option(&mut a, "update-environment[0]", "", true, false, false, false), "");
    assert_eq!(items(&a), vec![item("1", "B"), item("2", "C")]);
}

#[test]
fn session_unset_puts_the_global_array_back() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, "set -g update-environment[30] Z\n");
    set_session(&mut a, OPTION, "S", false);
    assert_eq!(items(&a), vec![item("0", "S")]);
    assert_eq!(apply_set_session_option(&mut a, OPTION, "", true, false, false, false), "");
    assert!(!session_overrides(&a, OPTION));
    assert_eq!(a.update_environment.values().len(), 13);
    assert_eq!(a.update_environment.get_index(30), Some("Z"));
}

#[test]
fn a_global_write_while_the_session_overrides_goes_to_the_global_array() {
    let mut a = app();
    set_session(&mut a, OPTION, "A B", false);
    write_shadowed_global(&mut a, OPTION, GlobalWrite::Set("G")).unwrap();
    assert_eq!(show(&a, true, false), "update-environment[0] G\n");
    assert_eq!(items(&a), vec![item("0", "A"), item("1", "B")]);

    write_shadowed_global(&mut a, OPTION, GlobalWrite::Append("H")).unwrap();
    write_shadowed_global(&mut a, "update-environment[5]", GlobalWrite::Set("I")).unwrap();
    assert_eq!(show(&a, true, false), "update-environment[0] G\nupdate-environment[1] H\nupdate-environment[5] I\n");
    assert_eq!(global_session_option_value(&a, OPTION), "G H I");

    // The session lets go: what the global array became applies.
    unset_session_option(&mut a, OPTION);
    assert_eq!(a.update_environment.values(), vec!["G", "H", "I"]);
}

#[test]
fn a_global_unset_while_the_session_overrides_restores_the_default() {
    let mut a = app();
    crate::config::parse_config_content(&mut a, "set -g update-environment ONLY\n");
    set_session(&mut a, OPTION, "S", false);
    write_shadowed_global(&mut a, OPTION, GlobalWrite::Unset).unwrap();
    assert_eq!(show(&a, true, false).lines().count(), 12);
    unset_session_option(&mut a, OPTION);
    assert_eq!(a.update_environment.values(), DEFAULT.to_vec());
}

#[test]
fn show_a_marks_what_the_session_inherits() {
    let mut a = app();
    let marked = show(&a, false, true);
    assert!(marked.starts_with("update-environment[0]* DISPLAY\nupdate-environment[1]* KRB5CCNAME\n"), "{marked}");
    set_session(&mut a, OPTION, "S", false);
    assert_eq!(show(&a, false, true), "update-environment[0] S\n");
}

#[test]
fn a_session_unset_of_one_element_does_nothing_when_the_session_has_none() {
    let mut a = app();
    unset_session_option(&mut a, "update-environment[0]");
    assert!(!session_overrides(&a, OPTION));
    assert_eq!(a.update_environment.values(), DEFAULT.to_vec());
}

#[test]
fn the_session_listing_shows_the_global_lines_for_g() {
    let mut a = app();
    set_session(&mut a, OPTION, "A B", false);
    let listing = "status on\nupdate-environment[0] A\nupdate-environment[1] B\nmouse on\n";
    assert_eq!(
        scope_session_listing(&a, listing, true, false),
        format!("status on\n{}mouse on\n", show(&a, true, false)),
    );
    // Without -g the session's own lines pass through.
    assert_eq!(scope_session_listing(&a, listing, false, false), listing);
}

#[test]
fn element_names_count_as_the_session_option() {
    assert!(is_session_option(OPTION));
    assert!(is_session_option("update-environment[3]"));
    assert!(!is_session_option("terminal-overrides[0]"));
}
