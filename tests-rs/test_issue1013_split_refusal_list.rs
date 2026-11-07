// Issue #1013: a refused split-window inside a command list is the
// command's error. On a one-shot connection it is answered as
// `ERROR: split-window: <why>`, so the rest of the list does not run and the
// CLI exits 1 with it on stderr, as tmux stops a list at a failed command.
// It used to go out as plain text: exit 0, the error on stdout, and the rest
// of the list ran.
//
// Real authenticated ingress, without a psmux server or a shell: a fake
// server loop answers every split with `split_reply` and records the pane
// titles `set-pane-title` asks for, the marker of which commands ran.
use super::*;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, RwLock};

const REFUSED: &str = "psmux: split-window: pane too small to split horizontally (20 cols, need 21)";

/// Sends `lines` with every split answered by `split_reply`, and returns
/// the titles set, in order, with everything the connection wrote back.
fn run(persistent: bool, split_reply: &'static str, lines: &[&str]) -> (Vec<String>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    let titles = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = titles.clone();
    std::thread::spawn(move || {
        while let Ok(req) = rx.recv() {
            match req {
                CtrlReq::SetPaneTitle(t) => seen.lock().unwrap().push(t),
                CtrlReq::SplitWindow(_, _, _, _, _, resp, ..) => {
                    let _ = resp.send(split_reply.to_string());
                }
                CtrlReq::SplitWindowPrint(_, _, _, _, _, _, resp, ..) => {
                    let _ = resp.send(split_reply.to_string());
                }
                other => drop(other),
            }
        }
    });
    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, tx, "test1013", Arc::new(RwLock::new(Default::default())));
        let _ = done_tx.send(());
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
    stream.write_all(b"AUTH test1013\n").unwrap();
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
    let mut out = Vec::new();
    let _ = std::io::Read::read_to_end(&mut reader, &mut out);
    assert!(done_rx.recv_timeout(Duration::from_secs(10)).is_ok(), "the handler did not return");
    std::thread::sleep(Duration::from_millis(50));
    let got = titles.lock().unwrap().clone();
    (got, String::from_utf8_lossy(&out).into_owned())
}

#[test]
fn one_shot_refused_split_print_stops_the_list_with_an_error_reply() {
    let (titles, out) = run(false, REFUSED, &["split-window -h -d -P -F '#{pane_id}' \\; set-pane-title ran"]);
    assert!(titles.is_empty(), "the rest of the list ran: {titles:?}");
    assert_eq!(out, "ERROR: split-window: pane too small to split horizontally (20 cols, need 21)\n");
}

#[test]
fn one_shot_refused_split_stops_the_list_with_an_error_reply() {
    let (titles, out) = run(false, REFUSED, &["split-window -h -d \\; set-pane-title ran"]);
    assert!(titles.is_empty(), "the rest of the list ran: {titles:?}");
    assert!(out.starts_with("ERROR: split-window: pane too small"), "reply: {out:?}");
}

#[test]
fn one_shot_refused_split_after_a_command_stops_there() {
    let (titles, out) = run(false, REFUSED, &["set-pane-title A \\; split-window -h -d \\; set-pane-title B"]);
    assert_eq!(titles, vec!["A"]);
    assert!(out.contains("ERROR: split-window: "), "reply: {out:?}");
}

#[test]
fn successful_split_list_runs_all_and_prints_the_id() {
    let (titles, out) = run(false, "%5", &["split-window -h -d -P -F '#{pane_id}' \\; set-pane-title ran"]);
    assert_eq!(titles, vec!["ran"]);
    assert_eq!(out, "%5\n");
    let (titles, out) = run(false, "", &["split-window -h -d \\; set-pane-title ran"]);
    assert_eq!(titles, vec!["ran"]);
    assert_eq!(out, "");
}

#[test]
fn persistent_refused_split_ends_its_line_and_the_next_line_runs() {
    let (titles, _) = run(true, REFUSED, &["split-window -h -d \\; set-pane-title B", "set-pane-title C"]);
    assert_eq!(titles, vec!["C"]);
}

#[test]
fn split_refusal_is_read_from_the_server_loop_text_only() {
    assert_eq!(split_refusal(REFUSED), Some("split-window: pane too small to split horizontally (20 cols, need 21)"));
    assert_eq!(split_refusal("%5"), None);
    assert_eq!(split_refusal(""), None);
    assert_eq!(split_refusal("psmux: something else"), None);
}

#[test]
fn cli_prints_a_lone_refusal_in_its_old_wording() {
    assert_eq!(
        crate::cli::split_window_error_text("ERROR: split-window: pane too small (1 rows, need 5)\n"),
        "psmux: split-window: pane too small (1 rows, need 5)\n"
    );
    assert_eq!(crate::cli::split_window_error_text("ERROR: can't find pane: %9\n"), "ERROR: can't find pane: %9\n");
}
