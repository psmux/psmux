// #1051: new-window and split-window read their flags the way tmux's getopt
// does, and every operand is the pane command. `split-window -d pwsh
// -NoProfile -Command x` ran a bare `pwsh` (only the first word was kept),
// and a word of the command could be read as a flag of split-window.
use super::*;

fn argv(words: &[&str]) -> String {
    format!("-- {}", crate::pane::requote_argv(words))
}

#[test]
fn every_operand_is_the_command() {
    let args = ["-d", "pwsh", "-NoProfile", "-Command", "Write-Output hi"];
    let (opts, cmd) = split_spawn_operands("split-window", &args);
    assert_eq!(opts, &["-d"]);
    assert_eq!(cmd, Some(argv(&["pwsh", "-NoProfile", "-Command", "Write-Output hi"])));

    let args = ["-d", "-n", "w", "cmd", "/c", "echo x"];
    let (opts, cmd) = split_spawn_operands("new-window", &args);
    assert_eq!(opts, &["-d", "-n", "w"]);
    assert_eq!(cmd, Some(argv(&["cmd", "/c", "echo x"])));
}

#[test]
fn double_dash_gives_the_same_command() {
    let a = ["-d", "--", "pwsh", "-NoProfile"];
    let b = ["-d", "pwsh", "-NoProfile"];
    assert_eq!(split_spawn_operands("split-window", &a).1, split_spawn_operands("split-window", &b).1);
}

#[test]
fn one_operand_is_a_shell_string() {
    let args = ["-v", "-d", "-c", "C:\\x", "pwsh -NoProfile -Command Start-Sleep 30"];
    let (opts, cmd) = split_spawn_operands("split-window", &args);
    assert_eq!(opts.len(), 4);
    assert_eq!(cmd.as_deref(), Some("pwsh -NoProfile -Command Start-Sleep 30"));
    let (_, cmd) = split_spawn_operands("split-window", &["-d", "--", "pwsh -NoProfile"]);
    assert_eq!(cmd.as_deref(), Some("pwsh -NoProfile"));
    let (_, cmd) = split_spawn_operands("split-window", &["-d"]);
    assert_eq!(cmd, None);
}

#[test]
fn flag_values_and_clusters_are_not_operands() {
    // -l, -p, -e, -F, -T, -m, -c take a value; -t too.
    let args = ["-h", "-l", "30%", "-e", "A=1", "-F", "#{pane_id}", "-T", "t", "-m", "msg", "-t", "%1", "pwsh"];
    let (opts, cmd) = split_spawn_operands("split-window", &args);
    assert_eq!(opts.len(), args.len() - 1);
    assert_eq!(cmd.as_deref(), Some("pwsh"));
    let (opts, cmd) = split_spawn_operands("new-window", &["-dPF", "#{window_id}", "vim"]);
    assert_eq!(opts, &["-dPF", "#{window_id}"]);
    assert_eq!(cmd.as_deref(), Some("vim"));
}

#[test]
fn a_dash_word_of_the_command_is_not_a_flag() {
    // `-h` and `-d` after the command are pwsh's, not split-window's.
    let (opts, _) = split_spawn_operands("split-window", &["pwsh", "-h"]);
    assert!(opts.is_empty());
    let (opts, _) = split_spawn_operands("new-window", &["git", "log", "-d"]);
    assert!(opts.is_empty());
}

#[test]
fn option_letters_stop_at_the_first_operand() {
    use crate::cli::options_and_operand_start as o;
    assert_eq!(o("split-window", &["-d", "-t", "A", "pwsh", "-NoProfile"]), ("dt".to_string(), 3));
    assert_eq!(o("split-window", &["-dP", "-F", "x", "--", "pwsh"]), ("dPF".to_string(), 4));
    assert_eq!(o("display-message", &["-c", "-p", "x"]), ("c".to_string(), 2));
    assert_eq!(o("display-message", &["-t-x", "-p"]), ("tp".to_string(), 2));
    assert_eq!(o("split-window", &["-l"]), ("l".to_string(), 1));
    assert_eq!(o("split-window", &["-", "-P"]), (String::new(), 0));
}

#[test]
fn respawn_keeps_every_operand() {
    let args = ["-k", "pwsh", "-NoProfile", "-Command", "Write-Output hi"];
    let (opts, cmd) = respawn_operands("respawn-pane", &args);
    assert_eq!(opts, &["-k"]);
    assert_eq!(cmd, Some(argv(&["pwsh", "-NoProfile", "-Command", "Write-Output hi"])));
    let (opts, cmd) = respawn_operands("respawn-window", &["-k", "-c", "C:\\x", "cmd", "/c", "echo x"]);
    assert_eq!(opts, &["-k", "-c", "C:\\x"]);
    assert_eq!(cmd, Some(argv(&["cmd", "/c", "echo x"])));
    // `-k` after the command is the command's.
    let (opts, _) = respawn_operands("respawn-pane", &["pwsh", "-k"]);
    assert!(opts.is_empty());
}

#[test]
fn respawn_one_operand_reads_as_before() {
    // The Claude Code teammate form: one shell string behind `--`, whole.
    let (_, cmd) = respawn_operands("respawn-pane", &["-k", "--", "\"C:\\a b\\x.exe\" --flag"]);
    assert_eq!(cmd.as_deref(), Some("\"C:\\a b\\x.exe\" --flag"));
    let (_, cmd) = respawn_operands("respawn-pane", &["-k", "\"pwsh -NoProfile\""]);
    assert_eq!(cmd.as_deref(), Some("pwsh -NoProfile"));
    let (_, cmd) = respawn_operands("respawn-pane", &["-k", "-e", "A=1"]);
    assert_eq!(cmd, None);
}
