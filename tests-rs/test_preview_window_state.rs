use super::*;
use crate::layout::LayoutJson;
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
        cursor_shape: 0, cursor_colour: None,
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
        title: None, copy_hl: Vec::new(), images: vec![],
    }
}

#[test]
fn layout_only_response_uses_default_border_modes_and_chooser_styles() {
    let raw = serde_json::to_string(&leaf(0, true)).unwrap();
    let state = parse_preview_window_state(&raw).unwrap();
    assert_eq!(state.border_lines, crate::border_lines::DEFAULT);
    assert_eq!(state.border_indicators, PaneBorderIndicators::Colour);
    assert!(state.pane_border_style.is_none());
    assert!(state.pane_active_border_style.is_none());

    let (inactive, active) = state.pane_border_styles(
        Style::default().fg(Color::Magenta),
        Style::default().fg(Color::Cyan),
    );
    assert_eq!(inactive.fg, Some(Color::Magenta));
    assert_eq!(active.fg, Some(Color::Cyan));
}

#[test]
fn state_response_preserves_target_border_styles() {
    let state = PreviewWindowState {
        layout: leaf(0, true),
        border_lines: "single".to_string(),
        border_indicators: PaneBorderIndicators::Arrows,
        pane_border_style: Some("fg=blue,bg=black".to_string()),
        pane_active_border_style: Some("fg=red,bg=yellow".to_string()),
        floating_pane_focused: false,
    };
    let raw = serde_json::to_string(&state).unwrap();
    let parsed = parse_preview_window_state(&raw).unwrap();
    assert_eq!(
        parsed.pane_border_style.as_deref(),
        Some("fg=blue,bg=black"),
    );
    assert_eq!(
        parsed.pane_active_border_style.as_deref(),
        Some("fg=red,bg=yellow"),
    );

    let (inactive, active) = parsed.pane_border_styles(
        Style::default().fg(Color::White),
        Style::default().fg(Color::Green),
    );
    assert_eq!(inactive.fg, Some(Color::Blue));
    assert_eq!(inactive.bg, Some(Color::Black));
    assert_eq!(active.fg, Some(Color::Red));
    assert_eq!(active.bg, Some(Color::Yellow));
}

#[test]
fn explicit_empty_state_styles_use_target_defaults() {
    let state = PreviewWindowState {
        layout: leaf(0, true),
        border_lines: "single".to_string(),
        border_indicators: PaneBorderIndicators::Colour,
        pane_border_style: Some(String::new()),
        pane_active_border_style: Some(String::new()),
        floating_pane_focused: false,
    };
    let (inactive, active) = state.pane_border_styles(
        Style::default().fg(Color::White),
        Style::default().fg(Color::Magenta),
    );
    assert_eq!(inactive.fg, Some(Color::DarkGray));
    assert_eq!(active.fg, Some(Color::Green));
    assert_eq!(inactive.bg, Some(Color::Reset));
    assert_eq!(active.bg, Some(Color::Reset));
}

#[test]
fn state_without_style_fields_uses_chooser_styles() {
    let raw = serde_json::json!({
        "layout": leaf(0, true),
        "border_lines": "spaces",
        "border_indicators": "both",
        "floating_pane_focused": false,
    })
    .to_string();
    let state = parse_preview_window_state(&raw).unwrap();
    assert_eq!(state.border_lines, "spaces");
    assert_eq!(state.border_indicators, PaneBorderIndicators::Both);
    assert!(state.pane_border_style.is_none());
    assert!(state.pane_active_border_style.is_none());

    let (inactive, active) = state.pane_border_styles(
        Style::default().fg(Color::Yellow),
        Style::default().fg(Color::LightBlue),
    );
    assert_eq!(inactive.fg, Some(Color::Yellow));
    assert_eq!(active.fg, Some(Color::LightBlue));
}

#[test]
fn preview_suppresses_tiled_indicators_when_float_is_focused() {
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let inactive = Style::default().fg(Color::DarkGray).bg(Color::Blue);
    let active = Style::default().fg(Color::Green).bg(Color::Red);
    let backend = TestBackend::new(10, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            crate::preview::render_preview_layout(
                frame,
                &layout,
                Rect::new(0, 0, 10, 6),
                inactive,
                active,
                crate::border_lines::border_chars("single"),
                PaneBorderIndicators::Arrows,
                true,
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    assert!(
        !buffer
            .content
            .iter()
            .any(|cell| matches!(cell.symbol(), "←" | "→" | "↑" | "↓")),
        "a focused float suppresses tiled preview arrows",
    );
    for y in 0..6 {
        assert_eq!(
            buffer[(4, y)].style().bg,
            Some(Color::Blue),
            "a focused float keeps the tiled divider inactive at row {y}",
        );
    }
}
