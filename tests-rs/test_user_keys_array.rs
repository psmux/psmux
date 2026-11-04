// user-keys as tmux's array option (options-table.c:734-743 at tag 3.8: a
// server option, OPTIONS_TABLE_IS_ARRAY, separator ",", no default), and the
// elements the clients get to match. The expected values are what tmux 3.8
// (`-f /dev/null`, WSL) printed for the same commands.

use super::*;
use crate::types::AppState;

const OPTION: &str = "user-keys";

fn app() -> AppState {
    AppState::new("test_user_keys_array".to_string())
}

fn items(a: &AppState) -> Vec<(u32, String)> {
    a.user_keys.numbered().map(|(n, s)| (n, s.to_string())).collect()
}

fn sent(a: &AppState) -> Option<Vec<(u32, String)>> {
    crate::server::helpers::expand_status_formats(a, "").client_render_options.user_keys
}

#[test]
fn a_new_server_has_no_element_and_sends_none() {
    let a = app();
    assert!(a.user_keys.is_empty());
    assert_eq!(array_option_show_lines(&a, OPTION, false), "user-keys\n");
    assert_eq!(get_option_value(&a, OPTION), "");
    assert_eq!(sent(&a), None, "nothing on the wire while the option is empty");
}

#[test]
fn set_split_append_and_unset_follow_tmux() {
    let mut a = app();
    apply_set_option(&mut a, OPTION, "a,b", false).unwrap();
    assert_eq!(items(&a), vec![(0, "a".to_string()), (1, "b".to_string())]);
    set_array_option(&mut a, OPTION, "c", true).unwrap();
    assert_eq!(items(&a).last(), Some(&(2, "c".to_string())));
    reset_option_to_default(&mut a, OPTION);
    assert!(a.user_keys.is_empty(), "-u puts back the empty default");
}

#[test]
fn the_config_file_keeps_the_escape_as_the_byte() {
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -s user-keys[0] \"\\e[5;30012~\"\nset -s user-keys[3] \"\\eOz\"\n",
    );
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert_eq!(items(&a), vec![(0, "\x1b[5;30012~".to_string()), (3, "\x1bOz".to_string())]);
    for name in ["user-keys", "user-keys[0]", "user-keys[3]"] {
        assert!(!a.user_options.contains_key(name), "{name} kept as text");
    }
}

#[test]
fn show_escapes_the_escape_as_tmux_does() {
    // tmux 3.8 printed exactly these two lines for the same two elements.
    let mut a = app();
    apply_set_option(&mut a, "user-keys[0]", "\x1b[5;30012~", false).unwrap();
    apply_set_option(&mut a, "user-keys[3]", "\x1bOz", false).unwrap();
    assert_eq!(
        array_option_show_lines(&a, OPTION, false),
        "user-keys[0] \"\\033[5;30012~\"\nuser-keys[3] \\033Oz\n"
    );
    assert_eq!(array_option_show_lines(&a, "user-keys[3]", false), "user-keys[3] \\033Oz\n");
    // `-v` prints the bytes as they are, as tmux does.
    assert_eq!(array_option_show_lines(&a, OPTION, true), "\x1b[5;30012~\n\x1bOz\n");
    assert_eq!(get_option_value(&a, OPTION), "\x1b[5;30012~ \x1bOz");
}

#[test]
fn the_config_file_reads_the_escapes_as_tmux_quotes_them() {
    // What tmux 3.8 stored for each line (source-file, then show -sv): the
    // lexer reads a backslash unquoted and in double quotes, not in single
    // quotes (cmd-parse.y:1703-1707), and reads it once.
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        r#"set -s user-keys[0] "\e[90~"
set -s user-keys[1] "\\e[91~"
set -s user-keys[2] '\e[92~'
set -s user-keys[3] \e[93~
set -s user-keys[4] "C:\\Users\\x"
set -s user-keys[5] 'a\\b'
set -s user-keys[6] "\u00e9\033Oz\8"
"#,
    );
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    for (n, want) in [
        (0, "\x1b[90~"),
        (1, "\\e[91~"),
        (2, "\\e[92~"),
        (3, "\x1b[93~"),
        (4, "C:\\Users\\x"),
        (5, "a\\\\b"),
        // `\uXXXX`, octal, and any other `\c` is `c`, so `\8` is `8`.
        (6, "\u{e9}\x1bOz8"),
    ] {
        assert_eq!(a.user_keys.get_index(n), Some(want), "user-keys[{n}]");
    }
}

#[test]
fn a_bad_octal_escape_keeps_its_backslash() {
    // `\4` to `\7` are an error in tmux ("invalid octal escape", the whole
    // file is refused); psmux keeps the backslash rather than refuse it.
    let mut a = app();
    crate::config::parse_config_content(&mut a, "set -s user-keys[2] \"a\\4\"\n");
    assert_eq!(a.user_keys.get_index(2), Some("a\\4"));
}

#[test]
fn the_command_line_value_is_taken_as_it_is() {
    // tmux reads no escape in an argument from the shell: `tmux set -s
    // user-keys[3] '\eOz'` keeps the backslash. A real ESC (PowerShell's
    // "`e") is the sequence.
    let mut a = app();
    apply_set_option(&mut a, "user-keys[1]", "\\e[96~", false).unwrap();
    assert_eq!(a.user_keys.get_index(1), Some("\\e[96~"));
    // show doubles the backslash, as args_escape does for any value: tmux
    // 3.8 printed `user-keys[6] \\e[96~` and `user-keys[4] C:\\Users\\x`.
    assert_eq!(array_option_show_lines(&a, "user-keys[1]", false), "user-keys[1] \\\\e[96~\n");
    apply_set_option(&mut a, "user-keys[4]", "C:\\Users\\x", false).unwrap();
    assert_eq!(array_option_show_lines(&a, "user-keys[4]", false), "user-keys[4] C:\\\\Users\\\\x\n");
    apply_set_option(&mut a, "user-keys[2]", "\x1b[96~", false).unwrap();
    assert_eq!(a.user_keys.get_index(2), Some("\x1b[96~"));
}

#[test]
fn the_clients_get_the_numbered_elements_up_to_1000() {
    let mut a = app();
    apply_set_option(&mut a, "user-keys[0]", "\x1b[5;30012~", false).unwrap();
    apply_set_option(&mut a, "user-keys[1000]", "\x1b[98~", false).unwrap();
    apply_set_option(&mut a, "user-keys[1001]", "\x1b[97~", false).unwrap();
    apply_set_option(&mut a, "user-keys[mine]", "\x1b[96~", false).unwrap();
    apply_set_option(&mut a, "user-keys[5]", "", false).unwrap();
    // `\377` is a byte that is no UTF-8; psmux keeps the value as text, so it
    // is U+FFFD, which every such byte also arrives as on the VT route.
    crate::config::parse_config_content(&mut a, "set -s user-keys[7] \"\\033\\377\"\n");
    assert_eq!(a.user_keys.get_index(7), Some("\x1b\u{fffd}"));
    // tmux reads elements 0 to KEYC_NUSER by number (tty-keys.c:533-540): a
    // named element, one above 1000 and an empty one are no key, and psmux
    // leaves out the one it cannot match as tmux does (user_keys::matchable).
    assert_eq!(
        sent(&a),
        Some(vec![(0, "\x1b[5;30012~".to_string()), (1000, "\x1b[98~".to_string())])
    );
}

#[test]
fn it_is_a_server_option() {
    assert!(crate::server::option_catalog::server_option_names().contains(&OPTION));
    let def = crate::server::option_catalog::OPTION_CATALOG.iter().find(|d| d.name == OPTION).unwrap();
    assert_eq!(def.default, "");
}
