//! Unwrap tmux DCS passthrough (`ESC P tmux; ... ESC \`) in the raw pane
//! output that control clients are sent as `%output`.
//!
//! A program that finds `$TMUX` set wraps sequences meant for the outer
//! terminal in a passthrough DCS, doubling each inner ESC. Claude Code does
//! this for its OSC 52 copy: `ESC P tmux; ESC ESC ]52;c;<base64> BEL ESC \`.
//! ConPTY (seen on build 26200) hands psmux that wrapper with the closing
//! `ESC \` gone: `ESC P tmux; ESC ESC ]52;c;<base64> BEL` and then the
//! program's next output. psmux's own grid is not hurt (its parser leaves
//! the DCS at the next ESC), but `%output` carries the raw bytes, and
//! iTerm2's tmux integration then sits inside an unterminated DCS: the
//! following redraw loses every ESC and lands on screen as text
//! (`[?25l[7;3H...`).
//!
//! The fix drops the wrapper for control clients and keeps what it wraps:
//! `ESC P tmux;` is removed, `ESC ESC` inside it becomes one ESC, and the
//! wrapper ends at `ESC \` or, when that was lost, at the first ESC not
//! followed by ESC or `\`, which is left in place. A control client is the
//! outer terminal, so the inner sequence is what the program meant for it.
//! The pane parser and `pipe-pane` still get the bytes as read.

use std::borrow::Cow;

const ESC: u8 = 0x1B;
const WRAPPER_START: &[u8] = b"\x1bPtmux;";

/// Per-pane state, owned by that pane's reader thread.
#[derive(Default)]
pub struct TmuxPassthroughUnwrapper {
    /// Inside a wrapper whose end has not been seen yet.
    inside: bool,
    /// Tail of the previous read that may continue into the next one: a
    /// partial `ESC P tmux;` outside a wrapper, or a lone ESC inside one.
    held: Vec<u8>,
}

impl TmuxPassthroughUnwrapper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes to queue for control clients in place of `input`. Borrows
    /// `input` unchanged when it holds no wrapper and nothing is pending.
    pub fn feed<'a>(&mut self, input: &'a [u8]) -> Cow<'a, [u8]> {
        if !self.inside && self.held.is_empty() && !may_start_wrapper(input) {
            return Cow::Borrowed(input);
        }
        let mut buf = std::mem::take(&mut self.held);
        buf.extend_from_slice(input);
        let mut out = Vec::with_capacity(buf.len());
        let mut i = 0;
        while i < buf.len() {
            let Some(off) = buf[i..].iter().position(|&b| b == ESC) else {
                out.extend_from_slice(&buf[i..]);
                break;
            };
            let e = i + off;
            out.extend_from_slice(&buf[i..e]);
            let rest = &buf[e..];
            if self.inside {
                match rest.get(1) {
                    None => {
                        self.held.push(ESC);
                        break;
                    }
                    Some(&ESC) => {
                        out.push(ESC);
                        i = e + 2;
                    }
                    Some(&b'\\') => {
                        self.inside = false;
                        i = e + 2;
                    }
                    Some(_) => {
                        // The closing ESC \ was lost: this ESC is the
                        // program's next sequence. Look at it again outside.
                        self.inside = false;
                        i = e;
                    }
                }
            } else if rest.starts_with(WRAPPER_START) {
                self.inside = true;
                i = e + WRAPPER_START.len();
            } else if rest.len() < WRAPPER_START.len() && WRAPPER_START.starts_with(rest) {
                self.held.extend_from_slice(rest);
                break;
            } else {
                out.push(ESC);
                i = e + 1;
            }
        }
        Cow::Owned(out)
    }
}

/// Whether `input` has an `ESC P` or ends in ESC, the only ways a wrapper
/// can start in it.
fn may_start_wrapper(input: &[u8]) -> bool {
    let mut from = 0;
    while let Some(off) = input[from..].iter().position(|&b| b == ESC) {
        let e = from + off;
        match input.get(e + 1) {
            None | Some(&b'P') => return true,
            Some(_) => from = e + 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> Vec<u8> {
        let mut u = TmuxPassthroughUnwrapper::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend_from_slice(&u.feed(c));
        }
        out
    }

    #[test]
    fn plain_output_is_borrowed_unchanged() {
        let mut u = TmuxPassthroughUnwrapper::new();
        let input = b"\x1b[?25l\x1b[7;3Hhello\x1b]52;c;aGk=\x07\x1b]0;title\x1b\\";
        assert!(matches!(u.feed(input), Cow::Borrowed(_)));
        assert_eq!(&*u.feed(input), &input[..]);
        // Another DCS takes the slow path but comes out the same.
        let dcs = b"\x1bP+q544e\x1b\\\x1bP>|psmux\x1b\\";
        assert_eq!(&*u.feed(dcs), &dcs[..]);
    }

    #[test]
    fn conpty_form_without_closing_st_is_unwrapped_and_redraw_kept() {
        // As read from ConPTY on build 26200 after Claude Code's copy.
        let input: &[u8] = b"\x1b]52;c;aGk=\x07\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b[?25l\x1b[7;3HHi";
        assert_eq!(
            run(&[input]),
            b"\x1b]52;c;aGk=\x07\x1b]52;c;aGk=\x07\x1b[?25l\x1b[7;3HHi".to_vec()
        );
    }

    #[test]
    fn well_formed_wrapper_is_unwrapped() {
        let input: &[u8] = b"a\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\b";
        assert_eq!(run(&[input]), b"a\x1b]52;c;aGk=\x07b".to_vec());
    }

    #[test]
    fn inner_st_is_kept_and_outer_st_dropped() {
        let input: &[u8] = b"\x1bPtmux;\x1b\x1b]1337;File=x:eA==\x1b\x1b\\\x1b\\z";
        assert_eq!(run(&[input]), b"\x1b]1337;File=x:eA==\x1b\\z".to_vec());
    }

    #[test]
    fn wrapper_split_at_every_byte_gives_the_same_result() {
        let input: &[u8] = b"x\x1b]52;c;aGk=\x07\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b[?25l\x1bPtmux;\x1b\x1b]8;;u\x1b\x1b\\\x1b\\y";
        let whole = run(&[input]);
        assert_eq!(whole, b"x\x1b]52;c;aGk=\x07\x1b]52;c;aGk=\x07\x1b[?25l\x1b]8;;u\x1b\\y".to_vec());
        for cut in 1..input.len() {
            assert_eq!(run(&[&input[..cut], &input[cut..]]), whole, "cut at {cut}");
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(run(&bytes), whole);
    }

    #[test]
    fn other_dcs_and_lookalikes_pass_through() {
        let input = b"\x1bPtmuy;q\x1b\\\x1bP1000p\x1bPtmu";
        // The trailing partial prefix is held until the next read decides it.
        let mut u = TmuxPassthroughUnwrapper::new();
        assert_eq!(&*u.feed(input), &b"\x1bPtmuy;q\x1b\\\x1bP1000p"[..]);
        assert_eq!(&*u.feed(b"lti"), &b"\x1bPtmulti"[..]);
    }

    #[test]
    fn text_inside_an_unterminated_wrapper_is_not_held_back() {
        let mut u = TmuxPassthroughUnwrapper::new();
        assert_eq!(&*u.feed(b"\x1bPtmux;\x1b\x1b]52;c;aGk=\x07plain text"), &b"\x1b]52;c;aGk=\x07plain text"[..]);
        assert_eq!(&*u.feed(b" more\x1b[0m"), &b" more\x1b[0m"[..]);
    }

    #[test]
    fn recorded_claude_copy_from_select_debug() {
        // Shape of the pane bytes recorded 2026-11-08 (base64 shortened).
        let b64 = b"SGkgQ2hyaXMsCgpHb29kIGNhdGNoLg==";
        let mut input = Vec::new();
        input.extend_from_slice(b"\x1b]52;c;");
        input.extend_from_slice(b64);
        input.push(0x07);
        input.extend_from_slice(b"\x1bPtmux;\x1b\x1b]52;c;");
        input.extend_from_slice(b64);
        input.push(0x07);
        input.extend_from_slice(b"\x1b[?25l\x1b[38;122H\x1b[K\x1b[40;3H\x1b[?25h\x1b[?2004h\x1b[?25l\x1b[7;3HHi Chris,");
        let out = run(&[&input[..]]);
        assert!(!out.windows(WRAPPER_START.len()).any(|w| w == WRAPPER_START));
        assert_eq!(out.windows(7).filter(|w| *w == b"\x1b]52;c;").count(), 2);
        assert!(out.ends_with(b"\x1b[?25l\x1b[7;3HHi Chris,"));
    }
}
