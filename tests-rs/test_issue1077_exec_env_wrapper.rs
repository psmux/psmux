// Issue #1077: Claude Code 2.1.329 starts teammate panes with
//   /bin/sh -c "cd '<dir>' && exec env CLAUDECODE=1 ... '<claude.exe>' --agent-id ..."
// (through split-window, and through `respawn-pane -k -- /bin/sh -c <cmd>`).
// psmux unwrapped the sh wrapper and handed `cd ... && exec env ...` to pwsh,
// which has no `exec` (and no `env`), so the pane died at once. Under tmux sh
// runs the line and `exec` replaces sh with the program; psmux now reads the
// line apart and starts the program itself.

use super::*;

#[cfg(windows)]
fn s(v: &str) -> String { v.to_string() }

#[cfg(windows)]
fn argv_of(b: &CommandBuilder) -> Vec<String> {
    b.get_argv().iter().map(|a| a.to_string_lossy().to_string()).collect()
}

#[cfg(windows)]
fn env_of(b: &CommandBuilder, key: &str) -> Option<String> {
    b.get_env(key).map(|v| v.to_string_lossy().to_string())
}

#[cfg(windows)]
fn cmd_exe() -> String {
    std::env::var("ComSpec").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".to_string())
}

#[cfg(windows)]
#[test]
fn wrapped_exec_env_line_is_read_apart() {
    let cmd = "/bin/sh -c \"cd 'C:\\Windows' && exec env CLAUDECODE=1 CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1 'C:\\Users\\me\\.local\\bin\\claude.exe' --agent-id Bob@team --team-name t\"";
    let line = parse_launch_line(cmd).expect("the 2.1.329 teammate line must be recognised");
    assert_eq!(line.cwd.as_deref(), Some("C:\\Windows"));
    assert_eq!(line.env_sets, vec![(s("CLAUDECODE"), s("1")), (s("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"), s("1"))]);
    assert_eq!(line.argv, vec![s("C:\\Users\\me\\.local\\bin\\claude.exe"), s("--agent-id"), s("Bob@team"), s("--team-name"), s("t")]);
    assert!(line.replaces_shell, "a wrapped exec line replaces the shell");
    assert!(!line.argv.iter().any(|w| w == "exec" || w == "env"), "neither exec nor env may reach the program position");
}

#[cfg(windows)]
#[test]
fn respawn_argv_form_is_read_apart() {
    // respawn-pane -k -t %1 -- /bin/sh -c "<cmd>": the argv form keeps each
    // word as the CLI sent it.
    let encoded = format!("-- {}", ["/bin/sh", "-c", "cd 'C:\\Windows' && exec env A=1 cmd /c echo OK %A%"]
        .iter().map(|w| crate::pane::quote_start_command_word(w)).collect::<Vec<_>>().join(" "));
    let line = parse_launch_line(&encoded).expect("the argv form must be recognised");
    assert_eq!(line.cwd.as_deref(), Some("C:\\Windows"));
    assert_eq!(line.env_sets, vec![(s("A"), s("1"))]);
    assert_eq!(line.argv, vec![s("cmd"), s("/c"), s("echo"), s("OK"), s("%A%")]);
    assert!(line.replaces_shell);
}

#[cfg(windows)]
#[test]
fn exec_without_env_and_unwrapped_exec_are_read() {
    let line = parse_launch_line("cd 'C:\\Windows' && exec cmd /k echo hi").expect("unwrapped exec line");
    assert_eq!(line.cwd.as_deref(), Some("C:\\Windows"));
    assert!(line.env_sets.is_empty());
    assert_eq!(line.argv[0], "cmd");
    assert!(line.replaces_shell);

    let line = parse_launch_line("/bin/sh -c \"exec 'C:\\Program Files\\x\\a b.exe' --flag 'two words'\"").expect("wrapped exec without env");
    assert_eq!(line.argv, vec![s("C:\\Program Files\\x\\a b.exe"), s("--flag"), s("two words")]);
}

#[cfg(windows)]
#[test]
fn quoted_values_and_paths_with_spaces_survive() {
    let line = parse_launch_line("/bin/sh -c \"cd 'C:\\Program Files' && exec env XDIR='D:\\POC Code\\todo' K=v 'C:\\Program Files\\a.exe' 'arg with space'\"")
        .expect("must parse");
    assert_eq!(line.cwd.as_deref(), Some("C:\\Program Files"));
    assert_eq!(line.env_sets, vec![(s("XDIR"), s("D:\\POC Code\\todo")), (s("K"), s("v"))]);
    assert_eq!(line.argv, vec![s("C:\\Program Files\\a.exe"), s("arg with space")]);
}

#[cfg(windows)]
#[test]
fn export_and_unset_steps_combine_with_exec_env() {
    let line = parse_launch_line("/bin/bash -c 'unset OLD && export P=1 && cd C:\\x && exec env Q=2 prog.exe'")
        .expect("must parse");
    assert_eq!(line.env_removes, vec![s("OLD")]);
    assert_eq!(line.env_sets, vec![(s("P"), s("1")), (s("Q"), s("2"))], "export first, then env, the order sh applies them");
    assert_eq!(line.cwd.as_deref(), Some("C:\\x"));
    assert_eq!(line.argv, vec![s("prog.exe")]);
}

#[cfg(windows)]
#[test]
fn lines_that_are_not_launch_lines_are_left_alone() {
    // Overstory style wrapper keeps the older wrapper handling.
    assert!(parse_launch_line("/bin/bash -c 'unset X && export Y=1 && claude --foo'").is_none());
    // Plain commands and unwrapped `cd && prog` stay with the pane shell.
    assert!(parse_launch_line("cd C:\\x && cmd /c echo hi").is_none());
    assert!(parse_launch_line("pwsh -NoLogo").is_none());
    assert!(parse_launch_line("exec").is_none());
    assert!(parse_launch_line("exec env A=1").is_none(), "nothing to run");
    assert!(parse_launch_line("execute.exe --x").is_none(), "`execute` is not `exec`");
    assert!(parse_launch_line("-- cmd.exe /k echo").is_none());
}

#[cfg(windows)]
#[test]
fn exec_line_spawns_the_program_directly_without_a_shell() {
    // The issue's own repro line: no pwsh between the pane and cmd.exe, so
    // no `exec`/`env` text can reach a shell, and A=1 is in the environment.
    let b = build_command(Some("/bin/sh -c \"cd 'C:\\Windows' && exec env A=1 cmd /c echo OK-NEW %A%\""), true, false);
    let argv = argv_of(&b);
    let prog = argv[0].to_ascii_lowercase();
    assert!(prog.ends_with("cmd") || prog.ends_with("cmd.exe"), "program must be cmd itself, got {argv:?}");
    assert!(!argv.iter().any(|a| a.contains("exec") || a.contains("env A=1")), "no shell text left: {argv:?}");
    assert_eq!(&argv[1..], &[s("/c"), s("echo"), s("OK-NEW"), s("%A%")]);
    assert_eq!(env_of(&b, "A").as_deref(), Some("1"));
    assert_eq!(b.get_cwd().map(|c| c.to_string_lossy().to_string()).as_deref(), Some("C:\\Windows"));
}

#[cfg(windows)]
#[test]
fn exec_line_with_full_program_path_spawns_it_directly() {
    let exe = cmd_exe();
    let line = format!("/bin/sh -c \"cd 'C:\\Windows' && exec env CLAUDECODE=1 '{}' /k echo x\"", exe);
    let b = build_command(Some(&line), true, false);
    let argv = argv_of(&b);
    assert_eq!(argv[0].to_ascii_lowercase(), exe.to_ascii_lowercase());
    assert_eq!(env_of(&b, "CLAUDECODE").as_deref(), Some("1"));
}

#[cfg(windows)]
#[test]
fn non_spawnable_program_goes_through_the_shell_without_exec_text() {
    // A name nothing on PATH resolves (a PowerShell function, say) needs the
    // pane shell; the text it gets must still be free of `exec` and `env`.
    let b = build_command(Some("/bin/sh -c \"exec env A=1 Some-PsmuxFunction1077 -X\""), true, false);
    let argv = argv_of(&b);
    let last = argv.last().cloned().unwrap_or_default();
    assert!(last.contains("Some-PsmuxFunction1077 -X"), "got {argv:?}");
    assert!(!last.contains("exec") && !last.contains("env "), "got {argv:?}");
    assert_eq!(env_of(&b, "A").as_deref(), Some("1"));
}

#[cfg(windows)]
#[test]
fn shell_rendering_matches_each_shell() {
    let argv = vec![s("C:\\Program Files\\a.exe"), s("--x"), s("two words")];
    assert_eq!(render_launch_for_shell("C:\\pwsh\\pwsh.exe", &argv), "& 'C:\\Program Files\\a.exe' --x 'two words'");
    assert_eq!(render_launch_for_shell("powershell.exe", &argv), "& 'C:\\Program Files\\a.exe' --x 'two words'");
    assert_eq!(render_launch_for_shell("C:\\Windows\\System32\\cmd.exe", &argv), "\"C:\\Program Files\\a.exe\" --x \"two words\"");
    assert_eq!(render_launch_for_shell("C:\\Git\\bin\\bash.exe", &argv), "'C:\\Program Files\\a.exe' --x 'two words'");
}

#[cfg(windows)]
#[test]
fn launch_cd_wins_over_the_spawn_site_directory() {
    // split-window sends the client's directory as the start directory; under
    // tmux the line's own `cd` runs after it inside sh, so it wins.
    let mut b = CommandBuilder::new("cmd.exe");
    b.cwd("C:\\Users");
    b.env("A", "session");
    apply_launch_overrides(&mut b, Some("/bin/sh -c \"cd 'C:\\Windows' && exec env A=1 cmd /k\""));
    assert_eq!(b.get_cwd().map(|c| c.to_string_lossy().to_string()).as_deref(), Some("C:\\Windows"));
    assert_eq!(env_of(&b, "A").as_deref(), Some("1"));

    // A `cd` into a directory that does not exist leaves the start directory.
    let mut b = CommandBuilder::new("cmd.exe");
    b.cwd("C:\\Users");
    apply_launch_overrides(&mut b, Some("cd 'C:\\no\\such\\dir1077' && exec cmd /k"));
    assert_eq!(b.get_cwd().map(|c| c.to_string_lossy().to_string()).as_deref(), Some("C:\\Users"));

    // Not a launch line: nothing changes.
    let mut b = CommandBuilder::new("cmd.exe");
    b.cwd("C:\\Users");
    apply_launch_overrides(&mut b, Some("cmd /k"));
    assert_eq!(b.get_cwd().map(|c| c.to_string_lossy().to_string()).as_deref(), Some("C:\\Users"));
}

#[cfg(windows)]
#[test]
fn shell_syntax_after_exec_keeps_the_shell_route() {
    let line = parse_launch_line("/bin/sh -c \"exec env A=1 prog.exe | tee log\"").expect("parses");
    assert!(!launch_argv_is_plain(&line.argv), "a pipe needs a shell");
    let line = parse_launch_line("/bin/sh -c \"exec env A=1 prog.exe --home $HOME\"").expect("parses");
    assert!(!launch_argv_is_plain(&line.argv), "a variable needs a shell");
}
