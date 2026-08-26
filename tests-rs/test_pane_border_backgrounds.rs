use super::*;
use crate::layout::LayoutJson;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;

fn leaf(id: usize, active: bool) -> LayoutJson {
    LayoutJson::Leaf {
        id,
        rows: 6,
        cols: 4,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: true,
        cursor_shape: 0,
        active,
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
    }
}

#[test]
fn pane_border_colors_keep_colors_and_ignore_attributes() {
    let style = parse_pane_border_colors(
        Some("fg=yellow,bg=blue,bold,underscore"),
        Style::default().fg(Color::Green),
    );
    assert_eq!(style.fg, Some(Color::Yellow));
    assert_eq!(style.bg, Some(Color::Blue));
    assert!(style.add_modifier.is_empty());
    assert!(style.sub_modifier.is_empty());

    let bg_only = parse_pane_border_colors(
        Some("bg=red"),
        Style::default().fg(Color::Green),
    );
    assert_eq!(
        bg_only.fg,
        Some(Color::Reset),
        "an explicit replacement must reset an omitted fg",
    );
    assert_eq!(bg_only.bg, Some(Color::Red));
}

#[test]
fn default_hover_style_preserves_the_border_background() {
    let mut cell = ratatui::buffer::Cell::default();
    cell.set_style(Style::default().fg(Color::Green).bg(Color::Blue));
    assert_eq!(
        cell.style().bg,
        Some(Color::Blue),
        "precondition: the border cell has a configured background",
    );

    cell.set_style(parse_pane_border_hover_style(Some("fg=yellow")));

    assert_eq!(cell.style().fg, Some(Color::Yellow));
    assert_eq!(cell.style().bg, Some(Color::Blue));
}

#[test]
fn two_pane_separator_and_labels_preserve_active_and_inactive_backgrounds() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let backend = TestBackend::new(10, 6);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 10, 6);
            let active_rect = compute_active_rect_json(&layout, area);
            render_layout_json(
                frame,
                &layout,
                area,
                false,
                inactive_style,
                active_style,
                false,
                Color::Reset,
                active_rect,
                "",
                false,
                "top",
                "#P",
                2,
                crate::border_lines::border_chars("single"),
                None,
                WindowContentStyles::default(),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let separator_x = (0..10)
        .find(|&x| buffer[(x, 0)].symbol() == "│")
        .expect("vertical separator");
    for y in 0..3 {
        assert_eq!(buffer[(separator_x, y)].style().bg, Some(Color::Red));
    }
    for y in 3..6 {
        assert_eq!(buffer[(separator_x, y)].style().bg, Some(Color::Blue));
    }

    let active_label = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "0")
        .expect("active label");
    let inactive_label = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "1")
        .expect("inactive label");
    assert_eq!(active_label.style().bg, Some(Color::Red));
    assert_eq!(inactive_label.style().bg, Some(Color::Blue));
    assert!(!active_label.style().add_modifier.contains(Modifier::BOLD));
}

#[test]
fn preview_preserves_active_and_inactive_border_backgrounds() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let backend = TestBackend::new(10, 6);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            crate::preview::render_dump_tree(
                frame,
                &layout,
                Rect::new(0, 0, 10, 6),
                inactive_style,
                active_style,
                None,
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let separator_x = (0..10)
        .find(|&x| buffer[(x, 0)].symbol() == "│")
        .expect("preview separator");
    assert_eq!(buffer[(separator_x, 0)].style().bg, Some(Color::Red));
    assert_eq!(buffer[(separator_x, 5)].style().bg, Some(Color::Blue));
}

fn nested_layout() -> LayoutJson {
    LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            leaf(0, false),
            LayoutJson::Split {
                kind: "Vertical".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(1, true), leaf(2, false)],
            },
        ],
    }
}

fn nested_junction_index(borders: &crate::rendering::BorderGeometry) -> usize {
    let junction = 3 * 12 + 5;
    assert!(borders.contains(&junction), "junction cell must be in the border geometry");
    assert!(borders.contains(&(junction - 12)), "vertical border must reach junction");
    assert!(borders.contains(&(junction + 1)), "horizontal border must leave junction");
    junction
}

fn render_border_layout(
    layout: &LayoutJson,
    width: u16,
    height: u16,
    inactive_style: Style,
    active_style: Style,
    line_mode: &str,
) -> (
    ratatui::buffer::Buffer,
    crate::rendering::BorderGeometry,
    Vec<usize>,
) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut borders = crate::rendering::BorderGeometry::default();
    let mut junctions = Vec::new();
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, width, height);
            let active_rect = compute_active_rect_json(layout, area);
            let chars = crate::border_lines::border_chars(line_mode);
            render_layout_json(
                frame,
                layout,
                area,
                false,
                inactive_style,
                active_style,
                false,
                Color::Reset,
                active_rect,
                "",
                false,
                "off",
                "",
                layout.count_leaves(),
                chars,
                None,
                WindowContentStyles::default(),
            );
            borders = border_geometry_from_layout(
                layout,
                area,
                frame.buffer_mut().area,
                false,
            );
            junctions = crate::rendering::fix_border_intersections(
                frame.buffer_mut(),
                chars,
                &borders,
            );
            recolor_border_junctions(
                frame.buffer_mut(),
                &junctions,
                active_rect,
                inactive_style,
                active_style,
            );
        })
        .unwrap();
    (terminal.backend().buffer().clone(), borders, junctions)
}

#[test]
fn nested_junction_background_tracks_active_pane_for_every_line_mode() {
    let layout = nested_layout();
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);

    for mode in ["single", "double", "heavy", "simple", "spaces"] {
        let (buffer, borders, _) = render_border_layout(
            &layout,
            12,
            8,
            inactive_style,
            active_style,
            mode,
        );
        let junction = nested_junction_index(&borders);
        assert_eq!(
            buffer.content[junction].style().bg,
            Some(Color::Red),
            "mode {mode}",
        );
    }
}

#[test]
fn collapsed_nested_layout_preserves_oriented_junction_and_corner_style() {
    let layout = nested_layout();
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let (buffer, _, _) = render_border_layout(
        &layout,
        12,
        2,
        inactive_style,
        active_style,
        "single",
    );
    assert_eq!(buffer[(5, 0)].symbol(), "├");
    assert_eq!(buffer[(5, 0)].style().bg, Some(Color::Red));
}

#[test]
fn adjacent_parallel_separators_remain_straight() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            leaf(0, true),
            LayoutJson::Split {
                kind: "Horizontal".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(1, false), leaf(2, false)],
            },
        ],
    };
    let (buffer, _, junctions) = render_border_layout(
        &layout,
        5,
        5,
        Style::default(),
        Style::default(),
        "single",
    );
    assert!(junctions.is_empty());
    for y in 0..5 {
        assert_eq!(buffer[(2, y)].symbol(), "│");
        assert_eq!(buffer[(3, y)].symbol(), "│");
    }
}

#[test]
fn parent_separator_overwrites_collapsed_child_orientation() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            LayoutJson::Split {
                kind: "Vertical".to_string(),
                sizes: vec![50, 50],
                children: vec![
                    LayoutJson::Split {
                        kind: "Vertical".to_string(),
                        sizes: vec![50, 50],
                        children: vec![
                            leaf(0, true),
                            LayoutJson::Split {
                                kind: "Vertical".to_string(),
                                sizes: vec![50, 50],
                                children: vec![leaf(1, false), leaf(2, false)],
                            },
                        ],
                    },
                    LayoutJson::Split {
                        kind: "Horizontal".to_string(),
                        sizes: vec![50, 50],
                        children: vec![leaf(3, false), leaf(4, false)],
                    },
                ],
            },
            leaf(5, false),
        ],
    };
    let (buffer, _, junctions) = render_border_layout(
        &layout,
        4,
        2,
        Style::default(),
        Style::default(),
        "single",
    );

    assert!(!junctions.contains(&5));
    assert_eq!(buffer[(1, 1)].symbol(), "│");
}

#[test]
fn nested_preview_junction_background_tracks_active_pane() {
    let layout = nested_layout();
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let backend = TestBackend::new(12, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            crate::preview::render_dump_tree(
                frame,
                &layout,
                Rect::new(0, 0, 12, 8),
                inactive_style,
                active_style,
                None,
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let borders = border_geometry_from_layout(
        &layout,
        Rect::new(0, 0, 12, 8),
        buffer.area,
        false,
    );
    let junction = nested_junction_index(&borders);
    assert_eq!(buffer.content[junction].style().bg, Some(Color::Red));
}

#[test]
fn no_border_mode_does_not_paint_junction_backgrounds() {
    let layout = nested_layout();
    let active_style = Style::default().fg(Color::Green).bg(Color::Red);
    let inactive_style = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let (buffer, _, junctions) = render_border_layout(
        &layout,
        12,
        8,
        inactive_style,
        active_style,
        "none",
    );

    assert!(junctions.is_empty());
    assert!(buffer.content.iter().all(|cell| {
        cell.style().bg != Some(Color::Red) && cell.style().bg != Some(Color::Blue)
    }));
}
