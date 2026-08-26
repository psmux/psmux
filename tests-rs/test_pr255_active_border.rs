// Tests for PR #255: active pane border indicator across all split layouts.
// Verifies LayoutJson::count_leaves() and that render_layout_json colors
// the borders adjacent to the active pane correctly for >= 3 panes.

use crate::layout::LayoutJson;

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

fn split(kind: &str, children: Vec<LayoutJson>) -> LayoutJson {
    LayoutJson::Split {
        kind: kind.to_string(),
        sizes: vec![50; children.len()],
        children,
    }
}

#[test]
fn count_leaves_single_pane_is_one() {
    let l = leaf(0, true);
    assert_eq!(l.count_leaves(), 1);
}

#[test]
fn count_leaves_two_pane_horizontal_split() {
    let l = split("Horizontal", vec![leaf(0, true), leaf(1, false)]);
    assert_eq!(l.count_leaves(), 2);
}

#[test]
fn count_leaves_three_pane_nested_split() {
    // Horizontal: [leaf, vertical: [leaf, leaf]]
    let l = split(
        "Horizontal",
        vec![
            leaf(0, true),
            split("Vertical", vec![leaf(1, false), leaf(2, false)]),
        ],
    );
    assert_eq!(l.count_leaves(), 3);
}

#[test]
fn count_leaves_four_pane_quad_layout() {
    // Horizontal: [Vertical: [leaf, leaf], Vertical: [leaf, leaf]]
    let l = split(
        "Horizontal",
        vec![
            split("Vertical", vec![leaf(0, true), leaf(1, false)]),
            split("Vertical", vec![leaf(2, false), leaf(3, false)]),
        ],
    );
    assert_eq!(l.count_leaves(), 4);
}

#[test]
fn count_leaves_deeply_nested() {
    // 5-pane: H[L, V[L, H[L, L]], L]
    let l = split(
        "Horizontal",
        vec![
            leaf(0, false),
            split(
                "Vertical",
                vec![
                    leaf(1, true),
                    split("Horizontal", vec![leaf(2, false), leaf(3, false)]),
                ],
            ),
            leaf(4, false),
        ],
    );
    assert_eq!(l.count_leaves(), 5);
}

#[cfg(windows)]
#[test]
fn render_three_panes_does_not_color_unrelated_separator_active() {
    // 3-pane H[active=0, V[1, 2]]
    // The vertical separator inside the right side (between 1 and 2) is NOT
    // adjacent to the active pane (id=0) and therefore must NOT be colored as
    // active_border_fg. Before PR #255, the legacy "both_leaves" half-highlight
    // path would color half of that inner separator as if its leaf were active.
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = split(
        "Horizontal",
        vec![
            leaf(0, true),
            split("Vertical", vec![leaf(1, false), leaf(2, false)]),
        ],
    );
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();
    let total = layout.count_leaves();
    assert_eq!(total, 3);
    let active_border = Color::Green;
    let inactive_border = Color::DarkGray;

    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(inactive_border),
            ratatui::style::Style::default().fg(active_border),
            false, Color::Reset,
            active_rect,
            "", false, "off", "",
            total,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
        let borders = crate::client::border_geometry_from_layout(
            &layout,
            area,
            f.buffer_mut().area,
            false,
        );
        crate::rendering::fix_border_intersections(
            f.buffer_mut(),
            crate::border_lines::border_chars("single"),
            &borders,
        );
    }).unwrap();

    // Inspect every horizontal separator cell '─' on the right side of the
    // outer split and assert NONE of them are colored active_border (Green),
    // because neither child of the inner vertical split is active.
    let buf = term.backend().buffer().clone();
    let area = buf.area;

    // Outer split is horizontal, so the outer vertical separator '│' sits
    // somewhere around column 30. The inner horizontal separator '─' sits on
    // the right side at some row. We look for '─' cells at column > 30.
    let mut bad_active_colored_dash = 0;
    let mut total_dash_right = 0;
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf.content[(y as usize) * (area.width as usize) + (x as usize)];
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            if ch == '─' && x > 30 {
                total_dash_right += 1;
                if cell.style().fg == Some(active_border) {
                    bad_active_colored_dash += 1;
                }
            }
        }
    }

    assert!(total_dash_right > 0, "expected horizontal separator on right side");
    assert_eq!(
        bad_active_colored_dash, 0,
        "PR #255 regression: {} dash cells on right side were colored as active even though active pane is on the left",
        bad_active_colored_dash
    );
}

#[cfg(windows)]
#[test]
fn render_two_panes_keeps_half_highlight_path() {
    // For exactly 2 panes, the legacy half-highlight path is preserved
    // (left side colored as active when left is active). Verifies that the
    // `total_panes == 2` guard does not break the simple split case.
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = split("Horizontal", vec![leaf(0, true), leaf(1, false)]);
    let backend = TestBackend::new(40, 12);
    let mut term = Terminal::new(backend).unwrap();
    let total = layout.count_leaves();
    let active_border = Color::Green;
    let inactive_border = Color::DarkGray;

    term.draw(|f| {
        let area = Rect::new(0, 0, 40, 12);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            ratatui::style::Style::default().fg(inactive_border),
            ratatui::style::Style::default().fg(active_border),
            false, Color::Reset,
            active_rect,
            "", false, "off", "",
            total,
            crate::border_lines::border_chars("single"),
            None,
            crate::client::WindowContentStyles::default(),
        );
        let borders = crate::client::border_geometry_from_layout(
            &layout,
            area,
            f.buffer_mut().area,
            false,
        );
        crate::rendering::fix_border_intersections(
            f.buffer_mut(),
            crate::border_lines::border_chars("single"),
            &borders,
        );
    }).unwrap();

    // The vertical separator '│' should have at least some cells colored as
    // active_border (the half adjacent to the active left pane).
    let buf = term.backend().buffer().clone();
    let area = buf.area;
    let mut active_pipe = 0;
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf.content[(y as usize) * (area.width as usize) + (x as usize)];
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            if ch == '│' && cell.style().fg == Some(active_border) {
                active_pipe += 1;
            }
        }
    }
    assert!(active_pipe > 0, "expected at least some active-colored pipe cells in 2-pane split");
}
