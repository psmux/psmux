// Issue #798 step 2: how the shared server runs one request of one session.
// No server, socket or pane is started: the sessions are bare AppStates.
// `enter` writes PSMUX_TARGET_SESSION and the job environment, so each test
// holds the process wide test env lock and puts the variable back.

use super::*;

fn session(name: &str) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("t798".to_string());
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

#[test]
fn a_panicking_request_ends_only_its_session_and_keeps_the_ids() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("t798".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let mut b = session("B");
    // A request of A that allocates two ids and then panics.
    let flow = run_request(&mut a, &mut ids, &mut ServerBuffers::new(), &ns, |app, _lp| {
        app.next_win_id += 1;
        app.next_pane_id += 2;
        panic!("injected #798 test panic");
    });
    assert_eq!(flow, Flow::EndSession, "the panicking session is quarantined");
    // The ids it took are not handed out again.
    assert_eq!((ids.next_window(), ids.next_pane()), (2, 3));
    // B runs normally afterwards, and continues the server wide sequence.
    let flow = run_request(&mut b, &mut ids, &mut ServerBuffers::new(), &ns, |app, _lp| {
        assert_eq!((app.next_win_id, app.next_pane_id), (2, 3));
        app.next_win_id += 1;
        app.next_pane_id += 1;
        Ok(Flow::Continue)
    });
    assert_eq!(flow, Flow::Continue);
    assert_eq!((ids.next_window(), ids.next_pane()), (3, 4));
}

#[test]
fn a_request_runs_with_its_own_session_entered() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("t798".to_string());
    let mut ids = ServerIds::new();
    let mut sessions = [session("A"), session("B")];
    for (i, want) in [(0usize, "t798__A"), (1, "t798__B"), (0, "t798__A")] {
        let seen = std::cell::RefCell::new(String::new());
        run_request(&mut sessions[i], &mut ids, &mut ServerBuffers::new(), &ns, |_app, _lp| {
            *seen.borrow_mut() = std::env::var("PSMUX_TARGET_SESSION").unwrap_or_default();
            Ok(Flow::Continue)
        });
        assert_eq!(seen.into_inner(), want);
    }
}

/// Run one show-buffer request of `s` through the shared dispatch.
fn show(s: &mut Session, ids: &mut ServerIds, bufs: &mut ServerBuffers, ns: &Option<String>, name: Option<&str>) -> Result<Option<String>, std::sync::mpsc::TryRecvError> {
    let pty_system = portable_pty::native_pty_system();
    match name {
        None => {
            let (rtx, rrx) = std::sync::mpsc::channel::<String>();
            run_request(s, ids, bufs, ns, |app, lp| {
                crate::server::shared::dispatch::dispatch(app, lp, CtrlReq::ShowBuffer(rtx), &*pty_system)
            });
            rrx.try_recv().map(Some)
        }
        Some(name) => {
            let (rtx, rrx) = std::sync::mpsc::channel::<Option<String>>();
            run_request(s, ids, bufs, ns, |app, lp| {
                crate::server::shared::dispatch::dispatch(app, lp, CtrlReq::ShowNamedBuffer(rtx, name.to_string()), &*pty_system)
            });
            rrx.try_recv()
        }
    }
}

// #896: show-buffer answers from the server's buffers (server/mod.rs
// ShowBuffer and ShowNamedBuffer). Since #925 they are one set for every
// session, as in tmux (paste.c:32 to :36), so B sees what A made.
#[test]
fn show_buffer_reads_the_server_wide_buffers() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("t798".to_string());
    let mut ids = ServerIds::new();
    let mut bufs = ServerBuffers::new();
    let mut a = session("A");
    let mut b = session("B");
    a.app.paste_add("hello".to_string());
    a.app.paste_set("kept", "named data".to_string());
    assert_eq!(show(&mut a, &mut ids, &mut bufs, &ns, None), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut a, &mut ids, &mut bufs, &ns, Some("buffer0")), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut a, &mut ids, &mut bufs, &ns, Some("kept")), Ok(Some("named data".to_string())));
    assert_eq!(show(&mut a, &mut ids, &mut bufs, &ns, Some("nosuch")), Ok(None));
    assert_eq!(show(&mut b, &mut ids, &mut bufs, &ns, None), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut b, &mut ids, &mut bufs, &ns, Some("buffer0")), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut b, &mut ids, &mut bufs, &ns, Some("kept")), Ok(Some("named data".to_string())));
    assert_eq!(show(&mut b, &mut ids, &mut bufs, &ns, Some("nosuch")), Ok(None));
}

#[test]
fn an_io_error_keeps_the_session() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = None;
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let flow = run_request(&mut a, &mut ids, &mut ServerBuffers::new(), &ns, |_app, _lp| {
        Err(io::Error::new(io::ErrorKind::Other, "routine failure"))
    });
    assert_eq!(flow, Flow::Continue);
}

// #925: a buffer made by one session's request is there for the next request
// of another session, and a request that panics does not take the server's
// buffers with its session.
#[test]
fn buffers_cross_sessions_and_survive_a_panicking_request() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("t925".to_string());
    let mut ids = ServerIds::new();
    let mut bufs = ServerBuffers::new();
    let mut a = session("A");
    let mut b = session("B");
    let mut c = session("C");
    run_request(&mut a, &mut ids, &mut bufs, &ns, |app, _lp| {
        app.paste_set("g", "hola".to_string());
        Ok(Flow::Continue)
    });
    let flow = run_request(&mut b, &mut ids, &mut bufs, &ns, |app, _lp| {
        app.paste_add("from b".to_string());
        panic!("injected #925 test panic");
    });
    assert_eq!(flow, Flow::EndSession);
    let seen = std::cell::RefCell::new((None, None));
    run_request(&mut c, &mut ids, &mut bufs, &ns, |app, _lp| {
        *seen.borrow_mut() = (app.paste_get("g").map(str::to_string), app.paste_top().map(str::to_string));
        Ok(Flow::Continue)
    });
    assert_eq!(seen.into_inner(), (Some("hola".to_string()), Some("from b".to_string())),
        "BUG #925: session C does not see the buffers A and B made");
}

use std::ffi::OsString;

fn os_pairs(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    pairs
        .iter()
        .map(|(name, value)| (OsString::from(name), OsString::from(value)))
        .collect()
}

fn values_of(env: &[(OsString, OsString)], name: &str) -> Vec<OsString> {
    env.iter()
        .filter(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
        .collect()
}

fn identity_b() -> Vec<(String, String)> {
    job_identity(4242, &Some("t798".to_string()), "t798__B")
}

#[test]
fn job_identity_names_the_session_and_its_port() {
    let identity = identity_b();
    let get = |name: &str| {
        identity
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(
        get("TMUX"),
        Some(crate::pane::tmux_env_value(Some(4242), Some("t798")))
    );
    assert_eq!(get("MSYS2_ENV_CONV_EXCL"), Some("TMUX".to_string()));
    assert_eq!(get("PSMUX_TARGET_SESSION"), Some("t798__B".to_string()));
    assert_eq!(identity.len(), 3);
}

#[test]
fn identity_replaces_names_of_any_case_once_each() {
    let env = os_pairs(&[
        ("tmux", "/tmp/psmux-1/t798,1111,0"),
        ("TMUX", "/tmp/psmux-2/t798,2222,0"),
        ("Psmux_Target_Session", "t798__A"),
        ("msys2_env_conv_excl", "OTHER"),
        ("PATH", "C:\\bin"),
    ]);
    let out = with_session_identity(env, &identity_b());
    for (name, value) in identity_b() {
        assert_eq!(
            values_of(&out, &name),
            vec![OsString::from(value)],
            "{name}"
        );
    }
    assert_eq!(values_of(&out, "PATH"), vec![OsString::from("C:\\bin")]);
    assert_eq!(out.len(), 4);
}

#[test]
fn identity_is_added_when_the_environment_lacks_it() {
    let out = with_session_identity(os_pairs(&[("PATH", "C:\\bin")]), &identity_b());
    for (name, value) in identity_b() {
        assert_eq!(
            values_of(&out, &name),
            vec![OsString::from(value)],
            "{name}"
        );
    }
    assert_eq!(out.len(), 4);
}

#[cfg(windows)]
#[test]
fn values_outside_the_identity_are_kept_byte_for_byte() {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let odd = OsString::from_wide(&[0x0041, 0xD800, 0x0042]);
    let env = vec![
        (OsString::from("ODD895"), odd.clone()),
        (OsString::from("TMUX"), OsString::from("x")),
    ];
    let out = with_session_identity(env, &identity_b());
    let kept = values_of(&out, "ODD895");
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept[0].encode_wide().collect::<Vec<u16>>(),
        odd.encode_wide().collect::<Vec<u16>>()
    );
}

struct VarBack(&'static str, Option<OsString>);
impl Drop for VarBack {
    fn drop(&mut self) {
        match self.1.take() {
            Some(value) => std::env::set_var(self.0, value),
            None => std::env::remove_var(self.0),
        }
    }
}

#[test]
fn the_reply_carries_the_identity_and_never_the_socket_path() {
    use base64::Engine;
    let _lock = crate::util::lock_test_env();
    let _socket = VarBack(
        crate::socket_path::SOCKET_PATH_ENV,
        std::env::var_os(crate::socket_path::SOCKET_PATH_ENV),
    );
    let _target = VarBack(
        "PSMUX_TARGET_SESSION",
        std::env::var_os("PSMUX_TARGET_SESSION"),
    );
    std::env::set_var(crate::socket_path::SOCKET_PATH_ENV, "C:\\t798\\sock");
    let ns = Some("t798".to_string());
    let mut ids = ServerIds::new();
    let mut session_a = session("A");
    session_a.app.control_port = Some(1111);
    let reply = std::cell::RefCell::new(String::new());
    run_request(&mut session_a, &mut ids, &mut ServerBuffers::new(), &ns, |_app, _lp| {
        *reply.borrow_mut() = job_environ_reply(&identity_b());
        Ok(Flow::Continue)
    });
    let reply = reply.into_inner();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(reply.as_bytes())
        .expect("base64");
    let env = crate::client_env::decode_env_block(&bytes).expect("env block");
    for (name, value) in identity_b() {
        assert_eq!(
            values_of(&env, &name),
            vec![OsString::from(value)],
            "{name}"
        );
    }
    assert!(values_of(&env, crate::socket_path::SOCKET_PATH_ENV).is_empty());
}

#[cfg(windows)]
const T895_WORKER: &str =
    "server::shared::server::tests_issue798_shared_server_requests::job_context_order_worker";
#[cfg(windows)]
static T895_REAL: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static T895_RELEASE_TIMED_OUT: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static T895_RELEASE: std::sync::Mutex<Option<std::sync::mpsc::Receiver<()>>> =
    std::sync::Mutex::new(None);
#[cfg(windows)]
static T895_HOOK_THREAD: std::sync::Mutex<Option<std::thread::ThreadId>> =
    std::sync::Mutex::new(None);

#[cfg(windows)]
fn t895_hook() -> Option<crate::format::JobContext> {
    *T895_HOOK_THREAD.lock().unwrap() = Some(std::thread::current().id());
    let captured = if T895_REAL.load(Ordering::SeqCst) {
        capture_job_context()
    } else {
        None
    };
    let release = T895_RELEASE
        .lock()
        .unwrap()
        .take()
        .expect("a release receiver for this case");
    Some(Box::new(move |command: &mut std::process::Command| {
        if release
            .recv_timeout(std::time::Duration::from_secs(15))
            .is_err()
        {
            T895_RELEASE_TIMED_OUT.store(true, Ordering::SeqCst);
            panic!("#895 test: no release within 15 s");
        }
        if let Some(apply) = captured {
            apply(command);
        }
    }))
}

#[cfg(windows)]
fn t895_enter(value: &str, only_b: Option<&str>, port: u16, dir: &std::path::Path) {
    std::env::set_var("T895_V", value);
    match only_b {
        Some(v) => std::env::set_var("T895_BONLY", v),
        None => std::env::remove_var("T895_BONLY"),
    }
    std::env::set_var("PSMUX_TARGET_SESSION", format!("t895__{value}"));
    crate::commands::publish_job_env(Some(port), Some("t895"));
    std::env::set_current_dir(dir).expect("enter the session directory");
}

#[cfg(windows)]
#[test]
#[ignore = "run by job_context_order in a child process"]
fn job_context_order_worker() {
    use std::io::Write as _;
    let Some(root) = std::env::var_os("PSMUX_T895_WORKER") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let mut result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("result.txt"))
        .expect("result file");
    let mut record = |line: &str| {
        writeln!(result, "{line}").expect("result line");
        result.flush().expect("result flush");
    };
    let probe = root.join("probe.cmd");
    std::fs::write(
        &probe,
        "@echo V=%T895_V%;B=%T895_BONLY%;D=%CD%;T=%PSMUX_TARGET_SESSION%;X=%TMUX%\r\n",
    )
    .expect("probe.cmd");
    let (dir_a, dir_b) = (root.join("dirA"), root.join("dirB"));
    std::fs::create_dir_all(&dir_a).expect("dirA");
    std::fs::create_dir_all(&dir_b).expect("dirB");
    let (port_a, port_b) = (41895, 42895);
    let tmux_a = crate::pane::tmux_env_value(Some(port_a), Some("t895"));
    crate::format::JOB_CONTEXT
        .set(t895_hook)
        .expect("JOB_CONTEXT is unset in the worker process");
    let format = format!("#({})", probe.display());
    for (case, real) in [("control", false), ("real", true)] {
        t895_enter("A", None, port_a, &dir_a);
        let (release, waiter) = std::sync::mpsc::channel();
        *T895_RELEASE.lock().unwrap() = Some(waiter);
        *T895_HOOK_THREAD.lock().unwrap() = None;
        T895_REAL.store(real, Ordering::SeqCst);
        let mut app = AppState::new("t895".to_string());
        app.status_interval = 1;
        let (tx, rx) = std::sync::mpsc::channel();
        app.format_job_tx = Some(tx);
        {
            let _async = crate::format::AsyncFormatGuard::new();
            crate::format::expand_format(&format, &app);
        }
        let hook_thread = *T895_HOOK_THREAD.lock().unwrap();
        if hook_thread != Some(std::thread::current().id()) {
            record(&format!("RESULT {case} fail hook thread {hook_thread:?}"));
            panic!("{case}: the hook did not run on the expanding thread: {hook_thread:?}");
        }
        t895_enter("B", Some("b"), port_b, &dir_b);
        release.send(()).expect("release");
        let received = rx.recv_timeout(std::time::Duration::from_secs(20));
        if let Err(e) = &received {
            let timed_out = T895_RELEASE_TIMED_OUT.load(Ordering::SeqCst);
            record(&format!(
                "RESULT {case} fail no result ({e}, release timed out: {timed_out})"
            ));
            panic!("{case}: no job result ({e}, release timed out: {timed_out})");
        }
        let (_, value) = received.expect("checked above");
        record(&format!("CASE {case} {value}"));
        let ok = if real {
            value == format!("V=A;B=;D={};T=t895__A;X={tmux_a}", dir_a.display())
        } else {
            value.starts_with(&format!("V=B;B=b;D={};T=t895__B;X=", dir_b.display()))
        };
        if !ok {
            record(&format!("RESULT {case} fail unexpected value"));
            panic!("{case}: unexpected job value {value}");
        }
        record(&format!("RESULT {case} ok"));
    }
    record("DONE 2");
}

#[cfg(windows)]
#[test]
fn job_context_order() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let root = std::env::temp_dir().join(format!("psmux_t895_{}_{unique}", std::process::id()));
    assert!(
        !root.to_string_lossy().chars().any(char::is_whitespace),
        "set TEMP to a folder without spaces (cmd /C runs the probe path): {}",
        root.display()
    );
    std::fs::create_dir_all(&root).expect("temp folder");
    let stdout = std::fs::File::create(root.join("child-out.txt")).expect("child-out.txt");
    let stderr = std::fs::File::create(root.join("child-err.txt")).expect("child-err.txt");
    let mut child = std::process::Command::new(std::env::current_exe().expect("test exe"))
        .args([
            "--exact",
            T895_WORKER,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("PSMUX_T895_WORKER", &root)
        .stdin(std::process::Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("spawn the worker");
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            break Some(status);
        }
        if started.elapsed() > std::time::Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let read = |name: &str| {
        let text = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        text.chars().take(4000).collect::<String>()
    };
    let result = read("result.txt");
    let report = format!(
        "worker status {status:?}\nresult.txt:\n{result}\nchild stdout:\n{}\nchild stderr:\n{}",
        read("child-out.txt"),
        read("child-err.txt")
    );
    let lines: Vec<&str> = result.lines().collect();
    let passed = status.is_some_and(|s| s.success())
        && ["RESULT control ok", "RESULT real ok", "DONE 2"]
            .iter()
            .all(|want| lines.contains(want));
    if let Err(e) = std::fs::remove_dir_all(&root) {
        eprintln!("could not remove {}: {e}", root.display());
    }
    assert!(passed, "{report}");
}
