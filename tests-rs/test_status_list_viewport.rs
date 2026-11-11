use super::*;
use ratatui::style::Color;

fn text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

#[test]
fn source_columns_follow_the_renderers_grapheme_boundaries() {
    let (left, right) = markers();
    for prefix in ["لا", "👩\u{200d}💻\u{ad}"] {
        let span = Span::raw(prefix);
        let prefix_width: usize = span.styled_graphemes(Style::default())
            .map(|g| usize::from(g.symbol.cell_width())).sum();
        assert_eq!(spans_visual_width(&[span.clone()]), prefix_width);
        let source = [span, Span::raw("ACTIVE"), Span::raw(" tail tail")];
        let layout = layout_status_list(
            &source, 8, prefix_width..prefix_width + 6, &left, &right,
        );
        assert_eq!(text(&layout.spans), "<ACTIVE>");
        assert_eq!(layout.project_range(prefix_width, prefix_width + 6), Some((1, 7)));
        assert_eq!(spans_visual_width(&layout.spans), 8);
    }
}

// Expected strings marked "tmux" were drawn by a real tmux client with the
// same status-format and width.

#[test]
fn custom_focus_runs_to_nolist_and_is_centred() {
    // tmux: |<CD>| (focus 2..8 ends at nolist, middle 5)
    let result = layout_format_line(
        "#[list=left-marker]<#[list=right-marker]>#[list=on]#[range=window|0]界#[norange]#[list=focus]#[range=window|1]ABCDEF#[norange]#[nolist]",
        4,
        Style::default(),
    );
    assert_eq!(vt100::str_width("界"), 2);
    assert_eq!(text(&result.spans), "<CD>");
    assert_eq!(result.ranges, vec![(StatusRangeType::Window(1), 1, 3)]);
}

#[test]
fn custom_left_partial_glyph_keeps_following_columns() {
    // The left marker covers column 1, the right half of 界 at 1..3 is blanked.
    let result = layout_format_line(
        "#[list=left-marker]<#[list=right-marker]>#[list=on]#[range=window|0]x界#[norange]#[list=focus]#[range=window|1]A#[norange list=on]BCDEF#[nolist]",
        4,
        Style::default(),
    );
    assert_eq!(text(&result.spans), "< A>");
    assert_eq!(result.ranges, vec![(StatusRangeType::Window(1), 2, 3)]);
}

#[test]
fn custom_focus_ended_by_list_on_matches_tmux() {
    for (format, width, expected) in [
        // tmux: |<EFG>|
        ("#[list=left-marker]<#[list=right-marker]>#[list=on]ABCDE#[list=focus]F#[list=on]GHIJ#[nolist]", 5, "<EFG>"),
        // tmux: |<GHIJ|
        ("#[list=left-marker]<#[list=right-marker]>#[list=on]ABCDEFGHI#[list=focus]J#[list=on]#[nolist]", 5, "<GHIJ"),
        // tmux: |ABCD>|
        ("#[list=left-marker]<#[list=right-marker]>#[list=on]AB#[list=focus]C#[list=on]DEFGHIJ#[nolist]", 5, "ABCD>"),
        // tmux: |abc>|
        ("#[list=right-marker]>#[list=on]ab#[list=focus]c#[list=on]defghij#[nolist]", 4, "abc>"),
        // tmux: |<JKLMNOPQ>| (centred list without focus keeps its middle)
        ("#[align=centre]#[list=left-marker]<#[list=right-marker]>#[list=on]ABCDEFGHIJKLMNOPQRSTUVWXYZ#[nolist]", 10, "<JKLMNOPQ>"),
        // tmux: |ABCDEFGHI>|
        ("#[align=right]#[list=left-marker]<#[list=right-marker]>#[list=on]ABCDEFGHIJKLMNOPQRSTUVWXYZ#[nolist]", 10, "ABCDEFGHI>"),
    ] {
        let result = layout_format_line(format, width, Style::default());
        assert_eq!(text(&result.spans), expected, "{format}");
    }
}

#[test]
fn custom_right_partial_glyph_does_not_take_later_span_content() {
    let result = layout_format_line(
        "#[list=right-marker]>#[list=on]ABC界#[fg=red]XYZ#[nolist]",
        5,
        Style::default(),
    );
    assert_eq!(vt100::str_width("ABC界XYZ"), 8);
    assert_eq!(text(&result.spans), "ABC >");
}

#[test]
fn custom_range_stops_before_right_marker() {
    let result = layout_format_line(
        "#[list=right-marker]>#[list=on]#[range=window|0]ABCDEFG#[norange]#[nolist]",
        5,
        Style::default(),
    );
    assert_eq!(text(&result.spans), "ABCD>");
    assert_eq!(result.ranges, vec![(StatusRangeType::Window(0), 0, 4)]);
}

#[test]
fn custom_clipped_base_does_not_leave_a_combining_mark() {
    // Focus middle 3 starts the viewport at column 1, past "a" and its mark.
    let result = layout_format_line(
        "#[list=right-marker]>#[list=on]a\u{301}bc#[list=focus]d#[list=on]efghij#[nolist]",
        4,
        Style::default(),
    );
    assert_eq!(text(&result.spans), "bcd>");
}

fn markers() -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    (vec![Span::raw("<")], vec![Span::raw(">")])
}

#[test]
fn fitting_and_empty_lists_are_unchanged() {
    let style = Style::default()
        .fg(Color::Red)
        .bg(Color::Blue)
        .underline_color(Color::Green)
        .add_modifier(Modifier::BOLD | Modifier::ITALIC)
        .remove_modifier(Modifier::DIM);
    let spans = vec![
        Span::styled("0:界", style),
        Span::styled(" | ", Style::default()),
        Span::styled("1:e\u{301}", style),
    ];
    let (left, right) = markers();
    let total = spans_visual_width(&spans);
    for width in [total, total + 1, total + 10] {
        let layout = layout_status_list(&spans, width, 0..1, &left, &right);
        assert_eq!(layout.spans, spans);
        assert_eq!(layout.project_range(0, usize::MAX), Some((0, total)));
        assert_eq!(layout.project_range(total, total + 1), None);
        assert_eq!(layout.project_range(2, 2), None);
        assert_eq!(layout.project_range(5, 1), None);
    }
    for width in [0, 1, 20] {
        let layout = layout_status_list(&[], width, 0..0, &left, &right);
        assert!(layout.spans.is_empty());
        assert_eq!(layout.project_range(0, usize::MAX), None);
    }
}

#[test]
fn point_focus_is_centred_like_tmux() {
    let spans = [Span::raw("ABCDEFGHIJ")];
    let (left, right) = markers();
    for (focus, expected, source, output) in [
        (0, "ABCD>", 0..4, (0, 4)),
        (2, "ABCD>", 0..4, (0, 4)),
        (3, "<CDE>", 2..5, (1, 4)),
        (5, "<EFG>", 4..7, (1, 4)),
        (9, "<GHIJ", 6..10, (1, 5)),
        (10, "<GHIJ", 6..10, (1, 5)),
    ] {
        let layout = layout_status_list(&spans, 5, focus..focus, &left, &right);
        assert_eq!(text(&layout.spans), expected, "focus {focus}");
        assert_eq!(layout.project_range(source.start, source.end), Some(output));
        assert_eq!(layout.project_range(0, usize::MAX), Some(output));
    }
}

#[test]
fn selected_interval_is_contained_when_it_fits_between_markers() {
    let source = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let spans = [Span::raw(source)];
    let (left, right) = markers();
    for width in 3..=14 {
        assert!(source.len() > width);
        for start in 0..source.len() {
            for len in 1..=(width - 2).min(source.len() - start) {
                let end = start + len;
                assert!(len <= width - 2);
                let layout = layout_status_list(&spans, width, start..end, &left, &right);
                let (a, b) = layout.project_range(start, end).unwrap();
                let rendered = text(&layout.spans);
                assert_eq!(
                    &rendered[a..b],
                    &source[start..end],
                    "width {width}, {start}..{end}"
                );
                assert_eq!(b - a, len);
                assert!(spans_visual_width(&layout.spans) <= width);
            }
        }
    }
}

#[test]
fn oversized_selected_interval_shows_its_middle() {
    // tmux centres the middle of the focus even when it is wider than the list.
    let spans = [Span::raw("ABCDEFGHIJKLMNOPQRSTUVWXYZ")];
    let (left, right) = markers();
    for (focus, expected) in [(0..12, "<FGH>"), (5..20, "<LMN>"), (20..26, "<WXYZ")] {
        assert!(focus.len() > 5);
        let layout = layout_status_list(&spans, 5, focus.clone(), &left, &right);
        assert_eq!(text(&layout.spans), expected);
        assert!(layout.project_range(focus.start, focus.end).is_some());
    }
}

#[test]
fn zero_one_and_two_columns_never_give_all_space_to_markers() {
    let spans = [Span::raw("ABCDEFGHIJ")];
    let (left, right) = markers();
    for (width, focus, expected) in [
        (0, 0, ""),
        (0, 5, ""),
        (1, 0, "A"),
        (1, 5, "F"),
        (1, 9, "J"),
        (2, 0, "A>"),
        (2, 5, "<F"),
        (2, 9, "<J"),
    ] {
        let layout = layout_status_list(&spans, width, focus..focus, &left, &right);
        assert_eq!(text(&layout.spans), expected);
        assert!(spans_visual_width(&layout.spans) <= width);
        assert_eq!(layout.project_range(0, usize::MAX).is_some(), width > 0);
    }
}

#[test]
fn arbitrary_marker_widths_are_measured_and_omitted_whole() {
    let spans = [Span::raw("ABCDEFGHIJKLMNO")];
    let left = [Span::styled("左", Style::default().fg(Color::Red))];
    let right = [Span::styled("右>", Style::default().fg(Color::Blue))];
    assert_eq!(spans_visual_width(&left), 2);
    assert_eq!(spans_visual_width(&right), 3);
    let middle = layout_status_list(&spans, 8, 6..6, &left, &right);
    assert_eq!(text(&middle.spans), "左EFG右>");
    assert_eq!(middle.spans.first(), left.first());
    assert_eq!(middle.spans.last(), right.first());
    assert_eq!(middle.project_range(0, usize::MAX), Some((2, 5)));
    for width in 0..=8 {
        for focus in [0, 6, 14] {
            let layout = layout_status_list(&spans, width, focus..focus, &left, &right);
            assert!(spans_visual_width(&layout.spans) <= width);
            if width > 0 {
                assert!(layout.project_range(0, usize::MAX).is_some());
            }
        }
    }
}

#[test]
fn empty_markers_still_scroll_and_map_content() {
    let spans = [Span::raw("ABCDEFGHIJ")];
    let layout = layout_status_list(&spans, 4, 5..8, &[], &[]);
    assert_eq!(text(&layout.spans), "EFGH");
    assert_eq!(layout.project_range(5, 8), Some((1, 4)));
    assert_eq!(layout.project_range(0, 4), None);
    assert_eq!(layout.project_range(8, 10), None);
}

#[test]
fn clipped_content_and_separators_keep_all_style_fields() {
    let tab = Style::default()
        .fg(Color::Yellow)
        .bg(Color::Blue)
        .underline_color(Color::Red)
        .add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT)
        .remove_modifier(Modifier::ITALIC);
    let separator = Style::default()
        .fg(Color::Magenta)
        .add_modifier(Modifier::REVERSED);
    let spans = [
        Span::styled("ABCDE", tab),
        Span::styled(" | ", separator),
        Span::styled("FGHIJ", tab),
    ];
    let (left, right) = markers();
    let layout = layout_status_list(&spans, 8, 4..9, &left, &right);
    assert_eq!(text(&layout.spans), "<DE | F>");
    assert_eq!(layout.spans[1], Span::styled("DE", tab));
    assert_eq!(layout.spans[2], spans[1]);
    assert_eq!(layout.spans[3], Span::styled("F", tab));
    assert_eq!(layout.project_range(0, 5), Some((1, 3)));
    assert_eq!(layout.project_range(8, 13), Some((6, 7)));
}

#[test]
fn unicode_boundary_blanks_are_not_clickable() {
    let (left, right) = markers();
    let spans = [Span::raw("x界ABCDEF")];
    let layout = layout_status_list(&spans, 4, 3..3, &left, &right);
    assert_eq!(text(&layout.spans), "< A>");
    assert_eq!(layout.project_range(1, 3), None);
    assert_eq!(layout.project_range(3, 9), Some((2, 3)));
    assert_eq!(layout.project_range(0, usize::MAX), Some((2, 3)));

    let spans = [Span::raw("ABC界"), Span::raw("XYZ")];
    let layout = layout_status_list(&spans, 5, 0..0, &left, &right);
    assert_eq!(text(&layout.spans), "ABC >");
    assert_eq!(layout.project_range(3, 5), None);
    assert_eq!(layout.project_range(5, 8), None);
    assert_eq!(layout.project_range(0, usize::MAX), Some((0, 3)));

    let layout = layout_status_list(&[Span::raw("界界")], 1, 1..1, &left, &right);
    assert_eq!(text(&layout.spans), " ");
    assert_eq!(layout.project_range(0, usize::MAX), None);
}

#[test]
fn combining_and_zwj_graphemes_survive_only_with_their_base() {
    let (left, right) = markers();
    for glyph in ["e\u{301}", "👩\u{200d}💻", "✈\u{fe0f}"] {
        let glyph_width = vt100::str_width(glyph);
        assert!(glyph_width > 0 && glyph_width <= 2);
        let source = [Span::raw(format!("AB{glyph}CDEFG"))];
        let layout =
            layout_status_list(&source, glyph_width + 3, 2..2 + glyph_width, &left, &right);
        assert!(text(&layout.spans).contains(glyph));
        assert_eq!(
            layout.project_range(2, 2 + glyph_width).map(|(a, b)| b - a),
            Some(glyph_width)
        );
        assert!(spans_visual_width(&layout.spans) <= glyph_width + 3);
    }
    let source = [Span::raw("a"), Span::raw("\u{301}bcdefgh")];
    let layout = layout_status_list(&source, 4, 3..3, &[], &right);
    assert_eq!(text(&layout.spans), "bcd>");
}

#[test]
fn drawn_unicode_markers_are_outside_window_hit_ranges() {
    use ratatui::buffer::Buffer;
    use ratatui::widgets::{Paragraph, Widget};

    let (left, right) = markers();
    let area = Rect::new(0, 0, 6, 1);
    for active in ["\u{ad}BCDEFGHIJKLMNOP", "ｶﾞBCDEFGHIJKLMNOP", "👩\u{200d}💻\u{ad}BCDEFGHIJKLMNOP", "لاBCDEFGHIJKLMNOP"] {
        let source = [Span::raw("AA"), Span::raw(active)];
        let end = spans_visual_width(&source);
        assert!(end - 2 > area.width as usize, "precondition: active tab overflows");
        let layout = layout_status_list(&source, area.width as usize, 2..end, &left, &right);
        let range = layout.project_range(2, end).unwrap();
        let mut buffer = Buffer::empty(area);
        Paragraph::new(Line::from(layout.spans)).render(area, &mut buffer);
        assert_eq!(buffer[(0, 0)].symbol(), "<", "{active:?}");
        assert_eq!(buffer[(5, 0)].symbol(), ">", "{active:?}");
        assert_eq!(range, (1, 5), "{active:?}");

        let format = format!("#[list=right-marker]>#[list=on]AA#[list=focus]#[range=window|1]{active}#[norange]#[nolist]");
        let custom = layout_format_line(&format, area.width as usize, Style::default());
        let mut buffer = Buffer::empty(area);
        Paragraph::new(Line::from(custom.spans)).render(area, &mut buffer);
        assert_eq!(buffer[(5, 0)].symbol(), ">", "custom {active:?}");
        // The focus runs to nolist, so the viewport is past "AA".
        assert_eq!(custom.ranges, vec![(StatusRangeType::Window(1), 0, 5)]);
    }
}

#[test]
fn source_coordinates_beyond_u16_do_not_wrap() {
    let prefix = "x".repeat(70_000);
    assert!(prefix.len() > u16::MAX as usize);
    let source = [
        Span::raw(prefix),
        Span::raw("ACTIVE"),
        Span::raw(" suffix suffix"),
    ];
    let (left, right) = markers();
    let layout = layout_status_list(&source, 10, 70_000..70_006, &left, &right);
    assert_eq!(text(&layout.spans), "<xACTIVE >");
    assert_eq!(layout.project_range(70_000, 70_006), Some((2, 8)));
    assert_eq!(layout.project_range(0, 69_999), None);
    assert_eq!(layout.project_range(70_006, usize::MAX), Some((8, 9)));
}

#[test]
fn custom_list_ranges_follow_section_offsets_without_claiming_markers() {
    // tmux: |LLABCD>RRR| for left and right, |LLCDEF>RRR| for centre.
    for (align, expected, drawn) in [
        ("left", (2, 6), "LLABCD>RRR"),
        ("centre", (2, 6), "LLCDEF>RRR"),
        ("right", (2, 6), "LLABCD>RRR"),
    ] {
        let format = format!("#[align=left]LL#[align={align}]#[list=right-marker]>#[list=on]#[range=window|0]ABCDEFGH#[norange]#[nolist]#[align=right]RRR");
        let result = layout_format_line(&format, 10, Style::default());
        assert_eq!(spans_visual_width(&result.spans), 10);
        assert_eq!(text(&result.spans), drawn, "align {align}");
        assert_eq!(
            result.ranges,
            vec![(StatusRangeType::Window(0), expected.0, expected.1)],
            "align {align}"
        );
    }
}

#[test]
fn custom_text_after_the_list_in_its_section_is_drawn_after_it() {
    // tmux's default status-format with status-justify right puts
    // status-right after the list in the same alignment (the "after" part).
    let format = "#[align=left range=left]LL#[norange]#[align=right list=on]#[list=left-marker]<#[list=right-marker]>#[list=on]#[range=window|0]ABC#[norange] #[list=focus range=window|1]DEF#[norange list=on] #[range=window|2]GHI#[norange]#[nolist align=right range=right]RR#[norange]";
    let result = layout_format_line(format, 20, Style::default());
    // tmux: |LL     ABC DEF GHIRR| and |LL<C DEF >RR|
    assert_eq!(text(&result.spans), "LL     ABC DEF GHIRR");
    let result = layout_format_line(format, 12, Style::default());
    assert_eq!(text(&result.spans), "LL<C DEF >RR");
    assert_eq!(result.ranges, vec![
        (StatusRangeType::Left, 0, 2),
        (StatusRangeType::Window(0), 3, 4),
        (StatusRangeType::Window(1), 5, 8),
        (StatusRangeType::Right, 10, 12),
    ]);
}

#[test]
fn custom_no_allocation_has_no_list_text_or_click_ranges() {
    let format = "#[list=left-marker]<#[list=right-marker]>#[list=on]#[range=window|0]ABCDEFGHIJ#[norange]#[nolist]#[align=right]RIGHT";
    for width in 0..=5 {
        let result = layout_format_line(format, width, Style::default());
        assert!(spans_visual_width(&result.spans) <= width);
        assert!(result.ranges.is_empty());
        assert!(!text(&result.spans).contains('<'));
        assert!(!text(&result.spans).contains('>'));
    }
}

#[test]
fn custom_long_source_ranges_are_projected_before_u16_conversion() {
    let prefix = "x".repeat(70_000);
    let format = format!("#[list=left-marker]<#[list=right-marker]>#[list=on]{prefix}#[list=focus]#[range=window|7]ACTIVE#[norange] tail tail#[nolist]");
    let result = layout_format_line(&format, 10, Style::default());
    // tmux: |<VE tail >| (the focus runs to nolist, so " tail tail" is in it)
    assert_eq!(text(&result.spans), "<VE tail >");
    assert_eq!(result.ranges, vec![(StatusRangeType::Window(7), 1, 3)]);
}
