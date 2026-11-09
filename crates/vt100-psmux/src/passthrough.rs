//! The body of a tmux passthrough, `ESC P tmux; ... ESC \`, which the pane
//! parser must not act on.
//!
//! tmux keeps the whole DCS in its input buffer and, at the ST, either drops
//! it (`allow-passthrough off`) or writes the part after `tmux;` to the outer
//! terminal; it never parses what is inside as pane output
//! (input_dcs_dispatch, input.c:2617-2683). vte ends every DCS at the first
//! ESC, so the doubled ESC in `ESC P tmux; ESC ESC ]52;...` ended the DCS and
//! vte then parsed the inner OSC 52 as if the pane had written it bare. The
//! pane parser stops at `tmux;` (`Perform::terminated`) and this skips the
//! rest of the wrapper before vte sees any of it.
//!
//! Where the wrapper ends:
//! - `ESC \`, as in tmux (input.c:698).
//! - An ESC followed by anything but ESC or `\`. Inside a passthrough every
//!   ESC is doubled, so a lone one can only be the start of the program's
//!   next sequence after a lost ST: the inbox conhost drops the closing ST of
//!   every passthrough (#1026). The ESC is handed back to vte. tmux would
//!   stay in the DCS until its 5 second timer; this keeps the following
//!   redraw.
//! - A byte other than ESC or BEL right after a BEL, for the same reason:
//!   conhost hands `... BEL` and then the program's text.
//! - 5 seconds after it began, at the next read: tmux's ground timer
//!   (input_start_ground_timer, input.c:826-832, started for every DCS at
//!   input.c:2547) resets the parser to ground then.

use std::time::{Duration, Instant};

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// tmux's ground timer: a DCS still open this long after it began is dropped.
const GROUND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Inside,
    Esc,
    AfterBel,
}

/// How a call to [`Passthrough::skip`] ended.
#[derive(PartialEq, Eq, Debug)]
pub(crate) enum Skip {
    /// Every byte was part of the wrapper, which goes on.
    More,
    /// The wrapper ended. `close` goes to vte to end its DCS, then normal
    /// parsing resumes at `resume`.
    End { resume: usize, close: &'static [u8] },
}

#[derive(Default, Debug)]
pub(crate) struct Passthrough {
    state: Option<(State, Instant)>,
}

impl Passthrough {
    pub(crate) fn begin(&mut self) {
        self.state = Some((State::Inside, Instant::now()));
    }

    pub(crate) fn active(&self) -> bool {
        self.state.is_some()
    }

    /// True once the wrapper has been open longer than tmux's ground timer.
    pub(crate) fn expired(&self) -> bool {
        self.state
            .is_some_and(|(_, since)| since.elapsed() >= GROUND_TIMEOUT)
    }

    pub(crate) fn end(&mut self) {
        self.state = None;
    }

    /// Consume wrapper bytes from the start of `bytes`.
    pub(crate) fn skip(&mut self, bytes: &[u8]) -> Skip {
        let Some((mut state, since)) = self.state else {
            return Skip::End { resume: 0, close: b"" };
        };
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            match state {
                State::Inside => {
                    // Payload bytes: jump to the next one that matters.
                    match memchr::memchr2(ESC, BEL, &bytes[i..]) {
                        Some(off) => {
                            i += off;
                            state = if bytes[i] == ESC { State::Esc } else { State::AfterBel };
                        }
                        None => break,
                    }
                }
                State::AfterBel => match b {
                    ESC => state = State::Esc,
                    BEL => {}
                    _ => {
                        self.state = None;
                        return Skip::End { resume: i, close: b"\x1b\\" };
                    }
                },
                State::Esc => match b {
                    ESC => state = State::Inside,
                    b'\\' => {
                        self.state = None;
                        return Skip::End { resume: i + 1, close: b"\x1b\\" };
                    }
                    _ => {
                        self.state = None;
                        return Skip::End { resume: i, close: b"\x1b" };
                    }
                },
            }
            i += 1;
        }
        self.state = Some((state, since));
        Skip::More
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[&[u8]]) -> (Vec<Skip>, bool) {
        let mut p = Passthrough::default();
        p.begin();
        let mut out = Vec::new();
        for c in chunks {
            out.push(p.skip(c));
        }
        (out, p.active())
    }

    #[test]
    fn ends_at_st() {
        let (r, active) = run(&[b"\x1b\x1b]52;c;aGk=\x07\x1b\\after"]);
        assert_eq!(r, vec![Skip::End { resume: 15, close: b"\x1b\\" }]);
        assert!(!active);
    }

    #[test]
    fn doubled_inner_st_stays_inside() {
        let input: &[u8] = b"\x1b\x1b]52;c;aGk=\x1b\x1b\\\x1b\\z";
        let (r, _) = run(&[input]);
        assert_eq!(r, vec![Skip::End { resume: input.len() - 1, close: b"\x1b\\" }]);
    }

    #[test]
    fn lost_st_ends_at_lone_esc() {
        let (r, _) = run(&[b"\x1b\x1b[6 q\x1b[?25l"]);
        assert_eq!(r, vec![Skip::End { resume: 7, close: b"\x1b" }]);
    }

    #[test]
    fn lost_st_ends_after_bel_text() {
        let (r, _) = run(&[b"\x1b\x1b]52;c;aGk=\x07plain"]);
        assert_eq!(r, vec![Skip::End { resume: 13, close: b"\x1b\\" }]);
    }

    #[test]
    fn split_reads_keep_state() {
        let input: &[u8] = b"\x1b\x1b]52;c;aGk=\x07\x1b\\after";
        for cut in 0..=input.len() {
            let mut p = Passthrough::default();
            p.begin();
            let a = p.skip(&input[..cut]);
            let got = match a {
                Skip::End { resume, close } => (resume, close),
                Skip::More => match p.skip(&input[cut..]) {
                    Skip::End { resume, close } => (cut + resume, close),
                    Skip::More => panic!("no end, cut {cut}"),
                },
            };
            assert_eq!(got, (15, &b"\x1b\\"[..]), "cut {cut}");
        }
    }

    #[test]
    fn payload_without_end_is_all_consumed() {
        let (r, active) = run(&[b"\x1b\x1b]1337;File=:", &[b'A'; 4096][..]]);
        assert_eq!(r, vec![Skip::More, Skip::More]);
        assert!(active);
    }
}
