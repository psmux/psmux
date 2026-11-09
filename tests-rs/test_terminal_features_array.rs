// terminal-features as tmux's array option (options-table.c:553-563 at tag
// 3.8: a server option, OPTIONS_TABLE_IS_ARRAY, separator ",", three default
// entries), and psmux adds a fourth, `*:progressbar`, the one element that
// differs. psmux reads only that feature from it (test_progress_bar_features.rs),
// so these check the data: each element keeps its own index,
// `terminal-features[N]` and `-u terminal-features[N]` work on one element,
// `-a` takes the lowest free index, `-u` of the whole array puts the default
// back, and show prints a line per element.
//
// The expected values are what tmux 3.8 (`-f /dev/null`, WSL) printed for
// the same commands, with the fourth element added.

use super::*;
use crate::types::AppState;

const OPTION: &str = "terminal-features";

const DEFAULT: [&str; 4] = ["xterm*:clipboard:ccolour:cstyle:focus:title", "screen*:title", "rxvt*:ignorefkeys", "*:progressbar"];

fn app() -> AppState {
    AppState::new("test_tf_array".to_string())
}

fn items(a: &AppState) -> Vec<(u32, String)> {
    a.terminal_features.numbered().map(|(index, entry)| (index, entry.to_string())).collect()
}

fn item(index: u32, entry: &str) -> (u32, String) {
    (index, entry.to_string())
}

fn default_items() -> Vec<(u32, String)> {
    DEFAULT.iter().enumerate().map(|(i, entry)| item(i as u32, entry)).collect()
}

#[test]
fn a_new_server_has_tmux_s_three_entries_and_progressbar() {
    let a = app();
    assert_eq!(items(&a), default_items());
    assert_eq!(
        array_option_show_lines(&a, OPTION, false),
        "terminal-features[0] xterm*:clipboard:ccolour:cstyle:focus:title\n\
         terminal-features[1] screen*:title\n\
         terminal-features[2] rxvt*:ignorefkeys\n\
         terminal-features[3] *:progressbar\n"
    );
    assert_eq!(
        array_option_show_lines(&a, OPTION, true),
        "xterm*:clipboard:ccolour:cstyle:focus:title\nscreen*:title\nrxvt*:ignorefkeys\n*:progressbar\n"
    );
    // `display -p '#{terminal-features}'` joins them with a space
    // (options_to_string, options.c).
    assert_eq!(get_option_value(&a, OPTION), DEFAULT.join(" "));
    // The catalog default is that same joined text.
    let def = crate::server::option_catalog::OPTION_CATALOG.iter().find(|d| d.name == OPTION).expect("in the catalog");
    assert_eq!(def.default, DEFAULT.join(" "));
}

#[test]
fn config_append_index_and_unset_follow_tmux() {
    let mut a = app();
    // The #973 suite's line: the leading `,` gives an empty piece, which is
    // skipped, so -a adds one element.
    crate::config::parse_config_content(
        &mut a,
        "set -as terminal-features ',xterm*:overline'\n\
         set -s terminal-features[7] foo:bar\n\
         set -su terminal-features[0]\n",
    );
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert_eq!(
        items(&a),
        vec![item(1, "screen*:title"), item(2, "rxvt*:ignorefkeys"), item(3, "*:progressbar"), item(4, "xterm*:overline"), item(7, "foo:bar")],
    );
    crate::config::parse_config_content(&mut a, "set -su terminal-features\n");
    assert_eq!(items(&a), default_items(), "-u puts the default back");
    for name in ["terminal-features", "terminal-features[7]", "terminal-features[0]"] {
        assert!(!a.user_options.contains_key(name), "{name} kept as text");
    }
}

#[test]
fn a_value_splits_on_comma_only() {
    let mut a = app();
    apply_set_option(&mut a, OPTION, "a,b c,,d", false).unwrap();
    assert_eq!(items(&a), vec![item(0, "a"), item(1, "b c"), item(2, "d")]);
    assert_eq!(
        array_option_show_lines(&a, OPTION, false),
        "terminal-features[0] a\nterminal-features[1] \"b c\"\nterminal-features[2] d\n"
    );
    set_array_option(&mut a, OPTION, "e", true).unwrap();
    assert_eq!(a.terminal_features.get_index(3), Some("e"));
    apply_set_option(&mut a, OPTION, "", false).unwrap();
    assert!(a.terminal_features.is_empty());
    assert_eq!(array_option_show_lines(&a, OPTION, false), "terminal-features\n");
    reset_option_to_default(&mut a, OPTION);
    assert_eq!(items(&a), default_items());
}

#[test]
fn one_element_and_a_named_key() {
    let mut a = app();
    apply_set_option(&mut a, "terminal-features[7]", "foo:bar", false).unwrap();
    apply_set_option(&mut a, "terminal-features[x]", "named", false).unwrap();
    assert_eq!(array_option_show_lines(&a, "terminal-features[7]", false), "terminal-features[7] foo:bar\n");
    assert_eq!(array_option_show_lines(&a, "terminal-features[6]", false), "terminal-features[6] ''\n");
    assert_eq!(get_option_value(&a, "terminal-features[7]"), "foo:bar");
    // Numbered elements first, then the named ones (options_array_cmp).
    assert!(array_option_show_lines(&a, OPTION, false).ends_with("terminal-features[7] foo:bar\nterminal-features[x] named\n"));
}

#[test]
fn a_key_that_is_no_key_is_no_option() {
    let mut a = app();
    let error = apply_set_option(&mut a, "terminal-features[]", "z", false).unwrap_err();
    assert!(error.contains("invalid option"), "{error}");
    assert!(!crate::server::option_catalog::is_valid_option_name("terminal-features[]"));
    assert!(crate::server::option_catalog::is_valid_option_name("terminal-features[3]"));
    assert_eq!(items(&a), default_items());
}

#[test]
fn it_is_a_server_option() {
    assert!(crate::server::option_catalog::server_option_names().contains(&OPTION));
}
