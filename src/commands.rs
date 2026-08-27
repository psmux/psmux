use std::io;
use std::time::Instant;
#[cfg(windows)]
use std::path::PathBuf;

use std::io::Write;
use crate::types::{AppState, Mode, Action, FocusDir, LayoutKind, MenuItem, Menu, Node};
use crate::tree::{compute_rects, kill_all_children, get_active_pane_id};
use crate::pane::{create_window, split_active, kill_active_pane};
use crate::copy_mode::{enter_copy_mode, scroll_copy_up, switch_with_copy_save, paste_latest,
    capture_active_pane, save_latest_buffer};
use crate::session::{send_control_to_port, list_all_sessions_tree};
use crate::window_ops::{toggle_zoom, unzoom_if_zoomed};

/// Parse a popup dimension spec: "80" (absolute) or "95%" (percentage of term_dim).
pub(crate) fn parse_popup_dim_local(spec: &str, term_dim: u16, default: u16) -> u16 {
    if let Some(pct_str) = spec.strip_suffix('%') {
        if let Ok(pct) = pct_str.parse::<u16>() {
            let pct = pct.min(100);
            (term_dim as u32 * pct as u32 / 100) as u16
        } else {
            default
        }
    } else {
        spec.parse().unwrap_or(default)
    }
}

/// The default format string for `display-message` when no argument is given (tmux parity).
pub(crate) const DISPLAY_MESSAGE_DEFAULT_FMT: &str =
    "[#{session_name}] #{window_index}:#{window_name}#{window_flags} \"#{pane_title}\" #{pane_index} #{pane_current_command}";

/// Resolve the shell and its invocation prefix for `run-shell` commands.
/// Returns (program, prefix_args) where prefix_args are flags like ["-NoProfile", "-Command"].
/// On Windows: tries pwsh -> powershell -> cmd.
/// On Unix: uses sh -c.
pub fn resolve_run_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        if let Ok(path) = which::which("pwsh") {
            return (path.to_string_lossy().into_owned(), vec!["-NoProfile".to_string(), "-Command".to_string()]);
        }
        if let Ok(path) = which::which("powershell") {
            return (path.to_string_lossy().into_owned(), vec!["-NoProfile".to_string(), "-Command".to_string()]);
        }
        if let Ok(system_root) = std::env::var("SystemRoot").or_else(|_| std::env::var("SYSTEMROOT")) {
            let powershell = PathBuf::from(&system_root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            if powershell.is_file() {
                return (powershell.to_string_lossy().into_owned(), vec!["-NoProfile".to_string(), "-Command".to_string()]);
            }
            let cmd = PathBuf::from(&system_root).join("System32").join("cmd.exe");
            if cmd.is_file() {
                return (cmd.to_string_lossy().into_owned(), vec!["/c".to_string()]);
            }
        }
        if let Ok(comspec) = std::env::var("ComSpec").or_else(|_| std::env::var("COMSPEC")) {
            let trimmed = comspec.trim();
            if !trimmed.is_empty() {
                return (trimmed.to_string(), vec!["/c".to_string()]);
            }
        }
        ("cmd".to_string(), vec!["/c".to_string()])
    }
    #[cfg(not(windows))]
    {
        ("sh".to_string(), vec!["-c".to_string()])
    }
}

/// Resolve a shell binary name to an available executable path.
/// Handles fallback between `pwsh` and `powershell` when one is not installed.
/// For `cmd`/`cmd.exe` or already-resolved paths, returns the input unchanged.
#[cfg(windows)]
fn resolve_shell_binary(name: &str) -> String {
    let lower = name.to_lowercase();
    let is_pwsh = lower == "pwsh" || lower == "pwsh.exe";
    let is_powershell = lower == "powershell" || lower == "powershell.exe";

    if is_pwsh {
        // Requested pwsh: verify it exists, fall back to powershell
        if which::which("pwsh").is_ok() {
            return name.to_string();
        }
        if let Ok(p) = which::which("powershell") {
            return p.to_string_lossy().into_owned();
        }
    } else if is_powershell {
        // Requested powershell: verify it exists, fall back to pwsh
        if which::which("powershell").is_ok() {
            return name.to_string();
        }
        if let Ok(p) = which::which("pwsh") {
            return p.to_string_lossy().into_owned();
        }
    }

    // cmd, cmd.exe, or already a full path: use as-is
    name.to_string()
}

/// Try to locate an existing file at the start of a command string.
/// Handles paths with spaces by progressively trying longer path prefixes
/// against the filesystem (e.g. "C:\Program Files\App\run.ps1 arg1 arg2"
/// tries "C:\Program", then "C:\Program Files\App\run.ps1", etc.).
/// Returns `Some((file_path, remaining_args))` on success.
#[cfg(windows)]
fn find_file_in_command(cmd: &str) -> Option<(String, String)> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() { return None; }
    let bytes = trimmed.as_bytes();
    let mut end = 0;
    loop {
        // Advance to the next whitespace boundary
        while end < bytes.len() && !bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        let candidate = &trimmed[..end];
        if std::path::Path::new(candidate).is_file() {
            let rest = trimmed[end..].trim_start().to_string();
            return Some((candidate.to_string(), rest));
        }
        if end >= bytes.len() { return None; }
        // Skip whitespace to the next word
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        if end >= bytes.len() { return None; }
    }
}

/// Build a `std::process::Command` for a run-shell invocation.
///
/// Avoids double-wrapping when the command already starts with a shell binary
/// (e.g., `pwsh -NoProfile -File script.ps1`). Also detects file paths
/// (including those with spaces) and uses the appropriate execution strategy:
/// `-File` for `.ps1`, direct `Command::new` for `.exe`/`.cmd`/`.bat`,
/// and PowerShell call operator `& 'path'` for other files with spaces.
pub fn build_run_shell_command(shell_cmd: &str) -> std::process::Command {
    #[cfg(windows)]
    {
        use crate::platform::HideWindowCommandExt;
        let lower = shell_cmd.trim_start().to_lowercase();

        // Case 1: Command already starts with a shell binary (pwsh, powershell, cmd).
        // Run it directly to avoid nesting `pwsh -Command "pwsh -File ..."`.
        // If the specified shell isn't found, fall back to the alternative
        // (e.g. pwsh -> powershell) so plugin configs work on systems that
        // only have one of the two installed.
        if lower.starts_with("pwsh ") || lower.starts_with("pwsh.exe ")
            || lower.starts_with("powershell ") || lower.starts_with("powershell.exe ")
            || lower.starts_with("cmd ") || lower.starts_with("cmd.exe ")
        {
            let parts = parse_command_line(shell_cmd);
            if parts.len() >= 2 {
                let prog = resolve_shell_binary(&parts[0]);
                let mut c = std::process::Command::new(&prog);
                for p in &parts[1..] { c.arg(p); }
                c.hide_window();
                return c;
            }
        }

        // Case 2: File path detection (handles spaces in paths).
        // Uses progressive path probing: for "C:\Program Files\App\run.ps1 arg1",
        // tries "C:\Program" (not a file), then "C:\Program Files\App\run.ps1"
        // (found!), returning the file path and remaining arguments separately.
        let trimmed = shell_cmd.trim();
        // Strip matching outer quotes (single or double) so file detection works
        // for run-shell "'~/path/to/script.ps1'" syntax from config or CLI
        let trimmed = if (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
                       || (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2) {
            &trimmed[1..trimmed.len()-1]
        } else {
            trimmed
        };
        if let Some((file_path, rest_args)) = find_file_in_command(trimmed) {
            let lower_path = file_path.to_lowercase();

            // .ps1 scripts: use -File which never splits paths at whitespace
            if lower_path.ends_with(".ps1") {
                let shell = if which::which("pwsh").is_ok() { "pwsh" } else { "powershell" };
                let mut c = std::process::Command::new(shell);
                c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &file_path]);
                if !rest_args.is_empty() {
                    for a in &parse_command_line(&rest_args) { c.arg(a); }
                }
                c.hide_window();
                return c;
            }

            // For other file types with spaces in the path, we must avoid
            // the Case 3 shell wrapping which breaks on spaces.
            if file_path.contains(' ') {
                let ext = std::path::Path::new(&file_path).extension()
                    .and_then(|e| e.to_str()).map(|e| e.to_lowercase());

                match ext.as_deref() {
                    // Native executables: Command::new handles path quoting via CreateProcess
                    Some("exe") | Some("com") => {
                        let mut c = std::process::Command::new(&file_path);
                        if !rest_args.is_empty() {
                            for a in &parse_command_line(&rest_args) { c.arg(a); }
                        }
                        c.hide_window();
                        return c;
                    }
                    // Batch files: run via cmd.exe /c with the path as a separate arg
                    // so CreateProcess quotes just the path, not path+args together
                    Some("cmd") | Some("bat") => {
                        let mut c = std::process::Command::new("cmd.exe");
                        c.arg("/c");
                        c.arg(&file_path);
                        if !rest_args.is_empty() {
                            for a in &parse_command_line(&rest_args) { c.arg(a); }
                        }
                        c.hide_window();
                        return c;
                    }
                    // Unknown extension with spaces: use the resolved shell with
                    // proper quoting. For PowerShell, use the call operator & 'path'
                    // so the path is treated as a single literal string.
                    _ => {
                        let (shell_prog, shell_args) = resolve_run_shell();
                        let lower_shell = shell_prog.to_lowercase();
                        let is_powershell = lower_shell.contains("pwsh")
                            || lower_shell.contains("powershell");
                        let mut c = std::process::Command::new(&shell_prog);
                        for a in &shell_args { c.arg(a); }
                        if is_powershell {
                            let escaped = file_path.replace('\'', "''");
                            let wrapped = if rest_args.is_empty() {
                                format!("& '{}'", escaped)
                            } else {
                                format!("& '{}' {}", escaped, rest_args)
                            };
                            c.arg(&wrapped);
                        } else {
                            // cmd.exe /c: pass path and args separately
                            c.arg(&file_path);
                            if !rest_args.is_empty() {
                                for a in &parse_command_line(&rest_args) { c.arg(a); }
                            }
                        }
                        c.hide_window();
                        return c;
                    }
                }
            }
            // File found but path has no spaces: fall through to Case 3.
            // The simple shell wrapping works fine without spaces.
        }

        // Case 3: Regular command string (no file path with spaces detected).
        // Wrap in the resolved shell (pwsh -Command / cmd /c / sh -c).
        let (shell_prog, shell_args) = resolve_run_shell();
        let mut c = std::process::Command::new(&shell_prog);
        for a in &shell_args { c.arg(a); }
        c.arg(shell_cmd);
        c.hide_window();
        c
    }
    #[cfg(not(windows))]
    {
        let (shell_prog, shell_args) = resolve_run_shell();
        let mut c = std::process::Command::new(&shell_prog);
        for a in &shell_args { c.arg(a); }
        c.arg(shell_cmd);
        c
    }
}

/// Show text output in a popup overlay (used by list-* commands inside a session).
fn show_output_popup(app: &mut AppState, title: &str, output: String) {
    let lines: Vec<&str> = output.lines().collect();
    let width = lines.iter().map(|l| l.len()).max().unwrap_or(40).max(20) as u16 + 4;
    let height = (lines.len() as u16 + 2).max(5);
    app.mode = Mode::PopupMode {
        command: title.to_string(),
        output,
        process: None,
        width: width.min(120),
        height,
        close_on_exit: false,
        popup_pane: None,
        scroll_offset: 0,
    };
}

/// Generate list-windows output from AppState (tmux-compatible format).
fn generate_list_windows(app: &AppState) -> String {
    crate::util::list_windows_tmux(app)
}

/// Generate list-panes output from AppState.
fn generate_list_panes(app: &AppState) -> String {
    let win = &app.windows[app.active_idx];
    fn collect(node: &Node, panes: &mut Vec<(usize, u16, u16)>) {
        match node {
            Node::Leaf(p) => { panes.push((p.id, p.last_cols, p.last_rows)); }
            Node::Split { children, .. } => { for c in children { collect(c, panes); } }
        }
    }
    let mut panes = Vec::new();
    collect(&win.root, &mut panes);
    let active_id = get_active_pane_id(&win.root, &win.active_path);
    let mut output = String::new();
    for (pos, (id, cols, rows)) in panes.iter().enumerate() {
        let idx = pos + app.pane_base_index;
        let marker = if active_id == Some(*id) { " (active)" } else { "" };
        output.push_str(&format!("{}: [{}x{}] [history {}/{}, 0 bytes] %{}{}\n",
            idx, cols, rows, app.history_limit, app.history_limit, id, marker));
    }
    output
}

/// Generate list-clients output from AppState.
fn generate_list_clients(app: &AppState) -> String {
    format!("/dev/pts/0: {}: {} [{}x{}] (utf8)\n",
        app.session_name,
        app.windows[app.active_idx].name,
        app.client_area.width,
        app.client_area.height)
}

/// Generate show-hooks output from AppState.
fn generate_show_hooks(app: &AppState) -> String {
    let mut output = String::new();
    for (name, commands) in &app.hooks {
        if commands.len() == 1 {
            output.push_str(&format!("{} -> {}\n", name, commands[0]));
        } else {
            for (i, cmd) in commands.iter().enumerate() {
                output.push_str(&format!("{}[{}] -> {}\n", name, i, cmd));
            }
        }
    }
    if output.is_empty() {
        output.push_str("(no hooks)\n");
    }
    output
}

/// Generate show-options output locally (embedded mode fallback).
fn generate_show_options(app: &AppState) -> String {
    let mut output = String::new();
    output.push_str(&format!("prefix {}\n", crate::config::format_key_binding(&app.prefix_key)));
    output.push_str(&format!("base-index {}\n", app.window_base_index));
    output.push_str(&format!("pane-base-index {}\n", app.pane_base_index));
    output.push_str(&format!("escape-time {}\n", app.escape_time_ms));
    output.push_str(&format!("mouse {}\n", if app.mouse_enabled { "on" } else { "off" }));
    output.push_str(&format!("scroll-enter-copy-mode {}\n", if app.scroll_enter_copy_mode { "on" } else { "off" }));
    output.push_str(&format!("choose-tree-preview {}\n", if app.choose_tree_preview { "on" } else { "off" }));
    output.push_str(&format!("status {}\n", if app.status_visible { "on" } else { "off" }));
    output.push_str(&format!("status-position {}\n", app.status_position));
    output.push_str(&format!("status-left \"{}\"\n", app.status_left));
    output.push_str(&format!("status-right \"{}\"\n", app.status_right));
    output.push_str(&format!("history-limit {}\n", app.history_limit));
    output.push_str(&format!("display-time {}\n", app.display_time_ms));
    output.push_str(&format!("mode-keys {}\n", app.mode_keys));
    output.push_str(&format!("focus-events {}\n", if app.focus_events { "on" } else { "off" }));
    output.push_str(&format!("renumber-windows {}\n", if app.renumber_windows { "on" } else { "off" }));
    output.push_str(&format!("automatic-rename {}\n", if app.automatic_rename { "on" } else { "off" }));
    output.push_str(&format!("monitor-activity {}\n", if app.monitor_activity { "on" } else { "off" }));
    output.push_str(&format!("synchronize-panes {}\n", if app.sync_input { "on" } else { "off" }));
    output.push_str(&format!("remain-on-exit {}\n", if app.remain_on_exit { "on" } else { "off" }));
    output.push_str(&format!("allow-predictions {}\n", if app.allow_predictions { "on" } else { "off" }));
    // Include @user-options
    for (key, val) in &app.user_options {
        output.push_str(&format!("{} \"{}\"\n", key, val));
    }
    output
}

/// Local join-pane: extract source pane and graft into target window.
fn join_pane_local(app: &mut AppState, src_win: Option<usize>, src_pane: Option<usize>,
                   target_win: Option<usize>, target_pane: Option<usize>, horizontal: bool) {
    // Resolve source/target display indices to Vec positions (default: active
    // window). win_pos honors gapped indices left by renumber-windows off.
    let src_pos = match src_win { Some(d) => app.win_pos(d), None => Some(app.active_idx) };
    let tgt_pos = match target_win { Some(d) => app.win_pos(d), None => Some(app.active_idx) };
    // tmux surfaces an explicit error rather than silently doing nothing when the
    // target cannot be resolved. Without this, `join-pane -t :N` where window N does
    // not exist (common when a user assumes base-index 1) fails with no feedback.
    let src_idx = match src_pos {
        Some(p) => p,
        None => {
            app.status_message = Some((format!("join-pane: can't find source window: {}", src_win.unwrap_or(0)), Instant::now(), None));
            return;
        }
    };
    let raw_target_win = match tgt_pos {
        Some(p) => p,
        None => {
            app.status_message = Some((format!("join-pane: can't find window: {}", target_win.unwrap_or(0)), Instant::now(), None));
            return;
        }
    };
    if src_idx == raw_target_win {
        app.status_message = Some(("join-pane: can't join a pane to its own window".to_string(), Instant::now(), None));
        return;
    }
    {
        // Resolve source pane path
        let src_path = if let Some(pidx) = src_pane {
            let mut leaves = Vec::new();
            crate::tree::collect_leaf_paths_pub(&app.windows[src_idx].root, &mut Vec::new(), &mut leaves);
            if let Some((_, p)) = leaves.get(pidx) {
                p.clone()
            } else {
                app.windows[src_idx].active_path.clone()
            }
        } else {
            app.windows[src_idx].active_path.clone()
        };
        let src_root = std::mem::replace(&mut app.windows[src_idx].root,
            Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
        let (remaining, extracted) = crate::tree::extract_node(src_root, &src_path);
        if let Some(pane_node) = extracted {
            let src_empty = remaining.is_none();
            if let Some(rem) = remaining {
                app.windows[src_idx].root = rem;
                app.windows[src_idx].active_path = crate::tree::first_leaf_path(&app.windows[src_idx].root);
            }
            let tgt = if src_empty && raw_target_win > src_idx { raw_target_win - 1 } else { raw_target_win };
            if src_empty {
                app.windows.remove(src_idx);
                app.on_window_removed(src_idx);
                if app.active_idx >= app.windows.len() {
                    app.active_idx = app.windows.len().saturating_sub(1);
                }
            }
            if tgt < app.windows.len() {
                // Resolve target pane path
                let tgt_path = if let Some(tpidx) = target_pane {
                    let mut leaves = Vec::new();
                    crate::tree::collect_leaf_paths_pub(&app.windows[tgt].root, &mut Vec::new(), &mut leaves);
                    if let Some((_, p)) = leaves.get(tpidx) {
                        p.clone()
                    } else {
                        app.windows[tgt].active_path.clone()
                    }
                } else {
                    app.windows[tgt].active_path.clone()
                };
                let split_kind = if horizontal { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                crate::tree::replace_leaf_with_split(&mut app.windows[tgt].root, &tgt_path, split_kind, pane_node);
                app.active_idx = tgt;
            }
        } else {
            if let Some(rem) = remaining {
                app.windows[src_idx].root = rem;
            }
        }
    }
}

/// Generate list-commands output.
fn generate_list_commands() -> String {
    crate::help::cli_command_lines().join("\n")
}

/// Build the choose-tree data for the WindowChooser mode.
pub fn build_choose_tree(app: &AppState) -> Vec<crate::session::TreeEntry> {
    let current_windows: Vec<(String, usize, String, bool, usize)> = app.windows.iter().enumerate().map(|(i, w)| {
        let panes = crate::tree::count_panes(&w.root);
        let size = format!("{}x{}", w.area.width, w.area.height);
        (w.name.clone(), panes, size, i == app.active_idx, app.win_display_index(i))
    }).collect();
    list_all_sessions_tree(&app.session_name, &current_windows)
}

/// Extract a window index from a tmux-style target string.
/// Handles formats like "0", ":0", ":=0", "=0", stripping leading ':'/'=' chars.
fn parse_window_target(target: &str) -> Option<usize> {
    let s = target.trim_start_matches(':').trim_start_matches('=');
    s.parse::<usize>().ok()
}

/// Parse a command string to an Action
pub fn parse_command_to_action(cmd: &str) -> Option<Action> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return None; }
    
    match parts[0] {
        "display-panes" | "displayp" => Some(Action::DisplayPanes),
        "new-window" | "neww" => {
            // If extra flags like -c, -d, -n, -F, -e or a shell command are present,
            // store as Command to preserve the full argument string (esp. -c for start dir).
            let has_extra = parts.len() > 1;
            if has_extra {
                Some(Action::Command(cmd.to_string()))
            } else {
                Some(Action::NewWindow)
            }
        }
        "split-window" | "splitw" | "split-pane" | "splitp" => {
            // If extra flags like -c, -d, -p, -F, or a shell command are present,
            // store as Command to preserve the full argument string.
            let has_extra = parts.iter().any(|p| matches!(*p, "-c" | "-d" | "-p" | "-l" | "-F" | "-P" | "-b" | "-f" | "-I" | "-Z" | "-e"))
                || parts.iter().any(|p| !p.starts_with('-') && *p != "split-window" && *p != "splitw" && *p != "split-pane" && *p != "splitp");
            if has_extra {
                Some(Action::Command(cmd.to_string()))
            } else if parts.iter().any(|p| *p == "-h") {
                Some(Action::SplitHorizontal)
            } else {
                Some(Action::SplitVertical)
            }
        }
        "kill-pane" | "killp" => Some(Action::KillPane),
        "next-window" | "next" => Some(Action::NextWindow),
        "previous-window" | "prev" => Some(Action::PrevWindow),
        "copy-mode" => {
            if parts.iter().any(|p| *p == "-u") {
                Some(Action::Command(cmd.to_string()))
            } else {
                Some(Action::CopyMode)
            }
        }
        "paste-buffer" | "pasteb" => Some(Action::Paste),
        "detach-client" | "detach" => Some(Action::Detach),
        "rename-window" | "renamew" => Some(Action::RenameWindow),
        "choose-window" | "choose-tree" => Some(Action::WindowChooser),
        "choose-session" => Some(Action::SessionChooser),
        "resize-pane" | "resizep" if parts.iter().any(|p| *p == "-Z") => Some(Action::ZoomPane),
        "zoom-pane" => Some(Action::ZoomPane),
        "select-pane" | "selectp" => {
            if parts.iter().any(|p| *p == "-Z") {
                Some(Action::Command(cmd.to_string()))
            } else if parts.iter().any(|p| *p == "-U") {
                Some(Action::MoveFocus(FocusDir::Up))
            } else if parts.iter().any(|p| *p == "-D") {
                Some(Action::MoveFocus(FocusDir::Down))
            } else if parts.iter().any(|p| *p == "-L") {
                Some(Action::MoveFocus(FocusDir::Left))
            } else if parts.iter().any(|p| *p == "-R") {
                Some(Action::MoveFocus(FocusDir::Right))
            } else {
                Some(Action::Command(cmd.to_string()))
            }
        }
        "last-window" | "last" => Some(Action::Command("last-window".to_string())),
        "last-pane" | "lastp" => Some(Action::Command("last-pane".to_string())),
        "swap-pane" | "swapp" => Some(Action::Command(cmd.to_string())),
        "resize-pane" | "resizep" => Some(Action::Command(cmd.to_string())),
        "rotate-window" | "rotatew" => Some(Action::Command(cmd.to_string())),
        "break-pane" | "breakp" => Some(Action::Command(cmd.to_string())),
        "respawn-pane" | "respawnp" => Some(Action::Command(cmd.to_string())),
        "respawn-window" | "respawnw" => Some(Action::Command(cmd.to_string())),
        "kill-window" | "killw" => Some(Action::Command(cmd.to_string())),
        "kill-session" | "kill-ses" => Some(Action::Command(cmd.to_string())),
        "kill-server" => Some(Action::Command(cmd.to_string())),
        "select-window" | "selectw" => Some(Action::Command(cmd.to_string())),
        "toggle-sync" => Some(Action::Command("toggle-sync".to_string())),
        "send-keys" | "send" => Some(Action::Command(cmd.to_string())),
        "send-prefix" => Some(Action::Command(cmd.to_string())),
        "set-option" | "set" | "setw" | "set-window-option" => Some(Action::Command(cmd.to_string())),
        "show-options" | "show" | "show-window-options" | "showw"
        | "show-option" | "show-window-option" => Some(Action::Command(cmd.to_string())),
        "source-file" | "source" => Some(Action::Command(cmd.to_string())),
        "select-layout" | "selectl" => Some(Action::Command(cmd.to_string())),
        "next-layout" | "nextl" => Some(Action::Command("next-layout".to_string())),
        "previous-layout" | "prevl" => Some(Action::Command("previous-layout".to_string())),
        "confirm-before" | "confirm" => Some(Action::Command(cmd.to_string())),
        "display-menu" | "menu" => Some(Action::Command(cmd.to_string())),
        "display-popup" | "popup" => Some(Action::Command(cmd.to_string())),
        "display-message" | "display" => Some(Action::Command(cmd.to_string())),
        "pipe-pane" | "pipep" => Some(Action::Command(cmd.to_string())),
        "rename-session" | "rename" => Some(Action::Command(cmd.to_string())),
        "clear-history" | "clearhist" => Some(Action::Command("clear-history".to_string())),
        "set-buffer" | "setb" => Some(Action::Command(cmd.to_string())),
        "delete-buffer" | "deleteb" => Some(Action::Command("delete-buffer".to_string())),
        "list-buffers" | "lsb" => Some(Action::Command(cmd.to_string())),
        "show-buffer" | "showb" => Some(Action::Command(cmd.to_string())),
        "choose-buffer" | "chooseb" => Some(Action::Command(cmd.to_string())),
        "load-buffer" | "loadb" => Some(Action::Command(cmd.to_string())),
        "save-buffer" | "saveb" => Some(Action::Command(cmd.to_string())),
        "capture-pane" | "capturep" => Some(Action::Command(cmd.to_string())),
        "list-windows" | "lsw" => Some(Action::Command(cmd.to_string())),
        "list-panes" | "lsp" => Some(Action::Command(cmd.to_string())),
        "list-clients" | "lsc" => Some(Action::Command(cmd.to_string())),
        "list-commands" | "lscm" => Some(Action::Command(cmd.to_string())),
        "list-keys" | "lsk" => Some(Action::Command(cmd.to_string())),
        "list-sessions" | "ls" => Some(Action::Command(cmd.to_string())),
        "show-hooks" => Some(Action::Command(cmd.to_string())),
        "show-messages" | "showmsgs" => Some(Action::Command(cmd.to_string())),
        "clock-mode" => Some(Action::Command(cmd.to_string())),
        "command-prompt" => Some(Action::Command(cmd.to_string())),
        "has-session" | "has" => Some(Action::Command(cmd.to_string())),
        "move-window" | "movew" => Some(Action::Command(cmd.to_string())),
        "swap-window" | "swapw" => Some(Action::Command(cmd.to_string())),
        "link-window" | "linkw" => Some(Action::Command(cmd.to_string())),
        "unlink-window" | "unlinkw" => Some(Action::Command(cmd.to_string())),
        "find-window" | "findw" => Some(Action::Command(cmd.to_string())),
        "move-pane" | "movep" => Some(Action::Command(cmd.to_string())),
        "join-pane" | "joinp" => Some(Action::Command(cmd.to_string())),
        "resize-window" | "resizew" => Some(Action::Command(cmd.to_string())),
        "run-shell" | "run" => Some(Action::Command(cmd.to_string())),
        "if-shell" | "if" => Some(Action::Command(cmd.to_string())),
        "wait-for" | "wait" => Some(Action::Command(cmd.to_string())),
        "set-environment" | "setenv" => Some(Action::Command(cmd.to_string())),
        "show-environment" | "showenv" => Some(Action::Command(cmd.to_string())),
        "set-hook" => Some(Action::Command(cmd.to_string())),
        "bind-key" | "bind" => Some(Action::Command(cmd.to_string())),
        "unbind-key" | "unbind" => Some(Action::Command(cmd.to_string())),
        "attach-session" | "attach" | "a" | "at" => Some(Action::Command(cmd.to_string())),
        "new-session" | "new" => Some(Action::Command(cmd.to_string())),
        "server-info" | "info" => Some(Action::Command(cmd.to_string())),
        "start-server" | "start" => Some(Action::Command(cmd.to_string())),
        "lock-client" | "lockc" => Some(Action::Command(cmd.to_string())),
        "lock-server" | "lock" => Some(Action::Command(cmd.to_string())),
        "lock-session" | "locks" => Some(Action::Command(cmd.to_string())),
        "refresh-client" | "refresh" => Some(Action::Command(cmd.to_string())),
        "suspend-client" | "suspendc" => Some(Action::Command(cmd.to_string())),
        "switch-client" | "switchc" => {
            // Check for -T flag to switch key table
            if let Some(pos) = parts.iter().position(|p| *p == "-T") {
                if let Some(table) = parts.get(pos + 1) {
                    Some(Action::SwitchTable(table.to_string()))
                } else {
                    Some(Action::Command(cmd.to_string()))
                }
            } else {
                Some(Action::Command(cmd.to_string()))
            }
        }
        _ => Some(Action::Command(cmd.to_string()))
    }
}

/// Format an Action back to a command string
pub fn format_action(action: &Action) -> String {
    match action {
        Action::DisplayPanes => "display-panes".to_string(),
        Action::NewWindow => "new-window".to_string(),
        Action::SplitHorizontal => "split-window -h".to_string(),
        Action::SplitVertical => "split-window -v".to_string(),
        Action::KillPane => "kill-pane".to_string(),
        Action::NextWindow => "next-window".to_string(),
        Action::PrevWindow => "previous-window".to_string(),
        Action::CopyMode => "copy-mode".to_string(),
        Action::Paste => "paste-buffer".to_string(),
        Action::Detach => "detach-client".to_string(),
        Action::RenameWindow => "rename-window".to_string(),
        Action::WindowChooser => "choose-window".to_string(),
        Action::SessionChooser => "choose-session".to_string(),
        Action::ZoomPane => "resize-pane -Z".to_string(),
        Action::MoveFocus(dir) => {
            let flag = match dir {
                FocusDir::Up => "-U",
                FocusDir::Down => "-D",
                FocusDir::Left => "-L",
                FocusDir::Right => "-R",
            };
            format!("select-pane {}", flag)
        }
        Action::Command(cmd) => cmd.clone(),
        Action::CommandChain(cmds) => cmds.join(" \\; "),
        Action::SwitchTable(table) => format!("switch-client -T {}", table),
    }
}

/// Parse a command line string, respecting quoted arguments
pub fn parse_command_line(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_double_quotes = false;
    let mut in_single_quotes = false;
    // Track whether the current token contained an explicit quote, so an
    // intentionally-empty quoted argument (e.g. `select-pane -T ""`) is
    // preserved as an empty string rather than dropped. Without this, an empty
    // `""`/`''` token is silently discarded and a following flag value is lost
    // (this was the root cause of #177: `select-pane -T ""` never cleared the
    // pane title because the empty value never reached SetPaneTitle).
    let mut had_quote = false;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if in_single_quotes {
            // Inside single quotes: everything is literal (no escape processing)
            if c == '\'' {
                in_single_quotes = false;
            } else {
                current.push(c);
            }
        } else if c == '\\' && in_double_quotes {
            // Inside double quotes, recognise two escape sequences:
            //   \"  → literal double-quote
            //   \\  → literal backslash
            // All other backslashes are kept literal because psmux is a
            // Windows-native tool where backslash is the normal path
            // separator (e.g. "C:\Program Files\Git\bin\bash.exe").
            if i + 1 < chars.len() && chars[i + 1] == '"' {
                current.push('"');
                i += 1; // skip the quote
            } else if i + 1 < chars.len() && chars[i + 1] == '\\' {
                current.push('\\');
                i += 1; // skip the second backslash
            } else {
                current.push(c); // literal backslash
            }
        } else if c == '"' {
            in_double_quotes = !in_double_quotes;
            had_quote = true;
        } else if c == '\'' && !in_double_quotes {
            in_single_quotes = true;
            had_quote = true;
        } else if c.is_whitespace() && !in_double_quotes {
            if !current.is_empty() || had_quote {
                args.push(current.clone());
                current.clear();
            }
            had_quote = false;
        } else {
            current.push(c);
        }
        i += 1;
    }

    if !current.is_empty() || had_quote {
        args.push(current);
    }

    args
}

/// Parse a menu definition string into a Menu structure
pub fn parse_menu_definition(def: &str, x: Option<i16>, y: Option<i16>) -> Menu {
    let mut menu = Menu {
        title: String::new(),
        items: Vec::new(),
        selected: 0,
        x,
        y,
    };
    
    let parts: Vec<&str> = def.split_whitespace().collect();
    if parts.is_empty() {
        return menu;
    }
    
    let mut i = 0;
    while i < parts.len() {
        if parts[i] == "-T" {
            if let Some(title) = parts.get(i + 1) {
                menu.title = title.trim_matches('"').to_string();
                i += 2;
                continue;
            }
        }
        
        if let Some(name) = parts.get(i) {
            let name = name.trim_matches('"').to_string();
            if name.is_empty() || name == "-" {
                menu.items.push(MenuItem {
                    name: String::new(),
                    key: None,
                    command: String::new(),
                    is_separator: true,
                });
                i += 1;
            } else {
                let key = parts.get(i + 1).map(|k| k.trim_matches('"').chars().next()).flatten();
                let command = parts.get(i + 2).map(|c| c.trim_matches('"').to_string()).unwrap_or_default();
                menu.items.push(MenuItem {
                    name,
                    key,
                    command,
                    is_separator: false,
                });
                i += 3;
            }
        } else {
            break;
        }
    }
    
    if menu.items.is_empty() && !def.is_empty() {
        menu.title = "Menu".to_string();
        menu.items.push(MenuItem {
            name: def.to_string(),
            key: Some('1'),
            command: def.to_string(),
            is_separator: false,
        });
    }
    
    menu
}

/// Ensure a run-shell command uses -b (background) so it does not
/// set "running: ..." status messages or create output popups.
pub fn ensure_background(cmd: &str) -> String {
    let t = cmd.trim_start();
    let prefix = if t.starts_with("run-shell ") {
        Some("run-shell")
    } else if t.starts_with("run ") {
        Some("run")
    } else {
        None
    };
    if let Some(p) = prefix {
        let rest = t[p.len()..].trim_start();
        if !rest.starts_with("-b") {
            return format!("{} -b {}", p, rest);
        }
    }
    cmd.to_string()
}

/// Fire hooks for a given event.
/// All run-shell commands from hooks are forced into background mode
/// to avoid "running: ..." status bar noise and output popups.
pub fn fire_hooks(app: &mut AppState, event: &str) {
    if let Some(commands) = app.hooks.get(event).cloned() {
        for cmd in commands {
            let bg_cmd = ensure_background(&cmd);
            let _ = execute_command_string(app, &bg_cmd);
        }
    }
}

/// Execute an Action (from key bindings)
pub fn execute_action(app: &mut AppState, action: &Action) -> io::Result<bool> {
    match action {
        Action::DisplayPanes => {
            let win = &app.windows[app.active_idx];
            let mut rects: Vec<(Vec<usize>, ratatui::prelude::Rect)> = Vec::new();
            compute_rects(&win.root, app.last_window_area, &mut rects);
            app.display_map.clear();
            for (i, (path, _)) in rects.into_iter().enumerate() {
                if i >= 10 { break; }
                let digit = (i + app.pane_base_index) % 10;
                app.display_map.push((digit, path));
            }
            app.mode = Mode::PaneChooser { opened_at: Instant::now() };
        }
        Action::MoveFocus(dir) => {
            let d = *dir;
            switch_with_copy_save(app, |app| { crate::input::move_focus(app, d); });
        }
        Action::NewWindow => {
            let pty_system = portable_pty::native_pty_system();
            create_window(&*pty_system, app, None, None, false)?;
        }
        Action::SplitHorizontal => {
            split_active(app, LayoutKind::Horizontal)?;
        }
        Action::SplitVertical => {
            split_active(app, LayoutKind::Vertical)?;
        }
        Action::KillPane => {
            kill_active_pane(app)?;
        }
        Action::NextWindow => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + 1) % app.windows.len();
                });
            }
        }
        Action::PrevWindow => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                });
            }
        }
        Action::CopyMode => {
            enter_copy_mode(app);
        }
        Action::Paste => {
            paste_latest(app)?;
        }
        Action::Detach => {
            return Ok(true);
        }
        Action::RenameWindow => {
            app.mode = Mode::RenamePrompt { input: String::new() };
        }
        Action::WindowChooser | Action::SessionChooser => {
            let tree = build_choose_tree(app);
            let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
            app.mode = Mode::WindowChooser { selected, tree };
        }
        Action::ZoomPane => {
            toggle_zoom(app);
        }
        Action::Command(cmd) => {
            execute_command_string(app, cmd)?;
        }
        Action::CommandChain(cmds) => {
            for cmd in cmds {
                execute_command_string(app, cmd)?;
            }
        }
        Action::SwitchTable(table) => {
            app.current_key_table = Some(table.clone());
        }
    }
    Ok(false)
}

pub fn execute_command_prompt(app: &mut AppState) -> io::Result<()> {
    let cmdline = match &app.mode { Mode::CommandPrompt { input, .. } => input.clone(), _ => String::new() };
    app.mode = Mode::Passthrough;

    // Split on \; or ; to support command chaining (issue #192)
    let sub_commands = crate::config::split_chained_commands_pub(&cmdline);
    if sub_commands.len() > 1 {
        for sub in &sub_commands {
            execute_command_string(app, sub)?;
        }
        return Ok(());
    }

    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    if parts.is_empty() { return Ok(()); }
    match parts[0] {
        // Commands that need local (embedded-mode) handling.
        // In server mode the client sends these via TCP directly, so
        // execute_command_prompt() is only reached in embedded mode.
        "new-window" | "neww" => {
            let pty_system = portable_pty::native_pty_system();
            create_window(&*pty_system, app, None, None, false)?;
        }
        "split-window" | "splitw" | "split-pane" | "splitp" => {
            let kind = if parts.iter().any(|p| *p == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
            let zoom_after_split = parts.iter().any(|p| *p == "-Z");
            if zoom_after_split {
                unzoom_if_zoomed(app);
            }
            split_active(app, kind)?;
            if zoom_after_split {
                toggle_zoom(app);
            }
        }
        "kill-pane" | "killp" => { kill_active_pane(app)?; }
        "capture-pane" | "capturep" => { capture_active_pane(app)?; }
        "save-buffer" | "saveb" => { if let Some(file) = parts.get(1) { save_latest_buffer(app, file)?; } }
        "list-sessions" | "ls" => { println!("default"); }
        "attach-session" | "attach" | "a" | "at" => { }
        // Everything else delegates to execute_command_string() which
        // handles 80+ commands (list-*, show-*, kill-*, display-*,
        // select-*, rename-*, set-*, bind-*, etc.) and forwards
        // anything it doesn't recognise to the server via TCP.
        _ => {
            execute_command_string(app, &cmdline)?;
        }
    }
    Ok(())
}

/// Execute a command string (used by menus, hooks, confirm dialogs, etc.)
pub fn execute_command_string(app: &mut AppState, cmd: &str) -> io::Result<()> {
    // Split on \; or ; to support command chaining (issue #192)
    let sub_commands = crate::config::split_chained_commands_pub(cmd);
    if sub_commands.len() > 1 {
        for sub in &sub_commands {
            execute_command_string_single(app, sub)?;
        }
        return Ok(());
    }
    execute_command_string_single(app, cmd)
}

fn execute_command_string_single(app: &mut AppState, cmd: &str) -> io::Result<()> {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.is_empty() { return Ok(()); }
    
    match parts[0] {
        "new-window" | "neww" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "split-window" | "splitw" | "split-pane" | "splitp" => {
            if let Some(port) = app.control_port {
                // Forward the full command string to preserve -c, -d, -p etc. flags
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else if parts.iter().any(|p| *p == "-Z") {
                let kind = if parts.iter().any(|p| *p == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                unzoom_if_zoomed(app);
                split_active(app, kind)?;
                toggle_zoom(app);
            }
        }
        "kill-pane" => {
            let _ = kill_active_pane(app);
        }
        "kill-window" | "killw" => {
            // Resolve an explicit -t here; an unresolvable target is an error,
            // not a fallback to the active window (tmux: "can't find window").
            let mut removed_pos = Some(app.active_idx);
            if let Some(t_pos) = parts.iter().position(|p| *p == "-t") {
                if let Some(t) = parts.get(t_pos + 1) {
                    let pt = crate::cli::parse_target(t);
                    removed_pos = if let Some(w) = pt.window {
                        if pt.window_is_id {
                            app.windows.iter().position(|x| x.id == w)
                        } else {
                            app.win_pos(w)
                        }
                    } else if let Some(ref n) = pt.window_name {
                        app.windows.iter().position(|x| x.name == *n)
                    } else {
                        // Bare session target: the active window, like tmux.
                        Some(app.active_idx)
                    };
                    if removed_pos.is_none() {
                        app.status_message = Some((
                            format!("can't find window: {}", t),
                            std::time::Instant::now(),
                            None,
                        ));
                    }
                }
            }
            if let Some(pos) = removed_pos {
                if app.windows.len() > 1 && pos < app.windows.len() {
                    let mut win = app.windows.remove(pos);
                    kill_all_children(&mut win.root);
                    app.on_window_removed(pos);
                    if app.active_idx > pos {
                        app.active_idx -= 1;
                    }
                    if app.active_idx >= app.windows.len() {
                        app.active_idx = app.windows.len() - 1;
                    }
                }
            }
        }
        "next-window" | "next" => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + 1) % app.windows.len();
                });
            }
        }
        "previous-window" | "prev" => {
            if !app.windows.is_empty() {
                switch_with_copy_save(app, |app| {
                    app.last_window_idx = app.active_idx;
                    app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len();
                });
            }
        }
        "last-window" | "last" => {
            if app.last_window_idx < app.windows.len() {
                switch_with_copy_save(app, |app| {
                    let tmp = app.active_idx;
                    app.active_idx = app.last_window_idx;
                    app.last_window_idx = tmp;
                });
            }
        }
        "select-window" | "selectw" => {
            if let Some(t_pos) = parts.iter().position(|p| *p == "-t") {
                if let Some(t) = parts.get(t_pos + 1) {
                    if let Some(idx) = parse_window_target(t) {
                        if let Some(internal_idx) = app.win_pos(idx) {
                            switch_with_copy_save(app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                        }
                    }
                }
            }
        }
        "select-pane" | "selectp" => {
            // Save/restore copy mode across pane switches (tmux parity #43)
            let is_last = parts.iter().any(|p| *p == "-l");
            if is_last {
                switch_with_copy_save(app, |app| {
                    let win = &mut app.windows[app.active_idx];
                    if !app.last_pane_path.is_empty() {
                        let tmp = win.active_path.clone();
                        win.active_path = app.last_pane_path.clone();
                        app.last_pane_path = tmp;
                    }
                });
                return Ok(());
            }
            let keep_zoom = parts.iter().any(|p| *p == "-Z");
            let dir = if parts.iter().any(|p| *p == "-U") { FocusDir::Up }
                else if parts.iter().any(|p| *p == "-D") { FocusDir::Down }
                else if parts.iter().any(|p| *p == "-L") { FocusDir::Left }
                else if parts.iter().any(|p| *p == "-R") { FocusDir::Right }
                else { return Ok(()); };
            if keep_zoom {
                switch_with_copy_save(app, |app| {
                    let win = &app.windows[app.active_idx];
                    app.last_pane_path = win.active_path.clone();
                    crate::input::move_focus_preserving_zoom(app, dir);
                });
            } else if app.windows[app.active_idx].zoom_saved.is_some() {
                // Zoom-aware directional navigation (tmux parity #134):
                // If zoomed, check if there's a direct neighbor OR a wrap target.
                // If yes: cancel zoom and navigate to it.
                // If no (single-pane window): no-op — stay zoomed.
                // Temporarily unzoom to compute real geometry
                let saved = app.windows[app.active_idx].zoom_saved.take();
                if let Some(ref s) = saved {
                    let win = &mut app.windows[app.active_idx];
                    for (p, sz) in s.iter() {
                        if let Some(Node::Split { sizes, .. }) = crate::tree::get_split_mut(&mut win.root, p) { *sizes = sz.clone(); }
                    }
                }
                crate::tree::resize_all_panes(app);
                // Find direct neighbor only (no wrap when zoomed — tmux parity)
                let win = &app.windows[app.active_idx];
                let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
                crate::tree::compute_rects(&win.root, app.last_window_area, &mut rects);
                let active_idx = rects.iter().position(|(path, _)| *path == win.active_path);
                let has_target = if let Some(ai) = active_idx {
                    let (_, arect) = &rects[ai];
                    crate::input::find_best_pane_in_direction(&rects, ai, arect, dir, &[], &[])
                        .is_some()
                } else { false };
                if has_target {
                    // Cancel zoom (already unzoomed) and navigate
                    switch_with_copy_save(app, |app| {
                        let win = &app.windows[app.active_idx];
                        app.last_pane_path = win.active_path.clone();
                        crate::input::move_focus(app, dir);
                    });
                } else {
                    // No target (single-pane) — re-zoom (restore saved zoom state)
                    if let Some(s) = saved {
                        let win = &mut app.windows[app.active_idx];
                        for (p, sz) in s.iter() {
                            if let Some(Node::Split { sizes, .. }) = crate::tree::get_split_mut(&mut win.root, p) { *sizes = sz.clone(); }
                        }
                        win.zoom_saved = Some(s);
                    }
                    crate::tree::resize_all_panes(app);
                }
            } else {
                switch_with_copy_save(app, |app| {
                    let win = &app.windows[app.active_idx];
                    app.last_pane_path = win.active_path.clone();
                    crate::input::move_focus(app, dir);
                });
            }
        }
        "last-pane" | "lastp" => {
            switch_with_copy_save(app, |app| {
                let win = &mut app.windows[app.active_idx];
                if !app.last_pane_path.is_empty() {
                    let tmp = win.active_path.clone();
                    win.active_path = app.last_pane_path.clone();
                    app.last_pane_path = tmp;
                }
            });
        }
        "rename-window" | "renamew" => {
            if let Some(name) = parts.get(1) {
                // tmux parity (#552): the argument is a format, expanded
                // against the window being renamed (cmd-rename-window.c uses
                // format_single_from_target), so `rename-window
                // '#{s/^XX //:window_name}'` transforms the current name.
                let name = crate::format::expand_format(name, app);
                if app.active_idx < app.windows.len() {
                    let win = &mut app.windows[app.active_idx];
                    win.name = name.clone();
                    win.manual_rename = true;
                }
                // Forward to server so external queries (display-message, list-windows) see the new name
                if let Some(port) = app.control_port {
                    let _ = send_control_to_port(port, &format!("rename-window {}\n", crate::util::quote_arg(&name)), &app.session_key);
                }
            }
        }
        "list-windows" | "lsw" => {
            let output = generate_list_windows(app);
            show_output_popup(app, "list-windows", output);
        }
        "list-panes" | "lsp" => {
            let output = generate_list_panes(app);
            show_output_popup(app, "list-panes", output);
        }
        "list-clients" | "lsc" => {
            let output = generate_list_clients(app);
            show_output_popup(app, "list-clients", output);
        }
        "list-commands" | "lscm" => {
            let output = generate_list_commands();
            show_output_popup(app, "list-commands", output);
        }
        "show-hooks" => {
            let output = generate_show_hooks(app);
            show_output_popup(app, "show-hooks", output);
        }
        "zoom-pane" | "zoom" | "resizep -Z" => {
            toggle_zoom(app);
        }
        "copy-mode" => {
            if parts.iter().any(|a| *a == "-u") {
                if app.scroll_enter_copy_mode {
                    enter_copy_mode(app);
                    let half = app.windows.get(app.active_idx)
                        .and_then(|w| crate::tree::active_pane(&w.root, &w.active_path))
                        .map(|p| p.last_rows as usize).unwrap_or(20);
                    scroll_copy_up(app, half);
                } else {
                    // scroll-enter-copy-mode off: forward PageUp to PTY (#284)
                    if let Some(win) = app.windows.get_mut(app.active_idx) {
                        if let Some(pane) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = pane.writer.write_all(b"\x1b[5~");
                        }
                    }
                }
            } else {
                enter_copy_mode(app);
            }
        }
        "display-panes" | "displayp" => {
            let win = &app.windows[app.active_idx];
            let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
            compute_rects(&win.root, app.last_window_area, &mut rects);
            app.display_map.clear();
            for (i, (path, _)) in rects.into_iter().enumerate() {
                if i >= 10 { break; }
                let digit = (i + app.pane_base_index) % 10;
                app.display_map.push((digit, path));
            }
            app.mode = Mode::PaneChooser { opened_at: Instant::now() };
        }
        "confirm-before" | "confirm" => {
            let rest = parts[1..].join(" ");
            app.mode = Mode::ConfirmMode {
                prompt: format!("Run '{}'?", rest),
                command: rest,
                input: String::new(),
            };
        }
        "display-menu" | "menu" => {
            let rest = parts[1..].join(" ");
            let menu = parse_menu_definition(&rest, None, None);
            if !menu.items.is_empty() {
                app.mode = Mode::MenuMode { menu };
            }
        }
        "display-popup" | "popup" => {
            // Re-tokenize with a quote-aware parser (#470). The raw `parts` above
            // come from `split_whitespace()`, which does NOT strip shell quotes.
            // When this command arrives from a display-menu item, the inner popup
            // command is still quoted (e.g. `display-popup -E 'lazygit'`), so a
            // naive split leaves the literal quotes on the command (`'lazygit'`),
            // which is not a runnable executable. The popup shell then exits
            // immediately and close-on-exit tears the popup back down before the
            // user ever sees it. parse_command_line() strips the quotes so the
            // command matches what the CLI/TCP path (connection.rs) already does.
            let qparts = parse_command_line(cmd);
            // Parse -w width, -h height, -E close-on-exit, -d start-dir flags
            let mut width_spec = "80".to_string();
            let mut height_spec = "24".to_string();
            let mut start_dir: Option<String> = None;
            let close_on_exit = qparts.iter().any(|p| p == "-E");
            let mut skip_indices = std::collections::HashSet::new();
            skip_indices.insert(0); // skip the command name itself
            let mut i = 1;
            while i < qparts.len() {
                match qparts[i].as_str() {
                    "-w" => { if let Some(v) = qparts.get(i + 1) { width_spec = v.to_string(); skip_indices.insert(i); skip_indices.insert(i + 1); i += 1; } }
                    "-h" => { if let Some(v) = qparts.get(i + 1) { height_spec = v.to_string(); skip_indices.insert(i); skip_indices.insert(i + 1); i += 1; } }
                    "-d" | "-c" => { if let Some(v) = qparts.get(i + 1) { start_dir = Some(v.to_string()); skip_indices.insert(i); skip_indices.insert(i + 1); i += 1; } }
                    "-E" | "-K" => { skip_indices.insert(i); }
                    _ => {}
                }
                i += 1;
            }
            // Resolve percentage dimensions against terminal size (#154)
            let (term_w, term_h) = crossterm::terminal::size().unwrap_or((120, 40));
            let width = parse_popup_dim_local(&width_spec, term_w, 80);
            let height = parse_popup_dim_local(&height_spec, term_h, 24);
            // Collect remaining args as the command (quotes already stripped)
            let rest: String = qparts.iter().enumerate()
                .filter(|(idx, _)| !skip_indices.contains(idx))
                .map(|(_, a)| a.as_str())
                .collect::<Vec<&str>>()
                .join(" ");

            // Spawn popup as a real Pane via the popup module
            let pane_result = if !rest.is_empty() {
                crate::popup::create_popup_pane(
                    &rest,
                    start_dir.as_deref(),
                    height.saturating_sub(2),
                    width.saturating_sub(2),
                    app.next_pane_id,
                    "1", // session name not available in local mode
                    &app.environment,
                    app.host_colors.as_ref(),
                )
            } else { None };

            app.mode = Mode::PopupMode {
                command: rest,
                output: String::new(),
                process: None,
                width,
                height,
                close_on_exit,
                popup_pane: pane_result,
                scroll_offset: 0,
            };
        }
        "resize-pane" | "resizep" => {
            if parts.iter().any(|p| *p == "-Z") {
                toggle_zoom(app);
            } else if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                // Local resize
                let amount = parts.windows(2).find(|w| w[0] == "-x" || w[0] == "-y")
                    .and_then(|w| w[1].parse::<i16>().ok());
                if parts.iter().any(|p| *p == "-U" || *p == "-D") {
                    let amt = amount.unwrap_or(1);
                    let adj = if parts.iter().any(|p| *p == "-U") { -amt } else { amt };
                    crate::window_ops::resize_pane_vertical(app, adj);
                } else if parts.iter().any(|p| *p == "-L" || *p == "-R") {
                    let amt = amount.unwrap_or(1);
                    let adj = if parts.iter().any(|p| *p == "-L") { -amt } else { amt };
                    crate::window_ops::resize_pane_horizontal(app, adj);
                }
            }
        }
        "swap-pane" | "swapp" => {
            // `-s <src> -t <dst>` swaps the two explicit panes (#442).
            // `-t <target>` alone swaps the active pane with an explicit target
            // pane (e.g. `swap-pane -t :.4` or `swap-pane -t %2`).  Without -t,
            // fall back to the directional -U/-D/-L/-R swap.
            let source = parts.iter().position(|p| *p == "-s")
                .and_then(|i| parts.get(i + 1)).map(|s| s.to_string());
            let target = parts.iter().position(|p| *p == "-t")
                .and_then(|i| parts.get(i + 1)).map(|s| s.to_string());
            let detach = parts.iter().any(|p| *p == "-d");
            if let (Some(src), Some(tgt)) = (source.as_ref(), target.as_ref()) {
                if let Some(port) = app.control_port {
                    let d = if detach { " -d" } else { "" };
                    let _ = send_control_to_port(port, &format!("swap-pane{} -s {} -t {}\n", d, src, tgt), &app.session_key);
                } else {
                    match (resolve_swap_pane_target_path(app, src), resolve_swap_pane_target_path(app, tgt)) {
                        (Some(sp), Some(dp)) => { crate::window_ops::swap_pane_between(app, sp, dp, detach); }
                        _ => { app.status_message = Some(("swap-pane: can't find pane".to_string(), Instant::now(), None)); }
                    }
                }
            } else if let Some(tgt) = target {
                if let Some(port) = app.control_port {
                    let _ = send_control_to_port(port, &format!("swap-pane -t {}\n", tgt), &app.session_key);
                } else {
                    let path = resolve_swap_pane_target_path(app, &tgt);
                    if let Some(path) = path {
                        crate::window_ops::swap_pane_with_path(app, path);
                    } else {
                        app.status_message = Some((format!("swap-pane: can't find pane: {}", tgt), Instant::now(), None));
                    }
                }
            } else if let Some(port) = app.control_port {
                let dir = if parts.iter().any(|p| *p == "-U") { "-U" }
                    else if parts.iter().any(|p| *p == "-L") { "-L" }
                    else if parts.iter().any(|p| *p == "-R") { "-R" }
                    else { "-D" };
                let _ = send_control_to_port(port, &format!("swap-pane {}\n", dir), &app.session_key);
            } else {
                let dir = if parts.iter().any(|p| *p == "-L") { FocusDir::Left }
                    else if parts.iter().any(|p| *p == "-R") { FocusDir::Right }
                    else if parts.iter().any(|p| *p == "-U") { FocusDir::Up }
                    else { FocusDir::Down };
                crate::window_ops::swap_pane(app, dir);
            }
        }
        "rotate-window" | "rotatew" => {
            if let Some(port) = app.control_port {
                let flag = if parts.iter().any(|p| *p == "-D") { "-D" } else { "" };
                let _ = send_control_to_port(port, &format!("rotate-window {}\n", flag), &app.session_key);
            } else {
                crate::window_ops::rotate_panes(app, !parts.iter().any(|p| *p == "-D"));
            }
        }
        "break-pane" | "breakp" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "break-pane\n", &app.session_key);
            } else {
                crate::window_ops::break_pane_to_window(app);
            }
        }
        "respawn-pane" | "respawnp" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let empty = parts.iter().any(|p| *p == "-E");
                let kill = parts.iter().any(|p| *p == "-k") || empty;
                // Honor `-- <shell-command>` (issue #399).
                let command = parts.iter().position(|p| *p == "--")
                    .map(|i| parts[i + 1..].join(" "))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                crate::window_ops::respawn_active_pane(app, None, None, kill, command.as_deref(), empty)?;
            }
        }
        "toggle-sync" => {
            app.sync_input = !app.sync_input;
        }
        "set-option" | "set" | "set-window-option" | "setw" => {
            // Always apply locally first (fix #179: TCP server drops these)
            crate::config::parse_config_line(app, cmd);
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "bind-key" | "bind" => {
            // Always apply locally first (fix #179: TCP server drops these)
            crate::config::parse_config_line(app, cmd);
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "unbind-key" | "unbind" => {
            // Always apply locally first (fix #179: TCP server drops these)
            crate::config::parse_config_line(app, cmd);
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "source-file" | "source" => {
            // Always apply locally first for immediate visual feedback,
            // then forward to server for authoritative state update.
            if let Some(path) = parts.get(1) {
                crate::config::source_file(app, path);
            }
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "send-keys" | "send" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                // Local: write key text directly to active pane
                let literal = parts.iter().any(|p| *p == "-l");
                let key_parts: Vec<&str> = parts[1..].iter().filter(|p| !p.starts_with('-')).copied().collect();
                if !key_parts.is_empty() {
                    if literal {
                        let text = key_parts.join(" ");
                        if let Some(win) = app.windows.get_mut(app.active_idx) {
                            if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                                let _ = p.writer.write_all(text.as_bytes());
                                let _ = p.writer.flush();
                            }
                        }
                    } else {
                        for key in &key_parts {
                            let key_upper = key.to_uppercase();
                            let expanded = match key_upper.as_str() {
                                "ENTER" => "\r".to_string(),
                                "TAB" => "\t".to_string(),
                                "BTAB" | "BACKTAB" => "\x1b[Z".to_string(),
                                "ESCAPE" | "ESC" => "\x1b".to_string(),
                                "SPACE" => " ".to_string(),
                                "BSPACE" | "BACKSPACE" => "\x7f".to_string(),
                                "UP" => "\x1b[A".to_string(),
                                "DOWN" => "\x1b[B".to_string(),
                                "RIGHT" => "\x1b[C".to_string(),
                                "LEFT" => "\x1b[D".to_string(),
                                "HOME" => "\x1b[H".to_string(),
                                "END" => "\x1b[F".to_string(),
                                "PAGEUP" | "PPAGE" => "\x1b[5~".to_string(),
                                "PAGEDOWN" | "NPAGE" => "\x1b[6~".to_string(),
                                "DELETE" | "DC" => "\x1b[3~".to_string(),
                                "INSERT" | "IC" => "\x1b[2~".to_string(),
                                "F1" => "\x1bOP".to_string(),
                                "F2" => "\x1bOQ".to_string(),
                                "F3" => "\x1bOR".to_string(),
                                "F4" => "\x1bOS".to_string(),
                                "F5" => "\x1b[15~".to_string(),
                                "F6" => "\x1b[17~".to_string(),
                                "F7" => "\x1b[18~".to_string(),
                                "F8" => "\x1b[19~".to_string(),
                                "F9" => "\x1b[20~".to_string(),
                                "F10" => "\x1b[21~".to_string(),
                                "F11" => "\x1b[23~".to_string(),
                                "F12" => "\x1b[24~".to_string(),
                                s if crate::input::parse_modified_special_key(s).is_some() => {
                                    crate::input::parse_modified_special_key(s).unwrap()
                                }
                                s if s.starts_with("C-M-") || s.starts_with("C-m-") => {
                                    if let Some(c) = key.chars().nth(4) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            format!("\x1b{}", ctrl as char)
                                        } else {
                                            String::new()
                                        }
                                    } else {
                                        key.to_string()
                                    }
                                }
                                // Ctrl+Shift+<punctuation/digit> collapsing to a C0 byte,
                                // e.g. Ctrl+/ arriving as "C-S--" -> 0x1f (^_), matching
                                // Ctrl+_ and tmux (issue #394).  Must precede the generic
                                // C- arm, whose nth(2) would read the 'S' and mis-send Ctrl+S.
                                s if (s.starts_with("C-S-") || s.starts_with("C-s-"))
                                    && s.chars().count() == 5
                                    && s.chars().nth(4).map_or(false, |c| !c.is_ascii_alphabetic()) =>
                                {
                                    if let Some(c) = s.chars().nth(4) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            String::from(ctrl as char)
                                        } else {
                                            String::new()
                                        }
                                    } else {
                                        key.to_string()
                                    }
                                }
                                s if s.starts_with("C-") => {
                                    if let Some(c) = s.chars().nth(2) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            #[cfg(windows)]
                                            if ctrl == 0x03 {
                                                if let Some(win) = app.windows.get_mut(app.active_idx) {
                                                    if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                                                        if p.child_pid.is_none() {
                                                            p.child_pid = crate::platform::mouse_inject::get_child_pid(&*p.child);
                                                        }
                                                        if let Some(pid) = p.child_pid {
                                                            crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                                                        }
                                                    }
                                                }
                                            }
                                            String::from(ctrl as char)
                                        } else {
                                            // Unsupported Ctrl combo — skip silently
                                            // to match tmux reject behavior.
                                            String::new()
                                        }
                                    } else {
                                        key.to_string()
                                    }
                                }
                                s if s.starts_with("M-") => {
                                    if let Some(c) = key.chars().nth(2) {
                                        format!("\x1b{}", c)
                                    } else {
                                        key.to_string()
                                    }
                                }
                                _ => key.to_string(),
                            };
                            if let Some(win) = app.windows.get_mut(app.active_idx) {
                                if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                                    // DECCKM app-cursor mode: SS3, not CSI (see crate::input::write_key_seq).
                                    crate::input::write_key_seq(p, expanded.as_bytes());
                                    let _ = p.writer.flush();
                                }
                            }
                        }
                    }
                }
            }
        }
        "detach-client" | "detach" => {
            // handled by caller to set quit flag
        }
        "rename-session" => {
            if let Some(name) = parts.get(1) {
                app.session_name = name.to_string();
                // Forward to server so external queries see the new session name
                if let Some(port) = app.control_port {
                    let _ = send_control_to_port(port, &format!("rename-session {}\n", crate::util::quote_arg(name)), &app.session_key);
                }
            }
        }
        "select-layout" | "selectl" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let layout = parts.get(1).unwrap_or(&"tiled");
                crate::layout::apply_layout(app, layout);
            }
        }
        "next-layout" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "next-layout\n", &app.session_key);
            } else {
                crate::layout::cycle_layout(app);
            }
        }
        "pipe-pane" | "pipep" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
        "choose-tree" | "choose-window" | "choose-session" => {
            let tree = build_choose_tree(app);
            let selected = tree.iter().position(|e| e.is_current_session && e.is_active_window && !e.is_session_header).unwrap_or(0);
            app.mode = Mode::WindowChooser { selected, tree };
        }
        "command-prompt" => {
            // Support -I initial_text, -p prompt (ignored), -1 (ignored)
            let initial = parts.windows(2).find(|w| w[0] == "-I").map(|w| w[1].to_string()).unwrap_or_default();
            app.command_vi_normal = false;
            app.mode = Mode::CommandPrompt { input: initial.clone(), cursor: initial.len() };
        }
        "paste-buffer" | "pasteb" => {
            paste_latest(app)?;
        }
        "set-buffer" | "setb" => {
            // Parse -b name, -w (clipboard), and extract content
            let mut i = 1;
            let mut buf_name: Option<String> = None;
            let mut content: Option<String> = None;
            let mut propagate_to_clipboard = false;
            while i < parts.len() {
                if parts[i] == "-b" {
                    if let Some(name) = parts.get(i + 1) {
                        buf_name = Some(name.to_string());
                    }
                    i += 2; // skip -b and its value (buffer name)
                } else if parts[i] == "-w" {
                    propagate_to_clipboard = true;
                    i += 1;
                } else if parts[i].starts_with('-') {
                    i += 1; // skip unknown flags
                } else {
                    // Everything from here is content
                    content = Some(parts[i..].join(" "));
                    break;
                }
            }
            if let Some(ref text) = content {
                if propagate_to_clipboard {
                    crate::clipboard::copy_to_system_clipboard(text);
                }
                if let Some(name) = buf_name {
                    app.named_buffers.insert(name, text.clone());
                } else {
                    app.paste_buffers.insert(0, text.clone());
                    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
                }
            }
        }
        "delete-buffer" | "deleteb" => {
            let buf_name: Option<String> = parts.windows(2).find(|w| w[0] == "-b").map(|w| w[1].to_string());
            if let Some(name) = buf_name {
                if let Ok(idx) = name.parse::<usize>() {
                    if idx < app.paste_buffers.len() { app.paste_buffers.remove(idx); }
                } else {
                    app.named_buffers.remove(&name);
                }
            } else {
                if !app.paste_buffers.is_empty() { app.paste_buffers.remove(0); }
            }
        }
        "list-buffers" | "lsb" => {
            let mut output = String::new();
            for (i, buf) in app.paste_buffers.iter().enumerate() {
                output.push_str(&format!("buffer{}: {} bytes: \"{}\"\n", i,
                    buf.len(), &buf.chars().take(50).collect::<String>()));
            }
            // List named buffers
            let mut names: Vec<&String> = app.named_buffers.keys().collect();
            names.sort();
            for name in names {
                let buf = &app.named_buffers[name];
                let preview: String = buf.chars().take(50).collect();
                output.push_str(&format!("{}: {} bytes: \"{}\"\n", name, buf.len(), preview));
            }
            if output.is_empty() { output.push_str("(no buffers)\n"); }
            show_output_popup(app, "list-buffers", output);
        }
        "show-buffer" | "showb" => {
            let buf_name: Option<String> = parts.windows(2).find(|w| w[0] == "-b").map(|w| w[1].to_string());
            if let Some(name) = buf_name {
                if let Ok(idx) = name.parse::<usize>() {
                    if let Some(buf) = app.paste_buffers.get(idx) {
                        show_output_popup(app, "show-buffer", buf.clone());
                    }
                } else if let Some(buf) = app.named_buffers.get(&name) {
                    show_output_popup(app, "show-buffer", buf.clone());
                }
            } else if let Some(buf) = app.paste_buffers.first() {
                show_output_popup(app, "show-buffer", buf.clone());
            }
        }
        "choose-buffer" | "chooseb" => {
            // Enter buffer chooser mode
            app.mode = Mode::BufferChooser { selected: 0 };
        }
        "clear-history" | "clearhist" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "clear-history\n", &app.session_key);
            } else {
                let allow_alt = app.allow_alternate_screen;
                let history_limit = app.history_limit;
                let win = &mut app.windows[app.active_idx];
                if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                    if let Ok(mut parser) = p.term.lock() {
                        let mut fresh = vt100::Parser::new(p.last_rows, p.last_cols, history_limit);
                        fresh.screen_mut().set_allow_alternate_screen(allow_alt);
                        *parser = fresh;
                    }
                }
            }
        }
        "kill-session" | "kill-ses" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "kill-session\n", &app.session_key);
            }
        }
        "kill-server" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "kill-server\n", &app.session_key);
            }
        }
        "has-session" | "has" => {
            // In embedded mode we ARE the session; always succeeds
        }
        "capture-pane" | "capturep" => {
            capture_active_pane(app)?;
        }
        "save-buffer" | "saveb" => {
            if let Some(file) = parts.get(1) {
                save_latest_buffer(app, file)?;
            }
        }
        "load-buffer" | "loadb" => {
            if let Some(path) = parts.get(1) {
                if let Ok(data) = std::fs::read_to_string(path) {
                    app.paste_buffers.insert(0, data);
                    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
                }
            }
        }
        "clock-mode" => {
            app.mode = Mode::ClockMode;
        }
        "list-sessions" | "ls" => {
            // Show all sessions from filesystem
            let output = crate::session::list_session_names().join("\n") + "\n";
            show_output_popup(app, "list-sessions", output);
        }
        "list-keys" | "lsk" => {
            let mut output = String::new();
            for (table_name, binds) in &app.key_tables {
                for bind in binds {
                    let key_str = crate::config::format_key_binding(&bind.key);
                    let cmd_str = format_action(&bind.action);
                    output.push_str(&format!("bind-key -T {} {} {}\n", table_name, key_str, cmd_str));
                }
            }
            if output.is_empty() { output.push_str("(no bindings)\n"); }
            show_output_popup(app, "list-keys", output);
        }
        "show-options" | "show" | "show-window-options" | "showw"
        | "show-option" | "show-window-option" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let output = generate_show_options(app);
                show_output_popup(app, "show-options", output);
            }
        }
        "display-message" | "display" => {
            if let Some(port) = app.control_port {
                // Forward to server; use default format when no args given
                let effective_cmd = if parts.len() <= 1 {
                    format!("display-message \"{}\"", DISPLAY_MESSAGE_DEFAULT_FMT)
                } else {
                    cmd.to_string()
                };
                let _ = send_control_to_port(port, &format!("{}\n", effective_cmd), &app.session_key);
            } else {
                // Local: expand format string and show as status message
                // Parse flags from parts (same as CLI/server):
                //   -d <ms>  per-message display duration
                //   -I <val> consumed (not implemented locally)
                //   -t <val> target (ignored locally)
                //   -p       print to stdout (ignored locally, we show on status bar)
                let mut msg_parts: Vec<&str> = Vec::new();
                let mut duration_ms: Option<u64> = None;
                let mut idx = 1;
                while idx < parts.len() {
                    match parts[idx] {
                        "-d" => {
                            if idx + 1 < parts.len() {
                                duration_ms = parts[idx + 1].parse::<u64>().ok();
                            }
                            idx += 1;
                        }
                        "-I" | "-t" => { idx += 1; }
                        "-p" => {}
                        other => { msg_parts.push(other); }
                    }
                    idx += 1;
                }
                let raw = msg_parts.join(" ");
                let msg = if raw.is_empty() {
                    DISPLAY_MESSAGE_DEFAULT_FMT.to_string()
                } else {
                    raw.trim_matches('"').trim_matches('\'').to_string()
                };
                let expanded = crate::format::expand_format(&msg, app);
                app.status_message = Some((expanded, Instant::now(), duration_ms));
            }
        }
        "show-messages" | "showmsgs" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                show_output_popup(app, "show-messages", "(no messages)\n".to_string());
            }
        }
        "set-environment" | "setenv" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let has_u = parts.iter().any(|p| *p == "-u");
                let non_flag: Vec<&str> = parts[1..].iter().filter(|p| !p.starts_with('-')).copied().collect();
                if has_u {
                    if let Some(key) = non_flag.first() {
                        app.environment.remove(*key);
                        std::env::remove_var(key);
                    }
                } else if non_flag.len() >= 2 {
                    app.environment.insert(non_flag[0].to_string(), non_flag[1].to_string());
                    std::env::set_var(non_flag[0], non_flag[1]);
                } else if non_flag.len() == 1 {
                    app.environment.insert(non_flag[0].to_string(), String::new());
                    std::env::set_var(non_flag[0], "");
                }
            }
        }
        "show-environment" | "showenv" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let mut output = String::new();
                for (key, value) in &app.environment {
                    output.push_str(&format!("{}={}\n", key, value));
                }
                if output.is_empty() { output.push_str("(no environment variables)\n"); }
                show_output_popup(app, "show-environment", output);
            }
        }
        "set-hook" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let has_unset = parts.iter().any(|p| *p == "-u" || *p == "-gu" || *p == "-ug");
                let has_append = parts.iter().any(|p| *p == "-a" || *p == "-ga" || *p == "-ag");
                let non_flag: Vec<&str> = parts[1..].iter().filter(|p| !p.starts_with('-')).copied().collect();
                if has_unset {
                    if let Some(name) = non_flag.first() {
                        app.hooks.remove(*name);
                    }
                } else if non_flag.len() >= 2 {
                    // Extract hook command from the raw cmd string to preserve quoting.
                    // non_flag[0] is the hook name; everything after it in the raw
                    // string is the command (may contain quoted paths with spaces).
                    let hook_name = non_flag[0];
                    let hook_cmd = if let Some(pos) = cmd.find(hook_name) {
                        let after_name = pos + hook_name.len();
                        cmd[after_name..].trim().to_string()
                    } else {
                        non_flag[1..].join(" ")
                    };
                    if has_append {
                        // Dedup identical handlers so a re-sourced config does
                        // not accumulate duplicate hooks that fire every tick
                        // (issue #459); mirrors the replace path's #133 guard.
                        let entry = app.hooks.entry(hook_name.to_string()).or_default();
                        if !entry.contains(&hook_cmd) {
                            entry.push(hook_cmd);
                        }
                    } else {
                        app.hooks.insert(hook_name.to_string(), vec![hook_cmd]);
                    }
                }
            }
        }
        "send-prefix" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "send-prefix\n", &app.session_key);
            } else {
                // Send the prefix key to the active pane as if typed
                let prefix = app.prefix_key;
                let encoded: Vec<u8> = match prefix.0 {
                    crossterm::event::KeyCode::Char(c) if prefix.1.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        vec![(c.to_ascii_lowercase() as u8) & 0x1F]
                    }
                    crossterm::event::KeyCode::Char(c) => format!("{}", c).into_bytes(),
                    _ => vec![],
                };
                if !encoded.is_empty() {
                    if let Some(win) = app.windows.get_mut(app.active_idx) {
                        if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = p.writer.write_all(&encoded);
                            let _ = p.writer.flush();
                        }
                    }
                }
            }
        }
        "if-shell" | "if" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                // Re-parse with quote-aware tokenizer so quoted args are handled
                let parsed = parse_command_line(cmd);
                let format_mode = parsed.iter().any(|p| p == "-F" || p == "-bF" || p == "-Fb");
                let positional: Vec<&str> = parsed[1..].iter()
                    .filter(|p| !p.starts_with('-'))
                    .map(|s| s.as_str())
                    .collect();
                if positional.len() >= 2 {
                    let condition = positional[0];
                    let true_cmd = positional[1];
                    let false_cmd = positional.get(2).copied();
                    let success = if format_mode {
                        let expanded = crate::format::expand_format(condition, app);
                        !expanded.is_empty() && expanded != "0"
                    } else if condition == "true" || condition == "1" {
                        true
                    } else if condition == "false" || condition == "0" {
                        false
                    } else {
                        {
                            let (shell_prog, mut shell_args) = resolve_run_shell();
                            shell_args.push(condition.to_string());
                            let mut cmd = std::process::Command::new(&shell_prog);
                            cmd.args(shell_args)
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null());
                            #[cfg(windows)]
                            { use crate::platform::HideWindowCommandExt; cmd.hide_window(); }
                            cmd.status()
                            .map(|s| s.success()).unwrap_or(false)
                        }
                    };
                    if let Some(chosen) = if success { Some(true_cmd) } else { false_cmd } {
                        execute_command_string(app, chosen)?;
                    }
                }
            }
        }
        "wait-for" | "wait" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
            // Local wait-for is a no-op (requires server coordination)
        }
        "find-window" | "findw" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let pattern = parts[1..].iter().find(|p| !p.starts_with('-')).unwrap_or(&"");
                let mut output = String::new();
                for (i, win) in app.windows.iter().enumerate() {
                    if win.name.contains(pattern) {
                        output.push_str(&format!("{}: {}\n", app.win_display_index(i), win.name));
                    }
                }
                if output.is_empty() { output.push_str(&format!("(no windows matching '{}')\n", pattern)); }
                show_output_popup(app, "find-window", output);
            }
        }
        "move-window" | "movew" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                // Same shared resolver the server path uses, so the command
                // prompt and a config `move-window` agree with the CLI on what
                // `-s`, `+1` and `{last}` mean (issue #602).
                let src = parts.windows(2).find(|w| w[0] == "-s").map(|w| w[1].to_string());
                let dst = parts.windows(2).find(|w| w[0] == "-t").map(|w| w[1].to_string())
                    .or_else(|| parts[1..].iter().find(|a| !a.starts_with('-')).map(|s| s.to_string()));
                let has = |f: &str| parts[1..].iter().any(|a| *a == f);
                if let Err(msg) = app.move_window(
                    src.as_deref(), dst.as_deref(),
                    has("-d"), has("-r"), has("-a"), has("-b"))
                {
                    app.status_message = Some((format!("move-window: {}", msg), std::time::Instant::now(), None));
                }
            }
        }
        "swap-window" | "swapw" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let src = parts.windows(2).find(|w| w[0] == "-s").map(|w| w[1].to_string());
                let dst = parts.windows(2).find(|w| w[0] == "-t").map(|w| w[1].to_string())
                    .or_else(|| parts[1..].iter().find(|a| !a.starts_with('-')).map(|s| s.to_string()));
                let detach = parts[1..].iter().any(|a| *a == "-d");
                let resolved = dst.as_deref().ok_or_else(|| "can't find window: ".to_string())
                    .and_then(|d| {
                        let spos = match src.as_deref() {
                            Some(s) => app.resolve_window_spec(s, false)?.pos()
                                .ok_or_else(|| format!("can't find window: {}", s))?,
                            None => app.active_idx,
                        };
                        let tpos = app.resolve_window_spec(d, false)?.pos()
                            .ok_or_else(|| format!("can't find window: {}", d))?;
                        Ok((spos, tpos))
                    });
                match resolved {
                    Ok((spos, tpos)) if spos != tpos => {
                        app.windows.swap(spos, tpos);
                        // tmux selects the destination index only WITH -d.
                        if detach && app.active_idx != tpos {
                            app.last_window_idx = app.active_idx;
                            app.active_idx = tpos;
                        }
                    }
                    Ok(_) => {}
                    Err(msg) => {
                        app.status_message = Some((format!("swap-window: {}", msg), std::time::Instant::now(), None));
                    }
                }
            }
        }
        "link-window" | "linkw" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                // Intra-session link-window: parse -s and -t flags
                let src_idx = parts.windows(2).find(|w| w[0] == "-s")
                    .and_then(|w| w[1].trim_start_matches(':').parse::<usize>().ok());
                let dst_idx = parts.windows(2).find(|w| w[0] == "-t")
                    .and_then(|w| w[1].trim_start_matches(':').parse::<usize>().ok());
                let src = src_idx.unwrap_or(app.active_idx);
                if src < app.windows.len() {
                    let src_id = app.windows[src].id;
                    let src_name = app.windows[src].name.clone();
                    let pty_system = portable_pty::native_pty_system();
                    if let Ok(()) = crate::pane::create_window(&*pty_system, app, None, None, false) {
                        let new_idx = app.windows.len() - 1;
                        app.windows[new_idx].linked_from = Some(src_id);
                        app.windows[new_idx].name = src_name;
                        if let Some(dst) = dst_idx {
                            if app.window_indices_valid() {
                                app.move_active_window_to_index(dst);
                            } else if dst < new_idx {
                                let win = app.windows.remove(new_idx);
                                app.windows.insert(dst, win);
                            }
                        }
                        fire_hooks(app, "window-linked");
                    }
                } else {
                    app.status_message = Some(("link-window: source window not found".to_string(), Instant::now(), None));
                }
            }
        }
        "unlink-window" | "unlinkw" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else if app.windows.len() > 1 {
                let removed_pos = app.active_idx;
                let mut win = app.windows.remove(removed_pos);
                kill_all_children(&mut win.root);
                app.on_window_removed(removed_pos);
                if app.active_idx >= app.windows.len() {
                    app.active_idx = app.windows.len() - 1;
                }
                fire_hooks(app, "window-unlinked");
            }
        }
        "move-pane" | "movep" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let horizontal = parts[1..].iter().any(|a| *a == "-h");
                let mut src_win: Option<usize> = None;
                let mut src_pane: Option<usize> = None;
                let mut tgt_win: Option<usize> = None;
                let mut tgt_pane: Option<usize> = None;
                let mut pi = 1;
                while pi < parts.len() {
                    match parts[pi] {
                        "-s" => {
                            if let Some(sv) = parts.get(pi + 1) {
                                let pt = crate::cli::parse_target(sv);
                                src_win = pt.window;
                                src_pane = pt.pane;
                            }
                            pi += 2; continue;
                        }
                        "-t" => {
                            if let Some(tv) = parts.get(pi + 1) {
                                let pt = crate::cli::parse_target(tv);
                                tgt_win = pt.window;
                                tgt_pane = pt.pane;
                            }
                            pi += 2; continue;
                        }
                        _ => {}
                    }
                    pi += 1;
                }
                // Legacy: bare integer as target window
                if tgt_win.is_none() {
                    tgt_win = parts[1..].iter()
                        .filter(|a| !a.starts_with('-'))
                        .find(|a| a.parse::<usize>().is_ok())
                        .and_then(|s| s.parse::<usize>().ok());
                }
                join_pane_local(app, src_win, src_pane, tgt_win, tgt_pane, horizontal);
            }
        }
        "join-pane" | "joinp" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            } else {
                let horizontal = parts[1..].iter().any(|a| *a == "-h");
                let mut src_win: Option<usize> = None;
                let mut src_pane: Option<usize> = None;
                let mut tgt_win: Option<usize> = None;
                let mut tgt_pane: Option<usize> = None;
                let mut pi = 1;
                while pi < parts.len() {
                    match parts[pi] {
                        "-s" => {
                            if let Some(sv) = parts.get(pi + 1) {
                                let pt = crate::cli::parse_target(sv);
                                src_win = pt.window;
                                src_pane = pt.pane;
                            }
                            pi += 2; continue;
                        }
                        "-t" => {
                            if let Some(tv) = parts.get(pi + 1) {
                                let pt = crate::cli::parse_target(tv);
                                tgt_win = pt.window;
                                tgt_pane = pt.pane;
                            }
                            pi += 2; continue;
                        }
                        _ => {}
                    }
                    pi += 1;
                }
                // Legacy: bare integer as target window
                if tgt_win.is_none() {
                    tgt_win = parts[1..].iter()
                        .filter(|a| !a.starts_with('-'))
                        .find(|a| a.parse::<usize>().is_ok())
                        .and_then(|s| s.parse::<usize>().ok());
                }
                join_pane_local(app, src_win, src_pane, tgt_win, tgt_pane, horizontal);
            }
        }
        "resize-window" | "resizew" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
            // resize-window depends on terminal size, only meaningful on server
        }
        "respawn-window" | "respawnw" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
            // respawn-window requires PTY system from server context
        }
        "previous-layout" | "prevl" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "previous-layout\n", &app.session_key);
            } else {
                crate::layout::cycle_layout_reverse(app);
            }
        }
        "attach-session" | "attach" | "a" | "at" => {
            // Already attached in a running session; no-op
        }
        "start-server" | "start" => {
            // Already running
        }
        "server-info" | "info" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "server-info\n", &app.session_key);
            } else {
                let output = format!("psmux {}\nSession: {}\nWindows: {}\nActive: {}\n",
                    crate::types::VERSION, app.session_name, app.windows.len(), app.active_idx);
                show_output_popup(app, "server-info", output);
            }
        }
        "new-session" | "new" => {
            // Issue #200: create a new session from inside a running session.
            // Parse flags: -s name, -d (detached), -n windowname, -c startdir, -e env
            let mut session_name: Option<String> = None;
            let mut detached = false;
            let mut window_name: Option<String> = None;
            let mut start_dir: Option<String> = None;
            let mut env_vars: Vec<(String, String)> = Vec::new();
            let mut initial_command: Option<String> = None;
            {
                let mut i = 1;
                while i < parts.len() {
                    match parts[i] {
                        "-s" => { i += 1; if i < parts.len() { session_name = Some(parts[i].trim_matches('"').to_string()); } }
                        "-n" => { i += 1; if i < parts.len() { window_name = Some(parts[i].trim_matches('"').to_string()); } }
                        "-c" => { i += 1; if i < parts.len() { start_dir = Some(parts[i].trim_matches('"').to_string()); } }
                        "-e" => {
                            i += 1;
                            match crate::util::parse_new_session_e_value_token(parts.get(i).copied()) {
                                Ok(p) => env_vars.push(p),
                                Err(e) => {
                                    app.status_message = Some((format!("psmux: {}", e), Instant::now(), None));
                                    return Ok(());
                                }
                            }
                        }
                        "-d" => { detached = true; }
                        "-A" | "-D" | "-E" | "-P" | "-X" => { /* compatibility flags, ignored */ }
                        "-F" | "-f" | "-t" | "-x" | "-y" => { i += 1; /* skip value */ }
                        other => {
                            // Positional arg: initial shell command (issue #229)
                            if !other.starts_with('-') {
                                initial_command = Some(parts[i..].iter().map(|s| s.trim_matches('"').to_string()).collect::<Vec<_>>().join(" "));
                                break;
                            }
                        }
                    }
                    i += 1;
                }
            }

            // Generate session name if not provided
            let ns_prefix = app.socket_name.as_deref();
            let name = session_name.unwrap_or_else(|| crate::session::next_session_name(ns_prefix));

            // Build port file base (with namespace prefix if applicable)
            let port_file_base = if let Some(ref sn) = app.socket_name {
                format!("{}__{}", sn, name)
            } else {
                name.clone()
            };

            // Check if session already exists
            let port_path = crate::paths::port_file(&port_file_base);
            if std::path::Path::new(&port_path).exists() {
                if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr = format!("127.0.0.1:{}", port);
                        if std::net::TcpStream::connect_timeout(
                            &addr.parse().unwrap(),
                            std::time::Duration::from_millis(100),
                        ).is_ok() {
                            app.status_message = Some((format!("session '{}' already exists", name), Instant::now(), None));
                            return Ok(());
                        }
                    }
                }
                // Stale port file, remove it
                let _ = std::fs::remove_file(&port_path);
            }

            // Try to claim a warm server first (fast path)
            let warm_disabled = std::env::var("PSMUX_NO_WARM").map(|v| v == "1" || v == "true").unwrap_or(false)
                || crate::config::is_warm_disabled_by_config();
            let claimed_warm = if !warm_disabled && initial_command.is_none() && start_dir.is_none() && env_vars.is_empty() {
                let warm_base = if let Some(ref sn) = app.socket_name {
                    format!("{}____warm__", sn)
                } else {
                    "__warm__".to_string()
                };
                let warm_port_path = crate::paths::port_file(&warm_base);
                if std::path::Path::new(&warm_port_path).exists() {
                    if let Ok(warm_port_str) = std::fs::read_to_string(&warm_port_path) {
                        if let Ok(warm_port) = warm_port_str.trim().parse::<u16>() {
                            let warm_addr = format!("127.0.0.1:{}", warm_port);
                            if std::net::TcpStream::connect_timeout(
                                &warm_addr.parse().unwrap(),
                                std::time::Duration::from_millis(100),
                            ).is_ok() {
                                let warm_key = crate::session::read_session_key(&warm_base).unwrap_or_default();
                                if !warm_key.is_empty() {
                                    let claim_cmd = format!("claim-session {}\n", crate::util::quote_arg(&name));
                                    match crate::session::send_auth_cmd_response(
                                        &warm_addr, &warm_key,
                                        claim_cmd.as_bytes(),
                                    ) {
                                        Ok(resp) if resp.contains("OK") => {
                                            if let Some(ref wn) = window_name {
                                                let new_key = crate::session::read_session_key(&port_file_base).unwrap_or_default();
                                                let _ = crate::session::send_auth_cmd(
                                                    &warm_addr, &new_key,
                                                    format!("rename-window {}\n", crate::util::quote_arg(wn)).as_bytes(),
                                                );
                                            }
                                            // Apply -e environment variables to the claimed warm session
                                            if !env_vars.is_empty() {
                                                let new_key = crate::session::read_session_key(&port_file_base).unwrap_or_default();
                                                for (k, v) in &env_vars {
                                                    let _ = crate::session::send_auth_cmd(
                                                        &warm_addr, &new_key,
                                                        format!("set-environment {} {}\n", crate::util::quote_arg(k), crate::util::quote_arg(v)).as_bytes(),
                                                    );
                                                }
                                            }
                                            true
                                        }
                                        _ => false,
                                    }
                                } else { false }
                            } else { false }
                        } else { false }
                    } else { false }
                } else { false }
            } else { false };

            if !claimed_warm {
                // Cold path: spawn a background server process
                let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
                let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), name.clone()];
                if let Some(ref sn) = app.socket_name {
                    server_args.push("-L".into());
                    server_args.push(sn.clone());
                }
                if let Some(ref dir) = start_dir {
                    server_args.push("-d".into());
                    server_args.push(dir.clone());
                }
                if let Some(ref wn) = window_name {
                    server_args.push("-n".into());
                    server_args.push(wn.clone());
                }
                // Pass initial command to server (issue #229)
                if let Some(ref cmd) = initial_command {
                    server_args.push("-c".into());
                    server_args.push(cmd.clone());
                }
                // Pass current terminal dimensions
                let area = app.client_area;
                if area.width > 1 && area.height > 1 {
                    server_args.push("-x".into());
                    server_args.push(area.width.to_string());
                    server_args.push("-y".into());
                    server_args.push(area.height.to_string());
                }
                // Pass -e environment variables to server
                for (k, v) in &env_vars {
                    server_args.push("-e".into());
                    server_args.push(format!("{}={}", k, v));
                }
                #[cfg(windows)]
                { let _ = crate::platform::spawn_server_hidden(&exe, &server_args); }
                #[cfg(not(windows))]
                {
                    let mut cmd_proc = std::process::Command::new(&exe);
                    for a in &server_args { cmd_proc.arg(a); }
                    cmd_proc.stdin(std::process::Stdio::null());
                    cmd_proc.stdout(std::process::Stdio::null());
                    cmd_proc.stderr(std::process::Stdio::null());
                    let _ = cmd_proc.spawn();
                }
            }

            // Wait for port file to appear (up to 5 seconds)
            for _ in 0..500 {
                if std::path::Path::new(&port_path).exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }

            if std::path::Path::new(&port_path).exists() {
                if !detached {
                    // Switch to the new session
                    if let Some(port) = app.control_port {
                        let switch_cmd = format!("switch-client -t {}\n", crate::util::quote_arg(&name));
                        let _ = send_control_to_port(port, &switch_cmd, &app.session_key);
                    }
                }
                app.status_message = Some((format!("created session '{}'", name), Instant::now(), None));
            } else {
                app.status_message = Some((format!("failed to create session '{}'", name), Instant::now(), None));
            }
        }
        "lock-client" | "lockc" | "lock-server" | "lock" | "lock-session" | "locks" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "lock-server\n", &app.session_key);
            }
            app.status_message = Some(("lock: not available on Windows".to_string(), Instant::now(), None));
        }
        "refresh-client" | "refresh" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "refresh-client\n", &app.session_key);
            }
            // Trigger redraw in all modes
            app.status_message = Some(("client refreshed".to_string(), Instant::now(), None));
        }
        "suspend-client" | "suspendc" => {
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "suspend-client\n", &app.session_key);
            }
            app.status_message = Some(("suspend: not available on Windows".to_string(), Instant::now(), None));
        }
        "choose-client" => {
            app.status_message = Some(("choose-client: single-client model (you are the only client)".to_string(), Instant::now(), None));
        }
        "customize-mode" => {
            // tmux 3.2+ customize-mode: interactive options editor
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, "customize-mode\n", &app.session_key);
            } else {
                // In-process fallback: build option list directly
                let options = crate::server::option_catalog::build_option_list(app);
                app.mode = Mode::CustomizeMode {
                    options,
                    selected: 0,
                    scroll_offset: 0,
                    editing: false,
                    edit_buffer: String::new(),
                    edit_cursor: 0,
                    filter: String::new(),
                };
            }
        }
        "run-shell" | "run" => {
            // Parse with quote-aware parser to handle nested quotes properly
            let args = parse_command_line(cmd);
            let mut cmd_parts: Vec<&str> = Vec::new();
            let mut background = false;
            for arg in &args[1..] {
                if arg == "-b" { background = true; }
                else { cmd_parts.push(arg); }
            }
            let shell_cmd = cmd_parts.join(" ");
            if shell_cmd.is_empty() {
                // No command given: show usage (tmux parity)
                app.status_message = Some((
                    "usage: run-shell [-b] shell-command".to_string(),
                    Instant::now(),
                    None,
                ));
            } else {
                // Expand #{...} format variables (tmux parity: run-shell's
                // command is format-expanded before it runs).
                //
                // This was missing entirely, and it silently broke every bind of
                // the shape `run-shell "helper --path '#{pane_current_path}'"`:
                // the helper received the literal string `#{pane_current_path}`.
                // Combined with `-b` discarding the spawn result below, such a
                // bind did nothing at all and reported nothing — no output, no
                // status message, no log line. Only `-c`/`-d` start-dirs were
                // being expanded (in server/mod.rs), which is why `popup -d
                // "#{pane_current_path}"` worked and this did not.
                let shell_cmd = crate::format::expand_format(&shell_cmd, app);
                // Expand ~ to home directory + XDG fallback for plugin paths
                let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd);
                // Set PSMUX_TARGET_SESSION so child scripts connect to the correct server
                let target_session = app.port_file_base();

                if background {
                    // -b flag: fire and forget, no output capture
                    let mut c = build_run_shell_command(&shell_cmd);
                    if !target_session.is_empty() {
                        c.env("PSMUX_TARGET_SESSION", &target_session);
                    }
                    // Report a spawn failure. `-b` means "don't wait for the
                    // command", not "don't tell me it never started" — the old
                    // `let _ = c.spawn();` made a broken background bind
                    // indistinguishable from an unbound key.
                    if let Err(e) = c.spawn() {
                        app.status_message = Some((
                            format!("run-shell: {}: {}", shell_cmd, e),
                            Instant::now(),
                            None,
                        ));
                    }
                } else {
                    // No -b: spawn async to avoid blocking the UI thread.
                    // Interactive commands (htop, vim, etc.) would freeze psmux
                    // if we used synchronous .output() on the main thread.
                    // Lazily create the channel pair on first use.
                    if app.run_shell_tx.is_none() {
                        let (tx, rx) = std::sync::mpsc::channel();
                        app.run_shell_tx = Some(tx);
                        app.run_shell_rx = Some(rx);
                    }
                    let tx = app.run_shell_tx.as_ref().unwrap().clone();
                    let shell_cmd = shell_cmd.clone();
                    let shell_cmd_display = shell_cmd.clone();
                    let target_session = target_session.clone();
                    std::thread::spawn(move || {
                        let mut c = build_run_shell_command(&shell_cmd);
                        if !target_session.is_empty() {
                            c.env("PSMUX_TARGET_SESSION", &target_session);
                        }
                        // Detach stdin so interactive programs exit immediately
                        c.stdin(std::process::Stdio::null());
                        match c.output() {
                            Ok(output) => {
                                let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                                let stderr = String::from_utf8_lossy(&output.stderr);
                                if !stderr.is_empty() {
                                    if !text.is_empty() && !text.ends_with('\n') {
                                        text.push('\n');
                                    }
                                    text.push_str(&stderr);
                                }
                                // Send result back; empty output is also sent so
                                // the status message "running..." can be cleared.
                                let _ = tx.send(("run-shell".to_string(), text));
                            }
                            Err(e) => {
                                let _ = tx.send(("run-shell".to_string(), format!("run-shell: {}", e)));
                            }
                        }
                    });
                    app.status_message = Some((
                        format!("running: {}", shell_cmd_display),
                        Instant::now(),
                        None,
                    ));
                }
            }
        }
        _ => {
            // Apply config locally (handles set, bind, source, etc.)
            let old_shell = app.default_shell.clone();
            crate::config::parse_config_line(app, cmd);
            if app.default_shell != old_shell {
                if let Some(mut wp) = app.warm_pane.take() {
                    wp.child.kill().ok();
                }
            }
            // Also forward unknown commands to server (catch-all for tmux compat)
            if let Some(port) = app.control_port {
                let _ = send_control_to_port(port, &format!("{}\n", cmd), &app.session_key);
            }
        }
    }
    Ok(())
}

fn resolve_swap_pane_target_path(app: &AppState, target: &str) -> Option<Vec<usize>> {
    if target.starts_with('{') {
        return crate::window_ops::pane_path_at_position(app, target);
    }
    if app.windows.is_empty() {
        return None;
    }
    let parsed = crate::cli::parse_target(target);
    let win = &app.windows[app.active_idx];
    match parsed.pane {
        Some(p) if parsed.pane_is_id => crate::tree::find_path_by_id(&win.root, p),
        Some(p) => p.checked_sub(app.pane_base_index)
            .and_then(|idx| crate::tree::path_by_position(&win.root, idx)),
        None => None,
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_commands.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_commands_new.rs"]
mod tests_new_commands;

#[cfg(test)]
#[path = "../tests-rs/test_commands_audit.rs"]
mod tests_commands_audit;

#[cfg(test)]
#[path = "../tests-rs/test_parity.rs"]
mod tests_parity;

#[cfg(test)]
#[path = "../tests-rs/test_issue275_detach_client.rs"]
mod tests_issue275_detach_client;

#[cfg(test)]
#[path = "../tests-rs/test_issue179_bind_key_uppercase.rs"]
mod tests_issue179_bind_key_uppercase;

#[cfg(test)]
#[path = "../tests-rs/test_issue192_command_chaining.rs"]
mod tests_issue192_command_chaining;

#[cfg(test)]
#[path = "../tests-rs/test_issue200_new_session.rs"]
mod tests_issue200_new_session;

#[cfg(test)]
#[path = "../tests-rs/test_run_shell_resolve.rs"]
mod tests_run_shell_resolve;

#[cfg(test)]
#[path = "../tests-rs/test_hide_window.rs"]
mod tests_hide_window;

#[cfg(test)]
#[path = "../tests-rs/test_issue209_tmux_compat.rs"]
mod tests_issue209_tmux_compat;

#[cfg(test)]
#[path = "../tests-rs/test_gastown_scenarios.rs"]
mod tests_gastown_scenarios;

#[cfg(test)]
#[path = "../tests-rs/test_issue210_gastown_fixes.rs"]
mod tests_issue210_gastown_fixes;

#[cfg(test)]
#[path = "../tests-rs/test_issue210_gastown_captures.rs"]
mod tests_issue210_gastown_captures;

#[cfg(test)]
#[path = "../tests-rs/test_issue215_session_persistence.rs"]
mod tests_issue215_session_persistence;

#[cfg(test)]
#[path = "../tests-rs/test_mega_unit_coverage.rs"]
mod tests_mega_unit_coverage;

#[cfg(test)]
#[path = "../tests-rs/test_flag_parity.rs"]
mod tests_flag_parity;

#[cfg(test)]
#[path = "../tests-rs/test_issue227_remain_on_exit_hooks.rs"]
mod tests_issue227_remain_on_exit_hooks;

#[cfg(test)]
#[path = "../tests-rs/test_issue235_display_panes_base_index.rs"]
mod tests_issue235_display_panes_base_index;

#[cfg(test)]
#[path = "../tests-rs/test_issue244_capture_scrollback.rs"]
mod tests_issue244_capture_scrollback;

#[cfg(test)]
#[path = "../tests-rs/test_issue245_mouse_selection.rs"]
mod tests_issue245_mouse_selection;

#[cfg(test)]
#[path = "../tests-rs/test_pr255_active_border.rs"]
mod tests_pr255_active_border;

#[cfg(test)]
#[path = "../tests-rs/test_pane_border_lines_render.rs"]
mod tests_pane_border_lines_render;

#[cfg(test)]
#[path = "../tests-rs/test_pane_content_box_drawing.rs"]
mod tests_pane_content_box_drawing;

#[cfg(test)]
#[path = "../tests-rs/test_copy_line_numbers_render.rs"]
mod tests_copy_line_numbers_render;

#[cfg(test)]
#[path = "../tests-rs/test_floating_render.rs"]
mod tests_floating_render;

#[cfg(test)]
#[path = "../tests-rs/test_pr207_compat_bugs.rs"]
mod tests_pr207_compat_bugs;

#[cfg(test)]
#[path = "../tests-rs/test_named_buffers.rs"]
mod tests_named_buffers;

#[cfg(test)]
#[path = "../tests-rs/test_issue273_send_prefix.rs"]
mod tests_issue273_send_prefix;

#[cfg(test)]
#[path = "../tests-rs/test_issue383_swap_pane_targets.rs"]
mod tests_issue383_swap_pane_targets;

#[cfg(test)]
#[path = "../tests-rs/test_issue402_parse.rs"]
mod tests_issue402_parse;

#[cfg(test)]
#[path = "../tests-rs/test_issue426_split_pane_alias.rs"]
mod tests_issue426_split_pane_alias;

#[cfg(test)]
#[path = "../tests-rs/test_issue470_menu_popup.rs"]
mod tests_issue470_menu_popup;

#[cfg(test)]
#[path = "../tests-rs/test_killwindow_bad_target.rs"]
mod tests_killwindow_bad_target;
