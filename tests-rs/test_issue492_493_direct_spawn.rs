// Issues #492 / #493: window/pane commands must spawn plain program
// invocations DIRECTLY (tmux spawn.c execvp parity) instead of wrapping
// everything in `<shell> -Command "<cmd>"`, which left a wrapper powershell
// process around every command pane (#493) and could not parse quoted
// executable paths containing spaces (#492).
#![cfg(windows)]
use super::*;

fn windir() -> String {
    std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_string())
}

#[test]
fn tokens_split_on_whitespace_and_quotes() {
    let t = split_spawn_tokens(r#""C:/Program Files/Git/bin/bash.exe" --login -i"#);
    assert_eq!(t, vec!["C:/Program Files/Git/bin/bash.exe", "--login", "-i"]);
    let t = split_spawn_tokens("cmd.exe /k echo hi");
    assert_eq!(t, vec!["cmd.exe", "/k", "echo", "hi"]);
    let t = split_spawn_tokens("'a b' c");
    assert_eq!(t, vec!["a b", "c"]);
}

#[test]
fn absolute_exe_path_spawns_directly() {
    let cmd = format!("{}\\System32\\cmd.exe", windir());
    let (prog, args) = try_direct_spawn(&cmd).expect("absolute exe path must direct-spawn");
    assert!(prog.to_lowercase().ends_with("cmd.exe"));
    assert!(args.is_empty());
}

#[test]
fn path_with_spaces_spawns_directly() {
    // Create a dummy exe inside a directory whose name contains a space.
    let dir = std::env::temp_dir().join("psmux 492 spawn test");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("space tool.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let raw = exe.to_string_lossy().into_owned();

    // Whole unquoted string (as it arrives from bind-key after quote
    // consumption, the reporter's exact case).
    let (prog, args) = try_direct_spawn(&raw).expect("space path must direct-spawn");
    assert_eq!(prog, raw);
    assert!(args.is_empty());

    // Longest-prefix resolution: unquoted space path followed by arguments.
    let with_args = format!("{} --login -i", raw);
    let (prog, args) = try_direct_spawn(&with_args).expect("space path + args must direct-spawn");
    assert_eq!(prog, raw);
    assert_eq!(args, vec!["--login", "-i"]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bare_program_name_with_args_spawns_directly() {
    // tmux execvp's a multi-argument shell-command and only routes a single
    // one through the shell (spawn.c), so a bare name WITH arguments belongs
    // on the direct path. psmux used to wrap those in `<shell> -Command`,
    // which on Windows is a whole second pwsh start: `new-session pwsh
    // -NoLogo -NoProfile -NoExit -File x.ps1` measured 278ms slower to its
    // first prompt than the same command exec'd directly, and the wrapper
    // (which has no -NoProfile of its own) also sourced the user's profile
    // that the inner -NoProfile had asked to skip.
    //
    // The carve-out this test used to pin, namely console utilities supposedly
    // needing the shell to re-establish console stdin, was re-measured and
    // does not reproduce: `timeout /t 3`, `ping -n 2 127.0.0.1` and
    // `nvim <file>` all run correctly spawned straight into a pane's ConPTY.
    let (prog, args) =
        try_direct_spawn("cmd.exe /c exit").expect("bare exe + args must direct-spawn");
    assert!(prog.to_lowercase().ends_with("cmd.exe"), "resolved to {prog}");
    assert!(prog.contains('\\'), "must resolve to a full path, got {prog}");
    assert_eq!(args, vec!["/c", "exit"]);

    let (prog, args) =
        try_direct_spawn("timeout /T 120 /nobreak").expect("timeout + args must direct-spawn");
    assert!(prog.to_lowercase().ends_with("timeout.exe"), "resolved to {prog}");
    assert_eq!(args, vec!["/T", "120", "/nobreak"]);

    let (prog, args) =
        try_direct_spawn("ping -t 127.0.0.1").expect("ping + args must direct-spawn");
    assert!(prog.to_lowercase().ends_with("ping.exe"), "resolved to {prog}");
    assert_eq!(args, vec!["-t", "127.0.0.1"]);
}

#[test]
fn lone_bare_program_name_keeps_the_shell() {
    // A single-word command is tmux's shell case, and on Windows it is also
    // the only thing that keeps `ls`, `cat`, `sort` and `where` resolving as
    // the PowerShell aliases users mean by them.
    assert!(try_direct_spawn("cmd.exe").is_none());
    assert!(try_direct_spawn("timeout").is_none());
}

#[test]
fn bare_name_resolving_to_a_script_keeps_the_shell() {
    // `npm` is an extensionless Node script and `<x>.cmd`/`.bat`/`.ps1` need
    // their interpreter; CreateProcessW refuses all of them with "%1 is not a
    // valid Win32 application" (measured: `new-session -- npm --version` dies
    // with os error 193), which would fail the whole pane spawn. Only
    // .exe/.com may take the direct path.
    let _lock = crate::util::lock_test_env();
    let dir = std::env::temp_dir().join("psmux 492 script fixture");
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = "psmux492fixturelauncher";
    std::fs::write(dir.join(format!("{fixture}.cmd")), b"@rem fixture").unwrap();
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{};{}", dir.display(), old));
    let got = try_direct_spawn(&format!("{fixture} --version"));
    std::env::set_var("PATH", &old);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        got.is_none(),
        "a .cmd launcher must keep the shell route, got {got:?}"
    );
}

#[test]
fn bare_shell_with_flags_is_not_wrapped_in_another_shell() {
    // End-to-end guard on the launch-to-prompt path: the pane command must be
    // the program itself, never `<shell> -Command "<the whole command>"`.
    let builder = build_command(Some("cmd.exe /c exit"), false, false);
    let argv = format!("{:?}", builder.get_argv()).to_lowercase();
    assert!(
        !argv.contains("-command"),
        "no shell wrapper expected for a bare exe + args, got: {argv}"
    );
    assert!(argv.contains("cmd.exe"), "got: {argv}");
}

#[test]
fn shell_syntax_falls_back_to_shell() {
    // Pipes, chains, redirects and variables need a real shell.
    assert!(try_direct_spawn("echo hi && cmd /k").is_none());
    assert!(try_direct_spawn("dir | more").is_none());
    assert!(try_direct_spawn("cmd.exe > out.txt").is_none());
    assert!(try_direct_spawn("echo $env:PATH").is_none());
    assert!(try_direct_spawn("echo %PATH%").is_none());
    // A first token with no on-disk executable anywhere falls back to the
    // shell.  (`echo` itself is machine-dependent: Git's usr\bin ships an
    // echo.exe, and resolving it via PATH matches tmux's execvp behavior.)
    assert!(try_direct_spawn("definitely-not-a-real-cmd-492 hello").is_none());
    // The #399 env-prefix path emits a pwsh call-operator string.
    assert!(try_direct_spawn("& C:/tools/thing.exe run").is_none());
}

#[test]
fn program_files_x86_style_path_spawns_directly() {
    // Parentheses in the path must not trip the shell-metachar bail when the
    // whole string is an existing file.
    let dir = std::env::temp_dir().join("psmux 492 (x86) test");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("tool.exe");
    std::fs::write(&exe, b"MZ").unwrap();
    let raw = exe.to_string_lossy().into_owned();
    let (prog, args) = try_direct_spawn(&raw).expect("(x86)-style path must direct-spawn");
    assert_eq!(prog, raw);
    assert!(args.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn build_command_direct_spawn_has_no_shell_wrapper() {
    // End-to-end through build_command: the resulting CommandBuilder must
    // target the exe itself, not pwsh/powershell/cmd wrapping it.
    let cmd = format!("{}\\System32\\cmd.exe", windir());
    let builder = build_command(Some(&cmd), false, false);
    // The arguments the pane will be spawned with. The builder's Debug text
    // also carries its environment and working directory, so a branch or a
    // checkout path with "command" in its name (GITHUB_HEAD_REF in CI) read
    // as a `-Command` wrapper there.
    let argv: Vec<String> = builder
        .get_argv()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert!(
        !argv.iter().any(|a| a.eq_ignore_ascii_case("-command")),
        "no `-Command` wrapper expected for a plain exe, got: {:?}",
        argv
    );
    assert_eq!(argv.first().map(String::as_str), Some(cmd.as_str()), "the exe itself is spawned: {:?}", argv);
}
