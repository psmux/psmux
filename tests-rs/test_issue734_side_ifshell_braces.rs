// Issue #734 side finding 2: a config file if-shell whose else block opens on
// the line that closes the true block was not understood:
//
//     if-shell 'exit 0' { set -g @a yes } { set -g @a no }
//
//     if-shell 'exit 1' {
//       set -g @c yes
//     } {
//       set -g @c no
//     }
//
// The first set nothing (`unknown command: {`), the second ran the wrong
// branch (the exit 0 form set `no`). tmux's lexer reads `{` and `}` as tokens
// wherever they stand (cmd-parse.y yylex), so both are ordinary tmux configs.
// psmux's collector works on whole lines, and only knew a header ending in
// `{`, a bare `}` and a bare `{`. Every brace now gets a line of its own
// before the collector runs.
//
// Conditions are psmux's literal shortcuts (`true` / `false`), so no shell is
// spawned.

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("t734side".to_string());
    app.control_port = None;
    app
}

fn opt(app: &AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

#[test]
fn one_line_if_shell_with_else_block() {
    let mut app = mock_app();
    parse_config_content(&mut app, "if-shell 'true' { set -g @a yes } { set -g @a no }\n");
    parse_config_content(&mut app, "if-shell 'false' { set -g @b yes } { set -g @b no }\n");
    assert_eq!(opt(&app, "@a").as_deref(), Some("yes"));
    assert_eq!(opt(&app, "@b").as_deref(), Some("no"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn one_line_if_shell_without_else_block() {
    let mut app = mock_app();
    parse_config_content(&mut app, "if-shell 'true' { set -g @a yes }\nif-shell 'false' { set -g @b yes }\n");
    assert_eq!(opt(&app, "@a").as_deref(), Some("yes"));
    assert_eq!(opt(&app, "@b"), None);
}

#[test]
fn close_and_open_on_one_line_between_multi_line_blocks() {
    let cfg = "\
if-shell 'false' {
  set -g @c yes
} {
  set -g @c no
}
if-shell 'true' {
  set -g @d yes
} {
  set -g @d no
}
set -g @after reached
";
    let mut app = mock_app();
    parse_config_content(&mut app, cfg);
    assert_eq!(opt(&app, "@c").as_deref(), Some("no"));
    assert_eq!(opt(&app, "@d").as_deref(), Some("yes"));
    assert_eq!(opt(&app, "@after").as_deref(), Some("reached"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn the_separate_line_form_still_works() {
    let cfg = "if-shell 'false' {\n  set -g @e yes\n}\n{\n  set -g @e no\n}\n";
    let mut app = mock_app();
    parse_config_content(&mut app, cfg);
    assert_eq!(opt(&app, "@e").as_deref(), Some("no"));
}

#[test]
fn a_nested_block_in_a_branch_runs() {
    let cfg = "\
if-shell 'true' {
  if-shell 'false' { set -g @n inner-yes } { set -g @n inner-no }
  set -g @m outer
}
";
    let mut app = mock_app();
    parse_config_content(&mut app, cfg);
    assert_eq!(opt(&app, "@n").as_deref(), Some("inner-no"));
    assert_eq!(opt(&app, "@m").as_deref(), Some("outer"));
}

#[test]
fn formats_and_quoted_braces_are_not_block_braces() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "if-shell -F '#{==:a,a}' { set -g @f '{x}' } { set -g @f no }\n",
    );
    assert_eq!(opt(&app, "@f").as_deref(), Some("{x}"));
}

#[test]
fn splitter_shapes() {
    assert_eq!(split_brace_line("set -g @x y"), None);
    assert_eq!(split_brace_line("if-shell 'true' 'set -g @a b'"), None);
    assert_eq!(
        split_brace_line("if-shell 'c' { set -g @a yes } { set -g @a no }"),
        Some(vec![
            "if-shell 'c' {".to_string(),
            "set -g @a yes".to_string(),
            "}".to_string(),
            "{".to_string(),
            "set -g @a no".to_string(),
            "}".to_string(),
        ])
    );
    assert_eq!(split_brace_line("} {"), Some(vec!["}".to_string(), "{".to_string()]));
    assert_eq!(split_brace_line("if-shell 'c' {"), Some(vec!["if-shell 'c' {".to_string()]));
    assert_eq!(
        split_brace_line("if -F #{x} { set -g @a '}' }"),
        Some(vec!["if -F #{x} {".to_string(), "set -g @a '}'".to_string(), "}".to_string()])
    );
}
