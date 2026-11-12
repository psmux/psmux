// run-shell and if-shell conditions pick their shell with resolve_run_shell.
// Under an SSH command or in session 0 a Store packaged pwsh cannot start
// (ACCESS_DENIED from MSIX activation), and panes already skip it
// (pane::cached_shell). run-shell did not: every run-shell and if-shell
// condition failed with "Access is denied" over SSH on a Store-pwsh-only box.
use super::*;

#[cfg(windows)]
#[test]
fn run_shell_never_picks_a_store_shell_where_store_apps_cannot_start() {
    let _g = crate::util::lock_test_env();
    let saved = std::env::var_os("SSH_CONNECTION");
    std::env::set_var("SSH_CONNECTION", "10.0.0.1 50000 10.0.0.2 22");
    let (prog, args) = resolve_run_shell();
    match saved {
        Some(v) => std::env::set_var("SSH_CONNECTION", v),
        None => std::env::remove_var("SSH_CONNECTION"),
    }
    assert!(!crate::pane::is_store_shell_path(&prog), "picked a Store shell under SSH: {prog}");
    assert!(!args.is_empty(), "the shell needs its command flag");
}

#[test]
fn store_shell_unusable_follows_the_ssh_markers() {
    let _g = crate::util::lock_test_env();
    let saved = std::env::var_os("SSH_CLIENT");
    std::env::set_var("SSH_CLIENT", "10.0.0.1 50000 22");
    let unusable = crate::pane::store_shell_unusable();
    match saved {
        Some(v) => std::env::set_var("SSH_CLIENT", v),
        None => std::env::remove_var("SSH_CLIENT"),
    }
    assert!(unusable);
}
