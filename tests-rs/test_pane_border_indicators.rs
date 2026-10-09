use super::*;
use crate::client::{render_float_overlays, FloatJson};
use crate::layout::{CellRunJson, LayoutJson, RowRunsJson};
use crate::pane_border::PaneBorderIndicators;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
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
        title: None, copy_hl: Vec::new(),
    }
}

fn render_layout(
    layout: &LayoutJson,
    indicators: PaneBorderIndicators,
    line_mode: &str,
    width: u16,
    height: u16,
) -> ratatui::buffer::Buffer {
    let inactive = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let active = Style::default().fg(Color::Green).bg(Color::Red);
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
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
                inactive,
                active,
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
                indicators,
            );
            let borders = border_geometry_from_layout(
                layout,
                area,
                frame.buffer_mut().area,
                false,
            );
            let junctions = crate::rendering::fix_border_intersections(
                frame.buffer_mut(),
                chars,
                &borders,
            );
            recolor_border_junctions(
                frame.buffer_mut(),
                &junctions,
                indicators.highlights_active().then_some(active_rect).flatten(),
                inactive,
                active,
            );
            draw_pane_border_arrows(
                frame.buffer_mut(),
                &borders,
                chars,
                active_rect,
                indicators,
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render(
    kind: &str,
    indicators: PaneBorderIndicators,
    line_mode: &str,
    active_child: usize,
) -> ratatui::buffer::Buffer {
    let layout = LayoutJson::Split {
        kind: kind.to_string(),
        sizes: vec![50, 50],
        children: vec![
            leaf(0, active_child == 0),
            leaf(1, active_child == 1),
        ],
    };
    render_layout(&layout, indicators, line_mode, 10, 6)
}

fn contains_arrow(buffer: &ratatui::buffer::Buffer) -> bool {
    buffer
        .content
        .iter()
        .any(|cell| matches!(cell.symbol(), "←" | "→" | "↑" | "↓"))
}

fn float_row(ch: char, width: usize) -> RowRunsJson {
    RowRunsJson {
        runs: (0..width)
            .map(|_| CellRunJson {
                text: ch.to_string(),
                fg: String::new(),
                bg: String::new(),
                flags: 0,
                width: 1,
                link: None,
                ul: 0,
                ulc: None,
            })
            .collect(),
    }
}

fn floating_pane(width: u16, height: u16) -> FloatJson {
    FloatJson {
        x: 1,
        y: 1,
        w: width,
        h: height,
        border: "single".to_string(),
        focused: true,
        title: String::new(),
        rows: (0..height.saturating_sub(2))
            .map(|_| float_row('Z', width.saturating_sub(2) as usize))
            .collect(),
        ..Default::default()
    }
}

fn render_float(
    pane: &FloatJson,
    width: u16,
    height: u16,
    inactive: Style,
    active: Style,
    indicators: PaneBorderIndicators,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_float_overlays(
                frame,
                Rect::new(0, 0, width, height),
                std::slice::from_ref(pane),
                crate::client::WindowContentStyles::default(),
                inactive,
                active,
                indicators,
            );
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

/// `off` is a genuine no-cue mode: the divider next to the active pane keeps
/// pane-border-style, exactly like a divider that touches no active pane, and
/// no arrows are drawn. Before this it kept pane-active-border-style across the
/// whole divider, so turning the indicators off produced a LOUDER cue than the
/// default colour split.
#[test]
fn off_mode_draws_no_active_cue_across_the_divider() {
    let buffer = render("Horizontal", PaneBorderIndicators::Off, "single", 0);
    let x = 4;
    assert!(!contains_arrow(&buffer), "off mode must not draw arrows");
    for y in 0..6 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Blue),
            "off mode must keep the inactive border style at divider row {y}",
        );
    }
}

/// `arrows` keeps the active border style on the adjacent divider and adds the
/// markers, so only the colour SPLIT is what `colour` and `both` add.
#[test]
fn arrows_mode_keeps_the_active_style_across_the_divider() {
    let buffer = render("Horizontal", PaneBorderIndicators::Arrows, "single", 0);
    let x = 4;
    assert!(contains_arrow(&buffer), "arrows mode must draw arrows");
    for y in 0..6 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Red),
            "arrows mode must keep the active border style at divider row {y}",
        );
    }
}

#[test]
fn colour_mode_splits_the_divider_style_at_the_active_pane_midpoint() {
    let buffer = render("Horizontal", PaneBorderIndicators::Colour, "single", 0);
    let x = 4;
    assert_eq!(buffer[(x, 0)].symbol(), "│");
    assert!(!contains_arrow(&buffer), "colour mode must not draw arrows");
    for y in 0..3 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Red),
            "active half should use the active background at divider row {y}",
        );
    }
    for y in 3..6 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Blue),
            "inactive half should use the inactive background at divider row {y}",
        );
    }
}

#[test]
fn arrows_mode_adds_an_inward_arrow_without_splitting_the_style() {
    let buffer = render("Horizontal", PaneBorderIndicators::Arrows, "single", 0);
    let x = 4;
    assert_eq!(buffer[(x, 1)].symbol(), "←");
    assert_eq!(buffer[(x, 1)].style().bg, Some(Color::Red));
    for y in [0, 2, 3, 4, 5] {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Red),
            "arrows mode must keep the active border style at divider row {y}",
        );
    }
}

#[test]
fn both_mode_combines_the_inward_arrow_and_split_style() {
    let buffer = render("Horizontal", PaneBorderIndicators::Both, "single", 0);
    let x = 4;
    assert_eq!(buffer[(x, 1)].symbol(), "←");
    for y in 0..3 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Red),
            "active half should use the active background at divider row {y}",
        );
    }
    for y in 3..6 {
        assert_eq!(
            buffer[(x, y)].style().bg,
            Some(Color::Blue),
            "inactive half should use the inactive background at divider row {y}",
        );
    }
}

#[test]
fn stacked_arrow_points_into_active_top_pane() {
    let buffer = render("Vertical", PaneBorderIndicators::Arrows, "single", 0);
    let arrow = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "↑")
        .expect("up arrow below active top pane");
    assert_eq!(arrow.style().bg, Some(Color::Red));
}

#[test]
fn arrows_remain_visible_with_space_borders() {
    let buffer = render("Horizontal", PaneBorderIndicators::Arrows, "spaces", 0);
    assert!(buffer.content.iter().any(|cell| cell.symbol() == "←"));
}

#[test]
fn arrows_point_into_active_right_and_bottom_panes() {
    let side_by_side = render(
        "Horizontal",
        PaneBorderIndicators::Arrows,
        "single",
        1,
    );
    assert!(side_by_side.content.iter().any(|cell| cell.symbol() == "→"));

    let stacked = render(
        "Vertical",
        PaneBorderIndicators::Arrows,
        "single",
        1,
    );
    assert!(stacked.content.iter().any(|cell| cell.symbol() == "↓"));
}

#[test]
fn one_row_nested_split_keeps_arrow_on_the_vertical_divider() {
    let layout = LayoutJson::Split {
        kind: "Vertical".to_string(),
        sizes: vec![50, 50],
        children: vec![
            LayoutJson::Split {
                kind: "Horizontal".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(0, false), leaf(1, true)],
            },
            leaf(2, false),
        ],
    };
    let area = Rect::new(0, 0, 4, 3);
    let borders = border_geometry_from_layout(&layout, area, area, false);
    let vertical_divider = 1;
    let root_separator = area.width as usize + 1;
    assert!(borders.contains_vertical(vertical_divider));
    assert!(!borders.contains_horizontal(vertical_divider));
    assert!(borders.contains_horizontal(root_separator));
    assert!(!borders.contains_vertical(root_separator));

    let buffer = render_layout(
        &layout,
        PaneBorderIndicators::Arrows,
        "single",
        area.width,
        area.height,
    );
    assert_eq!(buffer[(1, 0)].symbol(), "→");
    assert_ne!(buffer[(1, 1)].symbol(), "→");
}

#[test]
fn one_column_nested_split_keeps_arrow_on_the_horizontal_divider() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            LayoutJson::Split {
                kind: "Vertical".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(0, false), leaf(1, true)],
            },
            leaf(2, false),
        ],
    };
    let area = Rect::new(0, 0, 3, 4);
    let borders = border_geometry_from_layout(&layout, area, area, false);
    let horizontal_divider = 3;
    let root_separator = area.width as usize + 1;
    assert!(borders.contains_horizontal(horizontal_divider));
    assert!(!borders.contains_vertical(horizontal_divider));
    assert!(borders.contains_vertical(root_separator));
    assert!(!borders.contains_horizontal(root_separator));

    let buffer = render_layout(
        &layout,
        PaneBorderIndicators::Arrows,
        "single",
        area.width,
        area.height,
    );
    assert_eq!(buffer[(0, 1)].symbol(), "↓");
    assert_ne!(buffer[(1, 1)].symbol(), "↓");
}

#[test]
fn zero_row_nested_active_pane_does_not_mark_the_parent_separator() {
    let layout = LayoutJson::Split {
        kind: "Vertical".to_string(),
        sizes: vec![50, 50],
        children: vec![
            LayoutJson::Split {
                kind: "Horizontal".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(0, false), leaf(1, true)],
            },
            leaf(2, false),
        ],
    };
    let area = Rect::new(0, 0, 4, 2);
    let active_rect = compute_active_rect_json(&layout, area).unwrap();
    assert!(active_rect.width > 0);
    assert_eq!(active_rect.height, 0);

    let buffer = render_layout(
        &layout,
        PaneBorderIndicators::Arrows,
        "single",
        area.width,
        area.height,
    );
    assert!(!contains_arrow(&buffer));
}

#[test]
fn zero_column_nested_active_pane_does_not_mark_the_parent_separator() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            LayoutJson::Split {
                kind: "Vertical".to_string(),
                sizes: vec![50, 50],
                children: vec![leaf(0, false), leaf(1, true)],
            },
            leaf(2, false),
        ],
    };
    let area = Rect::new(0, 0, 2, 4);
    let active_rect = compute_active_rect_json(&layout, area).unwrap();
    assert_eq!(active_rect.width, 0);
    assert!(active_rect.height > 0);

    let buffer = render_layout(
        &layout,
        PaneBorderIndicators::Arrows,
        "single",
        area.width,
        area.height,
    );
    assert!(!contains_arrow(&buffer));
}

#[test]
fn documented_values_parse_to_each_mode() {
    for (raw, expected) in [
        ("off", PaneBorderIndicators::Off),
        ("colour", PaneBorderIndicators::Colour),
        ("arrows", PaneBorderIndicators::Arrows),
        ("both", PaneBorderIndicators::Both),
    ] {
        assert_eq!(PaneBorderIndicators::parse(raw), Ok(expected));
    }
}

#[test]
fn focused_float_suppresses_tiled_indicators() {
    let inactive = Style::default().fg(Color::DarkGray);
    let active = Style::default().fg(Color::Green);
    let rect = Some(Rect::new(0, 0, 10, 6));
    assert_eq!(tiled_border_focus(rect, inactive, active, true), (None, inactive));
    assert_eq!(tiled_border_focus(rect, inactive, active, false), (rect, active));
}

#[test]
fn focused_float_modes_preserve_title_and_active_border_style() {
    let inactive = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let active = Style::default().fg(Color::Green).bg(Color::Red);
    let mut pane = floating_pane(8, 5);
    pane.title = "T".to_string();

    for (mode, expects_arrows) in [
        (PaneBorderIndicators::Off, false),
        (PaneBorderIndicators::Colour, false),
        (PaneBorderIndicators::Arrows, true),
        (PaneBorderIndicators::Both, true),
    ] {
        let buffer = render_float(&pane, 20, 10, inactive, active, mode);
        assert_eq!(
            buffer[(1, 1)].style().fg,
            Some(Color::Green),
            "{mode:?} must preserve the active foreground",
        );
        assert_eq!(
            buffer[(1, 1)].style().bg,
            Some(Color::Red),
            "{mode:?} must preserve the active background",
        );
        assert!(
            buffer.content.iter().any(|cell| cell.symbol() == "T"),
            "{mode:?} must preserve the floating pane title",
        );
        assert_eq!(
            contains_arrow(&buffer),
            expects_arrows,
            "{mode:?} arrow visibility",
        );
    }
}

#[test]
fn floating_arrows_inherit_the_active_border_style() {
    let inactive = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let active = Style::default().fg(Color::Green).bg(Color::Red);
    let pane = floating_pane(8, 5);
    let buffer = render_float(
        &pane,
        20,
        10,
        inactive,
        active,
        PaneBorderIndicators::Arrows,
    );

    assert_eq!(buffer[(1, 3)].symbol(), "→");
    assert_eq!(buffer[(1, 3)].style().fg, Some(Color::Green));
    assert_eq!(buffer[(1, 3)].style().bg, Some(Color::Red));
    assert_eq!(buffer[(5, 1)].symbol(), "↓");
}

#[test]
fn minimum_float_places_all_four_inward_arrows() {
    let pane = floating_pane(3, 3);
    let buffer = render_float(
        &pane,
        5,
        5,
        Style::default(),
        Style::default(),
        PaneBorderIndicators::Arrows,
    );

    for (x, y, marker) in [
        (2, 1, "↓"),
        (2, 3, "↑"),
        (1, 2, "→"),
        (3, 2, "←"),
    ] {
        assert_eq!(
            buffer[(x, y)].symbol(),
            marker,
            "minimum float marker at ({x}, {y})",
        );
    }
}

#[test]
fn clipped_float_suppresses_arrows() {
    let pane = floating_pane(3, 3);
    let buffer = render_float(
        &pane,
        2,
        2,
        Style::default(),
        Style::default(),
        PaneBorderIndicators::Arrows,
    );
    assert!(!contains_arrow(&buffer));
}

#[test]
fn oversized_float_title_does_not_overflow_arrow_placement() {
    let mut pane = floating_pane(8, 5);
    pane.title = "T".repeat(70_000);
    let buffer = render_float(
        &pane,
        20,
        10,
        Style::default(),
        Style::default(),
        PaneBorderIndicators::Arrows,
    );
    assert_eq!(buffer[(1, 3)].symbol(), "→");
    assert_eq!(buffer[(8, 3)].symbol(), "←");
    assert!(!buffer.content.iter().any(|cell| cell.symbol() == "↓"));
}
