use std::io::{self, Write};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::native_pty_system;
use ratatui::prelude::*;

use crate::types::{AppState, Mode, FocusDir, LayoutKind, Node, Pane};
use crate::tree::{active_pane, active_pane_mut, compute_rects, path_exists};
use crate::pane::{create_window, split_active};
use crate::commands::{execute_action, execute_command_prompt, execute_command_string};
use crate::config::normalize_key_for_binding;
use crate::copy_mode::{enter_copy_mode, exit_copy_mode, switch_with_copy_save, move_copy_cursor,
    scroll_copy_up, scroll_copy_down, paste_latest, yank_selection,
    search_copy_mode, search_next, search_prev, scroll_to_top, scroll_to_bottom};
use crate::layout::{cycle_top_layout, apply_layout};
use crate::window_ops::{toggle_zoom, swap_pane, break_pane_to_window};

/// Refresh the status-bar prompt shown while the user is typing in copy-mode
/// search (#335). Without this the screen looks frozen because the search
/// input is otherwise invisible.
fn refresh_search_prompt(app: &mut AppState) {
    if let Mode::CopySearch { ref input, forward } = app.mode {
        let arrow = if forward { "down" } else { "up" };
        let prompt = format!("(search {}) {}", arrow, input);
        // display-time = 0 keeps the message sticky until cleared.
        app.status_message = Some((prompt, Instant::now(), Some(0)));
    }
}

/// Look up a copy-mode key binding for `key`, honouring `mode-keys`.
///
/// The table name selection matches tmux: `mode-keys vi` uses `copy-mode-vi`,
/// anything else uses `copy-mode`.
pub fn copy_mode_binding(app: &AppState, key: (KeyCode, KeyModifiers)) -> Option<crate::types::Action> {
    let table_name = if app.mode_keys == "vi" { "copy-mode-vi" } else { "copy-mode" };
    let key_tuple = normalize_key_for_binding(key);
    app.key_tables
        .get(table_name)?
        .iter()
        .find(|b| b.key == key_tuple)
        .map(|b| b.action.clone())
}

/// Run a resolved copy-mode binding.
///
/// Returns `true` when the binding was handled and the caller must NOT fall
/// through to the built-in copy-mode key handling.
///
/// `send-keys -X <cmd>` is by far the most common action in these tables (it is
/// how tmux-yank and every copy-mode rebind is written), and its implementation
/// lives in the server loop's own `CtrlReq::SendKeysX` arm. Since this function
/// is itself called from the loop, it queues the request rather than trying to
/// re-enter that arm — see `AppState::control_tx`. Everything else goes through
/// the normal action executor.
pub fn run_copy_mode_binding(app: &mut AppState, action: &crate::types::Action) -> bool {
    use crate::types::Action;
    if let Action::Command(cmd) = action {
        let parts = crate::commands::parse_command_line(cmd);
        if matches!(parts.first().map(|s| s.as_str()), Some("send-keys") | Some("send"))
            && parts.iter().any(|p| p == "-X")
        {
            // Mirror the TCP dispatcher's `has_x` arm exactly: tokens with
            // their quote grouping already stripped, flags dropped, joined
            // with spaces. The `SendKeysX` arm hands everything after the
            // copy-mode command name to `pwsh -Command` verbatim, so a quote
            // character that survives to this point turns the pipe command
            // into a string literal pwsh evaluates and discards.
            let mut rest: Vec<&str> = Vec::new();
            let mut skip_operand = false;
            for p in parts.iter().skip(1) {
                if skip_operand { skip_operand = false; continue; }
                match p.as_str() {
                    "-t" | "-N" => { skip_operand = true; }
                    s if s.starts_with('-') => {}
                    s => rest.push(s),
                }
            }
            if !rest.is_empty() {
                if let Some(tx) = app.control_tx.as_ref() {
                    let _ = tx.send(crate::types::CtrlReq::SendKeysX(rest.join(" ")));
                    return true;
                }
            }
            // No sender (or nothing after -X): fall through to the built-ins
            // rather than silently swallowing the key.
            return false;
        }
    }
    crate::commands::execute_action(app, action).is_ok()
}

/// **This function has no production callers.** Unit tests still invoke it
/// directly to exercise legacy prompt and key semantics.
///
/// It is the input dispatcher from the pre-server, single-process architecture.
/// The live path is client -> TCP -> `CtrlReq::SendText` / `SendKey` ->
/// `send_text_to_active` / `send_key_to_active`; none of it comes through here.
///
/// Key-table lookups and `switch-client -T <table>` state below are test-only
/// unless the same behavior exists in the live handlers.
///
pub fn handle_key(app: &mut AppState, key: KeyEvent) -> io::Result<bool> {
    // Fold NUL onto C-Space up front, for the same reason as the client input
    // loop: the raw `is_prefix` tuple comparisons below must see C-Space when
    // the terminal delivered a NUL (issue #504).
    let key = {
        let folded = crate::config::fold_nul_to_ctrl_space((key.code, key.modifiers));
        KeyEvent { code: folded.0, modifiers: folded.1, ..key }
    };
    match app.mode {
        Mode::Passthrough => {
            // Check switch-client -T key table first
            if let Some(table_name) = app.current_key_table.take() {
                let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
                if let Some(bind) = app.key_tables.get(&table_name)
                    .and_then(|t| t.iter().find(|b| b.key == key_tuple))
                    .cloned()
                {
                    return execute_action(app, &bind.action);
                }
                // Key not found in table — fall through to normal dispatch
            }
            let is_prefix = (key.code, key.modifiers) == app.prefix_key
                || matches!(key.code, KeyCode::Char(c) if c == '\u{0002}')
                || app.prefix2_key.map_or(false, |p2| (key.code, key.modifiers) == p2);
            if is_prefix {
                app.mode = Mode::Prefix { armed_at: Instant::now() };
                app.prefix_repeating = false;
                return Ok(false);
            }
            // Check root key table for bindings (bind-key -n / bind-key -T root)
            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
            if let Some(bind) = app.key_tables.get("root").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                // Skip scroll-triggered copy mode entry when the option is
                // off so the key (PageUp) reaches the PTY instead (#284).
                let is_scroll_copy = matches!(&bind.action, crate::types::Action::Command(cmd) if cmd.starts_with("copy-mode") && cmd.contains("-u"));
                if is_scroll_copy && !app.scroll_enter_copy_mode {
                    forward_key_to_active(app, key)?;
                    return Ok(false);
                }
                return execute_action(app, &bind.action);
            }
            forward_key_to_active(app, key)?;
            Ok(false)
        }
        Mode::Prefix { armed_at } => {
            let elapsed = armed_at.elapsed().as_millis() as u64;

            // If we're in repeat mode and the repeat window has expired,
            // exit prefix and forward the key to the active pane (tmux parity).
            if app.prefix_repeating && elapsed >= app.repeat_time_ms {
                app.mode = Mode::Passthrough;
                app.prefix_repeating = false;
                forward_key_to_active(app, key)?;
                return Ok(false);
            }
            
            let key_tuple = normalize_key_for_binding((key.code, key.modifiers));
            if let Some(bind) = app.key_tables.get("prefix").and_then(|t| t.iter().find(|b| b.key == key_tuple)).cloned() {
                if bind.repeat {
                    // Stay in prefix mode for repeat-time window
                    app.mode = Mode::Prefix { armed_at: Instant::now() };
                    app.prefix_repeating = true;
                } else {
                    app.mode = Mode::Passthrough;
                    app.prefix_repeating = false;
                }
                return execute_action(app, &bind.action);
            }
            
            let handled = match key.code {
                // Alt+Arrow: resize pane by 5 (must be before plain arrows)
                KeyCode::Up if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_vertical(app, -5); true
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_vertical(app, 5); true
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_horizontal(app, -5); true
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                    crate::window_ops::resize_pane_horizontal(app, 5); true
                }
                // Ctrl+Arrow: resize pane by 1 (must be before plain arrows)
                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_vertical(app, -1); true
                }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_vertical(app, 1); true
                }
                KeyCode::Left if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_horizontal(app, -1); true
                }
                KeyCode::Right if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    crate::window_ops::resize_pane_horizontal(app, 1); true
                }
                KeyCode::Left => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Left)); true }
                KeyCode::Right => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Right)); true }
                KeyCode::Up => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Up)); true }
                KeyCode::Down => { switch_with_copy_save(app, |app| move_focus(app, FocusDir::Down)); true }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let idx = d.to_digit(10).unwrap() as usize;
                    if let Some(internal_idx) = app.win_pos(idx) {
                        switch_with_copy_save(app, |app| {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = internal_idx;
                        });
                    }
                    true
                }
                KeyCode::Char('c') => {
                    let pty_system = native_pty_system();
                    create_window(&*pty_system, app, None, None, false)?;
                    true
                }
                KeyCode::Char('n') => {
                    if !app.windows.is_empty() {
                        switch_with_copy_save(app, |app| {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = (app.active_idx + 1) % app.windows.len();
                        });
                    }
                    true
                }
                KeyCode::Char('p') => {
                    if !app.windows.is_empty() {
                        switch_with_copy_save(app, |app| {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                        });
                    }
                    true
                }
                KeyCode::Char('%') => {
                    split_active(app, LayoutKind::Horizontal)?;
                    true
                }
                KeyCode::Char('"') => {
                    split_active(app, LayoutKind::Vertical)?;
                    true
                }
                KeyCode::Char('x') => {
                    app.mode = Mode::ConfirmMode {
                        prompt: "kill-pane? (y/n)".into(),
                        command: "kill-pane".into(),
                        input: String::new(),
                    };
                    true
                }
                KeyCode::Char('d') => {
                    return Ok(true);
                }
                KeyCode::Char('w') => {
                    let tree = crate::commands::build_choose_tree(app);
                    let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
                    app.mode = Mode::WindowChooser { selected, tree };
                    true
                }
                KeyCode::Char(',') => { app.mode = Mode::RenamePrompt { input: String::new() }; true }
                KeyCode::Char('\'') => { app.mode = Mode::WindowIndexPrompt { input: String::new() }; true }
                KeyCode::Char(' ') => { cycle_top_layout(app); true }
                KeyCode::Char('[') => { enter_copy_mode(app); true }
                KeyCode::Char(']') => { paste_latest(app)?; app.mode = Mode::Passthrough; true }
                KeyCode::Char(':') => {
                    app.command_vi_normal = false;
                    app.mode = Mode::CommandPrompt { input: String::new(), cursor: 0 };
                    true
                }
                KeyCode::Char('q') => {
                    let win = &app.windows[app.active_idx];
                    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
                    compute_rects(&win.root, app.last_window_area, &mut rects);
                    app.display_map.clear();
                    for (i, (path, _)) in rects.into_iter().enumerate() {
                        if i >= 10 { break; }
                        let digit = (i + app.pane_base_index) % 10;
                        app.display_map.push((digit, path));
                    }
                    app.mode = Mode::PaneChooser { opened_at: Instant::now() };
                    true
                }
                // --- zoom pane (z) ---
                KeyCode::Char('z') => { toggle_zoom(app); true }
                // --- next pane (o) ---
                KeyCode::Char('o') => {
                    switch_with_copy_save(app, |app| {
                        let win = &app.windows[app.active_idx];
                        let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
                        compute_rects(&win.root, app.last_window_area, &mut rects);
                        if let Some(cur) = rects.iter().position(|r| r.0 == win.active_path) {
                            let next = (cur + 1) % rects.len();
                            let new_path = rects[next].0.clone();
                            let win = &mut app.windows[app.active_idx];
                            app.last_pane_path = win.active_path.clone();
                            win.active_path = new_path;
                            // Update MRU
                            if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                                crate::tree::touch_mru(&mut win.pane_mru, pid);
                            }
                        }
                    });
                    true
                }
                // --- last pane (;) ---
                KeyCode::Char(';') => {
                    switch_with_copy_save(app, |app| {
                        let win = &mut app.windows[app.active_idx];
                        if !app.last_pane_path.is_empty() && path_exists(&win.root, &app.last_pane_path) {
                            let tmp = win.active_path.clone();
                            win.active_path = app.last_pane_path.clone();
                            app.last_pane_path = tmp;
                            // Update MRU
                            if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                                crate::tree::touch_mru(&mut win.pane_mru, pid);
                            }
                        }
                    });
                    true
                }
                // --- last window (l) ---
                KeyCode::Char('l') => {
                    if app.last_window_idx < app.windows.len() {
                        switch_with_copy_save(app, |app| {
                            let tmp = app.active_idx;
                            app.active_idx = app.last_window_idx;
                            app.last_window_idx = tmp;
                        });
                    }
                    true
                }
                // --- swap pane up/left ({) ---
                KeyCode::Char('{') => { swap_pane(app, FocusDir::Up); true }
                // --- swap pane down/right (}) ---
                KeyCode::Char('}') => { swap_pane(app, FocusDir::Down); true }
                // --- break pane to new window (!) ---
                KeyCode::Char('!') => { break_pane_to_window(app); true }
                // --- kill window (&) with confirmation ---
                KeyCode::Char('&') => {
                    app.mode = Mode::ConfirmMode {
                        prompt: "kill-window? (y/n)".into(),
                        command: "kill-window".into(),
                        input: String::new(),
                    };
                    true
                }
                // --- rename session ($) ---
                KeyCode::Char('$') => {
                    app.mode = Mode::RenameSessionPrompt { input: String::new() };
                    true
                }
                // --- Meta+1..5 preset layouts (like tmux) ---
                KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "even-horizontal"); true
                }
                KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "even-vertical"); true
                }
                KeyCode::Char('3') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "main-horizontal"); true
                }
                KeyCode::Char('4') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "main-vertical"); true
                }
                KeyCode::Char('5') if key.modifiers.contains(KeyModifiers::ALT) => {
                    apply_layout(app, "tiled"); true
                }
                // --- display pane info (i) ---
                KeyCode::Char('i') => {
                    // Display window/pane info in status bar (tmux prefix+i)
                    let win = &app.windows[app.active_idx];
                    let pane_count = crate::tree::count_panes(&win.root);
                    app.status_right = format!(
                        "#{} ({}) [{}x{}] panes:{}", 
                        app.active_idx, win.name,
                        app.last_window_area.width, app.last_window_area.height,
                        pane_count
                    );
                    true
                }
                // --- clock mode (t) ---
                KeyCode::Char('t') => {
                    app.mode = Mode::ClockMode;
                    true
                }
                // --- buffer chooser (=) ---
                KeyCode::Char('=') => {
                    app.mode = Mode::BufferChooser { selected: 0 };
                    true
                }
                _ => false,
            };

            if matches!(app.mode, Mode::Prefix { .. }) {
                // Arrow keys are repeatable by default (tmux binds them with -r)
                let is_repeatable = matches!(key.code,
                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                );
                if handled && is_repeatable {
                    // Stay in prefix mode for repeat-time window
                    app.mode = Mode::Prefix { armed_at: Instant::now() };
                    app.prefix_repeating = true;
                } else if !handled && elapsed < app.escape_time_ms {
                    return Ok(false);
                } else {
                    app.mode = Mode::Passthrough;
                    app.prefix_repeating = false;
                }
            }
            Ok(false)
        }
        Mode::CommandPrompt { .. } => {
            let vi_mode = app.user_options.get("status-keys").map(|v| v.as_str()) == Some("vi");

            // Vi normal mode handling
            if vi_mode && app.command_vi_normal {
                match key.code {
                    KeyCode::Esc => { app.command_vi_normal = false; app.mode = Mode::Passthrough; }
                    KeyCode::Enter => {
                        if let Mode::CommandPrompt { input, .. } = &app.mode {
                            if !input.is_empty() {
                                let cmd = input.clone();
                                app.command_history.push(cmd);
                                if app.command_history.len() > 100 { app.command_history.remove(0); }
                                app.command_history_idx = app.command_history.len();
                            }
                        }
                        app.command_vi_normal = false;
                        execute_command_prompt(app)?;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                            if *cursor > 0 { *cursor -= 1; }
                        }
                    }
                    KeyCode::Char('l') | KeyCode::Right => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            if *cursor < input.len() { *cursor += 1; }
                        }
                    }
                    KeyCode::Char('0') | KeyCode::Home => {
                        if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                            *cursor = 0;
                        }
                    }
                    KeyCode::Char('$') | KeyCode::End => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *cursor = input.len();
                        }
                    }
                    KeyCode::Char('b') => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            let mut pos = *cursor;
                            while pos > 0 && input.as_bytes().get(pos - 1) == Some(&b' ') { pos -= 1; }
                            while pos > 0 && input.as_bytes().get(pos - 1) != Some(&b' ') { pos -= 1; }
                            *cursor = pos;
                        }
                    }
                    KeyCode::Char('w') => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            let len = input.len();
                            let mut pos = *cursor;
                            while pos < len && input.as_bytes().get(pos) != Some(&b' ') { pos += 1; }
                            while pos < len && input.as_bytes().get(pos) == Some(&b' ') { pos += 1; }
                            *cursor = pos;
                        }
                    }
                    KeyCode::Char('x') | KeyCode::Delete => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            if *cursor < input.len() { input.remove(*cursor); }
                        }
                    }
                    KeyCode::Char('i') => { app.command_vi_normal = false; }
                    KeyCode::Char('a') => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            if *cursor < input.len() { *cursor += 1; }
                        }
                        app.command_vi_normal = false;
                    }
                    KeyCode::Char('A') => {
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *cursor = input.len();
                        }
                        app.command_vi_normal = false;
                    }
                    KeyCode::Char('I') => {
                        if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                            *cursor = 0;
                        }
                        app.command_vi_normal = false;
                    }
                    KeyCode::Up => {
                        if app.command_history_idx > 0 {
                            app.command_history_idx -= 1;
                            let cmd = app.command_history[app.command_history_idx].clone();
                            let len = cmd.len();
                            if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                                *input = cmd;
                                *cursor = len;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if app.command_history_idx < app.command_history.len() {
                            app.command_history_idx += 1;
                            let cmd = if app.command_history_idx < app.command_history.len() {
                                app.command_history[app.command_history_idx].clone()
                            } else {
                                String::new()
                            };
                            let len = cmd.len();
                            if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                                *input = cmd;
                                *cursor = len;
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }

            // Emacs mode / vi insert mode
            match key.code {
                KeyCode::Esc => {
                    if vi_mode {
                        app.command_vi_normal = true;
                    } else {
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Enter => {
                    // Save to history before executing
                    if let Mode::CommandPrompt { input, .. } = &app.mode {
                        if !input.is_empty() {
                            let cmd = input.clone();
                            app.command_history.push(cmd);
                            if app.command_history.len() > 100 { app.command_history.remove(0); }
                            app.command_history_idx = app.command_history.len();
                        }
                    }
                    execute_command_prompt(app)?;
                }
                KeyCode::Backspace => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor > 0 {
                            input.remove(*cursor - 1);
                            *cursor -= 1;
                        }
                    }
                }
                KeyCode::Delete => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor < input.len() {
                            input.remove(*cursor);
                        }
                    }
                }
                KeyCode::Left => {
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        if *cursor > 0 { *cursor -= 1; }
                    }
                }
                KeyCode::Right => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        if *cursor < input.len() { *cursor += 1; }
                    }
                }
                KeyCode::Home => {
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        *cursor = 0;
                    }
                }
                KeyCode::End => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        *cursor = input.len();
                    }
                }
                KeyCode::Up => {
                    // Cycle through command history (older)
                    if app.command_history_idx > 0 {
                        app.command_history_idx -= 1;
                        let cmd = app.command_history[app.command_history_idx].clone();
                        let len = cmd.len();
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *input = cmd;
                            *cursor = len;
                        }
                    }
                }
                KeyCode::Down => {
                    // Cycle through command history (newer)
                    if app.command_history_idx < app.command_history.len() {
                        app.command_history_idx += 1;
                        let cmd = if app.command_history_idx < app.command_history.len() {
                            app.command_history[app.command_history_idx].clone()
                        } else {
                            String::new()
                        };
                        let len = cmd.len();
                        if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                            *input = cmd;
                            *cursor = len;
                        }
                    }
                }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+A: move to beginning
                    if let Mode::CommandPrompt { cursor, .. } = &mut app.mode {
                        *cursor = 0;
                    }
                }
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+E: move to end
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        *cursor = input.len();
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+U: kill line (clear from cursor to start)
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.drain(..*cursor);
                        *cursor = 0;
                    }
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+K: kill to end of line
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.truncate(*cursor);
                    }
                }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Ctrl+W: delete word backwards
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        let mut pos = *cursor;
                        while pos > 0 && input.as_bytes().get(pos - 1) == Some(&b' ') { pos -= 1; }
                        while pos > 0 && input.as_bytes().get(pos - 1) != Some(&b' ') { pos -= 1; }
                        input.drain(pos..*cursor);
                        *cursor = pos;
                    }
                }
                KeyCode::Char(c) => {
                    if let Mode::CommandPrompt { input, cursor } = &mut app.mode {
                        input.insert(*cursor, c);
                        *cursor += 1;
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::WindowChooser { selected, ref tree } => {
            let tree_len = tree.len();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 { if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s -= 1; } }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected + 1 < tree_len { if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s += 1; } }
                }
                KeyCode::Enter => {
                    if let Mode::WindowChooser { selected: s, ref tree } = &app.mode {
                        let entry = &tree[*s];
                        if entry.is_current_session {
                            // Same session: switch window directly. window_index is a
                            // DISPLAY index (honors gaps); map it to a Vec position.
                            if let Some(wi) = entry.window_index {
                                if let Some(pos) = app.win_pos(wi) {
                                    app.last_window_idx = app.active_idx;
                                    app.active_idx = pos;
                                }
                            }
                        } else {
                            // Different session: set env and trigger switch
                            std::env::set_var("PSMUX_SWITCH_TO", &entry.session_name);
                        }
                    }
                    app.mode = Mode::Passthrough;
                }
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    // Quick-select by window number
                    let n = c.to_digit(10).unwrap_or(0) as usize;
                    if let Some(idx) = tree.iter().position(|e| !e.is_session_header && e.window_index == Some(n) && e.is_current_session) {
                        if let Mode::WindowChooser { selected: s, .. } = &mut app.mode { *s = idx; }
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::WindowIndexPrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => {
                    if let Mode::WindowIndexPrompt { input } = &app.mode {
                        if let Ok(idx) = input.parse::<usize>() {
                            if let Some(internal_idx) = app.win_pos(idx) {
                                switch_with_copy_save(app, |app| {
                                    app.last_window_idx = app.active_idx;
                                    app.active_idx = internal_idx;
                                });
                            }
                        }
                    }
                    app.mode = Mode::Passthrough;
                }
                KeyCode::Backspace => { if let Mode::WindowIndexPrompt { input } = &mut app.mode { let _ = input.pop(); } }
                KeyCode::Char(c) if c.is_ascii_digit() => { if let Mode::WindowIndexPrompt { input } = &mut app.mode { input.push(c); } }
                _ => {}
            }
            Ok(false)
        }
        Mode::RenamePrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => {
                    if let Mode::RenamePrompt { input } = &mut app.mode {
                        let name = input.clone();
                        app.mode = Mode::Passthrough;
                        // Update local state with bounds check
                        if app.active_idx < app.windows.len() {
                            app.windows[app.active_idx].name = name.clone();
                            app.windows[app.active_idx].manual_rename = true;
                        }
                        // Forward to server so external queries see the new name
                        if let Some(port) = app.control_port {
                            let _ = crate::session::send_control_to_port(port, &format!("rename-window {}\n", crate::util::quote_arg(&name)), &app.session_key);
                        }
                    }
                }
                KeyCode::Backspace => { if let Mode::RenamePrompt { input } = &mut app.mode { let _ = input.pop(); } }
                KeyCode::Char(c) => { if let Mode::RenamePrompt { input } = &mut app.mode { input.push(c); } }
                _ => {}
            }
            Ok(false)
        }
        Mode::RenameSessionPrompt { .. } => {
            match key.code {
                KeyCode::Esc => { app.mode = Mode::Passthrough; }
                KeyCode::Enter => {
                    if let Mode::RenameSessionPrompt { input } = &mut app.mode {
                        let name = input.clone();
                        app.mode = Mode::Passthrough;
                        // Update local state
                        app.session_name = name.clone();
                        // Forward to server so external queries see the new name
                        if let Some(port) = app.control_port {
                            let _ = crate::session::send_control_to_port(port, &format!("rename-session {}\n", crate::util::quote_arg(&name)), &app.session_key);
                        }
                    }
                }
                KeyCode::Backspace => { if let Mode::RenameSessionPrompt { input } = &mut app.mode { let _ = input.pop(); } }
                KeyCode::Char(c) => { if let Mode::RenameSessionPrompt { input } = &mut app.mode { input.push(c); } }
                _ => {}
            }
            Ok(false)
        }
        Mode::CopyMode => {
            // Check copy-mode key table for user bindings first (used by plugins like tmux-yank).
            // Shares its lookup with the LIVE dispatch path (send_key_to_active
            // / handle_copy_mode_char) rather than keeping a second copy — the
            // second copy is what let the live path go years without consulting
            // these tables at all.
            if let Some(action) = copy_mode_binding(app, (key.code, key.modifiers)) {
                if run_copy_mode_binding(app, &action) {
                    return Ok(false);
                }
            }
            // Handle register pending state (waiting for a-z after ")
            if app.copy_register_pending {
                app.copy_register_pending = false;
                if let KeyCode::Char(ch) = key.code {
                    if ch.is_ascii_lowercase() {
                        app.copy_register = Some(ch);
                    }
                }
                return Ok(false);
            }
            // Handle text-object pending state (waiting for w/W after a/i)
            if let Some(prefix) = app.copy_text_object_pending.take() {
                if let KeyCode::Char(ch) = key.code {
                    match (prefix, ch) {
                        (0, 'w') => { crate::copy_mode::select_a_word(app); }
                        (1, 'w') => { crate::copy_mode::select_inner_word(app); }
                        (0, 'W') => { crate::copy_mode::select_a_word_big(app); }
                        (1, 'W') => { crate::copy_mode::select_inner_word_big(app); }
                        _ => {}
                    }
                }
                return Ok(false);
            }
            // Handle find-char pending state (waiting for char after f/F/t/T)
            if let Some(pending) = app.copy_find_char_pending.take() {
                let n = app.copy_count.take().unwrap_or(1);
                if let KeyCode::Char(ch) = key.code {
                    match pending {
                        0 => { for _ in 0..n { crate::copy_mode::find_char_forward(app, ch); } }
                        1 => { for _ in 0..n { crate::copy_mode::find_char_backward(app, ch); } }
                        2 => { for _ in 0..n { crate::copy_mode::find_char_to_forward(app, ch); } }
                        3 => { for _ in 0..n { crate::copy_mode::find_char_to_backward(app, ch); } }
                        _ => {}
                    }
                }
                return Ok(false);
            }
            // Handle numeric prefix accumulation for copy-mode motions (vi-style)
            if let KeyCode::Char(d) = key.code {
                if d.is_ascii_digit() && !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) {
                    let digit = d.to_digit(10).unwrap() as usize;
                    if let Some(count) = app.copy_count {
                        // Accumulate: multiply by 10 and add digit (cap at 9999)
                        app.copy_count = Some((count * 10 + digit).min(9999));
                        return Ok(false);
                    } else if digit >= 1 {
                        // Start new count with 1-9
                        app.copy_count = Some(digit);
                        return Ok(false);
                    }
                    // digit == 0 with no existing count → fall through to line-start handler
                }
            }
            let copy_repeat = app.copy_count.take().unwrap_or(1);
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char(']') => { 
                    exit_copy_mode(app);
                }
                // Ctrl+C exits copy mode (tmux parity, fixes #25)
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    exit_copy_mode(app);
                }
                // Ctrl+Up / Ctrl+Down scroll the viewport by one line and leave the
                // cursor where it is (tmux key-bindings.c:635/:636 for copy-mode and
                // :727/:728 for copy-mode-vi). These must come BEFORE the plain
                // Up/Down arms or the modifier would be ignored (#596).
                KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => { for _ in 0..copy_repeat { scroll_copy_up(app, 1); } }
                KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => { for _ in 0..copy_repeat { scroll_copy_down(app, 1); } }
                KeyCode::Left | KeyCode::Char('h') => { for _ in 0..copy_repeat { move_copy_cursor(app, -1, 0); } }
                KeyCode::Right | KeyCode::Char('l') => { for _ in 0..copy_repeat { move_copy_cursor(app, 1, 0); } }
                KeyCode::Up | KeyCode::Char('k') => { for _ in 0..copy_repeat { move_copy_cursor(app, 0, -1); } }
                KeyCode::Down | KeyCode::Char('j') => { for _ in 0..copy_repeat { move_copy_cursor(app, 0, 1); } }
                // Page scroll: C-b / PageUp = page up, C-f / PageDown = page down
                KeyCode::PageUp => { scroll_copy_up(app, 10); }
                KeyCode::PageDown => { scroll_copy_down(app, 10); }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys == "emacs" { move_copy_cursor(app, -1, 0); }
                    else { scroll_copy_up(app, 10); }
                }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys == "emacs" { move_copy_cursor(app, 1, 0); }
                    else { scroll_copy_down(app, 10); }
                }
                // Half-page scroll: C-u / C-d
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| active_pane(&w.root, &w.active_path))
                        .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                    scroll_copy_up(app, half);
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| active_pane(&w.root, &w.active_path))
                        .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                    scroll_copy_down(app, half);
                }
                // Emacs copy-mode keys (must be before unqualified char matches).
                // tmux binds C-p/C-n in the copy-mode table to cursor-up/cursor-down,
                // NOT to scrolling (key-bindings.c:571 and :572). The viewport only
                // moves when the cursor is already at the top or bottom edge, which
                // is what move_copy_cursor already does. This arm used to call
                // scroll_copy_up/down here while the live named-key path called
                // move_copy_cursor, so the two dispatchers disagreed (#596).
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => { move_copy_cursor(app, 0, 1); }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => { move_copy_cursor(app, 0, -1); }
                KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => { crate::copy_mode::move_to_line_start(app); }
                // C-e and C-y are the one place the two tmux tables really disagree:
                // copy-mode binds C-e to end-of-line and leaves C-y unbound, while
                // copy-mode-vi binds C-e to scroll-down and C-y to scroll-up
                // (key-bindings.c:645 and :653). vi mode follows the vi table; `$`
                // is the vi way to reach the end of the line (#596).
                KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys == "emacs" { crate::copy_mode::move_to_line_end(app); }
                    else { for _ in 0..copy_repeat { scroll_copy_down(app, 1); } }
                }
                KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if app.mode_keys != "emacs" { for _ in 0..copy_repeat { scroll_copy_up(app, 1); } }
                }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => { scroll_copy_up(app, 10); }
                KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_forward(app); }
                KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::move_word_backward(app); }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::ALT) => { yank_selection(app)?; exit_copy_mode(app); }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                    refresh_search_prompt(app);
                }
                KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                    refresh_search_prompt(app);
                }
                KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    exit_copy_mode(app);
                }
                KeyCode::Char('g') => { scroll_to_top(app); }
                KeyCode::Char('G') => { scroll_to_bottom(app); }
                // Word motions: w = next word, b = prev word, e = end of word
                KeyCode::Char('w') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_forward(app); } }
                KeyCode::Char('b') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_backward(app); } }
                KeyCode::Char('e') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_end(app); } }
                // WORD motions: W = next WORD, B = prev WORD, E = end WORD
                KeyCode::Char('W') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_forward_big(app); } }
                KeyCode::Char('B') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_backward_big(app); } }
                KeyCode::Char('E') => { for _ in 0..copy_repeat { crate::copy_mode::move_word_end_big(app); } }
                // Shift+J / Shift+K scroll one line, cursor untouched. tmux binds
                // these only in copy-mode-vi (key-bindings.c:680 and :681), so they
                // stay vi only here too (#596).
                KeyCode::Char('J') if app.mode_keys != "emacs" => { for _ in 0..copy_repeat { scroll_copy_down(app, 1); } }
                KeyCode::Char('K') if app.mode_keys != "emacs" => { for _ in 0..copy_repeat { scroll_copy_up(app, 1); } }
                // Screen position: H = top, M = middle, L = bottom
                KeyCode::Char('H') => { crate::copy_mode::move_to_screen_top(app); }
                KeyCode::Char('M') => { crate::copy_mode::move_to_screen_middle(app); }
                KeyCode::Char('L') => { crate::copy_mode::move_to_screen_bottom(app); }
                // Find char: f/F/t/T — sets pending state for next char
                KeyCode::Char('f') => { app.copy_find_char_pending = Some(0); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('F') => { app.copy_find_char_pending = Some(1); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('t') => { app.copy_find_char_pending = Some(2); app.copy_count = Some(copy_repeat); }
                KeyCode::Char('T') => { app.copy_find_char_pending = Some(3); app.copy_count = Some(copy_repeat); }
                // D = copy from cursor to end of line
                KeyCode::Char('D') => { crate::copy_mode::copy_end_of_line(app)?; exit_copy_mode(app); }
                // Bracket matching: % = jump to matching bracket/paren/brace
                KeyCode::Char('%') => { crate::copy_mode::move_matching_bracket(app); }
                // Paragraph jump: { = previous paragraph, } = next paragraph
                KeyCode::Char('{') => { for _ in 0..copy_repeat { crate::copy_mode::move_prev_paragraph(app); } }
                KeyCode::Char('}') => { for _ in 0..copy_repeat { crate::copy_mode::move_next_paragraph(app); } }
                // Centre the cursor line in the pane: z = scroll-middle
                KeyCode::Char('z') => { crate::copy_mode::scroll_middle(app); }
                // Mark, jump repeat and live-refresh toggle (#498).
                // M-x must stay above the bare Char('x') matches, like the
                // other ALT-qualified arms in this table.
                KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::ALT) => { crate::copy_mode::jump_to_mark(app); }
                KeyCode::Char('X') => { crate::copy_mode::set_mark(app); }
                KeyCode::Char(';') => { for _ in 0..copy_repeat { crate::copy_mode::jump_again(app); } }
                KeyCode::Char(',') => { for _ in 0..copy_repeat { crate::copy_mode::jump_reverse(app); } }
                KeyCode::Char('r') => { crate::copy_mode::toggle_refresh(app); }
                // Line motions: 0 = start, $ = end, ^ = first non-blank
                KeyCode::Char('0') => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::Char('$') => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('^') => { crate::copy_mode::move_to_first_nonblank(app); }
                KeyCode::Home => { crate::copy_mode::move_to_line_start(app); }
                KeyCode::End => { crate::copy_mode::move_to_line_end(app); }
                KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // vi: toggle rectangle selection, emacs: page down
                    if app.mode_keys == "emacs" {
                        scroll_copy_down(app, 10);
                    } else {
                        app.copy_selection_mode = crate::types::SelectionMode::Rect;
                    }
                }
                KeyCode::Char('v') => {
                    // tmux parity #62: rectangle-toggle (not begin-selection)
                    app.copy_selection_mode = match app.copy_selection_mode {
                        crate::types::SelectionMode::Rect => crate::types::SelectionMode::Char,
                        _ => crate::types::SelectionMode::Rect,
                    };
                }
                KeyCode::Char('V') => {
                    // Start line-wise selection (vi visual-line mode)
                    if let Some((r,c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r,c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r,c));
                        app.copy_selection_mode = crate::types::SelectionMode::Line;
                    }
                }
                KeyCode::Char('o') => {
                    // Swap cursor and anchor
                    if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                        app.copy_anchor = Some(p);
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some(a);
                    }
                }
                KeyCode::Char('A') => {
                    // Append to buffer (yank + append to buffer 0)
                    if let (Some(_), Some(_)) = (app.copy_anchor, app.copy_pos) {
                        // Save current buffer 0
                        let prev = app.paste_buffers.first().cloned().unwrap_or_default();
                        yank_selection(app)?;
                        // buffer 0 is now the new yank; prepend old text
                        if let Some(buf) = app.paste_buffers.first_mut() {
                            let new_text = buf.clone();
                            *buf = format!("{}{}", prev, new_text);
                        }
                        exit_copy_mode(app);
                    }
                }
                // Space = begin selection (vi mode), Enter = copy-selection-and-cancel
                KeyCode::Char(' ') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some((r,c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r,c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r,c));
                        app.copy_selection_mode = crate::types::SelectionMode::Char;
                    }
                }
                KeyCode::Enter => {
                    // Copy selection and exit copy mode (vi Enter)
                    if app.copy_anchor.is_some() {
                        yank_selection(app)?;
                    }
                    exit_copy_mode(app);
                }
                KeyCode::Char('y') => { yank_selection(app)?; exit_copy_mode(app); }
                // --- copy-mode search ---
                KeyCode::Char('/') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: true };
                    refresh_search_prompt(app);
                }
                KeyCode::Char('?') => {
                    app.mode = Mode::CopySearch { input: String::new(), forward: false };
                    refresh_search_prompt(app);
                }
                KeyCode::Char('n') => { search_next(app); }
                KeyCode::Char('N') => { search_prev(app); }
                KeyCode::Char(' ') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Set mark (anchor)
                    if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                        app.copy_anchor = Some((r, c));
                        app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                        app.copy_pos = Some((r, c));
                    }
                }
                // Named register prefix: " then a-z
                KeyCode::Char('"') => { app.copy_register_pending = true; }
                // Text-object prefixes: a/i then w/W
                KeyCode::Char('a') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    app.copy_text_object_pending = Some(0);
                }
                KeyCode::Char('i') if !key.modifiers.contains(KeyModifiers::CONTROL) && !key.modifiers.contains(KeyModifiers::ALT) => {
                    app.copy_text_object_pending = Some(1);
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::CopySearch { .. } => {
            match key.code {
                KeyCode::Esc => {
                    // Cancel search, return to copy mode
                    app.mode = Mode::CopyMode;
                    app.status_message = None;
                }
                KeyCode::Enter => {
                    // Execute search
                    if let Mode::CopySearch { ref input, forward } = app.mode {
                        let query = input.clone();
                        let fwd = forward;
                        app.copy_search_query = query.clone();
                        app.copy_search_forward = fwd;
                        search_copy_mode(app, &query, fwd);
                        // Jump to first match
                        if !app.copy_search_matches.is_empty() {
                            let (r, c, _) = app.copy_search_matches[0];
                            app.copy_pos = Some((r, c));
                        }
                    }
                    app.mode = Mode::CopyMode;
                    app.status_message = None;
                }
                KeyCode::Backspace => {
                    if let Mode::CopySearch { ref mut input, .. } = app.mode { let _ = input.pop(); }
                    refresh_search_prompt(app);
                }
                KeyCode::Char(c) => {
                    if let Mode::CopySearch { ref mut input, .. } = app.mode { input.push(c); }
                    refresh_search_prompt(app);
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::PaneChooser { .. } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Char(d) if d.is_ascii_digit() => {
                    let choice = d.to_digit(10).unwrap() as usize;
                    if let Some((_, path)) = app.display_map.iter().find(|(n, _)| *n == choice) {
                        let win = &mut app.windows[app.active_idx];
                        win.active_path = path.clone();
                    }
                    app.mode = Mode::Passthrough;
                }
                _ => { app.mode = Mode::Passthrough; }
            }
            Ok(false)
        }
        Mode::MenuMode { ref mut menu } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { 
                    app.mode = Mode::Passthrough; 
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if menu.selected > 0 {
                        menu.selected -= 1;
                        while menu.selected > 0 && menu.items.get(menu.selected).map(|i| i.is_separator).unwrap_or(false) {
                            menu.selected -= 1;
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if menu.selected + 1 < menu.items.len() {
                        menu.selected += 1;
                        while menu.selected + 1 < menu.items.len() && menu.items.get(menu.selected).map(|i| i.is_separator).unwrap_or(false) {
                            menu.selected += 1;
                        }
                    }
                }
                KeyCode::Enter => {
                    if let Some(item) = menu.items.get(menu.selected) {
                        if !item.is_separator && !item.command.is_empty() {
                            let cmd = item.command.clone();
                            app.mode = Mode::Passthrough;
                            let _ = execute_command_string(app, &cmd);
                        } else {
                            app.mode = Mode::Passthrough;
                        }
                    } else {
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Char(c) => {
                    if let Some((_idx, item)) = menu.items.iter().enumerate().find(|(_, i)| i.key == Some(c)) {
                        if !item.is_separator && !item.command.is_empty() {
                            let cmd = item.command.clone();
                            app.mode = Mode::Passthrough;
                            let _ = execute_command_string(app, &cmd);
                        } else {
                            app.mode = Mode::Passthrough;
                        }
                    }
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::PopupMode { ref mut output, ref mut process, close_on_exit, ref mut popup_pane, ref mut scroll_offset, .. } => {
            let mut should_close = false;
            let mut exit_status: Option<std::process::ExitStatus> = None;
            
            // If we have a PTY popup, forward keys to it
            if let Some(ref mut pty) = popup_pane {
                match key.code {
                    KeyCode::Esc => {
                        // Check if the child has exited
                        if let Ok(Some(_)) = pty.child.try_wait() {
                            should_close = true;
                        } else {
                            // Forward Escape to the PTY
                            let _ = pty.writer.write_all(b"\x1b");
                        }
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                            let ctrl = (c as u8) & 0x1F;
                            let _ = pty.writer.write_all(&[ctrl]);
                        } else {
                            let mut buf = [0u8; 4];
                            let s = c.encode_utf8(&mut buf);
                            let _ = pty.writer.write_all(s.as_bytes());
                        }
                    }
                    KeyCode::Enter => { let _ = pty.writer.write_all(b"\r"); }
                    KeyCode::Backspace => { let _ = pty.writer.write_all(b"\x7f"); }
                    KeyCode::Tab => { let _ = pty.writer.write_all(b"\t"); }
                    KeyCode::BackTab => { let _ = pty.writer.write_all(b"\x1b[Z"); }
                    KeyCode::Up => { let _ = pty.writer.write_all(b"\x1b[A"); }
                    KeyCode::Down => { let _ = pty.writer.write_all(b"\x1b[B"); }
                    KeyCode::Right => { let _ = pty.writer.write_all(b"\x1b[C"); }
                    KeyCode::Left => { let _ = pty.writer.write_all(b"\x1b[D"); }
                    KeyCode::Home => { let _ = pty.writer.write_all(b"\x1b[H"); }
                    KeyCode::End => { let _ = pty.writer.write_all(b"\x1b[F"); }
                    KeyCode::PageUp => { let _ = pty.writer.write_all(b"\x1b[5~"); }
                    KeyCode::PageDown => { let _ = pty.writer.write_all(b"\x1b[6~"); }
                    KeyCode::Delete => { let _ = pty.writer.write_all(b"\x1b[3~"); }
                    _ => {}
                }
                // Check if child exited
                if let Ok(Some(_status)) = pty.child.try_wait() {
                    if close_on_exit {
                        should_close = true;
                    }
                }
            } else {
                // Non-PTY popup (static output)
                let total_lines = output.lines().count() as u16;
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        if let Some(ref mut proc) = process {
                            let _ = proc.kill();
                        }
                        should_close = true;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        *scroll_offset = scroll_offset.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if *scroll_offset < total_lines.saturating_sub(1) {
                            *scroll_offset += 1;
                        }
                    }
                    KeyCode::PageUp => {
                        *scroll_offset = scroll_offset.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        *scroll_offset = (*scroll_offset + 10).min(total_lines.saturating_sub(1));
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        *scroll_offset = 0;
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        *scroll_offset = total_lines.saturating_sub(1);
                    }
                    _ => {}
                }
                
                if let Some(ref mut proc) = process {
                    if let Ok(Some(status)) = proc.try_wait() {
                        exit_status = Some(status);
                        if close_on_exit {
                            should_close = true;
                        }
                    }
                }
                
                if let Some(status) = exit_status {
                    if !close_on_exit {
                        output.push_str(&format!("\n[Process exited with status: {}]", status));
                    }
                }
            }
            
            if should_close {
                app.mode = Mode::Passthrough;
            }
            
            Ok(false)
        }
        Mode::ConfirmMode { prompt: _, ref command, ref mut input } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.mode = Mode::Passthrough;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    let cmd = command.clone();
                    app.mode = Mode::Passthrough;
                    let _ = execute_command_string(app, &cmd);
                }
                KeyCode::Char(c) => {
                    input.push(c);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                _ => {}
            }
            Ok(false)
        }
        Mode::ClockMode => {
            // Any key exits clock mode
            app.mode = Mode::Passthrough;
            Ok(false)
        }
        Mode::CustomizeMode { ref options, selected: _, ref filter, editing, .. } => {
            if editing {
                match key.code {
                    KeyCode::Esc => {
                        if let Mode::CustomizeMode { editing: ref mut e, edit_buffer: ref mut eb, .. } = app.mode {
                            *e = false;
                            *eb = String::new();
                        }
                    }
                    KeyCode::Enter => {
                        if let Mode::CustomizeMode { ref mut editing, ref options, selected, ref edit_buffer, .. } = app.mode {
                            let name = options[selected].0.clone();
                            let value = edit_buffer.clone();
                            *editing = false;
                            crate::server::options::apply_set_option(app, &name, &value, true);
                            if let Mode::CustomizeMode { ref mut options, selected, .. } = app.mode {
                                options[selected].1 = value;
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        if let Mode::CustomizeMode { ref mut edit_buffer, ref mut edit_cursor, .. } = app.mode {
                            if *edit_cursor > 0 {
                                edit_buffer.remove(*edit_cursor - 1);
                                *edit_cursor -= 1;
                            }
                        }
                    }
                    KeyCode::Left => {
                        if let Mode::CustomizeMode { ref mut edit_cursor, .. } = app.mode {
                            *edit_cursor = edit_cursor.saturating_sub(1);
                        }
                    }
                    KeyCode::Right => {
                        if let Mode::CustomizeMode { ref mut edit_cursor, ref edit_buffer, .. } = app.mode {
                            if *edit_cursor < edit_buffer.len() { *edit_cursor += 1; }
                        }
                    }
                    KeyCode::Char(c) => {
                        if let Mode::CustomizeMode { ref mut edit_buffer, ref mut edit_cursor, .. } = app.mode {
                            edit_buffer.insert(*edit_cursor, c);
                            *edit_cursor += 1;
                        }
                    }
                    _ => {}
                }
            } else {
                let _visible_count = options.iter()
                    .filter(|(name, _, _)| filter.is_empty() || name.contains(filter.as_str()))
                    .count();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let visible: Vec<usize> = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i)
                                .collect();
                            if let Some(cur_pos) = visible.iter().position(|&i| i == *selected) {
                                if cur_pos > 0 {
                                    *selected = visible[cur_pos - 1];
                                    if cur_pos - 1 < *scroll_offset {
                                        *scroll_offset = cur_pos - 1;
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let visible: Vec<usize> = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i)
                                .collect();
                            if let Some(cur_pos) = visible.iter().position(|&i| i == *selected) {
                                if cur_pos + 1 < visible.len() {
                                    *selected = visible[cur_pos + 1];
                                    if cur_pos + 1 >= *scroll_offset + 20 {
                                        *scroll_offset = (cur_pos + 1).saturating_sub(19);
                                    }
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if let Mode::CustomizeMode { ref options, selected, ref mut editing, ref mut edit_buffer, ref mut edit_cursor, .. } = app.mode {
                            if let Some((_, value, _)) = options.get(selected) {
                                *edit_buffer = value.clone();
                                *edit_cursor = edit_buffer.len();
                                *editing = true;
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Mode::CustomizeMode { ref mut options, selected, .. } = app.mode {
                            if let Some(def) = crate::server::option_catalog::default_for(&options[selected].0) {
                                let name = options[selected].0.clone();
                                let value = def.to_string();
                                options[selected].1 = value.clone();
                                crate::server::options::apply_set_option(app, &name, &value, true);
                            }
                        }
                    }
                    KeyCode::Char('/') => {
                        // Enter filter mode via command prompt (simplified: clear filter or apply)
                        if let Mode::CustomizeMode { ref mut filter, ref mut scroll_offset, ref mut selected, .. } = app.mode {
                            if !filter.is_empty() {
                                // Toggle filter off
                                *filter = String::new();
                                *scroll_offset = 0;
                                *selected = 0;
                            }
                            // If filter is empty, we would need a mini prompt; for now users
                            // use the server path for full filter support
                        }
                    }
                    KeyCode::PageUp => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let visible: Vec<usize> = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i).collect();
                            if let Some(cur_pos) = visible.iter().position(|&i| i == *selected) {
                                let new_pos = cur_pos.saturating_sub(20);
                                *selected = visible[new_pos];
                                *scroll_offset = new_pos;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let visible: Vec<usize> = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i).collect();
                            if let Some(cur_pos) = visible.iter().position(|&i| i == *selected) {
                                let new_pos = (cur_pos + 20).min(visible.len().saturating_sub(1));
                                *selected = visible[new_pos];
                                if new_pos >= *scroll_offset + 20 {
                                    *scroll_offset = new_pos.saturating_sub(19);
                                }
                            }
                        }
                    }
                    KeyCode::Home | KeyCode::Char('g') => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let first = options.iter().enumerate()
                                .find(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i);
                            if let Some(idx) = first { *selected = idx; *scroll_offset = 0; }
                        }
                    }
                    KeyCode::End | KeyCode::Char('G') => {
                        if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, .. } = app.mode {
                            let last = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i).last();
                            if let Some(idx) = last {
                                *selected = idx;
                                let visible_len = options.iter()
                                    .filter(|(name, _, _)| filter.is_empty() || name.contains(filter.as_str()))
                                    .count();
                                *scroll_offset = visible_len.saturating_sub(20);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(false)
        }
        Mode::BufferChooser { selected } => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => { app.mode = Mode::Passthrough; }
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        if let Mode::BufferChooser { selected: s } = &mut app.mode { *s -= 1; }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = app.paste_buffers.len().saturating_sub(1);
                    if selected < max {
                        if let Mode::BufferChooser { selected: s } = &mut app.mode { *s += 1; }
                    }
                }
                KeyCode::Enter => {
                    // Paste selected buffer
                    if selected < app.paste_buffers.len() {
                        let text = app.paste_buffers[selected].clone();
                        app.mode = Mode::Passthrough;
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = write!(p.writer, "{}", text);
                        }
                    } else {
                        app.mode = Mode::Passthrough;
                    }
                }
                KeyCode::Char('d') | KeyCode::Delete => {
                    // Delete selected buffer
                    if selected < app.paste_buffers.len() {
                        app.paste_buffers.remove(selected);
                        if let Mode::BufferChooser { selected: s } = &mut app.mode {
                            if *s >= app.paste_buffers.len() && *s > 0 { *s -= 1; }
                        }
                        if app.paste_buffers.is_empty() { app.mode = Mode::Passthrough; }
                    }
                }
                _ => {}
            }
            Ok(false)
        }
    }
}

pub fn move_focus(app: &mut AppState, dir: FocusDir) {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    let mut active_idx = None;
    for (i, (path, _)) in rects.iter().enumerate() { if *path == win.active_path { active_idx = Some(i); break; } }
    let Some(ai) = active_idx else { return; };
    let (_, arect) = &rects[ai];
    // Collect pane IDs for MRU-based tie-breaking (issue #70)
    let pane_ids: Vec<usize> = rects.iter().map(|(path, _)| {
        crate::tree::get_active_pane_id(&win.root, path).unwrap_or(usize::MAX)
    }).collect();
    // Try direct neighbour first, then wrap to opposite edge (tmux parity #61)
    let target = find_best_pane_in_direction(&rects, ai, arect, dir, &pane_ids, &win.pane_mru)
        .or_else(|| find_wrap_target(&rects, ai, arect, dir, &pane_ids, &win.pane_mru));
    if let Some(ni) = target {
        // Update MRU: push the newly focused pane to front
        if let Some(new_pane_id) = pane_ids.get(ni) {
            crate::tree::touch_mru(&mut win.pane_mru, *new_pane_id);
        }
        win.active_path = rects[ni].0.clone();
    }
}

pub fn move_focus_preserving_zoom(app: &mut AppState, dir: FocusDir) {
    if app.windows.get(app.active_idx).map_or(false, |w| w.zoom_saved.is_some()) {
        let old_path = app.windows[app.active_idx].active_path.clone();
        toggle_zoom(app);
        move_focus(app, dir);
        toggle_zoom(app);
        if app.windows[app.active_idx].active_path == old_path {
            app.last_pane_path = old_path;
        }
    } else {
        move_focus(app, dir);
    }
}

/// Spatial pane navigation: find the best pane in the given direction.
/// Prefers panes that overlap on the perpendicular axis (visually adjacent),
/// then picks the closest by primary-axis gap, tie-broken by MRU recency
/// when multiple candidates have the same geometry (tmux parity #70).
pub fn find_best_pane_in_direction(
    rects: &[(Vec<usize>, Rect)],
    ai: usize,
    arect: &Rect,
    dir: FocusDir,
    pane_ids: &[usize],
    pane_mru: &[usize],
) -> Option<usize> {
    // Center of the active pane (scaled by 2 to avoid fractional math)
    let acx = arect.x as i32 * 2 + arect.width as i32;
    let acy = arect.y as i32 * 2 + arect.height as i32;

    // Check whether two 1-D ranges [a_start, a_start+a_len) and [b_start, b_start+b_len) overlap
    let ranges_overlap = |a_start: u16, a_len: u16, b_start: u16, b_len: u16| -> bool {
        let a_end = a_start + a_len;
        let b_end = b_start + b_len;
        a_start < b_end && b_start < a_end
    };

    // (index, primary_gap, perp_center_dist, has_perp_overlap, mru_rank)
    let mut best: Option<(usize, u32, i32, bool, usize)> = None;

    for (i, (_, r)) in rects.iter().enumerate() {
        if i == ai { continue; }
        // Primary-axis gap: the pane must be in the correct direction
        let (primary_gap, perp_overlap) = match dir {
            FocusDir::Left => {
                if r.x + r.width > arect.x { continue; }
                let gap = (arect.x - (r.x + r.width)) as u32;
                let overlap = ranges_overlap(r.y, r.height, arect.y, arect.height);
                (gap, overlap)
            }
            FocusDir::Right => {
                if r.x < arect.x + arect.width { continue; }
                let gap = (r.x - (arect.x + arect.width)) as u32;
                let overlap = ranges_overlap(r.y, r.height, arect.y, arect.height);
                (gap, overlap)
            }
            FocusDir::Up => {
                if r.y + r.height > arect.y { continue; }
                let gap = (arect.y - (r.y + r.height)) as u32;
                let overlap = ranges_overlap(r.x, r.width, arect.x, arect.width);
                (gap, overlap)
            }
            FocusDir::Down => {
                if r.y < arect.y + arect.height { continue; }
                let gap = (r.y - (arect.y + arect.height)) as u32;
                let overlap = ranges_overlap(r.x, r.width, arect.x, arect.width);
                (gap, overlap)
            }
        };

        // Perpendicular center distance (how far off-center the candidate is)
        let rcx = r.x as i32 * 2 + r.width as i32;
        let rcy = r.y as i32 * 2 + r.height as i32;
        let perp_dist = match dir {
            FocusDir::Left | FocusDir::Right => (rcy - acy).abs(),
            FocusDir::Up | FocusDir::Down => (rcx - acx).abs(),
        };

        // MRU rank: lower = more recently used (tmux parity #70)
        let rank = pane_ids.get(i)
            .map(|id| crate::tree::mru_rank(pane_mru, *id))
            .unwrap_or(usize::MAX);

        let dominated = if let Some((_, bg, bd, bo, br)) = best {
            // Prefer: (1) perp-overlapping over non-overlapping,
            //         (2) smaller primary gap,
            //         (3) among overlapping candidates with same gap → MRU (tmux parity #70),
            //         (4) among non-overlapping candidates → perpendicular center distance,
            //         (5) final fallback → MRU rank
            if perp_overlap && !bo {
                false  // new candidate has overlap, current best doesn't → new wins
            } else if !perp_overlap && bo {
                true   // current best has overlap, new doesn't → new loses
            } else if primary_gap < bg {
                false  // closer on primary axis
            } else if primary_gap > bg {
                true   // farther on primary axis
            } else if perp_overlap && bo {
                // Both candidates overlap the active pane's perpendicular
                // range with the same primary gap — use MRU directly.
                // tmux does NOT compare center distance for overlapping
                // candidates; it picks the most recently focused one.
                rank >= br
            } else if perp_dist < bd {
                false  // neither overlaps → closer perpendicular center
            } else if perp_dist > bd {
                true   // farther perpendicular center
            } else {
                rank >= br  // same geometry → MRU tie-break
            }
        } else {
            false  // no best yet
        };

        if !dominated {
            best = Some((i, primary_gap, perp_dist, perp_overlap, rank));
        }
    }

    best.map(|(idx, _, _, _, _)| idx)
}

/// Wrap-around pane navigation (tmux parity #61): when no pane exists in the
/// requested direction, wrap to the pane on the opposite edge.
/// For Right → leftmost pane, Left → rightmost, Down → topmost, Up → bottommost.
/// Prefers panes with perpendicular overlap, then closest to center.
pub fn find_wrap_target(
    rects: &[(Vec<usize>, Rect)],
    ai: usize,
    arect: &Rect,
    dir: FocusDir,
    pane_ids: &[usize],
    pane_mru: &[usize],
) -> Option<usize> {
    let acx = arect.x as i32 * 2 + arect.width as i32;
    let acy = arect.y as i32 * 2 + arect.height as i32;

    let ranges_overlap = |a_start: u16, a_len: u16, b_start: u16, b_len: u16| -> bool {
        let a_end = a_start + a_len;
        let b_end = b_start + b_len;
        a_start < b_end && b_start < a_end
    };

    // (index, edge_score, perp_center_dist, has_perp_overlap, mru_rank)
    // edge_score: lower = better (closer to the target edge after wrapping)
    let mut best: Option<(usize, i32, i32, bool, usize)> = None;

    for (i, (_, r)) in rects.iter().enumerate() {
        if i == ai { continue; }

        let (edge_score, perp_overlap) = match dir {
            // Going right, wrap to leftmost → prefer smallest x
            FocusDir::Right => {
                (r.x as i32, ranges_overlap(r.y, r.height, arect.y, arect.height))
            }
            // Going left, wrap to rightmost → prefer largest x+width (negate)
            FocusDir::Left => {
                (-((r.x + r.width) as i32), ranges_overlap(r.y, r.height, arect.y, arect.height))
            }
            // Going down, wrap to topmost → prefer smallest y
            FocusDir::Down => {
                (r.y as i32, ranges_overlap(r.x, r.width, arect.x, arect.width))
            }
            // Going up, wrap to bottommost → prefer largest y+height (negate)
            FocusDir::Up => {
                (-((r.y + r.height) as i32), ranges_overlap(r.x, r.width, arect.x, arect.width))
            }
        };

        let rcx = r.x as i32 * 2 + r.width as i32;
        let rcy = r.y as i32 * 2 + r.height as i32;
        let perp_dist = match dir {
            FocusDir::Left | FocusDir::Right => (rcy - acy).abs(),
            FocusDir::Up | FocusDir::Down => (rcx - acx).abs(),
        };

        let rank = pane_ids.get(i)
            .map(|id| crate::tree::mru_rank(pane_mru, *id))
            .unwrap_or(usize::MAX);

        let dominated = if let Some((_, be, bd, bo, br)) = best {
            if perp_overlap && !bo {
                false
            } else if !perp_overlap && bo {
                true
            } else if edge_score < be {
                false
            } else if edge_score > be {
                true
            } else if perp_overlap && bo {
                // Both overlap with same edge score → MRU (tmux parity #70)
                rank >= br
            } else if perp_dist < bd {
                false
            } else if perp_dist > bd {
                true
            } else {
                rank >= br  // same geometry → MRU tie-break
            }
        } else {
            false
        };

        if !dominated {
            best = Some((i, edge_score, perp_dist, perp_overlap, rank));
        }
    }

    // Tmux parity (#141): wrapped navigation must stay within the same
    // column (U/D) or row (L/R). If no candidate overlaps on the
    // perpendicular axis, the pane is alone in its row/column and
    // navigation should stay put (no-op) rather than jump sideways.
    best.filter(|(_, _, _, has_overlap, _)| *has_overlap)
        .map(|(idx, _, _, _, _)| idx)
}

/// Encode a crossterm `KeyEvent` into the byte sequence that should be
/// written to the child PTY.  Extracted as a standalone function so it can
/// be unit-tested without needing a full `AppState`.
///
/// Returns `None` for key codes we don't handle (F-keys, etc.).
/// Compute xterm modifier parameter: 1 + Shift*1 + Alt*2 + Ctrl*4.
/// Returns 1 when no modifiers are held (callers use >1 to decide whether to
/// emit the extended `;mod` form).
fn modifier_param(mods: KeyModifiers) -> u8 {
    let mut m: u8 = 1;
    if mods.contains(KeyModifiers::SHIFT) { m += 1; }
    if mods.contains(KeyModifiers::ALT) { m += 2; }
    if mods.contains(KeyModifiers::CONTROL) { m += 4; }
    m
}

/// Parse modifier+special key names like "C-Left", "S-Right", "C-S-Up",
/// "C-M-Home", etc. and return the xterm escape sequence.
/// Returns None if the string isn't a recognized modified special key.
pub fn parse_modified_special_key(s: &str) -> Option<String> {
    let upper = s.to_uppercase();
    // Extract modifier prefixes and base key name
    let mut rest = upper.as_str();
    let mut bits: u8 = 0;
    loop {
        if rest.starts_with("C-") { bits |= 4; rest = &rest[2..]; }
        else if rest.starts_with("M-") { bits |= 2; rest = &rest[2..]; }
        else if rest.starts_with("S-") { bits |= 1; rest = &rest[2..]; }
        else { break; }
    }
    if bits == 0 { return None; } // no modifiers found
    let m = bits + 1; // xterm modifier param = 1 + modifier bits
    // Match the base key name
    match rest {
        "ENTER" | "RETURN" | "CR" => Some(format!("\x1b[13;{}~", m)),
        "TAB" => Some(format!("\x1b[9;{}~", m)),
        "BTAB" | "BACKTAB" => {
            // Shift is implicit in BackTab; ensure Shift bit is set in the bitmask
            let sm = (bits | 1) + 1;
            Some(format!("\x1b[9;{}~", sm))
        }
        // Backspace is the one modified special key whose Windows encoding is a
        // RAW BYTE rather than a CSI sequence, so it cannot use `m` (issue #610).
        //
        // Measured against a real pseudoconsole (the same thing psmux gives a
        // pane), in both directions:
        //   * ConPTY's input parser turns the byte 0x7f into VK_BACK with no
        //     modifiers, and the byte 0x08 into VK_BACK with LEFT_CTRL_PRESSED.
        //   * conhost's own record to VT encoder produces exactly those two
        //     bytes for Backspace and Ctrl+Backspace, and 1b 7f for Alt+Backspace.
        // So 0x08 is not an approximation of Ctrl+Backspace on Windows, it IS
        // the encoding, and it is what a record reading app such as PSReadLine
        // needs to run BackwardKillWord.
        //
        // A CSI form would deliver NOTHING: ESC [ 127;5 u, ESC [ 8;5 u and
        // ESC [ 27;5;127 ~ were all measured to be silently discarded by the
        // ConPTY input parser, producing zero input records.
        //
        // Alt takes the usual ESC prefix, matching tmux 3.4, whose
        // `send-keys M-BSpace` writes 1b 7f into the pane.  Shift has no
        // distinct encoding on either side (conhost sends plain 0x7f for
        // Shift+Backspace), so S- collapses onto the unmodified byte.
        "BSPACE" | "BACKSPACE" => {
            let base = if bits & 4 != 0 { '\u{8}' } else { '\u{7f}' };
            if bits & 2 != 0 {
                Some(format!("\x1b{}", base))
            } else {
                Some(base.to_string())
            }
        }
        "LEFT" => Some(format!("\x1b[1;{}D", m)),
        "RIGHT" => Some(format!("\x1b[1;{}C", m)),
        "UP" => Some(format!("\x1b[1;{}A", m)),
        "DOWN" => Some(format!("\x1b[1;{}B", m)),
        "HOME" => Some(format!("\x1b[1;{}H", m)),
        "END" => Some(format!("\x1b[1;{}F", m)),
        "INSERT" | "IC" => Some(format!("\x1b[2;{}~", m)),
        "DELETE" | "DC" => Some(format!("\x1b[3;{}~", m)),
        "PAGEUP" | "PPAGE" => Some(format!("\x1b[5;{}~", m)),
        "PAGEDOWN" | "NPAGE" => Some(format!("\x1b[6;{}~", m)),
        s if s.starts_with('F') && s.len() >= 2 => {
            if let Ok(n) = s[1..].parse::<u8>() {
                let seq = encode_fkey(n, m);
                if seq.is_empty() { None } else { Some(String::from_utf8_lossy(&seq).into_owned()) }
            } else { None }
        }
        _ => None,
    }
}

/// Encode an F-key with optional xterm modifier parameter.
fn encode_fkey(n: u8, m: u8) -> Vec<u8> {
    // F1-F4 use SS3 when unmodified, CSI with modifier when modified.
    let (prefix, num) = match n {
        1 => if m > 1 { ("", Some((11, 'P'))) } else { return b"\x1bOP".to_vec() },
        2 => if m > 1 { ("", Some((12, 'Q'))) } else { return b"\x1bOQ".to_vec() },
        3 => if m > 1 { ("", Some((13, 'R'))) } else { return b"\x1bOR".to_vec() },
        4 => if m > 1 { ("", Some((14, 'S'))) } else { return b"\x1bOS".to_vec() },
        5 => ("", Some((15, '~'))),
        6 => ("", Some((17, '~'))),
        7 => ("", Some((18, '~'))),
        8 => ("", Some((19, '~'))),
        9 => ("", Some((20, '~'))),
        10 => ("", Some((21, '~'))),
        11 => ("", Some((23, '~'))),
        12 => ("", Some((24, '~'))),
        _ => return Vec::new(),
    };
    let _ = prefix;
    if let Some((code, suffix)) = num {
        if suffix == '~' {
            if m > 1 { format!("\x1b[{};{}~", code, m).into_bytes() }
            else { format!("\x1b[{}~", code).into_bytes() }
        } else {
            // F1-F4 modified: \x1b[1;{mod}P/Q/R/S
            format!("\x1b[1;{}{}", m, suffix).into_bytes()
        }
    } else {
        Vec::new()
    }
}

/// Convert a character into the byte produced by Ctrl+<char> for `send-keys`,
/// matching tmux's input-keys.c `standard_map` semantics.
///
/// This is used by `send-keys C-x` (and `C-M-x`) to produce the correct
/// terminal byte for non-letter keys. For example:
///   * `C-/` -> 0x1f (^_)        (NOT 0x0f, which is the naive `'/' & 0x1f`)
///   * `C-?` -> 0x7f (DEL)
///   * `C-3` -> 0x1b (ESC), `C-4` -> 0x1c, ..., `C-7` -> 0x1f
///   * `C-Space`, `C-2` -> 0x00 (NUL)
///   * `C-@`..`C-~` (letters and a few punctuation) -> `c & 0x1f`
///   * `C-!` -> '1' (literal printable, per tmux remap)
///
/// Returns `None` for keys that have no defined Ctrl encoding (tmux rejects
/// these by returning -1 from `input_key_vt10x`).
pub fn ctrl_char_send_keys_byte(c: char) -> Option<u8> {
    if !c.is_ascii() { return None; }
    let b = c as u8;
    // tmux input-keys.c standard_map: special punctuation/digit remaps.
    // Pairs: input char -> output byte. Some remaps are to printable ASCII
    // (e.g. C-! -> '1'); others to control bytes (e.g. C-/ -> 0x1f).
    let remap: &[(u8, u8)] = &[
        (b'1', b'1'), (b'!', b'1'),
        (b'9', b'9'), (b'(', b'9'),
        (b'0', b'0'), (b')', b'0'),
        (b'=', b'='), (b'+', b'+'),
        (b';', b';'), (b':', b';'),
        (b'\'', b'\''), (b'"', b'\''),
        (b',', b','), (b'<', b','),
        (b'.', b'.'), (b'>', b'.'),
        (b'/', 0x1f), (b'-', 0x1f),
        (b'8', 0x7f), (b'?', 0x7f),
        (b' ', 0x00), (b'2', 0x00),
    ];
    if let Some(&(_, v)) = remap.iter().find(|(k, _)| *k == b) {
        return Some(v);
    }
    // Digits 3-7 map to C0 codes 0x1b-0x1f.
    if (b'3'..=b'7').contains(&b) {
        return Some(b - 0x18);
    }
    // Standard Ctrl+letter/punct range '@'..'~' -> mask with 0x1f.
    if (b'@'..=b'~').contains(&b) {
        return Some(b.to_ascii_lowercase() & 0x1f);
    }
    None
}

pub fn encode_key_event(key: &KeyEvent) -> Option<Vec<u8>> {
    let encoded: Vec<u8> = match key.code {
        // AltGr detection: On Windows, AltGr is reported as Ctrl+Alt by the
        // console subsystem / crossterm.  International keyboards (German,
        // Czech, Polish, …) use AltGr to produce characters like \ @ { } [ ]
        // | ~ €.  crossterm delivers these as KeyCode::Char(produced_char)
        // with CONTROL|ALT modifiers.  The produced character is NOT an ASCII
        // letter (a-z), so we can distinguish AltGr from genuine Ctrl+Alt
        // combos and forward the character as-is.
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.modifiers.contains(KeyModifiers::ALT)
            && !c.is_ascii_lowercase() => {
            // AltGr-produced character — forward it verbatim (UTF-8).
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf);
            buf[..c.len_utf8()].to_vec()
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) && key.modifiers.contains(KeyModifiers::ALT) => {
            // Genuine Ctrl+Alt+letter — encode as ESC + ctrl-char.
            let ctrl_char = (c as u8) & 0x1F;
            vec![0x1b, ctrl_char]
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
            format!("\x1b{}", c).into_bytes()
        }
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Use the tmux-parity control mapping so punctuation collapses to the
            // correct C0 byte: C-/ -> 0x1f (^_), C-- (Ctrl+Shift+-) -> 0x1f, etc.
            // The naive `c & 0x1f` mis-mapped these (e.g. '/' -> 0x0f ^O, issue
            // #226) and broke neovim's Ctrl+/ comment-toggle under ConPTY
            // terminals like Alacritty/WezTerm (issue #394).
            let ctrl_char = ctrl_char_send_keys_byte(c).unwrap_or((c as u8) & 0x1F);
            vec![ctrl_char]
        }
        KeyCode::Char(c) if (c as u32) >= 0x01 && (c as u32) <= 0x1A => {
            vec![c as u8]
        }
        KeyCode::Char(c) => {
            format!("{}", c).into_bytes()
        }
        KeyCode::Enter => {
            let m = modifier_param(key.modifiers);
            if m > 1 {
                // On Windows, CSI 13;mod~ is non-standard and dropped by ConPTY.
                // Send ESC+CR (\x1b\r) for Shift/Alt+Enter — the same bytes VS Code's
                // xterm.js sends.  libuv preserves ESC as Alt prefix, so Node.js apps
                // (Claude Code) receive \x1b\r and interpret it as Shift+Enter.
                // Ctrl+Enter and Ctrl+Shift+Enter still use CSI encoding (those are
                // less common and consumed by other layers).
                #[cfg(windows)]
                {
                    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                    let has_alt = key.modifiers.contains(KeyModifiers::ALT);
                    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
                    if !has_ctrl {
                        return Some(b"\x1b\r".to_vec());
                    }
                    // Plain Ctrl+Enter: LF (0x0A), matching Windows Terminal and the
                    // native VK_RETURN injection payload (#409).  This is the byte
                    // fallback used only when native injection is unavailable.
                    if has_ctrl && !has_alt && !has_shift {
                        return Some(b"\n".to_vec());
                    }
                }
                // Non-Windows or other Ctrl combos: xterm modified-Enter: CSI 13 ; mod ~
                format!("\x1b[13;{}~", m).into_bytes()
            } else {
                b"\r".to_vec()
            }
        }
        KeyCode::Tab => {
            let m = modifier_param(key.modifiers);
            if m > 1 {
                // xterm modified-Tab: CSI 9 ; mod ~
                format!("\x1b[9;{}~", m).into_bytes()
            } else {
                b"\t".to_vec()
            }
        }
        KeyCode::BackTab => {
            let m = modifier_param(key.modifiers);
            if m > 1 {
                // Shift is implicit in BackTab; add it back for the modifier param
                let sm = m | 1; // ensure Shift bit is set
                format!("\x1b[9;{}~", sm).into_bytes()
            } else {
                b"\x1b[Z".to_vec()
            }
        }
        KeyCode::Backspace => b"\x08".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        // Arrow keys and special keys with xterm modifier encoding.
        // Format: \x1b[1;{mod}{letter} where mod = 1 + Shift*1 + Alt*2 + Ctrl*4
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down |
        KeyCode::Home | KeyCode::End => {
            let letter = match key.code {
                KeyCode::Up => 'A', KeyCode::Down => 'B',
                KeyCode::Right => 'C', KeyCode::Left => 'D',
                KeyCode::Home => 'H', KeyCode::End => 'F',
                _ => unreachable!(),
            };
            let m = modifier_param(key.modifiers);
            if m > 1 {
                format!("\x1b[1;{}{}", m, letter).into_bytes()
            } else {
                format!("\x1b[{}", letter).into_bytes()
            }
        }
        // Tilde-style keys: \x1b[{N};{mod}~ when modifiers present
        KeyCode::Insert | KeyCode::Delete | KeyCode::PageUp | KeyCode::PageDown => {
            let n = match key.code {
                KeyCode::Insert => 2, KeyCode::Delete => 3,
                KeyCode::PageUp => 5, KeyCode::PageDown => 6,
                _ => unreachable!(),
            };
            let m = modifier_param(key.modifiers);
            if m > 1 {
                format!("\x1b[{};{}~", n, m).into_bytes()
            } else {
                format!("\x1b[{}~", n).into_bytes()
            }
        }
        KeyCode::F(n) => {
            let m = modifier_param(key.modifiers);
            encode_fkey(n, m)
        }
        _ => return None,
    };
    Some(encoded)
}

/// Map a CSI cursor key (`ESC [` A/B/C/D/H/F) to its SS3 form (`ESC O` x) when the
/// target pane is in DECCKM application-cursor-keys mode; `None` keeps the CSI form.
///
/// Apps that enable application-cursor mode (via `ESC [ ? 1 h`), notably
/// PowerShell/PSReadLine reached through an SSH pane, expect SS3 for unmodified
/// arrows/Home/End and echo the raw escape when handed CSI instead. Local panes
/// usually leave the mode off, so the mismatch only bites over SSH. Modified cursor
/// keys stay xterm CSI (`ESC [ 1 ; mod x`); they are longer than 3 bytes, so the
/// `len() == 3` guard skips them. tmux parity (`input_key`, `MODE_KCURSOR`).
pub(crate) fn csi_cursor_to_ss3(seq: &[u8], app_cursor: bool) -> Option<[u8; 3]> {
    if app_cursor
        && seq.len() == 3
        && seq[0] == 0x1b
        && seq[1] == b'['
        && matches!(seq[2], b'A' | b'B' | b'C' | b'D' | b'H' | b'F')
    {
        Some([0x1b, b'O', seq[2]])
    } else {
        None
    }
}

/// Write a key's byte sequence to a pane, upgrading a CSI cursor key to its SS3
/// form when the pane is in DECCKM application-cursor mode (see csi_cursor_to_ss3).
/// The transform no-ops on every other sequence, so all named keys route through
/// this uniformly. Does not flush; callers flush once after the write.
pub(crate) fn write_key_seq(p: &mut crate::types::Pane, seq: &[u8]) {
    use std::io::Write as _;
    let app_cursor = p.term.lock()
        .map(|t| t.screen().application_cursor())
        .unwrap_or(false);
    let _ = match csi_cursor_to_ss3(seq, app_cursor) {
        Some(ss3) => p.writer.write_all(&ss3),
        None => p.writer.write_all(seq),
    };
}

/// A printable text keystroke on the INTERACTIVE input route (drives
/// `#{pane_last_text_input}`). Excludes control codes and any Ctrl/Alt-modified
/// key, so navigation, shortcuts, Enter, Tab, etc. don't count. Shift is fine
/// (capitals).
pub(crate) fn is_text_input_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(c)
            if !c.is_control()
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
    )
}

/// True when this key event represents plain Ctrl+C (or raw ETX / 0x03)
/// without Alt modifiers. Used to keep interrupt behavior on Windows.
pub(crate) fn is_ctrl_c_key_event(key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(c) => {
            (key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && c.eq_ignore_ascii_case(&'c'))
                || (!key.modifiers.contains(KeyModifiers::ALT) && c == '\u{0003}')
        }
        _ => false,
    }
}

/// Apply `f` to every pane that will RECEIVE the current interactive key --
/// every non-dead pane under sync-input, else the active pane if alive -- so the
/// route-signal timestamps match what's actually routed.
fn for_each_receiving_pane<F: FnMut(&mut Pane)>(app: &mut AppState, mut f: F) {
    let sync = app.sync_input;
    let win = &mut app.windows[app.active_idx];
    if sync {
        fn walk<F: FnMut(&mut Pane)>(node: &mut Node, f: &mut F) {
            match node {
                Node::Leaf(p) if !p.dead => f(p),
                Node::Leaf(_) => {}
                Node::Split { children, .. } => { for c in children { walk(c, f); } }
            }
        }
        walk(&mut win.root, &mut f);
    } else if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        if !p.dead {
            f(p);
        }
    }
}

/// Stamp the INTERACTIVE-route signals for input that arrived from an attached
/// client rather than from `send-keys`.
///
/// The signals are recorded where `forward_key_to_active` runs, but an attached
/// client does not run it: the client forwards its keystrokes to the server as
/// `send-text` / `send-key` control commands, and the server writes them to the
/// pty. So `#{pane_last_special_key}`, `_ms` and `#{pane_last_text_input}` were
/// empty for every real user of an attached session, in every path that can read
/// them, including the status bar the user is looking at.
///
/// The injected route stays unstamped, as documented: `send-keys` arrives as
/// CtrlReq::SendKeys (plural), a different message that does not call these.
pub fn stamp_interactive_text(app: &mut AppState) {
    let now = Instant::now();
    for_each_receiving_pane(app, |p| p.last_text_input = Some(now));
}

/// Same, for a non-text key forwarded by an attached client. `name` is the
/// canonical bind-key spelling the client already sends (Escape, Enter, Up, F9,
/// C-c, M-a, ...). A single-character payload is ordinary typing, so it is
/// recorded as text input to match `is_text_input_key`.
pub fn stamp_interactive_key(app: &mut AppState, name: &str) {
    if name.chars().count() == 1 {
        stamp_interactive_text(app);
        return;
    }
    // Report the CANONICAL bind-key spelling, not the client's wire spelling.
    // The client sends "esc" and "enter"; the documented value of
    // #{pane_last_special_key} is the bind-key name, "Escape" and "Enter", which
    // is what the local route produces via format_key_binding. Round-tripping the
    // wire name through the product's own key tables keeps the two routes
    // identical instead of hand-maintaining a second spelling.
    let canonical = crate::config::parse_key_name(name)
        .map(|(code, mods)| crate::config::format_key_binding(&(code, mods)))
        .unwrap_or_else(|| name.to_string());
    let now = Instant::now();
    for_each_receiving_pane(app, |p| p.last_special_key = Some((now, canonical.clone())));
}

pub fn forward_key_to_active(app: &mut AppState, key: KeyEvent) -> io::Result<()> {
    // Record use of the INTERACTIVE input route as read-only route signals:
    // `#{pane_last_text_input}` (printable text) and, for every other key,
    // `#{pane_last_special_key}` / `_ms` (the last non-text key -- Esc, Enter,
    // arrows, function keys, Ctrl/Alt chords -- by canonical bind-key name).
    // This route is handle_key -> forward_key_to_active; the injected route
    // (send-keys / send-paste / send-text -> send_text_to_active) does NOT pass
    // here, so it never updates these signals. for_each_receiving_pane stamps
    // exactly the panes that will RECEIVE the key, so the timestamps match what
    // is actually routed.
    if is_text_input_key(&key) {
        let now = Instant::now();
        for_each_receiving_pane(app, |p| p.last_text_input = Some(now));
    } else {
        let now = Instant::now();
        let name = crate::config::format_key_binding(&(key.code, key.modifiers));
        for_each_receiving_pane(app, |p| p.last_special_key = Some((now, name.clone())));
    }

    // On Windows, modified Enter delivery depends on the modifier:
    //
    // Shift/Alt+Enter (no Ctrl): Use VT encoding ONLY (\x1b\r).  Native
    //   WriteConsoleInputW injection would cause ConPTY to translate the
    //   KEY_EVENT back to plain \r, so VT-native apps (Claude Code) see a
    //   double Enter.
    //
    // Ctrl+Enter / Ctrl+Shift+Enter: Use native injection ONLY.  ConPTY
    //   cannot encode Ctrl+Enter in VT, so injection is the only reliable
    //   path for console apps (PSReadLine).  Falls back to xterm CSI
    //   encoding (\x1b[13;N~) if injection fails (for non-console apps).
    #[cfg(windows)]
    {
        if matches!(key.code, KeyCode::Enter) && !key.modifiers.is_empty() {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let shift = key.modifiers.contains(KeyModifiers::SHIFT);

            // Only use native injection when Ctrl is involved.
            if ctrl {
                let try_inject = |pane: &mut Pane| -> bool {
                    if let Some(pid) = pane.child_pid {
                        // send_modified_enter_event injects VK_RETURN with the correct
                        // modifier flags and (for plain Ctrl+Enter) an LF character
                        // payload, matching Windows Terminal so VT/raw readers see LF
                        // instead of CR (#409).
                        crate::platform::mouse_inject::send_modified_enter_event(pid, ctrl, alt, shift)
                    } else {
                        false
                    }
                };

                if app.sync_input {
                    let win = &mut app.windows[app.active_idx];
                    fn inject_all(node: &mut Node, ctrl: bool, alt: bool, shift: bool) {
                        match node {
                            Node::Leaf(p) if !p.dead => {
                                if let Some(pid) = p.child_pid {
                                    if !crate::platform::mouse_inject::send_modified_enter_event(pid, ctrl, alt, shift) {
                                        // Fallback: xterm CSI encoding for non-console apps
                                        let m: u8 = 1 + (shift as u8) + (alt as u8) * 2 + (ctrl as u8) * 4;
                                        let bytes = if m > 1 { format!("\x1b[13;{}~", m).into_bytes() } else { b"\r".to_vec() };
                                        let _ = p.writer.write_all(&bytes);
                                        let _ = p.writer.flush();
                                    }
                                }
                            }
                            Node::Leaf(_) => {}
                            Node::Split { children, .. } => {
                                for c in children { inject_all(c, ctrl, alt, shift); }
                            }
                        }
                    }
                    inject_all(&mut win.root, ctrl, alt, shift);
                    return Ok(());
                } else {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        if !active.dead {
                            if try_inject(active) {
                                return Ok(());
                            }
                            // Fallback: VT encoding (falls through below)
                        }
                    }
                }
            }
            // Shift/Alt+Enter (no Ctrl): fall through to VT encoding below.
        }

        // Ctrl+letter: inject via WriteConsoleInputW so ConPTY's VT parser
        // state is never touched.  Writing Win32 VT sequences to the pipe
        // leaves the parser buffering \x1b, which blocks subsequent ESC
        // delivery to apps like Neovim (#305, fixed for send-keys; this
        // fixes the live-keypress path).
        //
        // crossterm may report Ctrl+K two ways:
        //   1. KeyCode::Char('k') with KeyModifiers::CONTROL
        //   2. KeyCode::Char('\x0b') with NO modifiers (raw control byte)
        // We handle both variants here.
        if let KeyCode::Char(c) = key.code {
            let is_ctrl_c = is_ctrl_c_key_event(&key);
            let (inject_char, is_ctrl_letter) = if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && c.is_ascii_alphabetic()
            {
                (c, true)
            } else if !key.modifiers.contains(KeyModifiers::ALT)
                && (c as u32) >= 0x01
                && (c as u32) <= 0x1A
            {
                // Raw control byte → map back to the letter (0x01='a', 0x0B='k', etc.)
                let letter = (c as u8 + b'a' - 1) as char;
                (letter, true)
            } else {
                (c, false)
            };

            if is_ctrl_letter {
                let ctrl_char = (inject_char.to_ascii_lowercase() as u8) & 0x1F;

                if app.sync_input {
                    let win = &mut app.windows[app.active_idx];
                    fn inject_ctrl_all(node: &mut Node, ch: char, raw: u8, is_ctrl_c: bool) {
                        match node {
                            Node::Leaf(p) if !p.dead => {
                                // Ctrl+C: consult the interrupt router BEFORE writing the
                                // byte — when it decides "raw 0x03 only" it may strip
                                // PROCESSED_INPUT from the pane console so conhost hands
                                // the byte over as input instead of converting it into a
                                // console-wide CTRL_C_EVENT (which aborts a booting WSL
                                // launch, issue #579). Writing first loses that race.
                                #[cfg(windows)]
                                if is_ctrl_c {
                                    if let Some(pid) = p.child_pid {
                                        crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                                    }
                                }
                                let _ = p.writer.write_all(&[raw]);
                                let _ = p.writer.flush();
                                #[cfg(windows)]
                                if !is_ctrl_c {
                                    if let Some(pid) = p.child_pid {
                                        crate::platform::mouse_inject::send_modified_key_event(pid, ch, true, false, false);
                                    }
                                }
                                crate::debug_log::input_log("ctrl-key",
                                    &format!("sync inject_ctrl char='{}' pid={:?}", ch, p.child_pid));
                            }
                            Node::Leaf(_) => {}
                            Node::Split { children, .. } => {
                                for child in children { inject_ctrl_all(child, ch, raw, is_ctrl_c); }
                            }
                        }
                    }
                    inject_ctrl_all(&mut win.root, inject_char, ctrl_char, is_ctrl_c);
                } else {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                        if !active.dead {
                            // Same ordering as the sync arm above: router (and its
                            // possible PROCESSED_INPUT strip) before the byte (#579).
                            #[cfg(windows)]
                            if is_ctrl_c {
                                if let Some(pid) = active.child_pid {
                                    crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                                }
                            }
                            let _ = active.writer.write_all(&[ctrl_char]);
                            let _ = active.writer.flush();
                            #[cfg(windows)]
                            if !is_ctrl_c {
                                if let Some(pid) = active.child_pid {
                                    crate::platform::mouse_inject::send_modified_key_event(pid, inject_char, true, false, false);
                                }
                            }
                            crate::debug_log::input_log("ctrl-key",
                                &format!("inject_ctrl char='{}' pid={:?}", inject_char, active.child_pid));
                        }
                    }
                }
                return Ok(());
            }
        }
    }

    let encoded = match encode_key_event(&key) {
        Some(bytes) => bytes,
        None => return Ok(()),
    };

    if app.sync_input {
        // Fan out to ALL panes in the current window
        let win = &mut app.windows[app.active_idx];
        fn write_all_panes(node: &mut Node, data: &[u8]) {
            match node {
                Node::Leaf(p) if !p.dead => { let _ = p.writer.write_all(data); let _ = p.writer.flush(); }
                Node::Leaf(_) => {}
                Node::Split { children, .. } => { for c in children { write_all_panes(c, data); } }
            }
        }
        write_all_panes(&mut win.root, &encoded);

    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if !active.dead {
                let _ = active.writer.write_all(&encoded);
                let _ = active.writer.flush();

            }
        }
    }
    Ok(())
}

/// Chunked PTY write for paste delivery.  The PTY pipe can silently
/// drop bytes when a large payload (140+ lines) is written in a single
/// call because the OS pipe buffer fills up.  We split the text into
/// ~2 KiB chunks with small yields between them so the consumer
/// (shell / PSReadLine / nvim) has time to drain.  Bracket sequences
/// are tiny and always written in one shot.
fn write_paste_chunked(writer: &mut dyn std::io::Write, text: &[u8], bracket: bool) {
    const CHUNK: usize = 512;
    // Normalize line endings to CR for ConPTY.  Clipboard text may arrive
    // with LF (\n) or CRLF (\r\n), but ConPTY's input parser expects CR
    // (\r) for Enter.  Bare LF is misinterpreted by PSReadLine, causing
    // multi-line pastes to appear in reverse order.
    let text = {
        let mut out = Vec::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            if text[i] == b'\r' && i + 1 < text.len() && text[i + 1] == b'\n' {
                out.push(b'\r');
                i += 2; // CRLF → CR
            } else if text[i] == b'\n' {
                out.push(b'\r');
                i += 1; // LF → CR
            } else {
                out.push(text[i]);
                i += 1;
            }
        }
        out
    };
    let text = &text[..];
    if bracket { let _ = writer.write_all(b"\x1b[200~"); }
    let mut offset: usize = 0;
    while offset < text.len() {
        let remaining = (text.len() - offset).min(CHUNK);
        let chunk = &text[offset..offset + remaining];
        match writer.write(chunk) {
            Ok(0) => {
                // Zero bytes written — yield and retry once
                std::thread::sleep(std::time::Duration::from_millis(10));
                match writer.write(chunk) {
                    Ok(n) if n > 0 => { offset += n; }
                    _ => break, // give up on persistent failure
                }
            }
            Ok(n) => { offset += n; }
            Err(_) => break,
        }
        // Yield between chunks to let the consumer drain the buffer
        if offset < text.len() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    if bracket { let _ = writer.write_all(b"\x1b[201~"); }
    let _ = writer.flush();
}

/// Send pasted text to the active pane, wrapping in bracketed-paste
/// sequences (\x1b[200~ … \x1b[201~) when the child has enabled that mode.
/// This is the correct handler for `Event::Paste` (crossterm) and
/// drag-and-drop file paths, ensuring applications like Claude CLI can
/// distinguish paste/drop from typed input.
pub fn send_paste_to_active(app: &mut AppState, text: &str) -> io::Result<()> {
    // In clock mode, any input exits back to passthrough
    if matches!(app.mode, Mode::ClockMode) {
        app.mode = Mode::Passthrough;
        return Ok(());
    }
    // In copy / copy-search modes, treat like regular text
    if matches!(app.mode, Mode::CopyMode) {
        return send_text_to_active(app, text);
    }
    if matches!(app.mode, Mode::CopySearch { .. }) {
        return send_text_to_active(app, text);
    }

    // Check if the child requested bracketed paste mode
    let use_bracket = {
        let win = &app.windows[app.active_idx];
        if let Some(p) = crate::tree::active_pane(&win.root, &win.active_path) {
            if let Ok(parser) = p.term.lock() {
                let bp = parser.screen().bracketed_paste();
                crate::debug_log::input_log("paste", &format!("child bracketed_paste()={}", bp));
                bp
            } else {
                crate::debug_log::input_log("paste", "term lock failed");
                false
            }
        } else {
            crate::debug_log::input_log("paste", "no active pane");
            false
        }
    };
    crate::debug_log::input_log("paste", &format!("use_bracket={} text_len={} text_preview={:?}", use_bracket, text.len(), &text.chars().take(100).collect::<String>()));

    // On Windows, bracketed paste delivery is tricky:
    //
    // - ConPTY may strip \x1b[200~/201~ from the PTY input pipe (older Windows).
    // - WriteConsoleInputW can bypass ConPTY, but it sends each byte of the
    //   bracket sequence as a separate KEY_EVENT record.  Apps that read via
    //   ReadConsoleInputW (crossterm-based apps like Helix) cannot reassemble
    //   VT sequences from individual key events, so \x1b[200~ appears as the
    //   literal characters Esc [ 2 0 0 ~ in the editor (issue #98).
    // - Apps that read raw bytes via ReadFile (nvim via libuv) CAN parse the
    //   bracket sequences from console-injected KEY_EVENTs.
    //
    // Strategy: try the PTY pipe first with bracket markers.  This works on
    // newer Windows where ConPTY passes VT input through, and also works for
    // byte-stream readers (nvim).  If the child uses ReadConsoleInputW
    // (crossterm), ConPTY converts the VT bytes to KEY_EVENTs anyway, so the
    // brackets may still not be parsed -- but at least the text content
    // arrives correctly without stray visible bracket characters.
    //
    // For apps where PTY-pipe brackets get stripped by ConPTY, fall back to
    // console injection for the TEXT ONLY (no bracket markers) so the content
    // still arrives reliably.
    #[cfg(windows)]
    {
        if app.sync_input {
            let win = &mut app.windows[app.active_idx];
            fn write_all_panes(node: &mut crate::types::Node, text: &[u8], bracket: bool) {
                match node {
                    crate::types::Node::Leaf(p) => {
                        write_paste_chunked(&mut p.writer, text, bracket);
                    }
                    crate::types::Node::Split { children, .. } => {
                        for c in children { write_all_panes(c, text, bracket); }
                    }
                }
            }
            write_all_panes(&mut win.root, text.as_bytes(), use_bracket);
        } else {
            let win = &mut app.windows[app.active_idx];
            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                write_paste_chunked(&mut p.writer, text.as_bytes(), use_bracket);
            }
        }
    }

    // On non-Windows, use standard PTY pipe write with bracket sequences
    #[cfg(not(windows))]
    {
        if app.sync_input {
            let win = &mut app.windows[app.active_idx];
            fn write_paste_all_panes(node: &mut Node, text: &[u8], bracket: bool) {
                match node {
                    Node::Leaf(p) => {
                        write_paste_chunked(&mut p.writer, text, bracket);
                    }
                    Node::Split { children, .. } => {
                        for c in children { write_paste_all_panes(c, text, bracket); }
                    }
                }
            }
            write_paste_all_panes(&mut win.root, text.as_bytes(), use_bracket);
        } else {
            let win = &mut app.windows[app.active_idx];
            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                write_paste_chunked(&mut p.writer, text.as_bytes(), use_bracket);
            }
        }
    }
    Ok(())
}

/// Write raw bytes (decoded from `send-keys -H`) to whichever pane would
/// receive input.
///
/// Valid UTF-8 is handed to `send_text_to_active` so every existing mode
/// (copy, popup, confirm, menu, synchronized input) keeps its semantics.  A
/// byte run that is not valid UTF-8 bypasses that and goes straight to the
/// pane: callers that chunk their input can split a multi-byte character
/// across two `send-keys` calls, and a lossy decode would corrupt it.
pub fn send_bytes_to_active(app: &mut AppState, bytes: &[u8]) -> io::Result<()> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return send_text_to_active(app, text);
    }
    {
        let win = &mut app.windows[app.active_idx];
        if let Some(fi) = win.floating_focus {
            if let Some(fp) = win.floating.get_mut(fi) {
                let _ = fp.pane.writer.write_all(bytes);
                let _ = fp.pane.writer.flush();
                return Ok(());
            }
        }
    }
    if app.sync_input {
        let win = &mut app.windows[app.active_idx];
        fn write_all_panes(node: &mut Node, data: &[u8]) {
            match node {
                Node::Leaf(p) => { let _ = p.writer.write_all(data); let _ = p.writer.flush(); }
                Node::Split { children, .. } => { for c in children { write_all_panes(c, data); } }
            }
        }
        write_all_panes(&mut win.root, bytes);
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = p.writer.write_all(bytes);
            let _ = p.writer.flush();
        }
    }
    Ok(())
}

pub fn send_text_to_active(app: &mut AppState, text: &str) -> io::Result<()> {
    // In clock mode, any input exits back to passthrough
    if matches!(app.mode, Mode::ClockMode) {
        app.mode = Mode::Passthrough;
        return Ok(());
    }
    // Route input to active overlay (so CLI send-keys can interact with overlays)
    if matches!(app.mode, Mode::PopupMode { .. }) {
        // Escape (\x1b alone) closes popup; other text goes to popup PTY
        if text == "\x1b" {
            app.mode = Mode::Passthrough;
            return Ok(());
        }
        if let Mode::PopupMode { ref mut popup_pane, .. } = app.mode {
            if let Some(ref mut pty) = popup_pane {
                let _ = pty.writer.write_all(text.as_bytes());
                let _ = pty.writer.flush();
            }
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::ConfirmMode { .. }) {
        for c in text.chars() {
            match c {
                'y' | 'Y' => {
                    if let Mode::ConfirmMode { ref command, .. } = app.mode {
                        let cmd = command.clone();
                        app.mode = Mode::Passthrough;
                        crate::config::parse_config_line(app, &cmd);
                    }
                    return Ok(());
                }
                'n' | 'N' => {
                    app.mode = Mode::Passthrough;
                    return Ok(());
                }
                _ => {} // Ignore other chars during confirm
            }
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::MenuMode { .. }) {
        // Escape closes menu; other text is ignored (menu is navigated via send-key)
        if text == "\x1b" {
            app.mode = Mode::Passthrough;
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::PaneChooser { .. }) {
        // Escape closes display-panes
        if text == "\x1b" {
            app.mode = Mode::Passthrough;
            return Ok(());
        }
        // In display-panes mode, handle digit selection
        for c in text.chars() {
            if c.is_ascii_digit() {
                let idx = c.to_digit(10).unwrap() as usize;
                if let Some((_, path)) = app.display_map.iter().find(|(d, _)| *d == idx) {
                    let path = path.clone();
                    if let Some(win) = app.windows.get_mut(app.active_idx) {
                        win.active_path = path;
                    }
                }
                app.mode = Mode::Passthrough;
                return Ok(());
            }
        }
        return Ok(());
    }
    // In copy mode, interpret characters as copy-mode actions (never send to PTY)
    if matches!(app.mode, Mode::CopyMode) {
        for c in text.chars() {
            handle_copy_mode_char(app, c)?;
        }
        return Ok(());
    }
    // In copy-search mode, append characters to the search input
    if matches!(app.mode, Mode::CopySearch { .. }) {
        if let Mode::CopySearch { ref mut input, .. } = app.mode {
            for c in text.chars() {
                input.push(c);
            }
        }
        refresh_search_prompt(app);
        return Ok(());
    }

    // A focused floating pane (tmux new-pane) receives input instead of the
    // tiled active pane, while the layout keeps rendering underneath.
    {
        let win = &mut app.windows[app.active_idx];
        if let Some(fi) = win.floating_focus {
            if let Some(fp) = win.floating.get_mut(fi) {
                let _ = fp.pane.writer.write_all(text.as_bytes());
                let _ = fp.pane.writer.flush();
                return Ok(());
            }
        }
    }

    if app.sync_input {
        // Fan out to ALL panes in the current window
        let win = &mut app.windows[app.active_idx];
        fn write_all_panes(node: &mut Node, text: &[u8]) {
            match node {
                Node::Leaf(p) => { let _ = p.writer.write_all(text); let _ = p.writer.flush(); }
                Node::Split { children, .. } => { for c in children { write_all_panes(c, text); } }
            }
        }
        write_all_panes(&mut win.root, text.as_bytes());
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            let _ = p.writer.write_all(text.as_bytes());
            let _ = p.writer.flush();
        }
    }
    Ok(())
}

/// Dispatch a single character as a copy-mode action.
fn handle_copy_mode_char(app: &mut AppState, c: char) -> io::Result<()> {
    // User bindings from `bind -T copy-mode-vi` / `-T copy-mode` win over the
    // built-ins below.
    //
    // The live client/server path reaches copy mode here rather than through
    // `handle_key`, so check the table before pending state and built-ins.
    if let Some(action) = copy_mode_binding(app, (KeyCode::Char(c), KeyModifiers::NONE)) {
        if run_copy_mode_binding(app, &action) {
            return Ok(());
        }
    }
    // Handle text-object pending state (waiting for w/W after a/i)
    if let Some(prefix) = app.copy_text_object_pending.take() {
        match (prefix, c) {
            (0, 'w') => { crate::copy_mode::select_a_word(app); }
            (1, 'w') => { crate::copy_mode::select_inner_word(app); }
            (0, 'W') => { crate::copy_mode::select_a_word_big(app); }
            (1, 'W') => { crate::copy_mode::select_inner_word_big(app); }
            _ => {}
        }
        return Ok(());
    }
    // Handle find-char pending state (waiting for char after f/F/t/T).
    // Consume any numeric prefix so e.g. "3fx" jumps to the 3rd 'x' (#413).
    if let Some(pending) = app.copy_find_char_pending.take() {
        let n = app.copy_count.take().unwrap_or(1);
        for _ in 0..n {
            match pending {
                0 => crate::copy_mode::find_char_forward(app, c),
                1 => crate::copy_mode::find_char_backward(app, c),
                2 => crate::copy_mode::find_char_to_forward(app, c),
                3 => crate::copy_mode::find_char_to_backward(app, c),
                _ => {}
            }
        }
        return Ok(());
    }
    // Numeric prefix accumulation for vi-style motions (fixes #413). This path
    // handles copy-mode chars arriving via send-text (the route the client uses
    // for every plain printable key), which previously ignored counts entirely
    // so "5j" moved a single line instead of five.
    if c.is_ascii_digit() {
        let digit = c.to_digit(10).unwrap() as usize;
        if let Some(count) = app.copy_count {
            // Extend the pending count: "1" then "2" -> 12 (cap at 9999).
            app.copy_count = Some((count * 10 + digit).min(9999));
            return Ok(());
        } else if digit >= 1 {
            // Start a new count with 1-9.
            app.copy_count = Some(digit);
            return Ok(());
        }
        // digit == 0 with no existing count -> fall through to the '0'
        // line-start motion below.
    }
    // Any non-digit key consumes the pending count (default 1).
    let n = app.copy_count.take().unwrap_or(1);
    match c {
        'q' | ']' | '\x1b' => {
            exit_copy_mode(app);
        }
        'h' => { for _ in 0..n { move_copy_cursor(app, -1, 0); } }
        'l' => { for _ in 0..n { move_copy_cursor(app, 1, 0); } }
        'k' => { for _ in 0..n { move_copy_cursor(app, 0, -1); } }
        'j' => { for _ in 0..n { move_copy_cursor(app, 0, 1); } }
        'g' => { scroll_to_top(app); }
        'G' => { scroll_to_bottom(app); }
        'w' => { for _ in 0..n { crate::copy_mode::move_word_forward(app); } }
        'b' => { for _ in 0..n { crate::copy_mode::move_word_backward(app); } }
        'e' => { for _ in 0..n { crate::copy_mode::move_word_end(app); } }
        'W' => { for _ in 0..n { crate::copy_mode::move_word_forward_big(app); } }
        'B' => { for _ in 0..n { crate::copy_mode::move_word_backward_big(app); } }
        'E' => { for _ in 0..n { crate::copy_mode::move_word_end_big(app); } }
        // Shift+J / Shift+K scroll one line and leave the cursor alone. tmux binds
        // them only in copy-mode-vi (key-bindings.c:680 and :681), so they are vi
        // only here too. This is the path a real printable keypress takes on
        // Windows, so without an arm here the keys were dead (#596).
        'J' if app.mode_keys != "emacs" => { for _ in 0..n { scroll_copy_down(app, 1); } }
        'K' if app.mode_keys != "emacs" => { for _ in 0..n { scroll_copy_up(app, 1); } }
        'H' => { crate::copy_mode::move_to_screen_top(app); }
        'M' => { crate::copy_mode::move_to_screen_middle(app); }
        'L' => { crate::copy_mode::move_to_screen_bottom(app); }
        // Paragraph jumps and bracket matching (#498). These reached the
        // KeyCode table but not this one, and this is the path the client
        // actually uses for plain printable keys, so they were dead keys.
        '{' => { for _ in 0..n { crate::copy_mode::move_prev_paragraph(app); } }
        '}' => { for _ in 0..n { crate::copy_mode::move_next_paragraph(app); } }
        '%' => { crate::copy_mode::move_matching_bracket(app); }
        'z' => { crate::copy_mode::scroll_middle(app); }
        // Mark, jump repeat and live-refresh toggle (#498)
        'X' => { crate::copy_mode::set_mark(app); }
        ';' => { for _ in 0..n { crate::copy_mode::jump_again(app); } }
        ',' => { for _ in 0..n { crate::copy_mode::jump_reverse(app); } }
        'r' => { crate::copy_mode::toggle_refresh(app); }
        // Preserve the count so e.g. "3fx" finds the 3rd 'x' (consumed above).
        'f' => { app.copy_find_char_pending = Some(0); app.copy_count = Some(n); }
        'F' => { app.copy_find_char_pending = Some(1); app.copy_count = Some(n); }
        't' => { app.copy_find_char_pending = Some(2); app.copy_count = Some(n); }
        'T' => { app.copy_find_char_pending = Some(3); app.copy_count = Some(n); }
        'D' => { crate::copy_mode::copy_end_of_line(app)?; exit_copy_mode(app); }
        '0' => { crate::copy_mode::move_to_line_start(app); }
        '$' => { crate::copy_mode::move_to_line_end(app); }
        '^' => { crate::copy_mode::move_to_first_nonblank(app); }
        ' ' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
        }
        'v' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
        }
        'V' => {
            if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                app.copy_anchor = Some((r, c));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some((r, c));
                app.copy_selection_mode = crate::types::SelectionMode::Line;
            }
        }
        'o' => {
            if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                app.copy_anchor = Some(p);
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_pos = Some(a);
            }
        }
        'A' => {
            if let (Some(_), Some(_)) = (app.copy_anchor, app.copy_pos) {
                let prev = app.paste_buffers.first().cloned().unwrap_or_default();
                yank_selection(app)?;
                if let Some(buf) = app.paste_buffers.first_mut() {
                    let new_text = buf.clone();
                    *buf = format!("{}{}", prev, new_text);
                }
                exit_copy_mode(app);
            }
        }
        'y' => { yank_selection(app)?; exit_copy_mode(app); }
        '/' => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; refresh_search_prompt(app); }
        '?' => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; refresh_search_prompt(app); }
        'n' => { search_next(app); }
        'N' => { search_prev(app); }
        'i' => { app.copy_text_object_pending = Some(1); }  // inner text object
        'a' => { app.copy_text_object_pending = Some(0); }  // a text object
        _ => {} // Swallow unrecognized characters in copy mode
    }
    Ok(())
}

pub fn send_key_to_active(app: &mut AppState, k: &str) -> io::Result<()> {
    // In clock mode, any key exits back to passthrough
    if matches!(app.mode, Mode::ClockMode) {
        app.mode = Mode::Passthrough;
        return Ok(());
    }
    // Route named keys to active overlay (so CLI send-keys can interact with overlays)
    if matches!(app.mode, Mode::PopupMode { .. }) {
        // Map named keys to VT sequences for the popup PTY
        let seq = match k {
            "enter" => Some("\r"),
            "esc" | "escape" => {
                app.mode = Mode::Passthrough;
                return Ok(());
            }
            "tab" => Some("\t"),
            "backspace" | "bspace" => Some("\x7f"),
            "up" => Some("\x1b[A"),
            "down" => Some("\x1b[B"),
            "right" => Some("\x1b[C"),
            "left" => Some("\x1b[D"),
            "home" => Some("\x1b[H"),
            "end" => Some("\x1b[F"),
            "pageup" | "ppage" => Some("\x1b[5~"),
            "pagedown" | "npage" => Some("\x1b[6~"),
            "delete" | "dc" => Some("\x1b[3~"),
            "space" => Some(" "),
            _ => None,
        };
        if let Some(seq) = seq {
            if let Mode::PopupMode { ref mut popup_pane, .. } = app.mode {
                if let Some(ref mut pty) = popup_pane {
                    let _ = pty.writer.write_all(seq.as_bytes());
                    let _ = pty.writer.flush();
                }
            }
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::ConfirmMode { .. }) {
        match k {
            "esc" | "escape" => {
                app.mode = Mode::Passthrough;
            }
            _ => {} // y/n handled via send_text_to_active
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::MenuMode { .. }) {
        match k {
            "up" => {
                if let Mode::MenuMode { ref mut menu } = app.mode {
                    if menu.selected > 0 { menu.selected -= 1; }
                }
            }
            "down" => {
                if let Mode::MenuMode { ref mut menu } = app.mode {
                    let len = menu.items.len();
                    if menu.selected + 1 < len { menu.selected += 1; }
                }
            }
            "enter" => {
                if let Mode::MenuMode { ref menu } = app.mode {
                    if let Some(item) = menu.items.get(menu.selected) {
                        if !item.is_separator && !item.command.is_empty() {
                            let cmd = item.command.clone();
                            app.mode = Mode::Passthrough;
                            crate::config::parse_config_line(app, &cmd);
                            return Ok(());
                        }
                    }
                }
                app.mode = Mode::Passthrough;
            }
            "esc" | "escape" | "q" => {
                app.mode = Mode::Passthrough;
            }
            _ => {}
        }
        return Ok(());
    }
    if matches!(app.mode, Mode::PaneChooser { .. }) {
        match k {
            "esc" | "escape" => {
                app.mode = Mode::Passthrough;
            }
            _ => {}
        }
        return Ok(());
    }
    // --- Copy-search mode: handle esc/enter/backspace ---
    if matches!(app.mode, Mode::CopySearch { .. }) {
        match k {
            "esc" => { app.mode = Mode::CopyMode; app.status_message = None; }
            "enter" => {
                if let Mode::CopySearch { ref input, forward } = app.mode {
                    let query = input.clone();
                    let fwd = forward;
                    app.copy_search_query = query.clone();
                    app.copy_search_forward = fwd;
                    search_copy_mode(app, &query, fwd);
                    if !app.copy_search_matches.is_empty() {
                        let (r, c, _) = app.copy_search_matches[0];
                        app.copy_pos = Some((r, c));
                    }
                }
                app.mode = Mode::CopyMode;
                app.status_message = None;
            }
            "backspace" => {
                if let Mode::CopySearch { ref mut input, .. } = app.mode { input.pop(); }
                refresh_search_prompt(app);
            }
            _ => {}
        }
        return Ok(());
    }

    // --- Copy mode: full vi-style key table ---
    if matches!(app.mode, Mode::CopyMode) {
        // User `bind -T copy-mode-vi` / `-T copy-mode` bindings win over the
        // built-in defaults below. See handle_copy_mode_char for why this was
        // missing. `k` is a canonical key NAME here ("escape", "C-v", ...), so
        // it goes back through the same parser the config used to produce the
        // table's keys — that keeps one definition of what a key name means.
        if let Some((code, mods)) = crate::config::parse_key_string(k) {
            if let Some(action) = copy_mode_binding(app, (code, mods)) {
                if run_copy_mode_binding(app, &action) {
                    return Ok(());
                }
            }
        }
        match k {
            "esc" | "q" => {
                exit_copy_mode(app);
            }
            "enter" => {
                // Copy selection and exit copy mode (vi Enter)
                if app.copy_anchor.is_some() {
                    yank_selection(app)?;
                }
                exit_copy_mode(app);
            }
            "space" => {
                // Begin selection (like v in vi mode)
                if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                    app.copy_anchor = Some((r, c));
                    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                    app.copy_pos = Some((r, c));
                    app.copy_selection_mode = crate::types::SelectionMode::Char;
                }
            }
            "up" => { move_copy_cursor(app, 0, -1); }
            "down" => { move_copy_cursor(app, 0, 1); }
            "pageup" => { scroll_copy_up(app, 10); }
            "pagedown" => { scroll_copy_down(app, 10); }
            "left" => { move_copy_cursor(app, -1, 0); }
            "right" => { move_copy_cursor(app, 1, 0); }
            "home" => { crate::copy_mode::move_to_line_start(app); }
            "end" => { crate::copy_mode::move_to_line_end(app); }
            "C-b" | "c-b" => {
                if app.mode_keys == "emacs" { move_copy_cursor(app, -1, 0); }
                else { scroll_copy_up(app, 10); }
            }
            "C-f" | "c-f" => {
                if app.mode_keys == "emacs" { move_copy_cursor(app, 1, 0); }
                else { scroll_copy_down(app, 10); }
            }
            // tmux copy-mode C-p/C-n are cursor motions (key-bindings.c:571/:572),
            // and copy-mode-vi does not bind them at all. psmux keeps them active in
            // both mode-keys tables, but as CURSOR motion, matching tmux (#596).
            "C-n" | "c-n" => { move_copy_cursor(app, 0, 1); }
            "C-p" | "c-p" => { move_copy_cursor(app, 0, -1); }
            // Ctrl+Up / Ctrl+Down scroll the viewport one line without moving the
            // cursor. tmux binds these in BOTH copy-mode (key-bindings.c:635/:636)
            // and copy-mode-vi (:727/:728); psmux had no arm at all, so a real
            // Ctrl+Arrow in copy mode was silently swallowed (#596).
            "C-Up" | "c-up" | "C-up" => { scroll_copy_up(app, 1); }
            "C-Down" | "c-down" | "C-down" => { scroll_copy_down(app, 1); }
            "C-a" | "c-a" => { crate::copy_mode::move_to_line_start(app); }
            // The one place the two tmux copy tables really disagree: copy-mode
            // binds C-e to end-of-line and leaves C-y unbound, copy-mode-vi binds
            // C-e to scroll-down and C-y to scroll-up (key-bindings.c:645 and
            // :653). vi mode follows the vi table, and `$` is the vi way to reach
            // the end of a line (#596).
            "C-e" | "c-e" => {
                if app.mode_keys == "emacs" { crate::copy_mode::move_to_line_end(app); }
                else { scroll_copy_down(app, 1); }
            }
            "C-y" | "c-y" => {
                if app.mode_keys != "emacs" { scroll_copy_up(app, 1); }
            }
            "C-v" | "c-v" => { scroll_copy_down(app, 10); }
            "M-v" | "m-v" => { scroll_copy_up(app, 10); }
            "M-f" | "m-f" => { crate::copy_mode::move_word_forward(app); }
            "M-b" | "m-b" => { crate::copy_mode::move_word_backward(app); }
            "M-w" | "m-w" => { yank_selection(app)?; exit_copy_mode(app); }
            // jump-to-mark (#498). Alt keys reach copy mode as named keys via
            // `send-key M-x`, not through the char table, so this is the arm
            // that actually runs when the user presses M-x.
            "M-x" | "m-x" => { crate::copy_mode::jump_to_mark(app); }
            "C-s" | "c-s" => { app.mode = Mode::CopySearch { input: String::new(), forward: true }; refresh_search_prompt(app); }
            "C-r" | "c-r" => { app.mode = Mode::CopySearch { input: String::new(), forward: false }; refresh_search_prompt(app); }
            "C-c" | "c-c" => {
                exit_copy_mode(app);
            }
            "C-g" | "c-g" => {
                exit_copy_mode(app);
            }
            "c-space" | "C-space" => {
                // Set mark (anchor) at current position
                if let Some((r, c)) = crate::copy_mode::get_copy_pos(app) {
                    app.copy_anchor = Some((r, c));
                    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                    app.copy_pos = Some((r, c));
                }
            }
            "C-u" | "c-u" => {
                let half = app.windows.get(app.active_idx)
                    .and_then(|w| active_pane(&w.root, &w.active_path))
                    .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                scroll_copy_up(app, half);
            }
            "C-d" | "c-d" => {
                let half = app.windows.get(app.active_idx)
                    .and_then(|w| active_pane(&w.root, &w.active_path))
                    .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                scroll_copy_down(app, half);
            }
            _ => {}
        }
        return Ok(());
    }
    
    // Write a named key to a single pane (extracted for sync_input support).
    fn write_named_key_to_pane(p: &mut crate::types::Pane, k: &str) {
        use std::io::Write as _;
        match k {
            "enter" => write_key_seq(p, b"\r"),
            "tab" => write_key_seq(p, b"\t"),
            "btab" | "backtab" => write_key_seq(p, b"\x1b[Z"),
            "backspace" => write_key_seq(p, b"\x7f"),
            "delete" => write_key_seq(p, b"\x1b[3~"),
            "esc" => write_key_seq(p, b"\x1b"),
            "up" => write_key_seq(p, b"\x1b[A"),
            "down" => write_key_seq(p, b"\x1b[B"),
            "right" => write_key_seq(p, b"\x1b[C"),
            "left" => write_key_seq(p, b"\x1b[D"),
            "home" => write_key_seq(p, b"\x1b[H"),
            "end" => write_key_seq(p, b"\x1b[F"),
            "pageup" => write_key_seq(p, b"\x1b[5~"),
            "pagedown" => write_key_seq(p, b"\x1b[6~"),
            "insert" => write_key_seq(p, b"\x1b[2~"),
            "space" => write_key_seq(p, b" "),
            s if s.starts_with("f") && s.len() >= 2 && s.len() <= 3 => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    let seq = match n {
                        1 => "\x1bOP",
                        2 => "\x1bOQ",
                        3 => "\x1bOR",
                        4 => "\x1bOS",
                        5 => "\x1b[15~",
                        6 => "\x1b[17~",
                        7 => "\x1b[18~",
                        8 => "\x1b[19~",
                        9 => "\x1b[20~",
                        10 => "\x1b[21~",
                        11 => "\x1b[23~",
                        12 => "\x1b[24~",
                        _ => "",
                    };
                    if !seq.is_empty() { let _ = write!(p.writer, "{}", seq); }
                }
            }
            // Ctrl+Shift+<letter>: inject a native KEY_EVENT carrying BOTH the
            // Ctrl and Shift modifier flags so console-input apps (ReadConsoleInputW)
            // can distinguish it from a plain Ctrl+<letter> (issue #368: psmux used
            // to collapse Ctrl+Shift+V to Ctrl+V, stripping Shift).  We deliberately
            // do NOT also write the raw C0 byte: emitting both double-delivers the
            // key (issue #363).  VT-pipe apps still receive ConPTY's translation of
            // this single event.
            s if (s.starts_with("C-S-") || s.starts_with("c-s-"))
                && s.chars().count() == 5
                && s.chars().nth(4).map_or(false, |c| c.is_ascii_alphabetic()) =>
            {
                let c = s.chars().nth(4).unwrap().to_ascii_lowercase();
                let ctrl_char = (c as u8) & 0x1F;
                #[cfg(windows)]
                {
                    let injected = if let Some(pid) = p.child_pid {
                        crate::platform::mouse_inject::send_modified_key_event(pid, c, true, false, true)
                    } else {
                        false
                    };
                    if !injected {
                        // No child pid / injection failed: fall back to the raw
                        // control byte (Shift unrepresentable, but key still arrives).
                        let _ = p.writer.write_all(&[ctrl_char]);
                    }
                }
                #[cfg(not(windows))]
                {
                    // Non-Windows: no console-input modifier channel; legacy VT has
                    // no distinct Ctrl+Shift+<letter> encoding, so deliver the byte.
                    let _ = p.writer.write_all(&[ctrl_char]);
                }
            }
            // Ctrl+Shift+<punctuation/digit> that collapses to a single C0 byte.
            // ConPTY terminals (Alacritty, WezTerm) deliver Ctrl+/ as VK_OEM_MINUS
            // with Ctrl+Shift, so the client forwards it as "C-S--".  That must
            // reach the child as 0x1f (^_) — identical to Ctrl+_ and to tmux — so
            // neovim's Ctrl+/ comment-toggle fires.  Before this branch, "C-S--"
            // matched neither the alphabetic C-S- arm nor the len==3 C- arm and
            // was silently dropped (issue #394).  No native KEY_EVENT injection:
            // the raw byte is enough (ConPTY reconstructs the key) and injecting
            // both would double-deliver (issue #363).
            s if (s.starts_with("C-S-") || s.starts_with("c-s-"))
                && s.chars().count() == 5
                && s.chars().nth(4).map_or(false, |c| !c.is_ascii_alphabetic()) =>
            {
                let c = s.chars().nth(4).unwrap();
                if let Some(byte) = ctrl_char_send_keys_byte(c) {
                    let _ = p.writer.write_all(&[byte]);
                    let _ = p.writer.flush();
                }
            }
            // Ctrl+Break (issue #454).  The attached client traps the Ctrl+Break
            // console signal — which Windows never delivers as a keystroke — and
            // forwards it here as `send-key C-Break` so it interrupts the running
            // program instead of tearing down the client / session.
            //
            // We deliver a GENUINE CTRL_BREAK_EVENT to the pane's ConPTY child,
            // exactly like Windows Terminal.  Ctrl+Break exists to stop a program
            // that *ignores* Ctrl+C, so the old Ctrl+C path (raw 0x03 + a
            // CTRL_C_EVENT) was not enough — a Ctrl+C-immune program survived it.
            // Attaching to the child's console places us in its process group and
            // the broadcast reaches the child (proven to kill a Ctrl+C-immune
            // program); the server survives via its own CTRL_BREAK-surviving
            // console handler.  See platform::mouse_inject::send_ctrl_break_event.
            "break" | "Break" | "C-Break" | "c-break" | "C-break" => {
                #[cfg(windows)]
                if let Some(pid) = p.child_pid {
                    crate::platform::mouse_inject::send_ctrl_break_event(pid, false);
                }
            }
            s if s.starts_with("C-") && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('c');
                // tmux-parity mapping so C-/ -> 0x1f (^_), not the naive '/' & 0x1f
                // == 0x0f (^O) collision with C-o (issue #226/#394).  Letters keep
                // their usual byte (a->0x01 …), so Ctrl+<letter> is unaffected.
                let ctrl_char = ctrl_char_send_keys_byte(c)
                    .unwrap_or((c.to_ascii_lowercase() as u8) & 0x1F);
                // Always write the raw control byte so ConPTY can generate
                // console control events (e.g. CTRL_C_EVENT for \x03).
                // Raw bytes do NOT start with \x1b so they never corrupt
                // ConPTY's VT parser state.
                //
                // ConPTY already reconstructs the proper VK + LEFT_CTRL_PRESSED
                // console key event from this raw C0 byte, so console apps
                // (PSReadLine, neovim) receive a correct Ctrl+<letter> event
                // from the byte alone.  We must therefore NOT also inject a
                // separate KEY_EVENT via WriteConsoleInputW for letters — doing
                // both delivered the key TWICE (issue #363: a single <C-w>
                // arrived as <C-w><C-w>, turning neovim's window command into a
                // no-op and making `<C-w>s` behave like a bare `s`; PSReadLine's
                // Ctrl+W likewise deleted two words instead of one).
                // Ctrl+C is the sole exception: the interrupt router runs so the
                // child's console handler can be signalled (SIGINT parity, #338)
                // — BEFORE the byte, because when the router decides "raw 0x03
                // only" it may strip PROCESSED_INPUT from the pane console so
                // conhost delivers the byte as input instead of converting it
                // into a console-wide CTRL_C_EVENT that aborts a booting WSL
                // launch (#579). Writing first loses that race.
                #[cfg(windows)]
                if c.eq_ignore_ascii_case(&'c') {
                    if let Some(pid) = p.child_pid {
                        crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                    }
                }
                let _ = p.writer.write_all(&[ctrl_char]);
                let _ = p.writer.flush();
            }
            s if (s.starts_with("M-") || s.starts_with("m-")) && s.len() == 3 => {
                let c = s.chars().nth(2).unwrap_or('a');
                // Try native console injection (WriteConsoleInputW with LEFT_ALT_PRESSED)
                // first.  ConPTY does NOT reassemble ESC+char into Alt+key events, so
                // PSReadLine Alt+f/Alt+b/etc. won't work via the VT path.
                let injected = if let Some(pid) = p.child_pid {
                    crate::platform::mouse_inject::send_alt_key_event(pid, c)
                } else {
                    false
                };
                if !injected {
                    // Fallback: VT encoding (ESC + char) — works for VT-native apps
                    let _ = write!(p.writer, "\x1b{}", c);
                }
            }
            s if (s.starts_with("C-M-") || s.starts_with("c-m-")) && s.len() == 5 => {
                let c = s.chars().nth(4).unwrap_or('c');
                // Try native console injection (WriteConsoleInputW with
                // LEFT_CTRL_PRESSED | LEFT_ALT_PRESSED).  ConPTY does NOT
                // reassemble ESC + ctrl-char into Ctrl+Alt+key.
                let injected = if let Some(pid) = p.child_pid {
                    crate::platform::mouse_inject::send_modified_key_event(pid, c, true, true, false)
                } else {
                    false
                };
                if !injected {
                    let ctrl_char = (c.to_ascii_lowercase() as u8) & 0x1F;
                    let _ = p.writer.write_all(&[0x1b, ctrl_char]);
                }
            }
            // Modified Enter: for Ctrl combos, try native console injection
            // (WriteConsoleInputW) so PSReadLine sees the correct modifier flags.
            // For Shift/Alt-only combos, use VT encoding to avoid ConPTY
            // translating the injected KEY_EVENT back to plain \r (double Enter).
            #[cfg(windows)]
            s if {
                let u = s.to_uppercase();
                let r = u.trim_start_matches("C-").trim_start_matches("M-").trim_start_matches("S-");
                r == "ENTER" || r == "RETURN" || r == "CR"
            } => {
                let upper = s.to_uppercase();
                let has_shift = upper.contains("S-");
                let has_ctrl = upper.contains("C-");
                let has_alt = upper.contains("M-");
                let injected = if has_ctrl {
                    // Only use native injection for Ctrl combos.  For plain
                    // Ctrl+Enter this carries an LF payload (#409); Ctrl+Shift /
                    // Ctrl+Alt keep CR.
                    if let Some(pid) = p.child_pid {
                        crate::platform::mouse_inject::send_modified_enter_event(pid, has_ctrl, has_alt, has_shift)
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !injected {
                    if has_ctrl && !has_shift && !has_alt {
                        // Fallback: plain Ctrl+Enter is LF, matching WT (#409).
                        let _ = p.writer.write_all(b"\n");
                    } else if (has_shift || has_alt) && !has_ctrl {
                        // Fallback: ESC + CR for VT-native apps (Claude Code, etc.)
                        let _ = p.writer.write_all(b"\x1b\r");
                    } else {
                        // Ctrl+Shift/Ctrl+Alt+Enter and other combos: CSI encoding
                        if let Some(seq) = parse_modified_special_key(s) {
                            let _ = p.writer.write_all(seq.as_bytes());
                        }
                    }
                }
            }
            // Modifier + special key combos: C-Left, S-Right, C-S-Up, C-M-Home, etc.
            s if parse_modified_special_key(s).is_some() => {
                let seq = parse_modified_special_key(s).unwrap();
                let _ = p.writer.write_all(seq.as_bytes());
            }
            _ => {}
        }
        let _ = p.writer.flush();
    }

    // A focused floating pane (tmux new-pane) receives the key instead of the
    // tiled active pane.
    {
        let win = &mut app.windows[app.active_idx];
        if let Some(fi) = win.floating_focus {
            if let Some(fp) = win.floating.get_mut(fi) {
                write_named_key_to_pane(&mut fp.pane, k);
                return Ok(());
            }
        }
    }

    // Distribute the key to all panes (sync) or just the active pane.
    if app.sync_input {
        let win = &mut app.windows[app.active_idx];
        fn send_key_all_panes(node: &mut crate::types::Node, k: &str) {
            match node {
                crate::types::Node::Leaf(p) => write_named_key_to_pane(p, k),
                crate::types::Node::Split { children, .. } => {
                    for c in children { send_key_all_panes(c, k); }
                }
            }
        }
        send_key_all_panes(&mut win.root, k);
    } else {
        let win = &mut app.windows[app.active_idx];
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            write_named_key_to_pane(p, k);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests-rs/test_input.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_issue226_ctrl_slash.rs"]
mod tests_issue226_ctrl_slash;

#[cfg(test)]
#[path = "../tests-rs/test_issue394_ctrl_slash_conpty.rs"]
mod tests_issue394_ctrl_slash_conpty;

#[cfg(test)]
#[path = "../tests-rs/test_issue284_pageup_wsl.rs"]
mod tests_issue284_pageup_wsl;

#[cfg(test)]
#[path = "../tests-rs/test_pane_last_text_input.rs"]
mod tests_pane_last_text_input;

#[cfg(test)]
#[path = "../tests-rs/test_pane_last_special_key.rs"]
mod tests_pane_last_special_key;

#[cfg(test)]
#[path = "../tests-rs/test_issue413_copy_count.rs"]
mod tests_issue413_copy_count;

#[cfg(test)]
#[path = "../tests-rs/test_copy_mode_key_tables.rs"]
mod tests_copy_mode_key_tables;

#[cfg(test)]
#[path = "../tests-rs/test_issue596_copy_scroll_keys.rs"]
mod tests_issue596_copy_scroll_keys;

#[cfg(test)]
#[path = "../tests-rs/test_issue610_ctrl_backspace.rs"]
mod tests_issue610_ctrl_backspace;
