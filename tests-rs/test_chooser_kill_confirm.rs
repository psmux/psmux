// The tree and session choosers killed on `x` with nothing asked, where tmux
// puts a question up first and kills only on `y`
// (`window-tree.c:1282-1310`, and the callback at `:1137`). These are the two
// pure pieces of that: which key counts as the answer, and what the question
// names.
use super::*;

#[test]
fn only_y_answers_the_question() {
    assert!(chooser_kill_confirmed(KeyCode::Char('y')));
    assert!(chooser_kill_confirmed(KeyCode::Char('Y')));
}

#[test]
fn every_other_key_cancels() {
    // tmux's prompt is PROMPT_SINGLE, so it ends on the first key and the
    // callback kills only for `y`. Enter is in this list on purpose: psmux's
    // own `confirm-before` box takes it as yes, but in a chooser Enter is the
    // key that activates a row, so taking it as yes here would turn a missed
    // keystroke into a kill.
    for code in [
        KeyCode::Char('n'),
        KeyCode::Char('N'),
        KeyCode::Char('x'),
        KeyCode::Char('q'),
        KeyCode::Char('1'),
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Char(' '),
    ] {
        assert!(!chooser_kill_confirmed(code), "{:?} answered yes", code);
    }
}

#[test]
fn the_question_names_the_window_index() {
    // The tree rows are built as `"  2: bash* (1 panes)"`, and tmux's own
    // question carries the index alone, "Kill window 2? ".
    assert_eq!(chooser_kill_target("  2: bash* (1 panes)"), "2");
    assert_eq!(chooser_kill_target("  10: pwsh (3 panes)"), "10");
}

#[test]
fn a_row_with_no_colon_is_named_whole() {
    assert_eq!(chooser_kill_target("  plain"), "plain");
    assert_eq!(chooser_kill_target(""), "");
}
