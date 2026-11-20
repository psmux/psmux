//! Issue #1065: the tree search matches a pane row by the command running
//! in it, as tmux's window_tree_search does with `osdep_get_name`. The
//! client asks each session's server for `#{pane_id} #{pane_current_command}`
//! and reads the reply with `parse_pane_commands`.

use super::*;

#[test]
fn reads_one_command_per_pane_id() {
    let m = parse_pane_commands("%0 pwsh\n%3 nvim\r\n%12 cargo\n");
    assert_eq!(m.get(&0).map(String::as_str), Some("pwsh"));
    assert_eq!(m.get(&3).map(String::as_str), Some("nvim"));
    assert_eq!(m.get(&12).map(String::as_str), Some("cargo"));
    assert_eq!(m.len(), 3);
}

#[test]
fn a_command_with_spaces_is_kept_whole() {
    let m = parse_pane_commands("%4 npm run dev\n");
    assert_eq!(m.get(&4).map(String::as_str), Some("npm run dev"));
}

#[test]
fn lines_that_are_not_pane_rows_are_skipped() {
    let m = parse_pane_commands("ERROR: no server\n\n%x foo\n7 bar\n%5 ok\n");
    assert_eq!(m.len(), 1);
    assert_eq!(m.get(&5).map(String::as_str), Some("ok"));
}

#[test]
fn the_prompt_line_reads_like_tmux() {
    let line = chooser_search_prompt_line("logs", Style::default());
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "(search) logs ");
}
