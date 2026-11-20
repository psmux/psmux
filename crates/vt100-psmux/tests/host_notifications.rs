// A pane's own (unwrapped) notification OSCs never reach the host terminal,
// as in tmux 3.8: input_exit_osc (input.c) acts on OSC 9;4 progress and
// drops every other OSC 9, OSC 99, OSC 777, OSC 7501 and OSC 9001, whatever
// allow-passthrough says. A program reaches the outer terminal only through
// an explicit `DCS tmux;` wrapper with allow-passthrough on or all (see
// passthrough_allowed.rs). Its own test binary: the flag is process wide.

use vt100_psmux as vt100;

const UNWRAPPED: &[u8] = b"\x1b]9;hello;world\x07\x1b]777;notify;title;body\x1b\\\x1b]99;i=a:d=0:p=title;chunk\x1b\\\x1b]99;i=a:d=1:p=body:e=1;dGV4dA==\x07\x1b]7501;set;pid=1;state=working\x1b\\\x1b]9001;CmdNotFound;x\x07";

fn assert_nothing_forwarded(label: &str) {
    for cut in 0..=UNWRAPPED.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&UNWRAPPED[..cut]);
        p.process(&UNWRAPPED[cut..]);
        assert!(p.screen_mut().take_host_events().is_empty(), "{label}: split {cut}");
        assert!(p.screen().contents().is_empty(), "{label}: split {cut}: {:?}", p.screen().contents());
    }
    // The conhost path (doubled ESC split before the vte parser).
    let mut p = vt100::Parser::new(4, 80, 0);
    p.set_conhost_c0_cells(true);
    p.process(b"\x1b\x1b]9;notice\x07\x1b]777;notify;title;body\x1b\\");
    assert!(p.screen_mut().take_host_events().is_empty(), "{label}: conhost");
    assert!(!vt100::host_events_pending(), "{label}: nothing pending");
}

#[test]
fn unwrapped_notifications_are_never_forwarded_in_any_passthrough_mode() {
    assert!(!vt100::allow_passthrough(), "off by default, as in tmux");
    assert_nothing_forwarded("off");
    vt100::set_allow_passthrough(true);
    assert_nothing_forwarded("on");
    // The wrapped form is what reaches the host, consumed once.
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(b"\x1bPtmux;\x1b\x1b]9;wrapped\x07\x1b\\");
    assert!(vt100::host_events_pending());
    let events = p.screen_mut().take_host_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].bytes, b"\x1b]9;wrapped\x07");
    assert!(!vt100::host_events_pending());
    // A copy mode snapshot carries none, and dropping a parser releases its count.
    p.process(b"\x1bPtmux;\x1b\x1b]9;again\x07\x1b\\");
    assert!(p.snapshot().screen_mut().take_host_events().is_empty());
    drop(p);
    assert!(!vt100::host_events_pending());
    vt100::set_allow_passthrough(false);
}
