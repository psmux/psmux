// Issue #945: codepoint-widths as tmux's array option (options-table.c:337-344
// at tag 3.8: OPTIONS_TABLE_IS_ARRAY, separator ",", default ""). Each element
// keeps its own index, so `codepoint-widths[N]` and `-u codepoint-widths[N]` work
// on one element, `-a` takes the lowest free index, show prints a line per
// element, and the width table is built from the elements in index order
// (utf8.c:412-436, utf8_update_width_cache), so a later index wins.
//
// The expected values are what tmux 3.8 (`-f /dev/null`, WSL) printed for
// the same commands, except for the spaces: psmux keeps each element without
// the spaces around it, where tmux keeps them.
//
// Setting the option rebuilds the process global width table, which other
// tests in this binary read, so every value here names private use
// codepoints (U+E010 and up) that nothing else draws, and the guard empties
// the table when a test ends. The widths themselves are checked end to end
// by the integration suite.

use super::*;
use crate::types::AppState;

const OPTION: &str = "codepoint-widths";

fn app() -> AppState {
    AppState::new("test_cpw_array".to_string())
}

struct WidthTable;

impl Drop for WidthTable {
    fn drop(&mut self) {
        vt100::clear_codepoint_widths();
    }
}

fn items(a: &AppState) -> Vec<(u32, String)> {
    a.codepoint_widths.iter().map(|(index, entry)| (index, entry.to_string())).collect()
}

fn item(index: u32, entry: &str) -> (u32, String) {
    (index, entry.to_string())
}

#[test]
fn a_new_server_has_no_element() {
    let a = app();
    assert!(a.codepoint_widths.is_empty());
    assert_eq!(get_option_value(&a, OPTION), "");
    assert_eq!(array_option_show_lines(&a, OPTION, false), "codepoint-widths\n");
    assert_eq!(array_option_show_lines(&a, OPTION, true), "");
}

#[test]
fn config_set_append_index_and_unset_follow_tmux() {
    let _table = WidthTable;
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -s codepoint-widths 'U+E010=2,U+E011=1'\n\
         set -s codepoint-widths[5] U+E012=2\n\
         set -sa codepoint-widths U+E013=2\n",
    );
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert_eq!(
        items(&a),
        vec![item(0, "U+E010=2"), item(1, "U+E011=1"), item(2, "U+E013=2"), item(5, "U+E012=2")],
        "-a takes the lowest free index, not the end",
    );
    crate::config::parse_config_content(&mut a, "set -sa codepoint-widths[5] 0\nset -su codepoint-widths[0]\n");
    assert_eq!(
        items(&a),
        vec![item(1, "U+E011=1"), item(2, "U+E013=2"), item(5, "U+E012=20")],
        "-a on an element appends text, -u on one keeps the other indexes",
    );
    crate::config::parse_config_content(&mut a, "set -su codepoint-widths\n");
    assert!(a.codepoint_widths.is_empty(), "-u puts back the empty default");
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    for name in ["codepoint-widths", "codepoint-widths[5]", "codepoint-widths[0]"] {
        assert!(!a.user_options.contains_key(name), "{name} kept as text");
    }
}

#[test]
fn runtime_set_replaces_the_array_and_joins_with_a_space() {
    let _table = WidthTable;
    let mut a = app();
    apply_set_option(&mut a, "codepoint-widths[9]", "U+E014=2", false).unwrap();
    apply_set_option(&mut a, OPTION, "U+E010=2,U+E011=1", false).unwrap();
    assert_eq!(items(&a), vec![item(0, "U+E010=2"), item(1, "U+E011=1")]);
    // `display -p '#{codepoint-widths}'` printed `U+2502=2 U+4E2D=1`
    // (options_to_string, options.c); psmux used to join with a comma.
    assert_eq!(get_option_value(&a, OPTION), "U+E010=2 U+E011=1");
    assert_eq!(get_option_value(&a, "codepoint-widths[1]"), "U+E011=1");
    assert_eq!(get_option_value(&a, "codepoint-widths[7]"), "");
    set_array_option(&mut a, OPTION, "U+E012=2", true).unwrap();
    reset_option_to_default(&mut a, "codepoint-widths[0]");
    set_array_option(&mut a, OPTION, "U+E013=2", true).unwrap();
    assert_eq!(items(&a), vec![item(0, "U+E013=2"), item(1, "U+E011=1"), item(2, "U+E012=2")]);
    reset_option_to_default(&mut a, OPTION);
    assert!(a.codepoint_widths.is_empty());
}

#[test]
fn a_bad_index_is_no_option() {
    let mut a = app();
    let error = apply_set_option(&mut a, "codepoint-widths[x]", "U+E010=2", false).unwrap_err();
    assert!(error.contains("invalid option"), "{error}");
    assert_eq!(
        crate::server::option_catalog::validate_option_value("codepoint-widths[x]", "U+E010=2").unwrap_err(),
        "invalid option: codepoint-widths[x]"
    );
    assert!(!crate::server::option_catalog::is_valid_option_name("codepoint-widths[x]"));
    assert!(crate::server::option_catalog::is_valid_option_name("codepoint-widths[3]"));
    assert!(a.codepoint_widths.is_empty());
}

#[test]
fn each_element_is_kept_without_its_spaces() {
    // tmux 3.8 shows `codepoint-widths[1] " U+4E2D=1"` for this value and the
    // element then sets no width; psmux kept both working before this was an
    // array and still does.
    let _table = WidthTable;
    let mut a = app();
    apply_set_option(&mut a, OPTION, "U+E010=2, U+E011=1 , ,", false).unwrap();
    assert_eq!(items(&a), vec![item(0, "U+E010=2"), item(1, "U+E011=1")]);
    apply_set_option(&mut a, "codepoint-widths[4]", "  U+E012=2 ", false).unwrap();
    assert_eq!(a.codepoint_widths.get(4), Some("U+E012=2"));
    crate::config::parse_config_content(&mut a, "set -s codepoint-widths ' U+E013=2'\n");
    assert_eq!(items(&a), vec![item(0, "U+E013=2")]);
}

#[test]
fn the_width_table_is_built_in_index_order() {
    // `set -s codepoint-widths[5] U+2502=1`, then `[0] U+2502=2`: tmux 3.8
    // draws U+2502 one column wide, because [5] is applied after [0].
    let _table = WidthTable;
    let mut a = app();
    apply_set_option(&mut a, "codepoint-widths[5]", "U+E010=1", false).unwrap();
    apply_set_option(&mut a, "codepoint-widths[0]", "U+E010=2", false).unwrap();
    assert_eq!(codepoint_width_entries(&a), vec!["U+E010=2", "U+E010=1"]);
    // The client builds its own table from the same elements in the same
    // order, for the status line and the other text it measures itself.
    let options = crate::server::helpers::expand_status_formats(&a, "").client_render_options;
    assert_eq!(options.codepoint_widths, Some(vec!["U+E010=2".to_string(), "U+E010=1".to_string()]));
}

#[test]
fn the_client_gets_nothing_while_the_array_is_empty() {
    let _table = WidthTable;
    let mut a = app();
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(!json.contains("\"cpw\""), "{json}");
    apply_set_option(&mut a, OPTION, "U+E010=2", false).unwrap();
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(json.contains("\"cpw\":[\"U+E010=2\"]"), "{json}");
    reset_option_to_default(&mut a, "codepoint-widths[0]");
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(!json.contains("\"cpw\""), "{json}");
}

#[test]
fn show_prints_a_line_per_element_like_tmux() {
    let _table = WidthTable;
    let mut a = app();
    apply_set_option(&mut a, OPTION, "U+E010=2,U+E011=1", false).unwrap();
    apply_set_option(&mut a, "codepoint-widths[5]", "U+E012=2", false).unwrap();
    assert_eq!(
        array_option_show_lines(&a, OPTION, false),
        "codepoint-widths[0] U+E010=2\ncodepoint-widths[1] U+E011=1\ncodepoint-widths[5] U+E012=2\n"
    );
    assert_eq!(array_option_show_lines(&a, OPTION, true), "U+E010=2\nU+E011=1\nU+E012=2\n");
    assert_eq!(array_option_show_lines(&a, "codepoint-widths[5]", false), "codepoint-widths[5] U+E012=2\n");
    assert_eq!(array_option_show_lines(&a, "codepoint-widths[9]", false), "codepoint-widths[9] ''\n");
    assert_eq!(array_option_show_lines(&a, "codepoint-widths[9]", true), "\n");
}
