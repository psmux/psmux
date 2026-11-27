// DECRQSS replies, `DCS $ q Pt ST`. tmux answers `SP q` with the cursor
// style the program set, else the `cursor-style` option's, and anything else
// with `DCS 0 $ r ST` (input_handle_decrqss, input.c:2554-2617 at tag 3.8).
// psmux writes the number the VT510's DECRPSS way, `DCS 1 $ r Ps SP q ST`,
// which its own reader of a terminal's reply reads.
use super::*;
use crate::types::HostColors;

/// `cursor-style` and `cursor-blink` live in the environment, so a test that
/// reads the fallback takes them out under the shared lock and puts them
/// back afterwards (see `test_issue735_cursor_style_default.rs`). The DECRQM
/// tests read the same fallback.
pub(super) struct CursorEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl CursorEnv {
    pub(super) fn take() -> Self {
        let lock = crate::util::lock_test_env();
        let saved = ["PSMUX_CURSOR_STYLE", "PSMUX_CURSOR_BLINK"]
            .into_iter()
            .map(|name| {
                let previous = std::env::var(name).ok();
                std::env::remove_var(name);
                (name, previous)
            })
            .collect();
        Self { _lock: lock, saved }
    }
}

impl Drop for CursorEnv {
    fn drop(&mut self) {
        for (name, previous) in &self.saved {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

fn replies(bytes: &[u8]) -> Vec<PaneReply> {
    let queries = scan_device_queries(bytes);
    let mut open = false;
    pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), &mut open)
}

fn texts(bytes: &[u8]) -> Vec<String> {
    replies(bytes).into_iter().map(|r| r.text).collect()
}

#[test]
fn the_program_style_in_the_standard_form() {
    let _env = CursorEnv::take();
    assert_eq!(texts(b"\x1b[4 q\x1bP$q q\x1b\\"), vec!["\x1bP1$r4 q\x1b\\"]);
    assert_eq!(texts(b"\x1b[2 q\x1b[?12h\x1bP$q q\x1b\\"), vec!["\x1bP1$r1 q\x1b\\"]);
}

#[test]
fn no_program_style_falls_back_to_the_option() {
    let _env = CursorEnv::take();
    let ask: &[u8] = b"\x1bP$q q\x1b\\";
    assert_eq!(texts(ask), vec!["\x1bP1$r0 q\x1b\\"]);
    for (value, ps) in [
        ("default", 0),
        ("blinking-block", 1),
        ("block", 2),
        ("blinking-underline", 3),
        ("underline", 4),
        ("blinking-bar", 5),
        ("bar", 6),
    ] {
        std::env::set_var("PSMUX_CURSOR_STYLE", value);
        assert_eq!(texts(ask), vec![format!("\x1bP1$r{ps} q\x1b\\")], "{value}");
    }
    // The program's style wins over the option's.
    std::env::set_var("PSMUX_CURSOR_STYLE", "bar");
    assert_eq!(texts(b"\x1b[3 q\x1bP$q q\x1b\\"), vec!["\x1bP1$r3 q\x1b\\"]);
}

#[test]
fn psmux_reads_its_own_reply() {
    // The client reads the outer terminal's reply with this; tmux's form,
    // `DCS 1 $ r SP q 4 SP q ST`, reads as 0 there.
    let _env = CursorEnv::take();
    for style in 1..=6u8 {
        let text = cursor_style_reply(style);
        assert_eq!(crate::platform::parse_decrqss_cursor_style(text.as_bytes()), Some(style));
    }
    assert_eq!(crate::platform::parse_decrqss_cursor_style(b"\x1bP1$r q4 q\x1b\\"), Some(0));
}

#[test]
fn any_other_setting_is_refused() {
    for q in [&b"\x1bP$qm\x1b\\"[..], b"\x1bP$qr\x1b\\", b"\x1bP$q\"q\x1b\\", b"\x1bP$q\x1b\\"] {
        assert_eq!(texts(q), vec!["\x1bP0$r\x1b\\"], "{q:?}");
    }
}

#[test]
fn in_order_and_never_down_the_pipe() {
    let _env = CursorEnv::take();
    let got = replies(b"\x1b[5 q\x1bP$q q\x1b\\\x1b[c\x1bP$qm\x1b\\");
    let text: Vec<&str> = got.iter().map(|r| r.text.as_str()).collect();
    assert_eq!(text, vec!["\x1bP1$r5 q\x1b\\", "\x1b[?1;2;4c", "\x1bP0$r\x1b\\"]);
    // A DCS written to the pane's input pipe loses its body (#597).
    assert_eq!(got[0].fallback, ReplyFallback::Lost("DECRQSS"));
    assert_eq!(got[2].fallback, ReplyFallback::Lost("DECRQSS"));
    assert!(got.iter().all(|r| !r.console_host));
}
