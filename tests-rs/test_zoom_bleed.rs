use ratatui::layout::Rect;
use crate::layout::LayoutJson;

// Regression tests for zoom pane bleed bug.
//
// The fix: when zoomed, render_layout_json bypasses split_with_gaps entirely
// and passes the full area rect directly to the active child. The hidden child
// is never visited at all. These tests verify that invariant by inspecting the
// rendered cell buffer.

fn leaf(id: usize, active: bool) -> LayoutJson {
    LayoutJson::Leaf {
        id,
        rows: 10,
        cols: 20,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: false,
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

// Each test uses ratatui's TestBackend to render and inspect the cell buffer.
// The invariant: when zoomed, the hidden child must never be rendered.
// We verify this by giving each leaf a distinct pane_index label (via
// border_format="#{pane_index}" + border_status="bottom") and asserting:
//   - The hidden leaf's label does not appear anywhere in the buffer.
//   - The active leaf's label appears at column 0 of the bottom row,
//     proving it received area.x=0 (the full unshifted area).

#[cfg(windows)]
#[test]
fn zoomed_left_active_hidden_pane_label_never_rendered() {
    // sizes=[100, 0]: leaf 0 is active (left), leaf 1 is hidden (right).
    // Before the fix, split_with_gaps([100, 0]) gave leaf 1 a 1-px rect at
    // x=210; render_layout_json would visit it and draw its "1" label there.
    // After the fix, leaf 1 is never visited and "1" must not appear anywhere.
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![100, 0],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();

    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset,
            active_rect,
            "", true, "bottom", "#{pane_index}",
            2,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();

    let buf = term.backend().buffer().clone();
    let hidden_label = '1';
    let found = buf.content.iter().any(|cell| {
        cell.symbol().chars().next() == Some(hidden_label)
    });
    assert!(
        !found,
        "hidden pane label '1' must not appear anywhere in the buffer when zoomed"
    );

    // Active pane label "0" must appear at column 0 of the bottom row.
    let bottom_row = 19u16;
    let cell = &buf.content[(bottom_row as usize) * 60];
    assert_eq!(
        cell.symbol().chars().next(),
        Some('0'),
        "active pane label '0' must be at column 0 of the bottom row"
    );
}

#[cfg(windows)]
#[test]
fn zoomed_right_active_hidden_pane_label_never_rendered() {
    // sizes=[0, 100]: leaf 1 is active (right), leaf 0 is hidden (left).
    // Before the fix, the buggy path gave leaf 0 a 1-px rect AND shifted
    // leaf 1's origin to x=2, so its "1" label landed at x=2 not x=0.
    // After the fix, leaf 0 is never visited and leaf 1 gets the full area
    // (x=0), so its "1" label appears at column 0.
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![0, 100],
        children: vec![leaf(0, false), leaf(1, true)],
    };
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();

    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset,
            active_rect,
            "", true, "bottom", "#{pane_index}",
            2,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();

    let buf = term.backend().buffer().clone();
    let hidden_label = '0';
    let found = buf.content.iter().any(|cell| {
        cell.symbol().chars().next() == Some(hidden_label)
    });
    assert!(
        !found,
        "hidden pane label '0' must not appear anywhere in the buffer when zoomed"
    );

    // Active pane label "1" must appear at column 0 of the bottom row,
    // proving the fix passed the full area (x=0) instead of the gap-shifted rect.
    let bottom_row = 19u16;
    let cell = &buf.content[(bottom_row as usize) * 60];
    assert_eq!(
        cell.symbol().chars().next(),
        Some('1'),
        "active pane label '1' must be at column 0 of the bottom row"
    );
}

#[cfg(windows)]
#[test]
fn zoomed_top_active_hidden_pane_label_never_rendered() {
    // Vertical split, sizes=[100, 0]: leaf 0 is active (top), leaf 1 is hidden (bottom).
    // split_with_gaps with is_horizontal=false would steal 1 row from leaf 0
    // and give it to leaf 1 at y=20, rendering its label there.
    // After the fix, leaf 1 is never visited.
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = LayoutJson::Split {
        kind: "Vertical".to_string(),
        sizes: vec![100, 0],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();

    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset,
            active_rect,
            "", true, "bottom", "#{pane_index}",
            2,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();

    let buf = term.backend().buffer().clone();
    let hidden_label = '1';
    let found = buf.content.iter().any(|cell| {
        cell.symbol().chars().next() == Some(hidden_label)
    });
    assert!(
        !found,
        "hidden pane label '1' must not appear anywhere in the buffer when zoomed"
    );

    // Active pane label "0" must appear at column 0 of the bottom row,
    // proving it received the full area height (y=0..20) not a row-stolen rect.
    let bottom_row = 19u16;
    let cell = &buf.content[(bottom_row as usize) * 60];
    assert_eq!(
        cell.symbol().chars().next(),
        Some('0'),
        "active pane label '0' must be at the bottom row of the full area"
    );
}

#[cfg(windows)]
#[test]
fn zoomed_bottom_active_hidden_pane_label_never_rendered() {
    // Vertical split, sizes=[0, 100]: leaf 1 is active (bottom), leaf 0 is hidden (top).
    // The buggy path would give leaf 0 a 1-row rect at y=0 and shift leaf 1's
    // origin to y=2, so its label would land at y=2 not y=0.
    // After the fix, leaf 0 is never visited and leaf 1 gets the full area (y=0).
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = LayoutJson::Split {
        kind: "Vertical".to_string(),
        sizes: vec![0, 100],
        children: vec![leaf(0, false), leaf(1, true)],
    };
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();

    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset,
            active_rect,
            "", true, "bottom", "#{pane_index}",
            2,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
    }).unwrap();

    let buf = term.backend().buffer().clone();
    let hidden_label = '0';
    let found = buf.content.iter().any(|cell| {
        cell.symbol().chars().next() == Some(hidden_label)
    });
    assert!(
        !found,
        "hidden pane label '0' must not appear anywhere in the buffer when zoomed"
    );

    // Active pane label "1" must appear at column 0 of the bottom row,
    // proving the fix passed the full area (y=0) instead of the gap-shifted rect.
    let bottom_row = 19u16;
    let cell = &buf.content[(bottom_row as usize) * 60];
    assert_eq!(
        cell.symbol().chars().next(),
        Some('1'),
        "active pane label '1' must be at the bottom row of the full area"
    );
}
