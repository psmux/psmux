// Issue #734 follow up: the shell command of a plain `if-shell` (no -F) went to
// the shell VERBATIM, so a guard such as
//
//     if-shell "check #{pid}" "..." "..."
//
// saw the literal text `#{pid}`, while `if-shell -F` and run-shell expanded
// theirs. tmux format expands the shell command first, always
// (cmd-if-shell.c:86):
//
//     shellcmd = format_single_from_target(item, args_string(args, 0));
//
// and that expansion is format_expand, NOT format_expand_time, so a `%` in a
// shell command is never strftime (run-shell turned `50%d` into `5009`).
//
// These tests choose conditions whose EXPANSION is one of psmux's literal
// shortcuts ("true", "false", "1", "0"), so they decide the branch without
// spawning any shell when the expansion happens. Before the fix the literal
// `#{...}` text reached pwsh, where a leading `#` is a comment, the shell
// exited 0, and the TRUE branch ran: every "expands to false" case below took
// the wrong branch.

use super::*;

fn mock_app() -> AppState {
    let mut app = AppState::new("t734".to_string());
    app.control_port = None;
    app
}

fn opt(app: &AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

#[test]
fn untimed_expansion_keeps_percent_and_expands_formats() {
    let app = mock_app();
    let out = crate::format::expand_format_untimed("50%d %Y %VAR% #{session_name}", &app);
    assert_eq!(out, "50%d %Y %VAR% t734");
}

#[test]
fn timed_expansion_still_runs_strftime_for_status_and_display() {
    // The status line and display-message keep their strftime pass.
    let app = mock_app();
    let out = crate::format::expand_format("%Y", &app);
    assert!(out.chars().all(|c| c.is_ascii_digit()) && out.len() == 4, "got {out:?}");
}

#[test]
fn untimed_expansion_handles_double_hash_and_comments_like_tmux() {
    let app = mock_app();
    // `##` is a literal `#`; `# ` (hash space) and `#>` are left alone, so a
    // PowerShell comment or block comment end survives.
    assert_eq!(crate::format::expand_format_untimed("a ## b", &app), "a # b");
    assert_eq!(crate::format::expand_format_untimed("x # note", &app), "x # note");
    assert_eq!(crate::format::expand_format_untimed("<# c #>", &app), "<# c #>");
    // `#S` is the session name, as in tmux's format_upper table.
    assert_eq!(crate::format::expand_format_untimed("#S", &app), "t734");
}

#[test]
fn untimed_flag_is_restored_after_the_call() {
    let app = mock_app();
    let _ = crate::format::expand_format_untimed("#{session_name}", &app);
    let out = crate::format::expand_format("%Y", &app);
    assert_ne!(out, "%Y", "the strftime pass must come back after an untimed expansion");
}

#[test]
fn config_if_shell_expands_condition_before_running_it() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "if-shell \"#{?#{session_name},false,true}\" \"set -g @c734 yes\" \"set -g @c734 no\"\n",
    );
    assert_eq!(opt(&app, "@c734").as_deref(), Some("no"),
        "the condition must expand to `false` before it is evaluated");
}

#[test]
fn config_if_shell_true_expansion_takes_true_branch() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "if-shell \"#{?#{session_name},1,0}\" \"set -g @d734 yes\" \"set -g @d734 no\"\n",
    );
    assert_eq!(opt(&app, "@d734").as_deref(), Some("yes"));
}

#[test]
fn config_brace_if_shell_expands_condition() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "if-shell '#{?#{session_name},false,true}' {\n  set -g @e734 yes\n}\n{\n  set -g @e734 no\n}\n",
    );
    assert_eq!(opt(&app, "@e734").as_deref(), Some("no"));
}

#[test]
fn config_if_shell_f_path_is_unchanged() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "if-shell -F \"#{session_name}\" \"set -g @f734 yes\" \"set -g @f734 no\"\n",
    );
    assert_eq!(opt(&app, "@f734").as_deref(), Some("yes"));
}

#[test]
fn command_prompt_if_shell_expands_condition() {
    // The in process path (command prompt, key binding with no server port).
    let mut app = mock_app();
    let _ = crate::commands::execute_command_string(
        &mut app,
        "if-shell \"#{?#{session_name},false,true}\" \"set -g @g734 yes\" \"set -g @g734 no\"",
    );
    assert_eq!(opt(&app, "@g734").as_deref(), Some("no"));
}

// CLI `bind-key Y if-shell 'cond' 'set -g @k yes' 'set -g @k no'` used to go
// over the wire as one space joined line, so the server stored
// `if-shell cond set -g @k yes set -g @k no` and the key did nothing. tmux keeps
// each argument; a lone command argument is a command string and stays raw.
#[test]
fn bind_key_wire_line_keeps_each_bound_argument() {
    let argv = ["bind-key", "Y", "if-shell", "#{==:#{pane_id},x}", "set -g @k yes", "set -g @k no"];
    assert_eq!(
        crate::util::bind_key_wire_line(&argv),
        "bind-key Y if-shell #{==:#{pane_id},x} \"set -g @k yes\" \"set -g @k no\""
    );
}

#[test]
fn bind_key_wire_line_leaves_a_single_command_string_raw() {
    let argv = ["bind", "-T", "prefix", "x", "split-window -h"];
    assert_eq!(crate::util::bind_key_wire_line(&argv), "bind -T prefix x split-window -h");
    let argv = ["bind", "-n", "-r", "M-x", "display-message 'a b'"];
    assert_eq!(crate::util::bind_key_wire_line(&argv), "bind -n -r M-x display-message 'a b'");
}

#[test]
fn bind_key_wire_line_round_trips_through_the_server_parser() {
    let argv = ["bind-key", "-N", "my note", "Y", "if-shell", "exit 0", "set -g @k yes"];
    let line = crate::util::bind_key_wire_line(&argv);
    let parsed = crate::commands::parse_command_line(&line);
    assert_eq!(parsed, argv.iter().map(|s| s.to_string()).collect::<Vec<_>>());
}

