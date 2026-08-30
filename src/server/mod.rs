pub(crate) mod helpers;
pub(crate) mod options;
pub(crate) mod option_catalog;
mod connection;

use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::env;
use std::net::TcpListener;

use portable_pty::native_pty_system;
use ratatui::prelude::Rect;

use crate::types::{AppState, CtrlReq, Mode, FocusDir, LayoutKind, PipePaneState, VERSION,
    WaitChannel, WaitForOp, Node, Action, Bind};
use crate::platform::install_console_ctrl_handler;
use crate::pane::{create_window, create_window_with_env, create_window_raw, split_active_with_env, kill_active_pane, kill_pane_by_id, spawn_warm_pane};
use crate::tree::{self, active_pane, active_pane_mut, resize_all_panes, kill_all_children,
    find_window_index_by_id, focus_pane_by_id, focus_pane_by_id_no_mru, focus_pane_by_index, get_active_pane_id,
    get_split_mut, path_exists};

use helpers::{collect_pane_paths_server, serialize_bindings_json, json_escape_string,
    list_windows_json_with_tabs, combined_data_version, TMUX_COMMANDS};
use options::{get_option_value, render_window_options, apply_set_option};

use crate::input::{send_text_to_active, send_bytes_to_active, send_key_to_active, send_paste_to_active, move_focus, move_focus_preserving_zoom, find_best_pane_in_direction, find_wrap_target};
use crate::copy_mode::{enter_copy_mode, exit_copy_mode, move_copy_cursor, current_prompt_pos,
    yank_selection, scroll_copy_up, scroll_copy_down, switch_with_copy_save,
    capture_active_pane_text, capture_active_pane_range, capture_active_pane_styled};
use crate::layout::{dump_layout_json, dump_layout_json_fast, apply_layout, cycle_layout,
    cycle_layout_reverse};
use crate::window_ops::{toggle_zoom, remote_mouse_down, remote_mouse_drag, remote_mouse_up,
    remote_mouse_button, remote_mouse_motion, remote_scroll_up, remote_scroll_down,
    swap_pane, swap_pane_with_path, break_pane_to_window, unzoom_if_zoomed, resize_pane_vertical,
    resize_pane_horizontal, resize_pane_absolute, rotate_panes, respawn_active_pane,
    handle_pane_mouse, handle_pane_scroll, copy_drag_begin, handle_split_set_sizes, handle_split_resize_done};
use crate::config::{load_config, parse_key_string, format_key_binding, normalize_key_for_binding,
    parse_config_content};
use crate::commands::{parse_command_to_action, format_action, parse_menu_definition, execute_command_string};
use crate::util::{list_windows_json, list_tree_json, list_windows_tmux, base64_encode};
use crate::control;
use crate::format::{expand_format, format_list_windows, format_list_panes, set_buffer_idx_override, set_named_buffer_override};
use crate::help;

/// True when `path` sits on a mapped network drive (`DRIVE_REMOTE`): its
/// CreateFile can stall like a UNC path when the host is unreachable, so the
/// direct file sink refuses it and points the user at a shell sink (which
/// runs as a child process and stalls only itself). `GetDriveTypeW` reads
/// the local mount table and does not touch the network.
#[cfg(windows)]
fn file_sink_drive_is_remote(path: &str) -> bool {
    // Judge `\\?\Z:\...` like `Z:\...`.
    let p = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let bytes = p.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return false; // relative or non-drive path: resolved locally
    }
    let root: [u16; 4] = [bytes[0] as u16, b':' as u16, b'\\' as u16, 0];
    // winbase.h GetDriveTypeW return value (windows-sys 0.61 does not
    // re-export the DRIVE_* constants).
    const DRIVE_REMOTE: u32 = 4;
    unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE
    }
}

#[cfg(not(windows))]
fn file_sink_drive_is_remote(_path: &str) -> bool {
    false
}

/// Build a JSON fragment with overlay state (popup, menu, confirm, display_panes).
/// Delegates popup-specific serialization to the popup module.
fn serialize_overlay_json(app: &AppState) -> String {
    use crate::server::helpers::json_escape_string;

    // Popup overlay handles PopupMode, MenuMode, ConfirmMode, PaneChooser, and default
    let mut out = crate::popup::serialize_popup_overlay(app);

    // Include status_message for display-message without -p (#110).
    //
    // tmux(1) display-message: "a delay of zero waits for a key press."
    // So `-d 0` should keep the message visible until any key is pressed;
    // the SendKey / SendText handlers clear status_message, which dismisses
    // it naturally. Treat display_time == 0 as "sticky until keypress" by
    // skipping the time-based expiry check.
    if let Some((ref msg, since, per_msg_duration)) = app.status_message {
        let elapsed = since.elapsed().as_millis() as u64;
        let display_time = per_msg_duration.unwrap_or(app.display_time_ms);
        if display_time == 0 || elapsed < display_time {
            out.push_str(",\"status_message\":\"");
            out.push_str(&json_escape_string(msg));
            out.push('"');
        }
    }
    out
}

fn should_spawn_warm_server(app: &AppState) -> bool {
    app.warm_enabled && app.session_name != "__warm__" && !app.destroy_unattached
}

fn ensure_session_registry_files(app: &AppState) {
    let Some(port) = app.control_port else { return; };
    let dir = crate::paths::psmux_dir();
    let _ = std::fs::create_dir_all(&dir);

    let base = app.port_file_base();
    let port_path = crate::paths::port_file(&base);
    let key_path = crate::paths::key_file(&base);
    let sid_path = crate::paths::sid_file(&base);
    let port_value = port.to_string();
    let sid_value = app.session_id.to_string();

    // Write the .key (and .sid) BEFORE the .port file: the .port file is the
    // readiness beacon clients poll for, so every credential they will read
    // next must already be on disk when it appears. Writing .port first opened
    // a window where a cold-start attach read an empty .key and failed AUTH
    // with "psmux: auth failed" (issue #496).
    if std::fs::read_to_string(&key_path)
        .map(|s| s.trim() != app.session_key)
        .unwrap_or(true)
    {
        let _ = std::fs::write(&key_path, &app.session_key);
    }

    if std::fs::read_to_string(&sid_path)
        .map(|s| s.trim() != sid_value)
        .unwrap_or(true)
    {
        let _ = std::fs::write(&sid_path, &sid_value);
    }

    // Record this server's OS process ID (issue #448). Written together with
    // port/key/sid so a live server is never listening without a PID anchor, and
    // re-ensured periodically so the entry self-heals after rename/claim. This is
    // what lets startup reap live-but-orphaned duplicate servers by identity.
    //
    // The body is `pid:creation_filetime`: the creation time lets kill-server's
    // force-kill fallback confirm identity (exact match) before terminating, so a
    // recycled pid is never killed. Readers tolerate a bare pid too.
    let pid_path = crate::paths::pid_file(&base);
    let self_pid = std::process::id();
    let pid_value = crate::session::format_pid_file_contents(
        self_pid,
        crate::platform::process_kill::process_creation_time(self_pid).unwrap_or(0),
    );
    if std::fs::read_to_string(&pid_path)
        .map(|s| s.trim() != pid_value)
        .unwrap_or(true)
    {
        let _ = std::fs::write(&pid_path, &pid_value);
    }

    // Establish the namespace's stable identity (issue #509) before the .port
    // beacon, so a client that sees a ready server can immediately query
    // `#{server_instance}` and get a value. Minted by the namespace's first
    // server and left alone by every later one; re-ensured here so it self-heals
    // if the file is lost while the namespace is still up.
    let _ = crate::session::ensure_namespace_instance(app.socket_name.as_deref(), self_pid);

    // Claim this process for this data dir (issue #510). Keyed by PID and kept
    // outside the per-session registry on purpose: the `.pid` entry above is
    // removed with its session, but the reaper still needs to recognise a
    // server as its own AFTER that happens - a spawn-race duplicate, or a
    // registry wipe, is exactly the case it must clean up. Re-ensured here so
    // the claim self-heals if the file is deleted underneath a live server.
    crate::session::write_server_marker(self_pid);

    // .port goes LAST: it is the readiness beacon (see comment above).
    if std::fs::read_to_string(&port_path)
        .map(|s| s.trim() != port_value)
        .unwrap_or(true)
    {
        let _ = std::fs::write(&port_path, &port_value);
    }
}

/// Diagnostic-only logging for warm-server lifecycle races. Gated behind
/// PSMUX_WARM_DEBUG=1 so it is a no-op in normal operation and tests. Writes to
/// %TEMP%\psmux_warm_debug.log (never inside the repo).
fn warm_debug(msg: &str) {
    if std::env::var("PSMUX_WARM_DEBUG").map(|v| v == "1").unwrap_or(false) {
        let tmp = env::var("TEMP")
            .or_else(|_| env::var("TMP"))
            .unwrap_or_else(|_| ".".to_string());
        let path = format!("{}\\psmux_warm_debug.log", tmp);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = std::io::Write::write_all(
                &mut f,
                format!("[{} pid={}] {}\n", ts, std::process::id(), msg).as_bytes(),
            );
        }
    }
}

/// Check if the active pane is currently squelched (hiding injected cd+cls).
/// Uses the non-consuming `squelch_cleared()` so the layout serialiser can
/// still properly consume the sentinel via `take_squelch_cleared()`.
fn is_active_pane_squelched(app: &AppState) -> bool {
    if app.windows.is_empty() { return false; }
    let win = &app.windows[app.active_idx];
    if let Some(p) = active_pane(&win.root, &win.active_path) {
        if let Some(deadline) = p.squelch_until {
            let sentinel = p.term.lock()
                .map(|parser| parser.screen().squelch_cleared())
                .unwrap_or(false);
            !sentinel && Instant::now() < deadline
        } else { false }
    } else { false }
}

/// RAII guard for the warm-spawn lock file. Removing the file on drop lets a
/// later warm spawn proceed.
struct WarmSpawnLock(std::path::PathBuf);
impl Drop for WarmSpawnLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Acquire the warm-spawn lock guarding the check->spawn window in
/// `spawn_warm_server`. Returns `Some(guard)` when this caller owns the lock and
/// may proceed to spawn, or `None` when another spawn is already in progress (in
/// which case the caller must NOT spawn). A lock file older than 20s is treated
/// as abandoned (its owner died mid-spawn) and stolen.
fn acquire_warm_spawn_lock(lock_path: &str) -> Option<WarmSpawnLock> {
    use std::io::Write as _;
    let path = std::path::PathBuf::from(lock_path);
    for _ in 0..2 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                let _ = write!(f, "{}", std::process::id());
                return Some(WarmSpawnLock(path));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| std::time::SystemTime::now().duration_since(t).ok())
                    .map(|age| age > std::time::Duration::from_secs(20))
                    .unwrap_or(false);
                if stale {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                return None;
            }
            Err(_) => return None,
        }
    }
    None
}

/// Spawn a standby "warm server" process that pre-loads config + shell.
/// When `psmux new-session` is run later, the CLI claims this warm server
/// via `claim-session` instead of cold-spawning, making session creation
/// nearly instant.  The warm server uses session name `__warm__`.
fn spawn_warm_server(app: &AppState) {
    // destroy-unattached means the user expects the session to be torn down
    // when the last client leaves; keeping a hidden warm server alive breaks
    // that expectation and makes exit-empty appear ineffective.
    if !should_spawn_warm_server(app) {
        return;
    }
    // Skip if a warm server already exists
    let warm_base = if let Some(ref sn) = app.socket_name {
        format!("{}____warm__", sn)
    } else {
        "__warm__".to_string()
    };
    let warm_port_path = crate::paths::port_file(&warm_base);
    warm_debug(&format!("spawn_warm_server entry base={} port_exists={}", warm_base, std::path::Path::new(&warm_port_path).exists()));
    // Serialize the check->spawn window: without this, two callers can both see
    // "no warm" (a freshly-spawned warm hasn't written its port yet) and each
    // spawn one, orphaning all but the last. This is the primary process-leak source.
    let warm_lock_path = crate::paths::spawnlock_file(&warm_base);
    let spawn_lock = match acquire_warm_spawn_lock(&warm_lock_path) {
        Some(g) => g,
        None => { warm_debug("another warm spawn in progress -- skipping"); return; }
    };
    if std::path::Path::new(&warm_port_path).exists() {
        // Check if it's actually a live, unclaimed warm server.
        // TCP reachability alone is not sufficient: OS ephemeral-port reuse or
        // duplicated-warm churn can leave __warm__.port pointing at a real
        // claimed session.  That server answers TCP connects but is NOT warm,
        // so returning early here means warm never re-establishes and every
        // subsequent open stays cold (~1-5s) until the pointer is manually removed.
        let mut is_genuine_warm = false;
        if let Ok(port_str) = std::fs::read_to_string(&warm_port_path) {
            if let Ok(port) = port_str.trim().parse::<u16>() {
                let addr = format!("127.0.0.1:{}", port);
                if std::net::TcpStream::connect_timeout(
                    &addr.parse().unwrap(),
                    Duration::from_millis(100),
                ).is_ok() {
                    // TCP is up — verify the session name via AUTH.
                    let warm_key_path = crate::paths::key_file(&warm_base);
                    if let Ok(key) = std::fs::read_to_string(&warm_key_path) {
                        let key = key.trim().to_string();
                        if !key.is_empty() {
                            // Ask the server what its session name is.
                            // Response is the name followed by a newline.
                            // Timeout is capped at 500ms inside send_auth_cmd_response.
                            match crate::session::send_auth_cmd_response(
                                &addr, &key,
                                b"display-message -p '#{session_name}'\n",
                            ) {
                                Ok(resp) if resp.trim() == "__warm__" => {
                                    warm_debug("early-return: existing warm verified alive");
                                    is_genuine_warm = true;
                                }
                                Ok(resp) => {
                                    warm_debug(&format!(
                                        "warm port reachable but session='{}' (not __warm__) — treating as stale",
                                        resp.trim()
                                    ));
                                }
                                Err(_) => {
                                    warm_debug("warm port reachable but auth/query failed — treating as stale");
                                }
                            }
                        }
                    }
                }
            }
        }
        if is_genuine_warm {
            return;
        }
        // Stale or wrong-server port file — remove it (and matching key/sid files)
        warm_debug("removing STALE warm port/key/sid (unreachable or not a warm server)");
        let _ = std::fs::remove_file(&warm_port_path);
        let warm_key_path = crate::paths::key_file(&warm_base);
        let _ = std::fs::remove_file(&warm_key_path);
        let warm_sid_path = crate::paths::sid_file(&warm_base);
        let _ = std::fs::remove_file(&warm_sid_path);
    }
    warm_debug("SPAWNING new warm server");
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
    let mut args: Vec<String> = vec!["server".into(), "-s".into(), "__warm__".into()];
    if let Some(ref sn) = app.socket_name {
        args.push("-L".into());
        args.push(sn.clone());
    }
    // Pass current terminal dimensions so the warm server's first window
    // and warm pane are spawned at the right size.
    let area = app.client_area;
    if area.width > 1 && area.height > 1 {
        args.push("-x".into());
        args.push(area.width.to_string());
        args.push("-y".into());
        args.push(area.height.to_string());
    }
    #[cfg(windows)]
    {
        let spawned = crate::platform::spawn_server_hidden(&exe, &args);
        warm_debug(&format!("spawned warm server pid={:?}", spawned.as_ref().ok()));
        // Hold the spawn lock until the new warm server registers its port, so a
        // concurrent caller observes the genuine warm instead of racing another
        // spawn. Done off-thread to avoid blocking the caller.
        let port_path = warm_port_path.clone();
        std::thread::spawn(move || {
            let _lock = spawn_lock; // released on drop
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(100));
                if std::path::Path::new(&port_path).exists() {
                    std::thread::sleep(Duration::from_millis(150));
                    break;
                }
            }
        });
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new(&exe);
        for a in &args { cmd.arg(a); }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());
        let _ = cmd.spawn();
        drop(spawn_lock);
    }
}

/// Parse a popup dimension spec: "80" (absolute) or "95%" (percentage of term_dim).
fn parse_popup_dim(spec: &str, term_dim: u16, default: u16) -> u16 {
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

/// Process a single CtrlReq during the post-config plugin drain loop.
/// Handles the subset of requests that plugin scripts send (set, show, bind,
/// source-file) and silently drops others.
fn drain_plugin_req(
    app: &mut AppState,
    req: CtrlReq,
    shared_aliases: &std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>,
) {
    match req {
        CtrlReq::SetOption(option, value) => {
            apply_set_option(app, &option, &value, false);
            app.user_set_options.insert(option.clone());
            if option == "command-alias" {
                if let Ok(mut map) = shared_aliases.write() {
                    *map = app.command_aliases.clone();
                }
            }
            // pane-border-status changes the effective content height (#288)
            if option == "pane-border-status" {
                resize_all_panes(app);
            }
            if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(app) {
                resize_all_panes(app);
            }
        }
        CtrlReq::SetOptionQuiet(option, value, quiet) => {
            apply_set_option(app, &option, &value, quiet);
            app.user_set_options.insert(option.clone());
            if option == "command-alias" {
                if let Ok(mut map) = shared_aliases.write() {
                    *map = app.command_aliases.clone();
                }
            }
            if option == "pane-border-status" {
                resize_all_panes(app);
            }
            if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(app) {
                resize_all_panes(app);
            }
        }
        CtrlReq::SetOptionAppend(option, value) => {
            if option.starts_with('@') {
                let existing = app.user_options.get(&option).cloned().unwrap_or_default();
                app.user_options.insert(option, format!("{}{}", existing, value));
            } else {
                match option.as_str() {
                    "status-left" => app.status_left.push_str(&value),
                    "status-right" => app.status_right.push_str(&value),
                    "status-style" => app.status_style.push_str(&value),
                    "window-style" | "window-active-style" => {
                        app.user_options
                            .entry(option.clone())
                            .or_default()
                            .push_str(&value);
                        app.user_set_options.insert(option);
                    }
                    _ => {}
                }
            }
        }
        CtrlReq::SetOptionUnset(option) => {
            if option.starts_with('@') {
                app.user_options.remove(&option);
            } else if matches!(option.as_str(), "window-style" | "window-active-style") {
                app.user_options.remove(&option);
                app.user_set_options.remove(&option);
            }
        }
        CtrlReq::SetOptionToggle(option) => {
            // `set -g <bool-option>` with no value flips it (#535). The client
            // cannot compute the new value itself: only the server knows the
            // current one, so the toggle has to happen here.
            if crate::server::options::toggle_option(app, &option) {
                app.user_set_options.insert(option.clone());
            }
        }
        CtrlReq::SetOptionOnlyIfUnset(option, value) => {
            // Only set if the option hasn't been explicitly set by user/config.
            // For @-prefixed user options, check if the key exists.
            // For built-in options, check the user_set_options tracker.
            let already_set = if option.starts_with('@') {
                app.user_options.contains_key(&option)
            } else {
                app.user_set_options.contains(&option)
            };
            if !already_set {
                apply_set_option(app, &option, &value, false);
                app.user_set_options.insert(option.clone());
                if option == "command-alias" {
                    if let Ok(mut map) = shared_aliases.write() {
                        *map = app.command_aliases.clone();
                    }
                }
            }
        }
        CtrlReq::ShowOptionValue(resp, name) => {
            let val = get_option_value(app, &name);
            let _ = resp.send(val);
        }
        CtrlReq::ShowWindowOptionValue(resp, name, target) => {
            let val = crate::server::options::get_window_option_value_for(app, &name, target);
            let _ = resp.send(val);
        }
        CtrlReq::ShowOptions(resp) => {
            // Minimal: just send empty to unblock the caller
            let _ = resp.send(String::new());
        }
        CtrlReq::ShowWindowOptions(resp) => {
            let _ = resp.send(render_window_options(app));
        }
        CtrlReq::BindKey(table_name, key, command, repeat) => {
            if let Some(kc) = parse_key_string(&key) {
                let kc = normalize_key_for_binding(kc);
                let sub_cmds = crate::config::split_chained_commands_pub(&command);
                let action = if sub_cmds.len() > 1 {
                    Some(Action::CommandChain(sub_cmds))
                } else {
                    parse_command_to_action(&command)
                };
                if let Some(act) = action {
                    let table = app.key_tables.entry(table_name).or_default();
                    table.retain(|b| b.key != kc);
                    table.push(Bind { key: kc, action: act, repeat });
                }
            }
        }
        CtrlReq::SourceFile(path) => {
            app.defaults_suppressed = false;
            app.key_tables.clear();
            crate::config::populate_default_bindings(app);
            crate::config::source_file(app, &path);
            // A runtime source-file can record warnings (e.g. an unreadable
            // path); flush them like the startup load does or they never
            // reach config-warnings.log and the attach-time summary.
            write_config_warnings_log(&app.config_warnings);
            // source-file may change pane-border-status (#288)
            resize_all_panes(app);
        }
        CtrlReq::UnbindAll => {
            app.key_tables.clear();
            app.defaults_suppressed = true;
        }
        CtrlReq::UnbindAllInTable(table) => {
            if let Some(binds) = app.key_tables.get_mut(&table) {
                binds.clear();
            }
        }
        CtrlReq::UnbindKey(key, table) => {
            if let Some(kc) = parse_key_string(&key) {
                let kc = normalize_key_for_binding(kc);
                let target = table.unwrap_or_else(|| "prefix".to_string());
                if let Some(binds) = app.key_tables.get_mut(&target) {
                    binds.retain(|b| b.key != kc);
                }
            }
        }
        // Ignore other request types during plugin drain
        _ => {}
    }
}

/// Persist a server-startup failure to `~/.psmux/server-startup.log`.
///
/// The detached server has no visible stderr — when the initial pane spawn
/// fails (e.g. the `CreateProcessW err 87` from psmux issue #167) the user
/// sees only "psmux flashed black and returned to prompt".  This file lets
/// the user (or our docs) point them at concrete evidence:
///
///   - the actual error message (locale-specific GetLastError text),
///   - the build/version of psmux that produced it,
///   - the size of the inherited environment block (a likely culprit on
///     Microsoft-account profiles where OneDrive + WindowsApps inflate
///     the env to near the 32 KB Windows limit),
///   - the path psmux tried to spawn.
///
/// Best-effort: any error writing the log is swallowed (we are already
/// reporting the original failure up the call chain).
///
/// Windows-only: the log exists to explain detached-server spawn failures
/// (ConPTY `CreateProcessW` errors); the diagnostic it collects
/// (`encode_wide` environment sizes) is meaningless elsewhere.
#[cfg(windows)]
pub(crate) fn write_startup_error_log(err: &dyn std::fmt::Display) {
    let Some(dir) = crate::paths::psmux_dir_opt() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = format!("{}\\server-startup.log", dir);

    use std::os::windows::ffi::OsStrExt;
    let mut env_count = 0usize;
    let mut env_chars = 0usize;
    let mut env_largest = ("".to_string(), 0usize);
    for (k, v) in std::env::vars_os() {
        env_count += 1;
        let kl = k.encode_wide().count();
        let vl = v.encode_wide().count();
        env_chars += kl + 1 + vl + 1;
        let total = kl + vl + 1;
        if total > env_largest.1 {
            env_largest = (k.to_string_lossy().into_owned(), total);
        }
    }

    let cwd = std::env::current_dir().ok();
    let userprofile = std::env::var("USERPROFILE").ok();
    let onedrive_present = std::env::var("OneDrive").is_ok();
    let comspec = std::env::var("ComSpec").ok();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = format!(
        "psmux server startup error\n\
         ==========================\n\
         psmux version : {version}\n\
         git commit    : {commit}\n\
         when (epoch s): {now}\n\
         os.family     : windows\n\
         \n\
         error:\n\
           {err}\n\
         \n\
         spawn context:\n\
           CWD                 : {cwd:?}\n\
           USERPROFILE         : {up:?}\n\
           ComSpec             : {cs:?}\n\
           OneDrive present    : {od}\n\
           env vars (count)    : {ec}\n\
           env block size (wch): {eb} (Windows hard limit: 32767)\n\
           largest env entry   : {key} ({sz} chars)\n\
         \n\
         workarounds to try (in order):\n\
           1. PSMUX_NO_PASSTHROUGH=1   (skip ConPTY passthrough mode)\n\
           2. PSMUX_BARE_ENV=1         (spawn with minimal env block)\n\
           3. switch to a local Windows account (Microsoft account\n\
              profiles often inherit a bloated environment)\n\
           4. open an issue at https://github.com/psmux/psmux/issues/167\n\
              and attach this file\n",
        version = env!("CARGO_PKG_VERSION"),
        commit = format!(
            "{}{} ({})",
            env!("PSMUX_GIT_HASH"),
            if env!("PSMUX_GIT_DIRTY") == "true" { "-dirty" } else { "" },
            env!("PSMUX_GIT_DATE"),
        ),
        now = now,
        err = err,
        cwd = cwd,
        up = userprofile,
        cs = comspec,
        od = onedrive_present,
        ec = env_count,
        eb = env_chars,
        key = env_largest.0,
        sz = env_largest.1,
    );
    let _ = std::fs::write(&path, body);
}

/// Absolute path to `~/.psmux/server-startup.log`, or None if no home dir.
pub(crate) fn startup_error_log_path() -> Option<String> {
    Some(format!("{}\\server-startup.log", crate::paths::psmux_dir_opt()?))
}

/// Read the real failure reason out of a *fresh* `server-startup.log`.
///
/// Issue #370: when the initial pane spawn fails (e.g. a `default-shell`
/// pointing at a non-existent path), the detached server records the concrete
/// error here and exits, but the client the user is actually looking at only
/// printed a generic "failed to create session". This lets the client echo the
/// real cause to the terminal instead of leaving it buried in a log file.
///
/// `since_epoch` is the wall-clock second the current startup attempt began;
/// logs whose `when (epoch s)` predate it are stale (from an earlier run or an
/// adopted warm server) and are ignored. Returns `(error_text, log_path)`.
pub(crate) fn read_fresh_startup_error(since_epoch: u64) -> Option<(String, String)> {
    let path = startup_error_log_path()?;
    read_fresh_startup_error_at(&path, since_epoch)
}

/// Path-injectable core of [`read_fresh_startup_error`] — kept separate so unit
/// tests can exercise the freshness/parsing logic against a temp file without
/// mutating the process-global USERPROFILE/HOME env (which would race the
/// issue-167 log tests sharing this binary).
fn read_fresh_startup_error_at(path: &str, since_epoch: u64) -> Option<(String, String)> {
    let content = std::fs::read_to_string(path).ok()?;

    // Freshness gate: only surface a log written during this attempt.
    let when = content
        .lines()
        .find_map(|l| l.trim().strip_prefix("when (epoch s):"))
        .and_then(|v| v.trim().parse::<u64>().ok())?;
    // Allow 2s of slack so clock granularity / pre-spawn timing never hides a
    // genuinely-current failure.
    if when + 2 < since_epoch {
        return None;
    }

    // Extract the indented error block: the lines after the "error:" marker up
    // to the next blank line.
    let mut lines = content.lines();
    let mut err_lines: Vec<String> = Vec::new();
    while let Some(l) = lines.next() {
        if l.trim() == "error:" {
            for body_line in lines.by_ref() {
                if body_line.trim().is_empty() {
                    break;
                }
                err_lines.push(body_line.trim().to_string());
            }
            break;
        }
    }
    if err_lines.is_empty() {
        return None;
    }
    Some((err_lines.join(" "), path.to_string()))
}

/// Absolute path to `~/.psmux/config-warnings.log`, or None if no home dir.
pub(crate) fn config_warnings_log_path() -> Option<String> {
    Some(format!("{}\\config-warnings.log", crate::paths::psmux_dir_opt()?))
}

/// Persist non-fatal config parse warnings so the attaching client can echo
/// them to the user's terminal (issue #370 follow-up). The detached server has
/// no visible stderr, so an unknown option / malformed value / unknown command
/// would otherwise be silently dropped. A leading `when (epoch s)` line lets
/// the client ignore a stale file from an earlier run. Writing an empty list
/// removes any prior log so resolved configs don't keep re-reporting.
pub(crate) fn write_config_warnings_log(warnings: &[String]) {
    let Some(path) = config_warnings_log_path() else { return };
    if warnings.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut body = format!("when (epoch s): {}\n", now);
    for w in warnings {
        body.push_str(w);
        body.push('\n');
    }
    let _ = std::fs::write(&path, body);
}

/// Read fresh config warnings written during the current startup attempt.
/// `since_epoch` is when the attempt began; a log older than that (minus 2s of
/// clock slack) is stale and ignored. Returns the warning lines.
pub(crate) fn read_fresh_config_warnings(since_epoch: u64) -> Vec<String> {
    let Some(path) = config_warnings_log_path() else { return Vec::new() };
    let Ok(content) = std::fs::read_to_string(&path) else { return Vec::new() };
    let mut lines = content.lines();
    let when = lines
        .next()
        .and_then(|l| l.trim().strip_prefix("when (epoch s):").map(|v| v.trim().to_string()))
        .and_then(|v| v.parse::<u64>().ok());
    match when {
        Some(w) if w + 2 >= since_epoch => lines.map(|l| l.to_string()).filter(|l| !l.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// Move the single-server-per-name guard (issue #2) onto `new_base` after this
/// server's session was renamed or a warm server was claimed (issue #505).
///
/// The guard is a named mutex keyed on the session's port-file base, and that
/// base changes with the session name. Leaving it on the startup name breaks
/// both directions:
///
/// * the OLD name stays locked for the process's whole life, so a later server
///   spawned under it exits as a "duplicate" and never writes a `.port` file —
///   the caller reports `failed to create session '<old name>'`. The default
///   session name is `0`, which is exactly what a bare `new-session` auto-picks,
///   so renaming the first session broke plain `new-session` outright.
/// * the NEW name is left unguarded, silently disabling duplicate-server
///   protection for every renamed (and every warm-claimed) session.
///
/// Release before acquiring so renaming a session onto a name it already holds
/// cannot block on itself. A refused acquire is not fatal: the rename has
/// already happened, so we simply run unguarded rather than abandon the session.
/// Warm servers are guarded too: a namespace holds exactly one `__warm__`
/// server, and releasing that name here is precisely what lets the replacement
/// warm spawned after a claim acquire it (issue #459).
/// Remove the window at Vec position `pos`, killing its children. The last
/// window's children are killed in place (the empty-session reaper then ends
/// the server), matching the historical kill-window behavior. `active_idx`
/// shifts down when a window before it is removed so focus stays on the same
/// window; it only moves when the active window itself was the target.
fn kill_window_at(app: &mut AppState, pos: usize) {
    if pos >= app.windows.len() { return; }
    if app.windows.len() > 1 {
        let mut win = app.windows.remove(pos);
        kill_all_children(&mut win.root);
        app.on_window_removed(pos);
        if app.active_idx > pos { app.active_idx -= 1; }
        if app.active_idx >= app.windows.len() { app.active_idx = app.windows.len() - 1; }
    } else {
        // Last window: kill all children; reaper will detect empty session and exit
        kill_all_children(&mut app.windows[0].root);
    }
}

/// Apply one move-window request. The move itself lives on `AppState` so the
/// CLI, the command prompt and this path cannot drift; the only extra the
/// server can do is `-k`, which kills the window occupying the destination
/// index (PTY teardown is server-only).
fn move_window_request(
    app: &mut AppState,
    src: Option<&str>,
    dst: Option<&str>,
    detach: bool,
    kill: bool,
    renumber: bool,
    after: bool,
    before: bool,
) -> Result<(), String> {
    // -a/-b make room by shuffling, so they never collide and never need -k.
    if kill && !renumber && !after && !before {
        if let Some(occupant) = app.move_window_kill_target(src, dst) {
            kill_window_at(app, occupant);
        }
    }
    app.move_window(src, dst, detach, renumber, after, before)
}

fn rekey_session_guard(guard: &mut Option<crate::platform::SessionMutex>, new_base: &str) {
    *guard = None; // drop releases + closes the old name's mutex
    *guard = crate::platform::acquire_session_mutex(new_base);
    if guard.is_none() {
        warm_debug(&format!("session guard: '{}' already owned by a live server — running unguarded", new_base));
    }
}

pub fn run_server(session_name: String, socket_name: Option<String>, initial_command: Option<String>, raw_command: Option<Vec<String>>, start_dir: Option<String>, window_name: Option<String>, init_size: Option<(u16, u16)>, group_target: Option<String>, env_vars: Vec<(String, String)>) -> io::Result<()> {
    // Write crash info to a log file when stderr is unavailable (detached server)
    // and clean up port/key files so stale entries do not linger (issue #204).
    let panic_session_name = session_name.clone();
    let panic_socket_name = socket_name.clone();
    std::panic::set_hook(Box::new(move |info| {
        let path = crate::paths::psmux_dir_file("crash.log");
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(&path, format!("{info}\n\nBacktrace:\n{bt}"));
        // Remove port/key files to prevent stale entries after a panic
        let base = if let Some(ref sn) = panic_socket_name {
            format!("{}__{}", sn, panic_session_name)
        } else {
            panic_session_name.clone()
        };
        let _ = std::fs::remove_file(crate::paths::port_file(&base));
        let _ = std::fs::remove_file(crate::paths::key_file(&base));
        let _ = std::fs::remove_file(crate::paths::sid_file(&base));
        let _ = std::fs::remove_file(crate::paths::pid_file(&base));
        let _ = std::fs::remove_file(crate::paths::activity_file(&base));
    }));
    // Install console control handler to prevent termination on client detach
    install_console_ctrl_handler();

    let pty_system = native_pty_system();

    let mut app = AppState::new(session_name);
    // Claim a scheduling class before anything else runs (#608). A windowless
    // server never receives the foreground boost Windows reserves for the
    // process owning the active window, so on an oversubscribed box its reader
    // and writer threads queue behind every compute job on the machine and
    // typing lags. load_config below re-applies this from the `priority`
    // option if the user set one; PSMUX_PRIORITY outranks both.
    app.priority = crate::platform::resolve_priority(None, true);
    crate::platform::set_process_priority(&app.priority);
    // Preinitialize the async #(command) format-job channel (see the
    // format_job_rx doc in types.rs).
    {
        let (fjtx, fjrx) = std::sync::mpsc::channel();
        app.format_job_tx = Some(fjtx);
        app.format_job_rx = Some(fjrx);
    }
    app.socket_name = socket_name;
    app.session_group = group_target;
    // Server starts detached with a reasonable default window size
    app.attached_clients = 0;

    // ── P0: single-server-per-name guard (issue #2) ─────────────────────────
    // Hold a named mutex keyed on this session's base name for the server's whole
    // life. If another LIVE server already owns the name, we are a duplicate from
    // a cold-spawn race (has-session false-negatived under load, or two
    // `new-session -s X` raced) — exit cleanly so the winner stays the single
    // source of truth. Two servers on one name desync the .port/.key files and
    // wedge the session ("appears lost"). Warm (standby) servers are guarded on
    // the same terms: `__warm__.port` is a single file, so a namespace can only
    // ever publish one warm server, and an unguarded warm name let every failed
    // or slow registration strand another live process (issue #459). Fail-open:
    // any FFI hiccup yields a live guard, never a blocked legitimate start.
    // Re-keyed on every rename/claim, see rekey_session_guard (issue #505).
    let mut session_guard = {
        let base = app.port_file_base();
        {
            match crate::platform::acquire_session_mutex(&base) {
                Some(g) => Some(g),
                None => {
                    warm_debug(&format!("server STARTUP: session '{}' already owned by a live server — exiting duplicate", base));
                    return Ok(()); // do NOT touch the winner's .port/.key/.sid
                }
            }
        }
    };

    // Bind the control listener BEFORE loading config so that run-shell
    // commands spawned by load_config can connect back to the server.
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_rx = Some(rx);
    // Keep a sender in AppState so loop-resident code can queue follow-up work
    // (see the field's doc comment — copy-mode key tables need this).
    app.control_tx = Some(tx.clone());
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    app.control_port = Some(port);
    warm_debug(&format!("server STARTUP: session='{}' bound port={}", app.session_name, port));

    // Write port and key files IMMEDIATELY after binding, BEFORE loading
    // config or creating windows.  run-shell scripts (e.g. PPM) need the
    // port file to discover the server, and the client polls for it to know
    // the server is ready.
    let dir = crate::paths::psmux_dir();
    let _ = std::fs::create_dir_all(&dir);

    // Generate a random session key for security
    let session_key: String = {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        let s = RandomState::new();
        let mut h = s.build_hasher();
        h.write_u64(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64);
        h.write_u64(std::process::id() as u64);
        format!("{:016x}", h.finish())
    };

    app.session_key = session_key.clone();

    // Write the key file up front (user-only visibility on Windows comes from
    // the profile directory ACLs). This MUST happen before
    // ensure_session_registry_files writes the .port readiness beacon: a
    // truncate+rewrite of the .key after .port is visible gave attaching
    // clients a window to read an EMPTY key and fail with "psmux: auth
    // failed" on cold start (issue #496). ensure_session_registry_files sees
    // the matching content below and never rewrites it.
    {
        let keypath = crate::paths::key_file(&app.port_file_base());
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&keypath)
            .map(|mut f| std::io::Write::write_all(&mut f, session_key.as_bytes()));
    }

    // TEST-ONLY fault injection — compiled out of release builds entirely
    // (gated on debug_assertions); inert in debug unless the env var is set.
    // Delays the .port file write (which happens inside
    // ensure_session_registry_files below) while the server is otherwise
    // healthy, to deterministically reproduce a SLOW server startup under load.
    // The client's readiness gate must wait for the eventually-reachable server
    // rather than give up and orphan it. See tests/test_new_session_no_orphan.ps1.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = env::var("PSMUX_TEST_PORTFILE_DELAY_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                thread::sleep(Duration::from_millis(ms));
            }
        }
    }

    ensure_session_registry_files(&app);

    // TEST-ONLY fault injection — compiled out of release builds entirely.
    // Simulates the server dying AFTER writing its .port file but WITHOUT the
    // panic hook running (a hard exit / kill that leaves a stale .port behind).
    // The client's readiness gate must detect the dead server PID and fail fast
    // instead of blocking until the 15s deadline. See tests/test_new_session_hang.ps1.
    #[cfg(debug_assertions)]
    {
        if env::var("PSMUX_TEST_DIE_AFTER_PORTFILE").is_ok() {
            std::process::exit(3);
        }
    }

    let regpath = crate::paths::port_file(&app.port_file_base());
    let keypath = crate::paths::key_file(&app.port_file_base());

    // Expose the server identity via env var so that child processes spawned
    // by run-shell (from hooks, keybindings, etc.) can find this server when
    // they call `psmux set -g ...` or other CLI commands.
    env::set_var("PSMUX_TARGET_SESSION", app.port_file_base());

    // NOTE: the .key file is written BEFORE ensure_session_registry_files
    // above — never recreate/truncate it here, after the .port beacon is
    // already visible to attaching clients (issue #496).

    // Start accept thread BEFORE load_config so that run-shell commands
    // (e.g. PPM plugin manager) spawned during config parsing can connect
    // to the server.  Without this, run-shell scripts fail silently because
    // there is no TCP listener accepting connections yet.
    // Initialize shared aliases empty — will be populated after load_config.
    let shared_aliases: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>> =
        std::sync::Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
    let shared_aliases_main = shared_aliases.clone();

    thread::spawn(move || {
        for conn in listener.incoming() {
            if let Ok(stream) = conn {
                // Accepted control sockets must NOT be inheritable: the server
                // spawns children with bInheritHandles=TRUE (pane shells via
                // ConPTY, pipe-pane sinks, run-shell), and a long-lived child
                // that inherits a client's socket pins the connection open, so
                // the client waiting for close-as-end-of-response times out.
                // Concretely: `pipe-pane -o "<sink>" \; <cmd>` answered fine
                // but exited 1 with "no response from server (timed out)"
                // because the freshly spawned sink held a dup of the socket.
                #[cfg(windows)]
                unsafe {
                    use std::os::windows::io::AsRawSocket;
                    #[link(name = "kernel32")]
                    extern "system" {
                        fn SetHandleInformation(h: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
                    }
                    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
                    SetHandleInformation(stream.as_raw_socket() as *mut core::ffi::c_void, HANDLE_FLAG_INHERIT, 0);
                }
                let tx = tx.clone();
                let session_key_clone = session_key.clone();
                let aliases = shared_aliases.clone();
                thread::spawn(move || {
                    connection::handle_connection(stream, tx, &session_key_clone, aliases);
                }); // end per-connection thread
            }
        }
    });

    // Load config AFTER the TCP listener is bound, port/key files are written,
    // and the accept thread is running.  This ensures that run-shell commands
    // in the config (e.g. `run '~/.psmux/plugins/ppm/ppm.ps1'`) can connect
    // back to the server to apply settings.

    // Apply initial dimensions BEFORE warm pane spawn so spawn_warm_pane()
    // uses the correct terminal size.
    if let Some((w, h)) = init_size {
        let area = ratatui::layout::Rect { x: 0, y: 0, width: w, height: h };
        app.client_area = area;
        app.last_window_area = area;
    }

    // Apply -e environment variables BEFORE pane spawn so the first pane
    // inherits them via apply_user_environment().
    crate::util::merge_session_env_into_app(&mut app, &env_vars);

    // Pre-spawn a warm pane BEFORE loading config: the shell (pwsh) starts
    // loading immediately and runs in parallel with config parsing / plugin
    // initialization.  By the time create_window() consumes it, the shell
    // has had the full config-load duration (~100-500ms) as a head start.
    // Only when using default shell (no custom command).
    // For detached sessions without -x/-y, last_window_area defaults to
    // 120x30 which is fine for the warm pane (resized later on first attach).
    let early_warm = if initial_command.is_none() && raw_command.is_none() && start_dir.is_none() {
        match spawn_warm_pane(&*pty_system, &mut app) {
            Ok(wp) => Some(wp),
            Err(_) => None,
        }
    } else { None };

    crate::config::populate_default_bindings(&mut app);
    load_config(&mut app);
    // Surface any non-fatal config parse warnings to the attaching client
    // (issue #370 follow-up) instead of silently dropping them.
    write_config_warnings_log(&app.config_warnings);
    // Config may set pane-border-status which changes content height (#288)
    resize_all_panes(&mut app);

    // Execute queued plugin .ps1 scripts (e.g. theme plugins that use
    // PowerShell variables and call back to psmux via CLI).  We spawn
    // them async and then drain the CtrlReq channel in a mini-loop so
    // show-options / set requests from the scripts are handled before
    // the main UI starts.
    if !app.pending_plugin_scripts.is_empty() {
        let scripts: Vec<String> = app.pending_plugin_scripts.drain(..).collect();
        let target_session = app.port_file_base();
        let mut children: Vec<std::process::Child> = Vec::new();
        for ps1 in &scripts {
            // Resolve shell: pwsh (PS7) preferred, fall back to powershell.exe (Windows PS)
            let shell = if which::which("pwsh").is_ok() { "pwsh" } else { "powershell" };
            let mut cmd = std::process::Command::new(shell);
            cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", ps1]);
            if !target_session.is_empty() {
                cmd.env("PSMUX_TARGET_SESSION", &target_session);
            }
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::null());
            { use crate::platform::HideWindowCommandExt; cmd.hide_window(); }
            if let Ok(child) = cmd.spawn() {
                children.push(child);
            }
        }

        // Drain CtrlReq messages until all scripts finish (max 5s).
        if !children.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(5);
            // Temporarily take rx out of app to avoid borrow conflict
            if let Some(rx) = app.control_rx.take() {
                loop {
                    let all_done = children.iter_mut().all(|c| {
                        matches!(c.try_wait(), Ok(Some(_)))
                    });
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if all_done || remaining.is_zero() {
                        while let Ok(req) = rx.try_recv() {
                            drain_plugin_req(&mut app, req, &shared_aliases_main);
                        }
                        break;
                    }
                    match rx.recv_timeout(Duration::from_millis(50).min(remaining)) {
                        Ok(req) => drain_plugin_req(&mut app, req, &shared_aliases_main),
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
                app.control_rx = Some(rx);
            }
        }
    }

    // Reconcile the early warm pane (born with all defaults, before
    // load_config ran) with whatever the config actually established.
    // The decision lives in warm_pane_sync::for_post_config; this site
    // just stages the early pane into `app.warm_pane` so the policy
    // module can act on it uniformly.
    if let Some(wp) = early_warm {
        app.warm_pane = Some(wp);
        let sync = crate::warm_pane_sync::for_post_config(&app);
        crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
    }

    // Update shared aliases now that config has been loaded
    if let Ok(mut w) = shared_aliases_main.write() {
        *w = app.command_aliases.clone();
    }

    // TEST-ONLY fault injection — compiled out of release builds entirely.
    // Widens the new-session readiness race deterministically: the .port file
    // and accept thread are already up (the client's connect check passes), but
    // the initial window does not yet exist and the main request loop is not yet
    // answering. A large value (e.g. 60000) also simulates a server that is
    // ALIVE but whose create_window hangs forever, exercising the client's
    // bounded 15s deadline (it must return ~15s, never block indefinitely).
    // See tests/test_new_session_readiness.ps1 and tests/test_new_session_hang.ps1.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = env::var("PSMUX_TEST_WINDOW_DELAY_MS") {
            if let Ok(ms) = ms.parse::<u64>() {
                thread::sleep(Duration::from_millis(ms));
            }
        }
    }

    // Create initial window — if a warm pane was pre-spawned above,
    // create_window's fast path transplants it instantly.
    //
    // Set the server's working directory to the session start directory (the
    // -c dir if given, otherwise the launch dir) and DO NOT restore it
    // afterwards. This server process hosts exactly one session, so its cwd is
    // that session's start directory for the rest of its life. Every later
    // new-window / split / warm-pane replenish without an explicit -c then
    // inherits it — which is what makes an attached client's new-window/split
    // open in the session start directory, while preserving the warm-pane fast
    // path (warm panes are replenished in this same directory).
    if let Some(ref dir) = start_dir { env::set_current_dir(dir).ok(); }
    let create_result = if let Some(ref raw_args) = raw_command {
        create_window_raw(&*pty_system, &mut app, raw_args)
    } else {
        create_window(&*pty_system, &mut app, initial_command.as_deref(), None, false)
    };
    if let Err(e) = create_result {
        // Issue #167: when the server fails to spawn its initial pane the
        // detached process exits silently — the user sees only "flashes
        // black and returns to prompt" with no visible error.  Persist the
        // failure to a log file the user can find with their next breath
        // ("look in ~/.psmux/server-startup.log") instead of asking them
        // to rerun `psmux server` interactively to see the error.
        #[cfg(windows)]
        write_startup_error_log(&e);
        // Clean up port and key files so stale entries are not left
        // behind when the pane command fails to spawn (issue #204).
        let _ = std::fs::remove_file(&regpath);
        let _ = std::fs::remove_file(&keypath);
        crate::session::remove_session_id_file(&app.port_file_base());
        // Kill warm pane if one was pre-spawned
        if let Some(mut wp) = app.warm_pane.take() { wp.child.kill().ok(); }
        return Err(e);
    }
    // Resize panes now that the initial window exists and config is loaded.
    // pane-border-status needs 1 row per pane for the border label (#288).
    resize_all_panes(&mut app);
    // Apply window name if specified via -n.  Setting `manual_rename = true`
    // is critical (issue #266) — it implicitly disables automatic-rename for
    // the initial window of a `new-session -n NAME`, matching tmux semantics
    // and the two later `-n` paths in this file (lines ~789, ~812).
    if let Some(n) = window_name {
        app.windows.last_mut().map(|w| { w.name = n; w.manual_rename = true; });
    }
    // Replenish: spawn a warm pane for the NEXT new-window / split.
    // Always replenish when no warm pane is available.
    if app.warm_pane.is_none() {
        match spawn_warm_pane(&*pty_system, &mut app) {
            Ok(wp) => { app.warm_pane = Some(wp); }
            Err(e) => { eprintln!("psmux: warm pane pre-spawn failed: {e}"); }
        }
    }
    // Fire client-attached and session-created hooks once at startup so plugins
    // populate initial data (e.g. CPU/battery) even for detached sessions
    // (tppanel previews). Skip the warm server: firing here would double-fire
    // every client/session hook (e.g. a duplicate continuum auto-save loop). A
    // claimed warm server gets them once it becomes real (see CtrlReq::ClaimSession).
    if !app.is_warm_server() {
        crate::commands::fire_hooks(&mut app, "client-attached");
        crate::commands::fire_hooks(&mut app, "session-created");
    }
    // Spawn a warm server for the NEXT new-session when the current session
    // is allowed to keep background state alive.
    if should_spawn_warm_server(&app) {
        spawn_warm_server(&app);
    }
    let mut state_dirty = true;
    let mut cached_dump_state = String::new();
    let mut cached_data_version: u64 = 0;
    // Issue #7 batch D: the "NC" (no-change) fast path below is a *global*
    // cache shared by every connection. A brand-new persistent connection
    // (e.g. a monitoring/test TCP client) that dials in *after* a real
    // attached TUI client has already gone idle would otherwise receive
    // "NC" as its very first-ever dump-state response — meaningless to a
    // client with no prior frame to diff against, and since nothing then
    // changes, no further response ever gets pushed, so the connection
    // stalls until the writer thread's 5s timeout kills it. Track which
    // client_ids have already been served at least one full frame; only
    // those may receive the "NC" shortcut. Cleared on ClientDetach so this
    // does not grow unbounded across client reconnects.
    let mut dump_state_seen_full: std::collections::HashSet<u64> = std::collections::HashSet::new();
    // Cached metadata JSON — windows/tree/prefix change only on structural
    // mutations, so we rebuild them lazily via `meta_dirty`.
    let mut meta_dirty = true;
    let mut cached_windows_json = String::new();
    let mut cached_tree_json = String::new();
    let mut cached_prefix_str = String::new();
    let mut cached_prefix2_str = String::new();
    let mut cached_base_index: usize = 0;
    let mut cached_pred_dim: bool = false;
    let mut cached_status_style = String::new();
    let mut cached_bindings_json = String::from("[]");
    // Reusable buffer for building the combined JSON envelope.
    let mut combined_buf = String::with_capacity(32768);


    // Track when we recently sent keystrokes to the PTY.  While waiting
    // for the echo to appear we use a much shorter recv_timeout (1ms vs 5ms)
    // so that dump-state requests are served with minimal delay.  This is
    // critical for nested-shell latency (e.g. WSL inside pwsh) where the
    // echo path goes through ConPTY → pwsh → WSL → echo → ConPTY and can
    // take 10-30ms.  Without this, each "no-change" polling cycle costs up
    // to 5ms, adding cumulative latency visible as heavy input lag.
    let mut echo_pending_until: Option<Instant> = None;

    // Track when any client last requested a dump or sent input.
    // Used to ramp down the server loop frequency when truly idle.
    let mut last_client_activity = Instant::now();

    let mut last_registry_check = Instant::now();

    // #559: alert detection (activity/bell/monitor-silence) used to run only
    // inside DumpState handling and the server-push path, both of which need a
    // client. A detached session therefore never evaluated monitor-silence and
    // the silence flag could not fire (tmux fires alerts regardless of
    // attachment). This timestamp gates a client-independent check to 1 Hz.
    let mut last_alert_check = Instant::now();

    // Throttle reap_children: only check for exited processes every 250ms.
    // With hundreds of windows, calling try_wait() on every process each
    // loop iteration wastes CPU.  Exited processes are still reaped promptly
    // (250ms is imperceptible to users).
    let mut last_reap = Instant::now();

    // Persist temp_focus_restore across batch boundaries so that a
    // FocusWindowTemp/FocusPaneByIndexTemp in one batch plus the actual
    // command (e.g. CapturePane) in the next batch still works correctly.
    let mut temp_focus_restore: Option<(usize, usize)> = None;

    loop {
        // Tier 3 — keep warm-pane spawning OFF the command path. A warm pane is a
        // fresh shell + ConPTY; spawning one can block the single event loop for
        // 100ms–seconds under load. Doing it inline after every new-window/split
        // stalled *other* clients' commands, surfacing as the intermittent
        // `os error 10060` timeouts. Instead replenish only during a quiet gap
        // (no command processed in the last 20ms), so window-create bursts
        // transplant the ready pane instantly and the blocking spawn lands in idle
        // time. If no warm pane is ready when a new-window arrives, create_window
        // still spawns one synchronously — correctness is unchanged.
        if app.warm_pane.is_none() && last_client_activity.elapsed() >= Duration::from_millis(20) {
            if let Ok(wp) = spawn_warm_pane(&*pty_system, &mut app) {
                app.warm_pane = Some(wp);
            }
        }
        if last_registry_check.elapsed() >= Duration::from_secs(5) {
            last_registry_check = Instant::now();
            ensure_session_registry_files(&app);
        }

        // Adaptive timeout: ramps from 1ms (active typing/echo) through
        // 5ms (client recently active) up to 50ms (fully idle).  This
        // dramatically reduces CPU usage when the session is idle while
        // keeping responsiveness high during interaction.
        let data_ready = crate::types::PTY_DATA_READY.swap(false, std::sync::atomic::Ordering::AcqRel);
        if data_ready {
            state_dirty = true;
            // #548 follow-up: attribute any newly-enabled mouse protocol to
            // the process that was foreground when it appeared (shell =
            // PSReadLine's spurious tracking, non-shell = a genuine mouse
            // consumer).  Runs on the data tick because a DECSET transition
            // can only happen when pane output was just parsed; the process
            // walk in update_mouse_proto_owner only fires on transitions.
            for win in &mut app.windows {
                crate::tree::for_each_pane_mut(&mut win.root, &mut |pane| {
                    crate::window_ops::update_mouse_proto_owner(pane);
                });
                for fp in win.floating.iter_mut() {
                    crate::window_ops::update_mouse_proto_owner(&mut fp.pane);
                }
            }
            // Drain output ring buffers and send %output notifications to control clients
            if !app.control_clients.is_empty() {
                // Collect output from all panes first, then dispatch to clients
                let mut pane_outputs: Vec<(usize, String)> = Vec::new();
                for win in &app.windows {
                    crate::tree::for_each_pane(&win.root, &mut |pane: &crate::types::Pane| {
                        if let Ok(mut ring) = pane.output_ring.lock() {
                            if !ring.is_empty() {
                                let bytes: Vec<u8> = ring.drain(..).collect();
                                let data = String::from_utf8_lossy(&bytes).to_string();
                                pane_outputs.push((pane.id, data));
                            }
                        }
                    });
                }
                // Dispatch to each control client with pause-after logic
                let now = std::time::Instant::now();
                for (pane_id, data) in &pane_outputs {
                    for client in app.control_clients.values_mut() {
                        if client.paused_panes.contains(pane_id) {
                            continue;
                        }
                        if client.output_paused_panes.contains(pane_id) {
                            // Pane is paused for this client; drop output
                            continue;
                        }
                        if let Some(pause_secs) = client.pause_after_secs {
                            // Track output timing per pane
                            let last = client.pane_last_output.entry(*pane_id).or_insert(now);
                            let age = now.duration_since(*last);
                            *last = now;
                            if age.as_secs() >= pause_secs {
                                // Client fell behind: pause this pane
                                client.output_paused_panes.insert(*pane_id);
                                let _ = client.notification_tx.try_send(
                                    crate::types::ControlNotification::Pause { pane_id: *pane_id }
                                );
                                continue;
                            }
                            // Send as extended-output with age
                            let age_ms = age.as_millis() as u64;
                            let _ = client.notification_tx.try_send(
                                crate::types::ControlNotification::ExtendedOutput {
                                    pane_id: *pane_id,
                                    age_ms,
                                    data: data.clone(),
                                }
                            );
                        } else {
                            // No pause-after: send normal %output
                            let _ = client.notification_tx.try_send(
                                crate::types::ControlNotification::Output {
                                    pane_id: *pane_id,
                                    data: data.clone(),
                                }
                            );
                        }
                    }
                }
            }
            // Answer any ESC[6n queries — pwsh re-issues this after lock/unlock.
            if crate::types::CPR_DATA_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel) {
                for win in &mut app.windows {
                    helpers::drain_cpr_pending(&mut win.root);
                }
                // Also answer CPR queries for the warm pane — it is not in
                // any window yet, but pwsh / PSReadLine blocks on the ESC[6n
                // response during shell startup.  Without this, the warm
                // pane's shell never finishes loading.
                if let Some(ref mut wp) = app.warm_pane {
                    if wp.cpr_pending.swap(false, std::sync::atomic::Ordering::AcqRel) {
                        let (r, c) = wp.term.lock()
                            .map(|g| g.screen().cursor_position())
                            .unwrap_or((0, 0));
                        let response = format!("\x1b[{};{}R", r + 1, c + 1);
                        use std::io::Write as _;
                        let _ = wp.writer.write_all(response.as_bytes());
                        let _ = wp.writer.flush();
                    }
                }
                // Also answer CPR queries for an active popup PTY pane. Like the
                // warm pane it is not in any window tree, but an interactive popup
                // shell (PSReadLine / fzf) blocks on the ESC[6n response and would
                // otherwise render blank forever (#351).
                if let Mode::PopupMode { popup_pane: Some(ref mut pane), .. } = app.mode {
                    if pane.cpr_pending.swap(false, std::sync::atomic::Ordering::AcqRel) {
                        let (r, c) = pane.term.lock()
                            .map(|g| g.screen().cursor_position())
                            .unwrap_or((0, 0));
                        let response = format!("\x1b[{};{}R", r + 1, c + 1);
                        use std::io::Write as _;
                        let _ = pane.writer.write_all(response.as_bytes());
                        let _ = pane.writer.flush();
                    }
                }
                // Answer CPR for the active window's floating panes too — same
                // reason as popups: an interactive float shell blocks on ESC[6n.
                if let Some(win) = app.windows.get_mut(app.active_idx) {
                    for fp in win.floating.iter_mut() {
                        if fp.pane.cpr_pending.swap(false, std::sync::atomic::Ordering::AcqRel) {
                            let (r, c) = fp.pane.term.lock()
                                .map(|g| g.screen().cursor_position())
                                .unwrap_or((0, 0));
                            let response = format!("\x1b[{};{}R", r + 1, c + 1);
                            use std::io::Write as _;
                            let _ = fp.pane.writer.write_all(response.as_bytes());
                            let _ = fp.pane.writer.flush();
                        }
                    }
                }
            }
            // Issue #473: answer terminal color queries (OSC 4/10/11, CSI ?996n)
            // detected in pane output, so pane applications can discover the
            // terminal palette.  Colors come from the attached client's report
            // of its host terminal, else the Campbell defaults.
            if crate::types::COLOR_QUERY_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel) {
                let colors = app.host_colors.clone()
                    .unwrap_or_else(crate::types::HostColors::campbell);
                for win in &mut app.windows {
                    helpers::drain_color_queries(&mut win.root, &colors);
                    for fp in win.floating.iter_mut() {
                        let bits = fp.pane.color_query_pending.swap(0, std::sync::atomic::Ordering::AcqRel);
                        if bits != 0 {
                            helpers::answer_color_queries(bits, &mut *fp.pane.writer, fp.pane.child_pid, &colors);
                        }
                    }
                }
                if let Mode::PopupMode { popup_pane: Some(ref mut pane), .. } = app.mode {
                    let bits = pane.color_query_pending.swap(0, std::sync::atomic::Ordering::AcqRel);
                    if bits != 0 {
                        helpers::answer_color_queries(bits, &mut *pane.writer, pane.child_pid, &colors);
                    }
                }
            }
        }
        // When a popup PTY or a floating pane is active, always push frames so
        // interactive content (fzf, shell prompts) updates in real-time.
        if matches!(app.mode, Mode::PopupMode { .. })
            || app.windows.get(app.active_idx).map_or(false, |w| !w.floating.is_empty())
        {
            state_dirty = true;
        }
        let echo_active = echo_pending_until.map_or(false, |t| t.elapsed().as_millis() < 50);
        let idle_secs = last_client_activity.elapsed().as_secs();
        let timeout_ms: u64 = if echo_active || data_ready {
            1      // Active echo/data: 1ms for maximum responsiveness
        } else if idle_secs < 2 {
            5      // Recently active: 5ms (200 Hz)
        } else if crate::types::has_frame_receivers() {
            16     // Push clients attached: 16ms (~60 Hz) so PTY data
                   // is detected and pushed within one vsync period.
        } else {
            50     // No clients: 50ms (20 Hz) — saves CPU
        };
        // #559: run alert detection on a 1s cadence independent of clients so
        // monitor-silence/monitor-activity/bell flags fire in detached
        // sessions too (scripts read them via list-windows #{window_flags}).
        if last_alert_check.elapsed() >= Duration::from_secs(1) {
            last_alert_check = Instant::now();
            let alert_hooks = helpers::check_window_activity(&mut app);
            for event in &alert_hooks {
                crate::commands::fire_hooks(&mut app, event);
            }
        }
        if let Some(rx) = app.control_rx.as_ref() {
            if let Ok(req) = rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                last_client_activity = Instant::now();
                let mut pending = vec![req];
                // Drain any additional queued messages without blocking
                while let Ok(r) = rx.try_recv() {
                    pending.push(r);
                }
                // Also check if fresh PTY output arrived while we were
                // waiting – mark state dirty so DumpState produces a full
                // frame instead of "NC".
                if crate::types::PTY_DATA_READY.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    state_dirty = true;
                }
                // Process key/command inputs BEFORE dump-state requests.
                // This ensures ConPTY receives keystrokes before we serialize
                // the screen, reducing stale-frame responses.
                pending.sort_by_key(|r| match r {
                    CtrlReq::DumpState(..) => 1,
                    CtrlReq::DumpLayout(_) => 1,
                    CtrlReq::WindowDump(..) => 1,
                    _ => 0,
                });
                // Track temporary -t focus: save (active_idx, pane_id) when
                // FocusWindowTemp/FocusPaneTemp is seen, restore after next
                // non-temp command so the user's view doesn't jump.
                // We store the pane ID (not path) because kill-pane
                // restructures the tree, invalidating saved paths (#71).
                // NOTE: temp_focus_restore lives outside the loop so it
                // persists across batch boundaries (prevents race where
                // FocusWindowTemp and the actual command land in different
                // batches).
                for req in pending {
                    let mutates_state = !matches!(&req,
                        CtrlReq::DumpState(..)
                        | CtrlReq::SendText(_)
                        | CtrlReq::SendKey(_)
                        | CtrlReq::SendPaste(_)
                        | CtrlReq::WindowDump(..)
                        | CtrlReq::WindowLayout(..)
                    );
                    let is_temp_focus = matches!(&req, CtrlReq::FocusTargetTemp { .. });
                    let mut hook_event: Option<&str> = None;
                    // Track active_idx changes for debugging window-switch issues
                    let _prev_active_idx = app.active_idx;
                    let _req_tag: &str = match &req {
                        CtrlReq::NextWindow => "NextWindow",
                        CtrlReq::PrevWindow => "PrevWindow",
                        CtrlReq::SelectWindow(_) => "SelectWindow",
                        CtrlReq::FocusWindow(_) => "FocusWindow",
                        CtrlReq::FocusWindowById(_) => "FocusWindowById",
                        CtrlReq::FocusWindowByName(_) => "FocusWindowByName",
                        CtrlReq::FocusTargetTemp { .. } => "FocusTargetTemp",
                        CtrlReq::FocusWindowCmd(_) => "FocusWindowCmd",
                        CtrlReq::LastWindow => "LastWindow",
                        CtrlReq::MouseDown(..) => "MouseDown",
                        CtrlReq::MouseDownRight(..) => "MouseDownRight",
                        CtrlReq::MouseDownMiddle(..) => "MouseDownMiddle",
                        CtrlReq::FocusPane(_) => "FocusPane",
                        CtrlReq::NewWindow(..) => "NewWindow",
                        CtrlReq::KillWindow => "KillWindow",
                        CtrlReq::KillPane => "KillPane",
                        CtrlReq::KillPaneById(_) => "KillPaneById",
                        CtrlReq::BreakPane => "BreakPane",
                        CtrlReq::JoinPane { .. } => "JoinPane",
                        CtrlReq::MovePane { .. } => "MovePane",
                        CtrlReq::PaneForwardExtract(..) => "PaneForwardExtract",
                        CtrlReq::PaneForwardInject { .. } => "PaneForwardInject",
                        CtrlReq::PaneForwardResize(..) => "PaneForwardResize",
                        CtrlReq::PaneForwardStatus(..) => "PaneForwardStatus",
                        CtrlReq::PaneForwardKill(..) => "PaneForwardKill",
                        CtrlReq::MoveWindow { .. } => "MoveWindow",
                        CtrlReq::SwapWindow { .. } => "SwapWindow",
                        _ => "",
                    };
                    match req {
                CtrlReq::NewWindow(cmd, name, detached, start_dir, title, empty, env_sets) => {
                    if let Some(cmds) = app.hooks.get("before-new-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    let prev_idx = app.active_idx;
                    // Expand format variables like #{pane_current_path} (#111)
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let saved_dir = if start_dir.is_some() { env::current_dir().ok() } else { None };
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    if let Err(e) = create_window_with_env(&*pty_system, &mut app, cmd.as_deref(), start_dir.as_deref(), empty, &env_sets) {
                        eprintln!("psmux: new-window error: {e}");
                    }
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    if let Some(prev) = saved_dir { env::set_current_dir(prev).ok(); }
                    if let Some(n) = name { app.windows.last_mut().map(|w| { w.name = n; w.manual_rename = true; }); }
                    // -T: set the new pane's title at creation (tmux new-window -T).
                    if let Some(t) = title {
                        if let Some(win) = app.windows.last_mut() {
                            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                                p.title_locked = !t.is_empty();
                                p.title = t;
                            }
                        }
                    }
                    if detached { app.active_idx = prev_idx; }
                    // Replenish warm pane pool for next new-window
                    // Warm-pane replenish is deferred OFF the command path — it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-new-window");
                }
                CtrlReq::NewWindowPrint(cmd, name, detached, start_dir, format_str, resp, title, empty, env_sets) => {
                    if let Some(cmds) = app.hooks.get("before-new-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    let prev_idx = app.active_idx;
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let saved_dir = if start_dir.is_some() { env::current_dir().ok() } else { None };
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    if let Err(e) = create_window_with_env(&*pty_system, &mut app, cmd.as_deref(), start_dir.as_deref(), empty, &env_sets) {
                        eprintln!("psmux: new-window error: {e}");
                    }
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    if let Some(prev) = saved_dir { env::set_current_dir(prev).ok(); }
                    if let Some(n) = name { app.windows.last_mut().map(|w| { w.name = n; w.manual_rename = true; }); }
                    if let Some(t) = title {
                        if let Some(win) = app.windows.last_mut() {
                            if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                                p.title_locked = !t.is_empty();
                                p.title = t;
                            }
                        }
                    }
                    // Use full format engine for -P output (tmux compatible)
                    let new_win_idx = app.windows.len() - 1;
                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}");
                    let pane_info = crate::format::expand_format_for_window(fmt, &app, new_win_idx);
                    if detached { app.active_idx = prev_idx; }
                    let _ = resp.send(pane_info);
                    // Replenish warm pane pool for next new-window
                    // Warm-pane replenish is deferred OFF the command path — it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-new-window");
                }
                CtrlReq::SplitWindow(k, cmd, detached, start_dir, split_size, resp, title, env_sets, zoom_after_split) => {
                    if let Some(cmds) = app.hooks.get("before-split-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // tmux: split-window without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    // tmux: split-window INSIDE a floating pane creates ANOTHER
                    // floating pane (offset from it), not a tiled split.
                    let float_src = {
                        let win = &app.windows[app.active_idx];
                        win.floating_focus.and_then(|fi| win.floating.get(fi)).map(|fp| (fp.x, fp.y, fp.w, fp.h, fp.border.clone()))
                    };
                    if let Some((sx, sy, sw, sh, sborder)) = float_src {
                        // Floating panes are overlays, so tiled-window zoom does
                        // not apply to splits created from a floating pane.
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let nx = (sx + 2).min(win_w.saturating_sub(sw));
                        let ny = (sy + 2).min(win_h.saturating_sub(sh));
                        let inner_h = sh.saturating_sub(2).max(1);
                        let inner_w = sw.saturating_sub(2).max(1);
                        let cmdstr = cmd.clone().unwrap_or_default();
                        let sd = start_dir.clone().map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                        let pane_id = app.next_pane_id;
                        if let Some(mut pane) = crate::popup::create_popup_pane(&cmdstr, sd.as_deref(), inner_h, inner_w, pane_id, &app.session_name, &app.environment, app.host_colors.as_ref()) {
                            app.next_pane_id += 1;
                            let t = title.clone().unwrap_or_default();
                            if !t.is_empty() { pane.title = t.clone(); pane.title_locked = true; }
                            let win = &mut app.windows[app.active_idx];
                            win.floating.push(crate::types::FloatingPane { pane, x: nx, y: ny, w: sw, h: sh, border: sborder, id: pane_id, title: t, position: None });
                            if !detached { win.floating_focus = Some(win.floating.len() - 1); }
                            state_dirty = true;
                        }
                        let _ = resp.send(String::new());
                    } else {
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let saved_dir = if start_dir.is_some() { env::current_dir().ok() } else { None };
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let prev_path = app.windows[app.active_idx].active_path.clone();
                    let split_succeeded = match split_active_with_env(&mut app, k, cmd.as_deref(), Some(&*pty_system), start_dir.as_deref(), &env_sets) {
                        Err(e) => {
                            let msg = format!("split-window: {e}");
                            app.status_message = Some((msg.clone(), std::time::Instant::now(), None));
                            let _ = resp.send(format!("psmux: {msg}"));
                            false
                        }
                        Ok(()) => {
                            let _ = resp.send(String::new());
                            true
                        }
                    };
                    // Apply size if specified: (value, true) = percentage, (value, false) = cell count
                    if let Some((val, is_pct)) = split_size {
                        let pct = if is_pct {
                            val.clamp(1, 99)
                        } else {
                            // Convert cell count to percentage based on split direction
                            let area = app.last_window_area;
                            let total = if k == LayoutKind::Horizontal { area.width } else { area.height };
                            if total > 0 { ((val as u32 * 100) / total as u32).clamp(1, 99) as u16 } else { 50 }
                        };
                        let win = &mut app.windows[app.active_idx];
                        if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &prev_path) {
                            sizes[0] = 100 - pct;
                            sizes[1] = pct;
                        }
                    }
                    // -T: the just-created pane is currently active in this
                    // window, so set its title before any detached revert.
                    if let Some(t) = title {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                    }
                    if detached {
                        // Capture new pane ID before reverting focus
                        let new_pane_id = crate::tree::get_active_pane_id(
                            &app.windows[app.active_idx].root,
                            &app.windows[app.active_idx].active_path,
                        );
                        // Revert focus to the previously active pane.
                        // After split, prev_path now points to a Split node;
                        // the original pane is child [0] of that Split.
                        let mut revert_path = prev_path;
                        revert_path.push(0);
                        app.windows[app.active_idx].active_path = revert_path;
                        // Detached splits never focus the new pane — remove
                        // from MRU entirely so directional nav tie-breaks by
                        // pane_index among equally-unvisited candidates (#70).
                        if let Some(nid) = new_pane_id {
                            let win = &mut app.windows[app.active_idx];
                            win.pane_mru.retain(|&id| id != nid);
                        }
                    } else {
                        // Non-detached: new pane keeps focus.
                        // Cancel temp_focus_restore so -t doesn't revert (#112).
                        // The temporary focus just became permanent.
                        temp_focus_restore = None;
                        app.temp_focus_saved_active = None;
                    }
                    if zoom_after_split && split_succeeded {
                        toggle_zoom(&mut app);
                    }
                    if let Some(prev) = saved_dir { env::set_current_dir(prev).ok(); }
                    // Replenish warm pane for the next new-window/split
                    // Warm-pane replenish is deferred OFF the command path — it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                    }
                }
                CtrlReq::SplitWindowPrint(k, cmd, detached, start_dir, split_size, format_str, resp, title, env_sets, zoom_after_split) => {
                    if let Some(cmds) = app.hooks.get("before-split-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    unzoom_if_zoomed(&mut app);
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let saved_dir = if start_dir.is_some() { env::current_dir().ok() } else { None };
                    if let Some(dir) = &start_dir { env::set_current_dir(dir).ok(); }
                    let prev_path = app.windows[app.active_idx].active_path.clone();
                    let split_succeeded = match split_active_with_env(&mut app, k, cmd.as_deref(), Some(&*pty_system), start_dir.as_deref(), &env_sets) {
                        Err(e) => {
                            app.status_message = Some((format!("split-window: {e}"), std::time::Instant::now(), None));
                            eprintln!("psmux: split-window error: {e}");
                            false
                        }
                        Ok(()) => true,
                    };
                    // Apply size if specified: (value, true) = percentage, (value, false) = cell count
                    if let Some((val, is_pct)) = split_size {
                        let pct = if is_pct {
                            val.clamp(1, 99)
                        } else {
                            let area = app.last_window_area;
                            let total = if k == LayoutKind::Horizontal { area.width } else { area.height };
                            if total > 0 { ((val as u32 * 100) / total as u32).clamp(1, 99) as u16 } else { 50 }
                        };
                        let win = &mut app.windows[app.active_idx];
                        if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &prev_path) {
                            sizes[0] = 100 - pct;
                            sizes[1] = pct;
                        }
                    }
                    // -T: set the new (currently active) pane's title.
                    if let Some(t) = title {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                    }
                    // Use full format engine for -P output (tmux compatible)
                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}.#{pane_index}");
                    let pane_info = crate::format::expand_format_for_window(fmt, &app, app.active_idx);
                    if detached {
                        // Capture new pane ID before reverting focus
                        let new_pane_id = crate::tree::get_active_pane_id(
                            &app.windows[app.active_idx].root,
                            &app.windows[app.active_idx].active_path,
                        );
                        let mut revert_path = prev_path;
                        revert_path.push(0);
                        app.windows[app.active_idx].active_path = revert_path;
                        // Detached splits: remove from MRU (#70 pane_index tie-break)
                        if let Some(nid) = new_pane_id {
                            let win = &mut app.windows[app.active_idx];
                            win.pane_mru.retain(|&id| id != nid);
                        }
                    } else {
                        temp_focus_restore = None;
                        app.temp_focus_saved_active = None;
                    }
                    if zoom_after_split && split_succeeded {
                        toggle_zoom(&mut app);
                    }
                    let _ = resp.send(pane_info);
                    if let Some(prev) = saved_dir { env::set_current_dir(prev).ok(); }
                    // Replenish warm pane
                    // Warm-pane replenish is deferred OFF the command path — it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                }
                CtrlReq::KillPane => {
                    if let Some(cmds) = app.hooks.get("before-kill-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // A focused floating pane is closed by kill-pane instead of a
                    // tiled pane. The child is dropped with the FloatingPane.
                    let closed_float = {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if fi < win.floating.len() {
                                let mut fp = win.floating.remove(fi);
                                let _ = fp.pane.child.kill();
                                win.floating_focus = if win.floating.is_empty() { None } else { Some(win.floating.len() - 1) };
                                true
                            } else { false }
                        } else { false }
                    };
                    if closed_float {
                        state_dirty = true;
                    } else {
                        unzoom_if_zoomed(&mut app); let _ = kill_active_pane(&mut app); resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-kill-pane");
                    }
                }
                CtrlReq::KillPaneById(pid) => {
                    if let Some(cmds) = app.hooks.get("before-kill-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    unzoom_if_zoomed(&mut app); let _ = kill_pane_by_id(&mut app, pid); resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-kill-pane");
                }
                CtrlReq::CapturePane(resp, pane_id, preserve_trailing) => {
                    // Note: do NOT gate on is_active_pane_squelched here.
                    // Returning empty during the cd+cls squelch window makes
                    // iTerm2's initial attach paint a blank screen, since
                    // capture-pane is only requested once on attach.  Return
                    // current parser screen content; it's just cell text and
                    // any stale frame is harmless (subsequent %output rewrites).
                    if let Some(text) = capture_active_pane_text(&mut app, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneStyled(resp, s, e, pane_id, preserve_trailing) => {
                    if let Some(text) = capture_active_pane_styled(&mut app, s, e, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::CapturePaneRange(resp, s, e, pane_id, preserve_trailing) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                CtrlReq::FocusWindow(wid) => {
                    // wid is a display index (same as tmux window number), convert to internal array index
                    if let Some(internal_idx) = app.win_pos(wid) {
                        if internal_idx != app.active_idx {
                            switch_with_copy_save(&mut app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                            // Clear activity/bell/silence flags on the newly-focused window
                            if let Some(win) = app.windows.get_mut(internal_idx) {
                                win.activity_flag = false;
                                win.bell_flag = false;
                                win.silence_flag = false;
                            }
                            // Lazily resize panes in the newly-focused window
                            resize_all_panes(&mut app);
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::FocusWindowByName(ref name) => {
                    if let Some(internal_idx) = app.windows.iter().position(|w| w.name == *name) {
                        if internal_idx != app.active_idx {
                            switch_with_copy_save(&mut app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                            if let Some(win) = app.windows.get_mut(internal_idx) {
                                win.activity_flag = false;
                                win.bell_flag = false;
                                win.silence_flag = false;
                            }
                            resize_all_panes(&mut app);
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::FocusWindowById(id) => {
                    if let Some(internal_idx) = app.windows.iter().position(|w| w.id == id) {
                        if internal_idx != app.active_idx {
                            switch_with_copy_save(&mut app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                            if let Some(win) = app.windows.get_mut(internal_idx) {
                                win.activity_flag = false;
                                win.bell_flag = false;
                                win.silence_flag = false;
                            }
                            resize_all_panes(&mut app);
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::FocusPane(pid) => {
                    let old_path = app.windows[app.active_idx].active_path.clone();
                    switch_with_copy_save(&mut app, |app| { focus_pane_by_id(app, pid); });
                    if app.windows[app.active_idx].active_path != old_path { unzoom_if_zoomed(&mut app); }
                    meta_dirty = true;
                }
                CtrlReq::FocusPaneByIndex(idx) => {
                    let old_path = app.windows[app.active_idx].active_path.clone();
                    switch_with_copy_save(&mut app, |app| { focus_pane_by_index(app, idx); });
                    if app.windows[app.active_idx].active_path != old_path { unzoom_if_zoomed(&mut app); }
                    // Update MRU so directional navigation remembers this focus change
                    let win = &mut app.windows[app.active_idx];
                    if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                        crate::tree::touch_mru(&mut win.pane_mru, pid);
                    }
                    meta_dirty = true;
                }
                // ── Temporary focus for -t targeting ─────────────────────
                // Switches active_idx/active_path so the NEXT command in
                // the batch operates on the correct window/pane. After the
                // entire pending batch is processed, we restore the original
                // focus (see temp_focus_restore below).
                //
                // Resolution happens BEFORE any focus change: an
                // unresolvable target must be an error with zero side
                // effects, never a silent fallback to the active window
                // (issue #545 — the old per-kind Temp handlers had no miss
                // path, so the untargeted command that followed ran against
                // whatever was focused). Same convention as KillWindowTarget.
                CtrlReq::FocusTargetTemp { win, win_is_id, win_name, pane, pane_is_id, resp } => {
                    let win_idx: Option<usize> = if let Some(w) = win {
                        if win_is_id {
                            app.windows.iter().position(|x| x.id == w)
                        } else {
                            app.win_pos(w)
                        }
                    } else if let Some(ref n) = win_name {
                        app.windows.iter().position(|x| x.name == *n)
                    } else {
                        Some(app.active_idx)
                    };
                    let err: Option<String> = match win_idx {
                        None => {
                            let spec = if let Some(w) = win {
                                if win_is_id { format!("@{}", w) } else { w.to_string() }
                            } else {
                                win_name.clone().unwrap_or_default()
                            };
                            Some(format!("can't find window: {}", spec))
                        }
                        Some(idx) => match pane {
                            Some(p) if pane_is_id => {
                                if crate::tree::find_pane_by_id_global(&app, p).is_none() {
                                    Some(format!("can't find pane: %{}", p))
                                } else {
                                    None
                                }
                            }
                            Some(p) => {
                                // Positional index within the target window,
                                // matching what focus_pane_by_index resolves.
                                if p >= crate::tree::count_panes(&app.windows[idx].root) {
                                    Some(format!("can't find pane: {}", p))
                                } else {
                                    None
                                }
                            }
                            None => None,
                        },
                    };
                    match err {
                        Some(msg) => {
                            // Surface in the status bar for attached clients,
                            // same convention as join-pane (#437) and
                            // kill-window (8edd1cb). The Err reply makes the
                            // connection thread skip the follow-on command.
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                        None => {
                            if temp_focus_restore.is_none() {
                                let pane_id = crate::tree::get_active_pane_id(
                                    &app.windows[app.active_idx].root,
                                    &app.windows[app.active_idx].active_path,
                                ).unwrap_or(usize::MAX);
                                temp_focus_restore = Some((app.active_idx, pane_id));
                                // Remember the REAL active window so format
                                // evaluation of #{window_active} and the `*`
                                // flag is not fooled by the temporary switch
                                // (issue #551).
                                app.temp_focus_saved_active = Some(app.active_idx);
                            }
                            if win.is_some() || win_name.is_some() {
                                if let Some(internal_idx) = win_idx {
                                    app.active_idx = internal_idx;
                                    app.last_window_area = app.windows[internal_idx].area;
                                }
                            }
                            match pane {
                                Some(p) if pane_is_id => {
                                    // Use no-MRU variant: temporary -t targeting
                                    // should not pollute the recency list (#71).
                                    focus_pane_by_id_no_mru(&mut app, p);
                                }
                                Some(p) => {
                                    focus_pane_by_index(&mut app, p);
                                }
                                None => {}
                            }
                            let _ = resp.send(Ok(()));
                        }
                    }
                }
                CtrlReq::SessionInfo(resp) => {
                    let num_attached = app.client_registry.len();
                    let attached = if num_attached > 0 { " (attached)" } else { "" };
                    let group = if let Some(ref g) = app.session_group {
                        format!(" (group {})", g)
                    } else {
                        String::new()
                    };
                    let windows = app.windows.len();
                    let created = app.created_at.format("%a %b %e %H:%M:%S %Y");
                    let line = format!("{}: {} windows (created {}){}{}\n", app.session_name, windows, created, group, attached);
                    let _ = resp.send(line);
                }
                CtrlReq::SessionInfoFormat(resp, fmt) => {
                    let line = crate::format::format_list_sessions(&app, &fmt);
                    let _ = resp.send(format!("{}\n", line));
                }
                CtrlReq::ExpandFormat(fmt, resp) => {
                    // Synchronous by design: the caller is a connection thread
                    // that is about to run the expanded string, and it has
                    // already decided the round trip is worth it (it only sends
                    // this when the command actually contains `#{`).
                    let _ = resp.send(expand_format(&fmt, &app));
                }
                CtrlReq::ClientAttach(cid) => {
                    // Registration and the attached counter are one idempotent
                    // operation. A duplicate attach for the same connection
                    // must not leave the session permanently ghost-attached.
                    if app.register_client(cid, false) {
                        hook_event = Some("client-attached");
                        // update-environment: refresh env vars from the attaching client's environment
                        let update_vars = app.update_environment.clone();
                        for var_spec in &update_vars {
                            let remove = var_spec.starts_with('-');
                            let name = if remove { &var_spec[1..] } else { var_spec.as_str() };
                            if remove {
                                app.environment.remove(name);
                            } else if let Ok(val) = std::env::var(name) {
                                app.environment.insert(name.to_string(), val);
                            } else {
                                app.environment.remove(name);
                            }
                        }
                    }
                }
                CtrlReq::ClientDetach(cid) => {
                    // Issue #7 batch D: forget this client_id's dump-state
                    // "has seen a full frame" bookkeeping so a future
                    // reconnect (new TCP connection, same or different
                    // client_id namespace) never wrongly inherits it.
                    dump_state_seen_full.remove(&cid);
                    // Route through the idempotent reaper so a duplicate detach
                    // for one `cid` (e.g. reader-EOF and writer-teardown both
                    // observing the same dead connection) cannot over-decrement
                    // `attached_clients` or re-run the destroy-unattached path.
                    // Side effects run only on a real reap.
                    if app.reap_client(cid) {
                        // Recompute dynamic windows from the remaining clients.
                        if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        hook_event = Some("client-detached");
                        if app.attached_clients == 0 && app.destroy_unattached {
                            let regpath = crate::paths::port_file(&app.port_file_base());
                            let keypath = crate::paths::key_file(&app.port_file_base());
                            let _ = std::fs::remove_file(&regpath);
                            let _ = std::fs::remove_file(&keypath);
                            crate::session::remove_session_id_file(&app.port_file_base());
                            crate::types::shutdown_persistent_streams();
                            tree::kill_all_children_batch(&mut app.windows);
                            if let Some(mut wp) = app.warm_pane.take() {
                                wp.child.kill().ok();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(10));
                            std::process::exit(0);
                        }
                    }
                }
                CtrlReq::DumpLayout(resp) => {
                    let json = dump_layout_json(&mut app)?;
                    let _ = resp.send(json);
                }
                CtrlReq::DumpState(resp, allow_nc, dump_client_id) => {
                    // ── Activity / bell / silence detection ──
                    let alert_hooks = helpers::check_window_activity(&mut app);
                    for event in &alert_hooks {
                        if let Some(cmds) = app.hooks.get(*event) { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    }

                    // ── Propagate OSC 0/2 titles to pane.title ──
                    if helpers::propagate_osc_titles(&mut app) {
                        state_dirty = true;
                    }

                    // ── Automatic rename / allow-rename: resolve window names ──
                    {
                        let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                        let auto_rename = app.automatic_rename;
                        let allow_rename = app.allow_rename;
                        if (auto_rename || allow_rename) && !in_copy {
                            for win in app.windows.iter_mut() {
                                if win.manual_rename { continue; }
                                if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                                    if p.dead { continue; }
                                    if p.last_title_check.elapsed().as_millis() < 1000 { continue; }
                                    p.last_title_check = std::time::Instant::now();
                                    if p.child_pid.is_none() {
                                        p.child_pid = crate::platform::mouse_inject::get_child_pid(&*p.child);
                                    }
                                    let new_name = if auto_rename {
                                        // automatic-rename: use foreground process name
                                        if let Some(pid) = p.child_pid {
                                            match crate::platform::process_info::get_foreground_process_name(pid) {
                                                Some(name) => name,
                                                None => {
                                                    // No foreground child found.  Keep the current
                                                    // window name to avoid flashing to the shell
                                                    // name before a child process spawns (#229).
                                                    // Once a child appears, auto-rename will pick
                                                    // it up on the next tick.
                                                    continue;
                                                }
                                            }
                                        } else if allow_rename && !p.title.is_empty() {
                                            p.title.clone()
                                        } else {
                                            continue;
                                        }
                                    } else if allow_rename {
                                        // allow-rename only: use OSC title from child
                                        if let Ok(parser) = p.term.lock() {
                                            let title = parser.screen().title();
                                            if !title.is_empty() {
                                                title.to_string()
                                            } else {
                                                continue;
                                            }
                                        } else {
                                            continue;
                                        }
                                    } else {
                                        continue;
                                    };
                                    if !new_name.is_empty() && win.name != new_name {
                                        win.name = new_name;
                                        meta_dirty = true;
                                        state_dirty = true;
                                    }
                                }
                            }
                        }
                    }
                    // Fast-path: nothing changed at all → 2-byte "NC" marker
                    // instead of cloning 50-100KB of JSON.
                    // Only allowed for persistent connections that already have
                    // the previous frame; one-shot connections always need full state.
                    let has_squelch = app.windows.get(app.active_idx)
                        .and_then(|w| crate::tree::active_pane(&w.root, &w.active_path))
                        .map_or(false, |p| p.squelch_until.is_some());
                    if allow_nc
                        && !state_dirty
                        && !app.bell_forward
                        && !has_squelch
                        && !cached_dump_state.is_empty()
                        && cached_data_version == combined_data_version(&app)
                        && dump_state_seen_full.contains(&dump_client_id)
                    {
                        let _ = resp.send("NC".to_string());
                        continue;
                    }
                    // Rebuild metadata cache if structural changes happened.
                    if meta_dirty {
                        cached_windows_json = list_windows_json_with_tabs(&app)?;
                        cached_tree_json = list_tree_json(&app)?;
                        cached_prefix_str = format_key_binding(&app.prefix_key);
                        cached_prefix2_str = app.prefix2_key.as_ref().map(|k| format_key_binding(k)).unwrap_or_default();
                        cached_base_index = app.window_base_index;
                        cached_pred_dim = app.prediction_dimming;
                        cached_status_style = app.status_style.clone();
                        cached_bindings_json = serialize_bindings_json(&app);
                        meta_dirty = false;
                    }
                    let _t_layout = std::time::Instant::now();
                    let layout_json = dump_layout_json_fast(&mut app)?;
                    let _layout_ms = _t_layout.elapsed().as_micros();
                    combined_buf.clear();
                    // #372: style options must be format-expanded too, so a
                    // #{@var} colour reference resolves before the client's
                    // colour parser sees it (otherwise status-style falls back
                    // to bright green). wsf/wscf stay raw: they are per-window
                    // formats the client expands with each window's own context.
                    // #() in these periodic status/style formats expands ASYNC so
                    // a slow shell helper never blocks the server loop. The guard
                    // now lives inside expand_status_formats, so one-shot
                    // expansions elsewhere (display-message -p) still run
                    // synchronously (see format.rs) without anything to remember
                    // here.
                    let sf = helpers::expand_status_formats(&app, &cached_status_style);
                    let ss_escaped = json_escape_string(&sf.status_style);
                    let sl_expanded = json_escape_string(&sf.status_left);
                    let sr_expanded = json_escape_string(&sf.status_right);
                    let pbs_escaped = json_escape_string(&sf.pane_border_style);
                    let pabs_escaped = json_escape_string(&sf.pane_active_border_style);
                    let pbhs_escaped = json_escape_string(&sf.pane_border_hover_style);
                    let wsf_escaped = json_escape_string(&app.window_status_format);
                    let wscf_escaped = json_escape_string(&app.window_status_current_format);
                    let wss_escaped = json_escape_string(&sf.window_status_separator);
                    let ws_style_escaped = json_escape_string(&sf.window_status_style);
                    let wsc_style_escaped = json_escape_string(&sf.window_status_current_style);
                    let mode_style_escaped = json_escape_string(&sf.mode_style);
                    // #372: message-style was never sent to the client (it
                    // hard-coded bg=yellow,fg=black). Send it, format-expanded.
                    let message_style_escaped = json_escape_string(&sf.message_style);
                    let status_position_escaped = json_escape_string(&app.status_position);
                    let status_justify_escaped = json_escape_string(&app.status_justify);
                    let status_format_json = &sf.status_format_json;
                    let cursor_style_code = crate::rendering::configured_cursor_code();
                    let _ = std::fmt::Write::write_fmt(&mut combined_buf, format_args!(
                        "{{\"layout\":{},\"windows\":{},\"prefix\":\"{}\",\"prefix2\":\"{}\",\"tree\":{},\"base_index\":{},\"pane_base_index\":{},\"prediction_dimming\":{},\"status_style\":\"{}\",\"status_left\":\"{}\",\"status_right\":\"{}\",\"pane_border_style\":\"{}\",\"pane_active_border_style\":\"{}\",\"pane_border_hover_style\":\"{}\",\"wsf\":\"{}\",\"wscf\":\"{}\",\"wss\":\"{}\",\"ws_style\":\"{}\",\"wsc_style\":\"{}\",\"clock_mode\":{},\"bindings\":{},\"status_left_length\":{},\"status_right_length\":{},\"status_lines\":{},\"status_format\":{},\"mode_style\":\"{}\",\"message_style\":\"{}\",\"status_position\":\"{}\",\"status_justify\":\"{}\",\"cursor_style_code\":{},\"status_visible\":{},\"repeat_time\":{},\"zoomed\":{},\"defaults_suppressed\":{},\"pwsh_mouse_selection\":{},\"mouse_selection\":{},\"mouse_selection_force\":{},\"paste_detection\":{},\"choose_tree_preview\":{},\"scroll_enter_copy_mode\":{},\"bold_is_bright\":{}}}",
                        layout_json, cached_windows_json, cached_prefix_str, cached_prefix2_str, cached_tree_json, cached_base_index, app.pane_base_index, cached_pred_dim, ss_escaped, sl_expanded, sr_expanded, pbs_escaped, pabs_escaped, pbhs_escaped, wsf_escaped, wscf_escaped, wss_escaped, ws_style_escaped, wsc_style_escaped,
                        matches!(app.mode, Mode::ClockMode), cached_bindings_json,
                        app.status_left_length, app.status_right_length, app.status_lines, status_format_json,
                        mode_style_escaped, message_style_escaped, status_position_escaped, status_justify_escaped,
                        cursor_style_code, app.status_visible, app.repeat_time_ms,
                        app.windows.get(app.active_idx).map_or(false, |w| w.zoom_saved.is_some()),
                        app.defaults_suppressed,
                        app.pwsh_mouse_selection,
                        app.mouse_selection,
                        app.mouse_selection_force,
                        app.paste_detection,
                        app.choose_tree_preview,
                        app.scroll_enter_copy_mode,
                        app.bold_is_bright,
                    ));
                    helpers::append_extra_style_json(&mut combined_buf, &app);
                    // Issue #7 batch D: dump-state's JSON never identified which
                    // session it belonged to (no consumer could tell two
                    // sessions' dump-state responses apart without a separate
                    // display-message round trip). Append it alongside the
                    // other one-off top-level fields.
                    if combined_buf.ends_with('}') {
                        combined_buf.pop();
                        combined_buf.push_str(",\"session_name\":\"");
                        combined_buf.push_str(&json_escape_string(&app.session_name));
                        combined_buf.push_str("\"}");
                    }
                    // Inject overlay state (popup, menu, confirm, display_panes)
                    {
                        // Inject clock_colour if set
                        if let Some(cc) = app.user_options.get("clock-mode-colour") {
                            if combined_buf.ends_with('}') {
                                combined_buf.pop();
                                combined_buf.push_str(",\"clock_colour\":\"");
                                combined_buf.push_str(&json_escape_string(cc));
                                combined_buf.push_str("\"}");
                            }
                        }
                        // Inject pane-border-status and pane-border-format
                        if let Some(pbs) = app.user_options.get("pane-border-status") {
                            if combined_buf.ends_with('}') {
                                combined_buf.pop();
                                combined_buf.push_str(",\"pane_border_status\":\"");
                                combined_buf.push_str(&json_escape_string(pbs));
                                combined_buf.push('"');
                                if let Some(pbf) = app.user_options.get("pane-border-format") {
                                    combined_buf.push_str(",\"pane_border_format\":\"");
                                    combined_buf.push_str(&json_escape_string(pbf));
                                    combined_buf.push('"');
                                }
                                combined_buf.push('}');
                            }
                        }
                        // Inject pane-border-lines independently — it may be set
                        // without pane-border-status.
                        if let Some(pbl) = app.user_options.get("pane-border-lines") {
                            if combined_buf.ends_with('}') {
                                combined_buf.pop();
                                combined_buf.push_str(",\"pane_border_lines\":\"");
                                combined_buf.push_str(&json_escape_string(pbl));
                                combined_buf.push('"');
                                combined_buf.push('}');
                            }
                        }
                        helpers::append_copy_ln_json(&app, &mut combined_buf);
                        helpers::append_floats_json(&app, &mut combined_buf);
                        // set-titles: when on, ship the expanded set-titles-string
                        // so the client emits OSC 0 to its host terminal.
                        // Expanded under the async guard in
                        // expand_status_formats; it used to be expanded here,
                        // outside it.
                        if let Some(title) = sf.host_title.as_deref() {
                            if combined_buf.ends_with('}') {
                                combined_buf.pop();
                                combined_buf.push_str(",\"host_title\":\"");
                                combined_buf.push_str(&json_escape_string(title));
                                combined_buf.push_str("\"}");
                            }
                        }
                        // Issue #269: forward OSC 9;4 progress from the active
                        // pane so the client emits the same sequence to the
                        // host terminal (Windows Terminal taskbar/tab progress).
                        if combined_buf.ends_with('}') {
                            if let Some((s, v)) = helpers::active_pane_progress(&app) {
                                combined_buf.pop();
                                combined_buf.push_str(",\"host_progress\":\"");
                                combined_buf.push_str(&format!("{};{}", s, v));
                                combined_buf.push_str("\"}");
                            }
                        }
                        let overlay_json = serialize_overlay_json(&app);
                        if !overlay_json.is_empty() && combined_buf.ends_with('}') {
                            combined_buf.pop();
                            combined_buf.push_str(&overlay_json);
                            combined_buf.push('}');
                        }
                    }
                    cached_dump_state.clear();
                    cached_dump_state.push_str(&combined_buf);
                    // Ingest OSC 52 from pane child processes (e.g. Claude
                    // Code's `/copy`): paste buffer add plus staging for the
                    // dump-state injection below, which re-emits it as OSC 52
                    // on the client's stdout to the host terminal.  Gated by
                    // `set-clipboard` inside the helper.
                    crate::server::helpers::drain_osc52(&mut app);
                    // Inject one-shot clipboard data for OSC 52 delivery to
                    // the client.  Only the *response* includes this field;
                    // the cached copy does not, so subsequent NC frames won't
                    // re-trigger clipboard emission on the client.
                    if let Some(clip_text) = app.clipboard_osc52.take() {
                        let clip_b64 = base64_encode(&clip_text);
                        // Replace trailing '}' with the extra field
                        if combined_buf.ends_with('}') {
                            combined_buf.pop();
                            combined_buf.push_str(",\"clipboard_osc52\":\"");
                            combined_buf.push_str(&clip_b64);
                            combined_buf.push_str("\"}");
                        }
                    }
                    // Forward audible bell to client terminal
                    if app.bell_forward {
                        app.bell_forward = false;
                        if combined_buf.ends_with('}') {
                            combined_buf.pop();
                            combined_buf.push_str(",\"bell\":true}");
                        }
                    }
                    cached_data_version = combined_data_version(&app);
                    state_dirty = false;
                    // Timing log: dump-state build time
                    if std::env::var("PSMUX_LATENCY_LOG").unwrap_or_default() == "1" {
                        let total_us = _t_layout.elapsed().as_micros();
                        use std::io::Write as _;
                        static SRV_LOG: std::sync::OnceLock<std::sync::Mutex<std::fs::File>> = std::sync::OnceLock::new();
                        let log = SRV_LOG.get_or_init(|| {
                            let base = std::env::var("USERPROFILE")
                                .map(std::path::PathBuf::from)
                                .unwrap_or_else(|_| std::env::temp_dir());
                            let p = base.join("psmux_server_latency.log");
                            std::sync::Mutex::new(std::fs::File::create(p).expect("create latency log"))
                        });
                        if let Ok(mut f) = log.lock() {
                            let _ = writeln!(f, "[SRV] dump: layout={}us total={}us json_len={}", _layout_ms, total_us, combined_buf.len());
                        }
                    }
                    // Push the newly-built frame to ALL persistent clients so
                    // that other attached sessions see the update immediately,
                    // even if they are idle and not polling dump-state.
                    // Without this, the DumpState handler clears state_dirty,
                    // and the bottom-of-loop push section never fires for frames
                    // already served to the requesting client.
                    // Push combined_buf (not cached_dump_state) so one-shot
                    // fields like bell and clipboard reach all clients.
                    // The cached copy omits them for NC dedup safety.
                    crate::types::push_frame(&combined_buf);
                    let _ = resp.send(combined_buf.clone());
                    dump_state_seen_full.insert(dump_client_id);
                }
                CtrlReq::SendText(s) => { app.status_message = None; crate::input::stamp_interactive_text(&mut app); send_text_to_active(&mut app, &s)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::SendKey(k) => { app.status_message = None; crate::input::stamp_interactive_key(&mut app, &k); send_key_to_active(&mut app, &k)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::SendPaste(s) => { send_paste_to_active(&mut app, &s)?; echo_pending_until = Some(Instant::now()); }
                CtrlReq::ZoomPane => { toggle_zoom(&mut app); state_dirty = true; meta_dirty = true; hook_event = Some("after-resize-pane"); }
                CtrlReq::PrefixBegin => { app.client_prefix_active = true; state_dirty = true; }
                CtrlReq::PrefixEnd => { app.client_prefix_active = false; state_dirty = true; }
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); hook_event = Some("pane-mode-changed"); }
                CtrlReq::CopyEnterPageUp => {
                    if app.scroll_enter_copy_mode {
                        enter_copy_mode(&mut app);
                        let half = app.windows.get(app.active_idx)
                            .and_then(|w| active_pane(&w.root, &w.active_path))
                            .map(|p| p.last_rows as usize).unwrap_or(20);
                        scroll_copy_up(&mut app, half);
                        hook_event = Some("pane-mode-changed");
                    } else {
                        // scroll-enter-copy-mode is off: forward PageUp to the
                        // active pane so apps like less/vim/WSL receive it (#284).
                        send_text_to_active(&mut app, "\x1b[5~")?;
                        echo_pending_until = Some(Instant::now());
                    }
                }
                CtrlReq::ClockMode => { app.mode = Mode::ClockMode; state_dirty = true; hook_event = Some("pane-mode-changed"); }
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_anchor_scroll_offset = app.copy_scroll_offset; app.copy_pos = Some((r,c)); } }
                CtrlReq::CopyYank => {
                    let _ = yank_selection(&mut app);
                    if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    exit_copy_mode(&mut app);
                    hook_event = Some("pane-mode-changed");
                }
                CtrlReq::CopyRectToggle => {
                    app.copy_selection_mode = match app.copy_selection_mode {
                        crate::types::SelectionMode::Rect => crate::types::SelectionMode::Char,
                        _ => crate::types::SelectionMode::Rect,
                    };
                }
                CtrlReq::ClientSize(cid, w, h) => { 
                    app.client_sizes.insert(cid, (w, h));
                    app.latest_client_id = Some(cid);
                    app.latest_size_client_id = Some(cid);
                    app.client_area = Rect::new(0, 0, w, h);
                    // Update registry with new size and activity timestamp
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.width = w;
                        info.height = h;
                        info.last_activity = std::time::Instant::now();
                    }
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    resize_all_panes(&mut app);
                    // Reconcile warm pane dimensions through the central
                    // policy module so resize uses the same code path as
                    // every other warm-pane invalidation (#271).
                    let sync = crate::warm_pane_sync::for_resize(
                        &app,
                        app.client_area.height,
                        app.client_area.width,
                    );
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    hook_event = Some("client-resized");
                }
                CtrlReq::HostColors(spec) => {
                    // Issue #473: a client reported its host terminal's colors.
                    // Keep the most recent report — the newest attached client
                    // is what the user is actually looking at.
                    let hc = crate::types::HostColors::from_spec(&spec);
                    if hc.has_any() || hc.dark.is_some() {
                        app.host_colors = Some(hc);
                        // Issue #556: pane reader threads answer color queries
                        // synchronously; publish the update where they can see it.
                        crate::types::set_shared_host_colors(app.host_colors.clone());
                    }
                }
                CtrlReq::FocusPaneCmd(pid) => {
                    let old_path = app.windows[app.active_idx].active_path.clone();
                    switch_with_copy_save(&mut app, |app| { focus_pane_by_id(app, pid); });
                    if app.windows[app.active_idx].active_path != old_path { unzoom_if_zoomed(&mut app); }
                    meta_dirty = true;
                }
                CtrlReq::FocusWindowCmd(wid) => { switch_with_copy_save(&mut app, |app| { if let Some(idx) = find_window_index_by_id(app, wid) { app.active_idx = idx; } }); resize_all_panes(&mut app); meta_dirty = true; }
                CtrlReq::MouseDown(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_down(&mut app, x, y); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseDownRight(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 2, true); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseDownMiddle(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 1, true); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseDrag(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_drag(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseUp(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_up(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseUpRight(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 2, false); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::MouseUpMiddle(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 1, false); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // #604: a bare pointer move only earns a redraw if it actually
                // reached a pane.  Nothing the server renders depends on the
                // pointer position (border hover highlighting is drawn by the
                // client from its own pointer state), so marking the whole
                // state dirty here pushed a full frame and repainted the entire
                // screen for every pointer sample.  This is the same rule as
                // the PaneMouse arm below, for pointer samples that land
                // outside every pane.
                CtrlReq::MouseMove(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); if remote_mouse_motion(&mut app, x, y) { state_dirty = true; echo_pending_until = Some(Instant::now()); } } }
                CtrlReq::ScrollUp(cid, x, y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_scroll_up(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::ScrollDown(cid, x, y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_scroll_down(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // #604: `pane-mouse <id> 35 ...` is a bare pointer move.  When
                // the pane never asked for any-event tracking it changes
                // nothing, so it must not push a frame and repaint the client.
                // Measured on this tree with PSMUX_CLIENT_DEBUG=1: sweeping the
                // pointer 60 times across a pane running nvim drove 24 full
                // client redraws, against 0 while idle.  With this gate and the
                // client-side one in client.rs it is 1, the same as idle.
                CtrlReq::PaneMouse(cid, pane_id, button, col, row, press) => { if app.mouse_enabled { app.latest_client_id = Some(cid); let inert = crate::window_ops::pane_mouse_is_inert_motion(&app, pane_id, button); handle_pane_mouse(&mut app, pane_id, button, col, row, press); if !inert { state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } } }
                CtrlReq::CopyDragBegin(cid, pane_id, a_col, a_row, c_col, c_row, rect_sel) => { if app.mouse_enabled { app.latest_client_id = Some(cid); copy_drag_begin(&mut app, pane_id, a_col, a_row, c_col, c_row, rect_sel); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::PaneScroll(cid, pane_id, up, at) => { if app.mouse_enabled { app.latest_client_id = Some(cid); handle_pane_scroll(&mut app, pane_id, up, at); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::SplitSetSizes(cid, path, sizes) => { if app.mouse_enabled { app.latest_client_id = Some(cid); handle_split_set_sizes(&mut app, &path, &sizes); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                CtrlReq::SplitResizeDone(cid) => { if app.mouse_enabled { app.latest_client_id = Some(cid); handle_split_resize_done(&mut app); state_dirty = true; meta_dirty = true; } }
                CtrlReq::NextWindow => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                }
                CtrlReq::PrevWindow => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                }
                CtrlReq::RenameWindow(name) => {
                    if let Some(cmds) = app.hooks.get("before-rename-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // tmux parity (#552): the argument is a format, expanded
                    // against the target window (cmd-rename-window.c runs it
                    // through format_single_from_target). The temporary -t
                    // focus has already made the target the active window
                    // here, so #{window_name} etc. resolve against it —
                    // enabling idioms like `rename-window '#{b:pane_current_path}'`.
                    let name = expand_format(&name, &app);
                    let win = &mut app.windows[app.active_idx]; win.name = name; win.manual_rename = true; meta_dirty = true; hook_event = Some("after-rename-window");
                }
                CtrlReq::ListWindows(resp) => { helpers::propagate_osc_titles(&mut app); let json = list_windows_json(&app)?; let _ = resp.send(json); }
                CtrlReq::ListWindowsTmux(resp) => { helpers::propagate_osc_titles(&mut app); let text = list_windows_tmux(&app); let _ = resp.send(text); }
                CtrlReq::ListWindowsFormat(resp, fmt) => { helpers::propagate_osc_titles(&mut app); let text = format_list_windows(&app, &fmt); let _ = resp.send(text); }
                CtrlReq::ListTree(resp) => { let json = list_tree_json(&app)?; let _ = resp.send(json); }
                CtrlReq::WindowLayout(wid, resp) => {
                    let json = crate::util::window_layout_json(&app, wid)
                        .unwrap_or_else(|_| "{}".to_string());
                    let _ = resp.send(json);
                }
                CtrlReq::WindowDump(wid, resp) => {
                    let json = crate::layout::dump_window_layout_json(&mut app, wid)
                        .unwrap_or_else(|_| "{}".to_string());
                    let _ = resp.send(json);
                }
                CtrlReq::ToggleSync => { app.sync_input = !app.sync_input; }
                CtrlReq::SetPaneTitle(title) => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        p.title_locked = !title.is_empty();
                        p.title = title;
                    }
                    meta_dirty = true;
                }
                CtrlReq::SetPaneStyle(style) => {
                    // Per-pane styling (e.g. "bg=default,fg=blue") matching
                    // tmux's `-P` flag which sets window-style + window-active-style.
                    // Store on the pane for API compatibility; ConPTY rendering
                    // doesn't support per-pane fg/bg tinting yet.
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        p.pane_style = Some(style);
                    }
                }
                CtrlReq::SetPaneAttrs { title, style } => {
                    // select-pane -T/-P (#592): runs under a temporary -t
                    // focus, so "active pane" here is the target. Both
                    // attributes apply in this single request; the temp
                    // focus restores right after it.
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        if let Some(t) = title {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                        if let Some(s) = style {
                            p.pane_style = Some(s);
                        }
                    }
                    meta_dirty = true;
                }
                CtrlReq::SendBytes(bytes) => {
                    send_bytes_to_active(&mut app, &bytes)?;
                }
                CtrlReq::SendKeys(keys, literal) => {
                    let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                    if in_copy {
                        // In copy/search mode — route through mode-aware handlers
                        if literal {
                            send_text_to_active(&mut app, &keys.join(""))?;
                        } else {
                            // #490: `keys` holds the send-keys arguments as
                            // separate tokens. Match each WHOLE token as a
                            // named key or send it verbatim — never split a
                            // token on whitespace, which destroyed spacing
                            // inside quoted arguments.
                            let parts: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                            for key in parts.iter() {
                                let key_upper = key.to_uppercase();
                                let normalized = match key_upper.as_str() {
                                    "ENTER" => "enter",
                                    "TAB" => "tab",
                                    "BTAB" | "BACKTAB" => "btab",
                                    "ESCAPE" | "ESC" => "esc",
                                    "SPACE" => "space",
                                    "BSPACE" | "BACKSPACE" => "backspace",
                                    "UP" => "up",
                                    "DOWN" => "down",
                                    "RIGHT" => "right",
                                    "LEFT" => "left",
                                    "HOME" => "home",
                                    "END" => "end",
                                    "PAGEUP" | "PPAGE" => "pageup",
                                    "PAGEDOWN" | "NPAGE" => "pagedown",
                                    "DELETE" | "DC" => "delete",
                                    "INSERT" | "IC" => "insert",
                                    _ => "",
                                };
                                if !normalized.is_empty() {
                                    send_key_to_active(&mut app, normalized)?;
                                } else if key_upper.starts_with("C-") || key_upper.starts_with("M-") || (key_upper.starts_with("F") && key_upper.len() >= 2 && key_upper[1..].chars().all(|c| c.is_ascii_digit())) {
                                    send_key_to_active(&mut app, &key.to_lowercase())?;
                                } else {
                                    // Plain text char — route through send_text_to_active (handles copy mode chars)
                                    send_text_to_active(&mut app, key)?;
                                }
                            }
                        }
                    } else if literal {
                        send_text_to_active(&mut app, &keys.join(""))?;
                    } else {
                        // #490: `keys` holds the send-keys arguments as
                        // separate tokens. A token either matches a named key
                        // in its entirety or is typed verbatim with its
                        // whitespace intact; a single separator space is
                        // still inserted between adjacent PLAIN tokens for
                        // backward compatibility with multi word scripts
                        // (strict tmux would concatenate them).
                        let parts: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                        for (i, key) in parts.iter().enumerate() {
                            let key_upper = key.to_uppercase();
                            let _is_special = matches!(key_upper.as_str(), 
                                "ENTER" | "TAB" | "BTAB" | "BACKTAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                            ) || key_upper.starts_with("C-") || key_upper.starts_with("M-") || key_upper.starts_with("S-");
                            
                            match key_upper.as_str() {
                                "ENTER" => send_text_to_active(&mut app, "\r")?,
                                "TAB" => send_text_to_active(&mut app, "\t")?,
                                "BTAB" | "BACKTAB" => send_text_to_active(&mut app, "\x1b[Z")?,
                                "ESCAPE" | "ESC" => send_text_to_active(&mut app, "\x1b")?,
                                "SPACE" => send_text_to_active(&mut app, " ")?,
                                "BSPACE" | "BACKSPACE" => send_text_to_active(&mut app, "\x7f")?,
                                // DECCKM app-cursor mode: SS3, not CSI (see crate::input::csi_cursor_to_ss3).
                                "UP" => send_key_to_active(&mut app, "up")?,
                                "DOWN" => send_key_to_active(&mut app, "down")?,
                                "RIGHT" => send_key_to_active(&mut app, "right")?,
                                "LEFT" => send_key_to_active(&mut app, "left")?,
                                "HOME" => send_key_to_active(&mut app, "home")?,
                                "END" => send_key_to_active(&mut app, "end")?,
                                "PAGEUP" | "PPAGE" => send_text_to_active(&mut app, "\x1b[5~")?,
                                "PAGEDOWN" | "NPAGE" => send_text_to_active(&mut app, "\x1b[6~")?,
                                "DELETE" | "DC" => send_text_to_active(&mut app, "\x1b[3~")?,
                                "INSERT" | "IC" => send_text_to_active(&mut app, "\x1b[2~")?,
                                "F1" => send_text_to_active(&mut app, "\x1bOP")?,
                                "F2" => send_text_to_active(&mut app, "\x1bOQ")?,
                                "F3" => send_text_to_active(&mut app, "\x1bOR")?,
                                "F4" => send_text_to_active(&mut app, "\x1bOS")?,
                                "F5" => send_text_to_active(&mut app, "\x1b[15~")?,
                                "F6" => send_text_to_active(&mut app, "\x1b[17~")?,
                                "F7" => send_text_to_active(&mut app, "\x1b[18~")?,
                                "F8" => send_text_to_active(&mut app, "\x1b[19~")?,
                                "F9" => send_text_to_active(&mut app, "\x1b[20~")?,
                                "F10" => send_text_to_active(&mut app, "\x1b[21~")?,
                                "F11" => send_text_to_active(&mut app, "\x1b[23~")?,
                                "F12" => send_text_to_active(&mut app, "\x1b[24~")?,
                                // Modifier + special key combos (C-Left, S-Right, C-M-Up, etc.)
                                // must be checked BEFORE the generic C-x / M-x single-char handlers.
                                s if crate::input::parse_modified_special_key(s).is_some() => {
                                    let seq = crate::input::parse_modified_special_key(s).unwrap();
                                    send_text_to_active(&mut app, &seq)?;
                                }
                                s if s.starts_with("C-M-") || s.starts_with("C-m-") => {
                                    if let Some(c) = key.chars().nth(4) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            send_text_to_active(&mut app, &format!("\x1b{}", ctrl as char))?;
                                        }
                                    }
                                }
                                // Ctrl+Shift+<punctuation/digit> that collapses to a single
                                // C0 byte, e.g. Ctrl+/ delivered by ConPTY terminals
                                // (Alacritty, WezTerm) as "C-S--" (VK_OEM_MINUS + Ctrl +
                                // Shift).  It must reach the child as 0x1f (^_), matching
                                // Ctrl+_ and tmux, so neovim's Ctrl+/ comment toggle fires
                                // (issue #394).  This MUST precede the generic C- arm below,
                                // whose nth(2) extraction would otherwise read the 'S' and
                                // mis-send Ctrl+S.
                                s if (s.starts_with("C-S-") || s.starts_with("C-s-"))
                                    && s.chars().count() == 5
                                    && s.chars().nth(4).map_or(false, |c| !c.is_ascii_alphabetic()) =>
                                {
                                    if let Some(c) = s.chars().nth(4) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                        }
                                    }
                                }
                                s if s.starts_with("C-") => {
                                    if let Some(c) = s.chars().nth(2) {
                                        let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) else { continue };
                                        // On Windows with Win32 input mode, write the key as
                                        // a Win32 input mode escape sequence so ConPTY generates
                                        // a proper KEY_EVENT with VK + LEFT_CTRL_PRESSED (#305).
                                        #[cfg(windows)]
                                        {
                                            if c.is_ascii_alphabetic() {
                                                // Keep Ctrl+C on the legacy interrupt path:
                                                // raw 0x03 + the interrupt router. The router
                                                // runs BEFORE the byte: when it decides "raw
                                                // 0x03 only" it may strip PROCESSED_INPUT from
                                                // the pane console so conhost delivers the byte
                                                // as input instead of converting it into a
                                                // console-wide CTRL_C_EVENT that aborts a
                                                // booting WSL launch (#579).
                                                if ctrl == 0x03 {
                                                    if let Some(win) = app.windows.get_mut(app.active_idx) {
                                                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                                                            if p.child_pid.is_none() {
                                                                p.child_pid = crate::platform::mouse_inject::get_child_pid(&*p.child);
                                                            }
                                                            if let Some(pid) = p.child_pid {
                                                                crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                                                            }
                                                        }
                                                    }
                                                    send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                                } else {
                                                    let vk = crate::platform::mouse_inject::char_to_vk(c);
                                                    let scan = crate::platform::mouse_inject::vk_to_scan(vk);
                                                    let u_char = (c.to_ascii_lowercase() as u16) & 0x1F;
                                                    const LEFT_CTRL_PRESSED: u32 = 0x0008;
                                                    let seq = format!(
                                                        "\x1b[{};{};{};1;{};1_\x1b[{};{};{};0;{};1_",
                                                        vk, scan, u_char, LEFT_CTRL_PRESSED,
                                                        vk, scan, u_char, LEFT_CTRL_PRESSED
                                                    );
                                                    send_text_to_active(&mut app, &seq)?;
                                                }
                                            } else {
                                                send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                            }
                                        }
                                        #[cfg(not(windows))]
                                        send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                    }
                                }
                                s if s.starts_with("M-") => {
                                    if let Some(c) = key.chars().nth(2) {
                                        send_text_to_active(&mut app, &format!("\x1b{}", c))?;
                                    }
                                }
                                _ => {
                                    // Plain token: typed VERBATIM (#490 — the
                                    // token's own whitespace is untouched).
                                    // Keep the historical single separator
                                    // space between two adjacent plain tokens
                                    // so existing multi word scripts like
                                    // `send-keys echo hi Enter` keep working.
                                    send_text_to_active(&mut app, key)?;
                                    if i + 1 < parts.len() {
                                        let next_upper = parts[i + 1].to_uppercase();
                                        let next_is_special = matches!(next_upper.as_str(),
                                            "ENTER" | "TAB" | "BTAB" | "BACKTAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                            "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                            "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                            "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                                        ) || next_upper.starts_with("C-") || next_upper.starts_with("M-") || next_upper.starts_with("S-");
                                        if !next_is_special {
                                            send_text_to_active(&mut app, " ")?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    echo_pending_until = Some(Instant::now());
                }
                CtrlReq::SendKeysX(cmd) => {
                    // send-keys -X: dispatch copy-mode commands by name
                    // This is the primary mechanism used by tmux-yank and other plugins
                    let in_copy = matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. });
                    if !in_copy {
                        // Auto-enter copy mode for commands that require it
                        enter_copy_mode(&mut app);
                    }
                    match cmd.as_str() {
                        "cancel" => {
                            // Use the canonical exit: it also clears the
                            // pane-local `copy_state`.  Hand-rolling the exit
                            // here left that behind, and the next focus change
                            // (`select-pane`, which every mouse click sends)
                            // restored it through `switch_with_copy_save`, so a
                            // plain click after an external `send-keys -X
                            // cancel` silently re-entered copy mode.
                            crate::copy_mode::exit_copy_mode(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-mode-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        "begin-selection" => {
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                                app.copy_pos = Some((r,c));
                                app.copy_selection_mode = crate::types::SelectionMode::Char;
                            }
                        }
                        "select-line" => {
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                                app.copy_pos = Some((r,c));
                                app.copy_selection_mode = crate::types::SelectionMode::Line;
                            }
                        }
                        "rectangle-toggle" => {
                            app.copy_selection_mode = match app.copy_selection_mode {
                                crate::types::SelectionMode::Rect => crate::types::SelectionMode::Char,
                                _ => crate::types::SelectionMode::Rect,
                            };
                        }
                        "copy-selection" => {
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        "copy-selection-and-cancel" => {
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            crate::copy_mode::exit_copy_mode(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-mode-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        "copy-selection-no-clear" => {
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        s if s.starts_with("copy-pipe-and-cancel") || s.starts_with("copy-pipe") => {
                            // copy-pipe[-and-cancel] [command] — yank + pipe to command
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            // Extract pipe command from argument if present
                            let cancel = s.contains("cancel");
                            let pipe_cmd = cmd.strip_prefix("copy-pipe-and-cancel")
                                .or_else(|| cmd.strip_prefix("copy-pipe"))
                                .unwrap_or("")
                                .trim();
                            if !pipe_cmd.is_empty() {
                                if let Some(text) = app.paste_buffers.first().cloned() {
                                    // Pipe yanked text to the command's stdin
                                    let mut copy_pipe_cmd = std::process::Command::new(if cfg!(windows) { "pwsh" } else { "sh" });
                                    copy_pipe_cmd.args(if cfg!(windows) { vec!["-NoProfile", "-Command", pipe_cmd] } else { vec!["-c", pipe_cmd] })
                                        .stdin(std::process::Stdio::piped())
                                        .stdout(std::process::Stdio::null())
                                        .stderr(std::process::Stdio::null());
                                    { use crate::platform::HideWindowCommandExt; copy_pipe_cmd.hide_window(); }
                                    if let Ok(mut child) = copy_pipe_cmd.spawn() {
                                        if let Some(mut stdin) = child.stdin.take() {
                                            use std::io::Write;
                                            let _ = stdin.write_all(text.as_bytes());
                                        }
                                        let _ = child.wait();
                                    }
                                }
                            }
                            if cancel {
                                crate::copy_mode::exit_copy_mode(&mut app);
                                if let Some(cmds) = app.hooks.get("pane-mode-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            }
                        }
                        "cursor-up" => { move_copy_cursor(&mut app, 0, -1); }
                        "cursor-down" => { move_copy_cursor(&mut app, 0, 1); }
                        "cursor-left" => { move_copy_cursor(&mut app, -1, 0); }
                        "cursor-right" => { move_copy_cursor(&mut app, 1, 0); }
                        "start-of-line" => { crate::copy_mode::move_to_line_start(&mut app); }
                        "end-of-line" => { crate::copy_mode::move_to_line_end(&mut app); }
                        "back-to-indentation" => { crate::copy_mode::move_to_first_nonblank(&mut app); }
                        "next-word" => { crate::copy_mode::move_word_forward(&mut app); }
                        "previous-word" => { crate::copy_mode::move_word_backward(&mut app); }
                        "next-word-end" => { crate::copy_mode::move_word_end(&mut app); }
                        "next-space" => { crate::copy_mode::move_word_forward_big(&mut app); }
                        "previous-space" => { crate::copy_mode::move_word_backward_big(&mut app); }
                        "next-space-end" => { crate::copy_mode::move_word_end_big(&mut app); }
                        "top-line" => { crate::copy_mode::move_to_screen_top(&mut app); }
                        "middle-line" => { crate::copy_mode::move_to_screen_middle(&mut app); }
                        "bottom-line" => { crate::copy_mode::move_to_screen_bottom(&mut app); }
                        "history-top" => { crate::copy_mode::scroll_to_top(&mut app); }
                        "history-bottom" => { crate::copy_mode::scroll_to_bottom(&mut app); }
                        "halfpage-up" => {
                            let half = app.windows.get(app.active_idx)
                                .and_then(|w| active_pane(&w.root, &w.active_path))
                                .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                            scroll_copy_up(&mut app, half);
                        }
                        "halfpage-down" => {
                            let half = app.windows.get(app.active_idx)
                                .and_then(|w| active_pane(&w.root, &w.active_path))
                                .map(|p| (p.last_rows / 2) as usize).unwrap_or(10);
                            scroll_copy_down(&mut app, half);
                        }
                        "page-up" => { scroll_copy_up(&mut app, 20); }
                        "page-down" => { scroll_copy_down(&mut app, 20); }
                        "scroll-up" => { scroll_copy_up(&mut app, 1); }
                        "scroll-down" => { scroll_copy_down(&mut app, 1); }
                        "scroll-middle" => { crate::copy_mode::scroll_middle(&mut app); }
                        // tmux takes an optional search term argument on all
                        // four verbs (cmd-queue "send-keys -X search-backward
                        // foo"). Without the argument the interactive prompt
                        // opens, exactly as pressing `/` or `?` does.
                        s if s == "search-forward" || s == "search-backward"
                            || s == "search-forward-incremental"
                            || s == "search-backward-incremental"
                            || s.starts_with("search-forward ")
                            || s.starts_with("search-backward ")
                            || s.starts_with("search-forward-incremental ")
                            || s.starts_with("search-backward-incremental ") =>
                        {
                            let fwd = s.starts_with("search-forward");
                            let term = match s.find(' ') {
                                Some(i) => s[i + 1..].trim().to_string(),
                                None => String::new(),
                            };
                            if term.is_empty() {
                                app.mode = Mode::CopySearch { input: String::new(), forward: fwd };
                                let prompt = if fwd { "(search down) " } else { "(search up) " };
                                app.status_message = Some((prompt.to_string(), std::time::Instant::now(), Some(0)));
                            } else {
                                app.copy_search_query = term.clone();
                                app.copy_search_forward = fwd;
                                crate::copy_mode::search_copy_mode(&mut app, &term, fwd);
                                app.mode = Mode::CopyMode;
                            }
                        }
                        "search-again" => { crate::copy_mode::search_next(&mut app); }
                        "search-reverse" => { crate::copy_mode::search_prev(&mut app); }
                        "copy-end-of-line" => { let _ = crate::copy_mode::copy_end_of_line(&mut app); crate::copy_mode::exit_copy_mode(&mut app); }
                        "select-word" => {
                            // Select the word under cursor
                            crate::copy_mode::move_word_backward(&mut app);
                            if let Some((r,c)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r,c));
                                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                                app.copy_selection_mode = crate::types::SelectionMode::Char;
                            }
                            crate::copy_mode::move_word_end(&mut app);
                        }
                        "other-end" => {
                            if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                                app.copy_anchor = Some(p);
                                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                                app.copy_pos = Some(a);
                            }
                        }
                        "clear-selection" => {
                            app.copy_anchor = None;
                            app.copy_selection_mode = crate::types::SelectionMode::Char;
                        }
                        "append-selection" => {
                            // Append to existing buffer instead of replacing
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            if app.paste_buffers.len() >= 2 {
                                let appended = format!("{}{}", app.paste_buffers[1], app.paste_buffers[0]);
                                app.paste_buffers[0] = appended;
                            }
                        }
                        "append-selection-and-cancel" => {
                            let _ = yank_selection(&mut app);
                            if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            if app.paste_buffers.len() >= 2 {
                                let appended = format!("{}{}", app.paste_buffers[1], app.paste_buffers[0]);
                                app.paste_buffers[0] = appended;
                            }
                            app.mode = Mode::Passthrough;
                            app.copy_scroll_offset = 0;
                            app.copy_pos = None;
                            if let Some(cmds) = app.hooks.get("pane-mode-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        "copy-line" => {
                            // Select entire current line and yank
                            if let Some((r, _)) = crate::copy_mode::get_copy_pos(&mut app) {
                                app.copy_anchor = Some((r, 0));
                                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                                app.copy_selection_mode = crate::types::SelectionMode::Line;
                                let cols = app.windows.get(app.active_idx)
                                    .and_then(|w| active_pane(&w.root, &w.active_path))
                                    .map(|p| p.last_cols).unwrap_or(80);
                                app.copy_pos = Some((r, cols.saturating_sub(1)));
                                let _ = yank_selection(&mut app);
                                if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            }
                            app.mode = Mode::Passthrough;
                            app.copy_scroll_offset = 0;
                            app.copy_pos = None;
                            if let Some(cmds) = app.hooks.get("pane-mode-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                        }
                        s if s.starts_with("goto-line") => {
                            // goto-line <N> — jump to line N in scrollback
                            let n = s.strip_prefix("goto-line").unwrap_or("").trim()
                                .parse::<u16>().unwrap_or(0);
                            app.copy_pos = Some((n, 0));
                        }
                        "jump-forward" => { app.copy_find_char_pending = Some(0); }
                        "jump-backward" => { app.copy_find_char_pending = Some(1); }
                        "jump-to-forward" => { app.copy_find_char_pending = Some(2); }
                        "jump-to-backward" => { app.copy_find_char_pending = Some(3); }
                        "jump-again" => { crate::copy_mode::jump_again(&mut app); }
                        "jump-reverse" => { crate::copy_mode::jump_reverse(&mut app); }
                        "set-mark" => { crate::copy_mode::set_mark(&mut app); }
                        "jump-to-mark" => { crate::copy_mode::jump_to_mark(&mut app); }
                        // tmux 3.3 calls this refresh-toggle; older tables and
                        // the #498 report use refresh-from-pane for the same key.
                        "refresh-from-pane" | "refresh-toggle" => { crate::copy_mode::toggle_refresh(&mut app); }
                        "next-paragraph" => {
                            crate::copy_mode::move_next_paragraph(&mut app);
                        }
                        "previous-paragraph" => {
                            crate::copy_mode::move_prev_paragraph(&mut app);
                        }
                        "next-matching-bracket" => {
                            crate::copy_mode::move_matching_bracket(&mut app);
                        }
                        "stop-selection" => {
                            // Keep cursor position but stop extending selection
                            app.copy_anchor = None;
                        }
                        _ => {} // ignore unknown copy-mode commands
                    }
                }
                CtrlReq::SelectPane(dir, keep_zoom) => {
                    if let Some(cmds) = app.hooks.get("before-select-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // Auto-unzoom when navigating to another pane (tmux behavior).
                    // For directional nav: unzoom first so compute_rects uses
                    // real geometry, then re-zoom only if focus didn't change.
                    // For other cases: only unzoom if focus actually changes.
                    // (fixes #46)
                    match dir.as_str() {
                        "U" | "D" | "L" | "R" => {
                            let focus_dir = match dir.as_str() {
                                "U" => FocusDir::Up, "D" => FocusDir::Down,
                                "L" => FocusDir::Left, _ => FocusDir::Right,
                            };
                            if keep_zoom {
                                let old_path = app.windows[app.active_idx].active_path.clone();
                                switch_with_copy_save(&mut app, |app| {
                                    move_focus_preserving_zoom(app, focus_dir);
                                });
                                if app.windows[app.active_idx].active_path != old_path {
                                    app.last_pane_path = old_path;
                                }
                            } else {
                                let was_zoomed = unzoom_if_zoomed(&mut app);
                                if was_zoomed {
                                // Zoom-aware: check direct neighbor or wrap target (tmux parity: unzoom+wrap).
                                let win = &app.windows[app.active_idx];
                                let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
                                crate::tree::compute_rects(&win.root, app.last_window_area, &mut rects);
                                let active_idx = rects.iter().position(|(path, _)| *path == win.active_path);
                                let has_target = 
                                    if let Some(ai) = active_idx {
                                        let (_, arect) = &rects[ai];
                                        find_best_pane_in_direction(&rects, ai, arect, focus_dir, &[], &[])
                                            .or_else(|| find_wrap_target(&rects, ai, arect, focus_dir, &[], &[]))
                                            .is_some()
                                    } else { false };
                                    if has_target {
                                        let old_path = app.windows[app.active_idx].active_path.clone();
                                        switch_with_copy_save(&mut app, |app| {
                                            move_focus(app, focus_dir);
                                        });
                                        app.last_pane_path = old_path;
                                    } else {
                                        // No reachable pane (single-pane window) — re-zoom
                                        toggle_zoom(&mut app);
                                    }
                                } else {
                                    let old_path = app.windows[app.active_idx].active_path.clone();
                                    switch_with_copy_save(&mut app, |app| {
                                        move_focus(app, focus_dir);
                                    });
                                    if app.windows[app.active_idx].active_path != old_path {
                                        app.last_pane_path = old_path;
                                    }
                                }
                            }
                        }
                        "last" => {
                            // select-pane -l: switch to last active pane
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            switch_with_copy_save(&mut app, |app| {
                                let win = &mut app.windows[app.active_idx];
                                if !app.last_pane_path.is_empty() {
                                    let tmp = win.active_path.clone();
                                    win.active_path = app.last_pane_path.clone();
                                    app.last_pane_path = tmp;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                // Update MRU for the newly focused pane
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "mark" => {
                            // select-pane -m: mark the current pane
                            let win = &app.windows[app.active_idx];
                            if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                app.marked_pane = Some((app.active_idx, pid));
                            }
                        }
                        "next" => {
                            // select-pane next: cycle to next pane (like Prefix+o / tmux -t :.+)
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            switch_with_copy_save(&mut app, |app| {
                                let win = &app.windows[app.active_idx];
                                let mut pane_paths = Vec::new();
                                let mut path = Vec::new();
                                collect_pane_paths_server(&win.root, &mut path, &mut pane_paths);
                                if let Some(cur) = pane_paths.iter().position(|p| *p == win.active_path) {
                                    let next = (cur + 1) % pane_paths.len();
                                    let new_path = pane_paths[next].clone();
                                    let win = &mut app.windows[app.active_idx];
                                    app.last_pane_path = win.active_path.clone();
                                    win.active_path = new_path;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "prev" => {
                            // select-pane prev: cycle to previous pane (tmux -t :.-)
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            switch_with_copy_save(&mut app, |app| {
                                let win = &app.windows[app.active_idx];
                                let mut pane_paths = Vec::new();
                                let mut path = Vec::new();
                                collect_pane_paths_server(&win.root, &mut path, &mut pane_paths);
                                if let Some(cur) = pane_paths.iter().position(|p| *p == win.active_path) {
                                    let prev = (cur + pane_paths.len() - 1) % pane_paths.len();
                                    let new_path = pane_paths[prev].clone();
                                    let win = &mut app.windows[app.active_idx];
                                    app.last_pane_path = win.active_path.clone();
                                    win.active_path = new_path;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "unmark" => {
                            // select-pane -M: clear the marked pane
                            app.marked_pane = None;
                        }
                        _ => {}
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-pane");
                }
                CtrlReq::SelectWindow(idx) => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if let Some(internal_idx) = app.win_pos(idx) {
                        if internal_idx != app.active_idx {
                            switch_with_copy_save(&mut app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                            resize_all_panes(&mut app);
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::ListPanes(resp) => {
                    helpers::propagate_osc_titles(&mut app);
                    let mut output = String::new();
                    let win = &app.windows[app.active_idx];
                    fn collect_panes(node: &Node, panes: &mut Vec<(usize, u16, u16, vt100::MouseProtocolMode, vt100::MouseProtocolEncoding, bool)>) {
                        match node {
                            Node::Leaf(p) => {
                                let (mode, enc, alt) = match p.term.lock() {
                                    Ok(term) => {
                                        let screen = term.screen();
                                        (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding(), screen.alternate_screen())
                                    }
                                    Err(_) => {
                                        // Mutex poisoned — reader thread panicked.  Use safe defaults.
                                        (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default, false)
                                    }
                                };
                                panes.push((p.id, p.last_cols, p.last_rows, mode, enc, alt));
                            }
                            Node::Split { children, .. } => {
                                for c in children { collect_panes(c, panes); }
                            }
                        }
                    }
                    let mut panes = Vec::new();
                    collect_panes(&win.root, &mut panes);
                    let active_pane_id = crate::tree::get_active_pane_id(&win.root, &win.active_path);
                    for (pos, (id, cols, rows, _mode, _enc, _alt)) in panes.iter().enumerate() {
                        let idx = pos + app.pane_base_index;
                        let active_marker = if active_pane_id == Some(*id) { " (active)" } else { "" };
                        output.push_str(&format!("{}: [{}x{}] [history {}/{}, 0 bytes] %{}{}\n", idx, cols, rows, app.history_limit, app.history_limit, id, active_marker));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListPanesFormat(resp, fmt) => {
                    helpers::propagate_osc_titles(&mut app);
                    let text = format_list_panes(&app, &fmt, app.active_idx);
                    let _ = resp.send(text);
                }
                CtrlReq::ListAllPanes(resp) => {
                    let mut output = String::new();
                    fn collect_all_panes(node: &Node, panes: &mut Vec<(usize, u16, u16)>) {
                        match node {
                            Node::Leaf(p) => { panes.push((p.id, p.last_cols, p.last_rows)); }
                            Node::Split { children, .. } => { for c in children { collect_all_panes(c, panes); } }
                        }
                    }
                    for (wi, win) in app.windows.iter().enumerate() {
                        let mut panes = Vec::new();
                        collect_all_panes(&win.root, &mut panes);
                        for (id, cols, rows) in panes {
                            output.push_str(&format!("{}:{}: %{} [{}x{}]\n", app.session_name, app.win_display_index(wi), id, cols, rows));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListAllPanesFormat(resp, fmt) => {
                    let mut lines = Vec::new();
                    for wi in 0..app.windows.len() {
                        lines.push(format_list_panes(&app, &fmt, wi));
                    }
                    let _ = resp.send(lines.join("\n"));
                }
                CtrlReq::KillWindow => {
                    let active = app.active_idx;
                    kill_window_at(&mut app, active);
                    // Killing a window changes the active window and the window
                    // list, so resize the now-active window's panes and force a
                    // status-bar/window-list rebuild + push to attached clients.
                    // Without meta_dirty the cached window tabs stay stale and
                    // without state_dirty the no-change fast path skips the frame,
                    // leaving the bottom bar showing the killed window until the
                    // next input (issue #359). Mirrors every other structural
                    // mutation (new-window, kill-pane, select-window) and tmux's
                    // server_kill_window -> server_redraw_session_group.
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    state_dirty = true;
                    hook_event = Some("window-closed");
                }
                CtrlReq::KillWindowTarget { win, win_is_id, name, resp } => {
                    // Resolve the target here, on live state. An unresolvable
                    // target must be an error, never a fallback to the active
                    // window: the old temp-focus-then-kill dance silently
                    // no-opped the focus on a bad name/index/@id and then
                    // killed whatever was focused (session death when it was
                    // the last window). tmux: "can't find window: X", kills
                    // nothing, exit 1.
                    let resolved = if let Some(w) = win {
                        if win_is_id {
                            app.windows.iter().position(|x| x.id == w)
                        } else {
                            app.win_pos(w)
                        }
                    } else if let Some(ref n) = name {
                        app.windows.iter().position(|x| x.name == *n)
                    } else {
                        Some(app.active_idx)
                    };
                    match resolved {
                        Some(pos) => {
                            kill_window_at(&mut app, pos);
                            resize_all_panes(&mut app);
                            meta_dirty = true;
                            state_dirty = true;
                            hook_event = Some("window-closed");
                            let _ = resp.send(Ok(()));
                        }
                        None => {
                            let spec = if let Some(w) = win {
                                if win_is_id { format!("@{}", w) } else { w.to_string() }
                            } else {
                                name.clone().unwrap_or_default()
                            };
                            let msg = format!("can't find window: {}", spec);
                            // Surface in the status bar for attached clients,
                            // same convention as join-pane (#437).
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                    }
                }
                CtrlReq::KillSession => {
                    // Fire session-closed hook before cleanup
                    if let Some(cmds) = app.hooks.get("session-closed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // Remove port/key/sid files FIRST so clients see the session
                    // as gone immediately, then kill processes.
                    let regpath = crate::paths::port_file(&app.port_file_base());
                    let keypath = crate::paths::key_file(&app.port_file_base());
                    let _ = std::fs::remove_file(&regpath);
                    let _ = std::fs::remove_file(&keypath);
                    crate::session::remove_session_id_file(&app.port_file_base());
                    crate::types::send_directive_to_all_clients("DETACH");
                    std::thread::sleep(Duration::from_millis(50));
                    crate::types::shutdown_persistent_streams();
                    // Kill all child processes using a single process snapshot
                    tree::kill_all_children_batch(&mut app.windows);
                    // Kill warm pane's child (process::exit skips Drop)
                    if let Some(mut wp) = app.warm_pane.take() { wp.child.kill().ok(); }
                    // TerminateProcess is synchronous on Windows — processes
                    // are already dead.  Minimal delay for OS handle cleanup.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    std::process::exit(0);
                }
                CtrlReq::HasSession(resp) => {
                    let _ = resp.send(true);
                }
                CtrlReq::RenameSession(name) => {
                    if let Some(cmds) = app.hooks.get("before-rename-session") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    let old_path = crate::paths::port_file(&app.port_file_base());
                    let old_keypath = crate::paths::key_file(&app.port_file_base());
                    // Compute new port file base with socket_name prefix
                    let new_base = if let Some(ref sn) = app.socket_name {
                        format!("{}__{}" , sn, name)
                    } else {
                        name.clone()
                    };
                    let new_path = crate::paths::port_file(&new_base);
                    let new_keypath = crate::paths::key_file(&new_base);
                    if let Some(port) = app.control_port {
                        let _ = std::fs::remove_file(&old_path);
                        // Write this server's OWN in-memory key, NOT a copy of the
                        // old .key file. Under warm-server replenish churn the
                        // __warm__.key file may have been overwritten by a LATER warm
                        // server, so copying it would give the renamed/claimed session
                        // a key that does not match this server (seen as "Invalid
                        // session key" on a later command). app.session_key is correct.
                        let _ = std::fs::remove_file(&old_keypath);
                        let _ = std::fs::write(&new_keypath, &app.session_key);
                        // The activity stamp follows the session across the rename
                        // (issue #603); it must move BEFORE remove_session_id_file
                        // drops the old base's stamp with its .sid/.pid.
                        crate::session::carry_session_activity_file(&app.port_file_base(), &new_base);
                        // Rename .sid file to match new session name
                        crate::session::remove_session_id_file(&app.port_file_base());
                        crate::session::write_session_id_file(&new_base, app.session_id);
                        // Re-anchor the PID sentinel to the new base (issue #448):
                        // remove_session_id_file above dropped the old .pid.
                        crate::session::write_session_pid_file(&new_base, std::process::id());
                        // The new .port file goes LAST: it is the readiness beacon an
                        // attaching client polls for, so the .key/.sid/.pid it will
                        // read next must already exist when it appears (issue #496).
                        let _ = std::fs::write(&new_path, port.to_string());
                    }
                    app.session_name = name;
                    // Move the single-server-per-name guard onto the new name, or the
                    // old name stays locked forever and blocks re-creating it (#505).
                    rekey_session_guard(&mut session_guard, &app.port_file_base());
                    // Update env so run-shell/hooks from this server target the new name
                    env::set_var("PSMUX_TARGET_SESSION", app.port_file_base());
                    hook_event = Some("after-rename-session");
                }
                CtrlReq::ClaimSession(name, client_cwd, client_priority, resp) => {
                    // Guard against clobbering an already-claimed session. Under
                    // rapid `new-session`, a stale __warm__.port (or OS ephemeral
                    // port reuse) can route a claim to a server that has ALREADY
                    // been claimed — its session_name is no longer "__warm__".
                    // Renaming it again would rename the live session away and
                    // destroy it (observed as rapid new-session intermittently
                    // losing 1-4 of N sessions ~2s after creation). Refuse the
                    // claim so the CLI falls back to a cold-spawn, which is the
                    // reliable path. Only a genuine warm server may be claimed.
                    if app.session_name != "__warm__" {
                        warm_debug(&format!("CLAIM REFUSED: this server is '{}' (port={:?}), requested name='{}'", app.session_name, app.control_port, name));
                        let _ = resp.send("ERR: not a warm server (already claimed)\n".to_string());
                    } else {
                    warm_debug(&format!("CLAIM ACCEPT: __warm__ (port={:?}) -> '{}'", app.control_port, name));
                    // Same as RenameSession but with a synchronous response
                    // so the CLI knows the rename completed before attaching.
                    let old_path = crate::paths::port_file(&app.port_file_base());
                    let old_keypath = crate::paths::key_file(&app.port_file_base());
                    let new_base = if let Some(ref sn) = app.socket_name {
                        format!("{}__{}" , sn, name)
                    } else {
                        name.clone()
                    };
                    let new_path = crate::paths::port_file(&new_base);
                    let new_keypath = crate::paths::key_file(&new_base);
                    if let Some(port) = app.control_port {
                        let _ = std::fs::remove_file(&old_path);
                        // Write this server's OWN in-memory key, NOT a copy of the
                        // old .key file. Under warm-server replenish churn the
                        // __warm__.key file may have been overwritten by a LATER warm
                        // server, so copying it would give the renamed/claimed session
                        // a key that does not match this server (seen as "Invalid
                        // session key" on a later command). app.session_key is correct.
                        let _ = std::fs::remove_file(&old_keypath);
                        let _ = std::fs::write(&new_keypath, &app.session_key);
                        // The activity stamp follows the session across the rename
                        // (issue #603); it must move BEFORE remove_session_id_file
                        // drops the old base's stamp with its .sid/.pid.
                        crate::session::carry_session_activity_file(&app.port_file_base(), &new_base);
                        // Rename .sid file to match new session name
                        crate::session::remove_session_id_file(&app.port_file_base());
                        crate::session::write_session_id_file(&new_base, app.session_id);
                        // Re-anchor the PID sentinel to the new base (issue #448):
                        // remove_session_id_file above dropped the old .pid.
                        crate::session::write_session_pid_file(&new_base, std::process::id());
                        // The new .port file goes LAST: it is the readiness beacon an
                        // attaching client polls for, so the .key/.sid/.pid it will
                        // read next must already exist when it appears (issue #496).
                        let _ = std::fs::write(&new_path, port.to_string());
                    }
                    app.session_name = name;
                    // Move the guard from `__warm__` onto the claimed name (#505).
                    // Releasing the warm name is what frees it for the replacement
                    // warm spawned further below (issue #459).
                    rekey_session_guard(&mut session_guard, &app.port_file_base());
                    // Warm server's created_at is the warm process start time, not the
                    // user's session-creation time — reset on claim or list-sessions /
                    // session_created / uptime would report the warm pool's age.
                    app.created_at = chrono::Local::now();
                    // Same reason for the status-interval phase: the warm server skips
                    // the timer (see should_run_status_interval_timer), so its last-fire
                    // stamp is the warm start time — reset it so a claimed session fires
                    // one interval after creation, not immediately.
                    app.last_status_interval_fire = std::time::Instant::now();
                    // Update env so run-shell/hooks from this server target the new name
                    env::set_var("PSMUX_TARGET_SESSION", app.port_file_base());
                    // Honour the client's working directory: the warm server
                    // was spawned from a previous session whose CWD may differ
                    // from where the user ran `psmux` now.  Update the
                    // server's CWD (for future pane spawns) and silently
                    // inject `cd` into the active pane so the shell starts
                    // in the right directory.  A clear screen command is
                    // chained after cd so the user never sees the injected
                    // command or its echo.
                    if let Some(ref cwd) = client_cwd {
                        let cwd_path = std::path::Path::new(cwd);
                        if cwd_path.is_dir() {
                            let server_cwd_differs = env::current_dir()
                                .map(|cur| cur != cwd_path)
                                .unwrap_or(true);
                            if server_cwd_differs {
                                env::set_current_dir(cwd_path).ok();
                                // Silently re-home the warm server's active pane
                                // to the client's CWD (invisible cd + clear), so
                                // the shell it pre-spawned adopts the right dir.
                                // The snippet is written in the dialect of that
                                // shell, not of the host OS (#600).
                                let syntax = crate::pane::rehome_syntax_for_shell(&app.default_shell);
                                if let Some(win) = app.windows.last_mut() {
                                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                                        crate::pane::silent_rehome(p, cwd, syntax);
                                    }
                                }
                            }
                        }
                    }
                    // Update env so run-shell/hooks from this server target the new name
                    env::set_var("PSMUX_TARGET_SESSION", app.port_file_base());
                    // Re-load user config so the claimed session reflects the
                    // current config file.  The warm server loaded config at
                    // its own startup, but the user may have changed their
                    // config since then (or the warm server was spawned by a
                    // different session with a different PSMUX_CONFIG_FILE).
                    // Clear config-derived state first so the reload is authoritative:
                    // hooks removed from the config drop out, and `set-hook -a`
                    // append hooks don't stack a second copy onto the warm server's.
                    app.key_tables.clear();
                    app.hooks.clear();
                    app.defaults_suppressed = false;
                    crate::config::populate_default_bindings(&mut app);
                    load_config(&mut app);
                    // Surface config warnings to the claiming client (#370 follow-up).
                    write_config_warnings_log(&app.config_warnings);
                    // Config may set pane-border-status (#288)
                    resize_all_panes(&mut app);
                    // Update shared aliases after config reload
                    if let Ok(mut w) = shared_aliases_main.write() {
                        *w = app.command_aliases.clone();
                    }
                    // Adopt the claiming client's scheduling class (#608).
                    //
                    // MUST be after load_config above, not before: that reload
                    // re-applies the `priority` option out of the config file
                    // and would clobber anything set earlier in this arm.
                    //
                    // This is what makes the documented escape hatch work on
                    // the path most users actually take. A warm standby set its
                    // class at its OWN startup, from the environment of
                    // whichever server generation spawned it, which predates
                    // the user's shell entirely. Claiming it used to leave that
                    // stale class in place, so `PSMUX_PRIORITY=normal psmux`
                    // silently kept the above-normal default whenever a standby
                    // happened to be waiting, and cold spawned correctly
                    // whenever one did not. The value arrives already resolved
                    // by the client under the documented precedence (env, then
                    // option, then default), so applying it here simply
                    // finishes that resolution on the right process.
                    if let Some(ref want) = client_priority {
                        if crate::platform::normalize_priority(want).is_some() {
                            app.priority = crate::platform::resolve_priority(Some(want), false);
                            crate::platform::set_process_priority(&app.priority);
                            warm_debug(&format!("CLAIM: priority -> {}", app.priority));
                        }
                    }
                    // Fire client-attached/session-created for the now-real session:
                    // the startup path skips these while warm, so this is where a
                    // claimed session gets them - exactly once - starting plugins
                    // like continuum's auto-save and auto-restore.
                    crate::commands::fire_hooks(&mut app, "client-attached");
                    crate::commands::fire_hooks(&mut app, "session-created");
                    // Fire client-session-changed hook (warm server claimed by new session)
                    if let Some(cmds) = app.hooks.get("client-session-changed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    meta_dirty = true;
                    state_dirty = true;
                    let _ = resp.send("OK\n".to_string());
                    // Spawn a replacement warm server for the NEXT new-session
                    spawn_warm_server(&app);
                    hook_event = Some("after-rename-session");
                    }
                }
                CtrlReq::SwapPane(dir) => {
                    // tmux: swap-pane without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    let swapped = match dir.as_str() {
                        "U" => swap_pane(&mut app, FocusDir::Up),
                        "D" => swap_pane(&mut app, FocusDir::Down),
                        "L" => swap_pane(&mut app, FocusDir::Left),
                        "R" => swap_pane(&mut app, FocusDir::Right),
                        _ => swap_pane(&mut app, FocusDir::Down),
                    };
                    if swapped {
                        meta_dirty = true;
                        hook_event = Some("after-swap-pane");
                    }
                }
                CtrlReq::SwapPaneTarget(target, is_id) => {
                    // tmux: swap-pane without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    let path = {
                        let win = &app.windows[app.active_idx];
                        if is_id {
                            crate::tree::find_path_by_id(&win.root, target)
                        } else {
                            match target.checked_sub(app.pane_base_index) {
                                Some(idx) => crate::tree::path_by_position(&win.root, idx),
                                None => None,
                            }
                        }
                    };
                    if let Some(path) = path {
                        if swap_pane_with_path(&mut app, path) {
                            meta_dirty = true;
                            hook_event = Some("after-swap-pane");
                        }
                    } else {
                        app.status_message = Some((format!("swap-pane: can't find pane: {}", target), std::time::Instant::now(), None));
                    }
                }
                CtrlReq::SwapPaneSrcDst { src, src_is_id, dst, dst_is_id, detach } => {
                    // swap-pane -s <src> -t <dst>: swap two explicit panes (#442).
                    // tmux: swap-pane without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    fn resolve_pane_path(app: &AppState, val: usize, is_id: bool) -> Option<Vec<usize>> {
                        let win = &app.windows[app.active_idx];
                        if is_id {
                            crate::tree::find_path_by_id(&win.root, val)
                        } else {
                            val.checked_sub(app.pane_base_index)
                                .and_then(|idx| crate::tree::path_by_position(&win.root, idx))
                        }
                    }
                    let sp = resolve_pane_path(&app, src, src_is_id);
                    let dp = resolve_pane_path(&app, dst, dst_is_id);
                    match (sp, dp) {
                        (Some(sp), Some(dp)) => {
                            if crate::window_ops::swap_pane_between(&mut app, sp, dp, detach) {
                                meta_dirty = true;
                                hook_event = Some("after-swap-pane");
                            }
                        }
                        _ => {
                            app.status_message = Some(("swap-pane: can't find pane".to_string(), std::time::Instant::now(), None));
                        }
                    }
                }
                CtrlReq::SwapPanePosition(token) => {
                    // tmux: swap-pane without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    if let Some(path) = crate::window_ops::pane_path_at_position(&app, &token) {
                        if swap_pane_with_path(&mut app, path) {
                            meta_dirty = true;
                            hook_event = Some("after-swap-pane");
                        }
                    } else {
                        app.status_message = Some((format!("swap-pane: can't find pane: {}", token), std::time::Instant::now(), None));
                    }
                }
                CtrlReq::ResizePane(dir, amount) => {
                    // A focused floating pane resizes itself (and its PTY) instead
                    // of the tiled layout.
                    let mut handled_float = false;
                    {
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if let Some(fp) = win.floating.get_mut(fi) {
                                let d = amount as i16;
                                match dir.as_str() {
                                    "L" => fp.w = (fp.w as i16 - d).max(3) as u16,
                                    "R" => fp.w = (fp.w as i16 + d).max(3) as u16,
                                    "U" => fp.h = (fp.h as i16 - d).max(3) as u16,
                                    "D" => fp.h = (fp.h as i16 + d).max(3) as u16,
                                    _ => {}
                                }
                                fp.w = fp.w.min(win_w);
                                fp.h = fp.h.min(win_h);
                                let (nx, ny) = crate::floating::clamp_into(fp.x, fp.y, fp.w, fp.h, win_w, win_h);
                                fp.x = nx; fp.y = ny;
                                let inner_h = fp.h.saturating_sub(2).max(1);
                                let inner_w = fp.w.saturating_sub(2).max(1);
                                if fp.pane.last_rows != inner_h || fp.pane.last_cols != inner_w {
                                    let _ = fp.pane.master.resize(portable_pty::PtySize { rows: inner_h, cols: inner_w, pixel_width: 0, pixel_height: 0 });
                                    if let Ok(mut parser) = fp.pane.term.lock() { parser.screen_mut().set_size(inner_h, inner_w); }
                                    fp.pane.last_rows = inner_h;
                                    fp.pane.last_cols = inner_w;
                                }
                                handled_float = true;
                            }
                        }
                    }
                    if handled_float {
                        state_dirty = true;
                    } else {
                        unzoom_if_zoomed(&mut app);
                        match dir.as_str() {
                            "U" | "D" => { resize_pane_vertical(&mut app, if dir == "U" { -(amount as i16) } else { amount as i16 }); }
                            "L" | "R" => { resize_pane_horizontal(&mut app, if dir == "L" { -(amount as i16) } else { amount as i16 }); }
                            _ => {}
                        }
                        resize_all_panes(&mut app); meta_dirty = true;
                        hook_event = Some("after-resize-pane");
                    }
                }
                CtrlReq::SetBuffer(content) => {
                    app.paste_buffers.insert(0, content);
                    if app.paste_buffers.len() > 10 { app.paste_buffers.pop(); }
                }
                CtrlReq::SetNamedBuffer(name, content) => {
                    app.named_buffers.insert(name, content);
                }
                CtrlReq::ListBuffers(resp) => {
                    let mut output = String::new();
                    // List auto-named buffers (positional stack)
                    for (i, buf) in app.paste_buffers.iter().enumerate() {
                        let preview: String = buf.chars().take(50).collect();
                        output.push_str(&format!("buffer{}: {} bytes: \"{}\"\n", i, buf.len(), preview));
                    }
                    // List named buffers
                    let mut names: Vec<&String> = app.named_buffers.keys().collect();
                    names.sort();
                    for name in names {
                        let buf = &app.named_buffers[name];
                        let preview: String = buf.chars().take(50).collect();
                        output.push_str(&format!("{}: {} bytes: \"{}\"\n", name, buf.len(), preview));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListBuffersFormat(resp, fmt) => {
                    let mut output = Vec::new();
                    for (i, _buf) in app.paste_buffers.iter().enumerate() {
                        set_buffer_idx_override(Some(i));
                        output.push(expand_format(&fmt, &app));
                        set_buffer_idx_override(None);
                    }
                    // Named buffers with format: use name override
                    let mut names: Vec<String> = app.named_buffers.keys().cloned().collect();
                    names.sort();
                    for name in &names {
                        set_named_buffer_override(Some(name.clone()));
                        output.push(expand_format(&fmt, &app));
                        set_named_buffer_override(None);
                    }
                    let _ = resp.send(output.join("\n"));
                }
                CtrlReq::ShowBuffer(resp) => {
                    let content = app.paste_buffers.first().cloned().unwrap_or_default();
                    let _ = resp.send(content);
                }
                CtrlReq::ShowBufferAt(resp, idx) => {
                    let content = app.paste_buffers.get(idx).cloned();
                    let _ = resp.send(content);
                }
                CtrlReq::ShowNamedBuffer(resp, name) => {
                    let content = app.named_buffers.get(&name).cloned();
                    let _ = resp.send(content);
                }
                CtrlReq::DeleteBuffer => {
                    if !app.paste_buffers.is_empty() { app.paste_buffers.remove(0); }
                }
                CtrlReq::DeleteBufferAt(idx) => {
                    if idx < app.paste_buffers.len() { app.paste_buffers.remove(idx); }
                }
                CtrlReq::DeleteNamedBuffer(name) => {
                    app.named_buffers.remove(&name);
                }
                CtrlReq::PasteBufferAt(idx) => {
                    if idx < app.paste_buffers.len() {
                        let text = app.paste_buffers[idx].clone();
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = write!(p.writer, "{}", text);
                        }
                    }
                }
                CtrlReq::DisplayMessage(resp, fmt, target_pane_idx, set_status_bar, duration_ms) => {
                    // Propagate OSC titles so #{pane_title} reflects latest state
                    helpers::propagate_osc_titles(&mut app);
                    let result = if let Some(pane_idx) = target_pane_idx {
                        // -t targeting: evaluate format for the specific pane
                        // using PANE_POS_OVERRIDE so #{pane_active} reflects
                        // the REAL active pane, not the target (#113)
                        crate::format::expand_format_for_pane(&fmt, &app, app.active_idx, pane_idx)
                    } else {
                        expand_format(&fmt, &app)
                    };
                    if set_status_bar {
                        app.status_message = Some((result.clone(), Instant::now(), duration_ms));
                        state_dirty = true;
                    }
                    let _ = resp.send(result);
                }
                CtrlReq::DisplayMessageById(resp, fmt, pane_id, set_status_bar, duration_ms) => {
                    // Bare %N pane targeting (#332) — resolve the pane ID
                    // globally across all windows and expand the format with
                    // PANE_POS_OVERRIDE pointing at it.
                    helpers::propagate_osc_titles(&mut app);
                    let result = crate::format::expand_format_for_pane_by_id(&fmt, &app, pane_id);
                    if set_status_bar {
                        app.status_message = Some((result.clone(), Instant::now(), duration_ms));
                        state_dirty = true;
                    }
                    let _ = resp.send(result);
                }
                CtrlReq::LastWindow => {
                    if app.windows.len() > 1 && app.last_window_idx < app.windows.len() {
                        switch_with_copy_save(&mut app, |app| {
                            let tmp = app.active_idx;
                            app.active_idx = app.last_window_idx;
                            app.last_window_idx = tmp;
                        });
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                CtrlReq::LastPane => {
                    switch_with_copy_save(&mut app, |app| {
                        let win = &mut app.windows[app.active_idx];
                        if !app.last_pane_path.is_empty() && path_exists(&win.root, &app.last_pane_path) {
                            let tmp = win.active_path.clone();
                            win.active_path = app.last_pane_path.clone();
                            app.last_pane_path = tmp;
                        } else if !win.active_path.is_empty() {
                            let last = win.active_path.last_mut();
                            if let Some(idx) = last {
                                *idx = (*idx + 1) % 2;
                            }
                        }
                    });
                    meta_dirty = true;
                }
                CtrlReq::RotateWindow(reverse) => {
                    rotate_panes(&mut app, reverse);
                    hook_event = Some("after-rotate-window");
                }
                CtrlReq::DisplayPanes => {
                    app.mode = Mode::PaneChooser { opened_at: std::time::Instant::now() };
                    state_dirty = true;
                }
                CtrlReq::DisplayPaneSelect(digit) => {
                    // User pressed a digit during display-panes overlay: select the matching pane
                    let win = &app.windows[app.active_idx];
                    let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
                    crate::tree::compute_rects(&win.root, app.last_window_area, &mut rects);
                    for (i, (path, _)) in rects.iter().enumerate() {
                        if i >= 10 { break; }
                        let mapped = (i + app.pane_base_index) % 10;
                        if mapped == digit {
                            let new_path = path.clone();
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            app.windows[app.active_idx].active_path = new_path;
                            if app.windows[app.active_idx].active_path != old_path {
                                app.last_pane_path = old_path;
                            }
                            break;
                        }
                    }
                    app.mode = Mode::Passthrough;
                    state_dirty = true;
                    meta_dirty = true;
                }
                CtrlReq::BreakPane => {
                    unzoom_if_zoomed(&mut app);
                    break_pane_to_window(&mut app);
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    hook_event = Some("after-break-pane");
                    meta_dirty = true;
                }
                CtrlReq::JoinPane { src_win, src_pane, target_win, target_pane, horizontal }
                | CtrlReq::MovePane { src_win, src_pane, target_win, target_pane, horizontal } => {
                    unzoom_if_zoomed(&mut app);
                    // Resolve source/target display indices to Vec positions
                    // (default: active window). win_pos honors gapped indices.
                    let src_pos = match src_win { Some(d) => app.win_pos(d), None => Some(app.active_idx) };
                    let tgt_pos = match target_win { Some(d) => app.win_pos(d), None => Some(app.active_idx) };
                    // Surface an explicit error instead of silently doing nothing when the
                    // target cannot be resolved (issue #437). psmux defaults base-index to 0,
                    // so a tmux user typing `join-pane -t :2` on a 2-window session targets a
                    // non-existent window; the old silent no-op made join-pane appear broken.
                    if src_pos.is_none() {
                        app.status_message = Some((format!("join-pane: can't find source window: {}", src_win.unwrap_or(0)), Instant::now(), None));
                        meta_dirty = true;
                    } else if tgt_pos.is_none() {
                        app.status_message = Some((format!("join-pane: can't find window: {}", target_win.unwrap_or(0)), Instant::now(), None));
                        meta_dirty = true;
                    } else if src_pos == tgt_pos {
                        app.status_message = Some(("join-pane: can't join a pane to its own window".to_string(), Instant::now(), None));
                        meta_dirty = true;
                    } else {
                        let src_idx = src_pos.unwrap();
                        let raw_target_win = tgt_pos.unwrap();
                        // Resolve source pane path within source window
                        let src_path = if let Some(pidx) = src_pane {
                            // Get Nth pane path in DFS order
                            let mut leaves = Vec::new();
                            tree::collect_leaf_paths_pub(&app.windows[src_idx].root, &mut Vec::new(), &mut leaves);
                            if let Some((_, p)) = leaves.get(pidx) {
                                p.clone()
                            } else {
                                app.windows[src_idx].active_path.clone()
                            }
                        } else {
                            app.windows[src_idx].active_path.clone()
                        };
                        // Unzoom source window if needed
                        if let Some(saved) = app.windows[src_idx].zoom_saved.take() {
                            let win = &mut app.windows[src_idx];
                            for (p, sz) in saved.into_iter() {
                                if let Some(Node::Split { sizes, .. }) = crate::tree::get_split_mut(&mut win.root, &p) { *sizes = sz; }
                            }
                        }
                        let src_root = std::mem::replace(&mut app.windows[src_idx].root,
                            Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
                        let (remaining, extracted) = tree::extract_node(src_root, &src_path);
                        if let Some(pane_node) = extracted {
                            let src_empty = remaining.is_none();
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                                app.windows[src_idx].active_path = tree::first_leaf_path(&app.windows[src_idx].root);
                            }
                            // Adjust target index if source window will be removed and target is after it
                            let tgt = if src_empty && raw_target_win > src_idx { raw_target_win - 1 } else { raw_target_win };
                            if src_empty {
                                app.windows.remove(src_idx);
                                app.on_window_removed(src_idx);
                                if app.active_idx >= app.windows.len() {
                                    app.active_idx = app.windows.len().saturating_sub(1);
                                }
                            }
                            // Graft pane into target window
                            if tgt < app.windows.len() {
                                // Resolve target pane path
                                let tgt_path = if let Some(tpidx) = target_pane {
                                    let mut leaves = Vec::new();
                                    tree::collect_leaf_paths_pub(&app.windows[tgt].root, &mut Vec::new(), &mut leaves);
                                    if let Some((_, p)) = leaves.get(tpidx) {
                                        p.clone()
                                    } else {
                                        app.windows[tgt].active_path.clone()
                                    }
                                } else {
                                    app.windows[tgt].active_path.clone()
                                };
                                let split_kind = if horizontal { LayoutKind::Horizontal } else { LayoutKind::Vertical };
                                tree::replace_leaf_with_split(&mut app.windows[tgt].root, &tgt_path, split_kind, pane_node);
                                app.active_idx = tgt;
                            }
                            resize_all_panes(&mut app);
                            meta_dirty = true;
                            hook_event = Some("after-join-pane");
                        } else {
                            // Extraction failed — restore
                            if let Some(rem) = remaining {
                                app.windows[src_idx].root = rem;
                            }
                        }
                    }
                }
                // ── Cross-session pane forwarding ───────────────────────
                CtrlReq::PaneForwardExtract(win_idx, pane_idx, resp) => {
                    crate::cross_session_server::handle_pane_forward_extract(&mut app, win_idx, pane_idx, resp);
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                }
                CtrlReq::PaneForwardInject {
                    source_session, source_addr, source_key,
                    forward_id, fwd_port, pid, title, rows, cols,
                    screen_b64, target_win, target_pane, horizontal,
                } => {
                    crate::cross_session_server::handle_pane_forward_inject(
                        &mut app, source_session, source_addr, source_key,
                        forward_id, fwd_port, pid, title, rows, cols,
                        screen_b64, target_win, target_pane, horizontal,
                    );
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    hook_event = Some("after-join-pane");
                }
                CtrlReq::PaneForwardResize(fwd_id, fwd_rows, fwd_cols) => {
                    if let Some(fp) = app.forwarded_panes.get(&fwd_id) {
                        let _ = fp.master.resize(portable_pty::PtySize {
                            rows: fwd_rows, cols: fwd_cols, pixel_width: 0, pixel_height: 0,
                        });
                    }
                }
                CtrlReq::PaneForwardStatus(fwd_id, resp) => {
                    let status = if let Some(fp) = app.forwarded_panes.get_mut(&fwd_id) {
                        match fp.child.try_wait() {
                            Ok(Some(_)) => "exited".to_string(),
                            Ok(None) => "running".to_string(),
                            Err(_) => "exited".to_string(),
                        }
                    } else {
                        "exited".to_string()
                    };
                    let _ = resp.send(status);
                }
                CtrlReq::PaneForwardKill(fwd_id) => {
                    if let Some(mut fp) = app.forwarded_panes.remove(&fwd_id) {
                        fp.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
                        let _ = fp.child.kill();
                    }
                }
                CtrlReq::SetPaneOption(raw, option, value, resp) => {
                    // Resolve "" -> active pane, "%N"/"N" -> global pane id.
                    let pane = if raw.is_empty() {
                        let win = &mut app.windows[app.active_idx];
                        crate::tree::active_pane_mut(&mut win.root, &win.active_path)
                    } else {
                        match raw.trim().trim_start_matches('%').parse::<usize>() {
                            Ok(id) => crate::tree::find_pane_mut_by_id_global(&mut app, id),
                            Err(_) => None,
                        }
                    };
                    let reply = match pane {
                        None => format!("ERROR: can't find pane: {}", raw),
                        Some(p) => match option.as_str() {
                            "remain-on-exit" => {
                                if value.is_empty() {
                                    p.pane_options.remove("remain-on-exit");
                                    String::new()
                                } else if matches!(value.as_str(), "on" | "off" | "failed") {
                                    p.pane_options.insert("remain-on-exit".to_string(), value.clone());
                                    String::new()
                                } else {
                                    format!("ERROR: set-option -p remain-on-exit: bad value '{}' (want on, off or failed)", value)
                                }
                            }
                            // #613: authorize the wheel for THIS pane even when
                            // its application asked through neither signal the
                            // #598 gate accepts.  Pane scoped on purpose: the
                            // damage the gate prevents (htop typing the report
                            // into its search prompt) is decided per pane, so
                            // the escape hatch is too.
                            "@mouse-force" => {
                                if value.is_empty() {
                                    p.pane_options.remove("@mouse-force");
                                    String::new()
                                } else if matches!(value.as_str(), "on" | "off" | "1" | "0" | "true" | "false" | "yes" | "no") {
                                    p.pane_options.insert("@mouse-force".to_string(), value.clone());
                                    String::new()
                                } else {
                                    format!("ERROR: set-option -p @mouse-force: bad value '{}' (want on or off)", value)
                                }
                            }
                            // Loud refusal, never a silent stored no-op: the
                            // Claude Code teammate backend checked nothing but
                            // exit codes and a swallowed pane option looked
                            // exactly like success (#580).
                            other => format!("ERROR: pane-scoped option '{}' is not supported (supported: remain-on-exit, @mouse-force)", other),
                        },
                    };
                    let _ = resp.send(reply);
                }
                CtrlReq::ShowPaneOptions(raw, resp) => {
                    let pane = if raw.is_empty() {
                        let win = &mut app.windows[app.active_idx];
                        crate::tree::active_pane_mut(&mut win.root, &win.active_path)
                    } else {
                        match raw.trim().trim_start_matches('%').parse::<usize>() {
                            Ok(id) => crate::tree::find_pane_mut_by_id_global(&mut app, id),
                            Err(_) => None,
                        }
                    };
                    let reply = match pane {
                        None => format!("ERROR: can't find pane: {}", raw),
                        Some(p) => {
                            let mut keys: Vec<&String> = p.pane_options.keys().collect();
                            keys.sort();
                            keys.iter()
                                .map(|k| format!("{} {}", k, p.pane_options[*k]))
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    let _ = resp.send(reply);
                }
                CtrlReq::RespawnPane(workdir, kill, command, empty) => {
                    respawn_active_pane(&mut app, Some(&*pty_system), workdir.as_deref(), kill, command.as_deref(), empty)?;
                    hook_event = Some("after-respawn-pane");
                }
                CtrlReq::BindKey(table_name, key, command, repeat) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let kc = normalize_key_for_binding(kc);
                        // Support `\;` chaining in server-side bind-key
                        let sub_cmds = crate::config::split_chained_commands_pub(&command);
                        let action = if sub_cmds.len() > 1 {
                            Some(Action::CommandChain(sub_cmds))
                        } else {
                            parse_command_to_action(&command)
                        };
                        if let Some(act) = action {
                            let table = app.key_tables.entry(table_name).or_default();
                            table.retain(|b| b.key != kc);
                            table.push(Bind { key: kc, action: act, repeat });
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::UnbindKey(key, table) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let kc = normalize_key_for_binding(kc);
                        let target = table.unwrap_or_else(|| "prefix".to_string());
                        if let Some(binds) = app.key_tables.get_mut(&target) {
                            binds.retain(|b| b.key != kc);
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::UnbindAll => {
                    app.key_tables.clear();
                    app.defaults_suppressed = true;
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::UnbindAllInTable(table) => {
                    if let Some(binds) = app.key_tables.get_mut(&table) {
                        binds.clear();
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::ListKeys(resp) => {
                    // Build list-keys output from the canonical help module
                    let user_iter = app.key_tables.iter().flat_map(|(table_name, binds)| {
                        binds.iter().map(move |bind| {
                            let key_str = format_key_binding(&bind.key);
                            let action_str = format_action(&bind.action);
                            (table_name.as_str(), key_str, action_str, bind.repeat)
                        })
                    });
                    let output = help::build_list_keys_output(user_iter, app.defaults_suppressed);
                    let _ = resp.send(output);
                }
                CtrlReq::SetOption(option, value) => {
                    apply_set_option(&mut app, &option, &value, false);
                    app.user_set_options.insert(option.clone());
                    // Reconcile the warm pane with the new option value.
                    // All option-driven warm-pane lifecycle decisions
                    // route through this single module — see #271.
                    let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    // Update shared aliases if command-alias changed
                    if option == "command-alias" {
                        if let Ok(mut map) = shared_aliases_main.write() {
                            *map = app.command_aliases.clone();
                        }
                    }
                    // pane-border-status changes the effective content height (#288)
                    if option == "pane-border-status" {
                        resize_all_panes(&mut app);
                    }
                    if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                        resize_all_panes(&mut app);
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::SetWindowSize(value) => {
                    match crate::resize_window::set_active_window_size_mode(&mut app, value) {
                        Ok(()) => {
                            meta_dirty = true;
                            state_dirty = true;
                        }
                        Err(error) => {
                            app.status_message = Some((error, Instant::now(), None));
                        }
                    }
                }
                CtrlReq::SetOptionQuiet(option, value, quiet) => {
                    apply_set_option(&mut app, &option, &value, quiet);
                    app.user_set_options.insert(option.clone());
                    // Reconcile the warm pane with the new option value.
                    // Replaces the prior inline default-shell-only kill
                    // (#99) with a uniform table-driven policy that
                    // also covers history-limit (#271), allow-predictions,
                    // default-terminal, and claude-code-* options.
                    let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    // Update shared aliases if command-alias changed
                    if option == "command-alias" {
                        if let Ok(mut map) = shared_aliases_main.write() {
                            *map = app.command_aliases.clone();
                        }
                    }
                    // pane-border-status changes the effective content height (#288)
                    if option == "pane-border-status" {
                        resize_all_panes(&mut app);
                    }
                    if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                        resize_all_panes(&mut app);
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::SetOptionUnset(option) => {
                    // Reset option to default or remove @user-option
                    if option.starts_with('@') {
                        app.user_options.remove(&option);
                    } else {
                        match option.as_str() {
                            "status-left" => { app.status_left = "psmux:#I".to_string(); }
                            "status-right" => { app.status_right = "#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}\"#{=21:pane_title}\" %H:%M %d-%b-%y".to_string(); }
                            "mouse" => { app.mouse_enabled = true; }
                            "scroll-enter-copy-mode" => { app.scroll_enter_copy_mode = true; }
                            "pwsh-mouse-selection" => { app.pwsh_mouse_selection = false; }
                            "mouse-selection" => { app.mouse_selection = true; }
                            "mouse-selection-force" => { app.mouse_selection_force = false; }
                            "paste-detection" => { app.paste_detection = true; }
                            "choose-tree-preview" => { app.choose_tree_preview = false; }
                            "escape-time" => { app.escape_time_ms = 500; }
                            // #606: tmux restores the table default on -u.
                            "repeat-time" => { app.repeat_time_ms = 500; }
                            "history-limit" => { app.history_limit = 2000; }
                            "alternate-screen" => { app.allow_alternate_screen = true; }
                            "display-time" => { app.display_time_ms = 750; }
                            "mode-keys" => { app.mode_keys = "emacs".to_string(); }
                            "status" => { app.status_visible = true; }
                            "status-position" => { app.status_position = "bottom".to_string(); }
                            "status-style" => { app.status_style = String::new(); }
                            "renumber-windows" => { app.renumber_windows = false; }
                            "remain-on-exit" => { app.remain_on_exit = false; }
                            "destroy-unattached" => { app.destroy_unattached = false; }
                            "exit-empty" => { app.exit_empty = true; }
                            "automatic-rename" => { app.automatic_rename = true; }
                            "pane-border-style" => { app.pane_border_style = String::new(); }
                            "pane-active-border-style" => { app.pane_active_border_style = "fg=green".to_string(); }
                            "pane-border-hover-style" => { app.pane_border_hover_style = "fg=yellow".to_string(); }
                            "window-style" | "window-active-style" => {
                                app.user_options.remove(&option);
                                app.user_set_options.remove(&option);
                                state_dirty = true;
                            }
                            "window-status-format" => { app.window_status_format = "#I:#W#{?window_flags,#{window_flags}, }".to_string(); }
                            "window-status-current-format" => { app.window_status_current_format = "#I:#W#{?window_flags,#{window_flags}, }".to_string(); }
                            "window-status-separator" => { app.window_status_separator = " ".to_string(); }
                            "cursor-style" => { std::env::set_var("PSMUX_CURSOR_STYLE", "bar"); }
                            "cursor-blink" => { std::env::set_var("PSMUX_CURSOR_BLINK", "1"); }
                            _ => {}
                        }
                    }
                }
                CtrlReq::SetOptionAppend(option, value) => {
                    // Append to existing option value
                    if option.starts_with('@') {
                        let existing = app.user_options.get(&option).cloned().unwrap_or_default();
                        app.user_options.insert(option, format!("{}{}", existing, value));
                    } else {
                        match option.as_str() {
                            "status-left" => { app.status_left.push_str(&value); }
                            "status-right" => { app.status_right.push_str(&value); }
                            "status-style" => { app.status_style.push_str(&value); }
                            "pane-border-style" => { app.pane_border_style.push_str(&value); }
                            "pane-active-border-style" => { app.pane_active_border_style.push_str(&value); }
                            "pane-border-hover-style" => { app.pane_border_hover_style.push_str(&value); }
                            "window-style" | "window-active-style" => {
                                app.user_options
                                    .entry(option.clone())
                                    .or_default()
                                    .push_str(&value);
                                app.user_set_options.insert(option.clone());
                                state_dirty = true;
                            }
                            "window-status-format" => { app.window_status_format.push_str(&value); }
                            "window-status-current-format" => { app.window_status_current_format.push_str(&value); }
                            _ => {}
                        }
                    }
                }
                CtrlReq::SetOptionToggle(option) => {
                    // `set -g <bool-option>` with no value flips it (#535).
                    // Only the server knows the current value, so the flip has
                    // to happen here rather than client-side.
                    if crate::server::options::toggle_option(&mut app, &option) {
                        app.user_set_options.insert(option.clone());
                        let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                        crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                        meta_dirty = true;
                        state_dirty = true;
                    }
                }
                CtrlReq::SetOptionOnlyIfUnset(option, value) => {
                    let already_set = if option.starts_with('@') {
                        app.user_options.contains_key(&option)
                    } else {
                        app.user_set_options.contains(&option)
                    };
                    if !already_set {
                        apply_set_option(&mut app, &option, &value, false);
                        app.user_set_options.insert(option.clone());
                        if option == "command-alias" {
                            if let Ok(mut map) = shared_aliases_main.write() {
                                *map = app.command_aliases.clone();
                            }
                        }
                        meta_dirty = true;
                        state_dirty = true;
                    }
                }
                CtrlReq::ShowOptions(resp) => {
                    let mut output = String::new();
                    output.push_str(&format!("prefix {}\n", format_key_binding(&app.prefix_key)));
                    if let Some(ref p2) = app.prefix2_key {
                        output.push_str(&format!("prefix2 {}\n", format_key_binding(p2)));
                    }
                    output.push_str(&format!("base-index {}\n", app.window_base_index));
                    output.push_str(&format!("pane-base-index {}\n", app.pane_base_index));
                    output.push_str(&format!("escape-time {}\n", app.escape_time_ms));
                    output.push_str(&format!("mouse {}\n", if app.mouse_enabled { "on" } else { "off" }));
                    output.push_str(&format!("scroll-enter-copy-mode {}\n", if app.scroll_enter_copy_mode { "on" } else { "off" }));
                    output.push_str(&format!("pwsh-mouse-selection {}\n", if app.pwsh_mouse_selection { "on" } else { "off" }));
                    output.push_str(&format!("mouse-selection {}\n", if app.mouse_selection { "on" } else { "off" }));
                    output.push_str(&format!("mouse-selection-force {}\n", if app.mouse_selection_force { "on" } else { "off" }));
                    output.push_str(&format!("paste-detection {}\n", if app.paste_detection { "on" } else { "off" }));
                    output.push_str(&format!("choose-tree-preview {}\n", if app.choose_tree_preview { "on" } else { "off" }));
                    output.push_str(&format!("status {}\n", if app.status_visible { "on" } else { "off" }));
                    output.push_str(&format!("status-position {}\n", app.status_position));
                    output.push_str(&format!("status-left \"{}\"\n", app.status_left));
                    output.push_str(&format!("status-right \"{}\"\n", app.status_right));
                    output.push_str(&format!("history-limit {}\n", app.history_limit));
                    output.push_str(&format!("display-time {}\n", app.display_time_ms));
                    output.push_str(&format!("display-panes-time {}\n", app.display_panes_time_ms));
                    // #606: repeat-time answered `show-options -g repeat-time`
                    // but was missing from this full dump, so the obvious way
                    // to check it (`show-options -g | findstr repeat`) said the
                    // option did not exist. Same shape as #559 for
                    // monitor-silence.
                    output.push_str(&format!("repeat-time {}\n", app.repeat_time_ms));
                    // #608: this dump is hand written, so a new option is
                    // invisible to a bare `show-options -g` unless it is listed
                    // here as well as in get_option_value. Same trap as #606.
                    output.push_str(&format!("priority {}\n", app.priority));
                    output.push_str(&format!("mode-keys {}\n", app.mode_keys));
                    output.push_str(&format!("focus-events {}\n", if app.focus_events { "on" } else { "off" }));
                    output.push_str(&format!("renumber-windows {}\n", if app.renumber_windows { "on" } else { "off" }));
                    output.push_str(&format!("automatic-rename {}\n", if app.automatic_rename { "on" } else { "off" }));
                    output.push_str(&format!("monitor-activity {}\n", if app.monitor_activity { "on" } else { "off" }));
                    // #559: monitor-silence was queryable one-by-one but absent
                    // from this full dump, so `show-options -g | grep monitor`
                    // looked like the option had been silently dropped.
                    output.push_str(&format!("monitor-silence {}\n", app.monitor_silence));
                    output.push_str(&format!("synchronize-panes {}\n", if app.sync_input { "on" } else { "off" }));
                    output.push_str(&format!("remain-on-exit {}\n", if app.remain_on_exit { "on" } else { "off" }));
                    output.push_str(&format!("destroy-unattached {}\n", if app.destroy_unattached { "on" } else { "off" }));
                    output.push_str(&format!("exit-empty {}\n", if app.exit_empty { "on" } else { "off" }));
                    output.push_str(&format!("set-titles {}\n", if app.set_titles { "on" } else { "off" }));
                    if !app.set_titles_string.is_empty() {
                        output.push_str(&format!("set-titles-string \"{}\"\n", app.set_titles_string));
                    }
                    output.push_str(&format!(
                        "prediction-dimming {}\n",
                        if app.prediction_dimming { "on" } else { "off" }
                    ));
                    output.push_str(&format!("allow-predictions {}\n", if app.allow_predictions { "on" } else { "off" }));
                    output.push_str(&format!("cursor-style {}\n", std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "bar".to_string())));
                    output.push_str(&format!("cursor-blink {}\n", if std::env::var("PSMUX_CURSOR_BLINK").unwrap_or_else(|_| "1".to_string()) != "0" { "on" } else { "off" }));
                    {
                        let shell_val = if app.default_shell.is_empty() {
                            crate::pane::cached_shell().unwrap_or("pwsh.exe").to_string()
                        } else {
                            app.default_shell.clone()
                        };
                        output.push_str(&format!("default-shell {}\n", shell_val));
                    }
                    output.push_str(&format!("word-separators \"{}\"\n", app.word_separators));
                    if !app.pane_border_style.is_empty() {
                        output.push_str(&format!("pane-border-style \"{}\"\n", app.pane_border_style));
                    }
                    if !app.pane_active_border_style.is_empty() {
                        output.push_str(&format!("pane-active-border-style \"{}\"\n", app.pane_active_border_style));
                    }
                    if !app.pane_border_hover_style.is_empty() {
                        output.push_str(&format!("pane-border-hover-style \"{}\"\n", app.pane_border_hover_style));
                    }
                    if !app.status_style.is_empty() {
                        output.push_str(&format!("status-style \"{}\"\n", app.status_style));
                    }
                    if !app.status_left_style.is_empty() {
                        output.push_str(&format!("status-left-style \"{}\"\n", app.status_left_style));
                    }
                    if !app.status_right_style.is_empty() {
                        output.push_str(&format!("status-right-style \"{}\"\n", app.status_right_style));
                    }
                    output.push_str(&format!("status-interval {}\n", app.status_interval));
                    output.push_str(&format!("status-justify {}\n", app.status_justify));
                    output.push_str(&format!("window-status-format \"{}\"\n", app.window_status_format));
                    output.push_str(&format!("window-status-current-format \"{}\"\n", app.window_status_current_format));
                    if !app.window_status_style.is_empty() {
                        output.push_str(&format!("window-status-style \"{}\"\n", app.window_status_style));
                    }
                    if !app.window_status_current_style.is_empty() {
                        output.push_str(&format!("window-status-current-style \"{}\"\n", app.window_status_current_style));
                    }
                    if !app.window_status_activity_style.is_empty() {
                        output.push_str(&format!("window-status-activity-style \"{}\"\n", app.window_status_activity_style));
                    }
                    if !app.message_style.is_empty() {
                        output.push_str(&format!("message-style \"{}\"\n", app.message_style));
                    }
                    if !app.message_command_style.is_empty() {
                        output.push_str(&format!("message-command-style \"{}\"\n", app.message_command_style));
                    }
                    if !app.mode_style.is_empty() {
                        output.push_str(&format!("mode-style \"{}\"\n", app.mode_style));
                    }
                    // Include @user-options (used by plugins)
                    for (key, val) in &app.user_options {
                        output.push_str(&format!("{} \"{}\"\n", key, val));
                    }
                    // New options
                    output.push_str(&format!("main-pane-width {}\n", app.main_pane_width));
                    output.push_str(&format!("main-pane-height {}\n", app.main_pane_height));
                    output.push_str(&format!("status-left-length {}\n", app.status_left_length));
                    output.push_str(&format!("status-right-length {}\n", app.status_right_length));
                    output.push_str(&format!("window-size {}\n", app.window_size));
                    output.push_str(&format!("allow-passthrough {}\n", app.allow_passthrough));
                    output.push_str(&format!("set-clipboard {}\n", app.set_clipboard));
                    if !app.copy_command.is_empty() {
                        output.push_str(&format!("copy-command \"{}\"\n", app.copy_command));
                    }
                    output.push_str(&format!("allow-rename {}\n", if app.allow_rename { "on" } else { "off" }));
                    output.push_str(&format!("allow-set-title {}\n", if app.allow_set_title { "on" } else { "off" }));
                    output.push_str(&format!("bell-action {}\n", app.bell_action));
                    output.push_str(&format!("activity-action {}\n", app.activity_action));
                    output.push_str(&format!("silence-action {}\n", app.silence_action));
                    output.push_str(&format!("update-environment \"{}\"\n", app.update_environment.join(" ")));
                    if let Some(ref group) = app.session_group {
                        output.push_str(&format!("session-group \"{}\"\n", group));
                    }
                    for (alias, expansion) in &app.command_aliases {
                        output.push_str(&format!("command-alias \"{}={}\"\n", alias, expansion));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SourceFile(path) => {
                    // Reset binding state so config reload gets a clean slate.
                    // If the config has unbind-key -a, it will re-set the flag.
                    app.defaults_suppressed = false;
                    app.key_tables.clear();
                    crate::config::populate_default_bindings(&mut app);
                    // Use config helper for standard source-file behavior (-F support,
                    // nested parse context). Keep direct glob handling for wildcard sources.
                    let is_format_expand = path.starts_with("-F ") || path.starts_with("-F\t");
                    let path_for_glob = if is_format_expand { path[3..].trim() } else { &path };
                    if !is_format_expand && (path_for_glob.contains('*') || path_for_glob.contains('?')) {
                        let expanded = if path_for_glob.starts_with('~') {
                            let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                            path_for_glob.replacen('~', &home, 1)
                        } else {
                            path_for_glob.to_string()
                        };
                        if let Ok(entries) = glob::glob(&expanded) {
                            for entry in entries.flatten() {
                                if let Ok(contents) = std::fs::read_to_string(&entry) {
                                    parse_config_content(&mut app, &contents);
                                }
                            }
                        }
                    } else {
                        crate::config::source_file(&mut app, &path);
                    }
                    // A runtime source-file can record warnings (e.g. an
                    // unreadable path); flush them like the startup load does
                    // or they never reach config-warnings.log.
                    write_config_warnings_log(&app.config_warnings);
                    // source-file may change pane-border-status which
                    // affects pane content height (#288)
                    resize_all_panes(&mut app);
                    // Mark dirty so the client receives updated config
                    // (status bar, bindings, styles, etc.) on the next
                    // dump-state instead of getting an NC fast-path reply.
                    state_dirty = true;
                    meta_dirty = true;
                }
                CtrlReq::MoveWindow { src, dst, detach, kill, renumber, after, before, resp } => {
                    // tmux cmd-move-window.c. Both outcomes mark the state
                    // dirty: an attached client renders its window list from
                    // the frame the server pushes, and with neither flag set
                    // the status bar kept the pre-move list until some
                    // UNRELATED command happened to dirty the state (#601).
                    let outcome = move_window_request(
                        &mut app, src.as_deref(), dst.as_deref(),
                        detach, kill, renumber, after, before);
                    match outcome {
                        Ok(()) => {
                            resize_all_panes(&mut app);
                            state_dirty = true;
                            meta_dirty = true;
                            let _ = resp.send(Ok(()));
                        }
                        Err(msg) => {
                            // Persistent (TUI) clients have no reply stream for
                            // this path; surface the error in the status bar
                            // like kill-window does. That message is only ever
                            // drawn if the frame is pushed, so it needs the
                            // dirty flag as much as the success path does.
                            app.status_message = Some((format!("move-window: {}", msg), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                    }
                }
                CtrlReq::SwapWindow { src, dst, detach, resp } => {
                    // The two windows trade Vec positions while `window_indices`
                    // stays put, so they exchange display numbers (tmux swap-window
                    // swaps the two winlinks' window pointers, leaving the indices
                    // and therefore the current window NUMBER alone).
                    // #559: a spec that resolves to no window is an error the
                    // caller must see (tmux: "can't find window: N", exit 1).
                    let resolved = (|| -> Result<(usize, usize), String> {
                        let spos = match src.as_deref() {
                            Some(s) => app.resolve_window_spec(s, false)?.pos()
                                .ok_or_else(|| format!("can't find window: {}", s))?,
                            None => app.active_idx,
                        };
                        let tpos = app.resolve_window_spec(&dst, false)?.pos()
                            .ok_or_else(|| format!("can't find window: {}", dst))?;
                        Ok((spos, tpos))
                    })();
                    match resolved {
                        Err(msg) => {
                            app.status_message = Some((format!("swap-window: {}", msg), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                        Ok((spos, tpos)) => {
                            if spos != tpos {
                                app.windows.swap(spos, tpos);
                                // tmux only re-selects WITH -d, and it selects the
                                // DESTINATION index (cmd-swap-window.c), which now
                                // holds the window that used to be the source.
                                if detach && tpos < app.windows.len() {
                                    let prev = app.active_idx;
                                    if prev != tpos {
                                        app.last_window_idx = prev;
                                        app.active_idx = tpos;
                                    }
                                }
                                resize_all_panes(&mut app);
                            }
                            // The window LIST changed even though no pane
                            // content did, so the dump-state fast path would
                            // answer "NC" and the attached client would keep
                            // rendering the old order (#601).
                            state_dirty = true;
                            meta_dirty = true;
                            let _ = resp.send(Ok(()));
                        }
                    }
                }
                CtrlReq::LinkWindow(src_idx_opt, dst_idx_opt) => {
                    // link-window: within a single session, create a linked window
                    // referencing the source window. Since PTY handles can't be shared
                    // across windows, this spawns a new shell and marks it as linked.
                    let src = src_idx_opt.unwrap_or(app.active_idx);
                    if src < app.windows.len() {
                        let src_id = app.windows[src].id;
                        let src_name = app.windows[src].name.clone();
                        let pty_system = portable_pty::native_pty_system();
                        match crate::pane::create_window(&*pty_system, &mut app, None, None, false) {
                            Ok(()) => {
                                let new_idx = app.windows.len() - 1;
                                app.windows[new_idx].linked_from = Some(src_id);
                                app.windows[new_idx].name = src_name;
                                if let Some(dst) = dst_idx_opt {
                                    if app.window_indices_valid() {
                                        // dst is a display index; place the newly
                                        // created (active) linked window there.
                                        app.move_active_window_to_index(dst);
                                    } else if dst < new_idx {
                                        let win = app.windows.remove(new_idx);
                                        app.windows.insert(dst, win);
                                        if app.active_idx > dst && app.active_idx <= new_idx {
                                            app.active_idx = app.active_idx.saturating_sub(1);
                                        }
                                    }
                                }
                                resize_all_panes(&mut app);
                                meta_dirty = true;
                                hook_event = Some("window-linked");
                            }
                            Err(_e) => {
                                app.status_message = Some(("link-window: failed to create linked window".to_string(), std::time::Instant::now(), None));
                            }
                        }
                    } else {
                        app.status_message = Some(("link-window: source window not found".to_string(), std::time::Instant::now(), None));
                    }
                    state_dirty = true;
                }
                CtrlReq::UnlinkWindow => {
                    if app.windows.len() > 1 {
                        let removed_pos = app.active_idx;
                        let mut win = app.windows.remove(removed_pos);
                        kill_all_children(&mut win.root);
                        app.on_window_removed(removed_pos);
                        if app.active_idx >= app.windows.len() {
                            app.active_idx = app.windows.len() - 1;
                        }
                        resize_all_panes(&mut app);
                        meta_dirty = true;
                        hook_event = Some("window-unlinked");
                    }
                }
                CtrlReq::SetSessionGroup(group_name) => {
                    app.session_group = Some(group_name);
                    state_dirty = true;
                }
                CtrlReq::FindWindow(resp, pattern) => {
                    let mut output = String::new();
                    for (i, win) in app.windows.iter().enumerate() {
                        if win.name.contains(&pattern) {
                            output.push_str(&format!("{}: {} []\n", app.win_display_index(i), win.name));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::PipePane(cmd, stdin, stdout, toggle, mut reply) => {
                    // The `-t` target (if any) was temp-focused by the connection
                    // layer before this request ran, so the active pane here IS the
                    // requested target pane (issue #440 defect 2). The pipe binds to
                    // this concrete pane_id and keeps receiving that pane's output
                    // even after focus moves elsewhere.
                    let win = &app.windows[app.active_idx];
                    let pane_id = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
                    // A recorded pipe whose child has already exited is NOT an
                    // existing pipe. The reader thread drops the writer when a
                    // tee write fails, but nothing cleared app.pipe_panes, so a
                    // sink that exits on its own left the pane marked as piped
                    // forever and `-o` toggled OFF against a dead process
                    // instead of re-arming (issue #564). Reap the exited entries
                    // before the toggle decision reads them.
                    let mut reaped: Vec<usize> = Vec::new();
                    app.pipe_panes.retain_mut(|p| match p.process.as_mut() {
                        Some(child) => {
                            let alive = matches!(child.try_wait(), Ok(None));
                            if !alive { reaped.push(p.pane_id); }
                            alive
                        }
                        // No child: a direct file sink. Its liveness IS the
                        // registered writer — when the reader thread dropped
                        // it on a failed write, keeping the entry would make
                        // `-o` toggle OFF a dead sink again (the exact #564
                        // shape). A pane_id match from another writer kind
                        // (cross-session tunnel) errs on keeping the entry,
                        // which is the pre-existing behavior.
                        None => crate::types::PIPE_WRITERS
                            .lock()
                            .map(|w| w.iter().any(|(id, _)| *id == p.pane_id))
                            .unwrap_or(true),
                    });

                    // Drop any writer this pane's reader thread was teeing to
                    // (issue #440). Dropping the ChildStdin closes the pipe so the
                    // child sees EOF; the count gate is kept in sync so idle panes
                    // pay nothing.
                    let unregister_writer = |pid: usize| {
                        if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                            let before = writers.len();
                            writers.retain(|(id, _)| *id != pid);
                            let removed = before - writers.len();
                            if removed > 0 {
                                crate::types::PIPE_PANE_COUNT
                                    .fetch_sub(removed, std::sync::atomic::Ordering::Relaxed);
                            }
                        }
                    };

                    // Drop the writers belonging to the sinks reaped above. The
                    // reader thread already drops a writer when a tee write
                    // fails, but a sink that exited without the pane emitting
                    // anything since leaves one registered.
                    for pid in reaped { unregister_writer(pid); }
                    let has_existing = app.pipe_panes.iter().any(|p| p.pane_id == pane_id);

                    // Non-empty only when the sink could not be started; sent
                    // to the reply channel at the end of the arm so the
                    // one-shot CLI can exit non-zero (see CtrlReq::PipePane).
                    let mut outcome = String::new();

                    if cmd.is_empty() {
                        // No command: close any existing pipe on this pane
                        if let Some(idx) = app.pipe_panes.iter().position(|p| p.pane_id == pane_id) {
                            unregister_writer(pane_id);
                            if let Some(ref mut proc) = app.pipe_panes[idx].process {
                                let _ = proc.kill();
                            }
                            app.pipe_panes.remove(idx);
                        }
                    } else if toggle && has_existing {
                        // -o flag with existing pipe: close it (toggle off), don't start new
                        if let Some(idx) = app.pipe_panes.iter().position(|p| p.pane_id == pane_id) {
                            unregister_writer(pane_id);
                            if let Some(ref mut proc) = app.pipe_panes[idx].process {
                                let _ = proc.kill();
                            }
                            app.pipe_panes.remove(idx);
                        }
                    } else {
                        // Close any existing pipe first (replace)
                        if let Some(idx) = app.pipe_panes.iter().position(|p| p.pane_id == pane_id) {
                            unregister_writer(pane_id);
                            if let Some(ref mut proc) = app.pipe_panes[idx].process {
                                let _ = proc.kill();
                            }
                            app.pipe_panes.remove(idx);
                        }
                        // Direct file sink: the canonical tmux logging idiom
                        // `cat > file` / `cat >> file` cannot work through the
                        // Windows sink shell — PowerShell's `cat` is the
                        // Get-Content alias, never reads stdin, and exits at
                        // once, so the redirect left a 0-byte file with rc 0.
                        // Service the idiom in-process instead: the reader
                        // thread tees the pane's raw ConPTY bytes straight
                        // into the file (byte-faithful, no shell, no child to
                        // fail silently). Output direction only; `-I` and
                        // every other command shape keep the shell sink.
                        let file_sink = if stdout && !stdin {
                            crate::util::parse_cat_file_sink(&cmd)
                        } else {
                            None
                        };
                        if let Some((path, append)) = file_sink {
                            // The open runs on the server's single event loop,
                            // so it must never be a call that can stall or hit
                            // a device. A local CreateFile is microseconds;
                            // UNC/remote-drive resolution against an
                            // unreachable host blocks for tens of seconds and
                            // would freeze every pane, and a DOS device name
                            // opens the DEVICE (`cat > CON` would tee VT bytes
                            // into the server's own console). All of those are
                            // refused loudly; a sink command that reads stdin
                            // runs as a child process and stalls only itself.
                            // Known residual: a local directory junction that
                            // resolves to an unreachable target can still
                            // stall the open.
                            if let Some(reason) = crate::util::refuse_file_sink_path(&path) {
                                outcome = format!("ERROR: pipe-pane: {}: {}", reason, path);
                            } else if file_sink_drive_is_remote(&path) {
                                outcome = format!(
                                    "ERROR: pipe-pane: remote drive not supported for the direct file sink (use a sink command that reads stdin): {}",
                                    path
                                );
                            } else {
                            let mut opts = std::fs::OpenOptions::new();
                            opts.create(true).write(true);
                            if append { opts.append(true); } else { opts.truncate(true); }
                            match opts.open(&path) {
                                Ok(file) => {
                                    if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                                        writers.push((pane_id, Box::new(file)));
                                        crate::types::PIPE_PANE_COUNT
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        app.pipe_panes.push(PipePaneState {
                                            pane_id,
                                            process: None,
                                            stdin,
                                            stdout,
                                        });
                                    } else {
                                        // Poisoned registry: recording the pane as
                                        // piped with no writer would be exactly the
                                        // phantom pipe this change removes.
                                        outcome =
                                            "ERROR: pipe-pane: writer registry unavailable".to_string();
                                    }
                                }
                                Err(e) => {
                                    outcome = format!("ERROR: pipe-pane: can't open {}: {}", path, e);
                                }
                            }
                            }
                            // The reply channel only reaches the one-shot CLI;
                            // a pipe-pane issued from a key binding or the
                            // command prompt discards it, so mirror the
                            // failure on the status bar (same surface the
                            // shell-sink spawn failure below uses).
                            if !outcome.is_empty() {
                                app.status_message = Some((
                                    outcome.trim_start_matches("ERROR: ").to_string(),
                                    std::time::Instant::now(),
                                    None,
                                ));
                            }
                        } else {
                        // Start new pipe. Answer "accepted" BEFORE spawning:
                        // CreateProcess can stall for seconds on a cold
                        // antivirus scan of the shell image, and holding the
                        // reply hostage to it turns a successful pipe-pane
                        // into a client-side timeout error. A spawn failure
                        // is still not recorded (no phantom pipe) and is
                        // surfaced on the status bar below.
                        if let Some(tx) = reply.take() {
                            let _ = tx.send(String::new());
                        }
                        let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
                        let spawn_result = {
                            let mut c = std::process::Command::new(&shell_prog);
                            for a in &shell_args { c.arg(a); }
                            c.arg(&cmd);
                            c.stdin(if stdout { std::process::Stdio::piped() } else { std::process::Stdio::null() });
                            c.stdout(if stdin { std::process::Stdio::piped() } else { std::process::Stdio::null() });
                            c.stderr(std::process::Stdio::null());
                            { use crate::platform::HideWindowCommandExt; c.hide_window(); }
                            c.spawn()
                        };
                        match spawn_result {
                            Ok(mut child) => {
                                // Issue #440: hand the child's stdin to this pane's reader
                                // thread so pane output is actually fed to the pipe command.
                                // Without this the child blocked on an empty pipe forever and
                                // the sink stayed 0 bytes. Only the output direction (`-O` /
                                // default) registers a writer; `-I` (child stdout -> pane
                                // input) is unchanged.
                                if stdout {
                                    if let Some(stdin_handle) = child.stdin.take() {
                                        if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                                            writers.push((pane_id, Box::new(stdin_handle)));
                                            crate::types::PIPE_PANE_COUNT
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                    }
                                }

                                app.pipe_panes.push(PipePaneState {
                                    pane_id,
                                    process: Some(child),
                                    stdin,
                                    stdout,
                                });
                            }
                            Err(e) => {
                                // A sink that never started must not be recorded:
                                // the dead entry made a later `-o` toggle OFF a
                                // nonexistent pipe and hid the failure behind rc 0
                                // (`spawn().ok()` swallowed the error). The reply
                                // was already sent, so report where run-shell -b
                                // reports its spawn failures: the status bar.
                                app.status_message = Some((
                                    format!("pipe-pane: can't spawn sink: {}", e),
                                    std::time::Instant::now(),
                                    None,
                                ));
                            }
                        }
                        }
                    }

                    if let Some(reply) = reply {
                        let _ = reply.send(outcome);
                    }
                }
                CtrlReq::SelectLayout(layout) => {
                    unzoom_if_zoomed(&mut app);
                    apply_layout(&mut app, &layout);
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::NextLayout => {
                    unzoom_if_zoomed(&mut app);
                    cycle_layout(&mut app);
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::ListClients(resp) => {
                    // Emit exactly one row per real registered client. When the
                    // registry is empty (a detached session, or every client has
                    // been reaped) the output is empty, matching tmux, which
                    // prints nothing for a session with no attached clients.
                    //
                    // Issue #434: an old "backward compat" branch synthesized a
                    // phantom `/dev/pts/0` row from session geometry whenever the
                    // registry was empty. That fabricated a ghost client for a
                    // detached session that had never been attached, and left a
                    // stale row behind after a clean detach reaped the last real
                    // client, even though `#{session_attached}` correctly read 0.
                    // The registry is the single source of truth here, exactly as
                    // it already is for the ListClientsFormat (-F) path below.
                    let mut output = String::new();
                    let mut clients: Vec<&crate::types::ClientInfo> = app.client_registry.values().collect();
                    clients.sort_by_key(|c| c.id);
                    for ci in &clients {
                        let activity_secs = ci.last_activity.elapsed().as_secs();
                        let kind = if ci.is_control { " (control mode)" } else { "" };
                        output.push_str(&format!("{}: {}: {} [{}x{}] (utf8){} [activity={}s ago]\n",
                            ci.tty_name,
                            app.session_name,
                            app.windows[app.active_idx].name,
                            ci.width, ci.height,
                            kind,
                            activity_secs,
                        ));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ListClientsFormat(resp, fmt) => {
                    let mut output = String::new();
                    let mut clients: Vec<&crate::types::ClientInfo> = app.client_registry.values().collect();
                    clients.sort_by_key(|c| c.id);
                    for ci in &clients {
                        let activity_secs = ci.last_activity.elapsed().as_secs();
                        let line = fmt
                            .replace("#{client_name}", &ci.tty_name)
                            .replace("#{client_tty}", &ci.tty_name)
                            .replace("#{client_width}", &ci.width.to_string())
                            .replace("#{client_height}", &ci.height.to_string())
                            .replace("#{client_activity}", &activity_secs.to_string())
                            .replace("#{client_session}", &app.session_name)
                            .replace("#{session_name}", &app.session_name)
                            .replace("#{client_control_mode}", if ci.is_control { "1" } else { "0" });
                        output.push_str(&line);
                        output.push('\n');
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ForceDetachClient(target_cid) => {
                    // Force-detach a specific client by shutting down its TCP stream
                    app.client_sizes.remove(&target_cid);
                    let was_present = app.client_registry.remove(&target_cid).is_some();
                    if was_present {
                        app.attached_clients = app.attached_clients.saturating_sub(1);
                    }
                    if app.latest_client_id == Some(target_cid) {
                        app.latest_client_id = app.client_registry.keys().max().copied();
                    }
                    // Send a clean DETACH directive first so the client exits instead
                    // of treating the stream drop as a transient disconnect and
                    // reconnecting. Only wait if the directive was actually queued; a
                    // failed send means the client is already gone, so blocking the
                    // server loop would buy nothing.
                    if crate::types::send_directive_to_client(target_cid, "DETACH") {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    // Shut down the TCP stream to force disconnect
                    crate::types::shutdown_client_stream(target_cid);
                    // Recompute dynamic windows from the remaining clients.
                    if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                        resize_all_panes(&mut app);
                    }
                    // Fire detach notification
                    control::emit_notification(&app, crate::types::ControlNotification::ClientDetached {
                        client: format!("/dev/pts/{}", target_cid),
                    });
                    hook_event = Some("client-detached");
                    if app.attached_clients == 0 && app.destroy_unattached {
                        let regpath = crate::paths::port_file(&app.port_file_base());
                        let keypath = crate::paths::key_file(&app.port_file_base());
                        let _ = std::fs::remove_file(&regpath);
                        let _ = std::fs::remove_file(&keypath);
                        crate::session::remove_session_id_file(&app.port_file_base());
                        crate::types::shutdown_persistent_streams();
                        tree::kill_all_children_batch(&mut app.windows);
                        if let Some(mut wp) = app.warm_pane.take() {
                            wp.child.kill().ok();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        std::process::exit(0);
                    }
                }
                CtrlReq::ForceDetachClientByTty(tty, kill_parent) => {
                    // Look up the client by tty_name (e.g. "/dev/pts/2") and force-detach.
                    let target_cid: Option<u64> = app.client_registry.iter()
                        .find(|(_, ci)| ci.tty_name == tty)
                        .map(|(cid, _)| *cid);
                    if let Some(cid) = target_cid {
                        // Send a clean directive first so the client exits instead of
                        // reconnecting on the stream drop. -P also kills the parent.
                        // Only wait if the directive was actually queued; a failed send
                        // means the client is already gone.
                        let directive = if kill_parent { "DETACH-KILL-PARENT" } else { "DETACH" };
                        if crate::types::send_directive_to_client(cid, directive) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        app.client_sizes.remove(&cid);
                        let was_present = app.client_registry.remove(&cid).is_some();
                        if was_present {
                            app.attached_clients = app.attached_clients.saturating_sub(1);
                        }
                        if app.latest_client_id == Some(cid) {
                            app.latest_client_id = app.client_registry.keys().max().copied();
                        }
                        crate::types::shutdown_client_stream(cid);
                        if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        control::emit_notification(&app, crate::types::ControlNotification::ClientDetached {
                            client: tty.clone(),
                        });
                        hook_event = Some("client-detached");
                    }
                }
                CtrlReq::DetachAllOtherClients(except_cid, kill_parent) => {
                    // Detach all clients except the one with except_cid.
                    // Pass u64::MAX from CLI one-shot path to mean "no current client".
                    let targets: Vec<(u64, String)> = app.client_registry.iter()
                        .filter(|(cid, _)| **cid != except_cid)
                        .map(|(cid, ci)| (*cid, ci.tty_name.clone()))
                        .collect();
                    // Send a clean directive to each target first so they exit instead
                    // of reconnecting on the stream drop. -P also kills the parent.
                    let directive = if kill_parent { "DETACH-KILL-PARENT" } else { "DETACH" };
                    let mut any_sent = false;
                    for (cid, _) in &targets {
                        any_sent |= crate::types::send_directive_to_client(*cid, directive);
                    }
                    // Only wait if at least one directive was actually queued.
                    if any_sent {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    for (cid, tty) in &targets {
                        app.client_sizes.remove(cid);
                        if app.client_registry.remove(cid).is_some() {
                            app.attached_clients = app.attached_clients.saturating_sub(1);
                        }
                        crate::types::shutdown_client_stream(*cid);
                        control::emit_notification(&app, crate::types::ControlNotification::ClientDetached {
                            client: tty.clone(),
                        });
                    }
                    if !targets.is_empty() {
                        if app.latest_client_id.map_or(false, |c| !app.client_registry.contains_key(&c)) {
                            app.latest_client_id = app.client_registry.keys().max().copied();
                        }
                        if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        hook_event = Some("client-detached");
                    }
                    if app.attached_clients == 0 && app.destroy_unattached {
                        let regpath = crate::paths::port_file(&app.port_file_base());
                        let keypath = crate::paths::key_file(&app.port_file_base());
                        let _ = std::fs::remove_file(&regpath);
                        let _ = std::fs::remove_file(&keypath);
                        crate::session::remove_session_id_file(&app.port_file_base());
                        crate::types::shutdown_persistent_streams();
                        tree::kill_all_children_batch(&mut app.windows);
                        if let Some(mut wp) = app.warm_pane.take() {
                            wp.child.kill().ok();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        std::process::exit(0);
                    }
                }
                CtrlReq::DetachAllClients(kill_parent) => {
                    // Detach every attached client of this session.
                    let targets: Vec<(u64, String)> = app.client_registry.iter()
                        .map(|(cid, ci)| (*cid, ci.tty_name.clone()))
                        .collect();
                    // Send a clean directive to each client first so they exit instead
                    // of reconnecting on the stream drop. -P also kills the parent.
                    let directive = if kill_parent { "DETACH-KILL-PARENT" } else { "DETACH" };
                    let mut any_sent = false;
                    for (cid, _) in &targets {
                        any_sent |= crate::types::send_directive_to_client(*cid, directive);
                    }
                    // Only wait if at least one directive was actually queued.
                    if any_sent {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    for (cid, tty) in &targets {
                        app.client_sizes.remove(cid);
                        if app.client_registry.remove(cid).is_some() {
                            app.attached_clients = app.attached_clients.saturating_sub(1);
                        }
                        crate::types::shutdown_client_stream(*cid);
                        control::emit_notification(&app, crate::types::ControlNotification::ClientDetached {
                            client: tty.clone(),
                        });
                    }
                    if !targets.is_empty() {
                        app.latest_client_id = None;
                        app.client_prefix_active = false;
                        if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        hook_event = Some("client-detached");
                    }
                    if app.attached_clients == 0 && app.destroy_unattached {
                        let regpath = crate::paths::port_file(&app.port_file_base());
                        let keypath = crate::paths::key_file(&app.port_file_base());
                        let _ = std::fs::remove_file(&regpath);
                        let _ = std::fs::remove_file(&keypath);
                        crate::session::remove_session_id_file(&app.port_file_base());
                        crate::types::shutdown_persistent_streams();
                        tree::kill_all_children_batch(&mut app.windows);
                        if let Some(mut wp) = app.warm_pane.take() {
                            wp.child.kill().ok();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        std::process::exit(0);
                    }
                }
                CtrlReq::SetClientLastSession(cid, prev) => {
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.last_session = Some(prev);
                    }
                }
                CtrlReq::SwitchClient(target, flag, resp_tx) => {
                    // Resolve the target session name based on the flag
                    let current = app.port_file_base();
                    let all_sessions = crate::session::list_session_names();
                    let resolved = match flag {
                        't' => {
                            // Direct target: validate it exists
                            if target.is_empty() {
                                None
                            } else if all_sessions.contains(&target) {
                                Some(target.clone())
                            } else {
                                // Try partial match (prefix)
                                all_sessions.iter().find(|s| s.starts_with(&target)).cloned()
                            }
                        }
                        'n' => {
                            // Next session (alphabetically after current)
                            let pos = all_sessions.iter().position(|s| s == &current);
                            match pos {
                                Some(i) if i + 1 < all_sessions.len() => Some(all_sessions[i + 1].clone()),
                                Some(_) => all_sessions.first().cloned(), // wrap around
                                None => all_sessions.first().cloned(),
                            }
                        }
                        'p' => {
                            // Previous session (alphabetically before current)
                            let pos = all_sessions.iter().position(|s| s == &current);
                            match pos {
                                Some(0) => all_sessions.last().cloned(), // wrap around
                                Some(i) => Some(all_sessions[i - 1].clone()),
                                None => all_sessions.last().cloned(),
                            }
                        }
                        'l' => {
                            // Last session, taken from THIS client's own history.
                            //
                            // This used to read the data-dir-global
                            // `last_session` file, which every attach overwrites
                            // with the session being ENTERED. The `!= current`
                            // filter then guaranteed that the only value able to
                            // survive was one written by a DIFFERENT client, so
                            // `-l` relocated this client into a session it had
                            // never visited, chosen by whoever attached most
                            // recently anywhere on the machine (issue #566).
                            // The file keeps its real job as the routing hint
                            // consumed by resolve_last_session_name_ns.
                            //
                            // No recorded previous session is reported as "no
                            // last session" rather than borrowed from elsewhere,
                            // which is what tmux does for a client that has not
                            // moved.
                            app.latest_client_id
                                .and_then(|cid| app.client_registry.get(&cid))
                                .and_then(|info| info.last_session.clone())
                                .filter(|s| !s.is_empty() && s != &current && all_sessions.contains(s))
                        }
                        _ => None,
                    };
                    // #566: every branch now reports its outcome on the optional
                    // channel as well as on the TUI status line, because a
                    // one-shot CLI caller never sees a status line and so could
                    // not tell a completed switch from a silent no-op.
                    let mut reply = "OK".to_string();
                    match resolved {
                        Some(ref sess) if sess != &current => {
                            // Signal the attached client to switch by sending a directive
                            if let Some(cid) = app.latest_client_id {
                                crate::types::send_directive_to_client(cid, &format!("SWITCH {}", sess));
                            } else {
                                // No specific client ID, send to all attached clients
                                crate::types::send_directive_to_all_clients(&format!("SWITCH {}", sess));
                            }
                        }
                        Some(_) => {
                            // Target is the same as current session
                            app.status_message = Some(("switch-client: already on that session".to_string(), std::time::Instant::now(), None));
                            state_dirty = true;
                        }
                        None => {
                            let msg = if flag == 't' && !target.is_empty() {
                                format!("switch-client: session not found: {}", target)
                            } else if flag == 'l' {
                                "switch-client: no last session".to_string()
                            } else if all_sessions.len() <= 1 {
                                "switch-client: only one session available".to_string()
                            } else {
                                "switch-client: no target session".to_string()
                            };
                            reply = format!("ERROR {}", msg);
                            app.status_message = Some((msg, std::time::Instant::now(), None));
                            state_dirty = true;
                        }
                    }
                    if let Some(rtx) = resp_tx {
                        let _ = rtx.send(reply);
                    }
                }
                CtrlReq::SwitchClientTarget(raw, resp_tx) => {
                    // #483: `switch-client -t <target>` with a full
                    // session:window.pane / @window / %pane spec. Switch the
                    // client's session AND select the addressed window/pane, and
                    // validate the target exists so the CLI can exit non-zero.
                    let now = std::time::Instant::now();
                    let current = app.port_file_base();
                    let all_sessions = crate::session::list_session_names();
                    let pt = crate::cli::parse_target(&raw);
                    let sess_req = pt.session.clone().filter(|s| !s.is_empty());

                    // Resolve destination session: explicit prefix, else current.
                    let target_session = match &sess_req {
                        Some(s) if all_sessions.contains(s) => Some(s.clone()),
                        Some(s) => all_sessions.iter().find(|x| x.starts_with(s)).cloned(),
                        None => Some(current.clone()),
                    };

                    let outcome: Result<bool, String> = match target_session {
                        None => Err(format!("can't find session: {}", sess_req.clone().unwrap_or_default())),
                        Some(dest) => {
                            let same_session = dest == current;
                            if !same_session {
                                // Cross-session (#483): if the target also names
                                // a window/pane, pre-select it on the DESTINATION
                                // server (one server per session) so the client
                                // lands there after it re-attaches, then signal
                                // the client to re-attach to dest.
                                //
                                // #555: the window/pane component is resolved on
                                // the destination BEFORE anything is signalled.
                                // The old shape forwarded the pre-select
                                // fire-and-forget (FocusWindow silently no-ops on
                                // a miss) and emitted SWITCH unconditionally, so
                                // an unresolvable cross-session component exited
                                // 0 and switched anyway, while the identical
                                // same-session target correctly errored — a
                                // driver's exit code was right exactly half the
                                // time. resolve_session also replaces the
                                // hand-rolled port-file/key reads.
                                match crate::cross_session::resolve_session(&dest) {
                                    Err(_) => Err(format!("can't find session: {}", dest)),
                                    Ok((port, key)) => {
                                        match crate::cross_session::validate_switch_target(port, &key, &pt) {
                                            Err(e) => Err(e),
                                            Ok(()) => {
                                                if pt.pane.is_some() || pt.window.is_some() || pt.window_name.is_some() {
                                                    let sel = if pt.pane.is_some() { "select-pane" } else { "select-window" };
                                                    let msg = format!("TARGET {}\n{}\n", raw, sel);
                                                    let _ = crate::session::send_control_to_port(port, &msg, &key);
                                                }
                                                if let Some(cid) = app.latest_client_id {
                                                    crate::types::send_directive_to_client(cid, &format!("SWITCH {}", dest));
                                                } else {
                                                    crate::types::send_directive_to_all_clients(&format!("SWITCH {}", dest));
                                                }
                                                Ok(false)
                                            }
                                        }
                                    }
                                }
                            } else {
                            // Same-session window/pane selection acts on THIS
                            // server directly.
                                if let Some(pid) = pt.pane {
                                    if pt.pane_is_id {
                                        if crate::tree::find_pane_by_id_global(&app, pid).is_some() {
                                            switch_with_copy_save(&mut app, |app| { crate::tree::focus_pane_by_id(app, pid); });
                                            unzoom_if_zoomed(&mut app);
                                            resize_all_panes(&mut app);
                                            Ok(true)
                                        } else {
                                            Err(format!("can't find pane: %{}", pid))
                                        }
                                    } else {
                                        switch_with_copy_save(&mut app, |app| { crate::tree::focus_pane_by_index(app, pid); });
                                        unzoom_if_zoomed(&mut app);
                                        Ok(true)
                                    }
                                } else if let Some(w) = pt.window {
                                    let internal = if pt.window_is_id {
                                        app.windows.iter().position(|x| x.id == w)
                                    } else {
                                        app.win_pos(w)
                                    };
                                    match internal {
                                        Some(i) => {
                                            if i != app.active_idx {
                                                switch_with_copy_save(&mut app, |app| {
                                                    app.last_window_idx = app.active_idx;
                                                    app.active_idx = i;
                                                });
                                                if let Some(win) = app.windows.get_mut(i) {
                                                    win.activity_flag = false; win.bell_flag = false; win.silence_flag = false;
                                                }
                                                resize_all_panes(&mut app);
                                            }
                                            Ok(true)
                                        }
                                        None => Err(format!("can't find window: {}", raw)),
                                    }
                                } else if let Some(ref wname) = pt.window_name {
                                    match app.windows.iter().position(|x| x.name == *wname) {
                                        Some(i) => {
                                            if i != app.active_idx {
                                                switch_with_copy_save(&mut app, |app| {
                                                    app.last_window_idx = app.active_idx;
                                                    app.active_idx = i;
                                                });
                                                resize_all_panes(&mut app);
                                            }
                                            Ok(true)
                                        }
                                        None => Err(format!("can't find window: {}", wname)),
                                    }
                                } else {
                                    // Session-only target equal to the current session: no-op.
                                    Ok(false)
                                }
                            }
                        }
                    };

                    match outcome {
                        Ok(selected) => {
                            meta_dirty = true;
                            state_dirty = true;
                            if selected { hook_event = Some("after-select-window"); }
                            let _ = resp_tx.send("OK".to_string());
                        }
                        Err(msg) => {
                            app.status_message = Some((format!("switch-client: {}", msg), now, None));
                            state_dirty = true;
                            let _ = resp_tx.send(format!("ERROR {}", msg));
                        }
                    }
                }
                CtrlReq::SwitchClientTable(table) => {
                    app.current_key_table = Some(table);
                    state_dirty = true;
                }
                CtrlReq::ListCommands(resp) => {
                    let cmds = TMUX_COMMANDS.join("\n");
                    let _ = resp.send(cmds);
                }
                CtrlReq::LockClient => {
                    app.status_message = Some(("lock: not available on Windows".to_string(), std::time::Instant::now(), None));
                    state_dirty = true;
                }
                CtrlReq::RefreshClient => { state_dirty = true; meta_dirty = true; }
                CtrlReq::SuspendClient => {
                    app.status_message = Some(("suspend: not available on Windows".to_string(), std::time::Instant::now(), None));
                    state_dirty = true;
                }
                CtrlReq::CopyModePageUp => {
                    enter_copy_mode(&mut app);
                    move_copy_cursor(&mut app, 0, -20);
                }
                CtrlReq::ClearHistory => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        if let Ok(mut parser) = p.term.lock() {
                            *parser = vt100::Parser::new(p.last_rows, p.last_cols, app.history_limit);
                        }
                    }
                }
                CtrlReq::SaveBuffer(path) => {
                    if let Some(content) = app.paste_buffers.first() {
                        let _ = std::fs::write(&path, content);
                    }
                }
                CtrlReq::LoadBuffer(path) => {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        app.paste_buffers.insert(0, content);
                        if app.paste_buffers.len() > 10 {
                            app.paste_buffers.pop();
                        }
                    }
                }
                CtrlReq::SetEnvironment(key, value) => {
                    app.environment.insert(key.clone(), value.clone());
                    env::set_var(&key, &value);
                    // Env vars affect the child shell's process state,
                    // which can't be patched in place — must respawn.
                    // Centralised through warm_pane_sync (#137 / #271).
                    let sync = crate::warm_pane_sync::for_env_change();
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                }
                CtrlReq::UnsetEnvironment(key) => {
                    app.environment.remove(&key);
                    env::remove_var(&key);
                    let sync = crate::warm_pane_sync::for_env_change();
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                }
                CtrlReq::ShowEnvironment(resp) => {
                    let mut output = String::new();
                    // Show psmux/tmux-specific environment vars
                    for (key, value) in &app.environment {
                        output.push_str(&format!("{}={}\n", key, value));
                    }
                    // Also show inherited PSMUX_/TMUX_ vars from process env
                    for (key, value) in env::vars() {
                        if (key.starts_with("PSMUX") || key.starts_with("TMUX")) && !app.environment.contains_key(&key) {
                            output.push_str(&format!("{}={}\n", key, value));
                        }
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::SetHook(hook, cmd) => {
                    // Replace (not append) to match tmux semantics – prevents
                    // duplicate hooks on config reload (issue #133).
                    app.hooks.insert(hook, vec![cmd]);
                }
                CtrlReq::AppendHook(hook, cmd) => {
                    // -a/-ga: append to existing hook list so multiple
                    // plugins can register separate handlers (tmux semantics).
                    // Skip an identical command that is already registered: a
                    // config re-sourced N times (e.g. a plugin panel firing
                    // "Configuration reloaded" repeatedly) would otherwise
                    // accumulate N copies of the same handler and fire all of
                    // them every tick, spawning a runaway of processes for a
                    // status-interval run-shell hook (issue #459). This mirrors
                    // the replace path's dedup guard for issue #133.
                    let entry = app.hooks.entry(hook).or_insert_with(Vec::new);
                    if !entry.contains(&cmd) {
                        entry.push(cmd);
                    }
                }
                CtrlReq::ShowHooks(resp) => {
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
                    let _ = resp.send(output);
                }
                CtrlReq::RemoveHook(hook) => {
                    app.hooks.remove(&hook);
                }
                CtrlReq::KillServer => {
                    // Notify control clients that the server is going away,
                    // matching tmux's "%exit" wire notification before close.
                    // Flushes through the writer thread so iTerm2 sees a
                    // proper EOF-with-reason instead of a raw TCP RST.
                    if !app.control_clients.is_empty() {
                        control::emit_notification(
                            &app,
                            crate::types::ControlNotification::Exit {
                                reason: Some("server exited".to_string()),
                            },
                        );
                        // Brief drain window so writer threads can flush
                        // %exit + ST before the process exits.
                        std::thread::sleep(std::time::Duration::from_millis(80));
                    }
                    // Remove port/key files FIRST so clients see the session
                    // as gone immediately, then kill processes.
                    let regpath = crate::paths::port_file(&app.port_file_base());
                    let keypath = crate::paths::key_file(&app.port_file_base());
                    let _ = std::fs::remove_file(&regpath);
                    let _ = std::fs::remove_file(&keypath);
                    crate::types::send_directive_to_all_clients("DETACH");
                    std::thread::sleep(Duration::from_millis(50));
                    crate::types::shutdown_persistent_streams();
                    // Kill all child processes using a single process snapshot
                    tree::kill_all_children_batch(&mut app.windows);
                    // Kill warm pane's child (process::exit skips Drop)
                    if let Some(mut wp) = app.warm_pane.take() { wp.child.kill().ok(); }
                    // TerminateProcess is synchronous on Windows — processes
                    // are already dead.  Minimal delay for OS handle cleanup.
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    std::process::exit(0);
                }
                CtrlReq::WaitFor(channel, op) => {
                    match op {
                        WaitForOp::Lock => {
                            let entry = app.wait_channels.entry(channel).or_insert_with(|| WaitChannel {
                                locked: false,
                                waiters: Vec::new(),
                            });
                            entry.locked = true;
                        }
                        WaitForOp::Unlock => {
                            if let Some(ch) = app.wait_channels.get_mut(&channel) {
                                ch.locked = false;
                                for waiter in ch.waiters.drain(..) {
                                    let _ = waiter.send(());
                                }
                            }
                        }
                        WaitForOp::Signal => {
                            if let Some(ch) = app.wait_channels.get_mut(&channel) {
                                for waiter in ch.waiters.drain(..) {
                                    let _ = waiter.send(());
                                }
                            }
                        }
                        WaitForOp::Wait => {
                            app.wait_channels.entry(channel).or_insert_with(|| WaitChannel {
                                locked: false,
                                waiters: Vec::new(),
                            });
                        }
                    }
                }
                CtrlReq::DisplayMenu(menu_def, x, y) => {
                    let menu = parse_menu_definition(&menu_def, x, y);
                    if !menu.items.is_empty() {
                        app.mode = Mode::MenuMode { menu };
                        state_dirty = true;
                    }
                }
                CtrlReq::DisplayMenuDirect(menu) => {
                    if !menu.items.is_empty() {
                        app.mode = Mode::MenuMode { menu };
                        state_dirty = true;
                    }
                }
                CtrlReq::DisplayPopup(command, width_spec, height_spec, close_on_exit, start_dir) => {
                    // Resolve percentage dimensions against terminal area (#154)
                    let term_w = app.last_window_area.width;
                    let term_h = app.last_window_area.height;
                    let width = parse_popup_dim(&width_spec, term_w, 80);
                    let height = parse_popup_dim(&height_spec, term_h, 24);
                    // Expand format variables in start_dir (e.g. #{pane_current_path})
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let saved_dir = if start_dir.is_some() { env::current_dir().ok() } else { None };
                    if let Some(dir) = &start_dir { let _ = env::set_current_dir(dir); }
                    // Spawn the popup as a real PTY-backed Pane. An EMPTY command
                    // means "run an interactive shell" (tmux parity, issue #351):
                    // create_popup_pane() launches the shell as a REPL in that case.
                    let inner_h = height.saturating_sub(2);
                    let inner_w = width.saturating_sub(2);
                    let pane_result = crate::popup::create_popup_pane(
                        &command,
                        start_dir.as_deref(),
                        inner_h,
                        inner_w,
                        app.next_pane_id,
                        &app.session_name,
                        &app.environment,
                        app.host_colors.as_ref(),
                    );
                    if let Some(prev) = saved_dir { let _ = env::set_current_dir(prev); }

                    if pane_result.is_some() {
                        app.mode = Mode::PopupMode {
                            command: command.clone(),
                            output: String::new(),
                            process: None,
                            width,
                            height,
                            close_on_exit,
                            popup_pane: pane_result,
                            scroll_offset: 0,
                        };
                    } else {
                        // PTY spawn failed — fall back to a static, closable popup
                        // so the user is never stuck with a blank, unresponsive box.
                        app.mode = Mode::PopupMode {
                            command: command.clone(),
                            output: "Failed to start popup shell. Press 'q' or Escape to close\n".to_string(),
                            process: None,
                            width,
                            height,
                            close_on_exit: true,
                            popup_pane: None,
                            scroll_offset: 0,
                        };
                    }
                    state_dirty = true;
                }
                CtrlReq::NewFloat { command, x, y, w, h, border, title, start_dir, detached, empty, resp } => {
                    // A floating pane (tmux new-pane): a PTY-backed pane rendered
                    // over the active window's tiled layout. Flags match tmux:
                    // -x/-y are SIZE, -X/-Y are POSITION. Reuses the popup pane
                    // constructor for all PTY/vt100/reader-thread infrastructure.
                    let win_w = app.last_window_area.width.max(10);
                    let win_h = app.last_window_area.height.max(10);
                    let (dw, dh) = crate::floating::default_size(win_w, win_h);
                    // Panic-free clamp: min then max (win_w/win_h are >= 10 above).
                    let fw = w.unwrap_or(dw).min(win_w).max(3);
                    let fh = h.unwrap_or(dh).min(win_h).max(3);
                    // Position via -X/-Y (top-left); else centred (tmux default).
                    let (fx, fy) = if x.is_some() || y.is_some() {
                        crate::floating::clamp_into(x.unwrap_or(0), y.unwrap_or(0), fw, fh, win_w, win_h)
                    } else {
                        crate::floating::resolve_position("centre", win_w, win_h, fw, fh)
                    };
                    let sd = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    let inner_h = fh.saturating_sub(2).max(1);
                    let inner_w = fw.saturating_sub(2).max(1);
                    let pane_id = app.next_pane_id;
                    // -E: an empty pane has no process (blank until respawn-pane).
                    let pane_opt = if empty {
                        crate::popup::create_empty_pane(inner_h, inner_w, pane_id)
                    } else {
                        crate::popup::create_popup_pane(
                            &command,
                            sd.as_deref(),
                            inner_h,
                            inner_w,
                            pane_id,
                            &app.session_name,
                            &app.environment,
                            app.host_colors.as_ref(),
                        )
                    };
                    if let Some(mut pane) = pane_opt {
                        app.next_pane_id += 1;
                        let title_str = title.clone().unwrap_or_default();
                        if !title_str.is_empty() {
                            pane.title = title_str.clone();
                            pane.title_locked = true;
                        }
                        let border_style = if border.is_empty() { "single".to_string() } else { border.clone() };
                        let fp = crate::types::FloatingPane {
                            pane,
                            x: fx, y: fy, w: fw, h: fh,
                            border: border_style,
                            id: pane_id,
                            title: title_str,
                            position: None,
                        };
                        let win = &mut app.windows[app.active_idx];
                        win.floating.push(fp);
                        if !detached {
                            win.floating_focus = Some(win.floating.len() - 1);
                        }
                        state_dirty = true;
                        // -P: print the new pane id (tmux new-pane -P).
                        if let Some(r) = resp { let _ = r.send(format!("%{}", pane_id)); }
                    } else {
                        app.status_message = Some(("new-pane: failed to start pane".to_string(), std::time::Instant::now(), None));
                        if let Some(r) = resp { let _ = r.send(String::new()); }
                    }
                }
                CtrlReq::ConfirmBefore(prompt, cmd) => {
                    let prompt_text = if prompt.is_empty() {
                        format!("Confirm: {}? (y/n)", cmd)
                    } else {
                        // Don't append (y/n) if prompt already contains it
                        if prompt.contains("(y/n)") {
                            prompt.clone()
                        } else {
                            let base = prompt.trim_end_matches('?');
                            format!("{}? (y/n)", base)
                        }
                    };
                    app.mode = Mode::ConfirmMode {
                        prompt: prompt_text,
                        command: cmd,
                        input: String::new(),
                    };
                    state_dirty = true;
                }
                CtrlReq::ResizePaneAbsolute(axis, size) => {
                    // tmux: resize-pane -x/-y sets the focused float's absolute
                    // OUTER size (and its PTY) instead of the tiled pane.
                    let mut handled_float = false;
                    {
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if let Some(fp) = win.floating.get_mut(fi) {
                                match axis.as_str() {
                                    "x" => fp.w = size.max(3).min(win_w),
                                    "y" => fp.h = size.max(3).min(win_h),
                                    _ => {}
                                }
                                let (nx, ny) = crate::floating::clamp_into(fp.x, fp.y, fp.w, fp.h, win_w, win_h);
                                fp.x = nx; fp.y = ny;
                                let inner_h = fp.h.saturating_sub(2).max(1);
                                let inner_w = fp.w.saturating_sub(2).max(1);
                                if fp.pane.last_rows != inner_h || fp.pane.last_cols != inner_w {
                                    let _ = fp.pane.master.resize(portable_pty::PtySize { rows: inner_h, cols: inner_w, pixel_width: 0, pixel_height: 0 });
                                    if let Ok(mut parser) = fp.pane.term.lock() { parser.screen_mut().set_size(inner_h, inner_w); }
                                    fp.pane.last_rows = inner_h;
                                    fp.pane.last_cols = inner_w;
                                }
                                handled_float = true;
                            }
                        }
                    }
                    if handled_float {
                        state_dirty = true;
                    } else {
                        unzoom_if_zoomed(&mut app);
                        resize_pane_absolute(&mut app, &axis, size);
                        resize_all_panes(&mut app);
                        hook_event = Some("after-resize-pane");
                    }
                }
                CtrlReq::ResizePanePercent(axis, pct) => {
                    unzoom_if_zoomed(&mut app);
                    // Convert percentage to absolute size based on current window dimensions
                    let area = app.last_window_area;
                    let total = if axis == "x" { area.width } else { area.height };
                    let abs_size = ((total as u32) * (pct as u32) / 100).max(1) as u16;
                    resize_pane_absolute(&mut app, &axis, abs_size);
                    resize_all_panes(&mut app);
                    hook_event = Some("after-resize-pane");
                }
                CtrlReq::ShowOptionValue(resp, name) => {
                    let val = get_option_value(&app, &name);
                    let _ = resp.send(val);
                }
                CtrlReq::ShowWindowOptionValue(resp, name, target) => {
                    let val = crate::server::options::get_window_option_value_for(&app, &name, target);
                    let _ = resp.send(val);
                }
                CtrlReq::ShowWindowOptions(resp) => {
                    let _ = resp.send(render_window_options(&app));
                }
                CtrlReq::ChooseBuffer(resp) => {
                    let mut output = String::new();
                    for (i, buf) in app.paste_buffers.iter().enumerate() {
                        let preview: String = buf.chars().take(50).collect();
                        let preview = preview.replace('\n', "\\n").replace('\r', "");
                        output.push_str(&format!("buffer{}: {} bytes: \"{}\"\n", i, buf.len(), preview));
                    }
                    let mut names: Vec<&String> = app.named_buffers.keys().collect();
                    names.sort();
                    for name in names {
                        let buf = &app.named_buffers[name];
                        let preview: String = buf.chars().take(50).collect();
                        let preview = preview.replace('\n', "\\n").replace('\r', "");
                        output.push_str(&format!("{}: {} bytes: \"{}\"\n", name, buf.len(), preview));
                    }
                    let _ = resp.send(output);
                }
                CtrlReq::ServerInfo(resp) => {
                    let info = format!(
                        "psmux {} (Windows)\npid: {}\nsession: {}\nwindows: {}\nuptime: {}s\nsocket: {}",
                        VERSION,
                        std::process::id(),
                        app.session_name,
                        app.windows.len(),
                        (chrono::Local::now() - app.created_at).num_seconds(),
                        {
                            crate::paths::port_file(&app.port_file_base())
                        }
                    );
                    let _ = resp.send(info);
                }
                CtrlReq::SendPrefix => {
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
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            let _ = p.writer.write_all(&encoded);
                            let _ = p.writer.flush();
                        }
                    }
                }
                CtrlReq::PrevLayout => {
                    unzoom_if_zoomed(&mut app);
                    cycle_layout_reverse(&mut app);
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    state_dirty = true;
                }
                CtrlReq::FocusIn => {
                    if app.focus_events {
                        // Forward focus-in escape sequence to all panes in active window
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.writer.write_all(seq); let _ = p.writer.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[I");
                    }
                    hook_event = Some("pane-focus-in");
                }
                CtrlReq::FocusOut => {
                    if app.focus_events {
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.writer.write_all(seq); let _ = p.writer.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[O");
                    }
                    hook_event = Some("pane-focus-out");
                }
                CtrlReq::CommandPrompt(initial) => {
                    app.mode = Mode::CommandPrompt { input: initial.clone(), cursor: initial.len() };
                    state_dirty = true;
                }
                CtrlReq::ShowMessages(resp) => {
                    // Return message log (tmux stores recent log messages)
                    let _ = resp.send(String::new());
                }
                CtrlReq::ResizeWindow(request, resp) => {
                    let result = crate::resize_window::apply_resize_window(&mut app, &request);
                    match result {
                        Ok(resized) => {
                            state_dirty = true;
                            meta_dirty = true;
                            if let Some(window) = app.windows.get(resized.window_index) {
                                let layout = control::window_layout_string(window, resized.area);
                                control::emit_notification(&app, crate::types::ControlNotification::LayoutChange {
                                    window_id: resized.window_id,
                                    layout,
                                });
                            }
                            let _ = resp.send(Ok(()));
                        }
                        Err(error) => {
                            let _ = resp.send(Err(error));
                        }
                    }
                }
                CtrlReq::ControlClientResize { client_id, window_id, size } => {
                    let old_areas: std::collections::HashMap<usize, Rect> = app
                        .windows
                        .iter()
                        .map(|window| (window.id, window.area))
                        .collect();
                    if let Some(client) = app.control_clients.get_mut(&client_id) {
                        if let Some(window_id) = window_id {
                            if let Some(size) = size {
                                client.window_sizes.insert(window_id, size);
                            } else {
                                client.window_sizes.remove(&window_id);
                            }
                        } else {
                            client.size = size;
                        }
                        if size.is_some() {
                            app.latest_size_client_id = Some(client_id);
                        }
                        if window_id.is_none() {
                            if let Some((width, height)) = size {
                                app.client_area = Rect::new(0, 0, width, height);
                            }
                        }
                        let geometry_changed = crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                        if geometry_changed {
                            resize_all_panes(&mut app);
                        }
                        state_dirty = true;
                        meta_dirty = true;
                        for window in app.windows.iter().filter(|window| {
                            old_areas.get(&window.id).copied() != Some(window.area)
                        }) {
                            let layout = control::window_layout_string(window, window.area);
                            control::emit_notification(&app, crate::types::ControlNotification::LayoutChange {
                                window_id: window.id,
                                layout,
                            });
                        }
                    }
                }
                CtrlReq::RespawnWindow => {
                    // Kill all panes in the active window and respawn
                    respawn_active_pane(&mut app, Some(&*pty_system), None, true, None, false)?;
                    state_dirty = true;
                }
                CtrlReq::PopupInput(data) => {
                    if let Mode::PopupMode { ref mut popup_pane, .. } = app.mode {
                        if let Some(ref mut pty) = popup_pane {
                            // If child has exited, 'q' closes the popup
                            let child_exited = matches!(pty.child.try_wait(), Ok(Some(_)));
                            if child_exited && data == b"q" {
                                app.mode = Mode::Passthrough;
                            } else if !child_exited {
                                let _ = pty.writer.write_all(&data);
                                let _ = pty.writer.flush();
                            }
                        } else {
                            // No PTY means static popup — 'q' closes it
                            if data == b"q" {
                                app.mode = Mode::Passthrough;
                            }
                        }
                    }
                    state_dirty = true;
                }
                CtrlReq::OverlayClose => {
                    match app.mode {
                        Mode::PopupMode { .. } | Mode::MenuMode { .. } | Mode::ConfirmMode { .. } | Mode::PaneChooser { .. } | Mode::ClockMode | Mode::CustomizeMode { .. } => {
                            app.mode = Mode::Passthrough;
                            state_dirty = true;
                        }
                        _ => {}
                    }
                }
                CtrlReq::ConfirmRespond(yes) => {
                    if let Mode::ConfirmMode { ref command, .. } = app.mode {
                        let cmd = command.clone();
                        app.mode = Mode::Passthrough;
                        if yes {
                            let _ = execute_command_string(&mut app, &cmd);
                        }
                        state_dirty = true;
                    }
                }
                CtrlReq::MenuSelect(idx) => {
                    if let Mode::MenuMode { ref menu } = app.mode {
                        if let Some(item) = menu.items.get(idx) {
                            if !item.is_separator && !item.command.is_empty() {
                                let cmd = item.command.clone();
                                app.mode = Mode::Passthrough;
                                let _ = execute_command_string(&mut app, &cmd);
                                state_dirty = true;
                            }
                        }
                    }
                }
                CtrlReq::MenuNavigate(delta) => {
                    if let Mode::MenuMode { ref mut menu } = app.mode {
                        let len = menu.items.len();
                        if len > 0 {
                            if delta > 0 {
                                // Move down, skipping separators
                                let mut next = (menu.selected + 1) % len;
                                let start = next;
                                while menu.items[next].is_separator {
                                    next = (next + 1) % len;
                                    if next == start { break; }
                                }
                                menu.selected = next;
                            } else {
                                // Move up, skipping separators
                                let mut next = if menu.selected == 0 { len - 1 } else { menu.selected - 1 };
                                let start = next;
                                while menu.items[next].is_separator {
                                    next = if next == 0 { len - 1 } else { next - 1 };
                                    if next == start { break; }
                                }
                                menu.selected = next;
                            }
                            state_dirty = true;
                        }
                    }
                }
                CtrlReq::ShowTextPopup(title, content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let width = lines.iter().map(|l| l.len()).max().unwrap_or(40).max(20) as u16 + 4;
                    let height = (lines.len() as u16 + 2).max(5);
                    app.mode = Mode::PopupMode {
                        command: title,
                        output: content,
                        process: None,
                        width: width.min(120),
                        height,
                        close_on_exit: false,
                        popup_pane: None,
                        scroll_offset: 0,
                    };
                    state_dirty = true;
                }
                CtrlReq::StatusMessage(msg) => {
                    app.status_message = Some((msg, std::time::Instant::now(), None));
                    state_dirty = true;
                }
                CtrlReq::ClearPromptHistory => {
                    app.command_history.clear();
                    app.command_history_idx = 0;
                }
                CtrlReq::ShowPromptHistory(persistent) => {
                    if persistent {
                        let content = if app.command_history.is_empty() {
                            "(no prompt history)\n".to_string()
                        } else {
                            app.command_history.iter().enumerate()
                                .map(|(i, cmd)| format!("{}: {}", i, cmd))
                                .collect::<Vec<_>>().join("\n")
                        };
                        let lines: Vec<&str> = content.lines().collect();
                        let width = lines.iter().map(|l| l.len()).max().unwrap_or(40).max(20) as u16 + 4;
                        let height = (lines.len() as u16 + 2).max(5);
                        app.mode = Mode::PopupMode {
                            command: "show-prompt-history".to_string(),
                            output: content,
                            process: None,
                            width: width.min(120),
                            height: height.min(40),
                            close_on_exit: false,
                            popup_pane: None,
                            scroll_offset: 0,
                        };
                        state_dirty = true;
                    }
                }
                CtrlReq::ControlRegister { client_id, echo, notif_tx } => {
                    app.control_clients.insert(client_id, crate::types::ControlClient {
                        client_id,
                        cmd_counter: 0,
                        echo_enabled: echo,
                        notification_tx: notif_tx,
                        paused_panes: std::collections::HashSet::new(),
                        subscriptions: std::collections::HashMap::new(),
                        subscription_values: std::collections::HashMap::new(),
                        subscription_last_check: std::collections::HashMap::new(),
                        pause_after_secs: None,
                        output_paused_panes: std::collections::HashSet::new(),
                        pane_last_output: std::collections::HashMap::new(),
                        size: None,
                        window_sizes: std::collections::HashMap::new(),
                    });
                    // Register control clients with the same idempotent
                    // counter/registry invariant as normal TUI clients.
                    app.register_client(client_id, true);
                    // Real tmux fires server hooks (session-changed, window-add,
                    // etc.) as side effects of the initial attach-session command.
                    // iTerm2 depends on %session-changed to enable writes
                    // (_canWrite = YES) and flush its command queue. Without
                    // this notification, iTerm2 never sends any commands and
                    // sits idle forever.
                    //
                    // The unsolicited %begin/%end pair (flags=0) is emitted by
                    // connection.rs right after the DCS opener. That triggers
                    // tmuxInitialCommandDidCompleteSuccessfully in iTerm2 which
                    // queues the initialization commands. Then the
                    // %session-changed notification below enables writes so
                    // those queued commands actually get sent.
                    crate::control::emit_initial_state(&app, client_id);
                }
                CtrlReq::ControlSubscribe { client_id, name, target, format } => {
                    if let Some(cc) = app.control_clients.get_mut(&client_id) {
                        cc.subscriptions.insert(name.clone(), (target, format));
                        // Clear cached value so the first check always emits
                        cc.subscription_values.remove(&name);
                        cc.subscription_last_check.remove(&name);
                    }
                }
                CtrlReq::ControlUnsubscribe { client_id, name } => {
                    if let Some(cc) = app.control_clients.get_mut(&client_id) {
                        cc.subscriptions.remove(&name);
                        cc.subscription_values.remove(&name);
                        cc.subscription_last_check.remove(&name);
                    }
                }
                CtrlReq::ControlSetPauseAfter { client_id, pause_after_secs } => {
                    if let Some(cc) = app.control_clients.get_mut(&client_id) {
                        cc.pause_after_secs = pause_after_secs;
                        if pause_after_secs.is_none() {
                            // Clear all pause state when disabling
                            cc.output_paused_panes.clear();
                            cc.pane_last_output.clear();
                        }
                    }
                }
                CtrlReq::ControlContinuePane { client_id, pane_id } => {
                    if let Some(cc) = app.control_clients.get_mut(&client_id) {
                        if cc.output_paused_panes.remove(&pane_id) {
                            let _ = cc.notification_tx.try_send(
                                crate::types::ControlNotification::Continue { pane_id }
                            );
                        }
                    }
                }
                CtrlReq::ControlDeregister { client_id } => {
                    app.control_clients.remove(&client_id);
                    // Idempotent reap keeps the counter in lock-step with the
                    // registry even if a control client is deregistered twice.
                    app.reap_client(client_id);
                    if crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                        resize_all_panes(&mut app);
                        state_dirty = true;
                        meta_dirty = true;
                    }
                }
                CtrlReq::CustomizeMode => {
                    let options = crate::server::option_catalog::build_option_list(&app);
                    app.mode = Mode::CustomizeMode {
                        options,
                        selected: 0,
                        scroll_offset: 0,
                        editing: false,
                        edit_buffer: String::new(),
                        edit_cursor: 0,
                        filter: String::new(),
                    };
                    state_dirty = true;
                }
                CtrlReq::CustomizeNavigate(delta) => {
                    if let Mode::CustomizeMode { ref options, ref mut selected, ref filter, ref mut scroll_offset, editing, .. } = app.mode {
                        if !editing {
                            let visible: Vec<usize> = options.iter().enumerate()
                                .filter(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                                .map(|(i, _)| i)
                                .collect();
                            if !visible.is_empty() {
                                let cur_pos = visible.iter().position(|&i| i == *selected).unwrap_or(0);
                                let new_pos = if delta > 0 {
                                    (cur_pos + delta as usize).min(visible.len() - 1)
                                } else {
                                    cur_pos.saturating_sub((-delta) as usize)
                                };
                                *selected = visible[new_pos];
                                // Update scroll offset to keep selection visible
                                if new_pos < *scroll_offset {
                                    *scroll_offset = new_pos;
                                } else if new_pos >= *scroll_offset + 20 {
                                    *scroll_offset = new_pos.saturating_sub(19);
                                }
                            }
                            state_dirty = true;
                        }
                    }
                }
                CtrlReq::CustomizeEdit => {
                    if let Mode::CustomizeMode { ref options, selected, ref mut editing, ref mut edit_buffer, ref mut edit_cursor, .. } = app.mode {
                        if !*editing {
                            if let Some((_, value, _)) = options.get(selected) {
                                *edit_buffer = value.clone();
                                *edit_cursor = edit_buffer.len();
                                *editing = true;
                                state_dirty = true;
                            }
                        }
                    }
                }
                CtrlReq::CustomizeEditUpdate(text) => {
                    if let Mode::CustomizeMode { editing, ref mut edit_buffer, ref mut edit_cursor, .. } = app.mode {
                        if editing {
                            *edit_buffer = text.clone();
                            *edit_cursor = edit_buffer.len();
                            state_dirty = true;
                        }
                    }
                }
                CtrlReq::CustomizeEditConfirm => {
                    if let Mode::CustomizeMode { ref mut options, selected, ref mut editing, ref edit_buffer, .. } = app.mode {
                        if *editing {
                            let name = options[selected].0.clone();
                            let value = edit_buffer.clone();
                            options[selected].1 = value.clone();
                            *editing = false;
                            options::apply_set_option(&mut app, &name, &value, true);
                            state_dirty = true;
                        }
                    }
                }
                CtrlReq::CustomizeEditCancel => {
                    if let Mode::CustomizeMode { ref mut editing, ref mut edit_buffer, .. } = app.mode {
                        if *editing {
                            *editing = false;
                            *edit_buffer = String::new();
                            state_dirty = true;
                        }
                    }
                }
                CtrlReq::CustomizeResetDefault => {
                    if let Mode::CustomizeMode { ref mut options, selected, editing, .. } = app.mode {
                        if !editing {
                            if let Some(def) = option_catalog::default_for(&options[selected].0) {
                                let name = options[selected].0.clone();
                                let value = def.to_string();
                                options[selected].1 = value.clone();
                                options::apply_set_option(&mut app, &name, &value, true);
                                state_dirty = true;
                            }
                        }
                    }
                }
                CtrlReq::CustomizeFilter(text) => {
                    if let Mode::CustomizeMode { ref mut filter, ref mut selected, ref mut scroll_offset, ref options, .. } = app.mode {
                        *filter = text;
                        // Reset selection to first matching option
                        let first_match = options.iter().enumerate()
                            .find(|(_, (name, _, _))| filter.is_empty() || name.contains(filter.as_str()))
                            .map(|(i, _)| i);
                        if let Some(idx) = first_match {
                            *selected = idx;
                        }
                        *scroll_offset = 0;
                        state_dirty = true;
                    }
                }
                CtrlReq::RunCommand(cmd, resp) => {
                    let result = execute_command_string(&mut app, &cmd);
                    match result {
                        Ok(()) => { let _ = resp.send("OK".to_string()); }
                        Err(e) => { let _ = resp.send(format!("error: {}", e)); }
                    }
                }
            }
            // Log any active_idx change for debugging window-switch issues
            if app.active_idx != _prev_active_idx && crate::debug_log::server_log_enabled() {
                crate::debug_log::server_log("switch", &format!(
                    "active_idx changed {} -> {} by req={} hook={:?}",
                    _prev_active_idx, app.active_idx, _req_tag, hook_event));
            }
            // Fire any hooks registered for the event that just occurred
            if let Some(event) = hook_event {
                let _pre_hook_idx = app.active_idx;
                let cmds: Vec<String> = app.hooks.get(event).cloned().unwrap_or_default();
                for cmd in cmds {
                    let _ = execute_command_string(&mut app, &cmd);
                }
                // Emit control mode notifications for hook events
                if !app.control_clients.is_empty() {
                    let active_win = &app.windows[app.active_idx];
                    let win_id = active_win.id;
                    let active_pane_id = get_active_pane_id(&active_win.root, &active_win.active_path).unwrap_or(0);
                    match event {
                        "after-new-window" => {
                            control::emit_notification(&app, crate::types::ControlNotification::WindowAdd { window_id: win_id });
                        }
                        "after-kill-pane" | "window-closed" => {
                            control::emit_notification(&app, crate::types::ControlNotification::WindowClose { window_id: win_id });
                        }
                        "after-rename-window" => {
                            let name = active_win.name.clone();
                            control::emit_notification(&app, crate::types::ControlNotification::WindowRenamed { window_id: win_id, name });
                        }
                        "after-select-window" => {
                            control::emit_notification(&app, crate::types::ControlNotification::SessionWindowChanged {
                                session_id: app.session_id, window_id: win_id,
                            });
                        }
                        "after-select-pane" => {
                            control::emit_notification(&app, crate::types::ControlNotification::WindowPaneChanged {
                                window_id: win_id, pane_id: active_pane_id,
                            });
                        }
                        "after-rename-session" => {
                            let name = app.session_name.clone();
                            control::emit_notification(&app, crate::types::ControlNotification::SessionRenamed { name });
                        }
                        "client-attached" => {
                            let name = app.session_name.clone();
                            control::emit_notification(&app, crate::types::ControlNotification::SessionChanged {
                                session_id: app.session_id, name,
                            });
                        }
                        "client-detached" => {
                            control::emit_notification(&app, crate::types::ControlNotification::ClientDetached {
                                client: "client".to_string(),
                            });
                        }
                        "after-split-window" | "after-resize-pane" | "after-break-pane"
                        | "after-join-pane" | "after-rotate-window" | "after-swap-pane" => {
                            let layout = if let Some(w) = app.windows.iter().find(|w| w.id == win_id) {
                                control::window_layout_string(w, w.area)
                            } else {
                                let area = app.last_window_area;
                                format!("0000,{}x{},0,0", area.width, area.height)
                            };
                            control::emit_notification(&app, crate::types::ControlNotification::LayoutChange {
                                window_id: win_id,
                                layout,
                            });
                        }
                        "window-linked" => {
                            control::emit_notification(&app, crate::types::ControlNotification::WindowAdd { window_id: win_id });
                        }
                        "window-unlinked" => {
                            control::emit_notification(&app, crate::types::ControlNotification::WindowClose { window_id: win_id });
                        }
                        _ => {}
                    }
                }
                // Check if the hook itself changed active_idx
                if app.active_idx != _pre_hook_idx && crate::debug_log::server_log_enabled() {
                    crate::debug_log::server_log("switch", &format!(
                        "active_idx changed {} -> {} by HOOK event={}",
                        _pre_hook_idx, app.active_idx, event));
                }
            }
            // Restore temporary -t focus after non-temp command completes.
            // Use pane ID (not path) because kill-pane restructures the
            // tree and invalidates saved paths (#71).
            if !is_temp_focus {
                if let Some((restore_idx, restore_pane_id)) = temp_focus_restore.take() {
                    if restore_idx < app.windows.len() {
                        app.active_idx = restore_idx;
                        let win = &mut app.windows[restore_idx];
                        if let Some(path) = crate::tree::find_path_by_id(&win.root, restore_pane_id) {
                            win.active_path = path;
                        }
                        app.last_window_area = win.area;
                        // If the pane was killed, keep whatever active_path
                        // kill_pane_at_path already set (MRU target).
                    }
                    // Temporary focus is over; active_idx is real again.
                    app.temp_focus_saved_active = None;
                }
            }
            if mutates_state {
                state_dirty = true;
            }
        }
                // No trailing cleanup: temp_focus_restore persists across
                // batch boundaries so the actual command that follows in a
                // later batch can still benefit from the temp focus (and
                // will restore when it processes as a non-temp-focus req).
            }
        }
        // Drain async run-shell results (non-blocking).
        if let Some(rx) = app.run_shell_rx.as_ref() {
            while let Ok((title, text)) = rx.try_recv() {
                if !text.is_empty() {
                    let lines: Vec<&str> = text.lines().collect();
                    let width = lines.iter().map(|l| l.len()).max().unwrap_or(40).max(20) as u16 + 4;
                    let height = (lines.len() as u16 + 2).max(5);
                    app.mode = Mode::PopupMode {
                        command: title,
                        output: text,
                        process: None,
                        width: width.min(120),
                        height,
                        close_on_exit: false,
                        popup_pane: None,
                        scroll_offset: 0,
                    };
                    state_dirty = true;
                }
            }
        }
        // Drain async #(command) format-job results (non-blocking): refresh the
        // cache entry and repaint so the fresh output replaces the stale one.
        if let Some(rx) = app.format_job_rx.as_ref() {
            while let Ok((cmd, output)) = rx.try_recv() {
                if let Ok(mut guard) = app.format_shell_cache.lock() {
                    let now = std::time::Instant::now();
                    guard.insert(cmd, crate::types::ShellEntry { at: now, value: output, running: false });
                }
                state_dirty = true;
            }
        }
        // ── Server-push: proactively send frames to attached clients ──
        // Instead of waiting for clients to poll dump-state, serialize
        // and push whenever state changed (PTY output, new window, key
        // echo, etc.).  This gives event-driven rendering like wezterm:
        // frames arrive within 1-5ms of ConPTY output instead of waiting
        // for the next client poll cycle (up to 50ms).
        if (state_dirty || meta_dirty) && crate::types::has_frame_receivers() {
            // Check bell/activity state for the pushed frame
            let push_alert_hooks = helpers::check_window_activity(&mut app);
            for event in &push_alert_hooks {
                crate::commands::fire_hooks(&mut app, event);
            }
            // Rebuild metadata cache if structural changes happened.
            if meta_dirty {
                cached_windows_json = list_windows_json_with_tabs(&app)?;
                cached_tree_json = list_tree_json(&app)?;
                cached_prefix_str = format_key_binding(&app.prefix_key);
                cached_prefix2_str = app.prefix2_key.as_ref().map(|k| format_key_binding(k)).unwrap_or_default();
                cached_base_index = app.window_base_index;
                cached_pred_dim = app.prediction_dimming;
                cached_status_style = app.status_style.clone();
                cached_bindings_json = serialize_bindings_json(&app);
                meta_dirty = false;
            }
            let layout_json = dump_layout_json_fast(&mut app)?;
            combined_buf.clear();
            // #372: style options must be format-expanded too (see persistent
            // path above). wsf/wscf stay raw: per-window formats the client
            // expands with each window's own context.
            // All of it goes through the one guarded helper — this block used to
            // carry its own copy of the list WITHOUT the async guard, which is
            // what made every keystroke wait on a status-bar #() spawn.
            let sf = helpers::expand_status_formats(&app, &cached_status_style);
            let ss_escaped = json_escape_string(&sf.status_style);
            let sl_expanded = json_escape_string(&sf.status_left);
            let sr_expanded = json_escape_string(&sf.status_right);
            let pbs_escaped = json_escape_string(&sf.pane_border_style);
            let pabs_escaped = json_escape_string(&sf.pane_active_border_style);
            let pbhs_escaped = json_escape_string(&sf.pane_border_hover_style);
            let wsf_escaped = json_escape_string(&app.window_status_format);
            let wscf_escaped = json_escape_string(&app.window_status_current_format);
            let wss_escaped = json_escape_string(&sf.window_status_separator);
            let ws_style_escaped = json_escape_string(&sf.window_status_style);
            let wsc_style_escaped = json_escape_string(&sf.window_status_current_style);
            let mode_style_escaped = json_escape_string(&sf.mode_style);
            // #372: message-style was never sent to the client (it hard-coded
            // bg=yellow,fg=black). Send it, format-expanded.
            let message_style_escaped = json_escape_string(&sf.message_style);
            let status_position_escaped = json_escape_string(&app.status_position);
            let status_justify_escaped = json_escape_string(&app.status_justify);
            let status_format_json = &sf.status_format_json;
            let cursor_style_code = crate::rendering::configured_cursor_code();
            let _ = std::fmt::Write::write_fmt(&mut combined_buf, format_args!(
                "{{\"layout\":{},\"windows\":{},\"prefix\":\"{}\",\"prefix2\":\"{}\",\"tree\":{},\"base_index\":{},\"pane_base_index\":{},\"prediction_dimming\":{},\"status_style\":\"{}\",\"status_left\":\"{}\",\"status_right\":\"{}\",\"pane_border_style\":\"{}\",\"pane_active_border_style\":\"{}\",\"pane_border_hover_style\":\"{}\",\"wsf\":\"{}\",\"wscf\":\"{}\",\"wss\":\"{}\",\"ws_style\":\"{}\",\"wsc_style\":\"{}\",\"clock_mode\":{},\"bindings\":{},\"status_left_length\":{},\"status_right_length\":{},\"status_lines\":{},\"status_format\":{},\"mode_style\":\"{}\",\"message_style\":\"{}\",\"status_position\":\"{}\",\"status_justify\":\"{}\",\"cursor_style_code\":{},\"status_visible\":{},\"repeat_time\":{},\"zoomed\":{},\"pwsh_mouse_selection\":{},\"mouse_selection\":{},\"mouse_selection_force\":{},\"paste_detection\":{},\"choose_tree_preview\":{},\"scroll_enter_copy_mode\":{},\"bold_is_bright\":{}}}",
                layout_json, cached_windows_json, cached_prefix_str, cached_prefix2_str, cached_tree_json, cached_base_index, app.pane_base_index, cached_pred_dim, ss_escaped, sl_expanded, sr_expanded, pbs_escaped, pabs_escaped, pbhs_escaped, wsf_escaped, wscf_escaped, wss_escaped, ws_style_escaped, wsc_style_escaped,
                matches!(app.mode, Mode::ClockMode), cached_bindings_json,
                app.status_left_length, app.status_right_length, app.status_lines, status_format_json,
                mode_style_escaped, message_style_escaped, status_position_escaped, status_justify_escaped,
                cursor_style_code, app.status_visible, app.repeat_time_ms,
                app.windows.get(app.active_idx).map_or(false, |w| w.zoom_saved.is_some()),
                app.pwsh_mouse_selection,
                app.mouse_selection,
                app.mouse_selection_force,
                app.paste_detection,
                app.choose_tree_preview,
                app.scroll_enter_copy_mode,
                app.bold_is_bright,
            ));
            helpers::append_extra_style_json(&mut combined_buf, &app);
            // Inject overlay state (popup, menu, confirm, display_panes)
            {
                // Inject clock_colour if set
                if let Some(cc) = app.user_options.get("clock-mode-colour") {
                    if combined_buf.ends_with('}') {
                        combined_buf.pop();
                        combined_buf.push_str(",\"clock_colour\":\"");
                        combined_buf.push_str(&json_escape_string(cc));
                        combined_buf.push_str("\"}");
                    }
                }
                // Inject pane-border-status and pane-border-format
                if let Some(pbs) = app.user_options.get("pane-border-status") {
                    if combined_buf.ends_with('}') {
                        combined_buf.pop();
                        combined_buf.push_str(",\"pane_border_status\":\"");
                        combined_buf.push_str(&json_escape_string(pbs));
                        combined_buf.push('"');
                        if let Some(pbf) = app.user_options.get("pane-border-format") {
                            combined_buf.push_str(",\"pane_border_format\":\"");
                            combined_buf.push_str(&json_escape_string(pbf));
                            combined_buf.push('"');
                        }
                        combined_buf.push('}');
                    }
                }
                // Inject pane-border-lines independently — it may be set
                // without pane-border-status.
                if let Some(pbl) = app.user_options.get("pane-border-lines") {
                    if combined_buf.ends_with('}') {
                        combined_buf.pop();
                        combined_buf.push_str(",\"pane_border_lines\":\"");
                        combined_buf.push_str(&json_escape_string(pbl));
                        combined_buf.push('"');
                        combined_buf.push('}');
                    }
                }
                helpers::append_copy_ln_json(&app, &mut combined_buf);
                helpers::append_floats_json(&app, &mut combined_buf);
                // set-titles: when on, ship the expanded set-titles-string so the
                // client emits OSC 0 to its host terminal. Expanded up in
                // expand_status_formats, under the async guard — it used to be
                // expanded right here, outside it.
                if let Some(title) = sf.host_title.as_deref() {
                    if combined_buf.ends_with('}') {
                        combined_buf.pop();
                        combined_buf.push_str(",\"host_title\":\"");
                        combined_buf.push_str(&json_escape_string(title));
                        combined_buf.push_str("\"}");
                    }
                }
                // Issue #269: forward OSC 9;4 progress from the active pane.
                if combined_buf.ends_with('}') {
                    if let Some((s, v)) = helpers::active_pane_progress(&app) {
                        combined_buf.pop();
                        combined_buf.push_str(",\"host_progress\":\"");
                        combined_buf.push_str(&format!("{};{}", s, v));
                        combined_buf.push_str("\"}");
                    }
                }
                let overlay_json = serialize_overlay_json(&app);
                if !overlay_json.is_empty() && combined_buf.ends_with('}') {
                    combined_buf.pop();
                    combined_buf.push_str(&overlay_json);
                    combined_buf.push('}');
                }
            }
            // Ingest OSC 52 from pane child processes (e.g. Claude Code
            // `/copy`).  See sibling call in the dump-state response path
            // for full context.  Gated by `set-clipboard` inside the helper.
            crate::server::helpers::drain_osc52(&mut app);
            // Inject clipboard data if pending
            if let Some(clip_text) = app.clipboard_osc52.take() {
                let clip_b64 = base64_encode(&clip_text);
                if combined_buf.ends_with('}') {
                    combined_buf.pop();
                    combined_buf.push_str(",\"clipboard_osc52\":\"");
                    combined_buf.push_str(&clip_b64);
                    combined_buf.push_str("\"}");
                }
            }
            cached_dump_state.clear();
            cached_dump_state.push_str(&combined_buf);
            // Inject bell AFTER caching (one-shot: should not persist in cache)
            if app.bell_forward {
                app.bell_forward = false;
                if combined_buf.ends_with('}') {
                    combined_buf.pop();
                    combined_buf.push_str(",\"bell\":true}");
                }
            }
            cached_data_version = combined_data_version(&app);
            state_dirty = false;
            crate::types::push_frame(&combined_buf);
        }
        // ── Status-interval timer: fire hooks periodically ──
        if app.should_run_status_interval_timer() {
            let elapsed = app.last_status_interval_fire.elapsed().as_secs();
            if elapsed >= app.status_interval {
                app.last_status_interval_fire = std::time::Instant::now();
                let _pre_status_idx = app.active_idx;
                let cmds: Vec<String> = app.hooks.get("status-interval").cloned().unwrap_or_default();
                for cmd in cmds {
                    let bg_cmd = crate::commands::ensure_background(&cmd);
                    let _ = execute_command_string(&mut app, &bg_cmd);
                }
                if app.active_idx != _pre_status_idx && crate::debug_log::server_log_enabled() {
                    crate::debug_log::server_log("switch", &format!(
                        "active_idx changed {} -> {} by status-interval hook",
                        _pre_status_idx, app.active_idx));
                }
                // Mark state dirty so the next loop iteration pushes a fresh
                // frame with re-expanded strftime codes (%H:%M:%S, %r, etc.)
                // in status-left / status-right.  Without this, the status
                // bar clock never updates for persistent (TUI) clients.
                state_dirty = true;
            }
        }
        // ── Subscription check: expand format strings and emit %subscription-changed ──
        // Zero cost when no clients have subscriptions.
        if !app.control_clients.is_empty() {
            let now_sub = std::time::Instant::now();
            // Phase 1: collect (client_id, sub_name, format) pairs that need checking
            let mut to_check: Vec<(u64, String, String)> = Vec::new();
            for client in app.control_clients.values_mut() {
                if client.subscriptions.is_empty() {
                    continue;
                }
                let sub_names: Vec<String> = client.subscriptions.keys().cloned().collect();
                for name in sub_names {
                    // Rate limit: at most once per second per subscription
                    if let Some(last) = client.subscription_last_check.get(&name) {
                        if now_sub.duration_since(*last).as_secs() < 1 {
                            continue;
                        }
                    }
                    client.subscription_last_check.insert(name.clone(), now_sub);
                    let format = client.subscriptions[&name].1.clone();
                    to_check.push((client.client_id, name, format));
                }
            }
            // Phase 2: expand formats with immutable borrow of app
            let mut sub_results: Vec<(u64, String, String)> = Vec::new();
            for (cid, name, format) in &to_check {
                let expanded = crate::format::expand_format(format, &app);
                sub_results.push((*cid, name.clone(), expanded));
            }
            // Phase 3: compare and emit notifications
            let active_win = &app.windows[app.active_idx];
            let win_id = active_win.id;
            let pane_id = get_active_pane_id(&active_win.root, &active_win.active_path).unwrap_or(0);
            let session_id = app.session_id;
            let win_idx = app.active_idx;
            let mut sub_notifs: Vec<(u64, crate::types::ControlNotification)> = Vec::new();
            for (cid, name, expanded) in sub_results {
                if let Some(cc) = app.control_clients.get(&cid) {
                    let changed = match cc.subscription_values.get(&name) {
                        Some(prev) => prev != &expanded,
                        None => true,
                    };
                    if changed {
                        sub_notifs.push((cid, crate::types::ControlNotification::SubscriptionChanged {
                            name: name.clone(),
                            session_id,
                            window_id: win_id,
                            window_index: win_idx,
                            pane_id,
                            value: expanded.clone(),
                        }));
                    }
                }
            }
            // Phase 4: update cached values and send notifications
            for (cid, ref notif) in &sub_notifs {
                if let Some(cc) = app.control_clients.get_mut(cid) {
                    if let crate::types::ControlNotification::SubscriptionChanged { name, value, .. } = notif {
                        cc.subscription_values.insert(name.clone(), value.clone());
                    }
                }
            }
            for (cid, notif) in sub_notifs {
                if let Some(cc) = app.control_clients.get(&cid) {
                    let _ = cc.notification_tx.try_send(notif);
                }
            }
        }
        // ── PaneChooser timeout ──
        // Auto-close display-panes overlay after display-panes-time (default 1000ms).
        if let Mode::PaneChooser { opened_at } = &app.mode {
            if opened_at.elapsed() > Duration::from_millis(app.display_panes_time_ms) {
                app.mode = Mode::Passthrough;
                state_dirty = true;
            }
        }
        // ── Popup child exit detection ──
        // Check if popup PTY's child process has exited; if so, auto-close.
        if let Mode::PopupMode { ref mut popup_pane, close_on_exit, .. } = app.mode {
            let should_close = if let Some(ref mut pane) = popup_pane {
                matches!(pane.child.try_wait(), Ok(Some(_)))
            } else { false };
            if should_close && close_on_exit {
                app.mode = Mode::Passthrough;
                state_dirty = true;
            }
        }
        // Reap exited floating panes (tmux new-pane) across all windows: a float
        // whose child process has exited is removed, and the focus index is
        // fixed up so it never dangles past the end of the vec.
        for win in app.windows.iter_mut() {
            if win.floating.is_empty() { continue; }
            let before = win.floating.len();
            win.floating.retain_mut(|fp| !matches!(fp.pane.child.try_wait(), Ok(Some(_))));
            if win.floating.len() != before {
                // Simplest correct focus fix: focus the last remaining float, or
                // drop focus entirely when none remain.
                win.floating_focus = if win.floating.is_empty() {
                    None
                } else {
                    Some(win.floating.len() - 1)
                };
                state_dirty = true;
            }
        }
        // Check if all windows/panes have exited (throttled to every 250ms)
        if last_reap.elapsed() >= Duration::from_millis(100) {
            last_reap = Instant::now();
            // tmux parity (input_osc_52 paste_add): pane initiated OSC 52
            // must land in the paste buffer stack even when NO client is
            // attached, because tmux captures it server side during input
            // parsing. The dump-state builders only drain while a client
            // polls, so this tick is what makes show-buffer work for
            // detached sessions.
            crate::server::helpers::drain_osc52(&mut app);
            // #450: self-heal the warm pane pool.  The spare shell can die
            // while idling (shell crash, external kill, dead conhost); the
            // consume-time gate in create_window/split then falls back to a
            // cold spawn, but replacing the corpse here keeps the next
            // new-window on the instant warm path.
            let warm_dead = app.warm_pane.as_mut()
                .map(|wp| !crate::pane::warm_pane_is_live(wp))
                .unwrap_or(false);
            if warm_dead {
                if let Some(mut dead) = app.warm_pane.take() { dead.child.kill().ok(); }
                if let Ok(nw) = spawn_warm_pane(&*pty_system, &mut app) {
                    app.warm_pane = Some(nw);
                }
            }
            // #450 (opt-in `@heal-crashed-panes`): a shell can FailFast on its
            // very first ConPTY read right after a warm-pane transplant (pwsh
            // whose PSReadLine is not the active reader hits ERROR_INVALID_PARAMETER
            // in its fallback ReadLineFromFile). The pane passed the consume-time
            // liveness gate, then died a beat later, so the reaper would prune it
            // to a broken/empty window. If a pane's shell exits within a short
            // grace window of being spawned, treat it as crash-on-startup and
            // respawn a fresh shell IN PLACE (at most once per pane) so the user
            // still gets a working window. Runs BEFORE reap so the revived pane
            // is not pruned.
            if app.heal_crashed_panes() {
                let grace = Duration::from_millis(4000);
                let mut to_heal: Vec<(usize, Vec<usize>, usize)> = Vec::new();
                for wi in 0..app.windows.len() {
                    for id in tree::collect_pane_ids(&app.windows[wi].root) {
                        if app.healed_pane_ids.contains(&id) { continue; }
                        let Some(path) = tree::find_path_by_id(&app.windows[wi].root, id) else { continue; };
                        if let Some(pane) = tree::active_pane_mut(&mut app.windows[wi].root, &path) {
                            let young = pane.spawned_at.map(|t| t.elapsed() < grace).unwrap_or(false);
                            if !young { continue; }
                            if matches!(pane.child.try_wait(), Ok(Some(_))) {
                                to_heal.push((wi, path, id));
                            }
                        }
                    }
                }
                for (wi, path, id) in to_heal {
                    match crate::window_ops::heal_respawn_pane(&mut app, &*pty_system, wi, &path) {
                        Ok(()) => {
                            app.healed_pane_ids.insert(id);
                            state_dirty = true;
                            crate::debug_log::server_log("heal", &format!(
                                "respawned crashed pane {} in window {} (@heal-crashed-panes)", id, wi));
                        }
                        Err(e) => crate::debug_log::server_log("heal", &format!(
                            "heal respawn failed for pane {}: {}", id, e)),
                    }
                }
            }
            // Snapshot per-window state BEFORE reap so we can diff and emit
            // accurate %window-close / %layout-change / %window-pane-changed
            // notifications to control-mode clients (iTerm2 etc.).  Without
            // this, a pane that exits naturally (`exit` in pwsh, child dies)
            // is silently pruned server-side but iTerm2 keeps showing the
            // dead split forever.  Fixes the "exit doesn't kill the pane"
            // report on issue #261.
            let pre_reap: Vec<(usize, Option<usize>, usize)> = if !app.control_clients.is_empty() {
                app.windows.iter().map(|w| (
                    w.id,
                    tree::get_active_pane_id(&w.root, &w.active_path),
                    tree::count_panes(&w.root),
                )).collect()
            } else { Vec::new() };
            let pre_active_win_id: Option<usize> = if !app.control_clients.is_empty() && app.active_idx < app.windows.len() {
                Some(app.windows[app.active_idx].id)
            } else { None };

            let (all_empty, any_pruned, any_newly_dead) = tree::reap_children(&mut app)?;
            if any_pruned {
                // A pane was removed from the tree - resize remaining panes to fill the space
                resize_all_panes(&mut app);
                // Notify any attached control-mode clients about the diff.
                if !app.control_clients.is_empty() {
                    for (win_id, prev_active, prev_leaves) in &pre_reap {
                        if let Some(w) = app.windows.iter().find(|w| w.id == *win_id) {
                            let new_leaves = tree::count_panes(&w.root);
                            let new_active = tree::get_active_pane_id(&w.root, &w.active_path);
                            if new_leaves != *prev_leaves {
                                let layout = control::window_layout_string(w, w.area);
                                control::emit_notification(&app, crate::types::ControlNotification::LayoutChange {
                                    window_id: *win_id,
                                    layout,
                                });
                            }
                            if new_active != *prev_active {
                                if let Some(pid) = new_active {
                                    control::emit_notification(&app, crate::types::ControlNotification::WindowPaneChanged {
                                        window_id: *win_id,
                                        pane_id: pid,
                                    });
                                }
                            }
                        } else {
                            // Window completely removed (last pane died).
                            control::emit_notification(&app, crate::types::ControlNotification::WindowClose {
                                window_id: *win_id,
                            });
                        }
                    }
                    // If the session's active window changed (because the
                    // previous active window was removed), tell iTerm2.
                    if let Some(prev) = pre_active_win_id {
                        if app.active_idx < app.windows.len() {
                            let new_win_id = app.windows[app.active_idx].id;
                            if new_win_id != prev {
                                control::emit_notification(&app, crate::types::ControlNotification::SessionWindowChanged {
                                    session_id: app.session_id,
                                    window_id: new_win_id,
                                });
                            }
                        }
                    }
                }
            }
            if any_pruned || any_newly_dead {
                // A pane exited — fire hooks whether it was removed (remain-on-exit off)
                // or just marked dead (remain-on-exit on).  Fixes #227.
                state_dirty = true;
                meta_dirty = true;
                crate::commands::fire_hooks(&mut app, "pane-died");
                crate::commands::fire_hooks(&mut app, "pane-exited");
            }
            if app.exit_empty && all_empty {
                warm_debug(&format!("EXIT_EMPTY firing for session '{}' (all panes empty/dead) -> removing port file + process::exit", app.session_name));
                // Notify CC clients that the session is ending so iTerm2
                // closes the native window cleanly (same path as KillServer).
                if !app.control_clients.is_empty() {
                    control::emit_notification(
                        &app,
                        crate::types::ControlNotification::Exit { reason: None },
                    );
                    // Give notification threads time to flush %exit through
                    // the DCS stream before we tear down the process.
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
                let regpath = crate::paths::port_file(&app.port_file_base());
                let keypath = crate::paths::key_file(&app.port_file_base());
                let _ = std::fs::remove_file(&regpath);
                let _ = std::fs::remove_file(&keypath);
                crate::types::send_directive_to_all_clients("DETACH");
                std::thread::sleep(Duration::from_millis(50));
                crate::types::shutdown_persistent_streams();
                // Kill warm pane's child (process::exit skips Drop)
                if let Some(mut wp) = app.warm_pane.take() { wp.child.kill().ok(); }
                std::thread::sleep(std::time::Duration::from_millis(10));
                std::process::exit(0);
            }
        }
        // recv_timeout already handles the wait; no additional sleep needed.
    }
    #[allow(unreachable_code)]
    Ok(())
}

#[cfg(test)]
#[path = "../../tests-rs/test_issue505_rename_session_guard.rs"]
mod tests_issue505_rename_session_guard;

#[cfg(test)]
#[path = "../../tests-rs/test_issue574_rename_loop_guard.rs"]
mod tests_issue574_rename_loop_guard;

#[cfg(test)]
#[path = "../../tests-rs/test_server.rs"]
mod tests;

#[cfg(test)]
#[path = "../../tests-rs/test_issue169_manual_rename.rs"]
mod test_issue169;

#[cfg(test)]
#[path = "../../tests-rs/test_pane_title.rs"]
mod test_pane_title;

#[cfg(test)]
#[path = "../../tests-rs/test_issue202_switch_client.rs"]
mod test_issue202;

#[cfg(test)]
#[path = "../../tests-rs/test_new_session_env.rs"]
mod test_new_session_env;

#[cfg(test)]
#[cfg(windows)]
#[path = "../../tests-rs/test_issue167_startup_log.rs"]
mod test_issue167_startup_log;

#[cfg(test)]
#[path = "../../tests-rs/test_issue370_startup_error_passthrough.rs"]
mod test_issue370_startup_error_passthrough;

#[cfg(test)]
#[path = "../../tests-rs/test_issue459_warm_single_instance.rs"]
mod test_issue459_warm_single_instance;
