// allow-passthrough on/all queues exact unescaped DCS tmux bodies for the
// host, without parsing them as pane output. Off drops the body as tmux does.
// The setting is one per process and read at each DCS, so a change applies
// to a parser that already exists. Its own test binary: the flag is global,
// and the tests in passthrough_discard.rs assume off.

use vt100_psmux as vt100;

#[derive(Default)]
struct Rec {
    copies: Vec<Vec<u8>>,
}

impl vt100::Callbacks for Rec {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], data: &[u8]) {
        self.copies.push(data.to_vec());
    }
}

fn wrapped(b64: &[u8], st: bool) -> Vec<u8> {
    let mut v = b"\x1bPtmux;\x1b\x1b]52;c;".to_vec();
    v.extend_from_slice(b64);
    v.push(0x07);
    if st {
        v.extend_from_slice(b"\x1b\\");
    }
    v
}

#[test]
fn toggling_allow_passthrough_applies_to_a_live_parser() {
    assert!(!vt100::allow_passthrough(), "off by default, as in tmux");
    let mut p = vt100::Parser::new_with_callbacks(10, 60, 0, Rec::default());

    p.process(&wrapped(b"b2Zm", true));
    p.process(b"a");
    assert!(p.callbacks().copies.is_empty(), "off: dropped");

    vt100::set_allow_passthrough(true);
    p.process(&wrapped(b"b24x", true));
    p.process(b"b");
    // The inbox conhost form, closing ST lost, then a redraw.
    p.process(&wrapped(b"b24y", false));
    p.process(b"\x1b[1mc\x1b[m");
    assert!(p.callbacks().copies.is_empty(), "host passthrough never changes the pane screen");
    let events = p.screen_mut().take_host_events();
    assert_eq!(events.iter().map(|e| e.bytes.as_slice()).collect::<Vec<_>>(), vec![&b"\x1b]52;c;b24x\x07"[..], &b"\x1b]52;c;b24y\x07"[..]]);
    assert!(events.iter().all(|e| e.passthrough));

    vt100::set_allow_passthrough(false);
    p.process(&wrapped(b"b2Zm", false));
    p.process(b"\x1b[1md\x1b[m");
    assert!(p.callbacks().copies.is_empty(), "off again: dropped");
    assert!(p.screen_mut().take_host_events().is_empty());

    assert_eq!(p.screen().contents_between(0, 0, 0, 60).trim_end(), "abcd");
    assert!(p.screen().cell(0, 2).unwrap().bold());
    assert!(p.screen().cell(0, 3).unwrap().bold());
    vt100::set_allow_passthrough(true);
    let input = b"\x1b]9;first\x07\x1bPtmux;\x1b\x1b]99;i=n;payload\x1b\x1b\\\x1b\\\x1b]9001;done\x07";
    for cut in 0..=input.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&input[..cut]);
        p.process(&input[cut..]);
        // Only the wrapped body: tmux forwards no unwrapped OSC 9 / 9001,
        // whatever allow-passthrough says (input.c input_exit_osc).
        let events = p.screen_mut().take_host_events();
        assert_eq!(events.len(), 1, "split {cut}");
        assert_eq!(events[0].bytes, b"\x1b]99;i=n;payload\x1b\\");
        assert!(events[0].passthrough);
        assert!(p.screen().contents().is_empty());
        assert!(p.screen_mut().take_host_events().is_empty());
    }
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(b"\x1bPtmux;");
    p.process(&vec![b'x'; vt100::MAX_HOST_EVENT_BYTES + 1]);
    p.process(b"\x1b\\\x1bPtmux;\x1b\x1b]9;after\x07\x1b\\");
    let events = p.screen_mut().take_host_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].bytes, b"\x1b]9;after\x07");

    // No further process call: the inbox conhost may lose the outer ST and
    // an agent waiting for user input emits no following bytes at all.
    let silent = b"\x1bPtmux;\x1b\x1b]9;waiting\x07";
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(silent);
    let events = p.screen_mut().take_host_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].bytes, b"\x1b]9;waiting\x07");
    p.process(b"\x1b\\");
    assert!(p.screen_mut().take_host_events().is_empty(), "outer ST cannot replay an early notification");
    let multiple = b"\x1bPtmux;\x1b\x1b]9;one\x07\x1b\x1b]777;notify;t;two\x07";
    for cut in 0..=multiple.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&multiple[..cut]);
        let mut events = p.screen_mut().take_host_events();
        p.process(&multiple[cut..]);
        events.extend(p.screen_mut().take_host_events());
        assert_eq!(events.iter().map(|e| e.bytes.as_slice()).collect::<Vec<_>>(), vec![&b"\x1b]9;one\x07"[..], &b"\x1b]777;notify;t;two\x07"[..]], "split {cut}");
        p.process(b"\x1b\\");
        assert!(p.screen_mut().take_host_events().is_empty());
    }
    for input in [
        &b"\x1bPtmux;prefix\x1b\x1b]9;waiting\x07"[..],
        &b"\x1bPtmux;\x1b\x1b]1234;unknown\x07"[..],
        &b"\x1bPtmux;\x1b\x1b]9;4;1;50\x07"[..],
        &b"\x1bPtmux;\x1b\x1b]7501;?\x07"[..],
    ] {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(input);
        assert!(p.screen_mut().take_host_events().is_empty(), "generic or mixed bodies remain opaque until the wrapper ends");
    }
    // ConPTY drops each outer ST. A following wrapper starts with the same
    // lone ESC that ends the previous one, and must still stop VTE at tmux;.
    let consecutive = b"\x1bPtmux;\x1b\x1b]9;first\x07\x1bPtmux;\x1b\x1b]12345;opaque\x07plain";
    for cut in 0..=consecutive.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&consecutive[..cut]);
        let mut events = p.screen_mut().take_host_events();
        p.process(&consecutive[cut..]);
        events.extend(p.screen_mut().take_host_events());
        assert_eq!(events.iter().map(|e| e.bytes.as_slice()).collect::<Vec<_>>(), vec![&b"\x1b]9;first\x07"[..], &b"\x1b]12345;opaque\x07"[..]], "lost ST split {cut}");
        assert!(events.iter().all(|e| e.passthrough));
        assert_eq!(p.screen().contents(), "plain");
    }
    // Prefix delivery cannot predict the wrapper's future suffix: once a
    // complete notification was committed, a later generic suffix is separate.
    let prefix = b"\x1bPtmux;\x1b\x1b]9;committed\x07";
    for drop_suffix in [false, true] {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(prefix);
        let early = p.screen_mut().take_host_events();
        assert_eq!(early.len(), 1);
        assert_eq!(early[0].bytes, b"\x1b]9;committed\x07");
        if drop_suffix {
            p.process(&vec![b'x'; vt100::MAX_HOST_EVENT_BYTES + 1]);
            p.process(b"\x1b\\");
            assert!(p.screen_mut().take_host_events().is_empty(), "dropping a suffix cannot recall or replay the committed prefix");
        } else {
            p.process(b"\x1b\x1b[32msuffix\x1b\x1b[0m\x1b\\");
            let suffix = p.screen_mut().take_host_events();
            assert_eq!(suffix.len(), 1);
            assert_eq!(suffix[0].bytes, b"\x1b[32msuffix\x1b[0m");
        }
    }
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(prefix);
    assert_eq!(p.screen_mut().take_host_events().len(), 1);
    p.process(b"\x1b\x1b]12345;suffix\x07plain");
    let suffix = p.screen_mut().take_host_events();
    assert_eq!(suffix.len(), 1);
    assert_eq!(suffix[0].bytes, b"\x1b]12345;suffix\x07");
    assert_eq!(p.screen().contents(), "plain", "lost outer ST recovers without replaying the committed prefix");
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(b"\x1bPtmux;\x1b\x1b]9;7;opaque-command\x07\x1b\\");
    let explicit = p.screen_mut().take_host_events();
    assert_eq!(explicit.len(), 1, "explicit enabled passthrough remains opaque");
    assert!(explicit[0].passthrough);
    assert_eq!(explicit[0].bytes, b"\x1b]9;7;opaque-command\x07");
    // Exercise the production conhost wrapper, which splits process_vt at
    // doubled ESCs, with intact, silent and consecutive lost-ST wrappers.
    for (input, expected, text) in [
        (&b"\x1bPtmux;\x1b\x1b]99;i=n;payload\x1b\x1b\\\x1b\\"[..], vec![&b"\x1b]99;i=n;payload\x1b\\"[..]], ""),
        (&b"\x1bPtmux;\x1b\x1b]9;waiting\x07"[..], vec![&b"\x1b]9;waiting\x07"[..]], ""),
        (&consecutive[..], vec![&b"\x1b]9;first\x07"[..], &b"\x1b]12345;opaque\x07"[..]], "plain"),
    ] {
        for cut in 0..=input.len() {
            let mut p = vt100::Parser::new(4, 80, 0);
            p.set_conhost_c0_cells(true);
            p.process(&input[..cut]);
            let mut events = p.screen_mut().take_host_events();
            p.process(&input[cut..]);
            events.extend(p.screen_mut().take_host_events());
            assert_eq!(events.iter().map(|e| e.bytes.as_slice()).collect::<Vec<_>>(), expected, "conhost split {cut}");
            assert!(events.iter().all(|e| e.passthrough));
            assert_eq!(p.screen().contents(), text);
            p.process(b"\x1b\\");
            assert!(p.screen_mut().take_host_events().is_empty());
        }
    }
    vt100::set_allow_passthrough(false);
    let off = b"\x1bPtmux;\x1b\x1b]9;off\x07\x1b\\";
    for cut in 0..=off.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.set_conhost_c0_cells(true);
        p.process(&off[..cut]);
        p.process(&off[cut..]);
        assert!(p.screen_mut().take_host_events().is_empty(), "conhost off split {cut}");
    }

}
