//! The array option handling against tmux 3.7c's options.c and
//! cmd-set-option.c. The expected results were measured with tmux 3.7c on
//! command-alias and status-format, `-f /dev/null`, after
//! `set -s command-alias ''` to start from an empty array.

use super::*;

fn items(array: &OptionArray) -> Vec<(u32, &str)> {
    array.iter().collect()
}

/// A spec that splits on `separator` and keeps the spaces, as tmux does.
fn sep(separator: &'static str) -> ArraySpec {
    ArraySpec { name: "test", separator, default: "", trim: false }
}

#[test]
fn parse_name_splits_the_index_off() {
    assert_eq!(parse_name("command-alias"), Ok(("command-alias", None)));
    assert_eq!(parse_name("command-alias[0]"), Ok(("command-alias", Some(0))));
    assert_eq!(parse_name("status-format[12]"), Ok(("status-format", Some(12))));
    assert_eq!(parse_name("command-alias[01]"), Ok(("command-alias", Some(1))));
}

#[test]
fn parse_name_refuses_what_options_parse_refuses() {
    for name in ["", "a[", "a[]", "a[x]", "a[-1]", "a[1]x", "a[1][2]", "a[99999999999]"] {
        assert!(parse_name(name).is_err(), "{:?}", name);
    }
    assert_eq!(parse_name("a[x]"), Err("invalid option: a[x]".to_string()));
}

#[test]
fn spec_finds_an_array_by_its_base_name() {
    assert_eq!(spec("command-alias").map(|s| s.separator), Some(","));
    assert_eq!(spec("command-alias[3]").map(|s| s.name), Some("command-alias"));
    assert!(spec("status-left").is_none());
    assert!(spec("command-alias[x]").is_none());
}

#[test]
fn set_with_an_index_and_append_extends_that_element() {
    // `set -s command-alias[4] a=list-keys`, then
    // `set -as command-alias[4] ' -T root'`: "a=list-keys -T root".
    let mut a = OptionArray::default();
    a.apply_set(Some(4), "a=list-keys", false, &sep(","));
    a.apply_set(Some(4), " -T root", true, &sep(","));
    assert_eq!(items(&a), vec![(4, "a=list-keys -T root")]);
}

#[test]
fn append_without_an_index_takes_the_lowest_free_index() {
    // `[4]` set, then `set -as command-alias b=list-buffers` lands at [0].
    let mut a = OptionArray::default();
    a.apply_set(Some(4), "a=list-keys", false, &sep(","));
    a.apply_set(None, "b=list-buffers", true, &sep(","));
    assert_eq!(items(&a), vec![(0, "b=list-buffers"), (4, "a=list-keys")]);
    a.apply_set(None, "c=kill-pane", true, &sep(","));
    assert_eq!(items(&a), vec![(0, "b=list-buffers"), (1, "c=kill-pane"), (4, "a=list-keys")]);
}

#[test]
fn a_value_without_an_index_replaces_the_array() {
    // `set -s command-alias 'c=new-window,d=split-window -h'` after [0] and
    // [4] leaves [0] and [1] only.
    let mut a = OptionArray::default();
    a.apply_set(Some(0), "b=list-buffers", false, &sep(","));
    a.apply_set(Some(4), "a=list-keys", false, &sep(","));
    a.apply_set(None, "c=new-window,d=split-window -h", false, &sep(","));
    assert_eq!(items(&a), vec![(0, "c=new-window"), (1, "d=split-window -h")]);
}

#[test]
fn assign_splits_on_any_separator_character_and_skips_empty_pieces() {
    // status-format names no separator: `set -g status-format "a b,c"` makes
    // three elements.
    let mut a = OptionArray::default();
    a.apply_set(None, "a b,c", false, &sep(" ,"));
    assert_eq!(items(&a), vec![(0, "a"), (1, "b"), (2, "c")]);
    let mut a = OptionArray::default();
    a.apply_set(None, ",x,,y,", false, &sep(","));
    assert_eq!(items(&a), vec![(0, "x"), (1, "y")]);
}

#[test]
fn an_empty_separator_keeps_the_value_whole() {
    // A hook: `set-hook -g h[3] ...` then `set-hook -ga h 'display two'`
    // puts the second at [0], unsplit.
    let mut a = OptionArray::default();
    a.apply_set(Some(3), "display hi", false, &sep(""));
    a.apply_set(None, "display two", true, &sep(""));
    assert_eq!(items(&a), vec![(0, "display two"), (3, "display hi")]);
}

#[test]
fn an_empty_value_without_an_index_empties_the_array() {
    let mut a = OptionArray::default();
    a.apply_set(Some(2), "x=y", false, &sep(","));
    a.apply_set(None, "", false, &sep(","));
    assert!(a.is_empty());
}

#[test]
fn unset_with_an_index_keeps_the_other_indexes() {
    // `set -su command-alias[1]` after [0] [1] [2] leaves [0] and [2].
    let spec = spec("command-alias").unwrap();
    let mut a = OptionArray::default();
    a.apply_set(None, "c=new-window,d=split-window -h,e=kill-pane", false, &sep(","));
    a.apply_unset(Some(1), spec);
    assert_eq!(items(&a), vec![(0, "c=new-window"), (2, "e=kill-pane")]);
}

/// tmux 3.7c, `show -s command-alias` on a new server.
const COMMAND_ALIAS_DEFAULTS: &[(u32, &str)] = &[
    (0, "split-pane=split-window"),
    (1, "splitp=split-window"),
    (2, "server-info=show-messages -JT"),
    (3, "info=show-messages -JT"),
    (4, "choose-window=choose-tree -w"),
    (5, "choose-session=choose-tree -s"),
];

#[test]
fn command_alias_starts_with_tmux_defaults() {
    let a = OptionArray::with_default(spec("command-alias").unwrap());
    assert_eq!(items(&a), COMMAND_ALIAS_DEFAULTS.to_vec());
    assert_eq!(
        a.show_lines("command-alias", false),
        "command-alias[0] split-pane=split-window\n\
         command-alias[1] splitp=split-window\n\
         command-alias[2] \"server-info=show-messages -JT\"\n\
         command-alias[3] \"info=show-messages -JT\"\n\
         command-alias[4] \"choose-window=choose-tree -w\"\n\
         command-alias[5] \"choose-session=choose-tree -s\"\n"
    );
}

#[test]
fn unset_without_an_index_puts_the_default_back() {
    // `set -su command-alias` after replacing the array: the six defaults
    // come back, as options_remove_or_default does for a global option.
    let spec = spec("command-alias").unwrap();
    let mut a = OptionArray::with_default(spec);
    a.apply_set(None, "x=list-keys", false, &sep(","));
    assert_eq!(items(&a), vec![(0, "x=list-keys")]);
    a.apply_unset(None, spec);
    assert_eq!(items(&a), COMMAND_ALIAS_DEFAULTS.to_vec());
}

#[test]
fn joined_uses_spaces() {
    // `display -p '#{command-alias}'` printed "c=new-window e=kill-pane".
    let mut a = OptionArray::default();
    a.apply_set(Some(0), "c=new-window", false, &sep(","));
    a.apply_set(Some(2), "e=kill-pane", false, &sep(","));
    assert_eq!(a.joined(), "c=new-window e=kill-pane");
}

#[test]
fn show_lines_prints_as_show_options_does() {
    let mut a = OptionArray::default();
    assert_eq!(a.show_lines("command-alias", false), "command-alias\n");
    assert_eq!(a.show_lines("command-alias", true), "");
    a.apply_set(Some(0), "c=new-window", false, &sep(","));
    a.apply_set(Some(1), "d=split-window -h", false, &sep(","));
    assert_eq!(
        a.show_lines("command-alias", false),
        "command-alias[0] c=new-window\ncommand-alias[1] \"d=split-window -h\"\n"
    );
    assert_eq!(a.show_lines("command-alias", true), "c=new-window\nd=split-window -h\n");
}

#[test]
fn is_array_name_takes_the_bare_name_or_any_bracketed_index() {
    assert_eq!(is_array_name("terminal-overrides").map(|spec| spec.name), Some("terminal-overrides"));
    assert_eq!(is_array_name("terminal-overrides[10]").map(|spec| spec.name), Some("terminal-overrides"));
    // A bad index is still the array's, so set answers it with invalid option.
    assert_eq!(is_array_name("terminal-overrides[x]").map(|spec| spec.name), Some("terminal-overrides"));
    assert!(parse_name("terminal-overrides[x]").is_err());
    assert_eq!(is_array_name("command-alias[0]").map(|spec| spec.name), Some("command-alias"));
    assert!(is_array_name("terminal-overridesx").is_none());
    assert!(is_array_name("terminal-overrides[1").is_none());
    assert!(is_array_name("status-left").is_none());
}

#[test]
fn terminal_overrides_starts_as_tmux_default() {
    // options-table.c:518-525 at 3.7c: `.default_str = "linux*:AX@"`,
    // `.separator = ","`; tmux 3.7c `show -s terminal-overrides` prints
    // `terminal-overrides[0] linux*:AX@`.
    let spec = spec("terminal-overrides").unwrap();
    assert_eq!(spec.separator, ",");
    let array = OptionArray::with_default(spec);
    assert_eq!(items(&array), vec![(0, "linux*:AX@")]);
}

#[test]
fn a_trimming_spec_keeps_each_element_without_its_spaces() {
    // codepoint-widths: `set -s codepoint-widths "U+2502=2, U+4E2D=1"` keeps
    // two working entries. tmux 3.8 keeps the space, `[1] " U+4E2D=1"`.
    let trim = ArraySpec { trim: true, ..sep(",") };
    let mut a = OptionArray::default();
    a.apply_set(None, " U+2502=2, U+4E2D=1 ,  ,", false, &trim);
    assert_eq!(items(&a), vec![(0, "U+2502=2"), (1, "U+4E2D=1")]);
    a.apply_set(Some(5), "  U+25CF=2 ", false, &trim);
    assert_eq!(a.get(5), Some("U+25CF=2"));
    a.apply_set(None, "   ", false, &trim);
    assert!(a.is_empty(), "a value of spaces only is an empty value");
    let mut a = OptionArray::default();
    a.apply_set(None, "U+2502=2, U+4E2D=1", false, &sep(","));
    assert_eq!(items(&a), vec![(0, "U+2502=2"), (1, " U+4E2D=1")]);
}

#[test]
fn codepoint_widths_is_an_array_with_no_default() {
    // options-table.c:337-344 at tag 3.8: `.default_str = ""`,
    // `.separator = ","`; tmux 3.8 `show -s codepoint-widths` on a new
    // server prints the name alone.
    let spec = spec("codepoint-widths[2]").unwrap();
    assert_eq!((spec.name, spec.separator, spec.default, spec.trim), ("codepoint-widths", ",", "", true));
    let array = OptionArray::with_default(spec);
    assert!(array.is_empty());
    assert_eq!(array.show_lines("codepoint-widths", false), "codepoint-widths
");
}
