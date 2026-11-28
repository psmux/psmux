// Issue #1118: tmux's config parser turns `\\` into one backslash outside single
// quotes (cmd-parse.y), so `bind "C-\\"`, `bind C-\\`, `bind M-\\` and
// `set -g prefix C-\\` all name Ctrl+\ (or Alt+\). psmux read the key token as
// written and refused every one of them with "unknown key" ("bad key" for the
// prefix). A lone backslash (`C-\`, `'C-\'`) has to keep working: it is what
// psmux's own list-keys prints. The command line reads no escapes, as tmux
// takes an argv as it is.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const CTRL_BS: (KeyCode, KeyModifiers) = (KeyCode::Char('\\'), KeyModifiers::CONTROL);
const ALT_BS: (KeyCode, KeyModifiers) = (KeyCode::Char('\\'), KeyModifiers::ALT);

fn ch(c: char) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn every_tmux_spelling_of_ctrl_backslash() {
    for name in [r"C-\", r"C-\\", r#""C-\\""#, r"'C-\'", r"^\", r"^\\"] {
        assert_eq!(parse_config_key_name(name), Some(CTRL_BS), "{name}");
    }
}

#[test]
fn alt_backslash_and_plain_backslash() {
    for name in [r"M-\", r"M-\\", r#""M-\\""#, r"'M-\'"] {
        assert_eq!(parse_config_key_name(name), Some(ALT_BS), "{name}");
    }
    for name in [r"\", r"\\", r#""\\""#, r"'\'"] {
        assert_eq!(parse_config_key_name(name), Some(ch('\\')), "{name}");
    }
}

#[test]
fn single_quotes_keep_both_backslashes() {
    // tmux: no escapes inside single quotes, so these are two characters.
    assert_eq!(parse_config_key_name(r"'C-\\'"), None);
    assert_eq!(parse_config_key_name(r"'\;'"), None);
}

#[test]
fn escaped_punctuation_is_the_character() {
    for (name, c) in [(r#"\""#, '"'), (r"\;", ';'), (r"\#", '#'), (r"\~", '~'), (r"\$", '$')] {
        assert_eq!(parse_config_key_name(name), Some(ch(c)), "{name}");
    }
    assert_eq!(parse_config_key_name(r"M-\;"), Some((KeyCode::Char(';'), KeyModifiers::ALT)));
}

#[test]
fn names_without_a_backslash_are_unchanged() {
    assert_eq!(parse_config_key_name("C-a"), Some((KeyCode::Char('a'), KeyModifiers::CONTROL)));
    assert_eq!(parse_config_key_name("'|'"), Some(ch('|')));
    assert_eq!(parse_config_key_name("'\"'"), Some(ch('"')));
    assert_eq!(parse_config_key_name("|"), Some(ch('|')));
    assert_eq!(parse_config_key_name("F5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
}

#[test]
fn an_argv_key_reads_no_escapes() {
    // tmux takes `tmux bind 'C-\\' ...` from a shell as the four characters
    // and refuses it: "unknown key". parse_key_name is that route and kept
    // master's reading.
    assert_eq!(parse_key_name(r"C-\"), Some(CTRL_BS));
    assert_eq!(parse_key_name(r"\"), Some(ch('\\')));
    for name in [r"C-\\", r"\\", r"\;", r"\#"] {
        assert_eq!(parse_key_name(name), None, "{name}");
    }
}

fn table_key(line: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, line);
    assert!(app.config_warnings.is_empty(), "{line}: {:?}", app.config_warnings);
    app.key_tables.get("t").and_then(|t| t.first()).map(|b| b.key)
}

#[test]
fn config_bind_forms_match_tmux() {
    // What tmux's cmd-parse.y hands key_string_lookup_string for each form.
    for (line, want) in [
        (r"bind -T t \\ split-window -h", ch('\\')),
        (r"bind -T t '\' split-window -h", ch('\\')),
        (r#"bind -T t "\\" split-window -h"#, ch('\\')),
        (r"bind -T t \; split-window -h", ch(';')),
        (r"bind -T t ';' split-window -h", ch(';')),
        (r#"bind -T t \" split-window -h"#, ch('"')),
        (r#"bind -T t '"' split-window -h"#, ch('"')),
        (r"bind -T t | split-window -h", ch('|')),
        (r"bind -T t \# split-window -h", ch('#')),
        (r"bind -T t \~ split-window -h", ch('~')),
        (r"bind -T t \$ split-window -h", ch('$')),
        (r"bind -T t M-\\ split-window -h", ALT_BS),
        (r"bind -T t C-\\ split-window -h", CTRL_BS),
        (r#"bind -T t "C-\\" split-window -h"#, CTRL_BS),
        (r"bind -T t 'C-\' split-window -h", CTRL_BS),
        // psmux's own list-keys form, kept from master.
        (r"bind -T t C-\ split-window -h", CTRL_BS),
    ] {
        assert_eq!(table_key(line), Some(normalize_key_for_binding(want)), "{line}");
    }
}

#[test]
fn config_lines_bind_and_set_prefix() {
    for line in [r#"set -g prefix "C-\\""#, r"set -g prefix C-\\", r"set -g prefix 'C-\'"] {
        let mut app = crate::types::AppState::new("i1118".to_string());
        parse_config_content(&mut app, line);
        assert!(app.config_warnings.is_empty(), "{line}: {:?}", app.config_warnings);
        assert_eq!(app.prefix_key, Some(CTRL_BS), "{line}");
    }
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, r"set -g prefix2 C-\\");
    assert!(app.config_warnings.is_empty(), "prefix2: {:?}", app.config_warnings);
    assert_eq!(app.prefix2_key, Some(CTRL_BS));
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, r"unbind -n C-\\");
    assert!(app.config_warnings.is_empty(), "unbind: {:?}", app.config_warnings);
}

fn opt(content: &str, name: &str) -> Option<String> {
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, content);
    assert!(app.config_warnings.is_empty(), "{content}: {:?}", app.config_warnings);
    app.user_options.get(name).cloned()
}

#[test]
fn escaped_backslash_at_line_end_is_not_a_continuation() {
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, "set -g prefix C-\\\\\nset -g @after yes\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(app.prefix_key, Some(CTRL_BS));
    assert_eq!(app.user_options.get("@after").map(String::as_str), Some("yes"));

    // `\\` ending a bound command: the binding keeps it and the next line is
    // its own command (master joined them).
    let content = "bind -T t x send-keys -l \\\\\nset -g @next yes\n";
    assert_eq!(opt(content, "@next").as_deref(), Some("yes"));
    // A bare option value ending in `\\` keeps it as written, as other bare
    // values do, and the next line is its own.
    let content = "set -g @v ab\\\\\nset -g @next yes\n";
    assert_eq!(opt(content, "@v").as_deref(), Some("ab\\\\"));
    assert_eq!(opt(content, "@next").as_deref(), Some("yes"));
}

#[test]
fn one_trailing_backslash_still_continues() {
    assert_eq!(opt("set -g @joined \\\nyes\n", "@joined").as_deref(), Some("yes"));
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, "bind -T t y \\\n  split-window -h\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert!(app.key_tables.get("t").is_some_and(|t| t.iter().any(|b| b.key == ch('y'))));
}

#[test]
fn values_with_backslashes_are_unchanged() {
    // The same as master: quoted values ending in a backslash and Windows
    // paths.
    assert_eq!(opt(r"set -g @p 'C:\tmp\'", "@p").as_deref(), Some(r"C:\tmp\"));
    assert_eq!(opt(r#"set -g @p "C:\\tmp\\""#, "@p").as_deref(), Some(r"C:\tmp\"));
    assert_eq!(opt(r"set -g @p C:\tmp\x", "@p").as_deref(), Some(r"C:\tmp\x"));
    assert_eq!(opt(r#"set -g @p "a\\b""#, "@p").as_deref(), Some(r"a\b"));
    assert_eq!(opt(r"set -g @p 'a\b'", "@p").as_deref(), Some(r"a\b"));
}
