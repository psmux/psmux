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

/// Make `path` the current config file and return the one it replaced, for
/// a source job's command (CtrlReq::InFile).
pub(crate) fn swap_current_config_file(path: &str) -> String {
    CURRENT_CONFIG_FILE.with(|f| std::mem::replace(&mut *f.borrow_mut(), path.to_string()))
}

/// Put back the file swap_current_config_file replaced, if it replaced one.
pub(crate) fn restore_current_config_file(saved: &mut Option<String>) {
    if let Some(prev) = saved.take() {
        set_current_config_file(&prev);
    }
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
    // A re-seed restores every built-in copy-mode key, as tmux's
    // key_bindings_init does for its copy-mode tables.
    app.copy_mode_defaults_unbound.clear();
    let defaults = crate::help::PREFIX_DEFAULTS;
    let table = app.key_tables.entry("prefix".to_string()).or_default();
    for (key_str, cmd_str) in defaults {
        if let Some(key) = parse_key_name(key_str) {
            let key = normalize_key_for_binding(key);
            if let Some(action) = parse_command_to_action(cmd_str) {
                // Only add if not already present (user config may have overridden)
                if !table.iter().any(|b| b.key == key) {
                    table.push(Bind { key, action, repeat: false, note: None });
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
                    root_table.push(Bind { key, action, repeat: false, note: None });
                }
            }
        }
    }
}

/// The built-in keys of `table` that psmux handles itself, parsed once:
/// normalised key, the key name as `list-keys` prints it, and the command it
/// lists. For `copy-mode` / `copy-mode-vi` that is the copy-mode keys plus
/// the copy-mode wheel; for `root` the built-in mouse bindings
/// (`mouse_keys::ROOT_MOUSE_DEFAULTS`). Empty for any other table.
pub fn copy_mode_default_keys(table: &str) -> &'static [((KeyCode, KeyModifiers), &'static str, &'static str)] {
    use std::sync::OnceLock;
    type Parsed = Vec<((KeyCode, KeyModifiers), &'static str, &'static str)>;
    fn parse(lists: &[&'static [(&'static str, &'static str)]]) -> Parsed {
        lists.iter()
            .flat_map(|l| l.iter())
            .filter_map(|(k, c)| parse_key_name(k).map(|key| (normalize_key_for_binding(key), *k, *c)))
            .collect()
    }
    static VI: OnceLock<Parsed> = OnceLock::new();
    static EMACS: OnceLock<Parsed> = OnceLock::new();
    static ROOT: OnceLock<Parsed> = OnceLock::new();
    match table {
        "copy-mode-vi" => VI.get_or_init(|| parse(&[crate::help::COPY_MODE_VI_DEFAULTS, crate::mouse_keys::COPY_MODE_MOUSE_DEFAULTS])),
        "copy-mode" => EMACS.get_or_init(|| parse(&[crate::help::COPY_MODE_EMACS_DEFAULTS, crate::mouse_keys::COPY_MODE_MOUSE_DEFAULTS])),
        "root" => ROOT.get_or_init(|| parse(&[crate::mouse_keys::ROOT_MOUSE_DEFAULTS])),
        _ => &[],
    }
}

/// The copy-mode table the built-in handlers follow: `copy-mode-vi` for
/// `mode-keys vi`, `copy-mode` otherwise (tmux picks its table the same way).
pub fn active_copy_mode_table(app: &AppState) -> &'static str {
    if app.mode_keys == "vi" { "copy-mode-vi" } else { "copy-mode" }
}

/// True when `key` is a built-in copy-mode key of the active table that the
/// user unbound, so the built-in handler must leave it alone. In tmux an
/// unbound copy-mode key does nothing: the table simply has no entry for it.
pub fn copy_mode_default_unbound(app: &AppState, key: (KeyCode, KeyModifiers)) -> bool {
    if app.copy_mode_defaults_unbound.is_empty() { return false; }
    let key = normalize_key_for_binding(key);
    let table = active_copy_mode_table(app);
    app.copy_mode_defaults_unbound.contains(&(table.to_string(), key))
}

/// `unbind-key -T <table> <key>`: drop the key from the table, and when it is
/// a built-in copy-mode key record that, so the built-in handler stops acting
/// on it.
pub fn unbind_key_in_table(app: &mut AppState, table: &str, key: (KeyCode, KeyModifiers)) {
    let key = normalize_key_for_binding(key);
    if let Some(binds) = app.key_tables.get_mut(table) {
        binds.retain(|b| b.key != key);
    }
    if copy_mode_default_keys(table).iter().any(|(k, _, _)| *k == key) {
        app.copy_mode_defaults_unbound.insert((table.to_string(), key));
    }
}

/// `unbind-key -a -T <table>`: empty the table. For a copy-mode table that
/// takes the built-in keys away too, as it does in tmux.
pub fn unbind_all_in_table(app: &mut AppState, table: &str) {
    if let Some(binds) = app.key_tables.get_mut(table) {
        binds.clear();
    }
    for (k, _, _) in copy_mode_default_keys(table) {
        app.copy_mode_defaults_unbound.insert((table.to_string(), *k));
    }
}

/// Every binding `list-keys` reports, as `(table, key, command, repeat)`: the
/// key tables, then the built-in copy-mode keys that are neither rebound nor
/// unbound.
pub fn list_keys_entries(app: &AppState) -> Vec<(String, String, String, bool)> {
    let mut out: Vec<(String, String, String, bool)> = Vec::new();
    for (table_name, binds) in &app.key_tables {
        for bind in binds {
            out.push((
                table_name.clone(),
                format_key_binding(&bind.key),
                crate::commands::format_action(&bind.action),
                bind.repeat,
            ));
        }
    }
    for table in ["root", "copy-mode", "copy-mode-vi"] {
        let user = app.key_tables.get(table);
        for (key, name, cmd) in copy_mode_default_keys(table) {
            if user.map_or(false, |b| b.iter().any(|b| b.key == *key)) { continue; }
            if app.copy_mode_defaults_unbound.contains(&(table.to_string(), *key)) { continue; }
            out.push((table.to_string(), (*name).to_string(), (*cmd).to_string(), false));
        }
    }
    out
}

/// `list-keys -N` output (tmux cmd-list-keys.c, the notes_only template
/// `#{key_prefix} #{p|W:key_string} #{?key_note,#{key_note},#{key_command}}`).
///
/// With `-T` it lists that table, without it the prefix table then the root
/// table. Only bindings with a note are listed unless `-a` is given, `key`
/// keeps the one key, and every line starts with the prefix key, as in tmux.
pub fn list_key_notes(app: &AppState, table: Option<&str>, all: bool, key: Option<&str>) -> String {
    let tables: Vec<&str> = match table {
        Some(t) => vec![t],
        None => vec!["prefix", "root"],
    };
    let only = key.and_then(parse_key_name).map(normalize_key_for_binding);
    let mut rows: Vec<(String, String)> = Vec::new();
    for t in tables {
        let Some(binds) = app.key_tables.get(t) else { continue };
        for b in binds {
            if only.is_some_and(|k| k != b.key) {
                continue;
            }
            if b.note.is_none() && !all {
                continue;
            }
            let text = b.note.clone().unwrap_or_else(|| crate::commands::format_action(&b.action));
            rows.push((format_key_binding(&b.key), text));
        }
    }
    let width = rows.iter().map(|(k, _)| unicode_width::UnicodeWidthStr::width(k.as_str())).max().unwrap_or(0);
    let prefix = format_option_key(&app.prefix_key);
    let mut out = String::new();
    for (k, text) in rows {
        let pad = width.saturating_sub(unicode_width::UnicodeWidthStr::width(k.as_str()));
        out.push_str(&format!("{} {}{} {}\n", prefix, k, " ".repeat(pad), text));
    }
    out
}

/// The environment variable that marks a server as a session a config
/// file's `new-session` made (AppState::config_made_session). The connection
/// dispatcher sets it on the process it starts for a source job's
/// `new-session`, and the server takes it out of its own environment at once
/// so its panes and the servers it starts do not inherit it.
pub const CONFIG_SESSION_ENV: &str = "PSMUX_CONFIG_SESSION";

/// The `psmux server` argument that sets CONFIG_SESSION_ENV for the server,
/// for the dispatcher's `new-session`, which starts the server itself.
pub const CONFIG_SESSION_ARG: &str = "--config-session";

/// Whether the startup job of a session a config file's `new-session` made
/// (AppState::config_made_session) runs this kept command line.
///
/// tmux loads its config once per server (server-client.c:2578 to :2581,
/// cfg.c:89 to :90), and a session the config makes lives in that server, so it
/// shares what the config made server wide: options and bindings, the paste
/// buffers (one tree per server, paste.c:35), and what the run-shell scripts
/// set. psmux runs a server per session, so such a session runs those again:
/// its config lines apply as they do anywhere, and of its commands it runs
/// run-shell and the buffer commands. The others acted on the session that
/// loaded the config, and its `new-session` would start servers without end.
pub fn made_session_runs(line: &str) -> bool {
    let name = line.split_whitespace().next().unwrap_or("");
    matches!(
        name,
        "run-shell" | "run" | "set-buffer" | "setb" | "load-buffer" | "loadb" | "delete-buffer" | "deleteb"
    )
}

/// Load the startup config. Options and bindings before its first command
/// apply now, before the server has a window; from that command on, its lines
/// are kept in `app.startup_source_lines` for a source job, which the server
/// starts once its loop runs, as a runtime `source-file` keeps its own (issue
/// #842). tmux runs the whole config before the first client's command
/// (cfg.c:71 to :92), so its commands, a run-shell without `-b` that the lines
/// after it wait for included, all run. A warm standby keeps nothing: its one
/// window belongs to the next session, and the claim loads the config again.
/// A session a config's `new-session` made keeps its lines too, but its job
/// runs only some of the commands (made_session_runs).
pub fn load_config(app: &mut AppState) {
    // Start a fresh warning batch for this load/reload and mark the window so
    // nested `source-file` directives don't emit a runtime status message.
    app.config_warnings.clear();
    app.config_warn_line = None;
    app.startup_source_lines.clear();
    app.ppm_declared = false;
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
        app.startup_config_file = Some(expanded.clone());
        match std::fs::read_to_string(&expanded) {
            Ok(content) => parse_startup_config(app, &content),
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
    app.startup_config_file = None;
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            app.startup_config_file = Some(path.clone());
            set_current_config_file(&path);
            parse_startup_config(app, &content);
            set_current_config_file("");
            break;
        }
    }
}

/// Parse the startup config's text (load_config): from its first command on,
/// its lines are kept in `app.startup_source_lines` for the source job,
/// except on a warm standby, which keeps nothing, and in the shared server
/// (#798), which runs no source job yet: there the config is read as before,
/// its options applied and its run-shell started in place. The shared server
/// is the process started with SHARED_SERVER_ARG, as its own gate tells it.
fn parse_startup_config(app: &mut AppState, content: &str) {
    let shared_server = std::env::args().any(|a| a == crate::server::shared::SHARED_SERVER_ARG);
    if app.is_warm_server() || shared_server {
        parse_config_content(app, content);
    } else {
        app.startup_source_lines = parse_keeping_commands(app, |app| parse_config_content(app, content));
    }
}

/// Whether a runtime `source-file` names the config file the server started
/// with (`app.startup_config_file`) among its paths, so the source is a
/// config reload. `spec` is the arguments as the server receives them
/// (parse_source_spec): flags such as `-q` and `-F`, quotes, a leading `~`.
/// Paths are compared canonicalised, and on Windows without regard to case or
/// slash direction.
pub fn is_startup_config(app: &AppState, spec: &str) -> bool {
    let Some(start) = app.startup_config_file.as_deref() else { return false };
    let spec = parse_source_spec(spec);
    let expand = |p: &str| -> String {
        if p.starts_with('~') { p.replacen('~', &crate::paths::home_dir(), 1) } else { p.to_string() }
    };
    spec.paths.iter().any(|p| {
        let raw = if spec.format { crate::format::expand_format(p, app) } else { p.clone() };
        same_config_path(&expand(&raw), &expand(start))
    })
}

/// Two config paths naming the same file.
pub(crate) fn same_config_path(a: &str, b: &str) -> bool {
    let norm = |p: &std::path::Path| -> String {
        let s = p.to_string_lossy().replace('/', "\\");
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches('\\').to_string();
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => norm(&x) == norm(&y),
        _ => norm(std::path::Path::new(a)) == norm(std::path::Path::new(b)),
    }
}

pub fn parse_config_content(app: &mut AppState, content: &str) {
    parse_config_content_from(app, content, 1);
}

/// parse_config_content for text whose first line is line `first_line` of
/// its file, so its warnings and kept lines carry the file's line numbers (a
/// kept `if-shell` block, issue #842).
/// Join continuation lines (ending with \). Track the original 1-based line
/// number where each (possibly joined) logical line began, so config
/// warnings can be reported as `file:line: message` (issue #370 follow-up).
/// `first_line` is the file's line number of the first line of `content`.
fn join_continuations(content: &str, first_line: usize) -> Vec<(usize, String)> {
    let mut lines: Vec<(usize, String)> = Vec::new();
    let mut continuation = String::new();
    let mut cont_start: usize = 0;
    for (idx, line) in content.lines().enumerate() {
        let lineno = idx + first_line;
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
    lines
}

fn parse_config_content_from(app: &mut AppState, content: &str, first_line: usize) {
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

    let lines = join_continuations(content, first_line);

    // A command whose brace block spans several lines (`bind x {` ... `}`)
    // is one statement in tmux's grammar; make it one line here.
    let lines = join_brace_blocks(lines);

    // Each command on a line is a statement of its own (`a ; b`).
    let lines = split_lines_commands(lines);

    // tmux's lexer reads `{` and `}` as tokens wherever they stand
    // (cmd-parse.y yylex), so `if-shell C { A } { B }` on one line and a
    // `} {` line between the two blocks are both valid. The collector below
    // works on whole lines, so give every brace of an if-shell line, and of a
    // line that continues one (starts with a brace), a line of its own.
    let lines: Vec<(usize, String)> = lines
        .into_iter()
        .flat_map(|(lineno, line)| match split_brace_line(&line) {
            Some(parts) => parts.into_iter().map(|p| (lineno, p)).collect::<Vec<_>>(),
            None => vec![(lineno, line)],
        })
        .collect();
    // A block's commands are lines of their own now, so split them too
    // (`if-shell C { a ; b }` runs both, cmd-parse.y argument_statements).
    let lines = split_lines_commands(lines);

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
                    start_line: Some(*orig_lineno),
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
                        // tmux cmd-parse.y:264 puts %hidden in the GLOBAL
                        // environment with ENVIRON_HIDDEN: usable as $NAME and
                        // in formats, never passed to a child (#775).
                        app.env_scopes.global_hidden.insert(name.to_string(), value.to_string());
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
            // %hidden entries live in the global hidden set (#775); session
            // entries keep their old precedence over them.
            let mut vars: std::collections::HashMap<String, String> = app.env_scopes.global_hidden
                .iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            vars.extend(app.env_scopes.session_hidden.iter().map(|(k, v)| (k.clone(), v.clone())));
            vars.extend(app.environment.iter().map(|(k, v)| (k.clone(), v.clone())));
            expand_hidden_vars(l, &vars)
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

/// The block brace depth before each character of `chars`, the way tmux's
/// lexer reads block braces (cmd-parse.y yylex / yylex_token): outside
/// quotes, a `{` at the start of a token opens a block, and a `}` closes one
/// unless it closes a `#{...}` format. `None` when the braces do not balance
/// (a `}` with no block open, or a block still open at the end), so callers
/// fall back to reading the text without blocks, as psmux always did.
pub(crate) fn block_depth_map(chars: &[char]) -> Option<Vec<u32>> {
    let mut depths = Vec::with_capacity(chars.len());
    let mut depth: u32 = 0;
    let mut quote: Option<char> = None;
    let mut format_depth = 0usize;
    let mut at_token_start = true;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        depths.push(depth);
        if let Some(q) = quote {
            if c == '\\' && q == '"' && i + 1 < chars.len() {
                depths.push(depth);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            at_token_start = false;
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                at_token_start = false;
            }
            '\\' => {
                if i + 1 < chars.len() {
                    depths.push(depth);
                    i += 1;
                }
                at_token_start = false;
            }
            '#' if chars.get(i + 1) == Some(&'{') => {
                depths.push(depth);
                i += 1;
                format_depth += 1;
                at_token_start = false;
            }
            '}' if format_depth > 0 => {
                format_depth -= 1;
                at_token_start = false;
            }
            '{' if at_token_start => {
                depth += 1;
                at_token_start = true;
            }
            '}' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                // The `}` itself sits outside the block it closes.
                *depths.last_mut().unwrap() = depth;
                at_token_start = true;
            }
            ';' => at_token_start = true,
            c if c.is_whitespace() => at_token_start = true,
            _ => at_token_start = false,
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    Some(depths)
}

/// The inside of `text` when all of it is one brace block, `{ ... }`.
fn whole_block_inner(text: &str) -> Option<&str> {
    let t = text.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return None;
    }
    let chars: Vec<char> = t.chars().collect();
    let depths = block_depth_map(&chars)?;
    // Depth must stay above zero from the opening brace to the closing one.
    if depths.iter().skip(1).take(chars.len().saturating_sub(2)).any(|&d| d == 0) {
        return None;
    }
    Some(t[1..t.len() - 1].trim())
}

/// The command list of a key binding: `bind x a \; b`, `bind x 'a; b'` and
/// tmux's block form `bind x { a; b }` (one line, or several lines joined by
/// `join_brace_blocks`) all give `[a, b]`. Nested blocks stay inside their
/// command (`if -F c { a } { b }` is one command).
pub fn bind_command_list(command: &str) -> Vec<String> {
    match whole_block_inner(command) {
        Some(inner) => split_chained_commands(inner),
        None => split_chained_commands(command),
    }
}

/// [`bind_command_list`] with each command's `command-alias` expanded. tmux
/// parses the bound list at bind time (cmd-bind-key.c, cmd_parse_from_arguments)
/// and expands aliases there (cmd-parse.y cmd_parse_expand_alias), so
/// `bind x split-pane -h` binds `split-window -h` and list-keys shows that.
pub fn bind_commands(aliases: &crate::command_alias::SharedAliases, command: &str) -> Vec<String> {
    let aliases = crate::command_alias::read(aliases);
    let mut commands = Vec::new();
    for sub in bind_command_list(command) {
        match aliases.expand_lines(&crate::commands::parse_command_line(&sub)) {
            Some(lines) => commands.extend(lines),
            None => commands.push(sub),
        }
    }
    commands
}

/// Join the lines of a statement whose brace block spans several lines, as
/// tmux's grammar does (`bind x {`, statements on their own lines, `}`),
/// into one line: a newline between two statements becomes ` ; `, one after
/// an opening brace or before a closing one a space. if-shell blocks keep
/// their own collector below and are left alone, as are `%` directives and
/// comments. A line is only a block opener when it ENDS with a block brace,
/// so a key named `{` (`bind { swap-pane -U`, which psmux has always
/// accepted) never swallows the lines after it.
fn join_brace_blocks(lines: Vec<(usize, String)>) -> Vec<(usize, String)> {
    let opens_block = |l: &str| -> bool {
        let t = l.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('%')
            || t.starts_with('{') || t.starts_with('}')
            || t.starts_with("if-shell ") || t.starts_with("if ")
            || !t.ends_with('{')
        {
            return false;
        }
        // The trailing `{` must be a block brace that stays open.
        let mut probe = t.to_string();
        probe.push_str(" }");
        let chars: Vec<char> = probe.chars().collect();
        block_depth_map(&chars).is_some()
    };
    let mut out: Vec<(usize, String)> = Vec::with_capacity(lines.len());
    let mut iter = lines.into_iter();
    while let Some((lineno, line)) = iter.next() {
        if !opens_block(&line) {
            out.push((lineno, line));
            continue;
        }
        let mut acc = line.trim().to_string();
        for (_, next) in iter.by_ref() {
            let p = next.trim();
            if p.is_empty() || p.starts_with('#') {
                continue;
            }
            let sep = if acc.ends_with('{') || p.starts_with('}') { " " } else { " ; " };
            acc.push_str(sep);
            acc.push_str(p);
            let chars: Vec<char> = acc.chars().collect();
            if block_depth_map(&chars).is_some() {
                break;
            }
        }
        out.push((lineno, acc));
    }
    out
}

/// The commands on one config line, split at each `;` that ends a command.
///
/// tmux reads a file with its string grammar, where an unquoted `;` ends the
/// token it is in and the command with it, wherever it stands
/// (cmd-parse.y:1314 and :1675, tag 3.7c): `set -g @a on ; set -g @b on` and
/// `set -g @a on;set -g @b on` are two commands. psmux handed the whole line
/// to the parser, so a `set` line took the rest of the line for its value.
///
/// A `;` is left in place inside quotes, inside a `{ }` block (the block's
/// command splits it when it runs), after a backslash (`\;` is how a key
/// binding holds a list of commands), and after a `#` that starts a token,
/// which is a comment. A key name such as `M-;` is not cut either, as
/// split_chained_commands keeps it. A line that starts with `;` or `%` is
/// returned whole: tmux reports the first as a syntax error, which the line
/// parser still does, and `%if` lines are directives.
pub(crate) fn split_line_commands(line: &str) -> Vec<String> {
    let t = line.trim();
    if t.starts_with(';') || t.starts_with('%') || !t.contains(';') {
        return vec![line.to_string()];
    }
    let chars: Vec<char> = t.chars().collect();
    let depths = block_depth_map(&chars);
    let at_top = |i: usize| depths.as_ref().map_or(true, |d| d[i] == 0);
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut token_start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if in_single {
            if c == '\'' { in_single = false; }
            i += 1;
            continue;
        }
        if in_double {
            if c == '\\' { i += 2; continue; }
            if c == '"' { in_double = false; }
            i += 1;
            continue;
        }
        match c {
            '\\' => { i += 2; continue; }
            '\'' => in_single = true,
            '"' => in_double = true,
            ' ' | '\t' => token_start = i + 1,
            // The rest is a comment, as strip_inline_comment reads it.
            '#' if i == token_start => break,
            ';' if at_top(i) => {
                let ends_token = chars.get(i + 1).is_none_or(|n| n.is_whitespace());
                let token: String = chars[token_start..=i].iter().collect();
                if !(ends_token && is_modified_semicolon_key(&token)) {
                    let seg: String = chars[start..i].iter().collect();
                    if !seg.trim().is_empty() {
                        out.push(seg.trim().to_string());
                    }
                    start = i + 1;
                    token_start = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let rest: String = chars[start.min(chars.len())..].iter().collect();
    if !rest.trim().is_empty() {
        out.push(rest.trim().to_string());
    }
    out
}

/// Every line of `lines` split into its commands (split_line_commands), each
/// keeping the line number it came from.
fn split_lines_commands(lines: Vec<(usize, String)>) -> Vec<(usize, String)> {
    lines
        .into_iter()
        .flat_map(|(lineno, line)| {
            split_line_commands(&line).into_iter().map(move |c| (lineno, c))
        })
        .collect()
}

/// Split a config line at its block braces, for the if-shell collector.
///
/// Returns `None` when the line is left alone: it is not an if-shell line
/// and does not start with a brace, or it has no block brace. Otherwise the
/// pieces in order, where a header is kept with the `{` that opens its block
/// (`if-shell 'c' {`) and every other brace stands alone:
/// `if-shell 'c' { A } { B }` gives `if-shell 'c' {`, `A`, `}`, `{`, `B`, `}`.
///
/// A brace is a block brace the way tmux's lexer reads one: outside quotes,
/// `{` at the start of a token, and `}` anywhere except where it closes a
/// `#{...}` format (yylex_token, cmd-parse.y).
pub(crate) fn split_brace_line(line: &str) -> Option<Vec<String>> {
    let l = line.trim();
    let candidate = l.starts_with("if-shell ")
        || l.starts_with("if ")
        || l.starts_with('{')
        || l.starts_with('}');
    if !candidate {
        return None;
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut seg = String::new();
    let mut quote: Option<char> = None;
    let mut format_depth = 0usize;
    let mut at_token_start = true;
    let mut found = false;
    let mut chars = l.chars().peekable();
    let flush = |seg: &mut String, pieces: &mut Vec<String>| {
        let s = seg.trim();
        if !s.is_empty() {
            pieces.push(s.to_string());
        }
        seg.clear();
    };
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                seg.push(c);
                if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        seg.push(n);
                    }
                } else if c == q {
                    quote = None;
                }
                at_token_start = false;
                continue;
            }
            None => {}
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                seg.push(c);
                at_token_start = false;
            }
            '\\' => {
                seg.push(c);
                if let Some(n) = chars.next() {
                    seg.push(n);
                }
                at_token_start = false;
            }
            '#' if chars.peek() == Some(&'{') => {
                seg.push(c);
                seg.push(chars.next().unwrap());
                format_depth += 1;
                at_token_start = false;
            }
            '}' if format_depth > 0 => {
                seg.push(c);
                format_depth -= 1;
                at_token_start = false;
            }
            '{' if at_token_start => {
                found = true;
                // The header keeps its opening brace; a brace after a brace
                // (the else block of `} {`) stands alone.
                let s = seg.trim().to_string();
                seg.clear();
                if s.is_empty() {
                    pieces.push("{".to_string());
                } else {
                    pieces.push(format!("{} {{", s));
                }
                at_token_start = true;
            }
            '}' => {
                found = true;
                flush(&mut seg, &mut pieces);
                pieces.push("}".to_string());
                at_token_start = true;
            }
            c if c.is_whitespace() => {
                seg.push(c);
                at_token_start = true;
            }
            ';' => {
                seg.push(c);
                at_token_start = true;
            }
            _ => {
                seg.push(c);
                at_token_start = false;
            }
        }
    }
    if !found {
        return None;
    }
    flush(&mut seg, &mut pieces);
    Some(pieces)
}

/// Process a collected if-shell brace block by evaluating the condition
/// and executing the appropriate branch.
fn process_brace_if_shell(app: &mut AppState, bb: &parse_config_content_types::BraceBlock) {
    // Past a runtime source's first command, the block waits its turn whole:
    // its condition has to see what the commands before it made (issue #842).
    if source_deferring(app) {
        let mut text = format!("{}\n", bb.if_line);
        for l in &bb.true_lines {
            text.push_str(l);
            text.push('\n');
        }
        text.push_str("}\n");
        if !bb.else_lines.is_empty() {
            text.push_str("{\n");
            for l in &bb.else_lines {
                text.push_str(l);
                text.push('\n');
            }
            text.push_str("}\n");
        }
        defer_line(app, bb.start_line, crate::types::DeferredKind::Block(text));
        return;
    }
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

    // A shell condition is format expanded before it runs, like tmux
    // (cmd-if-shell.c:86); untimed, as `%` in a shell command is not strftime.
    if !format_mode {
        condition = crate::format::expand_format_untimed(&condition, app);
    }

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
        // Never the server's own stdio: dead after a C-c relay (#819).
        c.stdin(std::process::Stdio::null());
        crate::commands::null_job_output(&mut c);
        crate::commands::apply_job_env(&mut c);
        { use crate::platform::HideWindowCommandExt; c.hide_window(); }
        c.status().map(|s| s.success()).unwrap_or(false)
    };

    // Execute the appropriate branch
    let lines = if success { &bb.true_lines } else { &bb.else_lines };
    // A branch holding a block of its own (`if-shell A { if-shell B { ... } }`)
    // goes back through the block parser; a flat one keeps per line handling.
    let nested = lines.iter().any(|l| {
        let l = l.trim();
        (l.starts_with("if-shell ") || l.starts_with("if ")) && l.ends_with('{')
    });
    if nested {
        let saved = app.config_warn_line;
        parse_config_content(app, &lines.join("\n"));
        app.config_warn_line = saved;
        return;
    }
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
        /// The line the block's `if-shell` stands on.
        pub start_line: Option<usize>,
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

/// Whether parse_config_line applies `l` itself (the chain at its head), as
/// opposed to a command, which tmux runs from the queue. Matches that chain
/// prefix for prefix, so the two cannot disagree about a line.
pub(crate) fn is_config_directive(l: &str) -> bool {
    const HEADS: [&str; 17] = [
        "set-option ", "set ", "setw ", "set-window-option ",
        "bind-key ", "bind ", "unbind-key ", "unbind ",
        "source-file ", "source ", "run-shell ", "run ",
        "if-shell ", "if ", "set-hook ", "set-environment ", "setenv ",
    ];
    HEADS.iter().any(|h| l.starts_with(h))
}

/// Whether a runtime source has reached its first command, so every line from
/// here on is kept for the source job (issue #842).
fn source_deferring(app: &AppState) -> bool {
    app.source_defer.as_ref().is_some_and(|d| d.triggered)
}

/// Keep a line for the source job, with the file and line it came from.
fn defer_line(app: &mut AppState, line: Option<usize>, kind: crate::types::DeferredKind) {
    let file = current_config_file();
    let ancestors = SOURCE_STACK.with(|s| s.borrow().clone());
    if let Some(d) = app.source_defer.as_mut() {
        d.lines.push(crate::types::DeferredConfigLine { file, line, kind, ancestors });
    }
}

/// True if `name` is a recognized psmux command (canonical name or alias),
/// per the TMUX_COMMANDS catalog plus any user-defined command-aliases.
/// Used so the unknown-command warning does not fire on valid commands that
/// the config parser itself does not route (e.g. `new-window`, `display-message`).
pub(crate) fn is_known_command(app: &AppState, name: &str) -> bool {
    if crate::command_alias::read(&app.command_aliases).contains_key(name) {
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

    // Issue #635: tmux fails the whole config LINE when a value-taking flag
    // has no value ("-t expects an argument" from arguments.c) and never runs
    // the command. Report it the way every other config diagnostic is
    // reported and skip the directive rather than letting it act on the
    // default target.
    {
        let tokens = crate::commands::parse_command_line(l);
        if let Err(flag_error) = crate::cli::validate_command_line_flags(&tokens) {
            warn_config(app, flag_error);
            return;
        }
    }

    // A runtime source past its first command keeps every line, the options
    // too, so they run in file order after the commands before them (issue
    // #842). An unknown command is still reported here and never runs.
    if source_deferring(app) {
        let line = app.config_warn_line;
        if is_config_directive(l) && !run_shell_in_job(l) {
            defer_line(app, line, crate::types::DeferredKind::Directive(l.to_string()));
        } else {
            let cmd = l.split_whitespace().next().unwrap_or("");
            if is_known_command(app, cmd) {
                defer_line(app, line, crate::types::DeferredKind::Command(l.to_string()));
            } else {
                warn_config(app, format!("unknown command: {}", cmd));
            }
        }
        return;
    }

    // A run-shell that waits is a command in a runtime source: it starts the
    // queue, and the job waits for the shell before the next line runs.
    if app.source_defer.is_some() && run_shell_waits(l) {
        if let Some(d) = app.source_defer.as_mut() {
            d.triggered = true;
        }
        let line = app.config_warn_line;
        defer_line(app, line, crate::types::DeferredKind::Command(l.to_string()));
        return;
    }

    if l.starts_with("set-option ") || l.starts_with("set ") {
        parse_set_option(app, l, false);
    }
    else if l.starts_with("setw ") || l.starts_with("set-window-option ") {
        parse_set_option(app, l, true);
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
            // A glob in a file is a glob, as it is in tmux (cmd-source-file.c:221),
            // not a path with a `*` in it, and every path on the line is read.
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
        // Same parser and scopes as the CLI/TCP/prompt paths (#775): -g
        // global, -u, -r removal marker, -h hidden, -F format value. The
        // process environment is kept equal to what a child inherits.
        let tokens = crate::commands::parse_command_line(l);
        let args: Vec<&str> = tokens.iter().skip(1).map(|s| s.as_str()).collect();
        let parsed = crate::environ::parse_set_environment(&args).or_else(|e| {
            // psmux has always joined an unquoted multi word value; keep
            // accepting `set-environment NAME several words` from configs.
            if !e.starts_with("too many arguments") { return Err(e); }
            let name_at = args.iter().position(|a| !a.starts_with('-')).unwrap_or(0);
            let mut joined: Vec<String> = args[..=name_at].iter().map(|s| s.to_string()).collect();
            joined.push(args[name_at + 1..].join(" "));
            let refs: Vec<&str> = joined.iter().map(|s| s.as_str()).collect();
            crate::environ::parse_set_environment(&refs)
        });
        match parsed {
            Ok(mut a) => {
                if a.format {
                    if let Some(v) = a.value.take() {
                        a.value = Some(crate::format::expand_format_untimed(&v, app));
                    }
                }
                crate::environ::apply_set(&mut app.environment, &mut app.env_scopes, &a);
            }
            Err(e) => warn_config(app, format!("set-environment: {}", e)),
        }
    }
    else {
        // Unrecognized directive. Warn only when the first token is not a known
        // command (a known-but-unrouted command like `new-window` stays silent
        // to match prior behavior; a genuine typo like `bnid-key` is surfaced).
        let cmd = l.split_whitespace().next().unwrap_or("");
        // In a runtime source and in the startup config every command is kept
        // for the source job, which runs it through the connection dispatcher
        // once the commands before it have finished, as tmux's queue does
        // (issue #842). That is the one route a config file's commands take.
        // Where no job runs, a command is left out: a warm standby's own boot
        // (the claim loads the config again, with its job) and a runtime
        // source on a server with no window. A session a config's
        // `new-session` made has a job, which runs only some of the commands
        // (made_session_runs).
        if app.source_defer.is_some() && is_known_command(app, cmd) {
            if let Some(d) = app.source_defer.as_mut() {
                d.triggered = true;
            }
            let line = app.config_warn_line;
            defer_line(app, line, crate::types::DeferredKind::Command(l.to_string()));
            return;
        }
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
            // Outside quotes `\'` and `\"` are a literal quote, not a quote
            // that opens (parse_command_line, issue #823).
            '\\' if !in_single && matches!(line[idx + 1..].chars().next(), Some('\'' | '"')) => {
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
            // Adjacent quoted and escaped parts that make ONE argument, such
            // as 'a'\''b' or 'it'"'"'s', join as tmux joins them (issue #823).
            let joined = crate::commands::parse_command_line(rest);
            if joined.len() == 1 {
                return joined.into_iter().next().unwrap_or_default();
            }
            return strip_wrapping_quotes(rest).to_string();
        }
        out.push(c);
    }
    // Unterminated quote: behave as before.
    strip_wrapping_quotes(rest).to_string()
}

fn parse_set_option(app: &mut AppState, line: &str, window_command: bool) {
    let toks = tokens_with_offsets(line);
    if toks.len() < 2 { warn_config(app, "set-option requires an option name"); return; }

    let mut i = 1;
    let mut is_global = false;
    let mut global_flag = false;    // -g itself (is_global also covers -s)
    let mut format_expand = false;  // -F: expand format strings in value
    let mut only_if_unset = false;  // -o: only set if not already set
    let mut append_mode = false;    // -a: append to current value
    let mut unset_mode = false;     // -u: unset (reset to default)
    let mut quiet = false;          // -q: suppress the "already set" error
    let mut window_scope = window_command;
    let mut pane_scope = false;     // -p: the target pane's own table (#728)
    let mut target = String::new();

    while i < toks.len() {
        let p = toks[i].1.as_str();
        if p.starts_with('-') {
            if p.contains('g') { is_global = true; global_flag = true; }
            // -s is tmux's server scope (#618). psmux keeps a single option
            // store, so it resolves to the global one, exactly like -g. A
            // config carrying `set -s default-terminal xterm-256color` (or the
            // very common `set -sg escape-time 0`) must land the value, not
            // drop the scope on the floor.
            if p.contains('s') { is_global = true; }
            if p.contains('F') { format_expand = true; }
            if p.contains('o') { only_if_unset = true; }
            if p.contains('a') { append_mode = true; }
            if p.contains('w') { window_scope = true; }
            if p.contains('p') { pane_scope = true; }
            // -U is an unset alias of -u (tmux parity, #553); the contains
            // check is case-sensitive so both must be tested.
            if p.contains('u') || p.contains('U') { unset_mode = true; }
            // -q (quiet) suppresses the `-o` "already set" refusal below, which
            // is the one error tmux's cmd-set-option.c lets `-q` swallow at
            // exit 0 (`if (args_has(args, 'q')) goto out;`). Unknown options
            // are still reported.
            if p.contains('q') { quiet = true; }
            i += 1;
            // #648: the target was skipped and thrown away, so a config line
            // `set -w -t "s:zero" remain-on-exit on` could not name a window
            // even once windows had their own option tables.
            if p.contains('t') && i < toks.len() {
                target = strip_wrapping_quotes(&toks[i].1).to_string();
                i += 1;
            }
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
    // tmux refuses a name its table does not know on every scope and with
    // every flag, -u included: "invalid option: NAME", which -q silences
    // (cmd-set-option.c:262-270). Nothing is stored (#846).
    if !crate::server::option_catalog::is_valid_option_name(key) {
        if !quiet {
            warn_config(app, format!("unknown option '{}'", key));
        }
        return;
    }
    if let Err(error) =
        crate::server::option_catalog::validate_local_window_override(
            key,
            window_scope && !is_global,
        )
    {
        warn_config(app, error);
        return;
    }

    // #728: `-p` writes the target PANE's own table, through the same writer
    // the CLI and TCP routes use, so the command prompt and `source-file` land
    // `set -p @name v` on the pane instead of in the global store. A startup
    // config has no pane yet; that line keeps falling through to the global
    // store below, the way an untargeted `-w` line does.
    let has_pane_to_target = !app.windows.is_empty();
    if pane_scope && !is_global && has_pane_to_target {
        let value = if format_expand && !raw_value.is_empty() {
            crate::format::expand_format(&raw_value, app)
        } else {
            raw_value.clone()
        };
        if value.is_empty() && !unset_mode {
            warn_config(app, format!("empty value for '{}'", key));
            return;
        }
        let reply = crate::server::options::apply_set_pane_option(
            app, &target, key, &value, unset_mode, append_mode, only_if_unset, quiet,
        );
        if let Some(error) = reply.strip_prefix("ERROR: ") {
            warn_config(app, error.to_string());
        }
        return;
    }

    // `set -gw @k` / `setw -g @k` writes tmux's global window table, which
    // every window inherits from (#795 follow up).
    if crate::server::options::is_global_window_user_write(window_scope, global_flag, key) {
        let value = if format_expand && !raw_value.is_empty() {
            crate::format::expand_format(&raw_value, app)
        } else {
            raw_value.clone()
        };
        let reply = crate::server::options::apply_set_window_option(
            app,
            crate::server::options::GLOBAL_WINDOW_TABLE,
            key,
            &value,
            unset_mode,
            append_mode,
            only_if_unset,
            quiet,
        );
        if let Some(error) = reply.strip_prefix("ERROR: ") {
            warn_config(app, error.to_string());
        }
        return;
    }

    // #648: `-w` / `setw` without `-g` writes the TARGET WINDOW's own option
    // table, not the one global store. Same rule as the CLI and TCP routes:
    // scope follows the option NAME (tmux's options_scope_from_name), so a
    // session or server option under `-w` still lands in the global store and
    // every config that has ever relied on that keeps working.
    // A startup config file runs BEFORE any window exists, so an untargeted
    // `-w` line has no window to write to. Real tmux drops it on the floor
    // (measured on 3.4: `setw monitor-activity on` in a `-f` config left both
    // the window and the global table untouched), but psmux has always landed
    // those lines in the one store, and every `.psmux.conf` carrying a bare
    // `setw` line depends on that. So an untargeted `-w` with no window to aim
    // at falls through to the global window table below rather than vanishing.
    let has_window_to_target = !app.windows.is_empty()
        || !target.trim().is_empty();
    if window_scope
        && !is_global
        && has_window_to_target
        && crate::server::options::is_window_scoped_write(key)
    {
        let value = if format_expand && !raw_value.is_empty() {
            crate::format::expand_format(&raw_value, app)
        } else {
            raw_value.clone()
        };
        // A boolean named with no value toggles, exactly as it does at global
        // scope (#278) — against the value the WINDOW currently resolves to.
        let (value, unset_mode) = if value.is_empty() && !unset_mode && !append_mode
            && crate::server::options::is_boolean_option(key)
        {
            let index = crate::server::options::resolve_option_target_window(app, &target)
                .unwrap_or(app.active_idx);
            let current = crate::server::options::resolve_window_option(app, index, key);
            (if current == "on" { "off".to_string() } else { "on".to_string() }, false)
        } else {
            (value, unset_mode)
        };
        let reply = crate::server::options::apply_set_window_option(
            app, &target, key, &value, unset_mode, append_mode, only_if_unset, quiet,
        );
        if let Some(error) = reply.strip_prefix("ERROR: ") {
            warn_config(app, error.to_string());
        } else {
            app.user_set_options.insert(key.to_string());
        }
        return;
    }

    // #803: a session option without -g writes the SESSION's own table, and
    // `-u` drops that entry so the global shows through again. A startup
    // config runs before the session has a window, so its lines keep landing
    // in the global table they always have (the rule an untargeted -w line
    // follows, #648); the command prompt, key bindings and a runtime
    // source-file reach the session the way tmux's do.
    if crate::server::options::is_session_option(key) && !pane_scope {
        let value = if format_expand && !raw_value.is_empty() {
            crate::format::expand_format(&raw_value, app)
        } else {
            raw_value.clone()
        };
        if !is_global && !app.windows.is_empty() {
            let reply = crate::server::options::apply_set_session_option(
                app, key, &value, unset_mode, append_mode, only_if_unset, quiet,
            );
            if let Some(error) = reply.strip_prefix("ERROR: ") {
                warn_config(app, error.to_string());
            }
            return;
        }
        if is_global && crate::server::options::session_overrides(app, key) {
            use crate::server::options::GlobalWrite;
            let op = if unset_mode {
                GlobalWrite::Unset
            } else if append_mode {
                GlobalWrite::Append(&value)
            } else if value.is_empty() && crate::server::options::is_boolean_option(key) {
                GlobalWrite::Toggle
            } else if only_if_unset {
                GlobalWrite::OnlyIfUnset(&value)
            } else {
                GlobalWrite::Set(&value)
            };
            match crate::server::options::write_shadowed_global(app, key, op) {
                Err(error) if !(quiet && error.starts_with("already set")) => warn_config(app, error),
                _ => {}
            }
            return;
        }
    }

    // Handle -u (unset): restore the option's table default.
    //
    // Issue #619: the `user_set_options` erase used to be reachable only for
    // window-style and window-active-style (the two options #617 repaired), so
    // for every other non `@` option the key survived the unset and the `-o`
    // guard below still read it as set. `set -gu escape-time` followed by
    // `set -go escape-time 77` therefore left escape-time at its default and
    // dropped the 77 on the floor. tmux clears the option at the scope on `-u`
    // (cmd-set-option.c calls options_remove_or_default) and then `-o` finds
    // nothing set (`already = (o != NULL)` in the same file), so it applies.
    //
    // #619 follow up: this branch also wrote an EMPTY value where tmux writes
    // the TABLE DEFAULT, so `set -gu escape-time` in a config file left the old
    // number in place while the CLI route restored 500, and `set -gu
    // status-style` produced a styleless status bar rather than the stock
    // green one. Both the value restore and the explicit-set erase now live in
    // one shared helper that the server request loop and the plugin drain loop
    // call too, so all three unset routes land on the same value.
    if unset_mode {
        crate::server::options::reset_option_to_default(app, key);
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

    // Handle -o (only set if not currently set).
    //
    // Refusing is not enough: tmux REPORTS the refusal. cmd-set-option.c ends
    // its `-o` guard with
    //
    //     if (already) {
    //             if (args_has(args, 'q'))
    //                     goto out;
    //             cmdq_error(item, "already set: %s", argument);
    //             goto fail;
    //     }
    //
    // so without `-q` the command FAILS and names the option. psmux dropped the
    // write in silence on every route, which made `-o` useless for its one job:
    // a plugin that seeds a default it does not want to clobber could not tell
    // "I set it" from "the user already had it" (#619 follow up). The config
    // route records it as a config warning, exactly like "unknown option", so
    // it reaches ~/.psmux/config-warnings.log and the attach-time summary; the
    // in-TUI command prompt runs through this same parser, so it also gets a
    // status message the way a failed command should.
    // Expand format strings in the value if -F flag is set. No quote trimming
    // here any more: extract_option_value already resolved the quoting, so a
    // value whose content legitimately begins and ends with a quote keeps it.
    let value = if format_expand && !raw_value.is_empty() {
        crate::format::expand_format(&raw_value, app)
    } else {
        raw_value
    };

    if append_mode {
        if let Err(error) = crate::server::option_catalog::validate_option_append(key) {
            warn_config(app, error);
            return;
        }
        // An ARRAY element of its own, not text glued onto the joined value.
        if crate::command_alias::is_alias_option(key) {
            let set = crate::command_alias::write(&app.command_aliases).set(key, &value, true);
            if let Err(error) = set {
                warn_config(app, error);
            }
            return;
        }
    }

    // Handle -a (append to current value)
    let final_value = if append_mode {
        let current = crate::format::lookup_option_pub(key, app).unwrap_or_default();
        if matches!(key, "terminal-overrides" | "codepoint-widths") {
            // ARRAY option: `-a` adds elements, it does not glue characters
            // onto the last one (tmux options_array_assign), so
            // `set -ga terminal-overrides 'xterm*:Tc'` without a leading
            // comma still lands as its own element.
            format!("{},{}", current, value)
        } else {
            format!("{}{}", current, value)
        }
    } else {
        value
    };

    // -o still validates the requested assignment before deciding that the
    // existing value wins.
    if only_if_unset {
        let already_set = if key.starts_with('@') {
            app.user_options.contains_key(key)
        } else {
            app.user_set_options.contains(key)
        };
        if already_set {
            if let Err(error) =
                crate::server::option_catalog::validate_option_value(key, &final_value)
            {
                warn_config(app, error);
            } else if !quiet {
                warn_config(app, format!("already set: {}", key));
                if !in_startup_load() {
                    app.status_message = Some((
                        format!("already set: {}", key),
                        std::time::Instant::now(),
                        None,
                    ));
                }
            }
            return;
        }
    }

    // Pass key and value separately. Rejoining them into one string could not
    // represent a value with leading or trailing spaces, which the receiver
    // then trimmed back off (#536).
    if parse_option_value(app, key, &final_value, is_global) {
        // Track that this option was explicitly set (for -o only-if-unset checks)
        app.user_set_options.insert(key.to_string());
    }
}

/// Apply one option, given its name and its **exact** value.
///
/// The value arrives verbatim: the caller has already resolved quoting, so a
/// deliberate run of spaces, or leading/trailing whitespace inside a quoted
/// config value, survives to here. This used to take a single `"key value"`
/// string and re-split it, which could not represent those runs at all and
/// trimmed the ends back off (#536).
///
/// Returns false when validation rejects the value.
pub fn parse_option_value(app: &mut AppState, key: &str, value: &str, _is_global: bool) -> bool {
    let key = key.trim();
    if let Err(error) = crate::server::option_catalog::validate_option_value(key, value) {
        warn_config(app, error);
        return false;
    }

    // Keep the existing boolean diagnostics. Numeric assignments are validated
    // once above against the destination type declared by the option catalog.
    if !value.is_empty() {
        if let Some(def) = crate::server::option_catalog::OPTION_CATALOG
            .iter()
            .find(|d| d.name == key)
        {
            let v = value.trim();
            match def.option_type {
                crate::server::option_catalog::OptionType::Boolean => {
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
        "mouse-drag-enter-copy-mode" => app.mouse_drag_enter_copy_mode = matches!(value, "on" | "true" | "1" | "yes"),
        "ctrl-wheel-zoom" => app.ctrl_wheel_zoom = matches!(value, "on" | "true" | "1" | "yes"),
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
        "sixel" => app.sixel = matches!(value, "on" | "true" | "1" | "yes"),
        // A bad key was refused (and reported) by the catalog check at the top.
        "prefix" => {
            if let Ok(key) = parse_option_key(value, true) {
                app.prefix_key = key;
                ensure_prefix_self_binding(app);
            }
        }
        "prefix2" => {
            if value.is_empty() {
                app.prefix2_key = None;
            } else if let Ok(key) = parse_option_key(value, true) {
                app.prefix2_key = key;
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
            // Stored only. The blink reaches the terminal as part of the
            // DECSCUSR code the shape resolves to (rendering.rs), which is how
            // tmux carries it too, its `cursor-style` values naming the blink.
            // This used to also write DEC private mode 12 here, which went
            // nowhere: config parsing runs in the server, and the server is
            // started with its own hidden console (platform.rs
            // `spawn_server_hidden`), so those bytes landed on a screen buffer
            // no one sees.
            let on = matches!(value, "on"|"true"|"1");
            env::set_var("PSMUX_CURSOR_BLINK", if on { "1" } else { "0" });
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
            // off, on, failed, key or failed-key; the catalog check above
            // refused the rest.
            if let Some(remain) = crate::types::RemainOnExit::parse(value) {
                app.remain_on_exit = remain;
            }
        }
        "destroy-unattached" => {
            app.destroy_unattached = matches!(value, "on" | "true" | "1" | "yes");
        }
        "exit-empty" => {
            app.exit_empty = matches!(value, "on" | "true" | "1" | "yes");
            crate::server::sync_held_marker(app);
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
        "terminal-overrides" => {
            // Array option (issue #700): smcup/rmcup decide whether the
            // attach client uses the host's alternate screen.
            app.terminal_overrides = crate::terminal_overrides::split_array(value);
        }
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
        "pane-border-format" | "pane-border-status" | "pane-border-indicators" => { app.user_options.insert(key.to_string(), value.to_string()); }
        // Read back from user_options by the copy mode gutter and the border
        // renderer. Without an arm here they reached the fallthrough below,
        // which stored the value (so it worked) and reported the option as
        // unknown (#706). The value of copy-mode-line-numbers is checked
        // against its choices by the catalog before this match, like tmux.
        "copy-mode-line-numbers" | "copy-mode-line-number-style"
        | "copy-mode-current-line-number-style" | "pane-border-lines"
        | "copy-mode-match-style" | "copy-mode-current-match-style" | "copy-mode-mark-style" => {
            app.user_options.insert(key.to_string(), value.to_string());
        }
        "popup-style" | "popup-border-style" | "popup-border-lines" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "window-style" | "window-active-style" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "wrap-search" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "lock-after-time" | "lock-command" => { app.user_options.insert(key.to_string(), value.to_string()); }
        "main-pane-width" => {
            if let Some(n) = crate::server::options::parse_main_pane_size(value) {
                app.main_pane_width = n;
            }
        }
        "main-pane-height" => {
            if let Some(n) = crate::server::options::parse_main_pane_size(value) {
                app.main_pane_height = n;
            }
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
                app.warm_pane.target = 0;
                app.warm_pane.kill_all();
            } else if app.warm_pane.target == 0 {
                app.warm_pane.target = crate::types::default_warm_pool_size().max(1);
            }
        }
        "warm-pool-size" => {
            crate::server::options::set_warm_pool_size(app, value);
        }
        "codepoint-widths" => {
            crate::server::options::set_codepoint_widths(app, value);
        }
        // `command-alias[N]`, tmux's own spelling, as well as the bare name.
        name if crate::command_alias::is_alias_option(name) => {
            let set = crate::command_alias::write(&app.command_aliases).set(name, value, false);
            if let Err(error) = set {
                warn_config(app, error);
                return false;
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
                    return true;
                }
            }
            // A catalog option this match does not route is still a real
            // option: hand it to the runtime setter so `set -g NAME VALUE` in
            // a config file lands exactly where the same command typed at
            // runtime does. Before this, such an option (alternate-screen,
            // message-limit, history-file-limit) warned "unknown option" and
            // was parked in user_options where nothing reads it, so
            // `set -g alternate-screen off` in psmux.conf left the pane
            // honouring the alternate screen. The value was already
            // validated against the catalog at the top of this function.
            if crate::server::option_catalog::is_known_option(key) {
                if let Err(error) =
                    crate::server::options::apply_set_option(app, key, value, false)
                {
                    warn_config(app, error);
                    return false;
                }
                return true;
            }
            // Store @-prefixed user/plugin options separately from environment
            // so they don't leak into child shells (#105).
            if key.starts_with('@') {
                app.user_options.insert(key.to_string(), value.to_string());
            } else if !crate::server::option_catalog::is_valid_option_name(key) {
                // Not an option tmux knows either: tmux's config load reports
                // "invalid option" and sets nothing (cmd-set-option.c:269).
                // It used to be stored anyway, a plain name in the
                // environment map where `show -gv` read it back and `set -gu`
                // could not clear it (#846).
                warn_config(app, format!("unknown option '{}'", key));
                return false;
            } else {
                // A name tmux's table knows that psmux has no setter for
                // (visual-silence, lock-command, status-keys). tmux accepts it
                // and so does psmux: it is kept as text in user_options so
                // `show` reads it back, and never in the environment, which
                // would hand it to every pane as a variable (#137, #846). A
                // genuine typo was refused above with "unknown option".
                app.user_options.insert(key.to_string(), value.to_string());
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
                if plugin_name == "ppm" {
                    // ppm runs the entry script of every declared plugin that
                    // has no plugin.conf (its Initialize-Plugin), as tpm runs
                    // each plugin in tmux. Those scripts are left to it from
                    // here on, and those queued already are dropped: psmux
                    // running them too ran each one twice.
                    app.ppm_declared = true;
                    app.pending_plugin_scripts.clear();
                } else {
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
                            // Read as source-file reads a file, on the source
                            // stack: a plugin.conf that declares its own
                            // plugin, or two that declare each other, used to
                            // recurse until the server died at startup. One
                            // already being read is skipped without a word:
                            // declaring a plugin twice is not a mistake.
                            let key = source_key(conf);
                            if !SOURCE_STACK.with(|s| s.borrow().contains(&key)) {
                                source_path(app, conf, false);
                            }
                            found = true;
                            break;
                        }
                    }
                    // If no plugin.conf, try .ps1 entry scripts, unless ppm
                    // runs them
                    if !found && !app.ppm_declared {
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
                                // A script that only sets options and binds
                                // keys with literal text is applied as config
                                // lines, here and now.
                                if let Ok(content) = std::fs::read_to_string(ps1) {
                                    let prev_file = current_config_file();
                                    set_current_config_file(ps1);
                                    let applied = parse_ps1_plugin_script(app, &content);
                                    set_current_config_file(&prev_file);
                                    // Any other script is queued whole, to run
                                    // once the server is listening.
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
    true
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
/// A top-level token that *ends* with an unescaped `;` ends the command too,
/// the `;` dropped and the rest of the token kept (#734 follow up): `-s probe;
/// display-message -p done` is `-s probe` then `display-message -p done`. Both
/// tmux grammars agree on that: cmd_parse_from_arguments (cmd-parse.y:1098)
/// cuts an argument ending in `;` and ends the command there, and the string
/// lexer ends a token at an unquoted `;` and returns the `;` as the separator
/// (cmd-parse.y:1314 and :1675). psmux made a session named `probe;` and sent
/// the marker to it as arguments. `\;` at the end of a token stays a literal
/// `;` in both (cmd-parse.y:1100 and the lexer's escape).
///
/// A `;` in the middle of a token (`a;b`) is left alone. The two tmux grammars
/// disagree there (an argv element `a;b` is one argument, a command string
/// `a;b` is two commands), and the line read here is often a flattened argv
/// whose arguments were not quoted, so splitting it would change the value of
/// a plain `set -g @v a;b`.
fn split_chained_commands(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    // A `;` inside a brace block belongs to the block (`if -F c { a; b }` is
    // one command, cmd-parse.y argument_statements). Unbalanced braces are
    // read as before, with no blocks.
    let depths = block_depth_map(&chars);
    let at_top = |i: usize| depths.as_ref().map_or(true, |d| d[i] == 0);
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
        if !in_single && !in_double && at_top(i) && (c == ';' || (c == '\\' && chars.get(i + 1) == Some(&';'))) {
            let sep_len = if c == ';' { 1 } else { 2 };
            let starts_token = i == 0 || chars[i - 1].is_whitespace();
            let ends_token = chars.get(i + sep_len).is_none_or(|n| n.is_whitespace());
            if starts_token && ends_token {
                push_seg(&mut commands, &chars[seg_start..i]);
                i += sep_len;
                seg_start = i;
                continue;
            }
            // `word;` : a bare `;` closing a token (not `\;`, handled by the
            // escape arm below, and not a `;` inside a token).
            if c == ';' && !starts_token && ends_token && !{
                let ts = chars[..i].iter().rposition(|ch| ch.is_whitespace()).map_or(0, |p| p + 1);
                is_modified_semicolon_key(&chars[ts..=i].iter().collect::<String>())
            } {
                push_seg(&mut commands, &chars[seg_start..i]);
                i += 1;
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

/// A key name for the `;` key with modifiers, such as `M-;`, `C-M-;` or `^;`.
/// tmux needs these written `M-\;` (its lexer and its argv rule both cut the
/// `;`), but psmux has always taken `bind -n M-; ...` as written, so a token
/// like this does not end a command at its trailing `;`. No real argument
/// ending a command looks like one.
pub(crate) fn is_modified_semicolon_key(token: &str) -> bool {
    if token == "^;" {
        return true;
    }
    let mut rest = token;
    let mut modifiers = 0;
    while rest.len() > 2 && rest.as_bytes()[1] == b'-'
        && matches!(rest.as_bytes()[0].to_ascii_uppercase(), b'C' | b'M' | b'S')
    {
        rest = &rest[2..];
        modifiers += 1;
    }
    modifiers > 0 && rest == ";"
}

/// One whitespace separated token of `s` starting at or after byte `from`,
/// quote aware: `(start, end, value)` where `value` has one level of `'...'`
/// or `"..."` removed (and `\x` unescaped inside double quotes). Used where a
/// flag VALUE may be quoted text, such as `bind-key -N "a note"`.
pub(crate) fn next_quoted_token(s: &str, from: usize) -> Option<(usize, usize, String)> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] as char).is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    let start = i;
    let mut value = String::new();
    let mut quote: Option<char> = None;
    let mut chars = s[start..].char_indices().peekable();
    let mut end = s.len();
    while let Some((off, c)) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some('"') if c == '\\' => {
                if let Some((_, n)) = chars.next() {
                    value.push(n);
                }
            }
            Some(_) => value.push(c),
            None if c == '\'' || c == '"' => quote = Some(c),
            None if c.is_whitespace() => {
                end = start + off;
                break;
            }
            None => value.push(c),
        }
    }
    Some((start, end, value))
}

pub fn parse_bind_key(app: &mut AppState, line: &str) {
    let mut _key_table = "prefix".to_string();
    let mut _repeatable = false;
    let mut note: Option<String> = None;

    // The command name, then bind-key's own flags (tmux "nrN:T:"), then the
    // key, then the command, which is kept as raw text from there on.
    // Flags and the key are whitespace tokens taken as written (a bare `"`
    // or `'` is a key name, not a quote); only a flag VALUE is quote aware,
    // since a note is free text.
    let raw_token = |from: usize| -> Option<(usize, &str)> {
        let rest = &line[from..];
        let skip = rest.len() - rest.trim_start().len();
        let rest = &rest[skip..];
        if rest.is_empty() {
            return None;
        }
        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        Some((from + skip + len, &rest[..len]))
    };
    let Some((mut pos, _)) = raw_token(0) else { return; };
    let mut key_str: Option<&str> = None;
    while let Some((end, tok)) = raw_token(pos) {
        pos = end;
        // A flag must start with '-' AND be longer than 1 char (e.g. "-r", "-n", "-T").
        // A bare "-" is a valid key name, not a flag.
        if tok.starts_with('-') && tok.len() > 1 && tok != "--" {
            let flags = &tok[1..];
            if flags.contains('r') { _repeatable = true; }
            if flags.contains('n') { _key_table = "root".to_string(); }
            // A value flag ends its cluster and takes the next argument
            // (getopt), so `-rN note` and `-T tbl` both work.
            if flags.ends_with('T') || flags.ends_with('N') {
                if let Some((_, vend, val)) = next_quoted_token(line, pos) {
                    pos = vend;
                    if flags.ends_with('T') { _key_table = val; } else { note = Some(val); }
                }
            }
            continue;
        }
        if tok == "--" {
            if let Some((kend, k)) = raw_token(pos) {
                pos = kend;
                key_str = Some(k);
            }
        } else {
            key_str = Some(tok);
        }
        break;
    }
    let Some(key_str) = key_str else { return; };
    // The command as written, whitespace runs folded the way the old
    // split/join did.
    let command = line[pos..].split_whitespace().collect::<Vec<_>>().join(" ");
    if command.is_empty() { return; }
    
    // The command list: `a \; b`, `'a; b'` and tmux's block form `{ a; b }`,
    // aliases expanded.
    let sub_commands: Vec<String> = bind_commands(&app.command_aliases, &command);

    // Issue #635: tmux parses the bound command list at BIND time
    // (cmd-bind-key.c -> cmd_parse_from_arguments), so a dangling
    // value-taking flag refuses the binding instead of arming a key that
    // silently acts on the default target when pressed.
    for sub in &sub_commands {
        let tokens = crate::commands::parse_command_line(sub);
        if let Err(flag_error) = crate::cli::validate_command_line_flags(&tokens) {
            warn_config(app, flag_error);
            return;
        }
    }


    if let Some(key) = parse_key_name(key_str) {
        let key = normalize_key_for_binding(key);
        let action = if sub_commands.len() > 1 {
            // Multiple chained commands
            Action::CommandChain(sub_commands)
        } else if let Some(a) = sub_commands.first().and_then(|c| parse_command_to_action(c)) {
            a
        } else {
            return;
        };
        let table = app.key_tables.entry(_key_table).or_default();
        table.retain(|b| b.key != key);
        table.push(Bind { key, action, repeat: _repeatable, note });
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
            unbind_all_in_table(app, &t);
        } else {
            // -a with no -T or -n is the prefix table only, as in tmux
            // (cmd-unbind-key.c: tablename defaults to "prefix"), and as the
            // #195 follow up (b2410550) said. Root, copy-mode and any other
            // table keep their bindings.
            unbind_all_in_table(app, "prefix");
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
            unbind_key_in_table(app, &target, key);
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
    // No prefix (`set prefix None`), nothing to forward.
    let Some(prefix) = app.prefix_key else { return };
    let key = normalize_key_for_binding(prefix);
    let table = app.key_tables.entry("prefix".to_string()).or_default();
    if table.iter().any(|b| b.key == key) {
        return;
    }
    if let Some(action) = parse_command_to_action("send-prefix") {
        table.push(Bind { key, action, repeat: false, note: None });
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
    // A mouse key keeps every modifier: S-WheelUpPane and C-M-WheelUpPane are
    // distinct tmux keys, and none of the keyboard folds below apply to them.
    if crate::mouse_keys::is_mouse_key(&key) {
        return key;
    }
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

    // tmux mouse keys (WheelUpPane, C-MouseDown1Status, ...): see mouse_keys.
    if let Some(key) = crate::mouse_keys::parse(name) {
        return Some(key);
    }

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
    // The files being sourced right now, outermost first, by source_key.
    // Without a guard a self-sourcing psmux.conf overflows the stack and
    // crashes the server, so the session never comes up ("no server running").
    // A file already on it is a cycle and an error (issue #842); its length is
    // the nesting depth.
    static SOURCE_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// tmux's CMD_SOURCE_FILE_DEPTH_LIMIT (cmd-source-file.c:33): deeper than this
/// is "too many nested files".
const MAX_SOURCE_DEPTH: usize = 50;

struct SourceStackGuard;
impl Drop for SourceStackGuard {
    fn drop(&mut self) {
        SOURCE_STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// The name a sourced file is known by on the source stack: its canonical
/// path, compared without regard to case or slash direction on Windows, as
/// same_config_path compares config paths.
pub(crate) fn source_key(path: &str) -> String {
    let p = std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches('\\').to_string();
    if cfg!(windows) { s.to_lowercase() } else { s }
}

/// A `source-file` path as source_file opens it: `~` expanded, slashes made
/// native, and a `~/.psmux/` path that does not exist moved to its XDG
/// equivalent when that one does (issue #135).
fn expand_source_path(path: &str) -> String {
    let expanded_path = if path.starts_with('~') {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    };

    // Normalize path separators for Windows
    let expanded_path = expanded_path.replace('/', &std::path::MAIN_SEPARATOR.to_string());

    // Fallback: if path references ~/.psmux/ but doesn't exist and the
    // XDG equivalent (~/.config/psmux/) does, use that instead (issue #135).
    if !std::path::Path::new(&expanded_path).exists() {
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
    }
}

/// The arguments of a `source-file`: tmux's flags (`Fnqv`, `-t target`,
/// cmd-source-file.c:42), alone or in a cluster such as `-qF`, then one or
/// more paths, each of which may be a glob (cmd-source-file.c:203 to :242).
#[derive(Debug, Default, PartialEq)]
pub(crate) struct SourceSpec {
    /// In the order given, which is the order the files are read in
    /// (cmd-source-file.c:126 to :128).
    pub paths: Vec<String>,
    /// `-F`: each path is a format.
    pub format: bool,
    /// `-q`: a file that does not exist is not an error (cmd-source-file.c:221
    /// to :232).
    pub quiet: bool,
    /// `-n`: parse only, nothing is applied.
    pub parse_only: bool,
}

impl SourceSpec {
    /// The arguments as the server's `CtrlReq::SourceFile` reads them back
    /// with parse_source_spec: the flags, `--`, then every path quoted, so a
    /// path with a space or a leading `-` stays one path.
    pub(crate) fn wire(&self) -> String {
        let mut out = String::new();
        for (on, flag) in [(self.quiet, "-q "), (self.format, "-F "), (self.parse_only, "-n ")] {
            if on {
                out.push_str(flag);
            }
        }
        out.push_str("--");
        for p in &self.paths {
            out.push(' ');
            out.push_str(&crate::util::quote_arg(p));
        }
        out
    }
}

/// A `source-file`'s arguments, already split into words (the dispatcher and
/// control mode split the line themselves).
pub(crate) fn source_spec_from_args<S: AsRef<str>>(args: &[S]) -> SourceSpec {
    let mut out = SourceSpec::default();
    let mut i = 0;
    while i < args.len() {
        let word = args[i].as_ref();
        if word == "--" {
            i += 1;
            break;
        }
        if !word.starts_with('-') || word.len() == 1 {
            break;
        }
        let flags = &word[1..];
        for (pos, c) in flags.char_indices() {
            match c {
                'F' => out.format = true,
                'q' => out.quiet = true,
                'n' => out.parse_only = true,
                // `-t target`: the value is the rest of this word or the next.
                't' => {
                    if pos + 1 == flags.len() {
                        i += 1;
                    }
                    break;
                }
                _ => {}
            }
        }
        i += 1;
    }
    out.paths = args.iter().skip(i).map(|a| a.as_ref().to_string()).collect();
    out
}

/// A `source-file`'s arguments as text, as a config line, a key binding and
/// the server's `CtrlReq::SourceFile` carry them. Split as every other
/// command line is (parse_command_line), so quotes group a path with spaces
/// and a Windows path keeps its backslashes.
///
/// One exception keeps configs that already work working. Before paths were
/// split, the rest of the line was the one path, so `source-file C:\Users\Ann
/// Lee\x.conf` read that file. When the rest of the line, taken whole, names a
/// file that exists, it is still that one path. That also covers an apostrophe
/// in an unquoted path (`C:\Users\O'Brien\x.conf`), which the split reads as
/// the start of a quote and drops.
pub(crate) fn parse_source_spec(spec: &str) -> SourceSpec {
    let words = crate::commands::parse_command_line(spec);
    let mut out = source_spec_from_args(&words);
    if !out.format && !out.paths.is_empty() {
        if let Some(rest) = whole_path_rest(spec) {
            if out.paths != [rest.as_str()]
                && std::path::Path::new(&expand_source_path(&rest)).is_file()
            {
                out.paths = vec![rest];
            }
        }
    }
    out
}

/// The text after the flags of a `source-file` spec, as one string, verbatim:
/// what the whole line named before paths were split.
fn whole_path_rest(spec: &str) -> Option<String> {
    let mut rest = spec.trim();
    let mut skip_value = false;
    loop {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..end];
        if word.is_empty() {
            return None;
        }
        if skip_value {
            skip_value = false;
        } else if word == "--" {
            rest = rest[end..].trim_start();
            break;
        } else if word.starts_with('-') && word.len() > 1 {
            let flags = &word[1..];
            if let Some(pos) = flags.find('t') {
                skip_value = pos + 1 == flags.len();
            }
        } else {
            break;
        }
        rest = rest[end..].trim_start();
    }
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

/// The lines of a config file that always run, in order: continuation lines
/// joined, a multi-line `bind x {` ... `}` made one statement and each command
/// of a line taken alone, as parse_config_content reads them, and every line
/// inside a `%if` block or an `if-shell { } { }` block left out, because
/// whether those run is decided only when the file runs (issue #842).
fn unconditional_lines(content: &str) -> Vec<String> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let lines = split_lines_commands(join_brace_blocks(join_continuations(content, 1)));
    let mut out = Vec::new();
    let mut if_depth = 0usize;
    let mut block_depth = 0usize;
    let mut else_may_follow = false;
    for (_, line) in lines {
        let l = strip_inline_comment(line.trim()).trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if block_depth > 0 {
            let opens = l.matches('{').count();
            let closes = l.matches('}').count();
            block_depth = (block_depth + opens).saturating_sub(closes);
            else_may_follow = block_depth == 0;
            continue;
        }
        if else_may_follow && l == "{" {
            block_depth = 1;
            else_may_follow = false;
            continue;
        }
        else_may_follow = false;
        if l.starts_with("%if ") || l.starts_with("%if\t") {
            if_depth += 1;
            continue;
        }
        if l == "%endif" {
            if_depth = if_depth.saturating_sub(1);
            continue;
        }
        if l.starts_with('%') || if_depth > 0 {
            continue;
        }
        if (l.starts_with("if-shell ") || l.starts_with("if ")) && l.ends_with('{') {
            block_depth = 1;
            continue;
        }
        out.push(l.to_string());
    }
    out
}

/// The error for `spec` when sourcing it would source a file that is already
/// being sourced, found before anything in the file has run (issue #842).
/// Only the `source-file` lines that always run are followed, in nested
/// files too: one in a key binding, a `%if` block or an `if-shell` block runs
/// only if and when its turn comes, and source_file checks it then. So does a
/// `-F` path or a glob. tmux has no such check: it runs a self-sourcing file
/// down to its nesting limit of 50 and then reports "too many nested files"
/// (cmd-source-file.c:172 to :186). A file that sources itself is never
/// wanted, so psmux refuses it before any of its lines take effect.
fn find_source_cycle(spec: &str) -> Option<String> {
    let mut keys = SOURCE_STACK.with(|s| s.borrow().clone());
    let mut names: Vec<String> = Vec::new();
    scan_spec_cycle(&parse_source_spec(spec), &mut keys, &mut names)
}

/// The cycle error for any of `spec`'s paths, each followed in turn.
fn scan_spec_cycle(spec: &SourceSpec, keys: &mut Vec<String>, names: &mut Vec<String>) -> Option<String> {
    if spec.format || spec.parse_only {
        return None;
    }
    spec.paths
        .iter()
        .filter(|p| !p.contains('*') && !p.contains('?'))
        .find_map(|p| scan_source_cycle(&expand_source_path(p), keys, names))
}

fn scan_source_cycle(path: &str, keys: &mut Vec<String>, names: &mut Vec<String>) -> Option<String> {
    let key = source_key(path);
    if keys.contains(&key) {
        names.push(path.to_string());
        return Some(format!(
            "source-file: {} is already being sourced ({})",
            path,
            names.join(" -> ")
        ));
    }
    if keys.len() >= MAX_SOURCE_DEPTH {
        return None;
    }
    // A file that cannot be read is reported when source_file opens it.
    let content = std::fs::read_to_string(path).ok()?;
    keys.push(key);
    names.push(path.to_string());
    for l in unconditional_lines(&content) {
        let (head, rest) = match l.find(char::is_whitespace) {
            Some(end) => (&l[..end], &l[end..]),
            None => (l.as_str(), ""),
        };
        if head != "source-file" && head != "source" {
            continue;
        }
        if let Some(msg) = scan_spec_cycle(&parse_source_spec(rest), keys, names) {
            return Some(msg);
        }
    }
    keys.pop();
    names.pop();
    None
}

/// Run a `source-file`'s arguments (parse_source_spec): every path in the
/// order given, and a glob's matches in their order, as tmux reads them
/// (cmd-source-file.c:203 to :242, :126 to :128). A path that cannot be read
/// is a warning, and the paths after it are still read.
pub fn source_file(app: &mut AppState, spec: &str) {
    let spec = parse_source_spec(spec);
    // `-n` parses without running anything; nothing here applies it yet, so
    // it does nothing, as the server's source-file has always done with it.
    if spec.parse_only {
        return;
    }

    // A runtime `source-file` (not part of a startup batch and not nested) owns
    // its own warning batch and surfaces the result as a status message, once
    // for all its paths. Under source_runtime the batch is that function's,
    // and the source job reports it once the file's commands have run.
    let outermost_runtime = SOURCE_STACK.with(|s| s.borrow().is_empty())
        && !in_startup_load()
        && app.source_defer.is_none();
    if outermost_runtime {
        begin_source_warnings(app);
    }
    let first_warning = app.config_warnings.len();

    for path in &spec.paths {
        // tmux expands the format first and then globs the result
        // (cmd-source-file.c:205 to :221).
        let path = if spec.format { crate::format::expand_format(path, app) } else { path.clone() };
        if path.contains('*') || path.contains('?') {
            source_glob(app, &path, spec.quiet);
        } else {
            source_path(app, &path, spec.quiet);
        }
    }

    if outermost_runtime {
        let own = app.config_warnings.get(first_warning..).map(<[String]>::to_vec).unwrap_or_default();
        summarize_warnings(app, &own);
    }
}

/// A source job has reported: `warnings` are all of its source's, the
/// parse's first, and the job's own start at `new_from`. The parse's are in
/// `config_warnings` already, so only the job's own are added, after those of
/// any source that finished meanwhile, and the status line shows this
/// source's. Replacing the list instead lost a source that finished while the
/// job ran.
pub(crate) fn finish_source_job_warnings(app: &mut AppState, warnings: &[String], new_from: usize) {
    app.source_jobs_running = app.source_jobs_running.saturating_sub(1);
    app.config_warnings.extend(warnings.iter().skip(new_from).cloned());
    summarize_warnings(app, warnings);
}

/// Start the warning list of a runtime source. It starts over only when no
/// source job is running: a job's warnings stay in the list until it has
/// reported them, so a source that runs meanwhile adds its own after them.
fn begin_source_warnings(app: &mut AppState) {
    if app.source_jobs_running == 0 {
        app.config_warnings.clear();
    }
}

/// Source every file a glob matches, each with its own name and line numbers
/// in warnings and in #{current_file}.
fn source_glob(app: &mut AppState, pattern: &str, quiet: bool) {
    let expanded = if pattern.starts_with('~') {
        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
        pattern.replacen('~', &home, 1)
    } else {
        pattern.to_string()
    };
    let mut matched = false;
    if let Ok(entries) = glob::glob(&expanded) {
        for entry in entries.flatten() {
            matched = true;
            source_path(app, &entry.display().to_string(), false);
        }
    }
    // A pattern that matches nothing is tmux's "No such file or
    // directory" unless -q is given (cmd-source-file.c:221 to :232).
    if !matched && !quiet {
        warn_config(app, format!("No such file or directory: {}", pattern));
    }
}

/// Source one file, named as written: `~` and the `~/.psmux/` fallback are
/// resolved here.
fn source_path(app: &mut AppState, path: &str, quiet: bool) {
    let depth = SOURCE_STACK.with(|s| s.borrow().len());
    let expanded_path = expand_source_path(path);

    // `-q`: a file that does not exist is skipped without a word, as tmux's
    // -q does (cmd-source-file.c:221 to :232).
    if quiet && !std::path::Path::new(&expanded_path).exists() {
        return;
    }

    // A file that is already being sourced, here or further out, is a cycle:
    // refused, whatever path or format named it (issue #842). Past tmux's
    // nesting limit is "too many nested files", as tmux reports it.
    let key = source_key(&expanded_path);
    if SOURCE_STACK.with(|s| s.borrow().contains(&key)) {
        warn_config(app, format!("source-file: {} is already being sourced", expanded_path));
        return;
    }
    if depth >= MAX_SOURCE_DEPTH {
        warn_config(app, "too many nested files");
        return;
    }
    SOURCE_STACK.with(|s| s.borrow_mut().push(key));
    let _stack_guard = SourceStackGuard; // pops on every return path

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
            // only when the path is not a glob. Globs are expanded before this
            // (source_glob), so every miss here is a real miss.
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
}

/// Show the warning batch of a runtime source in the status line.
pub(crate) fn summarize_config_warnings(app: &mut AppState) {
    let warnings = app.config_warnings.clone();
    summarize_warnings(app, &warnings);
}

/// The status line message for one source's `warnings`, which may be fewer
/// than `config_warnings` holds while another source job runs.
pub(crate) fn summarize_warnings(app: &mut AppState, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    let n = warnings.len();
    let summary = if n == 1 {
        format!("config warning: {}", warnings[0])
    } else {
        format!("{} config warnings (first: {})", n, warnings[0])
    };
    // Hold the message for 5s (vs the 750ms default) so a config problem is
    // actually noticed rather than flashing past.
    app.status_message = Some((summary, std::time::Instant::now(), Some(5000)));
}

/// A `source-file` run on a live server (issue #842). Options and bindings
/// before the file's first command apply now; from that command on, every line
/// is kept, in file order, and returned for the source job
/// (src/server/source_job.rs), which runs them one after another as tmux's
/// queue runs a sourced file (cmd-source-file.c:119, cfg.c:155 to :204).
///
/// Nothing is kept on a server with no window yet (a plugin sourced during the
/// startup drain) or on a warm standby, whose one window belongs to the next
/// session: there the file runs as it always has. When nothing is kept, the
/// warnings are reported now; otherwise the job reports them at its end.
///
/// A file that would source a file already being sourced is refused before
/// any of it takes effect (find_source_cycle): nothing is applied or kept,
/// and the error is the result.
pub fn source_runtime(app: &mut AppState, spec: &str) -> crate::types::SourceParse {
    if let Some(msg) = find_source_cycle(spec) {
        begin_source_warnings(app);
        app.config_warnings.push(msg.clone());
        app.config_warn_line = None;
        summarize_warnings(app, std::slice::from_ref(&msg));
        return crate::types::SourceParse { lines: Vec::new(), errors: vec![msg] };
    }
    // Every warning of a runtime source is an error for the client that ran
    // it, as tmux's client hears of a missing file, an invalid option or a bad
    // value and exits 1 (issue #842); the rest of the file still runs.
    if app.windows.is_empty() || app.is_warm_server() {
        source_file(app, spec);
        return crate::types::SourceParse::default();
    }
    begin_source_warnings(app);
    let first_warning = app.config_warnings.len();
    app.config_warn_line = None;
    let lines = parse_keeping_commands(app, |app| source_file(app, spec));
    // This source's warnings only: the list may also hold those of a source
    // job that is still running, which are that job's client's errors.
    let own = app.config_warnings.get(first_warning..).map(<[String]>::to_vec).unwrap_or_default();
    if lines.is_empty() {
        summarize_warnings(app, &own);
    }
    crate::types::SourceParse { lines, errors: own }
}

/// A `source-file` a plugin script runs while the server starts, before it
/// has a window (the plugin drain in run_server). The file's lines from its
/// first command on join the startup config's (`app.startup_source_lines`),
/// so the startup job runs them after the config's own, once the window
/// exists. tmux runs such a file's commands too: a plugin started by the
/// config's `run-shell -b` sources it once the session exists. A warm
/// standby keeps nothing, as for its config.
/// Returns the file's warnings, the errors of the client that sourced it.
pub fn source_during_startup(app: &mut AppState, spec: &str) -> Vec<String> {
    app.config_warn_line = None;
    if app.is_warm_server() {
        // As before: a plain source, which reports its own warnings.
        source_file(app, spec);
        return Vec::new();
    }
    let first_warning = app.config_warnings.len();
    let lines = parse_keeping_commands(app, |app| source_file(app, spec));
    app.startup_source_lines.extend(lines);
    app.config_warnings.get(first_warning..).map(<[String]>::to_vec).unwrap_or_default()
}

/// Run `parse` with lines being kept from the first command on, and return
/// the kept lines for a source job: a runtime `source-file` (source_runtime)
/// and the startup config (load_config) both parse this way.
fn parse_keeping_commands(
    app: &mut AppState,
    parse: impl FnOnce(&mut AppState),
) -> Vec<crate::types::DeferredConfigLine> {
    let outer = app.source_defer.replace(crate::types::SourceDefer::default());
    parse(app);
    std::mem::replace(&mut app.source_defer, outer).unwrap_or_default().lines
}

/// Parse one config line a source job kept, now that the commands before it
/// have run (issue #842). The line's own file and line number are restored for
/// its warnings and #{current_file}. Lines it keeps in turn (a nested
/// `source-file`'s commands, an `if-shell` branch that is a command) come back
/// with the warnings, for the job to run next. The server's own warning batch
/// is left as it was.
pub fn replay_deferred_line(
    app: &mut AppState,
    item: &crate::types::DeferredConfigLine,
) -> crate::types::SourceReplayResult {
    let saved_warnings = std::mem::take(&mut app.config_warnings);
    let saved_line = app.config_warn_line;
    let outer = app.source_defer.replace(crate::types::SourceDefer::default());
    let prev_file = current_config_file();
    set_current_config_file(&item.file);
    app.config_warn_line = item.line;
    // The files that were being sourced where the line was read, so a
    // `source-file` it runs is checked against them and nests below them.
    let saved_stack = SOURCE_STACK.with(|s| std::mem::replace(&mut *s.borrow_mut(), item.ancestors.clone()));
    match &item.kind {
        crate::types::DeferredKind::Block(text) => {
            parse_config_content_from(app, text, item.line.unwrap_or(1));
        }
        crate::types::DeferredKind::Directive(l) | crate::types::DeferredKind::Command(l) => {
            parse_config_line(app, l);
        }
    }
    SOURCE_STACK.with(|s| *s.borrow_mut() = saved_stack);
    set_current_config_file(&prev_file);
    app.config_warn_line = saved_line;
    let kept = std::mem::replace(&mut app.source_defer, outer).unwrap_or_default();
    let warnings = std::mem::replace(&mut app.config_warnings, saved_warnings);
    crate::types::SourceReplayResult { warnings, deferred: kept.lines }
}

/// Parse a key string like "C-a", "M-x", "F1", "Space" into (KeyCode, KeyModifiers)
pub fn parse_key_string(key: &str) -> Option<(KeyCode, KeyModifiers)> {
    let key = key.trim();
    if let Some(mouse) = crate::mouse_keys::parse(key) {
        return Some(mouse);
    }
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

/// Parse the value of a key option (`prefix`, `prefix2`).
///
/// tmux types both OPTIONS_TABLE_KEY. "None", in any case, is a real key there
/// (KEYC_NONE, key-string.c:254) that never matches typed input, so it turns
/// the prefix off; anything key_string_lookup_string cannot read is refused
/// with "bad key: X" (options.c:1305) and the option keeps its value. psmux
/// used to drop both without a word (#802).
///
/// The two psmux key parsers accept slightly different spellings, so each
/// route keeps the parser it always used first (the config file
/// parse_key_name, the command path parse_key_string) and only falls back to
/// the other one.
pub fn parse_option_key(value: &str, config_route: bool) -> Result<Option<(KeyCode, KeyModifiers)>, String> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let parsed = if config_route {
        parse_key_name(trimmed).or_else(|| parse_key_string(trimmed))
    } else {
        parse_key_string(trimmed).or_else(|| parse_key_name(trimmed))
    };
    parsed.map(Some).ok_or_else(|| format!("bad key: {}", value))
}

/// Print a key option the way tmux does: KEYC_NONE is "None" (key-string.c:355).
pub fn format_option_key(key: &Option<(KeyCode, KeyModifiers)>) -> String {
    key.as_ref().map(format_key_binding).unwrap_or_else(|| "None".to_string())
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
        KeyCode::Char(c) if crate::mouse_keys::is_mouse_char(*c) => {
            crate::mouse_keys::name(*c).unwrap_or_else(|| "?".to_string())
        }
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
    // `-b` changes nothing here: this route always starts the command without
    // waiting (see the spawn below). A runtime source and the startup config
    // hand a run-shell that waits to their job instead (run_shell_waits), and
    // one with `-b` too once the file's first command has been kept
    // (run_shell_in_job).
    let parsed = match crate::commands::parse_run_shell_args(&args[1..]) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn_config(app, e);
            return;
        }
    };
    let delay = parsed.delay;
    let shell_cmd = parsed.command.join(" ");
    if shell_cmd.is_empty() { return; }

    // Format expand the command first, like tmux (cmd-run-shell.c:144) and
    // like the CLI and key binding paths already do. Untimed: `%` in a shell
    // command is not strftime.
    let shell_cmd = if shell_cmd.contains('#') {
        crate::format::expand_format_untimed(&shell_cmd, app)
    } else {
        shell_cmd
    };

    // Expand ~ to home directory + XDG fallback for plugin paths
    let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd);

    // A tmux plugin's `.tmux` entry script is a bash script. With Git Bash
    // it runs as any run-shell command does (build_run_shell_command); without
    // it, its plain tmux lines are applied as config lines.
    // The file check first: finding Git Bash scans PATH, which only a
    // `.tmux` script needs.
    if let Some(path) = tmux_entry_script(&shell_cmd) {
        if !crate::commands::can_run_tmux_scripts() {
            parse_tmux_entry_script(app, &path);
            return;
        }
    }
    // A warm standby's own config load starts no shell. Its claim loads the
    // config again and runs the shell then, for the session that exists by
    // that time; starting it here as well ran every config run-shell twice,
    // where tmux runs a config once. A `.tmux` script applied above is read,
    // as the config's own options are.
    if in_startup_load() && app.is_warm_server() {
        return;
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
    crate::commands::apply_run_shell_cwd(&mut cmd, parsed.cwd.as_deref());
    crate::commands::null_job_output(&mut cmd);
    if !target_session.is_empty() {
        cmd.env("PSMUX_TARGET_SESSION", &target_session);
    }
    match delay {
        // `-d`: started after the delay, on a thread so nothing here waits.
        Some(delay) => {
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let _ = cmd.spawn();
            });
        }
        None => {
            let _ = cmd.spawn();
        }
    }
}

/// How a run-shell line in a runtime source or the startup config runs: None
/// when parse_run_shell runs it, Some(waits) when it is a command of the
/// source job, which runs it through the connection dispatcher the way the
/// CLI's run-shell runs.
///
/// A plugin's `.tmux` entry script stays with parse_run_shell when there is
/// no Git Bash to run it, as it then applies the script's lines itself, and
/// so does a line whose flags do not parse, which it reports.
fn run_shell_job_kind(l: &str) -> Option<bool> {
    if !(l.starts_with("run-shell ") || l.starts_with("run ")) {
        return None;
    }
    let args = crate::commands::parse_command_line(l);
    let parsed = crate::commands::parse_run_shell_args(&args[1..]).ok()?;
    let command = parsed.command.join(" ");
    if command.trim().is_empty() && parsed.delay.is_none() {
        return None;
    }
    if tmux_entry_script(&crate::util::expand_run_shell_path(&command)).is_some()
        && !crate::commands::can_run_tmux_scripts()
    {
        return None;
    }
    Some(!parsed.background)
}

/// The `.tmux` entry script a run-shell command is, when the whole command
/// is that one file (quoted or not), as tpm runs a plugin's script. A
/// command that only names a `.tmux` file among other words is a command.
fn tmux_entry_script(shell_cmd: &str) -> Option<std::path::PathBuf> {
    let t = shell_cmd.trim();
    let t = ['\'', '"']
        .iter()
        .find_map(|&q| t.strip_prefix(q).and_then(|s| s.strip_suffix(q)))
        .unwrap_or(t);
    let path = std::path::Path::new(t);
    (t.ends_with(".tmux") && path.is_file()).then(|| path.to_path_buf())
}

/// Whether a run-shell line is a command its source job runs and waits for,
/// as tmux's queue waits for a run-shell without `-b` (cmd-run-shell.c:123
/// and :186 to :188), the lines after it included. Such a line starts the
/// kept part of the file.
///
/// The job's one-shot connection has a thread of its own that waits for the
/// shell while the server loop goes on, so a script that calls back into
/// psmux is answered. parse_run_shell cannot wait on the server thread for
/// exactly that reason.
fn run_shell_waits(l: &str) -> bool {
    run_shell_job_kind(l) == Some(true)
}

/// Whether a kept run-shell line goes to the job as a command. With `-b` too:
/// it starts in file order, as tmux's queue starts it, and nothing waits. A
/// `-b` line before the first command is not kept: it starts in place, after
/// every line before it, which is that order already.
fn run_shell_in_job(l: &str) -> bool {
    run_shell_job_kind(l).is_some()
}

/// Apply a plugin's `.ps1` entry script as config lines when all it does is
/// set options and bind keys with literal text:
///
///   $PSMUX = (Get-Command psmux).Source
///   & $PSMUX set -g key value 2>&1 | Out-Null
///   & $PSMUX bind-key ...
///
/// Every line is checked before any is applied. One line the parser cannot
/// read as the command PowerShell would run (a variable, a pipeline, an `if`,
/// a command other than set and bind) leaves the whole script to run, so no
/// line is applied here and then again by the script, and none is lost.
/// Returns whether the script was applied.
fn parse_ps1_plugin_script(app: &mut AppState, content: &str) -> bool {
    match literal_ps1_plugin_lines(content) {
        Some(lines) => {
            for line in &lines {
                parse_config_line(app, line);
            }
            true
        }
        None => false,
    }
}

/// The config lines a `.ps1` plugin script amounts to, or `None` when the
/// script has to run (see parse_ps1_plugin_script).
fn literal_ps1_plugin_lines(content: &str) -> Option<Vec<String>> {
    let mut lines = Vec::new();
    for line in content.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        // `$PSMUX = ...` finds the binary the calls below use.
        if let Some(rest) = l.strip_prefix("$PSMUX") {
            if rest.trim_start().starts_with('=') && !rest.contains(';') {
                continue;
            }
            return None;
        }
        let call = l.strip_prefix('&')?.trim_start().strip_prefix("$PSMUX")?;
        if !call.starts_with(char::is_whitespace) {
            return None;
        }
        // The output is thrown away: `2>&1 | Out-Null`, `2>$null`.
        let mut cmd = call.trim();
        loop {
            let before = cmd;
            for noise in ["| Out-Null", "2>&1", "2>$null", "*>$null", ">$null"] {
                if let Some(rest) = cmd.strip_suffix(noise) {
                    cmd = rest.trim_end();
                }
            }
            if cmd == before {
                break;
            }
        }
        let name = cmd.split_whitespace().next()?;
        if !matches!(name, "set" | "set-option" | "setw" | "set-window-option" | "bind" | "bind-key") {
            return None;
        }
        if !ps1_words_are_literal(cmd) {
            return None;
        }
        lines.push(cmd.to_string());
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
}

/// Whether PowerShell hands the words of `cmd` to psmux as the config parser
/// reads them. Quoted text holds no variable, escape or doubled quote. Outside
/// quotes there is no operator, no comma (an array), no backslash (an escape
/// in a config line, not in PowerShell), and no word that starts with `@` (a
/// splat) or `#` (a comment).
fn ps1_words_are_literal(cmd: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut word_start = true;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => {
                if chars.peek() == Some(&q) {
                    return false;
                }
                quote = None;
            }
            Some('"') if matches!(c, '$' | '`' | '\\') => return false,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '$' | '`' | '\\' | '|' | ';' | '&' | '<' | '>' | '(' | ')' | '{' | '}' | ',' => return false,
                '@' | '#' if word_start => return false,
                _ => {}
            },
        }
        word_start = quote.is_none() && c.is_whitespace();
    }
    quote.is_none()
}

/// Apply a plugin's `.tmux` entry script (bash) when there is no Git Bash to
/// run it (parse_run_shell).
///
/// Only what bash would surely do the same way is applied: a `tmux` call at
/// the top level of the script whose words are literal once the script's
/// simple variables are put in (its directory, a literal value). A call
/// inside an `if`, a loop or a function, or with any other `$`, is not: run
/// without its condition or its value it did the wrong thing, and
/// tmux-sensible's macOS-only `default-command` stopped every pane from
/// starting. When the script does anything else, one warning says that it
/// needs Git Bash.
///
/// Companion `*_tmux.conf` files in the script's directory are sourced too.
fn parse_tmux_entry_script(app: &mut AppState, path: &std::path::Path) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let dir = path.parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (lines, needs_bash) = tmux_entry_script_lines(&content, &dir);
    for l in &lines {
        parse_config_line(app, l);
    }
    if needs_bash {
        warn_config(app, format!(
            "{}: needs Git Bash (Git for Windows) to run; only its plain tmux lines were applied",
            path.display()
        ));
    }

    // Many themes ship their options as `*_tmux.conf` files beside the
    // script; source those as well.
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if ext == "conf" {
                        let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if fname.ends_with("_tmux.conf") || fname.ends_with("_options_tmux.conf") {
                            source_path(app, &p.to_string_lossy(), false);
                        }
                    }
                }
            }
        }
    }
}

/// The config lines a `.tmux` entry script's plain tmux calls make (see
/// parse_tmux_entry_script), and whether the script does anything more that
/// only bash can do. `dir` is the script's directory, what
/// `"$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"` gives.
fn tmux_entry_script_lines(content: &str, dir: &str) -> (Vec<String>, bool) {
    const OPENERS: [&str; 6] = ["if", "for", "while", "until", "case", "select"];
    const CLOSERS: [&str; 4] = ["fi", "done", "esac", "}"];
    const DECLARE: [&str; 5] = ["export", "local", "readonly", "declare", "typeset"];
    let dir = dir.replace('\\', "/");
    // The script's variables whose value is known: name and value.
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    let mut needs_bash = false;
    let mut depth = 0usize;
    let mut pending = String::new();
    for raw in content.lines() {
        // A line ending in `\` goes on in the next one.
        if let Some(head) = raw.strip_suffix('\\') {
            pending.push_str(head);
            continue;
        }
        pending.push_str(raw);
        let line = std::mem::take(&mut pending);
        let split = split_shell_line(&line);
        if split.commands.is_empty() {
            continue;
        }
        let words: Vec<Vec<&str>> = split.commands.iter()
            .map(|c| c.iter().map(|r| &line[r.clone()]).collect())
            .collect();
        let at_top = depth == 0;
        for w in &words {
            if OPENERS.contains(&w[0]) {
                depth += 1;
            } else if CLOSERS.contains(&w[0]) {
                depth = depth.saturating_sub(1);
            }
            if w.len() > 1 && w.last() == Some(&"{") {
                depth += 1;
            }
        }
        // The body of a block was judged with its first line.
        if !at_top {
            continue;
        }
        if let [w] = words.as_slice() {
            // A function's definition does nothing until it is called.
            if w.last() == Some(&"{")
                && (w[0].ends_with("()") || w[0] == "function" || w.get(1) == Some(&"()"))
            {
                continue;
            }
            // `NAME=value ...`, or `export NAME`, which keeps the value.
            let declare = DECLARE.contains(&w[0]);
            let assigns = if declare { &w[1..] } else { &w[..] };
            if !assigns.is_empty()
                && assigns.iter().all(|a| shell_assignment(a).is_some() || (declare && is_shell_name(a)))
            {
                for (name, value) in assigns.iter().filter_map(|a| shell_assignment(a)) {
                    vars.retain(|(k, _)| k != name);
                    if value.contains("dirname") && (value.contains("BASH_SOURCE") || value.contains("$0")) {
                        vars.push((name.to_string(), dir.clone()));
                    } else if let Some(v) = literal_shell_word(value) {
                        vars.push((name.to_string(), v));
                    }
                }
                continue;
            }
            if split.plain && w.len() > 1
                && matches!(w[0], "tmux" | "$TMUX_PROGRAM" | "\"$TMUX_PROGRAM\"")
            {
                let cmd = &split.commands[0];
                let args = expand_shell_vars(&line[cmd[1].start..cmd[cmd.len() - 1].end], &vars);
                if !args.contains('$') && !args.contains('`') {
                    out.push(args);
                    continue;
                }
            }
        }
        needs_bash = true;
    }
    (out, needs_bash)
}

/// One line of a `.tmux` script cut the way bash cuts it, as far as
/// tmux_entry_script_lines needs: its commands, split at `;`, `&&`, `||`
/// and `|` outside quotes, as byte ranges of their words. Quotes, `$( )`
/// and `${ }` stay inside a word, and a `#` that starts a word ends the
/// line. `plain` is false when bash would do more with the line than run
/// its words as they read: a pipe, a redirection, `&`, a subshell, a
/// backquote, or a `\` escape other than `\;`.
struct ShellLine {
    commands: Vec<Vec<std::ops::Range<usize>>>,
    plain: bool,
}

fn split_shell_line(l: &str) -> ShellLine {
    let b = l.as_bytes();
    let mut commands: Vec<Vec<std::ops::Range<usize>>> = vec![Vec::new()];
    let mut plain = true;
    let mut start: Option<usize> = None;
    let mut quote: Option<u8> = None;
    // Inside `$( )` or `${ }`: the closing byte and how deep.
    let mut nest: Option<(u8, u8, usize)> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if let Some((open, close, n)) = nest {
            if c == open {
                nest = Some((open, close, n + 1));
            } else if c == close {
                nest = if n == 1 { None } else { Some((open, close, n - 1)) };
            }
            i += 1;
            continue;
        }
        if quote == Some(b'\'') {
            if c == b'\'' {
                quote = None;
            }
            i += 1;
            continue;
        }
        if c == b'$' && matches!(b.get(i + 1), Some(b'(') | Some(b'{')) {
            let open = b[i + 1];
            nest = Some((open, if open == b'(' { b')' } else { b'}' }, 1));
            start.get_or_insert(i);
            i += 2;
            continue;
        }
        if c == b'`' {
            plain = false;
        }
        if quote == Some(b'"') {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                quote = None;
            }
            i += 1;
            continue;
        }
        let end_word = |start: &mut Option<usize>, commands: &mut Vec<Vec<std::ops::Range<usize>>>| {
            if let Some(s) = start.take() {
                commands.last_mut().unwrap().push(s..i);
            }
        };
        match c {
            b' ' | b'\t' => end_word(&mut start, &mut commands),
            b';' | b'&' | b'|' => {
                end_word(&mut start, &mut commands);
                if c != b';' && b.get(i + 1) == Some(&c) {
                    i += 1;
                } else if c != b';' {
                    plain = false;
                }
                commands.push(Vec::new());
            }
            b'#' if start.is_none() => break,
            _ => {
                match c {
                    b'\'' | b'"' => quote = Some(c),
                    b'\\' => {
                        if b.get(i + 1) != Some(&b';') {
                            plain = false;
                        }
                        start.get_or_insert(i);
                        i += 2;
                        continue;
                    }
                    b'<' | b'>' | b'(' | b')' => plain = false,
                    _ => {}
                }
                start.get_or_insert(i);
            }
        }
        i += 1;
    }
    if let Some(s) = start {
        commands.last_mut().unwrap().push(s..b.len());
    }
    // A quote or a `$( )` left open at the end makes the line bash's to read.
    if quote.is_some() || nest.is_some() {
        plain = false;
    }
    commands.retain(|c| !c.is_empty());
    ShellLine { commands, plain }
}

/// Whether `s` is a shell variable name.
fn is_shell_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `NAME=value` as its name and value, when `word` is a shell assignment.
fn shell_assignment(word: &str) -> Option<(&str, &str)> {
    let (name, value) = word.split_once('=')?;
    is_shell_name(name).then_some((name, value))
}

/// The value of a shell word that bash reads as it is written: bare text
/// or one quoted string, with no `$`, backquote or `\`.
fn literal_shell_word(word: &str) -> Option<String> {
    let inner = ['\'', '"']
        .iter()
        .find_map(|&q| word.strip_prefix(q).and_then(|s| s.strip_suffix(q)))
        .unwrap_or(word);
    (!inner.contains(['$', '`', '\\', '\'', '"'])).then(|| inner.to_string())
}

/// `text` with `$NAME` and `${NAME}` replaced for each of `vars`. Any other
/// `$` is left for the caller to see.
fn expand_shell_vars(text: &str, vars: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, used) = match after.strip_prefix('{') {
            Some(b) => match b.find('}') {
                Some(e) => (&b[..e], e + 2),
                None => ("", 0),
            },
            None => {
                let e = after
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(after.len());
                (&after[..e], e)
            }
        };
        match vars.iter().find(|(k, _)| k == name).filter(|_| !name.is_empty()) {
            Some((_, v)) => {
                out.push_str(v);
                rest = &after[used..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Execute an if-shell / if command from config.
/// The text of a double quoted if-shell branch with `\\` read as `\` and
/// `\"` as `"`, as tmux's lexer reads them inside double quotes
/// (cmd-parse.y yylex_token_escape). Every other backslash is kept as it is,
/// so a `\;` written in the branch still holds a binding's list of commands,
/// as psmux has always read it, and a path such as `C:\Users` is left alone.
pub(crate) fn unescape_double_quoted_branch(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if n == '\\' || n == '"' {
                    out.push(n);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Whether `part`, a word inside an if-shell argument quoted with `quote`,
/// ends that argument: it ends with the quote, and in double quotes that quote
/// is not escaped (`\"`, after an even run of backslashes it is a real quote).
fn closes_quote(part: &str, quote: char) -> bool {
    let Some(body) = part.strip_suffix(quote) else { return false };
    if quote != '"' {
        return true;
    }
    body.chars().rev().take_while(|&c| c == '\\').count() % 2 == 0
}

/// Syntax: if-shell [-bF] <condition> <true-cmd> [<false-cmd>]
/// Runs the condition command (or evaluates format with -F), then executes the
/// appropriate branch command as a config line.
fn parse_if_shell(app: &mut AppState, line: &str) {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 { return; }

    let mut format_mode = false;
    let mut _background = false;
    let mut positional: Vec<String> = Vec::new();
    // Which positionals were written in double quotes, where tmux's lexer
    // reads `\\` and `\"` as escapes (cmd-parse.y yylex_token_escape).
    let mut double_quoted: Vec<bool> = Vec::new();
    let mut i = 1;
    while i < parts.len() {
        match parts[i] {
            "-b" => { _background = true; }
            "-F" => { format_mode = true; }
            "-bF" | "-Fb" => { _background = true; format_mode = true; }
            "-t" => { i += 1; } // skip target
            s => {
                double_quoted.push(s.starts_with('"'));
                // Handle quoted strings that might span multiple parts
                if s.starts_with('"') || s.starts_with('\'') {
                    let quote = s.chars().next().unwrap();
                    if closes_quote(&s[1..], quote) {
                        positional.push(s[1..s.len()-1].to_string());
                    } else {
                        let mut buf = s[1..].to_string();
                        i += 1;
                        while i < parts.len() {
                            buf.push(' ');
                            buf.push_str(parts[i]);
                            if closes_quote(parts[i], quote) {
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
    // A shell condition is format expanded before it runs, like tmux
    // (cmd-if-shell.c:86 format_single_from_target). It went to the shell
    // verbatim, so `if-shell 'check #{pid}'` in a config file saw the literal
    // `#{pid}` (issue #734 follow up). Untimed: `%` in a shell command is not
    // strftime.
    let shell_condition = if format_mode {
        positional[0].clone()
    } else {
        crate::format::expand_format_untimed(&positional[0], app)
    };
    let condition = &shell_condition;
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
        // Never the server's own stdio: dead after a C-c relay (#819).
        c.stdin(std::process::Stdio::null());
        crate::commands::null_job_output(&mut c);
        crate::commands::apply_job_env(&mut c);
        { use crate::platform::HideWindowCommandExt; c.hide_window(); }
        c.status().map(|s| s.success()).unwrap_or(false)
    };

    let cmd_to_run = if success { Some((true_cmd, 1)) } else { false_cmd.map(|c| (c, 2)) };
    if let Some((cmd, idx)) = cmd_to_run {
        // A branch in double quotes holds its inner quotes and backslashes
        // escaped: tmux reads `"set -g @a \"x;y\""` as `set -g @a "x;y"` and
        // `"bind x a \\; b"` as `bind x a \; b`. Undo those two escapes so the
        // split below sees the quotes and the `\;` the branch really has.
        let unescaped;
        let cmd: &str = if double_quoted.get(idx) == Some(&true) {
            unescaped = unescape_double_quoted_branch(cmd);
            &unescaped
        } else {
            cmd
        };
        // Execute the branch as a config line (recursive — supports set, bind, source, etc.)
        // tmux parses the branch as a command string, so each command on it
        // runs (`if-shell true "set -g @a on ; set -g @b on"` sets both).
        for c in split_line_commands(cmd) {
            parse_config_line(app, &c);
        }
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
#[path = "../tests-rs/test_issue842_source_file_commands.rs"]
mod test_issue842_source_file_commands;

#[cfg(test)]
#[path = "../tests-rs/test_issue886_startup_config_run.rs"]
mod test_issue886_startup_config_run;

#[cfg(test)]
#[path = "../tests-rs/test_issue145_source_file.rs"]
mod tests_issue145_source_file;

#[cfg(test)]
#[path = "../tests-rs/test_issue193_scroll_enter_copy_mode.rs"]
mod tests_issue193_scroll_enter_copy_mode;

#[cfg(test)]
#[path = "../tests-rs/test_mouse_drag_enter_copy_mode.rs"]
mod test_mouse_drag_enter_copy_mode;

#[cfg(test)]
#[path = "../tests-rs/test_ctrl_wheel_zoom_option.rs"]
mod test_ctrl_wheel_zoom_option;

#[cfg(test)]
#[path = "../tests-rs/test_bind_brace_blocks.rs"]
mod test_bind_brace_blocks;

#[cfg(test)]
#[path = "../tests-rs/test_source_file_key_tables.rs"]
mod test_source_file_key_tables;

#[cfg(test)]
#[path = "../tests-rs/test_issue847_source_file_paths.rs"]
mod test_issue847_source_file_paths;

#[cfg(test)]
#[path = "../tests-rs/test_issue851_run_shell_wait.rs"]
mod test_issue851_run_shell_wait;

#[cfg(test)]
#[path = "../tests-rs/test_issue849_config_line_semicolon.rs"]
mod test_issue849_config_line_semicolon;

#[cfg(test)]
#[path = "../tests-rs/test_issue853_source_warnings_overlap.rs"]
mod test_issue853_source_warnings_overlap;

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
#[path = "../tests-rs/test_issue706_copy_mode_line_numbers.rs"]
mod tests_issue706_copy_mode_line_numbers;

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
#[path = "../tests-rs/test_issue802_prefix_none.rs"]
mod tests_issue802_prefix_none;

#[cfg(test)]
#[path = "../tests-rs/test_issue536_config_quoted_whitespace.rs"]
mod tests_issue536_config_quoted_whitespace;

#[cfg(test)]
#[path = "../tests-rs/test_issue606_repeat_time.rs"]
mod tests_issue606_repeat_time;

#[cfg(test)]
#[path = "../tests-rs/test_issue616_unicode_bind.rs"]
mod tests_issue616_unicode_bind;

#[cfg(test)]
#[path = "../tests-rs/test_issue619_set_option_unset_only.rs"]
mod tests_issue619_set_option_unset_only;

#[cfg(test)]
#[path = "../tests-rs/test_issue619_set_option_already_set.rs"]
mod tests_issue619_set_option_already_set;

#[cfg(test)]
#[path = "../tests-rs/test_config_option_parity.rs"]
mod tests_config_option_parity;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_ifshell_format.rs"]
mod tests_issue734_ifshell_format;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_side_ifshell_braces.rs"]
mod tests_issue734_side_ifshell_braces;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_side_bind_notes.rs"]
mod tests_issue734_side_bind_notes;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_followup_queue.rs"]
mod tests_issue734_followup_queue;

#[cfg(test)]
#[path = "../tests-rs/test_issue823_quote_escapes.rs"]
mod test_issue823_quote_escapes;
