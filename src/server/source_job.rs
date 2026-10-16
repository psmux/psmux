//! A sourced file's commands, run one after another (issue #842).
//!
//! tmux parses the whole file and inserts its command list right after the
//! source-file item (cmd-source-file.c:119, cfg.c:155 to :204), so each command
//! runs only once the one before it has finished, and a command without `-t`
//! acts on what the previous one made current (cmd-new-window.c:147,
//! cmd-split-window.c:217, cmd-find.c:975). The config parser can apply
//! options and bindings in place, but it cannot create windows or panes from
//! inside the server loop: those run in the request arms the connection
//! dispatcher feeds. config::source_runtime therefore keeps a file's lines from
//! its first command on, and this job runs them in order once the parse is
//! over: a command through the dispatcher, on a one-shot connection whose end
//! means the command is done, and a config line back on the server loop,
//! where it is parsed with its file and line restored.
//!
//! Each source gets a thread of its own. A command line that runs another
//! `source-file` waits in the dispatcher for that file's job, so one shared
//! worker would wait for itself.

use std::collections::VecDeque;
use std::sync::mpsc;

use crate::types::{CtrlReq, DeferredConfigLine, DeferredKind, SourceOutcome, SourceReplayResult};

/// What a source job needs from the server, so the job can be run against a
/// stand-in in tests.
pub(crate) trait SourceJobIo {
    /// Run one command line through the dispatcher and return its whole reply
    /// once the command has finished.
    fn run_command(&mut self, line: &str) -> std::io::Result<String>;
    /// Parse one deferred config line on the server loop. None when the loop
    /// did not answer, which ends the job.
    fn replay(&mut self, item: DeferredConfigLine) -> Option<SourceReplayResult>;
}

/// The server this job runs in, reached the way any client reaches it.
struct ServerIo {
    port: u16,
    key: String,
    tx: mpsc::Sender<CtrlReq>,
}

impl SourceJobIo for ServerIo {
    fn run_command(&mut self, line: &str) -> std::io::Result<String> {
        crate::session::exchange_on_port(self.port, &self.key, None, &format!("{}\n", line), None)
    }

    fn replay(&mut self, item: DeferredConfigLine) -> Option<SourceReplayResult> {
        let (rtx, rrx) = mpsc::channel();
        self.tx.send(CtrlReq::SourceReplay(item, rtx)).ok()?;
        rrx.recv().ok()
    }
}

/// A warning for a deferred line, `file:line: message` as warn_config writes
/// one.
pub(crate) fn deferred_warning(item: &DeferredConfigLine, msg: &str) -> String {
    match (item.file.is_empty(), item.line) {
        (false, Some(line)) => format!("{}:{}: {}", item.file, line, msg),
        (false, None) => format!("{}: {}", item.file, msg),
        _ => msg.to_string(),
    }
}

/// Split a command's reply into the output a client prints and the errors
/// it reports: each `ERROR: ` line is an error, recorded as a config warning
/// for the line that caused it too.
pub(crate) fn classify_reply(
    item: &DeferredConfigLine,
    reply: &str,
    warnings: &mut Vec<String>,
    outcome: &mut SourceOutcome,
) {
    for line in reply.split_inclusive('\n') {
        match line.strip_prefix("ERROR: ") {
            Some(msg) => {
                let msg = msg.trim_end();
                outcome.errors.push(msg.to_string());
                warnings.push(deferred_warning(item, msg));
            }
            None => outcome.output.push_str(line),
        }
    }
}

/// Run the deferred lines strictly in order. A config line that comes back
/// with lines of its own (a `source-file`, an `if-shell` whose branch holds a
/// command) has them run next, before the rest of this file, as tmux inserts a
/// nested file's commands right after the item that sourced it. `warnings`
/// starts as the batch the parse recorded and comes back with the job's added;
/// `errors`, the parse's errors, start the client's error list the same way.
pub(crate) fn run_source_job(
    io: &mut dyn SourceJobIo,
    lines: Vec<DeferredConfigLine>,
    mut warnings: Vec<String>,
    errors: Vec<String>,
) -> (Vec<String>, SourceOutcome) {
    let mut queue: VecDeque<DeferredConfigLine> = lines.into();
    let mut outcome = SourceOutcome { output: String::new(), errors };
    while let Some(item) = queue.pop_front() {
        match &item.kind {
            DeferredKind::Command(line) => match io.run_command(line) {
                Ok(reply) => classify_reply(&item, &reply, &mut warnings, &mut outcome),
                Err(e) => {
                    // The server is gone (a `kill-server` line) or cannot be
                    // reached; nothing after this line can run.
                    let msg = e.to_string();
                    outcome.errors.push(msg.clone());
                    warnings.push(deferred_warning(&item, &msg));
                    break;
                }
            },
            DeferredKind::Directive(_) | DeferredKind::Block(_) => match io.replay(item) {
                Some(result) => {
                    // A replayed line's warnings are errors for the client,
                    // as the parse's are (config::source_runtime).
                    outcome.errors.extend(result.warnings.iter().cloned());
                    warnings.extend(result.warnings);
                    for next in result.deferred.into_iter().rev() {
                        queue.push_front(next);
                    }
                }
                None => break,
            },
        }
    }
    (warnings, outcome)
}

/// Start the job for one source on a thread of its own. When it is done the
/// server loop gets SourceJobDone, which reports the warnings and answers the
/// client that ran `source-file` (if it waits). If the loop is gone, the
/// waiter is dropped with the message, which unblocks that client too.
pub(crate) fn spawn_source_job(
    port: u16,
    key: String,
    tx: mpsc::Sender<CtrlReq>,
    lines: Vec<DeferredConfigLine>,
    warnings: Vec<String>,
    errors: Vec<String>,
    waiter: Option<mpsc::Sender<SourceOutcome>>,
) {
    let done_tx = tx.clone();
    let _ = std::thread::Builder::new()
        .name("source-job".into())
        .spawn(move || {
            let mut io = ServerIo { port, key, tx };
            let (warnings, outcome) = run_source_job(&mut io, lines, warnings, errors);
            let _ = done_tx.send(CtrlReq::SourceJobDone { warnings, outcome, waiter });
        });
}

#[cfg(test)]
#[path = "../../tests-rs/test_issue842_source_job.rs"]
mod test_issue842_source_job;
