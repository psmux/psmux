// Discussion #694: tanaeakihiko's audit of the 51 default copy-mode-vi keys
// against tmux. Each section below pins one of the differences it found.

use super::*;
use super::test_copy_mode_snapshot::{app_with_pane, feed, filled, parked_live_term, view_term};

fn scroll(app: &AppState) -> usize {
    view_term(app).lock().map(|p| p.screen().scrollback()).unwrap_or(0)
}

// ── 3. refresh-from-pane ────────────────────────────────────────────────────
//
// Measured on dd695ea with a printer ticking every 400 ms: the drawn screen
// stayed on `tick 12` while the pane reached `tick 28` after `r`, and `r` only
// reset the scroll offset to 0. `r` flipped the #494 freeze off, which stopped
// meaning anything once copy mode moved onto a snapshot (d74f52a).

#[test]
fn refresh_from_pane_brings_new_output_into_the_snapshot() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 100);
    crate::copy_mode::enter_copy_mode(&mut app);
    let before = filled(&view_term(&app));

    feed(&live, "tick", 0, 10);
    assert_eq!(filled(&view_term(&app)), before, "the snapshot holds still while the pane prints");

    crate::copy_mode::refresh_from_pane(&mut app);

    assert_eq!(filled(&view_term(&app)), before + 10, "r must copy the new lines in");
    let parked = parked_live_term(&app).expect("still in copy mode on a snapshot");
    assert!(std::sync::Arc::ptr_eq(&parked, &live), "the live parser stays the one the reader feeds");
    assert!(!std::sync::Arc::ptr_eq(&view_term(&app), &live), "copy mode still reads a snapshot");
}

#[test]
fn refresh_from_pane_keeps_the_line_at_the_top_of_the_view() {
    // tmux 3.7 keeps `oy_from_top` (window-copy.c): the reader stays on the
    // same lines and the new output lands underneath.
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 200);
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::copy_mode::scroll_copy_up(&mut app, 30);
    assert_eq!(app.copy_scroll_offset, 30);

    feed(&live, "tick", 0, 8);
    crate::copy_mode::refresh_from_pane(&mut app);

    assert_eq!(app.copy_scroll_offset, 38, "8 new lines underneath push the offset by 8");
    assert_eq!(scroll(&app), 38, "the snapshot parser holds the same offset");
}

#[test]
fn scrolling_still_works_after_refresh_from_pane() {
    // The old toggle left scrolling nearly dead: 20 presses of cursor-up moved
    // the offset by one.
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 200);
    crate::copy_mode::enter_copy_mode(&mut app);
    feed(&live, "tick", 0, 5);
    crate::copy_mode::refresh_from_pane(&mut app);
    let at = app.copy_scroll_offset;
    crate::copy_mode::scroll_copy_up(&mut app, 20);
    assert_eq!(app.copy_scroll_offset, at + 20);
}

#[test]
fn refresh_from_pane_clears_the_selection_and_stays_in_copy_mode() {
    // window_copy_size_changed runs window_copy_clear_selection after the
    // re-clone; measured in tmux 3.4: selection_present 1 -> 0, in_mode 1.
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 60);
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_anchor = Some((3, 0));
    crate::copy_mode::refresh_from_pane(&mut app);
    assert!(app.copy_anchor.is_none());
    assert!(matches!(app.mode, Mode::CopyMode));
}

#[test]
fn refreshed_offset_follows_tmux_3_7() {
    // Same line on top: 100 lines of history at offset 0, 8 more arrive.
    assert_eq!(crate::copy_mode::refreshed_offset(100, 0, 108), (8, false));
    // Scrolled 30 up.
    assert_eq!(crate::copy_mode::refreshed_offset(100, 30, 108), (38, false));
    // The line on top was evicted: park on the oldest line.
    assert_eq!(crate::copy_mode::refreshed_offset(100, 90, 5), (5, true));
    // An offset past the history is clamped first, as tmux clamps oy.
    assert_eq!(crate::copy_mode::refreshed_offset(100, 500, 100), (100, false));
}

#[test]
fn refresh_toggle_follows_output_only_at_the_bottom() {
    // tmux after 3.7c: refresh-on rebuilds on a timer and follows new output
    // while the cursor is on the last row at offset 0.
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 100);
    crate::copy_mode::enter_copy_mode(&mut app);
    let rows = pane_of_rows(&app);
    app.copy_pos = Some((rows - 1, 0));
    crate::copy_mode::toggle_refresh(&mut app);
    assert!(app.copy_refresh_live);

    feed(&live, "tick", 0, 6);
    bump_version(&app);
    assert!(crate::copy_mode::tick_auto_refresh(&mut app));
    assert_eq!(app.copy_scroll_offset, 0, "at the bottom the view follows");
    assert_eq!(filled(&view_term(&app)), filled(&live));

    // Scrolled up, a refresh keeps the place instead.
    crate::copy_mode::scroll_copy_up(&mut app, 10);
    feed(&live, "tick", 6, 9);
    bump_version(&app);
    assert!(crate::copy_mode::tick_auto_refresh(&mut app));
    assert_eq!(app.copy_scroll_offset, 13);

    // A selection pauses it.
    app.copy_anchor = Some((0, 0));
    feed(&live, "tick", 9, 12);
    bump_version(&app);
    assert!(!crate::copy_mode::tick_auto_refresh(&mut app));

    crate::copy_mode::toggle_refresh(&mut app);
    assert!(!app.copy_refresh_live);
}

fn pane_of_rows(app: &AppState) -> u16 {
    super::test_copy_mode_snapshot::pane_of(app).map(|p| p.last_rows).unwrap_or(24)
}

fn bump_version(app: &AppState) {
    if let Some(p) = super::test_copy_mode_snapshot::pane_of(app) {
        p.data_version.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── 1. clear-selection on Escape ────────────────────────────────────────────
//
// Measured on dd695ea with real keystrokes: Space lll then Escape gave
// pane_in_mode 0. tmux binds Escape to clear-selection in copy-mode-vi
// (key-bindings.c:654) and to cancel in copy-mode (:577).

fn vi_app_with_selection() -> AppState {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 60);
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((10, 2));
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    crate::input::send_text_to_active(&mut app, "lll").unwrap();
    assert!(app.copy_anchor.is_some(), "Space starts a selection");
    app
}

#[test]
fn escape_clears_the_selection_and_stays_in_copy_mode_in_vi() {
    let mut app = vi_app_with_selection();
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    assert!(app.copy_anchor.is_none(), "Escape drops the selection");
    assert!(matches!(app.mode, Mode::CopyMode), "and copy mode stays up");
    // With nothing selected it does nothing: tmux 3.4 keeps pane_in_mode 1.
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    // q is the key that leaves.
    crate::input::send_text_to_active(&mut app, "q").unwrap();
    assert!(!app.mode.in_copy());
}

#[test]
fn escape_as_text_clears_the_selection_in_vi() {
    let mut app = vi_app_with_selection();
    crate::input::send_text_to_active(&mut app, "\x1b").unwrap();
    assert!(app.copy_anchor.is_none());
    assert!(matches!(app.mode, Mode::CopyMode));
}

#[test]
fn escape_still_cancels_with_emacs_keys() {
    let mut app = vi_app_with_selection();
    app.mode_keys = "emacs".to_string();
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    assert!(!app.mode.in_copy(), "copy-mode binds Escape to cancel");
}

#[test]
fn clear_selection_keeps_the_rectangle_flag() {
    // tmux's window_copy_clear_selection leaves rectflag alone.
    let mut app = vi_app_with_selection();
    crate::copy_mode::toggle_rectangle(&mut app);
    crate::copy_mode::clear_selection(&mut app);
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Rect);
}

// ── 4a and 8. the search prompt ─────────────────────────────────────────────
//
// Measured on dd695ea with real keystrokes: typing `row 7 alpha` into `/`
// showed `(search down) row7alpha`, C-a then Z gave `abcZ`, and Left made the
// prompt vanish while it stayed open. tmux's prompt is a line editor
// (prompt.c `prompt_key`).

fn search_input(app: &AppState) -> String {
    match app.mode {
        Mode::CopySearch { ref input, .. } => input.clone(),
        _ => panic!("the search prompt is not open: {:?}", std::mem::discriminant(&app.mode)),
    }
}

fn open_search(app: &mut AppState) {
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(app);
    crate::input::send_text_to_active(app, "/").unwrap();
}

#[test]
fn a_space_is_typed_into_the_search_prompt() {
    let mut app = app_with_pane();
    open_search(&mut app);
    crate::input::send_text_to_active(&mut app, "row").unwrap();
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    crate::input::send_text_to_active(&mut app, "7").unwrap();
    assert_eq!(search_input(&app), "row 7");
    assert_eq!(app.status_message.as_ref().map(|m| m.0.as_str()), Some("(search down) row 7"));
}

#[test]
fn search_prompt_cursor_keys_edit_in_place() {
    let mut app = app_with_pane();
    open_search(&mut app);
    crate::input::send_text_to_active(&mut app, "abc").unwrap();
    crate::input::send_key_to_active(&mut app, "C-a").unwrap();
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    assert_eq!(search_input(&app), "Zabc", "C-a goes to the start");
    crate::input::send_key_to_active(&mut app, "C-e").unwrap();
    crate::input::send_text_to_active(&mut app, "!").unwrap();
    assert_eq!(search_input(&app), "Zabc!", "C-e goes to the end");
    crate::input::send_key_to_active(&mut app, "left").unwrap();
    crate::input::send_key_to_active(&mut app, "left").unwrap();
    crate::input::send_key_to_active(&mut app, "backspace").unwrap();
    assert_eq!(search_input(&app), "Zac!", "Backspace deletes before the cursor");
    crate::input::send_key_to_active(&mut app, "home").unwrap();
    crate::input::send_key_to_active(&mut app, "delete").unwrap();
    assert_eq!(search_input(&app), "ac!", "Delete removes the character under the cursor");
    crate::input::send_key_to_active(&mut app, "right").unwrap();
    crate::input::send_key_to_active(&mut app, "C-k").unwrap();
    assert_eq!(search_input(&app), "a", "C-k cuts to the end");
    assert!(app.status_message.is_some(), "the prompt stays drawn after a cursor key");
}

#[test]
fn search_prompt_recalls_earlier_searches() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 30);
    open_search(&mut app);
    crate::input::send_text_to_active(&mut app, "first").unwrap();
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    crate::input::send_text_to_active(&mut app, "/second").unwrap();
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_key_to_active(&mut app, "up").unwrap();
    assert_eq!(search_input(&app), "second");
    crate::input::send_key_to_active(&mut app, "up").unwrap();
    assert_eq!(search_input(&app), "first");
    crate::input::send_key_to_active(&mut app, "up").unwrap();
    assert_eq!(search_input(&app), "first", "Up stops at the oldest");
    crate::input::send_key_to_active(&mut app, "down").unwrap();
    assert_eq!(search_input(&app), "second");
    crate::input::send_key_to_active(&mut app, "down").unwrap();
    assert_eq!(search_input(&app), "", "Down past the newest gives an empty line");
}

#[test]
fn prompt_edit_word_and_clear() {
    use crate::copy_mode::{prompt_edit, PromptEdit};
    let mut s = "foo bar baz".to_string();
    let mut back = 0;
    assert_eq!(prompt_edit(&mut s, &mut back, &mut None, "", "C-w", || None), PromptEdit::Edited);
    assert_eq!(s, "foo bar ");
    assert_eq!(prompt_edit(&mut s, &mut back, &mut None, "", "C-u", || None), PromptEdit::Edited);
    assert_eq!(s, "");
    assert_eq!(prompt_edit(&mut s, &mut back, &mut None, "", "x", || None), PromptEdit::Other);
}

#[test]
fn a_new_prompt_starts_with_the_cursor_at_the_end() {
    let mut app = app_with_pane();
    open_search(&mut app);
    crate::input::send_text_to_active(&mut app, "abc").unwrap();
    crate::input::send_key_to_active(&mut app, "home").unwrap();
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    crate::input::send_text_to_active(&mut app, "/xy").unwrap();
    assert_eq!(search_input(&app), "xy");
}

// ── 7. the cursor inside a selection ────────────────────────────────────────
//
// Measured on dd695ea with real keystrokes: with a selection the host cursor
// stayed on the cell after the selection (x 93 for a cursor at 92) and did not
// move when `o` put the copy cursor on the other end (copy_cursor_x 88, host
// cursor still 93). tmux shows the copy cursor with the terminal's own cursor
// whether or not a selection is active.

fn render_copy_leaf(sel: ((u16, u16), (u16, u16)), cursor: (u16, u16)) -> (ratatui::layout::Position, ratatui::buffer::Buffer) {
    render_copy_leaf_hl(Some(sel), cursor, Vec::new())
}

fn render_copy_leaf_hl(sel: Option<((u16, u16), (u16, u16))>, cursor: (u16, u16), hl: Vec<[u16; 4]>) -> (ratatui::layout::Position, ratatui::buffer::Buffer) {
    use crate::layout::{CellJson, LayoutJson};
    use ratatui::backend::{Backend, TestBackend};
    use ratatui::layout::Rect;
    use ratatui::style::Color;
    use ratatui::Terminal;
    let (w, h) = (40u16, 10u16);
    let cell = |ch: char| CellJson {
        text: ch.to_string(), fg: String::new(), bg: String::new(),
        bold: false, italic: false, underline: false, inverse: false,
        dim: false, blink: false, hidden: false, strikethrough: false,
    };
    let content: Vec<Vec<CellJson>> = (0..h).map(|_| (0..w).map(|_| cell('x')).collect()).collect();
    let leaf = LayoutJson::Leaf {
        id: 0, rows: h, cols: w, cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true, cursor_shape: 0,
        active: true, copy_mode: true, scroll_offset: 0, view_offset: 0,
        sel_start_row: sel.map(|s| s.0 .0), sel_start_col: sel.map(|s| s.0 .1),
        sel_end_row: sel.map(|s| s.1 .0), sel_end_col: sel.map(|s| s.1 .1),
        sel_mode: sel.map(|_| "char".to_string()),
        copy_cursor_row: Some(cursor.0), copy_cursor_col: Some(cursor.1),
        content, rows_v2: Vec::new(), title: None, copy_hl: hl,
    };
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 0, w, h);
        let active_rect = crate::client::compute_active_rect_json(&leaf, area);
        crate::client::render_layout_json(
            f, &leaf, area, false,
            ratatui::style::Style::default().fg(Color::DarkGray),
            ratatui::style::Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "bg=yellow,fg=black", false, "off", "", 1,
            crate::border_lines::border_chars("single"), None,
            crate::client::WindowContentStyles::default(),
            crate::pane_border::PaneBorderIndicators::Colour,
        );
    }).unwrap();
    let pos = term.backend_mut().get_cursor_position().unwrap();
    (pos, term.backend().buffer().clone())
}

#[test]
fn the_host_cursor_sits_on_the_copy_cursor_inside_a_selection() {
    let sel = ((4, 3), (4, 8));
    let (pos, buf) = render_copy_leaf(sel, (4, 8));
    assert_eq!((pos.x, pos.y), (8, 4), "the cursor is shown at the moving end");
    let cell = &buf[(8u16, 4u16)];
    assert!(!cell.modifier.contains(ratatui::style::Modifier::REVERSED),
        "the endpoint keeps the selection style (4853ddd)");
    assert_eq!(cell.bg, ratatui::style::Color::Yellow);

    // `o` puts the copy cursor on the other end, and the cursor follows.
    let (pos, _) = render_copy_leaf(sel, (4, 3));
    assert_eq!((pos.x, pos.y), (3, 4));
}

// ── 2. v is rectangle-toggle, Space is begin-selection ──────────────────────
//
// Measured on dd695ea with real keystrokes: `v` gave selection_present 1 and
// a second `v` kept it, so `v` began a character selection. tmux binds `v` to
// rectangle-toggle and Space to begin-selection in copy-mode-vi
// (key-bindings.c:705 and :656); measured in tmux 3.4, `v` with no selection
// gives rectangle_toggle 1 and selection_present 0.

#[test]
fn v_toggles_the_rectangle_and_starts_nothing_in_vi() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 40);
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((5, 1));
    crate::input::send_text_to_active(&mut app, "v").unwrap();
    assert!(app.copy_anchor.is_none(), "v selects nothing on its own");
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Rect);
    // Space then starts a selection in the rectangle shape v chose, as
    // window_copy_start_selection leaves rectflag alone.
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    assert!(app.copy_anchor.is_some());
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Rect);
    crate::input::send_text_to_active(&mut app, "v").unwrap();
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Char, "a second v toggles back");
    assert!(app.copy_anchor.is_some(), "and keeps the selection");
}

#[test]
fn space_after_a_line_selection_starts_a_character_one() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 40);
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "V").unwrap();
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Line);
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    assert_eq!(app.copy_selection_mode, crate::types::SelectionMode::Char);
}

#[test]
fn v_keeps_begin_selection_with_emacs_keys() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 40);
    app.mode_keys = "emacs".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "v").unwrap();
    assert!(app.copy_anchor.is_some());
}

#[test]
fn list_keys_shows_v_as_rectangle_toggle_and_escape_as_clear_selection() {
    let vi = crate::help::COPY_MODE_VI_DEFAULTS;
    assert!(vi.contains(&("v", "send-keys -X rectangle-toggle")));
    assert!(vi.contains(&("Space", "send-keys -X begin-selection")));
    assert!(vi.contains(&("Escape", "send-keys -X clear-selection")));
    assert!(vi.contains(&("q", "send-keys -X cancel")));
    assert!(vi.contains(&("r", "send-keys -X refresh-from-pane")));
}

// ── 4b and 5. match and mark highlighting ───────────────────────────────────
//
// Measured on dd695ea with real keystrokes and the client's console
// attributes: after `?beta` every row read `[attr 7x13,16391x1,7x106]`, only
// the cursor cell reversed, and after `X` the marked row read `7x120` like its
// neighbours. tmux paints them with copy-mode-match-style,
// copy-mode-current-match-style and copy-mode-mark-style
// (window_copy_update_style).

fn searched_app(query: &str) -> AppState {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row alpha beta", 0, 60);
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "?").unwrap();
    crate::input::send_text_to_active(&mut app, query).unwrap();
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    app
}

#[test]
fn every_visible_match_is_highlighted_and_the_current_one_differs() {
    let app = searched_app("beta");
    let hl = crate::copy_mode::copy_highlights(&app);
    let (cr, cc) = app.copy_pos.expect("the cursor is on the match");
    let current: Vec<_> = hl.iter().filter(|h| h[3] == crate::copy_mode::HL_CURRENT_MATCH).collect();
    assert_eq!(current.len(), 1, "exactly one current match: {hl:?}");
    assert_eq!((current[0][0], current[0][1]), (cr, cc));
    assert_eq!(current[0][2] - current[0][1], 3, "beta is four cells");
    let others = hl.iter().filter(|h| h[3] == crate::copy_mode::HL_MATCH).count();
    assert!(others >= 20, "the other visible rows are highlighted too, got {others}");
}

#[test]
fn n_moves_the_current_match() {
    let mut app = searched_app("beta");
    let before = app.copy_pos;
    crate::input::send_text_to_active(&mut app, "n").unwrap();
    assert_ne!(app.copy_pos, before);
    let hl = crate::copy_mode::copy_highlights(&app);
    let (cr, cc) = app.copy_pos.unwrap();
    assert!(hl.iter().any(|h| h[3] == crate::copy_mode::HL_CURRENT_MATCH && h[0] == cr && h[1] == cc));
}

#[test]
fn begin_selection_clears_the_match_highlight_like_tmux() {
    // begin-selection is WINDOW_COPY_CMD_CLEAR_ALWAYS in window-copy.c.
    let mut app = searched_app("beta");
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    let hl = crate::copy_mode::copy_highlights(&app);
    assert!(hl.iter().all(|h| h[3] >= crate::copy_mode::HL_MARK_LINE), "{hl:?}");
    // A cursor motion does not clear them in vi (CLEAR_EMACS_ONLY).
    let mut app = searched_app("beta");
    crate::input::send_text_to_active(&mut app, "j").unwrap();
    assert!(!crate::copy_mode::copy_highlights(&app).is_empty());
}

#[test]
fn the_marked_line_is_highlighted_and_follows_scrolling() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 80);
    app.mode_keys = "vi".to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((10, 4));
    crate::input::send_text_to_active(&mut app, "X").unwrap();
    let hl = crate::copy_mode::copy_highlights(&app);
    let cols = super::test_copy_mode_snapshot::pane_of(&app).unwrap().last_cols;
    assert!(hl.contains(&[10, 0, cols - 1, crate::copy_mode::HL_MARK_LINE]), "{hl:?}");
    assert!(hl.contains(&[10, 4, 4, crate::copy_mode::HL_MARK_CELL]));
    crate::copy_mode::scroll_copy_up(&mut app, 3);
    let hl = crate::copy_mode::copy_highlights(&app);
    assert!(hl.contains(&[13, 0, cols - 1, crate::copy_mode::HL_MARK_LINE]), "the mark stays on its line: {hl:?}");
}

#[test]
fn the_client_paints_matches_and_the_mark_in_tmux_default_styles() {
    use ratatui::style::Color;
    let hl = vec![[2, 4, 7, 0], [2, 10, 13, 1], [5, 0, 39, 2], [5, 3, 3, 3]];
    let (_, buf) = render_copy_leaf_hl(None, (2, 10), hl);
    assert_eq!(buf[(5u16, 2u16)].bg, Color::Cyan, "a match is bg=cyan");
    assert_eq!(buf[(5u16, 2u16)].fg, Color::Black);
    assert_eq!(buf[(11u16, 2u16)].bg, Color::Magenta, "the current match is bg=magenta");
    assert_eq!(buf[(0u16, 5u16)].bg, Color::Red, "the marked line is bg=red");
    assert_eq!(buf[(3u16, 5u16)].bg, Color::Black, "the marked cell has its colours swapped");
    assert_eq!(buf[(3u16, 5u16)].fg, Color::Red);
    assert_eq!(buf[(0u16, 6u16)].bg, Color::Reset, "other rows are untouched");
}

#[test]
fn the_selection_is_drawn_over_a_match() {
    use ratatui::style::Color;
    let hl = vec![[2, 4, 7, 0]];
    let (_, buf) = render_copy_leaf_hl(Some(((2, 0), (2, 5))), (2, 5), hl);
    assert_eq!(buf[(4u16, 2u16)].bg, Color::Yellow, "mode-style wins over the match");
    assert_eq!(buf[(6u16, 2u16)].bg, Color::Cyan);
}
