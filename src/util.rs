use std::io;

use serde::{Serialize, Deserialize};

use crate::types::{AppState, Node};

/// Shared lock for tests that mutate the process-global USERPROFILE/HOME env.
/// `std::env::set_var`/`remove_var` are process-wide, so every test module that
/// touches them (e.g. test_config_plugin_paths and test_issue167_startup_log)
/// must serialise through THIS single lock rather than a per-module mutex.
/// With separate mutexes, a reader in one module can observe a half-swapped env
/// set by another module and panic (flaky server-startup.log tests under the
/// full parallel suite). `lock_test_env` recovers a poisoned lock so a panicking
/// test cannot cascade failures into every later env-touching test.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn lock_test_env() -> std::sync::MutexGuard<'static, ()> {
    TEST_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Resolve a `-c` start-dir to one a freshly spawned pane shell can actually
/// enter, falling back to the user's home directory when it cannot.
///
/// `portable_pty`'s `cwd()` maps to the child's initial working directory, and
/// on Windows a directory the child cannot enter makes the spawn itself fail —
/// before the shell runs, so no amount of profile-level healing helps. The pane
/// dies on creation and psmux tears it straight back down. That is the
/// "prefix + | splits, opens, then immediately aborts" report.
///
/// Two cases produce an unusable start-dir in practice:
///   - the directory no longer exists (it was deleted, or renamed, since the
///     pane that is being split last `cd`'d into it), and
///   - a UNC path. `\\wsl.localhost\...` after a `cd` into WSL is the common
///     one. These are rejected even when currently reachable: whether they work
///     depends on the WSL VM being up and the share being mounted, so a split
///     that succeeds now and dies in ten minutes is worse than one that
///     predictably lands in the home directory.
///
/// Returning `None` means "do not set a cwd at all" and lets the child inherit
/// the server's — the last resort for the case where even home is unusable.
pub fn usable_start_dir(dir: &str) -> Option<std::path::PathBuf> {
    // UNC rejection is a Windows concept (the `\\wsl.localhost\...` case). On
    // Unix a leading `//` is a legitimate absolute path, so treating it as UNC
    // would wrongly send existing directories to the home fallback.
    #[cfg(windows)]
    fn is_unc(p: &str) -> bool {
        p.starts_with("\\\\") || p.starts_with("//")
    }
    #[cfg(not(windows))]
    fn is_unc(_p: &str) -> bool {
        false
    }

    if !dir.is_empty() && !is_unc(dir) && std::path::Path::new(dir).is_dir() {
        return Some(std::path::PathBuf::from(dir));
    }

    let home = crate::paths::home_dir();
    if !home.is_empty() && std::path::Path::new(&home).is_dir() {
        return Some(std::path::PathBuf::from(home));
    }

    None
}

/// Expand `~` to the user's home directory in a shell command string,
/// then rewrite `~/.psmux/plugins/` to `~/.config/psmux/plugins/` when
/// the classic path does not exist but the XDG path does (issue psmux-plugins#2).
pub fn expand_run_shell_path(cmd: &str) -> String {
    // Step 1: expand ~ to home directory
    let cmd = if cmd.contains('~') {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        cmd.replace("~/", &format!("{}/", home))
           .replace("~\\", &format!("{}\\", home))
    } else {
        cmd.to_string()
    };

    // Step 2: XDG fallback for plugin paths
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let classic_fwd = format!("{}/.psmux/plugins/", home);
    let classic_win = format!("{}\\.psmux\\plugins\\", home);
    if cmd.contains(&classic_fwd) || cmd.contains(&classic_win) {
        let classic_dir = std::path::Path::new(&home).join(".psmux").join("plugins");
        let xdg_base = std::env::var("XDG_CONFIG_HOME")
            .unwrap_or_else(|_| format!("{}\\.config", home));
        let xdg_dir = std::path::Path::new(&xdg_base).join("psmux").join("plugins");
        if !classic_dir.is_dir() && xdg_dir.is_dir() {
            let xdg_fwd = format!("{}/psmux/plugins/", xdg_base.replace('\\', "/"));
            let xdg_win = format!("{}\\psmux\\plugins\\", xdg_base);
            cmd.replace(&classic_fwd, &xdg_fwd).replace(&classic_win, &xdg_win)
        } else {
            cmd
        }
    } else {
        cmd
    }
}

pub fn infer_title_from_prompt(screen: &vt100::Screen, rows: u16, cols: u16) -> Option<String> {
    // Scan from cursor row (most likely prompt location) then fall back to last non-empty row
    let cursor_row = screen.cursor_position().0;
    let mut candidate_row: Option<u16> = None;
    // Try cursor row first, then scan downward, then scan upward
    for &r in [cursor_row].iter().chain((cursor_row + 1..rows).collect::<Vec<_>>().iter()).chain((0..cursor_row).rev().collect::<Vec<_>>().iter()) {
        let mut s = String::new();
        for c in 0..cols { if let Some(cell) = screen.cell(r, c) { s.push_str(cell.contents()); } else { s.push(' '); } }
        let t = s.trim_end();
        if !t.is_empty() && (t.contains('>') || t.contains('$') || t.contains('#') || t.contains(':')) {
            candidate_row = Some(r);
            break;
        }
    }
    // Fall back: use the row the cursor is on even if no prompt marker
    let row = candidate_row.unwrap_or(cursor_row);
    let mut s = String::new();
    for c in 0..cols { if let Some(cell) = screen.cell(row, c) { s.push_str(cell.contents()); } else { s.push(' '); } }
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() { return None; }
    // Only infer title from lines that look like prompts (contain a prompt marker)
    let has_prompt_marker = trimmed.contains('>') || trimmed.ends_with('$') || trimmed.ends_with('#');
    if !has_prompt_marker {
        // If no prompt marker, don't change the title — this is likely command output
        return None;
    }
    if let Some(pos) = trimmed.rfind('>') {
        let before = trimmed[..pos].trim().to_string();
        if before.contains("\\") || before.contains("/") {
            let parts: Vec<&str> = before.trim_matches(|ch: char| ch == '"').split(['\\','/']).collect();
            if let Some(base) = parts.last() { return Some(base.to_string()); }
        }
        return Some(before);
    }
    if let Some(pos) = trimmed.rfind('$') { return Some(trimmed[..pos].trim().to_string()); }
    if let Some(pos) = trimmed.rfind('#') { return Some(trimmed[..pos].trim().to_string()); }
    Some(trimmed)
}

// resolve_last_session_name and resolve_default_session_name are in session.rs

#[derive(Serialize, Deserialize)]
pub struct WinInfo { pub id: usize, pub name: String, pub active: bool, #[serde(default)] pub activity: bool, #[serde(default)] pub bell: bool, #[serde(default)] pub last: bool, #[serde(default)] pub tab_text: String, #[serde(default)] pub idx: usize }

#[derive(Serialize, Deserialize)]
pub struct PaneInfo { pub id: usize, pub title: String }

#[derive(Serialize, Deserialize)]
pub struct WinTree { pub id: usize, pub name: String, pub active: bool, pub panes: Vec<PaneInfo>, #[serde(default)] pub idx: usize }

/// Lightweight layout description for cross-session preview rendering
/// (issue #257). Mirrors the structural part of `LayoutJson` without any
/// pane content. Uses the same `type` discriminant so it deserializes
/// alongside the heavier dump-state layout.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum LayoutSimple {
    #[serde(rename = "split")]
    Split { kind: String, sizes: Vec<u16>, children: Vec<LayoutSimple> },
    #[serde(rename = "leaf")]
    Leaf { id: usize, #[serde(default)] active: bool },
}

pub fn list_windows_json(app: &AppState) -> io::Result<String> {
    let mut v: Vec<WinInfo> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() { v.push(WinInfo { id: w.id, name: w.name.clone(), active: i == app.active_idx, activity: w.activity_flag, bell: w.bell_flag, last: i == app.last_window_idx, tab_text: String::new(), idx: app.win_display_index(i) }); }
    let s = serde_json::to_string(&v).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))?;
    Ok(s)
}

/// tmux-compatible list-windows output: one line per window
/// Format: `<index>: <name><flag> (<pane_count> panes) [<width>x<height>]`
pub fn list_windows_tmux(app: &AppState) -> String {
    use crate::tree::*;
    fn count_panes(node: &Node) -> usize {
        match node {
            Node::Leaf(_) => 1,
            Node::Split { children, .. } => children.iter().map(|c| count_panes(c)).sum(),
        }
    }
    let mut lines = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        let flag = if i == app.active_idx { "*" } else if w.activity_flag { "#" } else { "-" };
        let pane_count = count_panes(&w.root);
        let (width, height) = if let Some(p) = active_pane(&w.root, &w.active_path) {
            (p.last_cols, p.last_rows)
        } else { (120, 30) };
        lines.push(format!("{}: {}{} ({} panes) [{}x{}]", app.win_display_index(i), w.name, flag, pane_count, width, height));
    }
    lines.join("\n")
}

pub fn list_tree_json(app: &AppState) -> io::Result<String> {
    fn collect_panes(node: &Node, out: &mut Vec<PaneInfo>) {
        match node {
            Node::Leaf(p) => { out.push(PaneInfo { id: p.id, title: p.title.clone() }); }
            Node::Split { children, .. } => { for c in children.iter() { collect_panes(c, out); } }
        }
    }
    let mut v: Vec<WinTree> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        let mut panes = Vec::new();
        collect_panes(&w.root, &mut panes);
        v.push(WinTree { id: w.id, name: w.name.clone(), active: i == app.active_idx, panes, idx: app.win_display_index(i) });
    }
    let s = serde_json::to_string(&v).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))?;
    Ok(s)
}

/// Build a simplified layout tree for a specific window (issue #257
/// preview rendering). Returns `None` if the window id is not found.
pub fn window_layout_simple(app: &AppState, win_id: usize) -> Option<LayoutSimple> {
    fn build(node: &Node, active_path: &[usize], cur_path: &mut Vec<usize>) -> LayoutSimple {
        match node {
            Node::Split { kind, sizes, children } => {
                let k = match *kind {
                    crate::types::LayoutKind::Horizontal => "Horizontal".to_string(),
                    crate::types::LayoutKind::Vertical => "Vertical".to_string(),
                };
                let mut ch = Vec::with_capacity(children.len());
                for (i, c) in children.iter().enumerate() {
                    cur_path.push(i);
                    ch.push(build(c, active_path, cur_path));
                    cur_path.pop();
                }
                LayoutSimple::Split { kind: k, sizes: sizes.clone(), children: ch }
            }
            Node::Leaf(p) => LayoutSimple::Leaf {
                id: p.id,
                active: cur_path.as_slice() == active_path,
            },
        }
    }
    let w = app.windows.iter().find(|w| w.id == win_id)?;
    let mut path = Vec::new();
    Some(build(&w.root, &w.active_path, &mut path))
}

pub fn window_layout_json(app: &AppState, win_id: usize) -> io::Result<String> {
    let layout = window_layout_simple(app, win_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "window not found"))?;
    serde_json::to_string(&layout)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))
}

pub const BASE64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &str) -> String {
    let bytes = data.as_bytes();
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        result.push(BASE64_CHARS[b0 >> 2] as char);
        result.push(BASE64_CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        if chunk.len() > 1 {
            result.push(BASE64_CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(BASE64_CHARS[b2 & 0x3f] as char);
        } else {
            result.push('=');
        }
    }
    result
}

pub fn base64_decode(encoded: &str) -> Option<String> {
    let mut result = Vec::new();
    // tmux parity: b64_pton (compat/base64.c) skips ASCII whitespace anywhere
    // in the payload, so OSC 52 producers that wrap long base64 still decode.
    // Any other non-alphabet byte still rejects the whole payload below.
    let chars: Vec<u8> = encoded
        .bytes()
        .filter(|&b| b != b'=' && !b.is_ascii_whitespace())
        .collect();
    for chunk in chars.chunks(4) {
        if chunk.len() < 2 { break; }
        let b0 = BASE64_CHARS.iter().position(|&c| c == chunk[0])? as u8;
        let b1 = BASE64_CHARS.iter().position(|&c| c == chunk[1])? as u8;
        result.push((b0 << 2) | (b1 >> 4));
        if chunk.len() > 2 {
            let b2 = BASE64_CHARS.iter().position(|&c| c == chunk[2])? as u8;
            result.push((b1 << 4) | (b2 >> 2));
            if chunk.len() > 3 {
                let b3 = BASE64_CHARS.iter().position(|&c| c == chunk[3])? as u8;
                result.push((b2 << 6) | b3);
            }
        }
    }
    String::from_utf8(result).ok()
}

/// Return color name as a string. Uses static strings for Default and
/// the 256 indexed colors to avoid heap allocations on every cell.
/// Quote and escape an argument for safe transmission over the control protocol.
/// Wraps the value in double quotes and escapes any embedded double quotes or
/// backslashes. Also escapes the two line-terminator bytes (0x0A/0x0D): the wire
/// protocol is line-oriented and the server reads one command per `read_line`, so
/// a raw newline inside an argument would cut the line and the tail would be
/// executed as a separate command against the caller's session (issue #560, a
/// send-keys injection). Escaping them to `\n`/`\r` keeps the whole argument on a
/// single wire line. Backslash is escaped first so the escape bytes introduced
/// here are not doubled.
pub fn quote_arg(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{}\"", escaped)
}

/// Split `claim-session` arguments into its positionals and its `-p` priority.
///
/// Lives here rather than inline in the connection handler so the wire contract
/// can be unit tested against the real parser instead of a copy that drifts
/// (#608). Returns `(positionals, priority)`, where positionals keep their
/// historical meaning: `[0]` session name, `[1]` optional client CWD.
///
/// Priority is a FLAG and not a third positional on purpose. The CWD is
/// optional, so a bare third positional would slide into index 1 whenever the
/// CWD was omitted and be silently mistaken for the directory. Consuming
/// `-p <value>` as a pair leaves both existing positionals exactly where they
/// were, and cannot collide with a real path, because the pre-existing filter
/// this replaces already assumed no positional starts with a dash.
pub fn parse_claim_args(args: &[&str]) -> (Vec<String>, Option<String>) {
    let mut priority: Option<String> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        let a = args[i];
        if a == "-p" {
            priority = args.get(i + 1).map(|s| (*s).to_string());
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            positionals.push(a.to_string());
        }
        i += 1;
    }
    (positionals, priority)
}

/// Quote an argument for the wire only when the server's quote-aware
/// re-tokenizer would otherwise corrupt it: empty (collapses into joining
/// whitespace), whitespace (re-split), or quote characters (stripped as
/// quoting syntax). Quoting always goes through `quote_arg` so backslashes
/// are escaped to match what `parse_command_line` decodes inside double
/// quotes — the old encoders escaped only `"`, so every `\\` collapsed to
/// `\` and a trailing `\` consumed the closing quote and swallowed the
/// following flags (issue #547). Values needing no quoting are passed
/// through untouched on purpose: outside quotes the parser treats
/// backslashes as literal, so an unquoted value is byte-exact.
pub fn quote_arg_if_needed(s: &str) -> String {
    if s.is_empty()
        || s.chars().any(char::is_whitespace)
        || s.contains('"')
        || s.contains('\'')
    {
        quote_arg(s)
    } else {
        s.to_string()
    }
}

/// Parse the canonical tmux logging idiom `cat > <path>` / `cat >> <path>`
/// so `pipe-pane` can service it in-process as a direct file sink.
///
/// On Windows the piped command runs under PowerShell (`resolve_run_shell`),
/// where `cat` is the `Get-Content` alias: it never reads stdin, exits at
/// once, and the redirection leaves a 0-byte file — so the single most
/// common tmux sink, and the one this repo's own docs show, could not work
/// through the shell at all. Recognizing the idiom here lets the server
/// write the pane's raw ConPTY bytes to the file itself: byte-faithful (no
/// PowerShell line decoding/re-encoding) and with no child process to fail
/// silently.
///
/// Returns `(path, append)` — `append` is true for `>>`. The path may be
/// single- or double-quoted (one level stripped; inner quote characters
/// arrive intact through the #547/#563 wire round-trip). Anything that is
/// not exactly this shape — options on `cat`, an unquoted path containing
/// whitespace, trailing tokens after a quoted path, shell metacharacters,
/// anything a shell would expand (`$var`, backticks, `%var%`, leading `~`)
/// — returns `None` and falls through to the shell sink unchanged: the
/// file sink must only ever intercept a path the user meant literally.
pub fn parse_cat_file_sink(cmd: &str) -> Option<(String, bool)> {
    let trimmed = cmd.trim();
    // PowerShell aliases are case-insensitive, so `CAT`/`Cat` hit the same
    // Get-Content trap as `cat` — match the word the same way.
    if trimmed.len() < 3 || !trimmed[..3].eq_ignore_ascii_case("cat") {
        return None;
    }
    let rest = &trimmed[3..];
    // `cat` must end at a word boundary: whitespace or the redirection itself.
    if !rest.is_empty() && !rest.starts_with('>') && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (append, rest) = if let Some(r) = rest.strip_prefix(">>") {
        (true, r)
    } else if let Some(r) = rest.strip_prefix('>') {
        (false, r)
    } else {
        return None;
    };
    let path_part = rest.trim();
    if path_part.is_empty() {
        return None;
    }
    let quoted = |q: char| {
        path_part.len() >= 2 && path_part.starts_with(q) && path_part.ends_with(q)
    };
    let path = if quoted('"') || quoted('\'') {
        let inner = &path_part[1..path_part.len() - 1];
        // A quote character inside means this was not one plainly quoted
        // path (e.g. `"a" "b"`, or nested quoting) — shell territory. `$`
        // and backticks are what PowerShell would expand inside double
        // quotes; `#` is what tmux would expand as a format (`#I`, `#{...}`).
        // Intercepting any of them would silently take the LITERAL text as
        // a filename, so they go to the shell too.
        if inner.is_empty() || inner.contains(['"', '\'', '$', '`', '#']) {
            return None;
        }
        inner.to_string()
    } else {
        // Unquoted: whitespace, further shell syntax, or anything a shell
        // would expand means the command is more than a plain literal file
        // redirect — leave it to the shell.
        if path_part.chars().any(char::is_whitespace)
            || path_part.contains(['>', '<', '|', '&', '"', '\'', ';', '$', '`', '%', '(', ')', '^', '#'])
            || path_part.starts_with('~')
        {
            return None;
        }
        path_part.to_string()
    };
    Some((path, append))
}

/// Path-shape gate for the direct file sink. The open runs on the server's
/// single event loop, so every path class whose CreateFile can stall or hit
/// a device instead of a file must be refused BEFORE opening:
///
/// - UNC (`\\server\share`, `//server/share`, `\\?\UNC\...`): CreateFile
///   against an unreachable host blocks for tens of seconds and would
///   freeze every pane. `\\?\C:\...` (extended-length local) is allowed.
/// - DOS reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`,
///   `LPT1`-`LPT9`, with or without an extension, in any directory):
///   CreateFile opens the DEVICE — `cat > CON` would tee raw VT bytes into
///   the server's own console while looking like a successful log.
///
/// Returns `Some(reason)` when the path must be refused. Remote-drive
/// detection needs a live Win32 call and lives with the caller.
pub fn refuse_file_sink_path(path: &str) -> Option<String> {
    if path.starts_with("\\\\.\\") {
        return Some("device namespace not supported for the direct file sink".to_string());
    }
    let is_unc_verbatim = path.to_ascii_lowercase().starts_with("\\\\?\\unc\\");
    let is_verbatim_local = path.starts_with("\\\\?\\") && !is_unc_verbatim;
    if !is_verbatim_local
        && (path.starts_with("\\\\") || path.starts_with("//"))
    {
        return Some("UNC path not supported for the direct file sink".to_string());
    }
    let basename = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path);
    let stem = basename.split('.').next().unwrap_or(basename);
    // `NUL:` / `COM1:` etc. are still the device — strip the DOS-style
    // trailing colon before matching.
    let stem_upper = stem.trim().trim_end_matches(':').to_ascii_uppercase();
    let reserved = matches!(stem_upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem_upper.len() == 4
            && (stem_upper.starts_with("COM") || stem_upper.starts_with("LPT"))
            && stem_upper.as_bytes()[3].is_ascii_digit()
            && stem_upper.as_bytes()[3] != b'0');
    if reserved {
        return Some(format!(
            "reserved device name '{}' not supported for the direct file sink",
            stem_upper
        ));
    }
    None
}

/// Parse `VARIABLE=value` for tmux `new-session -e` / internal `server -e`
/// (split on the first `=` so values may contain `=`).
pub fn parse_env_assignment(s: &str) -> Result<(String, String), &'static str> {
    let s = s.trim();
    let eq = s.find('=').ok_or("expected VARIABLE=value")?;
    if eq == 0 {
        return Err("invalid environment variable name");
    }
    let name = &s[..eq];
    let value = &s[eq + 1..];
    if !is_valid_env_var_name(name) {
        return Err("invalid environment variable name");
    }
    Ok((name.to_string(), value.to_string()))
}

/// Parse the token after `-e` on `new-session` or internal `server -e`.
/// Shared by CLI short flags, control protocol `new-session`, and [`collect_server_session_env_args`].
pub fn parse_new_session_e_value_token(next_arg: Option<&str>) -> Result<(String, String), String> {
    let Some(s) = next_arg else {
        return Err("-e requires a value".to_string());
    };
    parse_env_assignment(s).map_err(|e| format!("invalid -e: {}", e))
}

fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
    }
    true
}

/// Merge CLI `new-session -e` pairs into session environment.
pub fn merge_session_env_into_app(app: &mut crate::types::AppState, session_env: &[(String, String)]) {
    for (k, v) in session_env {
        app.environment.insert(k.clone(), v.clone());
    }
}

/// Collect `-e` flags from internal `server` argv (only before `--`).
pub fn collect_server_session_env_args(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let limit = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let mut out = Vec::new();
    let mut i = 0;
    while i < limit {
        if args[i] == "-e" {
            let pair = parse_new_session_e_value_token(args.get(i + 1).map(|s| s.as_str()))?;
            out.push(pair);
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(out)
}

/// Encode bytes as lowercase hex with no separators.
///
/// Used to carry a payload that must survive the control wire BYTE-EXACT.
/// Every command line the server receives is tokenized before a handler sees
/// it (`;` splits it into sub-commands, quote grouping is stripped, runs of
/// whitespace collapse), so any operand that is data rather than a word has to
/// be reduced to `[0-9a-f]` first.  `send -H` already does this for keystrokes;
/// buffer contents use the same shape.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{:02x}", b));
    }
    out
}

/// Decode the output of [`hex_encode`].  `None` for an odd length or any
/// non-hex character — a malformed payload is reported to the sender, never
/// silently turned into partial data.
pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::parse_command_line;

    /// The whole point of the hex wire: a buffer that the tokenizer would
    /// otherwise destroy survives encode → tokenize → decode byte for byte.
    #[test]
    fn test_hex_buffer_wire_survives_tokenizer() {
        let content = "printf '%s' 'QUOTED ARG'\tTAB\nline \"two\"; not-a-command\n\x1b[0m";
        let line = format!("set-buffer -b pb -H {}", hex_encode(content.as_bytes()));
        // What the server actually receives: the tokenized command line.
        let toks = parse_command_line(&line);
        assert_eq!(toks.len(), 5, "hex payload must stay a single token");
        let decoded = hex_decode(&toks[4]).expect("valid hex");
        assert_eq!(String::from_utf8(decoded).unwrap(), content);
    }

    #[test]
    fn test_hex_encode_is_lowercase_pairs() {
        assert_eq!(hex_encode(b"\x00\x0f\xff A"), "000fff2041");
    }

    #[test]
    fn test_hex_decode_rejects_malformed() {
        assert_eq!(hex_decode("abc"), None, "odd length");
        assert_eq!(hex_decode("zz"), None, "non-hex digit");
        assert_eq!(hex_decode(""), Some(Vec::new()));
    }

    #[test]
    fn test_quote_arg_simple() {
        assert_eq!(quote_arg("hello"), "\"hello\"");
    }

    #[test]
    fn test_quote_arg_with_spaces() {
        assert_eq!(quote_arg("cc 123"), "\"cc 123\"");
    }

    #[test]
    fn test_quote_arg_with_embedded_quotes() {
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
    }

    #[test]
    fn test_quote_arg_with_backslash() {
        assert_eq!(quote_arg("C:\\Users\\foo"), "\"C:\\\\Users\\\\foo\"");
    }

    #[test]
    fn test_quote_arg_empty() {
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn test_rename_session_roundtrip_with_spaces() {
        let name = "cc 123";
        let cmd = format!("rename-session {}", quote_arg(name));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["rename-session", "cc 123"]);
    }

    #[test]
    fn test_rename_window_roundtrip_with_spaces() {
        let name = "my window";
        let cmd = format!("rename-window {}", quote_arg(name));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["rename-window", "my window"]);
    }

    #[test]
    fn test_set_pane_title_roundtrip_with_spaces() {
        let title = "pane title here";
        let cmd = format!("set-pane-title {}", quote_arg(title));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["set-pane-title", "pane title here"]);
    }

    #[test]
    fn test_source_file_roundtrip_windows_path_with_spaces() {
        let path = "C:\\Program Files\\psmux\\config.conf";
        let cmd = format!("source-file {}", quote_arg(path));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["source-file", "C:\\Program Files\\psmux\\config.conf"]);
    }

    #[test]
    fn test_claim_session_roundtrip_with_spaces() {
        let name = "my session";
        let cwd = "C:\\Users\\My Name\\Documents";
        let cmd = format!("claim-session {} {}", quote_arg(name), quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "my session", "C:\\Users\\My Name\\Documents"]);
    }

    #[test]
    fn test_roundtrip_name_with_embedded_quotes() {
        let name = "say \"hello\" world";
        let cmd = format!("rename-session {}", quote_arg(name));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["rename-session", "say \"hello\" world"]);
    }

    #[test]
    fn test_roundtrip_no_spaces_still_works() {
        let name = "simple";
        let cmd = format!("rename-session {}", quote_arg(name));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["rename-session", "simple"]);
    }

    #[test]
    fn test_claim_session_roundtrip_root_dir() {
        // Root paths like C:\ end in a backslash which must survive
        // the quote_arg -> parse_command_line roundtrip.
        let name = "mysession";
        let cwd = "C:\\";
        let cmd = format!("claim-session {} {}", quote_arg(name), quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "mysession", "C:\\"]);
    }

    #[test]
    fn test_claim_session_roundtrip_trailing_backslash_dir() {
        // Paths ending in backslash (e.g. D:\Projects\) must roundtrip.
        let cwd = "D:\\Projects\\";
        let cmd = format!("claim-session sess {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "sess", "D:\\Projects\\"]);
    }

    #[test]
    fn test_claim_session_roundtrip_path_with_spaces() {
        let cwd = "C:\\Program Files\\My App\\Data";
        let cmd = format!("claim-session s1 {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "s1", "C:\\Program Files\\My App\\Data"]);
    }

    #[test]
    fn test_claim_session_roundtrip_deep_nested_path() {
        let cwd = "C:\\Users\\test\\Documents\\workspace\\project\\src\\components";
        let cmd = format!("claim-session s1 {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "s1", cwd]);
    }

    #[test]
    fn test_claim_session_roundtrip_unc_path() {
        let cwd = "\\\\server\\share\\folder";
        let cmd = format!("claim-session s1 {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "s1", "\\\\server\\share\\folder"]);
    }

    #[test]
    fn test_claim_session_roundtrip_path_with_parens() {
        let cwd = "C:\\Program Files (x86)\\App";
        let cmd = format!("claim-session s1 {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "s1", "C:\\Program Files (x86)\\App"]);
    }

    #[test]
    fn test_claim_session_roundtrip_path_with_ampersand() {
        let cwd = "C:\\R&D\\project";
        let cmd = format!("claim-session s1 {}", quote_arg(cwd));
        let args = parse_command_line(&cmd);
        assert_eq!(args, vec!["claim-session", "s1", "C:\\R&D\\project"]);
    }

    /// Verify that send-keys with Claude Code agent spawn commands preserves
    /// Windows paths and POSIX-escaped characters (psmux#172, #173, #180).
    /// The CLI wraps the key in double-quotes without escaping backslashes,
    /// and parse_command_line keeps lone backslashes literal (Windows paths).
    #[test]
    fn test_send_keys_claude_code_agent_command_preserves_backslashes() {
        // Simulate the control-protocol line built by the CLI send-keys handler:
        // send-keys "cd 'C:\path with spaces' && env CLAUDECODE=1 'C:\...\claude.exe' --agent-id ..." Enter
        let agent_cmd = "cd 'C:\\cctest\\a long dir name' && env CLAUDECODE=1 'C:\\Users\\foo\\.local\\bin\\claude.exe' --agent-id researcher\\@my-team";
        let line = format!("send-keys \"{}\" Enter", agent_cmd);
        let args = parse_command_line(&line);
        assert_eq!(args[0], "send-keys");
        assert_eq!(args[1], agent_cmd);
        assert_eq!(args[2], "Enter");
    }

    #[test]
    fn test_send_keys_single_quoted_windows_path() {
        // Single-quoted paths from shell-quote: 'C:\Users\foo'
        let line = "send-keys \"cd 'C:\\Users\\foo\\project'\" Enter";
        let args = parse_command_line(line);
        assert_eq!(args[1], "cd 'C:\\Users\\foo\\project'");
    }

    #[test]
    fn parse_env_assignment_basic() {
        assert_eq!(
            parse_env_assignment("FOO=bar").unwrap(),
            ("FOO".to_string(), "bar".to_string())
        );
    }

    #[test]
    fn parse_env_assignment_empty_value() {
        assert_eq!(
            parse_env_assignment("VAR=").unwrap(),
            ("VAR".to_string(), "".to_string())
        );
    }

    #[test]
    fn parse_env_assignment_value_with_equals() {
        assert_eq!(
            parse_env_assignment("FOO=a=b=c").unwrap(),
            ("FOO".to_string(), "a=b=c".to_string())
        );
    }

    #[test]
    fn parse_env_assignment_rejects_no_equals() {
        assert!(parse_env_assignment("FOO").is_err());
    }

    #[test]
    fn parse_env_assignment_rejects_bad_name() {
        assert!(parse_env_assignment("123=x").is_err());
        assert!(parse_env_assignment("bad-name=x").is_err());
    }

    #[test]
    fn parse_new_session_e_value_token_missing() {
        assert_eq!(
            parse_new_session_e_value_token(None).unwrap_err(),
            "-e requires a value"
        );
    }

    #[test]
    fn parse_new_session_e_value_token_ok() {
        let p = parse_new_session_e_value_token(Some("Z=1")).unwrap();
        assert_eq!(p, ("Z".to_string(), "1".to_string()));
    }

    #[test]
    fn collect_server_session_env_skips_after_dd() {
        let args: Vec<String> = vec![
            "psmux".into(), "server".into(), "-s".into(), "s1".into(),
            "-e".into(), "A=1".into(),
            "--".into(), "cmd".into(), "-e".into(), "IGNORE=me".into(),
        ];
        let v = collect_server_session_env_args(&args).unwrap();
        assert_eq!(v, vec![("A".to_string(), "1".to_string())]);
    }

    #[test]
    fn collect_server_session_env_duplicate_key_last_wins() {
        let args: Vec<String> = vec![
            "psmux".into(), "server".into(), "-s".into(), "s1".into(),
            "-e".into(), "FOO=first".into(),
            "-e".into(), "FOO=last".into(),
        ];
        let v = collect_server_session_env_args(&args).unwrap();
        assert_eq!(v.len(), 2);
        let mut app = crate::types::AppState::new("t".to_string());
        merge_session_env_into_app(&mut app, &v);
        assert_eq!(app.environment.get("FOO").map(|s| s.as_str()), Some("last"));
    }
}

pub fn color_to_name(c: vt100::Color) -> std::borrow::Cow<'static, str> {
    use std::borrow::Cow;
    match c {
        vt100::Color::Default => Cow::Borrowed("default"),
        vt100::Color::Idx(i) => {
            // Static lookup table for all 256 indexed colors
            static IDX_STRINGS: std::sync::LazyLock<[String; 256]> = std::sync::LazyLock::new(|| {
                std::array::from_fn(|i| format!("idx:{}", i))
            });
            Cow::Borrowed(&IDX_STRINGS[i as usize])
        }
        vt100::Color::Rgb(r,g,b) => Cow::Owned(format!("rgb:{},{},{}", r,g,b)),
    }
}

/// Environment marker put on a popup/float child so it is not mistaken for a
/// pane child by the nested-session guard.  See [`inside_psmux_pane`].
pub const POPUP_CHILD_ENV: &str = "PSMUX_POPUP";

/// True when this process runs inside a psmux **pane**, i.e. when a client that
/// grabs this terminal would genuinely nest a session inside another one.
///
/// tmux's `server_client_check_nested()` needs BOTH conditions to hold: `$TMUX`
/// is set AND the client's tty is the tty of one of the server's window panes.
/// A `display-popup` runs on a pty created by `job_run()`, which is never
/// registered in `all_window_panes`, so the tty half of the test fails and tmux
/// lets `tmux attach -t other` through inside a popup.  That is precisely what
/// makes the popup scratch-session idiom work upstream.
///
/// psmux has no ttys to compare, so the popup child carries
/// [`POPUP_CHILD_ENV`] instead (set in `popup::create_popup_pane`, cleared for
/// every real pane child in `pane::set_tmux_env`), and a popup child is treated
/// as not-in-a-pane exactly like tmux treats it (#537).
pub fn inside_psmux_pane() -> bool {
    if in_popup_child() {
        return false;
    }
    std::env::var("PSMUX_ACTIVE").ok().as_deref() == Some("1")
        || std::env::var("PSMUX_SESSION")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
}

/// True when this process was spawned as a popup/float child.
pub fn in_popup_child() -> bool {
    std::env::var(POPUP_CHILD_ENV).ok().as_deref() == Some("1")
}

/// True when the terminal this process talks to is drawn by psmux itself: a
/// pane child or a popup/float child.
///
/// Use this, not [`inside_psmux_pane`], for anything that treats the terminal
/// as a terminal (querying it and waiting for a reply). The two differ on
/// purpose in both directions:
///
///   * a popup counts here but not there, because a popup is not a nested
///     session yet its terminal is still psmux (#537), and
///   * `PSMUX_ACTIVE` counts there but NOT here. That variable only says "this
///     process is a psmux client", which every top-level client sets on itself
///     before it queries its real terminal. Treating it as evidence that psmux
///     draws the terminal would suppress the host-color query for EVERY client
///     and quietly undo #473. Only what the server plants on the children it
///     spawns is evidence about who draws the terminal.
pub fn psmux_drawn_terminal() -> bool {
    in_popup_child()
        || std::env::var("PSMUX_SESSION")
            .ok()
            .filter(|v| !v.is_empty())
            .is_some()
}

#[cfg(test)]
#[path = "../tests-rs/test_run_shell_format_and_start_dir.rs"]
mod tests_run_shell_format_and_start_dir;

#[cfg(test)]
#[path = "../tests-rs/test_issue537_popup_attach.rs"]
mod tests_issue537_popup_attach;

#[cfg(test)]
#[path = "../tests-rs/test_nested_client_color_query.rs"]
mod tests_nested_client_color_query;

#[cfg(test)]
#[path = "../tests-rs/test_deps_base64_parity.rs"]
mod tests_deps_base64_parity;

#[cfg(test)]
#[path = "../tests-rs/test_issue560_quote_arg_control_bytes.rs"]
mod tests_issue560_quote_arg_control_bytes;

#[cfg(test)]
#[path = "../tests-rs/test_pipe_pane_cat_file_sink.rs"]
mod tests_pipe_pane_cat_file_sink;
