// Issue #842: the job that runs a sourced file's kept lines in order
// (src/server/source_job.rs). A stand-in for the server records what the job
// asked for and in which order; exchange_on_port is checked against a real
// local listener.

use super::*;
use crate::types::{DeferredConfigLine, DeferredKind, SourceOutcome, SourceReplayResult};
use std::collections::HashMap;

fn line(file: &str, n: usize, kind: DeferredKind) -> DeferredConfigLine {
    DeferredConfigLine { file: file.to_string(), line: Some(n), kind, ancestors: Vec::new() }
}

fn cmd(s: &str) -> DeferredKind {
    DeferredKind::Command(s.to_string())
}

fn dir(s: &str) -> DeferredKind {
    DeferredKind::Directive(s.to_string())
}

/// Answers commands from a table (an unknown one answers empty) and replays
/// from another, and logs every call.
#[derive(Default)]
struct FakeServer {
    log: Vec<String>,
    /// The file each command was sent with, in order.
    files: Vec<String>,
    replies: HashMap<String, std::io::Result<String>>,
    replays: HashMap<String, Option<SourceReplayResult>>,
}

impl SourceJobIo for FakeServer {
    fn run_command(&mut self, file: &str, line: &str) -> std::io::Result<String> {
        self.log.push(format!("run {}", line));
        self.files.push(file.to_string());
        match self.replies.remove(line) {
            Some(r) => r,
            None => Ok(String::new()),
        }
    }

    fn replay(&mut self, item: DeferredConfigLine) -> Option<SourceReplayResult> {
        let text = match &item.kind {
            DeferredKind::Directive(t) | DeferredKind::Block(t) | DeferredKind::Command(t) => t.clone(),
        };
        self.log.push(format!("replay {}", text));
        self.replays
            .remove(&text)
            .unwrap_or(Some(SourceReplayResult { warnings: Vec::new(), deferred: Vec::new() }))
    }
}

#[test]
fn commands_run_one_after_another_in_file_order() {
    let mut io = FakeServer::default();
    let lines = vec![
        line("f", 1, cmd("new-window -n \"win-1\"")),
        line("f", 2, cmd("split-window -h")),
        line("f", 3, dir("set -g @x 1")),
        line("f", 4, cmd("select-pane -t 0")),
    ];
    let (_, outcome) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(
        io.log,
        vec![
            "run new-window -n \"win-1\"",
            "run split-window -h",
            "replay set -g @x 1",
            "run select-pane -t 0",
        ]
    );
    assert_eq!(outcome, SourceOutcome::default());
}

#[test]
fn a_nested_files_lines_run_before_the_rest_of_the_parent() {
    let mut io = FakeServer::default();
    io.replays.insert(
        "source-file child".to_string(),
        Some(SourceReplayResult {
            warnings: vec!["child:9: something".to_string()],
            deferred: vec![line("child", 1, cmd("split-window")), line("child", 2, cmd("rename-window nested"))],
        }),
    );
    let lines = vec![
        line("f", 1, cmd("new-window -n nest")),
        line("f", 2, dir("source-file child")),
        line("f", 3, dir("set -g @after 1")),
    ];
    let (warnings, _) = run_source_job(&mut io, lines, vec!["f:0: parse".to_string()], Vec::new());
    assert_eq!(
        io.log,
        vec![
            "run new-window -n nest",
            "replay source-file child",
            "run split-window",
            "run rename-window nested",
            "replay set -g @after 1",
        ]
    );
    assert_eq!(warnings, vec!["f:0: parse".to_string(), "child:9: something".to_string()]);
}

#[test]
fn an_error_reply_is_an_error_and_a_warning_at_its_line_and_the_rest_still_runs() {
    let mut io = FakeServer::default();
    io.replies.insert(
        "split-window -t nosuch".to_string(),
        Ok("ERROR: can't find pane: nosuch\n".to_string()),
    );
    let lines = vec![
        line("f.conf", 1, dir("set -g @e1 a")),
        line("f.conf", 2, cmd("split-window -t nosuch")),
        line("f.conf", 3, dir("set -g @e3 c")),
    ];
    let (warnings, outcome) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(outcome.errors, vec!["can't find pane: nosuch".to_string()]);
    assert_eq!(warnings, vec!["f.conf:2: can't find pane: nosuch".to_string()]);
    assert_eq!(io.log.last().map(String::as_str), Some("replay set -g @e3 c"));
}

#[test]
fn command_output_is_kept_for_the_client() {
    let mut io = FakeServer::default();
    io.replies.insert("display-message -p '#{window_name}'".to_string(), Ok("p1\n".to_string()));
    let lines = vec![line("f", 2, cmd("display-message -p '#{window_name}'"))];
    let (_, outcome) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(outcome.output, "p1\n");
    assert!(outcome.errors.is_empty());
}

#[test]
fn a_reply_mixing_output_and_errors_is_split_line_by_line() {
    let item = line("f", 5, cmd("x"));
    let mut warnings = Vec::new();
    let mut outcome = SourceOutcome::default();
    classify_reply(&item, "one\nERROR: bad\ntwo", &mut warnings, &mut outcome);
    assert_eq!(outcome.output, "one\ntwo");
    assert_eq!(outcome.errors, vec!["bad".to_string()]);
    assert_eq!(warnings, vec!["f:5: bad".to_string()]);
}

#[test]
fn a_lost_server_stops_the_job() {
    let mut io = FakeServer::default();
    io.replies.insert(
        "kill-server".to_string(),
        Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset")),
    );
    let lines = vec![line("f", 1, cmd("kill-server")), line("f", 2, cmd("new-window"))];
    let (_, outcome) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(io.log, vec!["run kill-server"]);
    assert_eq!(outcome.errors.len(), 1);
}

#[test]
fn a_replay_with_no_answer_stops_the_job() {
    let mut io = FakeServer::default();
    io.replays.insert("set -g @x 1".to_string(), None);
    let lines = vec![line("f", 1, dir("set -g @x 1")), line("f", 2, cmd("new-window"))];
    run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(io.log, vec!["replay set -g @x 1"]);
}

#[test]
fn warnings_are_written_the_way_warn_config_writes_them() {
    assert_eq!(deferred_warning(&line("f.conf", 2, cmd("x")), "m"), "f.conf:2: m");
    let no_line = DeferredConfigLine { file: "f.conf".to_string(), line: None, kind: cmd("x"), ancestors: Vec::new() };
    assert_eq!(deferred_warning(&no_line, "m"), "f.conf: m");
    let no_file = DeferredConfigLine { file: String::new(), line: Some(3), kind: cmd("x"), ancestors: Vec::new() };
    assert_eq!(deferred_warning(&no_file, "m"), "m");
}

// ── the transport ───────────────────────────────────────────────────────────

#[test]
fn exchange_on_port_returns_the_whole_reply_after_the_server_closes() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, seen_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            // The client half-closes after its request, so this read ends.
            let mut request = String::new();
            let _ = stream.read_to_string(&mut request);
            let _ = stream.write_all(b"OK\nline one\nline two\n");
            let _ = seen_tx.send(request);
        }
    });
    let reply = crate::session::exchange_on_port(port, "k", None, "list-windows\n", Some(std::time::Duration::from_secs(5)))
        .expect("reply");
    assert_eq!(reply, "line one\nline two\n");
    assert_eq!(seen_rx.recv().unwrap(), "AUTH k\nlist-windows\n");
}

#[test]
fn exchange_on_port_reports_a_refused_key() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = String::new();
            let _ = stream.read_to_string(&mut request);
            let _ = stream.write_all(b"ERROR: Invalid session key\n");
        }
    });
    let err = crate::session::exchange_on_port(port, "bad", None, "list-windows\n", Some(std::time::Duration::from_secs(5)))
        .unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn errors_from_the_parse_and_from_replays_reach_the_client() {
    let mut io = FakeServer::default();
    io.replays.insert(
        "source-file loop".to_string(),
        Some(SourceReplayResult {
            warnings: vec!["f:2: source-file: loop is already being sourced".to_string()],
            deferred: Vec::new(),
        }),
    );
    let lines = vec![line("f", 1, cmd("split-window")), line("f", 2, dir("source-file loop"))];
    let (_, outcome) = run_source_job(&mut io, lines, Vec::new(), vec!["f:9: too many nested files".to_string()]);
    assert_eq!(
        outcome.errors,
        vec![
            "f:9: too many nested files".to_string(),
            "f:2: source-file: loop is already being sourced".to_string(),
        ]
    );
}

#[test]
fn a_replayed_lines_warnings_are_errors_for_the_client() {
    let mut io = FakeServer::default();
    io.replays.insert(
        "set -g no-such-option 1".to_string(),
        Some(SourceReplayResult {
            warnings: vec!["f:3: unknown option: no-such-option".to_string()],
            deferred: Vec::new(),
        }),
    );
    let lines = vec![line("f", 2, cmd("split-window")), line("f", 3, dir("set -g no-such-option 1"))];
    let (warnings, outcome) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(outcome.errors, vec!["f:3: unknown option: no-such-option".to_string()]);
    assert_eq!(warnings, outcome.errors);
}

/// A stand-in whose first command panics, as a bug in the job would.
struct PanickingServer;

impl SourceJobIo for PanickingServer {
    fn run_command(&mut self, _file: &str, _line: &str) -> std::io::Result<String> {
        panic!("a bug in the job");
    }

    fn replay(&mut self, _item: DeferredConfigLine) -> Option<SourceReplayResult> {
        None
    }
}

#[test]
fn a_job_that_panics_still_reports_so_the_server_counts_it_done() {
    // #853: the server loop counts source jobs until each reports. A job that
    // panicked never sent SourceJobDone, so the count stayed up and the
    // warning list never started over again.
    let mut io = PanickingServer;
    let lines = vec![line("f", 2, cmd("split-window"))];
    let (warnings, outcome) =
        run_source_job_reporting(&mut io, lines, vec!["f:1: parse".to_string()], vec!["f:1: parse".to_string()]);
    assert_eq!(warnings[0], "f:1: parse", "the parse's warnings come back first: {:?}", warnings);
    assert_eq!(warnings.len(), 2, "{:?}", warnings);
    assert!(outcome.errors.last().unwrap().contains("stopped unexpectedly"), "{:?}", outcome.errors);
}

#[test]
fn a_job_that_does_not_panic_reports_as_before() {
    let lines = vec![line("f", 2, dir("set -g no-such-option 1"))];
    let mut io = FakeServer::default();
    let (w1, o1) = run_source_job_reporting(&mut io, lines.clone(), Vec::new(), Vec::new());
    let mut io = FakeServer::default();
    let (w2, o2) = run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(w1, w2);
    assert_eq!(o1.errors, o2.errors);
}

#[test]
fn each_command_goes_with_the_file_it_was_read_from() {
    // Issue #886: in tmux each command parsed from a file carries the file as
    // #{current_file} (cfg.c:139 and :191), so a run-shell after the first
    // command still finds #{d:current_file}. A nested file's commands go with
    // their own file, and the parent's after them with the parent's again.
    let mut io = FakeServer::default();
    io.replays.insert(
        "source-file child".to_string(),
        Some(SourceReplayResult { warnings: Vec::new(), deferred: vec![line("child", 1, cmd("run-shell nested"))] }),
    );
    let lines = vec![
        line("parent", 1, cmd("run-shell first")),
        line("parent", 2, dir("source-file child")),
        line("parent", 3, cmd("run-shell last")),
    ];
    run_source_job(&mut io, lines, Vec::new(), Vec::new());
    assert_eq!(io.files, vec!["parent", "child", "parent"]);
}
