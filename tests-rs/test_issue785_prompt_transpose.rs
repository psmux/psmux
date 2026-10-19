// Issue #785: C-t at the prompts transposes the two characters at the cursor,
// the way tmux's prompt does (status.c:1649 to :1661, tag 3.7c).
//
// The expected lines are the ones tmux 3.7c showed at its own command prompt,
// measured through a nested client with status-keys emacs:
//
//   abc, at the end, C-t           :acb
//   abc, at the end, C-t C-t       :abc
//   abcd, Left Left, C-t, X        :acbXd
//   abc, Home, C-t, X              :Xabc
//   ab, Home Right, C-t, X         :baX
//   a, C-t, X                      :aX
//   empty, C-t, X                  :X
//   日本語, at the end, C-t        :日語本
//   search prompt, abc, C-t        (search up) acb

use super::*;
use super::test_copy_mode_snapshot::app_with_pane;

/// Type `text`, press the named keys in order, then type `X` so the cursor
/// shows in the result, as the measurement did.
fn edit(text: &str, keys: &[&str]) -> String {
    let mut input = text.to_string();
    let mut back = 0usize;
    for k in keys {
        prompt_edit(&mut input, &mut back, &mut None, "", k, || None);
    }
    prompt_insert(&mut input, back, "X");
    input
}

#[test]
fn at_the_end_the_last_two_characters_change_places() {
    assert_eq!(edit("abc", &["C-t"]), "acbX");
}

#[test]
fn a_second_c_t_at_the_end_puts_them_back() {
    assert_eq!(edit("abc", &["C-t", "C-t"]), "abcX");
}

#[test]
fn inside_the_line_the_characters_either_side_of_the_cursor_change_places() {
    // The cursor sits between b and c; it ends after the pair, before d.
    assert_eq!(edit("abcd", &["Left", "Left", "C-t"]), "acbXd");
}

#[test]
fn after_the_first_character_the_first_two_change_places() {
    assert_eq!(edit("ab", &["Home", "Right", "C-t"]), "baX");
}

#[test]
fn at_the_start_nothing_changes_and_the_cursor_stays() {
    assert_eq!(edit("abc", &["Home", "C-t"]), "Xabc");
}

#[test]
fn one_character_or_none_is_left_alone() {
    assert_eq!(edit("a", &["C-t"]), "aX");
    assert_eq!(edit("", &["C-t"]), "X");
}

#[test]
fn characters_are_swapped_whole_not_by_byte() {
    assert_eq!(edit("日本語", &["C-t"]), "日語本X");
    assert_eq!(edit("日本語", &["Left", "C-t"]), "日語本X");
}

#[test]
fn c_t_is_an_editing_key_not_one_the_editor_ignores() {
    let mut input = "ab".to_string();
    let mut back = 0usize;
    assert_eq!(prompt_edit(&mut input, &mut back, &mut None, "", "C-t", || None), PromptEdit::Edited);
    // Even where it changes nothing, so a caller never treats it as unknown.
    let mut input = "a".to_string();
    assert_eq!(prompt_edit(&mut input, &mut back, &mut None, "", "c-t", || None), PromptEdit::Edited);
}

// ── the prompts that reach it ───────────────────────────────────────────────

#[test]
fn the_client_command_prompt_transposes_on_ctrl_t() {
    use crate::client::{command_prompt_key, PromptKeyAction};
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut buf = "abc".to_string();
    let mut back = 0usize;
    let act = command_prompt_key(&mut buf, &mut back, &mut None, "", KeyCode::Char('t'), KeyModifiers::CONTROL, || None);
    assert_eq!(act, PromptKeyAction::Edited);
    assert_eq!(buf, "acb");
    assert_eq!(back, 0);
}

#[test]
fn the_rename_overlay_transposes_on_ctrl_t() {
    let mut buf = "abcd".to_string();
    let mut back = 2usize;
    let closes = crate::client::rename_overlay_ctrl_key(&mut buf, &mut back, &mut None, "", 't', || None);
    assert!(!closes, "C-t edits the name, it does not close the overlay");
    assert_eq!(buf, "acbd");
    assert_eq!(back, 1);
}

#[test]
fn the_copy_mode_search_prompt_transposes_on_ctrl_t() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_text_to_active(&mut app, "abc").unwrap();
    crate::input::send_key_to_active(&mut app, "C-t").unwrap();
    match app.mode {
        Mode::CopySearch { ref input, .. } => assert_eq!(input, "acb"),
        _ => panic!("C-t closed the search prompt"),
    }
    assert_eq!(app.status_message.as_ref().map(|m| m.0.as_str()), Some("(search down) acb"));
}
