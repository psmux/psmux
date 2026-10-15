// Issue #823: argument parsing differs from tmux.
//
// tmux's lexer (cmd-parse.y yylex_token, :1702) treats `\` outside single
// quotes as an escape and keeps building the same token, so 'a'\''b' is one
// argument a'b. Inside double quotes yylex_token_escape (:1441) decodes
// \a \b \e \f \s \v \r \n \t, octal \NNN (first digit 0 to 3), \uXXXX,
// \UXXXXXXXX, and \c -> c for any other c.
//
// psmux keeps backslashes literal outside quotes and in single quotes on every
// route (Windows paths), decodes `\'` and `\"` outside quotes everywhere, and
// applies the full tmux double quote escapes only on the control mode route,
// where clients escape the tmux way. Config files keep the Windows friendly
// double quote rule (only \" and \\), because documented configs say
// set -g default-shell "C:\Program Files\Git\bin\bash.exe".

use super::*;
use crate::commands::{parse_command_line, parse_command_line_tmux};

fn v(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

// ------------------------------------------------ '\'' joins on every route

#[test]
fn posix_quote_idiom_is_one_argument() {
    let line = r#"rename-window -t '@1' 'a'\''b'"#;
    assert_eq!(parse_command_line(line), v(&["rename-window", "-t", "@1", "a'b"]));
    assert_eq!(parse_command_line_tmux(line), v(&["rename-window", "-t", "@1", "a'b"]));
}

#[test]
fn double_quote_idiom_still_joins() {
    let line = r#"rename-window 'it'"'"'s'"#;
    assert_eq!(parse_command_line(line), v(&["rename-window", "it's"]));
    assert_eq!(parse_command_line_tmux(line), v(&["rename-window", "it's"]));
}

#[test]
fn unquoted_escaped_quotes_are_literal() {
    assert_eq!(parse_command_line(r#"x a\"b it\'s"#), v(&["x", "a\"b", "it's"]));
    assert_eq!(parse_command_line_tmux(r#"x a\"b it\'s"#), v(&["x", "a\"b", "it's"]));
}

#[test]
fn backslash_in_single_quotes_never_escapes() {
    // 'a\' closes at the second quote: tmux never escapes in single quotes.
    assert_eq!(parse_command_line(r"x 'a\' b"), v(&["x", r"a\", "b"]));
    assert_eq!(parse_command_line_tmux(r"x 'L1\nL2'"), v(&["x", r"L1\nL2"]));
}

// ------------------------------------------- control mode double quote escapes

#[test]
fn tmux_double_quote_escapes_issue_line() {
    let line = r#"set-buffer -b mq "L1\nL2\tT\u0041\e[0m\$HOME\~x\101""#;
    assert_eq!(
        parse_command_line_tmux(line),
        v(&["set-buffer", "-b", "mq", "L1\nL2\tTA\x1b[0m$HOME~xA"])
    );
}

#[test]
fn tmux_control_code_points_and_simple_escapes() {
    assert_eq!(parse_command_line_tmux(r#"x "u1\u000au2\u0009T""#), v(&["x", "u1\nu2\tT"]));
    assert_eq!(parse_command_line_tmux(r#"x "\a\b\f\v\r\s""#), v(&["x", "\x07\x08\x0c\x0b\r "]));
    assert_eq!(parse_command_line_tmux(r#"x "\\back \"dq\" \#{x} \; %""#), v(&["x", "\\back \"dq\" #{x} ; %"]));
    assert_eq!(parse_command_line_tmux(r#"x "\U0001F600""#), v(&["x", "\u{1F600}"]));
}

#[test]
fn tmux_octal_bytes_build_utf8() {
    assert_eq!(parse_command_line_tmux(r#"x "caf\303\251""#), v(&["x", "caf\u{e9}"]));
    assert_eq!(parse_command_line_tmux(r#"x "\012""#), v(&["x", "\n"]));
}

#[test]
fn tmux_rejected_escapes_stay_literal() {
    // tmux reports a parse error for these; psmux keeps the text.
    assert_eq!(parse_command_line_tmux(r#"x "\4" "\uZZ" "\12""#), v(&["x", r"\4", r"\uZZ", r"\12"]));
    assert_eq!(parse_command_line_tmux(r#"x "\ud800z""#), v(&["x", r"\ud800z"]));
}

#[test]
fn tmux_escapes_do_not_split_tokens() {
    assert_eq!(parse_command_line_tmux(r#"x "a\nb" c"#), v(&["x", "a\nb", "c"]));
    assert_eq!(parse_command_line_tmux(r#"x "" "\n""#), v(&["x", "", "\n"]));
}

// ------------------------------------- config / prompt / wire stay Windows safe

#[test]
fn default_parser_keeps_double_quoted_windows_paths() {
    assert_eq!(
        parse_command_line(r#"set -g default-shell "C:\Program Files\Git\bin\bash.exe""#),
        v(&["set", "-g", "default-shell", r"C:\Program Files\Git\bin\bash.exe"])
    );
    assert_eq!(parse_command_line(r#"x "C:\new\temp" "\u000a""#), v(&["x", r"C:\new\temp", r"\u000a"]));
    assert_eq!(parse_command_line(r#"x "C:\\Users\\x" "a\"b""#), v(&["x", r"C:\Users\x", "a\"b"]));
}

const PATHS: &[&str] = &[
    r"C:\Users\x",
    r"C:\Users\x\new",
    r"C:\new\temp\bin",
    r"\\server\share",
    r"\\server\share\",
    r"\\?\C:\long\path",
    r"C:\Program Files\Git\bin\bash.exe",
    r"D:\",
    r"C:\dir\it's",
    r#"C:\dir\a"b"#,
];

#[test]
fn windows_paths_unquoted_and_single_quoted_round_trip_both_parsers() {
    for p in PATHS {
        if !p.contains(' ') && !p.contains('\'') && !p.contains('"') {
            let line = format!("x {}", p);
            assert_eq!(parse_command_line(&line), v(&["x", p]), "unquoted {}", p);
            assert_eq!(parse_command_line_tmux(&line), v(&["x", p]), "tmux unquoted {}", p);
        }
        if !p.contains('\'') {
            let line = format!("x '{}'", p);
            assert_eq!(parse_command_line(&line), v(&["x", p]), "single {}", p);
            assert_eq!(parse_command_line_tmux(&line), v(&["x", p]), "tmux single {}", p);
        }
    }
}

#[test]
fn windows_paths_round_trip_the_cli_wire() {
    // The CLI flattens argv with quote_arg_if_needed and the server splits the
    // line with parse_command_line: every value must come back byte exact.
    let extra = ["it's", "a\\\"b", "a\\'b", "x\\ny", "'a'\\''b'", "trail\\"];
    for p in PATHS.iter().copied().chain(extra.iter().copied()) {
        let line = format!("set -g @v {}", crate::util::quote_arg_if_needed(p));
        assert_eq!(parse_command_line(&line), v(&["set", "-g", "@v", p]), "wire {:?} as {}", p, line);
    }
}

#[test]
fn splitters_agree_on_escaped_quotes() {
    let parts = split_chained_commands_pub(r#"set -g @a 'a'\''b' ; set -g @b "x ; y""#);
    assert_eq!(parts, v(&[r#"set -g @a 'a'\''b'"#, r#"set -g @b "x ; y""#]));
}

// ------------------------------------------------------- config file set-option

#[test]
fn config_set_value_joins_adjacent_parts() {
    assert_eq!(extract_option_value(r#"'a'\''b'"#), "a'b");
    assert_eq!(extract_option_value(r#"'it'"'"'s'"#), "it's");
    // Two separate arguments keep the old fallback.
    assert_eq!(extract_option_value(r#""a" "b""#), r#"a" "b"#);
}

#[test]
fn config_file_lines_end_to_end() {
    let mut app = AppState::new("test_session".to_string());
    for line in [
        r#"set -g @o1 'a'\''b'"#,
        r#"set -g @o2 "C:\new\temp\bin""#,
        r"set -g @o3 'C:\Users\x\new'",
        r"set -g @o4 C:\Users\x\new",
        r"set -g @o5 \\server\share",
        r#"set -g @o6 "C:\\Users\\x""#,
        r#"set -g @o7 "C:\Program Files\Git\bin\bash.exe""#,
    ] {
        parse_config_line(&mut app, line);
    }
    let get = |k: &str| app.user_options.get(k).cloned().unwrap_or_default();
    assert_eq!(get("@o1"), "a'b");
    assert_eq!(get("@o2"), r"C:\new\temp\bin");
    assert_eq!(get("@o3"), r"C:\Users\x\new");
    assert_eq!(get("@o4"), r"C:\Users\x\new");
    assert_eq!(get("@o5"), r"\\server\share");
    assert_eq!(get("@o6"), r"C:\Users\x");
    assert_eq!(get("@o7"), r"C:\Program Files\Git\bin\bash.exe");
}
