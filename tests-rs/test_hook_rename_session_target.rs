use super::*;

// `rename-session` run in a server (a hook, a binding, the command prompt)
// reads -t as a session, as tmux does (CMD_FIND_SESSION). A session that does
// not exist is "can't find session" and this session keeps its name. The
// rename itself is the server's (CtrlReq::RenameSession), which moves the
// registry files from the CURRENT name, so the name must not change here
// first.

/// Each test runs on a private, empty data dir: the hook route reads the
/// session registry, and a developer's real session must never be reached.
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
        let dir = std::env::temp_dir().join(format!("psmux_hook_rename_{}_{}", std::process::id(), n));
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

fn server_app() -> (AppState, std::net::TcpListener) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut app = AppState::new("A".to_string());
    app.control_port = Some(listener.local_addr().unwrap().port());
    app.session_key = "key".to_string();
    (app, listener)
}

fn nothing_sent(listener: &std::net::TcpListener) -> bool {
    std::thread::sleep(std::time::Duration::from_millis(100));
    matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
}

fn received_lines(listener: &std::net::TcpListener) -> Vec<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
            Err(e) => panic!("no connection reached the session's own port: {e}"),
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut stream, &mut text).expect("reading the sent lines failed");
    text.lines().map(str::to_string).collect()
}

fn message(app: &AppState) -> Option<&str> {
    app.status_message.as_ref().map(|(text, _, _)| text.as_str())
}

#[test]
fn a_missing_session_target_renames_nothing() {
    for (command, shown) in [
        ("rename-session -t NOPE X", "NOPE"),
        ("rename-session -tNOPE X", "NOPE"),
        ("rename-session -t NOPE:1 X", "NOPE"),
        ("rename-session -t =NOPE X", "NOPE"),
        ("rename -t NOPE X", "NOPE"),
    ] {
        let _isolated = PrivateDataDir::new();
        let (mut app, listener) = server_app();
        execute_command_string(&mut app, command).unwrap();
        assert!(nothing_sent(&listener), "{command}: a rename was sent to the session's own port");
        assert_eq!(app.session_name, "A", "{command}: this session was renamed");
        let want = format!("can't find session: {shown}");
        assert_eq!(message(&app), Some(want.as_str()), "{command}");
    }
}

#[test]
fn the_own_session_or_no_target_asks_the_server_and_keeps_the_name_until_it_renames() {
    for command in ["rename-session X", "rename-session -t A X", "rename-session -t =A X", "rename-session -t A: X"] {
        let _isolated = PrivateDataDir::new();
        let (mut app, listener) = server_app();
        execute_command_string(&mut app, command).unwrap();
        assert_eq!(
            received_lines(&listener),
            vec!["AUTH key".to_string(), "rename-session -- \"X\"".to_string()],
            "{command}"
        );
        // The server's rename moves A's files to X; it must still read A.
        assert_eq!(app.session_name, "A", "{command}: renamed before the server moved the files");
        assert_eq!(message(&app), None, "{command}");
    }
}

#[test]
fn a_name_starting_with_a_dash_reaches_the_server_as_the_name() {
    let _isolated = PrivateDataDir::new();
    let (mut app, listener) = server_app();
    execute_command_string(&mut app, "rename-session -- -x").unwrap();
    assert_eq!(
        received_lines(&listener),
        vec!["AUTH key".to_string(), "rename-session -- \"-x\"".to_string()]
    );
}

#[test]
fn without_a_server_the_name_changes_here_and_a_missing_target_is_refused() {
    let _isolated = PrivateDataDir::new();
    let mut app = AppState::new("A".to_string());
    execute_command_string(&mut app, "rename-session -t NOPE X").unwrap();
    assert_eq!(app.session_name, "A");
    assert_eq!(message(&app), Some("can't find session: NOPE"));
    app.status_message = None;
    execute_command_string(&mut app, "rename-session X").unwrap();
    assert_eq!(app.session_name, "X");
}
