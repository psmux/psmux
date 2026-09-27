// Issue #702: the copy-mode position indicator printed the scroll offset on
// both sides of the slash, so it always read `[n/n]` and said nothing about
// where in the history the view was.
//
// Renders a copy-mode leaf through the real client `render_layout_json` into a
// headless TestBackend and reads the indicator back off the drawn buffer, the
// same way tests-rs/test_copy_line_numbers_render.rs proves the gutter. No PTY
// and no pseudo-console, so it is deterministic on Windows CI.
//
// The expected pairs come from tmux 3.7c driven live (`tmux -L postest`, a
// 30-row pane, `seq 1 200`, history 173 rows):
//
//   at the live bottom          #{copy_position}/#{copy_position_limit} = 0/173
//   scrolled up 68 lines                                                = 68/173
//   the same view, gutter set to `absolute`                             = 106/203

use crate::client::CopyLnRender;
use crate::copy_line_numbers::CopyLnMode;
use crate::layout::{CellJson, LayoutJson};

fn cell(ch: char) -> CellJson {
    CellJson {
        text: ch.to_string(), fg: String::new(), bg: String::new(),
        bold: false, italic: false, underline: false, inverse: false,
        dim: false, blink: false, hidden: false, strikethrough: false,
    }
}

/// A copy-mode leaf `h` rows tall and `w` cols wide, scrolled up by `oy`.
fn copy_leaf(w: u16, h: u16, oy: usize) -> LayoutJson {
    let content: Vec<Vec<CellJson>> = (0..h).map(|_| (0..w).map(|_| cell('X')).collect()).collect();
    LayoutJson::Leaf {
        id: 0, rows: h, cols: w, cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true, cursor_shape: 0,
        active: true, copy_mode: true, scroll_offset: oy, view_offset: oy,
        sel_start_row: None, sel_start_col: None, sel_end_row: None, sel_end_col: None,
        sel_mode: None, copy_cursor_row: Some(0), copy_cursor_col: Some(0),
        content, rows_v2: Vec::new(), title: None,
    }
}

/// Render the leaf into a `w` by `h` buffer, giving the pane an area `pane_h`
/// rows tall, and return row 1 of the buffer, where the indicator sits. Row 0
/// carries the `[copy mode]` label.
fn render_row1(leaf: &LayoutJson, copy: Option<CopyLnRender>, w: u16, h: u16, pane_h: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 0, w, pane_h);
        let active_rect = crate::client::compute_active_rect_json(leaf, area);
        crate::client::render_layout_json(
            f, leaf, area, false,
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), copy,
            crate::client::WindowContentStyles::default(),
            crate::pane_border::PaneBorderIndicators::Colour,
        );
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let aw = buf.area.width as usize;
    (0..aw).map(|c| buf.content[aw + c].symbol().chars().next().unwrap_or(' ')).collect()
}

/// The `[n/m]` the indicator drew, or None when it drew nothing.
fn indicator_of(row: &str) -> Option<String> {
    let start = row.rfind('[')?;
    let end = row[start..].find(']')? + start;
    Some(row[start..=end].to_string())
}

fn config(mode: CopyLnMode, hsize: usize) -> Option<CopyLnRender> {
    use ratatui::style::{Color, Style};
    Some(CopyLnRender {
        mode, hsize,
        num_style: Style::default().fg(Color::DarkGray),
        cur_style: Style::default().fg(Color::Yellow),
    })
}

#[test]
fn issue702_the_limit_is_the_scrollback_size_not_the_offset_again() {
    let leaf = copy_leaf(60, 30, 68);
    let row = render_row1(&leaf, config(CopyLnMode::Off, 173), 60, 30, 30);
    assert_eq!(indicator_of(&row).as_deref(), Some("[68/173]"),
        "the indicator must read offset over scrollback size, row was {:?}", row);
}

#[test]
fn issue702_the_indicator_shows_at_the_live_bottom() {
    // Before the fix an offset of 0 drew nothing at all, so entering copy mode
    // told you nothing until you scrolled. tmux draws it from the first frame.
    let leaf = copy_leaf(60, 30, 0);
    let row = render_row1(&leaf, config(CopyLnMode::Off, 173), 60, 30, 30);
    assert_eq!(indicator_of(&row).as_deref(), Some("[0/173]"),
        "row was {:?}", row);
}

#[test]
fn issue702_an_absolute_gutter_switches_the_indicator_to_absolute_lines() {
    // tmux `window_copy_formats` reads the pair differently once the gutter
    // counts from the top of the history, so the two agree on screen.
    let leaf = copy_leaf(60, 30, 68);
    for mode in [CopyLnMode::Absolute, CopyLnMode::Relative, CopyLnMode::Hybrid] {
        let row = render_row1(&leaf, config(mode, 173), 60, 30, 30);
        assert_eq!(indicator_of(&row).as_deref(), Some("[106/203]"),
            "mode {:?} row was {:?}", mode, row);
    }
}

#[test]
fn issue702_a_default_gutter_keeps_the_offset_reading() {
    let leaf = copy_leaf(60, 30, 68);
    let row = render_row1(&leaf, config(CopyLnMode::Default, 173), 60, 30, 30);
    assert_eq!(indicator_of(&row).as_deref(), Some("[68/173]"), "row was {:?}", row);
}

#[test]
fn issue702_a_pane_too_narrow_for_the_indicator_is_left_alone() {
    // `[68/173]` is 8 columns and the guard needs 2 more, so 10 is the width
    // where it stops fitting.
    let leaf = copy_leaf(10, 30, 68);
    let row = render_row1(&leaf, config(CopyLnMode::Off, 173), 10, 30, 30);
    assert_eq!(indicator_of(&row), None, "row was {:?}", row);
}

#[test]
fn issue702_a_one_row_pane_does_not_draw_below_itself() {
    // The indicator sits one row under the `[copy mode]` label, which is off
    // the end of a single-row pane. Drawing it there would land in whichever
    // pane owns the next row, so a pane that short draws nothing.
    let leaf = copy_leaf(60, 1, 68);
    let row = render_row1(&leaf, config(CopyLnMode::Off, 173), 60, 2, 1);
    assert_eq!(indicator_of(&row), None, "row was {:?}", row);
    assert_eq!(row.trim(), "", "the row under the pane must stay untouched, was {:?}", row);
}

#[test]
fn issue702_a_caller_without_copy_mode_state_draws_a_zero_limit() {
    // The layout preview passes None. It has no scrollback size to report, so
    // the limit is 0 rather than a number invented from the offset.
    let leaf = copy_leaf(60, 30, 68);
    let row = render_row1(&leaf, None, 60, 30, 30);
    assert_eq!(indicator_of(&row).as_deref(), Some("[68/0]"), "row was {:?}", row);
}
