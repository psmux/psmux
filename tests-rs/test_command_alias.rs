// command-alias is tmux's server ARRAY option. The tmux spelling
// `set -s command-alias[100] 'x=...'` used to land in the user option store
// as plain text, so the alias never existed. These tests pin the array, the
// lookup order, the show text and the expansion that every route shares.

use super::*;

fn words(line: &str) -> Vec<String> {
    crate::commands::parse_command_line(line)
}

#[test]
fn indexed_name_sets_that_element() {
    let mut a = CommandAliases::default();
    a.set("command-alias[100]", "zz=rename-window ZZ", false).unwrap();
    assert_eq!(a.get("zz").as_deref(), Some("rename-window ZZ"));
    assert_eq!(a.items().collect::<Vec<_>>(), vec![(100, "zz=rename-window ZZ")]);
}

#[test]
fn indexed_name_that_is_no_number_is_refused() {
    let mut a = CommandAliases::default();
    assert_eq!(
        a.set("command-alias[x]", "zz=list-keys", false),
        Err("invalid option: command-alias[x]".to_string())
    );
    assert!(a.is_empty());
}

#[test]
fn is_alias_option_takes_the_bare_and_indexed_names_only() {
    assert!(is_alias_option("command-alias"));
    assert!(is_alias_option("command-alias[3]"));
    assert!(!is_alias_option("command-aliases"));
    assert!(!is_alias_option("status-format[1]"));
}

#[test]
fn bare_name_replaces_the_whole_array() {
    // tmux: a value without an index clears the array and is split on `,`
    // (cmd-set-option.c:210-217), so only the last plain `set` survives.
    let mut a = CommandAliases::default();
    a.set("command-alias[5]", "sp=split-window", false).unwrap();
    a.set("command-alias", "nw=new-window", false).unwrap();
    assert_eq!(a.items().collect::<Vec<_>>(), vec![(0, "nw=new-window")]);
    a.set("command-alias", "sp=split-window -h,nw=new-window -d", false).unwrap();
    assert_eq!(
        a.items().collect::<Vec<_>>(),
        vec![(0, "sp=split-window -h"), (1, "nw=new-window -d")]
    );
}

#[test]
fn append_always_adds_an_element() {
    let mut a = CommandAliases::default();
    a.set("command-alias", "sp=split-window", false).unwrap();
    a.set("command-alias", "sp=split-window -h", true).unwrap();
    assert_eq!(a.items().count(), 2);
    // The lower index is the one tmux's cmd_get_alias finds first.
    assert_eq!(a.get("sp").as_deref(), Some("split-window"));
}

#[test]
fn append_on_an_element_extends_its_text() {
    let mut a = CommandAliases::default();
    a.set("command-alias[3]", "sp=split-window", false).unwrap();
    a.set("command-alias[3]", " -h", true).unwrap();
    assert_eq!(a.get("sp").as_deref(), Some("split-window -h"));
}

#[test]
fn unset_drops_one_element_or_the_whole_array() {
    let mut a = CommandAliases::default();
    a.set("command-alias[1]", "a=list-keys", false).unwrap();
    a.set("command-alias[2]", "b=list-buffers", false).unwrap();
    a.unset("command-alias[1]").unwrap();
    assert!(!a.contains_key("a"));
    assert!(a.contains_key("b"));
    // Without an index `-u` puts tmux's defaults back.
    a.unset("command-alias").unwrap();
    assert_eq!(a, CommandAliases::with_defaults());
}

#[test]
fn a_server_starts_with_tmux_default_aliases() {
    // options-table.c:301-306.
    let aliases = read(&app().command_aliases).clone();
    assert_eq!(aliases, CommandAliases::with_defaults());
    assert_eq!(aliases.get("split-pane").as_deref(), Some("split-window"));
    assert_eq!(aliases.get("splitp").as_deref(), Some("split-window"));
    assert_eq!(aliases.get("server-info").as_deref(), Some("show-messages -JT"));
    assert_eq!(aliases.get("info").as_deref(), Some("show-messages -JT"));
    assert_eq!(aliases.get("choose-window").as_deref(), Some("choose-tree -w"));
    assert_eq!(aliases.get("choose-session").as_deref(), Some("choose-tree -s"));
    assert_eq!(aliases.items().map(|(i, _)| i).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn name_matches_the_text_before_the_first_equals_exactly() {
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "x=display-message a=b", false).unwrap();
    assert_eq!(a.get("x").as_deref(), Some("display-message a=b"));
    assert_eq!(a.get("x=display-message a"), None);
    assert_eq!(a.get("X"), None);
}

#[test]
fn show_lines_print_index_and_quote_values_with_spaces() {
    let mut a = CommandAliases::default();
    assert_eq!(a.show_lines(false), "command-alias\n");
    assert_eq!(a.show_lines(true), "");
    a.set("command-alias[1]", "sp=split-window", false).unwrap();
    a.set("command-alias[100]", "zz=rename-window ZZ", false).unwrap();
    assert_eq!(
        a.show_lines(false),
        "command-alias[1] sp=split-window\ncommand-alias[100] \"zz=rename-window ZZ\"\n"
    );
    assert_eq!(a.show_lines(true), "sp=split-window\nzz=rename-window ZZ\n");
    assert_eq!(a.value("command-alias[100]"), "zz=rename-window ZZ");
    assert_eq!(a.value("command-alias[7]"), "");
}

#[test]
fn show_output_reads_back_into_the_same_table() {
    let mut a = CommandAliases::default();
    a.set("command-alias[1]", "sp=split-window", false).unwrap();
    a.set("command-alias[100]", "zz=display-message \"a b\"", false).unwrap();
    assert_eq!(from_show_output(&a.show_lines(false)), a);
    assert_eq!(from_show_output("command-alias\n"), CommandAliases::default());
}

#[test]
fn expand_appends_the_callers_words_to_the_last_command() {
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "sp=split-window -h", false).unwrap();
    assert_eq!(
        a.expand(&words("sp -d -c C:\\tmp")),
        Some(vec![words("split-window -h -d -c C:\\tmp")])
    );
    assert_eq!(a.expand(&words("split-window -h")), None);
}

#[test]
fn expand_keeps_quoted_words_whole() {
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "hi=display-message \"a b\"", false).unwrap();
    assert_eq!(
        a.expand(&words("hi")),
        Some(vec![vec!["display-message".to_string(), "a b".to_string()]])
    );
}

#[test]
fn expand_splits_a_command_list_and_extends_only_its_last_command() {
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "two=split-window ; select-layout", false).unwrap();
    assert_eq!(
        a.expand(&words("two tiled")),
        Some(vec![words("split-window"), words("select-layout tiled")])
    );
    assert_eq!(
        a.expand_lines(&words("two tiled")),
        Some(vec!["split-window".to_string(), "select-layout tiled".to_string()])
    );
}

#[test]
fn expand_of_an_empty_alias_gives_no_commands() {
    // The old expansion indexed [1..] of an empty split and panicked the
    // connection thread on `set command-alias 'e='`.
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "e=", false).unwrap();
    assert_eq!(a.expand(&words("e -x")), Some(Vec::new()));
}

#[test]
fn expand_lines_quote_words_so_they_parse_back() {
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "hi=display-message", false).unwrap();
    let lines = a.expand_lines(&words("hi \"a b\"")).unwrap();
    assert_eq!(lines, vec!["display-message \"a b\"".to_string()]);
    assert_eq!(words(&lines[0]), vec!["display-message".to_string(), "a b".to_string()]);
}

#[test]
fn expansion_is_one_level() {
    // tmux builds an alias's commands with CMD_PARSE_NOALIAS, so an alias
    // that names itself or another alias stops after one step.
    let mut a = CommandAliases::default();
    a.set("command-alias[0]", "x=x -a", false).unwrap();
    assert_eq!(a.expand(&words("x")), Some(vec![words("x -a")]));
}

fn app() -> crate::types::AppState {
    crate::types::AppState::new("alias-test".to_string())
}

fn user_option(app: &crate::types::AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

#[test]
fn config_line_with_an_index_defines_the_alias() {
    let mut app = app();
    crate::config::parse_config_content(&mut app, "set -s command-alias[100] 'zz=rename-window ZZ'\n");
    assert_eq!(read(&app.command_aliases).get("zz").as_deref(), Some("rename-window ZZ"));
    assert_eq!(user_option(&app, "command-alias[100]"), None, "not parked as text any more");
    assert_eq!(
        crate::server::options::get_option_value(&app, "command-alias[100]"),
        "zz=rename-window ZZ"
    );
}

#[test]
fn config_append_and_unset_work_on_the_array() {
    let mut app = app();
    crate::config::parse_config_content(
        &mut app,
        "set -s command-alias[4] 'a=list-keys'\nset -as command-alias 'b=list-buffers'\n",
    );
    // [4] replaced choose-window, one of the six defaults, and `-a` takes
    // the lowest free index, as tmux's options_array_assign does.
    assert_eq!(read(&app.command_aliases).value("command-alias[4]"), "a=list-keys");
    assert_eq!(read(&app.command_aliases).value("command-alias[6]"), "b=list-buffers");
    crate::config::parse_config_content(&mut app, "set -su command-alias[4]\n");
    assert!(!read(&app.command_aliases).contains_key("a"));
    assert!(read(&app.command_aliases).contains_key("b"));
    assert_eq!(
        read(&app.command_aliases).items().map(|(i, _)| i).collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 5, 6]
    );
    crate::config::parse_config_content(&mut app, "set -su command-alias\n");
    assert_eq!(*read(&app.command_aliases), CommandAliases::with_defaults());
    // An empty value without an index leaves the array empty.
    crate::config::parse_config_content(&mut app, "set -s command-alias ''\n");
    assert!(read(&app.command_aliases).is_empty());
}

#[test]
fn config_index_that_is_no_number_warns() {
    let mut app = app();
    crate::config::parse_config_content(&mut app, "set -s command-alias[x] 'zz=list-keys'\n");
    assert_eq!(*read(&app.command_aliases), CommandAliases::with_defaults());
    assert!(
        app.config_warnings.iter().any(|w| w.contains("invalid option: command-alias[x]")),
        "got: {:?}",
        app.config_warnings
    );
}

#[test]
fn command_string_route_runs_the_alias_with_quotes_and_lists() {
    // The route key bindings, the command prompt, hooks and menus take.
    let mut app = app();
    crate::config::parse_config_content(
        &mut app,
        "set -s command-alias[0] 'q=set -g @q \"a b\"'\n\
         set -s command-alias[1] 'two=set -g @one 1 ; set -g @two'\n",
    );
    crate::commands::execute_command_string(&mut app, "q").unwrap();
    assert_eq!(user_option(&app, "@q").as_deref(), Some("a b"));
    crate::commands::execute_command_string(&mut app, "two 2").unwrap();
    assert_eq!(user_option(&app, "@one").as_deref(), Some("1"));
    assert_eq!(user_option(&app, "@two").as_deref(), Some("2"));
}

#[test]
fn command_string_route_does_not_expand_an_alias_twice() {
    let mut app = app();
    crate::config::parse_config_content(&mut app, "set -s command-alias[0] 'set=set -g @hit'\n");
    // `set v` becomes `set -g @hit v`, whose `set` is not expanded again.
    crate::commands::execute_command_string(&mut app, "set v").unwrap();
    assert_eq!(user_option(&app, "@hit").as_deref(), Some("v"));
}
