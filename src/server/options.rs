use crate::types::AppState;
use crate::config::{format_key_binding, parse_key_string};

/// Upper bound for `repeat-time`, in milliseconds. tmux declares the option as
/// a number with minimum 0 and maximum 2000000 in options-table.c, so psmux
/// refuses anything outside that range on every route (#606).
pub(crate) const REPEAT_TIME_MAX_MS: i64 = 2_000_000;

fn is_window_option(name: &str) -> bool {
    matches!(
        name,
        "automatic-rename"
            | "monitor-activity"
            // #559: tmux classifies monitor-silence as a window option; without
            // this entry `show-options -w monitor-silence` returned an empty
            // value even after a successful `set -w monitor-silence N`.
            | "monitor-silence"
            | "remain-on-exit"
            | "window-status-format"
            | "window-status-current-format"
            | "window-status-separator"
            | "window-status-style"
            | "window-status-current-style"
            | "window-status-activity-style"
            | "window-status-bell-style"
            | "window-status-last-style"
            | "main-pane-width"
            | "main-pane-height"
            | "window-size"
    )
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
        // Consumed at src/rendering.rs, missing entry means border_lines::DEFAULT.
        "pane-border-lines" => crate::border_lines::DEFAULT,
        // Consumed at src/server/helpers.rs, a missing entry disables the gutter.
        "copy-mode-line-numbers" => "off",
        "copy-mode-line-number-style" => "fg=brightblack",
        "copy-mode-current-line-number-style" => "fg=yellow,bold",
        // TERM handed to panes when default-terminal was never set.
        "default-terminal" => "xterm-256color",
        // tmux reports `default` for a style that has not been overridden.
        "status-style" | "status-left-style" | "status-right-style"
        | "message-style" | "message-command-style" | "mode-style"
        | "pane-border-style" | "pane-active-border-style"
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
        "prediction-dimming" => if app.prediction_dimming { "on".into() } else { "off".into() },
        "allow-predictions" => if app.allow_predictions { "on".into() } else { "off".into() },
        "cursor-style" => std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string()),
        "cursor-blink" => if std::env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0" { "on".into() } else { "off".into() },
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
        "command-alias" => {
            app.command_aliases.iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",")
        }
        "warm" => if app.warm_enabled { "on".into() } else { "off".into() },
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
    if name == "automatic-rename" {
        let idx = target_window.unwrap_or(app.active_idx);
        if let Some(w) = app.windows.get(idx) {
            if w.manual_rename {
                return "off".into();
            }
        }
    }
    if name == "window-size" {
        let idx = target_window.unwrap_or(app.active_idx);
        if let Some(value) = app.windows.get(idx).and_then(|window| window.window_size.as_ref()) {
            return value.clone();
        }
    }
    get_option_value(app, name)
}

pub(crate) fn render_window_options(app: &AppState) -> String {
    let names = [
        "automatic-rename",
        "monitor-activity",
        "monitor-silence",
        "remain-on-exit",
        "window-status-format",
        "window-status-current-format",
        "window-status-separator",
        "window-status-style",
        "window-status-current-style",
        "window-status-activity-style",
        "window-status-bell-style",
        "window-status-last-style",
        "main-pane-width",
        "main-pane-height",
        "window-size",
    ];

    let mut output = String::new();
    for name in names {
        output.push_str(&format!("{} {}\n", name, get_window_option_value(app, name)));
    }
    output
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
    apply_set_option(app, option, new_value, false);
    true
}

/// Apply a set-option command. If `quiet` is true, unknown options are silently ignored.
pub(crate) fn apply_set_option(app: &mut AppState, option: &str, value: &str, _quiet: bool) {
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
        "mouse" => { app.mouse_enabled = value == "on" || value == "true" || value == "1" || value == "yes"; }
        "bold-is-bright" => {
            app.bold_is_bright = matches!(value, "on" | "true" | "1" | "yes");
            crate::platform::set_bold_is_bright(app.bold_is_bright);
        }
        "scroll-enter-copy-mode" => { app.scroll_enter_copy_mode = matches!(value, "on" | "true" | "1" | "yes"); }
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
            let on = matches!(value, "on"|"true"|"1");
            std::env::set_var("PSMUX_CURSOR_BLINK", if on { "1" } else { "0" });
            let _ = std::io::Write::write_all(&mut std::io::stdout(), if on { b"\x1b[?12h" } else { b"\x1b[?12l" });
            let _ = std::io::Write::flush(&mut std::io::stdout());
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
            if let Ok(n) = value.parse::<u16>() { app.main_pane_width = n; }
        }
        "main-pane-height" => {
            if let Ok(n) = value.parse::<u16>() { app.main_pane_height = n; }
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
        "warm" => {
            app.warm_enabled = matches!(value, "on" | "true" | "1" | "yes");
            // When warm is disabled, kill any existing warm pane AND warm server
            if !app.warm_enabled {
                if let Some(mut wp) = app.warm_pane.take() {
                    wp.child.kill().ok();
                }
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
                    return;
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
}

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
