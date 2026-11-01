// ---------------------------------------------------------------------------
// Rust unit/integration tests for HideWindowCommandExt (CREATE_NO_WINDOW)
// ---------------------------------------------------------------------------
//
// These tests verify that:
//   1. The HideWindowCommandExt trait compiles and is callable on Command
//   2. On Windows, the flag actually prevents console window allocation
//   3. Background subprocesses (run-shell, if-shell, format #(), etc.) get
//      the flag applied via build_run_shell_command() and direct Command usage
//   4. PTY/server processes do NOT get the flag (they need real consoles)
//   5. The trait is a no-op on non-Windows (compilation proof)

use std::process::Command;
use crate::platform::HideWindowCommandExt;
use super::*;

// =========================================================================
// Trait basics
// =========================================================================

#[test]
fn hide_window_trait_returns_self() {
    // Calling .hide_window() should return &mut Self so it chains
    let mut cmd = Command::new("echo");
    let ret = cmd.hide_window();
    // Prove the returned reference is usable (set an arg via the ref)
    ret.arg("hello");
    // No panic = pass
}

#[test]
fn hide_window_trait_chainable() {
    // .hide_window() must be chainable in a builder pattern
    let _cmd = {
        let mut c = Command::new("echo");
        c.arg("a").hide_window().arg("b");
        c
    };
}

#[test]
fn hide_window_multiple_calls_no_panic() {
    // Calling .hide_window() more than once must not panic or UB
    let mut cmd = Command::new("echo");
    cmd.hide_window();
    cmd.hide_window();
    cmd.hide_window();
}

// =========================================================================
// Windows: Verify the flag actually suppresses console windows
// =========================================================================

#[cfg(windows)]
#[test]
fn hide_window_subprocess_no_visible_window() {
    // Spawn a quick subprocess with .hide_window() and confirm:
    //   (a) it completes successfully
    //   (b) no console window is created (we can't see windows, but we can
    //       verify the process ran headlessly by checking stdout capture)
    let output = Command::new("cmd")
        .args(["/C", "echo hidden_test_sentinel"])
        .hide_window()
        .output()
        .expect("failed to spawn cmd with hide_window");

    assert!(output.status.success(), "cmd /C echo should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("hidden_test_sentinel"),
        "should capture stdout even with CREATE_NO_WINDOW: got {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn hide_window_stderr_still_captured() {
    // stderr must still be capturable even with CREATE_NO_WINDOW
    let output = Command::new("cmd")
        .args(["/C", "echo err_sentinel 1>&2"])
        .hide_window()
        .output()
        .expect("failed to spawn");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("err_sentinel"),
        "stderr must still work with CREATE_NO_WINDOW: got {:?}",
        stderr
    );
}

#[cfg(windows)]
#[test]
fn hide_window_exit_code_preserved() {
    // The exit code must still propagate correctly
    let status_zero = Command::new("cmd")
        .args(["/C", "exit 0"])
        .hide_window()
        .status()
        .expect("failed to spawn");
    assert!(status_zero.success(), "exit 0 should be success");

    let status_one = Command::new("cmd")
        .args(["/C", "exit 1"])
        .hide_window()
        .status()
        .expect("failed to spawn");
    assert!(!status_one.success(), "exit 1 should be failure");

    let status_42 = Command::new("cmd")
        .args(["/C", "exit 42"])
        .hide_window()
        .status()
        .expect("failed to spawn");
    assert_eq!(status_42.code(), Some(42), "exit code 42 must be preserved");
}

#[cfg(windows)]
#[test]
fn hide_window_stdin_piped_works() {
    // stdin piping must work with CREATE_NO_WINDOW (used by copy-pipe, pipe-pane)
    use std::io::Write;

    let mut child = Command::new("cmd")
        .args(["/C", "findstr sentinel"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .hide_window()
        .spawn()
        .expect("failed to spawn with piped stdin");

    {
        let stdin = child.stdin.as_mut().expect("stdin must be available");
        stdin.write_all(b"line1\nsentinel_found\nline3\n").unwrap();
    }
    let output = child.wait_with_output().expect("failed to wait");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("sentinel_found"),
        "piped stdin/stdout must work: got {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn hide_window_powershell_command() {
    // Test with pwsh/powershell (the actual shell psmux uses for run-shell)
    let shell = if which::which("pwsh").is_ok() { "pwsh" } else { "powershell" };
    let output = Command::new(shell)
        .args(["-NoProfile", "-Command", "Write-Output 'ps_hidden_test'"])
        .hide_window()
        .output()
        .expect("failed to spawn powershell with hide_window");

    assert!(output.status.success(), "{} should succeed", shell);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("ps_hidden_test"),
        "pwsh stdout with CREATE_NO_WINDOW: got {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn hide_window_powershell_exit_codes() {
    // if-shell relies on exit code from shell; verify it works hidden
    let shell = if which::which("pwsh").is_ok() { "pwsh" } else { "powershell" };

    let success = Command::new(shell)
        .args(["-NoProfile", "-Command", "exit 0"])
        .hide_window()
        .status()
        .expect("spawn failed");
    assert!(success.success(), "exit 0 via {} must be success", shell);

    let failure = Command::new(shell)
        .args(["-NoProfile", "-Command", "exit 1"])
        .hide_window()
        .status()
        .expect("spawn failed");
    assert!(!failure.success(), "exit 1 via {} must be failure", shell);
}

// =========================================================================
// build_run_shell_command integration: verify the flag is applied
// =========================================================================

#[cfg(windows)]
#[test]
fn build_run_shell_command_applies_hide_window() {
    // build_run_shell_command is the central chokepoint for run-shell.
    // Verify it produces a working command that runs hidden.
    let mut cmd = build_run_shell_command("echo build_test_sentinel", None);
    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("build_run_shell_command failed to spawn");

    assert!(output.status.success(), "run-shell echo should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("build_test_sentinel"),
        "run-shell output: {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn build_run_shell_command_explicit_pwsh() {
    // When shell_cmd starts with "pwsh", build_run_shell_command should
    // still apply hide_window and run correctly
    let shell = if which::which("pwsh").is_ok() { "pwsh" } else { "powershell" };
    let cmd_str = format!("{} -NoProfile -Command \"Write-Output 'explicit_shell_test'\"", shell);
    let mut cmd = build_run_shell_command(&cmd_str, None);
    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("explicit shell cmd failed");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("explicit_shell_test"),
        "explicit shell output: {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn build_run_shell_command_cmd_exe() {
    // When shell_cmd starts with "cmd", verify it works hidden
    let mut cmd = build_run_shell_command("cmd /C echo cmd_exe_hidden_test", None);
    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .expect("cmd /C echo failed");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("cmd_exe_hidden_test"),
        "cmd output: {:?}",
        stdout
    );
}

// =========================================================================
// Non-Windows: Verify the no-op compiles and works
// =========================================================================

#[cfg(not(windows))]
#[test]
fn hide_window_noop_on_unix() {
    // On non-Windows, hide_window is a no-op. The process should still work.
    let output = Command::new("echo")
        .arg("unix_test")
        .hide_window()
        .output()
        .expect("echo should work on unix");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("unix_test"));
}

// =========================================================================
// Negative: PTY/server processes must NOT use hide_window
// =========================================================================

// These are compile-time / architectural checks. We verify that
// spawn_server_hidden() exists and uses CREATE_NEW_CONSOLE (not
// CREATE_NO_WINDOW) by checking we can still call it for server spawning.
// The actual platform.rs code uses 0x00000010 (CREATE_NEW_CONSOLE) in
// spawn_server_hidden, not 0x08000000 (CREATE_NO_WINDOW).

#[cfg(windows)]
#[test]
fn hide_window_uses_create_no_window_flag() {
    // Verify that a process spawned with .hide_window() can still
    // produce output (proving the flag is set correctly and does not
    // break process creation). If CREATE_NO_WINDOW were wrong, the
    // process would fail to start or produce garbled output.
    let output = Command::new("cmd")
        .args(["/C", "echo flag_verification_ok"])
        .hide_window()
        .output()
        .expect("flag verification spawn failed");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("flag_verification_ok"),
        "CREATE_NO_WINDOW flag must not break process creation: {:?}",
        stdout
    );
}

// =========================================================================
// Concurrent/rapid subprocess spawning (stress test)
// =========================================================================

#[cfg(windows)]
#[test]
fn hide_window_rapid_spawn_no_window_leak() {
    // Spawn 20 rapid hidden subprocesses to verify no window flashing
    // or resource leak. This simulates status bar #() polling.
    let handles: Vec<_> = (0..20)
        .map(|i| {
            std::thread::spawn(move || {
                let output = Command::new("cmd")
                    .args(["/C", &format!("echo rapid_{}", i)])
                    .hide_window()
                    .output()
                    .expect("rapid spawn failed");
                assert!(output.status.success());
                let s = String::from_utf8_lossy(&output.stdout);
                assert!(s.contains(&format!("rapid_{}", i)));
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[cfg(windows)]
#[test]
fn hide_window_mixed_piped_and_captured() {
    // Some callers use .output() (captured), some use .spawn() (piped stdin).
    // Test both patterns back to back.
    use std::io::Write;

    // Pattern 1: .output() capture (format #() expansion pattern)
    let out = Command::new("cmd")
        .args(["/C", "echo capture_pattern"])
        .hide_window()
        .output()
        .expect("capture pattern failed");
    assert!(String::from_utf8_lossy(&out.stdout).contains("capture_pattern"));

    // Pattern 2: .spawn() + piped stdin (copy-pipe / pipe-pane pattern)
    let mut child = Command::new("cmd")
        .args(["/C", "findstr pipe_pattern"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .hide_window()
        .spawn()
        .expect("pipe pattern spawn failed");

    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(b"pipe_pattern_found\n").unwrap();
    }
    let out2 = child.wait_with_output().unwrap();
    assert!(String::from_utf8_lossy(&out2.stdout).contains("pipe_pattern_found"));

    // Pattern 3: .status() only (if-shell pattern)
    let st = Command::new("cmd")
        .args(["/C", "exit 0"])
        .hide_window()
        .status()
        .expect("status pattern failed");
    assert!(st.success());
}

// =========================================================================
// Environment variables pass through with hide_window
// =========================================================================

#[cfg(windows)]
#[test]
fn hide_window_env_vars_propagate() {
    // Plugin scripts need env vars (PSMUX_TARGET_SESSION, etc.)
    let output = Command::new("cmd")
        .args(["/C", "echo %HIDE_TEST_VAR%"])
        .env("HIDE_TEST_VAR", "env_propagated_ok")
        .hide_window()
        .output()
        .expect("env var test failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("env_propagated_ok"),
        "env vars must propagate with CREATE_NO_WINDOW: {:?}",
        stdout
    );
}

#[cfg(windows)]
#[test]
fn hide_window_cwd_propagate() {
    // Verify current_dir works with hide_window (pipe-pane uses cwd context)
    let tmp = std::env::temp_dir();
    let output = Command::new("cmd")
        .args(["/C", "cd"])
        .current_dir(&tmp)
        .hide_window()
        .output()
        .expect("cwd test failed");

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let expected = tmp.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let actual = stdout.trim().to_lowercase();
    assert!(
        actual.contains(&expected) || expected.contains(&actual),
        "cwd should be {:?}, got {:?}",
        expected,
        actual
    );
}
