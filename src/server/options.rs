use crate::types::AppState;
use crate::config::{format_key_binding, parse_key_string};
use crate::server::option_catalog::WINDOW_OPTION_NAMES;

/// Upper bound for `repeat-time`, in milliseconds. tmux declares the option as
/// a number with minimum 0 and maximum 2000000 in options-table.c, so psmux
/// refuses anything outside that range on every route (#606).
pub(crate) const REPEAT_TIME_MAX_MS: i64 = 2_000_000;

/// Split a `codepoint-widths` option value into its array entries.
///
/// tmux declares the option `OPTIONS_TABLE_IS_ARRAY` with `.separator = ","`,
/// so a value written as one string is split on commas into array items
/// (`options_array_assign` in options.c). Empty items are dropped, matching
/// tmux's own skip of an empty element.
pub(crate) fn split_codepoint_widths(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Push the stored `codepoint-widths` array into the process-global width
/// table that every width decision reads.
///
/// This is tmux's `utf8_update_width_cache()`, which `options.c` calls from
/// the option-changed hook (`if (strcmp(name, "codepoint-widths") == 0)`) so a
/// live `set -s codepoint-widths ...` affects the very next character drawn
/// rather than waiting for a server restart.
pub(crate) fn sync_codepoint_widths(app: &AppState) {
    vt100::set_codepoint_widths(&app.codepoint_widths);
}

/// Replace the whole `codepoint-widths` array and rebuild the width table.
/// `warm-pool-size N`: how many spare shells to keep pre spawned.
///
/// The number that matters is not "is there a spare" but "has the spare
/// finished starting". One spare guarantees the first creation is instant and
/// guarantees nothing about the second, because the second gets the refill the
/// first triggered, which is a shell that started moments ago. Depth is the
/// only cure. Clamped to [`WARM_POOL_SIZE_MAX`] because every spare is a real
/// process; `0` disables the pool, matching `set -g warm off` / `PSMUX_NO_WARM`.
/// A value that is not a number leaves the pool untouched, as with every other
/// numeric option here.
pub(crate) fn set_warm_pool_size(app: &mut AppState, value: &str) {
    let Ok(n) = value.trim().parse::<usize>() else { return };
    let n = n.min(crate::types::WARM_POOL_SIZE_MAX);
    app.warm_pane.target = n;
    if n == 0 {
        app.warm_pane.kill_all();
    }
    // Shrinking below the current depth: drop the surplus now rather than
    // waiting for creations to drain it, so the memory the user just asked to
    // give back is actually given back. Newest first, keeping the spares whose
    // shells are furthest through starting.
    app.warm_pane.trim_to(n);
    crate::warm_trace!("pool: target set to {} (depth={})", n, app.warm_pane.len());
}

pub(crate) fn set_codepoint_widths(app: &mut AppState, value: &str) {
    app.codepoint_widths = split_codepoint_widths(value);
    sync_codepoint_widths(app);
}

/// Append entries to the `codepoint-widths` array (`set -sa`) and rebuild.
pub(crate) fn append_codepoint_widths(app: &mut AppState, value: &str) {
    app.codepoint_widths.extend(split_codepoint_widths(value));
    sync_codepoint_widths(app);
}

/// Parse a main-pane-width / main-pane-height value.
///
/// psmux stores both as a percentage (see AppState::main_pane_width), and tmux
/// documents the percentage spelling for them (options-table.c:1474, "This may
/// be a percentage, for example '10%'"), so a trailing `%` is accepted and
/// dropped. A value that is not a number at all leaves the current setting
/// alone, which is what both setters did before.
pub(crate) fn parse_main_pane_size(value: &str) -> Option<u16> {
    value.trim().trim_end_matches('%').parse::<u16>().ok()
}

pub(crate) fn is_window_option(name: &str) -> bool {
    WINDOW_OPTION_NAMES.contains(&name)
}

/// True when a `-w` write for `name` belongs on the WINDOW rather than in the
/// global store (#648).
///
/// tmux decides scope from the option NAME, not from the flag:
/// `options_scope_from_name` looks the name up in the options table and uses
/// the scope declared there, so `set -w status-left x` in tmux still writes the
/// session option. psmux follows the same rule, so `-w` on a session or server
/// option keeps behaving exactly as it did.
///
/// User options (`@...`) are deliberately NOT included. tmux gives them the
/// scope of the flags because they carry no table entry, but every psmux read
/// of an `@` name goes through the session-wide `user_options` map — the
/// format expander (`#{@k}`), the pane `@mouse-force` latch, the plugin drain —
/// and none of those has a window to resolve against. Scoping the WRITE
/// without the reads would store the value where nothing could ever find it,
/// which is the silent no-op #580 exists to prevent. `set -wg @k v` and
/// `set -g @k v` therefore remain the way to set one.
pub(crate) fn is_window_scoped_write(name: &str) -> bool {
    is_window_option(name)
}

/// Resolve a raw `-t` target to the position of an existing window.
///
/// Every window-scoped option route funnels through here so `-t s:zero`,
/// `-t s:1`, `-t @3`, `-t :$` and a bare `-t s` all mean the same window they
/// mean for `select-window`. The route this replaces kept only the numeric
/// half of the parsed target and threw the NAME away, so `-t "s:zero"`
/// resolved to the active window and the reporter's targeted write landed
/// somewhere else entirely (#648).
///
/// An empty target is the active window, which is what tmux does when `-t` is
/// omitted (cmd-find.c falls back to the current window).
pub(crate) fn resolve_option_target_window(app: &AppState, raw: &str) -> Result<usize, String> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return Ok(app.active_idx);
    }
    // A pane target (`%N`, or `sess:win.pane`) names the window that holds it,
    // exactly as tmux's window resolution does for a pane spec.
    let parsed = crate::cli::parse_target(raw);
    if parsed.pane_is_id && parsed.window.is_none() && parsed.window_name.is_none() {
        if let Some(pane_id) = parsed.pane {
            return crate::tree::find_pane_by_id_global(app, pane_id)
                .map(|(window_index, _)| window_index)
                .ok_or_else(|| format!("can't find pane: %{}", pane_id));
        }
    }
    // `-t <session>` with no window part is the session's CURRENT window
    // (cmd-find.c: a session-only target resolves to `s->curw`). Without this
    // the bare session spelling every existing caller uses — `show-options -w
    // -v -t mysession automatic-rename` — would be read as a window NAMED
    // "mysession" and refused.
    if parsed.window.is_none() && parsed.window_name.is_none() && parsed.pane.is_none() {
        return Ok(app.active_idx);
    }
    app.resolve_window_spec(raw, false)?
        .pos()
        .ok_or_else(|| format!("can't find window: {}", raw))
}

/// The window's OWN value for `name`, or `None` when it inherits.
///
/// Borrowing on purpose: the render loop resolves several of these per frame
/// (`window-status-format` once per window, `remain-on-exit` once per reap)
/// and the overwhelmingly common answer is "this window set nothing", which
/// must not cost an allocation. Callers that already hold the typed global in
/// an `AppState` field use this and fall back to the field.
pub(crate) fn window_local_option<'a>(
    app: &'a AppState,
    window_index: usize,
    name: &str,
) -> Option<&'a str> {
    app.windows
        .get(window_index)?
        .window_options
        .get(name)
        .map(String::as_str)
}

/// Effective value of a window option for one window: the window's own entry,
/// else whatever the global store reports (`get_option_value`).
///
/// This is tmux's `options_get`, which walks from the window's table up to
/// `global_w_options` (options.c), and it is what `show-options -w` reports.
pub(crate) fn resolve_window_option(app: &AppState, window_index: usize, name: &str) -> String {
    if let Some(value) = window_local_option(app, window_index, name) {
        return value.to_string();
    }
    get_option_value(app, name)
}

/// Boolean form of [`resolve_window_option`] for a caller that already holds
/// the global as a typed `AppState` field. No allocation when the window
/// inherits, which is every window until someone writes to one.
pub(crate) fn window_flag(
    app: &AppState,
    window_index: usize,
    name: &str,
    global: bool,
) -> bool {
    match window_local_option(app, window_index, name) {
        Some(value) => matches!(value, "on" | "true" | "1" | "yes"),
        None => global,
    }
}

/// [`window_flag`] for a caller that already holds the `Window`, so a loop
/// that iterates `app.windows` mutably can resolve inside the loop instead of
/// collecting every window's answer into a per-tick `Vec` first. No
/// allocation on either path.
pub(crate) fn win_flag(window: &crate::types::Window, name: &str, global: bool) -> bool {
    match window.window_options.get(name).map(String::as_str) {
        Some(value) => matches!(value, "on" | "true" | "1" | "yes"),
        None => global,
    }
}

/// [`window_number`] for a caller that already holds the `Window`.
pub(crate) fn win_number(window: &crate::types::Window, name: &str, global: u64) -> u64 {
    window
        .window_options
        .get(name)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(global)
}

/// Numeric form of [`resolve_window_option`]. A window-local value that does
/// not parse falls back to the global, the same way every numeric setter in
/// this file leaves the current setting alone on a bad value.
pub(crate) fn window_number(
    app: &AppState,
    window_index: usize,
    name: &str,
    global: u64,
) -> u64 {
    window_local_option(app, window_index, name)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(global)
}

/// Write one window-scoped option onto ONE window (`set-option -w -t <window>`).
///
/// Returns the reply string the CLI prints: empty on success, `ERROR: ...`
/// otherwise. A name that is not window scoped is reported rather than
/// silently stored, because a stored no-op is exactly the failure mode #580
/// fixed for pane options.
pub(crate) fn set_window_option(
    app: &mut AppState,
    window_index: usize,
    name: &str,
    value: &str,
) -> Result<(), String> {
    crate::server::option_catalog::validate_option_value(name, value)?;
    let Some(window) = app.windows.get_mut(window_index) else {
        return Err(format!("can't find window: {}", window_index));
    };
    window.window_options.insert(name.to_string(), value.to_string());
    Ok(())
}

/// The whole of `set-option -w [-u|-a|-o] [-q] -t <window> <name> [value]`,
/// shared by the TCP/CLI route, the in-TUI command prompt and the config file
/// so all three land the same write (#648).
///
/// Returns the reply the caller prints: empty on success, `ERROR: ...`
/// otherwise. Never a silent stored no-op — a swallowed option write looks
/// exactly like success to a script that only reads exit codes, which is the
/// failure #580 fixed for pane options and #648 for window ones.
pub(crate) fn apply_set_window_option(
    app: &mut AppState,
    target: &str,
    option: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let index = match resolve_option_target_window(app, target) {
        Ok(index) => index,
        Err(error) => return format!("ERROR: {}", error),
    };
    if unset {
        unset_window_option(app, index, option);
        // `automatic-rename` predates the window store and also answers from
        // `Window::manual_rename` (#266). Unsetting the option has to clear
        // that flag too, otherwise the window keeps reporting `off` from a
        // second, invisible per window store.
        if let Some(window) = app.windows.get_mut(index) {
            match option {
                "automatic-rename" => window.manual_rename = false,
                "window-size" => window.window_size = None,
                _ => {}
            }
        }
        return String::new();
    }
    if only_if_unset && window_local_option(app, index, option).is_some() {
        if quiet {
            return String::new();
        }
        return format!("ERROR: already set: {}", option);
    }
    let value = if append {
        format!("{}{}", resolve_window_option(app, index, option), value)
    } else {
        value.to_string()
    };
    if let Err(error) = set_window_option(app, index, option, &value) {
        return format!("ERROR: {}", error);
    }
    // Side effects the global setter performs, applied to the targeted window
    // only. `set -w automatic-rename on` re-arms the rename loop for THIS
    // window the way `set -g` does for the active one (options.rs
    // apply_set_option), and `window-size` keeps its dedicated field so
    // resize-window and the layout code keep reading one value.
    match option {
        "automatic-rename" => {
            if matches!(value.as_str(), "on" | "true" | "1" | "yes") {
                if let Some(window) = app.windows.get_mut(index) {
                    window.manual_rename = false;
                }
            }
        }
        "window-size" => {
            if let Some(window) = app.windows.get_mut(index) {
                window.window_size = Some(value.clone());
            }
        }
        _ => {}
    }
    String::new()
}

/// Remove one window's own value so it inherits again (`set-option -w -u`).
///
/// tmux's `-u` at window scope is `options_remove`, NOT a write of the table
/// default: the entry goes away and the window follows `global_w_options`
/// from then on (options.c `options_remove_or_default`, which only defaults
/// when the table being edited IS one of the global ones).
pub(crate) fn unset_window_option(app: &mut AppState, window_index: usize, name: &str) {
    if let Some(window) = app.windows.get_mut(window_index) {
        window.window_options.remove(name);
    }
}

/// Effective value of an option that is stored empty or not stored at all.
///
/// Several options live in `user_options` (or are simply left blank at startup)
/// and the code that consumes them substitutes a built-in when the entry is
/// missing. `show-options` has to report that built-in, otherwise it tells the
/// user an option is unset when the feature is demonstrably active, and
/// customize-mode shows a "default" the running session does not have.
fn effective_when_unset(name: &str) -> Option<&'static str> {
    Some(match name {
        // Consumed by the client renderer; missing means border_lines::DEFAULT.
        "pane-border-lines" => crate::border_lines::DEFAULT,
        "pane-border-indicators" => crate::pane_border::INDICATORS_DEFAULT,
        // Consumed at src/server/helpers.rs, a missing entry disables the gutter.
        "copy-mode-line-numbers" => "off",
        "copy-mode-line-number-style" => "fg=brightblack",
        "copy-mode-current-line-number-style" => "fg=yellow,bold",
        // TERM handed to panes when default-terminal was never set.
        "default-terminal" => "xterm-256color",
        // The pane borders are the exception to the `default` rule below. tmux
        // gives them a real default style in options-table.c (:1605
        // `fg=themelightgrey`, :1540 `themegreen`) and `show -g` prints that
        // style, not the word `default`. psmux renders the same pair from
        // client::pane_border_default_style, so reporting `default` here
        // claimed a terminal default foreground the border never had, and the
        // option catalog copied that claim into the value `-u` restores (#626).
        "pane-border-style" => "fg=brightblack",
        "pane-active-border-style" => "fg=green",
        // tmux reports `default` for a style that has not been overridden.
        "status-style" | "status-left-style" | "status-right-style"
        | "message-style" | "message-command-style" | "mode-style"
        | "pane-border-hover-style" | "window-status-style"
        | "window-status-current-style" | "window-status-activity-style"
        | "window-status-bell-style" | "window-status-last-style" => "default",
        _ => return None,
    })
}

/// Get a single option's value by name (for `show-options -v name`).
pub(crate) fn get_option_value(app: &AppState, name: &str) -> String {
    let value = match name {
        "prefix" => format_key_binding(&app.prefix_key),
        "prefix2" => app.prefix2_key.as_ref().map(|k| format_key_binding(k)).unwrap_or_else(|| "none".to_string()),
        "base-index" => app.window_base_index.to_string(),
        "pane-base-index" => app.pane_base_index.to_string(),
        "escape-time" => app.escape_time_ms.to_string(),
        "mouse" => if app.mouse_enabled { "on".into() } else { "off".into() },
        "bold-is-bright" => if app.bold_is_bright { "on".into() } else { "off".into() },
        "scroll-enter-copy-mode" => if app.scroll_enter_copy_mode { "on".into() } else { "off".into() },
        "mouse-drag-enter-copy-mode" => if app.mouse_drag_enter_copy_mode { "on".into() } else { "off".into() },
        "pwsh-mouse-selection" => if app.pwsh_mouse_selection { "on".into() } else { "off".into() },
        "mouse-selection" => if app.mouse_selection { "on".into() } else { "off".into() },
        "mouse-selection-force" => if app.mouse_selection_force { "on".into() } else { "off".into() },
        "paste-detection" => if app.paste_detection { "on".into() } else { "off".into() },
        "choose-tree-preview" => if app.choose_tree_preview { "on".into() } else { "off".into() },
        "status" => {
            if !app.status_visible { "off".into() }
            else if app.status_lines >= 2 { app.status_lines.to_string() }
            else { "on".into() }
        }
        "status-position" => app.status_position.clone(),
        "status-left" => app.status_left.clone(),
        "status-right" => app.status_right.clone(),
        "history-limit" => app.history_limit.to_string(),
        "display-time" => app.display_time_ms.to_string(),
        "display-panes-time" => app.display_panes_time_ms.to_string(),
        "mode-keys" => app.mode_keys.clone(),
        "focus-events" => if app.focus_events { "on".into() } else { "off".into() },
        "renumber-windows" => if app.renumber_windows { "on".into() } else { "off".into() },
        "automatic-rename" => if app.automatic_rename { "on".into() } else { "off".into() },
        "allow-rename" => if app.allow_rename { "on".into() } else { "off".into() },
        "allow-set-title" => if app.allow_set_title { "on".into() } else { "off".into() },
        "monitor-activity" => if app.monitor_activity { "on".into() } else { "off".into() },
        "visual-activity" => if app.visual_activity { "on".into() } else { "off".into() },
        "aggressive-resize" => if app.aggressive_resize { "on".into() } else { "off".into() },
        "synchronize-panes" => if app.sync_input { "on".into() } else { "off".into() },
        "remain-on-exit" => if app.remain_on_exit { "on".into() } else { "off".into() },
        "destroy-unattached" => if app.destroy_unattached { "on".into() } else { "off".into() },
        "exit-empty" => if app.exit_empty { "on".into() } else { "off".into() },
        "set-titles" => if app.set_titles { "on".into() } else { "off".into() },
        // Report the format that actually drives the host title. An empty stored
        // value means "use the built-in", so report the built-in, not a blank.
        "set-titles-string" => if app.set_titles_string.is_empty() {
            "#S:#I:#W".to_string()
        } else {
            app.set_titles_string.clone()
        },
        // repeat-time had no arm at all, so `show-options -v repeat-time`
        // returned an empty string even though the option works.
        "repeat-time" => app.repeat_time_ms.to_string(),
        // Reports what the server process is ACTUALLY running at, which is not
        // always what a config file asked for: PSMUX_PRIORITY outranks the
        // option, and the startup resolve stores the winner here (#608).
        "priority" => app.priority.clone(),
        "prediction-dimming" => if app.prediction_dimming { "on".into() } else { "off".into() },
        "allow-predictions" => if app.allow_predictions { "on".into() } else { "off".into() },
        "cursor-style" => std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "default".to_string()),
        "cursor-blink" => if crate::rendering::cursor_blink_option()
            .unwrap_or(crate::rendering::CURSOR_BLINK_DEFAULT_BLINKS) { "on".into() } else { "off".into() },
        "default-shell" | "default-command" => {
            if app.default_shell.is_empty() {
                crate::pane::cached_shell().unwrap_or("pwsh.exe").to_string()
            } else {
                app.default_shell.clone()
            }
        }
        "default-terminal" => app.environment.get("TERM").cloned().unwrap_or_default(),
        "word-separators" => app.word_separators.clone(),
        "pane-border-style" => app.pane_border_style.clone(),
        "pane-active-border-style" => app.pane_active_border_style.clone(),
        "pane-border-hover-style" => app.pane_border_hover_style.clone(),
        "status-style" => app.status_style.clone(),
        "window-status-format" => app.window_status_format.clone(),
        "window-status-current-format" => app.window_status_current_format.clone(),
        "window-status-separator" => app.window_status_separator.clone(),
        "window-status-style" => app.window_status_style.clone(),
        "window-status-current-style" => app.window_status_current_style.clone(),
        "window-status-activity-style" => app.window_status_activity_style.clone(),
        "window-status-bell-style" => app.window_status_bell_style.clone(),
        "window-status-last-style" => app.window_status_last_style.clone(),
        "message-style" => app.message_style.clone(),
        "message-command-style" => app.message_command_style.clone(),
        "mode-style" => app.mode_style.clone(),
        "status-left-style" => app.status_left_style.clone(),
        "status-right-style" => app.status_right_style.clone(),
        "status-interval" => app.status_interval.to_string(),
        "status-justify" => app.status_justify.clone(),
        "bell-action" => app.bell_action.clone(),
        "visual-bell" => if app.visual_bell { "on".into() } else { "off".into() },
        "monitor-silence" => app.monitor_silence.to_string(),
        "activity-action" => app.activity_action.clone(),
        "silence-action" => app.silence_action.clone(),
        "update-environment" => app.update_environment.join(" "),
        "status-left-length" => app.status_left_length.to_string(),
        "status-right-length" => app.status_right_length.to_string(),
        "window-size" => app.window_size.clone(),
        "allow-passthrough" => app.allow_passthrough.clone(),
        "copy-command" => app.copy_command.clone(),
        "set-clipboard" => app.set_clipboard.clone(),
        "main-pane-width" => app.main_pane_width.to_string(),
        "main-pane-height" => app.main_pane_height.to_string(),
        "codepoint-widths" => app.codepoint_widths.join(","),
        "terminal-overrides" => app.terminal_overrides.join(","),
        "command-alias" => {
            app.command_aliases.iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",")
        }
        "warm" => if app.warm_enabled { "on".into() } else { "off".into() },
        "warm-pool-size" => app.warm_pane.target.to_string(),
        "alternate-screen" => if app.allow_alternate_screen { "on".into() } else { "off".into() },
        "claude-code-fix-tty" => if app.claude_code_fix_tty { "on".into() } else { "off".into() },
        "claude-code-force-interactive" => if app.claude_code_force_interactive { "on".into() } else { "off".into() },
        "session-group" => app.session_group.clone().unwrap_or_default(),
        _ => {
            // Check user_options first (@-prefixed), then environment
            app.user_options.get(name).cloned()
                .or_else(|| app.environment.get(name).cloned())
                .unwrap_or_default()
        }
    };

    // An empty result means "never set". Report what the feature will actually
    // use so `show-options` and customize-mode agree with the running session.
    if value.is_empty() {
        if let Some(effective) = effective_when_unset(name) {
            return effective.to_string();
        }
    }
    value
}

pub(crate) fn get_window_option_value(app: &AppState, name: &str) -> String {
    get_window_option_value_for(app, name, None)
}

/// Window-scoped option lookup that honours per-window overrides.
///
/// `target_window` selects which window to read from (e.g. for
/// `show-options -w -v automatic-rename -t SESSION:N`).  `None` means
/// "active window", which matches what tmux does when `-t` is omitted.
///
/// Currently only `automatic-rename` has a real per-window override
/// (driven by `Window::manual_rename`, which is set when the window is
/// created with `-n NAME` or renamed via `rename-window`).  Other
/// window options fall through to the global value — they don't have
/// per-window storage in psmux today and tmux also defaults to the
/// global value when no window-local override is set.
///
/// See psmux issue #266: prior to this helper, `show-options -w
/// automatic-rename` always returned the global value, so windows
/// born with `-n NAME` (which correctly set `manual_rename = true`)
/// still reported `automatic-rename on`, even though the rename loop
/// was correctly skipping them.  The bug was reporting-only on those
/// windows, but the spec violation could mislead user scripts that
/// branched on the option value.
pub(crate) fn get_window_option_value_for(
    app: &AppState,
    name: &str,
    target_window: Option<usize>,
) -> String {
    if !is_window_option(name) {
        return String::new();
    }
    let idx = target_window.unwrap_or(app.active_idx);
    // #648: the window's own entry outranks everything below it.
    if let Some(value) = window_local_option(app, idx, name) {
        return value.to_string();
    }
    if name == "automatic-rename" {
        if let Some(w) = app.windows.get(idx) {
            if w.manual_rename {
                return "off".into();
            }
        }
    }
    if name == "window-size" {
        if let Some(value) = app.windows.get(idx).and_then(|window| window.window_size.as_ref()) {
            return value.clone();
        }
    }
    get_option_value(app, name)
}

/// True when window `idx` takes `name` from the global store rather than from
/// its own table. Drives the `*` marker `show-options -A` puts on an inherited
/// entry (cmd-show-options.c: `if (o->owner != oo) ... "*"`).
pub(crate) fn window_option_is_inherited(app: &AppState, idx: usize, name: &str) -> bool {
    if window_local_option(app, idx, name).is_some() {
        return false;
    }
    // The two options that were per window BEFORE #648 gave windows a real
    // table are still window-local when their dedicated field is set.
    match name {
        "automatic-rename" => !app.windows.get(idx).is_some_and(|w| w.manual_rename),
        "window-size" => !app.windows.get(idx).is_some_and(|w| w.window_size.is_some()),
        _ => true,
    }
}

/// Which table one `show-options` window listing reads, and whether it answers
/// tmux's `-A` inheritance question.
///
/// tmux picks the table from the flags in `options_scope_from_flags`
/// (options.c:1086-1099): `-w` is `wl->window->options`, `-wg` is
/// `global_w_options`. `cmd_show_options_all` (cmd-show-options.c:241-290)
/// then walks the options table and, for each entry, calls
/// `options_get_only(oo, name)`: an entry the chosen table does not own is
/// SKIPPED unless `-A` was given, and with `-A` it is fetched from the parent
/// and printed with a `*`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum WindowListing {
    /// `show-options -w`: only the values that window itself stores.
    Local,
    /// `show-options -wA`: the window's own values plain, every inherited one
    /// marked with `*`, merged into ONE list.
    LocalAndInherited,
    /// `show-options -wg`: the global window table, which owns every entry, so
    /// nothing is ever skipped and nothing is ever marked.
    Global,
}

/// Body of `show-options -w [-A] [-g]` for one window.
///
/// Before #655 this always printed the RESOLVED view (every window option with
/// the value that window would use) and `-A` only added the marker, so a window
/// with one override reported all sixteen names and a window with none reported
/// sixteen names tmux prints nothing for. The listing now follows
/// `cmd_show_options_all`; the RESOLVED view a caller that probes window scope
/// for a session wide option depends on (#321) is still reachable, through the
/// `-v <name>` query (unchanged) and through `-wg`, which prints the whole
/// window table the way tmux's own `-wg` does.
pub(crate) fn render_window_options_for(
    app: &AppState,
    target_window: Option<usize>,
    listing: WindowListing,
) -> String {
    let idx = target_window.unwrap_or(app.active_idx);
    let mut output = String::new();
    for name in WINDOW_OPTION_NAMES {
        if listing == WindowListing::Global {
            output.push_str(&format!("{} {}\n", name, get_option_value(app, name)));
            continue;
        }
        let inherited = window_option_is_inherited(app, idx, name);
        if inherited && listing == WindowListing::Local {
            continue;
        }
        output.push_str(&format!(
            "{}{} {}\n",
            name,
            if inherited { "*" } else { "" },
            get_window_option_value_for(app, name, Some(idx)),
        ));
    }
    output
}

/// The global window table listing, which is what a caller with no window in
/// hand wants (`show-options -wg`, the option default audits).
pub(crate) fn render_window_options(app: &AppState) -> String {
    render_window_options_for(app, None, WindowListing::Global)
}

/// Returns `true` if the given option name is a boolean (on/off) option.
/// Used by set-option toggle logic (tmux parity: `set <option>` without a
/// value toggles boolean options).
pub(crate) fn is_boolean_option(name: &str) -> bool {
    matches!(
        name,
        "mouse"
            | "bold-is-bright"
            | "scroll-enter-copy-mode"
            | "mouse-drag-enter-copy-mode"
            | "pwsh-mouse-selection"
            | "mouse-selection"
            | "mouse-selection-force"
            | "paste-detection"
            | "choose-tree-preview"
            | "focus-events"
            | "renumber-windows"
            | "automatic-rename"
            | "allow-rename"
            | "allow-set-title"
            | "monitor-activity"
            | "visual-activity"
            | "synchronize-panes"
            | "remain-on-exit"
            | "destroy-unattached"
            | "exit-empty"
            | "set-titles"
            | "aggressive-resize"
            | "visual-bell"
            | "prediction-dimming"
            | "allow-predictions"
            | "cursor-blink"
            | "warm"
            | "alternate-screen"
            | "claude-code-fix-tty"
            | "claude-code-force-interactive"
            | "status"
    )
}

/// How a `set-option` that names an option but supplies **no value** must be
/// handled (issue #535). tmux 3.4 splits this case in two:
///
/// * boolean/flag options toggle silently and exit 0 (`set -g mouse` flips
///   on<->off). psmux already did this for config-file lines (#278);
/// * every other option is an error: `empty value` on stderr, exit 1.
///
/// `-q` deliberately does **not** enter into it. Both psmux's own CLI help and
/// tmux's manual scope `-q` to "errors about unknown or ambiguous options";
/// verified against tmux 3.4, where `set -gq @foo` still fails with
/// `empty value`. Callers must route the no-value case through here rather
/// than dropping it, which is what let #535 pass silently with exit 0.
pub(crate) fn missing_value_toggles(option: &str) -> bool {
    is_boolean_option(option)
}

/// Toggle a boolean option: read current value and flip it.
/// Returns `true` if the option was toggled, `false` if not a boolean option.
pub(crate) fn toggle_option(app: &mut AppState, option: &str) -> bool {
    if !is_boolean_option(option) {
        return false;
    }
    let current = get_option_value(app, option);
    let new_value = if current == "on" { "off" } else { "on" };
    apply_set_option(app, option, new_value, false).is_ok()
}

/// Restore one option to the value a freshly started server reports for it,
/// which is what `set-option -u` means (#619 follow up).
///
/// tmux does this in one place, `options_remove_or_default` (options.c ~1457):
///
/// ```c
/// if (o->tableentry != NULL &&
///     (oo == global_options || oo == global_s_options || oo == global_w_options))
///         options_default(oo, o->tableentry);
/// else
///         options_remove(o);
/// ```
///
/// so at a global scope a table option goes back to its options table default
/// and only a **user** option, which carries no table entry, is removed
/// outright.
///
/// psmux had no such single place. The unset was open coded three times and
/// each copy was wrong in its own way:
///
/// * the server request loop carried a hand written per option restore table of
///   about thirty arms with a `_ => {}` catch all, so every option it had never
///   heard of simply kept its value: `set -s default-terminal xterm-256color`
///   followed by `set -su default-terminal` still read `xterm-256color`, and
///   `status-left` was restored to `psmux:#I` where a fresh server reports
///   `[#S] `;
/// * the config parser wrote an EMPTY value instead of a default, so
///   `set -gu escape-time` in a config file left the old number where the CLI
///   restored 500;
/// * the plugin drain loop only erased the explicit set mark and never touched
///   the value at all.
///
/// `OPTION_CATALOG` already carries a default for every option it lists, and
/// `tests-rs/test_option_default_parity.rs` pins each of those defaults to a
/// freshly constructed `AppState`, so the catalog IS the options table psmux
/// was missing. Restoring through it means the table is written once, and the
/// parity test keeps it honest.
pub(crate) fn reset_option_to_default(app: &mut AppState, option: &str) {
    let key = option.trim();
    if key.is_empty() {
        return;
    }

    // Forget that the user ever set it, so a following `-o` sees an unset
    // option and applies (#619). tmux gets this for free because `-o` is
    // judged by `options_get_only`, which the unset has already cleared.
    app.user_set_options.remove(key);

    // A `@user` option has no table entry, so tmux takes the `options_remove`
    // branch: the key goes away rather than falling back to a default that
    // does not exist. `-o` tests user options by key presence, so an entry
    // left holding "" would read as set for ever.
    if key.starts_with('@') {
        app.user_options.remove(key);
        return;
    }

    // Drop any stored override first. Options whose default is represented by
    // a missing entry are fully restored by removal; applying their catalog
    // default would turn an unset option back into an explicit override.
    app.user_options.remove(key);
    if matches!(
        key,
        "window-style" | "window-active-style" | "pane-border-indicators"
    ) {
        return;
    }

    if let Some(default) = crate::server::option_catalog::default_for(key) {
        let _ = apply_set_option(app, key, default, true);
    }
}

/// Apply a set-option command. If `quiet` is true, unknown options are silently ignored.
pub(crate) fn apply_set_option(
    app: &mut AppState,
    option: &str,
    value: &str,
    _quiet: bool,
) -> Result<(), String> {
    crate::server::option_catalog::validate_option_value(option, value)?;
    match option {
        "status-left" => { app.status_left = value.to_string(); }
        "status-right" => { app.status_right = value.to_string(); }
        "status-left-length" => {
            if let Ok(n) = value.parse::<usize>() { app.status_left_length = n; }
        }
        "status-right-length" => {
            if let Ok(n) = value.parse::<usize>() { app.status_right_length = n; }
        }
        "base-index" => {
            if let Ok(idx) = value.parse::<usize>() {
                app.rebase_window_indices(idx);
                app.window_base_index = idx;
            }
        }
        "pane-base-index" => {
            if let Ok(idx) = value.parse::<usize>() {
                app.pane_base_index = idx;
            }
        }
        "codepoint-widths" => { set_codepoint_widths(app, value); }
        "terminal-overrides" => {
            app.terminal_overrides = crate::terminal_overrides::split_array(value);
        }
        "mouse" => { app.mouse_enabled = value == "on" || value == "true" || value == "1" || value == "yes"; }
        "bold-is-bright" => {
            app.bold_is_bright = matches!(value, "on" | "true" | "1" | "yes");
            crate::platform::set_bold_is_bright(app.bold_is_bright);
        }
        // Field write plus the platform call in one arm, the bold-is-bright
        // shape, so every set path (config file, CLI, in-TUI prompt, control
        // mode) applies to the live server process without its own hook.
        // An unusable value is refused rather than stored, so the class and
        // the reported option both stay where they were (#608).
        "priority" => {
            if crate::platform::normalize_priority(value).is_some() {
                app.priority = crate::platform::resolve_priority(Some(value), false);
                crate::platform::set_process_priority(&app.priority);
            }
        }
        "scroll-enter-copy-mode" => { app.scroll_enter_copy_mode = matches!(value, "on" | "true" | "1" | "yes"); }
        "mouse-drag-enter-copy-mode" => { app.mouse_drag_enter_copy_mode = matches!(value, "on" | "true" | "1" | "yes"); }
        "pwsh-mouse-selection" => { app.pwsh_mouse_selection = matches!(value, "on" | "true" | "1" | "yes"); }
        "mouse-selection" => { app.mouse_selection = matches!(value, "on" | "true" | "1" | "yes"); }
        "mouse-selection-force" => { app.mouse_selection_force = matches!(value, "on" | "true" | "1" | "yes"); }
        "paste-detection" => { app.paste_detection = matches!(value, "on" | "true" | "1" | "yes"); }
        "choose-tree-preview" => { app.choose_tree_preview = matches!(value, "on" | "true" | "1" | "yes"); }
        "prefix" => {
            if let Some(kc) = parse_key_string(value) {
                app.prefix_key = kc;
                crate::config::ensure_prefix_self_binding(app);
            }
        }
        "prefix2" => {
            if value.eq_ignore_ascii_case("none") || value.is_empty() {
                app.prefix2_key = None;
            } else if let Some(kc) = parse_key_string(value) {
                app.prefix2_key = Some(kc);
            }
        }
        "escape-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.escape_time_ms = ms;
            }
        }
        "history-limit" => {
            if let Ok(limit) = value.parse::<usize>() {
                app.history_limit = limit;
                // Warm pane reconciliation is handled centrally by
                // warm_pane_sync::for_option_change once the caller
                // runs apply_set_option here — see #271.
            }
        }
        "alternate-screen" => {
            app.allow_alternate_screen = matches!(value, "on" | "true" | "1" | "yes");
            // The flag is enforced inside the vt100 parser of each
            // pane.  warm_pane_sync::for_option_change patches the
            // existing warm pane's parser and walks live panes so the
            // change takes effect immediately (psmux issue #88).
        }
        "display-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.display_time_ms = ms;
            }
        }
        "display-panes-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.display_panes_time_ms = ms;
            }
        }
        "repeat-time" => {
            // Bounded like tmux (options-table.c: minimum 0, maximum
            // 2000000 ms). An out of range value is refused rather than
            // stored, so the command prompt and TCP routes cannot install a
            // repeat window the CLI guard would have rejected (#606).
            if let Ok(ms) = value.parse::<i64>() {
                if (0..=REPEAT_TIME_MAX_MS).contains(&ms) {
                    app.repeat_time_ms = ms as u64;
                }
            }
        }
        "mode-keys" => { app.mode_keys = value.to_string(); }
        "status" => {
            // Handle numeric values for multi-line status bar (tmux 3.2+)
            if let Ok(n) = value.parse::<usize>() {
                if n >= 2 {
                    app.status_visible = true;
                    app.status_lines = n;
                } else if n == 1 {
                    app.status_visible = true;
                    app.status_lines = 1;
                } else {
                    app.status_visible = false;
                    app.status_lines = 1;
                }
            } else {
                app.status_visible = matches!(value, "on" | "true");
                app.status_lines = 1;
            }
        }
        "status-position" => { app.status_position = value.to_string(); }
        "status-style" => { app.status_style = value.to_string(); }
        // Deprecated but ubiquitous: map status-bg/status-fg to status-style
        "status-bg" => {
            let current = &app.status_style;
            let filtered: String = current.split(',')
                .filter(|s| !s.trim().starts_with("bg="))
                .collect::<Vec<_>>().join(",");
            app.status_style = if filtered.is_empty() {
                format!("bg={}", value)
            } else {
                format!("{},bg={}", filtered, value)
            };
        }
        "status-fg" => {
            let current = &app.status_style;
            let filtered: String = current.split(',')
                .filter(|s| !s.trim().starts_with("fg="))
                .collect::<Vec<_>>().join(",");
            app.status_style = if filtered.is_empty() {
                format!("fg={}", value)
            } else {
                format!("{},fg={}", filtered, value)
            };
        }
        "focus-events" => { app.focus_events = matches!(value, "on" | "true" | "1" | "yes"); }
        "renumber-windows" => { app.renumber_windows = matches!(value, "on" | "true" | "1" | "yes"); }
        "remain-on-exit" => { app.remain_on_exit = matches!(value, "on" | "true" | "1" | "yes"); }
        "destroy-unattached" => { app.destroy_unattached = matches!(value, "on" | "true" | "1" | "yes"); }
        "exit-empty" => { app.exit_empty = matches!(value, "on" | "true" | "1" | "yes"); }
        "set-titles" => { app.set_titles = matches!(value, "on" | "true" | "1" | "yes"); }
        "set-titles-string" => { app.set_titles_string = value.to_string(); }
        "default-command" | "default-shell" => {
            // Strip surrounding quotes only when the entire value is wrapped
            // in matching quotes.  This handles `"C:/Program Files/..."` but
            // preserves `"C:/Program Files/..." --login` (quoted path + args).
            let v = value.trim();
            let stripped = if (v.starts_with('"') && v.ends_with('"'))
                || (v.starts_with('\'') && v.ends_with('\''))
            {
                &v[1..v.len() - 1]
            } else {
                v
            };
            app.default_shell = stripped.to_string();
        }
        "word-separators" => { app.word_separators = value.to_string(); }
        "aggressive-resize" => { app.aggressive_resize = matches!(value, "on" | "true" | "1" | "yes"); }
        "monitor-activity" => { app.monitor_activity = matches!(value, "on" | "true" | "1" | "yes"); }
        "visual-activity" => { app.visual_activity = matches!(value, "on" | "true" | "1" | "yes"); }
        "synchronize-panes" => { app.sync_input = matches!(value, "on" | "true" | "1" | "yes"); }
        "automatic-rename" => {
            app.automatic_rename = matches!(value, "on" | "true" | "1" | "yes");
            // When user explicitly enables automatic-rename, clear manual_rename
            // on the active window so auto-rename can take effect again.
            if app.automatic_rename {
                if let Some(w) = app.windows.get_mut(app.active_idx) {
                    w.manual_rename = false;
                }
            }
        }
        "allow-rename" => { app.allow_rename = matches!(value, "on" | "true" | "1" | "yes"); }
        "allow-set-title" => { app.allow_set_title = matches!(value, "on" | "true" | "1" | "yes"); }
        "activity-action" => { app.activity_action = value.to_string(); }
        "silence-action" => { app.silence_action = value.to_string(); }
        "bell-action" => { app.bell_action = value.to_string(); }
        "visual-bell" => { app.visual_bell = matches!(value, "on" | "true" | "1" | "yes"); }
        "monitor-silence" => {
            if let Ok(n) = value.parse::<u64>() { app.monitor_silence = n; }
        }
        "update-environment" => {
            app.update_environment = value.split_whitespace().map(|s| s.to_string()).collect();
        }
        "prediction-dimming" | "dim-predictions" => {
            app.prediction_dimming = !matches!(value, "off" | "false" | "0");
        }
        "allow-predictions" => {
            app.allow_predictions = matches!(value, "on" | "true" | "1" | "yes");
        }
        "cursor-style" => { std::env::set_var("PSMUX_CURSOR_STYLE", value); }
        "cursor-blink" => {
            // Stored only; see the same arm in config.rs for why the DEC
            // private mode 12 write that used to live here was removed.
            let on = matches!(value, "on"|"true"|"1");
            std::env::set_var("PSMUX_CURSOR_BLINK", if on { "1" } else { "0" });
        }
        "pane-border-style" => { app.pane_border_style = value.to_string(); }
        "pane-active-border-style" => { app.pane_active_border_style = value.to_string(); }
        "pane-border-hover-style" => { app.pane_border_hover_style = value.to_string(); }
        "window-status-format" => { app.window_status_format = value.to_string(); }
        "window-status-current-format" => { app.window_status_current_format = value.to_string(); }
        "window-status-separator" => { app.window_status_separator = value.to_string(); }
        "window-status-style" => { app.window_status_style = value.to_string(); }
        "window-status-current-style" => { app.window_status_current_style = value.to_string(); }
        "window-status-activity-style" => { app.window_status_activity_style = value.to_string(); }
        "window-status-bell-style" => { app.window_status_bell_style = value.to_string(); }
        "window-status-last-style" => { app.window_status_last_style = value.to_string(); }
        "mode-style" => { app.mode_style = value.to_string(); }
        "message-style" => { app.message_style = value.to_string(); }
        "message-command-style" => { app.message_command_style = value.to_string(); }
        "status-left-style" => { app.status_left_style = value.to_string(); }
        "status-right-style" => { app.status_right_style = value.to_string(); }
        "status-justify" => { app.status_justify = value.to_string(); }
        "status-interval" => {
            if let Ok(n) = value.parse::<u64>() { app.status_interval = n; }
        }
        "main-pane-width" => {
            if let Some(n) = parse_main_pane_size(value) { app.main_pane_width = n; }
        }
        "main-pane-height" => {
            if let Some(n) = parse_main_pane_size(value) { app.main_pane_height = n; }
        }
        "window-size" => { app.window_size = value.to_string(); }
        "allow-passthrough" => { app.allow_passthrough = value.to_string(); }
        "copy-command" => { app.copy_command = value.to_string(); }
        "set-clipboard" => { app.set_clipboard = value.to_string(); }
        "command-alias" => {
            // Format: "alias=expansion" e.g. "splitp=split-window"
            if let Some(pos) = value.find('=') {
                let alias = value[..pos].trim().to_string();
                let expansion = value[pos+1..].trim().to_string();
                app.command_aliases.insert(alias, expansion);
            }
        }
        "warm-pool-size" => {
            set_warm_pool_size(app, value);
        }
        "warm" => {
            app.warm_enabled = matches!(value, "on" | "true" | "1" | "yes");
            // When warm is disabled, kill any existing warm pane AND warm server
            if !app.warm_enabled {
                app.warm_pane.target = 0;
                app.warm_pane.kill_all();
                // Kill the background warm server process
                let warm_base = if let Some(ref sn) = app.socket_name {
                    format!("{}____warm__", sn)
                } else {
                    "__warm__".to_string()
                };
                let warm_port_path = crate::paths::port_file(&warm_base);
                if let Ok(port_str) = std::fs::read_to_string(&warm_port_path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr = format!("127.0.0.1:{}", port);
                        let key = crate::session::read_session_key(&warm_base)
                            .unwrap_or_default();
                        let _ = crate::session::send_auth_cmd(
                            &addr,
                            &key,
                            b"kill-server\n",
                        );
                    }
                }
                let _ = std::fs::remove_file(&warm_port_path);
                let warm_key_path = crate::paths::key_file(&warm_base);
                let _ = std::fs::remove_file(&warm_key_path);
            } else if app.warm_pane.target == 0 {
                // Turning warm back on restores the configured depth, not the
                // zero that turning it off left behind.
                app.warm_pane.target = crate::types::default_warm_pool_size().max(1);
            }
        }
        "claude-code-fix-tty" => {
            app.claude_code_fix_tty = matches!(value, "on" | "true" | "1" | "yes");
        }
        "claude-code-force-interactive" => {
            app.claude_code_force_interactive = matches!(value, "on" | "true" | "1" | "yes");
        }
        "session-group" => {
            if value.is_empty() || value == "none" {
                app.session_group = None;
            } else {
                app.session_group = Some(value.to_string());
            }
        }
        _ => {
            // Handle status-format[N] patterns
            if option.starts_with("status-format[") && option.ends_with(']') {
                if let Ok(idx) = option["status-format[".len()..option.len()-1].parse::<usize>() {
                    while app.status_format.len() <= idx {
                        app.status_format.push(String::new());
                    }
                    app.status_format[idx] = value.to_string();
                    return Ok(());
                }
            }
            // Store @user-options in dedicated map (NOT environment) to avoid
            // leaking into child shell env vars (#105).
            if option.starts_with('@') {
                app.user_options.insert(option.to_string(), value.to_string());
            } else if option == "default-terminal" {
                // tmux sets the TERM env var from this option (#137)
                app.environment.insert("TERM".to_string(), value.to_string());
            } else if option.contains('-') {
                // Options with hyphens (e.g. terminal-overrides, allow-rename)
                // are tmux config options, NOT environment variables.  Storing
                // them in app.environment causes PowerShell ParserErrors when
                // injected via $env:NAME syntax (#137).  Store in user_options.
                app.user_options.insert(option.to_string(), value.to_string());
            } else {
                // Simple names without hyphens are likely real env vars
                // (set via `set-environment` or plugin compat)
                app.environment.insert(option.to_string(), value.to_string());
            }
        }
    }
    Ok(())
}

/// Catalog options psmux stores per pane, in the order `show-options -p`
/// prints them.
///
/// `set-option -p` accepts one catalog name ([`apply_set_pane_option`]):
/// `remain-on-exit`, which a pane can inherit from its window and then the
/// global store. Everything else it stores is a USER option (`@mouse-force`,
/// and since #728 any `@name`). tmux prints a user option from the
/// table's own entries (cmd-show-options.c:249-254, the `options_table_entry(o)
/// == NULL` walk) and never invents an inherited one for it, so only the
/// catalog name belongs here.
pub(crate) const PANE_OPTION_NAMES: &[&str] = &["remain-on-exit"];

/// Resolve a `set-option -p` / `show-options -p` target to the pane it names.
///
/// `raw` is "" for the active pane, or a `%N` / `N` pane id. The TCP routes
/// resolve and focus a richer spelling before they get here (connection.rs
/// `pane_scope_target`), but the command prompt and config route hand over
/// what was typed, so a `sess:win.pane` spec is resolved like any other pane
/// target rather than silently meaning the active pane.
pub(crate) fn resolve_option_target_pane<'a>(
    app: &'a mut AppState,
    raw: &str,
) -> Option<&'a mut crate::types::Pane> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        let window = app.windows.get_mut(app.active_idx)?;
        return crate::tree::active_pane_mut(&mut window.root, &window.active_path);
    }
    let id = match raw.trim_start_matches('%').parse::<usize>() {
        Ok(id) => id,
        Err(_) => {
            let (window_pos, path) = crate::window_ops::resolve_pane_spec(app, raw).ok()?;
            let window = app.windows.get(window_pos)?;
            crate::tree::get_active_pane_id_at_path(&window.root, &path)?
        }
    };
    crate::tree::find_pane_mut_by_id_global(app, id)
}

/// The whole of `set-option -p [-u|-a|-o] [-q] [-t pane] <name> [value]`,
/// shared by the TCP/CLI routes, the in-TUI command prompt and the config
/// file so every route lands the same write (#728).
///
/// tmux keeps an options table on every pane and puts ANY `@name` user option
/// there (cmd-set-option.c with `-p` picks `wp->options`; a user option has no
/// table entry so it takes the scope of the flags). `#{@name}` for that pane
/// then resolves pane, window, session, global in that order (options.c
/// `options_get` walks `oo->parent`). psmux used to accept only
/// `remain-on-exit` and `@mouse-force` here and refused every other user
/// option, which broke `omx team` and the Claude Code teammate backend.
///
/// Semantics, each verified against tmux 3.4:
/// * `-u` removes the pane's own entry (an absent one is not an error), so the
///   pane inherits again.
/// * `-a` appends to the pane's OWN value only (options_set_string looks the
///   name up with options_get_only), never to an inherited one.
/// * `-o` on a name the pane already owns is `already set: <name>`, silent
///   under `-q`.
/// * A non `@` name other than the per pane catalog options psmux implements
///   stays a loud refusal, never a silently stored no-op (#580).
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_set_pane_option(
    app: &mut AppState,
    target: &str,
    option: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let Some(pane) = resolve_option_target_pane(app, target) else {
        return format!("ERROR: can't find pane: {}", target);
    };
    let store = &mut pane.pane_options;
    match option {
        "remain-on-exit" | "@mouse-force" => {
            // These two predate the general store and keep their validation.
            // An empty value without -u still means "unset", as it always did.
            if unset || value.is_empty() {
                store.remove(option);
                return String::new();
            }
            let valid = if option == "remain-on-exit" {
                matches!(value, "on" | "off" | "failed")
            } else {
                matches!(value, "on" | "off" | "1" | "0" | "true" | "false" | "yes" | "no")
            };
            if !valid {
                return if option == "remain-on-exit" {
                    format!("ERROR: set-option -p remain-on-exit: bad value '{}' (want on, off or failed)", value)
                } else {
                    format!("ERROR: set-option -p @mouse-force: bad value '{}' (want on or off)", value)
                };
            }
            if only_if_unset && store.contains_key(option) {
                return if quiet { String::new() } else { format!("ERROR: already set: {}", option) };
            }
            store.insert(option.to_string(), value.to_string());
            String::new()
        }
        name if name.starts_with('@') && name.len() > 1 => {
            if unset {
                store.remove(name);
                return String::new();
            }
            if only_if_unset && store.contains_key(name) {
                return if quiet { String::new() } else { format!("ERROR: already set: {}", name) };
            }
            let stored = if append {
                format!("{}{}", store.get(name).map(String::as_str).unwrap_or(""), value)
            } else {
                value.to_string()
            };
            store.insert(name.to_string(), stored);
            String::new()
        }
        // Loud refusal, never a silent stored no-op: the Claude Code teammate
        // backend checked nothing but exit codes and a swallowed pane option
        // looked exactly like success (#580).
        other => format!(
            "ERROR: pane-scoped option '{}' is not supported (supported: remain-on-exit, @mouse-force, any @user option)",
            other
        ),
    }
}

/// A pane's own value for a user option, then the value it inherits.
///
/// tmux resolves `#{@name}` for a pane through pane, window, session and
/// global tables (format.c format_find falls through to `options_get` on the
/// pane's options, whose parent chain is the window's, then the session's,
/// then the global ones). psmux keeps every non pane `@name` in one store
/// (`user_options`; a psmux server holds one session and `-w @name` lands
/// there too, see [`is_window_scoped_write`]), so the chain here is the pane
/// first, then that store.
pub(crate) fn pane_user_option(
    app: &AppState,
    pane: Option<&crate::types::Pane>,
    name: &str,
) -> Option<String> {
    if let Some(value) = pane.and_then(|p| p.pane_options.get(name)) {
        return Some(value.clone());
    }
    app.user_options.get(name).cloned()
}

/// Body of `show-options -p [-A]` for one pane.
///
/// `listing` is the server's `ShowPaneOptions` reply: one `name value` pair per
/// line for every option the pane actually stores. Without `-A` that IS the
/// answer, which is what tmux prints. With `-A` every catalog name the pane
/// does not own is appended with its inherited value and a `*`, the same rule
/// [`render_window_options_for`] applies one scope up (#655); before that `-A`
/// was silently a no-op on a bare pane listing while it already worked for a
/// named `-p` query (#647).
pub(crate) fn render_pane_options<F>(
    listing: &str,
    include_inherited: bool,
    mut inherited: F,
) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    // A refusal from the server (unknown pane target) is not an option
    // listing; hand it straight back so the caller still reports it.
    if listing.starts_with("ERROR:") {
        return format!("{}\n", listing);
    }
    let owns = |name: &str| -> bool {
        listing
            .lines()
            .any(|line| line.split(' ').next() == Some(name))
    };
    let mut output = String::new();
    for line in listing.lines() {
        if !line.trim().is_empty() {
            output.push_str(line);
            output.push('\n');
        }
    }
    if include_inherited {
        for name in PANE_OPTION_NAMES {
            if owns(name) {
                continue;
            }
            if let Some(value) = inherited(name) {
                output.push_str(&format!("{}* {}\n", name, value));
            }
        }
    }
    output
}

/// Pick one entry out of a pane option listing for `show-options -p <name>`.
///
/// `listing` is the server's `ShowPaneOptions` reply: one `name value` pair per
/// line for every option the pane actually stores. tmux answers a named query
/// from the pane's own store and prints nothing at all when the option is not
/// set there, unless `-A` is given, in which case the inherited value is shown
/// with a `*` marker (cmd-show-options.c:192-207). `-v` drops the name and
/// prints the bare value, which is the contract automation compares against
/// `on` / `off` (#647 WIN-02).
///
/// The returned string is ready to write, newline included, or empty when tmux
/// would print nothing.
pub(crate) fn select_pane_option_line<F>(
    listing: &str,
    name: &str,
    values_only: bool,
    include_inherited: bool,
    mut inherited: F,
) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    // A refusal from the server (unknown pane target) is not an option
    // listing; hand it straight back so the caller still reports it.
    if listing.starts_with("ERROR:") {
        return format!("{}\n", listing);
    }
    for line in listing.lines() {
        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, v),
            None => (line, ""),
        };
        if key != name {
            continue;
        }
        return if values_only {
            format!("{}\n", value)
        } else {
            format!("{} {}\n", key, value)
        };
    }
    if include_inherited {
        if let Some(value) = inherited(name) {
            return if values_only {
                format!("{}\n", value)
            } else {
                format!("{}* {}\n", name, value)
            };
        }
    }
    String::new()
}

/// [`select_pane_option_line`] plus tmux's verdict on a user option the pane
/// does not have (#728).
///
/// tmux 3.4 cmd-show-options.c: when the named option is not found and the
/// name starts with `@`, `-q` prints nothing at exit 0 and anything else is
/// `invalid option: @name` at exit 1. A catalog name the pane does not own
/// still prints nothing, which is what the #647 contract already does.
pub(crate) fn pane_option_query_reply<F>(
    listing: &str,
    name: &str,
    values_only: bool,
    include_inherited: bool,
    quiet: bool,
    inherited: F,
) -> String
where
    F: FnMut(&str) -> Option<String>,
{
    let out = select_pane_option_line(listing, name, values_only, include_inherited, inherited);
    if out.is_empty() && name.starts_with('@') && !quiet {
        return format!("ERROR: invalid option: {}\n", name);
    }
    out
}

#[cfg(test)]
#[path = "../../tests-rs/test_issue647_show_options_value.rs"]
mod tests_issue647_show_options_value;

#[cfg(test)]
#[path = "../../tests-rs/test_issue728_pane_user_options.rs"]
mod tests_issue728_pane_user_options;

#[cfg(test)]
#[path = "../../tests-rs/test_issue648_window_scoped_options.rs"]
mod tests_issue648_window_scoped_options;

#[cfg(test)]
#[path = "../../tests-rs/test_issue655_show_options_window_listing.rs"]
mod tests_issue655_show_options_window_listing;

#[cfg(test)]
#[path = "../../tests-rs/test_issue266_per_window_autorename.rs"]
mod tests_issue266_per_window_autorename;

#[cfg(test)]
#[path = "../../tests-rs/test_issue278_toggle_bool_option.rs"]
mod tests_issue278_toggle_bool_option;

#[cfg(test)]
#[path = "../../tests-rs/test_issue535_setoption_no_value.rs"]
mod tests_issue535_setoption_no_value;

#[cfg(test)]
#[path = "../../tests-rs/test_issue559_monitor_silence_options.rs"]
mod tests_issue559_monitor_silence_options;
