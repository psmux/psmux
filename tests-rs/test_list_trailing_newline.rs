// A listing reply ends in exactly one newline, as tmux prints it. The server
// wrote every listing as `{}\n`, but list-panes, list-buffers, list-keys,
// show-options and show-hooks build their rows with a newline after each
// row, so those printed an empty last line and `(list-panes).Count` was
// panes + 1. The CLI's own list-commands text had the same doubled newline.

use super::*;

fn reply(text: &str) -> String {
    format!("{}{}", text, listing_terminator(text))
}

#[test]
fn rows_ending_in_newline_get_no_second_one() {
    // The list-panes producer shape: one newline per row.
    let rows = "0: [120x14] [history 0/2000, 0 bytes] %1 (active)\n1: [120x15] [history 0/2000, 0 bytes] %2\n";
    let out = reply(rows);
    assert_eq!(out, rows);
    assert!(!out.ends_with("\n\n"));
    assert_eq!(out.lines().count(), 2);
}

#[test]
fn joined_rows_get_their_newline() {
    // The list-windows / -F producer shape: rows joined, no final newline.
    assert_eq!(reply("%1\n%2"), "%1\n%2\n");
    assert_eq!(reply("a"), "a\n");
}

#[test]
fn empty_reply_still_ends_in_a_newline() {
    // Registry readers (list-panes -a, list-sessions) read one line per
    // server and treat EOF without it as no reply (#809).
    assert_eq!(reply(""), "\n");
}

#[test]
fn a_real_blank_last_row_inside_the_text_is_kept() {
    // Only the terminator is normalised; rows themselves are untouched.
    assert_eq!(reply("a\n\nb"), "a\n\nb\n");
}

#[test]
fn list_commands_text_ends_in_exactly_one_newline() {
    let args = crate::list_commands::ListCommandsArgs::default();
    let t = crate::list_commands::render(&args, &mut |_, e| crate::list_commands::default_row(e)).unwrap();
    let t = t.as_str();
    assert!(t.ends_with('\n'));
    assert!(!t.ends_with("\n\n"));
    assert!(!t.ends_with("\r\n\r\n"));
    assert!(t.lines().last().map_or(false, |l| !l.trim().is_empty()));
}
