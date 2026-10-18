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
    // -l / -p size a tiled split; tmux reads only -x/-y/-X/-Y for a float.
    let p = parse_new_pane_args(&["-l", "50%", "-p", "30", "-h", "-b", "-f", "-Z", "-P"]);
    assert_eq!(p.command, "");
    assert!(p.print && p.unsupported.is_none());
    let p = parse_new_pane_args(&["-E"]);
    assert!(p.empty);
}

#[test]
fn new_pane_env_and_styles_are_kept() {
    let p = parse_new_pane_args(&["-e", "A=1", "-eB=two=2", "-s", "bg=blue", "-S", "fg=red", "-R", "fg=green", "--", "cmd.exe"]);
    assert_eq!(p.extra.env, vec![("A".to_string(), "1".to_string()), ("B".to_string(), "two=2".to_string())]);
    assert_eq!(p.extra.styles.style, "bg=blue");
    assert_eq!(p.extra.styles.active_border, "fg=red");
    assert_eq!(p.extra.styles.border, "fg=green");
    assert_eq!(p.command, "cmd.exe");
    assert!(p.refusal().is_none());
}

#[test]
fn new_pane_flags_a_float_cannot_honour_are_refused() {
    // They used to be consumed and ignored at exit 0.
    for (args, flag) in [
        (vec!["-k"], 'k'),
        (vec!["-m", "bye"], 'm'),
        (vec!["-I"], 'I'),
        (vec!["-O"], 'O'),
        (vec!["-M"], 'M'),
        (vec!["-W"], 'W'),
        (vec!["-L"], 'L'),
        (vec!["-dk"], 'k'),
    ] {
        let p = parse_new_pane_args(&args);
        assert_eq!(p.unsupported, Some(flag), "{args:?}");
        let msg = p.refusal().unwrap();
        assert!(msg.starts_with(&format!("new-pane -{}:", flag)), "{msg}");
        assert!(msg.contains("not supported"), "{msg}");
    }
}

#[test]
fn if_shell_parts_reads_the_operands() {
    let (f, pos) = if_shell_parts(&["-F", "#{pane_id}", "display -p yes", "display -p no"]);
    assert!(f);
    assert_eq!(pos, vec!["#{pane_id}", "display -p yes", "display -p no"]);
    let (f, pos) = if_shell_parts(&["-b", "true", "{", "rename-window", "a", "}"]);
    assert!(!f);
    assert_eq!(pos, vec!["true", "rename-window a"]);
    // Dash tokens inside a block belong to the block (tmux 3.4 keeps them):
    // they used to be dropped, so `{ display -p x }` ran `display x`.
    let (f, pos) = if_shell_parts(&["true", "{", "display", "-p", "x", "}"]);
    assert!(!f);
    assert_eq!(pos, vec!["true", "display -p x"]);
    let (f, pos) = if_shell_parts(&["-F", "0", "{", "set", "-g", "@b", "yes", "}", "{", "set", "-gF", "@b", "#{e|+:1,2}", "}"]);
    assert!(f, "-gF inside a block is not if-shell's -F");
    assert_eq!(pos.len(), 3);
    assert_eq!(pos[1], "set -g @b yes");
    assert_eq!(parse_command_line(&pos[2]), vec!["set", "-gF", "@b", "#{e|+:1,2}"]);
    let (f, _) = if_shell_parts(&["true", "{", "set", "-gF", "@b", "x", "}"]);
    assert!(!f, "an F inside a block does not turn on format mode");
    // A nested block stays one operand, braces included.
    let (_, pos) = if_shell_parts(&["true", "{", "if-shell", "true", "{", "display", "-p", "in", "}", "}"]);
    assert_eq!(pos, vec!["true", "if-shell true { display -p in }"]);
}

#[test]
fn run_shell_text_strips_only_its_own_flag_and_balanced_quotes() {
    assert_eq!(run_shell_command_text(&["-b", "'echo hi'"]), Ok((true, None, "echo hi".to_string())));
    assert_eq!(run_shell_command_text(&["psmux", "set", "-g", "@o", "v"]), Ok((false, None, "psmux set -g @o v".to_string())));
    assert_eq!(run_shell_command_text(&["echo", "'a'", "b'"]).unwrap().2, "echo 'a' b'");
}
