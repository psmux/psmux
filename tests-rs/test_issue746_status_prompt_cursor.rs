// Issue #746: a copy-mode prompt on the status line drew no text cursor.
//
// The position was known on the server as `copy_prompt_back`, characters back
// from the end of the input, and the client needs a byte offset into the
// prompt text, because `util::prompt_window` takes one. These check the
// conversion between the two, which is where a wide or multi byte character
// turns an offset into the wrong column, and the label length that keeps the
// label from scrolling away with the input.

use super::{copy_prompt_cursor, copy_prompt_text};
use crate::types::Mode;

fn search(input: &str) -> Mode {
    Mode::CopySearch { input: input.to_string(), forward: true }
}

/// The column the client draws at: the label does not scroll, only the input
/// after it, which is what tmux does (status.c:922 to :948).
fn drawn_column(mode: &Mode, back: usize, cols: usize) -> (String, usize) {
    let text = copy_prompt_text(mode).unwrap();
    let (label_len, at) = copy_prompt_cursor(mode, back).unwrap();
    let (label, input) = text.split_at(label_len);
    let label_cols = unicode_width::UnicodeWidthStr::width(label);
    let (shown, col) =
        crate::util::prompt_window(input, at - label_len, cols.saturating_sub(label_cols));
    (format!("{label}{shown}"), label_cols + col)
}

#[test]
fn an_empty_prompt_puts_the_cursor_after_the_label() {
    let m = search("");
    assert_eq!(copy_prompt_text(&m).as_deref(), Some("(search down) "));
    assert_eq!(copy_prompt_cursor(&m, 0), Some((14, 14)));
}

#[test]
fn with_nothing_moved_the_cursor_is_at_the_end() {
    let m = search("abc");
    assert_eq!(copy_prompt_cursor(&m, 0), Some((14, 17)), "14 for the label, 3 for abc");
}

#[test]
fn back_counts_characters_and_the_answer_counts_bytes() {
    // Four U+3042: 4 characters, 12 bytes, 8 display columns. One character
    // back from the end is three BYTES back, which is the whole point of
    // converting here instead of sending the character count.
    let m = search("\u{3042}\u{3042}\u{3042}\u{3042}");
    assert_eq!(
        copy_prompt_text(&m).as_deref(),
        Some("(search down) \u{3042}\u{3042}\u{3042}\u{3042}")
    );
    assert_eq!(copy_prompt_cursor(&m, 0), Some((14, 26)), "14 + 12");
    assert_eq!(copy_prompt_cursor(&m, 1), Some((14, 23)));
    assert_eq!(copy_prompt_cursor(&m, 4), Some((14, 14)), "all the way back is the input's start");
}

#[test]
fn a_four_byte_character_is_one_step() {
    let m = search("a\u{1f60a}b");
    assert_eq!(copy_prompt_cursor(&m, 0), Some((14, 20)), "14 + 1 + 4 + 1");
    assert_eq!(copy_prompt_cursor(&m, 1), Some((14, 19)), "before b");
    assert_eq!(copy_prompt_cursor(&m, 2), Some((14, 15)), "before the emoji, four bytes wide");
}

#[test]
fn back_is_clamped_to_the_input() {
    // A history recall replaces the input under the cursor, and a prompt that
    // was answered and reopened can carry a stale count. Neither may walk the
    // offset into the label.
    let m = search("ab");
    assert_eq!(copy_prompt_cursor(&m, 99), Some((14, 14)));
}

#[test]
fn the_goto_line_prompt_has_its_own_label() {
    let m = Mode::CopyGoto { input: "12".to_string() };
    assert_eq!(copy_prompt_text(&m).as_deref(), Some("(goto line) 12"));
    assert_eq!(copy_prompt_cursor(&m, 0), Some((12, 14)), "12 for the label, 2 for 12");
    assert_eq!(copy_prompt_cursor(&m, 2), Some((12, 12)));
}

#[test]
fn a_mode_with_no_prompt_has_no_cursor() {
    assert_eq!(copy_prompt_cursor(&Mode::CopyMode, 0), None);
    assert_eq!(copy_prompt_cursor(&Mode::Passthrough, 3), None);
}

#[test]
fn the_copy_mode_command_prompt_follows_back() {
    // `copy_prompt::feed` edits through the shared editor and keeps its
    // cursor in `copy_prompt_back`, like the search and goto line prompts,
    // so the cursor is drawn where the editor left it (#789).
    let p = crate::copy_prompt::CopyCommandPrompt {
        prompts: vec![("(goto line) ".to_string(), String::new())],
        current: 0,
        answers: Vec::new(),
        input: "7".to_string(),
        template: None,
        single: false,
        numeric: false,
        key: false,
        bspace_exit: false,
        search_history: false,
    };
    let m = Mode::CopyCommandPrompt(Box::new(p));
    assert_eq!(copy_prompt_text(&m).as_deref(), Some("(goto line) 7"));
    assert_eq!(copy_prompt_cursor(&m, 0), Some((12, 13)));
    assert_eq!(copy_prompt_cursor(&m, 1), Some((12, 12)), "one back is before the 7");
    assert_eq!(copy_prompt_cursor(&m, 5), Some((12, 12)), "a stale count is clamped to the input");
}

#[test]
fn the_drawn_column_is_a_display_width() {
    let m = search("\u{3042}\u{3042}\u{3042}\u{3042}");
    let (line, col) = drawn_column(&m, 0, 120);
    assert_eq!(line, "(search down) \u{3042}\u{3042}\u{3042}\u{3042}");
    assert_eq!(col, 22, "14 label columns plus 8 for four wide characters, not 26 bytes");
    let (_, col) = drawn_column(&m, 1, 120);
    assert_eq!(col, 20, "one character back is two columns back");
}

#[test]
fn a_long_search_term_scrolls_and_keeps_its_label() {
    let long: String = (0..40).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    let m = search(&long);
    let (line, col) = drawn_column(&m, 0, 30);
    assert!(
        line.starts_with("(search down) "),
        "the label stays while the input scrolls: {line:?}"
    );
    assert!(col < 30, "the cursor stays on the line, at {col}");
    assert!(
        line.ends_with(&long[long.len() - 8..]),
        "the end of the term is what is shown: {line:?}"
    );
    assert_eq!(
        unicode_width::UnicodeWidthStr::width(line.as_str()),
        30 - 1,
        "the window leaves the cursor's own cell on the line"
    );
}
