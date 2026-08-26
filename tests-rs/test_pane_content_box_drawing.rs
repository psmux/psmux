// Geometry-scoped border passes preserve box-drawing glyphs and styles emitted
// by applications inside pane content.

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

#[cfg(windows)]
#[test]
fn fix_border_intersections_leaves_pane_content_table_untouched() {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    // 2-pane horizontal split: a real separator column exists somewhere near
    // the middle of the 40-wide buffer. We inject the markdown table into the
    // left pane, well clear of that separator column.
    let layout = split("Horizontal", vec![leaf(0, true), leaf(1, false)]);
    let backend = TestBackend::new(40, 12);
    let mut term = Terminal::new(backend).unwrap();
    let total = layout.count_leaves();
    let border_fg = Color::DarkGray;
    let active_border_fg = Color::Green;
    let content_fg = Color::Yellow;
    let bchars = crate::border_lines::border_chars("single");

    // Markdown-table-like block: header separator row uses '┼', straight
    // lines use '│' and '─'. Written into rows 2..5, cols 2..9 — inside the
    // left pane (cols 0..~19), far from the separator column (~19-20).
    let table_rows: [&str; 3] = [
        "a│b─┼c─",
        "──┼────",
        "d│e─┼f─",
    ];

    let mut snapshot: Vec<(usize, char, Style)> = Vec::new();

    term.draw(|f| {
        let area = Rect::new(0, 0, 40, 12);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            Style::default().fg(border_fg), Style::default().fg(active_border_fg),
            false, Color::Reset,
            active_rect,
            "", false, "off", "",
            total,
            bchars,
            None,
            crate::client::WindowContentStyles::default(),
        );

        // Inject the fake table into the buffer as pane content would appear.
        let buf = f.buffer_mut();
        let w = buf.area.width as usize;
        let content_style = Style::default().fg(content_fg);
        for (r, row_str) in table_rows.iter().enumerate() {
            let y = 2 + r;
            for (c, ch) in row_str.chars().enumerate() {
                let x = 2 + c;
                let idx = y * w + x;
                if idx < buf.content.len() {
                    buf.content[idx].set_char(ch);
                    buf.content[idx].set_style(content_style);
                }
            }
        }

        // Snapshot the injected cells before running the post-pass.
        for (r, row_str) in table_rows.iter().enumerate() {
            let y = 2 + r;
            for (c, _) in row_str.chars().enumerate() {
                let x = 2 + c;
                let idx = y * w + x;
                if idx < buf.content.len() {
                    let cell = &buf.content[idx];
                    snapshot.push((idx, cell.symbol().chars().next().unwrap_or(' '), cell.style()));
                }
            }
        }

        let borders =
            crate::client::border_geometry_from_layout(&layout, area, buf.area, false);

        // Sanity: geometry must exclude every injected content cell,
        // otherwise this test isn't exercising the guard.
        for &(idx, _, _) in &snapshot {
            assert!(!borders.contains(&idx), "content cell idx {} unexpectedly marked as a separator", idx);
        }

        crate::rendering::fix_border_intersections(f.buffer_mut(), bchars, &borders);
    }).unwrap();

    let buf = term.backend().buffer().clone();
    for (idx, expected_ch, expected_style) in snapshot {
        let cell = &buf.content[idx];
        let actual_ch = cell.symbol().chars().next().unwrap_or(' ');
        assert_eq!(actual_ch, expected_ch, "content cell idx {} glyph changed", idx);
        assert_eq!(cell.style(), expected_style, "content cell idx {} style changed", idx);
        assert_ne!(cell.style().fg, Some(border_fg), "content cell idx {} recolored to border fg", idx);
        assert_ne!(cell.style().fg, Some(active_border_fg), "content cell idx {} recolored to active border fg", idx);
    }
}

// Regression test for the zoom-unaware follow-up bug:
// `border_geometry_from_layout`
// must mirror `render_layout_json`'s zoom early-return exactly. When zoomed,
// `window_ops::toggle_zoom` encodes the layout with sizes like `[100, 0]`.
// `split_with_gaps`'s min-1-cell-stealing turns `[100, 0]` into e.g. `[78, 1]`
// for width 80, which (if geometry were computed as though unzoomed) would
// mark a phantom separator column at `area.x + 78` — squarely inside content
// that `render_layout_json` actually rendered edge-to-edge across the full
// area for the zoomed pane. A junction char sitting on that column must never
// be treated as a real separator.
#[cfg(windows)]
#[test]
fn fix_border_intersections_leaves_zoomed_pane_content_untouched() {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    // Zoomed 2-pane horizontal split, sizes [100, 0] — exactly how
    // window_ops::toggle_zoom encodes "pane 0 is zoomed".
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![100, 0],
        children: vec![leaf(0, true), leaf(1, false)],
    };
    let backend = TestBackend::new(80, 12);
    let mut term = Terminal::new(backend).unwrap();
    let border_fg = Color::DarkGray;
    let active_border_fg = Color::Green;
    let content_fg = Color::Yellow;
    let bchars = crate::border_lines::border_chars("single");

    // Box-drawing content straddling column 78 (the phantom separator column
    // for the equivalent UNZOOMED [78, 1] split geometry), including a
    // junction glyph '┼' at that exact column.
    let table_rows: [&str; 3] = [
        "a─┼b",
        "──┼─",
        "c─┼d",
    ];
    let start_x: usize = 76; // columns 76..79; column 78 is the 3rd char (index 2)

    let mut snapshot: Vec<(usize, char, Style)> = Vec::new();
    let mut phantom_col_idx: usize = 0;

    term.draw(|f| {
        let area = Rect::new(0, 0, 80, 12);
        let active_rect = crate::client::compute_active_rect_json_zoom_aware(&layout, area, true);
        // Production sets total_panes = 1 when zoomed (client.rs: `if state.zoomed { 1 } else { root.count_leaves() }`).
        let total_panes = 1;
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            Style::default().fg(border_fg), Style::default().fg(active_border_fg),
            false, Color::Reset,
            active_rect,
            "", true, "off", "",
            total_panes,
            bchars,
            None,
            crate::client::WindowContentStyles::default(),
        );

        let buf = f.buffer_mut();
        let w = buf.area.width as usize;
        phantom_col_idx = 3 * w + 78;
        let content_style = Style::default().fg(content_fg);
        for (r, row_str) in table_rows.iter().enumerate() {
            let y = 3 + r;
            for (c, ch) in row_str.chars().enumerate() {
                let x = start_x + c;
                let idx = y * w + x;
                if idx < buf.content.len() {
                    buf.content[idx].set_char(ch);
                    buf.content[idx].set_style(content_style);
                }
            }
        }

        for (r, row_str) in table_rows.iter().enumerate() {
            let y = 3 + r;
            for (c, _) in row_str.chars().enumerate() {
                let x = start_x + c;
                let idx = y * w + x;
                if idx < buf.content.len() {
                    let cell = &buf.content[idx];
                    snapshot.push((idx, cell.symbol().chars().next().unwrap_or(' '), cell.style()));
                }
            }
        }

        let borders =
            crate::client::border_geometry_from_layout(&layout, area, buf.area, true);

        // The phantom-separator column (78) for the equivalent unzoomed
        // [78, 1] split geometry must NOT be marked as a separator when zoomed.
        assert!(
            !borders.contains(&phantom_col_idx),
            "zoomed geometry marked phantom separator column 78 (idx {}) as a real border",
            phantom_col_idx
        );
        for &(idx, _, _) in &snapshot {
            assert!(!borders.contains(&idx), "content cell idx {} unexpectedly marked as a separator under zoom", idx);
        }

        crate::rendering::fix_border_intersections(f.buffer_mut(), bchars, &borders);
    }).unwrap();

    let buf = term.backend().buffer().clone();
    for (idx, expected_ch, expected_style) in snapshot {
        let cell = &buf.content[idx];
        let actual_ch = cell.symbol().chars().next().unwrap_or(' ');
        assert_eq!(actual_ch, expected_ch, "zoomed content cell idx {} glyph changed", idx);
        assert_eq!(cell.style(), expected_style, "zoomed content cell idx {} style changed", idx);
        assert_ne!(cell.style().fg, Some(border_fg), "zoomed content cell idx {} recolored to border fg", idx);
        assert_ne!(cell.style().fg, Some(active_border_fg), "zoomed content cell idx {} recolored to active border fg", idx);
    }
}
