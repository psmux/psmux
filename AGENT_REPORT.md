# Issue #672 report

Worktree: `C:\Users\godwin\Documents\workspace\psmux\.claude\worktrees\agent-ad99a977a1772f253`
Branch: `worktree-agent-ad99a977a1772f253`, base `37e2990`.

## 1. Reproduction

The reported symptom does NOT reproduce on master. It reproduces exactly on the
reporter's own build.

The reporter states their version: `psmux 3.3.8 (66cf613 2026-08-18)`, WinGet. I built that
revision (`git worktree add ... v3.3.8`, `cargo build --release`) and confirmed the binary:

    tmux 3.3.8
    psmux 3.3.8 (66cf613 2026-08-18)

Running the new E2E against that binary, with `set -g default-shell "C:/Program Files/Git/bin/bash.exe"`:

    [FAIL] git bash, full path, forward slashes / split-window -c : pane scrollback contains a syntax error -> bash: syntax error near unexpected token `('
    [FAIL] git bash, full path, forward slashes / split-window -c : PowerShell rehome form reached a POSIX shell (found: SetCurrentDirectory, ProviderPath, try {, } catch {})
    [FAIL] git bash, full path, forward slashes / split-window -c : pane_current_path is 'C:\...\psmux672-start-i6726aec9e', expected 'C:\...\psmux672-target-i6726aec9e'
    [FAIL] git bash, full path, forward slashes / new-window -c : pane scrollback contains a syntax error -> bash: syntax error near unexpected token `('

    === RESULTS: 6 passed, 18 failed ===

Same result for the backslash spelling. The cold first pane is clean, the warm-served
split and new window are not, exactly as reported.

On master (before my fix) the same recipe is already clean: a live capture of the split pane shows

    $  cd 'C:/Users/godwin/AppData/Local/Temp/psmux-work-i67259f1'; clear

Cause of the difference: `rehome_command` in v3.3.8 had no shell classification at all, it
picked the snippet from `cfg!(windows)`:

    pub(crate) fn rehome_command(dir: &str) -> String {
        let escaped = dir.replace('\'', "''");
        let clear = if cfg!(windows) { "cls" } else { "clear" };
        if cfg!(windows) { format!(" cd '{}'; try {{ [System.IO.Directory]::SetCurrentDirectory(...) }} catch {{}}; {}\r", ...) }

`c3a8c64` (fix(#600), 2026-08-27) replaced that with `rehome_syntax_for_shell`. `git tag --contains c3a8c64`
is empty, so that fix is in no release. **#672 is #600 reported again against the last release.**

## 2. Residual bug found on master, and the causal chain

Probing every spelling with a unit matrix found a real one still on master:

    default-shell "C:/Program Files/Some Vendor/bin/bash.exe" -> PowerShell (expected Posix)
    ... the same for zsh, fish, sh and cmd

One sentence: `rehome_syntax_for_shell` asked `resolve_shell_program` for the program, and that
resolver identifies the program by looking it up on disk and on PATH and only falls back to a
quote-aware split when the lookup fails, so an unquoted `default-shell` path containing spaces
classifies correctly on a machine where the file is installed and wrongly on one where it is not
(the split turns `C:\Program Files\Git\bin\bash.exe` into the program `C:\Program`, whose stem names
no shell, so the pane gets the platform default, PowerShell).

Honest scope note: I could not build a user-visible E2E for this one, because a path psmux cannot
resolve is also a path it cannot spawn, so that pane has no working shell to receive the line either.
It is a latent defect: the dialect of a shell must not be a function of the filesystem, and it is
exactly the "match on the basename" rule the reporter asked for. It is proven at the unit level
(7 of 15 new tests failed before the fix, 0 after).

## 3. Fix

`src/pane.rs`:

* `program_stem` / `rehome_syntax_for_stem` split out, so the family decision is a pure function of a basename.
* when the resolved program names no known shell, the whole value is re-read as ONE path and its
  basename tried, but only when nothing in the value looks like an argument, so a real command line
  (`pwsh -File C:\x\bash.ps1`) is never read that way.
* `git-bash` folded into the POSIX family (the GUI launcher names bash even when
  `remap_git_bash_launcher` cannot find the console bash beside it).

Nothing else was changed. I checked the other shell classifiers: `is_posix_shell_path` and
`build_default_shell`'s `stem` checks are already basename based; `shell_needs_psrl_init` does
substring match the whole path, but it is only reached after `is_posix_shell_path` has already
claimed every POSIX shell, and a differential probe (a copy of cmd.exe under a directory named
`powershell-kit` vs `plain-kit`) produced byte identical panes, so it is not observably reachable.
Left alone rather than changed on speculation.

WSL: unchanged and now pinned. `C:\Windows\System32\bash.exe` classifies POSIX (its stem is `bash`,
and the shell behind it is a POSIX shell); `wsl.exe` keeps the platform default. Separately observed
and recorded in the E2E as a SKIP, not a pass: WSL bash gets the correct POSIX dialect but cannot
`cd` to a Windows path (it would need `/mnt/c/...`), so its own `pwd` still answers with the old
directory. Pre-existing, same class as #615, out of scope here.

## 4. Commits on this branch

    4ef48dc fix(#672): pick the rehome dialect from the shell's basename, never from the filesystem
    146ac11 test(#672): drive the reporter's Git Bash recipe through real warm panes, and say the basename rule in the docs
    c20c514 test(#672): poll the cold start instead of sleeping on it, and say what WSL bash can and cannot do
    3d19624 test(#672): plain punctuation in the nushell comment

## 5. Test files added

* `tests-rs/test_issue672_shell_basename_classify.rs`, registered in `src/pane.rs` next to the
  #600 module. 15 tests: bare name, full path forward and backslash, with and without `.exe`,
  mixed case, quoted, quoted with arguments, leading and trailing whitespace, for bash, zsh, fish,
  sh, dash, ksh, tcsh, csh, ash, busybox, cmd, pwsh, powershell and nushell, in a directory with
  spaces and one without; plus the reporter's exact config, the WSL pair, the unknown shell
  fallback, a directory merely named after another shell, and a spelling agreement test.
* `tests/test_issue672_gitbash_fullpath_warm_cd.ps1`. Isolated: throwaway `USERPROFILE`, its own
  `-L` namespace per case, teardown is `kill-server` on that namespace only, no by-name kills, no
  global kill-server.

## 6. Before and after

| | E2E (tests\test_issue672_gitbash_fullpath_warm_cd.ps1) | unit (cargo test --bin psmux issue672) |
|---|---|---|
| v3.3.8 66cf613 (reporter's build) | 6 passed, 18 failed | n/a |
| master 37e2990, before fix | not run as a suite (suite postdates it); matrix probe | 8 passed, 7 failed |
| this branch | 28 passed, 0 failed | 15 passed, 0 failed |

Fixed-side E2E quote:

    [PASS] git bash, full path, forward slashes / split-window -c : no syntax error in the pane scrollback
    [PASS] git bash, full path, forward slashes / split-window -c : no PowerShell rehome text in a POSIX pane
    [PASS] git bash, full path, forward slashes / split-window -c : pane_current_path is the requested directory
    [PASS] git bash, full path, forward slashes / split-window -c : the shell itself reports the requested directory (/tmp/psmux672-target-i6726d5cdd)
    === RESULTS: 28 passed, 0 failed ===

## 7. Regression suites run (with the worktree binary)

| suite | result |
|---|---|
| `cargo test --bin psmux issue672` | 15 passed, 0 failed |
| `cargo test --bin psmux issue600` | 14 passed, 0 failed |
| `cargo test --bin psmux warm` | 119 passed, 0 failed |
| `cargo test --bin psmux shell` | 194 passed, 0 failed |
| `cargo test --no-run` | whole crate compiles, all test binaries built |
| `tests\test_issue600_bash_rehome.ps1` | 11 passed, 0 failed |
| `tests\test_warm_pane_start_dir.ps1` (agent copy, by-name kill block removed) | 5 passed, 0 failed |
| `tests\test_issue659_warm_claim_environment.ps1` | 8 passed, 0 failed, 0 skipped |
| `tests\test_issue220_show_options_default_shell.ps1` | 8 passed, 0 failed |
| `tests\test_issue351_popup_default_shell.ps1` | 5 passed, 0 failed |
| `tests\test_issue615_wsl_pane_path.ps1` | first run 23 passed 3 failed (server died mid WSL section, "no server running"), rerun 26 passed 0 failed. Flake, not this change: the classifier is untouched when `default-shell` is empty, which is what that suite runs. |

Not run, deliberately: `tests\test_warm_pane.ps1`, `tests\test_warm_pane_sync_options.ps1`,
`tests\test_warm_off.ps1`, `tests\test_default_shell_cmd.ps1`, `tests\test_default_shell_wsl.ps1`,
`tests\test_issue99_default_shell_bash.ps1`. They issue a global `kill-server` in the default
namespace and some of them `Get-Process psmux | Stop-Process`, which would tear down other agents'
servers on this machine.

## 8. Docs

`docs/multi-shell.md`, "Start Directories and Warm Panes": states that the dialect is chosen from
the BASENAME of `default-shell` and lists the spellings that are equivalent; adds that
`git-bash.exe` counts as bash and that `C:\Windows\System32\bash.exe` is WSL's bash and gets the
bash row. The table itself was already accurate.

## 9. Build and cleanup

Built only with `cargo build --release` inside the worktree; `cargo install` was never run and
`~\.cargo\bin` was never touched (other agents are building on this machine). Every session ran
under an `i672*` namespace of its own and was torn down with `kill-server` on that namespace.

Leftovers verified zero: `Get-CimInstance Win32_Process -Filter "Name='psmux.exe'"` shows 4 psmux
processes on the machine and 0 of them belong to this work (no command line matching my namespaces
or temp homes); no `i672`/`psmux672` temp directories remain; no `i672` entries in the default
`~/.psmux` registry; the scratch `v3.3.8` git worktree has been removed. No windows were opened
(every session was detached) and no process was killed by name.
