// Issue #1014: a user-keys value set from the command prompt, a key binding or
// a hook line forwarded to another session kept `\e` as two characters. tmux
// reads every such line with its parser (cmd-parse.y:1701-1705, `\e` is ESC at
// :1478), and psmux read only the config file's line that way.
//
// `user_keys_line_tmux_read` rewrites the value of such a line so psmux's own
// lexer hands the setter the bytes tmux's parser would.

use super::*;
use crate::commands::parse_command_line;

const ESC: char = '\u{1b}';

/// The words the server reads from the rewritten line.
fn words(line: &str) -> Vec<String> {
    parse_command_line(&user_keys_line_tmux_read(line).expect("a user-keys line"))
}

#[test]
fn double_quoted_escape_is_esc() {
    let w = words(r#"set -s user-keys[0] "\e[99~""#);
    assert_eq!(w, vec!["set", "-s", "user-keys[0]", &format!("{ESC}[99~")]);
}

#[test]
fn unquoted_and_octal_escapes_are_esc() {
    // A binding is stored with its quotes gone (list-keys shows the word
    // bare), and tmux reads a bare `\e` as ESC too.
    assert_eq!(words(r"set -s user-keys[0] \e[99~")[3], format!("{ESC}[99~"));
    assert_eq!(words(r#"set -s user-keys[2] "\033Oz""#)[3], format!("{ESC}Oz"));
}

#[test]
fn single_quotes_keep_the_backslash() {
    assert_eq!(words(r"set -s user-keys[0] '\e[99~'")[3], r"\e[99~");
}

#[test]
fn target_flags_and_the_whole_array() {
    let w = words(r#"set -s -t =B: user-keys[0] "\e[99~""#);
    assert_eq!(w, vec!["set", "-s", "-t", "=B:", "user-keys[0]", &format!("{ESC}[99~")]);
    let w = words(r#"set-option -sa user-keys "\e[98~""#);
    assert_eq!(w[3], format!("{ESC}[98~"));
    let w = words(r#"set -s user-keys "\e[99~,\eOz""#);
    assert_eq!(w[3], format!("{ESC}[99~,{ESC}Oz"));
}

#[test]
fn a_value_with_a_space_or_quote_stays_one_word() {
    let w = words(r#"set -s user-keys[1] "\e[1 \"x""#);
    assert_eq!(w, vec!["set", "-s", "user-keys[1]", &format!("{ESC}[1 \"x")]);
}

#[test]
fn reading_twice_does_not_read_again() {
    // The line a client's connection rewrote is what a forward sends on; the
    // ESC is no backslash, so a second reading changes nothing.
    let once = user_keys_line_tmux_read(r#"set -s user-keys[0] "\e[99~""#).unwrap();
    assert_eq!(parse_command_line(&user_keys_line_tmux_read(&once).unwrap()), parse_command_line(&once));
}

#[test]
fn other_lines_are_left_alone() {
    // Other options keep a backslash as written, for Windows paths.
    assert_eq!(user_keys_line_tmux_read(r#"set -g default-shell "C:\Program Files\x\e.exe""#), None);
    assert_eq!(user_keys_line_tmux_read(r#"set -g @k "\e[99~""#), None);
    assert_eq!(user_keys_line_tmux_read(r#"display-message "\e[99~""#), None);
    assert_eq!(user_keys_line_tmux_read("set -su user-keys[0]"), None);
    assert_eq!(user_keys_line_tmux_read(""), None);
}
