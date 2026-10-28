//! Issue #924: in a context where an MSIX/Store packaged shell cannot be
//! activated (Windows session 0: OpenSSH, services, scheduled tasks), the
//! default shell must not be the Store `pwsh.exe` alias, or every pane spawn
//! fails with ACCESS_DENIED.
//!
//! Reproduced on 9b5c3e0e over `ssh localhost` with SHELL and the SSH_*
//! markers removed: `new-session -d` failed with
//! `CreateProcessW ...\WindowsApps\pwsh.exe ... Access is denied. (os error 5)`.
//! These tests pin the selection rule over explicit inputs; the live
//! behaviour is covered by tests/test_issue924_store_shell_session0.ps1.

use super::*;

const ALIAS: &str = "C:\\Users\\u\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe";
const INTERIOR: &str =
    "C:\\Program Files\\WindowsApps\\Microsoft.PowerShell_7.5.4.0_x64__8wekyb3d8bbwe\\pwsh.exe";
const CLASSIC: &str = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
const CMD: &str = "C:\\Windows\\System32\\cmd.exe";
const MSI_PWSH: &str = "C:\\Program Files\\PowerShell\\7\\pwsh.exe";

fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

#[test]
fn store_paths_are_recognised() {
    assert!(is_store_shell_path(ALIAS));
    assert!(is_store_shell_path(INTERIOR));
    assert!(!is_store_shell_path(CLASSIC));
    assert!(!is_store_shell_path(MSI_PWSH));
}

#[test]
fn store_walk_result_falls_back_to_classic_powershell_when_unusable() {
    let got = select_default_shell(None, s(ALIAS), || true, || s(CLASSIC), || s(CMD));
    assert_eq!(got.as_deref(), Some(CLASSIC));
}

#[test]
fn falls_back_to_cmd_when_no_classic_powershell() {
    let got = select_default_shell(None, s(INTERIOR), || true, || None, || s(CMD));
    assert_eq!(got.as_deref(), Some(CMD));
}

#[test]
fn classic_powershell_that_is_itself_a_store_path_is_skipped() {
    let got = select_default_shell(None, s(ALIAS), || true, || s(ALIAS), || s(CMD));
    assert_eq!(got.as_deref(), Some(CMD));
}

#[test]
fn store_shell_from_env_also_falls_back_when_unusable() {
    // tmux keeps $SHELL only when checkshell() passes; a Store shell in
    // session 0 cannot be executed, so it is skipped like a non executable.
    let got = select_default_shell(s(ALIAS), s(MSI_PWSH), || true, || s(CLASSIC), || s(CMD));
    assert_eq!(got.as_deref(), Some(CLASSIC));
}

#[test]
fn store_shell_is_kept_in_an_interactive_session() {
    let got = select_default_shell(None, s(ALIAS), || false, || s(CLASSIC), || s(CMD));
    assert_eq!(got.as_deref(), Some(ALIAS));
}

#[test]
fn non_store_shell_never_probes_the_session() {
    // Fast path: a non Store choice must not evaluate the probe or fallbacks.
    let got = select_default_shell(
        None,
        s(MSI_PWSH),
        || panic!("session probe must not run for a non Store shell"),
        || panic!("fallback lookup must not run"),
        || panic!("fallback lookup must not run"),
    );
    assert_eq!(got.as_deref(), Some(MSI_PWSH));
}

#[test]
fn shell_env_still_wins_over_the_walk() {
    let got = select_default_shell(s(CMD), s(ALIAS), || true, || s(CLASSIC), || None);
    assert_eq!(got.as_deref(), Some(CMD));
}

#[test]
fn no_fallback_available_keeps_the_store_shell_so_the_spawn_reports_it() {
    let got = select_default_shell(None, s(ALIAS), || true, || None, || None);
    assert_eq!(got.as_deref(), Some(ALIAS));
}

#[test]
fn nothing_found_is_none() {
    assert_eq!(select_default_shell(None, None, || true, || s(CLASSIC), || s(CMD)), None);
}

#[cfg(windows)]
#[test]
fn session_zero_probe_matches_the_process_session() {
    // A desktop test run is never session 0 (Windows OpenSSH always is, so
    // the check only asserts outside SSH).
    let over_ssh = std::env::var_os("SSH_CONNECTION").is_some();
    if !over_ssh {
        assert!(!in_session_zero(), "a desktop test run is not session 0");
    }
}
