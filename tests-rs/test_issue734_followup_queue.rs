// Issue #734 follow up (GiorgoLazaridis, 2026-10-09): an if-shell branch
// `new-session -d -P -F fmt -s probe ; display-message -p CREATED` printed the
// creation record but never ran the marker, and the unspaced spelling
// `-s probe; display-message -p CREATED` made a session named `probe;`.
//
// tmux splits a command at a token that ends in an unescaped `;` in both of
// its grammars: cmd_parse_from_arguments cuts the `;` off an argv element and
// ends the command (cmd-parse.y:1098), and the command string lexer ends a
// token at an unquoted `;` and returns `;` as the separator (cmd-parse.y:1314
// and :1675). These tests pin the server side splitter to that rule and the
// CLI wire quoting that keeps an argv value ending in `;` one argument.

use super::split_chained_commands_pub as split;
use super::is_modified_semicolon_key;

#[test]
fn the_reported_unspaced_branch_is_two_commands() {
    let fmt = "#S:#{window_index}\t#{pane_id}\t#{socket_path}\t#{pid}";
    let branch = format!("new-session -d -P -F '{fmt}' -s probe; display-message -p CREATED");
    assert_eq!(
        split(&branch),
        vec![
            format!("new-session -d -P -F '{fmt}' -s probe"),
            "display-message -p CREATED".to_string(),
        ],
        "a token ending in `;` must end the command, not become the session name `probe;`"
    );
}

#[test]
fn the_spaced_branch_is_unchanged() {
    assert_eq!(
        split("new-session -d -s probe ; display-message -p CREATED"),
        vec!["new-session -d -s probe", "display-message -p CREATED"]
    );
}

#[test]
fn a_quoted_token_followed_by_semicolon_ends_the_command() {
    // tmux: `'probe';` lexes as the token probe, then `;`.
    assert_eq!(
        split("new-session -d -s 'probe'; display-message -p x"),
        vec!["new-session -d -s 'probe'", "display-message -p x"]
    );
    assert_eq!(
        split(r#"display-message -p "a b"; display-message -p c"#),
        vec![r#"display-message -p "a b""#, "display-message -p c"]
    );
}

#[test]
fn a_semicolon_inside_quotes_is_still_data() {
    assert_eq!(split(r#"set -g @v "x;" ; set -g @w 1"#), vec![r#"set -g @v "x;""#, "set -g @w 1"]);
    assert_eq!(split("set -g @v 'echo a;'"), vec!["set -g @v 'echo a;'"]);
}

#[test]
fn an_escaped_trailing_semicolon_is_data() {
    // cmd-parse.y:1100: `\;` at the end of an argument is a literal `;`.
    assert_eq!(split(r"set -g @v x\;"), vec![r"set -g @v x\;"]);
    assert_eq!(split(r"set -g @v x\; ; set -g @w 1"), vec![r"set -g @v x\;", "set -g @w 1"]);
}

#[test]
fn a_mid_token_semicolon_is_data() {
    assert_eq!(split("set -g @v a;b"), vec!["set -g @v a;b"]);
}

#[test]
fn several_trailing_separators_make_several_commands() {
    assert_eq!(
        split("display-message -p a; display-message -p b; display-message -p c"),
        vec!["display-message -p a", "display-message -p b", "display-message -p c"]
    );
}

#[test]
fn a_modified_semicolon_key_is_not_a_separator() {
    // psmux has always bound `M-;` as written; keep it a key name.
    assert_eq!(split("bind-key -n M-; display-message hi"), vec!["bind-key -n M-; display-message hi"]);
    assert_eq!(split("bind-key -n C-M-; last-pane"), vec!["bind-key -n C-M-; last-pane"]);
    assert_eq!(split("bind-key ^; last-pane"), vec!["bind-key ^; last-pane"]);
    assert!(is_modified_semicolon_key("M-;"));
    assert!(is_modified_semicolon_key("c-s-;"));
    assert!(!is_modified_semicolon_key(";"));
    assert!(!is_modified_semicolon_key("probe;"));
    assert!(!is_modified_semicolon_key("X-;"));
    assert!(!is_modified_semicolon_key("M-a;"));
}

#[test]
fn cli_wire_quoting_keeps_an_argv_value_ending_in_semicolon() {
    use crate::util::quote_arg_if_needed as q;
    // On the wire bare `x;` would now end the command; quoted it stays data,
    // which is what the CLI route has always meant by an argv `x;`.
    assert_eq!(q("x;"), "\"x;\"");
    assert_eq!(split(&format!("set -g @v {}", q("x;"))), vec!["set -g @v \"x;\""]);
    // A lone `;` is a separator and a modified `;` key stays raw for bind-key.
    assert_eq!(q(";"), ";");
    assert_eq!(q("M-;"), "M-;");
    assert_eq!(q(r"x\;"), r"x\;");
    assert_eq!(q("plain"), "plain");
}
