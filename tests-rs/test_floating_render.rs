// Feature: floating panes (tmux new-pane) — deterministic render proof.
//
// Builds a FloatJson and renders it through the real client
// `render_float_overlays` into a headless TestBackend, then asserts the border
// glyphs land at the float's position and the pane content is drawn inside.
// Ground truth for a client-rendered overlay (capture-pane cannot see it).

use crate::client::{
    FloatJson, WindowContentStyles, render_float_overlays,
};
use crate::layout::{RowRunsJson, CellRunJson};

fn row_of(ch: char, n: usize) -> RowRunsJson {
    RowRunsJson {
        runs: (0..n).map(|_| CellRunJson {
            text: ch.to_string(), fg: String::new(), bg: String::new(),
            flags: 0, width: 1, link: None, ul: 0, ulc: None,
        }).collect(),
    }
}

fn render_with_styles(
    fl: &FloatJson,
    cw: u16,
    ch: u16,
    styles: WindowContentStyles,
) -> ratatui::buffer::Buffer {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    let backend = TestBackend::new(cw, ch);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        render_float_overlays(
            f,
            Rect::new(0, 0, cw, ch),
            std::slice::from_ref(fl),
            styles,
        );
    }).unwrap();
    term.backend().buffer().clone()
}

fn render(fl: &FloatJson, cw: u16, ch: u16) -> ratatui::buffer::Buffer {
    render_with_styles(fl, cw, ch, WindowContentStyles::default())
}

fn cell_char(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> char {
    let w = buf.area.width as usize;
    buf.content[(y as usize) * w + (x as usize)].symbol().chars().next().unwrap_or(' ')
}

fn make_float(x: u16, y: u16, w: u16, h: u16, border: &str) -> FloatJson {
    // inner is (w-2) x (h-2), filled with 'Z'.
    let rows: Vec<RowRunsJson> = (0..h.saturating_sub(2)).map(|_| row_of('Z', (w.saturating_sub(2)) as usize)).collect();
    FloatJson {
        x, y, w, h, border: border.to_string(), focused: true, title: String::new(), rows,
    }
}

#[test]
fn single_border_and_content_positioned() {
    let fl = make_float(5, 3, 20, 6, "single");
    let buf = render(&fl, 80, 24);
    // Top-left corner of the box at (x=5,y=3).
    assert_eq!(cell_char(&buf, 5, 3), '┌', "single top-left corner at float origin");
    assert_eq!(cell_char(&buf, 24, 3), '┐', "single top-right corner");
    assert_eq!(cell_char(&buf, 5, 8), '└', "single bottom-left corner");
    // Content 'Z' sits just inside the border.
    assert_eq!(cell_char(&buf, 6, 4), 'Z', "content drawn inside the border");
    // Outside the float is untouched (space).
    assert_eq!(cell_char(&buf, 0, 0), ' ');
}

#[test]
fn double_border_uses_double_glyphs() {
    let fl = make_float(2, 2, 16, 6, "double");
    let buf = render(&fl, 60, 20);
    assert_eq!(cell_char(&buf, 2, 2), '╔', "double top-left corner");
    assert_eq!(cell_char(&buf, 17, 2), '╗', "double top-right corner");
    assert_eq!(cell_char(&buf, 3, 3), 'Z', "content inside double border");
}

#[test]
fn heavy_border_uses_thick_glyphs() {
    let fl = make_float(0, 0, 12, 5, "heavy");
    let buf = render(&fl, 40, 12);
    assert_eq!(cell_char(&buf, 0, 0), '┏', "heavy top-left corner");
}

#[test]
fn none_border_draws_content_without_box() {
    let fl = make_float(4, 2, 14, 5, "none");
    let buf = render(&fl, 60, 20);
    // No corner glyph at the origin; content starts at the origin instead.
    assert_eq!(cell_char(&buf, 4, 2), 'Z', "none: content at origin, no border");
    for bad in ['┌', '╔', '┏'] {
        let found = buf.content.iter().any(|c| c.symbol().chars().next() == Some(bad));
        assert!(!found, "none border must draw no box glyph {:?}", bad);
    }
}

#[test]
fn floating_panes_apply_active_and_inactive_window_colors() {
    use ratatui::style::{Color, Style};

    let window_styles = WindowContentStyles {
        inactive: Some(
            Style::default()
                .fg(Color::Cyan)
                .bg(Color::DarkGray),
        ),
        active: Some(
            Style::default()
                .fg(Color::Yellow)
                .bg(Color::Blue),
        ),
    };
    let active = make_float(1, 1, 8, 5, "single");
    let active_buffer = render_with_styles(&active, 20, 10, window_styles);
    assert_eq!(
        active_buffer[(2, 2)].style().fg,
        Some(Color::Yellow),
    );
    assert_eq!(
        active_buffer[(2, 2)].style().bg,
        Some(Color::Blue),
    );

    let mut inactive = active;
    inactive.focused = false;
    inactive.rows[0].runs[0].bg = "magenta".to_string();
    let inactive_buffer = render_with_styles(&inactive, 20, 10, window_styles);
    assert_eq!(
        inactive_buffer[(2, 2)].style().bg,
        Some(Color::Magenta),
    );
    assert_eq!(
        inactive_buffer[(3, 2)].style().fg,
        Some(Color::Cyan),
    );
    assert_eq!(
        inactive_buffer[(3, 2)].style().bg,
        Some(Color::DarkGray),
    );
}

#[test]
fn focused_float_leaves_tiled_content_with_inactive_window_style() {
    use crate::client::{
        compute_active_rect_json, render_layout_json,
    };
    use crate::layout::LayoutJson;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    let layout = LayoutJson::Leaf {
        id: 0,
        rows: 1,
        cols: 12,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: true,
        cursor_shape: 0,
        active: true,
        copy_mode: false,
        scroll_offset: 0,
        view_offset: 0,
        sel_start_row: None,
        sel_start_col: None,
        sel_end_row: None,
        sel_end_col: None,
        sel_mode: None,
        copy_cursor_row: None,
        copy_cursor_col: None,
        content: Vec::new(),
        rows_v2: Vec::new(),
        title: None,
    };
    let window_styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::DarkGray)),
        active: Some(Style::default().bg(Color::Blue)),
    };
    let floating = make_float(2, 1, 6, 4, "single");
    let backend = TestBackend::new(12, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 12, 6);
            render_layout_json(
                frame,
                &layout,
                area,
                false,
                Color::DarkGray,
                Color::Green,
                false,
                Color::Reset,
                compute_active_rect_json(&layout, area),
                "",
                false,
                "off",
                "",
                1,
                crate::border_lines::border_chars("single"),
                None,
                window_styles.for_tiled_panes(true),
            );
            render_float_overlays(
                frame,
                area,
                std::slice::from_ref(&floating),
                window_styles,
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::DarkGray));
    assert_eq!(buffer[(3, 2)].style().bg, Some(Color::Blue));
}
