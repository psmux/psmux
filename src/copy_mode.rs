use std::io::{self, Write};

use crate::clipboard::copy_to_system_clipboard;
use crate::types::{AppState, Mode, CopyModeState};
use crate::tree::{active_pane, active_pane_mut};

/// Emit an OSC 52 escape sequence to set the terminal clipboard.
/// This works over SSH because the sequence travels through the SSH pipe
/// to the local terminal emulator (e.g. Windows Terminal, iTerm2).
/// The `writer` should be the client's stdout (not the server's).
pub fn emit_osc52<W: Write>(writer: &mut W, text: &str) {
    let encoded = crate::util::base64_encode(text);
    // OSC 52 ; c ; <base64> ST   (ST = ESC \\ or BEL)
    // Use BEL (\x07) as ST for broadest terminal compatibility.
    let _ = write!(writer, "\x1b]52;c;{}\x07", encoded);
    let _ = writer.flush();
}

pub fn enter_copy_mode(app: &mut AppState) {
    app.mode = Mode::CopyMode;
    // Start at the view currently on screen: with a direct-scrolled pane
    // (scroll-enter-copy-mode off, #193) the parser's scrollback is nonzero
    // while no copy state exists yet, and copy mode must keep that view —
    // an offset of 0 here would render live content but yank against the
    // scrolled rows.  At the live bottom this is the usual 0.  Every later
    // path keeps the two in sync (scroll_copy_up/down, exit_copy_mode).
    app.copy_scroll_offset = {
        let win = &app.windows[app.active_idx];
        active_pane(&win.root, &win.active_path)
            .and_then(|p| p.term.lock().ok().map(|t| t.screen().scrollback()))
            .unwrap_or(0)
    };
    app.copy_selection_mode = crate::types::SelectionMode::Char;
    app.copy_anchor = None;
    // Initialize copy_pos from the terminal cursor so the cursor is
    // visible immediately on entering copy mode (fixes #25).
    app.copy_pos = current_prompt_pos(app);
    app.copy_mouse_down_cell = None;
    app.copy_find_char_pending = None;
    app.copy_text_object_pending = None;
    app.copy_register_pending = false;
    app.copy_register = None;
    app.copy_count = None;
    app.copy_mark = None;
    app.copy_last_jump = None;
    app.copy_refresh_live = false;
    // Mark the active pane as being in copy mode (pane-local state).
    save_copy_state_to_pane(app);
}

/// Exit copy mode: reset all copy state and scroll the active pane back to
/// live output.  Every copy-mode exit path should call this to avoid leaving
/// a pane scrolled while no longer in copy mode (fixes #43).
pub fn exit_copy_mode(app: &mut AppState) {
    app.mode = Mode::Passthrough;
    app.copy_anchor = None;
    app.copy_pos = None;
    app.copy_mouse_down_cell = None;
    app.copy_scroll_offset = 0;
    // Clear the search prompt if it was lingering from CopySearch (#335).
    app.status_message = None;
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        // Clear the pane-local copy state so re-entering this pane won't
        // restore a stale copy mode.
        p.copy_state = None;
        if let Ok(mut parser) = p.term.lock() {
            parser.screen_mut().set_scrollback(0);
        }
    }
}

/// Save the current global copy-mode state into the active pane.
/// Called whenever we are about to switch away from a pane that is in copy mode.
pub fn save_copy_state_to_pane(app: &mut AppState) {
    let (in_search, search_input, search_input_forward) = match &app.mode {
        Mode::CopySearch { input, forward } => (true, input.clone(), *forward),
        _ => (false, String::new(), true),
    };
    let state = CopyModeState {
        anchor: app.copy_anchor,
        anchor_scroll_offset: app.copy_anchor_scroll_offset,
        pos: app.copy_pos,
        scroll_offset: app.copy_scroll_offset,
        selection_mode: app.copy_selection_mode,
        search_query: app.copy_search_query.clone(),
        count: app.copy_count,
        search_matches: app.copy_search_matches.clone(),
        search_idx: app.copy_search_idx,
        search_forward: app.copy_search_forward,
        find_char_pending: app.copy_find_char_pending,
        text_object_pending: app.copy_text_object_pending,
        register_pending: app.copy_register_pending,
        register: app.copy_register,
        mark: app.copy_mark,
        last_jump: app.copy_last_jump,
        in_search,
        search_input,
        search_input_forward,
    };
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        p.copy_state = Some(state);
    }
}

/// Restore copy-mode state from the newly-focused pane into the global
/// AppState fields.  If the pane has no saved copy state, set mode to
/// Passthrough.
pub fn restore_copy_state_from_pane(app: &mut AppState) {
    let win = &app.windows[app.active_idx];
    let state = active_pane(&win.root, &win.active_path)
        .and_then(|p| p.copy_state.clone());
    if let Some(s) = state {
        app.copy_anchor = s.anchor;
        app.copy_anchor_scroll_offset = s.anchor_scroll_offset;
        app.copy_pos = s.pos;
        app.copy_scroll_offset = s.scroll_offset;
        app.copy_selection_mode = s.selection_mode;
        app.copy_search_query = s.search_query;
        app.copy_count = s.count;
        app.copy_search_matches = s.search_matches;
        app.copy_search_idx = s.search_idx;
        app.copy_search_forward = s.search_forward;
        app.copy_find_char_pending = s.find_char_pending;
        app.copy_text_object_pending = s.text_object_pending;
        app.copy_register_pending = s.register_pending;
        app.copy_register = s.register;
        app.copy_mark = s.mark;
        app.copy_last_jump = s.last_jump;
        if s.in_search {
            app.mode = Mode::CopySearch { input: s.search_input, forward: s.search_input_forward };
        } else {
            app.mode = Mode::CopyMode;
        }
    } else {
        // New pane is not in copy mode — switch to passthrough.
        app.mode = Mode::Passthrough;
    }
}

/// Handle a pane or window focus change: save current copy state if in copy
/// mode, then after the switch, restore the new pane's state.
/// Call the `switch_fn` closure between save and restore to perform the
/// actual focus change.
pub fn switch_with_copy_save<F: FnOnce(&mut AppState)>(app: &mut AppState, switch_fn: F) {
    let was_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
    if was_copy {
        save_copy_state_to_pane(app);
    }
    switch_fn(app);
    // After switching, check if the new pane has copy state to restore.
    let win = &app.windows[app.active_idx];
    let new_pane_has_copy = active_pane(&win.root, &win.active_path)
        .map_or(false, |p| p.copy_state.is_some());
    if new_pane_has_copy {
        restore_copy_state_from_pane(app);
    } else if was_copy {
        // We were in copy mode but new pane is not — switch to passthrough.
        app.mode = Mode::Passthrough;
    }
}

/// Id of the pane that is active right now, if there is one.
pub fn active_pane_id(app: &AppState) -> Option<usize> {
    let win = app.windows.get(app.active_idx)?;
    crate::tree::get_active_pane_id(&win.root, &win.active_path)
}

/// Copy mode belongs to the pane it was entered in, exactly as it does in
/// tmux, where a mode is a property of one `window_pane` (`window.c`
/// `window_pane_create` starts every pane with an empty mode stack).  psmux
/// keeps the live copy cursor and selection in `AppState`, so those globals
/// are really "the mode of whichever pane is active", and `Pane::copy_state`
/// is where each pane's own mode is parked while another pane holds focus.
///
/// Focus commands keep the two in step through `switch_with_copy_save`.
/// Creating a pane or a window and killing a pane also change which pane is
/// active, and before #607 they did it without telling the copy layer: the
/// global `Mode::CopyMode` simply stayed put and became the brand new pane's
/// mode, or the dead pane's mode became the survivor's.
///
/// `park_mode_on_active_pane` hands the outgoing pane its own state back;
/// `retarget_mode_to_active_pane` makes the incoming pane's own state the
/// live one.  Call the first before the active pane changes and the second
/// after, passing the pane id captured before the change.
pub fn park_mode_on_active_pane(app: &mut AppState) {
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        save_copy_state_to_pane(app);
    }
}

/// Adopt the newly active pane's own mode.  `prev` is the pane id that was
/// active before the change; when the active pane did not actually move this
/// is a no-op, so a caller can use it unconditionally without clobbering the
/// live copy cursor with the parked (older) copy of it.
pub fn retarget_mode_to_active_pane(app: &mut AppState, prev: Option<usize>) {
    let now = active_pane_id(app);
    if now == prev {
        return;
    }
    let has_copy = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map_or(false, |p| p.copy_state.is_some());
    if has_copy {
        restore_copy_state_from_pane(app);
    } else if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        // The pane that owned copy mode is no longer the active one, and the
        // pane that is has never been in copy mode.  Drop the live copy
        // cursor with the mode so nothing of the old pane's selection is left
        // pointing into the new pane's grid.
        app.mode = Mode::Passthrough;
        app.copy_anchor = None;
        app.copy_pos = None;
        app.copy_scroll_offset = 0;
        app.copy_mouse_down_cell = None;
    }
}

pub fn current_prompt_pos(app: &mut AppState) -> Option<(u16,u16)> {
    let win = &mut app.windows[app.active_idx];
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let parser = p.term.lock().ok()?;
    let (r,c) = parser.screen().cursor_position();
    Some((r,c))
}

pub fn move_copy_cursor(app: &mut AppState, dx: i16, dy: i16) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    // Use tracked copy_pos if available, otherwise fall back to terminal cursor
    let (r, c) = app.copy_pos.unwrap_or_else(|| parser.screen().cursor_position());
    let rows = p.last_rows;
    let cols = p.last_cols;
    let desired_r = r as i16 + dy;
    let nc = (c as i16 + dx).max(0).min(cols as i16 - 1) as u16;
    // If cursor would move above the visible area, scroll up into scrollback
    if desired_r < 0 {
        let scroll_lines = (-desired_r) as usize;
        let current = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(current.saturating_add(scroll_lines));
        app.copy_scroll_offset = parser.screen().scrollback();
        app.copy_pos = Some((0, nc));
    }
    // If cursor would move below the visible area, scroll down (reduce scrollback)
    else if desired_r >= rows as i16 {
        let scroll_lines = (desired_r - rows as i16 + 1) as usize;
        let current = parser.screen().scrollback();
        if current > 0 {
            parser.screen_mut().set_scrollback(current.saturating_sub(scroll_lines));
            app.copy_scroll_offset = parser.screen().scrollback();
            app.copy_pos = Some((rows.saturating_sub(1), nc));
        } else {
            // Already at bottom, clamp
            app.copy_pos = Some((rows.saturating_sub(1), nc));
        }
    } else {
        app.copy_pos = Some((desired_r as u16, nc));
    }
}

/// Helper: read a full row of text from the active pane's screen.
fn read_row_text(app: &mut AppState, row: u16) -> Option<(String, u16)> {
    let win = &mut app.windows[app.active_idx];
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let parser = p.term.lock().ok()?;
    let screen = parser.screen();
    let cols = p.last_cols;
    let mut text = String::with_capacity(cols as usize);
    for c in 0..cols {
        if let Some(cell) = screen.cell(row, c) {
            let t = cell.contents();
            if t.is_empty() { text.push(' '); } else { text.push_str(t); }
        } else {
            text.push(' ');
        }
    }
    Some((text, cols))
}

/// Get the current copy-mode cursor position (from copy_pos or screen cursor).
pub fn get_copy_pos(app: &mut AppState) -> Option<(u16, u16)> {
    if let Some(pos) = app.copy_pos { return Some(pos); }
    current_prompt_pos(app)
}

/// Move cursor to start of line (0 key in vi copy mode).
pub fn move_to_line_start(app: &mut AppState) {
    if let Some((r, _)) = get_copy_pos(app) {
        app.copy_pos = Some((r, 0));
    }
}

/// Move cursor to end of line ($ key in vi copy mode).
pub fn move_to_line_end(app: &mut AppState) {
    if let Some((r, _)) = get_copy_pos(app) {
        let win = &app.windows[app.active_idx];
        if let Some(p) = active_pane(&win.root, &win.active_path) {
            let cols = p.last_cols;
            app.copy_pos = Some((r, cols.saturating_sub(1)));
        }
    }
}

/// Move cursor to first non-blank character (^ key in vi copy mode).
pub fn move_to_first_nonblank(app: &mut AppState) {
    if let Some((r, _)) = get_copy_pos(app) {
        if let Some((text, _)) = read_row_text(app, r) {
            let col = text.find(|c: char| !c.is_whitespace()).unwrap_or(0) as u16;
            app.copy_pos = Some((r, col));
        }
    }
}

/// Classify a character for word boundary detection.
/// Returns: 0 = whitespace, 1 = word char (alnum/_), 2 = punctuation/other
#[inline]
fn char_class(ch: char, seps: &str) -> u8 {
    if ch.is_whitespace() { 0 }
    else if seps.contains(ch) { 2 }
    else if ch.is_alphanumeric() || ch == '_' { 1 }
    else { 2 }
}

/// Move cursor to start of next word (w key in vi copy mode).
pub fn move_word_forward(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let seps = app.word_separators.clone();
    let (text, cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = c as usize;
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);

    // Phase 1: skip current word class
    if col < bytes.len() {
        let cls = char_class(bytes[col], &seps);
        while col < bytes.len() && char_class(bytes[col], &seps) == cls { col += 1; }
    }
    // Phase 2: skip whitespace
    while col < bytes.len() && bytes[col].is_whitespace() { col += 1; }

    if col < cols as usize {
        app.copy_pos = Some((r, col as u16));
    } else {
        // Wrap to next line
        let nr = (r + 1).min(rows.saturating_sub(1));
        if nr != r {
            if let Some((next_text, _)) = read_row_text(app, nr) {
                let next_bytes: Vec<char> = next_text.chars().collect();
                let mut nc = 0usize;
                while nc < next_bytes.len() && next_bytes[nc].is_whitespace() { nc += 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else {
                app.copy_pos = Some((nr, 0));
            }
        }
    }
}

/// Move cursor to start of previous word (b key in vi copy mode).
pub fn move_word_backward(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let seps = app.word_separators.clone();
    let (text, _) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = c as usize;

    if col == 0 {
        // Wrap to previous line
        if r > 0 {
            let nr = r - 1;
            if let Some((prev_text, prev_cols)) = read_row_text(app, nr) {
                let prev_bytes: Vec<char> = prev_text.chars().collect();
                let mut nc = (prev_cols as usize).min(prev_bytes.len()).saturating_sub(1);
                while nc > 0 && prev_bytes[nc].is_whitespace() { nc -= 1; }
                let cls = char_class(prev_bytes[nc], &seps);
                while nc > 0 && char_class(prev_bytes[nc - 1], &seps) == cls { nc -= 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else {
                app.copy_pos = Some((r - 1, 0));
            }
        }
        return;
    }

    // Phase 1: move left past whitespace
    while col > 0 && bytes[col - 1].is_whitespace() { col -= 1; }
    // Phase 2: move left past current word class
    if col > 0 {
        let cls = char_class(bytes[col - 1], &seps);
        while col > 0 && char_class(bytes[col - 1], &seps) == cls { col -= 1; }
    }
    app.copy_pos = Some((r, col as u16));
}

/// Move cursor to end of current word (e key in vi copy mode).
pub fn move_word_end(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let seps = app.word_separators.clone();
    let (text, cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = (c as usize) + 1; // start one past current position
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);

    // Skip whitespace
    while col < bytes.len() && bytes[col].is_whitespace() { col += 1; }
    // Find end of word class
    if col < bytes.len() {
        let cls = char_class(bytes[col], &seps);
        while col + 1 < bytes.len() && char_class(bytes[col + 1], &seps) == cls { col += 1; }
    }

    if col < cols as usize {
        app.copy_pos = Some((r, col as u16));
    } else {
        let nr = (r + 1).min(rows.saturating_sub(1));
        if nr != r {
            if let Some((next_text, _)) = read_row_text(app, nr) {
                let next_bytes: Vec<char> = next_text.chars().collect();
                let mut nc = 0usize;
                while nc < next_bytes.len() && next_bytes[nc].is_whitespace() { nc += 1; }
                let cls = if nc < next_bytes.len() { char_class(next_bytes[nc], &seps) } else { 0 };
                while nc + 1 < next_bytes.len() && char_class(next_bytes[nc + 1], &seps) == cls { nc += 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else {
                app.copy_pos = Some((nr, 0));
            }
        }
    }
}

/// Scroll the active pane's scrollback buffer without entering copy mode.
/// Used when scroll-enter-copy-mode is off (#193, credit: @jun2077681).
pub fn scroll_pane_scrollback(app: &mut AppState, lines: usize, up: bool) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    let current = parser.screen().scrollback();
    let new_offset = if up { current.saturating_add(lines) } else { current.saturating_sub(lines) };
    parser.screen_mut().set_scrollback(new_offset);
}

pub fn scroll_copy_up(app: &mut AppState, lines: usize) {
    scroll_pane_scrollback(app, lines, true);
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    app.copy_scroll_offset = parser.screen().scrollback();
}

pub fn scroll_copy_down(app: &mut AppState, lines: usize) {
    scroll_pane_scrollback(app, lines, false);
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    app.copy_scroll_offset = parser.screen().scrollback();
}

pub fn scroll_to_top(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(usize::MAX);
    app.copy_scroll_offset = parser.screen().scrollback();
}

pub fn scroll_to_bottom(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(0);
    app.copy_scroll_offset = 0;
}

pub fn yank_selection(app: &mut AppState) -> io::Result<()> {
    let (anchor, pos) = match (app.copy_anchor, app.copy_pos) { (Some(a), Some(p)) => (a,p), _ => return Ok(()) };
    let sel_mode = app.copy_selection_mode;
    let anchor_scroll = app.copy_anchor_scroll_offset;
    let current_scroll = app.copy_scroll_offset;
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let rows = p.last_rows;
    let cols = p.last_cols;

    // Compute absolute line positions (relative to an arbitrary reference).
    // abs = screen_row - scrollback_at_that_time
    // Higher abs = further down in the terminal buffer (more recent).
    let anchor_abs = anchor.0 as i64 - anchor_scroll as i64;
    let cursor_abs = pos.0 as i64 - current_scroll as i64;
    let sel_top_abs = anchor_abs.min(cursor_abs);
    let sel_bot_abs = anchor_abs.max(cursor_abs);
    let total_lines = (sel_bot_abs - sel_top_abs + 1) as usize;

    // For character mode: determine which endpoint is the "top" (first) line
    let (top_col, bot_col) = if anchor_abs <= cursor_abs {
        (anchor.1, pos.1)
    } else {
        (pos.1, anchor.1)
    };

    // Read all selected rows by adjusting scrollback as needed.
    // At scrollback S, row R shows absolute line (R - S).
    // To read absolute line L: row = L + S, needs 0 <= L + S < rows.
    let mut text = String::new();
    let mut abs_idx: usize = 0; // running index within selection
    let mut next_abs = sel_top_abs;
    while next_abs <= sel_bot_abs {
        // Set scrollback so next_abs maps to row 0 (or as close as possible)
        let target_sb = (-next_abs).max(0) as usize;
        parser.screen_mut().set_scrollback(target_sb);
        let actual_sb = parser.screen().scrollback() as i64;
        let vis_start_abs = -actual_sb;
        let vis_end_abs   = -actual_sb + rows as i64 - 1;
        let read_start = next_abs.max(vis_start_abs);
        let read_end   = sel_bot_abs.min(vis_end_abs);
        if read_start > read_end { break; }

        for aline in read_start..=read_end {
            let r = (aline + actual_sb) as u16;
            let is_first = abs_idx == 0;
            let is_last  = abs_idx + 1 == total_lines;
            match sel_mode {
                crate::types::SelectionMode::Rect => {
                    let c0 = anchor.1.min(pos.1); let c1 = anchor.1.max(pos.1);
                    let line = capture_row_text(parser.screen(), r, c0..c1.saturating_add(1));
                    text.push_str(line.trim_end());
                    if !is_last { text.push('\n'); }
                }
                crate::types::SelectionMode::Line => {
                    let line = capture_row_text(parser.screen(), r, 0..cols);
                    text.push_str(line.trim_end());
                    text.push('\n');
                }
                crate::types::SelectionMode::Char => {
                    if total_lines == 1 {
                        let c0 = anchor.1.min(pos.1); let c1 = anchor.1.max(pos.1);
                        text.push_str(&capture_row_text(parser.screen(), r, c0..c1.saturating_add(1)));
                    } else {
                        let line_start = if is_first { top_col } else { 0 };
                        let line_end   = if is_last  { bot_col } else { cols.saturating_sub(1) };
                        let line = capture_row_text(parser.screen(), r, line_start..line_end.saturating_add(1));
                        text.push_str(line.trim_end());
                        if !is_last { text.push('\n'); }
                    }
                }
            }
            abs_idx += 1;
        }
        next_abs = read_end + 1;
    }
    // Restore original scrollback
    parser.screen_mut().set_scrollback(current_scroll);
    // Store in named register if one was selected
    if let Some(reg) = app.copy_register.take() {
        app.named_registers.insert(reg, text.clone());
    }
    app.paste_buffers.insert(0, text.clone());
    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
    copy_to_system_clipboard(&text);
    // Stage text for OSC 52 delivery to the client (works over SSH)
    if app.set_clipboard != "off" {
        app.clipboard_osc52 = Some(text.clone());
    }
    // Pipe to copy-command if configured
    if !app.copy_command.is_empty() {
        let cmd = app.copy_command.clone();
        pipe_text_to_command(&text, &cmd);
    }
    Ok(())
}

/// Pipe text to a shell command's stdin.
fn pipe_text_to_command(text: &str, cmd: &str) {
    let shell = if cfg!(windows) { "pwsh" } else { "sh" };
    let args: Vec<&str> = if cfg!(windows) {
        vec!["-NoProfile", "-Command", cmd]
    } else {
        vec!["-c", cmd]
    };
    if let Ok(mut child) = {
        let mut cmd = std::process::Command::new(shell);
        cmd.args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        { use crate::platform::HideWindowCommandExt; cmd.hide_window(); }
        cmd.spawn()
    }
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }
}

pub fn paste_latest(app: &mut AppState) -> io::Result<()> {
    // If a named register was selected, paste from it
    if let Some(reg) = app.copy_register.take() {
        if let Some(text) = app.named_registers.get(&reg).cloned() {
            let win = &mut app.windows[app.active_idx];
            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.writer, "{}", text); }
        }
        return Ok(());
    }
    // Issue #428: fall back to the OS clipboard when the internal paste-buffer
    // stack is empty, so prefix+] pastes externally-copied text.
    let internal = app.paste_buffers.first().cloned().unwrap_or_default();
    let text = if internal.is_empty() {
        crate::clipboard::read_from_system_clipboard().unwrap_or_default()
    } else {
        internal
    };
    if !text.is_empty() {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) { let _ = write!(p.writer, "{}", text); }
    }
    Ok(())
}

pub fn capture_active_pane(app: &mut AppState) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows {
        text.push_str(capture_row_text(screen, r, 0..p.last_cols).trim_end());
        text.push('\n');
    }
    app.paste_buffers.insert(0, text);
    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
    Ok(())
}

/// Append one grid cell's text to a capture row.
///
/// A never-written cell (skipped over by a cursor advance such as CUF `ESC[nC`
/// or CHA `ESC[nG`) reports empty `contents()`, so pushing that string verbatim
/// contributes nothing and collapses the on-screen gap between words (issue
/// #443). Emit a single space for every in-bounds blank cell instead, matching
/// how the cell renders on screen. The second half of a wide glyph
/// (`is_wide_continuation`) is skipped entirely: its leading half already
/// carried the full multi-column character, so emitting a space here would add
/// a phantom column after every wide/CJK glyph.
fn push_capture_cell(row: &mut String, cell: Option<&vt100::Cell>) {
    match cell {
        Some(c) if c.is_wide_continuation() => {}
        Some(c) if c.has_contents() => row.push_str(c.contents()),
        _ => row.push(' '),
    }
}

/// Serialize grid columns `cols` of `row` with `push_capture_cell` semantics.
///
/// Every path that turns grid cells back into user-visible text routes through
/// here so the blank-cell backfill and wide-glyph handling of issue #443 stay
/// consistent across `capture-pane` and the copy-mode yanks. Callers that only
/// need a trimmed line should `trim_end()` the result themselves, since the
/// range variants of `capture-pane` deliberately keep their padding.
fn capture_row_text(screen: &vt100::Screen, row: u16, cols: std::ops::Range<u16>) -> String {
    let mut out = String::with_capacity(cols.len());
    for c in cols {
        push_capture_cell(&mut out, screen.cell(row, c));
    }
    out
}

/// Resolve the (window index, tree path) a capture should read.
///
/// An explicit `-t %N` pane id wins and is searched across every window
/// (same pane-by-id lookup as kill-pane); a missing or unresolvable id
/// falls back to the active pane of the active window, keeping the
/// pre-targeting behavior and error shape.
fn capture_target(app: &AppState, pane_id: Option<usize>) -> (usize, Vec<usize>) {
    if let Some(pid) = pane_id {
        if let Some((wi, path)) = app.windows.iter().enumerate().find_map(|(wi, win)| {
            crate::tree::find_path_by_id(&win.root, pid).map(|path| (wi, path))
        }) {
            return (wi, path);
        }
    }
    (app.active_idx, app.windows[app.active_idx].active_path.clone())
}

pub fn capture_active_pane_text(app: &mut AppState, pane_id: Option<usize>, preserve_trailing: bool) -> io::Result<Option<String>> {
    let (win_idx, path) = capture_target(app, pane_id);
    let win = &mut app.windows[win_idx];
    let p = match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows {
        let row = capture_row_text(screen, r, 0..p.last_cols);
        // -N (preserve_trailing) keeps the full row width, trailing spaces
        // included; without it, trim like tmux does by default.
        if preserve_trailing {
            text.push_str(&row);
        } else {
            text.push_str(row.trim_end());
        }
        text.push('\n');
    }
    // Trim trailing all-empty lines so iTerm2 doesn't advance its cursor
    // past the actual content on initial attach.
    while text.ends_with("\n\n") { text.pop(); }
    if text == "\n" { text.clear(); }
    Ok(Some(text))
}

pub fn save_latest_buffer(app: &mut AppState, file: &str) -> io::Result<()> {
    if let Some(buf) = app.paste_buffers.first() { std::fs::write(file, buf)?; }
    Ok(())
}

/// Geometry of the active pane's whole grid, as copy mode search needs it.
/// `history` is the number of lines currently held in the scrollback buffer,
/// `rows`/`cols` the size of the visible screen and `scrollback` the current
/// scroll offset. An absolute line index `abs` runs 0..history+rows and maps
/// to the visible row `abs - (history - scrollback)` whenever that is on
/// screen. This mirrors tmux's `gd->hsize` / `gd->sy` / `data->oy` triple.
struct GridGeometry {
    history: usize,
    rows: u16,
    cols: u16,
    scrollback: usize,
}

fn grid_geometry(app: &mut AppState) -> Option<GridGeometry> {
    let win = app.windows.get_mut(app.active_idx)?;
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let parser = p.term.lock().ok()?;
    let screen = parser.screen();
    Some(GridGeometry {
        history: screen.scrollback_filled(),
        rows: p.last_rows,
        cols: p.last_cols,
        scrollback: screen.scrollback(),
    })
}

/// Read every line of the pane, scrollback first and then the visible screen,
/// as one vector indexed by absolute line number.
///
/// The vt100 screen only ever exposes the rows currently framed by the scroll
/// offset, so the whole buffer is walked one screenful at a time by moving the
/// offset and restoring it afterwards. `Screen::rows` walks its iterator once
/// per screenful, which keeps this linear in the size of the buffer.
fn read_all_lines(app: &mut AppState) -> Option<(Vec<String>, GridGeometry)> {
    let geom = grid_geometry(app)?;
    let total = geom.history + geom.rows as usize;
    let win = app.windows.get_mut(app.active_idx)?;
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let mut parser = p.term.lock().ok()?;
    let mut lines: Vec<String> = vec![String::new(); total];
    let step = geom.rows.max(1) as usize;
    let mut sb = geom.history;
    loop {
        parser.screen_mut().set_scrollback(sb);
        let actual = parser.screen().scrollback();
        // top absolute line currently framed by the viewport
        let top = geom.history - actual;
        for (r, text) in parser.screen().rows(0, geom.cols).enumerate() {
            let abs = top + r;
            if abs < total { lines[abs] = text; }
        }
        if actual == 0 { break; }
        sb = actual.saturating_sub(step);
    }
    parser.screen_mut().set_scrollback(geom.scrollback);
    Some((lines, geom))
}

/// Bring absolute line `abs` into view and park the copy cursor on it.
///
/// Ported from tmux `window_copy_scroll_to` (window-copy.c): a line already on
/// screen never moves the viewport, otherwise the match is placed a quarter of
/// a screen up from the bottom.
pub fn scroll_to_abs_line(app: &mut AppState, abs: usize, col: u16) {
    let geom = match grid_geometry(app) { Some(g) => g, None => return };
    let rows = geom.rows.max(1) as usize;
    let hist = geom.history;
    let top = hist - geom.scrollback.min(hist);

    let (new_scrollback, cy) = if abs >= top && abs < top + rows {
        // Already visible: leave the viewport alone.
        (geom.scrollback, abs - top)
    } else {
        let gap = rows / 4;
        let offset = if abs < rows {
            0
        } else if abs + gap > hist + rows {
            hist
        } else {
            (abs + gap).saturating_sub(rows)
        };
        (hist - offset.min(hist), abs.saturating_sub(offset))
    };

    let win = match app.windows.get_mut(app.active_idx) { Some(w) => w, None => return };
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    if let Ok(mut parser) = p.term.lock() {
        parser.screen_mut().set_scrollback(new_scrollback);
        app.copy_scroll_offset = parser.screen().scrollback();
    }
    app.copy_pos = Some((cy.min(rows - 1) as u16, col));
}

/// The copy cursor as an absolute line index plus a column.
fn copy_cursor_abs(app: &mut AppState) -> (usize, u16) {
    let geom = match grid_geometry(app) { Some(g) => g, None => return (0, 0) };
    let (r, c) = get_copy_pos(app).unwrap_or((0, 0));
    let top = geom.history - geom.scrollback.min(geom.history);
    (top + r as usize, c)
}

/// Search the active pane for a query string across the WHOLE buffer, the
/// scrollback history included, exactly as tmux `window_copy_search_jump`
/// walks lines 0..gd->hsize + gd->sy - 1 (window-copy.c).
///
/// Populates `app.copy_search_matches` with (absolute_line, col_start,
/// col_end) tuples ordered along the search direction, starting from the
/// match nearest to the cursor, so `copy_search_matches[0]` is the hit a user
/// pressing Enter expects and `n` walks on from there.
pub fn search_copy_mode(app: &mut AppState, query: &str, forward: bool) {
    app.copy_search_matches.clear();
    app.copy_search_idx = 0;
    if query.is_empty() { return; }

    let (cur_abs, cur_col) = copy_cursor_abs(app);
    let (lines, _geom) = match read_all_lines(app) { Some(v) => v, None => return };

    let query_lower = query.to_lowercase();
    let qlen = query_lower.chars().count() as u16;

    // Ascending absolute order first; the direction ordering is applied below.
    let mut ascending: Vec<(usize, u16, u16)> = Vec::new();
    for (abs, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(&query_lower) {
            let byte_at = start + pos;
            let col_start = lower[..byte_at].chars().count() as u16;
            ascending.push((abs, col_start, col_start + qlen));
            start = byte_at + 1;
            if start >= lower.len() { break; }
        }
    }
    if ascending.is_empty() { return; }

    // tmux starts the scan from the cursor and only then wraps, so order the
    // list the same way: nearest hit in the search direction first.
    if forward {
        let after = |m: &(usize, u16, u16)| m.0 > cur_abs || (m.0 == cur_abs && m.1 > cur_col);
        let split = ascending.iter().position(after).unwrap_or(ascending.len());
        app.copy_search_matches.extend_from_slice(&ascending[split..]);
        app.copy_search_matches.extend_from_slice(&ascending[..split]);
    } else {
        let before = |m: &(usize, u16, u16)| m.0 < cur_abs || (m.0 == cur_abs && m.1 < cur_col);
        let count = ascending.iter().filter(|m| before(m)).count();
        let mut descending = ascending;
        descending.reverse();
        // `descending` is bottom-to-top; the first `len - count` entries sit at
        // or after the cursor, so rotate them to the back.
        let split = descending.len() - count;
        app.copy_search_matches.extend_from_slice(&descending[split..]);
        app.copy_search_matches.extend_from_slice(&descending[..split]);
    }

    if let Some(&(abs, col, _)) = app.copy_search_matches.first() {
        scroll_to_abs_line(app, abs, col);
    }
}

/// Jump to the next search match in copy mode.
pub fn search_next(app: &mut AppState) {
    if app.copy_search_matches.is_empty() { return; }
    let wrap = app.user_options.get("wrap-search").map(|v| v.as_str()) != Some("off");
    let next = app.copy_search_idx + 1;
    if next >= app.copy_search_matches.len() {
        if !wrap { return; }
        app.copy_search_idx = 0;
    } else {
        app.copy_search_idx = next;
    }
    let (abs, c, _) = app.copy_search_matches[app.copy_search_idx];
    scroll_to_abs_line(app, abs, c);
}

/// Move by WORD (whitespace-delimited) forward — W key
pub fn move_word_forward_big(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let (text, cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = c as usize;
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);
    // Skip non-whitespace
    while col < bytes.len() && !bytes[col].is_whitespace() { col += 1; }
    // Skip whitespace
    while col < bytes.len() && bytes[col].is_whitespace() { col += 1; }
    if col < cols as usize {
        app.copy_pos = Some((r, col as u16));
    } else {
        let nr = (r + 1).min(rows.saturating_sub(1));
        if nr != r {
            if let Some((next_text, _)) = read_row_text(app, nr) {
                let next_bytes: Vec<char> = next_text.chars().collect();
                let mut nc = 0usize;
                while nc < next_bytes.len() && next_bytes[nc].is_whitespace() { nc += 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else { app.copy_pos = Some((nr, 0)); }
        }
    }
}

/// Move by WORD backward — B key
pub fn move_word_backward_big(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let (text, _prev_cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = c as usize;
    if col == 0 {
        if r > 0 {
            let nr = r - 1;
            if let Some((prev_text, prev_cols)) = read_row_text(app, nr) {
                let prev_bytes: Vec<char> = prev_text.chars().collect();
                let mut nc = (prev_cols as usize).min(prev_bytes.len()).saturating_sub(1);
                while nc > 0 && prev_bytes[nc].is_whitespace() { nc -= 1; }
                while nc > 0 && !prev_bytes[nc - 1].is_whitespace() { nc -= 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else { app.copy_pos = Some((r - 1, 0)); }
        }
        return;
    }
    while col > 0 && bytes[col - 1].is_whitespace() { col -= 1; }
    while col > 0 && !bytes[col - 1].is_whitespace() { col -= 1; }
    app.copy_pos = Some((r, col as u16));
}

/// Move to WORD end — E key
pub fn move_word_end_big(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let (text, cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let mut col = (c as usize) + 1;
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);
    while col < bytes.len() && bytes[col].is_whitespace() { col += 1; }
    while col + 1 < bytes.len() && !bytes[col + 1].is_whitespace() { col += 1; }
    if col < cols as usize {
        app.copy_pos = Some((r, col as u16));
    } else {
        let nr = (r + 1).min(rows.saturating_sub(1));
        if nr != r {
            if let Some((next_text, _)) = read_row_text(app, nr) {
                let next_bytes: Vec<char> = next_text.chars().collect();
                let mut nc = 0usize;
                while nc < next_bytes.len() && next_bytes[nc].is_whitespace() { nc += 1; }
                while nc + 1 < next_bytes.len() && !next_bytes[nc + 1].is_whitespace() { nc += 1; }
                app.copy_pos = Some((nr, nc as u16));
            } else { app.copy_pos = Some((nr, 0)); }
        }
    }
}

/// Move to top of visible screen — H key
pub fn move_to_screen_top(app: &mut AppState) {
    app.copy_pos = Some((0, 0));
}

/// Move to middle of visible screen — M key
pub fn move_to_screen_middle(app: &mut AppState) {
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);
    app.copy_pos = Some((rows / 2, 0));
}

/// Move to bottom of visible screen — L key
pub fn move_to_screen_bottom(app: &mut AppState) {
    let rows = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows).unwrap_or(24);
    app.copy_pos = Some((rows.saturating_sub(1), 0));
}

/// Jump kinds shared by f/F/t/T and their `;` / `,` repeats.
pub const JUMP_FORWARD: u8 = 0;
pub const JUMP_BACKWARD: u8 = 1;
pub const JUMP_TO_FORWARD: u8 = 2;
pub const JUMP_TO_BACKWARD: u8 = 3;

/// Perform one f/F/t/T search WITHOUT recording it as the last jump.
/// `jump_again` and `jump_reverse` go through here so repeating a jump never
/// rewrites the stored direction (tmux keeps `jumptype` fixed across `,`).
fn apply_jump(app: &mut AppState, kind: u8, ch: char) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let text = match read_row_text(app, r) { Some((t, _)) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    match kind {
        JUMP_FORWARD => {
            for i in (c as usize + 1)..bytes.len() {
                if bytes[i] == ch { app.copy_pos = Some((r, i as u16)); return; }
            }
        }
        JUMP_BACKWARD => {
            for i in (0..(c as usize)).rev() {
                if bytes[i] == ch { app.copy_pos = Some((r, i as u16)); return; }
            }
        }
        // tmux starts the `t` search two cells along (window_copy_cursor_jump_to
        // uses cx + 2) so that repeating it with `;` steps past the character
        // the cursor is already parked in front of instead of stalling.
        JUMP_TO_FORWARD => {
            for i in (c as usize + 2)..bytes.len() {
                if bytes[i] == ch { app.copy_pos = Some((r, (i as u16).saturating_sub(1))); return; }
            }
        }
        JUMP_TO_BACKWARD => {
            for i in (0..(c as usize).saturating_sub(1)).rev() {
                if bytes[i] == ch { app.copy_pos = Some((r, (i as u16) + 1)); return; }
            }
        }
        _ => {}
    }
}

/// Find character forward on current line — f key
pub fn find_char_forward(app: &mut AppState, ch: char) {
    app.copy_last_jump = Some((JUMP_FORWARD, ch));
    apply_jump(app, JUMP_FORWARD, ch);
}

/// Find character backward on current line — F key
pub fn find_char_backward(app: &mut AppState, ch: char) {
    app.copy_last_jump = Some((JUMP_BACKWARD, ch));
    apply_jump(app, JUMP_BACKWARD, ch);
}

/// Find char up to (not including) forward — t key
pub fn find_char_to_forward(app: &mut AppState, ch: char) {
    app.copy_last_jump = Some((JUMP_TO_FORWARD, ch));
    apply_jump(app, JUMP_TO_FORWARD, ch);
}

/// Find char up to (not including) backward — T key
pub fn find_char_to_backward(app: &mut AppState, ch: char) {
    app.copy_last_jump = Some((JUMP_TO_BACKWARD, ch));
    apply_jump(app, JUMP_TO_BACKWARD, ch);
}

/// Repeat the last f/F/t/T in the same direction — `;` key.
pub fn jump_again(app: &mut AppState) {
    if let Some((kind, ch)) = app.copy_last_jump { apply_jump(app, kind, ch); }
}

/// Repeat the last f/F/t/T in the opposite direction — `,` key.
pub fn jump_reverse(app: &mut AppState) {
    if let Some((kind, ch)) = app.copy_last_jump {
        let reversed = match kind {
            JUMP_FORWARD => JUMP_BACKWARD,
            JUMP_BACKWARD => JUMP_FORWARD,
            JUMP_TO_FORWARD => JUMP_TO_BACKWARD,
            JUMP_TO_BACKWARD => JUMP_TO_FORWARD,
            _ => return,
        };
        apply_jump(app, reversed, ch);
    }
}

/// Move the view to an absolute scrollback offset, keeping copy_scroll_offset
/// in step with whatever the parser actually clamped to.
fn set_scroll_offset(app: &mut AppState, offset: usize) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(offset);
    app.copy_scroll_offset = parser.screen().scrollback();
}

/// Record the cursor position as the mark — X key (set-mark).
pub fn set_mark(app: &mut AppState) {
    if let Some((r, c)) = get_copy_pos(app) {
        app.copy_mark = Some((app.copy_scroll_offset, r, c));
    }
}

/// Swap the cursor with the mark — M-x (jump-to-mark).
///
/// tmux's window_copy_jump_to_mark() exchanges the two positions rather than
/// just moving to the mark, so pressing it twice brings you back to where you
/// jumped from.
pub fn jump_to_mark(app: &mut AppState) {
    let (mark_scroll, mark_row, mark_col) = match app.copy_mark { Some(m) => m, None => return };
    let here = match get_copy_pos(app) {
        Some((r, c)) => (app.copy_scroll_offset, r, c),
        None => return,
    };
    set_scroll_offset(app, mark_scroll);
    app.copy_pos = Some((mark_row, mark_col));
    app.copy_mark = Some(here);
}

/// Toggle whether the pane keeps following live output while in copy mode —
/// r key (refresh-from-pane / refresh-toggle).
///
/// psmux anchors the active pane while in copy mode (#494) so the view does
/// not shift under the cursor. This releases that anchor so the pane tracks
/// new output, and re-applies it on the next press.
pub fn toggle_refresh(app: &mut AppState) {
    app.copy_refresh_live = !app.copy_refresh_live;
    if app.copy_refresh_live {
        // Following live output means sitting at the bottom of the history,
        // which is where that output lands.
        scroll_to_bottom(app);
    }
}

/// Yank from cursor to end of line — D key
pub fn copy_end_of_line(app: &mut AppState) -> io::Result<()> {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return Ok(()) };
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let cols = p.last_cols;
    let text = capture_row_text(screen, r, c..cols).trim_end().to_string();
    app.paste_buffers.insert(0, text.clone());
    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
    copy_to_system_clipboard(&text);
    Ok(())
}

/// Jump to the previous search match in copy mode.
pub fn search_prev(app: &mut AppState) {
    if app.copy_search_matches.is_empty() { return; }
    let wrap = app.user_options.get("wrap-search").map(|v| v.as_str()) != Some("off");
    if app.copy_search_idx == 0 {
        if !wrap { return; }
        app.copy_search_idx = app.copy_search_matches.len() - 1;
    } else {
        app.copy_search_idx -= 1;
    }
    let (abs, c, _) = app.copy_search_matches[app.copy_search_idx];
    scroll_to_abs_line(app, abs, c);
}

/// Compute the (start, end) row range for capture-pane given optional -S/-E
/// values and the last visible row index.
///
/// Tmux semantics (from cmd-capture-pane.c):
///   Negative -S means "N scrollback lines above visible". Since psmux only
///   exposes visible rows here, any negative start clamps to 0 (top of visible),
///   matching tmux behavior when no scrollback history is available.
///   Negative -E likewise clamps to 0.
pub fn compute_capture_range(s: Option<i32>, e: Option<i32>, last_row: u16) -> (u16, u16) {
    let start = match s {
        Some(v) if v < 0 => 0u16,
        Some(v) => (v as u16).min(last_row),
        None => 0,
    };
    let end = match e {
        Some(v) if v < 0 => 0u16,
        Some(v) => (v as u16).min(last_row),
        None => last_row,
    };
    (start, end)
}

pub fn capture_active_pane_range(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, preserve_trailing: bool) -> io::Result<Option<String>> {
    let (win_idx, path) = capture_target(app, pane_id);
    let win = &mut app.windows[win_idx];
    let p = match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let rows = p.last_rows;
    let cols = p.last_cols;
    let last_row = rows.saturating_sub(1) as i32;

    // If all args are non-negative (or None), use the fast visible-only path
    let needs_scrollback = matches!(s, Some(v) if v < 0);
    if !needs_scrollback {
        let (start, end) = compute_capture_range(s, e, last_row as u16);
        let screen = parser.screen();
        let mut text = String::new();
        for r in start..=end {
            let row = capture_row_text(screen, r, 0..cols);
            // -N keeps trailing spaces per row; default trims them.
            if preserve_trailing {
                text.push_str(&row);
            } else {
                text.push_str(row.trim_end());
            }
            text.push('\n');
        }
        // An explicit range (-S/-E) is honored line for line, matching tmux:
        // every requested row is emitted, including trailing blank rows inside
        // the range. Unlike the no-range attach capture (capture_active_pane_text),
        // we do NOT collapse trailing empty lines here, so `capture-pane -p -S 0
        // -E 5` returns the full 6 requested lines instead of only the non-blank
        // prefix. The iTerm2 attach path never reaches this function (it uses the
        // no-range CtrlReq::CapturePane path).
        if text == "\n" { text.clear(); }
        return Ok(Some(text));
    }

    // Scrollback-aware capture path.
    // Absolute line numbering: 0 = top of visible (at scrollback 0),
    // negative = lines above visible top (into scrollback history).
    // Determine actual retained scrollback depth.
    let saved_sb = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(usize::MAX);
    let max_sb = parser.screen().scrollback() as i64;
    parser.screen_mut().set_scrollback(saved_sb);

    // Resolve start: i32::MIN means "all history", other negatives are offsets
    let start_abs: i64 = match s {
        Some(v) if v == i32::MIN => -max_sb,
        Some(v) => (v as i64).max(-max_sb),
        None => 0,
    };
    // Resolve end: negative means N lines above visible top, None means last visible row
    let end_abs: i64 = match e {
        Some(v) if v < 0 => (v as i64).max(-max_sb),
        Some(v) => (v as i64).min(last_row as i64),
        None => last_row as i64,
    };

    if start_abs > end_abs { parser.screen_mut().set_scrollback(saved_sb); return Ok(Some(String::new())); }

    // Walk scrollback in batches (same pattern as yank_selection).
    // At scrollback offset S, screen row R shows absolute line (R - S).
    // To read absolute line L, set scrollback so L maps to a visible row.
    let mut text = String::new();
    let mut next_abs = start_abs;
    while next_abs <= end_abs {
        let target_sb = (-next_abs).max(0) as usize;
        parser.screen_mut().set_scrollback(target_sb);
        let actual_sb = parser.screen().scrollback() as i64;
        let vis_start_abs = -actual_sb;
        let vis_end_abs = -actual_sb + rows as i64 - 1;
        let read_start = next_abs.max(vis_start_abs);
        let read_end = end_abs.min(vis_end_abs);
        if read_start > read_end { break; }

        for aline in read_start..=read_end {
            let r = (aline + actual_sb) as u16;
            let row = capture_row_text(parser.screen(), r, 0..cols);
            // -N keeps trailing spaces per row; default trims them.
            if preserve_trailing {
                text.push_str(&row);
            } else {
                text.push_str(row.trim_end());
            }
            text.push('\n');
        }
        next_abs = read_end + 1;
    }

    // Restore original scrollback offset (no side effects on user view)
    parser.screen_mut().set_scrollback(saved_sb);
    // Do NOT trim trailing blank lines here: this function is only reached
    // for an explicit -S/-E range (see connection.rs dispatch), and per the
    // sibling fast-path above (no-scrollback branch), an explicit range must
    // be honored line for line, including trailing blank rows inside the
    // range (e.g. `-S -5` on a fresh session with no scrollback should
    // return the full visible screen, not just the non-blank prefix).
    // The no-range attach path (capture_active_pane_text) has its own
    // trim for the iTerm2-initial-attach concern; it is never routed here.
    if text == "\n" { text.clear(); }
    Ok(Some(text))
}

/// Capture a pane's screen content with ANSI escape sequences preserved.
/// This is the `-e` flag for capture-pane.  Supports optional start/end range.
/// Negative -S values read from scrollback history; i32::MIN means all retained history.
/// `pane_id` is an explicit `-t %N` target (None = active pane); `preserve_trailing`
/// is the `-N` flag (keep trailing spaces per row, styled ones included).
pub fn capture_active_pane_styled(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, preserve_trailing: bool) -> io::Result<Option<String>> {
    let (win_idx, path) = capture_target(app, pane_id);
    let win = &mut app.windows[win_idx];
    let p = match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let rows = p.last_rows;
    let cols = p.last_cols;
    let last_row = rows.saturating_sub(1) as i32;

    // SGR delta tracker state (persists across scrollback batches)
    let mut prev_fg: Option<vt100::Color> = None;
    let mut prev_bg: Option<vt100::Color> = None;
    let mut prev_bold = false;
    let mut prev_dim = false;
    let mut prev_italic = false;
    let mut prev_underline = false;
    let mut prev_ul_style = vt100::UnderlineStyle::None;
    let mut prev_ulcolor: Option<vt100::Color> = None;
    let mut prev_blink = false;
    let mut prev_inverse = false;
    let mut prev_hidden = false;
    let mut prev_strikethrough = false;

    // Helper closure: render one screen row with SGR tracking
    let mut render_styled_row = |screen: &vt100::Screen, r: u16, text: &mut String| {
        let mut row_chars: Vec<String> = Vec::new();
        let mut row_sgr: Vec<Option<String>> = Vec::new();
        let mut any_style_active = false;
        for c in 0..cols {
            if let Some(cell) = screen.cell(r, c) {
                // Second half of a wide glyph: the leading half already emitted
                // the full multi-column character, so skip it entirely (issue
                // #443 fix must not add a phantom column after wide/CJK glyphs).
                if cell.is_wide_continuation() { continue; }
                let fg = cell.fgcolor();
                let bg = cell.bgcolor();
                let bold = cell.bold();
                let dim = cell.dim();
                let italic = cell.italic();
                let underline = cell.underline();
                let ul_style = cell.underline_style();
                let ulcolor = cell.underline_color();
                let blink = cell.blink();
                let inverse = cell.inverse();
                let hidden = cell.hidden();
                let strikethrough = cell.strikethrough();

                let style_changed = Some(fg) != prev_fg || Some(bg) != prev_bg
                    || bold != prev_bold || dim != prev_dim
                    || italic != prev_italic
                    || underline != prev_underline
                    || ul_style != prev_ul_style
                    || Some(ulcolor) != prev_ulcolor
                    || blink != prev_blink
                    || inverse != prev_inverse || hidden != prev_hidden
                    || strikethrough != prev_strikethrough;

                let sgr = if style_changed {
                    let mut params = Vec::new();
                    params.push("0".to_string());
                    if bold { params.push("1".to_string()); }
                    if dim { params.push("2".to_string()); }
                    if italic { params.push("3".to_string()); }
                    // tmux re-encodes styled underscores with the SGR 4
                    // subparameter form (grid.c grid_string_cells_code maps
                    // GRID_ATTR_UNDERSCORE_2..5 to codes 42..45 and prints
                    // them as "4:2".."4:5"), so a capture replays byte for
                    // byte in any terminal that understands undercurl.
                    match ul_style {
                        vt100::UnderlineStyle::None => {}
                        vt100::UnderlineStyle::Single => params.push("4".to_string()),
                        other => params.push(format!("4:{}", other.sgr_subparam())),
                    }
                    if blink { params.push("5".to_string()); }
                    if inverse { params.push("7".to_string()); }
                    if hidden { params.push("8".to_string()); }
                    if strikethrough { params.push("9".to_string()); }
                    match fg {
                        vt100::Color::Default => {}
                        vt100::Color::Idx(n) => {
                            if n < 8 { params.push(format!("{}", 30 + n)); }
                            else if n < 16 { params.push(format!("{}", 90 + n - 8)); }
                            else { params.push(format!("38;5;{}", n)); }
                        }
                        vt100::Color::Rgb(r, g, b) => { params.push(format!("38;2;{};{};{}", r, g, b)); }
                    }
                    match bg {
                        vt100::Color::Default => {}
                        vt100::Color::Idx(n) => {
                            if n < 8 { params.push(format!("{}", 40 + n)); }
                            else if n < 16 { params.push(format!("{}", 100 + n - 8)); }
                            else { params.push(format!("48;5;{}", n)); }
                        }
                        vt100::Color::Rgb(r, g, b) => { params.push(format!("48;2;{};{};{}", r, g, b)); }
                    }
                    match ulcolor {
                        vt100::Color::Default => {}
                        vt100::Color::Idx(n) => { params.push(format!("58;5;{}", n)); }
                        vt100::Color::Rgb(r, g, b) => { params.push(format!("58;2;{};{};{}", r, g, b)); }
                    }
                    prev_fg = Some(fg);
                    prev_bg = Some(bg);
                    prev_bold = bold;
                    prev_dim = dim;
                    prev_italic = italic;
                    prev_underline = underline;
                    prev_ul_style = ul_style;
                    prev_ulcolor = Some(ulcolor);
                    prev_blink = blink;
                    prev_inverse = inverse;
                    prev_hidden = hidden;
                    prev_strikethrough = strikethrough;
                    any_style_active = true;
                    Some(format!("\x1b[{}m", params.join(";")))
                } else {
                    None
                };
                row_sgr.push(sgr);
                // A never-written cell (skipped by a cursor advance) reports
                // empty contents; emit a space so interior gaps between words
                // survive instead of collapsing (issue #443).
                let contents = cell.contents();
                row_chars.push(if contents.is_empty() { " ".to_string() } else { contents.to_string() });
            } else {
                row_sgr.push(None);
                row_chars.push(" ".to_string());
            }
        }
        // -N (preserve_trailing): emit the full row width so styled trailing
        // spaces (e.g. a TUI's background fill painted to end-of-line) keep
        // their SGR when the capture is replayed. Without -N, trim after the
        // last non-whitespace cell like tmux does by default.
        let trim_end = if preserve_trailing {
            row_chars.len()
        } else {
            match row_chars.iter().rposition(|s| !s.is_empty() && s.trim() != "") { Some(pos) => pos + 1, None => 0 }
        };
        for c in 0..trim_end {
            if let Some(ref sgr) = row_sgr[c] { text.push_str(sgr); }
            text.push_str(&row_chars[c]);
        }
        if any_style_active {
            text.push_str("\x1b[0m");
            prev_fg = None;
            prev_bg = None;
            prev_bold = false;
            prev_dim = false;
            prev_italic = false;
            prev_underline = false;
            prev_ul_style = vt100::UnderlineStyle::None;
            prev_ulcolor = Some(vt100::Color::Default);
            prev_blink = false;
            prev_inverse = false;
            prev_hidden = false;
        }
        text.push('\n');
    };

    // Fast path: no scrollback needed
    let needs_scrollback = matches!(s, Some(v) if v < 0);
    if !needs_scrollback {
        let (start_row, end_row) = compute_capture_range(s, e, last_row as u16);
        let mut text = String::new();
        for r in start_row..=end_row {
            let screen = parser.screen();
            render_styled_row(screen, r, &mut text);
        }
        // Trim trailing all-empty rows — match the behaviour the plain
        // (non-styled) capture path has had for a long time (`while
        // text.ends_with("\n\n") { text.pop(); }` + the iTerm2 comment in
        // `capture_active_pane_text` and `capture_active_pane_range`).
        // The styled path needs its own helper because empty rows that
        // follow a styled row carry an `\x1b[0m` SGR reset between the
        // newlines, so the plain ends_with("\n\n") test misses them.
        // Without the trim, a downstream consumer that writes the snapshot
        // into a terminal (xterm.js for a screen-mirror UI, fresh xterm
        // window, …) leaves the cursor under the visible content — by as
        // many rows as the pane has trailing blanks. Aiball #531 + POC
        // (delta_y/x measured between display-message and xterm.js after
        // term.write) reported the same shift on tmux Linux even though
        // there the wrap at column 80 made it visually less obvious.
        trim_trailing_empty_styled_lines(&mut text);
        return Ok(Some(text));
    }

    // Scrollback-aware styled capture
    let saved_sb = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(usize::MAX);
    let max_sb = parser.screen().scrollback() as i64;
    parser.screen_mut().set_scrollback(saved_sb);

    let start_abs: i64 = match s {
        Some(v) if v == i32::MIN => -max_sb,
        Some(v) => (v as i64).max(-max_sb),
        None => 0,
    };
    let end_abs: i64 = match e {
        Some(v) if v < 0 => (v as i64).max(-max_sb),
        Some(v) => (v as i64).min(last_row as i64),
        None => last_row as i64,
    };

    if start_abs > end_abs { parser.screen_mut().set_scrollback(saved_sb); return Ok(Some(String::new())); }

    let mut text = String::new();
    let mut next_abs = start_abs;
    while next_abs <= end_abs {
        let target_sb = (-next_abs).max(0) as usize;
        parser.screen_mut().set_scrollback(target_sb);
        let actual_sb = parser.screen().scrollback() as i64;
        let vis_start_abs = -actual_sb;
        let vis_end_abs = -actual_sb + rows as i64 - 1;
        let read_start = next_abs.max(vis_start_abs);
        let read_end = end_abs.min(vis_end_abs);
        if read_start > read_end { break; }

        for aline in read_start..=read_end {
            let r = (aline + actual_sb) as u16;
            let screen = parser.screen();
            render_styled_row(screen, r, &mut text);
        }
        next_abs = read_end + 1;
    }

    parser.screen_mut().set_scrollback(saved_sb);
    trim_trailing_empty_styled_lines(&mut text);
    Ok(Some(text))
}

/// Strip trailing all-empty rows from a styled (`-e`) capture buffer.
///
/// A row in the styled output is one "line" terminated by `\n`. An empty
/// row carries either `\n` (no style ever active) or `\x1b[0m\n` (the
/// previous row left a style active and this row reset it). After all
/// trailing empty rows are gone, the buffer ends with the last
/// content-bearing row + its newline (or is empty if there was no
/// content at all). The plain-capture siblings already do this via
/// `while text.ends_with("\n\n") { text.pop(); }` — the styled path
/// needs its own helper because of the SGR resets between newlines.
fn trim_trailing_empty_styled_lines(text: &mut String) {
    loop {
        if !text.ends_with('\n') {
            return;
        }
        let trailing_nl_at = text.len() - 1;
        let line_start = text[..trailing_nl_at].rfind('\n').map_or(0, |i| i + 1);
        let last_line = &text[line_start..trailing_nl_at];
        // The line is "empty" when stripping any leading `\x1b[0m` SGR
        // resets leaves nothing.
        let mut remaining = last_line;
        while let Some(rest) = remaining.strip_prefix("\x1b[0m") {
            remaining = rest;
        }
        if !remaining.is_empty() {
            return; // last line has content — done.
        }
        text.truncate(line_start);
    }
}

#[cfg(test)]
mod trim_trailing_empty_styled_lines_tests {
    use super::trim_trailing_empty_styled_lines;

    fn run(input: &str) -> String {
        let mut s = String::from(input);
        trim_trailing_empty_styled_lines(&mut s);
        s
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(run(""), "");
    }

    #[test]
    fn no_trailing_newline_untouched() {
        assert_eq!(run("hello"), "hello");
        assert_eq!(run("\x1b[31mred"), "\x1b[31mred");
    }

    #[test]
    fn single_trailing_newline_after_content_kept() {
        assert_eq!(run("hello\n"), "hello\n");
        assert_eq!(run("first\nsecond\n"), "first\nsecond\n");
    }

    #[test]
    fn trailing_blank_lines_collapsed() {
        assert_eq!(run("hello\n\n\n\n"), "hello\n");
    }

    #[test]
    fn trailing_sgr_reset_lines_collapsed() {
        // Each trailing empty row carries an SGR reset before its \n.
        assert_eq!(run("hello\n\x1b[0m\n\x1b[0m\n"), "hello\n");
    }

    #[test]
    fn mixed_trailing_blank_and_reset_lines_collapsed() {
        assert_eq!(run("hello\n\n\x1b[0m\n\n\x1b[0m\n"), "hello\n");
    }

    #[test]
    fn fully_empty_input_truncates() {
        // Only resets + newlines, no content anywhere.
        assert_eq!(run("\n\n\n"), "");
        assert_eq!(run("\x1b[0m\n\x1b[0m\n"), "");
    }

    #[test]
    fn content_with_inline_resets_kept() {
        // SGR reset in the middle of a content row is part of that row, not a
        // trailing-empty marker — must NOT be confused.
        assert_eq!(
            run("\x1b[31mred\x1b[0m text\n\n"),
            "\x1b[31mred\x1b[0m text\n",
        );
    }

    #[test]
    fn multiple_sgr_resets_on_an_empty_line_still_empty() {
        // Pathological but legal: a row that resets and then resets again
        // (some renderers may emit double resets defensively).
        assert_eq!(run("hello\n\x1b[0m\x1b[0m\n"), "hello\n");
    }
}

/// Safety net for the paragraph walk. The walk normally terminates because
/// stepping stops making progress at the top/bottom of the buffer; this only
/// bounds a pathological history.
const PARAGRAPH_WALK_LIMIT: usize = 100_000;

/// Is the given visible row blank (whitespace only)?
fn row_is_blank(app: &mut AppState, row: u16) -> bool {
    match read_row_text(app, row) {
        Some((text, _)) => text.trim().is_empty(),
        None => true,
    }
}

/// Identifies the buffer line the copy cursor sits on. Comparing this before
/// and after a step tells us whether the cursor actually moved, which is how
/// the paragraph walk detects the top/bottom of the buffer.
fn copy_walk_key(app: &AppState) -> (usize, u16) {
    (app.copy_scroll_offset, app.copy_pos.map(|(r, _)| r).unwrap_or(0))
}

/// Step the copy cursor one line up or down, scrolling the view when the step
/// crosses the edge of the visible area. Returns false when the cursor could
/// not move (top or bottom of the buffer).
fn step_copy_line(app: &mut AppState, down: bool) -> bool {
    let before = copy_walk_key(app);
    move_copy_cursor(app, 0, if down { 1 } else { -1 });
    copy_walk_key(app) != before
}

/// Move to the blank line that ends the current paragraph — } key.
///
/// Mirrors tmux's window_copy_next_paragraph(): skip any blank lines under the
/// cursor, then walk the paragraph body, landing on the following blank line.
/// The walk steps through move_copy_cursor so it scrolls into history at the
/// edge of the viewport instead of stopping there (tmux walks the whole
/// buffer, not just the visible screen).
pub fn move_next_paragraph(app: &mut AppState) {
    let (r, _) = match get_copy_pos(app) { Some(p) => p, None => return };
    app.copy_pos = Some((r, 0));
    let mut row = r;
    let mut steps = 0usize;
    // Skip the blank lines the cursor is sitting in.
    while steps < PARAGRAPH_WALK_LIMIT && row_is_blank(app, row) {
        if !step_copy_line(app, true) { return; }
        row = app.copy_pos.map(|(rr, _)| rr).unwrap_or(row);
        steps += 1;
    }
    // Then walk the paragraph body to the blank line that follows it.
    while steps < PARAGRAPH_WALK_LIMIT && !row_is_blank(app, row) {
        if !step_copy_line(app, true) { return; }
        row = app.copy_pos.map(|(rr, _)| rr).unwrap_or(row);
        steps += 1;
    }
    app.copy_pos = Some((row, 0));
}

/// Move to the blank line that starts the current paragraph — { key.
///
/// Mirrors tmux's window_copy_previous_paragraph(), the upward counterpart of
/// move_next_paragraph().
pub fn move_prev_paragraph(app: &mut AppState) {
    let (r, _) = match get_copy_pos(app) { Some(p) => p, None => return };
    app.copy_pos = Some((r, 0));
    let mut row = r;
    let mut steps = 0usize;
    while steps < PARAGRAPH_WALK_LIMIT && row_is_blank(app, row) {
        if !step_copy_line(app, false) { return; }
        row = app.copy_pos.map(|(rr, _)| rr).unwrap_or(row);
        steps += 1;
    }
    while steps < PARAGRAPH_WALK_LIMIT && !row_is_blank(app, row) {
        if !step_copy_line(app, false) { return; }
        row = app.copy_pos.map(|(rr, _)| rr).unwrap_or(row);
        steps += 1;
    }
    app.copy_pos = Some((row, 0));
}

/// Scroll the line containing the cursor to the middle of the pane — z key.
///
/// Mirrors tmux's window_copy_cmd_scroll_middle(): the cursor stays on the
/// same buffer line and the view moves under it, clamped by how much
/// scrollback is actually available in that direction.
pub fn scroll_middle(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    let mid = p.last_rows.saturating_sub(1) / 2;
    let current = parser.screen().scrollback();
    // The cursor's buffer line is `base - scrollback + row`, so holding that
    // line fixed while the row becomes `mid` means the scrollback offset has
    // to change by `mid - row` in the same direction.
    if r < mid {
        // Cursor above the middle: pull older lines in above it (scroll up).
        parser.screen_mut().set_scrollback(current.saturating_add((mid - r) as usize));
        // set_scrollback clamps at the top of the history, so use what it
        // actually applied rather than what we asked for.
        let applied = parser.screen().scrollback().saturating_sub(current) as u16;
        app.copy_scroll_offset = parser.screen().scrollback();
        app.copy_pos = Some((r.saturating_add(applied), c));
    } else if r > mid {
        // Cursor below the middle: scroll down, bounded by the offset we have.
        let applied = ((r - mid) as usize).min(current);
        parser.screen_mut().set_scrollback(current - applied);
        app.copy_scroll_offset = parser.screen().scrollback();
        app.copy_pos = Some((r.saturating_sub(applied as u16), c));
    }
}

/// Move to matching bracket — % key
pub fn move_matching_bracket(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let win = match app.windows.get(app.active_idx) { Some(w) => w, None => return };
    let p = match active_pane(&win.root, &win.active_path) { Some(p) => p, None => return };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    let screen = parser.screen();
    
    // Get char at cursor
    let ch = screen.cell(r, c).map(|cell| {
        let t = cell.contents();
        t.chars().next().unwrap_or(' ')
    }).unwrap_or(' ');
    
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        '<' => ('<', '>', true),
        '>' => ('<', '>', false),
        _ => return,
    };
    
    let rows = p.last_rows;
    let cols = p.last_cols;
    let mut depth = 1i32;
    let mut cr = r;
    let mut cc = c;
    
    loop {
        if forward {
            cc += 1;
            if cc >= cols { cc = 0; cr += 1; }
            if cr >= rows { return; }
        } else {
            if cc == 0 {
                if cr == 0 { return; }
                cr -= 1;
                cc = cols.saturating_sub(1);
            } else { cc -= 1; }
        }
        
        let cell_ch = screen.cell(cr, cc).map(|cell| {
            cell.contents().chars().next().unwrap_or(' ')
        }).unwrap_or(' ');
        
        if cell_ch == open { depth += if forward { 1 } else { -1 }; }
        if cell_ch == close { depth += if forward { -1 } else { 1 }; }
        if depth == 0 {
            app.copy_pos = Some((cr, cc));
            return;
        }
    }
}

// ── Text Object Selection ──────────────────────────────────────────────

/// Select "inner word" (iw) — word under cursor without surrounding whitespace.
/// Uses `char_class` for word boundary detection (same as `w`/`b`/`e` motions).
pub fn select_inner_word(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let seps = app.word_separators.clone();
    let (text, _cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let col = c as usize;
    if col >= bytes.len() { return; }
    let cls = char_class(bytes[col], &seps);
    // Find start of word
    let mut start = col;
    while start > 0 && char_class(bytes[start - 1], &seps) == cls { start -= 1; }
    // Find end of word
    let mut end = col;
    while end + 1 < bytes.len() && char_class(bytes[end + 1], &seps) == cls { end += 1; }
    app.copy_anchor = Some((r, start as u16));
    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
    app.copy_pos = Some((r, end as u16));
    app.copy_selection_mode = crate::types::SelectionMode::Char;
}

/// Select "a word" (aw) — word under cursor plus trailing whitespace.
pub fn select_a_word(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let seps = app.word_separators.clone();
    let (text, _cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let col = c as usize;
    if col >= bytes.len() { return; }
    let cls = char_class(bytes[col], &seps);
    // Find start of word
    let mut start = col;
    while start > 0 && char_class(bytes[start - 1], &seps) == cls { start -= 1; }
    // Find end of word
    let mut end = col;
    while end + 1 < bytes.len() && char_class(bytes[end + 1], &seps) == cls { end += 1; }
    // Include trailing whitespace
    while end + 1 < bytes.len() && bytes[end + 1].is_whitespace() { end += 1; }
    app.copy_anchor = Some((r, start as u16));
    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
    app.copy_pos = Some((r, end as u16));
    app.copy_selection_mode = crate::types::SelectionMode::Char;
}

/// Select "inner WORD" (iW) — whitespace-delimited token without surrounding whitespace.
pub fn select_inner_word_big(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let (text, _cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let col = c as usize;
    if col >= bytes.len() { return; }
    if bytes[col].is_whitespace() {
        // Cursor on whitespace — select contiguous whitespace
        let mut start = col;
        while start > 0 && bytes[start - 1].is_whitespace() { start -= 1; }
        let mut end = col;
        while end + 1 < bytes.len() && bytes[end + 1].is_whitespace() { end += 1; }
        app.copy_anchor = Some((r, start as u16));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, end as u16));
    } else {
        // Cursor on non-whitespace — select contiguous non-whitespace
        let mut start = col;
        while start > 0 && !bytes[start - 1].is_whitespace() { start -= 1; }
        let mut end = col;
        while end + 1 < bytes.len() && !bytes[end + 1].is_whitespace() { end += 1; }
        app.copy_anchor = Some((r, start as u16));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, end as u16));
    }
    app.copy_selection_mode = crate::types::SelectionMode::Char;
}

/// Select "a WORD" (aW) — whitespace-delimited token plus trailing whitespace.
pub fn select_a_word_big(app: &mut AppState) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let (text, _cols) = match read_row_text(app, r) { Some(t) => t, None => return };
    let bytes: Vec<char> = text.chars().collect();
    let col = c as usize;
    if col >= bytes.len() { return; }
    if bytes[col].is_whitespace() {
        // Cursor on whitespace — select contiguous whitespace
        let mut start = col;
        while start > 0 && bytes[start - 1].is_whitespace() { start -= 1; }
        let mut end = col;
        while end + 1 < bytes.len() && bytes[end + 1].is_whitespace() { end += 1; }
        app.copy_anchor = Some((r, start as u16));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, end as u16));
    } else {
        // Cursor on non-whitespace — select contiguous non-whitespace
        let mut start = col;
        while start > 0 && !bytes[start - 1].is_whitespace() { start -= 1; }
        let mut end = col;
        while end + 1 < bytes.len() && !bytes[end + 1].is_whitespace() { end += 1; }
        // Include trailing whitespace
        while end + 1 < bytes.len() && bytes[end + 1].is_whitespace() { end += 1; }
        app.copy_anchor = Some((r, start as u16));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, end as u16));
    }
    app.copy_selection_mode = crate::types::SelectionMode::Char;
}

#[cfg(test)]
#[path = "../tests-rs/test_issue443_blank_cell_capture.rs"]
mod tests_issue443_blank_cell_capture;

#[cfg(test)]
#[path = "../tests-rs/test_capture_pane_fidelity.rs"]
mod tests_capture_pane_fidelity;

#[cfg(test)]
#[path = "../tests-rs/test_copy_cancel_stale_state.rs"]
mod tests_copy_cancel_stale_state;

#[cfg(test)]
#[path = "../tests-rs/test_issue612_copy_search_scrollback.rs"]
mod tests_issue612_copy_search_scrollback;
