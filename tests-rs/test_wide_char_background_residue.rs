// A row of Hangul on a coloured background, rewritten without a background
// and with the wide characters one column earlier, left the old background in
// the columns where a new blank lands on the right half of an old wide
// character, until a full redraw.  The host model below keeps per column only
// what this needs: printing a cell paints its background over the columns the
// symbol covers and nothing else, so a half overwritten wide character keeps
// its colour on the other half.

use super::*;
use ratatui::buffer::{Buffer, Cell, CellWidth};
use ratatui::widgets::{Paragraph, Widget};

const WIDTH: u16 = 52;
const GREEN: Color = Color::Indexed(22);
const OLD: &str = "+ 건을리뷰로찾았고수정은세레나가했습니다건을리뷰로찾";

fn frame(text: &str, style: Style, fixed: bool) -> Buffer {
    let area = Rect::new(0, 0, WIDTH, 1);
    let mut buf = Buffer::empty(area);
    let padding = " ".repeat(WIDTH as usize - unicode_width::UnicodeWidthStr::width(text));
    Paragraph::new(Line::from(Span::styled(format!("{text}{padding}"), style)))
        .render(area, &mut buf);
    if fixed {
        style_wide_trailing_cells(&mut buf);
    }
    buf
}

fn paint(screen: &mut [Color], updates: Vec<(u16, u16, &Cell)>) {
    for (column, _, cell) in updates {
        let end = (column as usize + (cell.cell_width() as usize).max(1)).min(screen.len());
        screen[column as usize..end].fill(cell.bg);
    }
}

fn green_columns(new: &str, fixed: bool) -> Vec<usize> {
    let blank = Buffer::empty(Rect::new(0, 0, WIDTH, 1));
    let first = frame(
        OLD,
        Style::default().fg(Color::Indexed(231)).bg(GREEN),
        fixed,
    );
    let second = frame(new, Style::default(), fixed);
    let mut screen = vec![Color::Reset; WIDTH as usize];
    paint(&mut screen, blank.diff(&first));
    paint(&mut screen, first.diff(&second));
    screen
        .iter()
        .enumerate()
        .filter(|(_, color)| **color == GREEN)
        .map(|(column, _)| column)
        .collect()
}

#[test]
fn model_reproduces_the_residue_without_the_fix() {
    assert_eq!(
        green_columns("가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나", false)[..3],
        [5, 11, 17]
    );
    assert_eq!(
        green_columns("+가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나", false)[..3],
        [3, 9, 15]
    );
    assert!(green_columns("ab cd ef gh ij kl mn op qr st uv wx yz ab cd ef gh", false).is_empty());
}

#[test]
fn hangul_over_hangul_leaves_no_background() {
    assert!(green_columns("가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나", true).is_empty());
}

#[test]
fn hangul_shifted_by_one_leaves_no_background() {
    assert!(green_columns("+가 나 다 라 마 바 사 아 자 차 카 타 파 하 가 나", true).is_empty());
}

#[test]
fn ascii_over_hangul_leaves_no_background() {
    assert!(green_columns("ab cd ef gh ij kl mn op qr st uv wx yz ab cd ef gh", true).is_empty());
}

#[test]
fn unchanged_frame_writes_nothing() {
    let first = frame(OLD, Style::default().bg(GREEN), true);
    assert!(first.diff(&first.clone()).is_empty());
}
