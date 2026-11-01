// PR #740: a foreground run-shell runs off the attached client's reader.
//
// 1. run_shell_output_text is the stdout + stderr merge both the persistent and
//    the one shot path render, factored out of the old inline code. These pin
//    that it is byte identical to what the inline code produced.
// 2. binding_wire_lines keeps tmux's ordering: a command list in which a
//    foreground run-shell is followed by more commands goes out as one line, so
//    the server can hold the tail until the shell exits.

use crate::run_shell::output_text as run_shell_output_text;

#[cfg(windows)]
fn status_ok() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}
#[cfg(not(windows))]
fn status_ok() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
}

fn out(stdout: &[u8], stderr: &[u8]) -> std::process::Output {
    std::process::Output { status: status_ok(), stdout: stdout.to_vec(), stderr: stderr.to_vec() }
}

/// The inline merge as it stood before PR #740, kept verbatim as the oracle.
fn previous_inline(o: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&o.stderr);
    if !stderr_text.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr_text);
    }
    text
}

#[test]
fn output_text_empty_is_empty_so_no_popup() {
    assert_eq!(run_shell_output_text("command", &out(b"", b"")), "");
}

#[test]
fn output_text_stdout_only_is_verbatim() {
    assert_eq!(run_shell_output_text("command", &out(b"hello\r\n", b"")), "hello\r\n");
    assert_eq!(run_shell_output_text("command", &out(b"no newline", b"")), "no newline");
}

#[test]
fn output_text_stderr_only_gets_no_leading_newline() {
    assert_eq!(run_shell_output_text("command", &out(b"", b"boom\n")), "boom\n");
}

#[test]
fn output_text_both_inserts_one_newline_only_when_missing() {
    assert_eq!(run_shell_output_text("command", &out(b"out", b"err")), "out\nerr");
    assert_eq!(run_shell_output_text("command", &out(b"out\n", b"err\n")), "out\nerr\n");
}

#[test]
fn output_text_trailing_newline_is_kept() {
    assert!(run_shell_output_text("command", &out(b"a\n", b"")).ends_with('\n'));
    assert!(!run_shell_output_text("command", &out(b"a", b"")).ends_with('\n'));
}

#[test]
fn output_text_matches_previous_inline_code() {
    let cases: &[(&[u8], &[u8])] = &[
        (b"", b""), (b"x", b""), (b"", b"y"), (b"x", b"y"), (b"x\n", b"y"),
        (b"x\r\n", b"y\r\n"), (b"\n", b"\n"), (b"\xff\xfe bad utf8", b"\xc3"),
        ("ünïcode ✓".as_bytes(), "错误".as_bytes()),
    ];
    for (so, se) in cases {
        let o = out(so, se);
        assert_eq!(run_shell_output_text("command", &o), previous_inline(&o), "stdout={so:?} stderr={se:?}");
    }
}

// ---------------------------------------------------------------------------

fn wire(cmds: &[&str]) -> Vec<String> {
    let v: Vec<String> = cmds.iter().map(|s| s.to_string()).collect();
    crate::client::binding_wire_lines(&v)
}

#[test]
fn wire_single_command_is_one_line() {
    assert_eq!(wire(&["select-pane -L"]), vec!["select-pane -L\n"]);
    assert_eq!(wire(&["run-shell 'sleep 4'"]), vec!["run-shell 'sleep 4'\n"]);
}

#[test]
fn wire_lists_without_a_waiting_run_shell_are_unchanged() {
    assert_eq!(wire(&["select-pane -L", "switch-client -T MOVE"]),
               vec!["select-pane -L\n", "switch-client -T MOVE\n"]);
    // -b does not wait in tmux either.
    assert_eq!(wire(&["run-shell -b 'x'", "set -g @a 1"]),
               vec!["run-shell -b 'x'\n", "set -g @a 1\n"]);
    // A run-shell that ends the list has nothing to hold.
    assert_eq!(wire(&["set -g @a 1", "run-shell 'x'"]),
               vec!["set -g @a 1\n", "run-shell 'x'\n"]);
}

#[test]
fn wire_joins_a_list_with_a_foreground_run_shell_before_its_tail() {
    let lines = wire(&["run-shell 'sleep 3'", "set -g @chain done"]);
    assert_eq!(lines, vec!["run-shell 'sleep 3' \\; set -g @chain done\n"]);
    let lines = wire(&["display hi", "run 'x'", "display bye"]);
    assert_eq!(lines.len(), 1);
}

#[test]
fn wire_joined_line_splits_back_into_the_same_commands() {
    let cmds = [
        "run-shell 'echo a ; b'",
        "display-message \"x \\; y\"",
        "set -g @p 'C:\\path\\'",
        "send-keys -t %1 'q' Enter",
    ];
    let lines = wire(&cmds);
    assert_eq!(lines.len(), 1);
    let back = crate::config::split_chained_commands_pub(lines[0].trim_end_matches('\n'));
    assert_eq!(back, cmds.iter().map(|s| s.to_string()).collect::<Vec<_>>());
}
