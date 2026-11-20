//! PR #1069: a window tab whose style says `fg=default` or `bg=default` keeps
//! the status bar's colour (tmux style_parse resolves `default` against the
//! status line cell, style.c:221-233, format-draw.c:828), and an inline
//! `#[bg=default]` inside the window format means the tab's own colour
//! (the default status-format puts #[push-default] after the window style).

use super::*;

fn resolved(window_style: &str, bar_fg: Color, bar_bg: Color) -> Style {
    let base = Style::default().fg(bar_fg).bg(bar_bg);
    resolve_tab_default_colours(base.patch(parse_tmux_style(window_style)), bar_fg, bar_bg)
}

#[test]
fn solarized_tab_keeps_the_bar_background() {
    let s = resolved("fg=colour244,bg=default", Color::Indexed(136), Color::Indexed(235));
    assert_eq!(s.bg, Some(Color::Indexed(235)));
    assert_eq!(s.fg, Some(Color::Indexed(244)));
}

#[test]
fn fg_default_takes_the_bar_foreground() {
    let s = resolved("fg=default,bg=red", Color::Yellow, Color::Green);
    assert_eq!(s.fg, Some(Color::Yellow));
    assert_eq!(s.bg, Some(Color::Red));
}

#[test]
fn a_named_colour_still_wins() {
    let s = resolved("fg=yellow,bg=blue", Color::Black, Color::Green);
    assert_eq!((s.fg, s.bg), (Some(Color::Yellow), Some(Color::Blue)));
}

#[test]
fn a_terminal_default_bar_stays_terminal_default() {
    // status-style bg=default: the bar itself is Reset, so the tab is too.
    let s = resolved("fg=yellow,bg=default", Color::Red, Color::Reset);
    assert_eq!(s.bg, Some(Color::Reset));
    assert_eq!(s.fg, Some(Color::Yellow));
}

#[test]
fn inline_bg_default_in_the_window_format_means_the_tab_colour() {
    let tab = Style::default().fg(Color::Yellow).bg(Color::Blue);
    let spans = parse_inline_styles("#[fg=red,bg=default]WWW", tab);
    let w = spans.iter().find(|s| s.content == "WWW").expect("WWW span");
    assert_eq!(w.style.fg, Some(Color::Red));
    assert_eq!(w.style.bg, Some(Color::Blue));
}

#[test]
fn inline_fg_default_means_the_base_foreground() {
    let tab = Style::default().fg(Color::Yellow).bg(Color::Blue);
    let spans = parse_inline_styles("#[fg=red]A#[fg=default]B", tab);
    let b = spans.iter().find(|s| s.content == "B").expect("B span");
    assert_eq!(b.style.fg, Some(Color::Yellow));
}
