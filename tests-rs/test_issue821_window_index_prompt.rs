// #821: prefix + `'` opens tmux's window index prompt.
//
// tmux binds the key to a plain prompt (key-bindings.c:364, tag 3.7c):
//
//     bind -N 'Prompt for window index to select' "'" {
//         command-prompt -T window-target -pindex { select-window -t ':%%' }
//     }
//
// psmux bound it to `select-window-index`, a name only the client knew, which
// opened a box of the client's own. The box took printable characters,
// Backspace, Enter and Escape and nothing else: every other key went on to the
// pane (`C-u` printed `^U` at the shell, `Up` recalled its history), the prefix
// and root bindings fired while it was open, and what was typed was not drawn
// until something else brought a frame.
//
// The key is tmux's command-prompt now, so the answer goes through the editor
// the command prompt uses. This file pins the path from the key to the command
// line: the default binding, the fallback the client opens before the first
// state sync, the flag parse and the substituted line. What the prompt editor
// does with each key is pinned by the command prompt's own tests, and the
// whole thing end to end by tests/test_issue821_window_index_prompt.ps1.

use super::*;

/// The default binding: tmux's command, in the quoting form psmux's parser
/// reads (tmux 3.7c prints its own as
/// `command-prompt -T window-target -p index { select-window -t ":%%" }`).
const TMUX_QUOTE: &str = "command-prompt -T window-target -p index \"select-window -t ':%%'\"";

fn default_for(key: &str) -> Option<&'static str> {
    crate::help::PREFIX_DEFAULTS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, cmd)| *cmd)
}

#[test]
fn prefix_quote_is_tmuxs_command_prompt() {
    assert_eq!(default_for("'"), Some(TMUX_QUOTE));
}

#[test]
fn the_fallback_opens_the_same_prompt_as_the_binding() {
    // The client opens the prompt from WINDOW_INDEX_PROMPT_ARGS when `'`
    // arrives before the first state sync, and for the old name. Both have to
    // be the binding, or the key would behave differently in the first moment
    // of a session.
    let binding = default_for("'").expect("'\\'' missing from PREFIX_DEFAULTS");
    assert_eq!(
        binding.strip_prefix("command-prompt ").expect("not a command-prompt"),
        crate::help::WINDOW_INDEX_PROMPT_ARGS,
    );
}

#[test]
fn the_prompt_is_headed_index_and_runs_select_window() {
    let spec = parse_command_prompt_args(crate::help::WINDOW_INDEX_PROMPT_ARGS);
    assert_eq!(spec.label.as_deref(), Some("index"));
    assert_eq!(spec.template.as_deref(), Some("select-window -t ':%%'"));
    assert_eq!(spec.initial, "");
    // A free text prompt: Enter answers it, not the first key.
    assert!(!spec.key && !spec.single && !spec.numeric && !spec.search);
}

#[test]
fn an_answer_becomes_a_select_window_line() {
    let tmpl = "select-window -t ':%%'";
    for (answer, line) in [
        ("2", "select-window -t ':2'"),
        ("12", "select-window -t ':12'"),
        // select-window resolves a name as well as an index.
        ("my win", "select-window -t ':my win'"),
        // tmux runs the template with an empty answer too, which is `:`, the
        // current window.
        ("", "select-window -t ':'"),
    ] {
        assert_eq!(
            crate::copy_prompt::final_command_tmux(Some(tmpl), &[answer.to_string()]),
            line,
            "answer {:?}",
            answer,
        );
    }
}

#[test]
fn the_quote_default_lands_in_the_prefix_table() {
    // populate_default_bindings drops a default whose command
    // parse_command_to_action does not recognise. Prove this one survives.
    let mut app = crate::types::AppState::new("t-quote".to_string());
    crate::config::populate_default_bindings(&mut app);
    let table = app.key_tables.get("prefix").expect("no prefix table");
    let bind = table
        .iter()
        .find(|b| {
            b.key
                == (
                    crossterm::event::KeyCode::Char('\''),
                    crossterm::event::KeyModifiers::NONE,
                )
        })
        .expect("'\\'' did not reach the prefix key table");
    match &bind.action {
        crate::types::Action::Command(cmd) => assert_eq!(cmd, TMUX_QUOTE),
        other => panic!(
            "'\\'' became {}, not a command string",
            crate::commands::format_action(other)
        ),
    }
    assert!(!bind.repeat, "'\\'' is not a repeat binding in tmux");
}
