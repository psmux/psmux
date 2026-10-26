// Tests for run-shell shell binary resolution and build_run_shell_command
//
// Covers:
// 1. resolve_shell_binary fallback between pwsh/powershell
// 2. build_run_shell_command Case 1 (shell binary prefix)
// 3. build_run_shell_command Case 2 (bare .ps1 file)
// 4. build_run_shell_command Case 3 (generic command)
// 5. expand_run_shell_path tilde expansion
// 6. run-shell command parsing from execute_command_string

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

fn mock_app_with_window() -> AppState {
    let mut app = mock_app();
    app.windows.push(make_window("shell", 0));
    app
}

// ─── resolve_shell_binary tests ─────────────────────────────────────────────

#[test]
#[cfg(windows)]
fn resolve_shell_binary_pwsh_returns_valid_shell() {
    // On this test machine, at least one of pwsh/powershell must exist
    let result = resolve_shell_binary("pwsh");
    // Should either return "pwsh" (if found) or a full path to powershell (fallback)
    let lower = result.to_lowercase();
    assert!(
        lower.contains("pwsh") || lower.contains("powershell"),
        "resolve_shell_binary('pwsh') should return a PS shell, got: {}",
        result
    );
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_powershell_returns_valid_shell() {
    let result = resolve_shell_binary("powershell");
    let lower = result.to_lowercase();
    assert!(
        lower.contains("pwsh") || lower.contains("powershell"),
        "resolve_shell_binary('powershell') should return a PS shell, got: {}",
        result
    );
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_powershell_exe_returns_valid_shell() {
    let result = resolve_shell_binary("powershell.exe");
    let lower = result.to_lowercase();
    assert!(
        lower.contains("pwsh") || lower.contains("powershell"),
        "resolve_shell_binary('powershell.exe') should return a PS shell, got: {}",
        result
    );
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_cmd_passthrough() {
    let result = resolve_shell_binary("cmd");
    assert_eq!(result, "cmd", "cmd should pass through unchanged");
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_cmd_exe_passthrough() {
    let result = resolve_shell_binary("cmd.exe");
    assert_eq!(result, "cmd.exe", "cmd.exe should pass through unchanged");
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_arbitrary_passthrough() {
    let result = resolve_shell_binary("notepad");
    assert_eq!(result, "notepad", "unknown binaries should pass through unchanged");
}

#[test]
#[cfg(windows)]
fn resolve_shell_binary_full_path_passthrough() {
    let result = resolve_shell_binary(r"C:\Windows\System32\cmd.exe");
    assert_eq!(result, r"C:\Windows\System32\cmd.exe", "full paths should pass through unchanged");
}

// ─── build_run_shell_command tests ──────────────────────────────────────────

#[test]
#[cfg(windows)]
fn build_run_shell_command_pwsh_prefix_creates_valid_command() {
    // Simulates what PPM plugin.conf does:
    // bind-key I run-shell 'pwsh -NoProfile -ExecutionPolicy Bypass -File "~/.psmux/plugins/ppm/scripts/install_plugins.ps1"'
    let cmd = build_run_shell_command("pwsh -NoProfile -Command echo hello");
    let prog = cmd.get_program().to_string_lossy().to_lowercase();
    assert!(
        prog.contains("pwsh") || prog.contains("powershell"),
        "Should resolve to a valid PS shell, got: {}",
        prog
    );
}

#[test]
#[cfg(windows)]
fn build_run_shell_command_powershell_prefix_creates_valid_command() {
    let cmd = build_run_shell_command("powershell.exe -ExecutionPolicy Bypass -File test.ps1");
    let prog = cmd.get_program().to_string_lossy().to_lowercase();
    assert!(
        prog.contains("pwsh") || prog.contains("powershell"),
        "Should resolve to a valid PS shell, got: {}",
        prog
    );
}

#[test]
#[cfg(windows)]
fn build_run_shell_command_cmd_prefix_passes_through() {
    let cmd = build_run_shell_command("cmd /c echo hello");
    let prog = cmd.get_program().to_string_lossy().to_lowercase();
    assert!(
        prog.contains("cmd"),
        "cmd should pass through to cmd, got: {}",
        prog
    );
}

#[test]
#[cfg(windows)]
fn build_run_shell_command_generic_command_uses_shell_wrapper() {
    let cmd = build_run_shell_command("echo hello");
    let prog = cmd.get_program().to_string_lossy().to_lowercase();
    // Generic commands get wrapped in a shell (Case 3)
    assert!(
        prog.contains("pwsh") || prog.contains("powershell") || prog.contains("cmd"),
        "Generic command should be wrapped in a shell, got: {}",
        prog
    );
}

#[test]
#[cfg(windows)]
fn build_run_shell_command_preserves_args() {
    let cmd = build_run_shell_command("pwsh -NoProfile -Command echo hello world");
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert!(args.contains(&"-NoProfile".to_string()), "Should preserve -NoProfile arg");
    assert!(args.contains(&"-Command".to_string()), "Should preserve -Command arg");
}

#[test]
#[cfg(windows)]
fn build_run_shell_command_runs_an_existing_exe_directly() {
    // No shell around a native executable, with or without spaces in its
    // path: hooks run such helpers on every pane switch.
    let tmp = std::env::temp_dir().join(format!("psmux_test_direct_exe_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    for dir in [tmp.join("plain"), tmp.join("with space")] {
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("helper.exe");
        std::fs::write(&exe, b"").unwrap();
        let cmd = build_run_shell_command(&format!("{} --quiet", exe.display()));
        assert_eq!(std::path::Path::new(cmd.get_program()), exe);
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args, vec!["--quiet".to_string()]);
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

// ─── expand_run_shell_path tests ────────────────────────────────────────────

#[test]
fn expand_tilde_forward_slash() {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let result = crate::util::expand_run_shell_path("~/.psmux/plugins/ppm/ppm.ps1");
    assert!(
        result.contains(&home),
        "~ should be expanded to home dir. Got: {}",
        result
    );
    assert!(
        !result.starts_with('~'),
        "Result should not start with ~ after expansion. Got: {}",
        result
    );
}

#[test]
fn expand_tilde_backslash() {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let result = crate::util::expand_run_shell_path(r"~\.psmux\plugins\ppm\ppm.ps1");
    assert!(
        result.contains(&home),
        "~ should be expanded to home dir. Got: {}",
        result
    );
}

#[test]
fn expand_no_tilde_unchanged() {
    let result = crate::util::expand_run_shell_path("/absolute/path/script.ps1");
    assert_eq!(result, "/absolute/path/script.ps1");
}

#[test]
fn expand_dollar_home_not_expanded() {
    // $HOME is a shell variable, not handled by expand_run_shell_path
    let result = crate::util::expand_run_shell_path("$HOME/.psmux/plugins/ppm/ppm.ps1");
    assert!(
        result.starts_with("$HOME"),
        "$HOME should NOT be expanded (that is shell's job). Got: {}",
        result
    );
}

// ─── run-shell command parsing via execute_command_string ───────────────────

#[test]
fn run_shell_no_args_shows_usage() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "run-shell").unwrap();
    assert!(
        app.status_message.is_some(),
        "run-shell with no args should set a status message"
    );
    let (msg, ..) = app.status_message.as_ref().unwrap();
    assert!(
        msg.contains("usage"),
        "Should show usage message, got: {}",
        msg
    );
}

#[test]
fn run_alias_no_args_shows_usage() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "run").unwrap();
    assert!(
        app.status_message.is_some(),
        "run with no args should set a status message"
    );
    let (msg, ..) = app.status_message.as_ref().unwrap();
    assert!(
        msg.contains("usage"),
        "Should show usage message, got: {}",
        msg
    );
}

#[test]
fn run_shell_background_flag_doesnt_block() {
    let mut app = mock_app_with_window();
    // -b flag should fire and forget
    execute_command_string(&mut app, "run-shell -b echo test").unwrap();
    // Should NOT set a "running:" status message (that is for foreground only)
    if let Some((msg, ..)) = &app.status_message {
        assert!(
            !msg.contains("running:"),
            "Background run should not set running status, got: {}",
            msg
        );
    }
}

#[test]
fn run_shell_foreground_sets_running_status() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "run-shell echo test_run_shell").unwrap();
    assert!(
        app.status_message.is_some(),
        "Foreground run-shell should set status message"
    );
    let (msg, ..) = app.status_message.as_ref().unwrap();
    assert!(
        msg.contains("running:"),
        "Should show 'running: ...' status, got: {}",
        msg
    );
}

#[test]
fn run_shell_quoted_command() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "run-shell 'echo hello world'").unwrap();
    assert!(
        app.status_message.is_some(),
        "Quoted run-shell should set status message"
    );
}

#[test]
fn run_shell_double_quoted_command() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, r#"run-shell "echo hello world""#).unwrap();
    assert!(
        app.status_message.is_some(),
        "Double quoted run-shell should set status message"
    );
}

// ─── ensure_background tests ────────────────────────────────────────────────

#[test]
fn ensure_background_adds_flag_to_run_shell() {
    let result = ensure_background("run-shell echo test");
    assert!(result.contains("-b"), "Should add -b flag. Got: {}", result);
    assert!(result.starts_with("run-shell -b"), "Flag should be right after command. Got: {}", result);
}

#[test]
fn ensure_background_adds_flag_to_run_alias() {
    let result = ensure_background("run echo test");
    assert!(result.contains("-b"), "Should add -b flag. Got: {}", result);
    assert!(result.starts_with("run -b"), "Flag should be right after alias. Got: {}", result);
}

#[test]
fn ensure_background_noop_when_already_background() {
    let result = ensure_background("run-shell -b echo test");
    assert_eq!(result, "run-shell -b echo test", "Should not double add -b flag");
}

#[test]
fn ensure_background_noop_for_non_run_commands() {
    let result = ensure_background("display-message hello");
    assert_eq!(result, "display-message hello", "Non run commands should be unchanged");
}

// ─── parse_command_line tests for edge cases ────────────────────────────────

#[test]
fn parse_command_line_simple() {
    let parts = parse_command_line("echo hello world");
    assert_eq!(parts, vec!["echo", "hello", "world"]);
}

#[test]
fn parse_command_line_double_quoted() {
    let parts = parse_command_line(r#"echo "hello world""#);
    assert_eq!(parts, vec!["echo", "hello world"]);
}

#[test]
fn parse_command_line_single_quoted() {
    let parts = parse_command_line("echo 'hello world'");
    assert_eq!(parts, vec!["echo", "hello world"]);
}

#[test]
fn parse_command_line_mixed_quotes() {
    let parts = parse_command_line(r#"run 'pwsh -Command "echo test"'"#);
    assert_eq!(parts, vec!["run", r#"pwsh -Command "echo test""#]);
}

#[test]
fn parse_command_line_windows_path() {
    let parts = parse_command_line(r#"pwsh -File "C:\Users\test\script.ps1""#);
    assert_eq!(parts, vec!["pwsh", "-File", r"C:\Users\test\script.ps1"]);
}

#[test]
fn parse_command_line_backslash_in_double_quotes() {
    // Backslash should be literal inside double quotes (Windows paths)
    let parts = parse_command_line(r#""C:\Program Files\test.exe" -arg"#);
    assert_eq!(parts, vec![r"C:\Program Files\test.exe", "-arg"]);
}

#[test]
fn parse_command_line_empty_string() {
    let parts = parse_command_line("");
    assert!(parts.is_empty());
}

// ─── run-shell child PATH ───────────────────────────────────────────────────
//
// A run-shell child inherits the SERVER's environment, which may not contain
// the directory psmux itself runs from: a parked warm server keeps the
// environment of whatever spawned it, even after another session claims it.
// Plugin scripts and user bindings that shell out to `psmux` then fail with
// "not recognized as the name of a cmdlet" although psmux is running.

#[cfg(windows)]
fn own_dir() -> String {
    std::env::current_exe()
        .expect("current_exe()")
        .parent()
        .expect("current_exe() has a parent")
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn child_path(cmd: &std::process::Command) -> Option<String> {
    cmd.get_envs()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .and_then(|(_, value)| value.map(|v| v.to_string_lossy().into_owned()))
}

#[cfg(windows)]
fn path_entries(path: &str) -> Vec<&str> {
    path.split(';').collect()
}

#[test]
#[cfg(windows)]
fn prepend_if_absent_prepends_missing_dir() {
    assert_eq!(
        prepend_if_absent(r"C:\Windows;C:\Windows\System32", r"D:\psmux").as_deref(),
        Some(r"D:\psmux;C:\Windows;C:\Windows\System32")
    );
}

#[test]
#[cfg(windows)]
fn prepend_if_absent_handles_empty_path() {
    assert_eq!(prepend_if_absent("", r"D:\psmux").as_deref(), Some(r"D:\psmux"));
}

#[test]
#[cfg(windows)]
fn prepend_if_absent_keeps_an_existing_entry() {
    assert_eq!(prepend_if_absent(r"D:\psmux;C:\Windows", r"D:\psmux"), None);
}

#[test]
#[cfg(windows)]
fn prepend_if_absent_ignores_case_and_trailing_separator() {
    assert_eq!(prepend_if_absent(r"d:\PSMUX\;C:\Windows", r"D:\psmux"), None);
    assert_eq!(prepend_if_absent(r"C:\Windows;D:\psmux", r"D:\psmux\"), None);
}

/// The reported failure: a server whose `PATH` does not contain the psmux
/// directory. The child still has to be able to resolve `psmux`, and the
/// directory must show up exactly once.
#[test]
#[cfg(windows)]
fn run_shell_child_gets_psmux_dir_on_a_poisoned_path() {
    let mut cmd = std::process::Command::new("cmd");
    apply_own_dir_to_path(&mut cmd, r"C:\Windows;C:\Windows\System32");

    let path = child_path(&cmd).expect("the child must receive an explicit PATH");
    let dir = own_dir();
    let entries = path_entries(&path);
    assert_eq!(
        entries.first().map(|entry| entry.trim_end_matches(|c| c == '\\' || c == '/')),
        Some(dir.trim_end_matches(|c| c == '\\' || c == '/')),
        "expected {dir} first on the child PATH, got {path}"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.trim_end_matches(|c| c == '\\' || c == '/').eq_ignore_ascii_case(&dir))
            .count(),
        1,
        "expected {dir} exactly once on the child PATH, got {path}"
    );
    assert!(
        path.ends_with(r"C:\Windows;C:\Windows\System32"),
        "the inherited PATH must be preserved after the prepended entry, got {path}"
    );
}

/// An environment that already knows about psmux is left exactly as the caller
/// set it up: no duplicate entry, no reordering.
#[test]
#[cfg(windows)]
fn run_shell_child_path_is_untouched_when_psmux_is_already_on_it() {
    let mut cmd = std::process::Command::new("cmd");
    apply_own_dir_to_path(&mut cmd, &format!("{};C:\\Windows", own_dir()));
    assert!(
        child_path(&cmd).is_none(),
        "an inherited PATH that already contains the psmux directory must not be rewritten"
    );
}


// ─── a plugin's .tmux entry script runs with Git Bash ──────────────────────

/// A Git for Windows install under `root`: its `bin\bash.exe`, the MSYS
/// `usr\bin\bash.exe` beside it and `cmd\git.exe`.
#[cfg(windows)]
fn fake_git_install(root: &std::path::Path) {
    for dir in ["bin", "usr\\bin", "cmd"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in ["bin\\bash.exe", "usr\\bin\\bash.exe", "cmd\\git.exe"] {
        std::fs::write(root.join(file), b"").unwrap();
    }
}

/// Git Bash is the `bin\bash.exe` of the install `git.exe` belongs to, else
/// of an install directory, and only with its MSYS bash beside it.
#[test]
#[cfg(windows)]
fn git_bash_is_found_from_git_or_an_install_directory() {
    let tmp = std::env::temp_dir().join(format!("psmux_test_git_bash_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (installed, other, stray) = (tmp.join("Git"), tmp.join("Other"), tmp.join("Stray"));
    fake_git_install(&installed);
    fake_git_install(&other);
    std::fs::create_dir_all(stray.join("bin")).unwrap();
    std::fs::write(stray.join("bin").join("bash.exe"), b"").unwrap();

    let git = installed.join("cmd").join("git.exe");
    assert_eq!(find_git_bash(Some(&git), &[other.clone()]), Some(installed.join("bin").join("bash.exe")));
    assert_eq!(find_git_bash(None, &[stray.clone(), other.clone()]), Some(other.join("bin").join("bash.exe")));
    assert_eq!(find_git_bash(Some(&stray.join("bin").join("git.exe")), &[stray.clone()]), None);

    let _ = std::fs::remove_dir_all(&tmp);
}

/// A run-shell command that is one `.tmux` file runs it with Git Bash, its
/// path written with `/` so the script's `dirname "${BASH_SOURCE[0]}"`
/// finds its directory.
#[test]
#[cfg(windows)]
fn a_tmux_script_runs_with_git_bash() {
    let Some(bash) = git_bash() else { return };
    let tmp = std::env::temp_dir().join(format!("psmux_test_tmux_script_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let script = tmp.join("p.tmux");
    std::fs::write(&script, "tmux set -g @p on\n").unwrap();

    let cmd = build_run_shell_command(&format!("'{}'", script.display()));
    assert_eq!(std::path::Path::new(cmd.get_program()), bash);
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert_eq!(args, vec![script.to_string_lossy().replace('\\', "/")]);

    let _ = std::fs::remove_dir_all(&tmp);
}
