// Feature: pane-border-lines — deterministic render proof.
//
// Renders a real 2-pane split through the actual client render function
// (`render_layout_json` + `fix_border_intersections`) into a headless ratatui
// TestBackend, then asserts the separator glyphs match the selected style.
// This exercises the exact code path the attached client uses, without a
// pseudo-console, so it is deterministic on Windows CI.
//
// Horizontal split  -> full-height VERTICAL separator column.
// Vertical  split    -> full-width HORIZONTAL separator row.

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
    LayoutJson::Split { kind: kind.to_string(), sizes: vec![50; children.len()], children }
}

/// Render a 2-pane split with the given `pane-border-lines` style and return a
/// map of char -> count over the whole buffer.
fn render_counts(kind: &str, style: &str, w: u16, h: u16) -> std::collections::HashMap<char, usize> {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = split(kind, vec![leaf(0, true), leaf(1, false)]);
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    let total = layout.count_leaves();
    let bchars = crate::border_lines::border_chars(style);

    term.draw(|f| {
        let area = Rect::new(0, 0, w, h);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area,
            false,
            Color::DarkGray, Color::Green,
            false, Color::Reset,
            active_rect,
            "", false, "off", "",
            total,
            bchars,
            None,
            crate::client::WindowContentStyles::default(),
        );
        let border_mask = crate::client::border_mask_from_layout(&layout, area, f.buffer_mut().area, false);
        crate::rendering::fix_border_intersections(f.buffer_mut(), bchars, &border_mask);
    }).unwrap();

    let buf = term.backend().buffer().clone();
    let mut counts = std::collections::HashMap::new();
    for cell in buf.content.iter() {
        let ch = cell.symbol().chars().next().unwrap_or(' ');
        *counts.entry(ch).or_insert(0) += 1;
    }
    counts
}

fn c(counts: &std::collections::HashMap<char, usize>, ch: char) -> usize {
    *counts.get(&ch).unwrap_or(&0)
}

#[test]
fn single_draws_light_vertical_separator() {
    let counts = render_counts("Horizontal", "single", 40, 12);
    assert!(c(&counts, '│') >= 8, "single H-split should draw a │ column, got {}", c(&counts, '│'));
    assert_eq!(c(&counts, '║'), 0, "single must not use double glyph");
    assert_eq!(c(&counts, '┃'), 0, "single must not use heavy glyph");
}

#[test]
fn double_draws_double_vertical_separator() {
    let counts = render_counts("Horizontal", "double", 40, 12);
    assert!(c(&counts, '║') >= 8, "double H-split should draw a ║ column, got {}", c(&counts, '║'));
    assert_eq!(c(&counts, '│'), 0, "double must not use single glyph");
}

#[test]
fn heavy_draws_heavy_vertical_separator() {
    let counts = render_counts("Horizontal", "heavy", 40, 12);
    assert!(c(&counts, '┃') >= 8, "heavy H-split should draw a ┃ column, got {}", c(&counts, '┃'));
    assert_eq!(c(&counts, '│'), 0, "heavy must not use single glyph");
}

#[test]
fn simple_draws_ascii_vertical_separator() {
    let counts = render_counts("Horizontal", "simple", 40, 12);
    assert!(c(&counts, '|') >= 8, "simple H-split should draw a | column, got {}", c(&counts, '|'));
    assert_eq!(c(&counts, '│'), 0, "simple must not use box glyph");
}

#[test]
fn none_draws_no_separator() {
    let counts = render_counts("Horizontal", "none", 40, 12);
    for ch in ['│', '║', '┃', '|', '─', '═', '━'] {
        assert_eq!(c(&counts, ch), 0, "none must draw no separator glyph, found {:?}", ch);
    }
}

#[test]
fn vertical_split_uses_horizontal_glyph() {
    // A vertical split produces a horizontal separator ROW.
    let single = render_counts("Vertical", "single", 40, 12);
    assert!(c(&single, '─') >= 8, "single V-split should draw a ─ row, got {}", c(&single, '─'));
    let double = render_counts("Vertical", "double", 40, 12);
    assert!(c(&double, '═') >= 8, "double V-split should draw a ═ row, got {}", c(&double, '═'));
    assert_eq!(c(&double, '─'), 0);
}

#[test]
fn nested_split_produces_double_junction() {
    // H[ leaf, V[leaf, leaf] ] with double style: the outer vertical separator
    // meets the inner horizontal separator, so a ╬/╠/╣ junction must appear.
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;

    let layout = split("Horizontal", vec![
        leaf(0, true),
        split("Vertical", vec![leaf(1, false), leaf(2, false)]),
    ]);
    let backend = TestBackend::new(60, 20);
    let mut term = Terminal::new(backend).unwrap();
    let total = layout.count_leaves();
    let bchars = crate::border_lines::border_chars("double");
    term.draw(|f| {
        let area = Rect::new(0, 0, 60, 20);
        let active_rect = crate::client::compute_active_rect_json(&layout, area);
        crate::client::render_layout_json(
            f, &layout, area, false, Color::DarkGray, Color::Green,
            false, Color::Reset, active_rect, "", false, "off", "", total, bchars,
            None,
            crate::client::WindowContentStyles::default(),
        );
        let border_mask = crate::client::border_mask_from_layout(&layout, area, f.buffer_mut().area, false);
        crate::rendering::fix_border_intersections(f.buffer_mut(), bchars, &border_mask);
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let mut junctions = 0;
    for cell in buf.content.iter() {
        let ch = cell.symbol().chars().next().unwrap_or(' ');
        if matches!(ch, '╬' | '╠' | '╣' | '╦' | '╩') { junctions += 1; }
    }
    assert!(junctions >= 1, "expected at least one double-line junction glyph, got {}", junctions);
}
