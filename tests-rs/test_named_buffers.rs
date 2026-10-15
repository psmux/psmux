// Named Paste Buffer Tests
// Proves named buffer support works exactly like tmux:
//   - set-buffer -b name stores in named_buffers HashMap
//   - show-buffer -b name retrieves from named_buffers
//   - delete-buffer -b name removes from named_buffers
//   - list-buffers shows both positional and named buffers
//   - Named buffers are independent of the positional stack
//   - Overwriting a named buffer replaces only that entry
//   - Positional (no -b) operations are unchanged

#[allow(unused_imports)]
use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

fn mock_app_with_window() -> AppState {
    let mut app = mock_app();
    app.windows.push(make_window("shell", 0));
    app
}

fn extract_popup(app: &AppState) -> (String, String) {
    match &app.mode {
        Mode::PopupMode { command, output, .. } => (command.clone(), output.clone()),
        other => panic!("Expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ========================================================================
// NAMED BUFFER SET
// ========================================================================

#[test]
fn set_buffer_named_stores_in_hashmap() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b myname HELLO");
    assert!(app.named_buffers.contains_key("myname"), "Named buffer 'myname' should exist");
    assert_eq!(app.named_buffers["myname"], "HELLO");
    assert!(app.paste_buffers.is_empty(), "Positional stack should stay empty");
}

#[test]
fn set_buffer_named_two_independent() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b alpha ALPHA_DATA");
    let _ = execute_command_string(&mut app, "set-buffer -b beta BETA_DATA");
    assert_eq!(app.named_buffers.len(), 2);
    assert_eq!(app.named_buffers["alpha"], "ALPHA_DATA");
    assert_eq!(app.named_buffers["beta"], "BETA_DATA");
    assert!(app.paste_buffers.is_empty());
}

#[test]
fn set_buffer_named_overwrite_replaces_only_that_name() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b buf1 ORIGINAL");
    let _ = execute_command_string(&mut app, "set-buffer -b buf2 OTHER");
    let _ = execute_command_string(&mut app, "set-buffer -b buf1 UPDATED");
    assert_eq!(app.named_buffers["buf1"], "UPDATED", "buf1 should be overwritten");
    assert_eq!(app.named_buffers["buf2"], "OTHER", "buf2 should be untouched");
}

#[test]
fn set_buffer_without_name_goes_to_stack() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer STACK_CONTENT");
    assert_eq!(app.paste_buffers.len(), 1);
    assert_eq!(app.paste_buffers[0], "STACK_CONTENT");
    assert!(app.named_buffers.is_empty());
}

#[test]
fn set_buffer_named_does_not_affect_stack() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    // Fill stack first
    let _ = execute_command_string(&mut app, "set-buffer STACK_ONE");
    let _ = execute_command_string(&mut app, "set-buffer STACK_TWO");
    // Add named buffer
    let _ = execute_command_string(&mut app, "set-buffer -b named NAMED_DATA");
    // Stack should be unchanged
    assert_eq!(app.paste_buffers.len(), 2);
    assert_eq!(app.paste_buffers[0], "STACK_TWO");
    assert_eq!(app.paste_buffers[1], "STACK_ONE");
    // Named buffer should exist
    assert_eq!(app.named_buffers["named"], "NAMED_DATA");
}

#[test]
fn setb_alias_named() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "setb -b alias_buf ALIAS_CONTENT");
    assert_eq!(app.named_buffers["alias_buf"], "ALIAS_CONTENT");
}

// ========================================================================
// NAMED BUFFER SHOW
// ========================================================================

#[test]
fn show_buffer_named_retrieves_correct_content() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.named_buffers.insert("test_buf".to_string(), "TEST_CONTENT".to_string());
    app.paste_add("STACK_TOP".to_string());
    let _ = execute_command_string(&mut app, "show-buffer -b test_buf");
    let (cmd, out) = extract_popup(&app);
    assert_eq!(cmd, "show-buffer");
    assert_eq!(out, "TEST_CONTENT", "Should show named buffer, not stack top");
}

#[test]
fn show_buffer_without_name_shows_stack_top() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_add("STACK_TOP".to_string());
    app.named_buffers.insert("other".to_string(), "OTHER_CONTENT".to_string());
    let _ = execute_command_string(&mut app, "show-buffer");
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "STACK_TOP", "No -b should show stack top");
}

#[test]
fn show_buffer_named_nonexistent_no_popup() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "show-buffer -b nonexistent");
    // Should not enter popup mode for nonexistent buffer
    assert!(
        !matches!(app.mode, Mode::PopupMode { .. }),
        "show-buffer for nonexistent named buffer should not show popup"
    );
}

// Issue #818: tmux has no index form of -b. `-b 1` is the buffer named 1
// (paste_get_name), and an automatic buffer is found by the name it was
// added with.
#[test]
fn show_buffer_numeric_name_is_a_name_not_an_index() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_set_stack(&["ZERO", "ONE", "TWO"]);
    let _ = execute_command_string(&mut app, "show-buffer -b 1");
    assert!(!matches!(app.mode, Mode::PopupMode { .. }), "no buffer is named 1");
    let _ = execute_command_string(&mut app, "set-buffer -b 1 NAMED_ONE");
    let _ = execute_command_string(&mut app, "show-buffer -b 1");
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "NAMED_ONE");
}

#[test]
fn show_buffer_finds_an_automatic_buffer_by_its_name() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer AUTO1");
    let _ = execute_command_string(&mut app, "set-buffer AUTO2");
    let _ = execute_command_string(&mut app, "show-buffer -b buffer0");
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "AUTO1", "buffer0 is the oldest and keeps its name");
}

#[test]
fn automatic_names_are_fixed_count_up_and_skip_names_in_use() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b buffer0 TAKEN");
    let _ = execute_command_string(&mut app, "set-buffer A");
    let _ = execute_command_string(&mut app, "set-buffer B");
    let names: Vec<&str> = app.paste_buffers.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["buffer2", "buffer1"], "buffer0 is in use, so A is buffer1");
    let _ = execute_command_string(&mut app, "delete-buffer -b buffer2");
    let _ = execute_command_string(&mut app, "set-buffer C");
    assert_eq!(app.paste_buffers[0].name, "buffer3", "the counter never goes back");
    assert_eq!(app.paste_buffers[1].name, "buffer1", "an older buffer is not renamed");
}

#[test]
fn set_buffer_named_replaces_an_automatic_buffer_of_that_name() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer AUTO1");
    let _ = execute_command_string(&mut app, "set-buffer AUTO2");
    let _ = execute_command_string(&mut app, "set-buffer -b buffer0 REPLACED");
    assert_eq!(app.paste_get("buffer0"), Some("REPLACED"));
    assert_eq!(app.paste_buffers.len() + app.named_buffers.len(), 2, "no second buffer0");
    let _ = execute_command_string(&mut app, "set-buffer -a -b buffer1 +APP");
    assert_eq!(app.paste_get("buffer1"), Some("AUTO2+APP"));
    assert_eq!(app.paste_buffers.len() + app.named_buffers.len(), 2);
}

#[test]
fn set_buffer_empty_adds_nothing() {
    let mut app = mock_app_with_window();
    app.paste_add(String::new());
    assert!(app.paste_buffers.is_empty(), "paste.c:165: empty data adds no buffer");
    app.paste_set("n", String::new());
    assert!(app.named_buffers.is_empty());
}

#[test]
fn list_buffers_names_match_buffer_name_format() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer AUTO1");
    let _ = execute_command_string(&mut app, "set-buffer AUTO2");
    let listing = app.paste_listing(false);
    let plain: Vec<&str> = listing.lines().map(|l| l.split(':').next().unwrap()).collect();
    let mut formatted = Vec::new();
    for i in 0..app.paste_buffers.len() {
        crate::format::set_buffer_idx_override(Some(i));
        formatted.push(crate::format::expand_format("#{buffer_name}", &app));
        crate::format::set_buffer_idx_override(None);
    }
    assert_eq!(plain, vec!["buffer1", "buffer0"]);
    assert_eq!(formatted, vec!["buffer1".to_string(), "buffer0".to_string()]);
}

#[test]
fn chooser_line_parses_back_to_its_name() {
    let mut app = mock_app_with_window();
    app.paste_add("two\nlines".to_string());
    app.paste_set("my: odd", "x".to_string());
    let listing = app.paste_listing(true);
    let parsed: Vec<_> = listing.lines().filter_map(crate::client::parse_buffer_line).collect();
    assert_eq!(parsed[0], ("buffer0".to_string(), 9, "two\\nlines".to_string()));
    assert_eq!(parsed[1], ("my: odd".to_string(), 1, "x".to_string()));
}

// ========================================================================
// NAMED BUFFER DELETE
// ========================================================================

#[test]
fn delete_buffer_named_removes_only_that_name() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.named_buffers.insert("keep".to_string(), "KEEP_DATA".to_string());
    app.named_buffers.insert("remove".to_string(), "REMOVE_DATA".to_string());
    let _ = execute_command_string(&mut app, "delete-buffer -b remove");
    assert!(!app.named_buffers.contains_key("remove"), "remove should be deleted");
    assert!(app.named_buffers.contains_key("keep"), "keep should remain");
    assert_eq!(app.named_buffers["keep"], "KEEP_DATA");
}

#[test]
fn delete_buffer_without_name_removes_stack_top() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_set_stack(&["A", "B"]);
    app.named_buffers.insert("named".to_string(), "NAMED".to_string());
    let _ = execute_command_string(&mut app, "delete-buffer");
    assert_eq!(app.paste_buffers, vec!["B"], "Should remove stack top");
    assert!(app.named_buffers.contains_key("named"), "Named buffers should be untouched");
}

// Issue #818: `-b 1` is the buffer named 1. With the stack C, B, A
// (buffer2, buffer1, buffer0) it removes nothing; `-b buffer1` removes B.
#[test]
fn delete_buffer_numeric_name_is_a_name_not_an_index() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_set_stack(&["A", "B", "C"]);
    let _ = execute_command_string(&mut app, "delete-buffer -b 1");
    assert_eq!(app.paste_buffers, vec!["A", "B", "C"], "no buffer is named 1");
    let _ = execute_command_string(&mut app, "delete-buffer -b buffer1");
    assert_eq!(app.paste_buffers, vec!["A", "C"], "buffer1 holds B");
}

#[test]
fn deleteb_alias_named() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.named_buffers.insert("target".to_string(), "DATA".to_string());
    let _ = execute_command_string(&mut app, "deleteb -b target");
    assert!(!app.named_buffers.contains_key("target"));
}

// ========================================================================
// LIST BUFFERS
// ========================================================================

#[test]
fn list_buffers_shows_both_stack_and_named() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.paste_set_stack(&["STACK_DATA"]);
    app.named_buffers.insert("custom".to_string(), "CUSTOM_DATA".to_string());
    let _ = execute_command_string(&mut app, "list-buffers");
    let (_, out) = extract_popup(&app);
    assert!(out.contains("buffer0"), "Should show positional buffer0");
    assert!(out.contains("STACK_DATA"), "Should show stack data preview");
    assert!(out.contains("custom"), "Should show named buffer");
    assert!(out.contains("CUSTOM_DATA"), "Should show named data preview");
}

#[test]
fn list_buffers_empty_shows_no_buffers() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "list-buffers");
    let (_, out) = extract_popup(&app);
    assert!(out.contains("no buffers"), "Should show 'no buffers' when empty");
}

#[test]
fn list_buffers_named_sorted_alphabetically() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    app.named_buffers.insert("zebra".to_string(), "Z_DATA".to_string());
    app.named_buffers.insert("alpha".to_string(), "A_DATA".to_string());
    app.named_buffers.insert("middle".to_string(), "M_DATA".to_string());
    let _ = execute_command_string(&mut app, "list-buffers");
    let (_, out) = extract_popup(&app);
    let alpha_pos = out.find("alpha").unwrap();
    let middle_pos = out.find("middle").unwrap();
    let zebra_pos = out.find("zebra").unwrap();
    assert!(alpha_pos < middle_pos, "alpha should appear before middle");
    assert!(middle_pos < zebra_pos, "middle should appear before zebra");
}

// ========================================================================
// ROUNDTRIP: SET + SHOW + DELETE
// ========================================================================

#[test]
fn named_buffer_full_roundtrip() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    // Set named buffer
    let _ = execute_command_string(&mut app, "set-buffer -b roundtrip HELLO_WORLD");
    assert_eq!(app.named_buffers["roundtrip"], "HELLO_WORLD");
    // Show named buffer
    let _ = execute_command_string(&mut app, "show-buffer -b roundtrip");
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "HELLO_WORLD");
    // Overwrite
    app.mode = Mode::Passthrough;
    let _ = execute_command_string(&mut app, "set-buffer -b roundtrip UPDATED");
    assert_eq!(app.named_buffers["roundtrip"], "UPDATED");
    // Verify show reflects update
    let _ = execute_command_string(&mut app, "show-buffer -b roundtrip");
    let (_, out) = extract_popup(&app);
    assert_eq!(out, "UPDATED");
    // Delete
    app.mode = Mode::Passthrough;
    let _ = execute_command_string(&mut app, "delete-buffer -b roundtrip");
    assert!(!app.named_buffers.contains_key("roundtrip"));
    // Show after delete should not produce popup
    let _ = execute_command_string(&mut app, "show-buffer -b roundtrip");
    // mode should still be passthrough (no popup for nonexistent buffer)
    assert!(!matches!(app.mode, Mode::PopupMode { .. }));
}

// ========================================================================
// MIXED OPERATIONS: Named + Positional
// ========================================================================

#[test]
fn mixed_named_and_positional_independent() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    // Set positional
    let _ = execute_command_string(&mut app, "set-buffer POS_A");
    let _ = execute_command_string(&mut app, "set-buffer POS_B");
    // Set named
    let _ = execute_command_string(&mut app, "set-buffer -b named1 NAMED_A");
    let _ = execute_command_string(&mut app, "set-buffer -b named2 NAMED_B");
    // Verify positional stack
    assert_eq!(app.paste_buffers.len(), 2);
    assert_eq!(app.paste_buffers[0], "POS_B");
    assert_eq!(app.paste_buffers[1], "POS_A");
    // Verify named
    assert_eq!(app.named_buffers.len(), 2);
    assert_eq!(app.named_buffers["named1"], "NAMED_A");
    assert_eq!(app.named_buffers["named2"], "NAMED_B");
    // Delete positional should not affect named
    let _ = execute_command_string(&mut app, "delete-buffer");
    assert_eq!(app.paste_buffers.len(), 1);
    assert_eq!(app.paste_buffers[0], "POS_A");
    assert_eq!(app.named_buffers.len(), 2, "Named buffers should be untouched");
    // Delete named should not affect positional
    let _ = execute_command_string(&mut app, "delete-buffer -b named1");
    assert_eq!(app.paste_buffers.len(), 1, "Positional should be untouched");
    assert_eq!(app.named_buffers.len(), 1);
    assert!(!app.named_buffers.contains_key("named1"));
    assert!(app.named_buffers.contains_key("named2"));
}

// ========================================================================
// EDGE CASES
// ========================================================================

#[test]
fn set_buffer_named_empty_content() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    // set-buffer -b empty (no content after name)
    let _ = execute_command_string(&mut app, "set-buffer -b empty_buf");
    // With no content, set-buffer should not create a named buffer
    // (the content is None since there's no positional arg)
    assert!(!app.named_buffers.contains_key("empty_buf"),
        "set-buffer with no content should not create entry");
}

#[test]
fn set_buffer_named_content_with_spaces() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    let _ = execute_command_string(&mut app, "set-buffer -b spaced hello world test");
    assert_eq!(app.named_buffers["spaced"], "hello world test",
        "Content after -b name should be joined with spaces");
}

#[test]
fn named_buffers_not_subject_to_10_cap() {
    let mut app = mock_app_with_window();
    app.control_port = None;
    // Create 15 named buffers
    for i in 0..15 {
        let _ = execute_command_string(&mut app, &format!("set-buffer -b nb{} content{}", i, i));
    }
    assert_eq!(app.named_buffers.len(), 15,
        "Named buffers should NOT be capped at 10 (unlike positional stack)");
    // Verify all exist
    for i in 0..15 {
        assert!(app.named_buffers.contains_key(&format!("nb{}", i)));
    }
}
