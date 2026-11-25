// Issue #1088: over -CC, `if-shell -F 1 'display-message -p yes'` answered
// ONE %begin/%end block holding `yes`. tmux answers an empty block for the
// if-shell item and then one block per command of the chosen branch, which
// it queues as new items after the if-shell (cmd-if-shell.c:99-100 and
// :169-170, one guard per item in cmd-queue.c:592 and :652). Clients count
// blocks to match replies to commands. Registered from
// src/server/connection.rs.
//
// No server is started: the dispatcher is driven with a bare channel and a
// stand in server that expands a format to itself.

use super::*;

/// Run one control command; return every reply it sent, in order.
fn replies(cmd: &str, args: &[&str]) -> Vec<String> {
    let (ctx, crx) = mpsc::channel::<CtrlReq>();
    let server = std::thread::spawn(move || {
        for req in crx {
            if let CtrlReq::DisplayMessage(resp, fmt, ..) = req {
                let _ = resp.send(fmt);
            }
        }
    });
    let (rs, rr) = mpsc::channel::<String>();
    {
        let tx = TargetedSender::new(&ctx, None);
        dispatch_control_command(cmd, args, &tx, rs, None, false, None, 0, &Default::default(), false, &Default::default());
    }
    drop(ctx);
    server.join().unwrap();
    rr.try_iter().collect()
}

fn branch(text: &str) -> String {
    format!("{}w{}", IF_SHELL_BRANCH, text)
}

#[test]
fn format_condition_hands_back_the_branch_instead_of_running_it() {
    // master ran the branch inside the dispatcher and answered "yes".
    assert_eq!(replies("if-shell", &["-F", "1", "display-message -p yes"]), vec![branch("display-message -p yes")]);
    assert_eq!(
        replies("if-shell", &["-F", "0", "display-message -p yes", "display-message -p no"]),
        vec![branch("display-message -p no")]
    );
    // A false condition with no else branch is one empty block.
    assert_eq!(replies("if-shell", &["-F", "0", "display-message -p yes"]), vec![String::new()]);
    // A two command branch goes back whole; the loop splits it.
    assert_eq!(
        replies("if-shell", &["-F", "1", "display-message -p a ; display-message -p b"]),
        vec![branch("display-message -p a ; display-message -p b")]
    );
}

#[test]
fn format_branch_that_does_not_parse_is_the_if_shell_error() {
    // args_make_commands_now: nothing of the branch runs.
    assert_eq!(
        replies("if-shell", &["-F", "1", "display-message -p a ; bogus-cmd"]),
        vec!["\u{0001}ERR\u{0001}unknown command: bogus-cmd".to_string()]
    );
    // The branch not taken is never parsed.
    assert_eq!(replies("if-shell", &["-F", "1", "display-message -p z", "bogus"]), vec![branch("display-message -p z")]);
}

#[test]
fn shell_condition_ends_its_block_before_the_branch() {
    // `true` and `false` are decided without a shell.
    assert_eq!(
        replies("if-shell", &["true", "display-message -p y", "display-message -p n"]),
        vec![IF_SHELL_BLOCK_DONE.to_string(), branch("display-message -p y")]
    );
    assert_eq!(
        replies("if-shell", &["false", "display-message -p y", "display-message -p n"]),
        vec![IF_SHELL_BLOCK_DONE.to_string(), branch("display-message -p n")]
    );
    assert_eq!(replies("if-shell", &["false", "display-message -p y"]), vec![IF_SHELL_BLOCK_DONE.to_string(), String::new()]);
    assert_eq!(
        replies("if-shell", &["-b", "true", "display-message -p bg"]),
        vec![IF_SHELL_BLOCK_DONE.to_string(), format!("{}b{}", IF_SHELL_BRANCH, "display-message -p bg")]
    );
}

#[test]
fn background_flag_is_read_from_the_flags_only() {
    assert!(if_shell_background(&["-b", "true", "x"]));
    assert!(if_shell_background(&["-bF", "1", "x"]));
    assert!(!if_shell_background(&["-F", "1", "x"]));
    // `-b` inside an operand is the branch's, not if-shell's.
    assert!(!if_shell_background(&["true", "-b"]));
    assert!(!if_shell_background(&["--", "-b"]));
}

fn queued(pending: &std::collections::VecDeque<ControlQueued>) -> Vec<(String, u64, bool, u8)> {
    pending.iter().map(|q| (q.line.clone(), q.group, q.echo, q.flags)).collect()
}

#[test]
fn branch_runs_next_as_its_own_list_with_one_entry_per_command() {
    // The rest of the line: `if-shell -F 1 'a ; b' ; display -p outer`.
    let mut pending = std::collections::VecDeque::new();
    pending.push_back(ControlQueued { line: "display-message -p outer".into(), group: 1, echo: true, flags: 1 });
    let mut next_group = 1;
    queue_if_shell_branch(&mut pending, &mut next_group, Some(branch("display-message -p a ; display-message -p b")), 1);
    assert_eq!(
        queued(&pending),
        vec![
            ("display-message -p a".to_string(), 2, false, 1),
            ("display-message -p b".to_string(), 2, false, 1),
            ("display-message -p outer".to_string(), 1, true, 1),
        ]
    );
    // A failure in the branch drops the rest of the branch only.
    pending.pop_front();
    pending.retain(|q| q.group != 2);
    assert_eq!(queued(&pending), vec![("display-message -p outer".to_string(), 1, true, 1)]);
}

#[test]
fn background_branch_is_appended_with_flags_zero() {
    let mut pending = std::collections::VecDeque::new();
    pending.push_back(ControlQueued { line: "display-message -p after".into(), group: 1, echo: true, flags: 1 });
    let mut next_group = 1;
    queue_if_shell_branch(&mut pending, &mut next_group, Some(format!("{}b{}", IF_SHELL_BRANCH, "display-message -p bg")), 1);
    assert_eq!(
        queued(&pending),
        vec![
            ("display-message -p after".to_string(), 1, true, 1),
            ("display-message -p bg".to_string(), 2, false, 0),
        ]
    );
    // No branch (a false condition without else) queues nothing.
    queue_if_shell_branch(&mut pending, &mut next_group, Some(String::new()), 1);
    queue_if_shell_branch(&mut pending, &mut next_group, None, 1);
    assert_eq!(pending.len(), 2);
}

#[test]
fn control_line_keeps_a_brace_block_whole() {
    // `{ a ; b }` is one if-shell operand, not two commands.
    assert_eq!(
        split_top_level_semicolons("if -F 1 { display -p a ; display -p b } ; display -p c"),
        vec!["if -F 1 { display -p a ; display -p b }".to_string(), "display -p c".to_string()]
    );
    // Formats and quoted braces are no blocks.
    assert_eq!(split_top_level_semicolons("display -p '#{pane_id}' ; display -p x"), vec!["display -p '#{pane_id}'", "display -p x"]);
    assert_eq!(split_top_level_semicolons("display -p '{' ; display -p x"), vec!["display -p '{'", "display -p x"]);
    // Unbalanced braces read as before.
    assert_eq!(split_top_level_semicolons("display -p { ; display -p x"), vec!["display -p {", "display -p x"]);
}

#[test]
fn guard_lines_carry_the_flags() {
    assert_eq!(crate::control::format_begin_flags(5, 7, 0), "%begin 5 7 0");
    assert_eq!(crate::control::format_end_flags(5, 7, 1), "%end 5 7 1");
    assert_eq!(crate::control::format_error_flags(5, 7, 0), "%error 5 7 0");
    assert_eq!(crate::control::format_begin_flags(5, 7, 1), crate::control::format_begin(5, 7));
}
