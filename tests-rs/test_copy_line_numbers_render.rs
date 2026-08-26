// Feature: copy-mode line numbers — deterministic render proof.
//
// Builds a single leaf in copy mode with known content, renders it through the
// real client `render_layout_json` into a headless TestBackend, and reads back
// the left gutter to assert the numbers match the selected mode. No PTY / no
// pseudo-console, so it is deterministic on Windows CI.

use crate::layout::{LayoutJson, CellJson};
use crate::client::CopyLnRender;
use crate::copy_line_numbers::CopyLnMode;

fn cell(ch: char) -> CellJson {
    CellJson {
        text: ch.to_string(), fg: String::new(), bg: String::new(),
        bold: false, italic: false, underline: false, inverse: false,
        dim: false, blink: false, hidden: false, strikethrough: false,
    }
}

/// A copy-mode leaf `h` rows tall, `w` cols wide, cursor at row `cy`, scrolled
/// up by `oy`, filled with 'X'.
fn copy_leaf(w: u16, h: u16, cy: u16, oy: usize) -> LayoutJson {
    let content: Vec<Vec<CellJson>> = (0..h).map(|_| (0..w).map(|_| cell('X')).collect()).collect();
    LayoutJson::Leaf {
        id: 0, rows: h, cols: w, cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true, cursor_shape: 0,
        active: true, copy_mode: true, scroll_offset: oy, view_offset: oy,
        sel_start_row: None, sel_start_col: None, sel_end_row: None, sel_end_col: None,
        sel_mode: None, copy_cursor_row: Some(cy), copy_cursor_col: Some(0),
        content, rows_v2: Vec::new(), title: None,
    }
}

/// Render and return the gutter text (leading `gw` columns) of each row.
fn render_gutters(leaf: &LayoutJson, mode: CopyLnMode, hsize: usize, w: u16, h: u16, gw: usize) -> Vec<String> {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    let copy_ln = Some(CopyLnRender {
        mode, hsize,
        num_style: Style::default().fg(Color::DarkGray),
        cur_style: Style::default().fg(Color::Yellow),
    });
    term.draw(|f| {
        let area = Rect::new(0, 0, w, h);
        let active_rect = crate::client::compute_active_rect_json(leaf, area);
        crate::client::render_layout_json(
            f, leaf, area, false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), copy_ln,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let aw = buf.area.width as usize;
    (0..h as usize).map(|r| {
        (0..gw).map(|c| buf.content[r * aw + c].symbol().chars().next().unwrap_or(' ')).collect::<String>()
    }).collect()
}

#[test]
fn relative_shows_distance_from_cursor() {
    let h = 10u16; let cy = 4u16;
    let leaf = copy_leaf(40, h, cy, 0);
    // width: hsize=0,height=10 -> 11 -> 2 digits -> min3 -> +1 = 4
    let gw = crate::copy_line_numbers::gutter_width(CopyLnMode::Relative, 0, h as usize);
    assert_eq!(gw, 4);
    let gutters = render_gutters(&leaf, CopyLnMode::Relative, 0, 40, h, gw);
    // cursor row shows 0; others show |r - cy|
    assert_eq!(gutters[4].trim(), "0", "cursor row must show 0, got {:?}", gutters[4]);
    assert_eq!(gutters[0].trim(), "4", "row 0 is 4 away from cursor row 4");
    assert_eq!(gutters[7].trim(), "3", "row 7 is 3 away from cursor row 4");
    // trailing space separator present
    assert!(gutters[4].ends_with(' '), "gutter must end with a space, got {:?}", gutters[4]);
}

#[test]
fn absolute_counts_from_history() {
    let h = 8u16;
    let leaf = copy_leaf(40, h, 0, 0);
    let hsize = 100usize;
    let gw = crate::copy_line_numbers::gutter_width(CopyLnMode::Absolute, hsize, h as usize);
    let gutters = render_gutters(&leaf, CopyLnMode::Absolute, hsize, 40, h, gw);
    // absolute = hsize - oy + py + 1; oy=0 -> row0=101, row3=104
    assert_eq!(gutters[0].trim(), "101", "got {:?}", gutters[0]);
    assert_eq!(gutters[3].trim(), "104", "got {:?}", gutters[3]);
}

#[test]
fn default_counts_from_top() {
    let h = 6u16;
    let leaf = copy_leaf(40, h, 3, 0);
    let gw = crate::copy_line_numbers::gutter_width(CopyLnMode::Default, 0, h as usize);
    let gutters = render_gutters(&leaf, CopyLnMode::Default, 0, 40, h, gw);
    // oy=0 -> number equals row index
    assert_eq!(gutters[0].trim(), "0");
    assert_eq!(gutters[5].trim(), "5");
}

#[test]
fn off_draws_no_gutter_and_keeps_content() {
    // With mode Off (copy_ln None), the first columns are content 'X', not numbers.
    let h = 6u16;
    let leaf = copy_leaf(40, h, 0, 0);
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;
    let backend = TestBackend::new(40, h);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 0, 40, h);
        let active_rect = crate::client::compute_active_rect_json(&leaf, area);
        crate::client::render_layout_json(
            f, &leaf, area, false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), None,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let aw = buf.area.width as usize;
    // First cell of first row should be content 'X' (no gutter shift).
    assert_eq!(buf.content[0].symbol().chars().next(), Some('X'), "off must not draw a gutter");
}

#[test]
fn gutter_shifts_content_right() {
    // With a gutter, content 'X' must start after the gutter width.
    let h = 6u16;
    let leaf = copy_leaf(40, h, 0, 0);
    let gw = crate::copy_line_numbers::gutter_width(CopyLnMode::Relative, 0, h as usize);
    let gutters_and_content = {
        use ratatui::backend::TestBackend;
        use ratatui::layout::Rect;
        use ratatui::style::{Color, Style};
        use ratatui::Terminal;
        let backend = TestBackend::new(40, h);
        let mut term = Terminal::new(backend).unwrap();
        let copy_ln = Some(CopyLnRender {
            mode: CopyLnMode::Relative, hsize: 0,
            num_style: Style::default().fg(Color::DarkGray),
            cur_style: Style::default().fg(Color::Yellow),
        });
        term.draw(|f| {
            let area = Rect::new(0, 0, 40, h);
            let active_rect = crate::client::compute_active_rect_json(&leaf, area);
            crate::client::render_layout_json(
                f, &leaf, area, false,
                ratatui::style::Style::default().fg(Color::DarkGray),
                ratatui::style::Style::default().fg(Color::Green),
                false, Color::Reset, active_rect, "", false, "off", "", 1,
                crate::border_lines::border_chars("single"), copy_ln,
                crate::client::WindowContentStyles::default(),
            );
        }).unwrap();
        let buf = term.backend().buffer().clone();
        let aw = buf.area.width as usize;
        buf.content[0 * aw + gw].symbol().chars().next()
    };
    assert_eq!(gutters_and_content, Some('X'), "content must begin right after the {}-col gutter", gw);
}
