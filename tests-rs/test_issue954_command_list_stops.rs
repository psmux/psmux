// Issue #954: a command list stops at its first failing command, as tmux's
// cmdq_next removes the rest of the failing command's group
// (cmd-queue.c:750-754, cmdq_remove_group at :443-456).
//
// Real authenticated ingress, without starting a psmux server or a shell:
// a fake server loop records the pane titles `set-pane-title` asks for (the
// marker of which commands ran) and answers every user option lookup with
// "not set", so `show-options @nope` fails as it does on a real server.
use super::*;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock};

/// Sends `lines` on a connection (persistent or one-shot), closes the write
/// half, and returns the pane titles the handler asked for, in order.
fn titles_after(persistent: bool, lines: &[&str]) -> Vec<String> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    let titles = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = titles.clone();
    std::thread::spawn(move || {
        while let Ok(req) = rx.recv() {
            match req {
                CtrlReq::SetPaneTitle(t) => seen.lock().unwrap().push(t),
                CtrlReq::UserOptionLookup(reply, ..) => {
                    let _ = reply.send(None);
                }
                other => drop(other),
            }
        }
    });
    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, tx, "test954", Arc::new(RwLock::new(Default::default())));
        let _ = done_tx.send(());
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
    stream.write_all(b"AUTH test954\n").unwrap();
    let mut reply = String::new();
    reader.read_line(&mut reply).unwrap();
    assert_eq!(reply, "OK\n");
    if persistent {
        stream.write_all(b"PERSISTENT\n").unwrap();
    }
    for line in lines {
        write!(stream, "{line}\n").unwrap();
    }
    stream.shutdown(Shutdown::Write).unwrap();
    // Read the replies off so the handler is never blocked on a full socket.
    let mut sink = Vec::new();
    let _ = std::io::Read::read_to_end(&mut reader, &mut sink);
    assert!(done_rx.recv_timeout(Duration::from_secs(10)).is_ok(), "the handler did not return");
    std::thread::sleep(Duration::from_millis(50));
    let out = titles.lock().unwrap().clone();
    out
}

#[test]
fn persistent_line_stops_at_show_options_error_and_next_line_runs() {
    let got = titles_after(true, &[
        "set-pane-title A \\; show-options @nope \\; set-pane-title B",
        "set-pane-title C",
    ]);
    assert_eq!(got, vec!["A", "C"], "the rest of the failing line ran, or the next line did not");
}

#[test]
fn persistent_line_stops_at_other_failures() {
    for failing in [
        "show-options -g nosuch",
        "show-options -w @nope",
        "show-window-options nosuch",
        "display-message -x hi",
        "nosuchcommand a b",
        "show-buffer -b nosuch",
    ] {
        let got = titles_after(true, &[
            &format!("set-pane-title A \\; {failing} \\; set-pane-title B"),
            "set-pane-title C",
        ]);
        assert_eq!(got, vec!["A", "C"], "after `{failing}`");
    }
}

#[test]
fn persistent_list_with_no_failure_runs_all_of_it() {
    let got = titles_after(true, &["set-pane-title A \\; set-pane-title B \\; set-pane-title C"]);
    assert_eq!(got, vec!["A", "B", "C"]);
}

#[test]
fn one_shot_list_stops_at_show_options_error() {
    for failing in ["show-options @nope", "show-options -g nosuch", "show-window-options nosuch"] {
        let got = titles_after(false, &[&format!("set-pane-title A \\; {failing} \\; set-pane-title B")]);
        assert_eq!(got, vec!["A"], "after `{failing}`");
    }
}

#[test]
fn drop_failed_list_ends_an_if_shell_branch_only() {
    let mut q: Vec<String> = ["b2", LIST_END, "outer"].iter().map(|s| s.to_string()).collect();
    drop_failed_list(&mut q);
    assert_eq!(q, vec!["outer"]);
    let mut q: Vec<String> = ["b2", LIST_END, "o1", LIST_END, "o2"].iter().map(|s| s.to_string()).collect();
    drop_failed_list(&mut q);
    assert_eq!(q, vec!["o1", LIST_END, "o2"]);
    let mut q: Vec<String> = vec!["a".into(), "b".into()];
    drop_failed_list(&mut q);
    assert!(q.is_empty());
}
