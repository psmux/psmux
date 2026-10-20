use std::io::{self, Write};

use crate::clipboard::copy_to_system_clipboard;
use crate::types::{AppState, Mode, CopyModeState};
use crate::tree::{active_pane, active_pane_mut};
use crate::grid_reader::{Grid, GridReader, ScreenGrid};

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

/// True while the focused pane is in copy mode, including its search prompt.
pub fn in_copy_mode(app: &AppState) -> bool {
    app.mode.in_copy()
}

pub fn enter_copy_mode(app: &mut AppState) {
    // tmux only creates the mode when the pane is not already in it
    // (`window_pane_set_mode` returns 1 for the same mode), so `copy-mode`
    // run again inside copy mode keeps the hidden indicator (#704).
    let fresh = !in_copy_mode(app);
    app.mode = Mode::CopyMode;
    // Copy mode reads a snapshot of the pane's screen (tmux's copy-mode grid):
    // the application keeps running underneath, so new output can neither
    // shift nor evict what the user is reading.
    {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            p.enter_copy_snapshot();
        }
    }
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
    // Nothing has pinned the endpoint to an older view, and a pin left by an
    // earlier session must not survive into this one: `CopyModeState` does not
    // carry it across a pane switch either.
    app.copy_pos_scroll_offset = None;
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
    app.copy_refresh_version = 0;
    app.copy_search_marks = false;
    // tmux sets `hide_position` from the `-H` flag every time the mode is
    // created (`window-copy.c` `window_copy_init`), so a plain entry always
    // shows the indicator. `enter_copy_mode_hidden` is the `-H` path.
    if fresh {
        app.copy_hide_position = false;
    }
    // Mark the active pane as being in copy mode (pane-local state).
    save_copy_state_to_pane(app);
}

/// `copy-mode -H`: enter copy mode with the position indicator hidden (#704).
///
/// Separate from `enter_copy_mode` rather than a parameter on it: that function
/// has more than forty callers and every one of them wants the indicator.
/// Like tmux, `-H` only counts when the mode is created: a pane already in
/// copy mode keeps whatever `toggle-position` left it with.
pub fn enter_copy_mode_hidden(app: &mut AppState) {
    let fresh = !in_copy_mode(app);
    enter_copy_mode(app);
    if fresh {
        app.copy_hide_position = true;
        save_copy_state_to_pane(app);
    }
}

/// The flags of a `copy-mode` command that change what it does, read the way
/// tmux's `cmd_copy_mode_exec` reads them (cmd-copy-mode.c).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CopyModeFlags {
    /// `-q`: leave the pane's mode instead of entering copy mode.
    pub quit: bool,
    /// `-H`: a fresh entry starts with the position indicator hidden.
    pub hide_position: bool,
    /// `-u`: page up after entering.
    pub page_up: bool,
}

impl CopyModeFlags {
    /// Read the flags from the arguments after `copy-mode`. Clustered flags
    /// (`-uH`) count like separate ones, and the values of `-t` and `-s` are
    /// skipped so a target is never read as flags.
    pub fn parse<S: AsRef<str>>(args: &[S]) -> Self {
        let mut flags = CopyModeFlags::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_ref();
            i += 1;
            if a == "--" { break; }
            if !a.starts_with('-') || a.len() < 2 { continue; }
            for (pos, ch) in a[1..].char_indices() {
                match ch {
                    'q' => flags.quit = true,
                    'H' => flags.hide_position = true,
                    'u' => flags.page_up = true,
                    't' | 's' => {
                        // The value is the rest of this argument, or the next one.
                        if pos + 1 == a.len() - 1 { i += 1; }
                        break;
                    }
                    _ => {}
                }
            }
        }
        flags
    }
}

/// Is `cmd` a `copy-mode` command that pages up (`-u`)? These are the
/// bindings `scroll-enter-copy-mode off` skips so the key reaches the pane
/// (#284). The flags are read the way `copy-mode` itself reads them, so a
/// cluster such as `-Hu` counts and a target such as `-t my-ubuntu` does not.
/// A text search for `-u` got both of those wrong.
pub fn is_page_up_copy_mode_command(cmd: &str) -> bool {
    // Only the first command of a `\;` chain decides, as the old prefix test did.
    let first = crate::config::split_chained_commands_pub(cmd).into_iter().next().unwrap_or_default();
    let parts = crate::commands::parse_command_line(&first);
    parts.first().map(|s| s.as_str()) == Some("copy-mode")
        && CopyModeFlags::parse(&parts[1..]).page_up
}

/// What running a `copy-mode` command did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyModeOutcome {
    /// The pane entered copy mode, or `-q` left a mode.
    ModeChanged,
    /// `-q` with no mode to leave.
    Nothing,
    /// `-u` with `scroll-enter-copy-mode` off: the caller forwards PageUp to
    /// the pane instead (#284).
    ForwardPageUp,
}

/// Run a `copy-mode` command on the focused pane the way tmux does:
/// `-q` leaves the mode (`window_pane_reset_mode_all`, which for psmux means
/// copy mode and clock mode, the two modes `pane_in_mode` reports), otherwise
/// the pane enters copy mode, hidden for `-H`, and `-u` pages up.
pub fn run_copy_mode_command(app: &mut AppState, flags: CopyModeFlags) -> CopyModeOutcome {
    if flags.quit {
        if in_copy_mode(app) {
            exit_copy_mode(app);
            return CopyModeOutcome::ModeChanged;
        }
        if matches!(app.mode, Mode::ClockMode) {
            app.mode = Mode::Passthrough;
            return CopyModeOutcome::ModeChanged;
        }
        return CopyModeOutcome::Nothing;
    }
    if flags.page_up && !app.scroll_enter_copy_mode {
        return CopyModeOutcome::ForwardPageUp;
    }
    if flags.hide_position {
        enter_copy_mode_hidden(app);
    } else {
        enter_copy_mode(app);
    }
    if flags.page_up {
        page_scroll(app, true, false);
    }
    CopyModeOutcome::ModeChanged
}

/// Exit copy mode: reset all copy state and scroll the active pane back to
/// live output.  Every copy-mode exit path should call this to avoid leaving
/// a pane scrolled while no longer in copy mode (fixes #43).
pub fn exit_copy_mode(app: &mut AppState) {
    app.mode = Mode::Passthrough;
    app.copy_anchor = None;
    app.copy_pos = None;
    app.copy_mouse_down_cell = None;
    app.copy_pos_published = None;
    app.copy_pos_scroll_offset = None;
    app.copy_scroll_offset = 0;
    // Clear the search prompt if it was lingering from CopySearch (#335).
    app.status_message = None;
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        // Clear the pane-local copy state so re-entering this pane won't
        // restore a stale copy mode.
        p.copy_state = None;
        // Show the live screen again first: `term` is the copy-mode snapshot
        // while we are here, and it is the *live* parser that has to end up
        // unscrolled (it may have been direct-scrolled when copy mode started,
        // #193).
        p.leave_copy_snapshot();
        if let Ok(mut parser) = p.term.lock() {
            parser.screen_mut().set_scrollback(0);
        }
    }
}

/// Keep copy-mode snapshots in sync with the modes the panes are in.
///
/// Copy mode displays a snapshot of the pane's screen (see
/// `Pane::enter_copy_snapshot`).  Entering and leaving normally go through
/// `enter_copy_mode` / `exit_copy_mode`, but those are not the only paths that
/// change `app.mode` or move focus; a missed restore would leave a pane frozen
/// on an old screen, and a missed snapshot would put copy mode back on the live
/// grid.  This is the idempotent reconciliation, safe to call per frame:
/// every pane in copy mode holds a snapshot, exactly while it is in copy mode.
///
/// A mode belongs to ONE pane, as it does in tmux, where `window_copy_init`
/// copies the grid of whichever `window_pane` enters the mode and that copy
/// lives on the pane until the mode is dismissed, focused or not.  psmux keeps
/// the live copy cursor in `AppState`, so the focused pane's mode is
/// `app.mode` and every other pane's is parked in its own `copy_state` (#607),
/// which is the same split `#{pane_in_mode}` answers from.  Gating this on the
/// active pane of the active window (as PR #671 did) meant a pane put into
/// copy mode with `copy-mode -t` lost its snapshot on the very next frame, and
/// its own output went back to pushing the view (#673).
pub fn sync_copy_snapshot(app: &mut AppState) {
    let focused_in_copy = in_copy_mode(app);
    let active_id = app
        .windows
        .get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.id);
    for win in app.windows.iter_mut() {
        crate::tree::for_each_pane_mut(&mut win.root, &mut |p: &mut crate::types::Pane| {
            let want = if Some(p.id) == active_id {
                focused_in_copy
            } else {
                p.copy_state.is_some()
            };
            if want {
                p.enter_copy_snapshot();
            } else {
                p.leave_copy_snapshot();
            }
        });
    }
}

/// Save the current global copy-mode state into the active pane.
/// Called whenever we are about to switch away from a pane that is in copy mode.
pub fn save_copy_state_to_pane(app: &mut AppState) {
    let state = live_copy_state(app);
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        p.copy_state = Some(state);
    }
}

/// The live copy mode held in `AppState`, as the state a pane parks.
fn live_copy_state(app: &AppState) -> CopyModeState {
    let (in_search, search_input, search_input_forward) = match &app.mode {
        Mode::CopySearch { input, forward } => (true, input.clone(), *forward),
        _ => (false, String::new(), true),
    };
    CopyModeState {
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
        hide_position: app.copy_hide_position,
        in_search,
        search_input,
        search_input_forward,
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
        app.copy_hide_position = s.hide_position;
        if s.in_search {
            app.mode = Mode::CopySearch { input: s.search_input, forward: s.search_input_forward };
            // The saved state carries the typed text, not a C-w cut, so C-y
            // starts again from the paste buffer rather than from a cut made
            // at another pane's prompt.
            app.prompt_saved = None;
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
    let previous = crate::program_status::focused_identity(app);
    let was_copy = in_copy_mode(app);
    let had_prompt = copy_prompt_text(&app.mode).is_some();
    let prev_pane = active_pane_id(app);
    if was_copy {
        save_copy_state_to_pane(app);
    }
    switch_fn(app);
    if crate::program_status::focused_identity(app) != previous {
        crate::program_status::acknowledge_focused(app);
    }
    if active_pane_id(app) != prev_pane {
        park_clock(app, prev_pane);
    }
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
    adopt_clock(app);
    sync_copy_prompt_status(app, had_prompt);
}

/// The status line text of the copy mode prompt `mode` holds open, if any.
///
/// The prompts are drawn as a sticky status message, and the status line is
/// not per pane, so this is the one place that says what the message reads.
pub fn copy_prompt_text(mode: &Mode) -> Option<String> {
    match mode {
        Mode::CopySearch { input, forward } => {
            let arrow = if *forward { "down" } else { "up" };
            Some(format!("(search {}) {}", arrow, input))
        }
        Mode::CopyGoto { input } => Some(format!("(goto line) {}", input)),
        Mode::CopyCommandPrompt(p) => Some(p.status_text()),
        _ => None,
    }
}

/// Where the label ends and where the cursor sits inside the text
/// `copy_prompt_text` returns, both as byte offsets into it.
///
/// `back` is `AppState::copy_prompt_back`, the number of characters between
/// the cursor and the end of the input, which is how the line editor keeps it
/// (tmux keeps the same position as `prompt_index`, prompt.c). Byte offsets
/// are what the client needs, because `util::prompt_window` takes one, so the
/// conversion happens here where the characters are rather than on the wire.
///
/// The label is reported separately because only the input scrolls when the
/// text outgrows the line: tmux draws the label at `ax`, then the input from
/// `ax + start` with its own offset (status.c:922 to :948), so the label stays
/// put and the user can still see which prompt is open.
pub fn copy_prompt_cursor(mode: &Mode, back: usize) -> Option<(usize, usize)> {
    let text = copy_prompt_text(mode)?;
    let (label_len, back) = match mode {
        Mode::CopySearch { input, .. } | Mode::CopyGoto { input } => (
            text.len() - input.len(),
            // A prompt that was answered and reopened can carry a stale back,
            // and a history recall replaces the input underneath it, so the
            // cursor is clamped to the text that is actually there.
            back.min(input.chars().count()),
        ),
        // A prompt opened by a copy mode binding edits through the same
        // editor (`copy_prompt::feed`) and keeps its cursor in the same field.
        Mode::CopyCommandPrompt(p) => (
            text.len() - p.input.len(),
            back.min(p.input.chars().count()),
        ),
        _ => (text.len(), 0),
    };
    let mut at = text.len();
    for ch in text.chars().rev().take(back) {
        at -= ch.len_utf8();
    }
    Some((label_len, at))
}

/// Make the status line show the prompt of the pane that has focus now.
///
/// A prompt's sticky message stayed behind when focus moved: `select-pane`
/// away from a pane with `(goto line) 1` or `(search down) x` open left that
/// text on the status line over a pane that was not even in copy mode, until
/// something else replaced it. `had_prompt` says whether the pane focus left
/// had a prompt open, which is the only case where the message is ours to
/// clear.
fn sync_copy_prompt_status(app: &mut AppState, had_prompt: bool) {
    if let Some(text) = copy_prompt_text(&app.mode) {
        app.status_message = Some((text, std::time::Instant::now(), Some(0)));
    } else if had_prompt {
        app.status_message = None;
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
    if in_copy_mode(app) {
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
    let had_prompt = copy_prompt_text(&app.mode).is_some();
    park_clock(app, prev);
    retarget_mode_inner(app);
    sync_copy_prompt_status(app, had_prompt);
}

/// Move the focus for one -t request without moving the focused pane's mode
/// with it (#828).  `switch_fn` changes the active pane; the pane that loses
/// focus keeps its own copy or clock mode parked, and the pane that gains it
/// brings its own back, as `window_pane_key` in tmux sends a key to the mode
/// of the pane it targets.  The status line is left alone: the switch is
/// undone after the request.
///
/// Only a pane's own mode moves.  A popup, menu, confirm or command prompt
/// belongs to the client in tmux, never to a pane, and stays where it is.
/// `prompts` says whether a copy mode prompt (search, goto line, a copy mode
/// command prompt) moves too: not when the request starts, so a prompt the
/// user is typing into is never touched by a script's -t command, but yes
/// when it ends, so a prompt opened under the target stays on the target.
/// Returns whether the modes moved.
pub fn switch_focus_keeping_modes<F: FnOnce(&mut AppState)>(app: &mut AppState, prompts: bool, switch_fn: F) -> bool {
    let prev = active_pane_id(app);
    let pane_mode = matches!(app.mode, Mode::Passthrough | Mode::ClockMode | Mode::CopyMode)
        || (prompts && app.mode.in_copy());
    switch_fn(app);
    if !pane_mode || active_pane_id(app) == prev {
        return false;
    }
    // The live state still belongs to `prev`: park it there by id.
    if in_copy_mode(app) {
        if let Some(id) = prev {
            let state = live_copy_state(app);
            if let Some(p) = crate::tree::find_pane_mut_by_id_global(app, id) {
                p.copy_state = Some(state);
            }
        }
    }
    park_clock(app, prev);
    retarget_mode_inner(app);
    true
}

/// The focus is leaving pane `prev`: if it is in clock mode, the mode stays
/// with that pane instead of following the focus.
fn park_clock(app: &mut AppState, prev: Option<usize>) {
    if matches!(app.mode, Mode::ClockMode) {
        if let Some(id) = prev {
            if !app.clock_panes.contains(&id) {
                app.clock_panes.push(id);
            }
        }
        app.mode = Mode::Passthrough;
    }
}

/// The focused pane takes back a clock mode it had parked.
fn adopt_clock(app: &mut AppState) {
    let Some(id) = active_pane_id(app) else { return };
    if let Some(i) = app.clock_panes.iter().position(|&p| p == id) {
        if matches!(app.mode, Mode::Passthrough) {
            app.clock_panes.swap_remove(i);
            app.mode = Mode::ClockMode;
        }
    }
}

fn retarget_mode_inner(app: &mut AppState) {
    let has_copy = app.windows.get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map_or(false, |p| p.copy_state.is_some());
    if has_copy {
        restore_copy_state_from_pane(app);
    } else if in_copy_mode(app) {
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
    adopt_clock(app);
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
    let nc = if dy == 0 {
        step_over_wide(parser.screen(), r, c, dx, cols)
    } else {
        (c as i16 + dx).max(0).min(cols as i16 - 1) as u16
    };
    // If cursor would move above the visible area, scroll up into scrollback
    let nr = if desired_r < 0 {
        let scroll_lines = (-desired_r) as usize;
        let current = parser.screen().scrollback();
        parser.screen_mut().set_scrollback(current.saturating_add(scroll_lines));
        app.copy_scroll_offset = parser.screen().scrollback();
        0
    }
    // If cursor would move below the visible area, scroll down (reduce scrollback)
    else if desired_r >= rows as i16 {
        let scroll_lines = (desired_r - rows as i16 + 1) as usize;
        let current = parser.screen().scrollback();
        if current > 0 {
            parser.screen_mut().set_scrollback(current.saturating_sub(scroll_lines));
            app.copy_scroll_offset = parser.screen().scrollback();
        }
        // Already at bottom, clamp
        rows.saturating_sub(1)
    } else {
        desired_r as u16
    };
    // An up or down step keeps the column, as tmux's does, even when it is the
    // second cell of a wide character on the new row. Moving it to the first
    // cell would lose the column: up and back down over a CJK line would end
    // one column to the left of where it began.
    app.copy_pos = Some((nr, nc));
}

/// Whether (`row`, `col`) is the second cell of a wide character, a CJK
/// character or an emoji sequence. It is padding: the character is drawn from
/// its first cell.
fn is_padding(screen: &vt100::Screen, row: u16, col: u16) -> bool {
    screen.cell(row, col).is_some_and(|cell| cell.is_wide_continuation())
}

/// `col`, or the first cell of the wide character whose second cell it is.
fn off_padding(screen: &vt100::Screen, row: u16, col: u16) -> u16 {
    let mut x = col;
    while x > 0 && is_padding(screen, row, x) {
        x -= 1;
    }
    x
}

/// The column one character left (`dx < 0`) or right of `col` on `row`.
///
/// Right steps over the second cell of a wide character, as tmux's
/// grid_reader_cursor_right does (grid-reader.c), and stays put when that
/// second cell is the last column. Stepping one cell at a time stopped on the
/// second cell, so a wide character took two presses to cross.
///
/// Left is tmux's grid_reader_cursor_left: from the second cell of a wide
/// character it first backs off to the first cell, then steps one cell, so it
/// can stop on the second cell of the character before. A left walk therefore
/// visits the same columns as tmux's, and a selection made leftwards copies
/// what tmux's copies.
fn step_over_wide(screen: &vt100::Screen, row: u16, col: u16, dx: i16, cols: u16) -> u16 {
    let last = cols.saturating_sub(1);
    if dx > 0 {
        let mut x = col.saturating_add(1).min(last);
        while x < last && is_padding(screen, row, x) {
            x += 1;
        }
        off_padding(screen, row, x)
    } else if dx < 0 {
        off_padding(screen, row, col.min(last)).saturating_sub(1)
    } else {
        col.min(last)
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

/// Whether the cell at (`row`, `col`) is blank the way tmux's grid_line_length
/// sees it: a space, or a cell nothing was ever written to. The second cell of
/// a wide character is not blank.
fn is_blank_cell(screen: &vt100::Screen, row: u16, col: u16) -> bool {
    match screen.cell(row, col) {
        Some(cell) => !cell.is_wide_continuation() && matches!(cell.contents(), "" | " "),
        None => true,
    }
}

/// The length of visible row `row` with its trailing blanks left out, tmux's
/// grid_line_length: one past the last cell that holds something.
fn row_text_length(screen: &vt100::Screen, row: u16, cols: u16) -> u16 {
    let mut x = cols;
    while x > 0 && is_blank_cell(screen, row, x - 1) {
        x -= 1;
    }
    x
}

/// The first visible row of the line `row` is part of: up while the row above
/// wrapped into the one below it.
fn logical_line_first_row(screen: &vt100::Screen, row: u16) -> u16 {
    let mut r = row;
    while r > 0 && screen.row_wrapped(r - 1) {
        r -= 1;
    }
    r
}

/// Append columns `sx..ex` of `row` to a copy, tmux's window_copy_copy_line.
///
/// Both ends are clamped to the row's text, so blanks after the text are not
/// copied and blanks between words are. A row that wrapped keeps its full
/// width instead (its spaces run on into the next row), and gets no newline
/// when the range reaches its end, so a wrapped line copies as one line.
fn copy_selection_row(out: &mut String, screen: &vt100::Screen, row: u16, sx: u16, ex: u16, cols: u16) {
    if sx > ex {
        return;
    }
    let wrapped = screen.row_wrapped(row);
    let xx = if wrapped { cols } else { row_text_length(screen, row, cols) };
    let (sx, ex) = (sx.min(xx), ex.min(xx));
    if sx < ex {
        out.push_str(&capture_row_text(screen, row, sx..ex));
    }
    if !wrapped || ex != xx {
        out.push('\n');
    }
}

/// Where the last row of a selection stops, tmux's window_copy_get_selection:
/// the end column is clamped to the row's text, and with emacs keys the cell
/// under the end is left out ("keep the top-left-most character, drop the
/// bottom-right-most, regardless of copy direction") while vi keeps it.
/// Returns (lastex, ey_last): the exclusive end, and the row's text length.
///
/// With vi keys tmux 3.7 and later never leave the cursor past the last
/// character (window_copy_cursor_limit, which a mouse drag goes through too),
/// so the vi end is held on the last character here. A drag past the text
/// then copies the text without a newline, as tmux 3.8 does; tmux 3.4, which
/// had no limit, added one.
fn selection_last_end(screen: &vt100::Screen, row: u16, end_col: u16, cols: u16, emacs: bool) -> (u16, u16) {
    let ey_last = row_text_length(screen, row, cols);
    if emacs {
        (end_col.min(ey_last), ey_last)
    } else {
        (end_col.min(ey_last.saturating_sub(1)) + 1, ey_last)
    }
}

/// tmux drops the newline a copy ends with ("unless at end in vi mode": a vi
/// selection whose end is past the text of its last row keeps it), and a
/// last row that wrapped exactly at the end added none to drop.
fn trim_selection_newline(text: &mut String, last_wrapped: bool, lastex: u16, ey_last: u16, emacs: bool) {
    if (emacs || lastex <= ey_last) && (!last_wrapped || lastex != ey_last) && text.ends_with('\n') {
        text.pop();
    }
}

/// The cells a character selection highlights, as inclusive (row, col)
/// bounds in display rows: `start` and `end` are its two ends in reading
/// order. With emacs keys the end cell is not selected (tmux
/// screen_check_selection), so the highlight stops one cell before it, and
/// `None` means nothing is selected yet (both ends on one cell). tmux still
/// paints column 0 when the end is at column 0; psmux paints what the copy
/// takes instead, so the highlight never shows a cell the buffer will not get.
pub fn char_selection_highlight(start: (u16, u16), end: (u16, u16), emacs: bool, cols: u16) -> Option<((u16, u16), (u16, u16))> {
    if !emacs {
        return Some((start, end));
    }
    if end.1 > 0 {
        if start.0 == end.0 && end.1 <= start.1 {
            return None;
        }
        return Some((start, (end.0, end.1 - 1)));
    }
    if end.0 <= start.0 {
        return None;
    }
    Some((start, (end.0 - 1, cols.saturating_sub(1))))
}

/// Where end-of-line puts the cursor, tmux's window_copy_cursor_end_of_line.
///
/// It follows a wrapped line to its last row (grid_reader_cursor_end_of_line
/// with wrap), scrolling down out of the scrollback when that row is below
/// the screen, and stops at the end of the text there, trailing blanks left
/// out. With vi keys the cursor stays on the last character, as tmux 3.7 and
/// later do (window_copy_cursor_limit); with emacs keys it goes one past it.
/// In a rectangle selection it goes to the last column, as tmux's does.
fn line_end_pos(parser: &mut vt100::Parser, row: u16, rows: u16, cols: u16, vi: bool, rect: bool) -> (u16, u16) {
    let last_row = rows.saturating_sub(1);
    let mut r = row.min(last_row);
    while parser.screen().row_wrapped(r) {
        if r < last_row {
            r += 1;
        } else {
            let sb = parser.screen().scrollback();
            if sb == 0 {
                break;
            }
            parser.screen_mut().set_scrollback(sb - 1);
        }
    }
    let last_col = cols.saturating_sub(1);
    if rect {
        return (r, last_col);
    }
    let screen = parser.screen();
    let len = row_text_length(screen, r, cols);
    let x = if vi { len.saturating_sub(1) } else { len };
    // The last character may be wide: keep vi keys on its first cell.
    (r, off_padding(screen, r, x.min(last_col)))
}

/// Where back-to-indentation puts the cursor, tmux's
/// grid_reader_cursor_back_to_indentation: the first cell that holds
/// something, from the first row of a wrapped line on, or `None` (stay put)
/// on a line that is all blank.
fn indentation_pos(screen: &vt100::Screen, row: u16, rows: u16, cols: u16) -> Option<(u16, u16)> {
    let mut r = logical_line_first_row(screen, row);
    loop {
        let len = row_text_length(screen, r, cols);
        if let Some(x) = (0..len).find(|&x| !is_blank_cell(screen, r, x)) {
            return Some((r, x));
        }
        if !screen.row_wrapped(r) || r + 1 >= rows {
            return None;
        }
        r += 1;
    }
}

/// Move cursor to start of line (0 key in vi copy mode).
///
/// On a wrapped line it goes to the first row of the line, as tmux's
/// grid_reader_cursor_start_of_line with wrap does.
pub fn move_to_line_start(app: &mut AppState) {
    if let Some((r, _)) = get_copy_pos(app) {
        let win = &mut app.windows[app.active_idx];
        let first = active_pane_mut(&mut win.root, &win.active_path)
            .and_then(|p| p.term.lock().ok().map(|parser| logical_line_first_row(parser.screen(), r)))
            .unwrap_or(r);
        app.copy_pos = Some((first, 0));
    }
}

/// Move cursor to end of line ($ in vi copy mode, C-e in emacs).
pub fn move_to_line_end(app: &mut AppState) {
    let Some((r, _)) = get_copy_pos(app) else { return };
    let vi = app.mode_keys != "emacs";
    let rect = app.copy_anchor.is_some() && app.copy_selection_mode == crate::types::SelectionMode::Rect;
    let win = &mut app.windows[app.active_idx];
    let Some(p) = active_pane_mut(&mut win.root, &win.active_path) else { return };
    let (rows, cols) = (p.last_rows, p.last_cols);
    let Ok(mut parser) = p.term.lock() else { return };
    let pos = line_end_pos(&mut parser, r, rows, cols, vi, rect);
    let sb = parser.screen().scrollback();
    drop(parser);
    app.copy_scroll_offset = sb;
    app.copy_pos = Some(pos);
}

/// Move cursor to first non-blank character (^ key in vi copy mode).
pub fn move_to_first_nonblank(app: &mut AppState) {
    app.copy_search_marks = false;
    if let Some((r, _)) = get_copy_pos(app) {
        let win = &mut app.windows[app.active_idx];
        let Some(p) = active_pane_mut(&mut win.root, &win.active_path) else { return };
        let (rows, cols) = (p.last_rows, p.last_cols);
        let Ok(parser) = p.term.lock() else { return };
        let found = indentation_pos(parser.screen(), r, rows, cols);
        drop(parser);
        if let Some(pos) = found {
            app.copy_pos = Some(pos);
        }
    }
}

/// Run a word motion from the copy cursor over the pane's whole grid, then
/// park the cursor where it ended, scrolling the view only as far as it takes
/// to show that line (tmux `window_copy_acquire_cursor_up` and
/// `window_copy_acquire_cursor_down`). A motion that ends on the second cell
/// of a wide character stays there, as tmux's does, the same as cursor-left.
fn word_motion(app: &mut AppState, motion: impl FnOnce(&mut GridReader<'_, ScreenGrid<'_>>)) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    let (px, py, top, history, rows, cols) = {
        let mut grid = ScreenGrid::new(parser.screen_mut());
        let start = grid.abs_of_row(r);
        let (top, history, rows, cols) = (grid.top(), grid.history(), grid.rows(), grid.width());
        let mut gr = GridReader::new(&mut grid, c as usize, start);
        motion(&mut gr);
        (gr.cx, gr.cy, top, history, rows, cols)
    };
    let rows = rows.max(1);
    let (scrollback, row) = if py < top {
        (history - py, 0)
    } else if py >= top + rows {
        (history.saturating_sub(py + 1 - rows), rows - 1)
    } else {
        (parser.screen().scrollback(), py - top)
    };
    parser.screen_mut().set_scrollback(scrollback);
    app.copy_scroll_offset = parser.screen().scrollback();
    app.copy_pos = Some((row as u16, px.min(cols.saturating_sub(1)) as u16));
}

/// The separators and the key table a word motion runs with.
fn word_settings(app: &AppState) -> (String, bool) {
    (app.word_separators.clone(), app.mode_keys == "vi")
}

/// `next-word` (w in vi copy mode).
pub fn move_word_forward(app: &mut AppState) {
    let (seps, _) = word_settings(app);
    word_motion(app, |gr| gr.cursor_next_word(&seps));
}

/// `previous-word` (b in vi copy mode). With emacs keys it also stops at the
/// end of a line that ends in whitespace, as tmux's does.
pub fn move_word_backward(app: &mut AppState) {
    let (seps, vi) = word_settings(app);
    word_motion(app, |gr| gr.cursor_previous_word(&seps, true, !vi));
}

/// `next-word-end` (e in vi copy mode).
pub fn move_word_end(app: &mut AppState) {
    let (seps, vi) = word_settings(app);
    word_motion(app, |gr| gr.cursor_next_word_end_keys(&seps, vi));
}

/// `select-word`: the word under the cursor, or the one before it from
/// whitespace (tmux `window_copy_cmd_select_word`).
pub fn select_word(app: &mut AppState) {
    let (seps, vi) = word_settings(app);
    word_motion(app, |gr| gr.cursor_previous_word(&seps, false, !vi));
    if let Some(pos) = get_copy_pos(app) {
        app.copy_anchor = Some(pos);
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_selection_mode = crate::types::SelectionMode::Char;
    }
    word_motion(app, |gr| gr.select_word_end(&seps, vi));
}

/// The kind of each cell of screen row `r` (see `GridReader::kind_at`), for
/// the text objects that work within one row.
fn row_kinds(app: &mut AppState, r: u16, seps: &str) -> Option<Vec<u8>> {
    let win = &mut app.windows[app.active_idx];
    let p = active_pane_mut(&mut win.root, &win.active_path)?;
    let mut parser = p.term.lock().ok()?;
    let mut grid = ScreenGrid::new(parser.screen_mut());
    let y = grid.abs_of_row(r);
    let cols = grid.width();
    let mut gr = GridReader::new(&mut grid, 0, y);
    Some((0..cols).map(|x| gr.kind_at(x, y, seps)).collect())
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

/// The pane height every copy-mode page motion measures itself against.
///
/// One place for the `active_pane` lookup that the page keys, the
/// `send-keys -X` verbs and `copy-mode -u` each used to spell out with their
/// own invented fallback (10, 20 and 24 in three different files). None when
/// there is no active pane to measure, and a caller that cannot measure a pane
/// has nothing to scroll either: `scroll_pane_scrollback` returns on the same
/// lookup.
pub fn active_pane_rows(app: &AppState) -> Option<u16> {
    app.windows
        .get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .map(|p| p.last_rows)
}

/// Lines one page motion moves in a pane `height` rows tall.
///
/// tmux computes this in exactly one place, `window_copy_pageup1` and its
/// `window_copy_pagedown1` twin (window-copy.c:767-773 and :825-831 at tag
/// 3.7c, :723-729 and :781-787 at 3.6a):
///
/// ```c
/// n = 1;
/// if (screen_size_y(s) > 2) {
///         if (half_page)
///                 n = screen_size_y(s) / 2;
///         else
///                 n = screen_size_y(s) - 2;
/// }
/// ```
///
/// A full page is the height minus two lines, a half page is half the height,
/// and a pane of two rows or fewer moves a single line so the view cannot get
/// stuck or jump the whole buffer.
pub fn page_lines(height: u16, half_page: bool) -> usize {
    if height <= 2 {
        return 1;
    }
    let n = if half_page { height / 2 } else { height - 2 };
    n.max(1) as usize
}

/// The scroll offset the active pane's parser actually holds.
///
/// `app.copy_scroll_offset` mirrors it, but `sync_copy_freeze` (layout.rs)
/// rewrites that field every frame, so a page motion that straddles a frame
/// boundary has to ask the parser itself.
fn active_pane_scroll_offset(app: &AppState) -> usize {
    app.windows
        .get(app.active_idx)
        .and_then(|w| active_pane(&w.root, &w.active_path))
        .and_then(|p| p.term.lock().ok().map(|t| t.screen().scrollback()))
        .unwrap_or(app.copy_scroll_offset)
}

/// Scroll the copy-mode view one page, or one half page, like tmux's
/// `window_copy_pageup1` / `window_copy_pagedown1`.
///
/// The view moves and the cursor keeps the screen row it is on.  The cursor
/// only moves when the history end clamps the scroll: tmux then shifts the
/// cursor row by the whole page amount, clamped to the top row going up
/// (window-copy.c:775-782) and to the last row going down (:833-840), which is
/// what lets repeated page-ups reach the first line of the history.
pub fn page_scroll(app: &mut AppState, up: bool, half_page: bool) {
    let rows = match active_pane_rows(app) { Some(r) => r, None => return };
    let n = page_lines(rows, half_page);
    let before = active_pane_scroll_offset(app);
    if up {
        scroll_copy_up(app, n);
    } else {
        scroll_copy_down(app, n);
    }
    let after = app.copy_scroll_offset;
    let moved = if up { after.saturating_sub(before) } else { before.saturating_sub(after) };
    if moved >= n {
        return; // the view absorbed the whole page, so the cursor stays put
    }
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let n16 = n.min(u16::MAX as usize) as u16;
    let nr = if up {
        r.saturating_sub(n16)
    } else {
        r.saturating_add(n16).min(rows.saturating_sub(1))
    };
    app.copy_pos = Some((nr, c));
}

/// `rectangle-toggle`: flip block selection on or off, tmux's
/// `window_copy_cmd_rectangle_toggle`.
///
/// One definition for every route that reaches it, because they had drifted:
/// the `-X rectangle-toggle` verb and `v` toggled, while `C-v` on the
/// pre-server dispatcher only ever switched block selection ON, so a second
/// press could not switch it back off.
pub fn toggle_rectangle(app: &mut AppState) {
    app.copy_search_marks = false;
    app.copy_selection_mode = match app.copy_selection_mode {
        crate::types::SelectionMode::Rect => crate::types::SelectionMode::Char,
        _ => crate::types::SelectionMode::Rect,
    };
    // The shape of the selection is the client's to draw and the cursor has
    // not moved, so without this the block does not appear until some later
    // frame. tmux redraws here too (`window_copy_cmd_rectangle_toggle`).
    app.copy_needs_redraw = true;
}

/// `copy-mode -u`: enter copy mode and scroll up one page.
///
/// tmux runs the same page motion the `page-up` key does, not a full screen:
/// cmd-copy-mode.c:99-100 calls `window_copy_pageup(wp, 0)`.
///
/// Returns false when `scroll-enter-copy-mode` is off, leaving copy mode
/// untouched so the caller can forward PageUp to the pane instead (#284).
pub fn enter_copy_mode_page_up(app: &mut AppState) -> bool {
    let flags = CopyModeFlags { page_up: true, ..CopyModeFlags::default() };
    run_copy_mode_command(app, flags) != CopyModeOutcome::ForwardPageUp
}

/// Copy-mode offset after the pane's retained history shrank by
/// `filled_before - filled_after` lines: shift it by the number of lines the
/// trim removed so the view stays on the same content, exactly like tmux's
/// `window_copy_resize` (window-copy.c).  An offset that would land past the
/// oldest retained line is clamped to that line: tmux pre-clamps `oy` to the
/// history size and re-derives it after the reflow, so the view lands on the
/// top of what is retained and copy mode stays up.  It never leaves copy
/// mode on a resize, and neither does psmux.
pub fn offset_after_trim(
    offset_before: usize,
    filled_before: usize,
    filled_after: usize,
) -> usize {
    if filled_after >= filled_before {
        return offset_before; // nothing was trimmed
    }
    offset_before
        .saturating_sub(filled_before - filled_after)
        .min(filled_after)
}

/// Keep the copy-mode view anchored across a pane resize.
///
/// `copy_scroll_offset` counts lines above the live bottom, so a resize that
/// trims retained scrollback (the vt100 screen reflows history, and a frozen
/// copy-mode pane can report a much smaller history mid-resize) leaves it
/// pointing past the oldest retained line.  The pane then renders at the very
/// top, and the freeze/dump sync writes that clamped offset straight back, so
/// the user stays pinned at the top until they press Esc.
///
/// Reproduced live with a phone client whose on-screen keyboard toggles the
/// pane height: in mode=1 the offset went 3771 -> 4390 while the retained
/// history fell to 508, then settled at scroll == history_size with the view
/// stuck on line 1.
pub fn reanchor_after_resize(app: &mut AppState, offset_before: usize, filled_before: usize) {
    if !in_copy_mode(app) {
        return;
    }
    let after = {
        let win = &mut app.windows[app.active_idx];
        let p = match active_pane_mut(&mut win.root, &win.active_path) {
            Some(p) => p,
            None => return,
        };
        let mut parser = match p.term.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let filled_after = parser.screen().scrollback_filled();
        let after = offset_after_trim(offset_before, filled_before, filled_after);
        parser.screen_mut().set_scrollback(after);
        after
    };
    // tmux parity: a resize never leaves copy mode. When the old offset can
    // no longer be reached the view sits on the oldest retained line, which
    // is where tmux's window_copy_resize lands too, and the user scrolls on
    // from there.
    app.copy_scroll_offset = after;
}

pub fn scroll_to_top(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(usize::MAX);
    app.copy_scroll_offset = parser.screen().scrollback();
    drop(parser);
    // tmux window_copy_cmd_history_top puts the cursor on the first cell of
    // the oldest line (`data->cy = 0; data->cx = 0;`). Leaving the cursor on
    // its old screen row parked it a whole screen below the top of history.
    app.copy_pos = Some((0, 0));
}

pub fn scroll_to_bottom(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(0);
    app.copy_scroll_offset = 0;
}

/// Read a line number the way tmux reads it and jump there.
///
/// An argument that is not a number, or one outside the range tmux reads with
/// `strtonum(linestr, -1, INT_MAX, &errstr)`, leaves the view alone
/// (window-copy.c:4587-4589). So does an empty one (window-copy.c:2491).
pub fn run_goto_line(app: &mut AppState, arg: &str) {
    let trimmed = arg.trim();
    if trimmed.is_empty() { return; }
    match trimmed.parse::<i64>() {
        Ok(n) if (-1..=i32::MAX as i64).contains(&n) => goto_line(app, n),
        _ => {}
    }
}

/// How many columns the copy-mode line number gutter pushes the active pane's
/// content to the right, 0 when no gutter is drawn.
///
/// The client paints the gutter from the same three inputs, the option, the
/// active pane's scrollback size and its height (the `copy_ln` render config
/// in client.rs), so this is the width that is on screen, and it is what a
/// pointer column has to be brought back through
/// (`copy_line_numbers::cursor_unoffset`).
pub fn active_gutter_width(app: &AppState) -> usize {
    let Some(win) = app.windows.get(app.active_idx) else { return 0 };
    gutter_width_at(app, &win.active_path)
}

/// The gutter width the pane at `path` in the active window is painted with
/// while it is the copy-mode pane. The width is per pane, the way tmux keeps
/// it per mode (`window_copy_line_number_width` reads the pane's own history
/// and height), so a press that moves copy mode onto another pane is measured
/// over the gutter that pane is drawn with from then on.
pub fn gutter_width_at(app: &AppState, path: &[usize]) -> usize {
    let mode = crate::copy_line_numbers::CopyLnMode::parse(
        app.user_options
            .get("copy-mode-line-numbers")
            .map(|s| s.as_str())
            .unwrap_or(crate::copy_line_numbers::DEFAULT),
    );
    if !mode.is_active() {
        return 0;
    }
    let Some(win) = app.windows.get(app.active_idx) else { return 0 };
    let Some(p) = active_pane(&win.root, path) else { return 0 };
    let Ok(parser) = p.term.lock() else { return 0 };
    crate::copy_line_numbers::gutter_width(
        mode,
        parser.screen().scrollback_filled(),
        p.last_rows as usize,
    )
}

/// The scroll offset `goto-line <lineno>` asks for, given the size of the
/// history and whether line numbers are being counted absolutely.
///
/// This is `window_copy_goto_line` (window-copy.c:4579-4606) with the screen
/// writes left out so it can be tested on its own. Absolute numbering counts
/// 1-based lines of the whole grid, so line `hsize + 1` is the first row of
/// the live screen; otherwise the number is how many lines back from the live
/// bottom to park, which is what the `default` gutter prints.
pub fn goto_line_offset(lineno: i64, hsize: usize, absolute: bool) -> usize {
    let hsize = hsize as i64;
    if absolute {
        let line = lineno.clamp(1, hsize + 1);
        (hsize - (line - 1)) as usize
    } else if lineno < 0 || lineno > hsize {
        hsize as usize
    } else {
        lineno as usize
    }
}

/// Bring line `lineno` into view, as tmux `window_copy_goto_line` does.
///
/// Only the viewport moves. tmux leaves the cursor row `cy` alone here, so the
/// copy cursor stays on the screen row it was on and comes to mean whichever
/// content line is now under it, which is also why `copy_pos_scroll_offset`
/// stays `None` (see its comment in types.rs: a scroll leaves it there).
pub fn goto_line(app: &mut AppState, lineno: i64) {
    let geom = match grid_geometry(app) { Some(g) => g, None => return };
    // `copy_line_numbers::is_absolute` is the same question the gutter asks, so
    // the number typed here always means what the gutter printed.
    let absolute = crate::copy_line_numbers::is_absolute(
        crate::copy_line_numbers::CopyLnMode::parse(
            app.user_options.get("copy-mode-line-numbers").map(|s| s.as_str()).unwrap_or("off"),
        ),
    );
    let offset = goto_line_offset(lineno, geom.history, absolute);

    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return };
    let mut parser = match p.term.lock() { Ok(g) => g, Err(_) => return };
    parser.screen_mut().set_scrollback(offset);
    app.copy_scroll_offset = parser.screen().scrollback();
}

/// The text a selection copies, tmux window_copy_get_selection, without
/// storing it anywhere. `anchor` and `cursor` are (absolute line, column),
/// absolute meaning screen row minus the scrollback it was measured at;
/// rows in the scrollback are read by moving the view, which is put back
/// to `restore_sb` afterwards. `emacs` is mode-keys emacs, which leaves
/// the end cell out.
pub(crate) fn selection_text(
    parser: &mut vt100::Parser,
    rows: u16,
    cols: u16,
    anchor: (i64, u16),
    cursor: (i64, u16),
    sel_mode: crate::types::SelectionMode,
    emacs: bool,
    restore_sb: usize,
) -> String {
    let ((anchor_abs, anchor_col), (cursor_abs, cursor_col)) = (anchor, cursor);
    let sel_top_abs = anchor_abs.min(cursor_abs);
    let sel_bot_abs = anchor_abs.max(cursor_abs);
    let total_lines = (sel_bot_abs - sel_top_abs + 1) as usize;

    // For character mode: the column of the end that comes first in reading
    // order, and of the one that comes last.
    let (top_col, bot_col) = if anchor <= cursor {
        (anchor_col, cursor_col)
    } else {
        (cursor_col, anchor_col)
    };
    // A rectangle leaves the cursor column out with emacs keys only when the
    // cursor is right of the anchor; it always keeps the anchor column.
    let (rect_sx, rect_ex) = if anchor_col < cursor_col {
        (anchor_col, if emacs { cursor_col } else { cursor_col + 1 })
    } else {
        (cursor_col, anchor_col + 1)
    };
    // The last row's end, for the final newline rule.
    let mut last_end = (false, 0u16, 0u16);

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
                    // Every row is copied from rect_sx to rect_ex and ends its
                    // line; only the copy's final newline is dropped below.
                    copy_selection_row(&mut text, parser.screen(), r, rect_sx, rect_ex, cols);
                    if is_last {
                        let ey_last = row_text_length(parser.screen(), r, cols);
                        last_end = (parser.screen().row_wrapped(r), rect_ex, ey_last);
                    }
                }
                crate::types::SelectionMode::Line => {
                    let line = capture_row_text(parser.screen(), r, 0..cols);
                    text.push_str(line.trim_end());
                    text.push('\n');
                }
                crate::types::SelectionMode::Char => {
                    // tmux window_copy_get_selection: the first row from the
                    // first end, middle rows whole, the last row to its end
                    // clamped to the row's text, so blanks between words at
                    // the end of the selection stay in the copy.
                    let sx = if is_first { top_col } else { 0 };
                    let ex = if is_last {
                        let (lastex, ey_last) = selection_last_end(parser.screen(), r, bot_col, cols, emacs);
                        last_end = (parser.screen().row_wrapped(r), lastex, ey_last);
                        lastex
                    } else {
                        cols
                    };
                    copy_selection_row(&mut text, parser.screen(), r, sx, ex, cols);
                }
            }
            abs_idx += 1;
        }
        next_abs = read_end + 1;
    }
    // Restore original scrollback
    parser.screen_mut().set_scrollback(restore_sb);
    if !matches!(sel_mode, crate::types::SelectionMode::Line) {
        let (last_wrapped, lastex, ey_last) = last_end;
        trim_selection_newline(&mut text, last_wrapped, lastex, ey_last, emacs);
    }
    text
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
    //
    // Each end carries its OWN scrollback, not the current one: an edge
    // auto-scroll (`scroll_copy_up`/`scroll_copy_down`) moves the view between
    // the anchor and the endpoint, so reading the endpoint against the current
    // offset puts it on the wrong content line and the yank no longer covers
    // what the client painted.
    let anchor_abs = anchor.0 as i64 - anchor_scroll as i64;
    let cursor_abs = pos.0 as i64 - app.copy_pos_scroll_offset.unwrap_or(current_scroll) as i64;
    let emacs = app.mode_keys == "emacs";
    let text = selection_text(&mut parser, rows, cols, (anchor_abs, anchor.1), (cursor_abs, pos.1), sel_mode, emacs, current_scroll);
    drop(parser);
    // tmux copies nothing when the selection holds nothing ("Don't bother if
    // no data"): an emacs selection whose two ends are one cell is empty.
    if text.is_empty() {
        return Ok(());
    }
    // Store in named register if one was selected
    if let Some(reg) = app.copy_register.take() {
        app.named_registers.insert(reg, text.clone());
    }
    app.paste_add(text.clone());
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
    let internal = app.paste_top().unwrap_or_default().to_string();
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
    let parser = match p.live_parser().lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows {
        text.push_str(capture_row_text(screen, r, 0..p.last_cols).trim_end());
        text.push('\n');
    }
    drop(parser);
    app.paste_add(text);
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

/// The capture-pane flags that shape each line: `-N` keeps trailing spaces,
/// `-J` joins wrapped rows into one line. A bare `bool` converts to `-N`
/// alone, the shape every capture had before `-J` reached the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureOpts {
    pub preserve_trailing: bool,
    pub join: bool,
}

impl From<bool> for CaptureOpts {
    fn from(preserve_trailing: bool) -> Self {
        CaptureOpts { preserve_trailing, join: false }
    }
}

/// The columns of `row` the pane actually wrote, tmux's `cellused`: up to
/// and including the last cell with contents. A space the program printed
/// counts, a cell nothing wrote or an erase cleared does not.
fn row_used_cols(screen: &vt100::Screen, row: u16, cols: u16) -> u16 {
    (0..cols)
        .rev()
        .find(|&c| screen.cell(row, c).is_some_and(|cell| cell.has_contents() || cell.is_wide_continuation()))
        .map_or(0, |c| c + 1)
}

/// Append one grid row of a plain capture (tmux cmd-capture-pane.c
/// `cmd_capture_pane_history`). Without `-J` a row is the full width, trimmed
/// unless `-N`. With `-J` tmux drops GRID_STRING_TRIM_SPACES and
/// GRID_STRING_EMPTY_CELLS, so a row is exactly the cells it used, spaces
/// kept, and a wrapped row gets no newline so the next row continues it.
fn push_capture_line(text: &mut String, screen: &vt100::Screen, row: u16, cols: u16, opts: CaptureOpts) {
    if opts.join {
        let used = row_used_cols(screen, row, cols);
        text.push_str(&capture_row_text(screen, row, 0..used));
        if screen.row_wrapped(row) {
            return;
        }
    } else {
        let line = capture_row_text(screen, row, 0..cols);
        if opts.preserve_trailing {
            text.push_str(&line);
        } else {
            text.push_str(line.trim_end());
        }
    }
    text.push('\n');
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

pub fn capture_active_pane_text(app: &mut AppState, pane_id: Option<usize>, opts: impl Into<CaptureOpts>) -> io::Result<Option<String>> {
    capture_pane_text_from(app, pane_id, opts, false)
}

/// `saved` reads the main screen the alternate screen covers (`capture-pane
/// -a`), `Ok(None)` when the alternate screen is not in use.
pub fn capture_pane_text_from(app: &mut AppState, pane_id: Option<usize>, opts: impl Into<CaptureOpts>, saved: bool) -> io::Result<Option<String>> {
    let opts = opts.into();
    // A floating pane: named by %id, or the focused one when the request has
    // no id (a -t naming it by index runs with it focused). capture_target
    // knows the tiled tree only, so a floating %id captured the ACTIVE pane
    // (issue #793 follow up).
    let float_at = match pane_id {
        Some(pid) if crate::tree::find_pane_by_id_global(app, pid).is_none() => {
            crate::tree::find_floating_by_id(app, pid)
        }
        Some(_) => None,
        None => {
            let w = &app.windows[app.active_idx];
            w.floating_focus.filter(|fi| *fi < w.floating.len()).map(|fi| (app.active_idx, fi))
        }
    };
    let p: &mut crate::types::Pane = if let Some((wi, fi)) = float_at {
        &mut app.windows[wi].floating[fi].pane
    } else {
        let (win_idx, path) = capture_target(app, pane_id);
        let win = &mut app.windows[win_idx];
        match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) }
    };
    let guard = match p.live_parser().lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let saved_parser;
    let parser: &vt100::Parser = if saved {
        match guard.saved_normal_snapshot() { Some(snap) => { saved_parser = snap; &saved_parser } None => return Ok(None) }
    } else {
        &guard
    };
    let screen = parser.screen();
    let mut text = String::new();
    for r in 0..p.last_rows {
        push_capture_line(&mut text, screen, r, p.last_cols, opts);
    }
    // Trim trailing all-empty lines so iTerm2 doesn't advance its cursor
    // past the actual content on initial attach.
    while text.ends_with("\n\n") { text.pop(); }
    if text == "\n" { text.clear(); }
    Ok(Some(text))
}

pub fn save_latest_buffer(app: &mut AppState, file: &str) -> io::Result<()> {
    if let Some(buf) = app.paste_top() { std::fs::write(file, buf)?; }
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

/// Every (overlapping) occurrence of `query_lower` in `lower`, as
/// (col_start, col_end) pairs in DISPLAY columns, the unit of the copy
/// cursor: a wide CJK or emoji character spans two cells of the grid row the
/// line was read from, so a character count put the cursor short of every
/// hit that followed one (tmux searches grid cells, window-copy.c).
///
/// The scan resumes one CHARACTER after each hit, not one byte: a query that
/// starts with a multi byte character (日本, école) used to resume inside it
/// and the slice panicked, taking the whole server down (#712 audit).
pub(crate) fn search_line_matches(lower: &str, query_lower: &str) -> Vec<(u16, u16)> {
    use unicode_width::UnicodeWidthStr;
    let mut out = Vec::new();
    if query_lower.is_empty() { return out; }
    let qlen = UnicodeWidthStr::width(query_lower) as u16;
    let mut start = 0usize;
    while let Some(pos) = lower[start..].find(query_lower) {
        let byte_at = start + pos;
        let col_start = UnicodeWidthStr::width(&lower[..byte_at]) as u16;
        out.push((col_start, col_start + qlen));
        start = byte_at + lower[byte_at..].chars().next().map_or(1, char::len_utf8);
        if start >= lower.len() { break; }
    }
    out
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

    // Ascending absolute order first; the direction ordering is applied below.
    let mut ascending: Vec<(usize, u16, u16)> = Vec::new();
    for (abs, line) in lines.iter().enumerate() {
        for (c0, c1) in search_line_matches(&line.to_lowercase(), &query_lower) {
            ascending.push((abs, c0, c1));
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
        app.copy_search_marks = true;
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

/// `next-space` (W in vi copy mode): words split by whitespace alone.
pub fn move_word_forward_big(app: &mut AppState) {
    word_motion(app, |gr| gr.cursor_next_word(""));
}

/// `previous-space` (B in vi copy mode).
pub fn move_word_backward_big(app: &mut AppState) {
    let (_, vi) = word_settings(app);
    word_motion(app, |gr| gr.cursor_previous_word("", true, !vi));
}

/// `next-space-end` (E in vi copy mode).
pub fn move_word_end_big(app: &mut AppState) {
    let (_, vi) = word_settings(app);
    word_motion(app, |gr| gr.cursor_next_word_end_keys("", vi));
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
    app.copy_search_marks = false;
    if let Some((r, c)) = get_copy_pos(app) {
        app.copy_mark = Some((app.copy_scroll_offset, r, c));
    }
    // The mark is drawn by the client from the `copy_hl` spans in the frame,
    // and nothing in the pane or the layout changed, so the frame has to be
    // asked for. tmux asks the same way: `window_copy_cmd_set_mark` returns
    // `WINDOW_COPY_CMD_REDRAW` (window-copy.c:2368 to :2376, tag 3.7c). The
    // `-X` route never needed it, because a command request marks the state
    // dirty on its own, which is why pressing X was the only way to see this.
    app.copy_needs_redraw = true;
}

/// Swap the cursor with the mark — M-x (jump-to-mark).
///
/// tmux's window_copy_jump_to_mark() exchanges the two positions rather than
/// just moving to the mark, so pressing it twice brings you back to where you
/// jumped from.
pub fn jump_to_mark(app: &mut AppState) {
    app.copy_search_marks = false;
    let (mark_scroll, mark_row, mark_col) = match app.copy_mark { Some(m) => m, None => return };
    let here = match get_copy_pos(app) {
        Some((r, c)) => (app.copy_scroll_offset, r, c),
        None => return,
    };
    set_scroll_offset(app, mark_scroll);
    app.copy_pos = Some((mark_row, mark_col));
    app.copy_mark = Some(here);
}

/// Where the view lands after the snapshot is rebuilt with `new_filled` lines
/// of history, given the old history size and scroll offset: the same line
/// stays at the top of the screen, counted from the top of the history, as
/// tmux's `window_copy_cmd_refresh_from_pane` keeps `oy_from_top`
/// (window-copy.c at tag 3.7c). The second value is true when that line is no
/// longer retained, in which case tmux parks the view on the oldest line with
/// the cursor on the top row.
pub fn refreshed_offset(old_filled: usize, old_offset: usize, new_filled: usize) -> (usize, bool) {
    let from_top = old_filled - old_offset.min(old_filled);
    if from_top <= new_filled {
        (new_filled - from_top, false)
    } else {
        (new_filled, true)
    }
}

/// Rebuild the focused pane's copy-mode snapshot from its live screen.
///
/// With `follow` the view goes to the live bottom with the cursor on the last
/// row, which is what tmux's automatic refresh does while the cursor sits
/// there (`window_copy_do_refresh`). Without it the reader keeps their place.
/// Either way the selection is cleared, as `window_copy_size_changed` does
/// after every re-clone. Returns false when the pane has no snapshot to
/// rebuild.
fn reclone_copy_snapshot(app: &mut AppState, follow: bool) -> bool {
    let (old_filled, old_offset, new_filled, rows, cols) = {
        let win = &mut app.windows[app.active_idx];
        let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return false };
        if p.live_term.is_none() {
            return false;
        }
        let (old_filled, old_offset) = match p.term.lock() {
            Ok(t) => (t.screen().scrollback_filled(), t.screen().scrollback()),
            Err(_) => return false,
        };
        p.leave_copy_snapshot();
        p.enter_copy_snapshot();
        let new_filled = p.term.lock().map(|t| t.screen().scrollback_filled()).unwrap_or(0);
        (old_filled, old_offset, new_filled, p.last_rows, p.last_cols)
    };
    let (want, clamped) = if follow { (0, false) } else { refreshed_offset(old_filled, old_offset, new_filled) };
    let actual = {
        let win = &mut app.windows[app.active_idx];
        match active_pane_mut(&mut win.root, &win.active_path).and_then(|p| p.term.lock().ok()) {
            Some(mut t) => {
                t.screen_mut().set_scrollback(want);
                t.screen().scrollback()
            }
            None => want,
        }
    };
    app.copy_scroll_offset = actual;
    app.copy_pos_scroll_offset = None;
    let last_row = rows.saturating_sub(1);
    let (r, c) = app.copy_pos.unwrap_or((last_row, 0));
    let c = c.min(cols.saturating_sub(1));
    app.copy_pos = Some(if follow {
        (last_row, c)
    } else if clamped {
        (0, c)
    } else {
        (r.min(last_row), c)
    });
    // The mark is kept as an offset from the bottom: move it by the lines
    // that were added underneath so it stays on its own line.
    if let Some((so, mr, mc)) = app.copy_mark {
        let grown = new_filled.saturating_sub(old_filled);
        app.copy_mark = Some((so + grown, mr, mc));
    }
    clear_selection(app);
    app.copy_needs_redraw = true;
    true
}

/// `refresh-from-pane`, the `r` key: copy the pane's current screen into copy
/// mode once, keeping the line at the top of the view where it was.
///
/// tmux 3.4 to 3.7c bind `r` to this one shot command
/// (`window_copy_cmd_refresh_from_pane`, window-copy.c at tag 3.7c). It used to
/// flip the #494 freeze off here, which stopped doing anything once copy mode
/// moved onto a snapshot (d74f52a): a snapshot receives no output, so `r` left
/// the view on the old screen and only reset the scroll position (discussion
/// #694).
pub fn refresh_from_pane(app: &mut AppState) {
    reclone_copy_snapshot(app, false);
}

/// `refresh-on`: rebuild the snapshot automatically while the pane prints,
/// tmux's `window_copy_refresh_start` (the `r` binding after tmux 3.7c).
pub fn refresh_on(app: &mut AppState) {
    if !app.copy_refresh_live {
        app.copy_refresh_live = true;
        app.copy_refresh_version = 0;
    }
}

/// `refresh-off`: stop the automatic refresh.
pub fn refresh_off(app: &mut AppState) {
    app.copy_refresh_live = false;
}

/// `refresh-toggle`.
pub fn toggle_refresh(app: &mut AppState) {
    if app.copy_refresh_live { refresh_off(app) } else { refresh_on(app) }
}

/// The automatic refresh, run for every frame the server builds: when the
/// pane printed since the last rebuild, rebuild the snapshot, following the
/// output only while the cursor is on the last row at the live bottom, and
/// never while a selection is being made (tmux `window_copy_refresh_timer`).
/// Returns true when the view changed.
pub fn tick_auto_refresh(app: &mut AppState) -> bool {
    if !app.copy_refresh_live || !matches!(app.mode, Mode::CopyMode) || app.copy_anchor.is_some() {
        return false;
    }
    let (version, rows) = {
        let win = match app.windows.get(app.active_idx) { Some(w) => w, None => return false };
        match active_pane(&win.root, &win.active_path) {
            Some(p) => (p.data_version.load(std::sync::atomic::Ordering::Relaxed), p.last_rows),
            None => return false,
        }
    };
    // tmux polls on a 50 ms timer; psmux runs this once per frame, and frames
    // are already coalesced, so a rebuild is due whenever the pane printed.
    if version == app.copy_refresh_version {
        return false;
    }
    let follow = app.copy_scroll_offset == 0
        && app.copy_pos.map_or(true, |(r, _)| r + 1 >= rows);
    let changed = reclone_copy_snapshot(app, follow);
    app.copy_refresh_version = version;
    changed
}

/// What a key did to the input of a copy-mode prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEdit {
    /// The key edited the input or moved the cursor.
    Edited,
    /// Up or C-p: the caller recalls an older entry from the history.
    HistoryUp,
    /// Down or C-n: a newer entry.
    HistoryDown,
    /// Not a line editing key.
    Other,
}

/// Apply one line editing key to a copy-mode prompt's `input`, whose cursor
/// sits `back` characters before the end. The keys are the subset of tmux's
/// prompt editor (prompt.c `prompt_key`, the `process_key` switch) that has a
/// meaning here: Left, Right, Home, End and their emacs spellings C-b, C-f,
/// C-a, C-e, Backspace and Delete at the cursor, C-u clears, C-k cuts to the
/// end, C-w cuts the word before the cursor, M-f and C-Right move to the end
/// of the next word, M-b and C-Left to the start of the word before, C-t
/// transposes the characters at the cursor, C-y inserts what C-w last cut or
/// else the top paste buffer, and Space inserts a space, which arrives as a
/// key of its own and used to be dropped (discussion #694).
///
/// `separators` is the `word-separators` option, which C-w and the word keys
/// split words with, as tmux's prompt does. `saved` is what C-w last cut at
/// this prompt, tmux's `prompt_saved`. The caller keeps it for as long as the
/// prompt is open and clears it when a prompt opens. `top_buffer` gives the
/// top paste buffer, and is only called for a C-y with nothing saved, because
/// the client's prompts have to ask the server for it.
pub fn prompt_edit(
    input: &mut String,
    back: &mut usize,
    saved: &mut Option<String>,
    separators: &str,
    key: &str,
    top_buffer: impl FnOnce() -> Option<String>,
) -> PromptEdit {
    let mut chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut b = (*back).min(len);
    let idx = len - b;
    let result = match key.to_ascii_lowercase().as_str() {
        "space" => { chars.insert(idx, ' '); PromptEdit::Edited }
        "left" | "c-b" => { if b < len { b += 1; } PromptEdit::Edited }
        "right" | "c-f" => { b = b.saturating_sub(1); PromptEdit::Edited }
        "home" | "c-a" => { b = len; PromptEdit::Edited }
        "end" | "c-e" => { b = 0; PromptEdit::Edited }
        "backspace" | "bspace" | "c-h" => {
            if idx > 0 { chars.remove(idx - 1); }
            PromptEdit::Edited
        }
        "delete" | "dc" | "c-d" => {
            if b > 0 { chars.remove(idx); b -= 1; }
            PromptEdit::Edited
        }
        "c-u" => { chars.clear(); b = 0; PromptEdit::Edited }
        "c-k" => { chars.truncate(idx); b = 0; PromptEdit::Edited }
        // tmux cuts back to where M-b would go (status.c:1558 to :1597).
        "c-w" => {
            let start = prompt_backward_word(&chars, idx, separators);
            // tmux keeps the cut for C-y, even an empty one, which then makes
            // C-y insert nothing rather than the paste buffer
            // (status.c:1583 to :1587).
            *saved = Some(chars.drain(start..idx).collect());
            PromptEdit::Edited
        }
        "m-f" | "c-right" => {
            b = len - prompt_forward_word(&chars, idx, separators);
            PromptEdit::Edited
        }
        "m-b" | "c-left" => {
            b = len - prompt_backward_word(&chars, idx, separators);
            PromptEdit::Edited
        }
        // tmux inserts what C-w last cut, and only when nothing was cut the
        // top paste buffer, up to its first control character. The cursor
        // ends after the inserted text (status_prompt_paste, status.c:1186 to
        // :1245, called from :1645).
        "c-y" => {
            let text = match saved {
                Some(cut) => cut.clone(),
                None => top_buffer().map(|t| prompt_paste_line(&t).to_string()).unwrap_or_default(),
            };
            chars.splice(idx..idx, text.chars());
            PromptEdit::Edited
        }
        // tmux steps the cursor right first unless it is at the end, so inside
        // the line the characters either side of the cursor change places and
        // at the end the last two do. The cursor ends after the pair. At the
        // start, or with fewer than two characters, nothing changes
        // (status.c:1649 to :1661).
        "c-t" => {
            let at = if idx < len { idx + 1 } else { idx };
            if at >= 2 {
                chars.swap(at - 2, at - 1);
                b = len - at;
            }
            PromptEdit::Edited
        }
        "up" | "c-p" => PromptEdit::HistoryUp,
        "down" | "c-n" => PromptEdit::HistoryDown,
        _ => PromptEdit::Other,
    };
    *input = chars.into_iter().collect();
    *back = b;
    result
}

/// Whether `c` is a prompt word separator. tmux tests only a one byte, one
/// cell character against the list (status_prompt_in_list, status.c:974), so
/// a wide character or a control character is never one.
fn prompt_in_list(separators: &str, c: char) -> bool {
    (' '..='~').contains(&c) && separators.contains(c)
}

/// Whether `c` is a space to tmux's prompt: only ' ' (status_prompt_space,
/// status.c:983).
fn prompt_space(c: char) -> bool {
    c == ' '
}

/// Where M-f takes the cursor from character index `idx`: over any spaces,
/// then to the end of the word, a run of separators or of other characters
/// (status_prompt_forward_word with emacs keys, status.c:1320 to :1358).
fn prompt_forward_word(chars: &[char], idx: usize, separators: &str) -> usize {
    let size = chars.len();
    let mut idx = idx.min(size);
    while idx != size && prompt_space(chars[idx]) {
        idx += 1;
    }
    if idx == size {
        return idx;
    }
    let word_is_separators = prompt_in_list(separators, chars[idx]) && !prompt_space(chars[idx]);
    loop {
        idx += 1;
        if idx == size || prompt_space(chars[idx]) || word_is_separators != prompt_in_list(separators, chars[idx]) {
            return idx;
        }
    }
}

/// Where M-b, and the start of what C-w cuts, is from character index `idx`:
/// back over any spaces, then to the start of the word
/// (status_prompt_backward_word, status.c:1399 to :1425).
fn prompt_backward_word(chars: &[char], idx: usize, separators: &str) -> usize {
    let mut idx = idx.min(chars.len());
    while idx != 0 {
        idx -= 1;
        if !prompt_space(chars[idx]) {
            break;
        }
    }
    let Some(&first) = chars.get(idx) else { return idx };
    let word_is_separators = prompt_in_list(separators, first);
    while idx != 0 {
        idx -= 1;
        if prompt_space(chars[idx]) || word_is_separators != prompt_in_list(separators, chars[idx]) {
            return idx + 1;
        }
    }
    idx
}

/// The part of a paste buffer that C-y puts into a prompt: everything before
/// the first control character or DEL, so only the first line of a buffer
/// that has several. tmux stops at the same bytes (status.c:1215).
pub fn prompt_paste_line(text: &str) -> &str {
    let end = text
        .char_indices()
        .find(|&(_, c)| (c as u32) < 0x20 || c == '\u{7f}')
        .map_or(text.len(), |(i, _)| i);
    &text[..end]
}

/// Insert typed text at a copy-mode prompt's cursor.
pub fn prompt_insert(input: &mut String, back: usize, text: &str) {
    let len = input.chars().count();
    let idx = len - back.min(len);
    let at = input.char_indices().nth(idx).map_or(input.len(), |(i, _)| i);
    input.insert_str(at, text);
}

/// Step through the search history the way tmux's prompt does: Up goes to
/// older entries and stops at the oldest, Down goes back towards the newest
/// and then to an empty line. Returns the text to show, or None when there is
/// nothing to move to.
pub fn search_history_step(history: &[String], pos: &mut Option<usize>, up: bool) -> Option<String> {
    if history.is_empty() {
        return None;
    }
    if up {
        let next = match *pos { None => history.len() - 1, Some(p) => p.saturating_sub(1) };
        *pos = Some(next);
        Some(history[next].clone())
    } else {
        match *pos {
            None => None,
            Some(p) if p + 1 < history.len() => {
                *pos = Some(p + 1);
                Some(history[p + 1].clone())
            }
            Some(_) => {
                *pos = None;
                Some(String::new())
            }
        }
    }
}

/// Remember an accepted search for the prompt history, skipping a repeat of
/// the newest entry, as tmux's `prompt_add_history` does.
pub fn remember_search(app: &mut AppState, query: &str) {
    if query.is_empty() || app.copy_search_history.last().map(|s| s.as_str()) == Some(query) {
        return;
    }
    app.copy_search_history.push(query.to_string());
    if app.copy_search_history.len() > 100 {
        app.copy_search_history.remove(0);
    }
}

/// `begin-selection`, the Space key: start a selection at the copy cursor.
///
/// tmux's `window_copy_start_selection` leaves `rectflag` alone, so a
/// rectangle toggled on with `v` before the selection starts is the shape the
/// selection takes. A line selection (`V`) is a different command and does
/// not carry over.
pub fn begin_selection(app: &mut AppState) {
    app.copy_search_marks = false;
    if let Some((r, c)) = get_copy_pos(app) {
        app.copy_anchor = Some((r, c));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, c));
        if app.copy_selection_mode == crate::types::SelectionMode::Line {
            app.copy_selection_mode = crate::types::SelectionMode::Char;
        }
    }
}

/// `select-line`, the `V` key: select the cursor's line.
pub fn select_line(app: &mut AppState) {
    app.copy_search_marks = false;
    if let Some((r, c)) = get_copy_pos(app) {
        app.copy_anchor = Some((r, c));
        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
        app.copy_pos = Some((r, c));
        app.copy_selection_mode = crate::types::SelectionMode::Line;
    }
    // Pressing V right after Space moves neither the cursor nor the anchor,
    // so only the selection mode changes and there is nothing for the frame
    // to notice. tmux's `window_copy_cmd_select_line` returns REDRAW.
    app.copy_needs_redraw = true;
}

/// Highlight kinds in [`copy_highlights`]: a search match, the match under
/// the cursor, the marked line, and the marked cell itself.
pub const HL_MATCH: u16 = 0;
pub const HL_CURRENT_MATCH: u16 = 1;
pub const HL_MARK_LINE: u16 = 2;
pub const HL_MARK_CELL: u16 = 3;

/// The highlighted spans on the focused pane's copy-mode screen, as
/// `[row, first column, last column, kind]` in screen cells, for the client to
/// paint with `copy-mode-match-style`, `copy-mode-current-match-style` and
/// `copy-mode-mark-style`.
///
/// This is what tmux's `window_copy_update_style` (window-copy.c) does per
/// cell: the marked line takes the mark style with the marked cell's colours
/// swapped, every search match on screen takes the match style, and the match
/// the cursor is on takes the current match style. Matches are only drawn
/// while `copy_search_marks` says the last search still owns the screen
/// (discussion #694).
pub fn copy_highlights(app: &AppState) -> Vec<[u16; 4]> {
    let mut out = Vec::new();
    let win = match app.windows.get(app.active_idx) { Some(w) => w, None => return out };
    let p = match active_pane(&win.root, &win.active_path) { Some(p) => p, None => return out };
    let (rows, cols) = (p.last_rows, p.last_cols);
    if rows == 0 || cols == 0 {
        return out;
    }
    let hist = p.term.lock().map(|t| t.screen().scrollback_filled()).unwrap_or(0);
    let so = app.copy_scroll_offset.min(hist);
    if let Some((mark_so, mr, mc)) = app.copy_mark {
        let row = mr as i64 + so as i64 - mark_so as i64;
        if row >= 0 && row < rows as i64 {
            out.push([row as u16, 0, cols - 1, HL_MARK_LINE]);
            out.push([row as u16, mc.min(cols - 1), mc.min(cols - 1), HL_MARK_CELL]);
        }
    }
    if app.copy_search_marks {
        let top = hist - so;
        let (cr, cc) = app.copy_pos.unwrap_or((u16::MAX, 0));
        for &(abs, c0, c1) in &app.copy_search_matches {
            if abs < top || abs >= top + rows as usize || c1 <= c0 || c0 >= cols {
                continue;
            }
            let row = (abs - top) as u16;
            let last = (c1 - 1).min(cols - 1);
            let kind = if row == cr && cc >= c0 && cc <= last { HL_CURRENT_MATCH } else { HL_MATCH };
            out.push([row, c0, last, kind]);
        }
    }
    out
}

/// The Escape key in copy mode. tmux binds it to `clear-selection` in
/// `copy-mode-vi` and to `cancel` in `copy-mode` (key-bindings.c:654 and
/// :577), so with `mode-keys vi` it drops the selection and stays in copy
/// mode, with nothing to do when there is no selection (measured in tmux 3.4:
/// `pane_in_mode` stays 1), and `q` is the key that leaves (discussion #694).
pub fn escape_key(app: &mut AppState) {
    if app.mode_keys == "vi" {
        clear_selection(app);
    } else {
        exit_copy_mode(app);
    }
}

/// `clear-selection`, tmux's `window_copy_clear_selection`: drop the selection
/// and stay in copy mode. A line selection goes back to a character one, while
/// the rectangle flag survives, as tmux keeps `rectflag` across it.
pub fn clear_selection(app: &mut AppState) {
    app.copy_search_marks = false;
    app.copy_anchor = None;
    if app.copy_selection_mode == crate::types::SelectionMode::Line {
        app.copy_selection_mode = crate::types::SelectionMode::Char;
    }
    // Dropping the selection and the search highlights takes colour OFF the
    // screen, which needs a frame as much as putting it on: without this the
    // matches from the last search stay lit until something else redraws.
    // tmux's `window_copy_cmd_clear_selection` returns REDRAW.
    app.copy_needs_redraw = true;
}

/// Hide or show the copy-mode position indicator, the `P` key and the
/// `toggle-position` command (#704).
///
/// tmux keeps this on the mode entry and flips it in
/// `window_copy_cmd_toggle_position` (`window-copy.c`), which is why it is
/// saved with the rest of the pane-local copy state rather than kept as a
/// client setting: two panes in copy mode answer independently, and leaving
/// the mode forgets it.
pub fn toggle_position(app: &mut AppState) {
    app.copy_hide_position = !app.copy_hide_position;
    // Nothing in the pane or the layout changes, so the frame has to be asked
    // for: measured on a key press without this, the indicator took 2.6s and
    // 3.8s to go, and once did not go at all inside ten seconds, because it
    // waited for an unrelated frame. The `-X` route never had the problem
    // because a command request marks the state dirty on its own.
    app.copy_needs_redraw = true;
    save_copy_state_to_pane(app);
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
    drop(parser);
    app.paste_add(text.clone());
    copy_to_system_clipboard(&text);
    Ok(())
}

/// Yank the cursor's whole line (tmux copy-line): the cursor does not move.
pub fn copy_line(app: &mut AppState) -> io::Result<()> {
    let (r, _) = match get_copy_pos(app) { Some(p) => p, None => return Ok(()) };
    let win = &mut app.windows[app.active_idx];
    let p = match active_pane_mut(&mut win.root, &win.active_path) { Some(p) => p, None => return Ok(()) };
    let parser = match p.term.lock() { Ok(g) => g, Err(_) => return Ok(()) };
    let screen = parser.screen();
    let cols = p.last_cols;
    let text = capture_row_text(screen, r, 0..cols).trim_end().to_string();
    drop(parser);
    app.paste_add(text.clone());
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

pub fn capture_active_pane_range(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, opts: impl Into<CaptureOpts>) -> io::Result<Option<String>> {
    capture_pane_range_from(app, s, e, pane_id, opts, false)
}

/// `saved`: see [`capture_pane_text_from`].
pub fn capture_pane_range_from(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, opts: impl Into<CaptureOpts>, saved: bool) -> io::Result<Option<String>> {
    let opts = opts.into();
    let (win_idx, path) = capture_target(app, pane_id);
    let win = &mut app.windows[win_idx];
    let p = match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) };
    let mut guard = match p.live_parser().lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let mut saved_parser;
    let parser: &mut vt100::Parser = if saved {
        match guard.saved_normal_snapshot() { Some(snap) => { saved_parser = snap; &mut saved_parser } None => return Ok(None) }
    } else {
        &mut guard
    };
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
            push_capture_line(&mut text, screen, r, cols, opts);
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
            push_capture_line(&mut text, parser.screen(), r, cols, opts);
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
///
/// Issue #685: the pane's OSC 4 palette is deliberately NOT applied here.
/// tmux's `capture-pane -e` goes through `grid_string_cells` (`grid.c`), which
/// reads `gc->fg` / `gc->bg` straight out of the cell; only the tty path
/// (`tty_check_fg` and friends) substitutes the palette.  So a capture reports
/// the indexed colour the pane actually wrote, and a caller that wants the
/// resolved RGB reads the rendered frame instead.
pub fn capture_active_pane_styled(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, opts: impl Into<CaptureOpts>) -> io::Result<Option<String>> {
    capture_pane_styled_from(app, s, e, pane_id, opts, false)
}

/// `saved`: see [`capture_pane_text_from`].
pub fn capture_pane_styled_from(app: &mut AppState, s: Option<i32>, e: Option<i32>, pane_id: Option<usize>, opts: impl Into<CaptureOpts>, saved: bool) -> io::Result<Option<String>> {
    let CaptureOpts { preserve_trailing, join } = opts.into();
    let (win_idx, path) = capture_target(app, pane_id);
    let win = &mut app.windows[win_idx];
    let p = match active_pane_mut(&mut win.root, &path) { Some(p) => p, None => return Ok(None) };
    let mut guard = match p.live_parser().lock() { Ok(g) => g, Err(_) => return Ok(None) };
    let mut saved_parser;
    let parser: &mut vt100::Parser = if saved {
        match guard.saved_normal_snapshot() { Some(snap) => { saved_parser = snap; &mut saved_parser } None => return Ok(None) }
    } else {
        &mut guard
    };
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

    // A -J row that ran on into the next one with a style still set.
    let mut style_carried = false;

    // Helper closure: render one screen row with SGR tracking
    let mut render_styled_row = |screen: &vt100::Screen, r: u16, text: &mut String| {
        let mut row_chars: Vec<String> = Vec::new();
        let mut row_sgr: Vec<Option<String>> = Vec::new();
        let mut any_style_active = false;
        // -J: the row ends after the last cell the pane wrote (tmux cellused).
        let mut used_chars = 0usize;
        for c in 0..cols {
            if let Some(cell) = screen.cell(r, c) {
                if cell.has_contents() { used_chars = row_chars.len() + 1; }
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
        //
        // -J: the cells the row used, spaces kept, and a wrapped row runs on
        // into the next one with no reset or newline between them, so the
        // style carries across the join as it does in tmux.
        let trim_end = if join {
            used_chars
        } else if preserve_trailing {
            row_chars.len()
        } else {
            match row_chars.iter().rposition(|s| !s.is_empty() && s.trim() != "") { Some(pos) => pos + 1, None => 0 }
        };
        for c in 0..trim_end {
            if let Some(ref sgr) = row_sgr[c] { text.push_str(sgr); }
            text.push_str(&row_chars[c]);
        }
        if join && screen.row_wrapped(r) {
            style_carried |= any_style_active;
            return;
        }
        let carried = std::mem::take(&mut style_carried);
        if any_style_active || carried {
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
    app.copy_search_marks = false;
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
    app.copy_search_marks = false;
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

/// Select the run of cells of one kind around the copy cursor on its row
/// (the vi text objects `iw`, `aw`, `iW` and `aW`, which tmux does not
/// have), adding the whitespace after it when `trailing` is set. The kinds
/// are the ones tmux's word motions use (`GridReader::kind_at`), so a word
/// here is the word `w` and `b` move by; with no separators, words are split
/// by whitespace alone.
fn select_word_object(app: &mut AppState, seps: &str, trailing: bool) {
    let (r, c) = match get_copy_pos(app) { Some(p) => p, None => return };
    let kinds = match row_kinds(app, r, seps) { Some(k) => k, None => return };
    let col = c as usize;
    if col >= kinds.len() { return; }
    let kind = kinds[col];
    let mut start = col;
    while start > 0 && kinds[start - 1] == kind { start -= 1; }
    let mut end = col;
    while end + 1 < kinds.len() && kinds[end + 1] == kind { end += 1; }
    if trailing {
        while end + 1 < kinds.len() && kinds[end + 1] == 0 { end += 1; }
    }
    app.copy_anchor = Some((r, start as u16));
    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
    app.copy_pos = Some((r, end as u16));
    app.copy_selection_mode = crate::types::SelectionMode::Char;
}

/// Select "inner word" (iw): the word under the cursor.
pub fn select_inner_word(app: &mut AppState) {
    let seps = app.word_separators.clone();
    select_word_object(app, &seps, false);
}

/// Select "a word" (aw): the word under the cursor and the whitespace after it.
pub fn select_a_word(app: &mut AppState) {
    let seps = app.word_separators.clone();
    select_word_object(app, &seps, true);
}

/// Select "inner WORD" (iW): the whitespace-delimited token under the cursor.
pub fn select_inner_word_big(app: &mut AppState) {
    select_word_object(app, "", false);
}

/// Select "a WORD" (aW): the token under the cursor and the whitespace after it.
pub fn select_a_word_big(app: &mut AppState) {
    select_word_object(app, "", true);
}

#[cfg(test)]
#[path = "../tests-rs/test_issue443_blank_cell_capture.rs"]
mod tests_issue443_blank_cell_capture;

#[cfg(test)]
#[path = "../tests-rs/test_capture_pane_fidelity.rs"]
mod tests_capture_pane_fidelity;

#[cfg(test)]
#[path = "../tests-rs/test_copy_mode_resize_reanchor.rs"]
mod test_copy_mode_resize_reanchor;

#[cfg(test)]
#[path = "../tests-rs/test_copy_mode_snapshot.rs"]
mod test_copy_mode_snapshot;

#[cfg(test)]
#[path = "../tests-rs/test_copy_cancel_stale_state.rs"]
mod tests_copy_cancel_stale_state;

#[cfg(test)]
#[path = "../tests-rs/test_issue612_copy_search_scrollback.rs"]
mod tests_issue612_copy_search_scrollback;

#[cfg(test)]
#[path = "../tests-rs/test_issue673_copy_snapshot_per_pane.rs"]
mod test_issue673_copy_snapshot_per_pane;

#[cfg(test)]
#[path = "../tests-rs/test_issue687_copy_mode_keyboard_selection.rs"]
mod test_issue687_copy_mode_keyboard_selection;

#[cfg(test)]
#[path = "../tests-rs/test_issue704_toggle_position.rs"]
mod test_issue704_toggle_position;

#[cfg(test)]
#[path = "../tests-rs/test_issue712_copy_search_utf8.rs"]
mod tests_issue712_copy_search_utf8;

#[cfg(test)]
#[path = "../tests-rs/test_disc694_copy_mode_vi_parity.rs"]
mod test_disc694_copy_mode_vi_parity;


#[cfg(test)]
#[path = "../tests-rs/test_issue746_status_prompt_cursor.rs"]
mod test_issue746_status_prompt_cursor;

#[cfg(test)]
#[path = "../tests-rs/test_issue753_copy_redraw.rs"]
mod test_issue753_copy_redraw;

#[cfg(test)]
#[path = "../tests-rs/test_issue785_prompt_transpose.rs"]
mod test_issue785_prompt_transpose;

#[cfg(test)]
#[path = "../tests-rs/test_issue815_prompt_paste.rs"]
mod test_issue815_prompt_paste;

#[cfg(test)]
#[path = "../tests-rs/test_issue789_copy_prompt_editor.rs"]
mod test_issue789_copy_prompt_editor;

#[cfg(test)]
#[path = "../tests-rs/test_issue789_followup_keys_quotes.rs"]
mod test_issue789_followup_keys_quotes;
#[cfg(test)]
#[path = "../tests-rs/test_issue810_saved_screen.rs"]
mod tests_issue810_saved_screen;

#[cfg(test)]
#[path = "../tests-rs/test_issue855_copy_cursor_wide.rs"]
mod test_issue855_copy_cursor_wide;

#[cfg(test)]
#[path = "../tests-rs/test_capture_pane_join.rs"]
mod test_capture_pane_join;

#[cfg(test)]
#[path = "../tests-rs/test_copy_selection_parity.rs"]
mod test_copy_selection_parity;

#[cfg(test)]
#[path = "../tests-rs/test_copy_mode_end_of_line.rs"]
mod test_copy_mode_end_of_line;

#[cfg(test)]
#[path = "../tests-rs/test_issue866_prompt_word_keys.rs"]
mod test_issue866_prompt_word_keys;

#[cfg(test)]
#[path = "../tests-rs/test_issue862_copy_word_motion.rs"]
mod test_issue862_copy_word_motion;
