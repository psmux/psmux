#[cfg(windows)]
use super::*;

#[cfg(windows)]
fn session_entries(names: &[&str]) -> Vec<(String, String)> {
    names
        .iter()
        .map(|name| (name.to_string(), format!("{}: info", name)))
        .collect()
}

#[cfg(windows)]
#[test]
fn session_filter_matches_names_case_insensitively() {
    let entries = session_entries(&["Alpha", "dev-api", "DEV-web", "production"]);
    assert_eq!(session_filtered_indices(&entries, "dev"), vec![1, 2]);
    assert_eq!(session_filtered_indices(&entries, "ALP"), vec![0]);
}

#[cfg(windows)]
#[test]
fn session_filter_does_not_match_session_info() {
    let entries = vec![
        ("alpha".to_string(), "contains needle in details".to_string()),
        ("needle-session".to_string(), "other details".to_string()),
    ];
    assert_eq!(session_filtered_indices(&entries, "needle"), vec![1]);
}

#[cfg(windows)]
#[test]
fn session_filter_escape_restores_selected_entry_in_full_list() {
    let entries = session_entries(&["alpha", "dev-api", "dev-web", "production"]);
    assert_eq!(
        session_filter_escape_selection(&entries, "dev", 1),
        Some(2),
    );
}

#[cfg(windows)]
#[test]
fn session_filter_escape_without_query_closes_picker() {
    let entries = session_entries(&["alpha"]);
    assert_eq!(session_filter_escape_selection(&entries, "", 0), None);
}

#[cfg(windows)]
#[test]
fn ime_detection_ascii_only() {
    // Pure ASCII text should NOT be detected as IME input
    assert!(!paste_buffer_has_non_ascii("abc"));
    assert!(!paste_buffer_has_non_ascii("hello world"));
    assert!(!paste_buffer_has_non_ascii("12345"));
    assert!(!paste_buffer_has_non_ascii(""));
}

#[cfg(windows)]
#[test]
fn ime_detection_japanese() {
    // Japanese IME input should be detected as non-ASCII
    assert!(paste_buffer_has_non_ascii("日本語"));
    assert!(paste_buffer_has_non_ascii("にほんご"));
    assert!(paste_buffer_has_non_ascii("abc日本語"));
}

#[cfg(windows)]
#[test]
fn ime_detection_chinese() {
    assert!(paste_buffer_has_non_ascii("中文"));
    assert!(paste_buffer_has_non_ascii("你好世界"));
}

#[cfg(windows)]
#[test]
fn ime_detection_korean() {
    assert!(paste_buffer_has_non_ascii("한국어"));
}

#[cfg(windows)]
#[test]
fn ime_detection_mixed() {
    // Mixed ASCII + CJK should be detected as non-ASCII
    assert!(paste_buffer_has_non_ascii("hello世界"));
    assert!(paste_buffer_has_non_ascii("a日b"));
}

#[cfg(windows)]
#[test]
fn flush_paste_pend_ascii_sends_as_paste() {
    // ASCII buffer with ≥3 chars should send as send-paste (paste detection intact)
    let mut buf = String::from("abcdef");
    let mut start: Option<std::time::Instant> = Some(std::time::Instant::now());
    let mut stage2 = true;
    let mut cmds: Vec<String> = Vec::new();
    let mut gesture = PasteGesture::default();
    flush_paste_pend_as_text(&mut buf, &mut start, &mut stage2, &mut cmds, &mut gesture);
    assert_eq!(cmds.len(), 1);
    assert!(cmds[0].starts_with("send-paste "));
}

#[cfg(windows)]
#[test]
fn flush_paste_pend_cjk_sends_as_text() {
    // Non-ASCII buffer should NEVER send as send-paste, even with ≥3 chars.
    // This is the core fix for issue #91.
    let mut buf = String::from("日本語テスト");
    let mut start: Option<std::time::Instant> = Some(std::time::Instant::now());
    let mut stage2 = false;
    let mut cmds: Vec<String> = Vec::new();
    let mut gesture = PasteGesture::default();
    flush_paste_pend_as_text(&mut buf, &mut start, &mut stage2, &mut cmds, &mut gesture);
    // Each character should be sent as individual send-text
    assert!(cmds.len() > 1, "CJK should be sent as individual send-text commands");
    for cmd in &cmds {
        assert!(cmd.starts_with("send-text "), "CJK char should be send-text, got: {}", cmd);
    }
}

#[cfg(windows)]
#[test]
fn flush_paste_pend_short_ascii_sends_as_text() {
    // <3 ASCII chars should be sent as individual keystrokes
    let mut buf = String::from("ab");
    let mut start: Option<std::time::Instant> = Some(std::time::Instant::now());
    let mut stage2 = false;
    let mut cmds: Vec<String> = Vec::new();
    let mut gesture = PasteGesture::default();
    flush_paste_pend_as_text(&mut buf, &mut start, &mut stage2, &mut cmds, &mut gesture);
    assert_eq!(cmds.len(), 2);
    assert!(cmds[0].starts_with("send-text "));
    assert!(cmds[1].starts_with("send-text "));
}

#[cfg(windows)]
#[test]
fn leading_plain_enter_is_buffered_when_paste_detection_on() {
    assert!(should_buffer_leading_paste_control(
        &KeyCode::Enter,
        KeyModifiers::empty(),
        true,
        false,
    ));
}

#[cfg(windows)]
#[test]
fn leading_plain_tab_is_buffered_when_paste_detection_on() {
    assert!(should_buffer_leading_paste_control(
        &KeyCode::Tab,
        KeyModifiers::empty(),
        true,
        false,
    ));
}

#[cfg(windows)]
#[test]
fn leading_enter_not_buffered_when_detection_off() {
    assert!(!should_buffer_leading_paste_control(
        &KeyCode::Enter,
        KeyModifiers::empty(),
        false,
        false,
    ));
}

#[cfg(windows)]
#[test]
fn modified_enter_not_buffered_for_paste() {
    assert!(!should_buffer_leading_paste_control(
        &KeyCode::Enter,
        KeyModifiers::SHIFT,
        true,
        false,
    ));
}

#[cfg(windows)]
#[test]
fn non_control_key_not_buffered_even_when_paste_pending() {
    assert!(!should_buffer_leading_paste_control(
        &KeyCode::Backspace,
        KeyModifiers::empty(),
        true,
        true,
    ));
}

#[cfg(windows)]
#[test]
fn leading_enter_waits_past_zero_latency_flush_when_detection_on() {
    assert!(!should_zero_latency_flush_paste_pend("\n", true, false, false));
}

#[cfg(windows)]
#[test]
fn leading_tab_waits_past_zero_latency_flush_when_detection_on() {
    assert!(!should_zero_latency_flush_paste_pend("\t", true, false, false));
}

#[cfg(windows)]
#[test]
fn leading_control_flushes_immediately_when_detection_off() {
    assert!(should_zero_latency_flush_paste_pend("\n", false, false, false));
    assert!(should_zero_latency_flush_paste_pend("\t", false, false, false));
}

#[cfg(windows)]
#[test]
fn normal_short_typing_still_flushes_immediately() {
    assert!(should_zero_latency_flush_paste_pend("a", true, false, false));
    assert!(should_zero_latency_flush_paste_pend("ab", true, false, false));
}

#[cfg(windows)]
#[test]
fn paste_states_do_not_zero_latency_flush() {
    assert!(!should_zero_latency_flush_paste_pend("a", true, true, false));
    assert!(!should_zero_latency_flush_paste_pend("a", true, false, true));
    assert!(!should_zero_latency_flush_paste_pend("abc", true, false, false));
}

// ── Issue #164: status-format[] must parse inline styles end-to-end ──

/// Verify that status_format strings from JSON deserialization flow through
/// parse_inline_styles correctly and produce styled (not literal) output.
#[cfg(windows)]
#[test]
fn status_format_inline_styles_end_to_end() {
    use ratatui::style::{Color, Style};
    use unicode_width::UnicodeWidthStr;

    // Simulate what the server sends: status_format with style directives
    let status_format: Vec<String> = vec![
        "#[align=left]Custom Line 1".to_string(),
        "#[fg=red]Custom Line 2".to_string(),
    ];

    let sb_base = Style::default().fg(Color::White).bg(Color::Black);

    // Test line 0 (status_format[0]) rendering path
    {
        let use_status_format_0 = !status_format.is_empty() && !status_format[0].is_empty();
        assert!(use_status_format_0, "status_format[0] should be detected as set");

        let fmt0_spans = crate::style::parse_inline_styles(&status_format[0], sb_base);
        assert_eq!(fmt0_spans.len(), 1, "Line 0 should produce 1 span, got {}", fmt0_spans.len());
        assert_eq!(fmt0_spans[0].content.as_ref(), "Custom Line 1",
            "Line 0 should NOT contain literal #[align=left], got: {:?}", fmt0_spans[0].content);
        // align=left is silently consumed, style stays at base
        assert_eq!(fmt0_spans[0].style.fg, Some(Color::White));
        assert_eq!(fmt0_spans[0].style.bg, Some(Color::Black));
    }

    // Test line 1 (status_format[1]) rendering path
    {
        let text = &status_format[1];
        let parsed_spans = crate::style::parse_inline_styles(text, sb_base);
        assert_eq!(parsed_spans.len(), 1, "Line 1 should produce 1 span, got {}", parsed_spans.len());
        assert_eq!(parsed_spans[0].content.as_ref(), "Custom Line 2",
            "Line 1 should NOT contain literal #[fg=red], got: {:?}", parsed_spans[0].content);
        assert_eq!(parsed_spans[0].style.fg, Some(Color::Red),
            "Line 1 fg should be Red (parsed from #[fg=red]), got {:?}", parsed_spans[0].style.fg);
        assert_eq!(parsed_spans[0].style.bg, Some(Color::Black),
            "Line 1 bg should remain Black from base, got {:?}", parsed_spans[0].style.bg);

        // Also verify padding uses visible width, not raw text length
        let visible_w: usize = parsed_spans.iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum();
        assert_eq!(visible_w, 13, "Visible width should be 13 (Custom Line 2), got {}", visible_w);
        // The raw status_format[1] is 23 chars (#[fg=red]Custom Line 2)
        // but visible is only 13 chars — padding must use 13, not 23
        assert!(text.len() > visible_w,
            "Raw text ({}) should be longer than visible width ({}) due to style directives",
            text.len(), visible_w);
    }
}

/// Verify that the JSON server payload correctly round-trips status_format
/// through serde deserialization without mangling style directives.
#[cfg(windows)]
#[test]
fn status_format_json_roundtrip_preserves_styles() {
    // Simulate the JSON fragment the server sends
    let json_fragment = r##"{"status_format":["","#[fg=red]Hello","#[fg=green,bg=blue]World"]}"##;

    #[derive(serde::Deserialize)]
    struct Partial {
        #[serde(default)]
        status_format: Vec<String>,
    }
    let parsed: Partial = serde_json::from_str(json_fragment).unwrap();
    assert_eq!(parsed.status_format.len(), 3);
    assert_eq!(parsed.status_format[0], "");
    assert_eq!(parsed.status_format[1], "#[fg=red]Hello",
        "Style directives must survive JSON roundtrip");
    assert_eq!(parsed.status_format[2], "#[fg=green,bg=blue]World",
        "Multi-directive styles must survive JSON roundtrip");

    // Now verify parse_inline_styles produces correct output from deserialized data
    use ratatui::style::{Color, Style};
    let base = Style::default();

    let spans1 = crate::style::parse_inline_styles(&parsed.status_format[1], base);
    assert_eq!(spans1.len(), 1);
    assert_eq!(spans1[0].content.as_ref(), "Hello");
    assert_eq!(spans1[0].style.fg, Some(Color::Red));

    let spans2 = crate::style::parse_inline_styles(&parsed.status_format[2], base);
    assert_eq!(spans2.len(), 1);
    assert_eq!(spans2[0].content.as_ref(), "World");
    assert_eq!(spans2[0].style.fg, Some(Color::Green));
    assert_eq!(spans2[0].style.bg, Some(Color::Blue));
}

// ── Issue #211: pwsh-mouse-selection helpers ──

/// Helper to create a CellRunJson for tests.
#[cfg(windows)]
fn make_run(text: &str, width: u16) -> crate::layout::CellRunJson {
    crate::layout::CellRunJson {
        text: text.to_string(),
        fg: String::new(),
        bg: String::new(),
        flags: 0,
        width,
        link: None,
        ul: 0,
        ulc: None,
    }
}

/// Helper to create a RowRunsJson for tests.
#[cfg(windows)]
fn make_row(runs: Vec<crate::layout::CellRunJson>) -> crate::layout::RowRunsJson {
    crate::layout::RowRunsJson { runs }
}

#[cfg(windows)]
fn make_leaf(id: usize, rows: &[&str]) -> crate::layout::LayoutJson {
    let cols = rows.first().map(|row| row.chars().count()).unwrap_or(0) as u16;
    crate::layout::LayoutJson::Leaf {
        id,
        rows: rows.len() as u16,
        cols,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: false,
        cursor_shape: 0,
        active: id == 0,
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
        rows_v2: rows
            .iter()
            .map(|row| make_row(vec![make_run(row, cols)]))
            .collect(),
        title: None,
    }
}

#[cfg(windows)]
#[test]
fn normalize_selection_reading_order() {
    // Start before end: no swap
    let (r0, c0, r1, c1) = normalize_selection((2, 1), (5, 3), false);
    assert_eq!((r0, c0, r1, c1), (1, 2, 3, 5));

    // Start after end: swapped
    let (r0, c0, r1, c1) = normalize_selection((5, 3), (2, 1), false);
    assert_eq!((r0, c0, r1, c1), (1, 2, 3, 5));
}

#[cfg(windows)]
#[test]
fn normalize_selection_block_mode() {
    // Block mode: min/max of each axis independently
    let (r0, c0, r1, c1) = normalize_selection((8, 5), (3, 2), true);
    assert_eq!((r0, c0, r1, c1), (2, 3, 5, 8));
}

#[cfg(windows)]
#[test]
fn row_chars_basic() {
    let runs = vec![
        make_run("AB", 2),
        make_run("C", 1),
        make_run(" ", 3),
    ];
    let chars = row_chars(&runs, 6);
    assert_eq!(chars, vec!['A', 'B', 'C', ' ', ' ', ' ']);
}

#[cfg(windows)]
#[test]
fn row_chars_width_clamp() {
    let runs = vec![make_run("ABCDE", 5)];
    let chars = row_chars(&runs, 3);
    assert_eq!(chars, vec!['A', 'B', 'C']);
}

#[cfg(windows)]
#[test]
fn is_word_char_basics() {
    assert!(is_word_char('a'));
    assert!(is_word_char('Z'));
    assert!(is_word_char('0'));
    assert!(is_word_char('_'));
    assert!(!is_word_char(' '));
    assert!(!is_word_char('-'));
    assert!(!is_word_char('.'));
}

#[cfg(windows)]
#[test]
fn char_at_col_basics() {
    let runs = vec![
        make_run("He", 2),
        make_run("llo", 3),
    ];
    assert_eq!(char_at_col(&runs, 0), 'H');
    assert_eq!(char_at_col(&runs, 1), 'e');
    assert_eq!(char_at_col(&runs, 2), 'l');
    assert_eq!(char_at_col(&runs, 3), 'l');
    assert_eq!(char_at_col(&runs, 4), 'o');
    // Out of range returns space
    assert_eq!(char_at_col(&runs, 10), ' ');
}

#[cfg(windows)]
#[test]
fn extract_selection_text_block_mode() {
    // A single leaf pane 10 cols wide, 3 rows
    let layout = crate::layout::LayoutJson::Leaf {
        id: 0,
        rows: 3,
        cols: 10,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: false,
        cursor_shape: 0,
        active: true,
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
        rows_v2: vec![
            make_row(vec![make_run("0123456789", 10)]),
            make_row(vec![make_run("abcdefghij", 10)]),
            make_row(vec![make_run("ABCDEFGHIJ", 10)]),
        ],
        title: None,
    };

    // Block select cols 2..5, rows 0..2
    let text = extract_selection_text(&layout, 10, 3, (2, 0), (5, 2), true, None, "off", "");
    assert_eq!(text, "2345\ncdef\nCDEF");

    // Non-block (reading order) same coordinates should give full intermediate rows
    let text_normal = extract_selection_text(&layout, 10, 3, (2, 0), (5, 2), false, None, "off", "");
    assert_eq!(text_normal, "23456789\nabcdefghij\nABCDEF");
}

#[cfg(windows)]
#[test]
fn extract_selection_text_clips_reading_order_to_origin_pane() {
    let layout = crate::layout::LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            make_leaf(0, &["abcde", "fghij", "klmno"]),
            make_leaf(1, &["ABCDE", "FGHIJ", "KLMNO"]),
        ],
    };
    let pane_clip = ratatui::layout::Rect { x: 0, y: 0, width: 5, height: 3 };

    let text = extract_selection_text(&layout, 11, 3, (1, 0), (3, 2), false, Some(pane_clip), "off", "");

    assert_eq!(text, "bcde\nfghij\nklmn");
}

#[cfg(windows)]
#[test]
fn extract_selection_text_clips_block_mode_to_origin_pane() {
    let layout = crate::layout::LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![
            make_leaf(0, &["abcde", "fghij", "klmno"]),
            make_leaf(1, &["ABCDE", "FGHIJ", "KLMNO"]),
        ],
    };
    let pane_clip = ratatui::layout::Rect { x: 0, y: 0, width: 5, height: 3 };

    let text = extract_selection_text(&layout, 11, 3, (1, 0), (8, 2), true, Some(pane_clip), "off", "");

    assert_eq!(text, "bcde\nghij\nlmno");
}

#[cfg(windows)]
#[test]
fn word_bounds_at_finds_word() {
    let layout = crate::layout::LayoutJson::Leaf {
        id: 0,
        rows: 1,
        cols: 20,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: false,
        cursor_shape: 0,
        active: true,
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
        rows_v2: vec![
            make_row(vec![make_run("hello world_test   ", 19), make_run(" ", 1)]),
        ],
        title: None,
    };

    let pane_rect = ratatui::layout::Rect { x: 0, y: 0, width: 20, height: 1 };

    // Click on 'h' (col 0): word is "hello" -> (0, 4)
    assert_eq!(word_bounds_at(&layout, 20, 1, pane_rect, 0, 0, "off", ""), Some((0, 4)));
    // Click on 'l' (col 3): still "hello" -> (0, 4)
    assert_eq!(word_bounds_at(&layout, 20, 1, pane_rect, 3, 0, "off", ""), Some((0, 4)));
    // Click on space (col 5): no word
    assert_eq!(word_bounds_at(&layout, 20, 1, pane_rect, 5, 0, "off", ""), None);
    // Click on 'w' (col 6): "world_test" -> (6, 15)
    assert_eq!(word_bounds_at(&layout, 20, 1, pane_rect, 6, 0, "off", ""), Some((6, 15)));
    // Click on '_' (col 11): still "world_test" since _ is a word char -> (6, 15)
    assert_eq!(word_bounds_at(&layout, 20, 1, pane_rect, 11, 0, "off", ""), Some((6, 15)));
}

#[cfg(windows)]
#[test]
fn pwsh_mouse_selection_option_default_off() {
    let state = crate::types::AppState::new("test-session".to_string());
    assert!(!state.pwsh_mouse_selection, "pwsh_mouse_selection should default to off");
}

// ── Issue #290: paste must not leak past the command prompt ─────────────
// route_paste_to_overlay is the helper that the Event::Paste branch in the
// client loop delegates to.  When an overlay returns true, the loop skips
// the `send-paste` forwarding, so paste content cannot reach the shell.

#[test]
fn paste_into_command_prompt_inserts_and_advances_cursor() {
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "hello",
        true, &mut command_buf, &mut command_cursor,
        false, &mut rename_buf,
        false, &mut pane_title_buf,
        false, &mut window_idx_buf,
    );
    assert!(consumed, "command_input overlay must consume paste");
    assert_eq!(command_buf, "hello");
    assert_eq!(command_cursor, 5);
}

#[test]
fn paste_into_command_prompt_inserts_at_cursor_position() {
    // User typed "abdef", moved cursor between b and d, then pastes "c".
    let mut command_buf = String::from("abdef");
    let mut command_cursor = 2;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "c",
        true, &mut command_buf, &mut command_cursor,
        false, &mut rename_buf,
        false, &mut pane_title_buf,
        false, &mut window_idx_buf,
    );
    assert!(consumed);
    assert_eq!(command_buf, "abcdef");
    assert_eq!(command_cursor, 3);
}

#[test]
fn paste_with_no_overlay_active_is_not_consumed() {
    // Caller must forward via send-paste when this returns false.
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "hello",
        false, &mut command_buf, &mut command_cursor,
        false, &mut rename_buf,
        false, &mut pane_title_buf,
        false, &mut window_idx_buf,
    );
    assert!(!consumed);
    assert!(command_buf.is_empty());
    assert!(rename_buf.is_empty());
    assert!(pane_title_buf.is_empty());
    assert!(window_idx_buf.is_empty());
}

#[test]
fn paste_into_rename_prompt_appends() {
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::from("foo");
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "bar",
        false, &mut command_buf, &mut command_cursor,
        true, &mut rename_buf,
        false, &mut pane_title_buf,
        false, &mut window_idx_buf,
    );
    assert!(consumed);
    assert_eq!(rename_buf, "foobar");
}

#[test]
fn paste_into_pane_title_appends() {
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::from("title");
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "-suffix",
        false, &mut command_buf, &mut command_cursor,
        false, &mut rename_buf,
        true, &mut pane_title_buf,
        false, &mut window_idx_buf,
    );
    assert!(consumed);
    assert_eq!(pane_title_buf, "title-suffix");
}

#[test]
fn paste_into_window_idx_prompt_keeps_only_digits() {
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "1a2b3",
        false, &mut command_buf, &mut command_cursor,
        false, &mut rename_buf,
        false, &mut pane_title_buf,
        true, &mut window_idx_buf,
    );
    assert!(consumed);
    assert_eq!(window_idx_buf, "123");
}

#[test]
fn paste_command_prompt_takes_precedence_over_other_overlays() {
    // If multiple overlay flags are accidentally true, command_input wins
    // (matches the if/else-if order in the helper).
    let mut command_buf = String::new();
    let mut command_cursor = 0;
    let mut rename_buf = String::new();
    let mut pane_title_buf = String::new();
    let mut window_idx_buf = String::new();
    let consumed = super::route_paste_to_overlay(
        "x",
        true, &mut command_buf, &mut command_cursor,
        true, &mut rename_buf,
        true, &mut pane_title_buf,
        true, &mut window_idx_buf,
    );
    assert!(consumed);
    assert_eq!(command_buf, "x");
    assert!(rename_buf.is_empty());
    assert!(pane_title_buf.is_empty());
    assert!(window_idx_buf.is_empty());
}

// ─── Duplicate paste read-back guard ───────────────────────────────────────
//
// The console host injects a clipboard paste as character events and crossterm
// can emit Event::Paste for the very same keystroke, so the client has more than
// one source for one paste, and the Ctrl+V Release fallback reads the clipboard
// as well.  What was forwarded therefore has to be remembered per gesture, since
// a paste can be split: `C2单元格应显示` arrives as `C2` (flushed immediately as
// typing by the zero-latency path) and then the CJK part (flushed as typing by
// the IME heuristic of issue #91), so the forwarded text is only a fragment of
// the clipboard and comparing the two misses.

#[cfg(windows)]
fn delivered_burst(text: &str, age_ms: u64) -> (String, std::time::Instant) {
    (
        text.to_string(),
        std::time::Instant::now() - std::time::Duration::from_millis(age_ms),
    )
}

#[cfg(windows)]
#[test]
fn a_fragmented_paste_gesture_blocks_the_read_back_of_the_whole_text() {
    let mut gesture = PasteGesture::default();
    gesture.start();
    gesture.record("C2");
    gesture.record("单元格应显示");
    // Only the last fragment is remembered, so the content comparison alone
    // cannot see that this is the paste the clipboard still holds.
    assert!(!duplicates_recent_paste("C2单元格应显示", gesture.recent()));
    assert!(gesture.blocks("C2单元格应显示"));
}

#[cfg(windows)]
#[test]
fn a_fragmented_ascii_formula_blocks_the_read_back_too() {
    // Reported the same way as the CJK case: `=(B3-B2)/B2` split by the
    // zero-latency flush (`=` first) and then forwarded as a paste.
    let mut gesture = PasteGesture::default();
    gesture.start();
    gesture.record("=");
    gesture.record("(B3-B2)/B2");
    assert!(!duplicates_recent_paste("=(B3-B2)/B2", gesture.recent()));
    assert!(gesture.blocks("=(B3-B2)/B2"));
}

#[cfg(windows)]
#[test]
fn a_gesture_that_forwarded_characters_blocks_the_read_back() {
    let mut gesture = PasteGesture::default();
    gesture.record("abc");
    assert!(gesture.blocks("abc"));
    // The host injected this paste; reading the clipboard now would add a
    // second copy of text the pane already has.
    assert!(gesture.blocks("abd"));
}

#[cfg(windows)]
#[test]
fn a_fresh_gesture_starts_clean() {
    let mut gesture = PasteGesture::default();
    gesture.record("abc");
    gesture.start();
    assert!(!gesture.blocks("abc"));
    assert!(!gesture.blocks("xyz"));
}

#[cfg(windows)]
#[test]
fn finishing_a_gesture_forgets_what_it_delivered() {
    let mut gesture = PasteGesture::default();
    gesture.record("abc");
    gesture.finish();
    assert!(!gesture.blocks("abc"));
}

#[cfg(windows)]
#[test]
fn nothing_delivered_is_never_a_duplicate() {
    let gesture = PasteGesture::default();
    assert!(!gesture.blocks("恭喜通关"));
    assert!(!gesture.blocks(""));
}

#[cfg(windows)]
#[test]
fn an_empty_delivery_is_not_a_delivery() {
    let mut gesture = PasteGesture::default();
    gesture.record("");
    assert!(!gesture.injected);
    assert!(!gesture.blocks("abc"));
}

#[cfg(windows)]
#[test]
fn the_same_text_after_the_window_does_not_block_a_read_back() {
    let mut gesture = PasteGesture::default();
    gesture.delivered = Some(delivered_burst("恭喜通关", 400));
    assert!(!gesture.blocks("恭喜通关"));
}

#[cfg(windows)]
#[test]
fn the_content_check_still_matches_an_identical_burst() {
    let recent = delivered_burst("恭喜通关", 5);
    assert!(duplicates_recent_paste("恭喜通关", Some((recent.0.as_str(), recent.1.elapsed()))));
    assert!(!duplicates_recent_paste("恭喜通关！", Some((recent.0.as_str(), recent.1.elapsed()))));
}

#[cfg(windows)]
#[test]
fn the_interrupt_flush_records_on_the_gesture() {
    let mut buf = String::from("恭喜通关");
    let mut start: Option<std::time::Instant> = Some(std::time::Instant::now());
    let mut stage2 = false;
    let mut cmds: Vec<String> = Vec::new();
    let mut gesture = PasteGesture::default();
    flush_paste_pend_as_text(&mut buf, &mut start, &mut stage2, &mut cmds, &mut gesture);
    assert!(cmds.iter().all(|c| c.starts_with("send-text ")),
        "CJK bursts still go out as individual text (issue #91)");
    assert!(gesture.blocks("恭喜通关"),
        "the delivered CJK burst must be remembered so the read-back is dropped");
}

// ── copy-mode cursor vs. the selection it sits on ────────────────────────
// Regression: the copy cursor cell used to be drawn REVERSED even when it was
// one of the selected cells.  A reversed selected cell renders as "text colour
// on the default background", so the last selected cell *looks* unselected and
// the copy is reported as one cell longer than the highlight.

#[cfg(windows)]
#[test]
fn a_cursor_on_a_selected_cell_must_not_be_reversed() {
    // single line, cols 4..=24 selected (the reported case: a path in the
    // middle of the line, cursor on the last cell)
    let start = Some((0u16, 4u16));
    let end = Some((0u16, 24u16));
    assert!(copy_cursor_in_selection(0, 4, start, end, "char"));
    assert!(copy_cursor_in_selection(0, 12, start, end, "char"));
    assert!(copy_cursor_in_selection(0, 24, start, end, "char"));
    // one past either end is outside
    assert!(!copy_cursor_in_selection(0, 3, start, end, "char"));
    assert!(!copy_cursor_in_selection(0, 25, start, end, "char"));
}

#[cfg(windows)]
#[test]
fn a_multi_row_cursor_follows_the_same_pairing_as_the_highlight() {
    // anchor (row 1, col 6) -> endpoint (row 4, col 2), the up/left drag
    let start = Some((1u16, 6u16));
    let end = Some((4u16, 2u16));
    assert!(copy_cursor_in_selection(1, 6, start, end, "char"));
    assert!(copy_cursor_in_selection(1, 30, start, end, "char")); // first row: to the right edge
    assert!(!copy_cursor_in_selection(1, 5, start, end, "char"));
    assert!(copy_cursor_in_selection(2, 0, start, end, "char")); // middle rows: full width
    assert!(copy_cursor_in_selection(4, 2, start, end, "char")); // last row: up to the endpoint
    assert!(!copy_cursor_in_selection(4, 3, start, end, "char"));
    assert!(!copy_cursor_in_selection(5, 0, start, end, "char"));
}

#[cfg(windows)]
#[test]
fn rectangle_and_line_modes_have_their_own_shape() {
    let start = Some((1u16, 10u16));
    let end = Some((3u16, 20u16));
    assert!(copy_cursor_in_selection(2, 15, start, end, "rect"));
    assert!(!copy_cursor_in_selection(2, 9, start, end, "rect"));
    assert!(!copy_cursor_in_selection(2, 21, start, end, "rect"));
    // line mode selects whole rows, whatever the columns are
    assert!(copy_cursor_in_selection(2, 0, start, end, "line"));
    assert!(copy_cursor_in_selection(3, 200, start, end, "line"));
    assert!(!copy_cursor_in_selection(0, 15, start, end, "line"));
}

#[cfg(windows)]
#[test]
fn no_selection_means_the_cursor_is_still_drawn() {
    // keyboard copy mode: nothing selected, so the cursor keeps its REVERSED
    // cell and the host cursor stays where the user is working
    assert!(!copy_cursor_in_selection(3, 3, None, None, "char"));
    assert!(!copy_cursor_in_selection(3, 3, Some((1, 1)), None, "char"));
    assert!(!copy_cursor_in_selection(3, 3, None, Some((5, 5)), "char"));
}
