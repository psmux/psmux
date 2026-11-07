// Issue #1005: a target attached to its flag (`-t:0`, `-tA:0`) from a hook,
// a key binding, a mouse binding or the command prompt applied to the ACTIVE
// window. tmux's getopt reads `-tVALUE` as `-t VALUE` everywhere
// (arguments.c args_parse_flag_argument); the CLI split it (main.rs), the in
// server route and the server's command handler did not, and their arms match
// the `-t` token alone. Registered from src/commands.rs.

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

/// A private data dir for the session registry, restored on drop. Held with
/// the shared env lock because PSMUX_DATA_DIR is process wide.
struct DataDir {
    old: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
}

impl DataDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("psmux_1005_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var_os("PSMUX_DATA_DIR");
        std::env::set_var("PSMUX_DATA_DIR", &dir);
        DataDir { old, dir }
    }

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

/// Session "A" with windows 0, 1 and 2, window 1 current, whose own server
/// is `own`.
fn session_a(own: &std::net::TcpListener) -> AppState {
    let mut app = AppState::new("A".to_string());
    app.windows = vec![make_window("zero", 10), make_window("one", 11), make_window("two", 12)];
    app.window_base_index = 0;
    app.active_idx = 1;
    app.control_port = Some(own.local_addr().unwrap().port());
    app.session_key = "keyA".to_string();
    app
}

/// The lines each connection to `l` carried, in order, until none is left.
fn lines_sent(l: &std::net::TcpListener) -> Vec<String> {
    let mut lines = Vec::new();
    loop {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        let mut stream = loop {
            match l.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return lines,
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(1))).unwrap();
        let mut text = String::new();
        let _ = std::io::Read::read_to_string(&mut stream, &mut text);
        lines.extend(text.lines().map(str::to_string));
    }
}

fn names(app: &AppState) -> Vec<&str> {
    app.windows.iter().map(|w| w.name.as_str()).collect()
}

fn words(line: &[&str]) -> Vec<String> {
    line.iter().map(|s| s.to_string()).collect()
}

/// What `line` sends to session A's own server, and the window names after.
fn run_on_a(line: &str) -> (Vec<String>, Vec<String>) {
    let own = listener();
    let mut app = session_a(&own);
    execute_command_string(&mut app, line).unwrap();
    let sent = lines_sent(&own);
    (sent, names(&app).into_iter().map(str::to_string).collect())
}

#[test]
fn rename_window_attached_target_goes_out_as_the_spaced_one() {
    // Master: `-t:0` was no exact `-t`, so the arm renamed the ACTIVE window
    // 1 in place and sent a bare `rename-window X` to the server.
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("rename");
    let spaced = run_on_a("rename-window -t :0 X");
    assert!(spaced.0.iter().any(|l| l.starts_with("rename-window -t ")), "spaced form sent {:?}", spaced.0);
    assert_eq!(spaced.1, vec!["zero", "one", "two"], "the spaced form leaves the rename to the server");
    assert_eq!(run_on_a("rename-window -t:0 X"), spaced);
    assert_eq!(run_on_a("rename-window -tA:0 X"), run_on_a("rename-window -t A:0 X"));
}

#[test]
fn attached_targets_in_a_command_list() {
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("list");
    assert_eq!(
        run_on_a("rename-window -t:0 X ; rename-window -t:2 Y"),
        run_on_a("rename-window -t :0 X ; rename-window -t :2 Y")
    );
}

#[test]
fn operand_after_double_dash_is_kept() {
    let _env = crate::util::lock_test_env();
    let _data = DataDir::new("dashdash");
    assert_eq!(run_on_a("rename-window -t:0 -- -tz"), run_on_a("rename-window -t :0 -- -tz"));
}

#[test]
fn kill_session_attached_target_reaches_the_named_session() {
    // Master: `kill-session -tK2` from the prompt ended the current session.
    let _env = crate::util::lock_test_env();
    let data = DataDir::new("kill");
    let (own, other) = (listener(), listener());
    data.register("K2", &other, "keyK2");
    let mut app = session_a(&own);
    execute_command_string(&mut app, "kill-session -tK2").unwrap();
    assert_eq!(lines_sent(&other), vec!["AUTH keyK2".to_string(), "kill-session -t K2".to_string()]);
    assert_eq!(lines_sent(&own), Vec::<String>::new(), "A's own server was sent nothing");
}

#[test]
fn handler_words_split_an_attached_target() {
    // The server's command handler (connection.rs) takes these words.
    assert_eq!(
        crate::cli::normalize_command_words(words(&["kill-session", "-tK2"]), true),
        words(&["kill-session", "-t", "K2"])
    );
    assert_eq!(
        crate::cli::normalize_command_words(words(&["rename-window", "-tA:0", "X"]), true),
        words(&["rename-window", "-t", "A:0", "X"])
    );
    // `-t=NAME` (#196) still works and an already split line is unchanged.
    assert_eq!(
        crate::cli::normalize_command_words(words(&["kill-session", "-t=K2"]), true),
        words(&["kill-session", "-t", "K2"])
    );
    assert_eq!(
        crate::cli::normalize_command_words(words(&["rename-window", "-t", ":0", "X"]), true),
        words(&["rename-window", "-t", ":0", "X"])
    );
}

#[test]
fn handler_words_keep_values_and_operands() {
    // A value of another flag and anything after the first operand is data.
    assert_eq!(
        crate::cli::normalize_command_words(words(&["display-message", "-F", "-tx", "-t:1", "hi"]), true),
        words(&["display-message", "-F", "-tx", "-t", ":1", "hi"])
    );
    assert_eq!(
        crate::cli::normalize_command_words(words(&["send-keys", "-t:1", "hi", "-tliteral"]), true),
        words(&["send-keys", "-t", ":1", "hi", "-tliteral"])
    );
    assert_eq!(
        crate::cli::normalize_command_words(words(&["set-option", "@k", "-tfoo"]), true),
        words(&["set-option", "@k", "-tfoo"])
    );
    // A binding's command is the binding's own line, read when the key runs.
    assert_eq!(
        crate::cli::normalize_command_words(words(&["bind-key", "-n", "F5", "rename-window", "-t:0", "B"]), true),
        words(&["bind-key", "-n", "F5", "rename-window", "-t:0", "B"])
    );
}

#[test]
fn handler_words_read_only_the_first_command() {
    // The list was split before: a word ending in `;` is the message here.
    let line = words(&["display-message", "-p", "a;", "rename-window", "-tz"]);
    assert_eq!(crate::cli::normalize_command_words(line.clone(), true), line);
    // A name the flag table does not know (an alias) is expanded first.
    let alias = words(&["myalias", "-tz", "x"]);
    assert_eq!(crate::cli::normalize_command_words(alias.clone(), true), alias);
    // Without `equals` (the in server route) `-t=K2` is tmux's reading,
    // `-t =K2`, the exact match target its resolver knows.
    assert_eq!(
        crate::cli::normalize_command_words(words(&["kill-session", "-t=K2"]), false),
        words(&["kill-session", "-t", "=K2"])
    );
}
