use super::*;

// A bare -t word that names no session, for the commands that read it as a
// session (tmux CMD_FIND_SESSION) and for the option and environment
// commands, is tmux's error, never this session (hook, binding, prompt and
// the TCP set route all go through route_to_other_session).

/// Each test runs on a private, empty data dir: the route reads the session
/// registry, and a developer's real session must never be reached.
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
        let dir = std::env::temp_dir().join(format!("psmux_missing_target_{}_{}", std::process::id(), n));
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

fn route(app: &mut AppState, line: &str) -> OtherSessionRoute {
    let words = parse_command_line(line);
    let parts: Vec<&str> = words.iter().map(String::as_str).collect();
    route_to_other_session(app, &parts, line)
}

#[test]
fn a_missing_session_is_tmuxs_error_for_session_and_option_commands() {
    let _isolated = PrivateDataDir::new();
    let mut app = AppState::new("A".to_string());
    for (line, want) in [
        ("list-windows -t NOPE", "can't find session: NOPE"),
        ("lsw -t NOPE", "can't find session: NOPE"),
        ("has-session -t NOPE", "can't find session: NOPE"),
        ("lock-session -t NOPE", "can't find session: NOPE"),
        ("set-environment -t NOPE EV 1", "no such session: NOPE"),
        ("setenv -t NOPE EV 1", "no such session: NOPE"),
        ("show-environment -t NOPE EV", "no such session: NOPE"),
        ("set -t NOPE @u 1", "no such session: NOPE"),
        ("set-option -t NOPE status-left x", "no such session: NOPE"),
        ("set -t =NOPE @u 1", "no such session: NOPE"),
        ("show -t NOPE status-left", "no such session: NOPE"),
    ] {
        assert_eq!(route(&mut app, line), OtherSessionRoute::Refused(want.to_string()), "{line}");
    }
}

#[test]
fn the_hook_route_puts_the_error_on_the_status_line_and_runs_nothing() {
    let _isolated = PrivateDataDir::new();
    let mut app = AppState::new("A".to_string());
    execute_command_string(&mut app, "set -t NOPE @u hookval").unwrap();
    let message = app.status_message.as_ref().map(|(text, _, _)| text.clone());
    assert_eq!(message.as_deref(), Some("no such session: NOPE"));
    assert!(
        !app.user_options.contains_key("@u"),
        "the option was set on this session"
    );
}

#[test]
fn own_session_and_window_shaped_targets_stay_here() {
    let _isolated = PrivateDataDir::new();
    let mut app = AppState::new("A".to_string());
    for line in [
        "list-windows -t A",
        "set -t A @u 1",
        "set-environment -t =A EV 1",
        "has-session -t A",
        // Not session commands: their bare word is a window or pane name,
        // or their own arm refuses it.
        "select-window -t NOPE",
        "kill-session -t NOPE",
        "rename-session -t NOPE X",
        "display -t NOPE -p x",
        "set -t %3 @u 1",
    ] {
        assert_eq!(route(&mut app, line), OtherSessionRoute::Local, "{line}");
    }
}

#[test]
fn an_existing_other_session_is_still_forwarded() {
    let _isolated = PrivateDataDir::new();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = crate::session::session_name_base(None, "B");
    std::fs::write(crate::paths::port_file(&base), listener.local_addr().unwrap().port().to_string()).unwrap();
    std::fs::write(crate::paths::key_file(&base), "keyB").unwrap();
    let mut app = AppState::new("A".to_string());
    for (line, sent) in [
        ("list-windows -t B", "list-windows -t B:"),
        ("set-environment -t B EV 1", "set-environment -t B: EV 1"),
        ("set -t B @u 1", "set -t B: @u 1"),
    ] {
        assert_eq!(route(&mut app, line), OtherSessionRoute::Forwarded, "{line}");
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
        let mut text = String::new();
        let mut buf = [0u8; 256];
        while !text.contains(sent) {
            match std::io::Read::read(&mut stream, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => text.push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
        assert!(text.starts_with("AUTH keyB\n"), "{line}: {text:?}");
        assert!(text.contains(sent), "{line}: sent {text:?}");
    }
}

#[test]
fn the_tcp_route_reads_a_bare_session_word_for_session_commands() {
    use crate::server::connection::explicit_target_session as session_of;
    assert_eq!(session_of("list-windows", Some("NOPE")).as_deref(), Some("NOPE"));
    assert_eq!(session_of("lsw", Some("=NOPE")).as_deref(), Some("NOPE"));
    assert_eq!(session_of("set-environment", Some("A")).as_deref(), Some("A"));
    assert_eq!(session_of("show-environment", Some("NOPE")).as_deref(), Some("NOPE"));
    assert_eq!(session_of("lock-session", Some("NOPE")).as_deref(), Some("NOPE"));
    // Window and pane commands keep their bare word a window.
    assert_eq!(session_of("select-window", Some("NOPE")), None);
    assert_eq!(session_of("capture-pane", Some("NOPE")), None);
    assert_eq!(session_of("list-windows", Some("%3")), None);
    // An explicit session part was already read.
    assert_eq!(session_of("capture-pane", Some("NOPE:1")).as_deref(), Some("NOPE"));
}

#[test]
fn the_cli_names_the_session_of_a_by_name_target_only() {
    assert_eq!(crate::named_target_session(Some("NOPE")).as_deref(), Some("NOPE"));
    assert_eq!(crate::named_target_session(Some("=NOPE")).as_deref(), Some("NOPE"));
    assert_eq!(crate::named_target_session(Some("NOPE:1.0")).as_deref(), Some("NOPE"));
    assert_eq!(crate::named_target_session(Some("%3")), None);
    assert_eq!(crate::named_target_session(Some("@1")), None);
    assert_eq!(crate::named_target_session(Some("$2")), None);
    assert_eq!(crate::named_target_session(Some("1.0")), None);
    assert_eq!(crate::named_target_session(Some(":1")), None);
    assert_eq!(crate::named_target_session(None), None);
}
