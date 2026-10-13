// Issue #793 follow up: the new-pane, if-shell and run-shell parsers the
// one shot and control mode routes share. Registered from
// src/server/connection.rs.

use super::*;

#[test]
fn new_pane_format_flag_is_not_the_command() {
    // -F's value used to fall into the command, so the float ran
    // `pwsh -Command "-F #{pane_id}"`, exited, and vanished.
    let p = parse_new_pane_args(&["-d", "-P", "-F", "#{pane_id}"]);
    assert_eq!(p.command, "");
    assert_eq!(p.format.as_deref(), Some("#{pane_id}"));
    assert!(p.detached && p.print);
    // The value glued to the flag, getopt style.
    let p = parse_new_pane_args(&["-PF#{pane_id}"]);
    assert_eq!((p.print, p.format.as_deref(), p.command.as_str()), (true, Some("#{pane_id}"), ""));
}

#[test]
fn new_pane_argv_after_marker_keeps_the_marker() {
    let p = parse_new_pane_args(&["-P", "-F", "id=#{pane_id}", "--", "cmd.exe", "/q", "/k"]);
    assert_eq!(p.format.as_deref(), Some("id=#{pane_id}"));
    assert!(p.command.starts_with("-- "), "{}", p.command);
    assert!(p.command.contains("cmd.exe") && p.command.contains("/k"), "{}", p.command);
    // One token after `--` keeps string semantics.
    let p = parse_new_pane_args(&["--", "vim notes.txt"]);
    assert_eq!(p.command, "vim notes.txt");
}

#[test]
fn new_pane_options_stop_at_the_first_operand() {
    // tmux getopt: `-x` after the command belongs to the command.
    let p = parse_new_pane_args(&["-x", "40", "-y", "10", "-X", "2", "-Y", "3", "-T", "t", "cmd.exe", "/k", "-x"]);
    assert_eq!((p.w, p.h, p.x, p.y), (Some(40), Some(10), Some(2), Some(3)));
    assert_eq!(p.title.as_deref(), Some("t"));
    assert_eq!(p.command, "cmd.exe /k -x");
    // Value flags tmux has that psmux does not use are consumed, not run.
    let p = parse_new_pane_args(&["-e", "A=1", "-s", "fg=red", "-m", "hi", "-l", "50%", "-P"]);
    assert_eq!(p.command, "");
    assert!(p.print);
    let p = parse_new_pane_args(&["-E"]);
    assert!(p.empty);
}

#[test]
fn if_shell_parts_reads_the_operands() {
    let (f, pos) = if_shell_parts(&["-F", "#{pane_id}", "display -p yes", "display -p no"]);
    assert!(f);
    assert_eq!(pos, vec!["#{pane_id}", "display -p yes", "display -p no"]);
    let (f, pos) = if_shell_parts(&["-b", "true", "{", "rename-window", "a", "}"]);
    assert!(!f);
    assert_eq!(pos, vec!["true", "rename-window a"]);
}

#[test]
fn run_shell_text_strips_only_its_own_flag_and_balanced_quotes() {
    assert_eq!(run_shell_command_text(&["-b", "'echo hi'"]), (true, "echo hi".to_string()));
    assert_eq!(run_shell_command_text(&["psmux", "set", "-g", "@o", "v"]), (false, "psmux set -g @o v".to_string()));
    assert_eq!(run_shell_command_text(&["echo", "'a'", "b'"]).1, "echo 'a' b'");
}
