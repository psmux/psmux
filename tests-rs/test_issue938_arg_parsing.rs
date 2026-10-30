// Issue #938: the in server route (hooks, key bindings, menus, the command
// prompt) read a window command's flags as exact tokens and ran a line that
// names another session on its own session. tmux reads every command line
// with args_parse (arguments.c:207-252: a cluster letter by letter, a value
// flag takes the rest of its word or the next one) and resolves the target
// before the command runs (cmd-queue.c:524, cmd_find_target), on the server
// that holds the named session. Registered from src/commands.rs.

use super::*;

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// Session "A" with windows 0, 1 and 2, window 2 current and window 1 the
/// last one (visited 0, 1, 2 in that order).
fn three_windows() -> AppState {
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window("zero", 10), make_window("one", 11), make_window("two", 12)];
    app.window_base_index = 0;
    app.active_idx = 2;
    app.last_window_idx = 1;
    app
}

fn window_ids(app: &AppState) -> Vec<usize> {
    app.windows.iter().map(|w| w.id).collect()
}

#[test]
fn clustered_kill_window_a_keeps_the_target_and_not_the_active_window() {
    // Master: `-at` was no exact `-t`, so the arm killed the ACTIVE window 2.
    let mut app = three_windows();
    execute_command_string(&mut app, "kill-window -at :1").unwrap();
    assert_eq!(window_ids(&app), vec![11], "only window 1 is left");
    assert_eq!(app.active_idx, 0);
}

#[test]
fn kill_window_a_apart_keeps_the_target() {
    // Master: -a was ignored and window 1, the one to keep, was killed.
    let mut app = three_windows();
    execute_command_string(&mut app, "kill-window -a -t :1").unwrap();
    assert_eq!(window_ids(&app), vec![11]);
}

#[test]
fn kill_window_a_without_target_keeps_the_current_window() {
    let mut app = three_windows();
    execute_command_string(&mut app, "kill-window -a").unwrap();
    assert_eq!(window_ids(&app), vec![12]);
}

#[test]
fn kill_window_a_on_a_lone_window_kills_nothing() {
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window("only", 10)];
    execute_command_string(&mut app, "kill-window -a").unwrap();
    assert_eq!(window_ids(&app), vec![10]);
}

#[test]
fn kill_window_without_a_still_kills_the_target() {
    let mut app = three_windows();
    execute_command_string(&mut app, "kill-window -t :1").unwrap();
    assert_eq!(window_ids(&app), vec![10, 12]);
}

#[test]
fn clustered_select_window_l_moves_to_the_last_window() {
    // Master: `-lt` was dropped and window 2 stayed current.
    let mut app = three_windows();
    execute_command_string(&mut app, "select-window -lt :0").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn select_window_l_apart_ignores_the_window_part() {
    // Master: -l was dropped on this route and `-t :0` selected window 0.
    let mut app = three_windows();
    execute_command_string(&mut app, "select-window -l -t :0").unwrap();
    assert_eq!(app.active_idx, 1);
}

#[test]
fn select_window_n_and_p_move_within_the_session() {
    let mut app = three_windows();
    execute_command_string(&mut app, "select-window -n -t :1").unwrap();
    assert_eq!(app.active_idx, 0, "-n wraps from window 2 to window 0");
    execute_command_string(&mut app, "select-window -p").unwrap();
    assert_eq!(app.active_idx, 2);
}

#[test]
fn plain_select_window_t_still_selects_the_window() {
    let mut app = three_windows();
    execute_command_string(&mut app, "select-window -t :0").unwrap();
    assert_eq!(app.active_idx, 0);
}

// ---- a target naming another session -------------------------------------

/// A private data dir for the registry, restored and removed on drop. Held
/// with the shared env lock because PSMUX_DATA_DIR is process wide.
struct DataDir {
    old: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
}

impl DataDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("psmux_938_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("PSMUX_DATA_DIR");
        std::env::set_var("PSMUX_DATA_DIR", &dir);
        DataDir { old, dir }
    }

    /// Register a session `base` whose server is `listener`, with `key`.
    fn register(&self, base: &str, listener: &std::net::TcpListener, key: &str) {
        let port = listener.local_addr().unwrap().port();
        std::fs::write(self.dir.join(format!("{}.port", base)), port.to_string()).unwrap();
        std::fs::write(self.dir.join(format!("{}.key", base)), key).unwrap();
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        match self.old.take() {
            Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
            None => std::env::remove_var("PSMUX_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn listener() -> std::net::TcpListener {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    l
}

/// Session "A" whose own server is `own`.
fn session_a(own: &std::net::TcpListener) -> AppState {
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window("zero", 10), make_window("one", 11)];
    app.control_port = Some(own.local_addr().unwrap().port());
    app.session_key = "keyA".to_string();
    app
}

/// The lines one connection to `l` carried, or None when nothing connected.
fn lines_sent(l: &std::net::TcpListener) -> Option<Vec<String>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let mut stream = loop {
        match l.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    };
    stream.set_nonblocking(false).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
    let mut text = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut text);
    Some(text.lines().map(str::to_string).collect())
}

fn status(app: &AppState) -> Option<&str> {
    app.status_message.as_ref().map(|(t, _, _)| t.as_str())
}

#[test]
fn hook_kill_session_naming_another_session_reaches_that_server() {
    // Master: a bare `kill-session` went to A's own server and ended A.
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("kill");
    let (own, other) = (listener(), listener());
    data.register("B", &other, "keyB");
    let mut app = session_a(&own);
    execute_command_string(&mut app, "kill-session -t B").unwrap();
    assert_eq!(lines_sent(&other), Some(vec!["AUTH keyB".to_string(), "kill-session -t B".to_string()]));
    assert_eq!(lines_sent(&own), None, "A's own server was sent nothing");
}

#[test]
fn hook_set_option_naming_another_session_reaches_that_server() {
    // Master: the line ran on A and set @x on A.
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("set");
    let (own, other) = (listener(), listener());
    data.register("B", &other, "keyB");
    let mut app = session_a(&own);
    execute_command_string(&mut app, "set -t B @x 1").unwrap();
    assert_eq!(lines_sent(&other), Some(vec!["AUTH keyB".to_string(), "set -t B @x 1".to_string()]));
    assert_eq!(lines_sent(&own), None);
    assert!(app.user_options.get("@x").is_none(), "@x is not set on A");
}

#[test]
fn explicit_session_part_that_does_not_exist_is_an_error() {
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("missing");
    let own = listener();
    let mut app = session_a(&own);
    execute_command_string(&mut app, "set -t D: @x 1").unwrap();
    assert_eq!(status(&app), Some("can't find session: D"));
    assert_eq!(lines_sent(&own), None);
    assert!(app.user_options.get("@x").is_none());
}

#[test]
fn attached_kill_session_target_that_does_not_exist_ends_nothing() {
    // Master: `-tD` was no exact `-t`, so the arm ended A.
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("attached");
    let own = listener();
    let mut app = session_a(&own);
    execute_command_string(&mut app, "kill-session -tD").unwrap();
    assert_eq!(status(&app), Some("can't find session: D"));
    assert_eq!(lines_sent(&own), None);
}

#[test]
fn forward_stays_in_this_servers_namespace() {
    // `-L ns`: B is ns__B. The default namespace's B in the same data dir is
    // another server and must not be reached.
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("ns");
    let (own, default_b, ns_b) = (listener(), listener(), listener());
    data.register("B", &default_b, "keyDefault");
    data.register("ns__B", &ns_b, "keyNs");
    let mut app = session_a(&own);
    app.socket_name = Some("ns".to_string());
    execute_command_string(&mut app, "kill-session -t B").unwrap();
    assert_eq!(lines_sent(&ns_b), Some(vec!["AUTH keyNs".to_string(), "kill-session -t B".to_string()]));
    assert_eq!(lines_sent(&default_b), None, "the default namespace's B was reached");
    assert_eq!(lines_sent(&own), None);
}

#[test]
fn own_session_is_not_forwarded() {
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("own");
    let own = listener();
    let mut app = session_a(&own);
    execute_command_string(&mut app, "kill-session -t A").unwrap();
    assert_eq!(lines_sent(&own), Some(vec!["AUTH keyA".to_string(), "kill-session".to_string()]));
}

#[test]
fn bare_name_of_a_local_window_stays_local_even_when_a_session_has_it() {
    // tmux reads a bare window target in the current session first
    // (cmd-find.c:1093-1110): window "one" here, not session "one".
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("bare");
    let (own, other) = (listener(), listener());
    data.register("one", &other, "keyOne");
    let mut app = session_a(&own);
    app.control_port = None;
    execute_command_string(&mut app, "kill-window -t one").unwrap();
    assert_eq!(window_ids(&app), vec![10], "window one of A was killed");
    assert_eq!(lines_sent(&other), None);
}

#[test]
fn kill_session_a_is_not_forwarded_to_the_kept_session() {
    // `-a -t B` keeps B; sending it to B's server would end B.
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("killa");
    let (own, other) = (listener(), listener());
    data.register("B", &other, "keyB");
    let mut app = session_a(&own);
    execute_command_string(&mut app, "kill-session -a -t B").unwrap();
    assert_eq!(lines_sent(&other), None);
}
