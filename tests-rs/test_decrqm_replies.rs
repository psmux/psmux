// DECRQM replies (`CSI ? Ps $ p`): each mode as it stood where the query was
// met, as tmux answers it (INPUT_CSI_QUERY_PRIVATE, input.c:1651-1722 at tag
// 3.8). The parser carries the value with the query; psmux fills in only
// 2026, from its SyncGate, and 12 when the program set neither a cursor
// shape nor a blink, from the cursor-style option (input.c:1666-1680).
use super::test_decrqss_replies::CursorEnv;
use super::*;
use crate::types::HostColors;

fn texts(bytes: &[u8]) -> Vec<String> {
    let queries = scan_device_queries(bytes);
    pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), &mut false)
        .into_iter()
        .map(|r| r.text)
        .collect()
}

#[test]
fn a_mode_reset_in_the_same_write_is_answered_as_it_stood() {
    // The reply used to be built from the screen after the whole batch.
    assert_eq!(texts(b"\x1b[?1h\x1b[?1$p\x1b[?1l"), vec!["\x1b[?1;1$y"]);
    assert_eq!(texts(b"\x1b[?25l\x1b[?25$p\x1b[?25h"), vec!["\x1b[?25;2$y"]);
    assert_eq!(texts(b"\x1b[?1049h\x1b[?1049$p\x1b[?1049l"), vec!["\x1b[?1049;1$y"]);
    assert_eq!(texts(b"\x1b[4h\x1b[4$p\x1b[4l"), vec!["\x1b[4;1$y"]);
    assert_eq!(texts(b"\x1b[?1$p\x1b[?1h\x1b[?1$p"), vec!["\x1b[?1;2$y", "\x1b[?1;1$y"]);
}

#[test]
fn origin_and_focus_modes() {
    assert_eq!(texts(b"\x1b[?6$p\x1b[?6h\x1b[?6$p"), vec!["\x1b[?6;2$y", "\x1b[?6;1$y"]);
    assert_eq!(texts(b"\x1b[?1004$p\x1b[?1004h\x1b[?1004$p"), vec!["\x1b[?1004;2$y", "\x1b[?1004;1$y"]);
    // OpenConsole's own `?1004h` right after the program's `?1004l`.
    assert_eq!(texts(b"\x1b[?1004h\x1b[?1004l\x1b[?1004h\x1b[?1004$p"), vec!["\x1b[?1004;2$y"]);
}

#[test]
fn theme_updates_stay_unknown() {
    assert_eq!(texts(b"\x1b[?2031h\x1b[?2031$p"), vec!["\x1b[?2031;0$y"]);
}

#[test]
fn the_cursor_blink_follows_the_program() {
    let _env = CursorEnv::take();
    std::env::set_var("PSMUX_CURSOR_STYLE", "blinking-bar");
    assert_eq!(texts(b"\x1b[?12l\x1b[?12$p"), vec!["\x1b[?12;2$y"]);
    assert_eq!(texts(b"\x1b[2 q\x1b[?12$p"), vec!["\x1b[?12;2$y"]);
    std::env::set_var("PSMUX_CURSOR_STYLE", "bar");
    assert_eq!(texts(b"\x1b[?12h\x1b[?12$p"), vec!["\x1b[?12;1$y"]);
    assert_eq!(texts(b"\x1b[1 q\x1b[?12$p"), vec!["\x1b[?12;1$y"]);
}

#[test]
fn the_cursor_blink_falls_back_to_the_option() {
    let _env = CursorEnv::take();
    let ask: &[u8] = b"\x1b[?12$p";
    assert_eq!(texts(ask), vec!["\x1b[?12;2$y"]);
    for (value, n) in [
        ("default", 2),
        ("blinking-block", 1),
        ("block", 2),
        ("blinking-underline", 1),
        ("underline", 2),
        ("blinking-bar", 1),
        ("bar", 2),
    ] {
        std::env::set_var("PSMUX_CURSOR_STYLE", value);
        assert_eq!(texts(ask), vec![format!("\x1b[?12;{n}$y")], "{value}");
    }
    // As tmux 3.8 measured with cursor-style blinking-bar: DECSCUSR 0 and an
    // RIS drop the program's blink, so the option's shows again.
    std::env::set_var("PSMUX_CURSOR_STYLE", "blinking-bar");
    for bytes in [&b"\x1b[2 q\x1b[0 q"[..], b"\x1b[?12l\x1bc", b"\x1b[?12l\x1b[0 q"] {
        assert_eq!(texts(&[bytes, ask].concat()), vec!["\x1b[?12;1$y"], "{bytes:?}");
    }
    // psmux's own cursor-blink decides the blink of the option's style.
    std::env::set_var("PSMUX_CURSOR_STYLE", "bar");
    std::env::set_var("PSMUX_CURSOR_BLINK", "1");
    assert_eq!(texts(ask), vec!["\x1b[?12;1$y"]);
}

#[test]
fn synchronized_output_still_reads_the_gate() {
    let open = DecrqmState { synchronized_output: true, ..DecrqmState::default() };
    let queries = scan_device_queries(b"\x1b[?2026$p");
    let got: Vec<String> = pane_replies(&queries, &open, &HostColors::campbell(), &mut false)
        .into_iter()
        .map(|r| r.text)
        .collect();
    assert_eq!(got, vec!["\x1b[?2026;1$y"]);
    assert_eq!(texts(b"\x1b[?2026$p"), vec!["\x1b[?2026;2$y"]);
}
