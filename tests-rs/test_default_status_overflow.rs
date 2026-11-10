use super::*;
use crate::style::{spans_visual_width, StatusRangeType as R};

fn text(layout: &crate::style::LayoutResult) -> String {
    layout.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn fixture(active: usize, width: usize, justify: &str) -> crate::style::LayoutResult {
    let tabs = (1..=12).map(|n| Span::raw(format!("{n:02}:tab "))).collect();
    let ranges = (1..=12).map(|n| (n, (n - 1) * 7, n * 7 - 1)).collect::<Vec<_>>();
    assert!(12 * 7 + 4 + 5 > width, "fixture must overflow");
    layout_default_status_line(vec![Span::raw("LEFT")], tabs, vec![Span::raw("RIGHT")],
        &ranges, Some(active), width, justify, Style::default())
}

#[test]
fn overflowing_first_middle_and_last_active_tabs_are_visible() {
    for justify in ["left", "centre", "right", "absolute-centre"] {
        for active in [1, 6, 12] {
            let layout = fixture(active, 40, justify);
            let drawn = text(&layout);
            assert!(drawn.starts_with("LEFT"), "{justify}: {drawn}");
            assert!(drawn.ends_with("RIGHT"), "{justify}: {drawn}");
            assert!(drawn.contains(&format!("{active:02}:tab")), "{justify}: {drawn}");
            assert_eq!(spans_visual_width(&layout.spans), 40);
            let (_, start, end) = layout.ranges.iter().find(|(kind, _, _)| *kind == R::Window(active)).unwrap();
            assert_eq!(&drawn[*start as usize..*end as usize], format!("{active:02}:tab"));
            assert_eq!(drawn.contains('<'), active != 1);
            assert_eq!(drawn.contains('>'), active != 12);
        }
    }
}

#[test]
fn clipped_ranges_exclude_markers_and_separators() {
    let layout = fixture(6, 40, "left");
    let drawn = text(&layout);
    for (col, ch) in drawn.char_indices() {
        if ch == '<' || ch == '>' || ch == ' ' {
            assert!(!layout.ranges.iter().any(|(kind, start, end)| {
                matches!(kind, R::Window(_)) && (*start as usize..*end as usize).contains(&col)
            }), "column {col} in {drawn}");
        }
    }
}

#[test]
fn fitting_layout_preserves_justification_and_style() {
    let style = Style::default().fg(Color::Red).add_modifier(Modifier::ITALIC);
    for (justify, expected_start) in [("left", 2), ("centre", 10), ("right", 18), ("absolute-centre", 10)] {
        let layout = layout_default_status_line(vec![Span::raw("LL")], vec![Span::styled("TAB", style)],
            vec![Span::raw("RRR")], &[(4, 0, 3)], Some(4), 24, justify, Style::default());
        assert_eq!(layout.ranges.iter().find(|(r, _, _)| *r == R::Window(4)),
            Some(&(R::Window(4), expected_start, expected_start + 3)));
        assert!(layout.spans.contains(&Span::styled("TAB", style)));
        assert!(!text(&layout).contains(['<', '>']));
        assert!(text(&layout).starts_with("LL"));
        assert!(text(&layout).ends_with("RRR"));
    }
}

#[test]
fn absolute_centre_draw_and_click_origins_agree_when_left_overlaps() {
    let layout = layout_default_status_line(vec![Span::raw("LLLLLLLLLL")], vec![Span::raw("TAB!")],
        vec![], &[(3, 0, 4)], Some(3), 20, "absolute-centre", Style::default());
    let drawn = text(&layout);
    assert_eq!(drawn.find("TAB!"), Some(10));
    assert!(layout.ranges.contains(&(R::Window(3), 10, 14)));
}

#[test]
fn tiny_terminal_has_only_bounded_nonempty_hit_regions() {
    for width in 0..=12 {
        let layout = fixture(6, width, "left");
        assert!(spans_visual_width(&layout.spans) <= width);
        for (_, start, end) in layout.ranges {
            assert!(start < end && end as usize <= width);
        }
    }
}
