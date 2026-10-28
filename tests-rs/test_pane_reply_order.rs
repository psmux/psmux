// The colour and XTVERSION replies to one read go in as one injection, in the
// order their queries came, as a terminal and tmux answer.
use super::injected_replies;

const XT: &str = "\x1bP>|tmux 3.4.0\x1b\\";
const BG: &str = "\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\";

#[test]
fn xtversion_asked_first_is_answered_first() {
    // What yazi sends at start up, in this order.
    let batch = b"\x1b[>q\x1b]11;?\x1b\\\x1b[16t\x1b[c";
    assert_eq!(injected_replies(batch, BG, XT), format!("{XT}{BG}"));
    assert_eq!(injected_replies(b"\x1b[>0q\x1b]10;?\x07", BG, XT), format!("{XT}{BG}"));
}

#[test]
fn a_colour_query_asked_first_is_answered_first() {
    let batch = b"\x1b]11;?\x1b\\\x1b[>q\x1b[c";
    assert_eq!(injected_replies(batch, BG, XT), format!("{BG}{XT}"));
    assert_eq!(injected_replies(b"\x1b[?996n\x1b[>q", BG, XT), format!("{BG}{XT}"));
    assert_eq!(injected_replies(b"\x1b]4;1;?\x07\x1b[>q", BG, XT), format!("{BG}{XT}"));
}

#[test]
fn one_kind_alone_is_that_reply() {
    assert_eq!(injected_replies(b"\x1b[>q", "", XT), XT);
    assert_eq!(injected_replies(b"\x1b]11;?\x07", BG, ""), BG);
    assert_eq!(injected_replies(b"plain", "", ""), "");
}

#[test]
fn a_query_split_off_into_another_read_leaves_the_colours_first() {
    // The XTVERSION query was in the previous read; only the colour query is
    // in this one, and the scanners still report both.
    assert_eq!(injected_replies(b"q\x1b]11;?\x07", BG, XT), format!("{BG}{XT}"));
}
