use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::types::{AppState, Pane, Node, LayoutKind, Window};
use crate::tree::{replace_leaf_with_split, active_pane_mut, kill_leaf};
use crate::format::hostname_cached;

/// Sentinel value for cursor_shape: means "no DECSCUSR received from child yet".
/// When ConPTY passthrough mode is unavailable, DECSCUSR sequences from child
/// processes are consumed by ConPTY and never forwarded.  Using this sentinel
/// lets the rendering code skip emitting any cursor-shape override, so the
/// real terminal keeps its user-configured default cursor.
pub const CURSOR_SHAPE_UNSET: u8 = 255;

/// Originally sent a preemptive cursor-position report (\x1b[1;1R) to the
/// ConPTY input pipe at spawn time.  Disabled in issue #313: the ConPTY is
/// created with `PSEUDOCONSOLE_WIN32_INPUT_MODE` which expects input sequences
/// ending with `_`, not `R`.  The raw VT response confused the Win32 input
/// parser, leaving it in a half-state that consumed the first user keystroke
/// and triggered a PSReadLine bell on every freshly-attached pane.
///
/// CPR queries from ConPTY (ESC\[6n) are now handled reactively by
/// `scan_cpr_query` + `drain_cpr_pending`, which respond with the correct
/// cursor position on demand.
pub fn conpty_preemptive_dsr_response(_writer: &mut dyn std::io::Write) {
    // no-op: reactive CPR responder handles all ESC[6n queries (#313)
}

/// Non-blocking pane writer: queues bytes to a dedicated flush thread.
///
/// The ConPTY input pipe has a fixed 64KB buffer. A pane child that stops
/// reading stdin (e.g. a TUI busy with a long redraw) makes a direct
/// `write_all` on the server thread block, wedging every session on the
/// server. tmux never has this problem because pty writes go through a
/// libevent bufferevent that buffers in memory and flushes asynchronously;
/// this queue mirrors that contract: writes always complete immediately,
/// bytes are delivered in order, and backpressure is absorbed by memory
/// exactly like tmux's event buffer.
struct QueuedPaneWriter {
    tx: std::sync::mpsc::Sender<Vec<u8>>,
}

impl std::io::Write for QueuedPaneWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.tx.send(buf.to_vec()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pane writer thread exited")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Wrap a raw PTY writer in a queue drained by a dedicated thread.
///
/// The inner writer's lifetime must match the pane's: dropping the ConPTY
/// master writer closes the child's input pipe, which a shell reads as EOF
/// and exits on — closing the whole window. A transient write error (e.g.
/// while a TUI child is tearing down) must therefore NOT end the thread;
/// it only stops further writes. The thread — and with it the inner
/// writer — goes away only when the queue side is dropped with the pane.
pub fn spawn_pane_write_queue(
    mut inner: Box<dyn std::io::Write + Send>,
) -> Box<dyn std::io::Write + Send> {
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let _ = std::thread::Builder::new()
        .name("pane-writer".to_string())
        .spawn(move || {
            let mut broken = false;
            while let Ok(mut buf) = rx.recv() {
                // Coalesce whatever else is already queued into one write.
                while let Ok(more) = rx.try_recv() {
                    buf.extend_from_slice(&more);
                }
                if broken {
                    continue;
                }
                if inner.write_all(&buf).is_err() {
                    broken = true;
                    continue;
                }
                let _ = inner.flush();
            }
        });
    Box::new(QueuedPaneWriter { tx })
}

/// Cached resolved shell path to avoid repeated `which::which()` PATH scans.
/// Resolved once on first use, reused for all subsequent pane spawns.
static CACHED_SHELL_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Swap an MSIX package-interior executable path for its app execution alias.
///
/// Store/MSIX packages (e.g. PowerShell installed from the Microsoft Store)
/// put their package-interior directory (`C:\Program Files\WindowsApps\<pkg>`)
/// on PATH, so `which` resolves `pwsh` to the exe INSIDE the package.
/// Launching that exe directly is unsupported: the MSIX activation that
/// CreateProcessW performs for a package-identity exe depends on the calling
/// process's environment, and under ConPTY with a redirected `USERPROFILE`
/// (test isolation, roaming setups) it fails with ACCESS_DENIED, so the pane
/// child never spawns.  The supported launch surface is the app execution
/// alias under `%LOCALAPPDATA%\Microsoft\WindowsApps`, which the loader
/// resolves in-kernel regardless of environment.  `which` cannot return the
/// alias itself: it is a zero-length APPEXECLINK reparse point that normal
/// metadata traversal cannot follow, so `which` skips it as invalid — hence
/// this post-resolution swap (probed with `symlink_metadata`, which does not
/// follow the reparse point).
#[cfg(windows)]
pub fn prefer_app_execution_alias(resolved: String) -> String {
    let lower = resolved.to_ascii_lowercase();
    // Package interior lives under `...\WindowsApps\<pkg>\...`; the alias dir
    // is `...\Microsoft\WindowsApps\` — only rewrite the former.
    if !lower.contains("\\windowsapps\\") || lower.contains("\\microsoft\\windowsapps\\") {
        return resolved;
    }
    let Some(file_name) = std::path::Path::new(&resolved).file_name() else {
        return resolved;
    };
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let alias = std::path::Path::new(&local)
            .join("Microsoft")
            .join("WindowsApps")
            .join(file_name);
        if std::fs::symlink_metadata(&alias).is_ok() {
            return alias.to_string_lossy().into_owned();
        }
    }
    resolved
}

#[cfg(not(windows))]
pub fn prefer_app_execution_alias(resolved: String) -> String {
    resolved
}

/// Get the cached shell path, resolving via `which` only on first call.
///
/// MSIX/Store-packaged executables (anywhere under `\WindowsApps\`, whether
/// the app-execution alias or the package-interior exe) cannot be activated
/// from a non-interactive SSH-spawned process: confirmed by a bare
/// `CreateProcessW` of the alias failing with ACCESS_DENIED under sshd with
/// no psmux code involved at all -- Windows' AppModel/MSIX activation
/// requires a proper interactive user session, which sshd's non-interactive
/// command execution does not provide. When this server process was itself
/// spawned from an SSH command (`SSH_CONNECTION`/`SSH_CLIENT` inherited from
/// sshd), skip a WindowsApps-hosted shell and fall back to the classic
/// (non-MSIX) `powershell.exe`, which every Windows install ships outside
/// the package store and which activates fine in any process context. This
/// turns a hard "psmux: failed to create session" for Store-only-pwsh users
/// attaching over SSH into a graceful degrade instead (issue #167 class).
pub fn cached_shell() -> Option<&'static str> {
    CACHED_SHELL_PATH.get_or_init(|| {
        let resolved = which::which("pwsh").ok()
            .or_else(|| which::which("powershell").ok())
            .or_else(|| which::which("cmd").ok())
            .map(|p| prefer_app_execution_alias(p.to_string_lossy().into_owned()));
        if let Some(ref path) = resolved {
            let is_store = path.to_ascii_lowercase().contains("\\windowsapps\\");
            let is_ssh_spawned = std::env::var("SSH_CONNECTION").is_ok()
                || std::env::var("SSH_CLIENT").is_ok();
            if is_store && is_ssh_spawned {
                if let Ok(classic) = which::which("powershell") {
                    let classic = classic.to_string_lossy().into_owned();
                    if !classic.to_ascii_lowercase().contains("\\windowsapps\\") {
                        return Some(classic);
                    }
                }
                // No classic PowerShell found; cmd.exe is never MSIX-packaged.
                if let Ok(cmd) = which::which("cmd") {
                    return Some(cmd.to_string_lossy().into_owned());
                }
            }
        }
        resolved
    }).as_deref()
}

/// Strip the explicit-argv marker from a command string for naming purposes.
///
/// `new-window -- prog args...` reaches here as `"-- prog args..."`: the
/// server keeps the `--` marker so `build_command` knows to exec the argv
/// directly (#582).  Naming code must skip the marker, or the window ends up
/// called `--` instead of the program (`build_command` decodes it, this used
/// not to).  `--` with no tail means "just the default shell", so `None`.
fn command_text_for_naming(cmd: &str) -> Option<&str> {
    let trimmed = cmd.trim_start();
    match trimmed.strip_prefix("--") {
        Some(rest) if rest.is_empty() => None,
        Some(rest) if rest.starts_with(char::is_whitespace) => {
            let tail = rest.trim();
            if tail.is_empty() { None } else { Some(tail) }
        }
        // e.g. "--foo": not the argv marker, an ordinary command string.
        _ => Some(cmd),
    }
}

/// Determine the default shell name for window naming (like tmux shows "bash", "zsh").
fn default_shell_name(command: Option<&str>, configured_shell: Option<&str>) -> String {
    let command = command.and_then(command_text_for_naming);
    if let Some(cmd) = command {
        // Extract the program name from the command string (space-aware)
        let (prog, _) = resolve_shell_program(cmd);
        std::path::Path::new(&prog)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(cmd)
            .to_string()
    } else if let Some(shell) = configured_shell {
        // Use configured default-shell name (space-aware)
        let (prog, _) = resolve_shell_program(shell);
        std::path::Path::new(&prog)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(shell)
            .to_string()
    } else {
        // Default shell — use cached resolved path
        cached_shell()
            .and_then(|p| std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "shell".into())
    }
}

/// Does this `default-shell`/`default-command` string need to be evaluated
/// fresh at consume time rather than transplanted from a pre-spawned warm
/// pane? True when it contains a format token (e.g. `#{pane_current_path}`):
/// that value depends on which pane is active *right now*, at the moment
/// new-window/split-window runs, so only a cold spawn's `expand_format` call
/// (which runs then, not at warm-pane pre-spawn time) can resolve it
/// correctly. A static command with no format tokens is fine to transplant —
/// `warm_pane_sync` already respawns the pool with that exact command on
/// change, so the pre-spawned process already matches what a cold spawn
/// would produce.
pub(crate) fn default_shell_needs_fresh_eval(default_shell: &str) -> bool {
    default_shell.contains("#{")
}

/// Command syntax a pane's shell understands for the injected rehome line.
///
/// The rehome is typed into an *already running* shell, so the snippet has to
/// parse in the language that shell speaks. Keying it on the host OS instead
/// of the shell is what broke Git Bash panes on Windows (#600): the PowerShell
/// form was typed into bash, which answered with
/// `bash: syntax error near unexpected token '('` and never changed directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RehomeSyntax {
    /// pwsh / powershell.
    PowerShell,
    /// bash, sh, zsh, fish, dash, ksh, csh and friends.
    Posix,
    /// cmd.exe.
    Cmd,
}

/// Pick the rehome syntax from the shell that is actually running in the pane.
///
/// `shell` is the configured `default-shell` (possibly a whole command line
/// with arguments, possibly empty). Empty means psmux spawned its own default
/// shell, which is pwsh/powershell on Windows and a POSIX shell elsewhere.
/// An unrecognised program keeps the platform default rather than guessing.
pub(crate) fn rehome_syntax_for_shell(shell: &str) -> RehomeSyntax {
    let platform_default = if cfg!(windows) { RehomeSyntax::PowerShell } else { RehomeSyntax::Posix };
    let shell = shell.trim();
    if shell.is_empty() {
        return platform_default;
    }
    let (program, _) = resolve_shell_program(shell);
    let program = remap_git_bash_launcher(program);
    let stem = std::path::Path::new(&program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program.as_str())
        .to_ascii_lowercase();
    if POSIX_SHELL_STEMS.contains(&stem.as_str()) || stem == "ash" || stem == "busybox" {
        RehomeSyntax::Posix
    } else if stem == "cmd" {
        RehomeSyntax::Cmd
    } else if stem == "pwsh" || stem == "powershell" {
        RehomeSyntax::PowerShell
    } else {
        platform_default
    }
}

/// Build the command injected to silently re-home a pane's shell to `dir`.
///
/// The trailing clear (`cls` for PowerShell and cmd, `clear` for POSIX shells)
/// wipes the visible echo and its CSI 2J is what ends the render squelch; the
/// trailing `\r` submits it as one command line. The leading space asks shells
/// that ignore space-prefixed commands to skip the history entry (best-effort,
/// not every shell honours it).
///
/// Quoting is per shell: PowerShell doubles an embedded single quote, POSIX
/// shells use the `'\''` idiom (a single-quoted string cannot contain an
/// escape), and cmd wraps in double quotes (a Windows path cannot contain one).
///
/// The PowerShell form also explicitly syncs the OS-level current directory via
/// `[System.IO.Directory]::SetCurrentDirectory`. PowerShell's `cd`/`Set-Location`
/// updates its own `$PWD` provider location but does NOT call Win32
/// `SetCurrentDirectory()`, so the process's PEB (which is what
/// `#{pane_current_path}` reads via a PEB walk) keeps reporting the
/// directory the shell was originally spawned in. Normally psmux's own
/// CWD_SYNC profile hook (`build_psrl_init`) papers over this by wrapping
/// `Set-Location`, but that hook is skipped whenever `-NoProfile` is in
/// effect (see `build_default_shell`). Embedding the sync directly in the
/// injected command keeps the rehome correct regardless of whether that
/// hook is installed. `cd` in a POSIX shell and `cd /d` in cmd both move the
/// process CWD themselves, so neither needs an equivalent.
pub(crate) fn rehome_command(dir: &str, syntax: RehomeSyntax) -> String {
    match syntax {
        RehomeSyntax::PowerShell => {
            let escaped = dir.replace('\'', "''");
            format!(
                " cd '{}'; try {{ [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) }} catch {{}}; cls\r",
                escaped
            )
        }
        RehomeSyntax::Posix => {
            // A Windows path reaches a POSIX shell (Git Bash, MSYS2, Cygwin)
            // with backslashes; forward slashes are accepted by the same
            // shells and leave nothing for the reader to misparse. Off
            // Windows a backslash is an ordinary filename character, so the
            // path is passed through untouched.
            let dir = if cfg!(windows) { dir.replace('\\', "/") } else { dir.to_string() };
            let escaped = dir.replace('\'', r"'\''");
            format!(" cd '{}'; clear\r", escaped)
        }
        RehomeSyntax::Cmd => {
            let escaped = dir.replace('"', "");
            format!(" cd /d \"{}\" & cls\r", escaped)
        }
    }
}

/// Silently re-home an already-running pane's shell to `dir`: inject
/// [`rehome_command`] and squelch the pane's rendering until the resulting
/// screen-clear (CSI 2J) fires or a 500ms safety timeout elapses, so the user
/// never sees the injected command or its echo.
///
/// A pre-spawned shell (warm pane or warm server) starts in the server CWD;
/// since a running process's CWD cannot be set externally, the shell moves
/// itself with `cd`. The shell must be at a fresh prompt so the injected line
/// runs immediately.
///
/// `syntax` must describe the shell actually running in `pane` (derive it with
/// [`rehome_syntax_for_shell`] from the effective `default-shell`); typing the
/// wrong dialect leaves stray text on screen and the pane in the wrong
/// directory (#600).
pub(crate) fn silent_rehome(pane: &mut Pane, dir: &str, syntax: RehomeSyntax) {
    use std::io::Write as _;
    let cd_cmd = rehome_command(dir, syntax);
    // Tell the vt100 parser to watch for the next screen-clear (CSI 2J/3J);
    // its arrival tells the layout serialiser the clear finished (event-driven).
    if let Ok(mut parser) = pane.term.lock() {
        parser.screen_mut().set_squelch_clear_pending(true);
    }
    // Reveal the pane when that clear arrives, or after 500ms as a fallback for
    // shells that never emit one — whichever happens first.
    pane.squelch_until = Some(Instant::now() + Duration::from_millis(500));
    let _ = pane.writer.write_all(cd_cmd.as_bytes());
    let _ = pane.writer.flush();
}

pub fn create_window(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, command: Option<&str>, start_dir: Option<&str>, empty: bool) -> io::Result<()> {
    create_window_with_env(pty_system, app, command, start_dir, empty, &[])
}

/// `create_window` plus per-pane environment from `new-window -e KEY=VALUE`
/// (tmux parity, issue #489). The extra variables are applied to the spawned
/// process only, after the session environment, so `-e` wins over
/// `set-environment`.
pub fn create_window_with_env(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, command: Option<&str>, start_dir: Option<&str>, empty: bool, extra_env: &[(String, String)]) -> io::Result<()> {
    // ── Empty window (tmux new-window -E): a new window whose single pane has
    // no command/process. It renders blank until respawn-pane gives it one. ──
    if empty {
        let area = app.client_area;
        let rows = (if area.height > 1 { area.height } else { 30 }).max(MIN_PANE_DIM);
        let cols = (if area.width > 1 { area.width } else { 120 }).max(MIN_PANE_DIM);
        let pane_id = app.next_pane_id;
        if let Some(pane) = crate::popup::create_empty_pane(rows, cols, pane_id) {
            app.next_pane_id += 1;
            app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: hostname_cached(), id: app.next_win_id, area: app.client_area, window_size: None, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
            app.next_win_id += 1;
            app.active_idx = app.windows.len() - 1;
            app.on_window_appended();
        }
        return Ok(());
    }
    // ── Fast path: use pre-spawned warm pane when creating a default shell ──
    // The warm pane has its shell already loaded (~470ms for pwsh), so the
    // prompt appears instantly — matching wezterm's "instant tab" feel.
    // A `-c <dir>` is honoured by re-homing the transplanted shell (below), so
    // the warm pane is used even when start_dir is set (#107).
    //
    // Gated on `!default_shell_needs_fresh_eval(&app.default_shell)`: a
    // *static* custom `default-shell`/`default-command` is fine to
    // transplant — `for_post_config`/`for_option_change` already respawn the
    // warm pane with that exact command, so the pre-spawned process is
    // semantically identical to a fresh cold spawn (cwd correction still
    // happens below via `silent_rehome`). But a *dynamic* one containing a
    // format token like `#{pane_current_path}` cannot be satisfied by
    // transplanting an already-running process at all — the value has to be
    // resolved fresh, at consume time, against whichever pane is currently
    // active, which only the cold path's `expand_format` call does. Bypass
    // to cold-spawn in that case; mirrors new-session's own warm-claim gate,
    // which likewise skips warm entirely when a custom config is in play
    // (see `has_custom_config` in main.rs).
    // A warm pane's shell is already running, so `-e` vars can no longer be
    // injected into its environment — bypass the transplant when -e is used.
    if command.is_none() && extra_env.is_empty() && !default_shell_needs_fresh_eval(&app.default_shell) && app.warm_pane.is_some() {
        let mut wp = app.warm_pane.take().unwrap();
        // Resize to current terminal dimensions if they changed since pre-spawn
        let area = app.client_area;
        let rows = if area.height > 1 { area.height } else { 30 }.max(MIN_PANE_DIM);
        let cols = if area.width > 1 { area.width } else { 120 }.max(MIN_PANE_DIM);
        let need_resize = rows != wp.rows || cols != wp.cols;
        // #450: the spare shell can die while idling in the pool (shell
        // crash, external kill, dead conhost).  Transplanting the corpse
        // yields a broken empty window that the reaper prunes one tick
        // later — the user sees prefix+c "open nothing" or flash a dead
        // pane.  Gate the transplant on child liveness; a failed ConPTY
        // resize doubles as a dead-conhost probe.  On a dead spare, fall
        // through to the synchronous cold-spawn path below.
        let mut live = warm_pane_is_live(&mut wp);
        if live && need_resize {
            let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
            live = wp.master.resize(size).is_ok();
        }
        if live {
            // Reconcile parser dimensions and scrollback cap.  The cap
            // sync is the consume-time safety net for #271 — even if a
            // future caller forgets to invoke warm_pane_sync on a state
            // change, the parser is brought to the live value here.
            if let Ok(mut parser) = wp.term.lock() {
                if need_resize {
                    parser.screen_mut().set_size(rows, cols);
                }
                crate::warm_pane_sync::reconcile_consumed_parser(&mut parser, app);
            }
            let epoch = std::time::Instant::now() - Duration::from_secs(2);
            let configured_shell = if app.default_shell.is_empty() { None } else { Some(app.default_shell.as_str()) };
            let mut pane = Pane { master: wp.master, writer: wp.writer, child: wp.child, term: wp.term, last_rows: rows, last_cols: cols, id: wp.pane_id, title: hostname_cached(), title_locked: false, child_pid: wp.child_pid, data_version: wp.data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, scroll_fg_cache: None, mouse_proto_owner: None, cursor_shape: wp.cursor_shape, bell_pending: wp.bell_pending, cpr_pending: wp.cpr_pending, color_query_pending: wp.color_query_pending, copy_state: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring: wp.output_ring, spawned_at: Some(std::time::Instant::now()) };
            // Honour `-c <dir>`: silently re-home the transplanted warm shell.
            // The snippet has to be written in the dialect of the shell the
            // warm pane is actually running, which is whatever `default-shell`
            // was when the pool spawned it (#600).
            if let Some(dir) = start_dir {
                let syntax = rehome_syntax_for_shell(configured_shell.unwrap_or(""));
                silent_rehome(&mut pane, dir, syntax);
            }
            let win_name = default_shell_name(None, configured_shell);
            let initial_pane_id = wp.pane_id;
            app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![initial_pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
            app.next_win_id += 1;
            app.active_idx = app.windows.len() - 1;
            app.on_window_appended();
            return Ok(());
        }
        // Dead spare: make sure the corpse is fully gone, then cold-spawn.
        wp.child.kill().ok();
    }
    // ── Normal path: spawn a new ConPTY + shell synchronously ──
    // Use actual terminal size if known, otherwise fall back to defaults
    let area = app.client_area;
    let rows = if area.height > 1 { area.height } else { 30 }.max(MIN_PANE_DIM);
    let cols = if area.width > 1 { area.width } else { 120 }.max(MIN_PANE_DIM);
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;

    // When no explicit command is given, use the configured default-shell
    // (from `set -g default-shell` / `default-command`).
    // Expand format variables like #{pane_current_path} at spawn time (#111).
    let expanded_shell = crate::format::expand_format(&app.default_shell, app);
    let mut shell_cmd = if command.is_some() {
        build_command(command, app.env_shim, app.allow_predictions)
    } else if !expanded_shell.is_empty() {
        build_default_shell(&expanded_shell, app.env_shim, app.allow_predictions)
    } else {
        build_command(None, app.env_shim, app.allow_predictions)
    };
    // Override CWD if -c start_dir was specified. Route it through
    // usable_start_dir so a UNC or since-deleted directory falls back to home
    // instead of killing the pane shell at spawn time — see that function for
    // why this belongs here rather than in each caller.
    if let Some(dir) = start_dir {
        if let Some(usable) = crate::util::usable_start_dir(dir) {
            shell_cmd.cwd(usable);
        }
    }
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    apply_user_environment(&mut shell_cmd, &app.environment);
    // new-window -e KEY=VALUE (#489): pane-scoped env, applied last so it
    // overrides the session environment, matching tmux.
    for (k, v) in extra_env { shell_cmd.env(k, v); }
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // On Windows ConPTY the slave handle MUST be closed after spawning so the
    // child owns the sole reference to the console input pipe.  Leaving it open
    // causes "The handle is invalid" IOExceptions inside the child process.
    drop(pair.slave);

    let scrollback = app.history_limit as u32;
    let mut parser = vt100::Parser::new(size.rows, size.cols, scrollback as usize);
    parser.screen_mut().set_allow_alternate_screen(app.allow_alternate_screen);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(parser));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<u8>::new()));
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), app.next_pane_id, child_pid);

    let configured_shell = if app.default_shell.is_empty() { None } else { Some(app.default_shell.as_str()) };
    let mut pty_writer = spawn_pane_write_queue(pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let epoch = std::time::Instant::now() - Duration::from_secs(2);
    let pane_id = app.next_pane_id;
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, scroll_fg_cache: None, mouse_proto_owner: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()) };
    app.next_pane_id += 1;
    let win_name = command.map(|c| default_shell_name(Some(c), None)).unwrap_or_else(|| default_shell_name(None, configured_shell));
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    app.on_window_appended();
    Ok(())
}

/// #450: is the pooled spare shell still alive?  A warm pane can die while
/// idling — the shell can crash (e.g. pwsh FailFast on a broken console
/// read), be killed externally, or lose its conhost.  `Ok(None)` from
/// `try_wait` is the only state in which a transplant is safe; both a
/// reported exit and a wait error mean the child is unusable.
pub fn warm_pane_is_live(wp: &mut crate::types::WarmPane) -> bool {
    matches!(wp.child.try_wait(), Ok(None))
}

/// Pre-spawn a shell in the background so the next `new-window` (default shell,
/// no custom command) can transplant it instantly.  The returned `WarmPane` has
/// its reader thread already running — by the time the user creates a new window
/// (typically 500ms+), pwsh will have fully loaded its profile and the prompt
/// is ready.
pub fn spawn_warm_pane(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState) -> io::Result<crate::types::WarmPane> {
    if !app.warm_enabled {
        return Err(io::Error::new(io::ErrorKind::Other, "warm panes disabled"));
    }
    let area = app.client_area;
    let rows = if area.height > 1 { area.height } else { 30 }.max(MIN_PANE_DIM);
    let cols = if area.width > 1 { area.width } else { 120 }.max(MIN_PANE_DIM);
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    // Expand format variables like #{pane_current_path} at spawn time (#111).
    let expanded_shell = crate::format::expand_format(&app.default_shell, app);
    let mut shell_cmd = if !expanded_shell.is_empty() {
        build_default_shell(&expanded_shell, app.env_shim, app.allow_predictions)
    } else {
        build_command(None, app.env_shim, app.allow_predictions)
    };
    let pane_id = app.next_pane_id;
    app.next_pane_id += 1;
    set_tmux_env(&mut shell_cmd, pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    apply_user_environment(&mut shell_cmd, &app.environment);
    let child = pair.slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    drop(pair.slave);
    let scrollback = app.history_limit as u32;
    let mut parser = vt100::Parser::new(rows, cols, scrollback as usize);
    parser.screen_mut().set_allow_alternate_screen(app.allow_alternate_screen);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(parser));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();
    let reader = pair.master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<u8>::new()));
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), pane_id, child_pid);
    let mut pty_writer = spawn_pane_write_queue(pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    conpty_preemptive_dsr_response(&mut *pty_writer);
    Ok(crate::types::WarmPane { master: pair.master, writer: pty_writer, child, term, data_version, cursor_shape, bell_pending, cpr_pending, color_query_pending, child_pid, pane_id, rows, cols, output_ring })
}

pub fn split_active(app: &mut AppState, kind: LayoutKind) -> io::Result<()> {
    split_active_with_command(app, kind, None, None, None)
}

/// Create a new window with a raw command (program + args, no shell wrapping)
pub fn create_window_raw(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, raw_args: &[String]) -> io::Result<()> {
    let area = app.client_area;
    let rows = if area.height > 1 { area.height } else { 30 };
    let cols = if area.width > 1 { area.width } else { 120 };
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;

    let mut shell_cmd = build_raw_command(raw_args);
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    apply_user_environment(&mut shell_cmd, &app.environment);
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – see create_window() comment.
    drop(pair.slave);

    let scrollback = app.history_limit;
    let mut parser = vt100::Parser::new(size.rows, size.cols, scrollback);
    parser.screen_mut().set_allow_alternate_screen(app.allow_alternate_screen);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(parser));
    let term_reader = term.clone();
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;

    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<u8>::new()));
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), app.next_pane_id, child_pid);

    let mut pty_writer = spawn_pane_write_queue(pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let epoch = std::time::Instant::now() - Duration::from_secs(2);
    let raw_pane_id = app.next_pane_id;
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: raw_pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, scroll_fg_cache: None, mouse_proto_owner: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()) };
    app.next_pane_id += 1;
    let win_name = std::path::Path::new(&raw_args[0]).file_stem().and_then(|s| s.to_str()).unwrap_or(&raw_args[0]).to_string();
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![raw_pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    app.on_window_appended();
    Ok(())
}

/// Minimum pane dimension (rows or cols) — ConPTY on Windows crashes
/// the child process if either dimension is less than 2.
pub const MIN_PANE_DIM: u16 = 2;

/// Minimum rows for a split to be allowed — each resulting pane needs at
/// least this many rows to run a shell prompt.
const MIN_SPLIT_ROWS: u16 = 2;
/// Minimum cols for a split to be allowed.
const MIN_SPLIT_COLS: u16 = 10;

pub fn split_active_with_command(app: &mut AppState, kind: LayoutKind, command: Option<&str>, pty_system_ref: Option<&dyn portable_pty::PtySystem>, start_dir: Option<&str>) -> io::Result<()> {
    split_active_with_env(app, kind, command, pty_system_ref, start_dir, &[])
}

/// `split_active_with_command` plus per-pane environment from
/// `split-window -e KEY=VALUE` (tmux parity, issue #489).
pub fn split_active_with_env(app: &mut AppState, kind: LayoutKind, command: Option<&str>, pty_system_ref: Option<&dyn portable_pty::PtySystem>, start_dir: Option<&str>, extra_env: &[(String, String)]) -> io::Result<()> {
    // ── Guard: refuse split if the active pane is too small ──────────
    // After splitting, each half gets roughly (dim / 2) - 1 (for the divider).
    // If that would be below MIN_PANE_DIM, deny the split to avoid crashing
    // the child process (ConPTY cannot function below ~2 rows or cols).
    {
        let win = &app.windows[app.active_idx];
        if let Some(p) = crate::tree::active_pane(&win.root, &win.active_path) {
            let (cur_rows, cur_cols) = (p.last_rows, p.last_cols);
            match kind {
                LayoutKind::Vertical => {
                    // Splitting vertically divides height; need room for 2 panes + 1 divider
                    if cur_rows < MIN_SPLIT_ROWS * 2 + 1 {
                        return Err(io::Error::new(io::ErrorKind::Other,
                            format!("pane too small to split vertically ({cur_rows} rows, need {})", MIN_SPLIT_ROWS * 2 + 1)));
                    }
                }
                LayoutKind::Horizontal => {
                    // Splitting horizontally divides width; need room for 2 panes + 1 divider
                    if cur_cols < MIN_SPLIT_COLS * 2 + 1 {
                        return Err(io::Error::new(io::ErrorKind::Other,
                            format!("pane too small to split horizontally ({cur_cols} cols, need {})", MIN_SPLIT_COLS * 2 + 1)));
                    }
                }
            }
        }
    }

    // Reuse provided PTY system or create one as fallback
    let owned_pty;
    let pty_system: &dyn portable_pty::PtySystem = if let Some(ps) = pty_system_ref {
        ps
    } else {
        owned_pty = native_pty_system();
        &*owned_pty
    };
    // Compute target pane size from the *active pane's* actual dimensions,
    // not the full window area — ensures we don't over-estimate and then
    // immediately resize to a tiny rect.
    let (pane_rows, pane_cols) = {
        let win = &app.windows[app.active_idx];
        if let Some(p) = crate::tree::active_pane(&win.root, &win.active_path) {
            (p.last_rows, p.last_cols)
        } else {
            let area = app.last_window_area;
            (if area.height > 1 { area.height } else { 30 }, if area.width > 1 { area.width } else { 120 })
        }
    };
    let (rows, cols) = match kind {
        LayoutKind::Vertical => {
            let half = (pane_rows.saturating_sub(1)) / 2; // subtract 1 for divider
            (half.max(MIN_PANE_DIM), pane_cols.max(MIN_PANE_DIM))
        }
        LayoutKind::Horizontal => {
            let half = (pane_cols.saturating_sub(1)) / 2;
            (pane_rows.max(MIN_PANE_DIM), half.max(MIN_PANE_DIM))
        }
    };
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };

    // ── Fast path: transplant warm pane for default-shell splits ─────
    // The warm pane has its shell already loaded (~470ms for pwsh).  Even
    // though its ConPTY was created at full-window size, resizing to the
    // split dimensions only costs a ConPTY repaint (~10-50ms) vs a full
    // cold spawn (~500ms).  Net result: split feels nearly instant.
    // A `-c <dir>` is honoured by re-homing the transplanted shell (below), so
    // the warm pane is used even when start_dir is set (#107).
    //
    // Gated on `!default_shell_needs_fresh_eval(...)` — see the matching
    // comment in `create_window` for why only a *dynamic* default-command
    // (format-variable-bearing) must bypass the transplant and cold-spawn
    // instead; a static custom default-shell is safe to transplant.
    // A warm pane's shell is already running, so `-e` vars can no longer be
    // injected into its environment — bypass the transplant when -e is used.
    if command.is_none() && extra_env.is_empty() && !default_shell_needs_fresh_eval(&app.default_shell) && app.warm_pane.is_some() {
        let mut wp = app.warm_pane.take().unwrap();
        let need_resize = rows != wp.rows || cols != wp.cols;
        // #450: never transplant a spare whose shell died in the pool —
        // see the matching gate in create_window.  Fall through to the
        // cold-spawn path below instead.
        let mut live = warm_pane_is_live(&mut wp);
        if live && need_resize {
            let sz = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
            live = wp.master.resize(sz).is_ok();
        }
        if live {
            // Same consume-time reconciliation as create_window — see
            // warm_pane_sync::reconcile_consumed_parser.
            if let Ok(mut parser) = wp.term.lock() {
                if need_resize {
                    parser.screen_mut().set_size(rows, cols);
                }
                crate::warm_pane_sync::reconcile_consumed_parser(&mut parser, app);
            }
            let epoch = std::time::Instant::now() - Duration::from_secs(2);
            let new_pane_id = wp.pane_id;
            let mut new_pane = Pane { master: wp.master, writer: wp.writer, child: wp.child, term: wp.term, last_rows: rows, last_cols: cols, id: new_pane_id, title: hostname_cached(), title_locked: false, child_pid: wp.child_pid, data_version: wp.data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, scroll_fg_cache: None, mouse_proto_owner: None, cursor_shape: wp.cursor_shape, bell_pending: wp.bell_pending, cpr_pending: wp.cpr_pending, color_query_pending: wp.color_query_pending, copy_state: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring: wp.output_ring, spawned_at: Some(std::time::Instant::now()) };
            // Honour `-c <dir>`: silently re-home the transplanted warm shell,
            // in the dialect that shell speaks (#600).
            if let Some(dir) = start_dir {
                let syntax = rehome_syntax_for_shell(&app.default_shell);
                silent_rehome(&mut new_pane, dir, syntax);
            }
            let new_leaf = Node::Leaf(new_pane);
            let win = &mut app.windows[app.active_idx];
            replace_leaf_with_split(&mut win.root, &win.active_path, kind, new_leaf);
            let mut new_path = win.active_path.clone();
            new_path.push(1);
            win.active_path = new_path;
            // Add new pane to MRU (most recent)
            crate::tree::touch_mru(&mut win.pane_mru, new_pane_id);
            return Ok(());
        }
        // Dead spare: make sure the corpse is fully gone, then cold-spawn.
        wp.child.kill().ok();
    }

    // ── Normal path: cold-spawn a new ConPTY + shell ────────────────
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    // When no explicit command is given, use the configured default-shell.
    // Expand format variables like #{pane_current_path} at spawn time (#111).
    let expanded_shell = crate::format::expand_format(&app.default_shell, app);
    let mut shell_cmd = if command.is_some() {
        build_command(command, app.env_shim, app.allow_predictions)
    } else if !expanded_shell.is_empty() {
        build_default_shell(&expanded_shell, app.env_shim, app.allow_predictions)
    } else {
        build_command(None, app.env_shim, app.allow_predictions)
    };
    // Override CWD if -c start_dir was specified. Route it through
    // usable_start_dir so a UNC or since-deleted directory falls back to home
    // instead of killing the pane shell at spawn time — see that function for
    // why this belongs here rather than in each caller.
    if let Some(dir) = start_dir {
        if let Some(usable) = crate::util::usable_start_dir(dir) {
            shell_cmd.cwd(usable);
        }
    }
    set_tmux_env(&mut shell_cmd, app.next_pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    apply_user_environment(&mut shell_cmd, &app.environment);
    // split-window -e KEY=VALUE (#489): pane-scoped env, applied last so it
    // overrides the session environment, matching tmux.
    for (k, v) in extra_env { shell_cmd.env(k, v); }
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – see create_window() comment.
    drop(pair.slave);
    let mut parser = vt100::Parser::new(size.rows, size.cols, app.history_limit);
    parser.screen_mut().set_allow_alternate_screen(app.allow_alternate_screen);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(parser));
    let term_reader = term.clone();
    let reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();
    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::<u8>::new()));
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), app.next_pane_id, child_pid);
    let mut pty_writer = pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?;
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let epoch = std::time::Instant::now() - Duration::from_secs(2);
    let split_pane_id = app.next_pane_id;
    let new_leaf = Node::Leaf(Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: split_pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, scroll_fg_cache: None, mouse_proto_owner: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()) });
    app.next_pane_id += 1;
    let win = &mut app.windows[app.active_idx];
    replace_leaf_with_split(&mut win.root, &win.active_path, kind, new_leaf);
    let mut new_path = win.active_path.clone();
    new_path.push(1);
    win.active_path = new_path;
    // Add new pane to MRU (most recent)
    crate::tree::touch_mru(&mut win.pane_mru, split_pane_id);
    Ok(())
}

fn kill_pane_at_path(win: &mut Window, path: &Vec<usize>) {
    // Get the ID of the pane being killed (for MRU removal)
    let killed_id = crate::tree::get_active_pane_id(&win.root, path);
    // Collect ordered pane IDs before kill for prev-by-index fallback (#71).
    let ordered_ids_before = crate::tree::collect_pane_ids(&win.root);
    // Explicitly kill the target pane's process tree FIRST.
    // remove_node() doesn't call kill_node() when the root is a single Leaf,
    // so we must do it here to ensure no orphaned processes.
    if let Some(p) = active_pane_mut(&mut win.root, path) {
        crate::platform::process_kill::kill_process_tree(&mut p.child);
    }
    kill_leaf(&mut win.root, path);
    // Remove killed pane from MRU
    if let Some(kid) = killed_id {
        crate::tree::remove_from_mru(&mut win.pane_mru, kid);
    }
    // Focus the most recently used remaining pane (tmux parity #71).
    // Walk the MRU list and pick the first pane that still exists.
    let mru_target = win.pane_mru.iter()
        .find_map(|&id| crate::tree::find_path_by_id(&win.root, id));
    // Fallback when MRU is empty (all remaining panes unvisited):
    // tmux picks previous pane by pane_index, or next if no previous.
    let fallback = || {
        if let Some(kid) = killed_id {
            let pos = ordered_ids_before.iter().position(|&id| id == kid);
            if let Some(pos) = pos {
                // Try previous by index first, then next
                let prev_id = if pos > 0 { Some(ordered_ids_before[pos - 1]) } else { None };
                let next_id = ordered_ids_before.get(pos + 1).copied();
                let candidate = prev_id.or(next_id);
                if let Some(cid) = candidate {
                    if let Some(path) = crate::tree::find_path_by_id(&win.root, cid) {
                        return path;
                    }
                }
            }
        }
        crate::tree::first_leaf_path(&win.root)
    };
    win.active_path = mru_target.unwrap_or_else(fallback);
}

pub fn kill_active_pane(app: &mut AppState) -> io::Result<()> {
    let win = &mut app.windows[app.active_idx];
    let active_path = win.active_path.clone();
    kill_pane_at_path(win, &active_path);
    Ok(())
}

pub fn kill_pane_by_id(app: &mut AppState, pane_id: usize) -> io::Result<()> {
    let restore_idx = app.active_idx;
    let restore_path = app.windows[restore_idx].active_path.clone();
    let restore_pane_id = crate::tree::get_active_pane_id(&app.windows[restore_idx].root, &restore_path);

    let target = app.windows.iter().enumerate().find_map(|(wi, win)| {
        crate::tree::find_path_by_id(&win.root, pane_id).map(|path| (wi, path))
    });

    let Some((target_idx, target_path)) = target else {
        return Ok(());
    };

    {
        let win = &mut app.windows[target_idx];
        kill_pane_at_path(win, &target_path);
    }

    // Only restore focus when the killed pane was in a DIFFERENT window.
    // For same-window kills, kill_pane_at_path already set the correct
    // MRU-based focus.  The old restore logic used path_exists() which
    // can succeed on stale indices that now point to a different pane
    // after tree restructuring (issue #140).
    if restore_idx < app.windows.len() && target_idx != restore_idx {
        app.active_idx = restore_idx;
        let restore_win = &mut app.windows[restore_idx];
        let resolved_restore_path = restore_pane_id
            .and_then(|id| crate::tree::find_path_by_id(&restore_win.root, id))
            .unwrap_or_else(|| crate::tree::first_leaf_path(&restore_win.root));
        restore_win.active_path = resolved_restore_path;
    }

    Ok(())
}

pub fn detect_shell() -> CommandBuilder {
    build_command(None, false, false)
}

/// Issue #167 escape hatch.  When the user sets `PSMUX_BARE_ENV=1`, replace
/// the inherited environment block on `builder` with the minimum set Windows
/// needs to launch a working pwsh process.  Useful when the parent's
/// environment is the cause of `CreateProcessW err 87` (e.g. Microsoft-account
/// profiles where OneDrive + WindowsApps inflate the env block close to
/// Windows's 32 KB hard limit, or where a single env var contains content
/// that the OS rejects).
///
/// Returns `true` when the bare-env path was taken (so callers can log).
/// Idempotent: calling it twice is safe.
pub fn apply_bare_env_if_set(builder: &mut CommandBuilder) -> bool {
    let on = std::env::var("PSMUX_BARE_ENV")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !on {
        return false;
    }
    builder.env_clear();
    // Re-add only what is genuinely required for a usable shell.  Anything
    // missing here is something the user explicitly opted out of by
    // setting PSMUX_BARE_ENV — psmux itself will fill in TERM/COLORTERM/
    // PSMUX_SESSION/TMUX afterwards via build_command + set_tmux_env.
    for key in [
        "SYSTEMROOT", "SYSTEMDRIVE", "WINDIR",
        "USERPROFILE", "USERNAME", "HOMEDRIVE", "HOMEPATH",
        "COMPUTERNAME", "COMSPEC", "PATH", "PATHEXT",
        "TEMP", "TMP",
        "PROCESSOR_ARCHITECTURE",
    ] {
        if let Ok(v) = std::env::var(key) {
            builder.env(key, v);
        }
    }
    true
}

/// Hand the real terminal's colors down to a child that psmux itself will draw.
///
/// A psmux client started in a pane or popup cannot ask its terminal what the
/// colors are, because its terminal is psmux and the reply would arrive as
/// injected console input long after the client stopped draining
/// (`platform::query_host_terminal_colors`).  Planting the parent's already
/// known values here keeps the nested client's palette correct with no query on
/// the wire.  Clearing the variable when the parent knows nothing is what stops
/// a stale palette from outliving the terminal it was measured on.
pub fn set_host_colors_env(builder: &mut CommandBuilder, host_colors: Option<&crate::types::HostColors>) {
    match host_colors {
        Some(hc) if hc.has_any() || hc.dark.is_some() => {
            builder.env("PSMUX_HOST_COLORS", hc.to_spec());
        }
        _ => builder.env_remove("PSMUX_HOST_COLORS"),
    }
}

/// Set TMUX, TMUX_PANE, and PSMUX_SESSION environment variables on a CommandBuilder.
/// TMUX format: /tmp/psmux-{server_pid}/{socket_name},{port},0
/// TMUX_PANE format: %{pane_id}
/// PSMUX_SESSION: actual session name (for Claude Code / tool detection)
/// The socket_name component encodes the -L namespace for child process resolution.
pub fn set_tmux_env(builder: &mut CommandBuilder, pane_id: usize, control_port: Option<u16>, socket_name: Option<&str>, session_name: &str, fix_tty: bool, _force_interactive: bool) {
    let server_pid = std::process::id();
    let port = control_port.unwrap_or(0);
    let sn = socket_name.unwrap_or("default");
    // Format compatible with tmux: <socket_path>,<pid>,<session_idx>
    // We encode the socket name in the path component for -L namespace resolution
    builder.env("TMUX", format!("/tmp/psmux-{}/{},{},0", server_pid, sn, port));
    builder.env("TMUX_PANE", format!("%{}", pane_id));
    // Override the placeholder "1" from build_command/build_default_shell with the
    // real session name.  Tools like Claude Code can use PSMUX_SESSION for explicit
    // psmux detection (e.g. `if (process.env.PSMUX_SESSION) return 'psmux'`).
    builder.env("PSMUX_SESSION", session_name);
    // This child IS a window pane, so it must never inherit the popup marker a
    // server started from inside a popup would still be carrying (#537).
    builder.env_remove(crate::util::POPUP_CHILD_ENV);
    // Prevent MSYS2/Git-Bash from path-mangling the TMUX value (which starts
    // with /tmp/ and would be rewritten to a Windows path otherwise).
    builder.env("MSYS2_ENV_CONV_EXCL", "TMUX");
    // Enable Claude Code agent teams feature.  The standalone binary gates
    // the entire teammate tool-set (spawnTeam, spawnTeammate, …) behind
    //   T8(): LA(process.env.CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS) || --agent-teams
    // Without this env var the team tools are never registered and Claude
    // always falls back to the in-process "Agent" tool.
    builder.env("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "1");

    // ── Claude Code workarounds (removable once upstream fixes land) ──
    //
    // claude-code-fix-tty (set -g claude-code-fix-tty on/off):
    //   Early Claude Code standalone binaries (v2.1.71) ignored `teammateMode`
    //   from settings.json, so psmux injects `--teammate-mode tmux` via the
    //   PowerShell env-shim `claude` wrapper.  Since psmux#399 (comment
    //   5041988743) the wrapper only injects when the user has NOT configured
    //   teammateMode in any settings.json Claude Code reads, because CLI flags
    //   outrank settings and blind injection silently overrode an explicit
    //   user choice.  Disable with: set -g claude-code-fix-tty off
    if fix_tty {
        builder.env("PSMUX_CLAUDE_TEAMMATE_MODE", "tmux");
    }

}

/// Apply user-defined environment variables (from set-environment -g) to a CommandBuilder.
/// This ensures variables set via config or runtime `set-environment` are explicitly
/// passed to every child pane, in addition to process inheritance.
pub fn apply_user_environment(builder: &mut CommandBuilder, environment: &std::collections::HashMap<String, String>) {
    for (key, value) in environment {
        builder.env(key, value);
    }
}

/// PowerShell env shim snippet — defines a `Global:env` function that translates
/// POSIX `env VAR=val ... command args` invocations into PowerShell equivalents.
///
/// Key design decisions for Windows + Claude Code agent teams compatibility:
///   1. POSIX backslash-escape removal uses `\\([^\w\\])` so that escapes like
///      `\@` and `\:` (produced by shell-quote) are stripped, while Windows
///      path separators (`\U` in `C:\Users`) are preserved (letter after `\`
///      is a `\w` character, so the regex does NOT match).
///   2. Escape stripping is applied to ALL arguments (env var values, the
///      command itself, and every trailing arg), not just env-var values.
///   3. `.js` / `.mjs` files are detected and automatically executed via
///      `node` because Windows associates `.js` with WScript.exe (WSH),
///      which cannot run Node.js code and instead shows error dialogs.
///   4. The shim is **always** installed (even when a native env.exe exists
///      on PATH) because Claude Code's shell-quote library produces POSIX
///      escapes (`\@`, `\:`) that native env.exe does not strip, causing
///      agent ID mismatches and spawn failures (psmux#172, #173, #180).
///      Users who need the raw env.exe can invoke it as `env.exe` explicitly.
const ENV_SHIM_PS: &str = concat!(
    "function Global:env { ",
    // _pu: POSIX-unescape helper — strips `\` before non-word, non-backslash
    // chars (e.g. \@ → @, \: → :) produced by npm shell-quote.
    // SKIPS Windows absolute paths (C:\...) where `\` is a directory
    // separator, not a POSIX escape.  On Linux paths use `/` so
    // there's never a collision; on Windows `\@` in a path like
    // `node_modules\@anthropic-ai` must be preserved.
    "function _pu($s){if($s -match '^[A-Za-z]:\\\\'){return $s}; $s -replace '\\\\([^\\w\\\\])','$1'}; ",
    // _shebang: reads the first line of a script file and extracts the
    // interpreter, mimicking Linux kernel shebang execution.
    // Handles #!/usr/bin/env node, #!/usr/bin/node, #!/usr/bin/env deno, etc.
    "function _shebang($f){ ",
    "try{ $l=(Get-Content $f -TotalCount 1 -EA Stop); ",
    "if($l -match '^#!\\s*(.+)$'){ ",
    "$p=$Matches[1].Trim(); ",
    "if($p -match '/env\\s+(.+)$'){return ($Matches[1].Trim()-split'\\s+')[0]}; ",
    "return ($p-split'/')[-1] } }catch{}; $null }; ",
    "$v=@{}; $i=0; ",
    "while($i -lt $args.Count){ ",
    "if([string]$args[$i] -match '^([A-Za-z_]\\w*)=(.*)$'){ ",
    "$v[$Matches[1]]=(_pu $Matches[2]); $i++ ",
    "} else { break } }; ",
    "if($i -lt $args.Count){ ",
    "foreach($e in $v.GetEnumerator()){[Environment]::SetEnvironmentVariable($e.Key,$e.Value,'Process')}; ",
    "$cmd=(_pu ([string]$args[$i])); $rest=@(); ",
    "if($i+1 -lt $args.Count){$rest=@($args[($i+1)..($args.Count-1)]|ForEach-Object{_pu ([string]$_)})}; ",
    // For script files (.js/.mjs/.ts/.sh/.py/etc), read the shebang line
    // to determine the interpreter — exactly like Linux kernel does.
    // Falls back to node for .js/.mjs only if no shebang is found
    // (since Windows associates .js with WScript.exe, not node).
    "$interp=$null; ",
    "$resolved=$cmd; if($cmd -match '^''(.+)''$'){$resolved=$Matches[1]}; ",
    "if(Test-Path $resolved -EA 0){$interp=(_shebang $resolved)}; ",
    "if($interp){& $interp $cmd @rest} ",
    "elseif($cmd -match '\\.m?js$'){& node $cmd @rest} ",
    "else{& $cmd @rest} ",
    "} elseif($v.Count -gt 0){ ",
    "foreach($e in $v.GetEnumerator()){[Environment]::SetEnvironmentVariable($e.Key,$e.Value,'Process')} ",
    "} else { Get-ChildItem Env:|ForEach-Object{$_.Name+'='+$_.Value} } }; ",
    // Claude Code teammate-mode wrapper (claude-code#26244, psmux#399):
    // Early standalone (Bun SFE) binaries ignored `teammateMode` from
    // settings.json, so psmux force-injected the `--teammate-mode` CLI flag.
    // Current Claude Code builds DO honour settings.json (debug log shows
    // `[TeammateModeSnapshot] Captured from config: ...`), and a CLI flag
    // outranks settings, so unconditional injection silently overrode an
    // explicit user choice.  The wrapper therefore injects ONLY when the user
    // has not configured teammateMode anywhere Claude Code reads it from:
    // an explicit CLI flag, managed settings, user-scope settings
    // (CLAUDE_CONFIG_DIR or ~/.claude), or a project's
    // .claude/settings(.local).json found by walking up from the CWD at call
    // time.  Disable entirely with: set -g claude-code-fix-tty off
    //
    // The real claude command is resolved at call time via Get-Command instead
    // of hardcoding claude.exe, because npm/nvm4w installs ship only claude.cmd
    // and claude.ps1 with no exe (psmux#475).  The CommandType filter
    // (Application = .exe/.cmd, ExternalScript = .ps1) excludes this wrapper
    // function itself, so there is no self-recursion.
    "if($env:PSMUX_CLAUDE_TEAMMATE_MODE){ ",
    // _psmux_tmcfg: $true when teammateMode is already configured in a settings
    // file Claude Code consults.  Checked at call time (not shim load time) so
    // cd'ing into a project with its own .claude/settings.json is honoured.
    "function Global:_psmux_tmcfg { ",
    "$fs=@(); ",
    "if($env:ProgramData){ $fs+=(Join-Path $env:ProgramData 'ClaudeCode/managed-settings.json') }; ",
    "$u=if($env:CLAUDE_CONFIG_DIR){$env:CLAUDE_CONFIG_DIR}else{Join-Path $env:USERPROFILE '.claude'}; ",
    "$fs+=(Join-Path $u 'settings.json'); ",
    "$d=$null; try{$d=(Get-Location -PSProvider FileSystem -EA Stop).ProviderPath}catch{}; ",
    "while($d){ ",
    "$fs+=(Join-Path $d '.claude/settings.json'); ",
    "$fs+=(Join-Path $d '.claude/settings.local.json'); ",
    "$p=Split-Path $d -Parent; if(-not $p -or $p -eq $d){break}; $d=$p }; ",
    "foreach($f in $fs){ try{ if((Test-Path -LiteralPath $f) -and ((Get-Content -LiteralPath $f -Raw) -match '\"teammateMode\"\\s*:')){ return $true } }catch{} }; ",
    "$false }; ",
    "function Global:claude { ",
    "$c=Get-Command claude -CommandType Application,ExternalScript -EA 0 | Select-Object -First 1; ",
    "if(-not $c){ $c='claude.exe' }; ",
    "if(($args -contains '--teammate-mode') -or (_psmux_tmcfg)){ & $c @args } ",
    "else{ & $c --teammate-mode $env:PSMUX_CLAUDE_TEAMMATE_MODE @args } } }",
);

/// PSReadLine prediction fix — disables predictions that crash with
/// NullReferenceException in GetHistoryItems() during ConPTY startup.
/// See https://github.com/psmux/psmux/issues/109
const PSRL_FIX: &str = concat!(
    "try { Set-PSReadLineOption -PredictionSource None -ErrorAction Stop } catch {}; ",
    "try { Set-PSReadLineOption -PredictionViewStyle InlineView -ErrorAction Stop } catch {}; ",
    "try { Remove-PSReadLineKeyHandler -Chord 'F2' -ErrorAction Stop } catch {}",
);

/// Minimal crash guard: saves the user's original PredictionSource, then
/// disables predictions to prevent the #109 NullReferenceException during
/// ConPTY startup.  Does NOT touch PredictionViewStyle or F2 so those stay
/// at whatever the system default is.  Used pre-profile when allow-predictions
/// is on (#150).
const PSRL_CRASH_GUARD: &str = concat!(
    "$Global:__psmux_origPred = try { (Get-PSReadLineOption).PredictionSource } catch { 'History' }; ",
    "try { Set-PSReadLineOption -PredictionSource None -ErrorAction Stop } catch {}",
);

/// Post-profile prediction restore: if PredictionSource is still None (meaning
/// the user's profile did not explicitly set it), restore the saved original.
/// If the profile DID set a value, we leave it alone.
/// Used post-profile when allow-predictions is on (#150).
const PSRL_PRED_RESTORE: &str = concat!(
    "if ((Get-PSReadLineOption).PredictionSource -eq 'None' -and $Global:__psmux_origPred -ne 'None') { ",
    "try { Set-PSReadLineOption -PredictionSource $Global:__psmux_origPred -ErrorAction Stop } catch {} ",
    "}",
);

/// Source all four PowerShell profile scripts in the standard order.
/// Used with -NoProfile to give us control over execution order — we disable
/// PSReadLine predictions BEFORE the profile loads (preventing the
/// GetHistoryItems NullReferenceException), then re-disable after the profile
/// in case the user's profile re-enables predictions.
const PROFILE_SOURCE: &str = concat!(
    "foreach ($__p in @(",
    "$PROFILE.AllUsersAllHosts,",
    "$PROFILE.AllUsersCurrentHost,",
    "$PROFILE.CurrentUserAllHosts,",
    "$PROFILE.CurrentUserCurrentHost",
    ")) { if ($__p -and (Test-Path $__p)) { try { . $__p } catch { Write-Warning \"psmux: profile error in ${__p}: $_\" } } }",
);

/// Sync PowerShell's $PWD to the OS-level CWD (#111).
/// PowerShell's `cd` (Set-Location) only updates `$PWD` internally and
/// does NOT call Win32 SetCurrentDirectory(). This means the process PEB
/// still shows the original spawn directory, causing #{pane_current_path}
/// to always return the initial CWD.
///
/// Instead of wrapping the `prompt` function (which conflicts with prompt
/// customizers like Starship, oh-my-posh, etc.), we wrap the three cmdlets
/// that actually change directories: Set-Location, Push-Location, and
/// Pop-Location.  This is invisible to prompt customizers and survives
/// `. $PROFILE` reloads.
const CWD_SYNC: &str = concat!(
    "if (-not (Test-Path variable:Global:__psmux_cwd_hook)) { ",
    "$Global:__psmux_cwd_hook = $true; ",
    "try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; ",
    "function Global:Set-Location { ",
    "Microsoft.PowerShell.Management\\Set-Location @args; ",
    "try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {} ",
    "}; ",
    "function Global:Push-Location { ",
    "Microsoft.PowerShell.Management\\Push-Location @args; ",
    "try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {} ",
    "}; ",
    "function Global:Pop-Location { ",
    "Microsoft.PowerShell.Management\\Pop-Location @args; ",
    "try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {} ",
    "} }",
);

/// True when the resolved shell path is a PowerShell (either PowerShell 7
/// `pwsh.exe` or Windows PowerShell 5.1 `powershell.exe`) and therefore needs
/// the interactive `psrl_init` block. Besides the PSReadLine prediction fix,
/// that block installs the Set-Location hook that keeps the Win32 process CWD
/// in sync so `#{pane_current_path}` tracks `cd` (issue #495). Both spawn
/// paths (`build_command`'s interactive branch and `build_default_shell`) must
/// use this so 5.1 is never left without the hook.
pub(crate) fn shell_needs_psrl_init(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("pwsh") || lower.contains("powershell")
}

/// Build the full interactive init string for PowerShell:
/// 1. Disable PSReadLine predictions (before profile — prevents #109 crash)
/// 2. Source the user's profile scripts
/// 3. If allow_predictions is false, re-disable predictions after the profile;
///    if allow_predictions is true, restore the saved original PredictionSource
///    only when the profile did not set one explicitly (#150)
/// 4. Install CWD sync hook (enables #{pane_current_path} — #111)
/// 5. Optionally append the env shim
fn build_psrl_init(env_shim: bool, allow_predictions: bool) -> String {
    let (pre_profile, post_profile) = if allow_predictions {
        (PSRL_CRASH_GUARD, PSRL_PRED_RESTORE)
    } else {
        (PSRL_FIX, PSRL_FIX)
    };
    let mut s = format!("{}; {}; {}; {}", pre_profile, PROFILE_SOURCE, post_profile, CWD_SYNC);
    if env_shim {
        s.push_str("; ");
        s.push_str(ENV_SHIM_PS);
    }
    s
}

/// Init block for PowerShell panes where the user explicitly passed
/// `-NoProfile`: no profile sourcing, but the PSReadLine fix and the CWD-sync
/// hook still apply. The hook is what keeps `#{pane_current_path}` tracking
/// `cd` (#495) — it is unrelated to profiles and must not be dropped just
/// because profile sourcing is skipped.
fn build_psrl_init_noprofile(env_shim: bool) -> String {
    let mut s = format!("{}; {}", PSRL_FIX, CWD_SYNC);
    if env_shim {
        s.push_str("; ");
        s.push_str(ENV_SHIM_PS);
    }
    s
}

/// True when `prog`'s file stem is exactly a PowerShell executable (`pwsh` or
/// `powershell`). Stricter than `shell_needs_psrl_init` (which substring
/// matches anywhere in the path) — used for directly spawned commands, where
/// appending PowerShell flags to a non-PowerShell exe would break it.
#[cfg(windows)]
fn is_powershell_program(prog: &str) -> bool {
    std::path::Path::new(prog)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| {
            let lower = s.to_ascii_lowercase();
            lower == "pwsh" || lower == "powershell"
        })
        .unwrap_or(false)
}

/// True when the args of a directly spawned PowerShell leave it interactive:
/// every arg is a `-Flag` and none of them executes a command or script
/// (`-Command`, `-File`, `-EncodedCommand` or their short forms). A
/// positional arg means a script/command (pwsh treats it as `-File`,
/// powershell as `-Command`), so the pane is not an interactive shell and
/// must not get the init block appended.
#[cfg(windows)]
fn powershell_args_interactive(args: &[String]) -> bool {
    args.iter().all(|a| {
        if !a.starts_with('-') {
            return false;
        }
        let flag = a.trim_start_matches('-').to_ascii_lowercase();
        !matches!(
            flag.as_str(),
            "command" | "c" | "file" | "f" | "encodedcommand" | "e" | "ec"
        )
    })
}

/// On Windows, translate Unix-style shell wrappers to Windows equivalents.
///
/// Tools like Overstory wrap agent commands in `/bin/bash -c '...'` for
/// environment setup (unset/export). This doesn't work on Windows because
/// `/bin/bash` doesn't exist. This function:
/// 1. If the command is `/bin/bash -c '...'` or `/bin/sh -c '...'`, try to
///    find `bash.exe` in PATH and rewrite to use the resolved path.
/// 2. If bash isn't available, extract the inner script and translate
///    common bash patterns (unset, export, &&) to PowerShell equivalents.
/// 3. For other Unix absolute paths (/usr/bin/foo), try to resolve the
///    basename from PATH.
#[cfg(windows)]
fn resolve_unix_path(cmd: &str) -> String {
    let trimmed = cmd.trim();

    // General case: resolve Unix absolute paths (e.g. /usr/bin/python3)
    if trimmed.starts_with('/') {
        let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
        let program = parts[0];
        let basename = std::path::Path::new(program)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(program);
        if let Ok(resolved) = which::which(basename) {
            let rest = if parts.len() > 1 { parts[1] } else { "" };
            if rest.is_empty() {
                return format!("\"{}\"", resolved.to_string_lossy());
            } else {
                return format!("\"{}\" {}", resolved.to_string_lossy(), rest);
            }
        }
    }

    // No translation needed
    cmd.to_string()
}

/// Detect if a command is a `/bin/bash -c '...'` or similar pattern.
/// Returns Some((inner_script, shell_name)) if matched.
#[cfg(windows)]
fn detect_bash_c_wrapper(cmd: &str) -> Option<(&str, &str)> {
    let shell_prefixes = [
        ("/bin/bash -c ", "bash"),
        ("/bin/sh -c ", "sh"),
        ("/usr/bin/bash -c ", "bash"),
        ("/usr/bin/sh -c ", "sh"),
        ("/usr/bin/env bash -c ", "bash"),
        ("/usr/bin/env sh -c ", "sh"),
    ];
    for (prefix, shell_name) in &shell_prefixes {
        if cmd.starts_with(prefix) {
            let rest = &cmd[prefix.len()..];
            // Strip outer quotes (single or double)
            let inner = if (rest.starts_with('\'') && rest.ends_with('\''))
                || (rest.starts_with('"') && rest.ends_with('"'))
            {
                &rest[1..rest.len() - 1]
            } else {
                rest
            };
            return Some((inner, shell_name));
        }
    }
    None
}

/// Parse a bash-style env setup script and extract environment modifications
/// plus the final command.  Returns (env_removes, env_sets, final_command).
///
/// This approach is **shell-agnostic**: instead of translating bash syntax to
/// PowerShell/cmd syntax, we parse the env operations and apply them directly
/// on the `CommandBuilder` (via `env_remove()` / `env()`).  The final command
/// is then executed through whatever default shell the user has configured,
/// without any env-manipulation syntax that could be shell-incompatible.
#[cfg(windows)]
fn parse_bash_env_script(script: &str) -> (Vec<String>, Vec<(String, String)>, String) {
    let mut removes: Vec<String> = Vec::new();
    let mut sets: Vec<(String, String)> = Vec::new();
    let mut final_parts: Vec<String> = Vec::new();

    let segments: Vec<&str> = script.split("&&").collect();
    for seg in &segments {
        let seg = seg.trim();
        if seg.is_empty() { continue; }

        if seg.starts_with("unset ") {
            let vars: Vec<&str> = seg["unset ".len()..].split_whitespace().collect();
            for var in vars {
                removes.push(var.to_string());
            }
        } else if seg.starts_with("export ") {
            let assign = &seg["export ".len()..];
            if let Some(eq_pos) = assign.find('=') {
                let var = assign[..eq_pos].to_string();
                let mut val = assign[eq_pos + 1..].trim().to_string();
                // Strip outer quotes
                if (val.starts_with('"') && val.ends_with('"'))
                    || (val.starts_with('\'') && val.ends_with('\''))
                {
                    val = val[1..val.len() - 1].to_string();
                }
                // Resolve $PATH / ${PATH} references to the actual current PATH value.
                // Also fix Unix `:` separator to Windows `;`.
                if let Ok(current_path) = std::env::var("PATH") {
                    val = val.replace(":$PATH", &format!(";{}", current_path))
                             .replace(":${PATH}", &format!(";{}", current_path))
                             .replace("$PATH:", &format!("{};", current_path))
                             .replace("${PATH}:", &format!("{};", current_path))
                             .replace("$PATH", &current_path)
                             .replace("${PATH}", &current_path);
                }
                sets.push((var, val));
            }
        } else {
            // Final command or unknown segment — preserve as-is
            final_parts.push(seg.to_string());
        }
    }

    let final_cmd = final_parts.join(" && ");
    (removes, sets, final_cmd)
}

/// Strip one layer of matching single or double quotes from a token.
#[cfg(windows)]
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Detect the POSIX `env VAR=val ... <program> <args>` launch idiom, optionally
/// preceded by `cd <dir> &&`.  Tools written for real tmux (notably Claude Code
/// agent-teams, which launches each teammate with
/// `cd '<cwd>' && env CLAUDECODE=1 ... '<claude>' --agent-id ...`) use it.
/// Under PowerShell `env` is not a command, so psmux would run the pane command
/// through pwsh and it dies with "env: The term 'env' is not recognized"
/// (intermittent: it only works when Git's usr\bin happens to be on PATH).
///
/// We parse the `cd` target and the `VAR=val` assignments out and return
/// `(cwd_override, env_sets, remaining_command)` so the caller can apply cwd/env
/// directly on the CommandBuilder and run the program without the `env` prefix,
/// independent of PATH.  Returns None when the `env ` idiom is not present.
/// (issue #399)
#[cfg(windows)]
fn detect_env_prefix_command(cmd: &str) -> Option<(Option<String>, Vec<(String, String)>, String)> {
    let mut rest = cmd.trim();
    let mut cwd_override: Option<String> = None;

    // Optional leading `cd <dir> && `
    if let Some(after_cd) = rest.strip_prefix("cd ") {
        if let Some(amp) = after_cd.find("&&") {
            cwd_override = Some(strip_quotes(after_cd[..amp].trim()).to_string());
            rest = after_cd[amp + 2..].trim();
        }
    }

    // Must be the `env ` idiom to apply this handling.
    let after_env = rest.strip_prefix("env ")?;

    // Consume leading `KEY=VALUE` tokens; the first non-assignment token begins
    // the program + args.
    let mut env_sets: Vec<(String, String)> = Vec::new();
    let mut remainder = after_env.trim_start();
    loop {
        let token_end = remainder.find(char::is_whitespace).unwrap_or(remainder.len());
        let token = &remainder[..token_end];
        if let Some(eq) = token.find('=') {
            let key = &token[..eq];
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                let val = strip_quotes(&token[eq + 1..]).to_string();
                env_sets.push((key.to_string(), val));
                remainder = remainder[token_end..].trim_start();
                continue;
            }
        }
        break;
    }

    if remainder.is_empty() {
        return None;
    }
    Some((cwd_override, env_sets, remainder.to_string()))
}

/// Split a spawn-command string into whitespace-separated tokens, honouring
/// double- and single-quoted segments (quotes are consumed).  Used by the
/// direct-spawn path below to recover `program + args` from strings like
/// `"C:/Program Files/Git/bin/bash.exe" --login -i`.
#[cfg(windows)]
fn split_spawn_tokens(cmd: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        match quote {
            Some(q) => {
                if c == q { quote = None; } else { cur.push(c); }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                c if c.is_whitespace() => {
                    if !cur.is_empty() { tokens.push(std::mem::take(&mut cur)); }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() { tokens.push(cur); }
    tokens
}

/// tmux parity for window/pane commands (issues #492, #493): tmux execs a
/// multi-argument shell-command DIRECTLY (spawn.c: `execvp(argv[0], argv)`)
/// and only routes single strings through `$SHELL -c`.  psmux wrapped every
/// command in `<shell> -Command "<cmd>"`, which (a) leaves a wrapper
/// powershell process around every pane (#493) and (b) breaks quoted
/// executable paths containing spaces, which pwsh cannot parse as a bare
/// statement (#492).
///
/// Resolve the command to `Some((program, args))` when it is an EXPLICIT
/// executable path we can spawn directly:
///   - shell syntax (pipes, redirects, `&&`, variables, ...) → None (needs a
///     real shell);
///   - the whole string is an existing executable path (spaces included);
///   - a quoted first token, or the longest token-prefix, is an existing
///     executable path (handles unquoted space paths with arguments).
///
/// Only commands whose program contains a path separator qualify — that is
/// the case both reporters hit (`C:/Program Files/Git/bin/bash.exe`,
/// `C:/cygwin64/bin/zsh.exe --login`).  Bare program names (`timeout`,
/// `ping`, `cmd.exe`) intentionally KEEP the historical shell wrapper:
/// console utilities like timeout.exe exit immediately when spawned without
/// the shell re-establishing console stdin ("input redirection is not
/// supported"), and the wrapper preserves their expected environment.
#[cfg(windows)]
fn try_direct_spawn(cmd: &str) -> Option<(String, Vec<String>)> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() { return None; }
    // pwsh call-operator form produced by the env-prefix path (#399) keeps
    // its established shell route.
    if trimmed.starts_with('&') { return None; }
    // Direct spawn is only for explicit paths.
    if !(trimmed.contains('/') || trimmed.contains('\\')) { return None; }
    let exists_as_program = |p: &str| -> Option<String> {
        if !(p.contains('/') || p.contains('\\')) { return None; }
        let path = std::path::Path::new(p);
        if path.is_file() { return Some(p.to_string()); }
        if !p.to_ascii_lowercase().ends_with(".exe") {
            let with_exe = format!("{}.exe", p);
            if std::path::Path::new(&with_exe).is_file() { return Some(with_exe); }
        }
        None
    };
    // CreateProcess is picky about forward slashes in the application path
    // (unix style `C:/...` is how users write these commands), so normalize
    // the program to backslashes before spawning.
    let normalize = |p: String| p.replace('/', "\\");
    // Whole string as one path — covers `C:/Program Files/Git/bin/bash.exe`
    // exactly as users write it in bind-key/new-window (quotes already
    // consumed by the command parser).  Checked BEFORE the metacharacter
    // bail so `C:\Program Files (x86)\...` paths are still resolved.
    if let Some(prog) = exists_as_program(trimmed) {
        return Some((normalize(prog), Vec::new()));
    }
    // Any shell metacharacter means the string needs a real shell.
    if trimmed.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | ';' | '`' | '$' | '(' | ')' | '%' | '\n' | '\r')) {
        return None;
    }
    let tokens = split_spawn_tokens(trimmed);
    if tokens.is_empty() { return None; }
    // Longest token-prefix that is an existing file: handles unquoted space
    // paths followed by arguments (`C:/Program Files/.../bash.exe --login`).
    for k in (1..=tokens.len()).rev() {
        let candidate = tokens[..k].join(" ");
        if let Some(prog) = exists_as_program(&candidate) {
            return Some((normalize(prog), tokens[k..].to_vec()));
        }
    }
    None
}

pub fn build_command(command: Option<&str>, env_shim: bool, allow_predictions: bool) -> CommandBuilder {
    // Capture CWD early — portable_pty on Windows defaults to USERPROFILE
    // (home dir) when no cwd is set on CommandBuilder, so we must set it
    // explicitly to honour the caller's working directory.
    let cwd = std::env::current_dir().ok();
    // tmux blocker idiom (issue #580): Claude Code's teammate backend (and
    // tmux scripting generally) creates placeholder panes with `-- cat` — on
    // Unix a process that blocks reading its terminal forever. On Windows a
    // bare `cat` either fails CreateProcessW (nothing on PATH) or hits
    // PowerShell's Get-Content alias, which prompts `Path[0]:` and leaves the
    // pane wedged at a parameter prompt. Substitute a faithful blocker: read
    // and discard stdin until EOF, then sleep forever (a ConPTY pane's stdin
    // never reaches EOF while the pane lives, so this blocks exactly like
    // `cat > /dev/null`). `respawn-pane -k` replaces it just like tmux.
    #[cfg(windows)]
    let command: Option<&str> = match command {
        Some(c) if matches!(c.trim(), "cat" | "cat -") => {
            Some("$__psmux_in=[Console]::In; while($__psmux_in.Read() -ge 0){}; Start-Sleep -Seconds 2147483")
        }
        other => other,
    };
    // tmux parity (#582): an explicit `-- prog args...` argv with more than
    // one token is exec'd DIRECTLY (tmux spawn.c execvp), never routed
    // through a shell wrapper. The CLI and server encode the argv form by
    // keeping the `--` marker at the head of the command string with each
    // token requoted; a single token after `--` keeps tmux's string
    // semantics (shell route), which also preserves the #580 teammate
    // respawn idiom `-- "<one quoted command>"`.
    #[cfg(windows)]
    let (command_owned, raw_argv): (Option<String>, Option<Vec<String>>) = match command {
        Some(c) => {
            let t = c.trim_start();
            match t.strip_prefix("--") {
                Some(rest) if rest.is_empty() => (None, None),
                Some(rest) if rest.starts_with(char::is_whitespace) => {
                    let toks = split_spawn_tokens(rest);
                    match toks.len() {
                        0 => (None, None),
                        1 => (Some(toks.into_iter().next().unwrap()), None),
                        _ => {
                            // Unix launch idioms (`env VAR=v prog`, `/bin/bash
                            // -c '...'`) need the string path's Windows
                            // translation (#399); direct-exec'ing `env` would
                            // fail outright. Shell metacharacters mean the
                            // argv was really a flattened shell string (the
                            // respawn wire path loses quotes), so those keep
                            // shell semantics too.
                            let tail = rest.trim();
                            let needs_shell = tail.chars().any(|c| matches!(c,
                                '&' | '|' | '<' | '>' | ';' | '`' | '$' | '(' | ')' | '%' | '\n' | '\r'))
                                || detect_bash_c_wrapper(tail).is_some()
                                || detect_env_prefix_command(tail).is_some();
                            if needs_shell {
                                (Some(tail.to_string()), None)
                            } else {
                                (None, Some(toks))
                            }
                        }
                    }
                }
                // e.g. "--foo": not the argv marker, an ordinary string.
                _ => (Some(c.to_string()), None),
            }
        }
        None => (None, None),
    };
    #[cfg(windows)]
    let command: Option<&str> = command_owned.as_deref();
    // A single token decoded from the `--` form can itself be the cat
    // blocker idiom; the substitution above ran before decoding, so route
    // it back through (depth is bounded: the plain "cat" string never
    // reaches this decoder again).
    #[cfg(windows)]
    if let Some(c) = command {
        if matches!(c.trim(), "cat" | "cat -") {
            return build_command(Some(c.trim()), env_shim, allow_predictions);
        }
    }
    #[cfg(windows)]
    if let Some(tokens) = raw_argv {
        // `-- cat -` spells the tmux blocker idiom too (#580).
        if tokens.len() == 2 && tokens[0] == "cat" && tokens[1] == "-" {
            return build_command(Some("cat"), env_shim, allow_predictions);
        }
        // CreateProcess resolves bare names (cmd.exe, ping) via its own
        // search path and auto-wraps .bat/.cmd; normalize unix-style
        // forward slashes like the direct-spawn path does.
        let prog = tokens[0].replace('/', "\\");
        let mut builder = CommandBuilder::new(&prog);
        if let Some(ref dir) = cwd { builder.cwd(dir); }
        apply_bare_env_if_set(&mut builder);
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        builder.env("PSMUX_SESSION", "1");
        let prog_args: Vec<String> = tokens[1..].to_vec();
        builder.args(&prog_args);
        // #495 follow-up, same as the direct-spawn path: an interactive
        // PowerShell needs psrl_init or #{pane_current_path} freezes.
        if is_powershell_program(&prog) && powershell_args_interactive(&prog_args) {
            let has_noprofile = prog_args.iter()
                .any(|a| a.eq_ignore_ascii_case("-NoProfile"));
            let psrl_init = if has_noprofile {
                build_psrl_init_noprofile(env_shim)
            } else {
                build_psrl_init(env_shim, allow_predictions)
            };
            if !has_noprofile {
                builder.args(["-NoProfile"]);
            }
            builder.args(["-NoLogo", "-NoExit", "-Command", &psrl_init]);
        }
        return builder;
    }
    if let Some(cmd) = command {
        // On Windows, detect `/bin/bash -c '...'` wrappers used by tools like
        // Overstory and omc for env var setup before launching agents.
        // Instead of translating to shell-specific syntax (which breaks if the
        // user's default shell is bash, cmd, or a different PowerShell version),
        // we parse the env operations from the bash script and apply them directly
        // on the CommandBuilder.  The final command is then passed to whatever
        // shell `cached_shell()` resolves to, env-manipulation-free.
        #[cfg(windows)]
        let (env_removes, env_sets, cmd, cwd_override, direct_ok) = {
            let trimmed = cmd.trim();
            if let Some((inner_script, _)) = detect_bash_c_wrapper(trimmed) {
                let (removes, sets, final_cmd) = parse_bash_env_script(inner_script);
                let final_cmd = if final_cmd.is_empty() {
                    cmd.to_string()
                } else {
                    resolve_unix_path(&final_cmd)
                };
                (removes, sets, final_cmd, None, false)
            } else if let Some((cwd_dir, sets, final_cmd)) = detect_env_prefix_command(trimmed) {
                // POSIX `env VAR=val <program>` idiom (issue #399: Claude Code
                // agent-teams teammate launch). Apply env/cwd directly and run the
                // program without the `env` prefix so it does not depend on the
                // `env` binary being on PATH. The program is a path/exe token, so
                // prefix the pwsh call operator `&` to invoke it rather than have
                // pwsh treat the first token as a string to print.
                (Vec::new(), sets, format!("& {}", final_cmd), cwd_dir, false)
            } else {
                (Vec::new(), Vec::new(), resolve_unix_path(cmd), None, true)
            }
        };
        #[cfg(not(windows))]
        let (env_removes, env_sets, cmd, cwd_override) = (Vec::<String>::new(), Vec::<(String, String)>::new(), cmd.to_string(), None::<String>);

        // An explicit `cd <dir>` from the launch idiom overrides the inherited CWD.
        let cwd = cwd_override
            .map(std::path::PathBuf::from)
            .or(cwd);

        // tmux parity (#492, #493): spawn plain program invocations DIRECTLY
        // instead of wrapping them in `<shell> -Command`. tmux only routes
        // shell-syntax command strings through a shell (spawn.c execvp's
        // multi-argument commands verbatim). This removes the lingering
        // powershell wrapper process around every command pane (#493) and
        // makes quoted executable paths containing spaces spawnable (#492).
        #[cfg(windows)]
        if direct_ok {
            if let Some((prog, prog_args)) = try_direct_spawn(&cmd) {
                let mut builder = CommandBuilder::new(&prog);
                if let Some(ref dir) = cwd { builder.cwd(dir); }
                apply_bare_env_if_set(&mut builder);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                for var in &env_removes { builder.env_remove(var); }
                for (k, v) in &env_sets { builder.env(k, v); }
                builder.args(&prog_args);
                // #495 follow-up: a directly spawned pwsh/powershell path is an
                // interactive PowerShell pane, and without psrl_init it lacks
                // the Set-Location hook, freezing #{pane_current_path} at the
                // spawn directory. Append the same init that default-shell
                // panes get, unless the user's args already execute a
                // command/script (then the pane is not an interactive shell).
                if is_powershell_program(&prog) && powershell_args_interactive(&prog_args) {
                    let has_noprofile = prog_args.iter()
                        .any(|a| a.eq_ignore_ascii_case("-NoProfile"));
                    let psrl_init = if has_noprofile {
                        build_psrl_init_noprofile(env_shim)
                    } else {
                        build_psrl_init(env_shim, allow_predictions)
                    };
                    if !has_noprofile {
                        builder.args(["-NoProfile"]);
                    }
                    builder.args(["-NoLogo", "-NoExit", "-Command", &psrl_init]);
                }
                return builder;
            }
        }

        let shell = cached_shell().map(|s| s.to_string());

        match shell {
            Some(path) => {
                let mut builder = CommandBuilder::new(&path);
                if let Some(ref dir) = cwd { builder.cwd(dir); }
                // Apply PSMUX_BARE_ENV BEFORE adding our own envs, so the
                // overrides we add below survive env_clear (#167).
                apply_bare_env_if_set(&mut builder);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                for var in &env_removes { builder.env_remove(var); }
                for (k, v) in &env_sets { builder.env(k, v); }

                let stem = std::path::Path::new(&path).file_stem()
                    .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                if stem == "pwsh" || stem == "powershell" {
                    builder.args(["-NoLogo", "-Command", &cmd]);
                } else if matches!(stem.as_str(), "bash" | "sh" | "zsh" | "fish" | "dash" | "ash") {
                    builder.args(["-c", &cmd]);
                } else {
                    builder.args(["/C", &cmd]);
                }
                builder
            }
            None => {
                let mut builder = CommandBuilder::new("pwsh.exe");
                if let Some(ref dir) = cwd { builder.cwd(dir); }
                apply_bare_env_if_set(&mut builder);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                for var in &env_removes { builder.env_remove(var); }
                for (k, v) in &env_sets { builder.env(k, v); }
                builder.args(["-NoLogo", "-Command", &cmd]);
                builder
            }
        }
    } else {
        let shell = cached_shell().map(|s| s.to_string());
        // PSReadLine v2.2.6+ enables PredictionSource HistoryAndPlugin by default.
        // Predictions cause display corruption in terminal multiplexers because
        // PSReadLine's VT rendering races with ConPTY output capture.
        // Issue #109: GetHistoryItems() throws NullReferenceException when
        // predictions are enabled in the profile before PSReadLine is fully
        // initialized inside ConPTY.  We use -NoProfile and source profiles
        // ourselves, sandwiching them between prediction-disable commands.
        let psrl_init = build_psrl_init(env_shim, allow_predictions);
        match shell {
            Some(path) => {
                let mut builder = CommandBuilder::new(&path);
                if let Some(ref dir) = cwd { builder.cwd(dir); }
                apply_bare_env_if_set(&mut builder);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                // Both PowerShell 7 (pwsh.exe) and Windows PowerShell 5.1
                // (powershell.exe) need psrl_init — it installs the
                // Set-Location hook that keeps #{pane_current_path} tracking
                // `cd` (issue #495). See shell_needs_psrl_init.
                if shell_needs_psrl_init(&path) {
                    builder.args(["-NoLogo", "-NoProfile", "-NoExit", "-Command", &psrl_init]);
                }
                builder
            }
            None => {
                let mut builder = CommandBuilder::new("pwsh.exe");
                if let Some(ref dir) = cwd { builder.cwd(dir); }
                apply_bare_env_if_set(&mut builder);
                builder.env("TERM", "xterm-256color");
                builder.env("COLORTERM", "truecolor");
                builder.env("PSMUX_SESSION", "1");
                // Apply the same -NoProfile + manual profile sourcing for
                // the fallback pwsh.exe path (previously had no PSRL fix).
                builder.args(["-NoLogo", "-NoProfile", "-NoExit", "-Command", &psrl_init]);
                builder
            }
        }
    }
}

/// Cached resolved default-shell path to avoid repeated `which::which()` scans.
static CACHED_DEFAULT_SHELL: std::sync::OnceLock<std::collections::HashMap<String, String>> = std::sync::OnceLock::new();
static CACHED_DEFAULT_SHELL_MAP: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> = std::sync::Mutex::new(None);

/// Resolve a program name via `which`, caching the result.
fn cached_which(program: &str) -> String {
    // Fast path: check if already cached in the global OnceLock for the default
    // (most common case is always the same shell)
    let mut map = CACHED_DEFAULT_SHELL_MAP.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(std::collections::HashMap::new);
    if let Some(cached) = map.get(program) {
        return cached.clone();
    }
    let resolved = which::which(program).ok()
        .map(|p| prefer_app_execution_alias(p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| program.to_string());
    map.insert(program.to_string(), resolved.clone());
    resolved
}

/// Split a shell config value into (program, extra_args), handling paths
/// that contain spaces (e.g. `C:/Program Files/Git/bin/bash.exe`).
///
/// Resolution order:
/// 1. If the whole string resolves to an existing executable, use it as-is.
/// 2. Otherwise, use quote-aware tokenising so that users can write
///    `"C:/Program Files/Git/bin/bash.exe" --login` with quotes.
fn resolve_shell_program(shell_path: &str) -> (String, Vec<String>) {
    // Fast path: whole string is the program (possibly with spaces in path).
    if std::path::Path::new(shell_path).is_file()
        || which::which(shell_path).is_ok()
    {
        return (shell_path.to_string(), vec![]);
    }

    // Quote-aware split (handles `"path with spaces" arg1 arg2`).
    let parsed = crate::commands::parse_command_line(shell_path);
    if parsed.is_empty() {
        return (shell_path.to_string(), vec![]);
    }
    let program = parsed[0].clone();
    let extra = parsed[1..].to_vec();
    (program, extra)
}

/// Shell executable stems that are POSIX-style shells. When one of these is
/// the configured `default-shell` it is spawned as a login shell (`-l`),
/// matching tmux, so Git-Bash/MSYS2 profile scripts run and set up PATH
/// (`/usr/bin` and friends). Without this an MSYS2 bash/zsh pane starts with
/// only the inherited Windows PATH and the whole Unix toolchain is
/// unreachable (issue #474).
const POSIX_SHELL_STEMS: &[&str] = &["bash", "zsh", "sh", "fish", "dash", "ksh", "tcsh", "csh"];

/// True when `path`'s file stem names a POSIX-style shell.
pub(crate) fn is_posix_shell_path(path: &str) -> bool {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| POSIX_SHELL_STEMS.contains(&s.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// `git-bash.exe` is a GUI launcher that spawns its own mintty window — as a
/// pane shell it produces a dead blank pane plus stray terminal windows
/// (issue #474). Substitute the real console `bash.exe` that ships beside it.
pub(crate) fn remap_git_bash_launcher(program: String) -> String {
    let path = std::path::Path::new(&program);
    let is_launcher = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("git-bash"))
        .unwrap_or(false);
    if is_launcher {
        if let Some(dir) = path.parent() {
            for candidate in [dir.join("bin").join("bash.exe"), dir.join("usr").join("bin").join("bash.exe")] {
                if candidate.is_file() {
                    return candidate.to_string_lossy().into_owned();
                }
            }
        }
    }
    program
}

/// Build a CommandBuilder that launches the given shell path interactively.
/// Used when `default-shell` / `default-command` is configured.
/// Supports pwsh, powershell, cmd, bash/zsh (spawned as login shells), and
/// any arbitrary executable.
pub fn build_default_shell(shell_path: &str, env_shim: bool, allow_predictions: bool) -> CommandBuilder {
    let (program, extra_args) = resolve_shell_program(shell_path);
    let program = remap_git_bash_launcher(program);

    // Resolve bare names via cached `which` — avoids repeated PATH scans.
    let resolved = cached_which(&program);

    let mut builder = CommandBuilder::new(&resolved);
    // Set CWD explicitly — portable_pty on Windows defaults to USERPROFILE
    // (home dir) when no cwd is set on CommandBuilder.
    if let Ok(dir) = std::env::current_dir() { builder.cwd(dir); }
    // PSMUX_BARE_ENV escape hatch (issue #167): clear inherited env before
    // adding our own.
    apply_bare_env_if_set(&mut builder);
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("PSMUX_SESSION", "1");

    // Prepend extra arguments (e.g. -NoProfile) BEFORE our -NoExit/-Command block
    // so they're interpreted as flags rather than as -Command arguments.
    if !extra_args.is_empty() {
        builder.args(extra_args.clone());
    }

    if is_posix_shell_path(&resolved) {
        // Keep the pane's working directory: Git-Bash and MSYS2 login shells
        // otherwise `cd $HOME` from /etc/profile.
        builder.env("CHERE_INVOKING", "1");
        // tmux parity: the default shell runs as a login shell so profile
        // scripts set up PATH (/usr/bin under MSYS2) and oh-my-zsh style
        // configs load. Skipped when the user passed their own args — they
        // control the invocation then.
        if extra_args.is_empty() {
            builder.args(["-l"]);
        }
    } else if shell_needs_psrl_init(&resolved) {
        // Issue #109: -NoProfile + manual profile sourcing to prevent
        // PSReadLine GetHistoryItems NullReferenceException.
        // If the user already passed -NoProfile in extra_args, we still
        // add ours (PowerShell accepts duplicates harmlessly) and skip
        // profile sourcing only if they explicitly opted out.
        let has_noprofile = extra_args.iter()
            .any(|a| a.eq_ignore_ascii_case("-NoProfile"));
        let psrl_init = if has_noprofile {
            // User explicitly wants no profile — apply PSRL fix + CWD-sync
            // hook + shim. The CWD hook is unrelated to profiles and must
            // stay, or #{pane_current_path} freezes (#495).
            build_psrl_init_noprofile(env_shim)
        } else {
            build_psrl_init(env_shim, allow_predictions)
        };
        if !has_noprofile {
            builder.args(["-NoProfile"]);
        }
        builder.args(["-NoLogo", "-NoExit", "-Command", &psrl_init]);
    }

    builder
}

/// Build a CommandBuilder for direct execution (no shell wrapping).
/// raw_args[0] is the program, rest are its arguments.
/// Used when -- separator is specified in new-session.
pub fn build_raw_command(raw_args: &[String]) -> CommandBuilder {
    if raw_args.is_empty() {
        return build_command(None, true, false);
    }
    // tmux blocker idiom (issue #580): `new-session -- cat` is how Claude
    // Code's teammate backend creates its root placeholder session. The
    // shell-wrapped paths substitute a stdin-draining blocker in
    // build_command, but this direct-exec path handed the literal `cat` to
    // CreateProcessW, which fails the whole new-session. Route the idiom
    // through build_command so it gets the same blocker.
    #[cfg(windows)]
    {
        let joined = raw_args.join(" ");
        if matches!(joined.trim(), "cat" | "cat -") {
            return build_command(Some("cat"), true, false);
        }
    }
    let program = &raw_args[0];
    let mut builder = CommandBuilder::new(program);
    // Set CWD explicitly — portable_pty on Windows defaults to USERPROFILE
    // (home dir) when no cwd is set on CommandBuilder.
    if let Ok(dir) = std::env::current_dir() { builder.cwd(dir); }
    builder.env("TERM", "xterm-256color");
    builder.env("COLORTERM", "truecolor");
    builder.env("PSMUX_SESSION", "1");
    if raw_args.len() > 1 {
        let args: Vec<&str> = raw_args[1..].iter().map(|s| s.as_str()).collect();
        builder.args(args);
    }
    builder
}

/// Spawn a dedicated PTY reader thread that processes output and updates the
/// data_version counter. Exits cleanly after 200 consecutive zero-byte reads
/// (indicating the PTY pipe is closed) or on any I/O error.
///
/// Uses an 8KB read buffer (down from 64KB) to reduce mutex hold time during
/// `parser.process()`, which improves DumpState latency under heavy output.

/// Scan raw ConPTY output for DECSCUSR cursor shape sequences (`\x1b[N q`).
/// Returns the last cursor shape value found, or None.
///
/// We accept all DECSCUSR cursor shape values (0-6) from child processes.
/// Value 0 resets to default, 1-2 = block, 3-4 = underline, 5-6 = bar.
fn scan_cursor_shape(data: &[u8]) -> Option<u8> {
    let mut last_shape: Option<u8> = None;
    let mut i = 0;
    while i < data.len() {
        if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'[' {
            let mut j = i + 2;
            let mut param: u8 = 0;
            while j < data.len() && data[j].is_ascii_digit() {
                param = param.saturating_mul(10).saturating_add(data[j] - b'0');
                j += 1;
            }
            // Check for SP q (space 0x20 + 'q') = DECSCUSR
            if j + 1 < data.len() && data[j] == b' ' && data[j + 1] == b'q' {
                if param <= 6 {
                    last_shape = Some(param);
                }
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    last_shape
}

/// Returns true if `data` contains the RMCUP sequence (ESC[?1049l).
fn scan_rmcup(data: &[u8]) -> bool {
    const RMCUP: &[u8] = b"\x1b[?1049l";
    data.windows(RMCUP.len()).any(|w| w == RMCUP)
}

/// Returns true if `data` contains a Cursor Position Request (ESC[6n).
/// ConPTY children (e.g. pwsh) emit this at startup and after session events
/// such as lock/unlock.  The host must respond with ESC[row;colR or the
/// child blocks indefinitely.
fn scan_cpr_query(data: &[u8]) -> bool {
    const CPR: &[u8] = b"\x1b[6n";
    data.contains(&0x1b) && data.windows(CPR.len()).any(|w| w == CPR)
}

/// Detects ESC[6n across batch boundaries. The parser thread scans output in
/// coalesced batches; a cursor-position request split across two batches is
/// invisible to the per-batch `scan_cpr_query` (no carry-over), so `cpr_pending`
/// is never set, the reply is never written, and whatever issued the request
/// waits for it forever. The asker can be pwsh after a lock/unlock, or conhost
/// at pane startup (PSUEDOCONSOLE_INHERIT_CURSOR) -- in that case the pane's
/// child hangs permanently in ConsoleCreateConnectionObject, before any user
/// code runs (reproduced deterministically: split the request and the per-batch
/// scan misses it, the pane never starts on an idle machine). This scanner
/// carries the last KEEP bytes between batches (one less than the sequence
/// length -- the most a boundary can hide) and rescans the boundary region, so
/// a split query is still detected.
struct CprScanner {
    tail: Vec<u8>,
}

impl CprScanner {
    /// One less than the query length: a 4-byte sequence not contained in a
    /// single batch has at most 3 bytes on either side of the boundary.
    const KEEP: usize = 3;

    fn new() -> Self {
        Self {
            tail: Vec::with_capacity(Self::KEEP),
        }
    }

    fn scan(&mut self, batch: &[u8]) -> bool {
        let mut hit = scan_cpr_query(batch);
        if !hit && !self.tail.is_empty() {
            // A match crossing the boundary has ≤KEEP bytes in the tail and
            // ≤KEEP bytes at the start of this batch.
            let mut boundary = self.tail.clone();
            boundary.extend_from_slice(&batch[..batch.len().min(Self::KEEP)]);
            hit = scan_cpr_query(&boundary);
        }
        if batch.len() >= Self::KEEP {
            self.tail.clear();
            self.tail.extend_from_slice(&batch[batch.len() - Self::KEEP..]);
        } else {
            self.tail.extend_from_slice(batch);
            let excess = self.tail.len().saturating_sub(Self::KEEP);
            self.tail.drain(..excess);
        }
        hit
    }
}

/// Issue #473: scan a byte region for terminal color queries emitted by the
/// pane's child and return the matching `color_query_pending` bitmask.
/// Recognized queries (all observed passing through ConPTY's output pipe):
///   OSC 4;<i>;?  → bit i (0-15)      palette entry query
///   OSC 10;?     → COLOR_QUERY_FG    foreground query
///   OSC 11;?     → COLOR_QUERY_BG    background query
///   CSI ?996n    → COLOR_QUERY_SCHEME light/dark query
/// Only matches whose final byte lands at offset >= `min_end` are counted —
/// the boundary rescan uses this so a match fully inside the carried tail
/// (already reported last batch) is not double-counted, which would trigger
/// a duplicate response after the first one was drained.
fn scan_color_queries(data: &[u8], min_end: usize) -> u32 {
    if !data.contains(&0x1b) { return 0; }
    let mut bits: u32 = 0;
    let mut i = 0;
    while i < data.len() {
        if data[i] != 0x1b { i += 1; continue; }
        let rest = &data[i..];
        // CSI ?996n  (light/dark scheme query)
        const SCHEME: &[u8] = b"\x1b[?996n";
        if rest.starts_with(SCHEME) {
            if i + SCHEME.len() > min_end { bits |= crate::types::COLOR_QUERY_SCHEME; }
            i += SCHEME.len();
            continue;
        }
        // OSC 10;? / OSC 11;?  (fg/bg query)
        for (pat, bit) in [(&b"\x1b]10;?"[..], crate::types::COLOR_QUERY_FG),
                           (&b"\x1b]11;?"[..], crate::types::COLOR_QUERY_BG)] {
            if rest.starts_with(pat) && i + pat.len() > min_end {
                bits |= bit;
            }
        }
        // OSC 4;<index>;?  (palette query)
        const OSC4: &[u8] = b"\x1b]4;";
        if rest.starts_with(OSC4) {
            let mut j = OSC4.len();
            let mut idx: u32 = 0;
            let mut digits = 0;
            while j < rest.len() && rest[j].is_ascii_digit() && digits < 3 {
                idx = idx * 10 + (rest[j] - b'0') as u32;
                j += 1;
                digits += 1;
            }
            if digits > 0 && j + 1 < rest.len() && rest[j] == b';' && rest[j + 1] == b'?' {
                if idx < 16 && i + j + 2 > min_end {
                    bits |= 1 << idx;
                }
                i += j + 2;
                continue;
            }
        }
        i += 1;
    }
    bits
}

/// Cross-batch detector for color queries, same rationale as `CprScanner`:
/// a query split across two coalesced batches is invisible to a per-batch
/// scan.  Carries the last KEEP bytes and rescans the boundary, counting only
/// matches that end inside the new batch.
struct ColorQueryScanner {
    tail: Vec<u8>,
}

impl ColorQueryScanner {
    /// One less than the longest query (`\x1b]4;15;?` = 9 bytes).
    const KEEP: usize = 8;

    fn new() -> Self {
        Self { tail: Vec::with_capacity(Self::KEEP) }
    }

    fn scan(&mut self, batch: &[u8]) -> u32 {
        let mut bits = scan_color_queries(batch, 0);
        if !self.tail.is_empty() {
            let mut boundary = self.tail.clone();
            let tail_len = boundary.len();
            boundary.extend_from_slice(&batch[..batch.len().min(Self::KEEP)]);
            // Only count matches that END past the carried tail — anything
            // fully inside the tail was already reported last batch.  A match
            // ends past the tail when its exclusive end exceeds tail_len.
            bits |= scan_color_queries(&boundary, tail_len);
        }
        if batch.len() >= Self::KEEP {
            self.tail.clear();
            self.tail.extend_from_slice(&batch[batch.len() - Self::KEEP..]);
        } else {
            self.tail.extend_from_slice(batch);
            let excess = self.tail.len().saturating_sub(Self::KEEP);
            self.tail.drain(..excess);
        }
        bits
    }
}

// TODO: The 8 Arc parameters below should be grouped into a `ReaderSignals`
// struct the next time a new signal is added, to keep the call-site manageable.
pub fn spawn_reader_thread(
    mut reader: Box<dyn std::io::Read + Send>,
    term_reader: Arc<Mutex<vt100::Parser>>,
    dv_writer: Arc<std::sync::atomic::AtomicU64>,
    cursor_shape: Arc<std::sync::atomic::AtomicU8>,
    bell_pending: Arc<std::sync::atomic::AtomicBool>,
    cpr_pending: Arc<std::sync::atomic::AtomicBool>,
    color_query_pending: Arc<std::sync::atomic::AtomicU32>,
    output_ring: Arc<Mutex<std::collections::VecDeque<u8>>>,
    pane_id: usize,
    child_pid: Option<u32>,
) {
    // ── Issue #246: split the old single reader thread into two threads ──
    //
    // The old code did `reader.read() → parser.lock() → parser.process(chunk)
    // → drop lock` for each chunk individually. When a TUI (Ink, PSReadLine,
    // pwsh-in-docker, etc.) emits a logical frame larger than the 64KB read
    // buffer — or when ConPTY/docker stdio splits a smaller frame across
    // multiple reads — the snapshot path in src/layout.rs could win the race
    // for the parser mutex BETWEEN two of those reads, serializing a partial
    // mid-frame state (typically: `ESC[2K` cleared a row but only some
    // `CUP+text` spans had landed). That partial state was rendered on the
    // client as the visible "sparse cells" / "remnant characters" artifact.
    //
    // Fix:
    //   • Reader thread: tight loop, ONLY does reader.read() and pushes raw
    //     bytes into a staging buffer. Never touches the parser mutex, so
    //     reads cannot be starved by snapshot work.
    //   • Parser thread: waits for staged bytes, then ADAPTIVELY coalesces:
    //     sleeps 1ms; if more bytes arrived, sleeps again; hard cap 8ms total.
    //     Then locks the parser ONCE and processes the entire batch
    //     atomically. Multi-chunk frames that arrive within the coalescing
    //     window land as a single unit — the snapshot can no longer observe
    //     a partial frame.
    //
    // Latency cost: 1ms minimum between byte arrival and render for streaming
    // output, capped at 8ms for sustained streams. Imperceptible to humans
    // and well below the 50ms keystroke-echo threshold.
    const COALESCE_TICK_MS: u64 = 1;
    const COALESCE_MAX_MS: u128 = 8;

    let staging: Arc<(Mutex<Vec<u8>>, Condvar)> = Arc::new((Mutex::new(Vec::with_capacity(131072)), Condvar::new()));
    let reader_done: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    // ── Reader thread: pure I/O, no parser lock ──
    let staging_r = staging.clone();
    let reader_done_r = reader_done.clone();
    let output_ring_r = output_ring.clone();
    thread::spawn(move || {
        // TEST-ONLY (PSMUX_TEST_READER_DELAY_MS): hold off this reader's first
        // read by a fixed duration. The reader is what drains conhost's startup
        // ESC[6n cursor-position request; with PSEUDOCONSOLE_INHERIT_CURSOR set,
        // conhost will not service the child's console connection until that
        // request is read and answered, so delaying the reader stalls the child
        // in ConsoleCreateConnectionObject for the duration -- the exact race
        // otherwise hit only under concurrent load. Without the flag conhost
        // issues no such request, so the delay only postpones psmux's parse of
        // the pane while the child comes up promptly. Placed here, not in
        // create_window, so it delays only the reader and not window
        // registration. Inert unless the env var is set; compiled out of release
        // builds. REMOVE with tests/test_first_pane_cpr_hang.ps1.
        #[cfg(debug_assertions)]
        {
            if let Ok(ms) = std::env::var("PSMUX_TEST_READER_DELAY_MS") {
                if let Ok(ms) = ms.parse::<u64>() {
                    if ms > 0 { thread::sleep(Duration::from_millis(ms)); }
                }
            }
        }
        let mut local = vec![0u8; 65536];
        let mut zero_reads: u32 = 0;
        let mut color_scanner = ColorQueryScanner::new();
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    zero_reads = 0;
                    // Push raw bytes into staging (no parser lock involved).
                    let (lock, cv) = &*staging_r;
                    if let Ok(mut buf) = lock.lock() {
                        buf.extend_from_slice(&local[..n]);
                        cv.notify_one();
                    }
                    // Issue #556: answer terminal color queries (OSC 4/10/11,
                    // CSI ?996n, issue #473) HERE, straight off the read,
                    // rather than signaling the server loop.  The old
                    // parser-thread detection + server-loop answer added
                    // 1-8ms of coalescing plus a loop tick, landing the
                    // reply after a startup probe's read window (closed by
                    // ConPTY's own DA1 auto-answer) — yazi then re-parsed
                    // the late reply as an interactive `shell` action.  The
                    // scan is one `memchr` for ESC when no query is present.
                    let color_query_bits = color_scanner.scan(&local[..n]);
                    if color_query_bits != 0 {
                        let colors = crate::types::shared_host_colors();
                        if !crate::server::helpers::answer_color_queries_sync(
                            color_query_bits, child_pid, &colors,
                        ) {
                            // Injection unavailable (no child pid, or a
                            // non-Windows build): keep the #473 server-loop
                            // pipe path as delivery of record.
                            color_query_pending.fetch_or(color_query_bits, Ordering::AcqRel);
                            crate::types::COLOR_QUERY_PENDING.store(true, Ordering::Release);
                        }
                    }
                    // Append raw output to ring buffer for control mode %output.
                    // This is independent of parser state and must stay live.
                    if let Ok(mut ring) = output_ring_r.lock() {
                        const MAX_RING: usize = 65536;
                        let space = MAX_RING.saturating_sub(ring.len());
                        if n <= space {
                            ring.extend(&local[..n]);
                        } else {
                            let drop_count = (n - space).min(ring.len());
                            ring.drain(..drop_count);
                            ring.extend(&local[..n]);
                        }
                    }
                    // Issue #440: tee raw pane output to any registered pipe-pane
                    // writer for this pane. The atomic gate keeps this a single
                    // relaxed load (no mutex) whenever nothing is being piped,
                    // which is the common case. When a writer's child has exited
                    // its pipe write fails; drop that entry and decrement the gate
                    // so the reader stops trying.
                    if crate::types::PIPE_PANE_COUNT.load(Ordering::Relaxed) > 0 {
                        if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                            use std::io::Write;
                            let mut i = 0;
                            while i < writers.len() {
                                if writers[i].0 == pane_id {
                                    let w = &mut writers[i].1;
                                    if w.write_all(&local[..n]).is_err() || w.flush().is_err() {
                                        writers.remove(i);
                                        crate::types::PIPE_PANE_COUNT.fetch_sub(1, Ordering::Relaxed);
                                        continue;
                                    }
                                }
                                i += 1;
                            }
                        }
                    }
                }
                Ok(_) => {
                    zero_reads += 1;
                    if zero_reads > 10 { break; }
                    thread::sleep(Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
        // Issue #440: this pane is gone. Drop any pipe-pane writer still bound
        // to it so the piped command sees EOF instead of blocking forever on a
        // stdin that will never fill, and keep the count gate in sync. pane_id
        // is monotonic and never reused, so this can only match this pane.
        if crate::types::PIPE_PANE_COUNT.load(Ordering::Relaxed) > 0 {
            if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                let before = writers.len();
                writers.retain(|(id, _)| *id != pane_id);
                let removed = before - writers.len();
                if removed > 0 {
                    crate::types::PIPE_PANE_COUNT.fetch_sub(removed, Ordering::Relaxed);
                }
            }
        }
        // Signal end-of-stream and wake parser thread one last time so it
        // can drain remaining bytes and run the alt-screen cleanup.
        reader_done_r.store(true, Ordering::Release);
        let (_, cv) = &*staging_r;
        cv.notify_all();
    });

    // ── Parser thread: coalesces staged bytes, processes under one lock ──
    thread::spawn(move || {
        let mut cpr_scanner = CprScanner::new();
        loop {
            // Wait for at least one byte (or shutdown).
            {
                let (lock, cv) = &*staging;
                let mut buf = match lock.lock() {
                    Ok(g) => g,
                    Err(_) => break,
                };
                while buf.is_empty() {
                    if reader_done.load(Ordering::Acquire) {
                        // Reader is gone and nothing left to drain — exit
                        // after running alt-screen cleanup below.
                        drop(buf);
                        if let Ok(mut parser) = term_reader.lock() {
                            if parser.screen().alternate_screen() {
                                parser.process(b"\x1b[?25h\x1b[?1049l");
                                cursor_shape.store(0, Ordering::Release);
                                dv_writer.fetch_add(1, Ordering::Release);
                                crate::types::PTY_DATA_READY.store(true, Ordering::Release);
                            }
                        }
                        return;
                    }
                    let res = cv.wait_timeout(buf, Duration::from_millis(100));
                    buf = match res {
                        Ok((g, _)) => g,
                        Err(_) => return,
                    };
                }
                // First bytes are present — release the lock so the reader can
                // keep pushing while we run the adaptive coalescing wait.
            }

            // Adaptive coalescing: keep waiting in 1ms ticks while new bytes
            // are still arriving, hard-capped at 8ms total. This bridges
            // multi-chunk frames into a single atomic parser update.
            let coalesce_start = Instant::now();
            let mut last_len: usize = {
                let (lock, _) = &*staging;
                lock.lock().map(|b| b.len()).unwrap_or(0)
            };
            loop {
                if coalesce_start.elapsed().as_millis() >= COALESCE_MAX_MS { break; }
                thread::sleep(Duration::from_millis(COALESCE_TICK_MS));
                let cur_len = {
                    let (lock, _) = &*staging;
                    lock.lock().map(|b| b.len()).unwrap_or(0)
                };
                if cur_len == last_len {
                    // No new bytes arrived in the last tick — frame boundary.
                    break;
                }
                last_len = cur_len;
            }

            // Take the entire staged batch.
            let bytes = {
                let (lock, _) = &*staging;
                match lock.lock() {
                    Ok(mut buf) => std::mem::take(&mut *buf),
                    Err(_) => break,
                }
            };
            if bytes.is_empty() { continue; }

            // Scan for cursor shape and RMCUP on the raw batch BEFORE
            // handing to vt100 parser (preserves prior ordering semantics).
            if let Some(shape) = scan_cursor_shape(&bytes) {
                cursor_shape.store(shape, Ordering::Release);
            }
            let rmcup = scan_rmcup(&bytes);
            let has_cpr_query = cpr_scanner.scan(&bytes);

            // Issue #502 diagnostic: capture the exact pre-parse byte stream
            // when PSMUX_PANE_RAW=1. Off by default, one atomic load when off.
            crate::debug_log::pane_raw(&bytes);

            if let Ok(mut parser) = term_reader.lock() {
                parser.process(&bytes);
                if parser.screen_mut().take_audible_bell() {
                    bell_pending.store(true, Ordering::Release);
                }
            }
            // When TUI sends RMCUP, reset cursor shape so it doesn't
            // persist from the exiting TUI app.
            if rmcup {
                cursor_shape.store(0, Ordering::Release);
            }
            // Signal the main loop to inject a CPR response (ESC[row;colR).
            // pwsh emits ESC[6n at startup and after session events such as
            // lock/unlock; the main loop writes the response via pane.writer.
            if has_cpr_query {
                cpr_pending.store(true, Ordering::Release);
                crate::types::CPR_DATA_PENDING.store(true, Ordering::Release);
            }
            // Issue #473 color queries are scanned and answered in the READER
            // thread above (issue #556): the coalescing wait this thread runs
            // before parsing is exactly the latency that made replies miss
            // startup probe windows.
            dv_writer.fetch_add(1, Ordering::Release);
            crate::types::PTY_DATA_READY.store(true, Ordering::Release);
        }
    });
}

#[cfg(test)]
#[path = "../tests-rs/test_issue399_env_prefix.rs"]
mod test_issue399_env_prefix;

#[cfg(test)]
#[path = "../tests-rs/test_issue151_strict_mode.rs"]
mod test_issue151_strict_mode;

#[cfg(test)]
#[path = "../tests-rs/test_issue155_output_rendering.rs"]
mod test_issue155_output_rendering;

#[cfg(test)]
#[path = "../tests-rs/test_issue165_prediction_view_style.rs"]
mod test_issue165_prediction_view_style;

#[cfg(test)]
#[path = "../tests-rs/test_issue271_warm_pane_history.rs"]
mod test_issue271_warm_pane_history;

#[cfg(test)]
#[path = "../tests-rs/test_issue88_alt_screen_toggle.rs"]
mod test_issue88_alt_screen_toggle;

#[cfg(test)]
#[path = "../tests-rs/test_cpr_responder.rs"]
mod test_cpr_responder;

#[cfg(test)]
#[path = "../tests-rs/test_issue473_color_queries.rs"]
mod test_issue473_color_queries;

#[cfg(test)]
#[path = "../tests-rs/test_windowsapps_alias_shell.rs"]
mod test_windowsapps_alias_shell;

#[cfg(test)]
#[path = "../tests-rs/test_issue475_claude_wrapper.rs"]
mod test_issue475_claude_wrapper;

#[cfg(test)]
#[path = "../tests-rs/test_issue399_teammate_settings_priority.rs"]
mod test_issue399_teammate_settings_priority;

#[cfg(test)]
mod test_parser_audible_bell {
    /// Helper: create a parser, process bytes, return whether bell rang.
    fn bell_after(data: &[u8]) -> bool {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(data);
        p.screen_mut().take_audible_bell()
    }

    /// Helper: process two chunks sequentially (simulates cross-chunk reads),
    /// return whether bell rang after the second chunk.
    fn bell_after_two_chunks(chunk1: &[u8], chunk2: &[u8]) -> bool {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(chunk1);
        // Consume any bell from chunk1 so we only test chunk2
        let _ = p.screen_mut().take_audible_bell();
        p.process(chunk2);
        p.screen_mut().take_audible_bell()
    }

    #[test]
    fn bare_bel() {
        assert!(bell_after(b"\x07"));
    }

    #[test]
    fn bel_in_plain_text() {
        assert!(bell_after(b"hello\x07world"));
    }

    #[test]
    fn osc_title_with_bel_terminator() {
        // OSC BEL terminator is NOT an audible bell
        assert!(!bell_after(b"\x1b]0;My Title\x07"));
    }

    #[test]
    fn osc_title_with_st_terminator() {
        assert!(!bell_after(b"\x1b]0;My Title\x1b\\"));
    }

    #[test]
    fn osc_then_standalone_bel() {
        // OSC terminated by BEL, then a real standalone BEL
        assert!(bell_after(b"\x1b]0;title\x07\x07"));
    }

    #[test]
    fn multiple_osc_no_real_bel() {
        assert!(!bell_after(b"\x1b]0;title1\x07\x1b]2;title2\x07"));
    }

    #[test]
    fn empty_data() {
        assert!(!bell_after(b""));
    }

    #[test]
    fn no_bel_at_all() {
        assert!(!bell_after(b"just text\x1b[31m"));
    }

    #[test]
    fn powershell_prompt_title_no_bell() {
        // Simulates PowerShell: sets title via OSC, then prints prompt (no BEL)
        let data = b"\x1b]0;PS C:\\Users\\test\x07\x1b[32mPS>\x1b[0m ";
        assert!(!bell_after(data));
    }

    #[test]
    fn take_clears_flag() {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x07");
        assert!(p.screen_mut().take_audible_bell());
        // Second take should be false (consumed)
        assert!(!p.screen_mut().take_audible_bell());
    }

    #[test]
    fn cross_chunk_osc_then_real_bel() {
        // Chunk 1 starts OSC without terminator; chunk 2 has the
        // OSC terminator BEL then a real standalone BEL.
        // The parser maintains state across chunks, so this works
        // correctly (unlike the old stateless scan_standalone_bel).
        let mut p = vt100::Parser::new(24, 80, 0);
        p.process(b"\x1b]0;title");
        assert!(!p.screen_mut().take_audible_bell());
        p.process(b"\x07\x07");
        assert!(p.screen_mut().take_audible_bell());
    }

    #[test]
    fn cross_chunk_osc_no_real_bel() {
        // Chunk 1 starts OSC; chunk 2 only has the OSC terminator.
        // No real bell should fire.
        assert!(!bell_after_two_chunks(b"\x1b]0;title", b"\x07"));
    }
}

// reap_children is in tree.rs

#[cfg(test)]
#[path = "../tests-rs/test_issue450_dead_warm_pane.rs"]
mod tests_issue450_dead_warm_pane;

#[cfg(test)]
#[path = "../tests-rs/test_warm_pane_start_dir.rs"]
mod tests_warm_pane_start_dir;

#[cfg(test)]
#[path = "../tests-rs/test_issue474_unix_shells.rs"]
mod tests_issue474_unix_shells;

#[cfg(test)]
#[path = "../tests-rs/test_issue492_493_direct_spawn.rs"]
mod tests_issue492_493_direct_spawn;

#[cfg(test)]
#[path = "../tests-rs/test_issue495_cwd_hook_gate.rs"]
mod tests_issue495_cwd_hook_gate;

#[cfg(test)]
#[path = "../tests-rs/test_issue495_direct_spawn_cwd_hook.rs"]
mod tests_issue495_direct_spawn_cwd_hook;

#[cfg(test)]
#[path = "../tests-rs/test_pane_writer_queue.rs"]
mod tests_pane_writer_queue;

#[cfg(test)]
#[path = "../tests-rs/test_pane_writer_transient_error.rs"]
mod tests_pane_writer_transient_error;

#[cfg(test)]
#[path = "../tests-rs/test_dashdash_window_name.rs"]
mod tests_dashdash_window_name;

#[cfg(test)]
#[path = "../tests-rs/test_issue600_bash_rehome.rs"]
mod tests_issue600_bash_rehome;
