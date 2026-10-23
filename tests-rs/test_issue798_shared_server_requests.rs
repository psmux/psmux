// Issue #798 step 2: how the shared server runs one request of one session.
// No server, socket or pane is started: the sessions are bare AppStates.
// `enter` writes PSMUX_TARGET_SESSION and the job environment, so each test
// holds the process wide test env lock and puts the variable back.

use super::*;

fn session(name: &str) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("t798".to_string());
    Session {
        app,
        lp: SessionLoop::new(Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()))),
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
    let flow = run_request(&mut a, &mut ids, &ns, |app, _lp| {
        app.next_win_id += 1;
        app.next_pane_id += 2;
        panic!("injected #798 test panic");
    });
    assert_eq!(flow, Flow::EndSession, "the panicking session is quarantined");
    // The ids it took are not handed out again.
    assert_eq!((ids.next_window(), ids.next_pane()), (2, 3));
    // B runs normally afterwards, and continues the server wide sequence.
    let flow = run_request(&mut b, &mut ids, &ns, |app, _lp| {
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
        run_request(&mut sessions[i], &mut ids, &ns, |_app, _lp| {
            *seen.borrow_mut() = std::env::var("PSMUX_TARGET_SESSION").unwrap_or_default();
            Ok(Flow::Continue)
        });
        assert_eq!(seen.into_inner(), want);
    }
}

/// Run one show-buffer request of `s` through the shared dispatch.
fn show(s: &mut Session, ids: &mut ServerIds, ns: &Option<String>, name: Option<&str>) -> Result<Option<String>, std::sync::mpsc::TryRecvError> {
    let pty_system = portable_pty::native_pty_system();
    match name {
        None => {
            let (rtx, rrx) = std::sync::mpsc::channel::<String>();
            run_request(s, ids, ns, |app, lp| {
                crate::server::shared::dispatch::dispatch(app, lp, CtrlReq::ShowBuffer(rtx), &*pty_system)
            });
            rrx.try_recv().map(Some)
        }
        Some(name) => {
            let (rtx, rrx) = std::sync::mpsc::channel::<Option<String>>();
            run_request(s, ids, ns, |app, lp| {
                crate::server::shared::dispatch::dispatch(app, lp, CtrlReq::ShowNamedBuffer(rtx, name.to_string()), &*pty_system)
            });
            rrx.try_recv()
        }
    }
}

// #896: show-buffer answers from the requesting session's own buffers, as a
// per session server does (server/mod.rs ShowBuffer and ShowNamedBuffer).
#[test]
fn show_buffer_reads_the_requesting_sessions_buffers() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = Some("t798".to_string());
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let mut b = session("B");
    a.app.paste_add("hello".to_string());
    a.app.paste_set("kept", "named data".to_string());
    assert_eq!(show(&mut a, &mut ids, &ns, None), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut a, &mut ids, &ns, Some("buffer0")), Ok(Some("hello".to_string())));
    assert_eq!(show(&mut a, &mut ids, &ns, Some("kept")), Ok(Some("named data".to_string())));
    assert_eq!(show(&mut a, &mut ids, &ns, Some("nosuch")), Ok(None));
    // B holds no buffer: A's are not visible from it.
    assert_eq!(show(&mut b, &mut ids, &ns, None), Ok(Some(String::new())));
    assert_eq!(show(&mut b, &mut ids, &ns, Some("buffer0")), Ok(None));
    assert_eq!(show(&mut b, &mut ids, &ns, Some("kept")), Ok(None));
}

#[test]
fn an_io_error_keeps_the_session() {
    let _lock = crate::util::lock_test_env();
    let _back = EnvBack(std::env::var("PSMUX_TARGET_SESSION").ok());
    let ns = None;
    let mut ids = ServerIds::new();
    let mut a = session("A");
    let flow = run_request(&mut a, &mut ids, &ns, |_app, _lp| {
        Err(io::Error::new(io::ErrorKind::Other, "routine failure"))
    });
    assert_eq!(flow, Flow::Continue);
}
