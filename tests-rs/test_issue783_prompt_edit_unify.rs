//! Issue #783: the client's command prompt goes through the shared prompt
//! editor.
//!
//! psmux had four prompt editors that shared no code: the client's command
//! prompt overlay, the server side `Mode::CommandPrompt`, the copy mode search
//! prompt, and the copy mode command prompt. The third one already used
//! `copy_mode::prompt_edit`, a tmux shaped editor, and so did the client's
//! rename overlay. The command prompt did not, which showed up three ways
//! (measured and reported in discussion #782):
//!
//! * The keys the editor has were missing: `C-b`, `C-f`, `C-d`, `C-h`, `C-p`,
//!   `C-n`, `C-c` and `C-g` did nothing at the command prompt.
//! * Worse than nothing, in fact: a key the overlay had no arm for fell
//!   through to the client's pane key handling, so `C-l` cleared the shell's
//!   screen behind the prompt and `C-p` recalled the shell's own history.
//!   tmux's prompt keeps every key it is given (`status.c:1704`).
//! * `C-u` deleted from the cursor to the start of the line, where tmux
//!   deletes the whole line (`status.c:1548`), which is also what the manual's
//!   "Delete entire command" says and what `prompt_edit` already did.
//!
//! These tests pin the prompt's key handling, not the editor itself, which
//! `test_disc694_copy_mode_vi_parity.rs` already covers.
//!
//! Registered from src/client.rs.

use super::*;

use crate::client::{command_prompt_key, PromptKeyAction};

// --------------------------------------------------------------------------
// The client's command prompt overlay
// --------------------------------------------------------------------------

/// Press one key at the client's prompt. `back` counts characters from the end
/// of the line, as the shared editor does.
fn ckey(buf: &mut String, back: &mut usize, code: KeyCode, mods: KeyModifiers) -> PromptKeyAction {
    command_prompt_key(buf, back, &mut None, "", code, mods, || None)
}

fn ctrl(buf: &mut String, back: &mut usize, c: char) -> PromptKeyAction {
    ckey(buf, back, KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn named(buf: &mut String, back: &mut usize, code: KeyCode) -> PromptKeyAction {
    ckey(buf, back, code, KeyModifiers::NONE)
}

/// Type text at the cursor the way the overlay's text arm does.
fn ctype(buf: &mut String, back: usize, text: &str) {
    crate::copy_mode::prompt_insert(buf, back, text);
}

#[test]
fn client_prompt_c_b_and_c_f_move_the_cursor() {
    let mut buf = "abc".to_string();
    let mut back = 0;
    assert_eq!(ctrl(&mut buf, &mut back, 'b'), PromptKeyAction::Edited);
    assert_eq!(buf, "abc", "C-b moves the cursor, it does not type a b");
    assert_eq!(back, 1);
    ctype(&mut buf, back, "X");
    assert_eq!(buf, "abXc");

    assert_eq!(ctrl(&mut buf, &mut back, 'f'), PromptKeyAction::Edited);
    assert_eq!(back, 0);
    ctype(&mut buf, back, "Y");
    assert_eq!(buf, "abXcY");
}

#[test]
fn client_prompt_c_a_and_c_e_jump_to_the_ends() {
    let mut buf = "one two".to_string();
    let mut back = 0;
    ctrl(&mut buf, &mut back, 'a');
    assert_eq!(back, 7, "C-a parks the cursor before the first character");
    ctype(&mut buf, back, ": ");
    assert_eq!(buf, ": one two");

    ctrl(&mut buf, &mut back, 'e');
    assert_eq!(back, 0);
}

#[test]
fn client_prompt_c_d_and_c_h_delete_either_side_of_the_cursor() {
    let mut buf = "abcd".to_string();
    let mut back = 0;
    ctrl(&mut buf, &mut back, 'b');
    ctrl(&mut buf, &mut back, 'b');
    assert_eq!(back, 2, "cursor on the c");
    ctrl(&mut buf, &mut back, 'd');
    assert_eq!(buf, "abd", "C-d takes the character AT the cursor");
    ctrl(&mut buf, &mut back, 'h');
    assert_eq!(buf, "ad", "C-h takes the one before it");
}

#[test]
fn client_prompt_c_u_clears_the_whole_line() {
    // tmux sets the buffer size and the index to zero (`status.c:1548`), which
    // is the manual's "Delete entire command". The overlay used to drain only
    // from the start of the line to the cursor, so `one two` with the cursor
    // two characters from the end was left as `wo`.
    let mut buf = "one two".to_string();
    let mut back = 2;
    assert_eq!(ctrl(&mut buf, &mut back, 'u'), PromptKeyAction::Edited);
    assert_eq!(buf, "");
    assert_eq!(back, 0);
}

#[test]
fn client_prompt_c_k_cuts_to_the_end() {
    let mut buf = "one two".to_string();
    let mut back = 4;
    ctrl(&mut buf, &mut back, 'k');
    assert_eq!(buf, "one");
    assert_eq!(back, 0);
}

#[test]
fn client_prompt_c_w_cuts_the_word_before_the_cursor() {
    let mut buf = "one two".to_string();
    let mut back = 0;
    ctrl(&mut buf, &mut back, 'w');
    assert_eq!(buf, "one ");
}

#[test]
fn client_prompt_c_c_c_g_and_c_bracket_cancel() {
    // tmux's prompt ends on Escape, C-c and C-g (`status.c:1671` to `:1674`),
    // and C-[ IS Escape.
    for c in ['c', 'g', '['] {
        let mut buf = "half typed".to_string();
        let mut back = 0;
        assert_eq!(
            ctrl(&mut buf, &mut back, c),
            PromptKeyAction::Close,
            "C-{} closes the prompt",
            c
        );
        assert_eq!(buf, "half typed", "cancelling does not edit the line");
    }
}

#[test]
fn client_prompt_c_p_and_c_n_ask_for_the_history() {
    let mut buf = "typed".to_string();
    let mut back = 0;
    assert_eq!(ctrl(&mut buf, &mut back, 'p'), PromptKeyAction::HistoryUp);
    assert_eq!(ctrl(&mut buf, &mut back, 'n'), PromptKeyAction::HistoryDown);
    assert_eq!(buf, "typed", "the caller replaces the line, not the editor");
    // The named keys are the same two functions.
    assert_eq!(named(&mut buf, &mut back, KeyCode::Up), PromptKeyAction::HistoryUp);
    assert_eq!(
        named(&mut buf, &mut back, KeyCode::Down),
        PromptKeyAction::HistoryDown
    );
}

#[test]
fn client_prompt_named_keys_reach_the_same_editor() {
    let mut buf = "abc".to_string();
    let mut back = 0;
    named(&mut buf, &mut back, KeyCode::Left);
    assert_eq!(back, 1);
    named(&mut buf, &mut back, KeyCode::Right);
    assert_eq!(back, 0);
    named(&mut buf, &mut back, KeyCode::Home);
    assert_eq!(back, 3);
    named(&mut buf, &mut back, KeyCode::Delete);
    assert_eq!(buf, "bc", "Delete takes the character at the cursor");
    named(&mut buf, &mut back, KeyCode::End);
    assert_eq!(back, 0);
    named(&mut buf, &mut back, KeyCode::Backspace);
    assert_eq!(buf, "b");
}

#[test]
fn client_prompt_keeps_a_key_it_has_no_use_for() {
    // The point of the change: these used to fall through to the client's pane
    // key handling. C-l cleared the shell's screen behind the prompt, C-p
    // recalled the shell's own history, and Tab, PageUp and the F keys went to
    // the pane as well. None of them edits the line, and none of them closes
    // the prompt, so the caller does nothing and the key stops here.
    for (code, mods) in [
        (KeyCode::Char('l'), KeyModifiers::CONTROL),
        (KeyCode::Char('x'), KeyModifiers::CONTROL),
        (KeyCode::Char('f'), KeyModifiers::ALT),
        (KeyCode::Tab, KeyModifiers::NONE),
        (KeyCode::PageUp, KeyModifiers::NONE),
        (KeyCode::F(5), KeyModifiers::NONE),
        (KeyCode::Insert, KeyModifiers::NONE),
    ] {
        let mut buf = "ls".to_string();
        let mut back = 0;
        assert_eq!(
            ckey(&mut buf, &mut back, code, mods),
            PromptKeyAction::Edited,
            "{:?} with {:?} must stop at the prompt",
            code,
            mods
        );
        assert_eq!(buf, "ls", "{:?} must not change the line", code);
        assert_eq!(back, 0);
    }
}

#[test]
fn client_prompt_counts_characters_not_bytes() {
    // Three characters, nine bytes.
    let mut buf = "日本語".to_string();
    let mut back = 0;
    ctrl(&mut buf, &mut back, 'b');
    assert_eq!(back, 1);
    ctype(&mut buf, back, "x");
    assert_eq!(buf, "日本x語");
    ctrl(&mut buf, &mut back, 'h');
    assert_eq!(buf, "日本語");
    ctrl(&mut buf, &mut back, 'd');
    assert_eq!(buf, "日本");
}
