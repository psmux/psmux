// The passthrough client (`attach -f passthrough`): what it prints for a
// capture, how it reads `%output`, and how it splits typed keys.
use super::*;

fn facts(cursor_x: u16, cursor_y: u16, history: usize, height: usize) -> PaneFacts {
    PaneFacts { pane_id: 3, cursor_x, cursor_y, cursor_visible: true, alternate: false, history, height }
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn facts_parse_from_the_display_format() {
    assert_eq!(parse_facts("%3 5 2 1 0 120 30"), Some(PaneFacts {
        pane_id: 3, cursor_x: 5, cursor_y: 2, cursor_visible: true, alternate: false, history: 120, height: 30,
    }));
    assert_eq!(parse_facts("%3 0 0 0 1 0 24").map(|f| (f.cursor_visible, f.alternate)), Some((false, true)));
    assert_eq!(parse_facts("3 0 0 1 0 0 24"), None, "a pane id carries its %");
    assert_eq!(parse_facts("%3 0 0"), None);
}

#[test]
fn a_capture_puts_history_then_one_row_per_pane_line() {
    let s = render_capture(&lines(&["h1", "h2", "a", "b"]), &facts(0, 1, 2, 3), true, false);
    // History, then the screen rows, padded to the pane's height because
    // capture-pane leaves off trailing empty rows.
    assert!(s.contains("\x1b[2Kh1\x1b[0m\r\n\x1b[2Kh2\x1b[0m\r\n\x1b[2Ka\x1b[0m\r\n\x1b[2Kb\x1b[0m\r\n\x1b[2K\x1b[0m"), "{s:?}");
    // From the last row, one up to the cursor's row.
    assert!(s.ends_with("\r\x1b[1A\x1b[?25h"), "{s:?}");
}

#[test]
fn a_capture_without_history_keeps_only_the_screen() {
    let s = render_capture(&lines(&["old", "a", "b"]), &facts(4, 1, 1, 2), false, false);
    assert!(!s.contains("old"), "{s:?}");
    assert!(s.ends_with("\r\x1b[4C\x1b[?25h"), "cursor on the last row, column 4: {s:?}");
}

#[test]
fn an_alternate_screen_capture_is_drawn_on_the_alternate_screen() {
    let mut f = facts(0, 0, 50, 2);
    f.alternate = true;
    let s = render_capture(&lines(&["x", "y"]), &f, true, false);
    assert!(s.starts_with("\x1b[0m\x1b[?1049h\x1b[H\x1b[2Kx"), "{s:?}");
    let back = render_capture(&lines(&["x"]), &facts(0, 0, 0, 1), false, true);
    assert!(back.starts_with("\x1b[0m\x1b[?1049l\r\n"), "{back:?}");
}

#[test]
fn output_lines_give_pane_and_payload() {
    assert_eq!(parse_output("%output %12 hello world"), Some((12, "hello world")));
    assert_eq!(parse_output("%extended-output %4 25 : a : b"), Some((4, "a : b")));
    assert_eq!(parse_output("%window-add @1"), None);
}

#[test]
fn unescape_undoes_escape_output() {
    let raw = "a\\b\r\n\x1b[31mgr\u{fc}n\t";
    assert_eq!(unescape_output(&crate::control::escape_output(raw)), raw.as_bytes());
    assert_eq!(unescape_output("\\12"), b"\\12", "a short escape is literal");
}

#[test]
fn keys_after_the_prefix() {
    let p = Some(0x02);
    let mut after = false;
    assert_eq!(route_keys(b"ls\r", p, &mut after), (b"ls\r".to_vec(), KeyAction::None));
    assert_eq!(route_keys(b"a\x02d!", p, &mut after), (b"a".to_vec(), KeyAction::Detach));
    assert_eq!(route_keys(b"\x02\x02", p, &mut after), (vec![0x02], KeyAction::None));
    assert_eq!(route_keys(b"\x02c", p, &mut after), (vec![0x02, b'c'], KeyAction::None));
    // A prefix at the end of one read applies to the next.
    assert_eq!(route_keys(b"\x02", p, &mut after), (vec![], KeyAction::None));
    assert!(after);
    assert_eq!(route_keys(b"d", p, &mut after), (vec![], KeyAction::Detach));
    // Without a one byte prefix every byte goes to the pane.
    assert_eq!(route_keys(b"\x02d", None, &mut after), (b"\x02d".to_vec(), KeyAction::None));
}

#[test]
fn keys_go_as_hex_bytes() {
    assert_eq!(send_keys_command(b"\x1b[A"), "send-keys -H 1b 5b 41");
}

#[test]
fn a_block_closes_on_its_own_guard_line() {
    let begin = "%begin 1700000000 7 1";
    assert_eq!(closes_block("%end 1700000000 7 1", begin), Some(true));
    assert_eq!(closes_block("%error 1700000000 7 1", begin), Some(false));
    assert_eq!(closes_block("%end 1700000000 8 1", begin), None, "another command's guard");
    assert_eq!(closes_block("%end of the story", begin), None, "pane text");
    assert!(answers_client(begin));
    assert!(!answers_client("%begin 1700000000 1 0"), "the server's own reply to the implicit attach");
}

#[test]
fn passthrough_is_a_client_flag() {
    use crate::cli::{attach_client_flags, ClientFlags};
    assert!(attach_client_flags(["-f", "passthrough"]).passthrough);
    assert!(attach_client_flags(["-t", "main", "-fread-only,passthrough"]) == ClientFlags { read_only: true, passthrough: true });
    assert!(!attach_client_flags(["-f", "passthrough,!passthrough"]).passthrough);
    assert!(!attach_client_flags(["-t", "passthrough"]).passthrough);
}
