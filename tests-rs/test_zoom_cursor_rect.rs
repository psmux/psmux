use ratatui::layout::Rect;
use crate::layout::LayoutJson;

fn leaf(id: usize, active: bool) -> LayoutJson {
    LayoutJson::Leaf {
        id,
        rows: 5,
        cols: 10,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: false,
        cursor_shape: 0, cursor_colour: None, default_colours: None,
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
        images: vec![],
    }
}

#[test]
fn zoomed_horizontal_non_first_active_uses_full_area() {
    let area = Rect::new(0, 0, 60, 20);
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![0, 100],
        children: vec![leaf(0, false), leaf(1, true)],
    };

    let unzoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, false).unwrap();
    let zoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, true).unwrap();

    assert_ne!(unzoomed.x, area.x, "baseline split traversal should offset second pane");
    assert_eq!(zoomed, area, "zoom-aware traversal should keep full-area origin");
}

#[test]
fn zoomed_vertical_non_first_active_uses_full_area() {
    let area = Rect::new(0, 0, 60, 20);
    let layout = LayoutJson::Split {
        kind: "Vertical".to_string(),
        sizes: vec![0, 100],
        children: vec![leaf(0, false), leaf(1, true)],
    };

    let unzoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, false).unwrap();
    let zoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, true).unwrap();

    assert_ne!(unzoomed.y, area.y, "baseline split traversal should offset lower pane");
    assert_eq!(zoomed, area, "zoom-aware traversal should keep full-area origin");
}

#[test]
fn zoomed_nested_non_first_active_does_not_compound_offsets() {
    let area = Rect::new(0, 0, 100, 40);
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![0, 100],
        children: vec![
            leaf(0, false),
            LayoutJson::Split {
                kind: "Vertical".to_string(),
                sizes: vec![0, 100],
                children: vec![leaf(1, false), leaf(2, true)],
            },
        ],
    };

    let unzoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, false).unwrap();
    let zoomed = crate::client::compute_active_rect_json_zoom_aware(&layout, area, true).unwrap();

    assert!(
        unzoomed.x > area.x || unzoomed.y > area.y,
        "baseline split traversal should offset nested non-first pane"
    );
    assert_eq!(zoomed, area, "zoom-aware traversal should avoid cumulative nested offsets");
}
