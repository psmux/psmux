// A tmux passthrough (`ESC P tmux; ... ESC \`) is not pane output: tmux keeps
// the whole DCS and never parses what is inside it (input_dcs_dispatch,
// input.c:2617-2683). vte ended the DCS at the first doubled ESC and then
// acted on the inner sequence, so a wrapped OSC 52 set the clipboard even with
// allow-passthrough off. The forms below are the ones the console hosts hand
// psmux: OpenConsole keeps the closing ST, the inbox conhost drops it (#1026).

use vt100_psmux as vt100;

#[derive(Default)]
struct Rec {
    copies: Vec<Vec<u8>>,
    titles: Vec<Vec<u8>>,
}

impl vt100::Callbacks for Rec {
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], data: &[u8]) {
        self.copies.push(data.to_vec());
    }
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.titles.push(title.to_vec());
    }
}

fn parse(chunks: &[&[u8]]) -> vt100::Parser<Rec> {
    let mut p = vt100::Parser::new_with_callbacks(10, 60, 0, Rec::default());
    for c in chunks {
        p.process(c);
    }
    p
}

fn row(p: &vt100::Parser<Rec>, r: u16) -> String {
    p.screen().contents_between(r, 0, r, 60).trim_end().to_string()
}

const WRAPPED_ST: &[u8] = b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\";
// As the inbox conhost hands it: no closing ST.
const WRAPPED_LOST: &[u8] = b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x07";

#[test]
fn wrapped_osc52_is_not_acted_on() {
    let mut input = WRAPPED_ST.to_vec();
    input.extend_from_slice(b"after");
    let p = parse(&[&input]);
    assert!(p.callbacks().copies.is_empty(), "wrapped OSC 52 set the clipboard");
    assert_eq!(row(&p, 0), "after");
}

#[test]
fn plain_osc52_still_is() {
    let p = parse(&[b"\x1b]52;c;aGk=\x07x"]);
    assert_eq!(p.callbacks().copies, vec![b"aGk=".to_vec()]);
}

#[test]
fn lost_st_then_redraw_keeps_the_redraw() {
    let mut input = b"\x1b]52;c;cGxhaW4=\x07".to_vec();
    input.extend_from_slice(WRAPPED_LOST);
    input.extend_from_slice(b"\x1b[3;5HREDRAW\x1b[1mB");
    let p = parse(&[&input]);
    assert_eq!(p.callbacks().copies, vec![b"cGxhaW4=".to_vec()], "only the plain copy");
    assert_eq!(row(&p, 2), "    REDRAWB");
    assert!(p.screen().cell(2, 10).unwrap().bold(), "the SGR after the wrapper applied");
}

#[test]
fn lost_st_then_text_keeps_the_text() {
    let mut input = WRAPPED_LOST.to_vec();
    input.extend_from_slice(b"plain");
    let p = parse(&[&input]);
    assert!(p.callbacks().copies.is_empty());
    assert_eq!(row(&p, 0), "plain");
}

#[test]
fn inner_st_and_other_inner_sequences_are_not_acted_on() {
    let input: &[u8] = b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\\
\x1bPtmux;\x1b\x1b]2;WRAPPEDTITLE\x07\x1b\\\
\x1bPtmux;\x1b\x1b[2J\x1b\x1b[HZAP\x1b\\ok";
    let p = parse(&[b"keep\r\n", input]);
    assert!(p.callbacks().copies.is_empty());
    assert!(p.callbacks().titles.is_empty(), "wrapped title applied");
    assert_eq!(row(&p, 0), "keep", "wrapped clear screen applied");
    assert_eq!(row(&p, 1), "ok");
}

#[test]
fn same_result_for_every_split() {
    let mut input = b"a".to_vec();
    input.extend_from_slice(WRAPPED_ST);
    input.extend_from_slice(b"b");
    input.extend_from_slice(WRAPPED_LOST);
    input.extend_from_slice(b"\x1b[Kc\x1b]52;c;eA==\x07d");
    let whole = parse(&[&input]);
    assert_eq!(row(&whole, 0), "abcd");
    assert_eq!(whole.callbacks().copies, vec![b"eA==".to_vec()]);
    for cut in 1..input.len() {
        let p = parse(&[&input[..cut], &input[cut..]]);
        assert_eq!(row(&p, 0), "abcd", "cut {cut}");
        assert_eq!(p.callbacks().copies, vec![b"eA==".to_vec()], "cut {cut}");
    }
    let bytes: Vec<&[u8]> = input.chunks(1).collect();
    let p = parse(&bytes);
    assert_eq!(row(&p, 0), "abcd");
    assert_eq!(p.callbacks().copies, vec![b"eA==".to_vec()]);
}

#[test]
fn other_dcs_unchanged() {
    // A lookalike is an ordinary DCS, and the sixel path still makes an image.
    let p = parse(&[b"\x1bPtmuy;\x1b\x1b]52;c;aGk=\x07x"]);
    assert_eq!(p.callbacks().copies, vec![b"aGk=".to_vec()], "not a passthrough: vte as before");
    let p = parse(&[b"\x1bPq#0;2;0;0;0#0~~~~\x1b\\"]);
    assert_eq!(p.screen().images().len(), 1);
}

#[test]
fn huge_wrapped_payload_is_skipped_without_growth() {
    let mut input = b"\x1bPtmux;\x1b\x1b]1337;File=inline=1:".to_vec();
    input.extend(std::iter::repeat(b'A').take(4 << 20));
    input.extend_from_slice(b"\x07\x1b\\after");
    let p = parse(&[&input]);
    assert_eq!(row(&p, 0), "after");
}
