//! Regression tests for issue #619 items 1 and 3.
//!
//! Item 1: tmux `tty.c` `tty_default_colours` falls back from
//! `window-active-style` to `window-style` per attribute. The active pane only
//! takes fg (or bg) from `window-active-style` when that style actually names
//! it, otherwise fg (or bg) comes from `window-style`. The two attributes fall
//! back independently, so `window-active-style fg=red` combined with
//! `window-style bg=blue` paints the active pane red on blue.
//!
//! Item 3: hidden runs (flag 64) must emit one space per column of the run,
//! because the column accounting advances by the full run width. A single
//! space painted one column while claiming `run_w`, which shifted every later
//! run on the row to the left.

use crate::client::WindowContentStyles;
use crate::layout::CellRunJson;
use ratatui::style::{Color, Style};

fn run(text: &str, fg: &str, bg: &str, flags: u8, width: u16) -> CellRunJson {
    CellRunJson {
        text: text.to_string(),
        fg: fg.to_string(),
        bg: bg.to_string(),
        flags,
        width,
        link: None,
        ul: 0,
        ulc: None,
    }
}

fn line_text(line: &ratatui::text::Line<'static>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

// ── Item 1: window-active-style falls back to window-style per attribute ──

#[test]
fn active_pane_falls_back_to_window_style_when_active_style_is_unset() {
    // tmux: `set -g window-style bg=colour52` alone tints every pane including
    // the active one, because cached_active_gc.bg stays 8 (unnamed).
    let styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::Indexed(52))),
        active: None,
        ..Default::default()
    };
    let resolved = styles.for_pane(true).expect("active pane inherits window-style");
    assert_eq!(resolved.bg, Some(Color::Indexed(52)));
}

#[test]
fn active_pane_falls_back_per_attribute_not_as_a_whole_style() {
    // window-active-style names only fg, window-style names only bg:
    // tty_default_colours picks fg from the active style and bg from the
    // inactive one, because the two branches are independent.
    let styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::Blue)),
        active: Some(Style::default().fg(Color::Red)),
        ..Default::default()
    };
    let resolved = styles.for_pane(true).expect("merged style");
    assert_eq!(resolved.fg, Some(Color::Red));
    assert_eq!(resolved.bg, Some(Color::Blue));
}

#[test]
fn active_style_default_colour_falls_back_like_tmux_colour_eight() {
    // `window-active-style bg=default` parses to Color::Reset, which is tmux's
    // colour 8 (COLOUR_DEFAULT). tty_default_colours treats that as unnamed and
    // falls back to window-style.
    let styles = WindowContentStyles {
        inactive: Some(Style::default().fg(Color::Green).bg(Color::Indexed(52))),
        active: Some(Style::default().bg(Color::Reset)),
        ..Default::default()
    };
    let resolved = styles.for_pane(true).expect("merged style");
    assert_eq!(resolved.bg, Some(Color::Indexed(52)));
    assert_eq!(resolved.fg, Some(Color::Green));
}

#[test]
fn active_style_wins_over_window_style_when_it_names_the_attribute() {
    let styles = WindowContentStyles {
        inactive: Some(Style::default().fg(Color::Green).bg(Color::Indexed(52))),
        active: Some(Style::default().fg(Color::Yellow).bg(Color::Magenta)),
        ..Default::default()
    };
    let resolved = styles.for_pane(true).expect("merged style");
    assert_eq!(resolved.fg, Some(Color::Yellow));
    assert_eq!(resolved.bg, Some(Color::Magenta));
}

#[test]
fn inactive_pane_never_reads_the_active_style() {
    let styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::Indexed(52))),
        active: Some(Style::default().fg(Color::Yellow).bg(Color::Magenta)),
        ..Default::default()
    };
    let resolved = styles.for_pane(false).expect("inactive style");
    assert_eq!(resolved.bg, Some(Color::Indexed(52)));
    assert_eq!(resolved.fg, None);
}

#[test]
fn no_styles_at_all_stays_none() {
    let styles = WindowContentStyles { inactive: None, active: None, ..Default::default() };
    assert!(styles.for_pane(true).is_none());
    assert!(styles.for_pane(false).is_none());
}

#[test]
fn legacy_renderer_agrees_with_the_live_client_on_the_fallback() {
    // src/rendering.rs picks the same effective style through the shared
    // helper, so both renderers stay in step.
    let inactive = Some(Style::default().bg(Color::Indexed(52)));
    let active = Some(Style::default().fg(Color::Red));
    let live = WindowContentStyles { inactive, active, ..Default::default() }.for_pane(true);
    let legacy = crate::style::active_window_style_with_fallback(active, inactive);
    assert_eq!(live.map(|s| (s.fg, s.bg)), legacy.map(|s| (s.fg, s.bg)));
    assert_eq!(legacy.and_then(|s| s.bg), Some(Color::Indexed(52)));
    assert_eq!(legacy.and_then(|s| s.fg), Some(Color::Red));
}

// ── Item 3: hidden runs must paint their full width ──

#[test]
fn preview_hidden_run_keeps_the_following_text_on_its_column() {
    // A three column hidden run followed by "XY". Before the fix the hidden run
    // emitted one space while claiming three columns, so the row rendered
    // " XY  " instead of "   XY".
    let runs = vec![run("ABC", "red", "", 64, 3), run("XY", "green", "", 0, 2)];
    let line = crate::preview::render_runs_line(&runs, 5);
    assert_eq!(line_text(&line), "   XY");
}

#[test]
fn preview_hidden_run_alone_fills_its_width() {
    let runs = vec![run("ABCD", "red", "", 64, 4)];
    let line = crate::preview::render_runs_line(&runs, 4);
    assert_eq!(line_text(&line), "    ");
}

#[test]
fn popup_hidden_run_keeps_the_following_text_on_its_column() {
    let runs = vec![run("ABC", "red", "", 64, 3), run("XY", "green", "", 0, 2)];
    let line = crate::client::render_popup_runs_line(&runs, 5);
    assert_eq!(line_text(&line), "   XY");
}

#[test]
fn popup_hidden_run_truncates_at_the_popup_edge() {
    let runs = vec![run("ABCD", "red", "", 64, 4)];
    let line = crate::client::render_popup_runs_line(&runs, 2);
    assert_eq!(line_text(&line), "  ");
}

// ── Item 4: dim=N in the window styles (tmux next-3.8) ──

#[test]
fn window_style_dim_percentage_is_parsed() {
    assert_eq!(crate::style::parse_style_dim("bg=colour52"), 0);
    assert_eq!(crate::style::parse_style_dim("bg=colour52,dim=40"), 40);
    assert_eq!(crate::style::parse_style_dim("dim=40%"), 40);
    assert_eq!(crate::style::parse_style_dim("DIM=100"), 100);
    // tmux strtonum rejects out of range and non numeric values.
    assert_eq!(crate::style::parse_style_dim("dim=101"), 0);
    assert_eq!(crate::style::parse_style_dim("dim=abc"), 0);
    // Bare `dim` stays the boolean SGR 2 attribute, not a percentage.
    assert_eq!(crate::style::parse_style_dim("dim"), 0);
}

#[test]
fn dim_percentage_scales_colours_like_tmux_colour_dim() {
    use crate::style::dim_colour_percent;
    // colour 196 is 0xff0000, halved is 0x7f0000 (255 * 50 / 100 = 127).
    assert_eq!(dim_colour_percent(Color::Indexed(196), 50), Color::Rgb(127, 0, 0));
    // The terminal default is left alone (tmux COLOUR_DEFAULT).
    assert_eq!(dim_colour_percent(Color::Reset, 50), Color::Reset);
    // dim=0 is a no op, dim of 100 or more is black.
    assert_eq!(dim_colour_percent(Color::Indexed(196), 0), Color::Indexed(196));
    assert_eq!(dim_colour_percent(Color::Indexed(196), 100), Color::Rgb(0, 0, 0));
    // Named colours resolve through the palette first (red is index 1, 0x800000).
    assert_eq!(dim_colour_percent(Color::Red, 50), Color::Rgb(64, 0, 0));
}

#[test]
fn xterm_palette_matches_the_tmux_table() {
    use crate::style::colour_256_to_rgb;
    assert_eq!(colour_256_to_rgb(0), (0x00, 0x00, 0x00));
    assert_eq!(colour_256_to_rgb(7), (0xc0, 0xc0, 0xc0));
    assert_eq!(colour_256_to_rgb(15), (0xff, 0xff, 0xff));
    assert_eq!(colour_256_to_rgb(16), (0x00, 0x00, 0x00));
    assert_eq!(colour_256_to_rgb(17), (0x00, 0x00, 0x5f));
    assert_eq!(colour_256_to_rgb(52), (0x5f, 0x00, 0x00));
    assert_eq!(colour_256_to_rgb(231), (0xff, 0xff, 0xff));
    assert_eq!(colour_256_to_rgb(232), (0x08, 0x08, 0x08));
    assert_eq!(colour_256_to_rgb(255), (0xee, 0xee, 0xee));
}

#[test]
fn dim_is_picked_per_pane_with_no_fallback_between_the_two_styles() {
    // tmux tty_default_colours takes cached_active_dim for the active pane
    // outright: unlike fg and bg, dim does not fall back to window-style.
    let styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::Indexed(52))),
        active: None,
        inactive_dim: 40,
        active_dim: 0,
    };
    assert_eq!(styles.dim_for_pane(false), 40);
    assert_eq!(styles.dim_for_pane(true), 0);
    // The colour still falls back even though the dim does not.
    assert_eq!(styles.for_pane(true).and_then(|s| s.bg), Some(Color::Indexed(52)));
}

#[test]
fn dim_reaches_the_rendered_pane_background() {
    use crate::layout::{LayoutJson, RowRunsJson};
    let layout = LayoutJson::Leaf {
        id: 0, rows: 1, cols: 2, cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true,
        cursor_shape: 0, active: true, copy_mode: false, scroll_offset: 0,
        view_offset: 0, sel_start_row: None, sel_start_col: None,
        sel_end_row: None, sel_end_col: None, sel_mode: None,
        copy_cursor_row: None, copy_cursor_col: None,
        content: Vec::new(),
        rows_v2: vec![RowRunsJson { runs: vec![run("AB", "", "", 0, 2)] }],
        title: None,
    };
    let styles = WindowContentStyles {
        inactive: None,
        active: Some(Style::default().bg(Color::Indexed(196))),
        inactive_dim: 0,
        active_dim: 50,
    };
    let backend = ratatui::backend::TestBackend::new(2, 1);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| {
        let area = ratatui::layout::Rect::new(0, 0, 2, 1);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            frame, &layout, area, false,
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Green),
            false,
            Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), None, styles,
        );
    }).unwrap();
    let buffer = terminal.backend().buffer().clone();
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Rgb(127, 0, 0)));
}

#[test]
fn parse_style_dim_survives_a_multibyte_part_that_straddles_byte_four() {
    // "aéé" is 5 bytes and byte 4 is inside the second "é". A byte slice at
    // index 4 panics; the parser must test the prefix by chars instead.
    assert_eq!(crate::style::parse_style_dim("fg=red,aéé"), 0);
    assert_eq!(crate::style::parse_style_dim("dim=40,aéé"), 40);
    assert_eq!(crate::style::parse_style_dim("DIM=25%"), 25);
    assert_eq!(crate::style::parse_style_dim("dim=101"), 0);
    assert_eq!(crate::style::parse_style_dim("dim"), 0);
}
