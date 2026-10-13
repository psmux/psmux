// tmux 3.x brace blocks in key bindings: `bind x { a; b }`, the same over
// several lines, and nested `if -F c { a } { b }` (cmd-parse.y
// argument_statements). psmux used to keep `{ set -g @a 1` as the whole
// command, so the key did nothing useful, and a multi-line block broke the
// config apart line by line.

use super::*;

fn app() -> crate::types::AppState {
    crate::types::AppState::new("test_session".to_string())
}

fn bound(app: &crate::types::AppState, table: &str, key: &str) -> Option<String> {
    let k = normalize_key_for_binding(parse_key_name(key)?);
    app.key_tables.get(table)?.iter().find(|b| b.key == k).map(|b| crate::commands::format_action(&b.action))
}

fn chain_len(app: &crate::types::AppState, table: &str, key: &str) -> usize {
    let k = normalize_key_for_binding(parse_key_name(key).unwrap());
    match &app.key_tables[table].iter().find(|b| b.key == k).unwrap().action {
        crate::types::Action::CommandChain(c) => c.len(),
        _ => 1,
    }
}

#[test]
fn block_depth_follows_the_tmux_lexer() {
    let d = |s: &str| block_depth_map(&s.chars().collect::<Vec<_>>());
    assert!(d("a { b } c").is_some());
    assert!(d("a { b").is_none(), "an open block is unbalanced");
    assert!(d("a } b").is_none(), "a close with nothing open");
    // A format is not a block, quoted or not.
    assert_eq!(d("display #{pane_id}").unwrap().iter().max(), Some(&0));
    assert_eq!(d("if -F '#{x}' { a }").unwrap().iter().max(), Some(&1));
    // A brace inside a word is not a block opener.
    assert_eq!(d("set -g @v a{b").unwrap().iter().max(), Some(&0));
    assert!(d("display '{'").is_some());
}

#[test]
fn a_semicolon_inside_a_block_does_not_split() {
    assert_eq!(split_chained_commands_pub("if -F c { a; b } { c }"), vec!["if -F c { a; b } { c }"]);
    assert_eq!(split_chained_commands_pub("a \\; b"), vec!["a", "b"]);
    assert_eq!(split_chained_commands_pub("x { a ; b } ; y"), vec!["x { a ; b }", "y"]);
}

#[test]
fn the_command_list_of_a_block_binding() {
    assert_eq!(bind_command_list("{ set -g @a 1; set -g @b 2 }"), vec!["set -g @a 1", "set -g @b 2"]);
    assert_eq!(bind_command_list("{ display hi }"), vec!["display hi"]);
    assert_eq!(
        bind_command_list("{ if -F '#{pane_in_mode}' { set -g @d in } { set -g @d out } }"),
        vec!["if -F '#{pane_in_mode}' { set -g @d in } { set -g @d out }"]
    );
    // Not one block: the old reading.
    assert_eq!(bind_command_list("set -g @a 1 \\; set -g @b 2"), vec!["set -g @a 1", "set -g @b 2"]);
    assert_eq!(bind_command_list("if -F c { a } { b }"), vec!["if -F c { a } { b }"]);
}

#[test]
fn one_line_block_in_a_config() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n F8 { set -g @b1 yes; set -g @b2 yes }\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(chain_len(&app, "root", "F8"), 2);
    assert_eq!(bound(&app, "root", "F8").unwrap(), "set -g @b1 yes \\; set -g @b2 yes");
}

#[test]
fn multi_line_block_in_a_config() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n F9 {\n  set -g @c1 yes\n\n  # a comment\n  set -g @c2 yes\n}\nbind -n F2 set -g @after 1\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(chain_len(&app, "root", "F9"), 2);
    assert_eq!(bound(&app, "root", "F9").unwrap(), "set -g @c1 yes \\; set -g @c2 yes");
    // The line after the block is its own statement.
    assert_eq!(bound(&app, "root", "F2").unwrap(), "set -g @after 1");
}

#[test]
fn nested_if_blocks_stay_one_command() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n F10 { if -F '#{pane_in_mode}' { set -g @d in } { set -g @d out } }\n");
    assert_eq!(chain_len(&app, "root", "F10"), 1);
    assert_eq!(bound(&app, "root", "F10").unwrap(), "if -F '#{pane_in_mode}' { set -g @d in } { set -g @d out }");

    let mut app2 = crate::types::AppState::new("t2".to_string());
    parse_config_content(&mut app2, "bind -n F11 {\n  if -F '#{pane_in_mode}' {\n    set -g @e1 in\n  } {\n    set -g @e1 out\n    set -g @e2 out\n  }\n}\n");
    assert!(app2.config_warnings.is_empty(), "{:?}", app2.config_warnings);
    assert_eq!(chain_len(&app2, "root", "F11"), 1);
    assert_eq!(
        bound(&app2, "root", "F11").unwrap(),
        "if -F '#{pane_in_mode}' { set -g @e1 in } { set -g @e1 out ; set -g @e2 out }"
    );
}

#[test]
fn a_key_named_brace_does_not_swallow_the_config() {
    let mut app = app();
    parse_config_content(&mut app, "bind { swap-pane -U\nbind } swap-pane -D\nbind x display hi\n");
    assert_eq!(bound(&app, "prefix", "{").unwrap(), "swap-pane -U");
    assert_eq!(bound(&app, "prefix", "}").unwrap(), "swap-pane -D");
    assert_eq!(bound(&app, "prefix", "x").unwrap(), "display hi");
}

#[test]
fn top_level_if_shell_blocks_are_unchanged() {
    let mut app = app();
    parse_config_content(&mut app, "if-shell 'true' {\n  bind -n F3 set -g @t 1\n}\n");
    assert_eq!(bound(&app, "root", "F3").unwrap(), "set -g @t 1");
    let mut app = crate::types::AppState::new("t3".to_string());
    parse_config_content(&mut app, "if-shell 'true' {\n  bind -n F4 {\n    set -g @u 1\n    set -g @v 2\n  }\n}\n");
    assert_eq!(chain_len(&app, "root", "F4"), 2, "{:?}", bound(&app, "root", "F4"));
}

#[test]
fn a_mouse_key_takes_a_block_too() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n WheelUpPane { if -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e } }\n");
    assert_eq!(
        bound(&app, "root", "WheelUpPane").unwrap(),
        "if -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e }"
    );
}
