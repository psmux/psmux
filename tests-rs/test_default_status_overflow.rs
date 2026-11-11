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
            if justify == "absolute-centre" {
                // tmux trims an absolute-centre list only against the full
                // width and draws it over status-left and status-right.
                assert!(!drawn.contains("LEFT") && !drawn.contains("RIGHT"), "{drawn}");
            } else {
                assert!(drawn.starts_with("LEFT"), "{justify}: {drawn}");
                assert!(drawn.ends_with("RIGHT"), "{justify}: {drawn}");
            }
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
    // tmux: |LLLLLLLLTAB!        | (the list is centred on the full width
    // and drawn over the end of status-left)
    assert_eq!(drawn, "LLLLLLLLTAB!        ");
    assert_eq!(layout.ranges, vec![(R::Left, 0, 10), (R::Window(3), 8, 12)]);
}

#[test]
fn matches_tmux_default_status_line() {
    // Drawn by a real tmux client: status-left "[s] ", status-right "RIGHT",
    // fifteen windows named pwsh, 60 columns.
    let tabs_for = |active: usize, sep: &str| {
        let mut tabs = Vec::new();
        let mut ranges = Vec::new();
        let mut col = 0;
        for n in 0..15 {
            if n > 0 { tabs.push(Span::raw(sep.to_string())); col += sep.len(); }
            let flag = if n == active { "*" } else if n == 0 { "-" } else { " " };
            let tab = format!("{n}:pwsh{flag}");
            ranges.push((n, col, col + tab.len()));
            col += tab.len();
            tabs.push(Span::raw(tab));
        }
        (tabs, ranges)
    };
    for (active, justify, sep, width, expected) in [
        (14, "left", " ", 60, "[s] <pwsh  10:pwsh  11:pwsh  12:pwsh  13:pwsh  14:pwsh*RIGHT"),
        (7, "left", " ", 60, "[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT"),
        (0, "left", " ", 60, "[s] 0:pwsh* 1:pwsh  2:pwsh  3:pwsh  4:pwsh  5:pwsh  6:>RIGHT"),
        (7, "centre", " ", 60, "[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT"),
        (7, "right", " ", 60, "[s] <wsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:p>RIGHT"),
        (7, "absolute-centre", " ", 60, "<  4:pwsh  5:pwsh  6:pwsh  7:pwsh* 8:pwsh  9:pwsh  10:pwsh >"),
        (7, "left", " | ", 60, "[s] < 5:pwsh  | 6:pwsh  | 7:pwsh* | 8:pwsh  | 9:pwsh  >RIGHT"),
        (14, "left", " ", 20, "[s] <  14:pwsh*RIGHT"),
        (7, "left", " ", 12, "[s] <w>RIGHT"),
        (7, "left", " ", 11, "[s] <wRIGHT"),
        // No room for the list: status-right keeps its end, as in tmux.
        (7, "left", " ", 7, "[s] GHT"),
        (7, "left", " ", 5, "[s] T"),
        (7, "left", " ", 3, "[s]"),
        (7, "left", " ", 2, "[s"),
        (7, "absolute-centre", " ", 11, "< 7:pwsh* >"),
        (7, "absolute-centre", " ", 7, "<:pwsh>"),
        (7, "absolute-centre", " ", 3, "<w>"),
        (7, "absolute-centre", " ", 2, "<w"),
    ] {
        let (tabs, ranges) = tabs_for(active, sep);
        let layout = layout_default_status_line(vec![Span::raw("[s] ")], tabs,
            vec![Span::raw("RIGHT")], &ranges, Some(active), width, justify, Style::default());
        assert_eq!(text(&layout), expected, "{justify} active {active} width {width}");
    }
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
