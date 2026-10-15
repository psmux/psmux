// Issue #830: `show-buffer` printed one line break more than the buffer holds.
//
// The server's `show-buffer` reply ends with a line break of its own. The CLI
// `save-buffer` already dropped it (#818), but `show-buffer` printed the reply
// as it came, so `abc` showed as `abc\n`, and `show-buffer > f` followed by
// `load-buffer f` grew the buffer by one line break. tmux writes the buffer
// exactly for both commands (cmd-save-buffer.c:108 and :115).
//
// Both commands now take the buffer from the reply with the same helper. The
// commands themselves are pinned end to end by
// tests/test_issue830_show_buffer_newline.ps1.

use super::buffer_from_show_reply;

#[test]
fn reply_line_break_is_dropped() {
    assert_eq!(buffer_from_show_reply("abc\n"), "abc");
}

#[test]
fn buffer_own_line_breaks_are_kept() {
    // Only the reply's own last line break goes, never the buffer's.
    assert_eq!(buffer_from_show_reply("x\ny\n\n"), "x\ny\n");
    assert_eq!(buffer_from_show_reply("p\r\nq\r\n\n"), "p\r\nq\r\n");
    assert_eq!(buffer_from_show_reply("\n\n"), "\n");
}

#[test]
fn empty_buffer_shows_nothing() {
    assert_eq!(buffer_from_show_reply("\n"), "");
    assert_eq!(buffer_from_show_reply(""), "");
}

#[test]
fn reply_without_line_break_is_left_alone() {
    assert_eq!(buffer_from_show_reply("abc"), "abc");
}
