// Issue #697 item 2: "the cursor flickers" while typing into fzf-lua's
// live_grep in Neovim inside psmux, and not in Neovim alone.
//
// Measured with tests/conpty697.cs: every keystroke made psmux send the host
// its cells with the cursor still VISIBLE (ratatui draws the diff first and
// hides the cursor after it), e.g.
//
//     ESC[?25h a ESC[78C <spinner> ESC[?25l ESC[4;19H ESC[?25h
//
// so the host showed the cursor running 78 columns to fzf's spinner and back,
// two or three times per keystroke. tmux hides the cursor before it draws
// (screen-redraw.c screen_redraw_screen: tty_update_mode(tty, tty->mode &
// ~CURSOR_MODES, NULL) ahead of the lines) and only writes cnorm/civis when
// the mode changes (tty.c tty_update_mode).
//
// These tests drive the real ratatui Terminal over PsmuxBackend with a
// recording writer and assert the policy on the bytes each frame produces:
//   * a frame that draws cells hides the cursor BEFORE the first cell,
//   * nothing is printed and the cursor never moves while it is visible,
//   * the cursor is shown only after it has been put back,
//   * the frame leaves as ONE write,
//   * an unchanged frame writes nothing at all.

use std::cell::RefCell;
use std::rc::Rc;

use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};

use crate::platform::{HostCursor, PsmuxBackend};

/// Records every flushed write separately, like console writes reaching the
/// host one WriteConsoleW at a time.
#[derive(Clone, Default)]
struct Recorder {
    pending: Rc<RefCell<Vec<u8>>>,
    writes: Rc<RefCell<Vec<Vec<u8>>>>,
}

impl std::io::Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.borrow_mut().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut p = self.pending.borrow_mut();
        if !p.is_empty() {
            self.writes.borrow_mut().push(std::mem::take(&mut *p));
        }
        Ok(())
    }
}

impl Recorder {
    fn take_writes(&self) -> Vec<String> {
        self.writes
            .borrow_mut()
            .drain(..)
            .map(|w| String::from_utf8_lossy(&w).into_owned())
            .collect()
    }
}

fn terminal(rec: &Recorder) -> Terminal<PsmuxBackend<Recorder>> {
    Terminal::with_options(
        PsmuxBackend::new(rec.clone()),
        TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, 120, 30)) },
    )
    .unwrap()
}

/// One client frame the way client.rs drives it: begin, draw, cursor, end.
fn frame(
    term: &mut Terminal<PsmuxBackend<Recorder>>,
    cells: &[(u16, u16, &str)],
    ratatui_cursor: Option<(u16, u16)>,
    cursor: Option<(u16, u16)>,
) {
    term.backend_mut().begin_frame();
    term.draw(|f| {
        for (x, y, s) in cells {
            f.buffer_mut()[(*x, *y)].set_symbol(s);
        }
        if let Some(p) = ratatui_cursor {
            f.set_cursor_position(p);
        }
    })
    .unwrap();
    term.backend_mut().request_cursor(cursor);
    term.backend_mut().end_frame().unwrap();
}

/// Split a VT string into escape sequences and single characters.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if b[i] == '\x1b' && i + 1 < b.len() && b[i + 1] == '[' {
            let mut j = i + 2;
            while j < b.len() && !('\x40'..='\x7e').contains(&b[j]) {
                j += 1;
            }
            out.push(b[i..=j.min(b.len() - 1)].iter().collect());
            i = j + 1;
        } else if b[i] == '\x1b' && i + 1 < b.len() {
            out.push(b[i..i + 2].iter().collect());
            i += 2;
        } else {
            out.push(b[i].to_string());
            i += 1;
        }
    }
    out
}

fn moves_cursor(t: &str) -> bool {
    if !t.starts_with('\x1b') {
        return true; // printable text or a control that moves the cursor
    }
    t.starts_with("\x1b[")
        && matches!(t.chars().last(), Some('H' | 'A' | 'B' | 'C' | 'D' | 'G' | 'd' | 'f'))
}

/// Number of tokens that print or move the cursor while it is visible.
/// `start_visible` is the host state before the write.
fn visible_draws(s: &str, start_visible: bool) -> usize {
    let mut vis = start_visible;
    let mut n = 0;
    for t in tokens(s) {
        match t.as_str() {
            "\x1b[?25l" => vis = false,
            "\x1b[?25h" => vis = true,
            _ if vis && moves_cursor(&t) => n += 1,
            _ => {}
        }
    }
    n
}

fn count(s: &str, pat: &str) -> usize {
    s.matches(pat).count()
}

#[test]
fn keystroke_frame_hides_before_drawing_and_shows_after_placing() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    // First frame: the prompt, cursor after it.
    frame(&mut term, &[(14, 3, ">"), (15, 3, " ")], None, Some((18, 3)));
    rec.take_writes();

    // A live_grep keystroke: the typed char at the cursor and fzf's spinner
    // 78 columns to the right, cursor one column further.
    frame(&mut term, &[(18, 3, "a"), (97, 3, "\u{2819}")], None, Some((19, 3)));
    let w = rec.take_writes();
    assert_eq!(w.len(), 1, "the frame must reach the host as one write: {w:?}");
    let s = &w[0];
    assert!(s.starts_with("\x1b[?25l"), "cursor must be hidden before the first cell: {s:?}");
    assert_eq!(count(s, "\x1b[?25l"), 1, "{s:?}");
    assert_eq!(count(s, "\x1b[?25h"), 1, "{s:?}");
    assert!(s.ends_with("\x1b[4;20H\x1b[?25h"), "shown only after CUP to the new spot: {s:?}");
    assert_eq!(visible_draws(s, true), 0, "nothing may be drawn while the cursor is visible: {s:?}");
}

#[test]
fn unchanged_frame_writes_nothing() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    rec.take_writes();
    for _ in 0..5 {
        frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    }
    let w = rec.take_writes();
    assert!(w.is_empty(), "idle frames must not toggle the cursor: {w:?}");
}

#[test]
fn cursor_only_move_is_a_plain_cup_without_toggling() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    rec.take_writes();
    frame(&mut term, &[(0, 0, "x")], None, Some((6, 5)));
    let w = rec.take_writes();
    assert_eq!(w, vec!["\x1b[6;7H".to_string()]);
}

#[test]
fn hidden_cursor_frames_hide_once() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    rec.take_writes();
    // The pane app hid its cursor (nvim while busy): one ?25l, then silence.
    frame(&mut term, &[(0, 0, "x")], None, None);
    assert_eq!(rec.take_writes(), vec!["\x1b[?25l".to_string()]);
    frame(&mut term, &[(1, 0, "y")], None, None);
    let w = rec.take_writes();
    assert_eq!(w.len(), 1);
    assert_eq!(count(&w[0], "\x1b[?25"), 0, "already hidden: {:?}", w[0]);
    // Shown again: placed first, then shown.
    frame(&mut term, &[(1, 0, "y")], None, Some((2, 2)));
    assert_eq!(rec.take_writes(), vec!["\x1b[3;3H\x1b[?25h".to_string()]);
}

#[test]
fn ratatui_cursor_request_in_copy_mode_follows_the_same_policy() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    rec.take_writes();
    // Copy mode: the client sets the cursor through ratatui and passes None.
    frame(&mut term, &[(10, 10, "s")], Some((10, 10)), None);
    let w = rec.take_writes();
    assert_eq!(w.len(), 1, "{w:?}");
    let s = &w[0];
    assert!(s.starts_with("\x1b[?25l"), "{s:?}");
    assert!(s.ends_with("\x1b[11;11H\x1b[?25h"), "{s:?}");
    assert_eq!(visible_draws(s, true), 0, "{s:?}");
}

#[test]
fn overlay_bytes_are_drawn_hidden() {
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    frame(&mut term, &[(0, 0, "x")], None, Some((5, 5)));
    rec.take_writes();
    term.backend_mut().begin_frame();
    term.draw(|f| {
        f.buffer_mut()[(0, 0)].set_symbol("x");
    })
    .unwrap();
    term.backend_mut().queue_drawing(b"\x1b7\x1b[2;2Hlink\x1b8").unwrap();
    term.backend_mut().request_cursor(Some((5, 5)));
    term.backend_mut().end_frame().unwrap();
    let w = rec.take_writes();
    assert_eq!(w, vec!["\x1b[?25l\x1b7\x1b[2;2Hlink\x1b8\x1b[6;6H\x1b[?25h".to_string()]);
}

#[test]
fn outside_a_frame_cursor_calls_write_through() {
    // Startup and shutdown call show_cursor/hide_cursor directly; those must
    // still reach the host immediately.
    let rec = Recorder::default();
    let mut term = terminal(&rec);
    term.show_cursor().unwrap();
    assert_eq!(rec.take_writes(), vec!["\x1b[?25h".to_string()]);
}

#[test]
fn host_cursor_policy_first_frame_hides_and_places() {
    let mut c = HostCursor::default();
    let mut out = Vec::new();
    c.begin_frame();
    c.before_draw(&mut out);
    c.request(Some((0, 0)));
    c.end_frame(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "\x1b[?25l\x1b[1;1H\x1b[?25h");
}
