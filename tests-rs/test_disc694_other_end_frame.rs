// disc694 section 7 after #232: `o` (other-end) swaps the two ends of a copy
// mode selection, and the cursor stayed drawn on the old end. Nothing asked
// for a frame, and the client's dump-state poll was answered NC because the
// data version added the cursor's and the anchor's columns the same way, so a
// swap on one row left the sum unchanged. An outside display-message used to
// push a frame for it by accident; #232 stopped queries from doing that.

use super::combined_data_version;

#[test]
fn swapping_the_ends_of_a_one_row_selection_changes_the_version() {
    let mut app = crate::types::AppState::new("disc694".to_string());
    app.copy_anchor = Some((4, 22));
    app.copy_pos = Some((4, 26));
    let before = combined_data_version(&app);
    app.copy_anchor = Some((4, 26));
    app.copy_pos = Some((4, 22));
    assert_ne!(before, combined_data_version(&app), "other-end on one row must not read as no change");
}

#[test]
fn swapping_the_ends_of_a_block_changes_the_version() {
    let mut app = crate::types::AppState::new("disc694b".to_string());
    app.copy_anchor = Some((2, 3));
    app.copy_pos = Some((7, 40));
    let before = combined_data_version(&app);
    app.copy_anchor = Some((7, 40));
    app.copy_pos = Some((2, 3));
    assert_ne!(before, combined_data_version(&app));
}

#[test]
fn moving_the_cursor_one_column_changes_the_version() {
    let mut app = crate::types::AppState::new("disc694c".to_string());
    app.copy_pos = Some((4, 26));
    let before = combined_data_version(&app);
    app.copy_pos = Some((4, 27));
    assert_ne!(before, combined_data_version(&app));
}

#[test]
fn the_selection_shape_and_position_indicator_change_the_version() {
    let mut app = crate::types::AppState::new("disc694d".to_string());
    app.copy_anchor = Some((4, 22));
    app.copy_pos = Some((4, 26));
    let ch = combined_data_version(&app);
    app.copy_selection_mode = crate::types::SelectionMode::Line;
    let line = combined_data_version(&app);
    app.copy_selection_mode = crate::types::SelectionMode::Rect;
    let rect = combined_data_version(&app);
    assert_ne!(ch, line, "V keeps both ends but changes what is drawn");
    assert_ne!(line, rect);
    assert_ne!(ch, rect, "a rectangle toggle keeps both ends");
    let shown = combined_data_version(&app);
    app.copy_hide_position = true;
    assert_ne!(shown, combined_data_version(&app), "the position indicator going away is a change");
}

/// Every input a mode can take (key, text, paste) asks for its frame while a
/// mode is open, or when it opens or closes one: there is no pane echo.
#[test]
fn keys_text_and_paste_taken_by_a_mode_mark_the_frame_dirty() {
    let src = include_str!("../src/server/mod.rs");
    for arm in ["CtrlReq::SendText(s) => {", "CtrlReq::SendKey(k) => {", "CtrlReq::SendPaste(s) => {"] {
        let i = src.find(arm).unwrap_or_else(|| panic!("{arm} not found"));
        let rest = &src[i + arm.len()..];
        let end = rest.find("\n                CtrlReq::").unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(body.contains("let mode_before = !matches!(app.mode, Mode::Passthrough);"), "{arm}: mode before the input");
        assert!(body.contains("mode_before || !matches!(app.mode, Mode::Passthrough)"), "{arm}: dirty if a mode took it");
    }
}

/// Both routes to other-end ask for their frame.
#[test]
fn other_end_asks_for_a_redraw() {
    let src = include_str!("../src/server/mod.rs");
    let i = src.find("\"other-end\" => {").expect("send -X other-end arm");
    assert!(src[i..i + 400].contains("copy_needs_redraw = true"), "send -X other-end must ask for its frame");
    let inp = include_str!("../src/input.rs");
    let j = inp.find("// Swap cursor and anchor").expect("copy mode o key");
    assert!(inp[j..j + 500].contains("copy_needs_redraw = true"), "the o key must ask for its frame");
}
