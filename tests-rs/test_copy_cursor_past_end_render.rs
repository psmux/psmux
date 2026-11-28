// The copy cursor one past a full row. With emacs keys it can sit at column
// sx, past the pane, as in tmux 3.8; tmux then draws a `$` in the last cell
// and puts the terminal cursor on it ("to indicate the cursor is at the end
// of the line rather than the cell before", window_copy_write_line,
// window-copy.c:5487-5490). With line numbers on, the same holds for any
// column past the content the pane shows.
//
// Renders a copy mode leaf through the real client `render_layout_json` into
// a headless TestBackend. Registered from src/commands.rs.

use crate::client::CopyLnRender;
use crate::copy_line_numbers::CopyLnMode;
use crate::layout::{CellJson, LayoutJson};

const W: u16 = 20;
const H: u16 = 6;

fn cell(ch: char) -> CellJson {
    CellJson {
        text: ch.to_string(), fg: String::new(), bg: String::new(),
        bold: false, italic: false, underline: false, inverse: false,
        dim: false, blink: false, hidden: false, strikethrough: false, overline: false, link: None,
    }
}

/// A copy mode leaf whose row 3 is full of 'a', the copy cursor at `cursor`.
/// Rows 0 and 1 are left alone: the copy mode label and the position
/// indicator are drawn over their ends.
fn leaf(cursor: (u16, u16)) -> LayoutJson {
    let content: Vec<Vec<CellJson>> = (0..H)
        .map(|r| (0..W).map(|_| cell(if r == 3 { 'a' } else { ' ' })).collect())
        .collect();
    LayoutJson::Leaf {
        id: 0, rows: H, cols: W, cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true, cursor_shape: 0, cursor_colour: None, default_colours: None,
        active: true, copy_mode: true, scroll_offset: 0, view_offset: 0,
        sel_start_row: None, sel_start_col: None, sel_end_row: None, sel_end_col: None,
        sel_mode: None, copy_cursor_row: Some(cursor.0), copy_cursor_col: Some(cursor.1),
        content, rows_v2: Vec::new(), title: None, clock: false, copy_hl: Vec::new(), images: vec![],
    }
}

/// Row 3 as drawn, and where the terminal cursor went.
fn render(leaf: &LayoutJson, line_numbers: bool) -> (String, (u16, u16)) {
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    let mut term = Terminal::new(TestBackend::new(W, H)).unwrap();
    let copy_ln = line_numbers.then(|| CopyLnRender {
        mode: CopyLnMode::Absolute, hsize: 0, hide_position: false,
        num_style: Style::default().fg(Color::DarkGray),
        cur_style: Style::default().fg(Color::Yellow), hl: Default::default(),
    });
    term.draw(|f| {
        let area = Rect::new(0, 0, W, H);
        let active_rect = crate::client::compute_active_rect_json(leaf, area);
        crate::client::render_layout_json(
            f, leaf, area, false,
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), copy_ln,
            crate::client::WindowContentStyles::default(),
            crate::pane_border::PaneBorderIndicators::Colour,
        );
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let row: String = (0..W as usize).map(|c| buf.content[3 * W as usize + c].symbol().to_string()).collect();
    let pos = term.backend_mut().get_cursor_position().unwrap();
    (row, (pos.x, pos.y))
}

#[test]
fn a_cursor_one_past_a_full_row_draws_a_dollar_in_the_last_cell() {
    let (row, cursor) = render(&leaf((3, W)), false);
    assert_eq!(row, format!("{}$", "a".repeat(W as usize - 1)));
    assert_eq!(cursor, (W - 1, 3), "the terminal cursor sits on the $");
}

#[test]
fn a_cursor_on_the_last_cell_draws_no_dollar() {
    let (row, cursor) = render(&leaf((3, W - 1)), false);
    assert_eq!(row, "a".repeat(W as usize));
    assert_eq!(cursor, (W - 1, 3));
}

#[test]
fn with_line_numbers_a_cursor_past_the_content_draws_a_dollar() {
    // A gutter of `gw` columns leaves W - gw for the content, so the content
    // column W - gw is past what the pane shows, as in tmux.
    let gw = crate::copy_line_numbers::gutter_width(CopyLnMode::Absolute, 0, H as usize) as u16;
    let (row, cursor) = render(&leaf((3, W - gw)), true);
    assert!(row.ends_with('$'), "row 3 is [{row}]");
    assert_eq!(cursor, (W - 1, 3));
    let (row, _) = render(&leaf((3, W - gw - 1)), true);
    assert!(row.ends_with('a'), "the last content column draws no $, row 3 is [{row}]");
}
