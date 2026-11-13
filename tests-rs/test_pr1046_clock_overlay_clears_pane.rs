//! PR #1046: clock mode owns the whole pane. tmux draws clock mode on its own
//! screen and `window_clock_draw_screen` (window-clock.c) clears it first, so
//! none of the pane's output shows around the digits; when the pane is too
//! small for the big digits it writes the plain time centred. psmux cleared
//! only the rectangle under the digits and drew nothing in a small pane.

use crate::client::render_clock_overlay;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

/// Fill a 40x12 screen with 'X' (the pane's own output), draw the clock over
/// `area`, and return the screen rows.
fn draw(area: Rect) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(40, 12)).unwrap();
    term.draw(|f| {
        let full = f.area();
        let lines: Vec<Line> = (0..full.height).map(|_| Line::from("X".repeat(full.width as usize))).collect();
        f.render_widget(Paragraph::new(lines), full);
        render_clock_overlay(f, area, Color::Blue, None);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect())
        .collect()
}

fn cut(rows: &[String], area: Rect) -> Vec<String> {
    rows[area.y as usize..(area.y + area.height) as usize]
        .iter()
        .map(|r| r.chars().skip(area.x as usize).take(area.width as usize).collect())
        .collect()
}

#[test]
fn big_clock_clears_the_whole_pane() {
    let area = Rect::new(2, 1, 34, 10);
    let rows = draw(area);
    let pane = cut(&rows, area);
    for (i, r) in pane.iter().enumerate() {
        assert!(!r.contains('X'), "pane row {i} still shows pane output: [{r}]");
    }
    let drawn = pane.iter().filter(|r| r.contains('#')).count();
    assert_eq!(drawn, 5, "the big digits take 5 rows: {pane:#?}");
    // Outside the pane nothing is touched.
    assert_eq!(rows[0], "X".repeat(40));
    assert_eq!(rows[11], "X".repeat(40));
    assert!(rows[5].starts_with("XX") && rows[5].ends_with("XXXX"), "row 5: [{}]", rows[5]);
}

#[test]
fn small_pane_shows_the_plain_time_centred() {
    // 20x3 is too small for the big digits (27 columns, 5 rows).
    let area = Rect::new(5, 4, 20, 3);
    let rows = draw(area);
    let pane = cut(&rows, area);
    for (i, r) in pane.iter().enumerate() {
        assert!(!r.contains('X'), "pane row {i} still shows pane output: [{r}]");
    }
    assert_eq!(pane[0].trim(), "");
    assert_eq!(pane[2].trim(), "");
    let mid = &pane[1];
    let t = mid.trim();
    assert_eq!(t.len(), 8, "plain HH:MM:SS on the middle row: [{mid}]");
    assert!(t.chars().enumerate().all(|(i, c)| if i == 2 || i == 5 { c == ':' } else { c.is_ascii_digit() }), "[{t}]");
    assert_eq!(mid.find(t), Some(6), "centred: (20 - 8) / 2 = 6, row [{mid}]");
}

#[test]
fn pane_too_narrow_for_the_time_is_just_cleared() {
    let area = Rect::new(0, 0, 6, 2);
    let rows = draw(area);
    for r in cut(&rows, area) {
        assert_eq!(r.trim(), "", "a pane narrower than the time is blank: [{r}]");
    }
    assert_eq!(&rows[0][6..], &"X".repeat(34));
}
