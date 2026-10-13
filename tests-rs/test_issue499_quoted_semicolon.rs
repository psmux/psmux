// Issue #499: a `;` (or `\;`) inside a double-quoted argument split the
// one-shot command line, truncating the stored value and executing the
// remainder as its own command.
//
// The splitter under test is `split_chained_commands`, used by the one-shot
// server path (`server/connection.rs handle_connection`), bind-key parsing,
// and the client-side chain paths. Separator semantics follow tmux's
// `cmd_parse_from_arguments`: only a whole top-level token of `;` / `\;`
// ends a command.

use super::split_chained_commands_pub as split;
use super::AppState;

// ---------------------------------------------------------------------------
// The reported bug: quoted semicolons are data, not separators.
// ---------------------------------------------------------------------------

#[test]
fn quoted_semicolon_does_not_split() {
    let out = split(r#"set-option -t demo @key "a ; b""#);
    assert_eq!(
        out.len(),
        1,
        "BUG #499: quoted `;` split the command line into {:?}",
        out
    );
    assert_eq!(out[0], r#"set-option -t demo @key "a ; b""#);
}

#[test]
fn quoted_escaped_semicolon_does_not_split() {
    let out = split(r#"set-option -t demo @key2 "a \; b""#);
    assert_eq!(
        out.len(),
        1,
        "BUG #499: quoted `\\;` split the command line into {:?}",
        out
    );
    assert_eq!(out[0], r#"set-option -t demo @key2 "a \; b""#);
}

#[test]
fn quoted_semicolon_does_not_leak_a_second_command() {
    // The security-relevant half of #499: everything after the quoted `;`
    // used to be dispatched as its own command (`"x ; kill-server"` killed
    // the server).
    let out = split(r#"set-option -t demo @key3 "x ; kill-server""#);
    assert_eq!(out.len(), 1, "remainder leaked as a command: {:?}", out);
    assert!(
        !out.iter().skip(1).any(|c| c.contains("kill-server")),
        "kill-server escaped the quotes: {:?}",
        out
    );
}

#[test]
fn single_quoted_semicolon_does_not_split() {
    let out = split("set-option -t demo @key 'a ; b'");
    assert_eq!(out.len(), 1, "single-quoted `;` split: {:?}", out);
    assert_eq!(out[0], "set-option -t demo @key 'a ; b'");
}

// ---------------------------------------------------------------------------
// Genuine chaining must keep working (the issue's suggested fix, reusing
// `split_top_level_semicolons`, would have broken `\;` entirely).
// ---------------------------------------------------------------------------

#[test]
fn escaped_semicolon_token_still_chains() {
    let out = split(r"split-window \; select-pane -D");
    assert_eq!(out, vec!["split-window", "select-pane -D"]);
}

#[test]
fn bare_semicolon_token_still_chains() {
    let out = split("split-window ; select-pane -D");
    assert_eq!(out, vec!["split-window", "select-pane -D"]);
}

#[test]
fn three_way_chain() {
    let out = split(r"new-window \; split-window -v \; select-pane -t 0");
    assert_eq!(
        out,
        vec!["new-window", "split-window -v", "select-pane -t 0"]
    );
}

#[test]
fn trailing_separator_yields_no_empty_command() {
    assert_eq!(split(r"new-window \;"), vec!["new-window"]);
    assert_eq!(split("new-window ;"), vec!["new-window"]);
}

#[test]
fn leading_separator_yields_no_empty_command() {
    assert_eq!(split("; new-window"), vec!["new-window"]);
}

#[test]
fn chaining_still_works_around_a_quoted_semicolon_value() {
    // A real separator and a quoted `;` in the same line.
    let out = split(r#"set-option @a "x ; y" \; set-option @b 2"#);
    assert_eq!(out.len(), 2, "expected exactly 2 sub-commands, got {:?}", out);
    assert_eq!(out[0], r#"set-option @a "x ; y""#);
    assert_eq!(out[1], "set-option @b 2");
}

// ---------------------------------------------------------------------------
// A semicolon in the middle of a token is not a separator, so `a;b`
// round-trips. A token that ENDS in `;` does end the command (#734 follow
// up): both tmux grammars agree (cmd-parse.y:1098 for argv, :1675 for a
// command string), so `a; b` is `a` then `b`.
// ---------------------------------------------------------------------------

#[test]
fn mid_token_semicolon_is_not_a_separator() {
    assert_eq!(split("set-option @key a;b"), vec!["set-option @key a;b"]);
}

#[test]
fn token_trailing_semicolon_ends_the_command() {
    assert_eq!(split("set-option @key a; b"), vec!["set-option @key a", "b"]);
}

#[test]
fn escaped_semicolon_inside_a_token_is_not_a_separator() {
    assert_eq!(split(r"set-option @key a\;b"), vec![r"set-option @key a\;b"]);
}

// ---------------------------------------------------------------------------
// Whitespace preservation: sub-commands are slices of the original line, not
// re-joined `split_whitespace()` tokens.
// ---------------------------------------------------------------------------

#[test]
fn inner_whitespace_survives_chaining() {
    let out = split(r#"set-option @a 1 \; set-option @b "x    y""#);
    assert_eq!(out.len(), 2);
    assert_eq!(
        out[1], r#"set-option @b "x    y""#,
        "runs of spaces inside a quoted value were collapsed"
    );
}

#[test]
fn tabs_inside_quotes_survive_chaining() {
    let out = split("set-option @a 1 \\; set-option @b \"x\ty\"");
    assert_eq!(out.len(), 2);
    assert_eq!(out[1], "set-option @b \"x\ty\"");
}

// ---------------------------------------------------------------------------
// Escapes and quote-state edge cases.
// ---------------------------------------------------------------------------

#[test]
fn escaped_quote_does_not_toggle_quote_state() {
    // The `\"` must not open a quoted region, so the trailing `\;` still
    // separates.
    let out = split(r#"display-message \"hi\" \; new-window"#);
    assert_eq!(out.len(), 2, "escaped quotes confused the splitter: {:?}", out);
    assert_eq!(out[1], "new-window");
}

#[test]
fn windows_path_backslashes_do_not_break_chaining() {
    let out = split(r#"set-option @p "C:\Program Files\Git\bin\bash.exe" \; new-window"#);
    assert_eq!(out.len(), 2, "path backslashes broke chaining: {:?}", out);
    assert_eq!(out[0], r#"set-option @p "C:\Program Files\Git\bin\bash.exe""#);
    assert_eq!(out[1], "new-window");
}

#[test]
fn semicolon_inside_quotes_next_to_a_real_separator() {
    let out = split(r#"set-option @a ";" \; new-window"#);
    assert_eq!(out.len(), 2, "got {:?}", out);
    assert_eq!(out[0], r#"set-option @a ";""#);
}

#[test]
fn unterminated_quote_does_not_panic() {
    // Malformed input must not panic or lose the line entirely.
    let out = split(r#"set-option @a "unterminated ; still here"#);
    assert_eq!(out.len(), 1, "got {:?}", out);
}

#[test]
fn trailing_backslash_does_not_panic() {
    let out = split(r"new-window \");
    assert!(!out.is_empty());
}

#[test]
fn empty_and_whitespace_input() {
    assert!(split("").is_empty());
    assert!(split("   ").is_empty());
    assert!(split(" ; ").is_empty());
}

// ---------------------------------------------------------------------------
// bind-key: the same splitter drives config chaining, so a quoted `;` in a
// bound command must survive into the binding.
// ---------------------------------------------------------------------------

#[test]
fn bind_key_with_quoted_semicolon_stays_one_command() {
    let mut app = AppState::new("test_session".to_string());
    super::parse_bind_key(&mut app, r#"bind-key X display-message "a ; b""#);
    let table = app.key_tables.get("prefix").expect("prefix table");
    let kb = table
        .iter()
        .find(|kb| kb.key.0 == crossterm::event::KeyCode::Char('X'))
        .expect("binding for X should exist");
    match &kb.action {
        crate::types::Action::CommandChain(cmds) => panic!(
            "BUG #499: quoted `;` split a bound command into a chain: {:?}",
            cmds
        ),
        crate::types::Action::Command(cmd) => assert!(
            cmd.contains("a ; b"),
            "bound command lost the quoted value: {}",
            cmd
        ),
        _ => panic!("expected a Command action for `display-message`"),
    }
}

#[test]
fn bind_key_real_chain_still_produces_a_chain() {
    let mut app = AppState::new("test_session".to_string());
    super::parse_bind_key(&mut app, r"bind-key Y split-window \; select-pane -D");
    let table = app.key_tables.get("prefix").expect("prefix table");
    let kb = table
        .iter()
        .find(|kb| kb.key.0 == crossterm::event::KeyCode::Char('Y'))
        .expect("binding for Y should exist");
    match &kb.action {
        crate::types::Action::CommandChain(cmds) => {
            assert_eq!(cmds.len(), 2, "expected 2 chained commands, got {:?}", cmds);
            assert_eq!(cmds[0], "split-window");
            assert_eq!(cmds[1], "select-pane -D");
        }
        _ => panic!("chained binding did not produce a CommandChain"),
    }
}
