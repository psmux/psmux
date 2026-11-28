// Issue #1118: tmux's config parser turns `\\` into one backslash outside single
// quotes (cmd-parse.y), so `bind "C-\\"`, `bind C-\\`, `bind M-\\` and
// `set -g prefix C-\\` all name Ctrl+\ (or Alt+\). psmux read the key token as
// written and refused every one of them with "unknown key" ("bad key" for the
// prefix). A lone backslash (`C-\`, `'C-\'`) has to keep working: it is what
// psmux's own list-keys prints.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const CTRL_BS: (KeyCode, KeyModifiers) = (KeyCode::Char('\\'), KeyModifiers::CONTROL);
const ALT_BS: (KeyCode, KeyModifiers) = (KeyCode::Char('\\'), KeyModifiers::ALT);

#[test]
fn every_tmux_spelling_of_ctrl_backslash() {
    for name in [r"C-\", r"C-\\", r#""C-\\""#, r"'C-\'", r"^\", r"^\\"] {
        assert_eq!(parse_key_name(name), Some(CTRL_BS), "{name}");
    }
}

#[test]
fn alt_backslash_and_plain_backslash() {
    for name in [r"M-\", r"M-\\", r#""M-\\""#, r"'M-\'"] {
        assert_eq!(parse_key_name(name), Some(ALT_BS), "{name}");
    }
    for name in [r"\", r"\\", r#""\\""#, r"'\'"] {
        assert_eq!(parse_key_name(name), Some((KeyCode::Char('\\'), KeyModifiers::NONE)), "{name}");
    }
}

#[test]
fn single_quotes_keep_both_backslashes() {
    // tmux: no escapes inside single quotes, so this is two characters.
    assert_eq!(parse_key_name(r"'C-\\'"), None);
}

#[test]
fn escaped_punctuation_is_the_character() {
    assert_eq!(parse_key_name(r#"\""#), Some((KeyCode::Char('"'), KeyModifiers::NONE)));
    assert_eq!(parse_key_name(r"\;"), Some((KeyCode::Char(';'), KeyModifiers::NONE)));
    assert_eq!(parse_key_name(r"M-\;"), Some((KeyCode::Char(';'), KeyModifiers::ALT)));
}

#[test]
fn names_without_a_backslash_are_unchanged() {
    assert_eq!(parse_key_name("C-a"), Some((KeyCode::Char('a'), KeyModifiers::CONTROL)));
    assert_eq!(parse_key_name("'|'"), Some((KeyCode::Char('|'), KeyModifiers::NONE)));
    assert_eq!(parse_key_name("F5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
}

#[test]
fn config_lines_bind_and_set_prefix() {
    for line in [
        r#"bind -n "C-\\" set -g @a 1"#,
        r"bind -n C-\\ set -g @a 1",
        r"bind -n 'C-\' set -g @a 1",
        r"bind -n C-\ set -g @a 1",
    ] {
        let mut app = crate::types::AppState::new("i1118".to_string());
        parse_config_content(&mut app, line);
        assert!(app.config_warnings.is_empty(), "{line}: {:?}", app.config_warnings);
        let root = app.key_tables.get("root").expect("root table");
        assert!(root.iter().any(|b| b.key == normalize_key_for_binding(CTRL_BS)), "{line}");
    }
    for line in [r#"set -g prefix "C-\\""#, r"set -g prefix C-\\", r"set -g prefix 'C-\'"] {
        let mut app = crate::types::AppState::new("i1118".to_string());
        parse_config_content(&mut app, line);
        assert!(app.config_warnings.is_empty(), "{line}: {:?}", app.config_warnings);
        assert_eq!(app.prefix_key, Some(CTRL_BS), "{line}");
    }
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, r"unbind -n C-\\");
    assert!(app.config_warnings.is_empty(), "unbind: {:?}", app.config_warnings);
}

#[test]
fn escaped_backslash_at_line_end_is_not_a_continuation() {
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, "set -g prefix C-\\\\\nset -g @after yes\n");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(app.prefix_key, Some(CTRL_BS));
    assert_eq!(app.user_options.get("@after").map(String::as_str), Some("yes"));

    // One backslash still joins the next line, as in tmux.
    let mut app = crate::types::AppState::new("i1118".to_string());
    parse_config_content(&mut app, "set -g \\\n@joined yes\n");
    assert_eq!(app.user_options.get("@joined").map(String::as_str), Some("yes"));
}
