// NO_COLOR in the client's environment stripped every colour the client drew.
//
// crossterm's `Colored` writes nothing when `NO_COLOR` is set (crossterm 0.29
// `style/types/colored.rs`, `ansi_color_disabled_memoized`), and the client
// wrote its foreground and background colours through crossterm's
// `SetForegroundColor` / `SetBackgroundColor`.  So `NO_COLOR=1 psmux attach`
// drew a pane printing red text as plain text, and every border and status
// line lost its colour.  tmux has no `NO_COLOR` handling; the panes inherit
// the variable and decide for themselves.
//
// crossterm reads the variable once per process, so these tests drive its
// switch directly instead of the environment, and restore it afterwards.

use std::sync::Mutex;

use crossterm::style::Colored;
use ratatui::buffer::{Buffer, Cell as RCell};
use ratatui::layout::Rect;
use ratatui::style::{Color as RColor, Style};

use super::{draw_cells, to_crossterm_color, write_sgr_color};

static CROSSTERM_COLOR_SWITCH: Mutex<()> = Mutex::new(());

/// Sets crossterm's process wide "colours disabled" switch for the life of
/// the guard.
struct ColorSwitch {
    previous: bool,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ColorSwitch {
    fn set(disabled: bool) -> Self {
        let lock = CROSSTERM_COLOR_SWITCH.lock().unwrap_or_else(|e| e.into_inner());
        let previous = Colored::ansi_color_disabled_memoized();
        Colored::set_ansi_color_disabled(disabled);
        Self { previous, _lock: lock }
    }
}

impl Drop for ColorSwitch {
    fn drop(&mut self) {
        Colored::set_ansi_color_disabled(self.previous);
    }
}

fn render(cells: &[(&'static str, Style)]) -> String {
    let mut buf = Buffer::empty(Rect::new(0, 0, cells.len() as u16, 1));
    for (i, (sym, style)) in cells.iter().enumerate() {
        let mut c = RCell::new(sym);
        c.set_style(*style);
        buf[(i as u16, 0)] = c;
    }
    let content: Vec<(u16, u16, &RCell)> =
        (0..cells.len() as u16).map(|x| (x, 0u16, &buf[(x, 0)])).collect();
    let mut out: Vec<u8> = Vec::new();
    draw_cells(&mut out, content.into_iter()).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn colours_reach_the_host_with_no_color_set() {
    let _switch = ColorSwitch::set(true);
    let out = render(&[
        ("R", Style::default().fg(RColor::Red)),
        ("B", Style::default().bg(RColor::Rgb(1, 2, 3))),
        ("I", Style::default().fg(RColor::Indexed(214)).bg(RColor::LightBlue)),
    ]);
    for want in ["\x1b[38;5;1m", "\x1b[48;2;1;2;3m", "\x1b[38;5;214m", "\x1b[48;5;12m", "\x1b[39m", "\x1b[49m"] {
        assert!(out.contains(want), "missing {want:?} in {out:?}");
    }
}

#[test]
fn sgr_colours_are_the_bytes_crossterm_writes() {
    let _switch = ColorSwitch::set(false);
    let mut colours = vec![
        RColor::Reset, RColor::Black, RColor::Red, RColor::Green, RColor::Yellow,
        RColor::Blue, RColor::Magenta, RColor::Cyan, RColor::Gray, RColor::DarkGray,
        RColor::LightRed, RColor::LightGreen, RColor::LightYellow, RColor::LightBlue,
        RColor::LightMagenta, RColor::LightCyan, RColor::White, RColor::Rgb(0, 128, 255),
    ];
    colours.extend((0..=255u8).map(RColor::Indexed));
    for colour in colours {
        let mut ours = Vec::new();
        write_sgr_color(&mut ours, 38, colour).unwrap();
        write_sgr_color(&mut ours, 48, colour).unwrap();
        let mut theirs = Vec::new();
        crossterm::queue!(
            theirs,
            crossterm::style::SetForegroundColor(to_crossterm_color(colour)),
            crossterm::style::SetBackgroundColor(to_crossterm_color(colour)),
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(ours).unwrap(),
            String::from_utf8(theirs).unwrap(),
            "{colour:?}"
        );
    }
}
