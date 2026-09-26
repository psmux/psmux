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
                crate::pty_trace::mark("w", 0, &buf);
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
        // tmux seeds `default-shell` from $SHELL before anything else
        // (tmux.c getshell(), gated by checkshell()). An explicit SHELL is the
        // user's choice, so it wins over the pwsh > powershell > cmd walk and
        // over the SSH store guard below (#683).
        if let Some(from_env) = shell_from_env() {
            return Some(from_env);
        }
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

/// The shell named by the SHELL environment variable, if it is one psmux can
/// actually spawn (#683).
///
/// tmux's `checkshell()` (tmux.c) accepts $SHELL only when it is an absolute
/// path, executable, and not tmux itself (`areshell()`). The Windows reading of
/// that rule: an absolute path must exist and carry an executable extension; a
/// bare name (`powershell`, `nu`, `wsl`) is resolved on PATH the same way the
/// `default-shell` option already accepts one; anything else, and in particular
/// the POSIX style `/usr/bin/bash` Git Bash exports, is not a Windows shell and
/// is ignored so the pwsh > powershell > cmd walk stays in charge.
pub fn shell_from_env() -> Option<String> {
    shell_from_env_value(std::env::var("SHELL").ok()?.as_str())
}

/// `shell_from_env` over an explicit value, so the rule is unit testable
/// without touching the process environment.
pub fn shell_from_env_value(raw: &str) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }
    let path = std::path::Path::new(value);
    let resolved = if path.is_absolute() {
        if !path.is_file() {
            return None;
        }
        value.to_string()
    } else if value.contains(['/', '\\']) {
        // `/usr/bin/bash`, `./sh`, `bin\sh.exe`: not an absolute Windows path.
        return None;
    } else {
        which::which(value).ok()?.to_string_lossy().into_owned()
    };
    let resolved_path = std::path::Path::new(&resolved);
    let ext = resolved_path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // CreateProcess can start these directly; a `.ps1` or an extensionless
    // file cannot be a shell here, just as a non executable fails X_OK.
    if !matches!(ext.as_str(), "exe" | "com" | "bat" | "cmd") {
        return None;
    }
    // areshell(): tmux refuses to use itself as the shell.
    let stem = resolved_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(stem.as_str(), "psmux" | "tmux" | "pmux") {
        return None;
    }
    Some(prefer_app_execution_alias(resolved))
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

/// Quote one command word the way tmux's `args_escape` (arguments.c) picks
/// quotes, so `#{pane_start_command}` reads like tmux's: a word carrying a
/// space or a shell metacharacter comes back quoted, a plain one bare.
///
/// tmux also runs the word through `utf8_stravis(VIS_OCTAL|VIS_CSTYLE|...)`,
/// which turns every `\` into `\\`. That is deliberate on Unix and actively
/// wrong here — a Windows command is full of literal backslashes
/// (`C:\tools\hud.ps1`), and doubling them would corrupt the very string a
/// tool matches on. So psmux keeps the quote SELECTION and drops the
/// backslash escaping.
fn quote_start_command_word(word: &str) -> String {
    // tmux: `static const char dquoted[] = " #';${}%";` then
    // `static const char squoted[] = " \"";`
    const DQUOTED: &[char] = &[' ', '\t', '#', '\'', ';', '$', '{', '}', '%'];
    const SQUOTED: &[char] = &[' ', '\t', '"'];
    if word.is_empty() {
        return "''".to_string();
    }
    if word.contains(DQUOTED) {
        format!("\"{}\"", word)
    } else if word.contains(SQUOTED) {
        format!("'{}'", word)
    } else {
        word.to_string()
    }
}

/// What to STORE in `Pane::start_command` for a pane about to be spawned with
/// `command` (issue #580).
///
/// tmux stores the spawn argv on the pane (`window_pane.argc/argv`) and
/// stringifies it only on demand (format.c `format_cb_start_command` ->
/// `cmd_stringify_argv`); the stored copy is the one a later bare respawn
/// re-executes. psmux mirrors that by storing the operand VERBATIM, exactly as
/// `build_command` would receive it (the `-- prog args` argv form keeps its
/// `--` marker), so the value round-trips back through `build_command` on a
/// respawn. Rendering for `#{pane_start_command}` happens in
/// `start_command_display`.
///
/// It is captured BEFORE `build_command` rewrites anything, so none of psmux's
/// spawn machinery (the `cat` blocker substitution, the shell wrapper, the
/// warm pool's `powershell-pane.cmd`) can leak into the variable.
pub fn start_command_raw(command: Option<&str>) -> String {
    let Some(cmd) = command else { return String::new() };
    let trimmed = cmd.trim();
    if trimmed == "--" { String::new() } else { trimmed.to_string() }
}

/// `start_command_raw` for the raw-argv window path (`create_window_raw`),
/// where the tokens arrive already split. Stored in the `--` form so a later
/// respawn re-execs the same argv rather than re-parsing it as a shell string.
pub fn start_command_raw_argv(raw_args: &[String]) -> String {
    if raw_args.is_empty() {
        return String::new();
    }
    let quoted: Vec<String> = raw_args.iter().map(|a| quote_start_command_word(a)).collect();
    format!("-- {}", quoted.join(" "))
}

/// Render a stored `Pane::start_command` for `#{pane_start_command}`.
///
/// tmux's `cmd_stringify_argv` returns `""` for an empty argv (so a
/// default-shell pane reports EMPTY, never the shell path) and otherwise joins
/// the escaped argv words. Matching that here:
///
/// * `""` (default shell, warm-pool claim included) -> `""`
/// * `-- prog args` -> `prog args` (the token list, already per-token quoted)
/// * any other string -> that string as ONE word, quoted if it needs it
pub fn start_command_display(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(rest) = trimmed.strip_prefix("--") {
        if rest.is_empty() {
            return String::new();
        }
        if rest.starts_with(char::is_whitespace) {
            return rest.trim().to_string();
        }
    }
    quote_start_command_word(trimmed)
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

/// The lowercased file stem of one program path: `C:\Program Files\Git\bin\bash.exe`
/// and `bash` both reduce to `bash`.
fn program_stem(program: &str) -> String {
    std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase()
}

/// The rehome dialect a shell's basename implies, or `None` when psmux does
/// not know the shell. The names are the ones `docs/multi-shell.md` documents.
fn rehome_syntax_for_stem(stem: &str) -> Option<RehomeSyntax> {
    if POSIX_SHELL_STEMS.contains(&stem) || matches!(stem, "ash" | "busybox" | "git-bash") {
        // `git-bash` is Git Bash's GUI launcher; remap_git_bash_launcher
        // rewrites it to the console `bash.exe` beside it when that file is
        // there, and when it is not the launcher still names bash.
        Some(RehomeSyntax::Posix)
    } else if stem == "cmd" {
        Some(RehomeSyntax::Cmd)
    } else if stem == "pwsh" || stem == "powershell" {
        Some(RehomeSyntax::PowerShell)
    } else {
        None
    }
}

/// Read `value` as ONE program path and return its stem, for the case where
/// `resolve_shell_program` could not resolve it.
///
/// That resolver identifies the program by looking it up on disk and on PATH,
/// and only falls back to a quote-aware split when the lookup fails. So an
/// UNQUOTED absolute path containing spaces classifies correctly on a machine
/// where it exists and wrongly on one where it does not: the split turns
/// `C:\Program Files\Git\bin\bash.exe` into the program `C:\Program` plus two
/// arguments, and the dialect then comes from `Program` (#672). Which dialect
/// a pane's shell speaks cannot depend on the filesystem, so when the resolved
/// program names no shell psmux knows, the whole value is read as a single
/// path instead.
///
/// Only when nothing in the value looks like an argument: a real command line
/// such as `pwsh -File C:\x\bash.ps1` ends in a path of its own and must never
/// be read this way.
fn whole_value_program_stem(value: &str) -> Option<String> {
    if value.contains('"') {
        return None;
    }
    let mut tokens = value.split_whitespace();
    tokens.next()?;
    if tokens.any(|t| t.starts_with('-') || t.starts_with('/')) {
        return None;
    }
    Some(program_stem(value))
}

/// Pick the rehome syntax from the shell that is actually running in the pane.
///
/// `shell` is the configured `default-shell` (possibly a whole command line
/// with arguments, possibly empty). Empty means psmux spawned its own default
/// shell, which is pwsh/powershell on Windows and a POSIX shell elsewhere.
/// An unrecognised program keeps the platform default rather than guessing.
///
/// The decision is made on the program's BASENAME, so every spelling of one
/// shell agrees: bare name or absolute path, forward or backslashes, with or
/// without `.exe`, any letter case, quoted, and with arguments (#672).
pub(crate) fn rehome_syntax_for_shell(shell: &str) -> RehomeSyntax {
    let platform_default = if cfg!(windows) { RehomeSyntax::PowerShell } else { RehomeSyntax::Posix };
    let shell = shell.trim();
    if shell.is_empty() {
        return platform_default;
    }
    let (program, _) = resolve_shell_program(shell);
    let program = remap_git_bash_launcher(program);
    if let Some(syntax) = rehome_syntax_for_stem(&program_stem(&program)) {
        return syntax;
    }
    // The resolver named no shell psmux knows. Before falling back to the
    // platform default, read the value as one path: that is what an unquoted
    // Git Bash path does on a machine where it is not installed (#672).
    if let Some(stem) = whole_value_program_stem(shell) {
        if let Some(syntax) = rehome_syntax_for_stem(&stem) {
            return syntax;
        }
    }
    platform_default
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
    // Remember where the pane was ASKED to be, and where its process actually
    // is right now, which is wherever the pool spawned it, i.e. the server's
    // own working directory.  `#{pane_current_path}` reports the request until
    // the reading moves off that directory, so the answer is never "wherever
    // the psmux server happens to be running from" (#615 follow up).  The
    // reading is taken with the same function the format path uses, so the two
    // are directly comparable.
    let stale = pane
        .child_pid
        .and_then(crate::platform::process_info::get_foreground_cwd)
        .or_else(|| {
            // Unreadable right now (a spare still coming up can refuse the
            // handle for a moment).  The pool spawns its spares in the server's
            // own directory, so that is the reading to expect once it can be
            // taken, and the one value `#{pane_current_path}` must never answer
            // with for a pane that asked for somewhere else.
            std::env::current_dir().ok().map(|d| d.to_string_lossy().into_owned())
        })
        .map(|c| crate::util::normalize_dir_for_display(&c));
    pane.cwd_hint = Some(crate::types::CwdHint {
        pid: pane.child_pid,
        requested: crate::util::normalize_dir_for_display(dir),
        stale,
        settled: std::sync::atomic::AtomicBool::new(false),
    });
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
    // #607: the new window's pane becomes the active one, so hand the pane
    // that is losing focus its own copy mode back before that happens and let
    // the new pane adopt its own (a fresh pane has none) afterwards.  Without
    // this the global `Mode::CopyMode` just stayed put and the brand new
    // window opened in copy mode.
    let prev_pane = crate::copy_mode::active_pane_id(app);
    crate::copy_mode::park_mode_on_active_pane(app);
    // ── Empty window (tmux new-window -E): a new window whose single pane has
    // no command/process. It renders blank until respawn-pane gives it one. ──
    if empty {
        let area = app.client_area;
        let rows = (if area.height > 1 { area.height } else { 30 }).max(MIN_PANE_DIM);
        let cols = (if area.width > 1 { area.width } else { 120 }).max(MIN_PANE_DIM);
        let pane_id = app.next_pane_id;
        if let Some(pane) = crate::popup::create_empty_pane(rows, cols, pane_id) {
            app.next_pane_id += 1;
            app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: hostname_cached(), id: app.next_win_id, area: app.client_area, window_size: None, window_options: Default::default(), activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
            app.next_win_id += 1;
            app.active_idx = app.windows.len() - 1;
            app.on_window_appended();
        }
        crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
    let warm_eligible = command.is_none() && extra_env.is_empty() && !default_shell_needs_fresh_eval(&app.default_shell);
    // A ready spare if there is one, else the oldest warming spare, else
    // nothing and a cold spawn below. See `WarmPool::claim`.
    let (mut claimed, mut was_ready) = if warm_eligible { app.warm_pane.claim() } else { (None, false) };
    if warm_eligible && claimed.is_none() {
        // Empty, but refills are already on their way. See `await_inflight_spare`.
        let (c, r) = await_inflight_spare(app, WARM_INFLIGHT_WAIT);
        claimed = c;
        was_ready = r;
    }
    if warm_eligible {
        app.warm_pane.note_claim(was_ready);
        if !was_ready {
            crate::warm_trace!(
                "claim(new-window): NO READY SPARE (depth={} got_warming={}) -- surging",
                app.warm_pane.len(),
                claimed.is_some()
            );
        }
        // Start the replacement batch now. If this claim missed, the caller is
        // about to wait out a shell startup either way, and the batch should be
        // booting during that wait rather than after it.
        schedule_warm_refill(app);
    }
    if claimed.is_none() {
        // Nothing to hand over, so the cold path below is about to take
        // `next_pane_id` for itself. Every id the in flight refills already
        // reserved is lower than that, so those spares may no longer be handed
        // out: doing so walked the visible sequence backwards (`%2 %3 %12 %4`).
        // Retire them now and refuse them on arrival. Applies whether the pool
        // was empty or the transplant was bypassed outright (a command, `-e`, a
        // dynamic default-shell).
        let dropped = app.warm_pane.set_issued_floor(app.next_pane_id);
        if dropped > 0 {
            crate::warm_trace!(
                "pool: id floor raised to {}, discarded {} spare(s) whose ids are now in the past",
                app.next_pane_id, dropped
            );
        }
    }
    if let Some(mut wp) = claimed {
        crate::warm_trace!(
            "claim(new-window): pane={} spare_age={:.1}ms pool_left={} ready_left={}",
            wp.pane_id,
            wp.spawned_at.elapsed().as_micros() as f64 / 1000.0,
            app.warm_pane.len(),
            app.warm_pane.ready_len()
        );
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
            let mut pane = Pane { master: wp.master, writer: wp.writer, child: wp.child, term: wp.term, last_rows: rows, last_cols: cols, id: wp.pane_id, title: hostname_cached(), title_locked: false, child_pid: wp.child_pid, data_version: wp.data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None, cursor_shape: wp.cursor_shape, bell_pending: wp.bell_pending, cpr_pending: wp.cpr_pending, color_query_pending: wp.color_query_pending, copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring: wp.output_ring, spawned_at: Some(std::time::Instant::now()), start_command: String::new(), cwd_hint: None };
            // A warm-pool spare is only claimed for a DEFAULT-SHELL pane
            // (`command.is_none()` gates this whole branch), so its
            // `#{pane_start_command}` is the empty string — the wrapper the
            // pool actually spawned (`powershell-pane.cmd`) is an
            // implementation detail and must never surface (#580).
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
            app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, window_options: Default::default(), activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![initial_pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
            app.next_win_id += 1;
            app.active_idx = app.windows.len() - 1;
            app.on_window_appended();
            crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
    crate::startup_trace::mark("srv.pty.open");
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    crate::startup_trace::mark("srv.pty.ready");

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
    if crate::startup_trace::on() {
        crate::startup_trace::mark_detail("srv.child.argv", &format!("{:?}", shell_cmd.get_argv()));
    }
    let child = pair
        .slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    crate::startup_trace::mark("srv.child.spawned");
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
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()), start_command: start_command_raw(command), cwd_hint: None };
    app.next_pane_id += 1;
    let win_name = command.map(|c| default_shell_name(Some(c), None)).unwrap_or_else(|| default_shell_name(None, configured_shell));
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, window_options: Default::default(), activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    app.on_window_appended();
    crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
    let params = warm_spawn_params(app)
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "warm panes disabled"))?;
    let out = spawn_warm_pane_from(pty_system, &params);
    // Synchronous spawn: the caller owns the result the instant this returns,
    // on the same thread that would run a teardown, so nothing is in flight
    // afterwards (#686).
    crate::warm_pane_sync::inflight::release(params.pane_id);
    out
}

/// Take one finished refill off the channel and put it in the pool, or drop it.
///
/// Shared by the server loop's drain and by [`await_inflight_spare`], because
/// both have to apply the same two rules: release the reservation, and refuse a
/// spare whose palette the server has already disowned. A spawn that was already
/// in flight when the palette changed lands carrying the OLD palette, and the
/// palette is planted in the child's environment, so letting it through would
/// hand the next window colours the server no longer has (#473 follow up).
pub fn land_spare(app: &mut AppState, slot: Option<crate::types::WarmPane>) {
    app.warm_pane.inflight = app.warm_pane.inflight.saturating_sub(1);
    match slot {
        Some(mut wp) => {
            // The server owns this child now: whatever happens to it below, it
            // is either in the pool (covered by `WarmPool::kill_all`) or killed
            // right here, so the teardown reaper must stop tracking it (#686).
            crate::warm_pane_sync::inflight::release(wp.pane_id);
            if wp.host_colors != app.host_colors {
                if let Some(pid) = wp.child_pid {
                    crate::platform::process_kill::kill_pid_tree(pid);
                }
                wp.child.kill().ok();
                crate::warm_trace!(
                    "pool: dropped a spare that landed with a stale palette, depth={} inflight={}",
                    app.warm_pane.len(), app.warm_pane.inflight
                );
            } else {
                let (id, pid) = (wp.pane_id, wp.child_pid);
                app.warm_pane.push(wp);
                crate::warm_trace!(
                    "pool: spare landed pane={} pid={:?}, depth={} inflight={}",
                    id, pid, app.warm_pane.len(), app.warm_pane.inflight
                );
            }
        }
        None => {
            crate::warm_trace!(
                "pool: refill failed, depth={} inflight={}",
                app.warm_pane.len(), app.warm_pane.inflight
            );
        }
    }
}

/// How long a claim will wait for a spare that is already being spawned before
/// giving up and spawning its own.
///
/// This used to be 80ms, "a little over two" of the 20 to 37ms a refill took to
/// post its spare back. That figure was measured when spawns were serialised,
/// which means it was the cost of ONE `CreateProcessW` with the machine
/// otherwise idle. A surge now runs [`WARM_SPAWN_CONCURRENCY`] of them at once
/// and each costs 90 to 190ms, so 80ms expired every single time, and what
/// follows a timeout is not merely one cold spawn (#686):
///
///   the claim cold spawns and takes the next pane id, which raises the pool's
///   issued floor above every id the in flight batch reserved, so all of those
///   spares are killed the moment they land ("spare landed, depth=0" in the
///   trace); the pool is then empty again, schedules another full batch, and
///   the next claim repeats it. A burst of ten new windows spawned and killed
///   fifty shells this way and its p50 was 251ms.
///
/// Waiting instead costs at most this budget and usually ends in an in flight
/// spare, which keeps the ids monotonic and the treadmill from ever starting.
/// 250ms covers the slowest spawn measured at the current concurrency; past it
/// the caller still cold spawns exactly as before.
pub const WARM_INFLIGHT_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// A claim found nothing, but refills are already in flight. Wait briefly for
/// the next one to land and use that, instead of spawning a further shell.
///
/// Two things make this the right answer rather than a cold spawn.
///
/// Ids. A spare's pane id is allocated when it is spawned, because it is planted
/// in the shell as TMUX_PANE and a child's environment cannot be rewritten. A
/// cold spawn takes an id above every id the in flight refills reserved, so those
/// spares can never be handed out afterwards without the visible sequence going
/// backwards, and retiring up to eight warming shells to keep it monotonic cost
/// the next creations their spare too (split p90 327 ms, two slow in ten).
/// Waiting keeps them.
///
/// Time. A refill thread posts its spare back as soon as `CreateProcess` returns,
/// measured at 20 to 37 ms, and the shell inside it is already starting. A cold
/// spawn costs that same call on the loop thread AND begins its shell startup
/// from zero. So the wait is never the slower choice.
///
/// Bounded: past the budget, the caller cold spawns as before.
pub fn await_inflight_spare(
    app: &mut AppState,
    budget: std::time::Duration,
) -> (Option<crate::types::WarmPane>, bool) {
    if app.warm_pane.inflight == 0 {
        return (None, false);
    }
    // Taken out and put back, as the plugin drain does with `control_rx`: the
    // receiver cannot be borrowed from `app` while `app` is also being mutated.
    let Some(rx) = app.warm_refill_rx.take() else { return (None, false) };
    let deadline = std::time::Instant::now() + budget;
    let mut out = (None, false);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok(slot) => {
                land_spare(app, slot);
                let got = app.warm_pane.claim();
                if got.0.is_some() {
                    crate::warm_trace!(
                        "claim: waited {:.1}ms for an in flight spare instead of cold spawning",
                        budget.saturating_sub(deadline.saturating_duration_since(std::time::Instant::now())).as_micros() as f64 / 1000.0
                    );
                    out = got;
                    break;
                }
                if app.warm_pane.inflight == 0 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    app.warm_refill_rx = Some(rx);
    out
}

/// How many spare shells may be inside `CreateProcessW` at the same time.
///
/// Not unbounded, and the measurement is the whole argument (#686). Spawns used
/// to be fully serialised by the console state lock, so a surge of eight cost
/// 45ms, then 90, then 135, up to 504ms for the last one. Letting all eight run
/// at once fixed the staircase and broke something else: eight `CreateProcessW`
/// plus eight booting pwsh saturate the machine, so each spawn cost 130 to
/// 330ms instead of 45ms, and the FIRST spare no longer landed inside
/// [`WARM_INFLIGHT_WAIT`]. Every claim then timed out, cold spawned, raised the
/// pool's issued floor, and killed the whole in flight batch on arrival, which
/// spawned another eight: a burst of ten new windows produced sixty shells and
/// a p50 of 251ms, worse than the serialised build.
///
/// Four is where the measurement puts the knee on this machine: three
/// concurrent spawns cost ~64ms each, five cost ~99ms, eight cost 130ms and up.
/// At four the per spawn cost stays near the single spawn figure, so the first
/// spare still lands inside the claim's wait budget, and eight spares take two
/// short waves instead of one long queue.
pub const WARM_SPAWN_CONCURRENCY: usize = 4;

static WARM_SPAWN_SLOTS: (std::sync::Mutex<usize>, std::sync::Condvar) =
    (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// A permit to be inside a ConPTY spawn. Released on drop.
pub struct WarmSpawnPermit;

impl Drop for WarmSpawnPermit {
    fn drop(&mut self) {
        let (m, cv) = &WARM_SPAWN_SLOTS;
        let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
        *n = n.saturating_sub(1);
        cv.notify_one();
    }
}

/// Wait for a spawn slot. `None` means the server is tearing down and this
/// spawn must not happen at all, which is also what keeps the teardown reaper
/// from waiting out its budget on threads that never created a process.
///
/// Only the background spare spawner queues here. A cold spawn on the loop
/// thread is a user waiting on a window, so it never queues behind spares.
pub fn warm_spawn_permit() -> Option<WarmSpawnPermit> {
    let (m, cv) = &WARM_SPAWN_SLOTS;
    let mut n = m.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if crate::warm_pane_sync::inflight::is_tearing_down() {
            return None;
        }
        if *n < WARM_SPAWN_CONCURRENCY {
            *n += 1;
            return Some(WarmSpawnPermit);
        }
        // Timed, so the teardown check above is re-run rather than waited on.
        let (g, _) = cv
            .wait_timeout(n, std::time::Duration::from_millis(5))
            .unwrap_or_else(|e| e.into_inner());
        n = g;
    }
}

/// Bring the spare pool up to its effective target, spawning each missing
/// spare on its own thread.
///
/// Called from the server loop every tick AND from each claim site. The claim
/// site matters more than it looks: a claim that finds no ready spare falls
/// through to a synchronous cold spawn that blocks the loop for a whole shell
/// startup, so a refill left to "the next loop tick" would not begin until the
/// slow creation the user is waiting for had already finished. Scheduling here
/// means the replacement batch boots *while* that cold spawn is happening.
///
/// Nothing happens when the pool is off (`warm off`, `warm-pool-size 0`,
/// `PSMUX_NO_WARM`): those produce a target of zero, hence no deficit. The
/// thread count is bounded by [`crate::types::WARM_POOL_SURGE_MAX`].
pub fn schedule_warm_refill(app: &mut AppState) {
    if !app.warm_enabled {
        return;
    }
    let Some(tx) = app.warm_refill_tx.clone() else { return };
    let standby = app.is_warm_server();
    let target = app.warm_pane.effective_target(standby);
    let deficit = app.warm_pane.deficit_for(target);
    for _ in 0..deficit {
        let Some(params) = warm_spawn_params(app) else { break };
        let spawn_pane_id = params.pane_id;
        let tx = tx.clone();
        app.warm_pane.inflight += 1;
        crate::warm_trace!(
            "pool: refill scheduled pane={} (depth={} ready={} inflight={} target={} surging={})",
            params.pane_id,
            app.warm_pane.len(),
            app.warm_pane.ready_len(),
            app.warm_pane.inflight,
            target,
            app.warm_pane.is_surging()
        );
        let started = std::thread::Builder::new()
            .name("psmux-warm-spawn".into())
            .spawn(move || {
                let Some(_permit) = warm_spawn_permit() else {
                    // Teardown began before this spawn started: no process was
                    // ever created, so there is nothing for the reaper to kill.
                    crate::warm_pane_sync::inflight::release(params.pane_id);
                    let _ = tx.send(None);
                    return;
                };
                let pty = portable_pty::native_pty_system();
                match spawn_warm_pane_from(&*pty, &params) {
                    Ok(wp) => {
                        let _ = tx.send(Some(wp));
                    }
                    Err(e) => {
                        crate::warm_trace!("pool: refill FAILED pane={}: {e}", params.pane_id);
                        // Nothing survives a failed spawn, so the reaper has
                        // nothing to chase. `record_pid` already dropped the
                        // entry on the shutdown path; this covers a genuine
                        // spawn failure.
                        crate::warm_pane_sync::inflight::release(params.pane_id);
                        // `None` still releases the reservation. Without it the
                        // pool would believe a spare is forever on its way.
                        let _ = tx.send(None);
                    }
                }
            });
        if started.is_err() {
            app.warm_pane.inflight -= 1;
            crate::warm_pane_sync::inflight::release(spawn_pane_id);
            break;
        }
    }
}

/// Everything `spawn_warm_pane_from` needs, snapshotted off `AppState`.
///
/// The snapshot exists so the actual spawn can run on a background thread:
/// `AppState` is owned by the server loop and cannot cross a thread boundary,
/// but a plain value like this can. Taken on the loop thread (it allocates the
/// pane id and resolves format variables against live state), consumed
/// wherever is convenient.
#[derive(Clone)]
pub struct WarmSpawnParams {
    pub rows: u16,
    pub cols: u16,
    pub expanded_shell: String,
    pub env_shim: bool,
    pub allow_predictions: bool,
    pub pane_id: usize,
    pub control_port: Option<u16>,
    pub socket_name: Option<String>,
    pub session_name: String,
    pub claude_code_fix_tty: bool,
    pub claude_code_force_interactive: bool,
    pub host_colors: Option<crate::types::HostColors>,
    pub environment: std::collections::HashMap<String, String>,
    pub history_limit: usize,
    pub allow_alternate_screen: bool,
}

/// Snapshot the spawn inputs and reserve a pane id. Returns `None` when the
/// pool is disabled, which is the one thing the caller must not paper over.
pub fn warm_spawn_params(app: &mut AppState) -> Option<WarmSpawnParams> {
    if !app.warm_enabled {
        return None;
    }
    let area = app.client_area;
    let rows = if area.height > 1 { area.height } else { 30 }.max(MIN_PANE_DIM);
    let cols = if area.width > 1 { area.width } else { 120 }.max(MIN_PANE_DIM);
    // Expand format variables like #{pane_current_path} at spawn time (#111).
    // Must happen here, on the loop thread, while `app` is in hand.
    let expanded_shell = crate::format::expand_format(&app.default_shell, app);
    let pane_id = app.next_pane_id;
    app.next_pane_id += 1;
    // Track the spawn from the moment it is issued, so a teardown that happens
    // while it is still inside CreateProcessW can still reap the shell (#686).
    crate::warm_pane_sync::inflight::issue(pane_id);
    Some(WarmSpawnParams {
        rows,
        cols,
        expanded_shell,
        env_shim: app.env_shim,
        allow_predictions: app.allow_predictions,
        pane_id,
        control_port: app.control_port,
        socket_name: app.socket_name.clone(),
        session_name: app.session_name.clone(),
        claude_code_fix_tty: app.claude_code_fix_tty,
        claude_code_force_interactive: app.claude_code_force_interactive,
        host_colors: app.host_colors.clone(),
        environment: app.environment.clone(),
        history_limit: app.history_limit,
        allow_alternate_screen: app.allow_alternate_screen,
    })
}

/// Create one spare shell from a snapshot. Touches no shared state, so it is
/// safe to call from the background spawner thread.
pub fn spawn_warm_pane_from(pty_system: &dyn portable_pty::PtySystem, p: &WarmSpawnParams) -> io::Result<crate::types::WarmPane> {
    let t0 = std::time::Instant::now();
    let (rows, cols) = (p.rows, p.cols);
    let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system
        .openpty(size)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    let t_openpty = t0.elapsed();
    let mut shell_cmd = if !p.expanded_shell.is_empty() {
        build_default_shell(&p.expanded_shell, p.env_shim, p.allow_predictions)
    } else {
        build_command(None, p.env_shim, p.allow_predictions)
    };
    let pane_id = p.pane_id;
    set_tmux_env(&mut shell_cmd, pane_id, p.control_port, p.socket_name.as_deref(), &p.session_name, p.claude_code_fix_tty, p.claude_code_force_interactive);
    set_host_colors_env(&mut shell_cmd, p.host_colors.as_ref());
    apply_user_environment(&mut shell_cmd, &p.environment);
    let t_spawn0 = std::time::Instant::now();
    let child = pair.slave
        .spawn_command(shell_cmd)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    let t_spawn = t_spawn0.elapsed();
    let console_wait = portable_pty::last_spawn_console_wait_us();
    // The pid exists now, so the server can reap this shell even if it never
    // becomes a pool member (#686). A `false` answer means the server has begun
    // to die: nothing will ever adopt this child, so kill it here.
    let early_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    if !crate::warm_pane_sync::inflight::record_pid(p.pane_id, early_pid) {
        if let Some(pid) = early_pid {
            crate::platform::process_kill::kill_pid_tree(pid);
        }
        let mut child = child;
        let _ = child.kill();
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "server is shutting down; spare killed on arrival",
        ));
    }
    drop(pair.slave);
    let scrollback = p.history_limit as u32;
    let mut parser = vt100::Parser::new(rows, cols, scrollback as usize);
    parser.screen_mut().set_allow_alternate_screen(p.allow_alternate_screen);
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
    let child_pid = early_pid;
    spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), pane_id, child_pid);
    let mut pty_writer = spawn_pane_write_queue(pair.master.take_writer()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    conpty_preemptive_dsr_response(&mut *pty_writer);
    let now = std::time::Instant::now();
    // The phase breakdown is what showed the surge was serialised: `pty` stayed
    // flat while `proc` and its `wait` component climbed by one whole
    // CreateProcessW per concurrent spawn (#686).
    crate::warm_trace!(
        "pool: spawned spare pane={} pid={:?} in {:.1}ms (pty {:.1}ms, proc {:.1}ms, console wait {:.1}ms)",
        pane_id,
        child_pid,
        t0.elapsed().as_micros() as f64 / 1000.0,
        t_openpty.as_micros() as f64 / 1000.0,
        t_spawn.as_micros() as f64 / 1000.0,
        console_wait as f64 / 1000.0
    );
    Ok(crate::types::WarmPane { master: pair.master, writer: pty_writer, child, term, data_version, cursor_shape, bell_pending, cpr_pending, color_query_pending, child_pid, pane_id, rows, cols, output_ring, spawned_at: now, ready: false, last_dv: 0, last_change: now, trace_settled: false, host_colors: p.host_colors.clone() })
}

pub fn split_active(app: &mut AppState, kind: LayoutKind) -> io::Result<()> {
    split_active_with_command(app, kind, None, None, None)
}

/// Create a new window with a raw command (program + args, no shell wrapping)
pub fn create_window_raw(pty_system: &dyn portable_pty::PtySystem, app: &mut AppState, raw_args: &[String]) -> io::Result<()> {
    // #607: same copy-mode ownership handover as create_window_with_env.
    let prev_pane = crate::copy_mode::active_pane_id(app);
    crate::copy_mode::park_mode_on_active_pane(app);
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
    let pane = Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: raw_pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()), start_command: start_command_raw_argv(raw_args), cwd_hint: None };
    app.next_pane_id += 1;
    let win_name = std::path::Path::new(&raw_args[0]).file_stem().and_then(|s| s.to_str()).unwrap_or(&raw_args[0]).to_string();
    app.windows.push(Window { root: Node::Leaf(pane), active_path: vec![], name: win_name, id: app.next_win_id, area: app.client_area, window_size: None, window_options: Default::default(), activity_flag: false, bell_flag: false, silence_flag: false, last_output_time: std::time::Instant::now(), last_seen_version: 0, manual_rename: false, layout_index: 0, pane_mru: vec![raw_pane_id], zoom_saved: None, linked_from: None, floating: Vec::new(), floating_focus: None });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    app.on_window_appended();
    crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
    Ok(())
}

/// Minimum pane dimension (rows or cols) psmux will aim a NEW pane at, and the
/// floor used when no real rect is known yet.  A brand new pane wants room for
/// a prompt, so 2 is the target; it is a policy number, not a platform limit.
///
/// It must never be used to size a pane that already has a layout slot.  Doing
/// that is what broke #644: a slot one row tall got a two row screen, so the
/// pane the client had one row for reported `pane_height` 2, the renderer took
/// it for an oversized preview, and it painted the screen's blank second row
/// into the only row on offer.  `MIN_PTY_DIM` is the floor for that case.
pub const MIN_PANE_DIM: u16 = 2;

/// Hard floor for a pseudoconsole HEIGHT.
///
/// tmux's own limit is `PANE_MINIMUM 1` (tmux.h:110) and a one row pane is
/// legal there, painted like any other.  ConPTY agrees for rows:
/// `CreatePseudoConsole` with a height of 1 starts a child that renders
/// normally, and a pane squeezed to one row and grown back keeps echoing
/// (measured 2026-09-12 with wide glyphs in the buffer and erase sequences
/// written at one row).  So a pane is sized to the slot the layout gave it and
/// only zero rows is refused.
pub const MIN_PTY_DIM: u16 = 1;

/// Hard floor for a pseudoconsole WIDTH, and the narrowest cell the layout will
/// hand out on the horizontal axis.
///
/// This one is NOT tmux's 1, and the difference is measured, not assumed.  A
/// pseudoconsole resized to one column while its buffer holds a double width
/// glyph (CJK, or a VS16 emoji) never recovers: after growing back to 31
/// columns the child repaints its prompt and then stops echoing input for good,
/// with the shell process still alive.  pwsh and cmd.exe behave the same, so it
/// is conhost, not the shell.  Plain ASCII at one column survives the same
/// sequence, and one ROW with the same glyphs survives too, which is why only
/// the width has a floor of two.  `tests/test_issue534_shrink_wide_erase.ps1`
/// is the end to end proof: it squeezes a wide glyph pane to the minimum, erases
/// over it, grows it back and expects an echo.  The floor is applied in
/// `split_with_gaps` so the CELL is never one column either, keeping
/// `pane_width` equal to the slot width (#644's invariant).
pub const MIN_PTY_COLS: u16 = 2;

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
    // #607: the split's new pane becomes the active one.  Park the copy mode
    // on the pane it was entered in first, then let the new pane adopt its
    // own (none), so the split does not open in copy mode and the pane the
    // user was reading does not silently leave it.
    let prev_pane = crate::copy_mode::active_pane_id(app);
    crate::copy_mode::park_mode_on_active_pane(app);
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
    let warm_eligible = command.is_none() && extra_env.is_empty() && !default_shell_needs_fresh_eval(&app.default_shell);
    // A ready spare if there is one, else the oldest warming spare, else
    // nothing and a cold spawn below. See `WarmPool::claim`.
    let (mut claimed, mut was_ready) = if warm_eligible { app.warm_pane.claim() } else { (None, false) };
    if warm_eligible && claimed.is_none() {
        // Empty, but refills are already on their way. See `await_inflight_spare`.
        let (c, r) = await_inflight_spare(app, WARM_INFLIGHT_WAIT);
        claimed = c;
        was_ready = r;
    }
    if warm_eligible {
        app.warm_pane.note_claim(was_ready);
        if !was_ready {
            crate::warm_trace!(
                "claim(split): NO READY SPARE (depth={} got_warming={}) -- surging",
                app.warm_pane.len(),
                claimed.is_some()
            );
        }
        schedule_warm_refill(app);
    }
    if claimed.is_none() {
        // Cold path below: see the matching comment in `create_window_with_env`.
        // Applies whether the pool was empty or the transplant was bypassed
        // outright (a command, `-e`, a dynamic default-shell); either way this
        // pane takes an id above every reserved one.
        let dropped = app.warm_pane.set_issued_floor(app.next_pane_id);
        if dropped > 0 {
            crate::warm_trace!(
                "pool: id floor raised to {}, discarded {} spare(s) whose ids are now in the past",
                app.next_pane_id, dropped
            );
        }
    }
    if let Some(mut wp) = claimed {
        crate::warm_trace!(
            "claim(split): pane={} spare_age={:.1}ms pool_left={} ready_left={}",
            wp.pane_id,
            wp.spawned_at.elapsed().as_micros() as f64 / 1000.0,
            app.warm_pane.len(),
            app.warm_pane.ready_len()
        );
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
            let mut new_pane = Pane { master: wp.master, writer: wp.writer, child: wp.child, term: wp.term, last_rows: rows, last_cols: cols, id: new_pane_id, title: hostname_cached(), title_locked: false, child_pid: wp.child_pid, data_version: wp.data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None, cursor_shape: wp.cursor_shape, bell_pending: wp.bell_pending, cpr_pending: wp.cpr_pending, color_query_pending: wp.color_query_pending, copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring: wp.output_ring, spawned_at: Some(std::time::Instant::now()), start_command: String::new(), cwd_hint: None };
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
            crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
    let new_leaf = Node::Leaf(Pane { master: pair.master, writer: pty_writer, child, term, last_rows: size.rows, last_cols: size.cols, id: split_pane_id, title: hostname_cached(), title_locked: false, child_pid, data_version, last_title_check: epoch, last_infer_title: epoch, dead: false, last_text_input: None, last_special_key: None, vt_bridge_cache: None, vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None, cursor_shape, bell_pending, cpr_pending, color_query_pending, copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(), squelch_until: None, output_ring, spawned_at: Some(std::time::Instant::now()), start_command: start_command_raw(command), cwd_hint: None });
    app.next_pane_id += 1;
    let win = &mut app.windows[app.active_idx];
    replace_leaf_with_split(&mut win.root, &win.active_path, kind, new_leaf);
    let mut new_path = win.active_path.clone();
    new_path.push(1);
    win.active_path = new_path;
    // Add new pane to MRU (most recent)
    crate::tree::touch_mru(&mut win.pane_mru, split_pane_id);
    crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
    // #607: the survivor must land in ITS OWN mode, not in the dead pane's.
    let prev_pane = crate::copy_mode::active_pane_id(app);
    let win = &mut app.windows[app.active_idx];
    let active_path = win.active_path.clone();
    kill_pane_at_path(win, &active_path);
    crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
    Ok(())
}

pub fn kill_pane_by_id(app: &mut AppState, pane_id: usize) -> io::Result<()> {
    // #607: same as kill_active_pane.  `retarget` is a no-op when the active
    // pane did not move (killing a pane in another window), so the live copy
    // cursor of a pane that keeps focus is never clobbered by its parked copy.
    let prev_pane = crate::copy_mode::active_pane_id(app);
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

    crate::copy_mode::retarget_mode_to_active_pane(app, prev_pane);
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
/// Byte offset of the first `&&` that is NOT inside a quoted segment, so a
/// directory whose name happens to contain `&&` cannot cut the `cd` operand
/// short.
#[cfg(windows)]
fn find_unquoted_ampamp(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => {
                if c == b'\'' || c == b'"' {
                    quote = Some(c);
                } else if c == b'&' && i + 1 < b.len() && b[i + 1] == b'&' {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Quote one already-unquoted token so it survives a PowerShell `&` call as a
/// single literal argument.  Only tokens that would otherwise be split (or that
/// carry a quote of their own) are wrapped, so the common no-space case keeps
/// the exact text the caller sent.
#[cfg(windows)]
fn quote_for_pwsh(tok: &str) -> String {
    if tok.is_empty() {
        return "''".to_string();
    }
    if tok.chars().any(|c| c.is_whitespace()) || tok.contains('\'') || tok.contains('"') {
        format!("'{}'", tok.replace('\'', "''"))
    } else {
        tok.to_string()
    }
}

/// Detect the POSIX `env VAR=val ... <program> <args>` launch idiom, optionally
/// preceded by `cd <dir> &&`.  Tools written for real tmux (notably Claude Code
/// agent-teams, which launches each teammate with
/// `cd '<cwd>' && env CLAUDECODE=1 ... '<claude>' --agent-id ...`) use it.
///
/// The scan is quote aware end to end (#634).  It used to cut tokens on plain
/// whitespace, so a single assignment whose value carried a space — POSIX
/// quoted exactly as `env` expects, e.g.
/// `env XDIR='D:\POC Code\todosample' '<prog>'` — was chopped in half: `XDIR`
/// took only `'D:\POC`, and the orphaned tail `Code\todosample'` became the
/// program.  The pane then ran `& Code\todosample' ...` and PowerShell, which
/// reads `Name\Command` as a module qualified call, answered
/// "The module 'Code' could not be loaded", which is what every teammate pane
/// showed when the project lived under a path with a space in it.
#[cfg(windows)]
fn detect_env_prefix_command(cmd: &str) -> Option<(Option<String>, Vec<(String, String)>, String)> {
    let mut rest = cmd.trim();
    let mut cwd_override: Option<String> = None;

    // Optional leading `cd <dir> && `
    if let Some(after_cd) = rest.strip_prefix("cd ") {
        if let Some(amp) = find_unquoted_ampamp(after_cd) {
            cwd_override = Some(strip_quotes(after_cd[..amp].trim()).to_string());
            rest = after_cd[amp + 2..].trim();
        }
    }

    // Must be the `env ` idiom to apply this handling.
    let after_env = rest.strip_prefix("env ")?;

    // Consume leading `KEY=VALUE` tokens; the first non-assignment token begins
    // the program + args.  Tokenising honours quotes, so a quoted value keeps
    // its spaces instead of spilling into the program position.
    let tokens = split_spawn_tokens(after_env);
    let mut env_sets: Vec<(String, String)> = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        let tok = &tokens[idx];
        if let Some(eq) = tok.find('=') {
            let key = &tok[..eq];
            if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                env_sets.push((key.to_string(), tok[eq + 1..].to_string()));
                idx += 1;
                continue;
            }
        }
        break;
    }

    if idx >= tokens.len() {
        return None;
    }
    // Re-quote for the `&` call the caller builds: the tokeniser consumed the
    // original quotes, so a program (or argument) holding a space has to get
    // them back or PowerShell would split it again.
    let remainder = tokens[idx..]
        .iter()
        .map(|t| quote_for_pwsh(t))
        .collect::<Vec<String>>()
        .join(" ");
    Some((cwd_override, env_sets, remainder))
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
/// A BARE program name also qualifies, but only with arguments after it and
/// only when PATH resolves it to an image CreateProcessW can run by itself
/// (see `bare_program_image`).  That is tmux's own rule: a multi-argument
/// shell-command is `execvp`'d, a single one goes to the shell.  It matters
/// far more on Windows than on Unix, because the shell psmux would otherwise
/// interpose is pwsh, not sh: `new-session pwsh -NoLogo -NoProfile -File x`
/// used to spawn `pwsh -NoLogo -Command "pwsh ..."`, and that outer pwsh cost
/// a measured 238ms of pure launch latency while also sourcing the user's
/// profile, which the inner `-NoProfile` had asked to skip.  The historical
/// carve-out that kept bare names on the shell route, namely console utilities
/// like timeout.exe supposedly needing the shell to re-establish console stdin,
/// does not reproduce: `timeout /t 3`, `ping -n 2 127.0.0.1` and `nvim <file>`
/// all run correctly when spawned straight into the pane's ConPTY.
#[cfg(windows)]
fn try_direct_spawn(cmd: &str) -> Option<(String, Vec<String>)> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() { return None; }
    // pwsh call-operator form produced by the env-prefix path (#399) keeps
    // its established shell route.
    if trimmed.starts_with('&') { return None; }
    let has_separator = trimmed.contains('/') || trimmed.contains('\\');
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
    if has_separator {
        if let Some(prog) = exists_as_program(trimmed) {
            return Some((normalize(prog), Vec::new()));
        }
    }
    // Any shell metacharacter means the string needs a real shell.
    if trimmed.chars().any(|c| matches!(c, '&' | '|' | '<' | '>' | ';' | '`' | '$' | '(' | ')' | '%' | '\n' | '\r')) {
        return None;
    }
    let tokens = split_spawn_tokens(trimmed);
    if tokens.is_empty() { return None; }
    // Longest token-prefix that is an existing file: handles unquoted space
    // paths followed by arguments (`C:/Program Files/.../bash.exe --login`).
    if has_separator {
        for k in (1..=tokens.len()).rev() {
            let candidate = tokens[..k].join(" ");
            if let Some(prog) = exists_as_program(&candidate) {
                return Some((normalize(prog), tokens[k..].to_vec()));
            }
        }
    }
    // Bare program name WITH arguments (`pwsh -NoLogo -File x.ps1`, `nvim f`,
    // `git status`). The "with arguments" half is tmux's rule, not a hedge:
    // tmux execvp's a multi-argument shell-command and shells a single one
    // (spawn.c), so a lone `ls`/`cat`/`top` keeps the shell that has always
    // resolved it as a PowerShell alias.
    if tokens.len() > 1 {
        if let Some(prog) = bare_program_image(&tokens[0]) {
            return Some((prog, tokens[1..].to_vec()));
        }
    }
    None
}

/// PATH-resolve a bare program name to an image `CreateProcessW` can run on
/// its own, or `None` when the name belongs to a shell.
///
/// Only `.exe`/`.com` qualify. `npm` resolves to an extensionless Node script
/// and `<x>.cmd`/`.bat`/`.ps1` need their interpreter; handing any of those to
/// CreateProcessW fails the whole pane spawn with "%1 is not a valid Win32
/// application" (measured: `new-session -- npm --version` dies with os error
/// 193). A name that resolves to nothing at all is a shell builtin, function
/// or alias (`ls`, `Get-ChildItem`, `Start-Process`) and equally needs the
/// shell. Either way the caller keeps the shell route these commands have
/// always used.
#[cfg(windows)]
fn bare_program_image(token: &str) -> Option<String> {
    if token.contains('/') || token.contains('\\') { return None; }
    let resolved = cached_which(token);
    // cached_which echoes its input back when PATH has nothing for it.
    if resolved == token { return None; }
    let lower = resolved.to_ascii_lowercase();
    if !(lower.ends_with(".exe") || lower.ends_with(".com")) { return None; }
    // A Store-packaged (MSIX) image cannot be activated from an SSH-spawned
    // process at all (see `cached_shell`, which degrades to classic
    // powershell.exe for exactly this reason). Leave those on the shell route
    // so that degrade still happens instead of failing the spawn outright.
    if lower.contains("\\windowsapps\\")
        && (std::env::var_os("SSH_CONNECTION").is_some()
            || std::env::var_os("SSH_CLIENT").is_some())
    {
        return None;
    }
    Some(resolved)
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

/// Issue #597: the terminal name and version psmux reports for XTVERSION
/// (`CSI > q`, xterm's "what terminal are you?" round trip).
///
/// psmux presents a tmux identity to everything inside a pane: `$TMUX` and
/// `$TMUX_PANE` are set, `psmux -V` prints `tmux <version>` first, and the
/// command surface is tmux's.  Programs that key off XTVERSION are keying off
/// the same identity, so the reply spells `tmux`, exactly like tmux's own
/// `input_reply(ictx, 1, "\033P>|tmux %s\033\\", getversion())` (input.c
/// INPUT_CSI_XDA).  `PSMUX_XTVERSION_NAME` overrides the name for anyone who
/// would rather be announced as `psmux`.
pub fn xtversion_reply() -> String {
    let name = std::env::var("PSMUX_XTVERSION_NAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "tmux".to_string());
    format!("\x1bP>|{} {}\x1b\\", name.trim(), crate::types::VERSION)
}

/// Returns true if `data` contains an XTVERSION query that deserves a reply.
///
/// The query is `CSI > Ps q`, and xterm only treats `Ps` of 0 (or an omitted
/// `Ps`) as "report your version"; other values are cursor-style requests that
/// must stay unanswered.  tmux applies the same rule -- `input_get(ictx, 0, 0,
/// 0)` defaults an absent parameter to 0 and replies only for 0.
///
/// ConPTY answers DA1, DA2, DSR and DECRQM itself and never forwards them, but
/// it does not know XTVERSION, so the query arrives in the pane's output stream
/// verbatim and psmux is the only thing that can answer it.
///
/// Only matches whose final `q` lands at offset >= `min_end` count, so the
/// boundary rescan below can ignore a query it already reported last batch.
fn scan_xtversion_query_from(data: &[u8], min_end: usize) -> bool {
    if !data.contains(&0x1b) { return false; }
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] != 0x1b || data[i + 1] != b'[' || data[i + 2] != b'>' {
            i += 1;
            continue;
        }
        let params_start = i + 3;
        let mut j = params_start;
        while j < data.len() && (data[j].is_ascii_digit() || data[j] == b';') {
            j += 1;
        }
        if j < data.len() && data[j] == b'q' && j >= min_end {
            let params = &data[params_start..j];
            let first = match params.iter().position(|&c| c == b';') {
                Some(p) => &params[..p],
                None => params,
            };
            if first.is_empty() || first.iter().all(|&c| c == b'0') {
                return true;
            }
        }
        i = params_start;
    }
    false
}

/// `scan_xtversion_query_from` over a whole batch.
fn scan_xtversion_query(data: &[u8]) -> bool {
    scan_xtversion_query_from(data, 0)
}

/// Detects an XTVERSION query split across two reads, the same way
/// `CprScanner` does for ESC[6n.  The reader thread hands the scanner whatever
/// one `read()` returned, so a query that straddles the boundary would
/// otherwise go unanswered and the asking program would sit through its whole
/// timeout.
struct XtversionScanner {
    tail: Vec<u8>,
}

impl XtversionScanner {
    /// One less than the longest query worth carrying: `ESC [ > 0 q` is five
    /// bytes, and a few more cover a parameter list such as `ESC [ > 0 ; 0 q`.
    const KEEP: usize = 15;

    fn new() -> Self {
        Self { tail: Vec::with_capacity(Self::KEEP) }
    }

    fn scan(&mut self, batch: &[u8]) -> bool {
        let mut hit = scan_xtversion_query(batch);
        if !hit && !self.tail.is_empty() {
            let mut boundary = self.tail.clone();
            let tail_len = boundary.len();
            boundary.extend_from_slice(&batch[..batch.len().min(Self::KEEP)]);
            // Only a query whose final `q` lands in the new batch is new; one
            // sitting entirely in the carried tail was answered last batch, and
            // answering it twice would hand the asking program a second reply
            // it never asked for.
            hit = scan_xtversion_query_from(&boundary, tail_len);
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

// ─── Issue #597: DA1, DA2, DSR and DECRQM, answered by psmux like tmux ───
//
// The inbox Windows console host answers these four itself and never forwards
// them, which is why psmux never needed to: a PSMUX_PANE_RAW capture of a pane
// on build 26200 contains no `ESC[c` at all, only the XTVERSION query conhost
// does not know.
//
// A console host that forwards them instead breaks two ways at once, both
// measured on 26200 with the `Microsoft.Windows.Console.ConPTY` 1.24 package
// loaded through `PSMUX_CONPTY_DIR`:
//
//   * OpenConsole 1.24 opens every pane by writing `ESC[1t ESC[c` toward psmux
//     and then parks the child inside its console connect until the DA1 answer
//     arrives.  With nobody answering, that is a fixed timeout of about three
//     seconds on every single pane launch (prompt visible at 3777 / 3747 /
//     3799 ms against 629 / 676 / 627 ms on the inbox host).
//   * A program inside the pane that asks DA1, DA2 or DECRQM gets zero bytes
//     back and falls through to whatever it does for a featureless terminal.
//
// tmux answers all of them from its own parser, so psmux answering them is
// parity rather than invention.  Citations, tmux next-3.8 `input.c` in
// C:\Users\godwin\Documents\workspace\tmux:
//
//   input.c:1581-1595  INPUT_CSI_DA        -> input_reply(ictx, 1, "\033[?1;2c")
//   input.c:1597-1608  INPUT_CSI_DA_TWO    -> input_reply(ictx, 1, "\033[>84;0;0c")
//   input.c:1722-1737  INPUT_CSI_DSR       -> "\033[0n" for 5,
//                                             "\033[%u;%uR" for 6
//   input.c:1650-1721  INPUT_CSI_QUERY_PRIVATE
//                                          -> "\033[?%d;%d$y", the mode table
//   input.c:1637-1649  INPUT_CSI_QUERY     -> "\033[%d;%d$y" (ANSI modes)
//   input.c:2123-2217  input_csi_dispatch_winops: `ESC[1t` falls in the
//                      `case 1:` arm that only `break`s, so tmux does NOT
//                      answer XTWINOPS 1 and neither does psmux.
//
// Two of those tmux cases are already psmux's and stay untouched here:
// `CSI 6 n` is answered by the CPR responder (`CprScanner` plus
// `helpers::drain_cpr_pending`, which reports the pane's real cursor), and
// `CSI ? 996 n` is answered by the colour path.  Answering either a second
// time here would hand the asking program a duplicate reply, so the scanner
// deliberately skips both.

/// A device query psmux answers itself when the console host forwards it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceQuery {
    /// `CSI c` or `CSI 0 c` — primary device attributes.
    Da1,
    /// `CSI > c` or `CSI > 0 c` — secondary device attributes.
    Da2,
    /// `CSI 5 n` — device status report ("are you ok").
    DsrStatus,
    /// `CSI ? Ps $ p` — DECRQM for a private (DEC) mode.
    DecrqmPrivate(u16),
    /// `CSI Ps $ p` — DECRQM for an ANSI mode.
    DecrqmAnsi(u16),
}

/// The pane screen state a DECRQM answer depends on.
///
/// Sampled under the parser lock immediately after the batch carrying the
/// query has been processed, so `ESC[?1049h ESC[?1049$p` in one write reports
/// the alternate screen as set rather than as reset.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DecrqmState {
    pub alternate_screen: bool,
    pub application_cursor: bool,
    pub cursor_visible: bool,
    pub bracketed_paste: bool,
    pub mouse_standard: bool,
    pub mouse_button: bool,
    pub mouse_all: bool,
    pub mouse_utf8: bool,
    pub mouse_sgr: bool,
}

impl DecrqmState {
    fn from_screen(s: &vt100::Screen) -> Self {
        Self {
            alternate_screen: s.alternate_screen(),
            application_cursor: s.application_cursor(),
            cursor_visible: !s.hide_cursor(),
            bracketed_paste: s.bracketed_paste(),
            mouse_standard: s.mouse_standard_flag(),
            mouse_button: s.mouse_button_flag(),
            mouse_all: s.mouse_all_flag(),
            mouse_utf8: s.mouse_utf8_flag(),
            mouse_sgr: s.mouse_sgr_flag(),
        }
    }
}

/// tmux's DECRQM value semantics: 1 set, 2 reset, 0 the terminal does not
/// recognise the mode, 4 permanently reset (input.c:1650-1721).
fn decrqm_private_value(mode: u16, st: &DecrqmState) -> u8 {
    let onoff = |b: bool| if b { 1u8 } else { 2u8 };
    match mode {
        1 => onoff(st.application_cursor),          // DECCKM
        3 => 4,                                     // DECCOLM, permanently reset
        25 => onoff(st.cursor_visible),             // DECTCEM
        47 | 1047 | 1049 => onoff(st.alternate_screen),
        1000 => onoff(st.mouse_standard),
        1002 => onoff(st.mouse_button),
        1003 => onoff(st.mouse_all),
        1005 => onoff(st.mouse_utf8),
        1006 => onoff(st.mouse_sgr),
        2004 => onoff(st.bracketed_paste),
        // 2026 (synchronized output) is deliberately 0, not 2.  tmux answers 2
        // because tmux implements MODE_SYNC; psmux's vt100 has no synchronized
        // output, so claiming the mode exists would invite programs to bracket
        // every frame with a sequence psmux drops on the floor.  0 is also
        // byte for byte what the inbox console host answers on 26200
        // (measured: `ESC[?2026;0$y`), so a pane sees the same reply whichever
        // console host is under it.  The same reasoning covers 2031.
        _ => 0,
    }
}

/// DECRQM for an ANSI mode.  psmux tracks none of them (tmux answers only IRM,
/// input.c:1637-1649), so every mode reports 0, "not recognised".
fn decrqm_ansi_value(_mode: u16) -> u8 { 0 }

/// The exact bytes tmux would send for `q`.  Empty when the query draws no
/// reply, which is tmux's `if (m > 0)` guard on both DECRQM arms.
pub(crate) fn device_reply(q: DeviceQuery, st: &DecrqmState) -> String {
    match q {
        DeviceQuery::Da1 => "\x1b[?1;2c".to_string(),
        DeviceQuery::Da2 => "\x1b[>84;0;0c".to_string(),
        DeviceQuery::DsrStatus => "\x1b[0n".to_string(),
        DeviceQuery::DecrqmPrivate(m) if m > 0 => {
            format!("\x1b[?{};{}$y", m, decrqm_private_value(m, st))
        }
        DeviceQuery::DecrqmAnsi(m) if m > 0 => {
            format!("\x1b[{};{}$y", m, decrqm_ansi_value(m))
        }
        _ => String::new(),
    }
}

/// The first parameter of a CSI parameter list, `None` when it is absent.
/// xterm and tmux both default an absent parameter to 0.
fn csi_first_param(params: &[u8]) -> Option<u16> {
    let first = match params.iter().position(|&c| c == b';') {
        Some(p) => &params[..p],
        None => params,
    };
    if first.is_empty() { return None; }
    let mut v: u32 = 0;
    for &c in first {
        if !c.is_ascii_digit() { return None; }
        v = v * 10 + u32::from(c - b'0');
        if v > u32::from(u16::MAX) { return None; }
    }
    u16::try_from(v).ok()
}

/// Collect every device query in `data` that starts before `max_start` and
/// ends past `min_end`, in the order they appear.
///
/// The two cuts are what let the boundary rescan below report exactly the
/// queries that straddle a batch boundary: one that ended in the previous
/// batch was answered then, and one that fits entirely inside the new batch
/// was already collected by the batch pass, so the boundary pass asks for
/// "started in the tail, finished in the batch" and nothing else.
fn scan_device_queries_window(
    data: &[u8],
    min_end: usize,
    max_start: usize,
    out: &mut Vec<DeviceQuery>,
) {
    if !data.contains(&0x1b) { return; }
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] != 0x1b || data[i + 1] != b'[' {
            i += 1;
            continue;
        }
        let mut j = i + 2;
        let private = if matches!(data[j], b'?' | b'>' | b'<' | b'=') {
            let m = data[j];
            j += 1;
            Some(m)
        } else {
            None
        };
        let params_start = j;
        while j < data.len() && (data[j].is_ascii_digit() || data[j] == b';') {
            j += 1;
        }
        let params = &data[params_start..j];
        // One intermediate byte is all these queries use: `$` in DECRQM.
        let intermediate = if j < data.len() && (0x20..=0x2f).contains(&data[j]) {
            let b = data[j];
            j += 1;
            Some(b)
        } else {
            None
        };
        if j >= data.len() {
            // Truncated at the edge of the batch: everything from here on is
            // parameter bytes, so there is no later ESC to find.
            return;
        }
        let end = j + 1;
        if end > min_end && i < max_start {
            let first = csi_first_param(params);
            match (data[j], private, intermediate) {
                // DA1 / DA2.  A non-zero parameter is not a request to report,
                // so it stays unanswered, exactly as tmux's
                // `input_get(ictx, 0, 0, 0)` switch does.
                (b'c', None, None) if matches!(first, None | Some(0)) => out.push(DeviceQuery::Da1),
                (b'c', Some(b'>'), None) if matches!(first, None | Some(0)) => {
                    out.push(DeviceQuery::Da2)
                }
                // DSR.  5 is ours; 6 belongs to the CPR responder, which knows
                // the cursor, and answering it here as well would double up.
                (b'n', None, None) if first == Some(5) => out.push(DeviceQuery::DsrStatus),
                // DECRQM.  `CSI ? 996 n` is the colour path's, not this one,
                // and it is an `n` with a `?`, so it never reaches here anyway.
                (b'p', Some(b'?'), Some(b'$')) => {
                    if let Some(m) = first { out.push(DeviceQuery::DecrqmPrivate(m)); }
                }
                (b'p', None, Some(b'$')) => {
                    if let Some(m) = first { out.push(DeviceQuery::DecrqmAnsi(m)); }
                }
                _ => {}
            }
        }
        i = params_start;
    }
}

/// `scan_device_queries_window` over a whole batch.
#[cfg(test)]
pub(crate) fn scan_device_queries(data: &[u8]) -> Vec<DeviceQuery> {
    let mut out = Vec::new();
    scan_device_queries_window(data, 0, usize::MAX, &mut out);
    out
}

/// Detects device queries split across two reads, the same way `CprScanner`
/// does for ESC[6n.  Without it a `ESC[` that ends one batch and a `c` that
/// starts the next would leave the asking program, or the console host itself,
/// waiting out its whole timeout.
pub(crate) struct DeviceQueryScanner {
    tail: Vec<u8>,
}

impl DeviceQueryScanner {
    /// One less than the longest query worth carrying.  `ESC [ ? 1 0 0 6 $ p`
    /// is nine bytes and a longer mode number only costs a couple more.
    const KEEP: usize = 15;

    pub(crate) fn new() -> Self {
        Self { tail: Vec::with_capacity(Self::KEEP) }
    }

    pub(crate) fn scan(&mut self, batch: &[u8]) -> Vec<DeviceQuery> {
        let mut out = Vec::new();
        if !self.tail.is_empty() {
            let mut boundary = self.tail.clone();
            let tail_len = boundary.len();
            boundary.extend_from_slice(&batch[..batch.len().min(Self::KEEP)]);
            // Started in the carried tail, finished in this batch.  Anything
            // that ended in the tail was answered last batch, and anything
            // that starts in the batch is the batch pass's to find, so the two
            // passes partition the queries instead of overlapping.
            scan_device_queries_window(&boundary, tail_len, tail_len, &mut out);
        }
        scan_device_queries_window(batch, 0, usize::MAX, &mut out);
        if batch.len() >= Self::KEEP {
            self.tail.clear();
            self.tail.extend_from_slice(&batch[batch.len() - Self::KEEP..]);
        } else {
            self.tail.extend_from_slice(batch);
            let excess = self.tail.len().saturating_sub(Self::KEEP);
            self.tail.drain(..excess);
        }
        out
    }
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
    /// Batches this small are echoes, not frames: parse them straight away
    /// instead of paying a coalescing tick. Measured with
    /// tests/probe_run.ps1: a pwsh echo arrives as a 62 byte chunk.
    const ECHO_CHUNK_MAX: usize = 1024;

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
        let mut xtversion_scanner = XtversionScanner::new();
        loop {
            match reader.read(&mut local) {
                Ok(n) if n > 0 => {
                    zero_reads = 0;
                    crate::pty_trace::mark("r", pane_id, &local[..n]);
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
                            color_query_bits, child_pid, &colors, pane_id,
                        ) {
                            // Injection unavailable (no child pid, or a
                            // non-Windows build): keep the #473 server-loop
                            // pipe path as delivery of record.
                            color_query_pending.fetch_or(color_query_bits, Ordering::AcqRel);
                            crate::types::COLOR_QUERY_PENDING.store(true, Ordering::Release);
                            // The server loop drains these bits only on a pass
                            // that saw pane data, and the parser thread has
                            // usually announced THIS read already: it wakes the
                            // loop within its coalescing wait, while a refused
                            // attach above can take longer than that.  The loop
                            // then finds nothing pending, and a program blocked
                            // on the reply prints nothing more to trigger
                            // another pass, so the pipe reply never went out
                            // (issue #597).  Ask for the pass ourselves.
                            crate::types::PTY_DATA_READY.store(true, Ordering::Release);
                            crate::types::wake_server_loop();
                        }
                    }
                    // Issue #597: answer XTVERSION (`CSI > q`) here too, for the
                    // same reason the colour queries are answered off the read:
                    // the asking program (Claude Code sends it three times at
                    // startup, WezTerm and others send it once) gives the
                    // terminal a short window and then decides it is talking to
                    // something featureless.  The reply goes in as console
                    // input rather than down the PTY writer because the ConPTY
                    // is created with PSEUDOCONSOLE_WIN32_INPUT_MODE, where a
                    // raw VT response wedges the Win32 input parser (issue
                    // #313, see conpty_preemptive_dsr_response).
                    //
                    // No pipe fallback here, decided by measurement rather
                    // than by symmetry with the colour path (issue #597): a
                    // DCS written to a live pane's input pipe on build 26200
                    // arrives as a bare ESC with its body eaten, so a fallback
                    // would deliver no reply AND hand the pane a spurious
                    // Escape keypress.  A failed injection is logged instead.
                    if xtversion_scanner.scan(&local[..n]) {
                        let reply = xtversion_reply();
                        let delivered = match child_pid {
                            Some(pid) => crate::platform::mouse_inject::send_vt_reply(
                                pid, &reply,
                            ),
                            None => false,
                        };
                        if !delivered {
                            crate::platform::mouse_inject::log_lost_reply(
                                child_pid, "XTVERSION", reply.len(),
                            );
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
        // Issue #597: DA1/DA2/DSR/DECRQM are answered from THIS thread, not
        // the reader thread the XTVERSION and colour answers live on, because
        // a DECRQM answer has to report the pane's mode state as of the bytes
        // that carried the query.  Here the state is one lock away and already
        // up to date; in the reader thread it would be a batch stale.  The
        // latency that pushed XTVERSION into the reader thread does not bite
        // either: a console host's opening `ESC[1t ESC[c` is 31 bytes, far
        // under ECHO_CHUNK_MAX, so it skips the coalescing wait entirely.
        let mut device_scanner = DeviceQueryScanner::new();
        // Issue #685: the palette generation last mirrored into
        // `types::PANE_PALETTES`.  A fresh parser starts at 0 with an empty
        // palette, so a pane id reused by respawn-pane withdraws whatever the
        // previous child published the moment its successor writes anything.
        let mut published_palette_gen: u64 = u64::MAX;
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
            // A keystroke echo is tens of bytes and arrives as one chunk, so
            // there is no multi-chunk frame to bridge and nothing to tear.
            // Waiting a tick anyway cost a tick of keystroke-to-screen latency
            // on every character typed, which is the one place on the path
            // where it is most visible. Anything larger is a redraw that can
            // legitimately span chunks, and still gets the full adaptive wait.
            if last_len > ECHO_CHUNK_MAX {
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
            let device_queries = device_scanner.scan(&bytes);

            // Issue #502 diagnostic: capture the exact pre-parse byte stream
            // when PSMUX_PANE_RAW=1. Off by default, one atomic load when off.
            crate::debug_log::pane_raw(&bytes);

            // Issue #597: sampled inside the parser lock below, so a DECRQM
            // that shares a write with the DECSET it asks about reports the
            // post-write value, the way tmux's in-order dispatch does.
            let mut decrqm_state: Option<DecrqmState> = None;
            if let Ok(mut parser) = term_reader.lock() {
                parser.process(&bytes);
                if !device_queries.is_empty() {
                    decrqm_state = Some(DecrqmState::from_screen(parser.screen()));
                }
                if parser.screen_mut().take_audible_bell() {
                    bell_pending.store(true, Ordering::Release);
                }
                // Issue #685: mirror the low sixteen OSC 4 entries out for the
                // colour query responder, which runs in the reader thread and
                // cannot take this lock.  The generation check makes this free
                // for every pane that never sets a palette, and near free for
                // one that sets it once at startup the way conhost does.
                let gen = parser.screen().palette_generation();
                if gen != published_palette_gen {
                    published_palette_gen = gen;
                    let screen = parser.screen();
                    let mut entries: [Option<(u8, u8, u8)>; 16] = [None; 16];
                    for (i, slot) in entries.iter_mut().enumerate() {
                        *slot = screen.palette_entry(
                            u8::try_from(i).unwrap_or(0),
                        );
                    }
                    crate::types::publish_pane_palette(pane_id, entries);
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
            // Issue #597: hand the composed DA1/DA2/DSR/DECRQM answers to the
            // server loop, which owns this pane's PTY writer.  The wake at the
            // bottom of this pass is what gets them written; on a pane launch
            // that is the difference between a prompt at 0.7 s and one at
            // 3.8 s, because OpenConsole parks the child's console connect
            // until its own `ESC[c` is answered.
            if !device_queries.is_empty() {
                let st = decrqm_state.unwrap_or_default();
                let mut reply: Vec<u8> = Vec::new();
                for q in &device_queries {
                    reply.extend_from_slice(device_reply(*q, &st).as_bytes());
                }
                crate::types::push_device_reply(pane_id, reply);
            }
            // Issue #473 color queries are scanned and answered in the READER
            // thread above (issue #556): the coalescing wait this thread runs
            // before parsing is exactly the latency that made replies miss
            // startup probe windows.
            dv_writer.fetch_add(1, Ordering::Release);
            crate::types::PTY_DATA_READY.store(true, Ordering::Release);
            crate::pty_trace::mark("p", pane_id, &bytes);
            // Wake the server loop on the event instead of leaving it to notice
            // the flag when its poll interval happens to expire.
            crate::types::wake_server_loop();
        }
    });
}

#[cfg(test)]
#[path = "../tests-rs/test_issue597_xtversion_reply.rs"]
mod test_issue597_xtversion_reply;

#[cfg(test)]
#[path = "../tests-rs/test_issue597_da_replies.rs"]
mod tests_issue597_da_replies;

#[cfg(test)]
#[path = "../tests-rs/test_issue399_env_prefix.rs"]
mod test_issue399_env_prefix;

#[cfg(test)]
#[path = "../tests-rs/test_issue634_env_value_spaces.rs"]
mod test_issue634_env_value_spaces;

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

#[cfg(test)]
#[path = "../tests-rs/test_issue672_shell_basename_classify.rs"]
mod tests_issue672_shell_basename_classify;

#[cfg(test)]
#[path = "../tests-rs/test_issue607_copy_mode_inherit.rs"]
mod tests_issue607_copy_mode_inherit;

#[cfg(test)]
#[path = "../tests-rs/test_issue630_conpty_cwd_pin.rs"]
mod tests_issue630_conpty_cwd_pin;

#[cfg(test)]
#[path = "../tests-rs/test_issue615_warm_claim_cwd_hint.rs"]
mod tests_issue615_warm_claim_cwd_hint;

#[cfg(test)]
#[path = "../tests-rs/test_issue683_shell_env.rs"]
mod tests_issue683_shell_env;
