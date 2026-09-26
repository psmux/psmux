//! Issue #630: a pane's ConPTY host must never hold the pane's start directory.
//!
//! Reported as "Split pane's ConPTY host pins source directory, blocking
//! deletion": after `split-window -c X`, directory X could not be deleted for
//! as long as the split pane lived, even once its shell had `cd`'d elsewhere:
//!
//!   Remove-Item: The process cannot access the file '...\X'
//!   because it is being used by another process.
//!
//! MEASURED cause (PEB `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory` of every
//! psmux descendant, before the fix):
//!
//!   83676  psmux     ...\neutral
//!   55360  conhost   ...\X        <== the pane's ConPTY host, parked in X for ever
//!   34044  pwsh      ...\neutral  <== the pane's shell, correctly moved out
//!
//! The server used to `env::set_current_dir(start_dir)` around pane creation.
//! `CreatePseudoConsole()` spawns the pane's `conhost.exe` from the server
//! process, so the host inherited that directory as its own Win32 current
//! directory and held an open handle on it for the pane's whole life. No `cd`
//! in the pane can move a console host, so the directory stayed locked until
//! the pane died.
//!
//! The start directory has always ALSO been delivered to the pane's shell
//! explicitly, as `lpCurrentDirectory` on `CreateProcessW` (a cold spawn) or as
//! an injected `cd` (a warm transplant), so the server chdir bought nothing.
//! The fix deletes those transient chdirs; see `src/server/mod.rs`.
//!
//! tmux parity: tmux `chdir()`s in the forked child only, so nothing but the
//! pane's own process ever holds the start directory, and it releases it on the
//! first `cd`. psmux now matches: the shell may hold X while it is really in X
//! (that is correct and identical to tmux), but no psmux-owned helper process
//! does.
//!
//! End to end coverage lives in `tests/test_issue630_conpty_cwd_pin.ps1`.

/// The server may keep exactly two Win32 current directories, and both are
/// deliberate and documented in place:
///
///   1. the session start directory, adopted once at session creation and kept
///      for the server's life so `new-window`/`split-window` without `-c`
///      inherit it, and
///   2. the client's directory adopted when a warm standby server is claimed.
///
/// Every OTHER `set_current_dir` on a pane creation path is the #630 defect:
/// whatever the server's current directory happens to be at
/// `CreatePseudoConsole()` time becomes a permanent handle held by the pane's
/// console host. Guard the count so a future refactor cannot quietly bring the
/// dance back on the `new-window` / `split-window` / `display-popup` paths.
#[test]
fn server_does_not_chdir_on_pane_creation_paths() {
    let src = include_str!("../src/server/mod.rs");
    let hits = src.matches("set_current_dir").count();
    assert_eq!(
        hits, 2,
        "src/server/mod.rs has {hits} set_current_dir call(s), expected exactly 2 \
         (session start directory, warm server claim). Issue #630: chdir'ing the \
         server around pane creation makes the pane's conhost.exe inherit the \
         start directory and pin it for the pane's whole life. Deliver the start \
         directory to the pane's SHELL instead, via CommandBuilder::cwd() for a \
         cold spawn or pane::silent_rehome() for a warm transplant."
    );
}

/// The start directory must still reach the pane, and it must reach it through
/// the command builder rather than through the server's process directory.
#[test]
fn start_dir_is_carried_on_the_command_not_the_process() {
    let src = include_str!("../src/pane.rs");
    // create_window_with_env and split_active_with_env both route -c through
    // usable_start_dir onto the builder. Two occurrences, one per path.
    let hits = src.matches("shell_cmd.cwd(usable)").count();
    assert!(
        hits >= 2,
        "expected the window-create and split paths to set the pane shell's cwd \
         explicitly from -c (found {hits}); without it, removing the server chdir \
         for #630 would leave -c with no effect at all"
    );
}

/// The real invariant, exercised against a live ConPTY: a directory handed to a
/// child as its start directory must be deletable once that child is gone, even
/// though the pseudo console (and therefore its `conhost.exe`) is still open.
///
/// This is the portable-pty half of #630. It fails if anything in the pty layer
/// leaks the child's start directory into the host, which is exactly what the
/// server's chdir used to do from the outside.
#[cfg(windows)]
#[test]
fn pty_host_does_not_pin_the_child_start_dir() {
    use portable_pty::{CommandBuilder, PtySize};
    use std::path::PathBuf;

    let uniq = format!(
        "psmux-i630-rs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let dir: PathBuf = std::env::temp_dir().join(uniq);
    std::fs::create_dir_all(&dir).expect("create temp start dir");

    // The pseudo console (and its conhost.exe) is created HERE, with this test
    // process's current directory. It must never adopt `dir` below.
    let pty_system = portable_pty::native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            // No ConPTY available (very old Windows, or a restricted CI job).
            // Nothing to assert; do not fail the suite over a missing API.
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("skipping: openpty unavailable: {e}");
            return;
        }
    };

    // A short lived child whose START DIRECTORY is `dir`. While it runs it
    // legitimately holds the directory, exactly as a shell sitting in it would.
    let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
    let mut cmd = CommandBuilder::new(&comspec);
    cmd.args(["/c", "exit"]);
    cmd.cwd(&dir);

    let mut child = match crate::util::spawn_pty_child(&*pair.slave, cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("skipping: spawn_command failed: {e}");
            return;
        }
    };
    drop(pair.slave);
    let _ = child.wait();

    // The child is gone; the pseudo console and its host are still alive. If the
    // host had inherited the child's start directory, this delete fails with
    // Win32 error 32.
    let mut last_err = None;
    let mut deleted = false;
    for _ in 0..40 {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {
                deleted = true;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }

    // Keep the pty (and its host) alive across the delete attempt.
    drop(pair.master);

    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        deleted,
        "issue #630: the start directory of a ConPTY child is still locked after \
         the child exited, so the pseudo console host inherited it: {last_err:?}"
    );
}
