// Issue #1097: the Windows 10 inbox conhost passes a program's own
// `CSI ?9001h` through to psmux, at the start of the pane's stream where a
// host that parses win32 input mode writes its request.  psmux took it as the
// host's and wrote the #729 records, which that host drops, so a raw mode
// child that asked for the mode (an Ink based TUI) read nothing for Up, F5 or
// C-Right.  On an inbox host before build 22000 the request is never the
// host's; a host supplied with PSMUX_CONPTY_DIR keeps the stream's word.

use super::*;
use crate::input::pane_input_fixture::{fixture, pane};

struct HostBuild;
impl HostBuild {
    fn set(build: Option<u32>) -> HostBuild {
        TEST_WIN32_INPUT_HOST_BUILD.with(|b| b.set(build));
        HostBuild
    }
}
impl Drop for HostBuild {
    fn drop(&mut self) {
        TEST_WIN32_INPUT_HOST_BUILD.with(|b| b.set(None));
    }
}

#[test]
fn the_request_is_never_the_windows_10_inbox_hosts() {
    assert!(!host_parses_win32_input(true, Some(19045), false));
    assert!(!host_parses_win32_input(true, Some(19041), false));
    assert!(!host_parses_win32_input(true, Some(WIN32_INPUT_INBOX_MIN_BUILD - 1), false));
}

#[test]
fn a_supplied_host_and_later_builds_keep_the_streams_word() {
    assert!(host_parses_win32_input(true, Some(19045), true), "OpenConsole on Windows 10");
    assert!(host_parses_win32_input(true, Some(WIN32_INPUT_INBOX_MIN_BUILD), false));
    assert!(host_parses_win32_input(true, Some(26300), false));
    assert!(host_parses_win32_input(true, None, false), "unknown build: the stream decides");
    for (build, supplied) in [(Some(19045), false), (Some(19045), true), (Some(26300), false), (None, false)] {
        assert!(!host_parses_win32_input(false, build, supplied), "no request: never {:?} {}", build, supplied);
    }
}

#[test]
fn a_child_that_asked_gets_the_vt_bytes_on_windows_10() {
    let _b = HostBuild::set(Some(19045));
    let (mut app, rec) = fixture(true);
    let p = pane(&mut app);
    assert!(!pane_host_parses_win32_input(p));
    for seq in [&b"\x1b[A"[..], b"\x1b[15~", b"\x1b[1;5C"] {
        write_pane_input(p, seq);
        assert_eq!(rec.take(), seq.to_vec(), "{:02x?}", seq);
    }
    assert!(!p.win32_input_latched, "no record was written, so nothing latched");
}

#[test]
fn a_child_that_asked_keeps_the_729_records_on_windows_11() {
    let _b = HostBuild::set(Some(26300));
    let (mut app, rec) = fixture(true);
    let p = pane(&mut app);
    assert!(pane_host_parses_win32_input(p));
    write_pane_input(p, b"\x1b[A");
    let sent = String::from_utf8(rec.take()).unwrap();
    assert!(sent.starts_with("\x1b[38;") && sent.ends_with('_'), "{:?}", sent);
}
