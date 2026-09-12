// ── src/help.rs ───────────────────────────────────────────────────────
// Comprehensive help / reference data for the C-b ? overlay and
// `list-keys` CLI command.  Kept as a standalone module so it does not
// bloat existing source files.
// ─────────────────────────────────────────────────────────────────────

/// Default root-table keybindings (no prefix required).
/// These match tmux defaults for the root key table.
/// tmux binds NO keys in the root table by default (mouse aside) — in
/// particular PPage/PageUp is bound only in the PREFIX table, so a bare
/// PageUp must reach the running application (pagers, editors) untouched
/// (issue #488). Users can restore the old behavior with
/// `bind-key -n PageUp copy-mode -u`.
pub const ROOT_DEFAULTS: &[(&str, &str)] = &[];

/// Default prefix-table keybindings.
/// Each entry is `(key_string, command_string)`.
/// The overlay and `list-keys` both use this as the canonical source
/// of truth, so there is exactly *one* place to update.
pub const PREFIX_DEFAULTS: &[(&str, &str)] = &[
    // ── Send prefix (tmux: bind C-b send-prefix) ──
    // Pressing the prefix key twice forwards a literal prefix keystroke to the
    // active pane, which lets shells like nushell/bash interpret C-a as
    // "go to start of line" even when C-a is the psmux prefix.  When the user
    // changes the prefix via `set -g prefix <key>`, the new key is also bound
    // to send-prefix automatically (see config::ensure_prefix_self_binding).
    ("C-b",     "send-prefix"),

    // ── Window management ──
    ("c",       "new-window"),
    ("n",       "next-window"),
    ("p",       "previous-window"),
    ("l",       "last-window"),
    ("w",       "choose-tree"),
    ("&",       "confirm-before -p 'kill-window #W? (y/n)' kill-window"),
    (",",       "rename-window"),
    ("'",       "select-window-index"),
    // tmux binds `.` by default: prompt for an index and move the current
    // window there. Measured on tmux 3.6a, `list-keys -T prefix` renders it as
    //     command-prompt -T target { move-window -t "%%" }
    // psmux has no `{}` command blocks, so the same command is written in the
    // quoting form psmux's parser reads. parse_command_line strips the quotes
    // again before move-window sees the index, so the two are equivalent.
    (".",       "command-prompt -T target \"move-window -t '%%'\""),
    ("0",       "select-window -t :0"),
    ("1",       "select-window -t :1"),
    ("2",       "select-window -t :2"),
    ("3",       "select-window -t :3"),
    ("4",       "select-window -t :4"),
    ("5",       "select-window -t :5"),
    ("6",       "select-window -t :6"),
    ("7",       "select-window -t :7"),
    ("8",       "select-window -t :8"),
    ("9",       "select-window -t :9"),

    // ── Pane splitting ──
    ("%",       "split-window -h"),
    ("\"",      "split-window -v"),

    // ── Pane navigation ──
    ("Up",      "select-pane -U"),
    ("Down",    "select-pane -D"),
    ("Left",    "select-pane -L"),
    ("Right",   "select-pane -R"),
    ("o",       "select-pane -t +"),
    (";",       "last-pane"),
    ("q",       "display-panes"),

    // ── Pane management ──
    ("x",       "confirm-before -p 'kill-pane #P? (y/n)' kill-pane"),
    ("z",       "resize-pane -Z"),
    ("{",       "swap-pane -U"),
    ("}",       "swap-pane -D"),
    ("!",       "break-pane"),

    // ── Pane resize (Ctrl+Arrow = 1 cell) ──
    ("C-Up",    "resize-pane -U"),
    ("C-Down",  "resize-pane -D"),
    ("C-Left",  "resize-pane -L"),
    ("C-Right", "resize-pane -R"),

    // ── Pane resize (Alt+Arrow = 5 cells) ──
    ("M-Up",    "resize-pane -U 5"),
    ("M-Down",  "resize-pane -D 5"),
    ("M-Left",  "resize-pane -L 5"),
    ("M-Right", "resize-pane -R 5"),

    // ── Layout ──
    ("Space",   "next-layout"),
    ("M-1",     "select-layout even-horizontal"),
    ("M-2",     "select-layout even-vertical"),
    ("M-3",     "select-layout main-horizontal"),
    ("M-4",     "select-layout main-vertical"),
    ("M-5",     "select-layout tiled"),

    // ── Session ──
    ("d",       "detach-client"),
    ("$",       "rename-session"),

    // ── Copy / Paste ──
    ("[",       "copy-mode"),
    // tmux: bind PPage { copy-mode -u } — enter copy mode scrolled up one
    // page. The root table deliberately has no PageUp binding (issue #488).
    ("PageUp",  "copy-mode -u"),
    ("]",       "paste-buffer"),
    ("=",       "choose-buffer"),
    ("#",       "list-buffers"),

    // ── Misc ──
    (":",       "command-prompt"),
    ("?",       "list-keys"),
    ("i",       "display-message"),
    ("t",       "clock-mode"),
    ("s",       "choose-session"),
    ("(",       "switch-client -p"),
    (")",       "switch-client -n"),
    ("v",       "rectangle-toggle"),
    ("y",       "copy-yank"),
];

// ─────────────────────────────────────────────────────────────────────
// Sections below are used *only* by the overlay — they don't affect
// key dispatching at all (that lives in input.rs).
// ─────────────────────────────────────────────────────────────────────

/// Section header + lines for copy-mode (vi) keybindings shown in the
/// overlay.
pub fn copy_mode_vi_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── copy-mode-vi ──────────────────────────────────────────".into());
    for (k, desc) in COPY_MODE_VI {
        v.push(format!("bind-key -T copy-mode-vi {} {}", k, desc));
    }
    v
}

const COPY_MODE_VI: &[(&str, &str)] = &[
    // Exit
    ("Escape",    "cancel (exit copy mode)"),
    ("q",         "cancel (exit copy mode)"),
    // Cursor movement
    ("h",         "cursor-left"),
    ("j",         "cursor-down"),
    ("k",         "cursor-up"),
    ("l",         "cursor-right"),
    ("Left",      "cursor-left"),
    ("Down",      "cursor-down"),
    ("Up",        "cursor-up"),
    ("Right",     "cursor-right"),
    // Words
    ("w",         "next-word"),
    ("b",         "previous-word"),
    ("e",         "next-word-end"),
    ("W",         "next-space"),
    ("B",         "previous-space"),
    ("E",         "next-space-end"),
    // Line
    ("0",         "start-of-line"),
    ("$",         "end-of-line"),
    ("^",         "back-to-indentation"),
    ("Home",      "start-of-line"),
    ("End",       "end-of-line"),
    // Scrolling
    ("C-u",       "halfpage-up"),
    ("C-d",       "halfpage-down"),
    ("C-b",       "page-up"),
    ("C-f",       "page-down"),
    ("PageUp",    "page-up"),
    ("PageDown",  "page-down"),
    // Document
    ("g",         "history-top"),
    ("G",         "history-bottom"),
    // Screen position
    ("H",         "top-line"),
    ("M",         "middle-line"),
    ("L",         "bottom-line"),
    // Find char
    ("f{char}",   "jump-forward"),
    ("F{char}",   "jump-backward"),
    ("t{char}",   "jump-to-forward"),
    ("T{char}",   "jump-to-backward"),
    (";",         "jump-again"),
    (",",         "jump-reverse"),
    // Mark
    ("X",         "set-mark"),
    ("M-x",       "jump-to-mark"),
    ("r",         "refresh-from-pane"),
    // Bracket / paragraph
    ("%",         "next-matching-bracket"),
    ("{",         "previous-paragraph"),
    ("}",         "next-paragraph"),
    ("z",         "scroll-middle"),
    // Selection
    ("v",         "rectangle-toggle"),
    ("V",         "select-line"),
    ("C-v",       "rectangle-toggle"),
    ("Space",     "begin-selection"),
    ("o",         "other-end (swap cursor/anchor)"),
    // Yank
    ("y",         "copy-selection-and-cancel"),
    ("Enter",     "copy-selection-and-cancel"),
    ("D",         "copy-end-of-line-and-cancel"),
    ("A",         "append-selection-and-cancel"),
    // Search
    ("/",         "search-forward"),
    ("?",         "search-backward"),
    ("n",         "search-again"),
    ("N",         "search-reverse"),
    // Registers / text objects
    ("\"{a-z}",   "set register for next yank"),
    ("aw",        "select-word (a word)"),
    ("iw",        "select-word (inner word)"),
    // Count prefix
    ("1-9",       "numeric prefix for motions"),
];

/// Section for copy-mode search bindings.
pub fn copy_search_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── copy-mode search ──────────────────────────────────────".into());
    for (k, desc) in COPY_SEARCH {
        v.push(format!("bind-key -T copy-mode-search {} {}", k, desc));
    }
    v
}

const COPY_SEARCH: &[(&str, &str)] = &[
    ("Escape",    "cancel search"),
    ("Enter",     "accept search / jump to match"),
    ("Backspace", "delete character"),
    ("{char}",    "append character to search pattern"),
];

/// Section for command-prompt bindings.
pub fn command_prompt_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── command-prompt ─────────────────────────────────────────".into());
    for (k, desc) in COMMAND_PROMPT {
        v.push(format!("  {} {}", k, desc));
    }
    v
}

const COMMAND_PROMPT: &[(&str, &str)] = &[
    ("Escape",    "cancel"),
    ("Enter",     "execute command (saved to history)"),
    ("Backspace", "delete char before cursor"),
    ("Delete",    "delete char at cursor"),
    ("Left",      "move cursor left"),
    ("Right",     "move cursor right"),
    ("Home",      "move cursor to start"),
    ("End",       "move cursor to end"),
    ("Up",        "history: older command"),
    ("Down",      "history: newer command"),
    ("C-a",       "move cursor to start"),
    ("C-e",       "move cursor to end"),
    ("C-u",       "kill line (clear to start)"),
    ("C-k",       "kill to end of line"),
    ("C-w",       "delete word backwards"),
];

/// Section: CLI command quick-reference (user-facing commands only).
pub fn cli_command_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── commands ───────────────────────────────────────────────".into());
    v.push("  (alias)               description".into());
    for (name, alias, desc) in CLI_COMMANDS {
        if alias.is_empty() {
            v.push(format!("  {:<24}{}", name, desc));
        } else {
            v.push(format!("  {:<13}({:<9}) {}", name, alias, desc));
        }
    }
    v
}

/// `(command_name, alias, description)` — only user-facing commands.
const CLI_COMMANDS: &[(&str, &str, &str)] = &[
    // Session
    ("attach-session",    "attach",   "Attach to an existing session"),
    ("detach-client",     "detach",   "Detach from the current session"),
    ("has-session",       "has",      "Check if a session exists"),
    ("kill-server",       "",         "Kill the server and all sessions"),
    ("kill-session",      "",         "Destroy a session"),
    ("list-sessions",     "ls",       "List sessions"),
    ("new-session",       "new",      "Create a new session"),
    ("rename-session",    "rename",   "Rename the current session"),
    ("switch-client",     "switchc",  "Switch to another session"),
    // Window
    ("choose-tree",       "",         "Interactive session/window chooser"),
    ("find-window",       "findw",    "Search for a window by name"),
    ("kill-window",       "killw",    "Destroy the current window"),
    ("last-window",       "last",     "Select the previous window"),
    ("link-window",       "linkw",    "Link window into another session"),
    ("list-windows",      "lsw",      "List windows"),
    ("move-window",       "movew",    "Move window to another index"),
    ("new-window",        "neww",     "Create a new window"),
    ("next-window",       "next",     "Move to the next window"),
    ("previous-window",   "prev",     "Move to the previous window"),
    ("rename-window",     "renamew",  "Rename the current window"),
    ("resize-window",     "resizew",  "Resize a window"),
    ("respawn-window",    "respawnw", "Restart the process in a window"),
    ("rotate-window",     "rotatew",  "Rotate pane positions"),
    ("select-window",     "selectw",  "Select a window by index"),
    ("swap-window",       "swapw",    "Swap two windows"),
    ("unlink-window",     "unlinkw",  "Unlink a window from the session"),
    // Pane
    ("break-pane",        "breakp",   "Break pane out to a new window"),
    ("capture-pane",      "capturep", "Capture pane contents to buffer"),
    ("display-panes",     "displayp", "Show pane numbers"),
    ("join-pane",         "joinp",    "Move a pane into another window"),
    ("kill-pane",         "killp",    "Kill the active pane"),
    ("last-pane",         "lastp",    "Select the previously active pane"),
    ("move-pane",         "movep",    "Move a pane to another window"),
    ("pipe-pane",         "pipep",    "Pipe pane output to a command"),
    ("resize-pane",       "resizep",  "Resize a pane (-Z to zoom)"),
    ("respawn-pane",      "respawnp", "Restart the process in a pane"),
    ("select-pane",       "selectp",  "Select/focus a pane"),
    ("split-window",      "splitw",   "Split current pane"),
    ("swap-pane",         "swapp",    "Swap two panes"),
    // Layout
    ("next-layout",       "nextl",    "Cycle to the next layout"),
    ("previous-layout",   "prevl",    "Cycle to the previous layout"),
    ("select-layout",     "selectl",  "Apply a layout preset"),
    // Copy / Paste
    ("choose-buffer",     "chooseb",  "Interactive buffer chooser"),
    ("clear-history",     "clearhist","Clear pane scrollback"),
    ("copy-mode",         "",         "Enter copy mode"),
    ("delete-buffer",     "deleteb",  "Delete a paste buffer"),
    ("list-buffers",      "lsb",      "List paste buffers"),
    ("load-buffer",       "loadb",    "Load buffer from file"),
    ("paste-buffer",      "pasteb",   "Paste buffer into pane"),
    ("save-buffer",       "saveb",    "Save buffer to file"),
    ("set-buffer",        "setb",     "Set a buffer's contents"),
    ("show-buffer",       "showb",    "Show buffer contents"),
    // Key binding
    ("bind-key",          "bind",     "Bind a key to a command"),
    ("list-keys",         "lsk",      "List key bindings"),
    ("unbind-key",        "unbind",   "Unbind a key"),
    // Configuration
    ("set-option",        "set",      "Set a session/server option"),
    ("set-window-option", "setw",     "Set a window option"),
    ("show-options",      "show",     "Show options"),
    ("show-window-options","showw",   "Show window options"),
    ("source-file",       "source",   "Load config file"),
    // Display / Info
    ("clock-mode",        "",         "Show a large clock"),
    ("command-prompt",    "",         "Open the command prompt"),
    ("display-menu",      "menu",     "Display an interactive menu"),
    ("display-message",   "display",  "Display a message / pane info"),
    ("display-popup",     "popup",    "Display a popup window"),
    ("list-commands",     "lscm",     "List available commands"),
    ("server-info",       "info",     "Show server information"),
    // Misc
    ("confirm-before",    "confirm",  "Confirm before running command"),
    ("if-shell",          "if",       "Conditional command execution"),
    ("list-clients",      "lsc",      "List connected clients"),
    ("refresh-client",    "refresh",  "Refresh the client display"),
    ("run-shell",         "run",      "Run a shell command"),
    ("send-keys",         "send",     "Send keys/text to a pane"),
    ("set-environment",   "setenv",   "Set an environment variable"),
    ("set-hook",          "",         "Set a hook on an event"),
    ("show-environment",  "showenv",  "Show environment variables"),
    ("show-hooks",        "",         "Show defined hooks"),
    ("show-messages",     "showmsgs", "Show server message log"),
    ("wait-for",          "wait",     "Wait/signal a named channel"),
];

/// Section: configurable options quick-reference.
pub fn options_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── options (set-option / set) ──────────────────────────────".into());
    v.push("  option                      default".into());
    for (name, default) in OPTIONS_REF {
        v.push(format!("  {:<30}{}", name, default));
    }
    v
}

const OPTIONS_REF: &[(&str, &str)] = &[
    // Key
    ("prefix",                     "C-b"),
    ("prefix2",                    "none"),
    // Behaviour
    ("escape-time",                "500"),
    ("base-index",                 "0"),
    ("pane-base-index",            "0"),
    ("history-limit",              "2000"),
    ("mouse",                      "on"),
    ("mode-keys",                  "emacs"),
    ("focus-events",               "off"),
    ("remain-on-exit",             "off"),
    ("renumber-windows",           "off"),
    ("aggressive-resize",          "off"),
    ("automatic-rename",           "on"),
    ("synchronize-panes",          "off"),
    ("set-titles",                 "off"),
    ("allow-passthrough",          "off"),
    ("priority",                   "above-normal"),
    ("default-command",            "(system shell)"),
    ("word-separators",            "\" -_@\""),
    // Display timing
    ("display-time",               "750"),
    ("display-panes-time",         "1000"),
    ("status-interval",            "15"),
    // Status bar
    ("status",                     "on"),
    ("status-position",            "bottom"),
    ("status-justify",             "left"),
    ("status-left",                "\"[#S] \""),
    ("status-right",               "\"#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}\"#{=21:pane_title}\" %H:%M %d-%b-%y\""),
    ("status-left-length",         "10"),
    ("status-right-length",        "40"),
    ("status-style",               "bg=green,fg=black"),
    ("status-left-style",          "\"\""),
    ("status-right-style",         "\"\""),
    // Window status
    ("window-status-format",       "#I:#W#{...}"),
    ("window-status-current-format", "#I:#W#{...}"),
    ("window-status-separator",    "\" \""),
    ("window-status-style",        "\"\""),
    ("window-status-current-style","\"\""),
    ("window-status-activity-style","reverse"),
    ("window-status-bell-style",   "reverse"),
    ("window-status-last-style",   "\"\""),
    // Pane borders
    ("pane-border-style",          "\"\""),
    ("pane-active-border-style",   "fg=green"),
    ("pane-border-hover-style",     "fg=yellow"),
    ("pane-border-indicators",     crate::pane_border::INDICATORS_DEFAULT),
    // Messages / Modes
    ("message-style",              "bg=yellow,fg=black"),
    ("message-command-style",      "bg=black,fg=yellow"),
    ("mode-style",                 "bg=yellow,fg=black"),
    // Monitoring
    ("monitor-activity",           "off"),
    ("monitor-silence",            "0"),
    ("visual-activity",            "off"),
    ("visual-bell",                "off"),
    ("bell-action",                "any"),
    // Layout
    ("main-pane-width",            "0 (60% heuristic)"),
    ("main-pane-height",           "0 (60% heuristic)"),
    // Copy / Clipboard
    ("copy-command",               "\"\""),
    // Unicode
    ("codepoint-widths",           "\"\""),
    ("set-clipboard",              "on"),
    ("set-titles-string",          "\"\""),
    // psmux extensions
    ("cursor-style",               "\"\""),
    ("cursor-blink",               "off"),
    ("prediction-dimming",         "off"),
    ("allow-predictions",          "off"),
    ("env-shim",                   "on"),
    ("claude-code-fix-tty",        "on"),
    ("claude-code-force-interactive", "on"),
];

/// Section: format variables quick-reference.
pub fn format_vars_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── format variables (#{...}) ───────────────────────────────".into());
    for (group, vars) in FORMAT_GROUPS {
        v.push(format!("  {}:", group));
        v.push(format!("    {}", vars));
    }
    v.push(String::new());
    v.push("  Modifiers: #{=N:var} truncate, #{T:var} strftime,".into());
    v.push("    #{?test,true,false} conditional, #{==:a,b} compare,".into());
    v.push("    #{e:var} shell escape, #{b:var} basename,".into());
    v.push("    #{d:var} dirname, #{m:pat,str} match, #{s/p/r/:var} sub".into());
    v
}

const FORMAT_GROUPS: &[(&str, &str)] = &[
    ("Session", "session_name session_id session_windows session_attached session_created session_path ..."),
    ("Window",  "window_index window_name window_active window_panes window_flags window_id window_layout window_zoomed_flag ..."),
    ("Pane",    "pane_index pane_id pane_title pane_width pane_height pane_active pane_current_command pane_current_path pane_pid pane_dead ..."),
    ("Cursor",  "cursor_x cursor_y cursor_character cursor_flag"),
    ("Copy",    "copy_cursor_x copy_cursor_y copy_cursor_word copy_cursor_line selection_present search_present scroll_position"),
    ("Buffer",  "buffer_name buffer_size buffer_sample buffer_created"),
    ("Client",  "client_width client_height client_name client_session client_prefix client_pid client_termname ..."),
    ("Server",  "pid version host hostname host_short"),
    ("Misc",    "history_limit history_size alternate_on pane_mode pane_in_mode"),
];

/// Section: hooks reference.
pub fn hooks_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── hooks (set-hook) ───────────────────────────────────────".into());
    v.push("  after-new-session     after-new-window      after-kill-pane".into());
    v.push("  after-split-window    after-select-window    after-select-pane".into());
    v.push("  after-resize-pane     after-rename-window    after-rename-session".into());
    v.push("  after-select-layout   after-copy-mode        after-set-option".into());
    v.push("  after-bind-key        after-unbind-key       after-source".into());
    v.push("  after-swap-pane       after-swap-window      client-attached".into());
    v.push("  client-detached".into());
    v
}

/// Section: mouse bindings.
pub fn mouse_lines() -> Vec<String> {
    let mut v = Vec::new();
    v.push(String::new());
    v.push("── mouse bindings (when mouse is on) ──────────────────────".into());
    v.push("  Left click status tab    switch to clicked window".into());
    v.push("  Left click pane          focus pane (+ forward to child)".into());
    v.push("  Left click border        begin drag-resize".into());
    v.push("  Left drag border         resize split interactively".into());
    v.push("  Scroll up/down           forward wheel to child (or copy mode scroll)".into());
    v
}

/// Build the full ordered list of lines for the C-b ? overlay.
///
/// `user_bindings` — `Vec<(repeat, table, key, command)>` from the
/// synced binding list.  Defaults that have been overridden by a user
/// binding in the prefix table are automatically excluded.
pub fn build_overlay_lines(
    user_bindings: &[(bool, String, String, String)],
    _defaults_suppressed: bool,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Since defaults are now populated in key_tables and synced as bindings,
    // all prefix bindings (defaults + user) come through user_bindings.
    // No need to separately iterate PREFIX_DEFAULTS.

    // ── 1. Prefix bindings ──
    lines.push("── prefix table (C-b + key) ───────────────────────────────".into());
    let prefix_bindings: Vec<_> = user_bindings.iter()
        .filter(|(_, t, _, _)| t == "prefix")
        .collect();
    for (repeat, table, key, cmd) in &prefix_bindings {
        let r = if *repeat { " -r" } else { "" };
        lines.push(format!("bind-key{} -T {} {} {}", r, table, key, cmd));
    }

    // ── 2. Non-prefix user bindings ──
    let non_prefix: Vec<_> = user_bindings.iter()
        .filter(|(_, t, _, _)| t != "prefix")
        .collect();
    if !non_prefix.is_empty() {
        lines.push(String::new());
        lines.push("── other table bindings ───────────────────────────────────".into());
        for (repeat, table, key, cmd) in &non_prefix {
            let r = if *repeat { " -r" } else { "" };
            lines.push(format!("bind-key{} -T {} {} {}", r, table, key, cmd));
        }
    }

    // ── 3-8. Reference sections ──
    lines.extend(copy_mode_vi_lines());
    lines.extend(copy_search_lines());
    lines.extend(command_prompt_lines());
    lines.extend(mouse_lines());
    lines.extend(cli_command_lines());
    lines.extend(options_lines());
    lines.extend(format_vars_lines());
    lines.extend(hooks_lines());

    lines
}

/// Build the output for the CLI `list-keys` command (server-side).
///
/// `user_tables` — iterator of `(table_name, key_str, action_str, repeat)`.
/// `defaults_suppressed` — when true, skip PREFIX_DEFAULTS (set by unbind-key -a).
pub fn build_list_keys_output<'a>(
    user_tables: impl Iterator<Item = (&'a str, String, String, bool)>,
    _defaults_suppressed: bool,
) -> String {
    let mut output = String::new();

    // Since defaults are now populated in key_tables (via populate_default_bindings),
    // all bindings (defaults + user overrides) come through user_tables.
    // No need to separately prepend PREFIX_DEFAULTS.
    let user_entries: Vec<(&str, String, String, bool)> = user_tables.collect();

    for (table, key, action, repeat) in &user_entries {
        let r = if *repeat { " -r" } else { "" };
        output.push_str(&format!("bind-key{} -T {} {} {}\n", r, table, key, action));
    }

    output
}
