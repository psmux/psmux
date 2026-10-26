// Issue #799: in control mode `send-keys -H` (and every `send-keys -l`, which
// the coalescer rewrites into `send -H`) returned from
// dispatch_control_command without answering, so the caller saw a dropped
// channel and printed `command timed out` / `%error`. tmux answers an empty
// block per send-keys (cmd-send-keys.c:253, one guard per item in
// cmd-queue.c:592 and :652). Registered from src/server/connection.rs.
//
// No server is started: the dispatcher is driven with a bare channel.

use super::*;

/// Run one control command against a stand in server that answers a copy
/// mode command with `x_outcome`; return (answered, reply, requests sent).
fn run_with(cmd: &str, args: &[&str], x_outcome: Result<(), String>) -> (bool, Result<String, mpsc::TryRecvError>, Vec<CtrlReq>) {
    let (ctx, crx) = mpsc::channel::<CtrlReq>();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for req in crx {
            if let CtrlReq::SendKeysXRun { cmd, count, resp: Some(resp) } = req {
                let _ = resp.send(x_outcome.clone());
                seen.push(CtrlReq::SendKeysXRun { cmd, count, resp: None });
            } else {
                seen.push(req);
            }
        }
        seen
    });
    let (rs, rr) = mpsc::channel::<String>();
    let answered = {
        let tx = TargetedSender::new(&ctx, None);
        dispatch_control_command(cmd, args, &tx, rs, None, false, None, 0, &Default::default(), false, &Default::default())
    };
    drop(ctx);
    let reqs = server.join().unwrap();
    (answered, rr.try_recv(), reqs)
}

fn run(cmd: &str, args: &[&str]) -> (bool, Result<String, mpsc::TryRecvError>, Vec<CtrlReq>) {
    run_with(cmd, args, Ok(()))
}

#[test]
fn send_keys_x_runs_the_copy_command_and_reports_tmux_error() {
    // The control route used to type the copy command's NAME into the pane.
    let (_, reply, reqs) = run("send-keys", &["-N", "2", "-X", "cursor-up"]);
    assert_eq!(reply, Ok(String::new()));
    assert!(matches!(reqs.as_slice(), [CtrlReq::SendKeysXRun { cmd, count: 2, .. }] if cmd == "cursor-up"));
    // cmd-send-keys.c:193: "not in a mode" is the command's %error.
    let (_, reply, _) = run_with("send-keys", &["-X", "cancel"], Err("not in a mode".to_string()));
    assert_eq!(reply, Ok("\u{0001}ERR\u{0001}not in a mode".to_string()));
}

#[test]
fn send_keys_repeat_and_end_of_options_reach_the_pane() {
    let (_, reply, reqs) = run("send-keys", &["-N", "3", "-l", "z"]);
    assert_eq!(reply, Ok(String::new()));
    assert_eq!(reqs.iter().filter(|r| matches!(r, CtrlReq::SendKeys(k, true) if k == &vec!["z".to_string()])).count(), 3);
    let (_, _, reqs) = run("send-keys", &["-N", "2", "-H", "79"]);
    assert_eq!(reqs.iter().filter(|r| matches!(r, CtrlReq::SendBytes(b) if b == &vec![0x79])).count(), 2);
    let (_, _, reqs) = run("send-keys", &["-l", "--", "-foo"]);
    assert!(matches!(reqs.as_slice(), [CtrlReq::SendKeys(k, true)] if k == &vec!["-foo".to_string()]));
}

#[test]
fn send_keys_hex_answers_an_empty_block() {
    let (answered, reply, reqs) = run("send-keys", &["-H", "65", "63", "0d"]);
    assert!(answered);
    assert_eq!(reply, Ok(String::new()), "-H must answer, not drop the reply");
    assert!(matches!(reqs.as_slice(), [CtrlReq::SendBytes(b)] if b == &vec![0x65, 0x63, 0x0d]));
}

#[test]
fn send_keys_hex_with_repeat_and_no_operands_still_answers() {
    let (_, reply, _) = run("send-keys", &["-N", "2", "-H", "79"]);
    assert_eq!(reply, Ok(String::new()));
    let (_, reply, reqs) = run("send", &["-H"]);
    assert_eq!(reply, Ok(String::new()));
    assert!(reqs.is_empty(), "nothing to write");
}

#[test]
fn send_keys_other_forms_answer_an_empty_block() {
    for args in [vec!["-l", "echo x"], vec!["Enter"], vec!["-X", "cancel"], vec!["-R"], vec!["-N", "3", "-l", "z"]] {
        let (answered, reply, _) = run("send-keys", &args);
        assert!(answered, "{args:?}");
        assert_eq!(reply, Ok(String::new()), "{args:?}");
    }
}

#[test]
fn show_buffer_of_a_missing_buffer_is_tmux_error_not_a_dropped_reply() {
    let (ctx, crx) = mpsc::channel::<CtrlReq>();
    let server = std::thread::spawn(move || {
        if let Ok(CtrlReq::ShowNamedBuffer(resp, name)) = crx.recv() {
            assert_eq!(name, "nosuch");
            let _ = resp.send(None);
        }
    });
    let (rs, rr) = mpsc::channel::<String>();
    let tx = TargetedSender::new(&ctx, None);
    assert!(dispatch_control_command("show-buffer", &["-b", "nosuch"], &tx, rs, None, false, None, 0, &Default::default(), false, &Default::default()));
    server.join().unwrap();
    // cmd-save-buffer.c:92: cmdq_error(item, "no buffer %s", bufname).
    assert_eq!(rr.try_recv(), Ok("\u{0001}ERR\u{0001}no buffer nosuch".to_string()));
}

#[test]
fn coalesced_sends_keep_one_reply_per_command() {
    let parts = vec![
        "display-message -p one".to_string(),
        "send-keys -t %1 -l 'echo ME'".to_string(),
        "send-keys -t %1 -l RGED".to_string(),
        "send -t %1 0xd".to_string(),
        "display-message -p two".to_string(),
    ];
    let out = coalesce_send_commands(parts.clone());
    assert_eq!(out.len(), parts.len(), "one entry, so one reply block, per command: {out:?}");
    assert_eq!(out[0], "display-message -p one");
    assert_eq!(out[1], "send -H -t %1 65 63 68 6f 20 4d 45 52 47 45 44 0d", "still one pane write");
    assert_eq!(out[2], format!("{}send-keys -t %1 -l RGED", COALESCED_SEND_REPLY));
    assert_eq!(out[3], format!("{}send -t %1 0xd", COALESCED_SEND_REPLY));
    assert_eq!(out[4], "display-message -p two");
}

#[test]
fn a_send_with_nothing_to_write_is_kept_so_it_is_answered() {
    // It used to vanish from the line, so it got no block at all.
    let out = coalesce_send_commands(vec!["send-keys -t %1 -l ''".to_string()]);
    assert_eq!(out, vec!["send-keys -t %1 -l ''"]);
    let out = coalesce_send_commands(vec!["send -l ''".to_string(), "send -l ''".to_string()]);
    assert_eq!(out, vec!["send -l ''".to_string(), format!("{}send -l ''", COALESCED_SEND_REPLY)]);
}
