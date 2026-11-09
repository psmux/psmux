// allow-passthrough on/all keeps the parser's old reading of a DCS tmux;
// passthrough: vte ends the DCS at the first doubled ESC and acts on the inner
// sequence, so a wrapped OSC 52 still copies. Off drops the body as tmux does.
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
    assert_eq!(p.callbacks().copies, vec![b"b24x".to_vec(), b"b24y".to_vec()], "on: acted on as before");

    vt100::set_allow_passthrough(false);
    p.process(&wrapped(b"b2Zm", false));
    p.process(b"\x1b[1md\x1b[m");
    assert_eq!(p.callbacks().copies.len(), 2, "off again: dropped");

    assert_eq!(p.screen().contents_between(0, 0, 0, 60).trim_end(), "abcd");
    assert!(p.screen().cell(0, 2).unwrap().bold());
    assert!(p.screen().cell(0, 3).unwrap().bold());
}
