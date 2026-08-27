// Warm-pane `-c <dir>` re-home: the fast paths (create_window / split) now
// transplant the warm pane even when a start_dir is requested, then silently
// re-home the pre-spawned shell to that directory via `silent_rehome`.
//
// These unit tests cover the pure command-construction half (`rehome_command`),
// which is the security-sensitive part (single-quote escaping). The full
// behavioural path — warm pane actually consumed for `new-window -c` and the
// shell landing in the requested dir — needs real ConPTY/shell scaffolding and
// is covered by tests/test_warm_pane_start_dir.ps1.

use super::*;

/// The injected command must: start with a space (kept out of shell history),
/// `cd` into the requested directory, chain a clear to hide the echo, and end
/// with a single CR so it submits as exactly one command line.
#[test]
fn rehome_command_wraps_dir_and_clears() {
    // Platform default dialect: PowerShell on Windows, POSIX elsewhere. Both
    // pass a backslash path through untouched, so the `cd` assertion below
    // holds either way.
    let cmd = rehome_command(r"C:\code\project", rehome_syntax_for_shell(""));
    assert!(cmd.starts_with(' '), "must start with a space, got {cmd:?}");
    assert!(cmd.ends_with('\r'), "must end with CR, got {cmd:?}");
    assert!(
        cmd.contains(r"cd 'C:\code\project'"),
        "must cd into the dir, got {cmd:?}"
    );
    let clear = if cfg!(windows) { "cls" } else { "clear" };
    assert!(cmd.contains(clear), "must chain {clear}, got {cmd:?}");
    assert_eq!(cmd.matches('\r').count(), 1, "exactly one line, got {cmd:?}");
}

/// A single quote in the path must be doubled so the single-quoted string stays
/// well-formed — otherwise the `cd` breaks (or a crafted path could inject a
/// second command). Precondition: the input actually contains a lone quote.
/// Doubling is the PowerShell rule; the POSIX `'\''` rule is covered in
/// tests-rs/test_issue600_bash_rehome.rs.
#[test]
fn rehome_command_escapes_single_quotes() {
    let input = r"C:\weird'dir";
    assert_eq!(input.matches('\'').count(), 1, "precondition: one lone quote");

    let cmd = rehome_command(input, RehomeSyntax::PowerShell);
    assert!(
        cmd.contains(r"cd 'C:\weird''dir'"),
        "lone quote must be doubled, got {cmd:?}"
    );
    // Still exactly one command line — the quote didn't terminate it early.
    assert_eq!(cmd.matches('\r').count(), 1, "got {cmd:?}");
}

/// Full contract lock on Windows: documents the precise bytes injected so a
/// future refactor can't silently change the wire format. Includes the
/// `SetCurrentDirectory` sync (see `rehome_command` doc comment) — PowerShell's
/// `cd` does not call Win32 `SetCurrentDirectory()`, so `#{pane_current_path}`
/// (a PEB walk) would otherwise keep reporting the shell's original spawn
/// directory after a warm-pane rehome, especially under `-NoProfile` where
/// psmux's own CWD_SYNC profile hook is skipped.
#[test]
fn rehome_command_exact_windows_form() {
    if cfg!(windows) {
        assert_eq!(
            rehome_command(r"C:\x", RehomeSyntax::PowerShell),
            " cd 'C:\\x'; try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; cls\r"
        );
    }
}

/// The CurrentDirectory sync must apply to the *requested* dir, not stay
/// hardcoded — precondition guard so `rehome_command_exact_windows_form`
/// above isn't the only thing pinning this behaviour.
#[test]
fn rehome_command_includes_current_directory_sync_on_windows() {
    if cfg!(windows) {
        let cmd = rehome_command(r"C:\code\project", RehomeSyntax::PowerShell);
        assert!(
            cmd.contains("[System.IO.Directory]::SetCurrentDirectory"),
            "must sync Win32 CurrentDirectory so a PEB-walk-based cwd query \
             (#{{pane_current_path}}) reflects the rehomed dir, got {cmd:?}"
        );
        // Still exactly one submitted line — the sync must be chained with
        // `;`, not its own Enter, or it would submit as a separate command.
        assert_eq!(cmd.matches('\r').count(), 1, "got {cmd:?}");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Behavioural: the `-c <dir>` fast path with real ConPTY spawns.
//
// These prove #436 composes with #450's dead-spare liveness gate: the warm-pane
// guard admits `-c` requests, on the path #450's gate protects. A LIVE spare must
// be transplanted AND re-homed for `-c`; a DEAD spare must never be transplanted
// (the #450 bug), falling through to a cold spawn that still honours `-c` via
// `shell_cmd.cwd`.
//
// Discriminators are structural, not timing-based: transplant ⇔ the delivered
// pane keeps the warm pane's id; re-home ⇔ `silent_rehome` set `squelch_until`.
// Landing the *cold* spawn in the requested dir is verified end-to-end by
// tests/test_warm_pane_start_dir.ps1 (a running process's CWD isn't cheaply
// observable in-process).

fn test_app() -> AppState {
    let mut app = AppState::new("warm_start_dir_test".to_string());
    app.warm_enabled = true;
    app.last_window_area = ratatui::prelude::Rect { x: 0, y: 0, width: 100, height: 30 };
    app
}

/// Kill the warm pane's child and wait until the OS reports it exited
/// (TerminateProcess is asynchronous; try_wait flips within milliseconds).
fn kill_warm_child(wp: &mut crate::types::WarmPane) {
    wp.child.kill().ok();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !matches!(wp.child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("warm child did not report exit within 5s of kill()");
}

fn active_pane_of(win: &mut Window) -> &mut Pane {
    let path = win.active_path.clone();
    active_pane_mut(&mut win.root, &path).expect("active pane")
}

fn cleanup(app: &mut AppState) {
    for win in app.windows.iter_mut() {
        crate::tree::kill_all_children(&mut win.root);
    }
    if let Some(mut wp) = app.warm_pane.take() {
        wp.child.kill().ok();
    }
}

/// Control / precondition: a live spare consumed WITHOUT `-c` is transplanted
/// but NOT re-homed, so `squelch_until` stays `None`. This is what makes the
/// `is_some()` check in `create_window_live_spare_with_start_dir_transplants_and_rehomes`
/// meaningful — without it, that assertion could pass vacuously.
#[test]
fn create_window_live_spare_without_start_dir_does_not_rehome() {
    let pty = native_pty_system();
    let mut app = test_app();
    let wp = spawn_warm_pane(&*pty, &mut app).expect("spawn warm pane");
    let warm_id = wp.pane_id;
    app.warm_pane = Some(wp);

    create_window(&*pty, &mut app, None, None, false).expect("create_window");

    let pane = active_pane_of(&mut app.windows[0]);
    assert_eq!(pane.id, warm_id, "live spare must be transplanted (warm fast path)");
    assert!(
        pane.squelch_until.is_none(),
        "no start_dir means no silent_rehome, so squelch_until must be None"
    );
    cleanup(&mut app);
}

/// #436 core: a live spare consumed WITH `-c <dir>` is STILL transplanted (warm
/// reuse — the whole point) and additionally re-homed, so `silent_rehome` runs
/// and sets `squelch_until`.
#[test]
fn create_window_live_spare_with_start_dir_transplants_and_rehomes() {
    let pty = native_pty_system();
    let mut app = test_app();
    let dir = std::env::temp_dir();
    let dir = dir.to_str().expect("temp dir path");
    let wp = spawn_warm_pane(&*pty, &mut app).expect("spawn warm pane");
    let warm_id = wp.pane_id;
    app.warm_pane = Some(wp);

    create_window(&*pty, &mut app, None, Some(dir), false).expect("create_window");

    let pane = active_pane_of(&mut app.windows[0]);
    assert_eq!(
        pane.id, warm_id,
        "#436: the warm spare must be transplanted even with -c (not cold-spawned)"
    );
    assert!(
        pane.squelch_until.is_some(),
        "#436: -c must silently re-home the transplanted shell (squelch_until set)"
    );
    assert!(
        matches!(pane.child.try_wait(), Ok(None)),
        "the transplanted shell must be alive"
    );
    cleanup(&mut app);
}

/// #450 gate under #436's looser guard: a DEAD spare must not be transplanted
/// just because `-c` makes the warm pane eligible. #450's liveness gate must
/// still reject it and fall through to a cold spawn.
#[test]
fn create_window_dead_spare_with_start_dir_cold_spawns() {
    let pty = native_pty_system();
    let mut app = test_app();
    let dir = std::env::temp_dir();
    let dir = dir.to_str().expect("temp dir path");
    let mut wp = spawn_warm_pane(&*pty, &mut app).expect("spawn warm pane");
    let warm_id = wp.pane_id;
    kill_warm_child(&mut wp);
    app.warm_pane = Some(wp);

    create_window(&*pty, &mut app, None, Some(dir), false).expect("create_window");

    assert!(app.warm_pane.is_none(), "the dead spare must be discarded, not restored");
    let pane = active_pane_of(&mut app.windows[0]);
    assert_ne!(
        pane.id, warm_id,
        "dead spare (id {warm_id}) was transplanted for -c — the #450 bug #436 must not reopen"
    );
    assert!(
        matches!(pane.child.try_wait(), Ok(None)),
        "the cold-spawned shell must be alive"
    );
    cleanup(&mut app);
}

/// Same gate on the split path: a dead spare must not be transplanted into a
/// `split-window -c`; the split still succeeds via cold spawn.
#[test]
fn split_dead_spare_with_start_dir_cold_spawns() {
    let pty = native_pty_system();
    let mut app = test_app();
    let dir = std::env::temp_dir();
    let dir = dir.to_str().expect("temp dir path");
    create_window(&*pty, &mut app, None, None, false).expect("create_window");
    let mut wp = spawn_warm_pane(&*pty, &mut app).expect("spawn warm pane");
    let warm_id = wp.pane_id;
    kill_warm_child(&mut wp);
    app.warm_pane = Some(wp);

    split_active_with_command(&mut app, LayoutKind::Vertical, None, Some(&*pty), Some(dir))
        .expect("split");

    let win = &mut app.windows[0];
    assert!(matches!(win.root, Node::Split { .. }), "split must still happen");
    let pane = active_pane_of(win);
    assert_ne!(pane.id, warm_id, "dead spare transplanted into the -c split (#450 bug)");
    assert!(
        matches!(pane.child.try_wait(), Ok(None)),
        "the split's cold-spawned shell must be alive"
    );
    cleanup(&mut app);
}
