// Issue #851: run-shell without -b waits for its command, and the commands
// after it wait with it (cmd-run-shell.c:123 and :186 to :188, tag 3.7c); -d
// delay waits that many seconds first (:125 to :131 and :178 to :184). psmux
// started a run-shell in a config file or a sourced file without waiting,
// whatever its flags, and did not read -d at all, so it went to the shell.
//
// These tests pin how the flags are read and which run-shell lines a runtime
// source hands to its job, which runs them on a one-shot connection and waits.
// The routes end to end are in tests/test_issue851_run_shell_wait.ps1.

use super::*;
use crate::commands::{parse_run_shell_args, RunShellArgs};
use crate::types::{DeferredConfigLine, DeferredKind};
use std::time::Duration;

fn args(s: &[&str]) -> Result<RunShellArgs, String> {
    parse_run_shell_args(s)
}

fn words(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
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

/// A server with one window, as a runtime source-file finds it.
fn app_with_window() -> AppState {
    let mut app = AppState::new("rsw".to_string());
    app.windows.push(make_window("shell", 0));
    app
}

/// A file of its own for each test, removed when the guard drops.
struct TempConf(std::path::PathBuf);
impl TempConf {
    fn new(tag: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "psmux_test_rsw_{}_{}_{}.conf",
            std::process::id(),
            tag,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::write(&path, content).unwrap();
        TempConf(path)
    }
    fn path(&self) -> String {
        self.0.display().to_string()
    }
}
impl Drop for TempConf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn kinds(lines: &[DeferredConfigLine]) -> Vec<DeferredKind> {
    lines.iter().map(|l| l.kind.clone()).collect()
}

fn command(s: &str) -> DeferredKind {
    DeferredKind::Command(s.to_string())
}

fn directive(s: &str) -> DeferredKind {
    DeferredKind::Directive(s.to_string())
}

// ── the flags ───────────────────────────────────────────────────────────────

#[test]
fn b_is_background_and_the_rest_is_the_command() {
    let a = args(&["-b", "echo", "hi"]).unwrap();
    assert!(a.background);
    assert_eq!(a.delay, None);
    assert_eq!(a.command, words(&["echo", "hi"]));
}

#[test]
fn d_takes_its_value_from_the_next_word_or_its_own() {
    assert_eq!(args(&["-d", "2", "echo"]).unwrap().delay, Some(Duration::from_secs(2)));
    assert_eq!(args(&["-d2", "echo"]).unwrap().delay, Some(Duration::from_secs(2)));
    assert_eq!(args(&["-d", "0.5", "echo"]).unwrap().delay, Some(Duration::from_millis(500)));
    assert_eq!(args(&["-d", "2", "echo"]).unwrap().command, words(&["echo"]));
}

#[test]
fn flags_can_share_a_word() {
    let a = args(&["-bd", "1", "echo"]).unwrap();
    assert!(a.background);
    assert_eq!(a.delay, Some(Duration::from_secs(1)));
    assert_eq!(a.command, words(&["echo"]));
}

#[test]
fn a_delay_that_is_not_a_number_is_an_error() {
    assert_eq!(args(&["-d", "soon", "echo"]).unwrap_err(), "invalid delay time: soon");
    assert_eq!(args(&["-db", "echo"]).unwrap_err(), "invalid delay time: b");
    assert!(args(&["-d"]).is_err(), "-d needs a value");
}

#[test]
fn d_with_no_command_only_waits() {
    let a = args(&["-d", "1"]).unwrap();
    assert_eq!(a.delay, Some(Duration::from_secs(1)));
    assert!(a.command.is_empty());
}

#[test]
fn a_delay_too_long_for_a_duration_is_the_longest_one_not_a_panic() {
    // Duration::from_secs_f64 panics at 2^64 seconds and up. On the server
    // thread (a sourced file's run-shell line) that ended the server.
    for v in ["1e20", "18446744073709551616", "1e308"] {
        let a = args(&["-d", v]).unwrap();
        assert_eq!(a.delay, Some(Duration::MAX), "-d {}", v);
    }
    assert_eq!(args(&["-bd", "1e20", "echo"]).unwrap().delay, Some(Duration::MAX));
    // The runtime source check reads the same flags and must not panic either.
    assert!(run_shell_waits("run-shell -d 1e20"));
    assert!(!run_shell_waits("run-shell -b -d 1e20 'echo hi'"));
    // A negative delay is no delay, as before.
    assert_eq!(args(&["-d", "-1", "echo"]).unwrap().delay, Some(Duration::ZERO));
}

#[test]
fn a_flag_psmux_does_not_read_starts_the_shell_command() {
    assert_eq!(args(&["-C", "display", "x"]).unwrap().command, words(&["-C", "display", "x"]));
    // Not a cluster of ours either: -b is not taken from `-bC`.
    let a = args(&["-bC", "x"]).unwrap();
    assert!(!a.background);
    assert_eq!(a.command, words(&["-bC", "x"]));
    // The shell command's own flags are left alone.
    assert_eq!(args(&["psmux", "set", "-g", "@o", "v"]).unwrap().command, words(&["psmux", "set", "-g", "@o", "v"]));
}

#[test]
fn a_later_b_is_still_the_flag_unless_dash_dash_ended_the_flags() {
    let a = args(&["echo", "-b"]).unwrap();
    assert!(a.background);
    assert_eq!(a.command, words(&["echo"]));
    let a = args(&["--", "echo", "-b"]).unwrap();
    assert!(!a.background);
    assert_eq!(a.command, words(&["echo", "-b"]));
}

// ── which lines wait ────────────────────────────────────────────────────────

#[test]
fn a_run_shell_without_b_waits() {
    assert!(run_shell_waits("run-shell \"ping -n 3 127.0.0.1\""));
    assert!(run_shell_waits("run 'echo hi'"));
    assert!(run_shell_waits("run-shell -d 2 'echo hi'"));
    assert!(run_shell_waits("run-shell -d 2"), "a delay alone waits");
}

#[test]
fn a_background_run_shell_or_a_bad_flag_does_not() {
    assert!(!run_shell_waits("run-shell -b 'echo hi'"));
    assert!(!run_shell_waits("run-shell -d soon 'echo hi'"));
    assert!(!run_shell_waits("set -g @a on"));
}

#[test]
fn a_background_run_shell_goes_to_the_job_but_a_bad_flag_does_not() {
    assert!(run_shell_in_job("run-shell -b 'echo hi'"));
    assert!(run_shell_in_job("run -b -d 2"));
    assert!(run_shell_in_job("run 'echo hi'"));
    assert!(!run_shell_in_job("run-shell -d soon 'echo hi'"));
    assert!(!run_shell_in_job("run-shell -b"), "the usage error stays with parse_run_shell");
    assert!(!run_shell_in_job("set -g @a on"));
}

// ── a runtime source ────────────────────────────────────────────────────────

#[test]
fn a_run_shell_starts_the_queue_and_the_next_line_waits_for_it() {
    let f = TempConf::new("first", "run-shell \"echo hi\"\nset -g @after on\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("run-shell \"echo hi\""), directive("set -g @after on")]);
    assert_eq!(app.user_options.get("@after"), None, "not before the shell has run");
}

#[test]
fn a_run_shell_after_a_command_is_a_command_too() {
    let f = TempConf::new("after", "display-message -p x\nrun-shell -d 1\nset -g @after on\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(
        kinds(&lines),
        vec![command("display-message -p x"), command("run-shell -d 1"), directive("set -g @after on")]
    );
}

#[test]
fn a_background_run_shell_after_a_command_is_a_command_too() {
    // It starts in file order through the dispatcher, as tmux's queue starts
    // it, and nothing waits for it.
    let f = TempConf::new("bg", "display-message -p x\nrun-shell -b 'echo hi'\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("display-message -p x"), command("run-shell -b 'echo hi'")]);
}

#[test]
fn a_background_run_shell_before_the_first_command_starts_in_place() {
    // Every line before it has applied already, which is tmux's order, and
    // the lines after it do not wait.
    let f = TempConf::new("bgfirst", "run-shell -b -d 5\nset -g @after on\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert!(lines.is_empty(), "{:?}", kinds(&lines));
    assert_eq!(app.user_options.get("@after").map(String::as_str), Some("on"));
}

#[test]
fn a_server_with_no_window_keeps_nothing() {
    // The startup drain and a warm standby have no source job, so a run-shell
    // there is started as before and nothing waits.
    let f = TempConf::new("nowin", "set -g @before on\nrun-shell -d 5\nset -g @after on\n");
    let mut app = AppState::new("rsw".to_string());
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert!(lines.is_empty());
    assert_eq!(app.user_options.get("@after").map(String::as_str), Some("on"));
}
