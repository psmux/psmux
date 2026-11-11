// `OSC 52 ; selection ; ?` asks for the clipboard. tmux keeps the selection
// characters it knows (`cpqs01234567`) and answers with the first
// (`input_osc_52_parse` and `input_osc_52_reply`, input.c:3341-3411 at tag
// 3.8). Every expectation is the selection tmux 3.8 answered with, measured.

use vt100_psmux as vt100;

fn queries(bytes: &str) -> Vec<vt100::TerminalQuery> {
    let mut parser = vt100::Parser::new(10, 40, 0);
    parser.screen_mut().capture_queries(true);
    parser.process(bytes.as_bytes());
    parser.screen_mut().take_queries()
}

fn clipboard(selection: u8, bel: bool) -> vt100::TerminalQuery {
    vt100::TerminalQuery::Clipboard { selection, bel }
}

#[test]
fn a_query_names_the_first_selection_tmux_knows() {
    for (written, selection) in [
        ("c", b'c'),
        ("", 0),
        ("pc", b'p'),
        ("xc", b'c'),
        ("x", 0),
        ("cc", b'c'),
        ("s0", b's'),
        ("7", b'7'),
    ] {
        assert_eq!(queries(&format!("\x1b]52;{written};?\x1b\\")), vec![clipboard(selection, false)], "{written:?}");
    }
}

#[test]
fn a_query_ended_in_bel_says_so() {
    assert_eq!(queries("\x1b]52;c;?\x07"), vec![clipboard(b'c', true)]);
}

#[test]
fn a_write_is_no_query_and_is_still_kept() {
    let mut parser = vt100::Parser::new(10, 40, 0);
    parser.screen_mut().capture_queries(true);
    parser.process(b"\x1b]52;c;aGVsbG8=\x1b\\\x1b]52;c;?\x1b\\");
    assert_eq!(parser.screen_mut().take_queries(), vec![clipboard(b'c', false)]);
    assert_eq!(parser.screen_mut().take_clipboard(), Some((b"c".to_vec(), b"aGVsbG8=".to_vec())));
}

#[test]
fn nothing_but_a_lone_question_mark_is_a_query() {
    for bytes in ["\x1b]52;?\x1b\\", "\x1b]52;c;??\x1b\\", "\x1b]52;c;?;x\x1b\\", "\x1b]52;c;\x1b\\"] {
        assert!(queries(bytes).is_empty(), "{bytes:?}");
    }
}

#[test]
fn a_query_split_at_every_byte() {
    let mut parser = vt100::Parser::new(10, 40, 0);
    parser.screen_mut().capture_queries(true);
    for b in b"\x1b]52;c;?\x1b\\" {
        parser.process(std::slice::from_ref(b));
    }
    assert_eq!(parser.screen_mut().take_queries(), vec![clipboard(b'c', false)]);
}
