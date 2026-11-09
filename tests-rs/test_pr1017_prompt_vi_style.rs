// PR #1017: message-command-style. tmux draws the command prompt with
// message-style, and with message-command-style only while the prompt is in
// vi command mode (prompt_effective_style, prompt.c:438-450 at tag 3.8), which
// Escape enters when status-keys is vi (prompt.c:1265, :819). psmux's client
// prompt had no command mode: Escape closed it.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const NONE: KeyModifiers = KeyModifiers::NONE;

#[test]
fn prompt_style_follows_the_mode() {
    assert_eq!(prompt_style(false, Some("bg=green"), Some("bg=red")), "bg=green");
    assert_eq!(prompt_style(true, Some("bg=green"), Some("bg=red")), "bg=red");
    // tmux's defaults when the server sent nothing.
    assert_eq!(prompt_style(false, None, Some("")), "bg=yellow,fg=black");
    assert_eq!(prompt_style(true, Some("bg=green"), None), "bg=black,fg=yellow");
}

#[test]
fn escape_enters_command_mode_and_steps_back() {
    let mut cmd = false;
    assert_eq!(vi_prompt_key(&mut cmd, KeyCode::Char('x'), NONE), ViPromptKey::Key(KeyCode::Char('x'), NONE));
    assert!(!cmd);
    assert_eq!(vi_prompt_key(&mut cmd, KeyCode::Esc, NONE), ViPromptKey::Edit("left"));
    assert!(cmd);
    // A second Escape keeps the prompt open in command mode.
    assert_eq!(vi_prompt_key(&mut cmd, KeyCode::Esc, NONE), ViPromptKey::Done);
    assert!(cmd);
    let mut cmd = false;
    assert_eq!(vi_prompt_key(&mut cmd, KeyCode::Char('['), KeyModifiers::CONTROL), ViPromptKey::Edit("left"));
    assert!(cmd);
}

#[test]
fn insert_keys_leave_command_mode() {
    for (c, want) in [
        ('i', ViPromptKey::Done),
        ('a', ViPromptKey::Key(KeyCode::Right, NONE)),
        ('A', ViPromptKey::Key(KeyCode::End, NONE)),
        ('I', ViPromptKey::Key(KeyCode::Home, NONE)),
        ('C', ViPromptKey::Key(KeyCode::Char('k'), KeyModifiers::CONTROL)),
        ('s', ViPromptKey::Key(KeyCode::Delete, NONE)),
        ('S', ViPromptKey::Key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
    ] {
        let mut cmd = true;
        assert_eq!(vi_prompt_key(&mut cmd, KeyCode::Char(c), NONE), want, "{c}");
        assert!(!cmd, "{c} goes back to insert mode");
    }
}

#[test]
fn command_mode_keys_are_vi_commands() {
    let table = [
        (KeyCode::Char('h'), ViPromptKey::Key(KeyCode::Left, NONE)),
        (KeyCode::Char('l'), ViPromptKey::Key(KeyCode::Right, NONE)),
        (KeyCode::Char('k'), ViPromptKey::Key(KeyCode::Up, NONE)),
        (KeyCode::Char('j'), ViPromptKey::Key(KeyCode::Down, NONE)),
        (KeyCode::Char('0'), ViPromptKey::Key(KeyCode::Home, NONE)),
        (KeyCode::Char('$'), ViPromptKey::Key(KeyCode::End, NONE)),
        (KeyCode::Char('x'), ViPromptKey::Key(KeyCode::Delete, NONE)),
        (KeyCode::Char('X'), ViPromptKey::Key(KeyCode::Backspace, NONE)),
        (KeyCode::Char('D'), ViPromptKey::Key(KeyCode::Char('k'), KeyModifiers::CONTROL)),
        (KeyCode::Char('d'), ViPromptKey::Key(KeyCode::Char('u'), KeyModifiers::CONTROL)),
        (KeyCode::Char('p'), ViPromptKey::Key(KeyCode::Char('y'), KeyModifiers::CONTROL)),
        (KeyCode::Char('q'), ViPromptKey::Key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        (KeyCode::Char('b'), ViPromptKey::Key(KeyCode::Char('b'), KeyModifiers::ALT)),
        (KeyCode::Char('w'), ViPromptKey::Edit("vi-w")),
        (KeyCode::Char('W'), ViPromptKey::Edit("vi-bigw")),
        (KeyCode::Char('e'), ViPromptKey::Edit("vi-e")),
        (KeyCode::Char('E'), ViPromptKey::Edit("vi-bige")),
        (KeyCode::Char('B'), ViPromptKey::Edit("vi-bigb")),
        (KeyCode::Backspace, ViPromptKey::Key(KeyCode::Left, NONE)),
        (KeyCode::Enter, ViPromptKey::Key(KeyCode::Enter, NONE)),
        // A letter with no vi meaning types nothing.
        (KeyCode::Char('z'), ViPromptKey::Done),
        (KeyCode::Char('1'), ViPromptKey::Done),
    ];
    for (code, want) in table {
        let mut cmd = true;
        assert_eq!(vi_prompt_key(&mut cmd, code, NONE), want, "{code:?}");
        assert!(cmd, "{code:?} stays in command mode");
    }
    let mut cmd = true;
    assert_eq!(
        vi_prompt_key(&mut cmd, KeyCode::Char('c'), KeyModifiers::CONTROL),
        ViPromptKey::Key(KeyCode::Char('c'), KeyModifiers::CONTROL)
    );
}

/// Run vi word motions on `text` with the cursor at character `at`, and
/// return where the cursor ends.
fn motion(text: &str, at: usize, key: &str) -> usize {
    let mut buf = text.to_string();
    let len = text.chars().count();
    let mut back = len - at;
    let mut saved = None;
    crate::copy_mode::prompt_edit(&mut buf, &mut back, &mut saved, "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~", key, || None);
    assert_eq!(buf, text, "a motion does not edit");
    len - back
}

#[test]
fn vi_word_motions_match_tmux() {
    //         0123456789012345678
    let t = "set -g foo.bar  baz";
    // w: to the start of the next word, over the spaces.
    assert_eq!(motion(t, 0, "vi-w"), 4);
    assert_eq!(motion(t, 4, "vi-w"), 5); // `-` is a separator word
    assert_eq!(motion(t, 7, "vi-w"), 10); // foo then `.`
    assert_eq!(motion(t, 11, "vi-w"), 16); // bar, then two spaces
    // W: spaces only.
    assert_eq!(motion(t, 7, "vi-bigw"), 16);
    // e: the last character of the word.
    assert_eq!(motion(t, 0, "vi-e"), 2);
    assert_eq!(motion(t, 2, "vi-e"), 4);
    assert_eq!(motion(t, 6, "vi-e"), 9);
    assert_eq!(motion(t, 6, "vi-bige"), 13);
    // B: back to the start of the space separated word.
    assert_eq!(motion(t, 13, "vi-bigb"), 7);
    // At the end nothing moves.
    assert_eq!(motion(t, 19, "vi-w"), 19);
    assert_eq!(motion(t, 19, "vi-e"), 19);
}
