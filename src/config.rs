use std::env;
use std::cell::RefCell;
use crossterm::event::{KeyCode, KeyModifiers};

use crate::types::{AppState, Action, Bind};
use crate::commands::parse_command_to_action;

// Track the current config file being parsed (for #{current_file}, #{d:current_file})
thread_local! {
    static CURRENT_CONFIG_FILE: RefCell<String> = RefCell::new(String::new());
    // True while a full startup/reload config batch is loading. Used so that a
    // `source-file` directive *inside* the config does not also set a runtime
    // status message — startup warnings are surfaced once via the log/client.
    static IN_STARTUP_LOAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Get the current config file path being parsed.
pub fn current_config_file() -> String {
    CURRENT_CONFIG_FILE.with(|f| f.borrow().clone())
}

/// Set the current config file path.
fn set_current_config_file(path: &str) {
    CURRENT_CONFIG_FILE.with(|f| *f.borrow_mut() = path.to_string());
}

fn in_startup_load() -> bool {
    IN_STARTUP_LOAD.with(|c| c.get())
}

/// RAII guard that marks the startup-config-load window.
struct StartupLoadGuard;
impl Drop for StartupLoadGuard {
    fn drop(&mut self) {
        IN_STARTUP_LOAD.with(|c| c.set(false));
    }
}

/// Quick scan of the config file to check if `set -g warm off` is present.
/// Used by the client side before attempting warm server claim.
/// Read the user's effective config file content: the PSMUX_CONFIG_FILE override
/// (with ~ expansion) if set, otherwise the first existing file in the default
/// search path.  Shared by the lightweight, server-less config probes that run
/// in the CLI before any server exists.
pub fn read_user_config_content() -> Option<String> {
    // Strip a leading UTF-8 BOM (Notepad / PowerShell Set-Content -Encoding UTF8
    // on Windows prepend EF BB BF) so first-line directives are recognised,
    // matching parse_config_content().
    let strip_bom = |s: String| s.strip_prefix('\u{FEFF}').map(str::to_string).unwrap_or(s);
    if let Ok(config_file) = env::var("PSMUX_CONFIG_FILE") {
        let expanded = if config_file.starts_with('~') {
            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
            config_file.replacen('~', &home, 1)
        } else {
            config_file
        };
        return std::fs::read_to_string(expanded).ok().map(strip_bom);
    }
    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
    let paths = [
        format!("{}/.psmux.conf", home),
        format!("{}/.psmuxrc", home),
        format!("{}/.tmux.conf", home),
        format!("{}/.config/psmux/psmux.conf", home),
    ];
    paths.iter().find_map(|p| std::fs::read_to_string(p).ok()).map(strip_bom)
}

/// If the user's config has a top-level `new-session` (or `new`) directive,
/// return its arguments (empty for a bare `new-session`); otherwise None.
///
/// tmux runs `new-session` from the config at server start, so a later
/// `attach-session` with no running server still finds a session to attach to.
/// psmux has no persistent server, so the CLI uses this to bootstrap that
/// session (see the attach path in main.rs, #362).  Only a simple top-level
/// directive is matched — `new-session` as the first token of a line — which is
/// the documented idiom; lines where `new-session` is an argument (e.g.
/// `bind-key x new-session`) are not matched.
pub fn config_new_session_args() -> Option<Vec<String>> {
    let content = read_user_config_content()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        let mut toks = trimmed.split_whitespace();
        match toks.next() {
            Some("new-session") | Some("new") => {
                return Some(toks.map(|s| s.to_string()).collect());
            }
            _ => {}
        }
    }
    None
}

pub fn is_warm_disabled_by_config() -> bool {
    let content = read_user_config_content();
    if let Some(content) = content {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') { continue; }
            // Match: set -g warm off, set warm off, set-option -g warm off, etc.
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 3 {
                let cmd = parts[0];
                if cmd == "set" || cmd == "set-option" {
                    // Find the option name and value, skipping flags like -g, -s, -q
                    let mut i = 1;
                    while i < parts.len() && parts[i].starts_with('-') { i += 1; }
                    if i + 1 < parts.len() && parts[i] == "warm" {
                        let val = parts[i + 1].trim_matches('"').trim_matches('\'');
                        return val == "off" || val == "false" || val == "0";
                    }
                }
            }
        }
    }
    false
}

/// The `priority` option as written in the user's config file, if it is there
/// and usable.
///
/// The attach client never runs load_config: it asks the server for the options
/// it renders with. But its own scheduling class has to be decided before it
/// has spoken to anybody, so it peeks at the file the same lightweight way
/// is_warm_disabled_by_config does (#608). Anything unparseable is simply
/// absent here; the server's full config pass is what reports it.
pub fn priority_from_config() -> Option<String> {
    let content = read_user_config_content()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        if parts[0] != "set" && parts[0] != "set-option" {
            continue;
        }
        let mut i = 1;
        while i < parts.len() && parts[i].starts_with('-') {
            i += 1;
        }
        if i + 1 < parts.len() && parts[i] == "priority" {
            let val = parts[i + 1].trim_matches('"').trim_matches('\'');
            return crate::platform::normalize_priority(val).map(|v| v.to_string());
        }
    }
    None
}

/// Populate key_tables with PREFIX_DEFAULTS and ROOT_DEFAULTS from help.rs.
/// This ensures default bindings live in key_tables (like tmux)
/// so that unbind-key <key> can actually remove them.
/// Must be called BEFORE load_config / source_file.
pub fn populate_default_bindings(app: &mut AppState) {
    let defaults = crate::help::PREFIX_DEFAULTS;
    let table = app.key_tables.entry("prefix".to_string()).or_default();
    for (key_str, cmd_str) in defaults {
        if let Some(key) = parse_key_name(key_str) {
            let key = normalize_key_for_binding(key);
            if let Some(action) = parse_command_to_action(cmd_str) {
                // Only add if not already present (user config may have overridden)
                if !table.iter().any(|b| b.key == key) {
                    table.push(Bind { key, action, repeat: false });
                }
            }
        }
    }

    // Root table defaults (e.g. PageUp -> copy-mode -u)
    let root_defaults = crate::help::ROOT_DEFAULTS;
    let root_table = app.key_tables.entry("root".to_string()).or_default();
    for (key_str, cmd_str) in root_defaults {
        if let Some(key) = parse_key_name(key_str) {
            let key = normalize_key_for_binding(key);
            if let Some(action) = parse_command_to_action(cmd_str) {
                if !root_table.iter().any(|b| b.key == key) {
                    root_table.push(Bind { key, action, repeat: false });
                }
            }
        }
    }
}

pub fn load_config(app: &mut AppState) {
    // Start a fresh warning batch for this load/reload and mark the window so
    // nested `source-file` directives don't emit a runtime status message.
    app.config_warnings.clear();
    app.config_warn_line = None;
    IN_STARTUP_LOAD.with(|c| c.set(true));
    let _startup_guard = StartupLoadGuard;

    // If -f flag was used, load that specific config file instead of default search
    if let Ok(config_file) = env::var("PSMUX_CONFIG_FILE") {
        let expanded = if config_file.starts_with('~') {
            config_file.replacen('~', &crate::paths::home_dir(), 1)
        } else {
            config_file
        };
        set_current_config_file(&expanded);
        match std::fs::read_to_string(&expanded) {
            Ok(content) => parse_config_content(app, &content),
            // An explicitly requested config file that cannot be read is a
            // mistake worth surfacing — the user named this path on purpose.
            Err(e) => {
                // Route through warn_config for consistent formatting. It prefixes
                // current_config_file (set to `expanded` above); no config line is
                // in play at initial load, so the result is `<path>: cannot read: ...`.
                warn_config(app, format!("cannot read: {}", e));
            }
        }
        set_current_config_file("");
        return;
    }

    // Use the canonical resolver rather than reading USERPROFILE/HOME directly.
    // `paths::home_dir()` deliberately demotes HOME to a last resort (issue
    // #474): under MSYS2/Git Bash, HOME is a POSIX path like /home/user, and
    // joining it with the backslash-separated names below produced
    // `/home/user\.config\psmux\psmux.conf`, which never matches anything — so
    // psmux silently started with NO config at all while every other part of it
    // used the real Windows profile directory.
    let home = crate::paths::home_dir();
    let paths = vec![
        format!("{}\\.psmux.conf", home),
        format!("{}\\.psmuxrc", home),
        format!("{}\\.tmux.conf", home),
        format!("{}\\.config\\psmux\\psmux.conf", home),
    ];
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            set_current_config_file(&path);
            parse_config_content(app, &content);
            set_current_config_file("");
            break;
        }
    }
}

pub fn parse_config_content(app: &mut AppState, content: &str) {
    // Strip UTF-8 BOM if present (common on Windows when files are saved
    // with Notepad or other editors that prepend EF BB BF).
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);

    // Process %if / %elif / %else / %endif conditional blocks.
    // These are tmux config-level directives that control which lines are parsed.
    //
    // %if "#{==:#{@option},value}"   — evaluate format condition
    // %elif "#{condition}"           — else-if branch
    // %else                          — else branch
    // %endif                         — end conditional block
    // %hidden NAME=value             — define a hidden variable (stored but not shown)
    //
    // Blocks can nest. We track a stack of (active, satisfied) states.
    // - active: whether the current block should execute lines
    // - satisfied: whether any branch of the current if/elif/else has matched
    struct IfState {
        active: bool,    // are we executing lines in this block?
        satisfied: bool, // has any branch of this if/elif/else already matched?
        parent_active: bool, // was the parent context active?
    }

    let mut if_stack: Vec<IfState> = Vec::new();

    // Join continuation lines (ending with \). Track the original 1-based line
    // number where each (possibly joined) logical line began, so config
    // warnings can be reported as `file:line: message` (issue #370 follow-up).
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut continuation = String::new();
    let mut cont_start: usize = 0;
    for (idx, line) in content.lines().enumerate() {
        let lineno = idx + 1;
        let trimmed = line.trim_end();
        if trimmed.ends_with('\\') {
            if continuation.is_empty() { cont_start = lineno; }
            continuation.push_str(trimmed.trim_end_matches('\\'));
            continuation.push(' ');
        } else {
            if !continuation.is_empty() {
                continuation.push_str(trimmed);
                lines.push((cont_start, continuation.clone()));
                continuation.clear();
            } else {
                lines.push((lineno, trimmed.to_string()));
            }
        }
    }
    if !continuation.is_empty() {
        lines.push((cont_start, continuation));
    }

    // Brace-block collection state for if-shell 'cond' { ... } syntax.
    // When we encounter a line like `if-shell 'false' {`, we collect
    // subsequent lines until the matching `}` and only execute them
    // if the condition is true. Supports an optional else block:
    //   if-shell 'cond' { ... } { ... }
    use parse_config_content_types::BraceBlock;
    let mut brace_block: Option<BraceBlock> = None;

    for (orig_lineno, line) in &lines {
        app.config_warn_line = Some(*orig_lineno);
        let l = line.trim();

        // Skip empty lines and comments (but comments start with # not %)
        if l.is_empty() {
            // Still collect empty lines inside brace blocks to preserve structure
            if let Some(ref mut bb) = brace_block {
                if bb.in_else { bb.else_lines.push(String::new()); }
                else { bb.true_lines.push(String::new()); }
            }
            continue;
        }

        // --- Brace-block collection ---
        if brace_block.is_some() {
            let bb = brace_block.as_mut().unwrap();
            // Count braces to handle nesting
            let opens = l.chars().filter(|&c| c == '{').count();
            let closes = l.chars().filter(|&c| c == '}').count();

            if l == "}" && bb.depth == 1 {
                // Closing brace at top level
                bb.depth = 0;
                // Continue to see if next line opens an else `{`.
                continue;
            } else if bb.depth == 0 && !bb.in_else && l == "{" {
                // Start of else block (on a separate line after closing `}`)
                bb.in_else = true;
                bb.depth = 1;
                continue;
            } else if bb.depth == 0 {
                // We're past the block(s). Process the collected brace block
                // and then fall through to process the current line normally.
                let finished = brace_block.take().unwrap();
                process_brace_if_shell(app, &finished);
                // Fall through to process `l` as a normal line
            } else {
                // Inside a block at depth >= 1
                bb.depth = bb.depth + opens - closes;
                if bb.in_else {
                    bb.else_lines.push(l.to_string());
                } else {
                    bb.true_lines.push(l.to_string());
                }
                continue;
            }
        }

        // --- Check if this line starts an if-shell brace block ---
        if (l.starts_with("if-shell ") || l.starts_with("if ")) && l.ends_with('{') {
            // Check %if stack — only start brace block if active
            let active = if_stack.last().map(|s| s.active).unwrap_or(true);
            if active {
                brace_block = Some(BraceBlock {
                    if_line: l.to_string(),
                    true_lines: Vec::new(),
                    else_lines: Vec::new(),
                    depth: 1,
                    in_else: false,
                });
            }
            continue;
        }

        // Handle %-directives before checking for # comments
        if l.starts_with('%') {
            if l.starts_with("%if ") || l.starts_with("%if\t") {
                let condition = l[3..].trim().trim_matches('"').trim_matches('\'');

                // Evaluate the condition using format expansion
                let parent_active = if_stack.last().map(|s| s.active).unwrap_or(true);
                let result = if parent_active {
                    let expanded = crate::format::expand_format(condition, app);
                    is_truthy_config(&expanded)
                } else {
                    false
                };

                if_stack.push(IfState {
                    active: parent_active && result,
                    satisfied: result,
                    parent_active,
                });
                continue;
            }

            if l.starts_with("%elif ") || l.starts_with("%elif\t") {
                if let Some(state) = if_stack.last_mut() {
                    let condition = l[5..].trim().trim_matches('"').trim_matches('\'');
                    if state.parent_active && !state.satisfied {
                        let expanded = crate::format::expand_format(condition, app);
                        let result = is_truthy_config(&expanded);
                        state.active = result;
                        if result { state.satisfied = true; }
                    } else {
                        state.active = false;
                    }
                }
                continue;
            }

            if l == "%else" {
                if let Some(state) = if_stack.last_mut() {
                    state.active = state.parent_active && !state.satisfied;
                    state.satisfied = true; // prevent further elif from matching
                }
                continue;
            }

            if l == "%endif" {
                if_stack.pop();
                continue;
            }

            if l.starts_with("%hidden ") {
                // %hidden NAME=VALUE — define a hidden config variable
                let rest = l[8..].trim();
                if let Some(eq_pos) = rest.find('=') {
                    let name = rest[..eq_pos].trim();
                    let value = rest[eq_pos + 1..].trim().trim_matches('"').trim_matches('\'');
                    // Only process if active
                    let active = if_stack.last().map(|s| s.active).unwrap_or(true);
                    if active {
                        app.environment.insert(name.to_string(), value.to_string());
                    }
                }
                continue;
            }

            // Unknown %-directive — skip
            continue;
        }

        // Regular line — only process if all enclosing %if blocks are active
        let active = if_stack.last().map(|s| s.active).unwrap_or(true);
        if !active { continue; }

        // Expand $NAME / ${NAME} references from %hidden variables.
        // tmux's %hidden directive defines server-level variables that are
        // expanded with $ syntax in subsequent config lines.
        let l = if l.contains('$') {
            expand_hidden_vars(l, &app.environment)
        } else {
            l.to_string()
        };

        parse_config_line(app, &l);
    }

    // Process any remaining unclosed brace block at end of file
    if let Some(finished) = brace_block.take() {
        process_brace_if_shell(app, &finished);
    }

    // Clear the line context so a later single runtime command does not inherit
    // a stale `file:line:` prefix.
    app.config_warn_line = None;
}

/// Process a collected if-shell brace block by evaluating the condition
/// and executing the appropriate branch.
fn process_brace_if_shell(app: &mut AppState, bb: &parse_config_content_types::BraceBlock) {
    // Extract the condition from the if-shell line (strip "if-shell " or "if " and trailing "{")
    let line = bb.if_line.trim();
    let rest = if line.starts_with("if-shell ") {
        &line[9..]
    } else if line.starts_with("if ") {
        &line[3..]
    } else {
        return;
    };
    let rest = rest.trim().trim_end_matches('{').trim();

    // Parse flags and extract condition
    let parts: Vec<&str> = rest.split_whitespace().collect();
    let mut format_mode = false;
    let mut condition = String::new();
    let mut i = 0;
    while i < parts.len() {
        match parts[i] {
            "-b" => {}
            "-F" => { format_mode = true; }
            "-bF" | "-Fb" => { format_mode = true; }
            "-t" => { i += 1; } // skip target
            s => {
                // Handle quoted string
                if s.starts_with('"') || s.starts_with('\'') {
                    let quote = s.chars().next().unwrap();
                    if s.ends_with(quote) && s.len() > 1 {
                        condition = s[1..s.len()-1].to_string();
                    } else {
                        let mut buf = s[1..].to_string();
                        i += 1;
                        while i < parts.len() {
                            buf.push(' ');
                            buf.push_str(parts[i]);
                            if parts[i].ends_with(quote) {
                                buf.truncate(buf.len() - 1);
                                break;
                            }
                            i += 1;
                        }
                        condition = buf;
                    }
                } else {
                    condition = s.to_string();
                }
                break;
            }
        }
        i += 1;
    }

    if condition.is_empty() { return; }

    // Evaluate the condition
    let success = if format_mode {
        let expanded = crate::format::expand_format(&condition, app);
        !expanded.is_empty() && expanded != "0"
    } else if condition == "true" || condition == "1" {
        true
    } else if condition == "false" || condition == "0" {
        false
    } else {
        let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
        let mut c = std::process::Command::new(&shell_prog);
        for a in &shell_args { c.arg(a); }
        c.arg(&condition);
        { use crate::platform::HideWindowCommandExt; c.hide_window(); }
        c.status().map(|s| s.success()).unwrap_or(false)
    };

    // Execute the appropriate branch
    let lines = if success { &bb.true_lines } else { &bb.else_lines };
    for line in lines {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') { continue; }
        parse_config_line(app, l);
    }
}

/// Types used internally by parse_config_content (declared in a module to
/// allow process_brace_if_shell to reference BraceBlock by path).
mod parse_config_content_types {
    pub struct BraceBlock {
        pub if_line: String,
        pub true_lines: Vec<String>,
        pub else_lines: Vec<String>,
        pub depth: usize,
        pub in_else: bool,
    }
}

/// Expand `$NAME` and `${NAME}` references to %hidden variable values.
/// Only expand if the variable exists in the environment map (which stores
/// both %hidden variables and @user-options without the @ prefix).
fn expand_hidden_vars(line: &str, env: &std::collections::HashMap<String, String>) -> String {
    let mut result = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'$' {
            // Check for ${NAME} syntax
            if i + 1 < len && bytes[i + 1] == b'{' {
                if let Some(close) = line[i + 2..].find('}') {
                    let name = &line[i + 2..i + 2 + close];
                    if let Some(val) = env.get(name) {
                        result.push_str(val);
                    } else {
                        // Not found — keep as literal
                        result.push_str(&line[i..i + 2 + close + 1]);
                    }
                    i = i + 2 + close + 1;
                    continue;
                }
            }
            // Check for $NAME syntax (NAME = [A-Z_][A-Z0-9_]*)
            let start = i + 1;
            let mut end = start;
            while end < len && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                let name = &line[start..end];
                if let Some(val) = env.get(name) {
                    result.push_str(val);
                    i = end;
                    continue;
                }
            }
            // Not a recognized variable — keep literal $
            result.push('$');
            i += 1;
        } else {
            // Advance by full UTF-8 character (not single byte) to preserve
            // multi-byte chars like ▶ (U+25B6, 3 bytes) and ◀ (U+25C0).
            if let Some(ch) = line[i..].chars().next() {
                result.push(ch);
                i += ch.len_utf8();
            } else {
                i += 1;
            }
        }
    }
    result
}

/// Check if a config-level condition result is truthy
fn is_truthy_config(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s != "0"
}

/// Record a config parse warning, prefixed with `file:line:` when that context
/// is known (issue #370 follow-up — surface problems instead of swallowing).
pub(crate) fn warn_config(app: &mut AppState, msg: impl Into<String>) {
    let m = msg.into();
    let file = current_config_file();
    let full = match (file.is_empty(), app.config_warn_line) {
        (false, Some(line)) => format!("{}:{}: {}", file, line, m),
        (false, None) => format!("{}: {}", file, m),
        _ => m,
    };
    app.config_warnings.push(full);
}

/// True if `name` is a recognized psmux command (canonical name or alias),
/// per the TMUX_COMMANDS catalog plus any user-defined command-aliases.
/// Used so the unknown-command warning does not fire on valid commands that
/// the config parser itself does not route (e.g. `new-window`, `display-message`).
pub(crate) fn is_known_command(app: &AppState, name: &str) -> bool {
    if app.command_aliases.contains_key(name) {
        return true;
    }
    crate::server::helpers::TMUX_COMMANDS.iter().any(|entry| {
        let (cmd, alias) = match entry.split_once(" (") {
            Some((c, a)) => (c, a.trim_end_matches(')')),
            None => (*entry, ""),
        };
        cmd == name || (!alias.is_empty() && alias == name)
    })
}

/// Strip a trailing `# comment` from a single config line.
///
/// In tmux a `#` begins a comment only when it is unquoted and appears at the
/// start of the line or immediately after whitespace. A `#` inside single or
/// double quotes (e.g. a `"#{...}"` format or `'#H'`), one escaped with a
/// backslash, or one in the middle of a word (e.g. `colour#aabbcc`) is left
/// untouched. This lets configs use trailing comments such as:
///
/// ```text
/// set -g base-index 1   # start windows at 1
/// ```
///
/// Issue #416.
fn strip_inline_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_ws = true; // start of line behaves like a word boundary
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // Backslash escapes the next byte (outside single quotes), matching
            // tmux's tokeniser. Skip both bytes so an escaped `#` stays literal.
            b'\\' if !in_single => {
                i += 2;
                prev_ws = false;
                continue;
            }
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && prev_ws => {
                return line[..i].trim_end();
            }
            _ => {}
        }
        prev_ws = matches!(bytes[i], b' ' | b'\t');
        i += 1;
    }
    line
}

pub fn parse_config_line(app: &mut AppState, line: &str) {
    let l = line.trim();
    if l.is_empty() || l.starts_with('#') { return; }

    // Strip a trailing inline `# comment` (issue #416). Done after the
    // leading-`#` check above so whole-line comments are still skipped, and
    // before any directive dispatch so trailing comments don't leak into
    // command arguments.
    let l = strip_inline_comment(l).trim_end();
    if l.is_empty() { return; }

    let l = if l.ends_with('\\') {
        l.trim_end_matches('\\').trim()
    } else {
        l
    };
    
    if l.starts_with("set-option ") || l.starts_with("set ") {
        parse_set_option(app, l);
    }
    else if l.starts_with("setw ") || l.starts_with("set-window-option ") {
        // setw maps to the same option parser (tmux window options overlap)
        parse_set_option(app, l);
    }
    else if l.starts_with("bind-key ") || l.starts_with("bind ") {
        parse_bind_key(app, l);
    }
    else if l.starts_with("unbind-key ") || l.starts_with("unbind ") {
        parse_unbind_key(app, l);
    }
    else if l.starts_with("source-file ") || l.starts_with("source ") {
        let parts: Vec<&str> = l.splitn(2, ' ').collect();
        if parts.len() > 1 {
            source_file(app, parts[1].trim());
        }
    }
    else if l.starts_with("run-shell ") || l.starts_with("run ") {
        parse_run_shell(app, l);
    }
    else if l.starts_with("if-shell ") || l.starts_with("if ") {
        parse_if_shell(app, l);
    }
    else if l.starts_with("set-hook ") {
        // Parse set-hook: set-hook [-g] [-a] [-u] hook-name [command]
        let parts: Vec<&str> = l.split_whitespace().collect();
        let mut i = 1;
        let mut unset = false;
        let mut append = false;
        while i < parts.len() && parts[i].starts_with('-') {
            if parts[i].contains('u') { unset = true; }
            if parts[i].contains('a') { append = true; }
            i += 1;
        }
        if unset {
            // set-hook -gu <hook-name>  →  remove the hook
            if i < parts.len() {
                app.hooks.remove(parts[i]);
            }
        } else if i + 1 < parts.len() {
            let hook = parts[i].to_string();
            let cmd = parts[i+1..].join(" ");
            // Strip matching outer quotes (single or double) that wrap the command
            let cmd = {
                let trimmed = cmd.trim();
                let bytes = trimmed.as_bytes();
                if bytes.len() >= 2 {
                    let first = bytes[0];
                    let last = bytes[bytes.len() - 1];
                    if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
                        trimmed[1..trimmed.len()-1].to_string()
                    } else {
                        cmd
                    }
                } else {
                    cmd
                }
            };
            if append {
                // -a/-ga: append to existing hook list (tmux multi-handler).
                // Dedup identical commands so a re-sourced config does not
                // accumulate duplicate handlers that fire every tick (issue
                // #459); mirrors the replace path's issue #133 guard.
                let entry = app.hooks.entry(hook).or_insert_with(Vec::new);
                if !entry.contains(&cmd) {
                    entry.push(cmd);
                }
            } else {
                // Replace (not append) to match tmux – prevents duplicates on
                // config reload (issue #133).
                app.hooks.insert(hook, vec![cmd]);
            }
        }
    }
    else if l.starts_with("set-environment ") || l.starts_with("setenv ") {
        let parts: Vec<&str> = l.split_whitespace().collect();
        let mut i = 1;
        while i < parts.len() && parts[i].starts_with('-') { i += 1; }
        if i + 1 < parts.len() {
            let val = parts[i+1..].join(" ");
            app.environment.insert(parts[i].to_string(), val.clone());
            // Also set on the server process so child panes inherit via env block
            std::env::set_var(parts[i], &val);
        } else {
            warn_config(app, "set-environment requires a name and value");
        }
    }
    else {
        // Unrecognized directive. Warn only when the first token is not a known
        // command (a known-but-unrouted command like `new-window` stays silent
        // to match prior behavior; a genuine typo like `bnid-key` is surfaced).
        let cmd = l.split_whitespace().next().unwrap_or("");
        if !cmd.is_empty() && !is_known_command(app, cmd) {
            warn_config(app, format!("unknown command: {}", cmd));
        }
    }
}

/// Strip one layer of matching wrapping quotes, if the whole string carries
/// them. This is the pre-#536 fallback and is kept for the shapes the
/// quote-aware scan below deliberately does not claim.
fn strip_wrapping_quotes(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

/// Split a config line into whitespace-separated tokens, treating a quoted run
/// as a single token, and record the byte offset where each token starts.
///
/// The offsets are the point of this: they let the caller recover the value
/// **verbatim** from the original line. `split_whitespace()` discards where the
/// runs of whitespace were, which is what made a quoted gap unrecoverable
/// (#536).
fn tokens_with_offsets(line: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut cur = String::new();
    let mut start = 0usize;
    let mut started = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut it = line.char_indices();
    while let Some((idx, c)) = it.next() {
        if !started && !c.is_whitespace() {
            start = idx;
            started = true;
        }
        match c {
            // Keep the escape pair intact; the value scan resolves it later.
            '\\' if in_double => {
                cur.push(c);
                if let Some((_, n)) = it.next() {
                    cur.push(n);
                }
            }
            '\'' if !in_double => {
                in_single = !in_single;
                cur.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                cur.push(c);
            }
            _ if c.is_whitespace() && !in_single && !in_double => {
                if started {
                    out.push((start, std::mem::take(&mut cur)));
                    started = false;
                }
            }
            _ => cur.push(c),
        }
    }
    if started {
        out.push((start, cur));
    }
    out
}

/// Resolve a `set-option` value from the remainder of a config line.
///
/// A value wrapped in matching quotes that close at end of line is taken
/// byte-exact, so `set -g @x "A     B"` keeps all five spaces and
/// `"   leading"` keeps its indent. Inside double quotes `\"` and `\\` are
/// unescaped, matching both tmux and the tokenizer the CLI path already uses.
/// Anything else (a bare word, or a quote that does not close the line) falls
/// back to the old behaviour: verbatim with trailing whitespace removed and one
/// layer of wrapping quotes stripped.
fn extract_option_value(rest: &str) -> String {
    let rest = rest.trim_end();
    let mut chars = rest.char_indices();
    let quote = match chars.next() {
        Some((_, c)) if c == '"' || c == '\'' => c,
        Some(_) => return rest.to_string(),
        None => return String::new(),
    };
    let mut out = String::new();
    while let Some((idx, c)) = chars.next() {
        // Only double quotes process escapes, matching tmux and
        // commands::parse_command_line.
        if quote == '"' && c == '\\' {
            if let Some((_, n)) = chars.clone().next() {
                if n == '"' || n == '\\' {
                    out.push(n);
                    chars.next();
                    continue;
                }
            }
            out.push(c);
            continue;
        }
        if c == quote {
            // Only a closing quote that ends the line delimits the whole
            // value. Trailing content means this is some other shape (a
            // chained command, two quoted words), so leave it to the fallback
            // rather than silently claiming half of it.
            if rest[idx + c.len_utf8()..].trim().is_empty() {
                return out;
            }
            return strip_wrapping_quotes(rest).to_string();
        }
        out.push(c);
    }
    // Unterminated quote: behave as before.
    strip_wrapping_quotes(rest).to_string()
}

fn parse_set_option(app: &mut AppState, line: &str) {
    let toks = tokens_with_offsets(line);
    if toks.len() < 2 { warn_config(app, "set-option requires an option name"); return; }

    let mut i = 1;
    let mut is_global = false;
    let mut format_expand = false;  // -F: expand format strings in value
    let mut only_if_unset = false;  // -o: only set if not already set
    let mut append_mode = false;    // -a: append to current value
    let mut unset_mode = false;     // -u: unset (reset to default)

    while i < toks.len() {
        let p = toks[i].1.as_str();
        if p.starts_with('-') {
            if p.contains('g') { is_global = true; }
            if p.contains('F') { format_expand = true; }
            if p.contains('o') { only_if_unset = true; }
            if p.contains('a') { append_mode = true; }
            // -U is an unset alias of -u (tmux parity, #553); the contains
            // check is case-sensitive so both must be tested.
            if p.contains('u') || p.contains('U') { unset_mode = true; }
            // -q (quiet): no-op — we don't produce errors for unknown options
            // -w: window option — treat same as global for our single-server model
            i += 1;
            if p.contains('t') && i < toks.len() { i += 1; }
        } else {
            break;
        }
    }

    if i >= toks.len() { warn_config(app, "set-option requires an option name"); return; }

    // Extract key, then take the value from the ORIGINAL line starting at the
    // next token's offset, so quoted whitespace survives (#536).
    let key = strip_wrapping_quotes(&toks[i].1).to_string();
    let key = key.as_str();
    let raw_value = match toks.get(i + 1) {
        Some((off, _)) => extract_option_value(&line[*off..]),
        None => String::new(),
    };

    // Handle -u (unset): reset option to empty
    if unset_mode {
        parse_option_value(app, key, "", is_global);
        return;
    }

    // No value provided: toggle boolean options (tmux parity #278)
    if raw_value.is_empty() && !unset_mode && !append_mode {
        if crate::server::options::is_boolean_option(key) {
            crate::server::options::toggle_option(app, key);
            app.user_set_options.insert(key.to_string());
            return;
        }
    }

    // Handle -o (only set if not currently set)
    if only_if_unset {
        // For @-prefixed user options, check if key exists
        // For built-in options, check the user_set_options tracker
        let already_set = if key.starts_with('@') {
            app.user_options.contains_key(key)
        } else {
            app.user_set_options.contains(key)
        };
        if already_set { return; }
    }

    // Expand format strings in the value if -F flag is set. No quote trimming
    // here any more: extract_option_value already resolved the quoting, so a
    // value whose content legitimately begins and ends with a quote keeps it.
    let value = if format_expand && !raw_value.is_empty() {
        crate::format::expand_format(&raw_value, app)
    } else {
        raw_value
    };

    // Handle -a (append to current value)
    let final_value = if append_mode {
        let current = crate::format::lookup_option_pub(key, app).unwrap_or_default();
        format!("{}{}", current, value)
    } else {
        value
    };

    // Pass key and value separately. Rejoining them into one string could not
    // represent a value with leading or trailing spaces, which the receiver
    // then trimmed back off (#536).
    parse_option_value(app, key, &final_value, is_global);
    // Track that this option was explicitly set (for -o only-if-unset checks)
    app.user_set_options.insert(key.to_string());
}

/// Apply one option, given its name and its **exact** value.
///
/// The value arrives verbatim: the caller has already resolved quoting, so a
/// deliberate run of spaces, or leading/trailing whitespace inside a quoted
/// config value, survives to here. This used to take a single `"key value"`
/// string and re-split it, which could not represent those runs at all and
/// trimmed the ends back off (#536).
pub fn parse_option_value(app: &mut AppState, key: &str, value: &str, _is_global: bool) {
    let key = key.trim();

    // Validate the value against the option's declared type from the catalog
    // (issue #370 follow-up). Only options that exist in the catalog are
    // checked, so unmodeled/user options never produce a false warning.
    if !value.is_empty() {
        if let Some(def) = crate::server::option_catalog::OPTION_CATALOG
            .iter()
            .find(|d| d.name == key)
        {
            let v = value.trim();
            match def.option_type {
                "number" => {
                    if v.parse::<i64>().is_err() {
                        warn_config(app, format!(
                            "invalid value '{}' for option '{}' (expected a number)", v, key));
                    }
                }
                "boolean" => {
                    // Accept the usual boolean tokens, and also any integer:
                    // a few "boolean" options (e.g. `status`) also take counts.
                    let lv = v.to_ascii_lowercase();
                    let ok = matches!(lv.as_str(),
                        "on"|"off"|"true"|"false"|"1"|"0"|"yes"|"no")
                        || v.parse::<i64>().is_ok();
                    if !ok {
                        warn_config(app, format!(
                            "invalid value '{}' for option '{}' (expected on/off)", v, key));
                    }
                }
                _ => {}
            }
        }
    }

    match key {
        "status-left" => app.status_left = value.to_string(),
        "status-right" => app.status_right = value.to_string(),
        "mouse" => app.mouse_enabled = matches!(value, "on" | "true" | "1" | "yes"),
        "scroll-enter-copy-mode" => app.scroll_enter_copy_mode = matches!(value, "on" | "true" | "1" | "yes"),
        "pwsh-mouse-selection" => app.pwsh_mouse_selection = matches!(value, "on" | "true" | "1" | "yes"),
        "mouse-selection" => app.mouse_selection = matches!(value, "on" | "true" | "1" | "yes"),
        "mouse-selection-force" => app.mouse_selection_force = matches!(value, "on" | "true" | "1" | "yes"),
        "paste-detection" => app.paste_detection = matches!(value, "on" | "true" | "1" | "yes"),
        "choose-tree-preview" => app.choose_tree_preview = matches!(value, "on" | "true" | "1" | "yes"),
        // The config-file path is a SEPARATE match from apply_set_option, so an
        // option that has to do something (rather than just store a field)
        // needs an arm in both. Same as bold-is-bright just below.
        // The generic catalog check above validates only "number" and
        // "boolean", so the restricted value set is enforced here by hand and
        // a bad one warns instead of silently falling back (#608).
        "priority" => match crate::platform::normalize_priority(value) {
            Some(_) => {
                app.priority = crate::platform::resolve_priority(Some(value), false);
                crate::platform::set_process_priority(&app.priority);
            }
            None => warn_config(app, format!(
                "invalid value '{}' for option 'priority' (expected {})",
                value.trim(),
                crate::platform::PRIORITY_VALUES.join(", ")
            )),
        },
        "bold-is-bright" => {
            app.bold_is_bright = matches!(value, "on" | "true" | "1" | "yes");
            crate::platform::set_bold_is_bright(app.bold_is_bright);
        }
        "prefix" => {
            if let Some(key) = parse_key_name(value) {
                app.prefix_key = key;
                ensure_prefix_self_binding(app);
            }
        }
        "prefix2" => {
            if value == "none" || value.is_empty() {
                app.prefix2_key = None;
            } else if let Some(key) = parse_key_name(value) {
                app.prefix2_key = Some(key);
            }
        }
        "escape-time" => {
            if let Ok(ms) = value.parse::<u64>() {
                app.escape_time_ms = ms;
            }
        }
        // Issue #606: repeat-time was documented, catalogued and honoured by
        // the server's set-option, but this match had no arm for it, so a
        // `.tmux.conf` line fell through to the catch-all below, was reported
        // as "unknown option 'repeat-time'" and left `repeat_time_ms` at its
        // 500 ms default. tmux (options-table.c) bounds it to
        // 0..=REPEAT_TIME_MAX_MS milliseconds; 0 disables repeat entirely.
        "repeat-time" => match value.parse::<i64>() {
            Ok(ms) if (0..=crate::server::options::REPEAT_TIME_MAX_MS).contains(&ms) => {
                app.repeat_time_ms = ms as u64;
            }
            Ok(ms) if ms < 0 => warn_config(app, format!("value is too small: {}", value)),
            Ok(_) => warn_config(app, format!("value is too large: {}", value)),
            // A non-numeric value already produced the catalog type warning
            // above, so do not warn about it twice.
            Err(_) => {}
        },
        "prediction-dimming" | "dim-predictions" => {
            app.prediction_dimming = !matches!(value, "off" | "false" | "0");
        }
        "cursor-style" => env::set_var("PSMUX_CURSOR_STYLE", value),
        "cursor-blink" => {
            let on = matches!(value, "on"|"true"|"1");
            env::set_var("PSMUX_CURSOR_BLINK", if on { "1" } else { "0" });
            let _ = std::io::Write::write_all(&mut std::io::stdout(), if on { b"\x1b[?12h" } else { b"\x1b[?12l" });
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
        "status" => {
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
            }
        }
        "status-style" => {
            app.status_style = value.to_string();
        }
        "status-position" => {
            app.status_position = value.to_string();
        }
        "status-interval" => {
            if let Ok(n) = value.parse::<u64>() { app.status_interval = n; }
        }
        "status-justify" => { app.status_justify = value.to_string(); }
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
        "history-limit" => {
            if let Ok(limit) = value.parse::<usize>() {
                app.history_limit = limit;
            }
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
        "default-command" | "default-shell" => {
            app.default_shell = value.to_string();
        }
        "word-separators" => {
            app.word_separators = value.to_string();
        }
        "renumber-windows" => {
            app.renumber_windows = matches!(value, "on" | "true" | "1" | "yes");
        }
        "mode-keys" => {
            app.mode_keys = value.to_string();
        }
        "focus-events" => {
            app.focus_events = matches!(value, "on" | "true" | "1" | "yes");
        }
        "monitor-activity" => {
            app.monitor_activity = matches!(value, "on" | "true" | "1" | "yes");
        }
        "visual-activity" => {
            app.visual_activity = matches!(value, "on" | "true" | "1" | "yes");
        }
        "remain-on-exit" => {
            app.remain_on_exit = matches!(value, "on" | "true" | "1" | "yes");
        }
        "destroy-unattached" => {
            app.destroy_unattached = matches!(value, "on" | "true" | "1" | "yes");
        }
        "exit-empty" => {
            app.exit_empty = matches!(value, "on" | "true" | "1" | "yes");
        }
        "aggressive-resize" => {
            app.aggressive_resize = matches!(value, "on" | "true" | "1" | "yes");
        }
        "set-titles" => {
            app.set_titles = matches!(value, "on" | "true" | "1" | "yes");
        }
        "set-titles-string" => {
            app.set_titles_string = value.to_string();
        }
        "status-keys" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "pane-border-style" => { app.pane_border_style = value.to_string(); }
        "pane-active-border-style" => { app.pane_active_border_style = value.to_string(); }
        "pane-border-hover-style" => { app.pane_border_hover_style = value.to_string(); }
        "window-status-format" => { app.window_status_format = value.to_string(); }
        "window-status-current-format" => { app.window_status_current_format = value.to_string(); }
        "window-status-separator" => { app.window_status_separator = value.to_string(); }
        "automatic-rename" => {
            app.automatic_rename = matches!(value, "on" | "true" | "1" | "yes");
        }
        "synchronize-panes" => {
            app.sync_input = matches!(value, "on" | "true" | "1" | "yes");
        }
        "allow-rename" => {
            app.allow_rename = matches!(value, "on" | "true" | "1" | "yes");
        }
        "allow-set-title" => {
            app.allow_set_title = matches!(value, "on" | "true" | "1" | "yes");
        }
        "terminal-overrides" => { /* tmux terminfo override — accepted for compatibility, no-op on Windows */ }
        "default-terminal" => {
            // tmux sets the TERM env var from this option (#137)
            app.environment.insert("TERM".to_string(), value.to_string());
        }
        "update-environment" => {
            // tmux: space-separated list of env var names to update from client on attach
            app.update_environment = value.split_whitespace().map(|s| s.to_string()).collect();
        }
        "bell-action" => { app.bell_action = value.to_string(); }
        "visual-bell" => { app.visual_bell = matches!(value, "on" | "true" | "1" | "yes"); }
        "activity-action" => {
            app.activity_action = value.to_string();
        }
        "silence-action" => {
            app.silence_action = value.to_string();
        }
        "monitor-silence" => {
            if let Ok(n) = value.parse::<u64>() { app.monitor_silence = n; }
        }
        "message-style" => { app.message_style = value.to_string(); }
        "message-command-style" => { app.message_command_style = value.to_string(); }
        "mode-style" => { app.mode_style = value.to_string(); }
        "window-status-style" => { app.window_status_style = value.to_string(); }
        "window-status-current-style" => { app.window_status_current_style = value.to_string(); }
        "window-status-activity-style" => { app.window_status_activity_style = value.to_string(); }
        "window-status-bell-style" => { app.window_status_bell_style = value.to_string(); }
        "window-status-last-style" => { app.window_status_last_style = value.to_string(); }
        "status-left-style" => { app.status_left_style = value.to_string(); }
        "status-right-style" => { app.status_right_style = value.to_string(); }
        "clock-mode-colour" | "clock-mode-style" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "pane-border-format" | "pane-border-status" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "popup-style" | "popup-border-style" | "popup-border-lines" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "window-style" | "window-active-style" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "wrap-search" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "lock-after-time" | "lock-command" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "main-pane-width" => {
            if let Ok(n) = value.parse::<u16>() { app.main_pane_width = n; }
        }
        "main-pane-height" => {
            if let Ok(n) = value.parse::<u16>() { app.main_pane_height = n; }
        }
        "status-left-length" => {
            if let Ok(n) = value.parse::<usize>() { app.status_left_length = n; }
        }
        "status-right-length" => {
            if let Ok(n) = value.parse::<usize>() { app.status_right_length = n; }
        }
        "window-size" => { app.window_size = value.to_string(); }
        "allow-passthrough" => { app.allow_passthrough = value.to_string(); }
        "copy-command" => { app.copy_command = value.to_string(); }
        "set-clipboard" => { app.set_clipboard = value.to_string(); }
        "env-shim" => {
            app.env_shim = matches!(value, "on" | "true" | "1" | "yes");
        }
        "allow-predictions" => {
            app.allow_predictions = matches!(value, "on" | "true" | "1" | "yes");
        }
        "claude-code-fix-tty" => {
            app.claude_code_fix_tty = matches!(value, "on" | "true" | "1" | "yes");
        }
        "claude-code-force-interactive" => {
            app.claude_code_force_interactive = matches!(value, "on" | "true" | "1" | "yes");
        }
        "warm" => {
            app.warm_enabled = matches!(value, "on" | "true" | "1" | "yes");
            if !app.warm_enabled {
                if let Some(mut wp) = app.warm_pane.take() {
                    wp.child.kill().ok();
                }
            }
        }
        "command-alias" => {
            if let Some(pos) = value.find('=') {
                let alias = value[..pos].trim().to_string();
                let expansion = value[pos+1..].trim().to_string();
                app.command_aliases.insert(alias, expansion);
            }
        }
        _ => {
            // Handle status-format[N] patterns
            if key.starts_with("status-format[") && key.ends_with(']') {
                if let Ok(idx) = key["status-format[".len()..key.len()-1].parse::<usize>() {
                    while app.status_format.len() <= idx {
                        app.status_format.push(String::new());
                    }
                    app.status_format[idx] = value.to_string();
                    return;
                }
            }
            // Store @-prefixed user/plugin options separately from environment
            // so they don't leak into child shells (#105).
            if key.starts_with('@') {
                app.user_options.insert(key.to_string(), value.to_string());
            } else if key.contains('-') {
                // Options with hyphens are tmux config options, NOT environment
                // variables.  Storing them in environment causes PowerShell
                // ParserErrors when injected via $env:NAME syntax (#137).
                // A non-@ hyphenated key that reached this arm is not a known
                // option — surface it as a typo (#370 follow-up) but still
                // store it so forward-compat / plugin behavior is unchanged.
                // Skip array-style keys (`name[N]`) such as command-alias[0]
                // or terminal-overrides[1], which are valid tmux syntax.
                if !key.contains('[') {
                    warn_config(app, format!("unknown option '{}'", key));
                }
                app.user_options.insert(key.to_string(), value.to_string());
            } else {
                if !key.contains('[') {
                    warn_config(app, format!("unknown option '{}'", key));
                }
                app.environment.insert(key.to_string(), value.to_string());
            }

            // Auto-source plugin conf files when @plugin is declared.
            // This makes theme/settings load synchronously during config
            // parsing instead of waiting for PPM's async run-shell to
            // source them later (which causes a visible flash).
            //
            // Format: set -g @plugin 'org/plugin-name' or 'plugin-name'
            // Tries:  ~/.psmux/plugins/<full-value>/plugin.conf
            //   then: ~/.psmux/plugins/<last-component>/plugin.conf
            if key == "@plugin" && !value.is_empty() {
                // Strip an optional '#branch' suffix before deriving the plugin
                // path. Branch names may contain '/' (e.g. 'temp/integration'),
                // so splitting the raw value on '/' would treat the branch tail
                // as the plugin name and never find the installed directory.
                let base = value.split('#').next().unwrap_or(value);
                let plugin_name = base.rsplit('/').next().unwrap_or(base);
                if plugin_name != "ppm" {
                    let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                    let xdg_config = env::var("XDG_CONFIG_HOME")
                        .unwrap_or_else(|_| format!("{}\\.config", home));
                    let candidates = [
                        // Classic paths: ~/.psmux/plugins/
                        format!("{}\\.psmux\\plugins\\{}\\plugin.conf", home, base.replace('/', "\\")),
                        format!("{}\\.psmux\\plugins\\{}\\plugin.conf", home, plugin_name),
                        format!("{}\\.psmux\\plugins\\psmux-plugins\\{}\\plugin.conf", home, plugin_name),
                        // XDG paths: ~/.config/psmux/plugins/
                        format!("{}\\psmux\\plugins\\{}\\plugin.conf", xdg_config, base.replace('/', "\\")),
                        format!("{}\\psmux\\plugins\\{}\\plugin.conf", xdg_config, plugin_name),
                        format!("{}\\psmux\\plugins\\psmux-plugins\\{}\\plugin.conf", xdg_config, plugin_name),
                    ];
                    let mut found = false;
                    for conf in &candidates {
                        if std::path::Path::new(conf).exists() {
                            let prev_file = current_config_file();
                            set_current_config_file(conf);
                            if let Ok(content) = std::fs::read_to_string(conf) {
                                parse_config_content(app, &content);
                            }
                            set_current_config_file(&prev_file);
                            found = true;
                            break;
                        }
                    }
                    // If no plugin.conf, try .ps1 entry scripts
                    if !found {
                        let ps1_candidates = [
                            // Classic paths
                            format!("{}\\.psmux\\plugins\\{}\\{}.ps1", home, base.replace('/', "\\"), plugin_name),
                            format!("{}\\.psmux\\plugins\\{}\\{}.ps1", home, plugin_name, plugin_name),
                            format!("{}\\.psmux\\plugins\\psmux-plugins\\{}\\{}.ps1", home, plugin_name, plugin_name),
                            // XDG paths
                            format!("{}\\psmux\\plugins\\{}\\{}.ps1", xdg_config, base.replace('/', "\\"), plugin_name),
                            format!("{}\\psmux\\plugins\\{}\\{}.ps1", xdg_config, plugin_name, plugin_name),
                            format!("{}\\psmux\\plugins\\psmux-plugins\\{}\\{}.ps1", xdg_config, plugin_name, plugin_name),
                        ];
                        for ps1 in &ps1_candidates {
                            if std::path::Path::new(ps1).exists() {
                                // First try static extraction of set/bind commands
                                if let Ok(content) = std::fs::read_to_string(ps1) {
                                    let prev_file = current_config_file();
                                    set_current_config_file(ps1);
                                    let applied = parse_ps1_plugin_script(app, &content);
                                    set_current_config_file(&prev_file);
                                    // If the script uses PS variables (theme plugins),
                                    // static extraction yields unresolved $vars.
                                    // Queue for post-startup execution when the
                                    // server is listening.
                                    if !applied {
                                        app.pending_plugin_scripts.push(ps1.clone());
                                    }
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Split a string into tokens respecting single and double quotes.
/// `command-prompt -I '#W' 'rename-window "%%"'` → ["-I", "#W", "rename-window \"%%\""]
pub fn shell_words(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && !in_single {
            if let Some(&next) = chars.peek() {
                current.push(next);
                chars.next();
            }
        } else if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if c.is_whitespace() && !in_single && !in_double {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Split a bind-key command string on `\;` or bare `;` to produce sub-commands.
/// Handles: `split-window \; select-pane -D` → ["split-window", "select-pane -D"]
pub fn split_chained_commands_pub(command: &str) -> Vec<String> {
    split_chained_commands(command)
}
/// Split a command line into chained sub-commands.
///
/// A separator is a *whole top-level token* of exactly `;` or `\;`, matching
/// tmux's `cmd_parse_from_arguments`, which only ends a command when an
/// argument consists of (or ends with) an unescaped semicolon. Two properties
/// matter here and both were broken before (#499):
///
///  1. **Quote awareness.** A `;` inside single or double quotes is data, not a
///     separator. The one-shot CLI path flattens argv into a single line and
///     wraps any argument containing whitespace in double quotes, so a user
///     value like `"a ; b"` used to be split mid-value: the option was stored
///     truncated to `a` and the remainder (`b"`) was dispatched as its own
///     command. With a real command after the `;` that meant arbitrary command
///     execution from inside a quoted *value* (`"x ; kill-server"` killed the
///     server).
///
///  2. **Whitespace preservation.** Sub-commands are returned as slices of the
///     original line rather than re-joined `split_whitespace()` tokens, so runs
///     of spaces inside a quoted argument survive chaining (previously
///     `set-option @a 1 \; set-option @b "x    y"` stored `x y`).
///
/// A semicolon that is only *part* of a token (`a;b`, `a; b`) is left alone,
/// again matching tmux, which inspects the whole argument.
fn split_chained_commands(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut commands: Vec<String> = Vec::new();
    let mut seg_start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;

    fn push_seg(commands: &mut Vec<String>, seg: &[char]) {
        let s: String = seg.iter().collect();
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            commands.push(trimmed.to_string());
        }
    }

    while i < chars.len() {
        let c = chars[i];

        // Separator check first: only a standalone top-level `;` / `\;` token.
        if !in_single && !in_double && (c == ';' || (c == '\\' && chars.get(i + 1) == Some(&';'))) {
            let sep_len = if c == ';' { 1 } else { 2 };
            let starts_token = i == 0 || chars[i - 1].is_whitespace();
            let ends_token = chars.get(i + sep_len).is_none_or(|n| n.is_whitespace());
            if starts_token && ends_token {
                push_seg(&mut commands, &chars[seg_start..i]);
                i += sep_len;
                seg_start = i;
                continue;
            }
        }

        match c {
            // An escape pair is copied over verbatim; skipping the escaped
            // character keeps `\"` from toggling the quote state.
            '\\' if !in_single => { i += 2; continue; }
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            _ => {}
        }
        i += 1;
    }

    push_seg(&mut commands, &chars[seg_start.min(chars.len())..]);
    commands
}

pub fn parse_bind_key(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 { return; }
    
    let mut i = 1;
    let mut _key_table = "prefix".to_string();
    let mut _repeatable = false;
    
    while i < parts.len() {
        let p = parts[i];
        // A flag must start with '-' AND be longer than 1 char (e.g. "-r", "-n", "-T").
        // A bare "-" is a valid key name, not a flag.
        if p.starts_with('-') && p.len() > 1 {
            if p.contains('r') { _repeatable = true; }
            if p.contains('n') { _key_table = "root".to_string(); }
            if p.contains('T') {
                i += 1;
                if i < parts.len() { _key_table = parts[i].to_string(); }
            }
            i += 1;
        } else {
            break;
        }
    }
    
    if i >= parts.len() { return; }
    let key_str = parts[i];
    i += 1;
    
    if i >= parts.len() { return; }
    let command = parts[i..].join(" ");
    
    // Split on `\;` or `;` to support command chaining (like tmux `bind x split-window \; select-pane -D`)
    let sub_commands: Vec<String> = split_chained_commands(&command);
    
    if let Some(key) = parse_key_name(key_str) {
        let key = normalize_key_for_binding(key);
        let action = if sub_commands.len() > 1 {
            // Multiple chained commands
            Action::CommandChain(sub_commands)
        } else if let Some(a) = parse_command_to_action(&command) {
            a
        } else {
            return;
        };
        let table = app.key_tables.entry(_key_table).or_default();
        table.retain(|b| b.key != key);
        table.push(Bind { key, action, repeat: _repeatable });
    } else {
        // Silence is what made issue #616 so hard to see: a key name the parser
        // did not understand vanished with no boot warning, nothing in
        // list-keys and no clue in config-warnings.log. tmux reports
        // `unknown key: <name>` (cmd-bind-key.c), so say the same thing --
        // except for the names tmux knows and psmux simply does not model
        // (mouse events and friends), which have always been accepted quietly
        // and must not start warning in every ported config.
        if !is_unmodelled_tmux_key_name(key_str) {
            warn_config(app, format!("unknown key: {}", key_str));
        }
    }
}

pub fn parse_unbind_key(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 2 { return; }
    
    let mut i = 1;
    let mut unbind_all = false;
    let mut table: Option<String> = None;
    
    while i < parts.len() {
        let p = parts[i];
        if p.starts_with('-') {
            if p.contains('a') { unbind_all = true; }
            if p.contains('n') { table = Some("root".to_string()); }
            if p.contains('T') && i + 1 < parts.len() {
                i += 1;
                table = Some(parts[i].to_string());
            }
            i += 1;
        } else {
            break;
        }
    }
    
    if unbind_all {
        if let Some(t) = table {
            // -a -T <table>: only clear that table
            if let Some(binds) = app.key_tables.get_mut(&t) {
                binds.clear();
            }
        } else {
            // -a (no table): clear ALL tables + suppress defaults
            app.key_tables.clear();
            app.defaults_suppressed = true;
        }
        return;
    }
    
    if i < parts.len() {
        if let Some(key) = parse_key_name(parts[i]) {
            let key = normalize_key_for_binding(key);
            // Remove from the targeted table only (tmux behavior).
            // Default is "prefix" when no -n or -T is specified.
            let target = table.unwrap_or_else(|| "prefix".to_string());
            if let Some(binds) = app.key_tables.get_mut(&target) {
                binds.retain(|b| b.key != key);
            }
        } else {
            // Same reasoning as parse_bind_key: tmux's cmd-unbind-key.c reports
            // `unknown key: <name>` rather than quietly doing nothing, and the
            // names psmux does not model are excused the same way.
            if !is_unmodelled_tmux_key_name(parts[i]) {
                warn_config(app, format!("unknown key: {}", parts[i]));
            }
        }
    }
}

/// Ensure the current prefix key is bound to `send-prefix` in the prefix table.
/// This makes pressing the prefix key twice forward a literal prefix keystroke
/// to the active pane (matches tmux's `bind C-b send-prefix` default, but also
/// follows the prefix when the user does `set -g prefix C-a`).
///
/// Existing bindings for the prefix key are preserved — the user's override
/// always wins (e.g. `bind C-a some-other-command` after `set prefix C-a`).
pub fn ensure_prefix_self_binding(app: &mut AppState) {
    let key = normalize_key_for_binding(app.prefix_key);
    let table = app.key_tables.entry("prefix".to_string()).or_default();
    if table.iter().any(|b| b.key == key) {
        return;
    }
    if let Some(action) = parse_command_to_action("send-prefix") {
        table.push(Bind { key, action, repeat: false });
    }
}

/// Fold the NUL key (ASCII 0x00) onto `C-Space`.
///
/// A terminal encodes Ctrl+Space as the NUL byte 0x00.  On Windows the console
/// delivers that byte as a KEY_EVENT with `wVirtualKeyCode = VK_2` and
/// `UnicodeChar = 0`, because NUL is classically typed as Ctrl+@ (Shift+2 on a
/// US layout).  crossterm resolves that zero UnicodeChar back through the
/// keyboard layout with an empty key state, so it surfaces as `Char('2')` with
/// CONTROL.  Physical Ctrl+2, physical Ctrl+Shift+2, and a NUL byte written by
/// a ConPTY-hosted terminal are byte-for-byte identical in that record, so they
/// cannot be told apart, and in terminal terms they are all genuinely the same
/// key: NUL.
///
/// tmux folds the same way in `tty-keys.c`:
///
/// ```text
/// /* C-Space is special. */
/// if ((key & KEYC_MASK_KEY) == C0_NUL)
///         key = ' ' | KEYC_CTRL | (key & KEYC_META);
/// ```
///
/// Without this fold `set -g prefix C-Space` is dead on Windows for any
/// terminal whose only way to emit Ctrl+Space is to send a literal NUL, which
/// is exactly Alacritty's documented `chars = <NUL escape>` workaround (issue
/// #504).  The fold is applied symmetrically to incoming key events and to
/// registered binding keys, so an existing `bind-key C-2` keeps firing.
pub fn fold_nul_to_ctrl_space(key: (KeyCode, KeyModifiers)) -> (KeyCode, KeyModifiers) {
    let is_nul = match key.0 {
        // Windows console encoding of NUL (Ctrl+@ / Ctrl+Shift+2 / Ctrl+Space).
        KeyCode::Char('2') => key.1.contains(KeyModifiers::CONTROL)
            && !key.1.contains(KeyModifiers::ALT),
        // A literal NUL that reached the key layer as a character.
        KeyCode::Char('\0') => true,
        _ => false,
    };
    if is_nul {
        // SHIFT is noise here: Ctrl+Shift+2 and Ctrl+2 are the same NUL.
        (KeyCode::Char(' '), key.1.difference(KeyModifiers::SHIFT) | KeyModifiers::CONTROL)
    } else {
        key
    }
}

/// Normalize a key tuple for binding comparison.
/// Strips SHIFT from Char events since the character itself encodes shift information.
/// e.g., '|' already implies Shift was pressed, so (Char('|'), SHIFT) and (Char('|'), NONE) should match.
///
/// On Windows, also strips Ctrl+Alt from non-lowercase-letter Char events.
/// AltGr on Windows is reported as Ctrl+Alt, so characters produced via AltGr
/// (e.g. `[` `]` `{` `}` `@` `\` `|` `~` on German/Czech keyboards) arrive
/// as Char('[') with CONTROL|ALT modifiers.  Stripping those fake modifiers
/// lets the binding lookup match the registered `[` binding (issue #287).
///
/// NUL (`C-2` / `C-Space`) is folded first so that a binding registered as
/// `C-2` and one registered as `C-Space` resolve to the same tuple (#504).
pub fn normalize_key_for_binding(key: (KeyCode, KeyModifiers)) -> (KeyCode, KeyModifiers) {
    let key = fold_nul_to_ctrl_space(key);
    match key.0 {
        KeyCode::Char(c) => {
            let mut mods = key.1.difference(KeyModifiers::SHIFT);
            // On Windows, AltGr is reported as Ctrl+Alt.  Non-lowercase-letter
            // chars with both Ctrl and Alt are AltGr-produced — strip the fake
            // Ctrl+Alt so they match plain bindings like `[`, `]`, `@`, etc.
            #[cfg(windows)]
            if mods.contains(KeyModifiers::CONTROL)
                && mods.contains(KeyModifiers::ALT)
                && !c.is_ascii_lowercase()
            {
                mods = mods.difference(KeyModifiers::CONTROL);
                mods = mods.difference(KeyModifiers::ALT);
            }
            (key.0, mods)
        }
        _ => key,
    }
}

/// Map a multi-character key name (case-insensitive) to a KeyCode.
/// Returns None if the name is not recognized.
fn named_key(name: &str) -> Option<KeyCode> {
    match name.to_lowercase().as_str() {
        "space" => Some(KeyCode::Char(' ')),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "btab" | "backtab" => Some(KeyCode::BackTab),
        "escape" | "esc" => Some(KeyCode::Esc),
        "bspace" | "backspace" => Some(KeyCode::Backspace),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" | "ppage" | "pgup" => Some(KeyCode::PageUp),
        "pagedown" | "npage" | "pgdn" => Some(KeyCode::PageDown),
        "insert" | "ic" => Some(KeyCode::Insert),
        "delete" | "dc" => Some(KeyCode::Delete),
        "f1" => Some(KeyCode::F(1)),
        "f2" => Some(KeyCode::F(2)),
        "f3" => Some(KeyCode::F(3)),
        "f4" => Some(KeyCode::F(4)),
        "f5" => Some(KeyCode::F(5)),
        "f6" => Some(KeyCode::F(6)),
        "f7" => Some(KeyCode::F(7)),
        "f8" => Some(KeyCode::F(8)),
        "f9" => Some(KeyCode::F(9)),
        "f10" => Some(KeyCode::F(10)),
        "f11" => Some(KeyCode::F(11)),
        "f12" => Some(KeyCode::F(12)),
        _ => None,
    }
}

pub fn parse_key_name(name: &str) -> Option<(KeyCode, KeyModifiers)> {
    let name = name.trim();
    // Strip surrounding quotes (single or double) — plugins often quote special chars
    // e.g., bind-key '|' split-window -h
    let name = if (name.starts_with('\'') && name.ends_with('\'') && name.len() >= 2)
        || (name.starts_with('"') && name.ends_with('"') && name.len() >= 2) {
        &name[1..name.len()-1]
    } else {
        name
    };

    // ── Extract all modifier prefixes (C-, M-, S-) then resolve the base key ──
    // This supports arbitrary combinations: C-Tab, C-S-Tab, C-M-S-Up, etc.
    let mut rest = name;
    let mut mods = KeyModifiers::NONE;
    loop {
        if rest.starts_with("C-") { mods |= KeyModifiers::CONTROL; rest = &rest[2..]; }
        else if rest.starts_with("M-") { mods |= KeyModifiers::ALT; rest = &rest[2..]; }
        else if rest.starts_with("S-") { mods |= KeyModifiers::SHIFT; rest = &rest[2..]; }
        else if rest.starts_with("^") && rest.len() > 1 { mods |= KeyModifiers::CONTROL; rest = &rest[1..]; }
        else { break; }
    }

    if mods != KeyModifiers::NONE {
        // S-Tab (with or without other modifiers) → BackTab + remaining mods
        if rest.eq_ignore_ascii_case("Tab") && mods.contains(KeyModifiers::SHIFT) {
            return Some((KeyCode::BackTab, mods.difference(KeyModifiers::SHIFT)));
        }
        if let Some(kc) = named_key(rest) {
            return Some((kc, mods));
        }
        // A key is one CHARACTER, not one BYTE. `M-<U+0444>` leaves two bytes
        // of UTF-8 in `rest`, so the old byte-length gate failed and every
        // non-ASCII modifier binding was dropped without a word (issue #616).
        // tmux takes the same shape in key-string.c: the ASCII fast path is
        // `string[1] == '\0' && string[0] <= 127`, and anything else is decoded
        // as UTF-8 and OR'd with the modifiers.
        if rest.chars().count() == 1 {
            if let Some(c) = rest.chars().next() {
                if mods.contains(KeyModifiers::SHIFT) {
                    return Some((KeyCode::Char(map_key_case(c, true)), mods.difference(KeyModifiers::SHIFT)));
                }
                return Some((KeyCode::Char(map_key_case(c, false)), mods));
            }
        }
        // Unrecognized key after modifiers — fall through
    }
    
    match name.to_uppercase().as_str() {
        "ENTER" => return Some((KeyCode::Enter, KeyModifiers::NONE)),
        "TAB" => return Some((KeyCode::Tab, KeyModifiers::NONE)),
        "BTAB" => return Some((KeyCode::BackTab, KeyModifiers::NONE)),
        "ESCAPE" | "ESC" => return Some((KeyCode::Esc, KeyModifiers::NONE)),
        "SPACE" => return Some((KeyCode::Char(' '), KeyModifiers::NONE)),
        "BSPACE" | "BACKSPACE" => return Some((KeyCode::Backspace, KeyModifiers::NONE)),
        "UP" => return Some((KeyCode::Up, KeyModifiers::NONE)),
        "DOWN" => return Some((KeyCode::Down, KeyModifiers::NONE)),
        "LEFT" => return Some((KeyCode::Left, KeyModifiers::NONE)),
        "RIGHT" => return Some((KeyCode::Right, KeyModifiers::NONE)),
        "HOME" => return Some((KeyCode::Home, KeyModifiers::NONE)),
        "END" => return Some((KeyCode::End, KeyModifiers::NONE)),
        "PAGEUP" | "PPAGE" | "PGUP" => return Some((KeyCode::PageUp, KeyModifiers::NONE)),
        "PAGEDOWN" | "NPAGE" | "PGDN" => return Some((KeyCode::PageDown, KeyModifiers::NONE)),
        "INSERT" | "IC" => return Some((KeyCode::Insert, KeyModifiers::NONE)),
        "DELETE" | "DC" => return Some((KeyCode::Delete, KeyModifiers::NONE)),
        "F1" => return Some((KeyCode::F(1), KeyModifiers::NONE)),
        "F2" => return Some((KeyCode::F(2), KeyModifiers::NONE)),
        "F3" => return Some((KeyCode::F(3), KeyModifiers::NONE)),
        "F4" => return Some((KeyCode::F(4), KeyModifiers::NONE)),
        "F5" => return Some((KeyCode::F(5), KeyModifiers::NONE)),
        "F6" => return Some((KeyCode::F(6), KeyModifiers::NONE)),
        "F7" => return Some((KeyCode::F(7), KeyModifiers::NONE)),
        "F8" => return Some((KeyCode::F(8), KeyModifiers::NONE)),
        "F9" => return Some((KeyCode::F(9), KeyModifiers::NONE)),
        "F10" => return Some((KeyCode::F(10), KeyModifiers::NONE)),
        "F11" => return Some((KeyCode::F(11), KeyModifiers::NONE)),
        "F12" => return Some((KeyCode::F(12), KeyModifiers::NONE)),
        _ => {}
    }
    
    // One character, not one byte (issue #616): `bind <U+044B>` is a 2 byte
    // key name and used to fall straight through to None.
    if name.chars().count() == 1 {
        if let Some(c) = name.chars().next() {
            return Some((KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    None
}

/// True when `name` is a key name tmux's `key_string_lookup_string` accepts but
/// psmux has no KeyCode for: the mouse event family, the terminal-report
/// pseudo keys and `UserN`.
///
/// These are NOT parse errors. `bind -n WheelUpPane ...` is a normal line in a
/// ported tmux config (psmux's own FAQ quotes tmux's default binding), and
/// psmux has always accepted the command and simply not acted on it. The
/// unknown-key diagnostic added for issue #616 must catch typos, not start
/// failing every config that carries a mouse binding, so those names are
/// excused here and stay silent exactly as before.
///
/// tmux builds the mouse names from KEYC_MOUSE_STRING in tmux.h: an event name
/// (MouseDown1, MouseDragEnd3, WheelUp, DoubleClick1 ...) followed by a
/// location suffix (Pane, Status, StatusLeft, Border, ScrollbarSlider ...).
pub fn is_unmodelled_tmux_key_name(name: &str) -> bool {
    // Strip the same modifier prefixes tmux strips before its table lookup.
    let mut rest = name;
    loop {
        let lower_two: String = rest.chars().take(2).collect::<String>().to_lowercase();
        if matches!(lower_two.as_str(), "c-" | "m-" | "s-") {
            rest = &rest[2..];
        } else {
            break;
        }
    }

    const SPECIAL: &[&str] = &[
        "any", "none", "mouse", "dragging", "focusin", "focusout",
        "pastestart", "pasteend", "reportdarktheme", "reportlighttheme",
        "mousemovepane", "mousemovestatus", "mousemovestatusleft",
        "mousemovestatusright", "mousemoveborder",
    ];
    let lower = rest.to_lowercase();
    if SPECIAL.contains(&lower.as_str()) {
        return true;
    }
    // UserN
    if let Some(n) = lower.strip_prefix("user") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            return true;
        }
    }

    const LOCATIONS: &[&str] = &[
        "pane", "status", "statusleft", "statusright", "statusdefault",
        "scrollbarup", "scrollbarslider", "scrollbardown", "empty", "border",
    ];
    let Some(head) = LOCATIONS
        .iter()
        .filter_map(|loc| lower.strip_suffix(loc))
        .max_by_key(|h| h.len())
    else {
        return false;
    };
    // The event name: a prefix plus an optional button number.
    const EVENTS: &[&str] = &[
        "mousedown", "mouseup", "mousedrag", "mousedragend",
        "secondclick", "doubleclick", "tripleclick",
    ];
    if head == "wheelup" || head == "wheeldown" {
        return true;
    }
    EVENTS.iter().any(|e| {
        head.strip_prefix(e)
            .map(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(false)
    })
}

/// Unicode aware single character case mapping for the modifier forms of a key
/// name: `S-x` names the SHIFTED character and `C-A` normalises to `C-a`, which
/// is what psmux has always done for ASCII via `to_ascii_uppercase` /
/// `to_ascii_lowercase`. Those are no-ops outside ASCII, so a Cyrillic `S-` form
/// would have silently collapsed onto the unshifted key once #616 let it parse
/// at all, and `normalize_key_for_binding` drops SHIFT from every Char, so the
/// two bindings would have overwritten each other.
///
/// Falls back to the original character when the mapping is not one to one
/// (U+00DF uppercases to "SS", U+0130 lowercases to two scalars) so a key name
/// stays exactly one character in every case.
fn map_key_case(c: char, upper: bool) -> char {
    let mut mapped: Vec<char> = if upper {
        c.to_uppercase().collect()
    } else {
        c.to_lowercase().collect()
    };
    if mapped.len() == 1 { mapped.remove(0) } else { c }
}

thread_local! {
    // Guards against runaway recursion when a config sources itself (directly or
    // in a cycle). Without this a self-sourcing psmux.conf overflows the stack and
    // crashes the server, so the session never comes up ("no server running").
    static SOURCE_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

const MAX_SOURCE_DEPTH: u32 = 16;

struct SourceDepthGuard;
impl Drop for SourceDepthGuard {
    fn drop(&mut self) {
        SOURCE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

pub fn source_file(app: &mut AppState, path: &str) {
    let depth = SOURCE_DEPTH.with(|d| d.get());
    if depth >= MAX_SOURCE_DEPTH {
        eprintln!("psmux: source-file: maximum nesting depth ({}) exceeded; ignoring '{}' (recursive source-file?)", MAX_SOURCE_DEPTH, path);
        return;
    }
    SOURCE_DEPTH.with(|d| d.set(depth + 1));
    let _depth_guard = SourceDepthGuard; // decrements on every return path

    let path = path.trim().trim_matches('"').trim_matches('\'');

    // Handle -F flag: expand format strings in the path
    let (path, format_expand) = if path.starts_with("-F ") || path.starts_with("-F\t") {
        (path[3..].trim().trim_matches('"').trim_matches('\''), true)
    } else {
        (path, false)
    };

    let expanded_path = if format_expand {
        crate::format::expand_format(path, app)
    } else {
        path.to_string()
    };

    let expanded_path = if expanded_path.starts_with('~') {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        expanded_path.replacen('~', &home, 1)
    } else {
        expanded_path
    };

    // Normalize path separators for Windows
    let expanded_path = expanded_path.replace('/', &std::path::MAIN_SEPARATOR.to_string());

    // Fallback: if path references ~/.psmux/ but doesn't exist and the
    // XDG equivalent (~/.config/psmux/) does, use that instead (issue #135).
    let expanded_path = if !std::path::Path::new(&expanded_path).exists() {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        let classic = format!("{}\\.psmux\\", home);
        if expanded_path.starts_with(&classic) {
            let xdg_base = env::var("XDG_CONFIG_HOME")
                .unwrap_or_else(|_| format!("{}\\.config", home));
            let xdg_alt = expanded_path.replacen(&classic, &format!("{}\\psmux\\", xdg_base), 1);
            if std::path::Path::new(&xdg_alt).exists() { xdg_alt } else { expanded_path }
        } else {
            expanded_path
        }
    } else {
        expanded_path
    };

    // A runtime `source-file` (not part of a startup batch and not nested) owns
    // its own warning batch and surfaces the result as a status message.
    let outermost_runtime = depth == 0 && !in_startup_load();
    if outermost_runtime {
        app.config_warnings.clear();
    }

    // Save and restore current_config_file around the nested parse
    let prev_file = current_config_file();
    set_current_config_file(&expanded_path);

    match std::fs::read_to_string(&expanded_path) {
        Ok(content) => parse_config_content(app, &content),
        Err(e) => {
            // A `source-file` naming a path that cannot be read used to be a
            // silent no-op: no warning, no log line, nothing. Since the whole
            // keybinding layer of a split config lives behind one such line, a
            // typo or a moved repo produced a psmux with stock defaults and no
            // indication why. Record it like any other config warning so it
            // reaches ~/.psmux/config-warnings.log and the attach-time stderr
            // summary.
            //
            // tmux parity note: tmux errors on a missing source-file too, but
            // only when the path is not a glob. psmux has no glob support here,
            // so every miss is a real miss.
            //
            // Route through warn_config for consistent formatting, but point it
            // at the source-file directive itself — the parent file and line,
            // where the user's fix belongs — not the child that couldn't be
            // read. current_config_file was switched to the child above, and
            // config_warn_line still holds the parent's line (source_file runs
            // inside parse_config_content's per-line loop), so restore the parent
            // file first; then name the missing child in the message.
            set_current_config_file(&prev_file);
            warn_config(app, format!("cannot source {}: {}", expanded_path, e));
        }
    }

    set_current_config_file(&prev_file);

    if outermost_runtime && !app.config_warnings.is_empty() {
        let n = app.config_warnings.len();
        let summary = if n == 1 {
            format!("config warning: {}", app.config_warnings[0])
        } else {
            format!("{} config warnings (first: {})", n, app.config_warnings[0])
        };
        // Hold the message for 5s (vs the 750ms default) so a config problem is
        // actually noticed rather than flashing past.
        app.status_message = Some((summary, std::time::Instant::now(), Some(5000)));
    }
}

/// Parse a key string like "C-a", "M-x", "F1", "Space" into (KeyCode, KeyModifiers)
pub fn parse_key_string(key: &str) -> Option<(KeyCode, KeyModifiers)> {
    let key = key.trim();
    let mut mods = KeyModifiers::empty();
    let mut key_part = key;
    
    // Characters, not bytes (issue #616). `M-<U+0444>` is 3 characters but 4
    // bytes; more importantly the single-character arm below used byte length,
    // so the CLI / server route dropped every non-ASCII key exactly the way the
    // config route did.
    while key_part.chars().count() > 2 {
        if key_part.starts_with("C-") || key_part.starts_with("c-") {
            mods |= KeyModifiers::CONTROL;
            key_part = &key_part[2..];
        } else if key_part.starts_with("M-") || key_part.starts_with("m-") {
            mods |= KeyModifiers::ALT;
            key_part = &key_part[2..];
        } else if key_part.starts_with("S-") || key_part.starts_with("s-") {
            mods |= KeyModifiers::SHIFT;
            key_part = &key_part[2..];
        } else {
            break;
        }
    }
    
    let keycode = match key_part.to_lowercase().as_str() {
        // Single character keys: preserve the ORIGINAL case from key_part, not the lowercased version.
        // This is critical for case-sensitive bind-key (issue #157): bind-key T != bind-key t.
        _ if key_part.chars().count() == 1 => {
            KeyCode::Char(key_part.chars().next().unwrap())
        }
        "space" => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "btab" | "backtab" => KeyCode::BackTab,
        "escape" | "esc" => KeyCode::Esc,
        "backspace" | "bspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "ppage" => KeyCode::PageUp,
        "pagedown" | "npage" => KeyCode::PageDown,
        "insert" | "ic" => KeyCode::Insert,
        "delete" | "dc" => KeyCode::Delete,
        "f1" => KeyCode::F(1),
        "f2" => KeyCode::F(2),
        "f3" => KeyCode::F(3),
        "f4" => KeyCode::F(4),
        "f5" => KeyCode::F(5),
        "f6" => KeyCode::F(6),
        "f7" => KeyCode::F(7),
        "f8" => KeyCode::F(8),
        "f9" => KeyCode::F(9),
        "f10" => KeyCode::F(10),
        "f11" => KeyCode::F(11),
        "f12" => KeyCode::F(12),
        "\"" => KeyCode::Char('"'),
        "%" => KeyCode::Char('%'),
        "," => KeyCode::Char(','),
        "." => KeyCode::Char('.'),
        ":" => KeyCode::Char(':'),
        ";" => KeyCode::Char(';'),
        "[" => KeyCode::Char('['),
        "]" => KeyCode::Char(']'),
        "{" => KeyCode::Char('{'),
        "}" => KeyCode::Char('}'),
        _ => {
            return None;
        }
    };
    
    Some((keycode, mods))
}

/// Format a key binding back to string representation
pub fn format_key_binding(key: &(KeyCode, KeyModifiers)) -> String {
    let (keycode, mods) = key;
    let mut result = String::new();
    
    if mods.contains(KeyModifiers::CONTROL) {
        result.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        result.push_str("M-");
    }
    if mods.contains(KeyModifiers::SHIFT) {
        result.push_str("S-");
    }
    
    let key_str = match keycode {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "BTab".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PPage".to_string(),
        KeyCode::PageDown => "NPage".to_string(),
        KeyCode::Insert => "IC".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        _ => "?".to_string(),
    };
    
    result.push_str(&key_str);
    result
}

/// Execute a run-shell / run command from config or hooks.
/// Syntax: run-shell [-b] <command>
/// Always spawns non-blocking to avoid deadlocks when hooks fire on the
/// server thread (scripts may call back to psmux via CLI).
fn parse_run_shell(app: &mut AppState, line: &str) {
    // Use quote-aware parser to properly handle nested quotes and escapes
    let args = crate::commands::parse_command_line(line);
    if args.len() < 2 { return; }
    let mut cmd_parts: Vec<&str> = Vec::new();
    for arg in &args[1..] {
        if arg == "-b" { /* background flag — always spawn anyway */ }
        else { cmd_parts.push(arg); }
    }
    let shell_cmd = cmd_parts.join(" ");
    if shell_cmd.is_empty() { return; }

    // Expand ~ to home directory + XDG fallback for plugin paths
    let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd);

    // ── Handle .tmux files natively ──────────────────────────────────
    // .tmux files are bash scripts used by tmux plugins. On Windows they
    // can't be executed by pwsh. Parse them for `tmux source`, `tmux set`,
    // etc. and apply the extracted commands as config lines.
    let trimmed_cmd = shell_cmd.trim().trim_matches('\'').trim_matches('"');
    if trimmed_cmd.ends_with(".tmux") {
        let tmux_path = std::path::Path::new(trimmed_cmd);
        if tmux_path.is_file() {
            parse_tmux_entry_script(app, tmux_path);
            return;
        }
    }
    // Also handle .ps1 files natively when possible: if the command is a
    // bare .ps1 path (no arguments), we can run it directly with pwsh -File
    // which is more reliable than -Command for script paths with spaces.

    // Always spawn non-blocking: run-shell commands from hooks may call back
    // to the psmux server (e.g., `psmux set -g @option value`), which would
    // deadlock if we blocked the server thread with .output().
    // Set PSMUX_TARGET_SESSION so child scripts connect to the correct server
    // (especially important when using -L socket namespaces like in tppanel preview).
    let target_session = app.port_file_base();
    let mut cmd = crate::commands::build_run_shell_command(&shell_cmd);
    if !target_session.is_empty() {
        cmd.env("PSMUX_TARGET_SESSION", &target_session);
    }
    let _ = cmd.spawn();
}

/// Parse a `.tmux` entry script (bash) and extract tmux commands from it.
///
/// .tmux files are the standard entry point for tmux plugins. They are bash
/// scripts that typically call `tmux source <file>`, `tmux set -g ...`, etc.
/// On Windows we can't run bash, so we parse the script and translate the
/// tmux CLI calls into psmux config lines.
///
/// Supported patterns:
/// Parse a .ps1 plugin entry script and extract psmux set/bind commands.
///
/// Plugin .ps1 scripts use patterns like:
///   & $PSMUX set -g key value 2>&1 | Out-Null
///   & $PSMUX bind-key ...
///
/// We extract the psmux command portion and apply it as config.
/// Returns true if all extracted values are literal (no unresolved PS variables).
/// Returns false if the script uses PowerShell variables that need runtime eval.
fn parse_ps1_plugin_script(app: &mut AppState, content: &str) -> bool {
    let mut has_ps_vars = false;
    let mut applied_any = false;

    for line in content.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') { continue; }

        // Match patterns like: & $PSMUX set -g ... 2>&1 | Out-Null
        // Also: & $PSMUX bind-key ... 2>&1 | Out-Null
        let cmd_start = if let Some(pos) = l.find("$PSMUX ") {
            // Ensure it's preceded by "& " (PowerShell call operator)
            let prefix = &l[..pos];
            if prefix.trim_end().ends_with('&') {
                Some(pos + 7) // skip "$PSMUX "
            } else {
                None
            }
        } else {
            None
        };

        let cmd = match cmd_start {
            Some(start) => &l[start..],
            None => continue,
        };

        // Strip trailing PowerShell noise: 2>&1 | Out-Null, 2>$null
        let cmd = cmd.split(" 2>&1").next().unwrap_or(cmd);
        let cmd = cmd.split(" 2>$null").next().unwrap_or(cmd);
        let cmd = cmd.trim();

        // Check for unresolved PowerShell variables (e.g., $bg1, $fg)
        // but not $PSMUX or $TMUX which are expected patterns
        if cmd.contains('$') {
            // Check if it's a PS variable reference (not env var pattern)
            let has_var = cmd.split('$').skip(1).any(|part| {
                let first_word: String = part.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                !first_word.is_empty() && first_word != "PSMUX" && first_word != "TMUX"
                    && first_word != "env" && first_word != "null"
            });
            if has_var { has_ps_vars = true; }
        }

        if cmd.starts_with("set ") || cmd.starts_with("set-option ")
            || cmd.starts_with("bind-key ") || cmd.starts_with("bind ")
            || cmd.starts_with("setw ") || cmd.starts_with("set-window-option ") {
            if !has_ps_vars {
                parse_config_line(app, cmd);
                applied_any = true;
            }
        }
    }

    // Return true if we applied commands and they were all literal
    applied_any && !has_ps_vars
}

///   tmux source[-file] "path"       → source-file "path"
///   tmux set[-option] [-g] key val  → set [-g] key val
///   tmux setw key val               → setw key val
///   PLUGIN_DIR=...                  → track for variable expansion
fn parse_tmux_entry_script(app: &mut AppState, path: &std::path::Path) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    // Determine the directory of the .tmux file for $PLUGIN_DIR / ${PLUGIN_DIR}
    let plugin_dir = path.parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Also look for PLUGIN_DIR assignment in the script (may differ)
    let mut script_plugin_dir = plugin_dir.clone();
    // Common pattern:  PLUGIN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    // We can't evaluate bash, so we just use the file's parent directory.

    for line in content.lines() {
        let l = line.trim();
        // Skip empty lines, comments, shebang
        if l.is_empty() || l.starts_with('#') { continue; }

        // Track explicit PLUGIN_DIR assignment (best-effort)
        if l.starts_with("PLUGIN_DIR=") || l.starts_with("export PLUGIN_DIR=") {
            // If it's a simple literal path, use it
            let val = l.splitn(2, '=').nth(1).unwrap_or("").trim_matches('"').trim_matches('\'');
            if !val.contains('$') && !val.contains('`') && !val.is_empty() {
                script_plugin_dir = val.to_string();
            }
            // Otherwise keep using the .tmux file's parent dir
            continue;
        }

        // Skip other bash-isms (variable assignments, if/fi, for, etc.)
        if l.contains("BASH_SOURCE") || l.starts_with("cd ") || l.starts_with("export ")
            || l.starts_with("if ") || l == "fi" || l.starts_with("for ")
            || l.starts_with("done") || l.starts_with("then") || l.starts_with("else")
            || l.starts_with("local ") || l.starts_with("readonly ") {
            continue;
        }

        // Extract tmux commands: look for lines starting with `tmux `
        let tmux_cmd = if l.starts_with("tmux ") {
            &l[5..]
        } else if l.starts_with("\"$TMUX_PROGRAM\" ") || l.starts_with("$TMUX_PROGRAM ") {
            // Some plugins use $TMUX_PROGRAM variable
            let start = l.find(' ').unwrap_or(l.len());
            l[start..].trim()
        } else {
            continue;
        };

        // Expand $PLUGIN_DIR, ${PLUGIN_DIR}, $CURRENT_DIR, ${CURRENT_DIR}
        let expanded = tmux_cmd
            .replace("${PLUGIN_DIR}", &script_plugin_dir)
            .replace("$PLUGIN_DIR", &script_plugin_dir)
            .replace("${CURRENT_DIR}", &script_plugin_dir)
            .replace("$CURRENT_DIR", &script_plugin_dir);

        // Now parse the tmux subcommand as a psmux config line
        let expanded = expanded.trim();
        if expanded.starts_with("source-file ") || expanded.starts_with("source ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("set-option ") || expanded.starts_with("set ")
            || expanded.starts_with("set -g ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("setw ") || expanded.starts_with("set-window-option ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("run-shell ") || expanded.starts_with("run ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("bind-key ") || expanded.starts_with("bind ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("if-shell ") || expanded.starts_with("if ") {
            parse_config_line(app, expanded);
        } else if expanded.starts_with("set-hook ") {
            parse_config_line(app, expanded);
        } else {
            // Try to parse it anyway — it might be a valid config directive
            parse_config_line(app, expanded);
        }
    }

    // Fallback: if we didn't find any tmux commands in the script, try to
    // source .conf files from the same directory (many themes ship both
    // .tmux entry script and .conf files).
    // Check if we actually parsed anything by looking at common indicators
    // (status-left, status-right being changed from defaults).
    // For now, also auto-source any *_tmux.conf or *.conf files in the dir.
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if ext == "conf" {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        // Source companion .conf files (but not the .tmux script itself)
                        // Prioritize files like plugin_name_options.conf, plugin_name.conf
                        if fname.ends_with("_tmux.conf") || fname.ends_with("_options_tmux.conf") {
                            source_file(app, &p.to_string_lossy());
                        }
                    }
                }
            }
        }
    }
}

/// Execute an if-shell / if command from config.
/// Syntax: if-shell [-bF] <condition> <true-cmd> [<false-cmd>]
/// Runs the condition command (or evaluates format with -F), then executes the
/// appropriate branch command as a config line.
fn parse_if_shell(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 { return; }

    let mut format_mode = false;
    let mut _background = false;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1;
    while i < parts.len() {
        match parts[i] {
            "-b" => { _background = true; }
            "-F" => { format_mode = true; }
            "-bF" | "-Fb" => { _background = true; format_mode = true; }
            "-t" => { i += 1; } // skip target
            s => {
                // Handle quoted strings that might span multiple parts
                if s.starts_with('"') || s.starts_with('\'') {
                    let quote = s.chars().next().unwrap();
                    if s.ends_with(quote) && s.len() > 1 {
                        positional.push(s[1..s.len()-1].to_string());
                    } else {
                        let mut buf = s[1..].to_string();
                        i += 1;
                        while i < parts.len() {
                            buf.push(' ');
                            buf.push_str(parts[i]);
                            if parts[i].ends_with(quote) {
                                buf.truncate(buf.len() - 1);
                                break;
                            }
                            i += 1;
                        }
                        positional.push(buf);
                    }
                } else {
                    positional.push(s.to_string());
                }
            }
        }
        i += 1;
    }

    if positional.len() < 2 { return; }
    let condition = &positional[0];
    let true_cmd = &positional[1];
    let false_cmd = positional.get(2);

    let success = if format_mode {
        let expanded = crate::format::expand_format(condition, app);
        !expanded.is_empty() && expanded != "0"
    } else if condition == "true" || condition == "1" {
        true
    } else if condition == "false" || condition == "0" {
        false
    } else {
        let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
        let mut c = std::process::Command::new(&shell_prog);
        for a in &shell_args { c.arg(a); }
        c.arg(condition);
        { use crate::platform::HideWindowCommandExt; c.hide_window(); }
        c.status().map(|s| s.success()).unwrap_or(false)
    };

    let cmd_to_run = if success { Some(true_cmd) } else { false_cmd };
    if let Some(cmd) = cmd_to_run {
        // Execute the branch as a config line (recursive — supports set, bind, source, etc.)
        parse_config_line(app, cmd);
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_config_plugin_paths.rs"]
mod tests_plugin_paths;

#[cfg(test)]
#[path = "../tests-rs/test_issue137_env_leak.rs"]
mod tests_issue137_env_leak;

#[cfg(test)]
#[path = "../tests-rs/test_issue157_bind_key_case.rs"]
mod tests_issue157_bind_key_case;

#[cfg(test)]
#[path = "../tests-rs/test_issue145_source_file.rs"]
mod tests_issue145_source_file;

#[cfg(test)]
#[path = "../tests-rs/test_issue193_scroll_enter_copy_mode.rs"]
mod tests_issue193_scroll_enter_copy_mode;

#[cfg(test)]
#[path = "../tests-rs/test_issue198_unbind_individual.rs"]
mod tests_issue198_unbind_individual;

#[cfg(test)]
#[path = "../tests-rs/test_issue198_cv_persist.rs"]
mod tests_issue198_cv_persist;

#[cfg(test)]
#[path = "../tests-rs/test_issue198_pastedetect_frame_parity.rs"]
mod tests_issue198_pastedetect_frame_parity;

#[cfg(test)]
#[path = "../tests-rs/test_config_exhaustive.rs"]
mod tests_config_exhaustive;

#[cfg(test)]
#[path = "../tests-rs/test_issue268_set_titles.rs"]
mod tests_issue268_set_titles;

#[cfg(test)]
#[path = "../tests-rs/test_issue287_german_keyboard.rs"]
mod tests_issue287_german_keyboard;

#[cfg(test)]
#[path = "../tests-rs/test_issue362_config_new_session.rs"]
mod tests_issue362_config_new_session;

#[cfg(test)]
#[path = "../tests-rs/test_issue370_config_warnings.rs"]
mod tests_issue370_config_warnings;

#[cfg(test)]
#[path = "../tests-rs/test_issue416_inline_comments.rs"]
mod tests_issue416_inline_comments;

#[cfg(test)]
#[path = "../tests-rs/test_issue425_bold_is_bright_option.rs"]
mod tests_issue425_bold_is_bright_option;

#[cfg(test)]
#[path = "../tests-rs/test_issue459_hook_accumulation.rs"]
mod tests_issue459_hook_accumulation;

#[cfg(test)]
#[path = "../tests-rs/test_issue488_pageup_defaults.rs"]
mod tests_issue488_pageup_defaults;

#[cfg(test)]
#[path = "../tests-rs/test_issue499_quoted_semicolon.rs"]
mod tests_issue499_quoted_semicolon;

#[cfg(test)]
#[path = "../tests-rs/test_issue504_ctrl_space_nul.rs"]
mod tests_issue504_ctrl_space_nul;

#[cfg(test)]
#[path = "../tests-rs/test_issue536_config_quoted_whitespace.rs"]
mod tests_issue536_config_quoted_whitespace;

#[cfg(test)]
#[path = "../tests-rs/test_issue606_repeat_time.rs"]
mod tests_issue606_repeat_time;

#[cfg(test)]
#[path = "../tests-rs/test_issue616_unicode_bind.rs"]
mod tests_issue616_unicode_bind;
