use crate::client::WindowContentStyles;
use crate::layout::{CellJson, CellRunJson, LayoutJson, RowRunsJson};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Terminal;

fn full_cell(text: &str, fg: &str, bg: &str) -> CellJson {
    CellJson {
        text: text.to_string(),
        fg: fg.to_string(),
        bg: bg.to_string(),
        bold: false,
        italic: false,
        underline: false,
        inverse: false,
        dim: false,
        blink: false,
        hidden: false,
        strikethrough: false,
    }
}

fn run(text: &str, fg: &str, bg: &str, width: u16) -> CellRunJson {
    CellRunJson {
        text: text.to_string(),
        fg: fg.to_string(),
        bg: bg.to_string(),
        flags: 0,
        width,
        link: None,
        ul: 0,
        ulc: None,
    }
}

fn leaf(
    active: bool,
    copy_mode: bool,
    cols: u16,
    content: Vec<Vec<CellJson>>,
    rows_v2: Vec<RowRunsJson>,
) -> LayoutJson {
    LayoutJson::Leaf {
        id: 0,
        rows: 1,
        cols,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: true,
        cursor_shape: 0,
        active,
        copy_mode,
        scroll_offset: 0,
        view_offset: 0,
        sel_start_row: None,
        sel_start_col: None,
        sel_end_row: None,
        sel_end_col: None,
        sel_mode: None,
        copy_cursor_row: None,
        copy_cursor_col: None,
        content,
        rows_v2,
        title: None,
    }
}

fn render_at(
    layout: &LayoutJson,
    width: u16,
    height: u16,
    styles: WindowContentStyles,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            let active_rect = crate::client::compute_active_rect_json(layout, area);
            crate::client::render_layout_json(
                frame,
                layout,
                area,
                false,
                Style::default().fg(Color::DarkGray),
                Style::default().fg(Color::Green),
                false,
                Color::Reset,
                active_rect,
                "",
                false,
                "off",
                "",
                1,
                crate::border_lines::border_chars("single"),
                None,
                styles,
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render(layout: &LayoutJson, width: u16, styles: WindowContentStyles) -> ratatui::buffer::Buffer {
    render_at(layout, width, 1, styles)
}

#[test]
fn active_window_style_defaults_full_cells_without_overriding_explicit_colors() {
    let layout = leaf(
        true,
        true,
        2,
        vec![vec![
            full_cell("A", "", ""),
            full_cell("B", "red", "magenta"),
        ]],
        Vec::new(),
    );
    let styles = WindowContentStyles {
        inactive: Some(Style::default().fg(Color::Cyan).bg(Color::White)),
        active: Some(Style::default().fg(Color::Yellow).bg(Color::Blue)),
        ..Default::default()
    };

    let buffer = render(&layout, 2, styles);
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Yellow));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Blue));
    assert_eq!(buffer[(1, 0)].style().fg, Some(Color::Red));
    assert_eq!(buffer[(1, 0)].style().bg, Some(Color::Magenta));
}

#[test]
fn inactive_window_style_defaults_row_runs_without_overriding_explicit_colors() {
    let layout = leaf(
        false,
        false,
        4,
        Vec::new(),
        vec![RowRunsJson {
            runs: vec![
                run("A", "", "", 1),
                run("B", "green", "cyan", 1),
                run("  ", "", "", 2),
            ],
        }],
    );
    let styles = WindowContentStyles {
        inactive: Some(Style::default().fg(Color::LightBlue).bg(Color::DarkGray)),
        active: Some(Style::default().fg(Color::Yellow).bg(Color::Blue)),
        ..Default::default()
    };

    let buffer = render(&layout, 4, styles);
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::LightBlue));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::DarkGray));
    assert_eq!(buffer[(1, 0)].style().fg, Some(Color::Green));
    assert_eq!(buffer[(1, 0)].style().bg, Some(Color::Cyan));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::DarkGray));
    assert_eq!(buffer[(3, 0)].style().bg, Some(Color::DarkGray));
}

#[test]
fn active_window_style_fills_unrepresented_pane_rows() {
    let layout = leaf(true, false, 3, Vec::new(), Vec::new());
    let styles = WindowContentStyles {
        inactive: Some(Style::default().fg(Color::Cyan).bg(Color::White)),
        active: Some(Style::default().fg(Color::Yellow).bg(Color::Blue)),
        ..Default::default()
    };

    let buffer = render_at(&layout, 3, 3, styles);
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(buffer[(x, y)].style().fg, Some(Color::Yellow));
            assert_eq!(buffer[(x, y)].style().bg, Some(Color::Blue));
        }
    }
}

#[test]
fn active_window_background_fills_columns_after_explicit_full_cell() {
    let layout = leaf(
        true,
        true,
        1,
        vec![vec![full_cell("A", "red", "magenta")]],
        Vec::new(),
    );
    let styles = WindowContentStyles {
        inactive: None,
        active: Some(Style::default().bg(Color::Blue)),
        ..Default::default()
    };

    let buffer = render(&layout, 3, styles);
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Magenta));
    assert_eq!(buffer[(1, 0)].style().bg, Some(Color::Blue));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Blue));
}

#[test]
fn inactive_window_background_fills_columns_after_explicit_row_run() {
    let layout = leaf(
        false,
        false,
        1,
        Vec::new(),
        vec![RowRunsJson {
            runs: vec![run("A", "green", "magenta", 1)],
        }],
    );
    let styles = WindowContentStyles {
        inactive: Some(Style::default().bg(Color::DarkGray)),
        active: None,
        ..Default::default()
    };

    let buffer = render(&layout, 3, styles);
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Magenta));
    assert_eq!(buffer[(1, 0)].style().bg, Some(Color::DarkGray));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::DarkGray));
}

#[test]
fn explicit_hidden_run_background_covers_the_full_run_width() {
    let layout = leaf(
        true,
        false,
        3,
        Vec::new(),
        vec![RowRunsJson {
            runs: vec![CellRunJson {
                text: "ABC".to_string(),
                fg: "red".to_string(),
                bg: "magenta".to_string(),
                flags: 64,
                width: 3,
                link: None,
                ul: 0,
                ulc: None,
            }],
        }],
    );
    let styles = WindowContentStyles {
        inactive: None,
        active: Some(Style::default().bg(Color::Blue)),
        ..Default::default()
    };

    let buffer = render(&layout, 3, styles);
    for x in 0..3 {
        assert_eq!(buffer[(x, 0)].symbol(), " ");
        assert_eq!(buffer[(x, 0)].style().fg, Some(Color::Red));
        assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Magenta));
    }
}

#[test]
fn clock_overlay_preserves_the_active_window_background() {
    let backend = TestBackend::new(40, 5);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            crate::client::render_clock_overlay(
                frame,
                Rect::new(0, 0, 40, 5),
                Color::Red,
                Some(Style::default().bg(Color::Blue)),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    for y in 0..5 {
        for x in 5..34 {
            assert_eq!(buffer[(x, y)].style().bg, Some(Color::Blue));
        }
    }
}
