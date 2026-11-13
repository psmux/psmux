// shared: requests an attached client sends on its persistent connection
// (a binding, the prompt, a menu) that the shared dispatch dropped without a
// word, so prefix ] or a buffer command did nothing in shared mode. The
// session local ones are now copied from run_server; the rest put a status
// message on the session's clients. No server, socket or pane is started:
// the sessions are bare AppStates.

use super::*;
use std::sync::mpsc;

fn session(name: &str) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("tdrops".to_string());
    let aliases = app.command_aliases.clone();
    Session {
        app,
        lp: SessionLoop::new(aliases),
        cwd: None,
        port: 0,
        stop: Arc::new(AtomicBool::new(false)),
        _guard: None,
    }
}

struct EnvBack(Option<String>);
impl Drop for EnvBack {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("PSMUX_TARGET_SESSION", v),
            None => std::env::remove_var("PSMUX_TARGET_SESSION"),
        }
    }
}

fn send(s: &mut Session, ids: &mut ServerIds, ns: &Option<String>, req: CtrlReq) {
    send_b(s, ids, &mut ServerBuffers::new(), ns, req)
}

fn send_b(s: &mut Session, ids: &mut ServerIds, bufs: &mut ServerBuffers, ns: &Option<String>, req: CtrlReq) {
    let pty_system = portable_pty::native_pty_system();
    let flow = run_request(s, ids, bufs, ns, |app, lp| {
        crate::server::shared::dispatch::dispatch(app, lp, req, &*pty_system)
    });
    assert_eq!(flow, Flow::Continue);
}

fn status(app: &AppState) -> Option<String> {
    app.status_message.as_ref().map(|(m, _, _)| m.clone())
}

#[test]
fn named_buffers_are_set_appended_listed_and_deleted_server_wide() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut bufs = ServerBuffers::new();
    let mut a = session("A");
    let mut b = session("B");
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::SetNamedBuffer("nb".into(), "one".into()));
    assert_eq!(bufs.get("nb"), Some("one"),
        "BUG: shared dispatch dropped SetNamedBuffer (set-buffer -b)");
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::AppendNamedBuffer("nb".into(), "two".into()));
    assert_eq!(bufs.get("nb"), Some("onetwo"), "set-buffer -a appends");
    assert_eq!(b.app.paste_get("nb"), None, "outside a request a session holds no buffer of its own");

    let (tx, rx) = mpsc::channel();
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::ListBuffers(tx));
    let listing = rx.try_recv().expect("BUG: list-buffers got no reply");
    assert!(listing.contains("nb"), "{listing}");

    let (tx, rx) = mpsc::channel();
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::DeleteNamedBuffer("nb".into(), Some(tx)));
    assert_eq!(rx.try_recv(), Ok(true), "delete-buffer -b answers found");
    assert_eq!(bufs.get("nb"), None);
}

#[test]
fn the_top_buffer_is_deleted_and_files_load_and_save() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut bufs = ServerBuffers::new();
    let mut a = session("A");
    let dir = std::env::temp_dir().join(format!("psmux_tdrops_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("in.txt");
    std::fs::write(&file, "FROM_FILE").unwrap();

    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::LoadBuffer(file.display().to_string()));
    assert_eq!(bufs.top(), Some("FROM_FILE"), "BUG: load-buffer dropped");
    let out = dir.join("out.txt");
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::SaveBuffer(out.display().to_string()));
    assert_eq!(std::fs::read_to_string(&out).ok().as_deref(), Some("FROM_FILE"), "BUG: save-buffer dropped");
    send_b(&mut a, &mut ids, &mut bufs, &ns, CtrlReq::DeleteBuffer);
    assert_eq!(bufs.top(), None, "BUG: delete-buffer dropped");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_request_still_not_run_says_so_on_its_session_only() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let mut b = session("B");
    send(&mut a, &mut ids, &ns, CtrlReq::RenameSession("C".into()));
    assert_eq!(status(&a.app).as_deref(), Some("not supported in shared server mode yet: rename-session"),
        "BUG: a refused request vanished without a word");
    assert_eq!(a.app.session_name, "A", "nothing was run");
    assert_eq!(status(&b.app), None, "B is not told");
    send(&mut a, &mut ids, &ns, CtrlReq::LockClient);
    assert_eq!(status(&a.app).as_deref(), Some("not supported in shared server mode yet: lock-client"));
}

#[test]
fn internal_requests_stay_silent() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    send(&mut a, &mut ids, &ns, CtrlReq::DebugStall(0));
    assert_eq!(status(&a.app), None, "a request the user did not make shows nothing");
}

#[test]
fn a_forward_request_of_the_handler_stays_silent() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let (reply_sender, reply_receiver) = mpsc::channel();
    let line = "set -t B: -- @probe v".to_string();
    let request = CtrlReq::ForwardToOtherSession(line, reply_sender);
    send(&mut a, &mut ids, &ns, request);
    assert_eq!(status(&a.app), None, "a set with a target shows no refusal");
    assert!(
        reply_receiver.recv().is_err(),
        "the handler gets no answer and runs its own set"
    );
}

#[test]
fn the_ported_commands_run_as_one_shot_commands_too() {
    for ok in ["select-layout", "selectl", "next-layout", "nextl", "previous-layout", "prevl",
        "rotate-window", "rotatew", "swap-pane", "swapp", "respawn-pane", "respawnp",
        "clear-history", "clearhist", "set-buffer", "setb", "paste-buffer", "pasteb",
        "list-buffers", "lsb", "delete-buffer", "deleteb", "save-buffer", "saveb",
        "load-buffer", "loadb", "show-messages", "showmsgs"] {
        assert!(router::is_supported(ok), "{ok}");
    }
    for no in ["rename-session", "break-pane", "join-pane", "display-menu", "display-popup",
        "move-window", "swap-window", "choose-buffer", "lock-client"] {
        assert!(!router::is_supported(no), "{no}");
    }
}

#[test]
fn server_info_and_info_reach_show_messages_through_the_default_aliases() {
    let a = session("A");
    let alias = |name: &str| alias_of(&a.app.command_aliases, name);
    assert_eq!(alias("server-info").as_deref(), Some("show-messages -JT"));
    assert_eq!(alias("info").as_deref(), Some("show-messages -JT"));
    for line in [
        "server-info",
        "info",
        "show-messages -JT",
        "show-messages -t /dev/pts/0",
        "showmsgs",
    ] {
        assert_eq!(
            router::first_unsupported_aliased(line, alias),
            None,
            "{line}"
        );
    }
}

#[test]
fn show_messages_answers_with_the_summary_of_its_own_session() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("tdrops".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let mut b = session("B");
    let (tx, rx) = mpsc::channel();
    send(&mut a, &mut ids, &ns, CtrlReq::ServerInfo(tx));
    let from_a = rx
        .try_recv()
        .expect("BUG: show-messages got no reply from A");
    let (tx, rx) = mpsc::channel();
    send(&mut b, &mut ids, &ns, CtrlReq::ServerInfo(tx));
    let from_b = rx
        .try_recv()
        .expect("BUG: show-messages got no reply from B");
    let lines_a: Vec<&str> = from_a.lines().collect();
    let lines_b: Vec<&str> = from_b.lines().collect();
    assert_eq!(lines_a.len(), 6, "{from_a}");
    assert_eq!(lines_b.len(), 6, "{from_b}");
    assert_eq!(lines_a[2], "session: A", "{from_a}");
    assert_eq!(lines_b[2], "session: B", "{from_b}");
    assert!(lines_a[5].ends_with("\\tdrops__A.port"), "{from_a}");
    assert!(lines_b[5].ends_with("\\tdrops__B.port"), "{from_b}");
    assert_eq!(
        lines_a[1], lines_b[1],
        "one shared server answers for both sessions"
    );
}
