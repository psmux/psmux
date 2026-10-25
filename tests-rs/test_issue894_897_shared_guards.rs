// Issues #894 and #897 (shared server, #798 step 2): the loop's own per
// session work is guarded like a request, a stale namespace marker is never
// deleted by a reader, the namespace lock cannot be a session's lock, and
// every command line of a connection is checked, aliases applied.
// No server, socket or pane is started.

use super::*;

fn session(name: &str, id: usize) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("t894".to_string());
    app.session_id = id;
    Session {
        app,
        lp: SessionLoop::new(Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()))),
        cwd: None,
        port: 0,
        stop: Arc::new(AtomicBool::new(false)),
        _guard: None,
    }
}

// #894: a panic in the data ready or device reply pass of ONE session used to
// unwind the loop thread and end the server with every session in it.
#[test]
fn a_panic_in_the_loops_per_session_work_ends_only_that_session() {
    let mut sessions: BTreeMap<usize, Session> = BTreeMap::new();
    for (id, name) in [(1usize, "A"), (2, "B"), (3, "C")] {
        sessions.insert(id, session(name, id));
    }
    let mut ran: Vec<String> = Vec::new();
    let ended = for_each_session_guarded(&mut sessions, |app, lp| {
        ran.push(app.session_name.clone());
        lp.state_dirty = true;
        if app.session_name == "B" {
            panic!("injected #894 test panic");
        }
    });
    assert_eq!(ended, vec![2], "only the panicking session is to end");
    assert_eq!(ran, vec!["A", "B", "C"], "the sessions after it still ran");
    assert!(sessions[&3].lp.state_dirty);
}

#[test]
fn the_loops_per_session_work_names_the_session_for_the_crash_report() {
    let mut sessions: BTreeMap<usize, Session> = BTreeMap::new();
    sessions.insert(7, session("named", 7));
    let mut seen = String::new();
    for_each_session_guarded(&mut sessions, |_app, _lp| {
        seen = CURRENT.lock().map(|g| g.clone()).unwrap_or_default();
    });
    assert_eq!(seen, "t894__named");
}

#[test]
fn the_guarded_new_session_output_is_the_plain_one() {
    let app = AppState::new("made".to_string());
    let args = NewSessionArgs { print: true, ..Default::default() };
    assert!(new_session_output_guarded(&app, &args).starts_with("session=made\n"));
    let quiet = NewSessionArgs::default();
    assert_eq!(new_session_output_guarded(&app, &quiet), "session=made");
}

// #897 A: `live_marker` removed a stale marker. Between its read and that
// removal another CLI can start the namespace's server, whose marker then
// went instead, and every later new-session in the namespace timed out.
struct DataDirBack(Option<std::ffi::OsString>);
impl Drop for DataDirBack {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
            None => std::env::remove_var("PSMUX_DATA_DIR"),
        }
    }
}

#[test]
fn a_reader_never_deletes_a_stale_marker() {
    let _lock = crate::util::lock_test_env();
    let _back = DataDirBack(std::env::var_os("PSMUX_DATA_DIR"));
    let dir = std::env::temp_dir().join(format!("psmux_t897a_{}_{}", std::process::id(), random_key()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("PSMUX_DATA_DIR", &dir);
    let ns = Some("t897a");
    let path = registry::marker_path(ns);
    // This process, with a creation time it never had: not the server.
    let stale = registry::Marker { pid: std::process::id(), creation: 1, port: 1, key: "stale".into() };
    registry::write_marker(&path, &stale).unwrap();
    assert_eq!(registry::live_marker(ns), None, "a stale marker is not a live server");
    // In the race the file the reader would delete is by now the new
    // server's marker, so reading must leave it where it is.
    assert!(path.exists(), "a stale marker is left for the next server to replace");
    // The next server renames its marker over it.
    let me = std::process::id();
    let live = registry::Marker {
        pid: me,
        creation: crate::platform::process_kill::process_creation_time(me).unwrap_or(0),
        port: 2,
        key: "live".into(),
    };
    registry::write_marker(&path, &live).unwrap();
    if live.creation != 0 {
        assert_eq!(registry::live_marker(ns), Some(live.clone()));
        assert_eq!(registry::live_marker(ns), Some(live), "reading twice keeps it");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// #897 C: the namespace lock was `<ns>@shared-server` under the per session
// lock prefix, which is exactly the lock of session `y@shared-server` in
// `-L x` when the namespace is `x__y`.
#[test]
fn the_namespace_lock_is_never_a_session_lock() {
    let tag = "tag";
    for ns in [None, Some("x__y"), Some("x"), Some("default"), Some("a\\b"), Some("a/b")] {
        let name = namespace_lock_name(ns, tag);
        assert!(name.starts_with("Local\\psmux-shared-server-tag-"), "{name}");
        assert!(!name.starts_with("Local\\psmux-session-"), "{name}");
        assert!(!name["Local\\".len()..].contains('\\'), "no backslash past the prefix: {name}");
    }
    // The step 2 collision: shared -L x__y against per session x / y@shared-server.
    assert_ne!(
        namespace_lock_name(Some("x__y"), tag),
        format!("Local\\psmux-session-{}-{}", tag, "x__y@shared-server")
    );
    // Namespaces never share a lock.
    let names: std::collections::HashSet<String> = [None, Some("default"), Some("a\\b"), Some("a/b"), Some("a%5Cb"), Some("a_b")]
        .into_iter()
        .map(|ns| namespace_lock_name(ns, tag))
        .collect();
    assert_eq!(names.len(), 6);
}

// #897 B: the router checked the name before alias expansion.
#[test]
fn an_alias_cannot_carry_a_refused_command_in() {
    let mut map = std::collections::HashMap::new();
    map.insert("list-tree".to_string(), "resize-pane -Z".to_string());
    map.insert("lw".to_string(), "list-windows -a".to_string());
    let lookup = |n: &str| map.get(n).cloned();
    assert_eq!(router::first_unsupported_aliased("list-tree", lookup), Some("resize-pane".into()));
    assert_eq!(router::first_unsupported_aliased("lw", lookup), None);
    assert_eq!(router::first_unsupported_aliased("send-keys x ; list-tree", lookup), Some("resize-pane".into()));
    // No alias: the plain check, unchanged.
    assert_eq!(router::first_unsupported("list-tree"), None);
    assert_eq!(router::first_unsupported("resize-pane -Z"), Some("resize-pane".into()));
}

#[test]
fn an_undecided_opening_is_gated_only_after_a_good_auth() {
    assert!(router::opening_authenticates(b"AUTH k1\n", "k1"));
    assert!(router::opening_authenticates(b"AUTH k1\r\n   ", "k1"));
    let mut long = b"AUTH k1\n".to_vec();
    long.extend(std::iter::repeat(b' ').take(17000));
    assert!(router::opening_authenticates(&long, "k1"));
    assert!(!router::opening_authenticates(b"AUTH k1", "k1"), "the AUTH line is not whole yet");
    assert!(!router::opening_authenticates(b"AUTH k2\n", "k1"));
    assert!(!router::opening_authenticates(b"HELLO\n", "k1"));
}
