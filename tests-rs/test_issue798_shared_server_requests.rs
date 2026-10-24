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
    run_request(&mut session_a, &mut ids, &ns, |_app, _lp| {
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
