use super::*;

// The routing decision the TCP route asks for before it relays a session
// command to the session its -t names (CtrlReq::DecideOtherSession), and the
// CLI's tmux wording for a -t naming no session.

struct PrivateDataDir {
    old: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl PrivateDataDir {
    fn new() -> Self {
        let lock = crate::util::lock_test_env();
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("psmux_target_relay_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("PSMUX_DATA_DIR");
        std::env::set_var("PSMUX_DATA_DIR", &dir);
        PrivateDataDir { old, dir, _lock: lock }
    }
}

impl Drop for PrivateDataDir {
    fn drop(&mut self) {
        match self.old.take() {
            Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
            None => std::env::remove_var("PSMUX_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn decide(app: &AppState, line: &str) -> OtherSessionDecision {
    let words = parse_command_line(line);
    let parts: Vec<&str> = words.iter().map(String::as_str).collect();
    decide_other_session(app, &parts, line)
}

fn with_registered_b() -> u16 {
    let base = crate::session::session_name_base(None, "B");
    std::fs::write(crate::paths::port_file(&base), "45678").unwrap();
    std::fs::write(crate::paths::key_file(&base), "keyB").unwrap();
    45678
}

#[test]
fn a_session_command_naming_another_session_is_decided_for_that_server() {
    let _isolated = PrivateDataDir::new();
    let port = with_registered_b();
    let app = AppState::new("A".to_string());
    for (line, sent) in [
        ("show -t B -v status-left", "show -t B: -v status-left"),
        ("list-windows -t B", "list-windows -t B:"),
        ("show-environment -t B EVB", "show-environment -t B: EVB"),
        ("set-environment -t B EV x", "set-environment -t B: EV x"),
    ] {
        assert_eq!(
            decide(&app, line),
            OtherSessionDecision::Other { name: "B".to_string(), port, key: "keyB".to_string(), line: sent.to_string() },
            "{line}"
        );
    }
}

#[test]
fn a_missing_session_is_refused_and_this_session_stays_here() {
    let _isolated = PrivateDataDir::new();
    let app = AppState::new("A".to_string());
    assert_eq!(decide(&app, "show -t NOPE -v status-left"), OtherSessionDecision::Refused("no such session: NOPE".to_string()));
    assert_eq!(decide(&app, "show-environment -t NOPE EV"), OtherSessionDecision::Refused("no such session: NOPE".to_string()));
    assert_eq!(decide(&app, "list-windows -t NOPE"), OtherSessionDecision::Refused("can't find session: NOPE".to_string()));
    assert_eq!(decide(&app, "show -t A -v status-left"), OtherSessionDecision::Local);
    assert_eq!(decide(&app, "list-windows -t =A"), OtherSessionDecision::Local);
    assert_eq!(decide(&app, "show -t %1 -v status-left"), OtherSessionDecision::Local);
}

#[test]
fn deciding_sends_nothing() {
    let _isolated = PrivateDataDir::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = crate::session::session_name_base(None, "B");
    std::fs::write(crate::paths::port_file(&base), listener.local_addr().unwrap().port().to_string()).unwrap();
    std::fs::write(crate::paths::key_file(&base), "keyB").unwrap();
    let app = AppState::new("A".to_string());
    assert!(matches!(decide(&app, "show -t B -v status-left"), OtherSessionDecision::Other { .. }));
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(
        matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "the decision connected to B"
    );
}

#[test]
fn the_cli_words_a_missing_session_as_tmux_does() {
    assert_eq!(crate::missing_session_cli_wording("list-windows"), Some("can't find session"));
    assert_eq!(crate::missing_session_cli_wording("lsw"), Some("can't find session"));
    for cmd in ["set-environment", "setenv", "show-environment", "showenv", "show-options", "show", "show-option"] {
        assert_eq!(crate::missing_session_cli_wording(cmd), Some("no such session"), "{cmd}");
    }
    // Commands with their own reporting, or whose -t is not a session.
    for cmd in ["kill-session", "has-session", "send-keys", "new-session", "attach-session", "set-option"] {
        assert_eq!(crate::missing_session_cli_wording(cmd), None, "{cmd}");
    }
}
