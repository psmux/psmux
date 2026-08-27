// Issue #600: "Failure to properly set current directory".
//
// With `default-shell` pointing at Git Bash, every warm-pane consume that
// carried a start directory typed the PowerShell rehome snippet into bash:
//
//     $  cd 'C:\Users\UserName1'; try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; cls
//     bash: syntax error near unexpected token `('
//
// The snippet was chosen by `cfg!(windows)` (the host OS) instead of by the
// shell actually running in the pane, so bash got PowerShell, cmd.exe got
// PowerShell, and in both cases the pane also stayed in the wrong directory
// because the `cd` never executed.
//
// These tests pin the dialect selection and the exact wire form per shell
// family. The end-to-end behaviour (no stray text on screen, `pwd` inside the
// pane equal to the requested dir, for new-window -c / split-window -c / the
// warm-server claim) is covered by tests/test_issue600_bash_rehome.ps1.

use super::*;

// ─────────────────────────── dialect selection ───────────────────────────

/// The reporter's configuration: `default-shell` is a Git Bash path. That must
/// select the POSIX dialect, never PowerShell.
#[test]
fn git_bash_default_shell_selects_posix() {
    for shell in [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
        "bash",
        "/usr/bin/bash",
    ] {
        assert_eq!(
            rehome_syntax_for_shell(shell),
            RehomeSyntax::Posix,
            "{shell} must be treated as a POSIX shell (#600)"
        );
    }
}

/// The other POSIX families psmux already knows about (POSIX_SHELL_STEMS) plus
/// the busybox/ash pair must all get the POSIX form too.
#[test]
fn other_posix_shells_select_posix() {
    for shell in ["sh", "zsh", "fish", "dash", "ksh", "tcsh", "csh", "ash", "busybox"] {
        assert_eq!(
            rehome_syntax_for_shell(shell),
            RehomeSyntax::Posix,
            "{shell} must select the POSIX rehome form"
        );
    }
}

/// cmd.exe understands neither PowerShell's `;` chaining nor `try {}`; it must
/// get its own dialect. Before the fix it was handed the PowerShell snippet and
/// answered "The system cannot find the path specified."
#[test]
fn cmd_default_shell_selects_cmd() {
    for shell in [r"C:\Windows\System32\cmd.exe", "cmd", "cmd.exe"] {
        assert_eq!(
            rehome_syntax_for_shell(shell),
            RehomeSyntax::Cmd,
            "{shell} must select the cmd rehome form (#600)"
        );
    }
}

/// Regression guard for the shell the rehome was originally written for: the
/// PowerShell behaviour must be unchanged, both when named explicitly and when
/// `default-shell` is empty on Windows (psmux spawns pwsh/powershell itself).
#[test]
fn powershell_and_empty_default_shell_select_powershell() {
    for shell in ["pwsh", "powershell", r"C:\Program Files\PowerShell\7\pwsh.exe"] {
        assert_eq!(
            rehome_syntax_for_shell(shell),
            RehomeSyntax::PowerShell,
            "{shell} must keep the PowerShell rehome form"
        );
    }
    let expected = if cfg!(windows) { RehomeSyntax::PowerShell } else { RehomeSyntax::Posix };
    assert_eq!(rehome_syntax_for_shell(""), expected, "empty default-shell means the platform default");
    assert_eq!(rehome_syntax_for_shell("   "), expected, "blank default-shell means the platform default");
}

/// `default-shell` may carry arguments; the dialect must come from the program,
/// not from the whole string. Quoted paths with spaces must survive too.
#[test]
fn default_shell_with_arguments_resolves_the_program() {
    assert_eq!(rehome_syntax_for_shell("bash -l"), RehomeSyntax::Posix);
    assert_eq!(
        rehome_syntax_for_shell("\"C:\\Program Files\\Git\\bin\\bash.exe\" -i"),
        RehomeSyntax::Posix,
        "a quoted Git Bash path with an argument must still resolve to bash"
    );
    assert_eq!(rehome_syntax_for_shell("pwsh -NoProfile"), RehomeSyntax::PowerShell);
    assert_eq!(rehome_syntax_for_shell("cmd.exe /k"), RehomeSyntax::Cmd);
}

/// `git-bash.exe` is the GUI launcher psmux already remaps to the real console
/// bash before spawning (see remap_git_bash_launcher); the dialect must follow
/// the remap, not the launcher name.
#[test]
fn git_bash_launcher_follows_the_remap() {
    let launcher = r"C:\Program Files\Git\git-bash.exe";
    if std::path::Path::new(r"C:\Program Files\Git\bin\bash.exe").is_file() {
        assert_eq!(
            rehome_syntax_for_shell(launcher),
            RehomeSyntax::Posix,
            "git-bash.exe remaps to bash.exe, so it must select the POSIX form"
        );
    }
}

/// An unrecognised program keeps the platform default rather than guessing a
/// dialect that could spray text into the pane.
#[test]
fn unknown_shell_keeps_the_platform_default() {
    let expected = if cfg!(windows) { RehomeSyntax::PowerShell } else { RehomeSyntax::Posix };
    assert_eq!(rehome_syntax_for_shell("some-unheard-of-shell-9f3a"), expected);
}

// ────────────────────────────── wire forms ───────────────────────────────

/// Exact POSIX wire form. This is the line the reporter should have received.
/// It must contain no PowerShell tokens at all: `(`, `)`, `{`, `}` and `try`
/// are exactly what bash choked on.
#[test]
fn posix_rehome_exact_form_has_no_powershell_tokens() {
    let cmd = rehome_command(r"C:\code\project", RehomeSyntax::Posix);
    if cfg!(windows) {
        assert_eq!(cmd, " cd 'C:/code/project'; clear\r");
    }
    assert!(
        !cmd.contains("SetCurrentDirectory"),
        "the .NET sync is PowerShell-only and is a bash syntax error, got {cmd:?}"
    );
    for tok in ['(', ')', '{', '}'] {
        assert!(!cmd.contains(tok), "POSIX form must not contain {tok:?}, got {cmd:?}");
    }
    assert!(cmd.starts_with(' '), "must start with a space, got {cmd:?}");
    assert_eq!(cmd.matches('\r').count(), 1, "exactly one submitted line, got {cmd:?}");
    assert!(cmd.ends_with("; clear\r"), "must chain a clear to hide the echo, got {cmd:?}");
}

/// A Windows path reaching Git Bash / MSYS2 / Cygwin is normalised to forward
/// slashes, which those shells accept and which leaves no backslash for the
/// reader to treat as an escape. Off Windows a backslash is an ordinary
/// filename character and must survive untouched.
#[test]
fn posix_rehome_normalises_windows_separators() {
    let cmd = rehome_command(r"C:\Users\UserName1\My Code", RehomeSyntax::Posix);
    if cfg!(windows) {
        assert!(cmd.contains("cd 'C:/Users/UserName1/My Code'"), "got {cmd:?}");
        assert!(!cmd.contains('\\'), "no backslash may survive on Windows, got {cmd:?}");
    } else {
        assert!(cmd.contains(r"cd 'C:\Users\UserName1\My Code'"), "got {cmd:?}");
    }
}

/// POSIX single-quote escaping uses the `'\''` idiom: a single-quoted POSIX
/// string cannot contain an escaped quote, so it has to be closed, an escaped
/// quote emitted, and the string reopened. Doubling (the PowerShell rule) would
/// silently split the path instead.
#[test]
fn posix_rehome_escapes_single_quotes_the_posix_way() {
    let input = "/tmp/weird'dir";
    assert_eq!(input.matches('\'').count(), 1, "precondition: one lone quote");
    let cmd = rehome_command(input, RehomeSyntax::Posix);
    assert!(
        cmd.contains(r"cd '/tmp/weird'\''dir'"),
        "must use the POSIX '\\'' idiom, not PowerShell doubling, got {cmd:?}"
    );
    // The PowerShell rule would have produced `weird''dir` (a doubled quote
    // with no backslash), which POSIX reads as an empty string concatenation
    // and which would silently drop the quote from the path.
    assert!(
        !cmd.contains(r"weird''dir"),
        "PowerShell-style doubling must not leak in, got {cmd:?}"
    );
    assert_eq!(cmd.matches('\r').count(), 1, "exactly one submitted line, got {cmd:?}");
}

/// Exact cmd.exe wire form: `cd /d` so a drive change is honoured, `&` because
/// cmd does not chain on `;`, double quotes because a Windows path cannot
/// contain one, and `cls` to hide the echo.
#[test]
fn cmd_rehome_exact_form() {
    assert_eq!(
        rehome_command(r"C:\code\project", RehomeSyntax::Cmd),
        " cd /d \"C:\\code\\project\" & cls\r"
    );
    let cmd = rehome_command(r"D:\other dir", RehomeSyntax::Cmd);
    assert!(cmd.contains("cd /d "), "must use /d so a drive change works, got {cmd:?}");
    assert!(!cmd.contains(';'), "cmd does not chain on ';', got {cmd:?}");
    assert!(!cmd.contains("SetCurrentDirectory"), "PowerShell-only, got {cmd:?}");
    assert_eq!(cmd.matches('\r').count(), 1, "exactly one submitted line, got {cmd:?}");
}

/// The PowerShell form is untouched by the fix, including the Win32 CWD sync
/// that `#{pane_current_path}`'s PEB walk depends on.
#[test]
fn powershell_rehome_form_is_unchanged() {
    assert_eq!(
        rehome_command(r"C:\x", RehomeSyntax::PowerShell),
        " cd 'C:\\x'; try { [System.IO.Directory]::SetCurrentDirectory($PWD.ProviderPath) } catch {}; cls\r"
    );
}

/// Every dialect must produce exactly one submitted line and start with the
/// history-skipping space, whatever the path contains.
#[test]
fn all_dialects_submit_exactly_one_line() {
    for syntax in [RehomeSyntax::PowerShell, RehomeSyntax::Posix, RehomeSyntax::Cmd] {
        for dir in [r"C:\a", r"C:\a b\c", "/tmp/x", r"C:\it's"] {
            let cmd = rehome_command(dir, syntax);
            assert!(cmd.starts_with(' '), "{syntax:?} on {dir:?}: {cmd:?}");
            assert!(cmd.ends_with('\r'), "{syntax:?} on {dir:?}: {cmd:?}");
            assert_eq!(cmd.matches('\r').count(), 1, "{syntax:?} on {dir:?}: {cmd:?}");
            assert!(!cmd.contains('\n'), "{syntax:?} on {dir:?}: {cmd:?}");
        }
    }
}

/// End-to-end within the process: consuming a warm pane for `new-window -c`
/// while `default-shell` is Git Bash must inject the POSIX line, not the
/// PowerShell one. This is the exact composition that produced the reported
/// bash syntax error.
#[test]
fn warm_consume_with_bash_default_shell_picks_posix() {
    let bash = [
        r"C:\Program Files\Git\bin\bash.exe",
        r"C:\Program Files\Git\usr\bin\bash.exe",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).is_file());
    let Some(bash) = bash else {
        // No Git Bash on this machine; the pure-function tests above still
        // cover the selection rule.
        return;
    };
    let syntax = rehome_syntax_for_shell(bash);
    assert_eq!(syntax, RehomeSyntax::Posix);
    let injected = rehome_command(r"C:\Users\UserName1", syntax);
    assert!(
        !injected.contains("syntax") && !injected.contains('('),
        "the line typed into bash must be valid bash, got {injected:?}"
    );
    assert!(injected.contains("cd 'C:/Users/UserName1'"), "got {injected:?}");
}
