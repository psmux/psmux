use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;
use std::net::TcpStream;

use crate::types::{
    ControlNotification, CtrlReq, LayoutKind, WaitForOp, WindowDumpFormat,
};

/// Clear HANDLE_FLAG_INHERIT on a connection socket (see the comment at the
/// clone sites in `handle_connection`). No-op off Windows.
#[cfg(windows)]
fn clear_inherit(s: &TcpStream) {
    use std::os::windows::io::AsRawSocket;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(h: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    unsafe {
        SetHandleInformation(s.as_raw_socket() as *mut core::ffi::c_void, HANDLE_FLAG_INHERIT, 0);
    }
}
#[cfg(not(windows))]
fn clear_inherit(_s: &TcpStream) {}

/// Run `psmux [-L <this server's namespace>] new-session <args>` and return
/// what it printed, with a failure turned into an `ERROR:` line (#734 follow
/// up). The routing variables this server carries are removed so the child
/// decides everything from the namespace alone.
fn run_cli_new_session(args: &[&str]) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
    let mut cmd = std::process::Command::new(&exe);
    if let Some(ns) = crate::server::server_namespace() {
        cmd.arg("-L").arg(ns);
    }
    cmd.arg("new-session").args(args);
    for var in ["PSMUX_TARGET_SESSION", "PSMUX_TARGET_FULL", crate::session::ROUTE_WARM_ENV] {
        cmd.env_remove(var);
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    { use crate::platform::HideWindowCommandExt; cmd.hide_window(); }
    match cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                let err = err.trim().trim_start_matches("psmux:").trim();
                let err = if err.is_empty() { "new-session failed" } else { err };
                text.push_str(&format!("ERROR: {}\n", err));
            } else if text.trim().is_empty() {
                // The CLI prints nothing on a plain success; the wire contract
                // of this handler has always been `OK` (test_issue505 and the
                // raw socket callers read exactly that). A `-P` report is the
                // reply when there is one.
                text = "OK\n".to_string();
            }
            text
        }
        Err(e) => format!("ERROR: new-session: {}\n", e),
    }
}

/// What a one-shot detached `new-session` sends back. Alone on its connection
/// it keeps the historical `OK` for a plain success (raw socket callers read
/// exactly that). Inside a command list (an `if-shell` branch such as
/// `new-session -d -s x ; display-message -p done`) that placeholder would be
/// printed to the user ahead of the next command's output, and tmux prints
/// nothing for a `new-session -d` without `-P` (cmd-new-session.c prints only
/// under -P), so it is dropped there. A `-P` report and an `ERROR:` line pass
/// through either way.
pub(crate) fn one_shot_new_session_reply(out: String, in_command_list: bool) -> String {
    if in_command_list && out == "OK\n" {
        String::new()
    } else {
        out
    }
}

/// The write side of a client connection: one logical reply, one send.
///
/// The default `Write::write_fmt` hands every piece of a format string to
/// `write_all` on its own, so on a TCP_NODELAY socket `write!(s, "{}\n", text)`
/// went out as two segments (the text, then the newline) and
/// `writeln!(s, "ERROR: {}", e)` as three, so what a reader got from one recv
/// depended on segment timing. A reply's end is the close of the connection
/// (one-shot) or its newline (persistent frames); one send per reply keeps the
/// bytes a reader sees independent of timing. This wrapper formats the whole argument
/// list into one buffer and sends it with a single `write_all`, so every
/// `write!` / `writeln!` on a connection is one send; `write`, `write_all` and
/// `flush` pass straight through. It derefs to the socket for the read only
/// calls (`set_nodelay`, `shutdown`, `register_persistent_stream`), and
/// `try_clone` hands back another `ReplyStream` so clones keep the property.
///
/// The second field records that a formatted reply starting with `ERROR` went
/// out since [`ReplyStream::take_error`] last looked: the one-shot loop ends a
/// command list there, as tmux's cmd-queue.c removes the rest of the list
/// after a command fails. Every command error on this connection is written
/// with `write!`/`writeln!` as an `ERROR...` line (pane text such as
/// capture-pane's goes out with `write_all` and is not looked at).
pub(crate) struct ReplyStream(TcpStream, bool);

impl ReplyStream {
    fn new(stream: TcpStream) -> ReplyStream {
        ReplyStream(stream, false)
    }

    fn try_clone(&self) -> io::Result<ReplyStream> {
        self.0.try_clone().map(ReplyStream::new)
    }

    /// Whether an error reply was written since the last call, clearing it.
    fn take_error(&mut self) -> bool {
        std::mem::take(&mut self.1)
    }

    fn send_reply(&mut self, text: &str) -> io::Result<()> {
        if is_error_reply(text) {
            self.1 = true;
        }
        self.0.write_all(text.as_bytes())
    }

    /// The underlying socket, for a clone that is only ever shut down.
    fn socket(&self) -> &TcpStream {
        &self.0
    }
}

impl std::ops::Deref for ReplyStream {
    type Target = TcpStream;
    fn deref(&self) -> &TcpStream {
        &self.0
    }
}

impl Write for ReplyStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.write_all(buf)
    }

    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> io::Result<()> {
        match args.as_str() {
            Some(s) => self.send_reply(s),
            None => self.send_reply(&std::fmt::format(args)),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

/// A command's failure reply on a one-shot connection: `ERROR: <message>`, or
/// `ERROR <message>` as the switch-client handlers answer.
pub(crate) fn is_error_reply(text: &str) -> bool {
    text.starts_with("ERROR")
}

/// Whether `name` is a tmux command or alias in the [`TMUX_COMMANDS`] catalog.
pub(crate) fn is_catalog_command(name: &str) -> bool {
    TMUX_COMMANDS.iter().any(|entry| match entry.split_once(" (") {
        Some((command, alias)) => command == name || alias.trim_end_matches(')') == name,
        None => *entry == name,
    })
}

/// Whether the control mode parser accepts `name` as a command besides the
/// [`TMUX_COMMANDS`] catalog: the psmux and iTerm2 names its dispatcher runs,
/// and a prefix of a catalog name, which tmux resolves to the one command it
/// abbreviates (cmd.c cmd_find_entry) and the dispatcher answers on its own.
pub(crate) fn is_control_extension_command(name: &str) -> bool {
    matches!(name, "dump" | "dump-state" | "new-pane" | "newp" | "split-pane" | "splitp"
        | "zoom-pane" | "phony-command" | "run-command" | "runcmd")
        || (!name.is_empty() && TMUX_COMMANDS.iter().any(|entry| entry.starts_with(name)))
}

/// Expand a `set-option -F` value as a format before it is stored.
///
/// tmux's set-option takes `-F` and runs the value through format_expand before
/// writing it, and psmux's config-file parser has always done the same. The
/// CLI and TCP paths could not, because format expansion needs the AppState
/// that lives on the server thread, so `-F` was rejected outright. Both
/// set-option handlers now route their value through here, which asks the
/// server thread to expand it with the same CtrlReq the bind-key path uses.
/// Without `-F`, or for an empty value, this is the identity.
fn expand_set_option_value(
    tx: &TargetedSender,
    format_expand: bool,
    value: String,
) -> String {
    if !format_expand || value.is_empty() {
        return value;
    }
    let (rtx, rrx) = mpsc::channel::<String>();
    if tx.send(CtrlReq::ExpandFormat(value.clone(), rtx)).is_err() {
        return value;
    }
    // On timeout keep the unexpanded text: storing the literal format is no
    // worse than dropping the assignment.
    rrx.recv_timeout(Duration::from_secs(5)).unwrap_or(value)
}

/// if-shell's arguments: whether `-F` is given, and the operands (condition,
/// command, else command), a `{ ... }` block collapsed into one operand.
///
/// Flags are if-shell's own only at the top level. Inside a block every token
/// belongs to the block's commands: the old reader dropped every `-` token
/// before collapsing blocks, so `if-shell true { display -p x }` ran
/// `display x` and `{ set -gF @o '#{...}' }` ran `set @o '#{...}'` (issue
/// #793 follow up; tmux 3.4 keeps them, cmd-parse.y). Block tokens are
/// requoted so the command reparses to the same words.
fn if_shell_parts(args: &[&str]) -> (bool, Vec<String>) {
    let mut format_mode = false;
    let mut positional: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut block: Vec<String> = Vec::new();
    for a in args {
        if depth == 0 {
            if *a == "{" {
                depth = 1;
                block.clear();
            } else if a.starts_with('-') && a.len() > 1 && positional.is_empty() {
                // getopt: flags come before the first operand.
                if a[1..].contains('F') { format_mode = true; }
            } else {
                positional.push(a.to_string());
            }
            continue;
        }
        match *a {
            "{" => { depth += 1; block.push(a.to_string()); }
            "}" => {
                depth -= 1;
                if depth == 0 {
                    positional.push(block.join(" "));
                } else {
                    block.push(a.to_string());
                }
            }
            _ => block.push(crate::util::quote_arg_if_needed(a)),
        }
    }
    // An unclosed block still counts as the operand it started.
    if depth > 0 && !block.is_empty() {
        positional.push(block.join(" "));
    }
    (format_mode, positional)
}

/// Decide an if-shell condition: a `-F` format (true unless empty or "0") or
/// a shell command (its exit status), both format expanded first like tmux
/// (cmd-if-shell.c:86).
///
/// `target_failed`: the -t named nothing (CMD_FIND_CANFAIL), so tmux expands
/// with an EMPTY target: `#{pane_id}` is "" and `-F '#{pane_id}'` is false.
/// Expanding against the active pane took the true branch (issue #793 follow
/// up). Server variables such as `#{pid}` still resolve.
fn if_shell_condition(
    tx: &TargetedSender,
    condition: &str,
    format_mode: bool,
    target_failed: bool,
    client: Option<crate::types::ClientSel>,
) -> bool {
    if format_mode {
        let (rtx, rrx) = mpsc::channel::<String>();
        if target_failed {
            let _ = tx.send(CtrlReq::ExpandFormatNoTarget(condition.to_string(), rtx));
        } else {
            let _ = tx.send(CtrlReq::DisplayMessage(rtx, condition.to_string(), None, false, None, client, Default::default()));
        }
        let expanded = rrx.recv().unwrap_or_default();
        return !expanded.is_empty() && expanded != "0";
    }
    // The shell command is format expanded before it runs. It went to the
    // shell verbatim once, so a guard such as `if-shell 'check #{pid}'` saw
    // the literal `#{pid}` (issue #734 follow up).
    let condition = expand_shell_command_for(tx, condition.to_string(), target_failed);
    if condition == "true" || condition == "1" {
        true
    } else if condition == "false" || condition == "0" {
        false
    } else {
        let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
        let mut c = std::process::Command::new(&shell_prog);
        for a in &shell_args { c.arg(a); }
        c.arg(condition.as_str());
        // Console input injection can detach the server and invalidate its
        // stdin handle. Conditions never consume pane input.
        c.stdin(std::process::Stdio::null());
        c.stdout(std::process::Stdio::null());
        c.stderr(std::process::Stdio::null());
        crate::commands::apply_job_env(&mut c);
        { use crate::platform::HideWindowCommandExt; c.hide_window(); }
        c.status().map(|s| s.success()).unwrap_or(false)
    }
}

/// run-shell's flags (`-b`, `-d delay`, commands::parse_run_shell_args) and
/// its shell command: the rest, a balanced pair of outer quotes removed
/// (#402), not yet format expanded.
fn run_shell_command_text(
    args: &[&str],
) -> Result<(bool, Option<Duration>, String), String> {
    let parsed = crate::commands::parse_run_shell_args(args)?;
    let shell_cmd = parsed.command.join(" ");
    let trimmed = shell_cmd.trim();
    let shell_cmd = if trimmed.len() >= 2
        && ((trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    };
    Ok((parsed.background, parsed.delay, shell_cmd))
}

/// `expand_shell_command`, or with no session, window or pane when the
/// command's -t named nothing (tmux CMD_FIND_CANFAIL runs it with an empty
/// target; issue #793 follow up).
fn expand_shell_command_for(tx: &TargetedSender, cmd: String, target_failed: bool) -> String {
    if !target_failed {
        return expand_shell_command(tx, cmd);
    }
    if !cmd.contains('#') {
        return cmd;
    }
    let (rtx, rrx) = mpsc::channel::<String>();
    if tx.send(CtrlReq::ExpandFormatNoTarget(cmd.clone(), rtx)).is_err() {
        return cmd;
    }
    rrx.recv_timeout(Duration::from_secs(5)).unwrap_or(cmd)
}

/// Format expand the shell command of if-shell or run-shell before it runs.
///
/// tmux expands it unconditionally (cmd-if-shell.c:86
/// format_single_from_target, cmd-run-shell.c:144 format_expand), against the
/// command's target, or with no session at all on an empty server, where
/// server variables such as `#{pid}` still resolve. A command with no `#` in
/// it expands to itself, so the round trip to the server thread is skipped.
/// The expansion is untimed: a `%` in a shell command is not strftime.
fn expand_shell_command(tx: &TargetedSender, cmd: String) -> String {
    if !cmd.contains('#') {
        return cmd;
    }
    let (rtx, rrx) = mpsc::channel::<String>();
    if tx.send(CtrlReq::ExpandFormat(cmd.clone(), rtx)).is_err() {
        return cmd;
    }
    // On timeout run the unexpanded text: no worse than not expanding, and
    // better than dropping the command silently.
    rrx.recv_timeout(Duration::from_secs(5)).unwrap_or(cmd)
}

/// The sender a command's handler sends its requests through.  When the
/// command has a validated -t target, EVERY request it sends goes out as
/// `CtrlReq::Targeted(target, request)`, so the server applies the target to
/// that request and to nothing else, whatever other clients or panes queue in
/// between.  Without a target it passes requests through untouched.
///
/// Deliberately not `Deref` to the raw sender: a handler cannot reach the
/// server except through `send`, so no request of a targeted command can
/// silently go out untargeted.
pub(crate) struct TargetedSender<'a> {
    inner: &'a mpsc::Sender<CtrlReq>,
    target: Option<crate::types::TempTarget>,
}

impl<'a> TargetedSender<'a> {
    pub(crate) fn new(inner: &'a mpsc::Sender<CtrlReq>, target: Option<crate::types::TempTarget>) -> Self {
        TargetedSender { inner, target }
    }

    pub(crate) fn send(&self, req: CtrlReq) -> Result<(), mpsc::SendError<CtrlReq>> {
        match &self.target {
            Some(t) => self.inner.send(CtrlReq::Targeted(t.clone(), Box::new(req))),
            None => self.inner.send(req),
        }
    }

    /// The validated target this sender wraps requests in, for a request that
    /// carries its target itself (`CtrlReq::SetPaneInputOff`).
    pub(crate) fn target(&self) -> Option<&crate::types::TempTarget> {
        self.target.as_ref()
    }

    /// Send `req` WITHOUT the temporary-focus wrapper.  Only for a request
    /// that already carries `target()` inside it.  A send fails only when the
    /// server loop is gone, and the caller's reply wait then returns at once.
    pub(crate) fn send_unwrapped(&self, req: CtrlReq) {
        let _ = self.inner.send(req);
    }
}

/// Validate a command's -t target on the server, read only.  Ok is the
/// target in stable id form for `TargetedSender`; Err is tmux's message and
/// the command must not run.
///
/// The old wait gave up after 5 s and then sent the command anyway, without
/// its focus having been applied in any way the command could rely on.  The
/// reply cannot be overtaken (the command's own requests queue behind this
/// one in the same FIFO), so waiting longer delays nothing.  A server that
/// does not answer within a minute does not get the command at all.
fn validate_target(
    tx: &mpsc::Sender<CtrlReq>,
    target: crate::types::TempTarget,
) -> Result<crate::types::TempTarget, String> {
    let (s, r) = mpsc::channel::<Result<crate::types::TempTarget, String>>();
    if tx.send(CtrlReq::ValidateTarget { target, resp: s }).is_err() {
        return Err("server is shutting down".to_string());
    }
    match r.recv_timeout(Duration::from_secs(60)) {
        Ok(result) => result,
        Err(_) => Err("no response from server (timed out)".to_string()),
    }
}

/// Rewrite a bare `-t` name of a window or pane command to the `@id` / `%id`
/// it names on live state (`CtrlReq::ResolveTarget`). `Ok(None)`: nothing to
/// rewrite. `Err`: tmux's "can't find ..."; the command must not run.
fn bare_target_rewrite(
    cmd: &str,
    args: &[&str],
    tx: &mpsc::Sender<CtrlReq>,
) -> Result<Option<Vec<String>>, String> {
    let Some(kind) = crate::commands::in_server_target_kind(cmd) else { return Ok(None) };
    let end = crate::cli::outer_target_scan_end(cmd, args).min(args.len());
    let Some(i) = args[..end].iter().position(|a| *a == "-t") else { return Ok(None) };
    let Some(t) = args.get(i + 1) else { return Ok(None) };
    if !crate::cli::bare_target_token(t) {
        return Ok(None);
    }
    let (s, r) = mpsc::channel();
    if tx.send(CtrlReq::ResolveTarget { kind, spec: t.to_string(), resp: s }).is_err() {
        return Ok(None);
    }
    match r.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(Some(id))) => Ok(Some(args.iter().enumerate()
            .map(|(j, a)| if j == i + 1 { id.clone() } else { a.to_string() })
            .collect())),
        Ok(Err(e)) => Err(e),
        _ => Ok(None),
    }
}

/// Does `cmd`'s outer `-t` name a session that must be THIS server's own?
///
/// One psmux server holds one session, so the session part of a `-t` (or of a
/// TARGET line) is only ever a claim about which server the connection
/// reached. These commands are the exceptions: their target names another
/// session on purpose (switch-client, the cross-session window and pane
/// movers, new-session's group target, kill-session's forward), a client
/// rather than a session (detach-client and the other target-client
/// commands), or they resolve the session themselves (has-session, and
/// set-option's own parser).
fn target_session_must_be_ours(cmd: &str) -> bool {
    !matches!(
        cmd,
        "switch-client" | "switchc" | "detach-client" | "detach"
            | "kill-session" | "kill-ses" | "has-session" | "has"
            | "new-session" | "new" | "attach-session" | "attach" | "a" | "at"
            | "refresh-client" | "refresh" | "lock-client" | "lockc"
            | "suspend-client" | "suspendc" | "show-messages" | "showmsgs"
            | "display-panes" | "displayp"
            | "link-window" | "linkw" | "move-window" | "movew"
            | "join-pane" | "joinp" | "move-pane" | "movep"
            | "swap-window" | "swapw" | "swap-pane" | "swapp"
            | "set-option" | "set" | "set-window-option" | "setw"
            // set-buffer's -t is a client; its arm refuses one that names
            // another session (commands::set_buffer_foreign_session).
            | "set-buffer" | "setb"
    )
}

/// The session an outer target names explicitly, when it must be this
/// server's own (issue #793).
///
/// Only the explicit forms count: `sess:...` and `$N`. A bare word keeps its
/// existing meaning (a window-shaped token, or a name some handlers resolve
/// themselves), so nothing that worked before is reinterpreted.
///
/// Without this check a connection that reached the wrong server, such as the
/// namespace's warm standby, ran `send-keys -t t:@1.%1 C-c` and
/// `respawn-pane -k -t t:@1.%1` against that server's OWN %1 and answered OK,
/// while session t never saw them. tmux answers "can't find session: X".
pub(crate) fn explicit_target_session(cmd: &str, raw_target: Option<&str>) -> Option<String> {
    if !target_session_must_be_ours(cmd) {
        return None;
    }
    let raw = crate::cli::strip_exact_match_prefix(raw_target?);
    if !raw.contains(':') && !raw.starts_with('$') {
        return None;
    }
    parse_target(raw).session.filter(|s| !s.is_empty())
}

/// Commands that resolve the pane of their `-t` themselves, so the generic
/// temporary focus leaves the pane alone, but whose pane must still exist.
///
/// display-message expands its format for the named pane (by %id across
/// windows, #332, or by index, #113), and capture-pane and kill-pane by %id
/// look the id up on their own. A pane that does not exist used to give the
/// ACTIVE pane's values (`display-message -p -t %99` printed `%1`), an empty
/// capture, or a kill of nothing, all at exit 0 over TCP and control mode
/// (issue #793 follow up). The CLI already refused them client side.
pub(crate) fn pane_checked_not_focused(cmd: &str, pane_is_id: bool) -> bool {
    matches!(cmd, "display-message" | "display")
        || (pane_is_id && matches!(cmd, "capture-pane" | "capturep" | "kill-pane" | "killp"))
}

/// tmux's CMD_FIND_CANFAIL commands whose own work does not need the target
/// (cmd-run-shell.c, cmd-if-shell.c, cmd-source-file.c): a target that names
/// nothing is not an error, the command runs without one.
pub(crate) fn target_can_fail(cmd: &str) -> bool {
    matches!(cmd, "run-shell" | "run" | "if-shell" | "if" | "source-file" | "source")
}

/// SetPaneOption/ShowPaneOptions resolve only "" (the active pane) and bare
/// "%N"/"N" pane ids. set-option keeps its -t in the argument list for the
/// shared parser (unlike every other command, whose -t without_outer_target
/// strips), so a richer form like "session:win.pane" reaches these arms
/// verbatim. That form was already resolved AND validated by the
/// ValidateTarget issued before dispatch, and each request then runs with that
/// pane focused (CtrlReq::Targeted), making the active pane the target;
/// forwarding the raw text made the bare-id parse fail and reported a pane
/// that provably exists as missing (#583 arm 6 regression).
fn pane_scope_target(raw: String) -> String {
    if raw.is_empty() || raw.trim().trim_start_matches('%').parse::<usize>().is_ok() {
        raw
    } else {
        String::new()
    }
}
use crate::cli::{extract_flag_value, parse_set_option_args, parse_target};
use crate::util::base64_decode;
use crate::control;

/// Append-only AUTH diagnostics, gated by PSMUX_AUTH_DEBUG=1. Written to
/// %TEMP%\psmux_auth_debug.log so concurrent processes never truncate each
/// other (issue #496 forensics).
fn auth_debug(msg: &str) {
    if std::env::var("PSMUX_AUTH_DEBUG").map(|v| v == "1").unwrap_or(false) {
        let tmp = std::env::var("TEMP")
            .or_else(|_| std::env::var("TMP"))
            .unwrap_or_else(|_| ".".to_string());
        let path = format!("{}\\psmux_auth_debug.log", tmp);
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = std::io::Write::write_all(
                &mut f,
                format!("[{} pid={}] {}\n", ts, std::process::id(), msg).as_bytes(),
            );
        }
    }
}
use crate::commands::parse_command_line;
use super::helpers::TMUX_COMMANDS;

/// Split a command line on top-level `;` separators, respecting single and
/// double quotes and `\` escapes. Real tmux's parser treats `;` as a command
/// separator on the same line; iTerm2's `sendCommandList` joins many commands
/// with "; " into one wire line and expects one %begin/%end pair per command.
fn split_top_level_semicolons(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && !in_single {
            // Escape: copy the backslash and the next char (if any) verbatim.
            cur.push(c);
            if let Some(nc) = chars.next() { cur.push(nc); }
            continue;
        }
        match c {
            '\'' if !in_double => { in_single = !in_single; cur.push(c); }
            '"'  if !in_single => { in_double = !in_double; cur.push(c); }
            ';'  if !in_single && !in_double => {
                let trimmed = cur.trim().to_string();
                if !trimmed.is_empty() { out.push(trimmed); }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let trimmed = cur.trim().to_string();
    if !trimmed.is_empty() { out.push(trimmed); }
    out
}

/// The send-keys requests for one command line, shared by the one shot
/// route and the control route so every flag means the same on both. The
/// control route used to carry its own copy that knew only -H and -l: -X
/// typed its copy command's NAME into the pane, and -N and `--` were
/// dropped (issue #799). The error is tmux's for a refused -X.
fn send_keys_requests(args: &[&str], tx: &TargetedSender, client_id: u64) -> Result<(), String> {
    // End-of-options: a bare `--` terminates flag parsing. Every token after
    // it is an operand even if it begins with '-', and `--` itself is
    // consumed. Without this, `send-keys -l -- -foo` dropped `--` as a dash
    // token and dropped `-foo` because its first byte is '-', so a
    // dash-leading literal had no deliverable spelling (issue #562).
    let end_of_opts: Option<usize> = args.iter().position(|a| *a == "--");
    let past_opts = |i: usize| -> bool { end_of_opts.is_some_and(|eo| i > eo) };
    let is_marker = |i: usize| -> bool { end_of_opts == Some(i) };
    // tmux short-flag clusters (e.g. iTerm2's `send -lt %1 l`): inspect
    // each `-xyz` arg and check whether any of x/y/z is a known flag.
    // Tokens at or after the `--` marker are operands, not flags.
    let flag_has = |c: char| -> bool {
        args.iter().enumerate().any(|(i, a)| {
            !is_marker(i) && !past_opts(i)
                && a.starts_with('-') && !a.starts_with("--")
                && a.chars().skip(1).any(|fc| fc == c)
        })
    };
    // Returns true if the previous arg is a short-flag cluster whose
    // *trailing* character takes an operand (e.g. -t, -lt, -N).
    let prev_consumes_operand = |i: usize| -> bool {
        if i == 0 { return false; }
        if is_marker(i - 1) || past_opts(i - 1) { return false; }
        if let Some(prev) = args.get(i - 1) {
            if prev.starts_with('-') && !prev.starts_with("--") && prev.len() >= 2 {
                if let Some(last) = prev.chars().last() {
                    return matches!(last, 't' | 'T' | 'N' | 'R' | 'c');
                }
            }
        }
        false
    };
    // A token counts as an operand (a key to deliver) when it is past the
    // `--` marker, or when it is neither a flag cluster nor a flag's value.
    // The `--` marker itself is never an operand.
    let is_operand = |i: usize, a: &str| -> bool {
        if is_marker(i) { return false; }
        // An EMPTY argument sends nothing, as in tmux. Writing
        // `send-keys -t x "X" ""` is an ordinary way to say "type X with no
        // trailing newline", and the empty argument must produce no
        // keystroke whatsoever.
        //
        // It used to qualify as an operand (an empty string does not start
        // with '-') and travel on as a key NAMED "", which matched no key
        // name and fell through to a handler that deleted the rest of the
        // line. Measured against tmux 3.4 with an identical sequence:
        //
        //   type "echo arrow_ABC", then Left Left Left, then `"X" ""`
        //     tmux   -> arrow_XABC
        //     psmux  -> arrow_AX     (everything right of the cursor gone)
        //
        // It stayed hidden because it is invisible when the cursor is at the
        // end of the line: only an edit in the MIDDLE of a line shows it.
        // The check is before past_opts so an empty token after `--` is
        // dropped too.
        if a.is_empty() { return false; }
        if past_opts(i) { return true; }
        !a.starts_with('-') && !prev_consumes_operand(i)
    };
    let literal = flag_has('l');
    let paste_mode = flag_has('p');
    let has_x = flag_has('X');
    let hex_mode = flag_has('H');
    // tmux send-keys -M: forward the mouse event of the running mouse
    // binding (see window_ops::send_mouse_event). It sends no keys.
    let mouse_mode = flag_has('M');
    // Parse -N <count> for repeat (look for any cluster ending in 'N')
    let mut repeat_count: usize = 1;
    if let Some(n_pos) = args.iter().enumerate().position(|(i, a)| !is_marker(i) && !past_opts(i) && a.starts_with('-') && !a.starts_with("--") && a.ends_with('N')) {
        if let Some(count_str) = args.get(n_pos + 1) {
            repeat_count = count_str.parse::<usize>().unwrap_or(1).max(1);
        }
    }
    if mouse_mode {
        let _ = tx.send(CtrlReq::SendMouse(client_id));
    } else if hex_mode {
        // tmux `send-keys -H`: every operand is one hexadecimal BYTE value.
        // tmux tags these keys KEYC_LITERAL and writes them to the pty as
        // single bytes ("can't be more than 8 bits"), so this is a byte
        // channel, not a codepoint channel.  Decode here and pass the raw
        // bytes through untouched so UTF-8 sequences and escape sequences
        // survive verbatim.
        let mut bytes: Vec<u8> = Vec::new();
        for (i, a) in args.iter().enumerate() {
            if !is_operand(i, a) { continue; }
            // A malformed operand is dropped rather than leaked into the
            // pane as its literal token text.
            if let Ok(byte) = u8::from_str_radix(a, 16) { bytes.push(byte); }
        }
        if !bytes.is_empty() {
            for _ in 0..repeat_count {
                let _ = tx.send(CtrlReq::SendBytes(bytes.clone()));
            }
        }
    } else if has_x {
        // send-keys -X copy-mode-command
        let cmd_parts: Vec<&str> = args.iter().enumerate()
            .filter(|(i, a)| is_operand(*i, a))
            .map(|(_, a)| *a).collect();
        // One request carrying the count: tmux hands -N to the copy
        // command as its repeat (`wme->prefix`), and refuses -X outside
        // a mode with "not in a mode" (cmd-send-keys.c).
        let (rtx, rrx) = mpsc::channel();
        let _ = tx.send(CtrlReq::SendKeysXRun {
            cmd: cmd_parts.join(" "),
            count: repeat_count,
            resp: Some(rtx),
        });
        if let Ok(Err(e)) = rrx.recv_timeout(Duration::from_secs(5)) {
            return Err(e);
        }
    } else {
        let keys: Vec<String> = args.iter()
            .enumerate()
            .filter(|(i, a)| is_operand(*i, a))
            .map(|(_, a)| {
                // Convert real-tmux 0xNN hex codepoint syntax (sent by
                // iTerm2's gateway: e.g. `send -t %1 0xd` for Enter) into
                // the literal character so SendKeys forwards the right
                // byte to the PTY instead of the string "0xd".
                let s = *a;
                if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()) {
                        if let Ok(n) = u32::from_str_radix(rest, 16) {
                            if let Some(c) = char::from_u32(n) {
                                return c.to_string();
                            }
                        }
                    }
                }
                s.to_string()
            })
            .collect();
        // If any key was a hex-converted single byte, force literal mode so
        // the byte is written verbatim and not parsed as a key name.
        let any_hex = args.iter().any(|a| {
            let s = *a;
            if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit());
            }
            false
        });
        let effective_literal = literal || any_hex;
        for _ in 0..repeat_count {
            if paste_mode {
                let _ = tx.send(CtrlReq::SendPaste(keys.join("")));
            } else {
                // #490: hand the tokens over UNJOINED so quoted
                // arguments keep their exact whitespace end to end.
                let _ = tx.send(CtrlReq::SendKeys(keys.clone(), effective_literal));
            }
        }
    }
    Ok(())
}

/// Try to decode a single `send`/`send-keys` command into the literal byte
/// payload it would inject and the pane target.  Returns `None` if the
/// command uses features we don't safely coalesce (e.g. `-X`, `-p`, `-N`,
/// or named keys like `Up`/`Tab`) — in that case the caller falls back to
/// normal per-command dispatch.
///
/// This is used to merge consecutive `send` sub-commands within one input
/// line into a single PTY write.  iTerm2 sends arrow keys as
/// `send -t %1 0x1b 0x5b; send -lt %1 A` — two separate sub-commands.  If
/// each becomes its own PTY write, pwsh's PSReadLine times out between the
/// ESC byte and the `[A` and emits them as literal characters.  Coalescing
/// guarantees the whole VT sequence reaches the shell in one read().
fn decode_send_command(line: &str) -> Option<(String, Vec<u8>)> {
    let toks = parse_command_line(line);
    if toks.is_empty() { return None; }
    let cmd = toks[0].as_str();
    if cmd != "send" && cmd != "send-keys" { return None; }
    let args: Vec<&str> = toks[1..].iter().map(|s| s.as_str()).collect();

    // Bail on modes that require special semantics.
    let any_short = |c: char| {
        args.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.chars().skip(1).any(|fc| fc == c))
    };
    if any_short('X') || any_short('p') || any_short('N') || any_short('R') { return None; }
    // A bare `--` means end-of-options, and the tokens after it are operands
    // even if they begin with '-'. This coalescing fast path filters operands
    // by `starts_with('-')`, which would drop them, so defer to the normal
    // send-keys handler that understands `--` (issue #562).
    if args.iter().any(|a| *a == "--") { return None; }

    let prev_consumes_operand = |i: usize| -> bool {
        if i == 0 { return false; }
        if let Some(prev) = args.get(i - 1) {
            if prev.starts_with('-') && !prev.starts_with("--") && prev.len() >= 2 {
                if let Some(last) = prev.chars().last() {
                    return matches!(last, 't' | 'T' | 'N' | 'R' | 'c');
                }
            }
        }
        false
    };

    // Find target (-t / -lt /...).  Default to %active if absent.
    let mut target: Option<String> = None;
    for (i, a) in args.iter().enumerate() {
        if a.starts_with('-') && !a.starts_with("--") && a.ends_with('t') {
            if let Some(t) = args.get(i + 1) { target = Some((*t).to_string()); break; }
        }
    }

    let literal = any_short('l');
    // -H marks every operand as one raw hexadecimal byte (tmux KEYC_LITERAL).
    let literal_byte = any_short('H');
    let mut bytes: Vec<u8> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a.starts_with('-') { continue; }
        if prev_consumes_operand(i) { continue; }
        // An empty operand contributes no keystroke (tmux semantics), so it must
        // not reach the key lookup here either.
        if a.is_empty() { continue; }
        // Hex codepoint?
        let s = *a;
        if literal_byte {
            match u8::from_str_radix(s, 16) {
                Ok(byte) => { bytes.push(byte); continue; }
                // Malformed operand: refuse to coalesce and let the send-keys
                // handler decide what to do with the whole command.
                Err(_) => return None,
            }
        }
        if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()) {
                // `0xNN` is a codepoint (iTerm2's encoding for typed
                // characters), so it is UTF-8 encoded here.  Raw bytes only
                // ever arrive through -H above.  Encoding every value keeps
                // 0x80-0xFF correct: they are Latin-1 codepoints, not bytes.
                if let Ok(n) = u32::from_str_radix(rest, 16) {
                    if let Some(c) = char::from_u32(n) {
                        let mut buf = [0u8; 4];
                        bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        continue;
                    }
                }
            }
        }
        // Non-literal mode + non-hex token = could be a named key (Up, Tab,
        // BSpace, C-a, ...).  We can't safely turn that into raw bytes here,
        // so refuse to coalesce.
        if !literal { return None; }
        bytes.extend_from_slice(s.as_bytes());
    }

    Some((target.unwrap_or_else(|| String::new()), bytes))
}

/// The capture-pane line flags: `-N` keeps trailing spaces, `-J` joins
/// wrapped rows (and keeps their spaces, as tmux does).
fn capture_opts(args: &[&str]) -> crate::copy_mode::CaptureOpts {
    crate::copy_mode::CaptureOpts {
        preserve_trailing: crate::cli::has_short_flag(args, 'N'),
        join: crate::cli::has_short_flag(args, 'J'),
    }
}

/// tmux's `capture-pane -p` prints the capture and then exactly one newline
/// (cmd_capture_pane_exec drops a final '\n' and file_print adds one), so a
/// `-J` capture that ends inside a wrapped line still ends its output.
fn capture_print_text(mut text: String) -> String {
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The capture-pane forms that do not read the visible grid
/// (cmd-capture-pane.c cmd_capture_pane_exec): `-P` is the pending incomplete
/// escape sequence, which psmux's parser does not keep, so it is always empty;
/// `-a` is the main screen behind the alternate screen, "no alternate screen"
/// (or with `-q`, nothing) when the pane is not on it. `None` for any other
/// capture.
fn capture_pane_special(
    args: &[&str],
    tx: &TargetedSender,
    start: Option<i32>,
    end: Option<i32>,
    pane_id: Option<usize>,
) -> Option<Result<String, String>> {
    let has = |c: char| crate::cli::has_short_flag(args, c);
    if has('P') {
        return Some(Ok(String::new()));
    }
    if !has('a') {
        return None;
    }
    let (rtx, rrx) = mpsc::channel::<Option<String>>();
    let _ = tx.send(CtrlReq::CapturePaneSaved {
        resp: rtx,
        start,
        end,
        pane_id,
        styled: has('e'),
        opts: capture_opts(args),
    });
    // A wait that runs out is the timeout error, not "no alternate screen"
    // (the same rule as the user option read, issue #832).
    Some(match rrx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(text)) => Ok(text),
        Err(_) => Err("no response from server (timed out)".to_string()),
        Ok(None) if has('q') => Ok(String::new()),
        Ok(None) => Err("no alternate screen".to_string()),
    })
}

/// Run a parsed set-buffer on the server: `Err` is tmux's error text.
/// `raw_target` is the `-t` the dispatcher peeled off the line, which is
/// set-buffer's target client (a session name is refused when it is another
/// session's, commands::set_buffer_foreign_session).
fn run_set_buffer(tx: &TargetedSender, mut set: crate::commands::SetBufferArgs, raw_target: Option<&str>) -> Result<(), String> {
    if set.target.is_none() {
        set.target = raw_target.map(str::to_string);
    }
    let (rtx, rrx) = mpsc::channel();
    let _ = tx.send(CtrlReq::SetBufferCmd(set, rtx));
    rrx.recv_timeout(Duration::from_secs(5)).unwrap_or(Ok(()))
}

/// Re-join already-tokenized command args into a single command string,
/// re-quoting any token that held whitespace or quote characters so the
/// grouping survives a later re-parse by `parse_command_line` (#476).
/// `parse_command_line` consumes the quotes during tokenization, so a plain
/// `join(" ")` flattens `if-shell -F '1' 'set -g @r A' 'set -g @r B'` into
/// ungrouped words and the stored binding silently dispatches garbage.
/// Tokens with embedded single quotes use double quotes (single-quoted
/// content is fully literal in `parse_command_line`, so `'\''` cannot work);
/// everything else uses single quotes. Chain separators (`;`, `\;`) are left
/// bare so command chaining still splits.
/// Value that follows `flag` in `args`, e.g. the `-s` of `swap-window -s 2`.
/// Kept RAW: window target specs are resolved on the server, which is the only
/// place that knows whether `+1` is a window, an index or a name (issue #602).
fn flag_value(args: &[&str], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].to_string())
}

/// Whether `arg` is a list-keys flag cluster (tmux "1aF:NO:P:rT:") that
/// carries flag `c`.
fn list_keys_flag(arg: &str, c: char) -> bool {
    arg.len() > 1
        && arg.starts_with('-')
        && arg != "--"
        && arg[1..].chars().all(|ch| "1aNr".contains(ch))
        && arg[1..].contains(c)
}

/// The table and key of one `bind-key [-r] -T <table> <key> <command>` line.
fn list_keys_line_table_key(line: &str) -> (Option<&str>, Option<&str>) {
    let mut it = line.split(' ').skip_while(|t| *t != "-T").skip(1);
    (it.next(), it.next())
}

/// `list-keys -N`'s operands: (`-T` table, `-a`, key filter).
fn list_keys_notes_args(args: &[&str]) -> (Option<String>, bool, Option<String>) {
    let mut table = None;
    let mut all = false;
    let mut key = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-T" => { table = args.get(i + 1).map(|s| s.to_string()); i += 2; continue; }
            "-F" | "-O" | "-P" | "-t" => { i += 2; continue; }
            a if list_keys_flag(a, 'a') => all = true,
            a if a.starts_with('-') && a.len() > 1 => {}
            a => { if key.is_none() { key = Some(a.to_string()); } }
        }
        i += 1;
    }
    (table, all, key)
}

/// The request a `set-hook [-agu] [-t target] hook [command]` makes, or
/// `None` when there is nothing to do. Flags are read only before the hook
/// name and `--` ends them (#583). A single command argument is the command
/// string itself, as tmux takes it (cmd-set-option.c parses the hook's value
/// as a command), so `set-hook -g h 'new-window -d'` stores
/// `new-window -d`; it used to keep the quotes on the control mode and raw
/// TCP routes and the hook ran nothing. Several words are re-quoted so they
/// parse back the same.
fn set_hook_request(args: &[&str]) -> Option<CtrlReq> {
    let mut flag_chars = String::new();
    let mut non_flag: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if non_flag.is_empty() {
            if a == "--" {
                non_flag.extend(args[i + 1..].iter().copied());
                break;
            }
            if a == "-t" {
                i += 2;
                continue;
            }
            if a.starts_with('-') && a.len() > 1 && a.chars().skip(1).all(|c| c.is_ascii_alphabetic()) {
                flag_chars.push_str(&a[1..]);
                i += 1;
                continue;
            }
        }
        non_flag.push(a);
        i += 1;
    }
    let name = non_flag.first()?.to_string();
    if flag_chars.contains('u') {
        return Some(CtrlReq::RemoveHook(name));
    }
    let command = match &non_flag[1..] {
        [] => return None,
        [one] => one.to_string(),
        words => requote_command_tail(words),
    };
    Some(if flag_chars.contains('a') {
        CtrlReq::AppendHook(name, command)
    } else {
        CtrlReq::SetHook(name, command)
    })
}

fn requote_command_tail(args: &[&str]) -> String {
    // One encoder for the argv form, the one build_command decodes (#794).
    crate::pane::requote_argv(args)
}

/// The positional `shell-command` operand of `respawn-pane` / `respawn-window`
/// (tmux takes it plain, not behind `--`). Everything that is a flag, or the
/// value of the one value-taking flag left in `args` (`-c`; `-t` is stripped
/// upstream by `without_outer_target`), is skipped.
fn respawn_positional_command(args: &[&str]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "-c" || a == "-e" || a == "-t" {
            i += 2;
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        let s = a.trim_matches('"').trim().to_string();
        return if s.is_empty() { None } else { Some(s) };
    }
    None
}

/// Every `-e KEY=VALUE` of a spawning command, in order. Like tmux's
/// `environ_put`, a value without `=` is ignored. Used by respawn-pane and
/// respawn-window (#708); new-window and split-window collect theirs inline.
pub(crate) fn env_flag_values(args: &[&str]) -> Vec<(String, String)> {
    args.windows(2)
        .filter(|w| w[0] == "-e")
        .filter_map(|w| w[1].trim_matches('"').split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn without_outer_target<'a>(cmd: &str, args: &[&'a str]) -> Vec<&'a str> {
    let scan_end = crate::cli::outer_target_scan_end(cmd, args);
    let mut filtered = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        if i < scan_end && args[i] == "-t" {
            i += 2;
        } else {
            filtered.push(args[i]);
            i += 1;
        }
    }
    filtered
}

/// The window selecting control requests one `select-window` emits, decided in
/// ONE place (issue #690).
///
/// The bug was two places deciding: the generic `-t` focus block sent a
/// permanent `FocusWindow` for the target, and this command's own arm sent a
/// `SelectWindow` for the same window right after it.  Every request carries
/// its own hook slot in the server loop, so a single `select-window` ran
/// `after-select-window` twice, and a hook that pasted landed its text twice.
/// tmux fires a command's after hook once, from the command queue:
/// cmd-queue.c `cmdq_fire_command` calls
/// `cmdq_insert_hook(s, item, &fs, "after-%s", name)` once, after the command's
/// exec returns.
///
/// The choice between the forms is tmux's, from cmd-select-window.c: `-n`,
/// then `-p`, then `-l` are each the whole operation, and only when none of
/// them is given does the `-t` target decide.  A plain index goes through
/// `SelectWindow`, which is also the request that fires
/// `before-select-window`, and fires it before the switch.  An `@id` and a
/// window name keep the focus requests they have always used.
///
/// `args` must already have had the outer `-t` removed by
/// `without_outer_target`, so the only positional left is tmux's window
/// number.
pub(crate) fn select_window_requests(
    args: &[&str],
    raw_target: Option<&str>,
    target_win: Option<usize>,
    target_win_is_id: bool,
    target_win_name: Option<&str>,
) -> (Vec<CtrlReq>, Option<mpsc::Receiver<Result<(), String>>>) {
    if args.iter().any(|a| *a == "-n") {
        return (vec![CtrlReq::NextWindow], None);
    }
    if args.iter().any(|a| *a == "-p") {
        return (vec![CtrlReq::PrevWindow], None);
    }
    if args.iter().any(|a| *a == "-l") {
        let (s, r) = mpsc::channel();
        return (vec![CtrlReq::LastWindow { resp: Some(s) }], Some(r));
    }
    // Issue #693 item 4: everything the `-t` can name goes through the ONE
    // resolver move-window and swap-window have used since #602, which is
    // tmux's `cmd_find_get_window_with_session` in the same order: `@id`, the
    // `+N`/`-N` offsets, the `!`/`^`/`$` symbols and their braced spellings,
    // a display index, then an exact window name. `select-window` never
    // called it, so `-t +1` was read as the literal index 1 and `-t !`,
    // `-t {end}`, `-t -` and `-t +` died on the CLI as session names.
    if let Some(spec) = select_window_spec(raw_target) {
        let (s, r) = mpsc::channel();
        return (vec![CtrlReq::SelectWindowSpec { spec, resp: s }], Some(r));
    }
    if target_win_is_id {
        // #497: an @id target must never be re-sent as an INDEX.
        return (match target_win {
            Some(id) => vec![CtrlReq::FocusWindowById(id)],
            None => Vec::new(),
        }, None);
    }
    let idx = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .and_then(|s| s.parse::<usize>().ok())
        .or(target_win);
    if let Some(idx) = idx {
        return (vec![CtrlReq::SelectWindow(idx)], None);
    }
    (match target_win_name {
        Some(name) => vec![CtrlReq::FocusWindowByName(name.to_string())],
        None => Vec::new(),
    }, None)
}

/// The raw `-t` of a `select-window` that names a WINDOW, so the server side
/// resolver decides what it means; None when it names a session (or nothing),
/// which psmux has always routed by session name and which #693 does not
/// change.
///
/// tmux tries the token as a window first and only falls back to a session
/// (cmd-find.c:344-350); psmux runs one server per session, so the session
/// fallback is the CLI's routing step and a bare NAME never reaches here as a
/// window.
pub(crate) fn select_window_spec(raw_target: Option<&str>) -> Option<String> {
    let t = raw_target?.trim();
    if t.is_empty() { return None; }
    // A `.pane` component belongs to the pane focus, not to the window
    // resolver: `select-window -t @2.0` and `-t sess:1.0` both name window
    // @2 / window 1 (#497).  Only an UNAMBIGUOUS suffix is split off, because
    // a window NAME may legitimately contain a dot, which is the same rule
    // `cli_validate_window_pane_target` uses.
    let (prefix, rest) = match t.find(':') {
        Some(c) => (&t[..=c], &t[c + 1..]),
        None => ("", t),
    };
    let rest = rest.trim();
    let window_part = match rest.rfind('.') {
        Some(d) => {
            let pane = &rest[d + 1..];
            let unambiguous = !pane.is_empty()
                && (pane.starts_with('%')
                    || pane.chars().all(|c| c.is_ascii_digit())
                    || matches!(pane, "+" | "-"));
            if unambiguous { &rest[..d] } else { rest }
        }
        None => rest,
    };
    let window_part = window_part.trim();
    // `sess:` with nothing after it means that session's current window,
    // which is where we already are.
    if window_part.is_empty() { return None; }
    if !prefix.is_empty() {
        return Some(format!("{}{}", prefix, window_part));
    }
    if window_part.starts_with('@') || crate::cli::bare_target_names_a_window(window_part) {
        return Some(window_part.to_string());
    }
    None
}

/// Does this `select-pane` carry an operation of its own, one that
/// `CtrlReq::SelectPane` will perform and whose `after-select-pane` it will
/// fire?
///
/// tmux's cmd-select-pane.c runs exactly one of these per command and returns
/// before the generic target activation: `-l`/last-pane at :164, `-m`/`-M` at
/// :100, `-e`/`-d` at :180, `-T`/`-P` at :247. The relative `:.+` / `:.-`
/// forms and `-U/-D/-L/-R` fall through to the activation at :274. Either way
/// the command is ONE operation and fires its after hook at most once (#690,
/// cmd-queue.c `cmdq_fire_command`), so when this is true the `-t` request
/// must not fire it as well.
pub(crate) fn select_pane_has_own_operation(args: &[&str], raw_target: Option<&str>) -> bool {
    let relative = raw_target.map_or(false, |t| {
        t.contains(".+") || t.contains(".-") || t == "+" || t == "-" || t == ":.+" || t == ":.-"
    });
    relative
        || args.iter().any(|a| {
            matches!(*a, "-U" | "-D" | "-L" | "-R" | "-l" | "-m" | "-M" | "-e" | "-d")
        })
}

/// `select-pane -e` / `-d` as an input toggle: `Some((off, last))` when this
/// command sets (`-d`, off true) or clears (`-e`) the target pane's input-off
/// flag, `last` when `-l` moves that to the window's last pane.
///
/// tmux's cmd-select-pane.c tests `-l` first and toggles the LAST pane's
/// `PANE_INPUTOFF` there when `-e`/`-d` come with it; `-m`/`-M` come next and
/// return before `-e`/`-d` are looked at, and `-e` is tested before `-d`, so
/// `-e` wins when both are given.  A toggle also returns before `-T`/`-P`.
///
/// None for the forms psmux still runs as a NAVIGATION: a direction or a
/// relative `:.+`/`:.-` target with `-e`/`-d`.  tmux toggles the pane in that
/// direction there without selecting it; psmux keeps its long standing
/// behaviour (it moves) rather than guess, since none of its directional code
/// can name the neighbour without moving to it.
///
/// The flags are read the way tmux's getopt reads `DdegLlMmP:RT:t:UZ`, so a
/// `-T` title, `-P` style or `-t` target VALUE is never taken for a flag:
/// `select-pane -T -d` sets the title "-d" and leaves input alone.
pub(crate) fn select_pane_input_toggle(args: &[&str], raw_target: Option<&str>) -> Option<(bool, bool)> {
    let flags = select_pane_flags(args);
    let has = |f: char| flags.contains(&f);
    let enable = has('e');
    let disable = has('d');
    if !enable && !disable {
        return None;
    }
    let last = has('l');
    if !last {
        let relative = raw_target.is_some_and(|t| {
            t.contains(".+") || t.contains(".-") || t == "+" || t == "-"
        });
        if relative || ['U', 'D', 'L', 'R', 'm', 'M'].iter().any(|f| has(*f)) {
            return None;
        }
    }
    Some((!enable, last))
}

/// `last-pane -e` / `-d`: `Some(off)` when the command toggles the last pane's
/// input instead of selecting it.  tmux's last-pane runs cmd_select_pane_exec,
/// whose `-l` branch tests `-e` before `-d` (cmd-select-pane.c:178-186).  Its
/// template `det:Z` takes a value only for `-t`, which `select_pane_flags`
/// steps over (`-P`/`-T` never reach here: the flag validator refuses them).
pub(crate) fn last_pane_input_toggle(args: &[&str]) -> Option<bool> {
    let flags = select_pane_flags(args);
    if flags.contains(&'e') {
        Some(false)
    } else if flags.contains(&'d') {
        Some(true)
    } else {
        None
    }
}

/// The target a `CtrlReq::SetPaneInputOff` carries: the validated one, plus a
/// `%N` pane id the route deliberately left unvalidated because it may name a
/// floating pane, which `ValidateTarget` cannot see (`set_pane_input_off`
/// resolves it, and reports tmux's `can't find pane` for a bad one).
fn input_toggle_target(
    validated: Option<&crate::types::TempTarget>,
    pane: Option<usize>,
    pane_is_id: bool,
) -> Option<crate::types::TempTarget> {
    let mut target = validated.cloned();
    if let (Some(p), true) = (pane, pane_is_id) {
        let t = target.get_or_insert_with(Default::default);
        t.pane = Some(p);
        t.pane_is_id = true;
    }
    target
}

/// The boolean flags of a `select-pane` argument list, parsed like tmux's
/// getopt string `DdegLlMmP:RT:t:UZ` (cmd-select-pane.c): flags may be
/// clustered (`-dt %1`), `-P`, `-T` and `-t` consume the rest of their word
/// or the next argument as a value, and `--` or the first non-option ends
/// the options.
fn select_pane_flags(args: &[&str]) -> Vec<char> {
    select_pane_getopt(args).0
}

/// The `-t` value of a `select-pane` / `last-pane` argument list, read by the
/// same getopt walk as [`select_pane_flags`], so `-dt %1` and `-t%1` name
/// `%1` and a `-T -t` title is not a target.
pub(crate) fn select_pane_target_arg<'a>(args: &[&'a str]) -> Option<&'a str> {
    select_pane_getopt(args).1
}

fn select_pane_getopt<'a>(args: &[&'a str]) -> (Vec<char>, Option<&'a str>) {
    let mut flags = Vec::new();
    let mut target = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if a == "--" || !a.starts_with('-') || a.len() < 2 {
            break;
        }
        for (pos, c) in a.char_indices().skip(1) {
            if matches!(c, 'P' | 'T' | 't') {
                // The value is the rest of this word, or else the next one.
                let rest = &a[pos + c.len_utf8()..];
                let value = if rest.is_empty() {
                    i += 1;
                    args.get(i).copied()
                } else {
                    Some(rest)
                };
                if c == 't' {
                    target = value;
                }
                break;
            }
            flags.push(c);
        }
        i += 1;
    }
    (flags, target)
}

/// The single request a `select-pane`'s `-t` emits (issue #691).
///
/// One place decides, the way `select_window_requests` does for #690: the
/// generic `-t` focus block no longer sends a `FocusWindow` (which names
/// `after-select-window`, the wrong hook: tmux's cmd-select-pane.c never fires
/// it) plus a pane focus that names no hook at all.
pub(crate) fn select_pane_requests(
    args: &[&str],
    raw_target: Option<&str>,
    target_win: Option<usize>,
    target_win_is_id: bool,
    target_win_name: Option<&str>,
    target_pane: Option<usize>,
    pane_is_id: bool,
) -> Vec<CtrlReq> {
    if target_win.is_none() && target_win_name.is_none() && target_pane.is_none() {
        return Vec::new();
    }
    vec![CtrlReq::SelectPaneTarget {
        win: target_win,
        win_is_id: target_win_is_id,
        win_name: target_win_name.map(|s| s.to_string()),
        pane: target_pane,
        pane_is_id,
        fire_hook: !select_pane_has_own_operation(args, raw_target),
    }]
}

/// Walk the sub-commands produced by `split_top_level_semicolons` and merge
/// any consecutive run of `send`/`send-keys` commands targeting the same
/// pane into a single synthesized `send -lt <target> <bytes>` command.
/// This keeps multi-byte VT sequences (arrows, function keys, etc.) atomic
/// when they reach the shell PTY.
///
/// Every merged command after the first leaves a `COALESCED_SEND_REPLY`
/// placeholder behind, so the client still gets one empty `%begin`/`%end`
/// block per command it sent: tmux guards each queue item on its own
/// (cmd-queue.c:592 and :652), and a client pairs replies to commands by
/// count (issue #799).
fn coalesce_send_commands(parts: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(parts.len());
    let mut acc: Vec<u8> = Vec::new();
    let mut acc_target: Option<String> = None;
    let mut merged: Vec<String> = Vec::new();

    fn flush(out: &mut Vec<String>, acc: &mut Vec<u8>, target: &mut Option<String>, merged: &mut Vec<String>) {
        let mut parts = merged.drain(..);
        let Some(first) = parts.next() else { return };
        if acc.is_empty() {
            // Nothing to write (only empty operands): the first command runs
            // as sent, so it is still validated and answered.
            out.push(first);
        } else {
            // Re-emit as `send -H`: a byte-exact, pure-ASCII command line.  The
            // previous `send -l '<latin1>'` form ran every byte through a latin-1
            // char round trip, which double-encoded anything the decoder had
            // already turned into UTF-8 (a `send 0x4e2d` arrived as "ä¸­").
            let hex: Vec<String> = acc.iter().map(|b| format!("{:02x}", b)).collect();
            let line = match target.as_deref() {
                Some(t) if !t.is_empty() => format!("send -H -t {} {}", t, hex.join(" ")),
                _ => format!("send -H {}", hex.join(" ")),
            };
            out.push(line);
        }
        out.extend(parts.map(|part| format!("{}{}", COALESCED_SEND_REPLY, part)));
        acc.clear();
        *target = None;
    }

    for part in parts {
        match decode_send_command(&part) {
            Some((tgt, bytes)) => {
                let target_match = merged.is_empty()
                    || acc_target.as_deref() == Some(tgt.as_str());
                if !target_match {
                    flush(&mut out, &mut acc, &mut acc_target, &mut merged);
                }
                if merged.is_empty() { acc_target = Some(tgt); }
                acc.extend_from_slice(&bytes);
                merged.push(part);
            }
            None => {
                flush(&mut out, &mut acc, &mut acc_target, &mut merged);
                out.push(part);
            }
        }
    }
    flush(&mut out, &mut acc, &mut acc_target, &mut merged);
    out
}

/// Marks a sub-command that `coalesce_send_commands` merged into the send
/// before it: its keys are already written, so it only answers an empty
/// block. The rest of the string is the command as the client sent it.
const COALESCED_SEND_REPLY: &str = "\u{0001}COALESCED-SEND\u{0001}";

/// tmux's `break-pane` template when `-P` is given without `-F`
/// (cmd-break-pane.c:29, BREAK_PANE_TEMPLATE).
pub const BREAK_PANE_TEMPLATE: &str = "#{session_name}:#{window_index}.#{pane_index}";

/// Split a `break-pane` command line into the request the server applies and
/// the `-P` format, if any.
///
/// tmux's flag set is `"abdPF:n:s:t:"` (cmd-break-pane.c:37). psmux parsed NONE
/// of it: the whole command reached the server as a bare `BreakPane`, so `-d`
/// did nothing and `-s` silently broke the ACTIVE pane (issue #689). Shared by
/// the plain CLI route and the control / in-TUI route so both agree.
///
/// `outer_target` is the `-t` value the generic target parser already peeled
/// off the command line; for break-pane it is a DESTINATION window, never a
/// pane to operate on.
pub fn parse_break_pane_args(
    args: &[&str],
    outer_target: Option<&str>,
) -> (crate::window_ops::BreakPaneRequest, Option<String>) {
    let mut req = crate::window_ops::BreakPaneRequest::default();
    req.dst = outer_target.map(|t| t.to_string());
    let mut print = false;
    let mut format: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-d" => { req.detach = true; }
            "-a" => { req.after = true; }
            "-b" => { req.before = true; }
            "-P" => { print = true; }
            "-s" => { if let Some(v) = args.get(i + 1) { req.src = Some(v.trim_matches('"').to_string()); i += 1; } }
            "-t" => { if let Some(v) = args.get(i + 1) { req.dst = Some(v.trim_matches('"').to_string()); i += 1; } }
            "-n" => { if let Some(v) = args.get(i + 1) { req.name = Some(v.trim_matches('"').to_string()); i += 1; } }
            "-F" => { if let Some(v) = args.get(i + 1) { format = Some(v.trim_matches('"').to_string()); i += 1; } }
            _ => {}
        }
        i += 1;
    }
    let print = if print {
        Some(format.unwrap_or_else(|| BREAK_PANE_TEMPLATE.to_string()))
    } else {
        None
    };
    (req, print)
}

/// A `link-window` command line, shared by the plain CLI route and the
/// control / in-TUI route so both agree (issue #693 item 1).
///
/// tmux's flag set is `"abdks:t:"` (cmd-move-window.c:49), `-s` is
/// `CMD_FIND_WINDOW` and `-t` is the `CMD_FIND_WINDOW / CMD_FIND_WINDOW_INDEX`
/// destination (:83), so the destination need not exist yet. psmux read both
/// as `trim_start_matches(':').parse::<usize>()`, which could not read a
/// session qualified `-s sess:0` at all.
///
/// `outer_target` is the `-t` value the generic target parser already peeled
/// off the command line.
pub struct LinkWindowArgs {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub detach: bool,
    pub kill: bool,
    pub after: bool,
    pub before: bool,
}

pub fn parse_link_window_args(args: &[&str], outer_target: Option<&str>) -> LinkWindowArgs {
    let mut out = LinkWindowArgs {
        src: None,
        dst: outer_target.map(|t| t.to_string()),
        detach: false,
        kill: false,
        after: false,
        before: false,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-d" => out.detach = true,
            "-k" => out.kill = true,
            "-a" => out.after = true,
            "-b" => out.before = true,
            "-s" => { if let Some(v) = args.get(i + 1) { out.src = Some(v.trim_matches('"').to_string()); i += 1; } }
            "-t" => { if let Some(v) = args.get(i + 1) { out.dst = Some(v.trim_matches('"').to_string()); i += 1; } }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The `-t` of an `unlink-window` (tmux flag set `"kt:"`,
/// cmd-kill-window.c:51). It never reached the arm before, so the command
/// always unlinked the ACTIVE window (issue #693 item 2).
pub fn parse_unlink_window_target(args: &[&str], outer_target: Option<&str>) -> Option<String> {
    let mut target = outer_target.map(|t| t.to_string());
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-t" {
            if let Some(v) = args.get(i + 1) { target = Some(v.trim_matches('"').to_string()); i += 1; }
        }
        i += 1;
    }
    target
}

/// Parsed `new-pane` flags. Semantics match tmux `cmd-split-window.c`:
/// `-x`=width, `-y`=height, `-X`=x-position, `-Y`=y-position, `-B`=border-lines,
/// `-T`=title, `-c`=start-directory, `-d`=detached, `-P`=print pane id.
struct ParsedNewPane {
    command: String,
    x: Option<u16>,     // -X x-position
    y: Option<u16>,     // -Y y-position
    w: Option<u16>,     // -x width
    h: Option<u16>,     // -y height
    border: String,     // -B
    title: Option<String>, // -T
    start_dir: Option<String>, // -c
    detached: bool,     // -d
    print: bool,        // -P
    empty: bool,        // -E (empty pane, no command)
    format: Option<String>, // -F (the -P template)
    extra: crate::types::FloatExtras, // -e, -s, -S, -R
    /// A tmux new-pane flag psmux's floating panes cannot honour (-k, -m,
    /// -I, -O, -M, -W, -L, -Z). Refused with a message, never ignored.
    unsupported: Option<char>,
}

impl ParsedNewPane {
    /// The refusal for an `unsupported` flag, or None.
    fn refusal(&self) -> Option<String> {
        let f = self.unsupported?;
        let what = match f {
            'k' | 'm' => "remain-on-exit for a floating pane",
            'I' => "an input pane (-I)",
            'O' => "a modal pane (-O)",
            'M' => "mouse placement (-M)",
            'W' => "waiting for the pane's command (-W)",
            'L' => "a tiled pane from new-pane (use split-window)",
            _ => "this flag",
        };
        Some(format!("new-pane -{}: {} is not supported", f, what))
    }
}

/// What `new-pane -P` prints: the new pane's `%id`, or `-F`'s format expanded
/// for that pane (tmux cmd-split-window.c expands the -P template against the
/// new pane). `send` delivers the expansion request to the server loop.
fn new_pane_print_text(id_text: String, format: Option<&str>, send: impl Fn(CtrlReq)) -> String {
    let Some(fmt) = format else { return id_text };
    let Some(id) = id_text.trim().strip_prefix('%').and_then(|n| n.parse::<usize>().ok()) else {
        return id_text;
    };
    let (rtx, rrx) = mpsc::channel::<String>();
    send(CtrlReq::DisplayMessageById(rtx, fmt.to_string(), id, false, None, None, Default::default()));
    rrx.recv_timeout(Duration::from_secs(2)).unwrap_or(id_text)
}

/// tmux's new-pane flags that take a value (cmd-split-window.c,
/// "bB:c:de:EfF:hIkl:LMm:Op:PR:s:S:t:T:vWx:X:y:Y:Z").
const NEW_PANE_VALUE_FLAGS: &str = "BceFlmpRsStTxXyY";

/// Parse `new-pane` the way tmux's getopt does: options up to the first
/// operand or `--`, every operand after that is the command.
///
/// Issue #793 follow up: the old scan matched a handful of known flags
/// anywhere and joined everything else into the command, so the value of a
/// flag it did not know became the command. `new-pane -P -F '#{pane_id}'`
/// started `pwsh -Command "-F #{pane_id}"`, which exited at once and took
/// the floating pane with it, after -P had already printed its id.
fn parse_new_pane_args(args: &[&str]) -> ParsedNewPane {
    let mut detached = false;
    let mut print = false;
    let mut empty = false;
    let mut border = String::new();
    let mut title: Option<String> = None;
    let mut start_dir: Option<String> = None;
    let mut format: Option<String> = None;
    let mut extra = crate::types::FloatExtras::default();
    let mut unsupported: Option<char> = None;
    // x/y = POSITION (from -X/-Y); w/h = SIZE (from -x/-y). tmux ordering.
    let (mut x, mut y, mut w, mut h): (Option<u16>, Option<u16>, Option<u16>, Option<u16>) = (None, None, None, None);
    // Where the command starts, and whether a `--` introduced it.
    let mut cmd_from = args.len();
    let mut after_marker = false;
    let mut i = 0;
    'opts: while i < args.len() {
        let a = args[i];
        if a == "--" {
            cmd_from = i + 1;
            after_marker = true;
            break;
        }
        if !a.starts_with('-') || a.len() < 2 {
            cmd_from = i;
            break;
        }
        let flags: Vec<char> = a[1..].chars().collect();
        let mut j = 0;
        while j < flags.len() {
            let f = flags[j];
            if NEW_PANE_VALUE_FLAGS.contains(f) {
                // The value is the rest of this token, or the next argument.
                let rest: String = flags[j + 1..].iter().collect();
                let value: Option<String> = if !rest.is_empty() {
                    Some(rest)
                } else {
                    i += 1;
                    args.get(i).map(|v| v.to_string())
                };
                let v = value.unwrap_or_default();
                let v = v.trim_matches('"').to_string();
                match f {
                    'B' => border = v,
                    'T' => title = Some(v),
                    'c' => start_dir = Some(v),
                    'F' => format = Some(v),
                    'x' => w = v.parse().ok(),
                    'y' => h = v.parse().ok(),
                    'X' => x = v.parse().ok(),
                    'Y' => y = v.parse().ok(),
                    // -e KEY=VALUE, repeatable: the new process's environment.
                    'e' => {
                        if let Some((k, val)) = v.split_once('=') {
                            extra.env.push((k.to_string(), val.to_string()));
                        }
                    }
                    's' => extra.styles.style = v,
                    'S' => extra.styles.active_border = v,
                    'R' => extra.styles.border = v,
                    'm' => { unsupported.get_or_insert('m'); }
                    // -l/-p size a TILED split; tmux's floating cell reads
                    // only -x/-y/-X/-Y (layout_floating_args_parse), so for a
                    // floating pane they change nothing there either.
                    _ => {}
                }
                i += 1;
                continue 'opts;
            }
            match f {
                'd' => detached = true,
                'P' => print = true,
                'E' => empty = true,
                'k' | 'I' | 'O' | 'M' | 'W' | 'L' => { unsupported.get_or_insert(f); }
                // -b/-f/-h place a TILED split; a floating pane has no
                // neighbour to go before or beside, and -Z zooms only for a
                // modal pane (layout_get_floating_cell). tmux changes nothing
                // for them on a plain floating pane either.
                _ => {}
            }
            j += 1;
        }
        i += 1;
    }
    let tail = &args[cmd_from.min(args.len())..];
    // tmux parity (#582): a multi-token argv after `--` is exec'd directly
    // (tmux execvp), so keep the marker plus token boundaries for
    // build_command's argv decoder. A single token keeps tmux's string
    // semantics (shell route). Operands without `--` keep the string form
    // they always had.
    let command = if after_marker && tail.len() > 1 {
        format!("-- {}", requote_command_tail(tail))
    } else {
        tail.join(" ")
    };
    ParsedNewPane { command, x, y, w, h, border, title, start_dir, detached, print, empty, format, extra, unsupported }
}

/// The request for `list-clients [-F format] [-f filter]` (issue #724).
fn list_clients_request(args: &[&str], resp: mpsc::Sender<String>) -> CtrlReq {
    let fmt = extract_flag_value(args, "-F");
    let filter = extract_flag_value(args, "-f");
    match (fmt, filter) {
        (None, None) => CtrlReq::ListClients(resp),
        (fmt, filter) => CtrlReq::ListClientsFormat(
            resp,
            fmt.unwrap_or_else(|| crate::format::default_list_clients_format().to_string()),
            filter,
        ),
    }
}

/// The process on the other end of this loopback connection (issue #724).
fn connection_peer_pid(stream: &TcpStream) -> Option<u32> {
    let peer = stream.peer_addr().ok()?.port();
    let ours = stream.local_addr().ok()?.port();
    crate::platform::process_kill::loopback_peer_pid(peer, ours)
}

/// What a read only client (`attach -r`, tmux CLIENT_READONLY) may still send.
///
/// tmux drops a read only client's keys, pastes and mouse events
/// (server-client.c:1425, :1639, :1646) and refuses any bound or typed command
/// that is not flagged CMD_READONLY (key-bindings.c, server-client.c:2751):
/// attach-session, copy-mode, detach-client, list-clients, send-keys -X and
/// switch-client. The rest of this list is psmux's own client protocol (frames,
/// size, focus and identity reports) and queries that change nothing, without
/// which the client could not draw at all.
pub(crate) fn readonly_client_may_run(cmd: &str) -> bool {
    matches!(cmd,
        // psmux client protocol
        "dump-state" | "dump" | "dump-layout" | "session-info" | "client-size"
        | "host-colors" | "client-attach" | "client-detach" | "client-last-session"
        | "client-flags" | "client-environ" | "focus-in" | "focus-out" | "prefix-begin" | "prefix-end"
        | "overlay-close" | "window-layout" | "window-dump" | "list-tree"
        // tmux CMD_READONLY commands
        | "attach-session" | "attach" | "detach-client" | "detach"
        | "switch-client" | "switchc" | "list-clients" | "lsc"
        | "copy-mode" | "copy-enter" | "copy-move" | "copy-anchor"
        | "rectangle-toggle" | "copy-mode-page-up" | "copy-yank"
        // overlays that only show something
        | "display-panes" | "displayp" | "menu-navigate"
        // queries
        | "list-windows" | "lsw" | "list-panes" | "lsp" | "list-sessions" | "ls"
        | "list-buffers" | "lsb" | "list-keys" | "lsk" | "list-commands" | "lscm"
        | "has-session" | "show-buffer" | "showb" | "show-environment" | "showenv"
        | "show-hooks" | "show-messages" | "showmsgs" | "show-options" | "show"
        | "show-window-options" | "showw" | "server-info" | "info"
    )
}

/// Did a client read merely not complete, rather than fail?
///
/// Every client connection is read with a `set_read_timeout` budget, and the
/// expiry of that budget is what keeps a persistent reader looping instead of
/// blocking. It is **not** a dead client, and the three reader loops must not
/// treat it as one.
///
/// `WouldBlock`/`TimedOut` are not the only shapes that expiry takes. Rust
/// opens its sockets with `WSA_FLAG_OVERLAPPED`, and on Windows a receive that
/// times out on such a socket can surface as `WSA_IO_PENDING` (os error 997,
/// "overlapped I/O operation is in progress"), which maps to
/// `ErrorKind::Uncategorized`. Treating that as fatal closed the connection of
/// a perfectly healthy idle desktop client; the client reconnected under a
/// fresh client id, and because a reconnect used not to re-report its size,
/// `window-size latest` could never size the window for it again (see
/// `tests-rs/test_client_size_after_reconnect.rs`). `WSAEINTR` (10004, which
/// Rust also leaves as `Uncategorized`) and `WSAETIMEDOUT` (10060) belong in the
/// same bucket: the read did not fail, it just did not complete.
fn is_read_retry(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    ) || matches!(e.raw_os_error(), Some(997) | Some(10004) | Some(10060))
}

/// Merge a run-shell child's stdout and stderr the way both the persistent and
/// the one-shot path render it: stdout first, a newline between the two when
/// neither already provides one.
pub(crate) fn run_shell_output_text(out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr_text = String::from_utf8_lossy(&out.stderr);
    if !stderr_text.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr_text);
    }
    text
}

/// Handle a single TCP connection from a client.
/// Parses auth, optional TARGET/PERSISTENT flags, then dispatches commands
/// to the main server event loop via the `tx` channel.
pub(crate) fn handle_connection(
    stream: TcpStream,
    tx: mpsc::Sender<CtrlReq>,
    session_key: &str,
    aliases: std::sync::Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>,
) {
let client_id = crate::types::next_client_id();
// Enable TCP_NODELAY for low-latency responses
let _ = stream.set_nodelay(true);
// Clone stream for writing, original goes into BufReader for reading
let mut write_stream = match stream.try_clone() {
    Ok(s) => ReplyStream::new(s),
    Err(_) => return,
};
// Every socket handle on this connection must be non-inheritable: the server
// spawns children with bInheritHandles=TRUE (pane shells via ConPTY,
// pipe-pane sinks, run-shell), and a long-lived child that inherits a dup of
// this socket pins the connection open past our close, so a client waiting
// for EOF-as-end-of-reply times out ("no response from server (timed out)"
// on `pipe-pane -o <sink> \; <cmd>`). try_clone creates a NEW socket handle,
// so the accept-time scrub in run_server does not cover the clones — scrub
// each one where it is made.
clear_inherit(&stream);
clear_inherit(&write_stream);

// Set initial timeout for auth (reduced from 5s - client sends immediately)
let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
let mut r = io::BufReader::new(stream);

// Read the authentication line
let mut auth_line = String::new();
if r.read_line(&mut auth_line).is_err() {
    auth_debug(&format!("client_id={} reject: auth read_line error/timeout", client_id));
    return;
}

// Verify session key
let auth_line = auth_line.trim();
if !auth_line.starts_with("AUTH ") {
    auth_debug(&format!("client_id={} reject: no AUTH prefix, line={:?}", client_id, auth_line));
    // Legacy client without auth - reject for security
    let _ = write_stream.write_all(b"ERROR: Authentication required\n");
    let _ = write_stream.flush();
    return;
}
let provided_key = auth_line.strip_prefix("AUTH ").unwrap_or("");
if provided_key != session_key {
    auth_debug(&format!(
        "client_id={} reject: key mismatch provided={:?} expected={:?}",
        client_id, provided_key, session_key
    ));
    let _ = write_stream.write_all(b"ERROR: Invalid session key\n");
    let _ = write_stream.flush();
    return;
}
// Auth successful - send OK and flush immediately
let _ = write_stream.write_all(b"OK\n");
let _ = write_stream.flush();

// Use a reasonable timeout for the first command after AUTH.
// Clients may have a small delay between AUTH and the actual command.
let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(2000)));

// Check for PERSISTENT flag and optional TARGET line
let mut persistent = false;
let mut resp_tx_opt: Option<mpsc::Sender<crate::types::WriterWake>> = None;
let mut global_target_win: Option<usize> = None;
let mut global_target_win_is_id = false;
let mut global_target_win_name: Option<String> = None;
let mut global_target_pane: Option<usize> = None;
let mut global_pane_is_id = false;
let mut line = String::new();
if r.read_line(&mut line).is_err() {
    return;
}

// Check if client requests persistent connection mode
if line.trim() == "PERSISTENT" {
    persistent = true;
    // Enable TCP_NODELAY for low-latency persistent connections
    let _ = r.get_ref().set_nodelay(true);
    let _ = write_stream.set_nodelay(true);
    // Use longer read timeout for persistent mode - client controls pacing
    let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(5000)));

    // Track this stream so the server can explicitly shut it down before
    // process::exit(0).  Without this, the client never gets EOF on
    // Windows loopback sockets.
    crate::types::register_persistent_stream(client_id, &write_stream);
    
    // Spawn a dedicated writer thread so the read loop never blocks
    // waiting for dump-state responses.  The read loop sends oneshot
    // receivers here; the writer thread waits for each response and
    // writes it to TCP in order.
    let mut ws_bg = write_stream.try_clone().unwrap();
    clear_inherit(&ws_bg);
    // Prevent the writer from blocking indefinitely when the client's TCP
    // receive buffer fills up (e.g. during a slow render). Without a write
    // timeout, a full socket causes write() to block forever, silently
    // freezing frame delivery. 5 s matches the command-response timeout.
    let _ = ws_bg.set_write_timeout(Some(Duration::from_secs(5)));
    let (resp_tx, resp_rx) = mpsc::channel::<crate::types::WriterWake>();

    // Register a frame slot for server-pushed frames (event-driven rendering).
    // Slot holds at most one pending frame; push_frame() overwrites any
    // unconsumed frame because only the latest snapshot is worth rendering.
    let frame_slot = crate::types::register_frame_channel(client_id, resp_tx.clone());

    // Register a directive channel for queued directives (e.g. SWITCH).
    // Directives use a separate mpsc channel so they are never affected
    // by frame slot contention.
    let directive_rx = crate::types::register_directive_channel(client_id);

    // Clone the write socket so the Guard can shut down the connection when
    // the writer exits. shutdown(Both) on any clone affects the underlying
    // socket, causing the client's reader thread to receive EOF and reconnect
    // instead of hanging indefinitely with a frozen last frame.
    //
    // We use write_stream (not ws_bg) as the source so that even under fd
    // pressure the clone chain stays shallow.  If the clone fails here we
    // return early — the client immediately sees a closed connection and
    // reconnects, which is far better than hanging with no shutdown signal.
    let ws_shutdown = match write_stream.socket().try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    clear_inherit(&ws_shutdown);
    let tx_writer = tx.clone();
    std::thread::spawn(move || {
        // Deregister the frame channel and shut down the TCP connection when
        // this thread exits for any reason (write timeout, resp_rx disconnect,
        // etc.). The shutdown causes the client's reader thread to see EOF,
        // which triggers reconnect rather than leaving the client frozen.
        //
        // Also enqueue ClientDetach so a teardown observed *only* by the writer
        // path (write timeout / broken pipe / resp_rx disconnect, before the
        // reader loop reaches its EOF branch, or when the reader never set
        // `attached_sent`) still reaps the `client_registry` entry. The reaper
        // is idempotent, so if the reader path also fires ClientDetach for the
        // same `client_id` the second one is a harmless no-op. The client
        // reconnects under a *new* `client_id`, so reaping the old id here does
        // not remove the live reconnected client.
        struct Guard { client_id: u64, shutdown: std::net::TcpStream, tx: mpsc::Sender<CtrlReq> }
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
                crate::types::deregister_frame_channel(self.client_id);
                crate::types::remove_directive_channel(self.client_id);
                crate::types::deregister_persistent_stream(self.client_id);
                let _ = self.tx.send(CtrlReq::ClientDetach(self.client_id));
            }
        }
        let _guard = Guard { client_id, shutdown: ws_shutdown, tx: tx_writer };

        loop {
            // Each iteration drains all three sources in priority order
            // (directives, command responses, frame slot). The frame slot
            // is always checked, so command-response activity cannot
            // starve frame delivery.

            // 0. Drain queued directives (non-blocking).
            while let Ok(directive) = directive_rx.try_recv() {
                if write!(ws_bg, "{}\n", directive).is_err() { return; }
                if ws_bg.flush().is_err() { return; }
            }
            // 1. Drain pending command responses.
            //
            // This wait runs before the frame slot is checked below, so in
            // principle it delays a pushed frame by up to its duration. It was
            // A/B'd at 1ms against 5ms with n=60 on a quiet machine and moved
            // the keystroke median by 0.11ms (18.44 -> 18.33) — nothing, because
            // while a client is typing the echo frame reaches it as a
            // dump-state RESPONSE through the arm above rather than through the
            // slot. 5ms stays: 1ms would cost five times the wakeups per
            // attached client to buy noise.
            match resp_rx.recv_timeout(Duration::from_millis(5)) {
                // A pushed frame woke us; fall through to the slot check.
                Ok(crate::types::WriterWake::Frame) => {}
                Ok(crate::types::WriterWake::Resp(rrx)) => {
                    // Use a timeout matching the TCP write timeout (5 s) so the
                    // writer thread cannot block indefinitely if the command
                    // handler is slow or panics without sending a response.
                    // A timeout (or disconnected sender) is treated as fatal:
                    // break so Guard::drop fires, the client receives EOF, and
                    // reconnects cleanly rather than stalling on a silent drop.
                    match rrx.recv_timeout(Duration::from_secs(5)) {
                        Ok(text) => {
                            if write!(ws_bg, "{}\n", text).is_err() { return; }
                            if ws_bg.flush().is_err() { return; }
                        }
                        Err(_) => return,
                    }
                    while let Ok(next) = resp_rx.try_recv() {
                        let rrx = match next {
                            crate::types::WriterWake::Frame => continue,
                            crate::types::WriterWake::Resp(r) => r,
                        };
                        match rrx.recv_timeout(Duration::from_secs(5)) {
                            Ok(text) => {
                                if write!(ws_bg, "{}\n", text).is_err() { return; }
                                if ws_bg.flush().is_err() { return; }
                            }
                            Err(_) => return,
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // 2. Take the latest pushed frame from the slot.
            let frame = frame_slot.lock().ok().and_then(|mut slot| slot.take());
            if let Some(text) = frame {
                if write!(ws_bg, "{}\n", text).is_err() { return; }
                if ws_bg.flush().is_err() { return; }
                crate::pty_trace::mark_plain("t", text.len());
            }
        }
    });
    resp_tx_opt = Some(resp_tx);
    line.clear();
    if r.read_line(&mut line).is_err() {
        return;
    }
}

// Check for CONTROL, CONTROL_QUIET or CONTROL_NOECHO (control mode).
//
// CONTROL is `-C` on a console: the client switched the console's own echo
// off to read it raw, so the server writes each command line back in its
// place, which is what a tty shows a `tmux -C` user. CONTROL_QUIET is `-C`
// on a pipe or a pty that echoes for itself: tmux never echoes anything, and
// a client reading the stream sees only protocol lines (issue #857).
// CONTROL_NOECHO is `-CC`.
let control_echo = line.trim() == "CONTROL";
let control_quiet = line.trim() == "CONTROL_QUIET";
let control_noecho = line.trim() == "CONTROL_NOECHO";
if control_echo || control_quiet || control_noecho {
    let _ = r.get_ref().set_nodelay(true);
    let _ = write_stream.set_nodelay(true);
    let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(5000)));

    let ctrl_client_id = crate::types::next_client_id();
    crate::types::register_persistent_stream(ctrl_client_id, &write_stream);

    let (notif_tx, notif_rx) = std::sync::mpsc::channel::<ControlNotification>();

    // Wrap the write stream in a mutex so that the notification writer
    // thread and the command-response loop never interleave bytes on
    // the TCP socket.  Real tmux is single-threaded, so it never has
    // this problem; we need explicit synchronization.
    let write_lock = std::sync::Arc::new(std::sync::Mutex::new(write_stream));

    // Spawn notification writer thread BEFORE writing DCS or registering,
    // so it is ready to drain notifications as soon as they arrive.
    let ws_notif = write_lock.clone();
    // Set while a command is between its dispatch and its written reply, so
    // a detach's `%exit` follows the `%end` of the command that caused it,
    // as in tmux (server_client_check_exit runs after the command).
    let cmd_busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cmd_busy_notif = cmd_busy.clone();
    // Shared with the server loop: pause-after, paused panes, backlog age (#826).
    let output_gate = std::sync::Arc::new(control::ControlOutputGate::new(
        r.get_ref().try_clone().ok(),
    ));
    let gate_notif = output_gate.clone();
    let notif_thread = std::thread::spawn(move || {
        while let Ok(notif) = notif_rx.recv() {
            let is_detach = matches!(notif, ControlNotification::Detached);
            if is_detach {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while cmd_busy_notif.load(std::sync::atomic::Ordering::Acquire)
                    && std::time::Instant::now() < deadline
                {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            // %pause lines the server loop decided on while this thread was
            // busy (or blocked on a client that stopped reading).
            let mut lines: Vec<String> = gate_notif
                .take_pending_pauses()
                .into_iter()
                .map(|p| format!("%pause %{}", p))
                .collect();
            let mut is_exit = is_detach || matches!(notif, ControlNotification::Exit { .. });
            match notif {
                ControlNotification::PaneOutput { pane_id, data, seq, queued_at } => {
                    gate_notif.written(data.len());
                    match gate_notif.dequeue(pane_id, seq, queued_at, std::time::Instant::now()) {
                        control::GateWrite::Write { age_ms, extended: true } => {
                            lines.push(control::format_notification(
                                &ControlNotification::ExtendedOutput { pane_id, age_ms, data },
                            ));
                        }
                        control::GateWrite::Write { extended: false, .. } => {
                            lines.push(control::format_notification(
                                &ControlNotification::Output { pane_id, data },
                            ));
                        }
                        control::GateWrite::Pause => lines.push(format!("%pause %{}", pane_id)),
                        control::GateWrite::Drop => {}
                        control::GateWrite::TooFarBehind => {
                            lines.push("%exit too far behind".to_string());
                            is_exit = true;
                        }
                    }
                }
                // Only a wake-up: the line itself came from the gate above.
                ControlNotification::Pause { .. } => {}
                other => lines.push(control::format_notification(&other)),
            }
            let too_far = gate_notif.is_too_far_behind();
            if too_far && !is_exit {
                // Dropped by the server loop (control.c "too far behind").
                gate_notif.shutdown_stream();
                break;
            }
            if lines.is_empty() {
                continue;
            }
            let mut ws = match ws_notif.lock() {
                Ok(ws) => ws,
                Err(_) => break,
            };
            let mut failed = false;
            for formatted in &lines {
                if writeln!(ws, "{}", formatted).is_err() { failed = true; break; }
            }
            if failed || ws.flush().is_err() { break; }
            if too_far {
                drop(ws);
                gate_notif.shutdown_stream();
                break;
            }
            // Exit notification written — now signal the client to exit.
            // Writing %exit through the DCS stream (before TCP close) lets
            // iTerm2 receive it as a DCS message and close native windows
            // immediately.  Then we break so the server can close the TCP.
            if is_exit { break; }
        }
        // The client is gone from this session: it was detached (%exit
        // written above), the server is ending, or it left (stdin EOF, its
        // ControlDeregister dropped the sender after everything queued was
        // written). Close the connection so the client's reader sees EOF
        // now; nothing else ever closed it, so a client that closed stdin
        // waited out its 2 s drain before printing %exit (#825).
        if let Ok(ws) = ws_notif.lock() {
            let _ = ws.shutdown(std::net::Shutdown::Both);
        }
    });

    // For -CC (no-echo) mode, emit the DCS opening sequence "\033P1000p"
    // before anything else. Real tmux writes exactly 7 bytes with NO
    // trailing newline (tmux/control.c control_start()). The next bytes
    // on the wire are the first %begin line, so iTerm2 sees:
    //   \x1bP1000p%begin <time> 1 0\n%end <time> 1 0\n
    // which enters DCS mode and delivers "%begin ..." as the first
    // DCS data line.
    //
    // After the DCS, we emit a synthetic %begin/%end pair representing
    // the response to the implicit attach-session that bare `tmux -CC` runs.
    // Real tmux uses flags=0 (server-originated) here. iTerm2's parseBegin:
    //   - flag=1 (client-originated) requires a queued command in
    //     commandQueue_, otherwise aborts with "%begin with empty command
    //     queue" → tmuxHostDisconnected → "Detached".
    //   - flag=0 (server-originated) creates a synthetic currentCommand_
    //     and the matching %end fires tmuxInitialCommandDidCompleteSuccessfully
    //     which kicks off iTerm's tmux integration (phony-command, ping, etc.).
    //
    // `-C` gets the same block without the DCS: tmux answers the command
    // line command first in every control mode, and a client that pairs
    // replies with commands in order counts on it. psmux used to send a
    // blank line here instead, so every later reply landed on the command
    // before it (issue #857).
    {
        let mut ws = write_lock.lock().unwrap();
        let init_ts = chrono::Utc::now().timestamp();
        // One write: DCS opener (no newline) and the block together.
        let _ = ws.write_all(control::initial_reply_block(init_ts, control_noecho).as_bytes());
        let _ = ws.flush();
    }

    // NOW register with the server. This triggers emit_initial_state()
    // which sends %session-changed and other notifications through the
    // notification channel. Because we flushed the DCS + %begin/%end
    // above, those bytes are already in the kernel send buffer and will
    // arrive at the client before any notifications.
    let _ = tx.send(CtrlReq::ControlRegister {
        client_id: ctrl_client_id,
        echo: control_echo,
        notif_tx: notif_tx,
        // `#{client_pid}` of a -CC client is the process at the other end (#724).
        pid: connection_peer_pid(r.get_ref()),
        output_gate: output_gate.clone(),
    });

    // Control mode command loop: read lines, dispatch, wrap in %begin/%end/%error
    // Number 1 is the initial block above; tmux numbers every command
    // uniquely, so the first stdin command is 2, never a second 1.
    let mut cmd_counter: u64 = 1;
    let tx_ctrl = tx.clone();
    let aliases_ctrl = aliases.clone();
    // Queue of pending sub-command strings produced by splitting a single input
    // line on top-level `;` (real tmux does this in its command parser). iTerm2's
    // sendCommandList joins many commands with "; " into one wire line and
    // expects one %begin/%end pair per sub-command.
    let mut pending: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    loop {
        let trimmed_owned: String = if let Some(s) = pending.pop_front() {
            s
        } else {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) => break, // EOF
                Err(e) => {
                    // A read budget that expired is not a client that died.
                    if is_read_retry(&e) {
                        continue;
                    }
                    break;
                }
                Ok(_) => {}
            }

            // Strip leading ASCII control characters (e.g., \x03 Ctrl-C) that
            // iTerm2 sends when entering tmux gateway mode. Real tmux's command
            // parser silently ignores these; without this strip they get glued
            // onto the first command name (e.g. "\x03phony-command") and are
            // rejected as "unknown command", causing iTerm2 to detach.
            let trimmed_raw = line.trim();
            let stripped = trimmed_raw.trim_start_matches(|c: char| (c as u32) < 0x20 && c != '\t');
            if stripped.is_empty() { continue; }

            // Split on top-level `;` (respecting single/double quotes and `\`
            // escapes). If the line splits into multiple sub-commands, queue
            // the rest and process the first; this mirrors real tmux's parser
            // and is required for iTerm2's multi-command kickoff lines like
            // `show -v -q -t $0 @x; refresh-client -C 80,25; show ...`.
            let parts = split_top_level_semicolons(stripped);
            // tmux parses the whole line before it runs any of it, so an
            // unknown command anywhere in it is one `parse error` block and
            // nothing runs (control.c control_error, issue #813).
            let unknown = parts.iter().find_map(|part| {
                let name = parse_command_line(part).into_iter().next()?;
                let known = is_catalog_command(&name)
                    || is_control_extension_command(&name)
                    || aliases_ctrl.read().map(|m| m.contains_key(&name)).unwrap_or(false);
                (!known).then_some(name)
            });
            if let Some(name) = unknown {
                cmd_counter += 1;
                let ts = chrono::Utc::now().timestamp();
                let mut out = String::new();
                if control_echo {
                    out.push_str(stripped);
                    out.push('\n');
                }
                out.push_str(&control::format_begin(ts, cmd_counter));
                out.push('\n');
                out.push_str(&format!("parse error: unknown command: {}\n", name));
                out.push_str(&control::format_error(ts, cmd_counter));
                out.push('\n');
                let mut ws = write_lock.lock().unwrap();
                let _ = ws.write_all(out.as_bytes());
                let _ = ws.flush();
                continue;
            }
            let parts = coalesce_send_commands(parts);
            if parts.is_empty() { continue; }
            let mut iter = parts.into_iter();
            let first = iter.next().unwrap();
            for rest in iter { pending.push_back(rest); }
            first
        };
        let trimmed: &str = trimmed_owned.trim();
        if trimmed.is_empty() { continue; }

        cmd_counter += 1;
        let ts = chrono::Utc::now().timestamp();

        // A send merged into the one before it: its keys went out with that
        // write, and it answers its own empty block like any send (#799).
        if let Some(original) = trimmed.strip_prefix(COALESCED_SEND_REPLY) {
            let mut out = String::new();
            if control_echo {
                out.push_str(original);
                out.push('\n');
            }
            out.push_str(&control::format_begin(ts, cmd_counter));
            out.push('\n');
            out.push_str(&control::format_end(ts, cmd_counter));
            out.push('\n');
            let mut ws = write_lock.lock().unwrap();
            let _ = ws.write_all(out.as_bytes());
            let _ = ws.flush();
            continue;
        }

        // Dispatch the command (before acquiring write lock)
        // tmux's escapes inside double quotes: a control client escapes text
        // the tmux way, `"L1\nL2"` is two lines (issue #823).
        let parsed = crate::cli::normalize_flag_equals(
            crate::commands::parse_command_line_tmux(trimmed));
        let raw_cmd = parsed.first().map(|s| s.as_str()).unwrap_or("");

        if raw_cmd.is_empty() {
            let mut out = String::new();
            if control_echo {
                out.push_str(trimmed);
                out.push('\n');
            }
            out.push_str(&control::format_begin(ts, cmd_counter));
            out.push('\n');
            out.push_str(&control::format_end(ts, cmd_counter));
            out.push('\n');
            let mut ws = write_lock.lock().unwrap();
            let _ = ws.write_all(out.as_bytes());
            let _ = ws.flush();
            continue;
        }

        // Check aliases
        let alias_expanded = if let Ok(map) = aliases_ctrl.read() {
            map.get(raw_cmd).cloned()
        } else { None };

        let (cmd_name, cmd_args): (&str, Vec<&str>) = if let Some(ref expanded) = alias_expanded {
            let parts: Vec<&str> = expanded.split_whitespace().collect();
            let mut all: Vec<&str> = parts[1..].to_vec();
            all.extend(parsed.iter().skip(1).map(|s| s.as_str()));
            (parts.first().copied().unwrap_or(raw_cmd), all)
        } else {
            (raw_cmd, parsed.iter().skip(1).map(|s| s.as_str()).collect())
        };
        // Clustered select-pane / last-pane flags, as in the one-shot route.
        let ctrl_cluster_split: Vec<String>;
        let cmd_args: Vec<&str> = if matches!(cmd_name, "select-pane" | "selectp" | "last-pane" | "lastp") {
            ctrl_cluster_split = crate::cli::expand_flag_clusters(cmd_name, &cmd_args);
            ctrl_cluster_split.iter().map(|s| s.as_str()).collect()
        } else {
            cmd_args
        };
        // A bare `-t` name, resolved as on the attached route (see
        // bare_target_rewrite); an error is this command's %error.
        let ctrl_resolved_args: Vec<String>;
        let mut ctrl_target_err: Option<String> = None;
        let cmd_args: Vec<&str> = match bare_target_rewrite(cmd_name, &cmd_args, &tx_ctrl) {
            Ok(Some(new)) => {
                ctrl_resolved_args = new;
                ctrl_resolved_args.iter().map(String::as_str).collect()
            }
            Ok(None) => cmd_args,
            Err(e) => {
                ctrl_target_err = Some(e);
                cmd_args
            }
        };

        // Parse -t from command args
        let mut ctrl_target_win: Option<usize> = None;
        let mut ctrl_target_win_is_id = false;
        let mut ctrl_target_win_name: Option<String> = None;
        let mut ctrl_target_pane: Option<usize> = None;
        let mut ctrl_pane_is_id = false;
        let mut ctrl_raw_target: Option<String> = None;
        let ctrl_set_option = matches!(
            cmd_name,
            "set-option" | "set" | "set-window-option" | "setw"
        );
        let ctrl_set_window_option =
            matches!(cmd_name, "set-window-option" | "setw");
        let ctrl_set_args = ctrl_set_option
            .then(|| parse_set_option_args(&cmd_args));
        if let Some(parsed_set) = ctrl_set_args.as_ref() {
            if let Some(value) = parsed_set.target {
                ctrl_raw_target =
                    Some(crate::cli::strip_exact_match_prefix(value).to_string());
                let parsed_target = parse_target(value);
                if parsed_target.window.is_some() {
                    ctrl_target_win = parsed_target.window;
                    ctrl_target_win_is_id = parsed_target.window_is_id;
                    ctrl_target_win_name = None;
                } else if parsed_target.window_name.is_some() {
                    ctrl_target_win_name = parsed_target.window_name;
                    ctrl_target_win = None;
                    ctrl_target_win_is_id = false;
                }
                if parsed_target.pane.is_some() {
                    ctrl_target_pane = parsed_target.pane;
                    ctrl_pane_is_id = parsed_target.pane_is_id;
                }
            }
        } else {
            let target_scan_end = crate::cli::outer_target_scan_end(cmd_name, &cmd_args);
            let mut i = 0;
            while i < target_scan_end {
                if cmd_args[i] == "-t" {
                    if let Some(v) = cmd_args.get(i+1) {
                        // Issue #558: drop the '=' exact-match marker (see TARGET capture).
                        ctrl_raw_target = Some(crate::cli::strip_exact_match_prefix(v).to_string());
                        // Issue #692: a window command's bare `-t 0` is a window
                        // index in the current session (tmux cmd-find.c:443),
                        // not a session name.
                        let v = &crate::cli::coerce_bare_window_target(cmd_name, v);
                        let pt = parse_target(v);
                        if pt.window.is_some() { ctrl_target_win = pt.window; ctrl_target_win_is_id = pt.window_is_id; ctrl_target_win_name = None; }
                        else if pt.window_name.is_some() { ctrl_target_win_name = pt.window_name; ctrl_target_win = None; ctrl_target_win_is_id = false; }
                        if pt.pane.is_some() {
                            ctrl_target_pane = pt.pane;
                            ctrl_pane_is_id = pt.pane_is_id;
                        }
                    }
                    i += 2; continue;
                }
                i += 1;
            }
        }

        let filtered_args = if ctrl_set_option {
            cmd_args.clone()
        } else {
            without_outer_target(cmd_name, &cmd_args)
        };

        // Apply target focus
        // tmux parity (#592): select-pane -T/-P is title/style-only (see the
        // one-shot path below) — route it through the validated temp focus
        // instead of permanently moving the user's active window/pane.
        let ctrl_sp_attr_only = matches!(cmd_name, "select-pane" | "selectp")
            && cmd_args.windows(2).any(|w| w[0] == "-T" || w[0] == "-P")
            && !cmd_args.iter().any(|a| matches!(*a, "-U" | "-D" | "-L" | "-R" | "-l" | "-m" | "-M" | "-e" | "-d"));
        // select-pane -d/-e: validated target, never focused (see the
        // one-shot route's `sp_input_toggle`).
        let ctrl_sp_input_toggle = matches!(cmd_name, "select-pane" | "selectp")
            && select_pane_input_toggle(&cmd_args, ctrl_raw_target.as_deref()).is_some();
        let is_focus_cmd = matches!(cmd_name, "select-window" | "selectw" | "select-pane" | "selectp") && !ctrl_sp_attr_only && !ctrl_sp_input_toggle;
        // Same skip list as the one-shot path below. The two lists had
        // diverged (issue #545 sub-note): join-pane/move-pane/move-window/
        // swap-window/switch-client resolve their own targets (or name a
        // destination that need not exist yet), so temp-focusing here would
        // either undo the change (#483) or misdirect it (#442) for
        // control-mode clients exactly as it did for one-shot ones.
        // break-pane's -t is a DESTINATION window index that need not exist
        // yet (cmd-break-pane.c:43, CMD_FIND_WINDOW_INDEX), and its own parser
        // owns it. Temp-focusing it used to be the only reason `break-pane -t`
        // preserved the current window (#689).
        // link-window's -t is a DESTINATION window index that need not exist
        // yet either (cmd-move-window.c:83, the CMD_FIND_WINDOW_INDEX shared
        // with move-window), and unlink-window's -t names the window to
        // unlink; both now own their target, so the temp focus must not eat it
        // (issue #693 items 1 and 2).
        let skip_target_focus = matches!(cmd_name, "join-pane" | "joinp" | "move-pane" | "movep"
            | "new-window" | "neww"
            | "move-window" | "movew" | "swap-window" | "swapw"
            | "break-pane" | "breakp"
            | "link-window" | "linkw" | "unlink-window" | "unlinkw"
            | "switch-client" | "switchc" | "resize-window" | "resizew"
            | "kill-window" | "killw" | "detach-client" | "detach"
            // command-prompt's -t is a target CLIENT (cmd-command-prompt.c:84),
            // resolved by its own arm (#789 follow up).
            | "command-prompt"
            // So is set-buffer's (cmd-set-buffer.c:40).
            | "set-buffer" | "setb");
        // capture-pane -t %N resolves the pane id inside the capture itself;
        // swap-pane resolves its own target and swaps it with the *current*
        // active pane (temp-focusing the target would make active == target
        // and turn the swap into a no-op).
        let ctrl_capture_by_id = matches!(cmd_name, "capture-pane" | "capturep") && ctrl_pane_is_id && ctrl_target_pane.is_some();
        // select-pane -d/-e -t %N may name a floating pane, which ValidateTarget
        // cannot see; set_pane_input_off resolves the id itself.
        let ctrl_input_toggle_by_id = ctrl_sp_input_toggle && ctrl_pane_is_id && ctrl_target_pane.is_some();
        let skip_pane_focus = matches!(cmd_name, "display-message" | "display" | "swap-pane" | "swapp") || skip_target_focus || ctrl_capture_by_id || ctrl_input_toggle_by_id;
        // Issue #635: a dangling value-taking flag is reported through the
        // same %error path as an unresolvable -t, and skips dispatch (and the
        // temp focus below) entirely, so control clients see tmux's message
        // and nothing is mutated.
        // The validated target every request of this command carries.
        let mut ctrl_command_target: Option<crate::types::TempTarget> = None;
        // A CMD_FIND_CANFAIL command whose -t named nothing (see the one shot
        // route's `target_failed`).
        let mut ctrl_target_failed = false;
        // Issue #793: a `sess:...` target naming another session is
        // "can't find session" here, never this server's own panes.
        let ctrl_target_session = explicit_target_session(cmd_name, ctrl_raw_target.as_deref());
        let mut focus_err = crate::cli::validate_flag_arguments(cmd_name, &cmd_args)
            .err()
            .or_else(|| {
                ctrl_set_args.as_ref().and_then(|parsed_set| {
                    parsed_set.validate(ctrl_set_window_option).err()
                })
            })
            .or(ctrl_target_err);
        if focus_err.is_none() && is_focus_cmd {
            if let Some(ref s) = ctrl_target_session {
                let spec = crate::types::TempTarget { session: Some(s.clone()), ..Default::default() };
                if let Err(e) = validate_target(&tx_ctrl, spec) {
                    focus_err = Some(e);
                }
            }
            // select-pane -t %99 was a silent %end here (issue #793 follow
            // up). Existence only, so a floating pane still passes.
            let has_target = ctrl_target_win.is_some() || ctrl_target_win_name.is_some() || ctrl_target_pane.is_some();
            if focus_err.is_none() && has_target && matches!(cmd_name, "select-pane" | "selectp") {
                let spec = crate::types::TempTarget {
                    win: ctrl_target_win,
                    win_is_id: ctrl_target_win_is_id,
                    win_name: ctrl_target_win_name.clone(),
                    pane: ctrl_target_pane,
                    pane_is_id: ctrl_pane_is_id,
                    pane_check_only: true,
                    ..Default::default()
                };
                if let Err(e) = validate_target(&tx_ctrl, spec) {
                    focus_err = Some(e);
                }
            }
        }
        if focus_err.is_none() {
            if is_focus_cmd && matches!(cmd_name, "select-pane" | "selectp") {
                // Issue #691: one request for the whole target, so the command
                // fires its own hook (`after-select-pane`) once and never
                // `after-select-window`.
                for req in select_pane_requests(
                    &cmd_args,
                    ctrl_raw_target.as_deref(), ctrl_target_win, ctrl_target_win_is_id,
                    ctrl_target_win_name.as_deref(), ctrl_target_pane, ctrl_pane_is_id,
                ) {
                    let _ = tx_ctrl.send(req);
                }
            } else if is_focus_cmd {
                // #693 item 4: select-window resolves its whole `-t` through
                // the one shared resolver on this route too, so `-t +1`,
                // `-t !` and `-t {end}` mean the same thing from a control
                // client as they do from move-window.
                let (reqs, resp_r) = select_window_requests(
                    &filtered_args, ctrl_raw_target.as_deref(), ctrl_target_win,
                    ctrl_target_win_is_id, ctrl_target_win_name.as_deref());
                let decided = !reqs.is_empty();
                for req in reqs {
                    let _ = tx_ctrl.send(req);
                }
                if let Some(r) = resp_r {
                    if let Ok(Err(e)) = r.recv_timeout(Duration::from_secs(5)) {
                        focus_err = Some(e);
                    }
                }
                if !decided {
                    if let Some(wid) = ctrl_target_win {
                        if ctrl_target_win_is_id {
                            let _ = tx_ctrl.send(CtrlReq::FocusWindowById(wid));
                        } else {
                            let _ = tx_ctrl.send(CtrlReq::FocusWindow(wid));
                        }
                    } else if let Some(ref wname) = ctrl_target_win_name {
                        let _ = tx_ctrl.send(CtrlReq::FocusWindowByName(wname.clone()));
                    }
                }
                if let Some(pid) = ctrl_target_pane {
                    if ctrl_pane_is_id {
                        let _ = tx_ctrl.send(CtrlReq::FocusPane(pid));
                    } else {
                        let _ = tx_ctrl.send(CtrlReq::FocusPaneByIndex(pid));
                    }
                }
            } else {
                // Validated target (issue #545): on an unresolvable
                // window/pane target the command must not run, reply %error
                // instead of silently executing against the active window.
                // On success every request of the command carries the
                // resolved target (TargetedSender).
                let want_win = (ctrl_target_win.is_some()
                    || ctrl_target_win_name.is_some())
                    && !skip_target_focus;
                let want_pane = ctrl_target_pane.is_some() && !skip_pane_focus;
                let check_pane = ctrl_target_pane.is_some() && !want_pane
                    && pane_checked_not_focused(cmd_name, ctrl_pane_is_id);
                if want_win || want_pane || check_pane || ctrl_target_session.is_some() {
                    let spec = crate::types::TempTarget {
                        win: if want_win { ctrl_target_win } else { None },
                        win_is_id: ctrl_target_win_is_id,
                        win_name: if want_win { ctrl_target_win_name.clone() } else { None },
                        pane: if want_pane || check_pane { ctrl_target_pane } else { None },
                        pane_is_id: ctrl_pane_is_id,
                        session: ctrl_target_session.clone(),
                        pane_check_only: check_pane,
                    };
                    match validate_target(&tx_ctrl, spec) {
                        // A session-only check resolves to no window or pane:
                        // the command then acts on the real focus as before.
                        Ok(resolved) if resolved.win.is_some() || resolved.pane.is_some() => {
                            ctrl_command_target = Some(resolved)
                        }
                        Ok(_) => {}
                        // tmux CMD_FIND_CANFAIL: the command runs, with no target.
                        Err(_) if target_can_fail(cmd_name) => { ctrl_target_failed = true; }
                        Err(e) => focus_err = Some(e),
                    }
                }
            }
        }

        // A command that waits on a job (run-shell, and if-shell's shell
        // condition) opens its block BEFORE the job runs, as tmux does: the
        // guard line goes out when the item starts (cmd-queue.c:592,
        // cmdq_guard "begin") and `%end` when the job's callback continues the
        // item. Built after the job, a 7 s run-shell showed nothing at all
        // for 7 s and then `%begin` and `%end` together (issue #793 follow
        // up). Notifications may arrive between the two lines, as in tmux.
        // From here to the written reply no `continue` leaves the iteration.
        cmd_busy.store(true, std::sync::atomic::Ordering::Release);
        let early_begin = focus_err.is_none()
            && matches!(cmd_name, "run-shell" | "run" | "if-shell" | "if");
        if early_begin {
            let mut head = String::new();
            if control_echo {
                head.push_str(trimmed);
                head.push('\n');
            }
            head.push_str(&control::format_begin(ts, cmd_counter));
            head.push('\n');
            let mut ws = write_lock.lock().unwrap();
            let _ = ws.write_all(head.as_bytes());
            let _ = ws.flush();
        }

        // Dispatch command (use a oneshot for the response)
        let (resp_s, resp_r) = mpsc::channel::<String>();
        let response_result = if let Some(err) = focus_err {
            // Unresolvable -t target: skip dispatch entirely and surface
            // the diagnostic through the %error path (sentinel encoding,
            // same as dispatcher-signalled errors).
            Some(Ok(format!("\u{0001}ERR\u{0001}{}", err)))
        } else {
            let targeted_tx = TargetedSender::new(&tx_ctrl, ctrl_command_target.take());
            let dispatched = dispatch_control_command(
                cmd_name, &filtered_args, &targeted_tx, resp_s,
                ctrl_target_pane, ctrl_pane_is_id, ctrl_raw_target.as_deref(),
                ctrl_client_id, ctrl_target_failed,
            );
            // Collect the response BEFORE acquiring the write lock, so the
            // notification thread can still write while we wait.
            if dispatched {
                if early_begin {
                    // A job takes as long as it takes; tmux waits for it.
                    Some(resp_r.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected))
                } else {
                    Some(resp_r.recv_timeout(Duration::from_secs(5)))
                }
            } else {
                None
            }
        };

        // Acquire write lock for the ENTIRE %begin … %end sequence so
        // notifications from the notification thread never interleave
        // with command responses.  This matches real tmux's single-
        // threaded behaviour where command output and notifications are
        // serialized on one bufferevent.
        //
        // The whole block (echo, %begin, body, %end or %error) is built
        // first and sent with one write under the lock, so a reply leaves
        // in one send instead of one per line; the bytes are unchanged.
        let mut out = String::new();

        if !early_begin {
            // Echo the command if -C mode
            if control_echo {
                out.push_str(trimmed);
                out.push('\n');
            }

            // Send %begin
            out.push_str(&control::format_begin(ts, cmd_counter));
            out.push('\n');
        }

        let mut failed = false;
        match response_result {
            Some(Ok(response)) => {
                // Sentinel-encoded error: dispatcher signals %error
                // instead of %end by prefixing with \u{0001}ERR\u{0001}.
                let (is_error, body) = if let Some(stripped) = response.strip_prefix("\u{0001}ERR\u{0001}") {
                    (true, stripped)
                } else {
                    (false, response.as_str())
                };
                if !body.is_empty() {
                    out.push_str(body);
                    if !body.ends_with('\n') {
                        out.push('\n');
                    }
                }
                let footer = if is_error {
                    control::format_error(ts, cmd_counter)
                } else {
                    control::format_end(ts, cmd_counter)
                };
                out.push_str(&footer);
                out.push('\n');
                failed = is_error;
                // tmux tells the control client the session list changed once
                // the command that created a session is done (notify.c
                // session-created fires %sessions-changed after the %end).
                // The new session lives on its own server, so this server's
                // notification thread never sees it; the reply carries it.
                if !is_error && matches!(cmd_name, "new-session" | "new") {
                    out.push_str("%sessions-changed\n");
                }
            }
            Some(Err(_)) => {
                out.push_str("command timed out\n");
                out.push_str(&control::format_error(ts, cmd_counter));
                out.push('\n');
                failed = true;
            }
            None => {
                // Command dispatched without response channel (fire and forget)
                out.push_str(&control::format_end(ts, cmd_counter));
                out.push('\n');
            }
        }
        let mut ws = write_lock.lock().unwrap();
        let _ = ws.write_all(out.as_bytes());
        let _ = ws.flush();
        drop(ws);
        cmd_busy.store(false, std::sync::atomic::Ordering::Release);
        // A failed command drops the rest of its line: tmux removes the
        // remaining items of the command list (cmd-queue.c cmdq_next,
        // cmdq_remove_group after CMD_RETURN_ERROR), so `set-buffer ... ;
        // paste-buffer ...` never pastes when set-buffer fails (issue #813).
        // `pending` only ever holds the rest of the current line.
        if failed {
            pending.clear();
        }
    }

    // Deregister and clean up.
    // The CLIENT emits %exit + ST to stdout (matching real tmux's
    // client.c), so the server does not need to write ST here.
    let _ = tx.send(CtrlReq::ControlDeregister { client_id: ctrl_client_id });
    // The writer closes the socket once it has drained (see above); the
    // registry clone only has to go, or it leaks for the server's life.
    crate::types::deregister_persistent_stream(ctrl_client_id);
    drop(notif_thread);
    return;
}

// Check if this line is a TARGET specification
// Save raw target for relative pane specifiers like :.+ and :.-
let mut global_raw_target: Option<String> = None;
if line.trim().starts_with("TARGET ") {
    let target_spec = line.trim().strip_prefix("TARGET ").unwrap_or("");
    // Issue #558: keep the spec raw for relative pane forms, but drop the
    // tmux '=' exact-match marker so name compares in handlers (kill-session)
    // see the plain session name. parse_target strips it internally anyway.
    global_raw_target = Some(crate::cli::strip_exact_match_prefix(target_spec).to_string());
    let parsed = parse_target(target_spec);
    global_target_win = parsed.window;
    global_target_win_is_id = parsed.window_is_id;
    global_target_win_name = parsed.window_name;
    global_target_pane = parsed.pane;
    global_pane_is_id = parsed.pane_is_id;
    // Now read the actual command line
    line.clear();
    if r.read_line(&mut line).is_err() {
        return;
    }
}

// Set short read timeout for batched command processing
let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(10)));

// Process commands in a loop to handle batching
let mut attached_sent = false;
// `attach -r` (issue #724): set by the client's `client-flags read-only`.
let mut client_readonly = false;
// The client's environment from `client-environ` (#775 update-environment).
let mut client_environ: Option<Vec<(String, String)>> = None;
let mut pending_chain: Vec<String> = Vec::new();
// The rest of a command list that follows a foreground run-shell, held until
// that shell exits (PR #740). tmux runs `run-shell X \; cmd` in that order: the
// run-shell item waits in the client's queue (cmd-run-shell.c returns
// CMD_RETURN_WAIT, cmd-queue.c marks it CMDQ_WAITING) and `cmd` runs only once
// the job's callback continues the queue. The shell itself runs on its own
// thread so this reader keeps reading; the receiver disconnects when it ends.
// The 10 ms read timeout above brings the loop back here to release it.
let mut deferred_chains: Vec<(mpsc::Receiver<()>, Vec<String>)> = Vec::new();
loop {
    if pending_chain.is_empty() && line.trim().is_empty() && !deferred_chains.is_empty() {
        if let Some(i) = deferred_chains.iter().position(|(done, _)| {
            matches!(done.try_recv(), Err(mpsc::TryRecvError::Disconnected))
        }) {
            pending_chain = deferred_chains.remove(i).1;
        }
    }
    // A failed command ends the rest of its list (see ReplyStream::take_error).
    if write_stream.take_error() && !persistent {
        pending_chain.clear();
    }
    // Check pending chained commands before reading from socket
    let mut from_list = false;
    if !pending_chain.is_empty() {
        line = pending_chain.remove(0);
        from_list = true;
    } else if line.trim().is_empty() {
        // Try to read another command with timeout
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => {
                // EOF - client disconnected. A zero byte read is EOF here, but
                // it is also how a timed out socket read surfaces on some
                // Windows stacks, so log which one we think we saw: this line
                // is the only witness that separates a real disconnect from a
                // client that goes on receiving frames while losing its input.
                crate::debug_log::server_log(
                    "client-reader",
                    &format!("client {client_id}: batching read EOF (attached_sent={attached_sent}), closing the connection"),
                );
                if attached_sent {
                    let _ = tx.send(CtrlReq::ClientDetach(client_id));
                }
                crate::types::teardown_client_connection(client_id);
                break;
            }
            Err(e) => {
                // In persistent mode, timeouts are expected - keep waiting
                if persistent && is_read_retry(&e) {
                    line.clear(); // Clear any partial data from interrupted read
                    continue;
                }
                crate::debug_log::server_log(
                    "client-reader",
                    &format!("client {client_id}: batching read error {e:?} (attached_sent={attached_sent}), closing the connection"),
                );
                if attached_sent {
                    let _ = tx.send(CtrlReq::ClientDetach(client_id));
                }
                crate::types::teardown_client_connection(client_id);
                break; // Real error or non-persistent timeout
            }
            Ok(_) => {
                // Same activity ping as the batching read at the bottom of
                // this loop. An idle persistent client reaches its next
                // command HERE (the 10 ms batching read has already timed
                // out and cleared the line), so without this `window-size
                // latest` would only follow commands that arrive inside the
                // batching window.
                if !crate::client::is_bare_motion_cmd(&line)
                    && !crate::client::is_client_poll_cmd(&line)
                    && !crate::client::is_focus_loss_cmd(&line)
                {
                    let _ = tx.send(CtrlReq::ClientActivity(client_id));
                }
                continue; // Process the new line
            }
        }
    }
    
    // Use quote-aware parser to preserve arguments with spaces
    // Handle command chaining (\; or ;) by splitting into sub-commands
    let sub_cmds = crate::config::split_chained_commands_pub(line.trim());
    // This command is one of a list (`a ; b`), whose rest stops if it fails.
    let in_command_list = from_list || sub_cmds.len() > 1;
    let effective_line: String;
    if sub_cmds.len() > 1 {
        effective_line = sub_cmds[0].clone();
        // Ahead of whatever is already queued: a line that splits here while
        // a list is pending is an `if-shell` branch fed back below, and tmux
        // inserts a branch's commands right after the if-shell item
        // (cmd-if-shell.c cmdq_insert_after), before the rest of the list.
        // Appended, `if-shell C 'a ; b' ; c` ran a, c, b.
        pending_chain.splice(0..0, sub_cmds.into_iter().skip(1));
    } else {
        effective_line = line.trim().to_string();
    }
    let parsed = crate::cli::normalize_flag_equals(parse_command_line(&effective_line));
    let raw_cmd = parsed.get(0).map(|s| s.as_str()).unwrap_or("");
    // Check command aliases before normal dispatch
    let alias_expanded = if let Ok(map) = aliases.read() {
        map.get(raw_cmd).cloned()
    } else { None };
    let (cmd, args): (&str, Vec<&str>) = if let Some(ref expanded) = alias_expanded {
        // Alias expansion: replace command name, keep original args
        let expanded_parts: Vec<&str> = expanded.split_whitespace().collect();
        let mut all_args: Vec<&str> = expanded_parts[1..].to_vec();
        all_args.extend(parsed.iter().skip(1).map(|s| s.as_str()));
        (expanded_parts.first().copied().unwrap_or(raw_cmd), all_args)
    } else {
        (raw_cmd, parsed.iter().skip(1).map(|s| s.as_str()).collect())
    };
    // select-pane / last-pane read flags by exact token below (the -t scan,
    // the -l/-m/-T arms), so a clustered `-dt %2` lost its target and fenced
    // the ACTIVE pane.  Split clusters the way tmux's getopt reads them.
    let cluster_split: Vec<String>;
    let args: Vec<&str> = if matches!(cmd, "select-pane" | "selectp" | "last-pane" | "lastp") {
        cluster_split = crate::cli::expand_flag_clusters(cmd, &args);
        cluster_split.iter().map(|s| s.as_str()).collect()
    } else {
        args
    };

// Issue #635: a value-taking flag with nothing after it never runs. Checked
// here, before any target resolution or temp focus, so the failure path has
// no side effect at all (the window/session must still exist afterwards).
if let Err(flag_error) = crate::cli::validate_flag_arguments(cmd, &args) {
    let _ = writeln!(write_stream, "ERROR: {}", flag_error);
    let _ = write_stream.flush();
    // A failed command ends a one-shot connection with the rest of its chain
    // unrun, as tmux removes the rest of a command list after an error
    // (cmd-queue.c). Commands that succeed break only once the chain is
    // drained (`if !persistent && pending_chain.is_empty()` below).
    if !persistent { break; }
    line.clear();
    continue;
}

// A read only client looks but does not touch (issue #724). Dropped here,
// before any target is resolved or focused, so a refused command has no side
// effect at all. Nothing is written back: this is the attached client's frame
// stream, and tmux too drops a read only client's keys without a word.
if persistent && client_readonly && !readonly_client_may_run(cmd) {
    line.clear();
    continue;
}

// A bare `-t` name of a window or pane command (`kill-window -t keywin`,
// from a key binding or the command prompt) is a window of this session
// before it is a session name, as in tmux; parse_target alone filed it as a
// session and the command acted on the CURRENT window. Resolved on live
// state and rewritten to its id; naming nothing, the command does not run.
// The CLI sends a command's `-t` as the TARGET line; a bare one gets the
// same treatment, as the command's own `-t`.
let target_line_args: Vec<&str>;
let probe_args: &[&str] = match global_raw_target.as_deref() {
    Some(g) if crate::cli::bare_target_token(g)
        && crate::commands::in_server_target_kind(cmd).is_some()
        && !args[..crate::cli::outer_target_scan_end(cmd, &args).min(args.len())].contains(&"-t") =>
    {
        target_line_args = ["-t", g].into_iter().chain(args.iter().copied()).collect();
        &target_line_args
    }
    _ => &args,
};
let resolved_target_args: Vec<String>;
let args: Vec<&str> = match bare_target_rewrite(cmd, probe_args, &tx) {
    Ok(Some(new)) => {
        resolved_target_args = new;
        resolved_target_args.iter().map(String::as_str).collect()
    }
    Ok(None) => args,
    Err(e) => {
        // An attached client sees it in the status bar (set by the server).
        if !persistent {
            let _ = writeln!(write_stream, "ERROR: {}", e);
            let _ = write_stream.flush();
            break;
        }
        line.clear();
        continue;
    }
};

// Parse -t argument from command line (takes precedence over global TARGET)
let mut target_win: Option<usize> = global_target_win;
let mut target_win_is_id: bool = global_target_win_is_id;
let mut target_win_name: Option<String> = global_target_win_name.clone();
let mut target_pane: Option<usize> = global_target_pane;
let mut pane_is_id = global_pane_is_id;
// Save raw -t value for relative pane targets like :.+ or :.-
// Falls back to global_raw_target from TARGET protocol line
let mut raw_target: Option<String> = global_raw_target.clone();
let set_option_command = matches!(
    cmd,
    "set-option" | "set" | "set-window-option" | "setw"
);
if set_option_command {
    if let Some(value) = parse_set_option_args(&args).target {
        raw_target = Some(crate::cli::strip_exact_match_prefix(value).to_string());
        let parsed_target = parse_target(value);
        if parsed_target.window.is_some() {
            target_win = parsed_target.window;
            target_win_is_id = parsed_target.window_is_id;
            target_win_name = None;
        } else if parsed_target.window_name.is_some() {
            target_win_name = parsed_target.window_name;
            target_win = None;
            target_win_is_id = false;
        }
        if parsed_target.pane.is_some() {
            target_pane = parsed_target.pane;
            pane_is_id = parsed_target.pane_is_id;
        }
    }
} else {
    let target_scan_end = crate::cli::outer_target_scan_end(cmd, &args);
    let mut i = 0;
    while i < target_scan_end {
        if args[i] == "-t" {
            if let Some(v) = args.get(i+1) {
            // Issue #558: drop the '=' exact-match marker (see TARGET capture).
                raw_target = Some(crate::cli::strip_exact_match_prefix(v).to_string());
                // Issue #692: a window command's bare `-t 0` is a window index
                // in the current session (tmux cmd-find.c:443), not a session
                // name, so put the colon back before the generic parse.
                let v = &crate::cli::coerce_bare_window_target(cmd, v);
                // Parse the -t value using parse_target for consistent handling
                let pt = parse_target(v);
                if pt.window.is_some() { target_win = pt.window; target_win_is_id = pt.window_is_id; target_win_name = None; }
                else if pt.window_name.is_some() { target_win_name = pt.window_name; target_win = None; target_win_is_id = false; }
                if pt.pane.is_some() {
                    target_pane = pt.pane;
                    pane_is_id = pt.pane_is_id;
                }
            }
            i += 2; continue;
        }
        i += 1;
    }
}
let args = if set_option_command {
    args
} else {
    without_outer_target(cmd, &args)
};
if set_option_command {
    let parsed_set = parse_set_option_args(&args);
    let window_command = matches!(cmd, "set-window-option" | "setw");
    if let Err(error) = parsed_set.validate(window_command) {
        let _ = writeln!(write_stream, "ERROR: {}", error);
        let _ = write_stream.flush();
        if !persistent { break; }
        line.clear();
        continue;
    }
}
// tmux parity (#592): `select-pane -T`/`-P` is a title/style-only
// operation — tmux's cmd-select-pane.c sets the title and returns
// before any activation. Classify it as a NON-focus command so it takes
// the validated temp-focus path below: the attribute lands on the -t
// target and the user's active window/pane are restored afterwards.
// Movement flags (-U/-D/-L/-R/-l/-m/-M/-e/-d) keep the focus path.
let sp_attr_only = matches!(cmd, "select-pane" | "selectp")
    && args.windows(2).any(|w| w[0] == "-T" || w[0] == "-P")
    && !args.iter().any(|a| matches!(*a, "-U" | "-D" | "-L" | "-R" | "-l" | "-m" | "-M" | "-e" | "-d"));
// select-pane -d/-e (tmux PANE_INPUTOFF) selects nothing either: its -t is
// validated like any other command's and handed to the request, never focused.
let sp_input_toggle = matches!(cmd, "select-pane" | "selectp")
    && select_pane_input_toggle(&args, raw_target.as_deref()).is_some();
// Commands that should permanently change focus when used with -t
let is_focus_cmd = matches!(cmd, "select-window" | "selectw" | "select-pane" | "selectp") && !sp_attr_only && !sp_input_toggle;
// Commands that handle -t internally and should NOT get FocusWindowTemp.
// switch-client resolves the window/pane target itself (#483) and makes the
// change PERMANENT; letting the generic block issue a temporary focus here
// would restore the old focus after the batch and silently undo the switch.
// detach-client's -t is a CLIENT spec (numeric id, %ID, or tty name) that
// its handler resolves itself — it must never be validated as a pane/window
// target (a client id shaped like %N would be rejected whenever no pane %N
// exists, and a nonexistent client is that command's documented safe no-op).
// break-pane's -t is a DESTINATION window index that need not exist yet
// (cmd-break-pane.c:43, CMD_FIND_WINDOW_INDEX), and break-pane's own parser
// owns it. The temporary focus was the ONLY reason `break-pane -t` left the
// current window alone, which is why bare `break-pane -d` still switched (#689).
// link-window's -t is the same CMD_FIND_WINDOW_INDEX destination
// (cmd-move-window.c:83) and unlink-window's -t names the window to unlink
// (cmd-kill-window.c:75-83); both own their target now (#693 items 1 and 2).
// new-window's -t is the same kind of destination: `sess:N` naming no window
// is the index the new window takes (cmd-new-window.c, CMD_FIND_WINDOW_INDEX),
// so validating it as an existing window dropped every such command at rc 0.
let skip_target_focus = matches!(cmd, "join-pane" | "joinp" | "move-pane" | "movep"
    | "new-window" | "neww"
    | "move-window" | "movew" | "swap-window" | "swapw"
    | "break-pane" | "breakp"
    | "link-window" | "linkw" | "unlink-window" | "unlinkw"
    | "switch-client" | "switchc" | "resize-window" | "resizew"
    | "kill-window" | "killw" | "detach-client" | "detach"
            // command-prompt's -t is a target CLIENT (cmd-command-prompt.c:84),
            // resolved by its own arm (#789 follow up).
            | "command-prompt"
            // So is set-buffer's (cmd-set-buffer.c:40).
            | "set-buffer" | "setb");
let targeted_kill_pane_id = if matches!(cmd, "kill-pane" | "killp") && pane_is_id {
    target_pane
} else {
    None
};
// capture-pane resolves a -t %N target by pane id inside the capture
// itself (any window), so a temporary focus would only churn the active
// window for other clients. Non-id targets (-t 0.1) still use the
// temporary-focus path below.
let capture_pane_by_id = matches!(cmd, "capture-pane" | "capturep") && pane_is_id && target_pane.is_some();
// swap-pane swaps the target with the *current* active pane; focusing the
// target first would make active == target and turn the swap into a no-op.
// select-pane -d/-e -t %N may name a floating pane, which ValidateTarget
// cannot see; set_pane_input_off resolves the id itself.
let input_toggle_by_id = sp_input_toggle && pane_is_id && target_pane.is_some();
let skip_pane_focus = matches!(cmd, "display-message" | "display" | "swap-pane" | "swapp") || skip_target_focus || capture_pane_by_id || input_toggle_by_id;
// Issue #690: `select-window` decides its own window target, in
// `select_window_requests`, and this block does not touch it.  Both used
// to act on it, a permanent FocusWindow here and a SelectWindow from the
// command's arm below, and since every request carries its own hook slot
// in the server loop, one `select-window` ran `after-select-window`
// twice.  A pane part on a select-window target is still focused here.
let selectw_owns_window_target = matches!(cmd, "select-window" | "selectw");
// Issue #691: and `select-pane` owns its WHOLE target, window part
// included, for the same reason. Focusing the window part here fired
// `after-select-window` for a command tmux gives `after-select-pane`.
let selectp_owns_target = matches!(cmd, "select-pane" | "selectp");
// The validated target every request of this command carries (None: the
// command has no -t, or owns it, and acts on the real focus).
let mut command_target: Option<crate::types::TempTarget> = None;
// A CMD_FIND_CANFAIL command whose -t named nothing: tmux runs it with an
// empty target, so its formats expand with no session, window or pane.
let mut target_failed = false;
// Issue #793: a `sess:...` target naming another session is tmux's "can't
// find session", checked before the command touches anything.
let target_session = explicit_target_session(cmd, raw_target.as_deref());
if is_focus_cmd {
    if let Some(ref s) = target_session {
        let spec = crate::types::TempTarget { session: Some(s.clone()), ..Default::default() };
        if let Err(e) = validate_target(&tx, spec) {
            let _ = writeln!(write_stream, "ERROR: {}", e);
            let _ = write_stream.flush();
            if !persistent { break; }
            line.clear();
            continue;
        }
    }
    // A select-pane alone on the command line had its target checked by the
    // CLI (cli_validate_window_pane_target); one inside a list did not, and
    // its request has no reply. Check it here so a target that names nothing
    // is tmux's "can't find pane" and ends the list.
    // Alone on a raw TCP line it was not checked by anyone (issue #793 follow
    // up): `select-pane -t %99` answered nothing at all and did nothing. The
    // pane part is checked for existence only, so a floating pane still
    // passes and select_pane_requests keeps owning the selection.
    let has_target = target_win.is_some() || target_win_name.is_some() || target_pane.is_some();
    if selectp_owns_target && !persistent && has_target {
        let spec = crate::types::TempTarget {
            win: target_win,
            win_is_id: target_win_is_id,
            win_name: target_win_name.clone(),
            pane: target_pane,
            pane_is_id,
            pane_check_only: true,
            ..Default::default()
        };
        if let Err(e) = validate_target(&tx, spec) {
            let _ = writeln!(write_stream, "ERROR: {}", e);
            let _ = write_stream.flush();
            break;
        }
    }
    if selectp_owns_target {
        for req in select_pane_requests(
            &args, raw_target.as_deref(), target_win, target_win_is_id,
            target_win_name.as_deref(), target_pane, pane_is_id,
        ) {
            let _ = tx.send(req);
        }
    } else {
        if !selectw_owns_window_target {
            if let Some(wid) = target_win {
                if target_win_is_id {
                    let _ = tx.send(CtrlReq::FocusWindowById(wid));
                } else {
                    let _ = tx.send(CtrlReq::FocusWindow(wid));
                }
            } else if let Some(ref wname) = target_win_name {
                let _ = tx.send(CtrlReq::FocusWindowByName(wname.clone()));
            }
        }
        if let Some(pid) = target_pane {
            if pane_is_id {
                let _ = tx.send(CtrlReq::FocusPane(pid));
            } else {
                let _ = tx.send(CtrlReq::FocusPaneByIndex(pid));
            }
        }
    }
} else {
    // Validated target (issue #545): the server resolves the window/pane
    // target and replies Err on a miss, in which case the command must NOT
    // run (the old fire-and-forget temp focus silently no-opped on a bad
    // target and the untargeted command then executed against the ACTIVE
    // window at rc=0).  On success every request the command sends carries
    // the resolved target (TargetedSender), so it acts on that pane however
    // its requests interleave with anyone else's.
    let want_win = (target_win.is_some() || target_win_name.is_some()) && !skip_target_focus;
    let want_pane = target_pane.is_some() && !skip_pane_focus && targeted_kill_pane_id.is_none();
    let check_pane = target_pane.is_some() && !want_pane && pane_checked_not_focused(cmd, pane_is_id);
    if want_win || want_pane || check_pane || target_session.is_some() {
        let spec = crate::types::TempTarget {
            win: if want_win { target_win } else { None },
            win_is_id: target_win_is_id,
            win_name: if want_win { target_win_name.clone() } else { None },
            pane: if want_pane || check_pane { target_pane } else { None },
            pane_is_id,
            session: target_session.clone(),
            pane_check_only: check_pane,
        };
        match validate_target(&tx, spec) {
            // A session-only check resolves to no window or pane: the
            // command then acts on the real focus, as it always did.
            Ok(resolved) if resolved.win.is_none() && resolved.pane.is_none() => {}
            Ok(resolved) => command_target = Some(resolved),
            // tmux CMD_FIND_CANFAIL: the command runs, with no target. The
            // commands it queues (an if-shell branch, a sourced file) run
            // with no target either, so a TARGET line naming nothing must
            // not follow them down the list: the CLI sends `if-shell -t %99
            // ... 'display -p yes'` as TARGET %99 plus the command, and the
            // branch's display-message would otherwise be refused for %99.
            Err(_) if target_can_fail(cmd) => {
                target_failed = true;
                global_target_win = None;
                global_target_win_is_id = false;
                global_target_win_name = None;
                global_target_pane = None;
                global_pane_is_id = false;
                global_raw_target = None;
            }
            Err(e) => {
                // Unresolvable target: report (tmux: "can't find window: X",
                // exit 1, zero side effects) and skip the command entirely.
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
                if !persistent { break; }
                line.clear();
                continue;
            }
        }
    }
}
let targeted_tx = TargetedSender::new(&tx, command_target);
{
let tx = &targeted_tx;
match cmd {
    "new-window" | "neww" => {
        let name: Option<String> = args.windows(2).find(|w| w[0] == "-n").map(|w| w[1].trim_matches('"').to_string());
        let start_dir: Option<String> = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
        let detached = args.iter().any(|a| *a == "-d");
        let print_info = args.iter().any(|a| *a == "-P");
        let format_str: Option<String> = extract_flag_value(&args, "-F").map(|s| s.trim_matches('"').to_string());
        let title: Option<String> = extract_flag_value(&args, "-T").map(|s| s.trim_matches('"').to_string());
        let empty = args.iter().any(|a| *a == "-E");
        // -e KEY=VALUE (repeatable, tmux parity, #489): collect environment
        // for the new pane. The values must also be excluded from the
        // shell-command extraction below or they get spawned as the command.
        let env_sets: Vec<(String, String)> = args.windows(2)
            .filter(|w| w[0] == "-e")
            .filter_map(|w| w[1].trim_matches('"').split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        // tmux parity (#582): `-- prog args...` is an explicit argv. A
        // multi-token argv keeps the `--` marker plus token boundaries so
        // build_command execs it directly (tmux execvp); a single token
        // keeps string semantics. Without `--`, the historical single-token
        // extraction applies (the CLI sends the string form as one quoted
        // arg).
        let cmd_str: Option<String> = if let Some(pos) = args.iter().position(|a| *a == "--") {
            let tail = &args[pos + 1..];
            if tail.len() > 1 {
                Some(format!("-- {}", requote_command_tail(tail)))
            } else {
                tail.first().map(|s| s.trim_matches('"').to_string()).filter(|s| !s.is_empty())
            }
        } else {
            args.iter()
                .find(|a| !a.starts_with('-') && args.windows(2).all(|w| !(w[0] == "-n" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-c" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-F" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-T" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-e" && w[1] == **a)) && !args.iter().any(|f| f.starts_with("-F") && f.len() > 2 && &f[2..] == **a))
                .map(|s| s.trim_matches('"').to_string())
        };
        // -t (already peeled off into raw_target), -a, -b, -k and -S decide the
        // index; the server resolves them against its window list.
        let placement = crate::types::NewWindowPlacement::from_args(&args, raw_target.as_deref());
        if print_info {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::NewWindowPrint(cmd_str, name, detached, start_dir, format_str, rtx, title, empty, env_sets, placement));
            if let Ok(text) = rrx.recv_timeout(Duration::from_millis(2000)) {
                if !text.is_empty() {
                    let _ = write!(write_stream, "{}\n", text);
                    let _ = write_stream.flush();
                }
            }
            if !persistent && pending_chain.is_empty() { break; }
        } else if persistent {
            let _ = tx.send(CtrlReq::NewWindow(cmd_str, name, detached, start_dir, title, empty, env_sets, placement, None));
        } else {
            // A one-shot caller learns the outcome: tmux exits 1 on
            // "create window failed: index N in use" and "can't find window".
            let (otx, orx) = mpsc::channel::<Result<(), String>>();
            let _ = tx.send(CtrlReq::NewWindow(cmd_str, name, detached, start_dir, title, empty, env_sets, placement, Some(otx)));
            if let Ok(Err(e)) = orx.recv_timeout(Duration::from_secs(5)) {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "split-window" | "splitw" | "split-pane" | "splitp" => {
        let kind = if args.iter().any(|a| *a == "-h") { LayoutKind::Horizontal } else { LayoutKind::Vertical };
        let detached = args.iter().any(|a| *a == "-d");
        let zoom_after_split = args.iter().any(|a| *a == "-Z");
        let print_info = args.iter().any(|a| *a == "-P");
        let format_str: Option<String> = extract_flag_value(&args, "-F").map(|s| s.trim_matches('"').to_string());
        let title: Option<String> = extract_flag_value(&args, "-T").map(|s| s.trim_matches('"').to_string());
        let start_dir: Option<String> = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
        // -p N = percentage, -l N = cell count, -l N% = percentage (tmux semantics)
        let split_size: Option<(u16, bool)> = args.windows(2).find(|w| w[0] == "-p")
            .and_then(|w| w[1].trim_matches('%').parse::<u16>().ok())
            .map(|v| (v, true))
            .or_else(|| args.windows(2).find(|w| w[0] == "-l")
                .and_then(|w| {
                    let raw = &w[1];
                    let is_pct = raw.ends_with('%');
                    raw.trim_end_matches('%').parse::<u16>().ok().map(|v| (v, is_pct))
                }));
        // -e KEY=VALUE (repeatable, tmux parity, #489): environment for the
        // new pane. Excluded from shell-command extraction below — before
        // this fix the -e value itself was spawned as the pane command,
        // which flashed a red error and closed the pane instantly.
        let env_sets: Vec<(String, String)> = args.windows(2)
            .filter(|w| w[0] == "-e")
            .filter_map(|w| w[1].trim_matches('"').split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        // -b: new pane top/left (SPAWN_BEFORE); -f: span the whole window
        // (SPAWN_FULLSIZE). Both used to be parsed by nobody.
        let split_size = crate::pane::SplitPlacement {
            size: split_size,
            before: crate::cli::has_short_flag(&args, 'b'),
            full: crate::cli::has_short_flag(&args, 'f'),
        };
        // tmux parity (#582): same `--` argv handling as new-window above.
        let cmd_str: Option<String> = if let Some(pos) = args.iter().position(|a| *a == "--") {
            let tail = &args[pos + 1..];
            if tail.len() > 1 {
                Some(format!("-- {}", requote_command_tail(tail)))
            } else {
                tail.first().map(|s| s.trim_matches('"').to_string()).filter(|s| !s.is_empty())
            }
        } else {
            args.iter()
                .find(|a| !a.starts_with('-') && args.windows(2).all(|w| !(w[0] == "-c" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-p" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-l" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-T" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-F" && w[1] == **a)) && args.windows(2).all(|w| !(w[0] == "-e" && w[1] == **a)))
                .map(|s| s.trim_matches('"').to_string())
        };
        if print_info {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SplitWindowPrint(kind, cmd_str, detached, start_dir, split_size, format_str, rtx, title, env_sets, zoom_after_split));
            if let Ok(text) = rrx.recv_timeout(Duration::from_millis(2000)) {
                let _ = write!(write_stream, "{}\n", text);
                let _ = write_stream.flush();
            }
            if !persistent && pending_chain.is_empty() { break; }
        } else {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SplitWindow(kind, cmd_str, detached, start_dir, split_size, rtx, title, env_sets, zoom_after_split));
            if let Ok(err_msg) = rrx.recv_timeout(Duration::from_millis(2000)) {
                if !err_msg.is_empty() {
                    let _ = write!(write_stream, "{}\n", err_msg);
                    let _ = write_stream.flush();
                }
            }
        }
    }
    "kill-pane" | "killp" => {
        if let Some(pid) = targeted_kill_pane_id {
            let _ = tx.send(CtrlReq::KillPaneById(pid));
        } else {
            let _ = tx.send(CtrlReq::KillPane);
        }
    }
    "capture-pane" | "capturep" => {
        let print_stdout = crate::cli::has_short_flag(&args, 'p');
        let escape_seqs = crate::cli::has_short_flag(&args, 'e');
        // -N keeps trailing spaces, -J joins wrapped rows; both act on the
        // grid, where the wrap flag of each row is known.
        let opts = capture_opts(&args);
        // -t %N target: resolved server-side by pane id across all windows.
        let capture_pane_id = if pane_is_id { target_pane } else { None };
        // Parse -S start and -E end (negative = scrollback offset, - = entire scrollback)
        let s_arg = args.windows(2).find(|w| w[0] == "-S").map(|w| w[1]);
        let e_arg = args.windows(2).find(|w| w[0] == "-E").map(|w| w[1]);
        let start: Option<i32> = match s_arg {
            Some("-") => Some(i32::MIN), // entire scrollback start
            Some(v) => v.parse::<i32>().ok(),
            None => None,
        };
        let end: Option<i32> = match e_arg {
            Some("-") => None, // to end of visible
            Some(v) => v.parse::<i32>().ok(),
            None => None,
        };
        // -P and -a answer before the visible grid is read (#810).
        let captured = match capture_pane_special(&args, &tx, start, end, capture_pane_id) {
            Some(Ok(text)) => Some(if text.is_empty() && print_stdout { "\n".to_string() } else { text }),
            Some(Err(e)) => {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("capture-pane".to_string(), format!("ERROR: {}", e)));
                } else {
                    let _ = write!(write_stream, "ERROR: {}\n", e);
                    let _ = write_stream.flush();
                }
                None
            }
            None => {
                let (rtx, rrx) = mpsc::channel::<String>();
                if escape_seqs {
                    let _ = tx.send(CtrlReq::CapturePaneStyled(rtx, start, end, capture_pane_id, opts));
                } else if s_arg.is_some() || e_arg.is_some() {
                    let _ = tx.send(CtrlReq::CapturePaneRange(rtx, start, end, capture_pane_id, opts));
                } else {
                    let _ = tx.send(CtrlReq::CapturePane(rtx, capture_pane_id, opts));
                }
                rrx.recv().ok()
            }
        };
        if captured.is_none() && !persistent && pending_chain.is_empty() { break; }
        if let Some(text) = captured {
            if print_stdout {
                let text = capture_print_text(text);
                // Write text directly — it already ends with \n from capture
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("capture-pane".to_string(), text));
                } else {
                    let _ = write_stream.write_all(text.as_bytes());
                    let _ = write_stream.flush();
                }
                if !persistent && pending_chain.is_empty() { break; }
            } else {
                let _ = tx.send(CtrlReq::SetBuffer(text));
            }
        }
    }
    "dump-layout" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::DumpLayout(rtx));
        if let Ok(text) = rrx.recv() { 
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("dump-layout".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); 
                let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "dump-state" | "dump" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::DumpState(rtx, persistent, client_id));
        if let Some(ref rtx_bg) = resp_tx_opt {
            // Persistent mode: hand off to writer thread (non-blocking).
            // This lets the read loop keep processing keys immediately.
            let _ = rtx_bg.send(crate::types::WriterWake::Resp(rrx));
        } else {
            // One-shot mode: block and respond inline
            if let Ok(text) = rrx.recv() { 
                let _ = write!(write_stream, "{}\n", text); 
                let _ = write_stream.flush();
            }
            if !persistent && pending_chain.is_empty() { break; }
        }
    }
    "send-text" => {
        if let Some(payload) = args.get(0) { let _ = tx.send(CtrlReq::SendText(payload.to_string())); }
    }
    "send-paste" => {
        if let Some(encoded) = args.get(0) {
            if let Some(decoded) = base64_decode(encoded) {
                let _ = tx.send(CtrlReq::SendPaste(decoded));
            }
        }
    }
    "send-key" => {
        if let Some(payload) = args.get(0) {
            crate::pty_trace::mark("i", 0, payload.as_bytes());
            let _ = tx.send(CtrlReq::SendKey(payload.to_string()));
        }
    }
    "zoom-pane" | "resize-pane" | "resizep" if args.iter().any(|a| *a == "-Z") => { let _ = tx.send(CtrlReq::ZoomPane); }
    "zoom-pane" => { let _ = tx.send(CtrlReq::ZoomPane); }
    "prefix-begin" => { let _ = tx.send(CtrlReq::PrefixBegin); }
    "prefix-end" => { let _ = tx.send(CtrlReq::PrefixEnd); }
    "copy-enter" => { let _ = tx.send(CtrlReq::CopyEnter); }
    "copy-move" => {
        if args.len() >= 2 { if let (Ok(dx), Ok(dy)) = (args[0].parse::<i16>(), args[1].parse::<i16>()) { let _ = tx.send(CtrlReq::CopyMove(dx, dy)); } }
    }
    "copy-anchor" => { let _ = tx.send(CtrlReq::CopyAnchor); }
    "rectangle-toggle" => { let _ = tx.send(CtrlReq::CopyRectToggle); }
    "copy-yank" => { let _ = tx.send(CtrlReq::CopyYank); }
    "client-size" => {
        if args.len() >= 2 { if let (Ok(w), Ok(h)) = (args[0].parse::<u16>(), args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::ClientSize(client_id, w, h)); } }
    }
    "host-colors" => {
        // Issue #473: client reports its host terminal's colors (queried at
        // attach time) so the server can answer pane color queries.
        if let Some(spec) = args.get(0) { let _ = tx.send(CtrlReq::HostColors(spec.to_string())); }
    }
    "focus-pane" => {
        if let Some(pid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusPaneCmd(pid)); }
    }
    "focus-window" => {
        if let Some(wid) = args.get(0).and_then(|s| s.parse::<usize>().ok()) { let _ = tx.send(CtrlReq::FocusWindowCmd(wid)); }
    }
    "mouse-down" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDown(client_id,x,y)); } }
    }
    "mouse-down-right" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDownRight(client_id,x,y)); } }
    }
    "mouse-down-middle" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDownMiddle(client_id,x,y)); } }
    }
    "mouse-drag" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseDrag(client_id,x,y)); } }
    }
    "mouse-up" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUp(client_id,x,y)); } }
    }
    "mouse-up-right" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUpRight(client_id,x,y)); } }
    }
    "mouse-up-middle" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseUpMiddle(client_id,x,y)); } }
    }
    "mouse-move" => {
        if args.len()>=2 { if let (Ok(x),Ok(y))=(args[0].parse::<u16>(),args[1].parse::<u16>()) { let _ = tx.send(CtrlReq::MouseMove(client_id,x,y)); } }
    }
    "scroll-up" => {
        let x = args.get(0).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        let y = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        if args.get(3).map_or(false, |a| *a == "S") {
            let mods = args.get(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let _ = tx.send(CtrlReq::StatusWheel(client_id, true, mods));
        } else {
            let _ = tx.send(CtrlReq::ScrollUp(client_id, x, y));
        }
    }
    "scroll-down" => {
        let x = args.get(0).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        let y = args.get(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
        if args.get(3).map_or(false, |a| *a == "S") {
            let mods = args.get(2).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let _ = tx.send(CtrlReq::StatusWheel(client_id, false, mods));
        } else {
            let _ = tx.send(CtrlReq::ScrollDown(client_id, x, y));
        }
    }
    "pane-mouse" => {
        // pane-mouse PANE_ID BUTTON COL ROW M|m
        if args.len() >= 5 {
            if let (Ok(pane_id), Ok(button), Ok(col), Ok(row)) = (
                args[0].parse::<usize>(), args[1].parse::<u8>(),
                args[2].parse::<i16>(), args[3].parse::<i16>()
            ) {
                let press = args[4] != "m";
                let _ = tx.send(CtrlReq::PaneMouse(client_id, pane_id, button, col, row, press));
            }
        }
    }
    "copy-drag-begin" => {
        // copy-drag-begin PANE_ID ANCHOR_COL ANCHOR_ROW CUR_COL CUR_ROW [b]
        // Sent when a normal-mode drag selection crosses the pane's top edge
        // — or its bottom edge over a direct-scrolled view (#193); the
        // trailing "b" marks a rectangular (block) selection.
        if args.len() >= 5 {
            if let (Ok(pane_id), Ok(a_col), Ok(a_row), Ok(c_col), Ok(c_row)) = (
                args[0].parse::<usize>(), args[1].parse::<i16>(), args[2].parse::<i16>(),
                args[3].parse::<i16>(), args[4].parse::<i16>()
            ) {
                let rect_sel = args.get(5).map_or(false, |a| *a == "b");
                let _ = tx.send(CtrlReq::CopyDragBegin(client_id, pane_id, a_col, a_row, c_col, c_row, rect_sel));
            }
        }
    }
    "mouse-key" => {
        // mouse-key NAME TARGET COL ROW BTN: a client routed mouse key that
        // has a user binding (mouse_keys::client_overrides). TARGET is the
        // mouse target as `%pane` or `:window`.
        if args.len() >= 5 {
            let col = args[2].parse::<i16>().unwrap_or(0);
            let row = args[3].parse::<i16>().unwrap_or(0);
            let btn = args.get(4).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
            let _ = tx.send(CtrlReq::MouseKey(client_id, args[0].to_string(), args[1].to_string(), col, row, btn));
        }
    }
    "mouse-forward" => {
        // mouse-forward PANE BTN COL ROW M|m: an unbound root mouse key, for
        // the program in the pane only if it asked for the mouse (tmux
        // forward_key). No focus change.
        if args.len() >= 5 {
            if let (Ok(pane_id), Ok(btn), Ok(col), Ok(row)) = (
                args[0].parse::<usize>(), args[1].parse::<u8>(),
                args[2].parse::<i16>(), args[3].parse::<i16>()
            ) {
                let _ = tx.send(CtrlReq::MouseForward(client_id, pane_id, btn, col, row, args[4] != "m"));
            }
        }
    }
    "pane-scroll" => {
        // pane-scroll PANE_ID up|down [COL ROW]
        // COL/ROW are the pointer's pane-relative 0-based position.  They are
        // optional so that a client from an older build still scrolls (#570).
        if args.len() >= 2 {
            if let Ok(pane_id) = args[0].parse::<usize>() {
                let up = args[1] == "up";
                let at = match (args.get(2).and_then(|s| s.parse::<i16>().ok()),
                                args.get(3).and_then(|s| s.parse::<i16>().ok())) {
                    (Some(col), Some(row)) => Some((col, row)),
                    _ => None,
                };
                // The held modifiers as tmux's mouse mask, absent from an
                // older client (no modifier).
                let mods = args.get(4).and_then(|s| s.parse::<u8>().ok()).unwrap_or(0);
                let _ = tx.send(CtrlReq::PaneScroll(client_id, pane_id, up, at, mods));
            }
        }
    }
    "split-sizes" => {
        // split-sizes PATH SIZE1,SIZE2,...  (PATH is "_" for root, or dot-separated indices)
        if args.len() >= 2 {
            let path: Vec<usize> = if args[0] == "_" {
                Vec::new()
            } else {
                args[0].split('.').filter_map(|s| s.parse().ok()).collect()
            };
            let sizes: Vec<u16> = args[1].split(',').filter_map(|s| s.parse().ok()).collect();
            if sizes.len() >= 2 {
                let _ = tx.send(CtrlReq::SplitSetSizes(client_id, path, sizes));
            }
        }
    }
    "split-resize-done" => {
        let _ = tx.send(CtrlReq::SplitResizeDone(client_id));
    }
    "next-window" | "next" => { let _ = tx.send(CtrlReq::NextWindow); }
    "previous-window" | "prev" => { let _ = tx.send(CtrlReq::PrevWindow); }
    "rename-window" | "renamew" => { if let Some(name) = crate::cli::first_operand("rename-window", &args) { let _ = tx.send(CtrlReq::RenameWindow(name.to_string())); } }
    "list-windows" | "lsw" => {
        // Extract -F format if provided (supports -F val and -Fval)
        let fmt = extract_flag_value(&args, "-F");
        if let Some(fmt_str) = fmt {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListWindowsFormat(rtx, fmt_str));
            if let Ok(text) = rrx.recv() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("list-windows".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                }
            }
        } else if args.iter().any(|a| *a == "-J") {
            // JSON output for programmatic use
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListWindows(rtx));
            if let Ok(text) = rrx.recv() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("list-windows".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                }
            }
        } else {
            // tmux-compatible text output (default)
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListWindowsTmux(rtx));
            if let Ok(text) = rrx.recv() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("list-windows".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                }
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "list-tree" => { let (rtx, rrx) = mpsc::channel::<String>(); let _ = tx.send(CtrlReq::ListTree(rtx)); if let Ok(text) = rrx.recv() { if persistent { let _ = tx.send(CtrlReq::ShowTextPopup("list-tree".to_string(), text)); } else { let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush(); } } if !persistent && pending_chain.is_empty() { break; } }
    "window-layout" => {
        // Issue #257: return simplified layout JSON for a given window id.
        // Usage: window-layout <window_id>
        let wid: Option<usize> = args.get(0).and_then(|a| a.trim_start_matches('@').parse::<usize>().ok());
        if let Some(wid) = wid {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::WindowLayout(wid, rtx));
            if let Ok(text) = rrx.recv() {
                let _ = write!(write_stream, "{}\n", text);
                let _ = write_stream.flush();
            }
        } else {
            let _ = write!(write_stream, "{{}}\n");
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "window-dump" => {
        // Return a fully styled LayoutJson, or the layout plus preview styles
        // and focus metadata when `state` is requested.
        // Usage: window-dump <window_id> [state]
        let wid: Option<usize> = args.get(0).and_then(|a| a.trim_start_matches('@').parse::<usize>().ok());
        if let Some(wid) = wid {
            let (rtx, rrx) = mpsc::channel::<String>();
            let format = if args.get(1) == Some(&"state") {
                WindowDumpFormat::PreviewState
            } else {
                WindowDumpFormat::Layout
            };
            let _ = tx.send(CtrlReq::WindowDump(wid, format, rtx));
            if let Ok(text) = rrx.recv() {
                let _ = write!(write_stream, "{}\n", text);
                let _ = write_stream.flush();
            }
        } else {
            let _ = write!(write_stream, "{{}}\n");
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "toggle-sync" => { let _ = tx.send(CtrlReq::ToggleSync); }
    "set-pane-title" => { let title = args.join(" "); let _ = tx.send(CtrlReq::SetPaneTitle(title)); }
    "send-keys" | "send" => {
        if let Err(e) = send_keys_requests(&args, tx, client_id) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "select-pane" | "selectp" if sp_input_toggle => {
        // select-pane -d / -e: tmux's PANE_INPUTOFF, on the -t pane (or with
        // -l the last pane), selecting nothing (cmd-select-pane.c).
        let (off, last) = select_pane_input_toggle(&args, raw_target.as_deref()).unwrap_or((true, false));
        let (resp_s, resp_r) = mpsc::channel();
        tx.send_unwrapped(CtrlReq::SetPaneInputOff {
            target: input_toggle_target(tx.target(), target_pane, pane_is_id),
            off,
            last,
            resp: Some(resp_s),
        });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "select-pane" | "selectp" => {
        // Detect relative pane targets: -t :.+  or  -t :.-
        let is_next_pane = raw_target.as_deref().map_or(false, |t| t.contains(".+") || t == "+" || t == ":.+");
        let is_prev_pane = raw_target.as_deref().map_or(false, |t| t.contains(".-") || t == "-" || t == ":.-");
        let dir = if is_next_pane { "next" }
            else if is_prev_pane { "prev" }
            else if args.iter().any(|a| *a == "-U") { "U" }
            else if args.iter().any(|a| *a == "-D") { "D" }
            else if args.iter().any(|a| *a == "-L") { "L" }
            else if args.iter().any(|a| *a == "-R") { "R" }
            else if args.iter().any(|a| *a == "-l") { "last" }
            else if args.iter().any(|a| *a == "-m") { "mark" }
            else if args.iter().any(|a| *a == "-M") { "unmark" }
            // -e / -d without any of the above are the input toggle arm.
            else { "" };
        // Check for -T title and -P style (per-pane style, e.g.
        // "bg=default,fg=blue" — Claude Code uses -P for agent pane
        // coloring; stored even if rendering doesn't support it yet).
        // Sent as ONE SetPaneAttrs request (#592): under the temporary
        // -t focus the restore fires after the first non-temp request,
        // so two separate sends would mis-target the second attribute.
        let title = args.windows(2).find(|w| w[0] == "-T").map(|w| w[1].to_string());
        let pane_style = args.windows(2).find(|w| w[0] == "-P").map(|w| w[1].to_string());
        if title.is_some() || pane_style.is_some() {
            let _ = tx.send(CtrlReq::SetPaneAttrs { title, style: pane_style });
        }
        if dir == "last" {
            // #693 item 5: `select-pane -l` with no last pane is tmux's
            // "no last pane" at exit 1 (cmd-select-pane.c:176), not a silent
            // success. It is the same operation as `last-pane`, so it takes
            // the same request.
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::LastPane { resp: resp_s });
            if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
            }
        } else if !dir.is_empty() {
            let keep_zoom = args.iter().any(|a| *a == "-Z");
            let _ = tx.send(CtrlReq::SelectPane(dir.to_string(), keep_zoom));
        }
    }
    "select-window" | "selectw" => {
        // Exactly one window request per command, chosen in one place (#690).
        let (reqs, resp_r) = select_window_requests(
            &args, raw_target.as_deref(), target_win, target_win_is_id,
            target_win_name.as_deref());
        for req in reqs {
            let _ = tx.send(req);
        }
        // #693 item 4: a spec that resolves to no window is tmux's
        // "can't find window: N" at exit 1, not a silent no-op.
        if let Some(r) = resp_r {
            if let Ok(Err(e)) = r.recv_timeout(Duration::from_secs(5)) {
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
            }
        }
    }
    "list-panes" | "lsp" => {
        let fmt = extract_flag_value(&args, "-F");
        // tmux: -a = all panes across all sessions, -s = all panes in target session
        // psmux uses per-session servers, so -s is equivalent to listing the current
        // session's panes (same as no flag). -a lists all panes in this server too
        // since there's only one session per server.
        let all = args.iter().any(|a| *a == "-a");
        let session_scope = args.iter().any(|a| *a == "-s");
        let (rtx, rrx) = mpsc::channel::<String>();
        if let Some(fmt_str) = fmt {
            if all || session_scope {
                let _ = tx.send(CtrlReq::ListAllPanesFormat(rtx, fmt_str));
            } else {
                let _ = tx.send(CtrlReq::ListPanesFormat(rtx, fmt_str));
            }
        } else {
            if all {
                let _ = tx.send(CtrlReq::ListAllPanes(rtx));
            } else if session_scope {
                // -s: list all panes in the targeted session (all windows)
                let _ = tx.send(CtrlReq::ListAllPanes(rtx));
            } else {
                let _ = tx.send(CtrlReq::ListPanes(rtx));
            }
        }
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("list-panes".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "kill-window" | "killw" => {
        // Resolve the -t target server-side. The generic temp-focus block is
        // skipped for kill-window (see skip_target_focus): focusing a target
        // that fails to resolve silently left the PREVIOUS window focused and
        // the bare KillWindow then killed it — `kill-window -t sess:typo`
        // destroyed whatever was active (tmux: "can't find window", no kill).
        if target_win.is_some() || target_win_name.is_some() {
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::KillWindowTarget {
                win: target_win,
                win_is_id: target_win_is_id,
                name: target_win_name.clone(),
                resp: resp_s,
            });
            if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
                // persistent clients see it in the status bar (set server-side)
            }
        } else {
            let _ = tx.send(CtrlReq::KillWindow);
        }
    }
    "kill-session" | "kill-ses" => {
        // If -t <target> is given, kill that session instead of self.
        // The target may be specified without the -L socket-name namespace
        // prefix (e.g. "worker1" instead of "ns1__worker1"), so if the raw
        // path is missing we ask our own server for its session name and
        // fall through to KillSession when raw_target matches us.
        if let Some(ref tgt) = raw_target {
            let port_path = crate::paths::port_file(tgt);
            let mut handled = false;
            // A differently spelled registry entry is another session: never
            // kill `repro-case` for `-t REPRO-CASE` (issue #774).
            let case_variant = crate::session::registry_case_variant(tgt).is_some();
            if case_variant {
                // fall through to the exact self-name comparison below
            } else if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                if let Ok(port) = port_str.trim().parse::<u16>() {
                    let key = crate::session::read_session_key(tgt).unwrap_or_default();
                    let _ = crate::session::send_control_to_port(port, "kill-session\n", &key);
                    handled = true;
                }
            }
            if !handled {
                // Query our own session name. If it matches the target
                // (in-namespace name), kill self. Otherwise the target
                // simply does not exist on this server.
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::SessionInfo(rtx));
                if let Ok(line) = rrx.recv() {
                    let self_name = line.split(':').next().unwrap_or("").trim();
                    if !self_name.is_empty() && self_name == tgt {
                        let _ = tx.send(CtrlReq::KillSession);
                    }
                }
            }
        } else {
            let _ = tx.send(CtrlReq::KillSession);
        }
    }
    "has-session" => {
        let (rtx, rrx) = mpsc::channel::<bool>();
        let _ = tx.send(CtrlReq::HasSession(rtx));
        if let Ok(exists) = rrx.recv() {
            if !exists { std::process::exit(1); }
        }
    }
    "rename-session" | "rename" => {
        if let Some(name) = crate::cli::first_operand("rename-session", &args) {
            let _ = tx.send(CtrlReq::RenameSession(name.to_string()));
        }
    }
    "claim-session" => {
        // Warm-server claim: rename + synchronous response so CLI knows it's done.
        // Usage: claim-session <name> [<client-cwd>] [-p <priority>] [-e <env-file>] [-n <window-name>]
        //
        // Positionals and flags are split by crate::util::parse_claim_args_full
        // so the wire contract has one implementation and one set of tests.
        // `-e` names a file holding the claiming client's environment block,
        // which the standby adopts so it stops carrying the environment of
        // whatever spawned it (#659). `-n` is the `new-session -n NAME` window
        // name, applied inside the claim so it is already in place when this
        // answers OK (#674).
        let parsed = crate::util::parse_claim_args_full(&args);
        let (non_flag, client_priority) = (parsed.positionals, parsed.priority);
        if let Some(name) = non_flag.first().cloned() {
            let client_cwd = non_flag.get(1).cloned();
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ClaimSession(name, client_cwd, client_priority, parsed.env_file, parsed.window_name, rtx));
            if let Ok(resp) = rrx.recv_timeout(std::time::Duration::from_secs(5)) {
                let _ = write!(write_stream, "{}", resp);
                let _ = write_stream.flush();
            }
        }
    }
    "swap-pane" | "swapp" => {
        let detach = args.iter().any(|a| *a == "-d");
        let raw_s = args.iter().position(|a| *a == "-s")
            .and_then(|i| args.get(i + 1).copied());
        let raw_t = args.iter().position(|a| *a == "-t")
            .and_then(|i| args.get(i + 1).copied())
            .or_else(|| raw_target.as_deref());
        // `-t <pane>` (with or without `-s`) is resolved session wide, so
        // either half may name a pane in another window (#689). A `{position}`
        // token stays on the layout-token path; anything that names no pane at
        // all falls through to the directional -U/-D form.
        let names_pane = |s: &str| {
            let pt = parse_target(s);
            pt.pane.is_some() || pt.window.is_some() || pt.window_name.is_some()
        };
        if let Some(t) = raw_t.filter(|t| !t.starts_with('{') && (names_pane(t) || raw_s.is_some())) {
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::SwapPaneSrcDst {
                src: raw_s.map(|s| s.to_string()),
                dst: t.to_string(),
                detach,
                resp: resp_s,
            });
            if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
            }
        } else if let Some(tok) = raw_t.filter(|t| t.starts_with('{')) {
            // Layout position token like {top-right} — layout-independent.
            let _ = tx.send(CtrlReq::SwapPanePosition(tok.to_string()));
        } else {
            let dir = if args.iter().any(|a| *a == "-U") { "U" }
                else if args.iter().any(|a| *a == "-L") { "L" }
                else if args.iter().any(|a| *a == "-R") { "R" }
                else { "D" };
            let _ = tx.send(CtrlReq::SwapPane(dir.to_string()));
        }
    }
    "resize-pane" | "resizep" => {
        // Check for zoom toggle first (issue #35)
        if args.iter().any(|a| *a == "-Z") {
            let _ = tx.send(CtrlReq::ZoomPane);
        } else
        // Check for absolute resize (-x N or -y N), supporting both
        // absolute values (e.g. "60") and percentage strings (e.g. "30%").
        if let Some(xval) = args.windows(2).find(|w| w[0] == "-x").map(|w| w[1]) {
            if let Some(pct) = xval.strip_suffix('%').and_then(|n| n.parse::<u8>().ok()) {
                let _ = tx.send(CtrlReq::ResizePanePercent("x".to_string(), pct));
            } else if let Ok(abs) = xval.parse::<u16>() {
                let _ = tx.send(CtrlReq::ResizePaneAbsolute("x".to_string(), abs));
            }
        } else if let Some(yval) = args.windows(2).find(|w| w[0] == "-y").map(|w| w[1]) {
            if let Some(pct) = yval.strip_suffix('%').and_then(|n| n.parse::<u8>().ok()) {
                let _ = tx.send(CtrlReq::ResizePanePercent("y".to_string(), pct));
            } else if let Ok(abs) = yval.parse::<u16>() {
                let _ = tx.send(CtrlReq::ResizePaneAbsolute("y".to_string(), abs));
            }
        } else {
            let amount = args.iter().find(|a| a.parse::<u16>().is_ok()).and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
            let dir = if args.iter().any(|a| *a == "-U") { "U" }
                else if args.iter().any(|a| *a == "-D") { "D" }
                else if args.iter().any(|a| *a == "-L") { "L" }
                else if args.iter().any(|a| *a == "-R") { "R" }
                else { "D" };
            let _ = tx.send(CtrlReq::ResizePane(dir.to_string(), amount));
        }
    }
    "set-buffer" | "setb" => {
        // The parser and executor control mode uses (#813), so `-n` renames
        // here too instead of storing the new name as data, a bundled `-ab
        // NAME` keeps its `-a`, and an empty data argument changes nothing
        // while no data at all is tmux's "no data specified".
        let result = crate::commands::parse_set_buffer_args(&args).and_then(|set| match set {
            Some(set) => run_set_buffer(&tx, set, raw_target.as_deref()),
            None => Ok(()),
        });
        if let Err(err) = result {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("set-buffer".to_string(), format!("ERROR: {}", err)));
            } else {
                let _ = write!(write_stream, "ERROR: {}\n", err);
                let _ = write_stream.flush();
            }
        }
    }
    "paste-buffer" | "pasteb" => {
        // Issue #684: one parser AND one executor for both dispatches, so -d,
        // -s and -t reach the CLI route as well and the in server route
        // (commands.rs) cannot drift from it again.
        //
        // This used to be a buffer lookup (ShowNamedBuffer, a round trip) and
        // then a separate SendPaste.  The lookup is a non-focus request, so it
        // spent the validated `-t` focus the dispatcher had just applied and
        // the paste that followed landed in whatever pane the user was looking
        // at (gabri-ns on #684).  One request now carries the whole command and
        // the target with it, which is also what tmux does: cmd-paste-buffer.c
        // resolves the pane with cmd_find_pane and writes to it.
        //
        // `-t` here keeps its `without_outer_target` stripping, so re-read it
        // from the raw target the dispatcher parsed.
        let mut pb_args = crate::commands::parse_paste_buffer_args(&args);
        if pb_args.target.is_none() {
            pb_args.target = raw_target.clone();
        }
        let (rtx, rrx) = mpsc::channel::<Option<String>>();
        let _ = tx.send(CtrlReq::PasteBuffer(pb_args, rtx));
        if let Ok(Some(msg)) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("paste-buffer".to_string(), format!("ERROR: {}", msg)));
            } else {
                let _ = write!(write_stream, "ERROR: {}\n", msg);
                let _ = write_stream.flush();
            }
        }
    }
    "list-buffers" | "lsb" => {
        let fmt = extract_flag_value(&args, "-F");
        let (rtx, rrx) = mpsc::channel::<String>();
        if let Some(fmt_str) = fmt {
            let _ = tx.send(CtrlReq::ListBuffersFormat(rtx, fmt_str));
        } else {
            let _ = tx.send(CtrlReq::ListBuffers(rtx));
        }
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("list-buffers".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "show-buffer" | "showb" => {
        let buf_name: Option<String> = args.windows(2).find(|w| w[0] == "-b").map(|w| w[1].to_string());
        // By name only (paste_get_name): `-b 1` is the buffer named 1, and a
        // name no buffer has is an error (cmd-save-buffer.c:92).
        let text: String = if let Some(name) = buf_name {
            let (rtx, rrx) = mpsc::channel::<Option<String>>();
            let _ = tx.send(CtrlReq::ShowNamedBuffer(rtx, name.clone()));
            rrx.recv().ok().flatten().unwrap_or_else(|| format!("ERROR: no buffer {}", name))
        } else {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ShowBuffer(rtx));
            rrx.recv().unwrap_or_default()
        };
        if persistent {
            let _ = tx.send(CtrlReq::ShowTextPopup("show-buffer".to_string(), text));
        } else {
            let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "delete-buffer" => {
        let buf_name: Option<String> = args.windows(2).find(|w| w[0] == "-b").map(|w| w[1].to_string());
        if let Some(name) = buf_name {
            // By name only; a name no buffer has is an error
            // (cmd-set-buffer.c:73).
            let (rtx, rrx) = mpsc::channel::<bool>();
            let _ = tx.send(CtrlReq::DeleteNamedBuffer(name.clone(), Some(rtx)));
            if rrx.recv() == Ok(false) {
                let err = format!("unknown buffer: {}", name);
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("delete-buffer".to_string(), format!("ERROR: {}", err)));
                } else {
                    let _ = write!(write_stream, "ERROR: {}\n", err);
                    let _ = write_stream.flush();
                }
            }
        } else {
            let _ = tx.send(CtrlReq::DeleteBuffer);
        }
    }
    "delete-buffer-at" => {
        if let Some(idx_str) = args.get(0) {
            if let Ok(idx) = idx_str.parse::<usize>() {
                let _ = tx.send(CtrlReq::DeleteBufferAt(idx));
            }
        }
    }
    "paste-buffer-at" => {
        if let Some(idx_str) = args.get(0) {
            if let Ok(idx) = idx_str.parse::<usize>() {
                let _ = tx.send(CtrlReq::PasteBufferAt(None, idx));
            }
        }
    }
    // The chooser's paste, by the buffer's name (#818).
    "paste-buffer-named" => {
        if let Some(name) = args.get(0) {
            let _ = tx.send(CtrlReq::PasteBufferAt(Some(name.to_string()), 0));
        }
    }
    "choose-buffer" | "chooseb" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ChooseBuffer(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("choose-buffer".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "display-message" | "display" => {
        // A flag display-message does not have is refused like tmux
        // ("unknown flag -x"), not printed as message text (#795 follow up).
        // Every tmux flag is parsed as a flag (`aCc:d:lINpt:F:v`), never
        // printed as message text (#795 follow up).
        let parsed = match crate::cli::parse_display_message_args(&args) {
            Ok(parsed) => parsed,
            Err(error) => {
                let _ = write!(write_stream, "ERROR: {}\n", error);
                let _ = write_stream.flush();
                if !persistent && pending_chain.is_empty() { break; }
                continue;
            }
        };
        // `-a` prints the variable list to the caller like `-p` does
        // (cmd-display-message.c format_each -> cmdq_print).
        let print_stdout = parsed.print || parsed.list_all;
        let duration_ms: Option<u64> = parsed.delay.as_deref().and_then(|d| d.parse::<u64>().ok());
        // -c target-client (issue #724): the client the client_* variables
        // describe.
        let client_sel = parsed.client.clone().map(crate::types::ClientSel::Name);
        let flags = crate::types::DisplayFlags {
            literal: parsed.literal && !parsed.list_all,
            ignore_keys: parsed.ignore_keys,
        };
        let fmt = if parsed.list_all {
            crate::cli::DISPLAY_ALL_VARIABLES
                .iter()
                .map(|name| format!("{}=#{{{}}}", name, name))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            parsed.template().unwrap_or_else(|| crate::commands::DISPLAY_MESSAGE_DEFAULT_FMT.to_string())
        };
        // `-v` logs the expansion before the result (FORMAT_VERBOSE).
        let verbose_template = (parsed.verbose && !parsed.list_all && !parsed.literal).then(|| fmt.clone());
        // Pass target pane index for PANE_POS_OVERRIDE (#113).
        // Bare %N (pane_is_id=true) goes through DisplayMessageById which
        // resolves the pane ID globally across windows (#332).
        let (rtx, rrx) = mpsc::channel::<String>();
        if pane_is_id {
            if let Some(pid) = target_pane {
                let _ = tx.send(CtrlReq::DisplayMessageById(rtx, fmt, pid, !print_stdout, duration_ms, client_sel, flags));
            } else {
                let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt, None, !print_stdout, duration_ms, client_sel, flags));
            }
        } else {
            let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt, target_pane, !print_stdout, duration_ms, client_sel, flags));
        }
        if let Ok(text) = rrx.recv() {
            if let Some(template) = &verbose_template {
                let trace = crate::util::display_verbose_trace(template, &text);
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("display-message".to_string(), trace));
                } else {
                    let _ = write!(write_stream, "{}", trace);
                    let _ = write_stream.flush();
                }
            }
            if print_stdout {
                // #647 (WIN-03): tmux prints `display-message -p` through
                // server_client_print(tc, 0, evb) (cmd-display-message.c:152),
                // whose parse == 0 arm always visually encodes the result with
                // VIS_OCTAL|VIS_CSTYLE|VIS_NOSLASH (server-client.c:3089-3091).
                // ESC, CR, BEL and friends become printable escapes; a tab or
                // newline is left alone, because VIS_TAB and VIS_NL are not in
                // that flag set.
                let text = crate::util::visual_escape_message(&text);
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("display-message".to_string(), text));
                } else {
                    let _ = writeln!(write_stream, "{}", text);
                    let _ = write_stream.flush();
                }
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "last-window" | "last" => {
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::LastWindow { resp: Some(resp_s) });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "last-pane" | "lastp" if last_pane_input_toggle(&args).is_some() => {
        // last-pane -d / -e: select-pane -l -d / -e, the -t naming the
        // window whose last pane it is; nothing is selected.
        let off = last_pane_input_toggle(&args).unwrap_or(true);
        let (resp_s, resp_r) = mpsc::channel();
        tx.send_unwrapped(CtrlReq::SetPaneInputOff {
            target: tx.target().cloned(),
            off,
            last: true,
            resp: Some(resp_s),
        });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "last-pane" | "lastp" => {
        // tmux's last-pane shares cmd_select_pane_exec with `select-pane -l`,
        // so it owes the same `no last pane` at exit 1 (cmd-select-pane.c:176).
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::LastPane { resp: resp_s });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "rotate-window" | "rotatew" => {
        // tmux tests for -D alone and falls through to the -U branch for
        // everything else, so bare `rotate-window` is `-U`.  This arm used to
        // send the flag through unnegated, which ran -U as a -D rotation and
        // made the two routes disagree with each other (#645).
        let upward = !args.iter().any(|a| *a == "-D");
        let _ = tx.send(CtrlReq::RotateWindow(upward));
    }
    "display-panes" | "displayp" => { let _ = tx.send(CtrlReq::DisplayPanes); }
    "break-pane" | "breakp" => {
        let (req, print) = parse_break_pane_args(&args, raw_target.as_deref());
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::BreakPaneReq { req, print, resp: resp_s });
        match resp_r.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(text)) => {
                if !text.is_empty() {
                    let _ = writeln!(write_stream, "{}", text);
                    let _ = write_stream.flush();
                }
            }
            Ok(Err(e)) => {
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
            }
            Err(_) => {}
        }
    }
    "join-pane" | "joinp" | "move-pane" | "movep" => {
        // -t was parsed by the generic -t handler above into target_win /
        // target_pane (with pane_is_id); -s is parsed here. Both keep the
        // `%id` form: `-s %3` used to become pane INDEX 3 of the active
        // window, so a source already in the target window joined nothing,
        // or the wrong pane, at exit 0.
        let horizontal = args.contains(&"-h");
        let src_raw = flag_value(&args, "-s");
        let src = src_raw.as_deref()
            .map(|sv| crate::types::TempTarget::from_parsed(&parse_target(sv)))
            .unwrap_or_default();
        let mut dst = crate::types::TempTarget {
            win: target_win,
            win_is_id: target_win_is_id,
            win_name: target_win_name.clone(),
            pane: target_pane,
            pane_is_id,
            session: None, pane_check_only: false,
        };
        // No -t window: a bare integer is the target window (legacy compat).
        if dst.win.is_none() && dst.win_name.is_none() && !dst.pane_is_id {
            dst.win = args.iter().enumerate()
                .find(|(i, a)| a.parse::<usize>().is_ok() && (*i == 0 || !matches!(args[*i - 1], "-s" | "-l" | "-p")))
                .and_then(|(_, s)| s.parse::<usize>().ok());
            dst.win_is_id = false;
        }
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::JoinPane {
            src_raw,
            src,
            dst,
            horizontal,
            // -d: graft the pane without switching to the target window
            // (cmd-join-pane.c:515). It was parsed nowhere, so join-pane
            // always switched, the same defect break-pane had (#689).
            detach: args.contains(&"-d"),
            // -b: the moved pane goes left of / above the target (#725).
            before: args.contains(&"-b"),
            resp: resp_s,
            same_window: matches!(cmd, "move-pane" | "movep"),
            size: crate::commands::join_size_arg(&args),
        });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "respawn-pane" | "respawnp" => {
        let workdir = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].to_string());
        let empty = args.iter().any(|a| *a == "-E");
        // -E implies replacing the running pane, so it also kills it.
        let kill = args.iter().any(|a| *a == "-k") || empty;
        // Honor `-- <shell-command>` (issue #399): Claude Code agent-teams
        // delivers the teammate launch via `respawn-pane -k -t %N -- "<cmd>"`.
        // Without this the pane is respawned with the default shell and the
        // teammate never boots (mailbox stays unread, task never runs).
        // tmux parity (#582): a multi-token argv after `--` keeps the marker
        // and token boundaries so build_command execs it directly; the
        // single-quoted-string teammate form keeps shell semantics.
        let command = args.iter().position(|a| *a == "--")
            .map(|i| {
                let tail = &args[i + 1..];
                if tail.len() > 1 {
                    format!("-- {}", requote_command_tail(tail))
                } else {
                    tail.join(" ")
                }
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            // tmux documents the command as a plain positional operand
            // (`respawn-pane [-k] [-c dir] [-t target] [shell-command]`), not
            // as a `--` tail. Without this the positional form was silently
            // dropped and the pane came back as the default shell, which also
            // left `#{pane_start_command}` unable to follow a respawn (#580).
            // `-t` is already stripped by `without_outer_target`; `-c` is the
            // only other flag here that takes a value.
            .or_else(|| respawn_positional_command(&args));
        // Read the reply: every respawn failure is a routine command error
        // ("pane ... still active" without -k, a bad -c, an unspawnable
        // command). Fire-and-forget here meant the CLI exited 0 with empty
        // output for a refusal that had just taken the whole server down.
        let (resp_s, resp_r) = mpsc::channel();
        // -e KEY=VALUE for the new process (#708). It was parsed (and kept
        // out of the command operand) but never sent, so it was dropped.
        let env_sets = env_flag_values(&args);
        let _ = tx.send(CtrlReq::RespawnPane(workdir, kill, command, empty, resp_s, env_sets));
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    // ── Cross-session pane forwarding commands ──────────────────────
    "pane-forward-extract" => {
        // Usage: pane-forward-extract <spec>, where <spec> is `:<win>.<pane>`,
        // `:<win>` (its active pane) or `%<id>`; the leading colon keeps a `1.0`
        // from parsing as session "1", pane 0. An older CLI sends a bare
        // `<win>.<pane>`, which is read as before.
        let spec = args.first().copied().unwrap_or(":0.0");
        let target = if spec.starts_with([':', '%', '@']) {
            crate::types::TempTarget::from_parsed(&parse_target(spec))
        } else {
            let pt = parse_target(spec);
            crate::types::TempTarget { win: Some(pt.window.unwrap_or(0)), pane: Some(pt.pane.unwrap_or(0)), ..Default::default() }
        };
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::PaneForwardExtract(target, rtx));
        if let Ok(resp) = rrx.recv_timeout(std::time::Duration::from_millis(5000)) {
            let _ = write!(write_stream, "{}\n", resp);
            let _ = write_stream.flush();
        } else {
            let _ = write!(write_stream, "ERR timeout\n");
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "pane-forward-inject" => {
        // Usage: pane-forward-inject <src_session> <src_addr> <src_key> <fwd_id> <fwd_port>
        //        <pid> <title> <rows> <cols> <screen_b64_len> [-h] [-t win.pane]
        // Followed by optional screen base64 data on next line.
        if args.len() >= 10 {
            let source_session = args[0].to_string();
            let source_addr = args[1].to_string();
            let source_key = args[2].to_string();
            let forward_id: u64 = args[3].parse().unwrap_or(0);
            let fwd_port: u16 = args[4].parse().unwrap_or(0);
            let pid: u32 = args[5].parse().unwrap_or(0);
            let title = args[6].replace('\x01', " ");
            let rows: u16 = args[7].parse().unwrap_or(24);
            let cols: u16 = args[8].parse().unwrap_or(80);
            let screen_b64_len: usize = args[9].parse().unwrap_or(0);
            let horizontal = args.iter().any(|a| *a == "-h");
            // `-tgt=<spec>`: the window and pane the joined pane goes next to
            // (`:<win>`, `:<win>.<pane>`, `%<id>`). The CLI used to drop it,
            // so a join that named a target pane landed beside the active
            // one. A generic `-t` from an older client still counts.
            let target = match args.iter().find_map(|a| a.strip_prefix("-tgt=")) {
                Some(spec) => crate::types::TempTarget::from_parsed(&parse_target(spec)),
                None => crate::types::TempTarget {
                    win: target_win, win_is_id: target_win_is_id, win_name: target_win_name.clone(),
                    pane: target_pane, pane_is_id, session: None, pane_check_only: false,
                },
            };
            // Read screen base64 data from remaining args/payload
            let screen_b64 = if screen_b64_len > 0 {
                // The base64 data may be appended after the args as a separate read
                let payload: String = args[10..].iter()
                    .filter(|a| **a != "-h" && !a.starts_with("-t"))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                if payload.len() >= screen_b64_len {
                    // base64 is ASCII; a corrupt payload must not panic here.
                    payload.get(..screen_b64_len).unwrap_or(&payload).to_string()
                } else {
                    payload
                }
            } else {
                String::new()
            };
            let (rs, rr) = mpsc::channel();
            let _ = tx.send(CtrlReq::PaneForwardInject {
                source_session, source_addr, source_key,
                forward_id, fwd_port, pid, title, rows, cols, screen_b64,
                target, horizontal, resp: rs,
            });
            match rr.recv_timeout(std::time::Duration::from_millis(5000)) {
                Ok(Err(e)) => { let _ = writeln!(write_stream, "ERR {}", e); }
                _ => { let _ = writeln!(write_stream, "OK"); }
            }
            let _ = write_stream.flush();
        } else {
            let _ = write!(write_stream, "ERR not enough args\n");
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "pane-forward-resize" => {
        // Usage: pane-forward-resize <forward_id> <rows> <cols>
        if args.len() >= 3 {
            let fwd_id: u64 = args[0].parse().unwrap_or(0);
            let rows: u16 = args[1].parse().unwrap_or(24);
            let cols: u16 = args[2].parse().unwrap_or(80);
            let _ = tx.send(CtrlReq::PaneForwardResize(fwd_id, rows, cols));
            let _ = write!(write_stream, "OK\n");
        }
        let _ = write_stream.flush();
        if !persistent && pending_chain.is_empty() { break; }
    }
    "pane-forward-status" => {
        // Usage: pane-forward-status <forward_id>
        let fwd_id: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::PaneForwardStatus(fwd_id, rtx));
        if let Ok(resp) = rrx.recv_timeout(std::time::Duration::from_millis(2000)) {
            let _ = write!(write_stream, "{}\n", resp);
        } else {
            let _ = write!(write_stream, "exited\n");
        }
        let _ = write_stream.flush();
        if !persistent && pending_chain.is_empty() { break; }
    }
    "pane-forward-kill" => {
        // Usage: pane-forward-kill <forward_id>
        let fwd_id: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(0);
        let _ = tx.send(CtrlReq::PaneForwardKill(fwd_id));
        let _ = write!(write_stream, "OK\n");
        let _ = write_stream.flush();
        if !persistent && pending_chain.is_empty() { break; }
    }
    "session-info" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::SessionInfo(rtx));
        // Bounded wait: session-info doubles as the Tier 2 execution barrier for
        // send_control, so it must never block the connection thread forever if
        // the event loop is momentarily wedged — that would leak the thread and
        // hold the socket open. 3s is far longer than a healthy loop cycle.
        if let Ok(line) = rrx.recv_timeout(Duration::from_secs(3)) {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("session-info".to_string(), line));
            } else {
                let _ = write!(write_stream, "{}\n", line); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "client-environ" => {
        // The attaching client's environment (tmux MSG_IDENTIFY_ENVIRON), a
        // base64 Windows environment block, for update-environment (#775).
        use base64::Engine;
        client_environ = args.first()
            .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.as_bytes()).ok())
            .and_then(|bytes| crate::client_env::decode_env_block(&bytes))
            .map(|entries| entries.into_iter()
                .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
                .collect());
        if !persistent { let _ = write!(write_stream, "ok\n"); }
    }
    "job-environ" => {
        // The environment a job of this server runs with (#804), a base64
        // Windows environment block, for a run-shell or if-shell the CLI
        // runs in its own process: tmux runs every job in the server.
        use base64::Engine;
        let block = crate::client_env::encode_env_block(crate::commands::job_environment());
        if !persistent {
            let _ = write!(write_stream, "{}\n", base64::engine::general_purpose::STANDARD.encode(&block));
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "client-attach" => {
        if !attached_sent {
            if let Some(env) = client_environ.clone() {
                let _ = tx.send(CtrlReq::ClientEnviron(env));
            }
            let _ = tx.send(CtrlReq::ClientAttach(client_id, connection_peer_pid(r.get_ref())));
            attached_sent = true;
        }
        if !persistent { let _ = write!(write_stream, "ok\n"); }
    }
    // tmux client flags the attaching client asks for (issue #724). Only
    // `read-only` (`attach -r`) is meaningful today; `!read-only` clears it,
    // the way tmux's server_client_set_flags reads a leading `!`.
    "client-flags" => {
        for flag in args.iter().flat_map(|a| a.split(',')) {
            let (off, name) = match flag.strip_prefix('!') {
                Some(n) => (true, n),
                None => (false, flag),
            };
            if name == "read-only" {
                client_readonly = !off;
                let _ = tx.send(CtrlReq::SetClientReadonly(client_id, client_readonly));
            }
        }
        if !persistent { let _ = write!(write_stream, "ok\n"); }
    }
    // Records the session this client arrived FROM (issue #566). Sent by the
    // client right after client-attach, because that is what creates the
    // registry entry this value is stored on.
    "client-last-session" => {
        if let Some(prev) = args.get(0) {
            if !prev.is_empty() {
                let _ = tx.send(CtrlReq::SetClientLastSession(client_id, prev.to_string()));
            }
        }
        if !persistent { let _ = write!(write_stream, "ok\n"); }
    }
    "client-detach" => {
        let _ = tx.send(CtrlReq::ClientDetach(client_id));
        attached_sent = false;
        if !persistent { let _ = write!(write_stream, "ok\n"); }
    }
    "bind-key" | "bind" => {
        let mut table = "prefix".to_string();
        let mut repeatable = false;
        let mut note: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            match args[i] {
                "-T" if i + 1 < args.len() => {
                    table = args[i + 1].to_string();
                    i += 2; continue;
                }
                // tmux bind-key -N note (cmd-bind-key.c "nrN:T:"). Unknown
                // here, it was read as the KEY and the binding vanished.
                "-N" if i + 1 < args.len() => {
                    note = Some(args[i + 1].to_string());
                    i += 2; continue;
                }
                "-n" => { table = "root".to_string(); i += 1; continue; }
                "-r" => { repeatable = true; i += 1; continue; }
                _ => break,
            }
        }
        if i < args.len() && i + 1 < args.len() {
            let key = args[i].to_string();
            let command = requote_command_tail(&args[i + 1..]);
            // Issue #635: tmux parses the bound command list at BIND time
            // (cmd-bind-key.c), so a dangling value-taking flag refuses the
            // binding rather than arming a key that silently acts on the
            // default target when it is pressed.
            let flag_error = crate::config::bind_command_list(&command)
                .iter()
                .find_map(|sub| {
                    let tokens = crate::commands::parse_command_line(sub);
                    crate::cli::validate_command_line_flags(&tokens).err()
                });
            if let Some(flag_error) = flag_error {
                let _ = writeln!(write_stream, "ERROR: {}", flag_error);
                let _ = write_stream.flush();
            } else {
                let _ = tx.send(CtrlReq::BindKey(table, key, command, repeatable, note));
            }
        }
    }
    "unbind-key" | "unbind" => {
        if args.iter().any(|a| *a == "-a" || (a.starts_with('-') && a.contains('a'))) {
            // Check if -T or -n was explicitly specified
            let mut has_table = false;
            let mut table = String::new();
            for (j, a) in args.iter().enumerate() {
                if *a == "-T" { if let Some(t) = args.get(j + 1) { table = t.to_string(); has_table = true; } }
                if *a == "-n" { table = "root".to_string(); has_table = true; }
            }
            if has_table {
                let _ = tx.send(CtrlReq::UnbindAllInTable(table));
            } else {
                let _ = tx.send(CtrlReq::UnbindAll);
            }
        } else {
            // Parse -n / -T flags for table-specific individual unbind
            let mut table: Option<String> = None;
            let mut t_value_idx: Option<usize> = None;
            let mut target_session_idx: Option<usize> = None;
            for (j, a) in args.iter().enumerate() {
                if *a == "-T" {
                    if let Some(t) = args.get(j + 1) {
                        table = Some(t.to_string());
                        t_value_idx = Some(j + 1);
                    }
                }
                if *a == "-n" { table = Some("root".to_string()); }
                // -t <session> is the target flag; skip its value
                if *a == "-t" { target_session_idx = Some(j + 1); }
            }
            // Find the key argument: first non-flag arg that isn't the -T table value
            // or the -t session target value
            let key_arg = args.iter().enumerate()
                .filter(|(i, a)| !a.starts_with('-') && Some(*i) != t_value_idx && Some(*i) != target_session_idx)
                .map(|(_, a)| *a)
                .next();
            if let Some(key) = key_arg {
                let _ = tx.send(CtrlReq::UnbindKey(key.to_string(), table));
            }
        }
    }
    "list-keys" | "lsk" if args.iter().any(|a| list_keys_flag(a, 'N')) => {
        // tmux list-keys -N: the bindings that carry a note, as notes.
        let (table, all, key) = list_keys_notes_args(&args);
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ListKeyNotes(rtx, table, all, key));
        if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("list-keys".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "list-keys" | "lsk" => {
        // Parse -T <table> for filtering by key table
        let table_filter = args.windows(2).find(|w| w[0] == "-T").map(|w| w[1].to_string());
        // Remaining non-flag args are optional key filter
        let key_filter: Option<String> = args.iter()
            .enumerate()
            .filter(|(i, a)| {
                !a.starts_with('-')
                && !(i > &0 && args.get(i - 1).map_or(false, |prev| *prev == "-T"))
            })
            .map(|(_, a)| a.to_string())
            .next();
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ListKeys(rtx));
        if let Ok(text) = rrx.recv() {
            let filtered = if table_filter.is_some() || key_filter.is_some() {
                text.lines().filter(|line| {
                    // list-keys output format: "bind-key [-r] -T <table> <key> <command>".
                    // The optional -r shifts every field, so read the table
                    // and key after the -T rather than at fixed positions (a
                    // repeatable binding used to vanish from -T and key filters).
                    let (line_table, line_key) = list_keys_line_table_key(line);
                    if let Some(ref tbl) = table_filter {
                        if line_table != Some(tbl.as_str()) {
                            return false;
                        }
                    }
                    if let Some(ref key) = key_filter {
                        if let Some(k) = line_key {
                            if k != key.as_str() {
                                return false;
                            }
                        }
                    }
                    true
                }).collect::<Vec<&str>>().join("\n")
            } else {
                text
            };
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("list-keys".to_string(), filtered));
            } else {
                let _ = write!(write_stream, "{}\n", filtered); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "set-option" | "set" | "set-window-option" | "setw" => {
        // Flags parse only BEFORE the first positional, and `--` ends option
        // parsing entirely (#583, the set-option sibling of the #562
        // send-keys fix). tmux (getopt) stops scanning at the option name,
        // so a dash-leading VALUE is data, never a flag: `set @k -u` must
        // store the literal "-u", not route the command to the unset path
        // (that was a silent rc-0 key deletion). Combined tokens like -ga,
        // -gu, and -gt still work; the shared parser separates any -t target
        // from the option operands.
        //
        // -U is an unset alias (tmux parity, #553). It was only recognized
        // CLIENT-side, so `set -U @x V` cleared the CLI's empty-value guard,
        // arrived here unrecognized, and fell through to the plain SET path:
        // the option was written where the caller asked for an unset, rc 0.
        let crate::cli::ParsedSetOptionArgs {
            flag_chars,
            positionals: non_flag_args,
            target: set_target,
            ..
        } = parse_set_option_args(&args);
        let has_u = flag_chars.contains('u') || flag_chars.contains('U');
        let has_a = flag_chars.contains('a');
        let has_q = flag_chars.contains('q');
        let has_o = flag_chars.contains('o');
        // `-F` expands the value as a format before it is stored (tmux parity).
        let has_f = flag_chars.contains('F');
        // -s is the server scope flag (#618). psmux runs one server per session
        // and keeps a single option store, so -s selects the same store as -g;
        // it is NOT genuine cross-session server-option storage, it just puts
        // the write where a tool passing tmux 3.2+ syntax expects to find it.
        let global = flag_chars.contains('g') || flag_chars.contains('s');
        // tmux parity (#580): `-p` is a bare PANE-SCOPE flag like `-w`; it
        // never consumes the next argument.
        let pane_scope = flag_chars.contains('p');
        // #648: `-w` (and the `setw` spelling) without `-g`/`-s` writes the
        // TARGET WINDOW's own option table, not the one global store. Only a
        // name the catalog marks window scope, or a user option, is scoped
        // that way — tmux derives scope from the option name, so `set -w` on
        // a session option keeps landing in the session store.
        let window_scope = (flag_chars.contains('w')
            || matches!(cmd, "set-window-option" | "setw"))
            && !global
            && !pane_scope;
        let window_option = window_scope
            && non_flag_args
                .first()
                .is_some_and(|name| crate::server::options::is_window_scoped_write(name));
        // `set -gw @k` writes tmux's global window table (#795 follow up).
        let global_window_user = non_flag_args.first().is_some_and(|name| {
            crate::server::options::is_global_window_user_write(
                flag_chars.contains('w') || matches!(cmd, "set-window-option" | "setw"),
                flag_chars.contains('g'),
                name,
            )
        }) && !pane_scope;
        if window_option || global_window_user {
            let raw_target = if global_window_user {
                crate::server::options::GLOBAL_WINDOW_TABLE.to_string()
            } else {
                set_target
                    .map(|t| t.trim_matches('"').to_string())
                    .unwrap_or_default()
            };
            let option = non_flag_args[0].to_string();
            let value = if has_u {
                String::new()
            } else {
                let joined = non_flag_args[1..].join(" ").trim_matches('"').to_string();
                expand_set_option_value(&tx, has_f, joined)
            };
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SetWindowOption {
                target: raw_target,
                option,
                value,
                unset: has_u,
                append: has_a,
                only_if_unset: has_o,
                quiet: has_q,
                resp: rtx,
            });
            if let Ok(reply) = rrx.recv_timeout(Duration::from_millis(2000)) {
                if !reply.is_empty() {
                    let _ = write!(write_stream, "{}\n", reply);
                    let _ = write_stream.flush();
                }
            }
        } else if pane_scope {
            let raw_target = pane_scope_target(
                extract_flag_value(&args, "-t")
                    .map(|s| s.trim_matches('"').to_string())
                    .unwrap_or_default(),
            );
            // #728: -a / -o / -q reach the pane writer too; they used to be
            // dropped on this route, so `set -pa` replaced instead of appending.
            let reply = match non_flag_args.first() {
                None => "ERROR: set-option -p: option and value required".to_string(),
                Some(option) => {
                    let value = if has_u {
                        String::new()
                    } else {
                        let joined = non_flag_args[1..].join(" ").trim_matches('"').to_string();
                        expand_set_option_value(&tx, has_f, joined)
                    };
                    let (rtx, rrx) = mpsc::channel::<String>();
                    let _ = tx.send(CtrlReq::SetPaneOption {
                        target: raw_target,
                        option: option.to_string(),
                        value,
                        unset: has_u,
                        append: has_a,
                        only_if_unset: has_o,
                        quiet: has_q,
                        resp: rtx,
                    });
                    rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default()
                }
            };
            if !reply.is_empty() {
                let _ = write!(write_stream, "{}\n", reply);
                let _ = write_stream.flush();
            }
        } else if !global
            && non_flag_args
                .first()
                .is_some_and(|name| crate::server::options::is_session_option(name))
        {
            // #803: without -g a session option goes to the SESSION's own
            // table; `-u` drops that entry so the global shows through again.
            let option = non_flag_args[0].to_string();
            let value = if has_u {
                String::new()
            } else {
                expand_set_option_value(&tx, has_f, non_flag_args[1..].join(" "))
            };
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SetSessionOption {
                option,
                value,
                unset: has_u,
                append: has_a,
                only_if_unset: has_o,
                quiet: has_q,
                resp: rtx,
            });
            if let Ok(reply) = rrx.recv_timeout(Duration::from_millis(2000)) {
                if !reply.is_empty() {
                    let _ = write!(write_stream, "{}\n", reply);
                    let _ = write_stream.flush();
                }
            }
        } else if has_u {
            if let Some(option) = non_flag_args.first() {
                if *option == "window-size" && !global {
                    let _ = tx.send(CtrlReq::SetWindowSize(None));
                } else {
                    let _ = tx.send(CtrlReq::SetOptionUnset(option.to_string()));
                }
            }
        } else if non_flag_args.len() >= 2 {
            let option = non_flag_args[0].to_string();
            let value = non_flag_args[1..].join(" ");
            let value = expand_set_option_value(&tx, has_f, value);
            if option == "window-size" && !global {
                let _ = tx.send(CtrlReq::SetWindowSize(Some(value)));
            } else if has_a {
                let _ = tx.send(CtrlReq::SetOptionAppend(option, value));
            } else if has_o {
                // `-o` on an option that is already set is an ERROR in tmux
                // (`already set: <name>`, exit 1), quiet only under `-q`. The
                // request used to be fire and forget, so the refusal was
                // invisible on this route; ask for the answer unless the
                // caller passed -q (#619 follow up).
                if has_q {
                    let _ = tx.send(CtrlReq::SetOptionOnlyIfUnset(option, value, None));
                } else {
                    let (rtx, rrx) = mpsc::channel::<String>();
                    let _ = tx.send(CtrlReq::SetOptionOnlyIfUnset(option, value, Some(rtx)));
                    if let Ok(reply) = rrx.recv_timeout(Duration::from_millis(2000)) {
                        if !reply.is_empty() {
                            let _ = write!(write_stream, "{}\n", reply);
                            let _ = write_stream.flush();
                        }
                    }
                }
            } else {
                let _ = tx.send(CtrlReq::SetOptionQuiet(option, value, has_q));
            }
        } else if non_flag_args.len() == 1 {
            // An option name with no value (#535). Previously this fell off the
            // end of the chain and the command vanished: no option set, no
            // warning, exit 0. tmux 3.4 splits it two ways, and so do we.
            let option = non_flag_args[0];
            if crate::server::options::missing_value_toggles(option) {
                // Boolean flag: `set -g mouse` toggles it (tmux parity, #278).
                // The config-file parser already did this; the CLI/TCP path
                // dropped it, so `psmux set -g mouse` was a no-op.
                let _ = tx.send(CtrlReq::SetOptionToggle(option.to_string()));
            }
            // Otherwise it is an error ("empty value"). The CLI reports it on
            // stderr with exit 1 before it ever reaches us (main.rs), which is
            // the only place an exit code exists; nothing to apply here.
        }
    }
    // Singular aliases: tmux's unambiguous-prefix resolution makes
    // `show-option` / `show-window-option` valid spellings there (#586).
    "show-options" | "show" | "show-window-options" | "showw"
    | "show-option" | "show-window-option" => {
        // Support combined flag tokens like -gv, -wv, -Av (tmux compat)
        let combined_has = |ch: char| -> bool {
            args.iter().any(|a| {
                if *a == format!("-{}", ch) { return true; }
                // Check combined tokens like -gv, -wvs, etc.
                a.starts_with('-') && a.len() > 2 && a.chars().skip(1).all(|c| c.is_ascii_alphabetic()) && a.contains(ch)
            })
        };
        let has_a = combined_has('A');
        let has_s = combined_has('s');
        let has_w = combined_has('w');
        let window_scope = matches!(cmd, "show-window-options" | "showw" | "show-window-option") || has_w;
        let has_v = combined_has('v');
        let has_q = combined_has('q');
        // #655: `-g` picks the GLOBAL window table, not the target window's own
        // one (options.c options_scope_from_flags:1086-1090). It used to be
        // parsed nowhere on this route, so `show -wg` answered with the active
        // window's resolved values and reported a window-local override as if
        // the global table held it.
        let has_g = combined_has('g');
        let window_listing = if has_g {
            crate::server::options::WindowListing::Global
        } else if has_a {
            crate::server::options::WindowListing::LocalAndInherited
        } else {
            crate::server::options::WindowListing::Local
        };
        // Issue #618: `-s` used to be parsed into a variable nothing read, so
        // `show-options -s` was accepted but printed the whole store, session
        // options and all. tmux points `-s` at the server option table
        // (options.c options_scope_from_flags) and a bare `show-options -s`
        // there lists server options only, so narrow the listing to the
        // catalog's server-scope names. A named query (`show -s escape-time`)
        // is untouched: tmux ignores `-s` for a table option too
        // (options_scope_from_name derives the scope from the name).
        let server_scope = has_s && !window_scope;
        // `values_only` is applied here rather than by the generic name
        // stripper below: an empty server option (copy-command is usually
        // empty) has no space to split on, so the stripper would have printed
        // its NAME where tmux prints a blank line.
        let server_listing = |values_only: bool| -> String {
            let mut out = String::new();
            for name in crate::server::option_catalog::server_option_names() {
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::ShowOptionValue(rtx, name.to_string()));
                if let Ok(v) = rrx.recv_timeout(Duration::from_millis(2000)) {
                    if let Some(text) = crate::terminal_overrides::show_array_lines(name, &v, values_only) {
                        out.push_str(&text);
                    } else if values_only {
                        out.push_str(&format!("{}\n", v));
                    } else {
                        out.push_str(&format!("{} {}\n", name, v));
                    }
                }
            }
            out
        };
        // Pane scope (issue #580): list the target pane's `set-option -p`
        // options. `-p` is a bare flag; only -t carries a value.
        if combined_has('p') {
            let raw_t = extract_flag_value(&args, "-t")
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_default();
            let raw_target = pane_scope_target(raw_t.clone());
            // #647 (WIN-02): a named query used to be ignored, so
            // `show-options -p -v -t %0 remain-on-exit` printed the whole pane
            // store as `name value` pairs where tmux prints just `on`. tmux
            // resolves the single entry and then, with -v, prints only the
            // value (cmd-show-options.c:193 `if (args_has(args, 'v'))
            // cmdq_print(item, "%s", value);`), so scripts can compare stdout
            // with on/off directly.
            let name = args.iter()
                .filter(|a| !a.starts_with('-'))
                .copied()
                .last();
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ShowPaneOptions(raw_target, rtx));
            if let Ok(reply) = rrx.recv_timeout(Duration::from_millis(2000)) {
                // What a pane INHERITS is its window's value, then the global
                // one, not the global one alone. tmux walks the same chain
                // (`show -pA` in a window with `remain-on-exit on` prints
                // `remain-on-exit* on`, verified against tmux 3.4).
                // A user option has no window catalog entry; its parent is
                // the one `@name` store (#728).
                let inherited = |n: &str| -> Option<String> {
                    let (frtx, frrx) = mpsc::channel::<String>();
                    if n.starts_with('@') {
                        let _ = tx.send(CtrlReq::ShowOptionValue(frtx, n.to_string()));
                    } else {
                        let _ = tx.send(CtrlReq::ShowWindowOptionValue(
                            frtx,
                            n.to_string(),
                            raw_t.clone(),
                        ));
                    }
                    frrx.recv_timeout(Duration::from_millis(2000)).ok()
                        .filter(|v| !v.is_empty())
                };
                let out = if let Some(name) = name {
                    crate::server::options::pane_option_query_reply(
                        &reply, name, has_v, has_a, has_q, inherited,
                    )
                } else {
                    crate::server::options::render_pane_options(&reply, has_a, inherited)
                };
                if !out.is_empty() {
                    let _ = write_stream.write_all(out.as_bytes());
                    let _ = write_stream.flush();
                }
            }
            if !persistent && pending_chain.is_empty() { break; }
            continue;
        }
        // The operand read the way tmux's getopt reads it, so the `-t`
        // value is never taken for an option name.
        let opt_name: Option<&str> = crate::cli::first_operand("show-options", &args);
        // The RAW -t target (issue #266 — per-window options like
        // automatic-rename must answer for the explicitly targeted window;
        // #648 — the server resolves it, because keeping only the numeric
        // half here threw the `s:NAME` form away and silently answered for
        // the ACTIVE window instead).
        let target_window: String = extract_flag_value(&args, "-t")
            .map(|t| t.trim_matches('"').to_string())
            .unwrap_or_default();
        // A name no option table knows, user options aside, is what tmux
        // calls an invalid option: cmd-show-options.c:117 fails with
        // "invalid option: NAME" unless -q (#846). It used to print an empty
        // value at rc 0, or read back whatever text an earlier `set` stored.
        if let Some(name) = opt_name.filter(|n| {
            !crate::server::option_catalog::is_valid_option_name(n)
        }) {
            if !has_q {
                let output = format!("ERROR: invalid option: {}\n", name);
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
                } else {
                    let _ = write_stream.write_all(output.as_bytes());
                    let _ = write_stream.flush();
                }
            }
            if !persistent && pending_chain.is_empty() { break; }
            continue;
        }
        // A user option is answered from the table the flags pick, and one
        // that table does not hold is "invalid option: @name" at rc 1, which
        // `-q` silences (cmd-show-options.c). It used to print the bare name
        // at rc 0, and `-q` with a name printed the whole listing.
        if let Some(name) = opt_name.filter(|n| n.starts_with('@')) {
            let table = if !window_scope {
                crate::server::options::UserOptionTable::Session
            } else if has_g {
                crate::server::options::UserOptionTable::GlobalWindow
            } else {
                crate::server::options::UserOptionTable::Window { inherited: has_a }
            };
            let (rtx, rrx) = mpsc::channel::<Option<(String, bool)>>();
            let _ = tx.send(CtrlReq::UserOptionLookup(rtx, name.to_string(), target_window.clone(), table));
            // A wait that runs out is not "not set": answering empty (or
            // "invalid option") for a key that is set left a script no way to
            // tell a busy server from an unset option (issue #832). It is the
            // client's own timeout error, and -q does not hide it.
            let output = match rrx.recv_timeout(Duration::from_millis(2000)) {
                Ok(Some((value, inherited))) => {
                    crate::server::options::render_user_option_line(name, &value, inherited, has_v)
                }
                Err(_) => "ERROR: no response from server (timed out)\n".to_string(),
                Ok(None) if has_q => String::new(),
                Ok(None) => format!("ERROR: invalid option: {}\n", name),
            };
            if !output.is_empty() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
                } else {
                    let _ = write_stream.write_all(output.as_bytes());
                    let _ = write_stream.flush();
                }
            }
            if !persistent && pending_chain.is_empty() { break; }
            continue;
        }
        // #803: a session listing answers for the table the flags pick: `-g`
        // prints the global values the session may be overriding, `-A` marks
        // every session option the session inherits with `*`.
        let scope_listing = |text: String| -> String {
            if window_scope || !(has_g || has_a) {
                return text;
            }
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ScopeSessionListing(rtx, text.clone(), has_g, has_a));
            rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or(text)
        };
        let session_query = opt_name.filter(|name| {
            !window_scope && (has_g || has_a) && crate::server::options::is_session_option(name)
        });
        if let Some(name) = session_query {
            // #803: `show -g <name>` reads the global table, `show -A <name>`
            // marks a value the session inherits (cmd-show-options.c:197).
            let (rtx, rrx) = mpsc::channel::<(String, bool)>();
            let _ = tx.send(CtrlReq::SessionOptionQuery(rtx, name.to_string(), has_g));
            if let Ok((value, inherited)) = rrx.recv_timeout(Duration::from_millis(2000)) {
                let output = if has_v {
                    format!("{}\n", value)
                } else {
                    format!("{}{} {}\n", name, if has_a && inherited { "*" } else { "" }, value)
                };
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
                } else {
                    let _ = write_stream.write_all(output.as_bytes());
                    let _ = write_stream.flush();
                }
            }
        } else if opt_name.is_some() {
            // Single-option query: show-options -v <name> or show <name>
            if let Some(name) = opt_name {
                let (rtx, rrx) = mpsc::channel::<String>();
                // `-wg <name>` reads the GLOBAL window table, never the target
                // window's own one (#655), the same split tmux makes in
                // options_scope_from_name (options.c:1046-1056).
                if window_scope && !has_g {
                    let _ = tx.send(CtrlReq::ShowWindowOptionValue(rtx, name.to_string(), target_window.clone()));
                } else if window_scope && name.starts_with('@') {
                    // `show -gw @k`: the global window table's user option.
                    let _ = tx.send(CtrlReq::ShowWindowOptionValue(
                        rtx,
                        name.to_string(),
                        crate::server::options::GLOBAL_WINDOW_TABLE.to_string(),
                    ));
                } else {
                    let _ = tx.send(CtrlReq::ShowOptionValue(rtx, name.to_string()));
                }
                if let Ok(text) = rrx.recv() {
                    // Options with no real per-window meaning that libtmux/tmuxp
                    // nonetheless probe via `-w` (issue #321); always resolve these
                    // to the global value even without `-A`.
                    const ALWAYS_GLOBAL_FALLBACK: &[&str] = &["pane-base-index", "base-index", "mouse"];
                    let should_fallback = window_scope
                        && (has_a || ALWAYS_GLOBAL_FALLBACK.contains(&name));
                    let resolved = if text.is_empty() && should_fallback {
                        // Fall back to global/session options. Without -A this only
                        // applies to the allowlist above; with -A it applies to any
                        // option so `show-window-options -A -v prefix` still works
                        // (session-only options must stay empty without -A so they
                        // don't leak through show-window-options, see #showw_sendkeys_p).
                        let (frtx, frrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::ShowOptionValue(frtx, name.to_string()));
                        frrx.recv().unwrap_or_default()
                    } else {
                        text
                    };
                    if !(has_q && resolved.is_empty()) {
                        let array_text = if window_scope {
                            None
                        } else {
                            crate::terminal_overrides::show_array_lines(name, &resolved, has_v)
                        };
                        let output = if let Some(text) = array_text {
                            text
                        } else if has_v {
                            format!("{}\n", resolved)
                        } else {
                            format!("{} {}\n", name, resolved)
                        };
                        if persistent {
                            let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
                        } else {
                            let _ = write_stream.write_all(output.as_bytes());
                            let _ = write_stream.flush();
                        }
                    }
                }
            }
        } else if has_v && opt_name.is_none() && server_scope {
            let output = server_listing(true);
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
            } else {
                let _ = write_stream.write_all(output.as_bytes());
                let _ = write_stream.flush();
            }
        } else if has_v && opt_name.is_none() {
            // -v without option name: list all options, values only
            let (rtx, rrx) = mpsc::channel::<String>();
            if window_scope {
                let _ = tx.send(CtrlReq::ShowWindowOptionsFor(rtx, target_window.clone(), window_listing));
            } else {
                let _ = tx.send(CtrlReq::ShowOptions(rtx));
            }
            if let Ok(text) = rrx.recv().map(|text| scope_listing(text)) {
                // Extract values only (each line is "option_name value")
                let values_only: String = text.lines()
                    .filter_map(|line| {
                        let trimmed = line.trim();
                        if trimmed.is_empty() { return None; }
                        // Split at first space: name value
                        if let Some(pos) = trimmed.find(' ') {
                            Some(&trimmed[pos + 1..])
                        } else {
                            Some(trimmed) // option with no value
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let output = if values_only.is_empty() { String::new() } else { format!("{}\n", values_only) };
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), output));
                } else {
                    let _ = write_stream.write_all(output.as_bytes());
                    let _ = write_stream.flush();
                }
            }
        } else {
            if window_scope {
                let (rtx, rrx) = mpsc::channel::<String>();
                // #648: `-A` asks tmux's inheritance question, so the listing
                // marks every option this window takes from the global store
                // with a trailing `*` on the NAME (cmd-show-options.c).
                // #655: that is the WHOLE of what `-A` adds. This arm used to
                // append the entire session listing after the window one, so
                // `show -wA` answered with 77 lines where tmux prints one
                // merged window-scope list.
                let _ = tx.send(CtrlReq::ShowWindowOptionsFor(rtx, target_window.clone(), window_listing));
                if let Ok(text) = rrx.recv() {
                    if persistent {
                        let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), text));
                    } else if !text.is_empty() {
                        // A window that owns nothing prints NOTHING, not the
                        // blank line a bare `{}\n` would emit (#655).
                        let _ = write!(write_stream, "{}\n", text);
                        let _ = write_stream.flush();
                    }
                }
            } else if server_scope {
                let text = server_listing(false);
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
                }
            } else {
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::ShowOptions(rtx));
                if let Ok(text) = rrx.recv().map(|text| scope_listing(text)) {
                    if persistent {
                        let _ = tx.send(CtrlReq::ShowTextPopup("show-options".to_string(), text));
                    } else {
                        let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                    }
                }
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "source-file" | "source" => {
        // tmux's flags `Fnqv` and `-t target` (cmd-source-file.c:42), then one
        // or more paths, all of which are read in order.
        let spec = crate::config::source_spec_from_args(&args);
        if spec.paths.is_empty() {
            // tmux's args check (cmd-source-file.c:42 wants at least one).
            let _ = writeln!(write_stream, "ERROR: source-file: too few arguments (need at least 1)");
        } else if !spec.parse_only {
            // -q and -F go along so the server reads every path the same
            // way (config::parse_source_spec reads the flags back).
            let source_spec = spec.wire();
            // A one-shot source-file answers once the file has run, as
            // tmux's does: cmd-source-file.c returns CMD_RETURN_WAIT and
            // the item continues only when the inserted commands are done
            // (cmd_source_file_done, :131), and the client exits after
            // that. It used to answer at once while the server went on
            // running the file (if-shell conditions are shells), so the
            // caller's next query queued behind the whole file and timed
            // out on a loaded machine (#734 follow up, side findings 2B).
            let (done_tx, done_rx) = mpsc::channel::<crate::types::SourceOutcome>();
            let waiter = (!persistent).then_some(done_tx);
            let _ = tx.send(CtrlReq::SourceFile(source_spec, waiter));
            // The file's commands print what they print, and each error
            // is an `ERROR:` line, which makes the CLI exit 1 as tmux's
            // client does (issue #842). Written with write_all so this
            // source-file itself does not count as failed: tmux runs the
            // rest of the line (`source-file f \; display x`) whatever a
            // command inside the file did.
            if let Ok(outcome) = done_rx.recv() {
                let mut reply = outcome.output;
                if !reply.is_empty() && !reply.ends_with('\n') {
                    reply.push('\n');
                }
                for e in &outcome.errors {
                    reply.push_str(&format!("ERROR: {}\n", e));
                }
                if !reply.is_empty() {
                    let _ = write_stream.write_all(reply.as_bytes());
                }
            }
        }
    }
    "move-window" | "movew" => {
        // Both ends stay RAW here: `+1`, `-1`, `{last}`, `$`, a window name and
        // a plain index all mean different things, and only the server holds the
        // window list needed to tell them apart (issue #602). The old code
        // parsed the destination to a usize and never looked at `-s` at all, so
        // `move-window -s S:2 -t S:9` moved the ACTIVE window.
        let src = flag_value(&args, "-s");
        let dst = raw_target.clone()
            .or_else(|| flag_value(&args, "-t"))
            .or_else(|| {
                args.iter().enumerate()
                    .find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[*i - 1] != "-s"))
                    .map(|(_, a)| a.to_string())
            });
        let has = |f: &str| args.iter().any(|a| *a == f);
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::MoveWindow {
            src,
            dst,
            detach: has("-d"),
            kill: has("-k"),
            renumber: has("-r"),
            after: has("-a"),
            before: has("-b"),
            resp: resp_s,
        });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "swap-window" | "swapw" => {
        // Source: `-s <win>`, kept raw for the server-side resolver (#559:
        // `-s sess:99` used to fail a bare usize parse and silently fall back
        // to the ACTIVE window).
        let src = flag_value(&args, "-s");
        // Destination: the raw -t value (#559: a bare `-t 0` from a raw TCP
        // client parses as a SESSION target, leaving target_win None and
        // silently doing nothing), else a bare positional.
        let dst = raw_target.clone()
            .or_else(|| flag_value(&args, "-t"))
            .or_else(|| target_win.map(|w| w.to_string()))
            .or_else(|| {
                args.iter().enumerate()
                    .find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[*i - 1] != "-s"))
                    .map(|(_, a)| a.to_string())
            });
        match dst {
            Some(d) => {
                let (resp_s, resp_r) = mpsc::channel();
                let _ = tx.send(CtrlReq::SwapWindow {
                    src,
                    dst: d,
                    detach: args.iter().any(|a| *a == "-d"),
                    resp: resp_s,
                });
                if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
                    if !persistent {
                        let _ = writeln!(write_stream, "ERROR: {}", e);
                        let _ = write_stream.flush();
                    }
                }
            }
            None => {
                // tmux requires -t; without one there is nothing to swap with.
                if !persistent {
                    let _ = writeln!(write_stream, "ERROR: can't find window: ");
                    let _ = write_stream.flush();
                }
            }
        }
    }
    "link-window" | "linkw" => {
        // `-s` and `-t` are RAW specs now (#693 item 1): a session qualified
        // source reads, and the destination need not exist yet.
        let la = parse_link_window_args(&args, raw_target.as_deref());
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::LinkWindowReq {
            src: la.src, dst: la.dst, detach: la.detach,
            kill: la.kill, after: la.after, before: la.before,
            resp: resp_s,
        });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "unlink-window" | "unlinkw" => {
        let target = parse_unlink_window_target(&args, raw_target.as_deref());
        let (resp_s, resp_r) = mpsc::channel();
        let _ = tx.send(CtrlReq::UnlinkWindowReq { target, resp: resp_s });
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "find-window" | "findw" => {
        let pattern = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::FindWindow(rtx, pattern));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("find-window".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "pipe-pane" | "pipep" => {
        let stdin_flag = args.iter().any(|a| *a == "-I");
        let stdout_flag = args.iter().any(|a| *a == "-O");
        let toggle = args.iter().any(|a| *a == "-o");
        // #482: everything after pipe-pane's own options (-I/-O/-o; the -t target
        // was already stripped by the global -t parser) is an opaque shell
        // command. Skip only the leading option flags and keep the rest verbatim
        // so the command's OWN dash-flags survive (e.g.
        // `pwsh -NoProfile -EncodedCommand <b64>`). Previously every dash-token
        // was filtered out, silently mangling the piped command.
        let mut start = 0;
        while start < args.len() && matches!(args[start], "-I" | "-O" | "-o") {
            start += 1;
        }
        let cmd = args[start..].join(" ");
        let (stdin, stdout) = if !stdin_flag && !stdout_flag {
            (false, true)
        } else {
            (stdin_flag, stdout_flag)
        };
        // Same reply shape as #559/#566: a direct file sink that cannot be
        // opened must reach the caller as a non-zero exit, not a silent
        // rc-0 no-op. The handler answers "" on acceptance; only an
        // "ERROR: ..." reply is forwarded.
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::PipePane(cmd, stdin, stdout, toggle, Some(rtx)));
        let resp = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
        if !persistent {
            if !resp.is_empty() {
                let _ = write!(write_stream, "{}\n", resp);
                let _ = write_stream.flush();
            }
            // pipe-pane can sit in the middle of a chained line
            // (`pipe-pane -o ... \; display-message ...`); breaking with
            // queued sub-commands would silently drop the rest of the
            // chain. With no chain pending, the client's half-close ends
            // the loop on the next read anyway.
            if pending_chain.is_empty() { break; }
        }
    }
    "select-layout" | "selectl" => {
        let layout = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"tiled").to_string();
        let _ = tx.send(CtrlReq::SelectLayout(layout));
    }
    "next-layout" | "nextl" => {
        let _ = tx.send(CtrlReq::NextLayout);
    }
    "list-clients" | "lsc" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(list_clients_request(&args, rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("list-clients".to_string(), text));
            } else {
                // Each row already ends in a newline; a second one printed an
                // empty line after the list, and a line for no clients at all.
                let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "switch-client" | "switchc" => {
        let has_big_t = args.windows(2).any(|w| w[0] == "-T");
        if has_big_t {
            let table = args.windows(2).find(|w| w[0] == "-T").map(|w| w[1].to_string()).unwrap_or_default();
            if persistent {
                // The attached client latches its own table and only tells the
                // server so `#{client_key_table}` agrees; nothing to reply to.
                let _ = tx.send(CtrlReq::SwitchClientTable(table, None));
            } else {
                // tmux errors with "table %s doesn't exist" for a table that
                // has no bindings, so a one-shot caller must see it (#640).
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::SwitchClientTable(table, Some(rtx)));
                let resp = rrx.recv_timeout(Duration::from_millis(2000))
                    .unwrap_or_else(|_| "OK".to_string());
                let _ = write!(write_stream, "{}\n", resp);
                let _ = write_stream.flush();
            }
        } else if args.contains(&"-n") || args.contains(&"-p") || args.contains(&"-l") {
            // #566: these three were fire-and-forget, so a failed or misdirected
            // switch was indistinguishable from a successful one at the CLI (rc 0,
            // no output). Give them the same reply channel the -t arm already has
            // so the caller can exit non-zero, matching tmux.
            let flag = if args.contains(&"-n") { 'n' }
                       else if args.contains(&"-p") { 'p' }
                       else { 'l' };
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SwitchClient(String::new(), flag, Some(rtx)));
            let resp = rrx.recv_timeout(Duration::from_millis(2000))
                .unwrap_or_else(|_| "OK".to_string());
            if !persistent {
                let _ = write!(write_stream, "{}\n", resp);
                let _ = write_stream.flush();
            }
        } else {
            // -t <target> was already extracted into raw_target by the global -t
            // parser. Pass the FULL target (session:window.pane / @window / %pane)
            // to the server loop so it switches the session AND selects the
            // addressed window/pane, and validates existence (#483). Previously
            // the window/pane suffix was stripped and silently ignored.
            let target = raw_target.clone().unwrap_or_default();
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SwitchClientTarget(target, rtx));
            let resp = rrx.recv_timeout(Duration::from_millis(2000))
                .unwrap_or_else(|_| "OK".to_string());
            if !persistent {
                let _ = write!(write_stream, "{}\n", resp);
                let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "lock-client" | "lockc" => {
        let _ = tx.send(CtrlReq::LockClient);
    }
    "refresh-client" | "refresh" => {
        // tmux restricts -C/-B/-A/-f to control-mode clients. A one-shot CLI
        // client is not one, so reject the flag instead of silently ignoring
        // it and letting the caller believe the size/subscription was applied.
        if let Some(flag) = args.iter().find(|a| matches!(**a, "-C" | "-B" | "-A" | "-f")) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: refresh-client {}: not a control client", flag);
                let _ = write_stream.flush();
            }
        } else if args.iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('l')) {
            // tmux refresh-client -l: ask the client's terminal for its
            // clipboard (OSC 52) and store the answer as a new paste buffer
            // (cmd-refresh-client.c tty_clipboard_query). psmux's client is
            // on this desktop, so the Windows clipboard is read directly and
            // the buffer exists before the next command runs, which makes
            // `refresh-client -l ; paste-buffer -p` paste the clipboard
            // (#796).
            if let Some(text) = crate::clipboard::read_from_system_clipboard() {
                if !text.is_empty() {
                    let _ = tx.send(CtrlReq::SetBuffer(text));
                }
            }
        } else {
            let _ = tx.send(CtrlReq::RefreshClient);
        }
    }
    "suspend-client" | "suspendc" => {
        let _ = tx.send(CtrlReq::SuspendClient);
    }
    "copy-mode-page-up" => {
        let _ = tx.send(CtrlReq::CopyModePageUp);
    }
    "clear-history" | "clearhist" => {
        let _ = tx.send(CtrlReq::ClearHistory);
    }
    // TEST ONLY: `debug-stall <ms>` stalls the server loop so a test can
    // reproduce a long stall on demand.  Inert (an unknown command) unless
    // the server process was started with PSMUX_TEST_STALL_HOOK=1; the CLI
    // has no verb for it, tests send it as a raw line over the TCP port.
    "debug-stall" if std::env::var("PSMUX_TEST_STALL_HOOK").as_deref() == Ok("1") => {
        let ms = args.first().and_then(|a| a.parse::<u64>().ok()).unwrap_or(0);
        let _ = tx.send(CtrlReq::DebugStall(ms));
    }
    "save-buffer" | "saveb" => {
        let path = args.iter().find(|a| **a == "-" || !a.starts_with('-')).unwrap_or(&"").to_string();
        let _ = tx.send(CtrlReq::SaveBuffer(path));
    }
    "load-buffer" | "loadb" => {
        let path = args.iter().find(|a| **a == "-" || !a.starts_with('-')).unwrap_or(&"").to_string();
        let _ = tx.send(CtrlReq::LoadBuffer(path));
    }
    "set-environment" | "setenv" => {
        // tmux cmd-set-environment.c: -g global / session scope, -u unset,
        // -r removal marker, -h hidden, -F format expanded value (#775).
        match crate::environ::parse_set_environment(&args) {
            Ok(mut parsed) => {
                if let Some(v) = parsed.value.take() {
                    parsed.value = Some(expand_set_option_value(tx, parsed.format, v));
                }
                let _ = tx.send(CtrlReq::SetEnvironment(parsed));
            }
            Err(e) => {
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(e));
                } else {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                }
            }
        }
    }
    "show-environment" | "showenv" => {
        // tmux cmd-show-environment.c: one entry for a NAME, `-NAME` for a
        // removal, `unknown variable: NAME` for a miss, -g the global
        // (server start) environment, -s shell syntax, -h hidden (#775).
        let reply = match crate::environ::parse_show_environment(&args) {
            Ok(parsed) => {
                let (rtx, rrx) = mpsc::channel::<Result<String, String>>();
                let _ = tx.send(CtrlReq::ShowEnvironment(parsed, rtx));
                rrx.recv().ok()
            }
            Err(e) => Some(Err(e)),
        };
        match reply {
            Some(Ok(text)) => {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("show-environment".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
                }
            }
            Some(Err(e)) => {
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(e));
                } else {
                    let _ = writeln!(write_stream, "ERROR: {}", e); let _ = write_stream.flush();
                }
            }
            None => {}
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "set-hook" => {
        // Same positional discipline as set-option (#583): flags parse only
        // BEFORE the hook name, `--` ends option parsing. `set-hook
        // after-new-window -u` used to route to the unset path and silently
        // delete the hook at rc 0; a dash-leading token after the hook name
        // is part of the hook command.
        if let Some(req) = set_hook_request(&args) {
            let _ = tx.send(req);
        }
    }
    "show-hooks" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ShowHooks(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("show-hooks".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "wait-for" => {
        let lock = args.iter().any(|a| *a == "-L");
        let signal = args.iter().any(|a| *a == "-S");
        let unlock = args.iter().any(|a| *a == "-U");
        let channel = args.iter().find(|a| !a.starts_with('-')).unwrap_or(&"").to_string();
        let op = if lock { WaitForOp::Lock }
            else if signal { WaitForOp::Signal }
            else if unlock { WaitForOp::Unlock }
            else { WaitForOp::Wait };
        let _ = tx.send(CtrlReq::WaitFor(channel, op));
    }
    "display-menu" | "menu" => {
        let mut x_pos: Option<i16> = None;
        let mut y_pos: Option<i16> = None;
        let mut title = String::new();
        let mut skip_indices = std::collections::HashSet::new();
        let mut i = 0;
        while i < args.len() {
            match args[i] {
                "-x" => { if let Some(v) = args.get(i+1) { x_pos = v.parse().ok(); skip_indices.insert(i); skip_indices.insert(i+1); i += 1; } }
                "-y" => { if let Some(v) = args.get(i+1) { y_pos = v.parse().ok(); skip_indices.insert(i); skip_indices.insert(i+1); i += 1; } }
                "-T" => { if let Some(v) = args.get(i+1) { title = v.to_string(); skip_indices.insert(i); skip_indices.insert(i+1); i += 1; } }
                _ => {}
            }
            i += 1;
        }
        // Collect remaining positional args (name, key, command triplets)
        let positional: Vec<&str> = args.iter().enumerate()
            .filter(|(idx, a)| !skip_indices.contains(idx) && !a.starts_with('-'))
            .map(|(_, a)| *a).collect();
        // Build menu from triplets
        let mut menu = crate::types::Menu { title, items: Vec::new(), selected: 0, x: x_pos, y: y_pos };
        let mut pi = 0;
        while pi < positional.len() {
            let name = positional[pi];
            if name.is_empty() || name == "-" {
                menu.items.push(crate::types::MenuItem { name: String::new(), key: None, command: String::new(), is_separator: true });
                pi += 1;
            } else {
                let key = positional.get(pi + 1).and_then(|k| k.chars().next());
                let command = positional.get(pi + 2).map(|c| c.to_string()).unwrap_or_default();
                menu.items.push(crate::types::MenuItem { name: name.to_string(), key, command, is_separator: false });
                pi += 3;
            }
        }
        if !menu.items.is_empty() {
            let _ = tx.send(CtrlReq::DisplayMenuDirect(menu));
        }
    }
    "new-pane" | "newp" => {
        let p = parse_new_pane_args(&args);
        if let Some(msg) = p.refusal() {
            let _ = writeln!(write_stream, "ERROR: {}", msg);
            let _ = write_stream.flush();
            if !persistent { break; }
            line.clear();
            continue;
        }
        if p.print {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::NewFloat { command: p.command, x: p.x, y: p.y, w: p.w, h: p.h, border: p.border, title: p.title, start_dir: p.start_dir, detached: p.detached, empty: p.empty, resp: Some(rtx), extra: p.extra });
            if let Ok(text) = rrx.recv_timeout(Duration::from_millis(2000)) {
                let text = new_pane_print_text(text, p.format.as_deref(), |r| { let _ = tx.send(r); });
                let _ = write!(write_stream, "{}\n", text);
                let _ = write_stream.flush();
            }
            if !persistent && pending_chain.is_empty() { break; }
        } else {
            let _ = tx.send(CtrlReq::NewFloat { command: p.command, x: p.x, y: p.y, w: p.w, h: p.h, border: p.border, title: p.title, start_dir: p.start_dir, detached: p.detached, empty: p.empty, resp: None, extra: p.extra });
        }
    }
    "display-popup" | "popup" => {
        // One parser for every display-popup path (#792): -B, -T, -b, -S, -s
        // and the rest of tmux's flags are read here instead of leaking into
        // the command string.
        match crate::popup::parse_display_popup_args(&args) {
            Ok(req) => {
                // Default close-on-exit = true (tmux parity: popup closes when command finishes)
                let close_on_exit = !req.keep_open;
                let _ = tx.send(CtrlReq::DisplayPopup(req, close_on_exit));
            }
            Err(e) => {
                let _ = writeln!(write_stream, "ERROR: display-popup: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "confirm-before" | "confirm" => {
        let mut prompt: Option<String> = None;
        let mut i = 0;
        while i < args.len() {
            if args[i] == "-p" {
                if let Some(p) = args.get(i+1) { prompt = Some(p.to_string()); i += 1; }
            }
            i += 1;
        }
        let command = crate::cli::deferred_command_start(cmd, &args)
            .map(|i| requote_command_tail(&args[i..]))
            .unwrap_or_default();
        let prompt_str = prompt.unwrap_or_else(|| format!("Run '{}'", command));
        let _ = tx.send(CtrlReq::ConfirmBefore(prompt_str, command));
    }
    // tmux standard aliases (issue #275: full -a/-s/-t/-P parity)
    "detach-client" | "detach" => {
        let kill_parent = args.iter().any(|a| *a == "-P");
        let detach_all_others = args.iter().any(|a| *a == "-a");
        // -s <session> targets a specific session.  We're already routed to this
        // server (one server per session), so -s anything is honored by detaching
        // every client of this session.
        let detach_session = args.windows(2).any(|w| w[0] == "-s");
        // -t <target>: numeric ID, %ID, or tty_name like "/dev/pts/2"
        let target_str = raw_target.clone();
        let target_cid_numeric: Option<u64> = target_str.as_ref()
            .and_then(|t| t.trim_start_matches('%').parse::<u64>().ok());

        if detach_session {
            let _ = tx.send(CtrlReq::DetachAllClients(kill_parent));
            // This client is part of the session, so it will be detached too.
            attached_sent = false;
        } else if detach_all_others {
            let _ = tx.send(CtrlReq::DetachAllOtherClients(client_id, kill_parent));
            // Current client stays attached.
        } else if let Some(cid) = target_cid_numeric {
            if cid == client_id {
                if kill_parent {
                    let _ = crate::types::send_directive_to_client(client_id, "DETACH-KILL-PARENT");
                }
                let _ = tx.send(CtrlReq::ClientDetach(client_id));
                attached_sent = false;
            } else {
                // `%N` and `N` name the client listed as /dev/pts/N. That number
                // is the client's pid now (issue #724); the server falls back to
                // the connection id N for a client listed under it.
                let _ = tx.send(CtrlReq::ForceDetachClientByTty(format!("/dev/pts/{}", cid), kill_parent));
            }
        } else if let Some(tty) = target_str {
            // Non-numeric -t value: treat as a tty_name lookup.
            let _ = tx.send(CtrlReq::ForceDetachClientByTty(tty, kill_parent));
        } else {
            // No flags, no -t: detach THIS client.
            if kill_parent {
                let _ = crate::types::send_directive_to_client(client_id, "DETACH-KILL-PARENT");
            }
            let _ = tx.send(CtrlReq::ClientDetach(client_id));
            attached_sent = false;
        }
    }
    "attach-session" | "attach" => {
        if !attached_sent {
            if let Some(env) = client_environ.clone() {
                let _ = tx.send(CtrlReq::ClientEnviron(env));
            }
            let _ = tx.send(CtrlReq::ClientAttach(client_id, connection_peer_pid(r.get_ref())));
            attached_sent = true;
        }
    }
    "kill-server" => {
        // An ATTACHED client means tmux's "kill the server I am on", which in
        // psmux is every server on this socket (#649). A one-shot CLI
        // connection is the CLI's own fan-out arriving, so it must kill this
        // server and nothing else, or the fan-out would recurse.
        if persistent {
            let all = args.iter().any(|a| *a == "-a" || *a == "--all");
            let _ = tx.send(CtrlReq::KillServerScoped(all));
        } else {
            let _ = tx.send(CtrlReq::KillServer);
            // Hold this one-shot connection open until the server exits: the
            // caller reads it until EOF and force-kills the pid 50ms after,
            // so an early close makes the force-kill land in the middle of
            // the shutdown and orphan whatever it had not killed yet (#686).
            std::thread::sleep(crate::warm_pane_sync::KILL_SERVER_HOLD);
        }
    }
    "choose-tree" | "choose-window" | "choose-session" => {
        // These are interactive choosers — send a dump that client handles
        // For now, map to listing which the client renders as a chooser
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ListTree(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("choose-tree".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "copy-mode" => {
        // `-q` leaves, `-H` hides the position indicator for this entry only
        // (`window-copy.c` `window_copy_init` reads the flag), `-u` pages up,
        // and they combine the way tmux's `cmd_copy_mode_exec` combines them
        // (#704).
        let _ = tx.send(CtrlReq::CopyModeCmd(crate::copy_mode::CopyModeFlags::parse(&args)));
    }
    "clock-mode" => { let _ = tx.send(CtrlReq::ClockMode); }
    // Overlay interaction commands (sent by client during active overlays)
    "popup-input" => {
        if let Some(encoded) = args.get(0) {
            if let Some(decoded) = base64_decode(encoded) {
                let _ = tx.send(CtrlReq::PopupInput(decoded.into_bytes()));
            }
        }
    }
    "popup-input-raw" => {
        // Raw bytes (not base64) for single-byte key sequences
        if let Some(encoded) = args.get(0) {
            if let Some(decoded) = base64_decode(encoded) {
                let _ = tx.send(CtrlReq::PopupInput(decoded.into_bytes()));
            }
        }
    }
    "overlay-close" => { let _ = tx.send(CtrlReq::OverlayClose); }
    "display-panes-select" => {
        if let Some(idx) = args.get(0).and_then(|s| s.parse::<usize>().ok()) {
            let _ = tx.send(CtrlReq::DisplayPaneSelect(idx));
        }
    }
    "confirm-respond" => {
        let yes = args.get(0).map(|a| *a == "y" || *a == "yes").unwrap_or(false);
        let _ = tx.send(CtrlReq::ConfirmRespond(yes));
    }
    "menu-select" => {
        if let Some(idx) = args.get(0).and_then(|s| s.parse::<usize>().ok()) {
            let _ = tx.send(CtrlReq::MenuSelect(idx));
        }
    }
    "menu-navigate" => {
        let delta = args.get(0).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        let _ = tx.send(CtrlReq::MenuNavigate(delta));
    }
    "customize-navigate" => {
        let delta = args.get(0).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
        let _ = tx.send(CtrlReq::CustomizeNavigate(delta));
    }
    "customize-edit" => {
        let _ = tx.send(CtrlReq::CustomizeEdit);
    }
    "customize-edit-update" => {
        let text = args.join(" ");
        let _ = tx.send(CtrlReq::CustomizeEditUpdate(text));
    }
    "customize-edit-confirm" => {
        let _ = tx.send(CtrlReq::CustomizeEditConfirm);
    }
    "customize-edit-cancel" => {
        let _ = tx.send(CtrlReq::CustomizeEditCancel);
    }
    "customize-reset-default" => {
        let _ = tx.send(CtrlReq::CustomizeResetDefault);
    }
    "customize-filter" => {
        let text = args.join(" ");
        let _ = tx.send(CtrlReq::CustomizeFilter(text));
    }
    "__config-warnings" => {
        // Internal: the client that just started or claimed this server asks
        // for the warnings its config load recorded (#706). Not a tmux command
        // and not in the command table.
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ConfigWarnings(rtx));
        if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
            let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "show-messages" | "showmsgs" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ShowMessages(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("show-messages".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "command-prompt" => {
        // Opened on an attached client (#789 follow up). Without -b the
        // command line waits until the prompt closes, as tmux's does
        // (cmd-command-prompt.c:174 returns CMD_RETURN_WAIT). A client's own
        // persistent connection never waits: that client reports the prompt
        // closed over this same connection.
        let mut line = args.iter().map(|a| crate::util::quote_arg_if_needed(a)).collect::<Vec<_>>().join(" ");
        // The outer -t was lifted off by the target scan above; it names the
        // client the prompt opens on, so it goes back in front.
        if let Some(t) = raw_target.as_deref() {
            line = format!("-t {} {}", crate::util::quote_arg_if_needed(t), line);
        }
        let background = crate::client::parse_command_prompt_args(&line).background;
        let wait = !persistent && !background;
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::CommandPrompt(line, wait, rtx));
        if let Ok(reply) = rrx.recv() {
            if !persistent && !reply.is_empty() {
                let _ = writeln!(write_stream, "{}", reply);
                let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "prompt-done" => {
        if let Some(id) = args.first().and_then(|s| s.parse::<u64>().ok()) {
            let _ = tx.send(CtrlReq::PromptDone(id));
        }
    }
    "run-shell" | "run" => {
        // Only strip run-shell's OWN "-b" flag. A blind `!a.starts_with('-')`
        // filter here also deleted flags belonging to the wrapped shell
        // command itself (e.g. `run-shell psmux new-window -n X -c 'dir'`
        // lost both -n and -c, `run-shell psmux set-option -g @o v` lost
        // -g), silently mangling the command into positional garbage. This
        // was the root cause of #402's "[A] command-prompt single-quoted -c
        // FAILS" case: the interactive command-prompt/TCP path reaches this
        // arm directly, while the stored-bind path (fixed in eb5bee1) goes
        // through commands.rs::execute_command_string_single, which already
        // does an exact "-b" match instead of a starts_with prefix filter.
        // Strip only a BALANCED pair of outer wrapping quotes, e.g.
        // run-shell "'~/plugins/foo.tmux'" -> ~/plugins/foo.tmux.
        // A blind trim_matches here was the root cause of #402: it also removed
        // a lone TRAILING quote when the command's last argument was legitimately
        // quoted (e.g. `psmux new-window -c 'C:\path'` or `pwsh -Command "..."`),
        // producing an unterminated-string parse error in the spawned shell so the
        // command silently never ran. Only unwrap when both ends match the same quote.
        let parsed = run_shell_command_text(&args);
        let parse_failed = parsed.is_err();
        let (background, delay, shell_cmd) = match parsed {
            Ok(parsed) => parsed,
            Err(e) => {
                // `invalid delay time: x`, as tmux's cmdq_error.
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(e));
                } else {
                    let _ = write!(write_stream, "ERROR: {}\n", e);
                    let _ = write_stream.flush();
                }
                (false, None, String::new())
            }
        };
        // Expand formats against live server state (tmux parity). Without
        // this, `run-shell "helper '#{pane_id}'"` passed the helper the
        // literal text `#{pane_id}`; with `-b` swallowing the spawn result, the
        // bind then failed completely silently. Every `#` sequence counts, not
        // only `#{`: tmux turns `##` into `#` and `#S` into the session name.
        let shell_cmd = expand_shell_command_for(tx, shell_cmd, target_failed);
        // Expand ~ to home directory + XDG fallback for plugin paths
        let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd);
        if shell_cmd.is_empty() && delay.is_some() {
            // `-d` with no command only waits (cmd-run-shell.c:125), and the
            // commands after it wait with it unless `-b` is given.
            let delay = delay.unwrap_or_default();
            if !background && persistent {
                let (done, done_rx) = mpsc::channel::<()>();
                if !pending_chain.is_empty() {
                    deferred_chains.push((done_rx, std::mem::take(&mut pending_chain)));
                }
                std::thread::spawn(move || {
                    let _done = done;
                    std::thread::sleep(delay);
                });
            } else if !background {
                std::thread::sleep(delay);
            }
        } else if shell_cmd.is_empty() {
            if !persistent && !parse_failed {
                let _ = write!(write_stream, "usage: run-shell [-b] [-d delay] shell-command\n");
                let _ = write_stream.flush();
            }
        } else {
            if background {
                let mut c = crate::commands::build_run_shell_command(&shell_cmd);
                crate::commands::null_job_output(&mut c);
                if let Some(delay) = delay {
                    // `-b -d N`: start it after the delay, without waiting
                    // for either (cmd-run-shell.c:178).
                    let inner = tx.inner.clone();
                    let target = tx.target.clone();
                    let shown = shell_cmd.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(delay);
                        if let Err(e) = c.spawn() {
                            let tx = TargetedSender::new(&inner, target);
                            let _ = tx.send(CtrlReq::StatusMessage(format!("run-shell: {}: {}", shown, e)));
                        }
                    });
                }
                // `-b` means "don't wait for it", not "don't tell me it never
                // started". Swallowing this made a broken background bind
                // indistinguishable from an unbound key.
                else if let Err(e) = c.spawn() {
                    // No trailing newline in the message itself: StatusMessage is
                    // stored verbatim in app.status_message and rendered in the
                    // status bar, where a stray newline corrupts the line. The
                    // newline belongs only on the stream write.
                    let err_msg = format!("run-shell: {}: {}", shell_cmd, e);
                    if persistent {
                        let _ = tx.send(CtrlReq::StatusMessage(err_msg));
                    } else {
                        let _ = write!(write_stream, "{}\n", err_msg);
                        let _ = write_stream.flush();
                    }
                }
            } else if persistent {
                // Do NOT run the shell on this thread. It is the reader for an
                // attached client, and a prefix binding arrives as two lines:
                // the command, then the client's `prefix-end`. Blocking on
                // c.output() here withholds that prefix-end - so #{client_prefix}
                // (the PREFIX status indicator) stays on - and every keystroke
                // behind it until the command exits. Run it on a thread and post
                // the result back through the server loop, the way commands.rs
                // already does for the in-process path.
                let mut c = crate::commands::build_run_shell_command(&shell_cmd);
                // The thread needs an owned sender + target: `tx` here borrows
                // the per-command TargetedSender, which does not outlive the
                // command loop.
                let inner = tx.inner.clone();
                let target = tx.target.clone();
                // Whatever follows in this command list waits for the shell,
                // as in tmux. `done` is dropped when the thread ends, which
                // is what releases it at the top of the loop.
                let (done, done_rx) = mpsc::channel::<()>();
                if !pending_chain.is_empty() {
                    deferred_chains.push((done_rx, std::mem::take(&mut pending_chain)));
                }
                std::thread::spawn(move || {
                    let _done = done;
                    if let Some(delay) = delay {
                        std::thread::sleep(delay);
                    }
                    let tx = TargetedSender::new(&inner, target);
                    match c.output() {
                        Ok(out) => {
                            let text = run_shell_output_text(&out);
                            if !text.is_empty() {
                                let _ = tx.send(CtrlReq::ShowTextPopup("run-shell".to_string(), text));
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(CtrlReq::StatusMessage(format!("run-shell: {}", e)));
                        }
                    }
                });
            } else {
                // A one-shot connection (the CLI, a source job) has a thread
                // of its own, so the wait happens here and the reply, which
                // ends the command, comes after the delay and the shell.
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
                let mut c = crate::commands::build_run_shell_command(&shell_cmd);
                match c.output() {
                    Ok(out) => {
                        let text = run_shell_output_text(&out);
                        if !text.is_empty() {
                            let _ = write!(write_stream, "{}", text);
                            let _ = write_stream.flush();
                        }
                    }
                    Err(e) => {
                        let _ = write!(write_stream, "run-shell: {}\n", e);
                        let _ = write_stream.flush();
                    }
                }
            }
        }
    }
    "if-shell" | "if" => {
        let (format_mode, positional) = if_shell_parts(&args);
        if positional.len() >= 2 {
            let condition = &positional[0];
            let true_cmd = &positional[1];
            let false_cmd = positional.get(2);
            // An attached client's own if-shell -F is about that client.
            let client = persistent.then_some(crate::types::ClientSel::Id(client_id));
            let success = if_shell_condition(tx, condition, format_mode, target_failed, client);
            let cmd_to_run = if success { Some(true_cmd) } else { false_cmd };
            if let Some(chosen) = cmd_to_run {
                if !pending_chain.is_empty() {
                    // Inside a list the branch goes to the FRONT of what is
                    // queued, as tmux inserts it right after the if-shell item
                    // (cmd-if-shell.c:100 and :169, cmdq_insert_after). Put in
                    // `line`, the top of the loop took the next queued command
                    // over it, so `if-shell C 'a ; b' ; c` ran only c (#734
                    // follow up).
                    pending_chain.insert(0, chosen.to_string());
                    line.clear();
                    continue;
                }
                // Feed the chosen command back into the line buffer so the
                // main dispatch loop processes it as a regular command.
                line.clear();
                line.push_str(chosen);
                line.push('\n');
                continue;  // re-enter the dispatch loop with the new command
            }
        }
    }
    "list-sessions" | "ls" => {
        let fmt = extract_flag_value(&args, "-F");
        if let Some(fmt_str) = fmt {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt_str, None, false, None, None, Default::default()));
            if let Ok(text) = rrx.recv() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("list-sessions".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                }
            }
        } else {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::SessionInfo(rtx));
            if let Ok(text) = rrx.recv() {
                if persistent {
                    let _ = tx.send(CtrlReq::ShowTextPopup("list-sessions".to_string(), text));
                } else {
                    let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
                }
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "new-session" | "new" => {
        // new-session -t target: set session group on this server
        if let Some(target) = args.windows(2).find(|w| w[0] == "-t").map(|w| w[1].to_string()) {
            let _ = tx.send(CtrlReq::SetSessionGroup(target));
        } else {
            // Issue #200: spawn a new session from inside a running session.
            // Parse flags
            let mut sess_name: Option<String> = None;
            let mut detached = false;
            let mut window_name: Option<String> = None;
            let mut start_dir: Option<String> = None;
            let mut init_width: Option<String> = None;
            let mut init_height: Option<String> = None;
            let mut env_vars: Vec<(String, String)> = Vec::new();
            let mut env_parse_err: Option<String> = None;
            let mut initial_command: Option<String> = None;
            {
                let mut i = 0;
                while i < args.len() {
                    match args[i] {
                        "-s" => { i += 1; if i < args.len() { sess_name = Some(args[i].trim_matches('"').to_string()); } }
                        "-n" => { i += 1; if i < args.len() { window_name = Some(args[i].trim_matches('"').to_string()); } }
                        "-c" => { i += 1; if i < args.len() { start_dir = Some(args[i].trim_matches('"').to_string()); } }
                        "-x" => { i += 1; if i < args.len() { init_width = Some(args[i].to_string()); } }
                        "-y" => { i += 1; if i < args.len() { init_height = Some(args[i].to_string()); } }
                        "-e" => {
                            i += 1;
                            match crate::util::parse_new_session_e_value_token(args.get(i).copied()) {
                                Ok(p) => env_vars.push(p),
                                Err(e) => {
                                    env_parse_err = Some(e);
                                    break;
                                }
                            }
                        }
                        "-d" => { detached = true; }
                        "-t" => { i += 1; /* already handled above */ }
                        "-F" | "-f" => { i += 1; /* skip value */ }
                        other => {
                            // Positional arg: initial shell command (issue #229)
                            if !other.starts_with('-') {
                                initial_command = Some(args[i..].iter().map(|s| s.trim_matches('"').to_string()).collect::<Vec<_>>().join(" "));
                                break;
                            }
                        }
                    }
                    i += 1;
                }
            }

            if let Some(ref err) = env_parse_err {
                let msg = format!("psmux: {}\n", err);
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(msg.trim().to_string()));
                } else {
                    let _ = write!(write_stream, "{}", msg);
                    let _ = write_stream.flush();
                }
                if !persistent { break; }
            } else if detached && !persistent {
                // A one-shot detached new-session (typically the chosen branch
                // of a CLI `if-shell`, which forwards it here) runs the real
                // CLI new-session in this server's namespace, so it gets
                // everything the CLI does: the namespace prefix (this handler
                // used to create the session in the DEFAULT namespace whatever
                // server it reached), the warm claim, a held server's claim
                // (#734: the session lands in the empty server the caller
                // already identified), and the `-P`/`-F` report.
                //
                // The rest of its command list runs after it (#734 follow
                // up): this arm used to `break` unconditionally, so in
                // `new-session -d -P ; display-message -p CREATED` the marker
                // never ran. tmux runs every command of a list in order and
                // stops only at one that fails (cmd-queue.c cmdq_next and
                // cmdq_remove_group); a failure here is an `ERROR:` reply,
                // which clears the list at the top of the loop.
                let out = run_cli_new_session(&args);
                let out = one_shot_new_session_reply(out, in_command_list);
                if !out.is_empty() {
                    let _ = write!(write_stream, "{}", out);
                    let _ = write_stream.flush();
                }
                if pending_chain.is_empty() { break; }
            } else {

            let ns = crate::server::server_namespace();
            let name = sess_name.unwrap_or_else(|| crate::session::next_session_name(ns.as_deref()));

            // The session belongs to this server's namespace (#734 follow
            // up): the bare name used to put it in the default namespace.
            // It is the base the new server writes, so a name `ns__x` under
            // -L ns is `ns__ns__x` (discussion #844).
            let port_file_base = crate::session::session_name_base(ns.as_deref(), &name);

            let port_path = crate::paths::port_file(&port_file_base);

            let reserved = crate::session::reserved_session_name_error(ns.as_deref(), &name);

            // Check if session already exists
            let already_exists = if std::path::Path::new(&port_path).exists() {
                if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
                        std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(100)).is_ok()
                    } else { false }
                } else { false }
            } else { false };

            if let Some(msg) = reserved {
                // Discussion #844: refused like a duplicate, nothing is made.
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(msg));
                } else {
                    let _ = write!(write_stream, "{}\n", msg);
                    let _ = write_stream.flush();
                    break;
                }
            } else if already_exists {
                // tmux's wording (cmd-new-session.c:138), the same string the CLI
                // path has printed since discussion #210; gastown greps for it.
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(format!("duplicate session: {}", name)));
                } else {
                    let _ = write!(write_stream, "duplicate session: {}\n", name);
                    let _ = write_stream.flush();
                    break;
                }
            } else if let Err(e) = crate::server::shared::check_server_start() {
                // #798: the switch says shared (or is bad); start nothing here.
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(e));
                } else {
                    let _ = writeln!(write_stream, "ERROR: {}", e);
                    let _ = write_stream.flush();
                    break;
                }
            } else {
                // Spawn new server
                let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
                let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), name.clone()];
                if let Some(ref n) = ns {
                    server_args.push("-L".into());
                    server_args.push(n.clone());
                }

                if let Some(ref dir) = start_dir {
                    server_args.push("-d".into());
                    server_args.push(dir.clone());
                }
                if let Some(ref wn) = window_name {
                    server_args.push("-n".into());
                    server_args.push(wn.clone());
                }
                // Pass initial command to server (issue #229)
                if let Some(ref cmd) = initial_command {
                    server_args.push("-c".into());
                    server_args.push(cmd.clone());
                }
                // Pass -x/-y initial dimensions to server
                if let Some(ref w) = init_width {
                    server_args.push("-x".into());
                    server_args.push(w.clone());
                }
                if let Some(ref h) = init_height {
                    server_args.push("-y".into());
                    server_args.push(h.clone());
                }
                // Pass -e environment variables to server
                for (k, v) in &env_vars {
                    server_args.push("-e".into());
                    server_args.push(format!("{}={}", k, v));
                }
                #[cfg(windows)]
                { let _ = crate::platform::spawn_server_hidden(&exe, &server_args); }
                #[cfg(not(windows))]
                {
                    let mut cmd_proc = std::process::Command::new(&exe);
                    for a in &server_args { cmd_proc.arg(a); }
                    cmd_proc.stdin(std::process::Stdio::null());
                    cmd_proc.stdout(std::process::Stdio::null());
                    cmd_proc.stderr(std::process::Stdio::null());
                    let _ = cmd_proc.spawn();
                }

                // Wait for port file
                for _ in 0..500 {
                    if std::path::Path::new(&port_path).exists() { break; }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }

                if std::path::Path::new(&port_path).exists() {
                    if !detached {
                        // In-process follow-up to a new-session: nobody is waiting
                        // on a switch result here, the reply for this command has
                        // already been decided below.
                        let _ = tx.send(CtrlReq::SwitchClient(name.clone(), 't', None));
                    }
                    if persistent {
                        let _ = tx.send(CtrlReq::StatusMessage(format!("created session '{}'", name)));
                    } else {
                        let _ = write!(write_stream, "OK\n");
                        let _ = write_stream.flush();
                    }
                } else {
                    if persistent {
                        let _ = tx.send(CtrlReq::StatusMessage(format!("failed to create session '{}'", name)));
                    } else {
                        let _ = write!(write_stream, "failed to create session '{}'\n", name);
                        let _ = write_stream.flush();
                    }
                }
            }
            } // env_parse_err else
        }
    }
    "list-commands" | "lscm" => {
        let cmds = TMUX_COMMANDS.join("\n");
        if persistent {
            let _ = tx.send(CtrlReq::ShowTextPopup("list-commands".to_string(), cmds));
        } else {
            let _ = write!(write_stream, "{}\n", cmds);
            let _ = write_stream.flush();
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "server-info" | "info" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ServerInfo(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("server-info".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}\n", text); let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "start-server" => {
        // Server is already running if we're here, no-op
        if !persistent && pending_chain.is_empty() { break; }
    }
    "send-prefix" => {
        let _ = tx.send(CtrlReq::SendPrefix);
    }
    "previous-layout" | "prevl" => {
        let _ = tx.send(CtrlReq::PrevLayout);
    }
    "resize-window" | "resizew" => {
        match crate::resize_window::parse_resize_window(&args, raw_target.as_deref()) {
            Ok(request) => {
                let (resize_tx, resize_rx) = mpsc::channel();
                if tx.send(CtrlReq::ResizeWindow(request, resize_tx)).is_ok() {
                    match resize_rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            if persistent {
                                let _ = tx.send(CtrlReq::StatusMessage(error));
                            } else {
                                let _ = writeln!(write_stream, "ERROR: {}", error);
                                let _ = write_stream.flush();
                            }
                        }
                        Err(_) if !persistent => {
                            let _ = writeln!(write_stream, "ERROR: resize-window timed out");
                            let _ = write_stream.flush();
                        }
                        Err(_) => {}
                    }
                }
            }
            Err(error) => {
                if persistent {
                    let _ = tx.send(CtrlReq::StatusMessage(error));
                } else {
                    let _ = writeln!(write_stream, "ERROR: {}", error);
                    let _ = write_stream.flush();
                }
            }
        }
    }
    "respawn-window" | "respawnw" => {
        // tmux: `respawn-window [-k] [-c dir] [-t target] [shell-command]`.
        // The command operand follows the same rule as respawn-pane, so a
        // respawn-window that carries one also replaces the pane's recorded
        // `#{pane_start_command}` (#580).
        let command = args.iter().position(|a| *a == "--")
            .map(|i| {
                let tail = &args[i + 1..];
                if tail.len() > 1 {
                    format!("-- {}", requote_command_tail(tail))
                } else {
                    tail.join(" ")
                }
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| respawn_positional_command(&args));
        let workdir = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].to_string());
        let (resp_s, resp_r) = mpsc::channel();
        let env_sets = env_flag_values(&args);
        let _ = tx.send(CtrlReq::RespawnWindow(workdir, command, resp_s, env_sets));
        if let Ok(Err(e)) = resp_r.recv_timeout(Duration::from_secs(5)) {
            if !persistent {
                let _ = writeln!(write_stream, "ERROR: {}", e);
                let _ = write_stream.flush();
            }
        }
    }
    "lock-server" | "lock-session" | "lock" | "locks" => {
        // Lock is a no-op on Windows (no terminal locking concept)
        // Stub for compatibility
    }
    "focus-in" => {
        let _ = tx.send(CtrlReq::SetClientFocus(client_id, true));
        let _ = tx.send(CtrlReq::FocusIn);
    }
    "focus-out" => {
        let _ = tx.send(CtrlReq::SetClientFocus(client_id, false));
        let _ = tx.send(CtrlReq::FocusOut);
    }
    "choose-client" => {
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::ListClients(rtx));
        if let Ok(text) = rrx.recv() {
            if persistent {
                let _ = tx.send(CtrlReq::ShowTextPopup("choose-client".to_string(), text));
            } else {
                let _ = write!(write_stream, "{}", text);
                let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    "customize-mode" => {
        // tmux 3.2+ customize-mode: interactive options editor
        let _ = tx.send(CtrlReq::CustomizeMode);
    }
    "clear-prompt-history" | "clearphist" => {
        let _ = tx.send(CtrlReq::ClearPromptHistory);
    }
    "show-prompt-history" | "showphist" => {
        let _ = tx.send(CtrlReq::ShowPromptHistory(persistent));
    }
    "server-access" => {
        // Multi-user server access — not applicable to psmux
    }
    "run-command" | "runcmd" => {
        // Route command through the server-side execute_command_string path
        // (same code path as keybindings and command prompt).
        let full_cmd = args.join(" ");
        let (rtx, rrx) = mpsc::channel::<String>();
        let _ = tx.send(CtrlReq::RunCommand(full_cmd, rtx));
        if let Ok(resp) = rrx.recv_timeout(std::time::Duration::from_secs(15)) {
            if persistent {
                let _ = tx.send(CtrlReq::StatusMessage(resp));
            } else {
                let _ = write!(write_stream, "{}\n", resp);
                let _ = write_stream.flush();
            }
        }
        if !persistent && pending_chain.is_empty() { break; }
    }
    _ => {
        // A name no arm knows did nothing at all, silently, and the list
        // went on (`set @a 1 ; bogus ; set @b 2` set both at exit 0). tmux
        // refuses an unknown command ("unknown command: bogus") and runs
        // none of the list after it; the reply's ERROR ends the list here.
        if !persistent && in_command_list && !is_catalog_command(cmd) {
            let _ = writeln!(write_stream, "ERROR: unknown command: {}", cmd);
            let _ = write_stream.flush();
        }
    }
}
}
    // Process pending chained commands before reading from socket. A command
    // that answered with an error ends the list there, as tmux's cmd-queue.c
    // removes the rest of a command list after CMD_RETURN_ERROR.
    if write_stream.take_error() && !persistent {
        pending_chain.clear();
    }
    // The next command is taken at the top of the loop. Taking it here as
    // well skipped every other command of a list of three or more: this one
    // was overwritten there by the one after it.
    if !pending_chain.is_empty() {
        line.clear();
        continue;
    }
    // Try to read next command for batching (with timeout)
    line.clear();
    match r.read_line(&mut line) {
        Ok(0) => {
            // EOF - client disconnected. Logged for the same reason as the
            // batching read above, and closed for real: a client whose reader
            // ends but whose writer and stream stay alive keeps painting frames
            // while nothing can reach the server from its keyboard or mouse.
            crate::debug_log::server_log(
                "client-reader",
                &format!("client {client_id}: read EOF (attached_sent={attached_sent}), closing the connection"),
            );
            if attached_sent {
                let _ = tx.send(CtrlReq::ClientDetach(client_id));
            }
            crate::types::teardown_client_connection(client_id);
            break;
        }
        Err(e) => {
            if persistent && is_read_retry(&e) {
                line.clear(); // Clear any partial data from interrupted read
                continue; // Persistent mode - keep waiting
            }
            crate::debug_log::server_log(
                "client-reader",
                &format!("client {client_id}: read error {e:?} (attached_sent={attached_sent}), closing the connection"),
            );
            if attached_sent {
                let _ = tx.send(CtrlReq::ClientDetach(client_id));
            }
            crate::types::teardown_client_connection(client_id);
            break; // Non-persistent timeout or real error
        }
        Ok(_) => {
            // Any real request makes this client the latest one for
            // `window-size latest` (tmux tracks the most recently active
            // client).  A bare pointer sample is not user intent (#604), so
            // hovering over a pane must not steal the size from the client
            // the user is working in; neither is the client's frame poll,
            // which runs once a second while idle: two clients of different
            // sizes polling in turn resized every pane twice a second and made
            // the pane's program repaint, which read as a flicker. Nor is a
            // client reporting that its terminal LOST focus: tmux counts
            // focus-in and filters focus-out out by name, because the window
            // the user just left is the last one that should take the size.
            if !crate::client::is_bare_motion_cmd(&line)
                && !crate::client::is_client_poll_cmd(&line)
                && !crate::client::is_focus_loss_cmd(&line)
            {
                let _ = tx.send(CtrlReq::ClientActivity(client_id));
            }
        }
    }
} // end command loop
    // A one-shot connection ends with the reply and then EOF: that is the
    // whole contract a client reads to (tmux's client reads its server until
    // the peer closes, never on a timer: proc.c:82-91 hands the closed read to
    // client_dispatch, client.c:579). Close it gracefully: everything written
    // above is already in the send buffer, shutdown(Write) queues the FIN
    // behind it, and the input the client may still have in flight is read
    // off before the socket is dropped, because closesocket() on a socket
    // with unread input sends RST instead of FIN and a reset can discard the
    // reply the client has not read yet. The drain is bounded by the 10 ms
    // batching read timeout set above and by a byte cap, and costs the client
    // nothing: its FIN has already gone out.
    if !persistent {
        let _ = write_stream.flush();
        let _ = write_stream.shutdown(std::net::Shutdown::Write);
        let mut sink = [0u8; 4096];
        let mut drained = 0usize;
        while drained < 64 * 1024 {
            match io::Read::read(&mut r, &mut sink) {
                Ok(0) | Err(_) => break,
                Ok(n) => drained += n,
            }
        }
    }
}

/// Dispatch a command from a control mode client.
/// Returns true if a response was sent through `resp_tx`, false for fire-and-forget commands.
fn dispatch_control_command(
    cmd: &str,
    args: &[&str],
    tx: &TargetedSender,
    resp_tx: mpsc::Sender<String>,
    target_pane: Option<usize>,
    pane_is_id: bool,
    raw_target: Option<&str>,
    client_id: u64,
    target_failed: bool,
) -> bool {
    match cmd {
        "list-windows" | "lsw" => {
            let format_str = extract_flag_value(&args, "-F");
            let (rtx, rrx) = mpsc::channel::<String>();
            if let Some(fmt) = format_str {
                let _ = tx.send(CtrlReq::ListWindowsFormat(rtx, fmt));
            } else {
                let _ = tx.send(CtrlReq::ListWindowsTmux(rtx));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "list-panes" | "lsp" => {
            let all = args.iter().any(|a| *a == "-a");
            let session_scope = args.iter().any(|a| *a == "-s");
            let format_str = extract_flag_value(&args, "-F");
            let (rtx, rrx) = mpsc::channel::<String>();
            if all || session_scope {
                if let Some(fmt) = format_str {
                    let _ = tx.send(CtrlReq::ListAllPanesFormat(rtx, fmt));
                } else {
                    let _ = tx.send(CtrlReq::ListAllPanes(rtx));
                }
            } else {
                if let Some(fmt) = format_str {
                    let _ = tx.send(CtrlReq::ListPanesFormat(rtx, fmt));
                } else {
                    let _ = tx.send(CtrlReq::ListPanes(rtx));
                }
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "display-message" | "display" => {
            // Same parse as the one-shot route: every tmux flag is a flag.
            let parsed = match crate::cli::parse_display_message_args(args) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let _ = resp_tx.send(format!("ERROR: {}", error));
                    return true;
                }
            };
            let print_mode = parsed.print || parsed.list_all;
            let flags = crate::types::DisplayFlags {
                literal: parsed.literal && !parsed.list_all,
                ignore_keys: parsed.ignore_keys,
            };
            let fmt = if parsed.list_all {
                crate::cli::DISPLAY_ALL_VARIABLES
                    .iter()
                    .map(|name| format!("{}=#{{{}}}", name, name))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                // An empty template prints an empty line; only a missing one
                // is the default message (cmd-display-message.c, issue #813).
                parsed
                    .template()
                    .map(|t| t.trim_matches('"').to_string())
                    .unwrap_or_else(|| crate::commands::DISPLAY_MESSAGE_DEFAULT_FMT.to_string())
            };
            let verbose_template = (parsed.verbose && !parsed.list_all && !parsed.literal).then(|| fmt.clone());
            let target_pane_idx = if pane_is_id { None } else { target_pane };
            // A control client's current client is itself, and `-c` names
            // another one (issue #724).
            let client = Some(match extract_flag_value(args, "-c") {
                Some(c) => crate::types::ClientSel::Name(c),
                None => crate::types::ClientSel::Id(client_id),
            });
            let (rtx, rrx) = mpsc::channel::<String>();
            if pane_is_id {
                if let Some(pid) = target_pane {
                    let _ = tx.send(CtrlReq::DisplayMessageById(rtx, fmt, pid, !print_mode, None, client, flags));
                } else {
                    let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt, target_pane_idx, !print_mode, None, client, flags));
                }
            } else {
                let _ = tx.send(CtrlReq::DisplayMessage(rtx, fmt, target_pane_idx, !print_mode, None, client, flags));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                // Same visual encoding as the one-shot route above (#647).
                let shown = if print_mode { crate::util::visual_escape_message(&text) } else { String::new() };
                let reply = match &verbose_template {
                    Some(template) => format!("{}{}", crate::util::display_verbose_trace(template, &text), shown),
                    None if print_mode => shown,
                    None => text,
                };
                let reply = reply.trim_end_matches('\n');
                // `-p` always prints its one line, an empty one included
                // (control_write of the expanded message).
                let reply = if print_mode && reply.is_empty() { "\n" } else { reply };
                let _ = resp_tx.send(reply.to_string());
            }
            true
        }
        "new-pane" | "newp" => {
            let p = parse_new_pane_args(args);
            if let Some(msg) = p.refusal() {
                let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", msg));
                return true;
            }
            if p.print {
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::NewFloat { command: p.command, x: p.x, y: p.y, w: p.w, h: p.h, border: p.border, title: p.title, start_dir: p.start_dir, detached: p.detached, empty: p.empty, resp: Some(rtx), extra: p.extra });
                if let Ok(text) = rrx.recv_timeout(Duration::from_secs(2)) {
                    let text = new_pane_print_text(text, p.format.as_deref(), |r| { let _ = tx.send(r); });
                    let _ = resp_tx.send(text);
                }
                true
            } else {
                let _ = tx.send(CtrlReq::NewFloat { command: p.command, x: p.x, y: p.y, w: p.w, h: p.h, border: p.border, title: p.title, start_dir: p.start_dir, detached: p.detached, empty: p.empty, resp: None, extra: p.extra });
                false
            }
        }
        // tmux runs new-session from a control client like from any client
        // (cmd-new-session.c): the session is created, `-P` prints its report,
        // a clash is `duplicate session: K`, and the control client hears
        // %sessions-changed. It fell to the catch all here, so a control
        // client could not create a session at all. The real CLI new-session
        // runs in this server's namespace, the same route a forwarded one-shot
        // `new-session -d` takes. It always runs detached: a psmux control
        // client is bound to the one session server it attached to, so the
        // switch tmux makes without -d is not available to it.
        "new-session" | "new" => {
            let mut ns_args: Vec<&str> = args.to_vec();
            if !ns_args.iter().any(|a| *a == "-d") {
                ns_args.insert(0, "-d");
            }
            if let Some(t) = raw_target {
                if !ns_args.iter().any(|a| *a == "-t") {
                    ns_args.push("-t");
                    ns_args.push(t);
                }
            }
            let out = run_cli_new_session(&ns_args);
            let reply = match out.lines().find_map(|l| l.strip_prefix("ERROR: ")) {
                Some(err) => format!("\u{0001}ERR\u{0001}{}", err),
                None if out == "OK\n" => String::new(),
                None => out.trim_end_matches('\n').to_string(),
            };
            let _ = resp_tx.send(reply);
            true
        }
        "new-window" | "neww" => {
            let name = args.windows(2).find(|w| w[0] == "-n").map(|w| w[1].trim_matches('"').to_string());
            let start_dir = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
            let detached = crate::cli::has_short_flag(&args, 'd');
            let print_info = crate::cli::has_short_flag(&args, 'P');
            let format_str = extract_flag_value(&args, "-F").map(|s| s.trim_matches('"').to_string());
            let title = extract_flag_value(&args, "-T").map(|s| s.trim_matches('"').to_string());
            let empty = args.iter().any(|a| *a == "-E");
            // Skip arg if it's a flag, the value of a flag, or a flag-cluster
            // value (e.g. the format string after `-PF`).
            let mut skip: std::collections::HashSet<usize> = std::collections::HashSet::new();
            for (i, a) in args.iter().enumerate() {
                if a.starts_with('-') && !a.starts_with("--") {
                    skip.insert(i);
                    // Two-token forms: next arg is the value
                    if matches!(*a, "-n" | "-c" | "-F" | "-t" | "-x" | "-y" | "-e" | "-T") {
                        skip.insert(i + 1);
                    } else if a.len() > 2
                        && a.chars().skip(1).all(|c| c.is_ascii_alphabetic())
                        && matches!(a.chars().last(), Some('n') | Some('c') | Some('F') | Some('t') | Some('x') | Some('y') | Some('e') | Some('T'))
                    {
                        // Cluster ending in value-taking flag: -PF <value>
                        skip.insert(i + 1);
                    }
                }
            }
            let cmd_str: Option<String> = args.iter().enumerate()
                .find(|(i, _)| !skip.contains(i))
                .map(|(_, s)| s.trim_matches('"').to_string());
            // -e KEY=VALUE environment for the new pane (tmux parity, #489).
            let env_sets: Vec<(String, String)> = args.windows(2)
                .filter(|w| w[0] == "-e")
                .filter_map(|w| w[1].trim_matches('"').split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            let placement = crate::types::NewWindowPlacement::from_args(args, raw_target);
            if print_info {
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::NewWindowPrint(cmd_str, name, detached, start_dir, format_str, rtx, title, empty, env_sets, placement));
                if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                    let text = match text.strip_prefix("ERROR: ") {
                        Some(e) => format!("\u{0001}ERR\u{0001}{}", e),
                        None => text,
                    };
                    let _ = resp_tx.send(text);
                }
                true
            } else {
                let (otx, orx) = mpsc::channel::<Result<(), String>>();
                let _ = tx.send(CtrlReq::NewWindow(cmd_str, name, detached, start_dir, title, empty, env_sets, placement, Some(otx)));
                match orx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                    _ => { let _ = resp_tx.send(String::new()); }
                }
                true
            }
        }
        "split-window" | "splitw" | "split-pane" | "splitp" => {
            let kind = if crate::cli::has_short_flag(&args, 'h') {
                LayoutKind::Horizontal
            } else {
                LayoutKind::Vertical
            };
            let cmd_str = args.windows(2).find(|w| w[0] == "-c").map(|_| ()).and(None);
            let start_dir = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].trim_matches('"').to_string());
            let detached = crate::cli::has_short_flag(&args, 'd');
            let zoom_after_split = crate::cli::has_short_flag(&args, 'Z');
            let print_info = crate::cli::has_short_flag(&args, 'P');
            let format_str = extract_flag_value(&args, "-F").map(|s| s.trim_matches('"').to_string());
            let title = extract_flag_value(&args, "-T").map(|s| s.trim_matches('"').to_string());
            // -p N = percentage, -l N = cell count, -l N% = percentage (tmux semantics)
            let split_size: Option<(u16, bool)> = args.windows(2).find(|w| w[0] == "-p")
                .and_then(|w| w[1].trim_end_matches('%').parse::<u16>().ok())
                .map(|v| (v, true))
                .or_else(|| args.windows(2).find(|w| w[0] == "-l")
                    .and_then(|w| {
                        let raw = &w[1];
                        let is_pct = raw.ends_with('%');
                        raw.trim_end_matches('%').parse::<u16>().ok().map(|v| (v, is_pct))
                    }));
            // -e KEY=VALUE environment for the new pane (tmux parity, #489).
            let env_sets: Vec<(String, String)> = args.windows(2)
                .filter(|w| w[0] == "-e")
                .filter_map(|w| w[1].trim_matches('"').split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
                .collect();
            let split_size = crate::pane::SplitPlacement {
                size: split_size,
                before: crate::cli::has_short_flag(&args, 'b'),
                full: crate::cli::has_short_flag(&args, 'f'),
            };
            let (rtx, rrx) = mpsc::channel::<String>();
            if print_info {
                let _ = tx.send(CtrlReq::SplitWindowPrint(kind, cmd_str, detached, start_dir, split_size, format_str, rtx, title, env_sets, zoom_after_split));
            } else {
                let _ = tx.send(CtrlReq::SplitWindow(kind, cmd_str, detached, start_dir, split_size, rtx, title, env_sets, zoom_after_split));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "send-keys" | "send" => {
            // tmux answers an empty block once the keys are written
            // (cmd-send-keys.c:253). The -H branch of the copy this replaced
            // returned without answering, so the dropped sender came out as
            // `command timed out` / %error (issue #799); every merged
            // `send -l` took that branch too.
            match send_keys_requests(args, tx, client_id) {
                Ok(()) => { let _ = resp_tx.send(String::new()); }
                Err(e) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
            }
            true
        }
        "capture-pane" | "capturep" => {
            let start = args.windows(2).find(|w| w[0] == "-S").and_then(|w| if w[1] == "-" { Some(i32::MIN) } else { w[1].parse::<i32>().ok() });
            let end = args.windows(2).find(|w| w[0] == "-E").and_then(|w| w[1].parse::<i32>().ok());
            let styled = crate::cli::has_short_flag(&args, 'e');
            // -N keeps trailing spaces, -J joins wrapped rows (tmux parity).
            let opts = capture_opts(args);
            // -t %N target: resolved server-side by pane id across all windows.
            let capture_pane_id = if pane_is_id { target_pane } else { None };
            // -P and -a answer before the visible grid is read (#810).
            match capture_pane_special(args, tx, start, end, capture_pane_id) {
                Some(Ok(text)) => {
                    // tmux writes one line even for an empty result.
                    let _ = resp_tx.send(if text.is_empty() { "\n".to_string() } else { text });
                    return true;
                }
                Some(Err(e)) => {
                    let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e));
                    return true;
                }
                None => {}
            }
            let (rtx, rrx) = mpsc::channel::<String>();
            if styled {
                let _ = tx.send(CtrlReq::CapturePaneStyled(rtx, start, end, capture_pane_id, opts));
            } else if start.is_some() || end.is_some() {
                let _ = tx.send(CtrlReq::CapturePaneRange(rtx, start, end, capture_pane_id, opts));
            } else {
                let _ = tx.send(CtrlReq::CapturePane(rtx, capture_pane_id, opts));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(capture_print_text(text));
            }
            true
        }
        "kill-pane" | "killp" => {
            if pane_is_id {
                if let Some(pid) = target_pane {
                    let _ = tx.send(CtrlReq::KillPaneById(pid));
                }
            } else {
                let _ = tx.send(CtrlReq::KillPane);
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "kill-window" | "killw" => {
            // Resolve any -t target server-side and error on a miss instead
            // of killing the active window (same fix as the one-shot path).
            let parsed = raw_target.map(parse_target);
            let (t_win, t_win_is_id, t_name) = match parsed {
                Some(pt) => (pt.window, pt.window_is_id, pt.window_name),
                None => (None, false, None),
            };
            if t_win.is_some() || t_name.is_some() {
                let (resp_s, resp_r) = mpsc::channel();
                let _ = tx.send(CtrlReq::KillWindowTarget {
                    win: t_win,
                    win_is_id: t_win_is_id,
                    name: t_name,
                    resp: resp_s,
                });
                match resp_r.recv_timeout(Duration::from_secs(5)) {
                    Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                    _ => { let _ = resp_tx.send(String::new()); }
                }
            } else {
                let _ = tx.send(CtrlReq::KillWindow);
                let _ = resp_tx.send(String::new());
            }
            true
        }
        "link-window" | "linkw" => {
            let la = parse_link_window_args(args, raw_target);
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::LinkWindowReq {
                src: la.src, dst: la.dst, detach: la.detach,
                kill: la.kill, after: la.after, before: la.before,
                resp: resp_s,
            });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "unlink-window" | "unlinkw" => {
            let target = parse_unlink_window_target(args, raw_target);
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::UnlinkWindowReq { target, resp: resp_s });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "select-window" | "selectw" => {
            // Already handled by target focus above
            let _ = resp_tx.send(String::new());
            true
        }
        "select-pane" | "selectp" if select_pane_input_toggle(args, raw_target).is_some() => {
            // select-pane -d / -e (tmux PANE_INPUTOFF); same request as the
            // one-shot route, target carried inside it, nothing focused.
            let (off, last) = select_pane_input_toggle(args, raw_target).unwrap_or((true, false));
            let (resp_s, resp_r) = mpsc::channel();
            tx.send_unwrapped(CtrlReq::SetPaneInputOff {
                target: input_toggle_target(tx.target(), target_pane, pane_is_id),
                off,
                last,
                resp: Some(resp_s),
            });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "select-pane" | "selectp" => {
            // Handle -T title / -P style. One SetPaneAttrs request (#592):
            // under a temporary -t focus the restore fires after the first
            // non-temp request, so separate sends would mis-target.
            let title = args.windows(2).find(|w| w[0] == "-T").map(|w| w[1].trim_matches('"').to_string());
            let style = args.windows(2).find(|w| w[0] == "-P").map(|w| w[1].trim_matches('"').to_string());
            if title.is_some() || style.is_some() {
                let _ = tx.send(CtrlReq::SetPaneAttrs { title, style });
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "rename-window" | "renamew" => {
            if let Some(name) = args.last() {
                let _ = tx.send(CtrlReq::RenameWindow(name.trim_matches('"').to_string()));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "rename-session" | "rename" => {
            if let Some(name) = args.last() {
                let _ = tx.send(CtrlReq::RenameSession(name.trim_matches('"').to_string()));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "set-option" | "set" | "set-window-option" | "setw" => {
            let parsed_set = parse_set_option_args(args);
            let window_command = matches!(cmd, "set-window-option" | "setw");
            if let Err(error) = parsed_set.validate(window_command) {
                let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", error));
                return true;
            }
            let crate::cli::ParsedSetOptionArgs {
                flag_chars,
                positionals: positional,
                target: set_target,
                ..
            } = parsed_set;
            let quiet = flag_chars.contains('q');
            // -U is an unset alias (tmux parity, #553) — same fix as the
            // one-shot handler above.
            let unset = flag_chars.contains('u') || flag_chars.contains('U');
            let append = flag_chars.contains('a');
            // -s resolves to the same single option store as -g.
            let global = flag_chars.contains('g') || flag_chars.contains('s');
            let only_if_unset = flag_chars.contains('o');
            // `-p` is a bare pane-scope flag (#580), like `-w`: it never
            // consumes the next argument. Only -t carries a value here.
            let pane_scope = flag_chars.contains('p');
            // `-F` expands the value as a format before it is stored.
            let format_expand = flag_chars.contains('F');
            if pane_scope {
                let raw = pane_scope_target(
                    extract_flag_value(&args, "-t")
                        .map(|s| s.trim_matches('"').to_string())
                        .unwrap_or_default(),
                );
                let (rtx, rrx) = mpsc::channel::<String>();
                let value = if unset && !positional.is_empty() {
                    String::new()
                } else if positional.len() >= 2 {
                    let value = positional[1..].join(" ").trim_matches('"').to_string();
                    expand_set_option_value(tx, format_expand, value)
                } else {
                    let _ = resp_tx.send("ERROR: set-option -p: option and value required".to_string());
                    return true;
                };
                let _ = tx.send(CtrlReq::SetPaneOption {
                    target: raw,
                    option: positional[0].to_string(),
                    value,
                    unset,
                    append,
                    only_if_unset,
                    quiet,
                    resp: rtx,
                });
                let reply = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
                let _ = resp_tx.send(reply);
                return true;
            }
            // #648: `-w`/`setw` without `-g` writes the target window's own
            // option table. Same rule as the one-shot route: only a name the
            // catalog marks window scope, or a user option, is scoped that way.
            let window_scope = (flag_chars.contains('w') || window_command) && !global;
            // `set -gw @k` writes tmux's global window table (#795 follow up).
            let global_window_user = positional.first().is_some_and(|name| {
                crate::server::options::is_global_window_user_write(
                    flag_chars.contains('w') || window_command,
                    flag_chars.contains('g'),
                    name,
                )
            });
            if global_window_user
                || (window_scope
                    && positional
                        .first()
                        .is_some_and(|name| crate::server::options::is_window_scoped_write(name)))
            {
                let raw_target = if global_window_user {
                    crate::server::options::GLOBAL_WINDOW_TABLE.to_string()
                } else {
                    set_target
                        .map(|t| t.trim_matches('"').to_string())
                        .unwrap_or_default()
                };
                let option = positional[0].to_string();
                let value = if unset {
                    String::new()
                } else {
                    let joined = positional[1..].join(" ").trim_matches('"').to_string();
                    expand_set_option_value(tx, format_expand, joined)
                };
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::SetWindowOption {
                    target: raw_target,
                    option,
                    value,
                    unset,
                    append,
                    only_if_unset,
                    quiet,
                    resp: rtx,
                });
                let reply = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
                let _ = resp_tx.send(reply);
                return true;
            }
            // #803: without -g a session option goes to the SESSION's own
            // table, as on the one-shot route above.
            if !global
                && positional
                    .first()
                    .is_some_and(|name| crate::server::options::is_session_option(name))
            {
                let value = if unset {
                    String::new()
                } else {
                    let val = positional[1..].join(" ").trim_matches('"').to_string();
                    expand_set_option_value(tx, format_expand, val)
                };
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::SetSessionOption {
                    option: positional[0].to_string(),
                    value,
                    unset,
                    append,
                    only_if_unset,
                    quiet,
                    resp: rtx,
                });
                let reply = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
                let _ = resp_tx.send(reply);
                return true;
            }
            if unset && !positional.is_empty() {
                if positional[0] == "window-size" && !global {
                    let _ = tx.send(CtrlReq::SetWindowSize(None));
                } else {
                    let _ = tx.send(CtrlReq::SetOptionUnset(positional[0].to_string()));
                }
            } else if positional.len() >= 2 {
                let key = positional[0].to_string();
                let val = positional[1..].join(" ").trim_matches('"').to_string();
                let val = expand_set_option_value(tx, format_expand, val);
                if key == "window-size" && !global {
                    let _ = tx.send(CtrlReq::SetWindowSize(Some(val)));
                } else if append {
                    let _ = tx.send(CtrlReq::SetOptionAppend(key, val));
                } else if only_if_unset {
                    // Same as the one-shot handler above: report tmux's
                    // `already set: <name>` refusal instead of dropping the
                    // command in silence, unless -q asked for silence (#619).
                    if quiet {
                        let _ = tx.send(CtrlReq::SetOptionOnlyIfUnset(key, val, None));
                    } else {
                        let (rtx, rrx) = mpsc::channel::<String>();
                        let _ = tx.send(CtrlReq::SetOptionOnlyIfUnset(key, val, Some(rtx)));
                        let reply = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
                        let _ = resp_tx.send(reply);
                        return true;
                    }
                } else if quiet || global {
                    let _ = tx.send(CtrlReq::SetOptionQuiet(key, val, quiet));
                } else {
                    let _ = tx.send(CtrlReq::SetOption(key, val));
                }
            } else if positional.len() == 1 && !unset && !append {
                // Option name, no value (#535), see the matching arm above.
                // Boolean options toggle; anything else is an "empty value"
                // error the CLI has already reported with exit 1.
                if crate::server::options::missing_value_toggles(positional[0]) {
                    let _ = tx.send(CtrlReq::SetOptionToggle(positional[0].to_string()));
                }
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "show-options" | "show" | "show-window-options" | "showw"
        | "show-option" | "show-window-option" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let combined_has2 = |ch: char| -> bool {
                args.iter().any(|a| {
                    if *a == format!("-{}", ch) { return true; }
                    a.starts_with('-') && a.len() > 2 && a.chars().skip(1).all(|c| c.is_ascii_alphabetic()) && a.contains(ch)
                })
            };
            // Pane scope (#580): list a pane's `set-option -p` options.
            if combined_has2('p') {
                let raw_t = extract_flag_value(&args, "-t")
                    .map(|s| s.trim_matches('"').to_string())
                    .unwrap_or_default();
                let raw = pane_scope_target(raw_t.clone());
                let _ = tx.send(CtrlReq::ShowPaneOptions(raw, rtx));
                let reply = rrx.recv_timeout(Duration::from_millis(2000)).unwrap_or_default();
                // #647 (WIN-02): a named query answers with that one option,
                // and `-v` answers with the value alone, the same as every
                // other scope. Without this the attached route repeated the
                // whole pane store.
                let name = args.iter()
                    .filter(|a| !a.starts_with('-'))
                    .copied()
                    .last();
                // Same split as the one-shot route: `@name` inherits from the
                // user option store (#728).
                let inherited = |n: &str| -> Option<String> {
                    let (frtx, frrx) = mpsc::channel::<String>();
                    if n.starts_with('@') {
                        let _ = tx.send(CtrlReq::ShowOptionValue(frtx, n.to_string()));
                    } else {
                        let _ = tx.send(CtrlReq::ShowWindowOptionValue(
                            frtx,
                            n.to_string(),
                            raw_t.clone(),
                        ));
                    }
                    frrx.recv_timeout(Duration::from_millis(2000)).ok()
                        .filter(|v| !v.is_empty())
                };
                let out = match name {
                    Some(name) => crate::server::options::pane_option_query_reply(
                        &reply,
                        name,
                        combined_has2('v'),
                        combined_has2('A'),
                        combined_has2('q'),
                        inherited,
                    ),
                    // #655: `-A` adds the inherited entries here too, so the
                    // command prompt and the CLI print the same listing.
                    None => crate::server::options::render_pane_options(
                        &reply, combined_has2('A'), inherited,
                    ),
                };
                let _ = resp_tx.send(out.trim_end_matches('\n').to_string());
                return true;
            }
            let value_only = combined_has2('v');
            let window_scope2 = matches!(cmd, "show-window-options" | "showw" | "show-window-option") || combined_has2('w');
            // Server scope (#618), same narrowing as the one-shot handler above.
            let server_scope2 = combined_has2('s') && !window_scope2;
            let opt_name = crate::cli::first_operand("show-options", args).map(|s| s.to_string());
            let has_opt_name = opt_name.is_some();
            // The RAW -t target, resolved server-side (#266, #648) — same as
            // the primary handler above.
            let target_window2: String = extract_flag_value(&args, "-t")
                .map(|t| t.trim_matches('"').to_string())
                .unwrap_or_default();
            // A name no option table knows: "invalid option", silenced by
            // `-q` (cmd-show-options.c:117, #846). Same as the one-shot route.
            if let Some(name) = opt_name.as_deref().filter(|n| {
                !crate::server::option_catalog::is_valid_option_name(n)
            }) {
                let reply = if combined_has2('q') {
                    String::new()
                } else {
                    format!("\u{0001}ERR\u{0001}invalid option: {}", name)
                };
                let _ = resp_tx.send(reply);
                return true;
            }
            // A user option the chosen table does not hold is "invalid
            // option" like tmux; `-q` silences it. Same as the one-shot route.
            if let Some(name) = opt_name.as_deref().filter(|n| n.starts_with('@')) {
                let table = if !window_scope2 {
                    crate::server::options::UserOptionTable::Session
                } else if combined_has2('g') {
                    crate::server::options::UserOptionTable::GlobalWindow
                } else {
                    crate::server::options::UserOptionTable::Window { inherited: combined_has2('A') }
                };
                let (urtx, urrx) = mpsc::channel::<Option<(String, bool)>>();
                let _ = tx.send(CtrlReq::UserOptionLookup(urtx, name.to_string(), target_window2.clone(), table));
                let reply = match urrx.recv_timeout(Duration::from_millis(2000)) {
                    Ok(Some((value, inherited))) => crate::server::options::render_user_option_line(
                        name, &value, inherited, value_only,
                    ).trim_end_matches('\n').to_string(),
                    // A wait that ran out is an error, never "not set" (#832).
                    Err(_) => "\u{0001}ERR\u{0001}no response from server (timed out)".to_string(),
                    Ok(None) if combined_has2('q') => String::new(),
                    // tmux answers %error here (cmd-show-options.c), not a
                    // body under %end (issue #813).
                    Ok(None) => format!("\u{0001}ERR\u{0001}invalid option: {}", name),
                };
                let _ = resp_tx.send(reply);
                return true;
            }
            if opt_name.is_none() && server_scope2 {
                // Bare `show-options -s`: server options only (tmux parity).
                let mut text = String::new();
                for name in crate::server::option_catalog::server_option_names() {
                    let (srtx, srrx) = mpsc::channel::<String>();
                    let _ = tx.send(CtrlReq::ShowOptionValue(srtx, name.to_string()));
                    if let Ok(v) = srrx.recv_timeout(Duration::from_millis(2000)) {
                        if let Some(t) = crate::terminal_overrides::show_array_lines(name, &v, value_only) {
                            text.push_str(&t);
                        } else if value_only {
                            text.push_str(&format!("{}\n", v));
                        } else {
                            text.push_str(&format!("{} {}\n", name, v));
                        }
                    }
                }
                let _ = resp_tx.send(text);
                return true;
            }
            // Array options print one `name[i] value` line per element (#700).
            let array_name = opt_name.clone().filter(|n| !window_scope2 && n == "terminal-overrides");
            if let Some(name) = opt_name {
                // #648: `-wv <name>` used to fall into the plain
                // ShowOptionValue arm because `value_only` was tested first,
                // so the attached route answered with the GLOBAL value while
                // the one-shot route answered per window. Window scope decides
                // which store is read; `-v` only decides whether the name is
                // printed alongside the value.
                // `-wg <name>` is the global window table, not this window's
                // own store (#655).
                if window_scope2 && !combined_has2('g') {
                    let _ = tx.send(CtrlReq::ShowWindowOptionValue(rtx, name, target_window2.clone()));
                } else if window_scope2 && name.starts_with('@') {
                    // `show -gw @k`: the global window table's user option.
                    let _ = tx.send(CtrlReq::ShowWindowOptionValue(
                        rtx,
                        name,
                        crate::server::options::GLOBAL_WINDOW_TABLE.to_string(),
                    ));
                } else {
                    let _ = tx.send(CtrlReq::ShowOptionValue(rtx, name));
                }
            } else if window_scope2 {
                // Same three-way table choice as the one-shot route, so the
                // command prompt and the CLI print the same listing (#655).
                // `-v` no longer has to suppress `-A`: the marker rides on the
                // NAME and the values-only stripper below drops the whole name.
                let listing = if combined_has2('g') {
                    crate::server::options::WindowListing::Global
                } else if combined_has2('A') {
                    crate::server::options::WindowListing::LocalAndInherited
                } else {
                    crate::server::options::WindowListing::Local
                };
                let _ = tx.send(CtrlReq::ShowWindowOptionsFor(
                    rtx,
                    target_window2.clone(),
                    listing,
                ));
            } else {
                let _ = tx.send(CtrlReq::ShowOptions(rtx));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                if value_only && !has_opt_name {
                    // Strip option names, keep values only
                    let values_only: String = text.lines()
                        .filter_map(|line| {
                            let t = line.trim();
                            if t.is_empty() { return None; }
                            if let Some(pos) = t.find(' ') { Some(&t[pos + 1..]) } else { Some(t) }
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let _ = resp_tx.send(values_only);
                } else if let Some(t) = array_name
                    .as_deref()
                    .and_then(|n| crate::terminal_overrides::show_array_lines(n, &text, value_only))
                {
                    let _ = resp_tx.send(t.trim_end_matches('\n').to_string());
                } else {
                    let _ = resp_tx.send(text);
                }
            }
            true
        }
        "list-keys" | "lsk" if args.iter().any(|a| list_keys_flag(a, 'N')) => {
            let (table, all, key) = list_keys_notes_args(&args);
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListKeyNotes(rtx, table, all, key));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text.trim_end_matches('\n').to_string());
            }
            true
        }
        "list-keys" | "lsk" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListKeys(rtx));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "list-sessions" | "ls" => {
            let format_str = extract_flag_value(&args, "-F");
            let (rtx, rrx) = mpsc::channel::<String>();
            if let Some(fmt) = format_str {
                let _ = tx.send(CtrlReq::SessionInfoFormat(rtx, fmt));
            } else {
                let _ = tx.send(CtrlReq::SessionInfo(rtx));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "list-buffers" | "lsb" => {
            let format_str = extract_flag_value(&args, "-F");
            let (rtx, rrx) = mpsc::channel::<String>();
            if let Some(fmt) = format_str {
                let _ = tx.send(CtrlReq::ListBuffersFormat(rtx, fmt));
            } else {
                let _ = tx.send(CtrlReq::ListBuffers(rtx));
            }
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "show-buffer" | "showb" => {
            let buf_name: Option<String> = args.windows(2).find(|w| w[0] == "-b").map(|w| w[1].to_string());
            let text: Option<String> = if let Some(name) = buf_name {
                let (rtx, rrx) = mpsc::channel::<Option<String>>();
                // By name only (paste_get_name): `-b 1` is the buffer named 1.
                let _ = tx.send(CtrlReq::ShowNamedBuffer(rtx, name.clone()));
                match rrx.recv_timeout(Duration::from_secs(5)) {
                    Ok(Some(text)) => Some(text),
                    // A missing buffer is tmux's error (cmd-save-buffer.c:92),
                    // not a dropped reply the caller reports as a timeout.
                    Ok(None) => {
                        let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}no buffer {}", name));
                        return true;
                    }
                    Err(_) => None,
                }
            } else {
                let (rtx, rrx) = mpsc::channel::<String>();
                let _ = tx.send(CtrlReq::ShowBuffer(rtx));
                rrx.recv_timeout(Duration::from_secs(5)).ok()
            };
            if let Some(text) = text {
                let _ = resp_tx.send(text);
            }
            true
        }
        "has-session" | "has" => {
            let (rtx, rrx) = mpsc::channel::<bool>();
            let _ = tx.send(CtrlReq::HasSession(rtx));
            if let Ok(exists) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(if exists { String::new() } else { "session not found".to_string() });
            }
            true
        }
        "list-clients" | "lsc" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(list_clients_request(args, rtx));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "detach-client" | "detach" => {
            // Sent by a CONTROL client: `client_id` is that client. tmux
            // (cmd-detach-client.c): -s detaches every client of the target
            // session, -a every client but the sender, -t the named client,
            // and no flag the sender itself. This arm used to detach every
            // client of the session for no flag and for `-t <self>`, because
            // the outer `-t` had already been peeled off into `raw_target`
            // (#825).
            let kill_parent = args.iter().any(|a| *a == "-P");
            let detach_all_others = args.iter().any(|a| *a == "-a");
            let detach_session = args.windows(2).any(|w| w[0] == "-s");
            let target_str: Option<String> = raw_target
                .map(|s| s.to_string())
                .or_else(|| extract_flag_value(&args, "-t").map(|s| s.to_string()));

            if detach_session {
                let _ = tx.send(CtrlReq::DetachAllClients(kill_parent));
            } else if detach_all_others {
                let _ = tx.send(CtrlReq::DetachAllOtherClients(client_id, kill_parent));
            } else if let Some(t) = target_str {
                // `%N` and `N` name the client listed as /dev/pts/N, as on
                // the attached route.
                let tty = match t.trim_start_matches('%').parse::<u64>() {
                    Ok(n) => format!("/dev/pts/{}", n),
                    Err(_) => t,
                };
                let _ = tx.send(CtrlReq::ForceDetachClientByTty(tty, kill_parent));
            } else {
                let _ = tx.send(CtrlReq::ForceDetachClient(client_id));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "kill-session" => {
            let _ = tx.send(CtrlReq::KillSession);
            let _ = resp_tx.send(String::new());
            true
        }
        "kill-server" => {
            let _ = tx.send(CtrlReq::KillServer);
            // Deliberately NOT answered here (#686). The caller
            // (`session::kill_servers_in_scope`) reads this socket until EOF
            // because "EOF means the server is gone", then force-kills the pid
            // 50ms later. Answering straight away closes the socket while the
            // shutdown has barely started, so the force-kill lands in the
            // middle of it and every pane shell and pool spare it had not
            // reached yet is orphaned. The server's own exit is what closes
            // this socket, which is the EOF the caller actually wants. The
            // sleep is the wedged-server fallback: if the shutdown never
            // happens, answer late rather than never.
            std::thread::sleep(crate::warm_pane_sync::KILL_SERVER_HOLD);
            let _ = resp_tx.send(String::new());
            true
        }
        "select-layout" | "selectl" => {
            if let Some(layout) = args.first() {
                let _ = tx.send(CtrlReq::SelectLayout(layout.to_string()));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "next-layout" | "nextl" => {
            let _ = tx.send(CtrlReq::NextLayout);
            let _ = resp_tx.send(String::new());
            true
        }
        "resize-pane" | "resizep" => {
            if args.iter().any(|a| *a == "-Z") {
                let _ = tx.send(CtrlReq::ZoomPane);
            } else if let Some(xval) = args.windows(2).find(|w| w[0] == "-x").map(|w| w[1]) {
                if let Some(pct) = xval.strip_suffix('%').and_then(|n| n.parse::<u8>().ok()) {
                    let _ = tx.send(CtrlReq::ResizePanePercent("x".to_string(), pct));
                } else if let Ok(abs) = xval.parse::<u16>() {
                    let _ = tx.send(CtrlReq::ResizePaneAbsolute("x".to_string(), abs));
                }
            } else if let Some(yval) = args.windows(2).find(|w| w[0] == "-y").map(|w| w[1]) {
                if let Some(pct) = yval.strip_suffix('%').and_then(|n| n.parse::<u8>().ok()) {
                    let _ = tx.send(CtrlReq::ResizePanePercent("y".to_string(), pct));
                } else if let Ok(abs) = yval.parse::<u16>() {
                    let _ = tx.send(CtrlReq::ResizePaneAbsolute("y".to_string(), abs));
                }
            } else {
                let amount = args.iter().filter(|a| !a.starts_with('-')).next()
                    .and_then(|s| s.parse::<u16>().ok()).unwrap_or(1);
                let dir = if args.iter().any(|a| *a == "-U") { "U" }
                    else if args.iter().any(|a| *a == "-D") { "D" }
                    else if args.iter().any(|a| *a == "-L") { "L" }
                    else if args.iter().any(|a| *a == "-R") { "R" }
                    else { "D" };
                let _ = tx.send(CtrlReq::ResizePane(dir.to_string(), amount));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "swap-pane" | "swapp" => {
            // A `{...}` position token (e.g. {top-right}) resolves to whatever
            // pane currently sits there — layout-independent.  Otherwise resolve
            // an inline `-t <target>` / pre-parsed control target; else directional.
            // -s <src> -t <dst>: swap two explicit panes (#442). Only when both
            // resolve to a concrete pane; otherwise fall through.
            let detach = args.iter().any(|a| *a == "-d");
            let raw_s = args.iter().position(|a| *a == "-s")
                .and_then(|i| args.get(i + 1).copied());
            let raw_t = args.iter().position(|a| *a == "-t")
                .and_then(|i| args.get(i + 1).copied())
                .or(raw_target);
            // Both halves are resolved session wide by the server, so either
            // may name a pane in another window (#689).
            let names_pane = |s: &str| {
                let pt = parse_target(s);
                pt.pane.is_some() || pt.window.is_some() || pt.window_name.is_some()
            };
            if let Some(t) = raw_t.filter(|t| !t.starts_with('{') && (names_pane(t) || raw_s.is_some())) {
                let (sw_s, sw_r) = mpsc::channel();
                let _ = tx.send(CtrlReq::SwapPaneSrcDst {
                    src: raw_s.map(|s| s.to_string()),
                    dst: t.to_string(),
                    detach,
                    resp: sw_s,
                });
                match sw_r.recv_timeout(Duration::from_secs(5)) {
                    Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                    _ => { let _ = resp_tx.send(String::new()); }
                }
                return true;
            } else if let Some(tok) = raw_t.filter(|t| t.starts_with('{')) {
                let _ = tx.send(CtrlReq::SwapPanePosition(tok.to_string()));
            } else if let Some(p) = target_pane {
                let (sw_s, sw_r) = mpsc::channel();
                let dst = if pane_is_id { format!("%{}", p) } else { format!(".{}", p) };
                let _ = tx.send(CtrlReq::SwapPaneSrcDst { src: None, dst, detach, resp: sw_s });
                match sw_r.recv_timeout(Duration::from_secs(5)) {
                    Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                    _ => { let _ = resp_tx.send(String::new()); }
                }
                return true;
            } else {
                let direction = if args.iter().any(|a| *a == "-U") { "U".to_string() }
                               else if args.iter().any(|a| *a == "-L") { "L".to_string() }
                               else if args.iter().any(|a| *a == "-R") { "R".to_string() }
                               else { "D".to_string() };
                let _ = tx.send(CtrlReq::SwapPane(direction));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "bind-key" | "bind" => {
            // Parse bind-key's own flags, then treat everything after
            // the key name as the verbatim command (preserving flags like -c).
            let mut table_name = "prefix".to_string();
            let mut repeat = false;
            let mut note: Option<String> = None;
            let mut i = 0;
            while i < args.len() {
                match args[i] {
                    "-T" if i + 1 < args.len() => {
                        table_name = args[i + 1].to_string();
                        i += 2; continue;
                    }
                    "-N" if i + 1 < args.len() => {
                        note = Some(args[i + 1].to_string());
                        i += 2; continue;
                    }
                    "-n" => { table_name = "root".to_string(); i += 1; continue; }
                    "-r" => { repeat = true; i += 1; continue; }
                    _ => break,
                }
            }
            if i < args.len() && i + 1 < args.len() {
                let key = args[i].to_string();
                let command = requote_command_tail(&args[i + 1..]);
                let _ = tx.send(CtrlReq::BindKey(table_name, key, command, repeat, note));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "unbind-key" | "unbind" => {
            if args.iter().any(|a| *a == "-a" || (a.starts_with('-') && a.contains('a'))) {
                let mut has_table = false;
                let mut table = String::new();
                for (j, a) in args.iter().enumerate() {
                    if *a == "-T" { if let Some(t) = args.get(j + 1) { table = t.to_string(); has_table = true; } }
                    if *a == "-n" { table = "root".to_string(); has_table = true; }
                }
                if has_table {
                    let _ = tx.send(CtrlReq::UnbindAllInTable(table));
                } else {
                    let _ = tx.send(CtrlReq::UnbindAll);
                }
            } else {
                // Parse -n / -T flags for table-specific individual unbind
                let mut table: Option<String> = None;
                let mut t_value_idx: Option<usize> = None;
                let mut target_session_idx: Option<usize> = None;
                for (j, a) in args.iter().enumerate() {
                    if *a == "-T" {
                        if let Some(t) = args.get(j + 1) {
                            table = Some(t.to_string());
                            t_value_idx = Some(j + 1);
                        }
                    }
                    if *a == "-n" { table = Some("root".to_string()); }
                    // -t <session> is the target flag; skip its value
                    if *a == "-t" { target_session_idx = Some(j + 1); }
                }
                // Find the key argument: first non-flag arg that isn't the -T table value
                // or the -t session target value
                let key_arg = args.iter().enumerate()
                    .filter(|(i, a)| !a.starts_with('-') && Some(*i) != t_value_idx && Some(*i) != target_session_idx)
                    .map(|(_, a)| *a)
                    .next();
                if let Some(key) = key_arg {
                    let _ = tx.send(CtrlReq::UnbindKey(key.to_string(), table));
                }
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "source-file" | "source" => {
            // The same arguments as the other routes: flags, then every path.
            // The first argument used to be the path even when it was a flag,
            // so `source-file -q f` read a file named `-q`.
            let spec = crate::config::source_spec_from_args(args);
            if spec.paths.is_empty() {
                let _ = resp_tx.send("\u{0001}ERR\u{0001}source-file: too few arguments (need at least 1)".to_string());
                return true;
            }
            if !spec.parse_only {
                let _ = tx.send(CtrlReq::SourceFile(spec.wire(), None));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "set-environment" | "setenv" => {
            // Same parser and semantics as the CLI/TCP arm (#775).
            match crate::environ::parse_set_environment(args) {
                Ok(mut parsed) => {
                    if let Some(v) = parsed.value.take() {
                        parsed.value = Some(expand_set_option_value(tx, parsed.format, v));
                    }
                    let _ = tx.send(CtrlReq::SetEnvironment(parsed));
                    let _ = resp_tx.send(String::new());
                }
                Err(e) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
            }
            true
        }
        "show-environment" | "showenv" => {
            match crate::environ::parse_show_environment(args) {
                Ok(parsed) => {
                    let (rtx, rrx) = mpsc::channel::<Result<String, String>>();
                    let _ = tx.send(CtrlReq::ShowEnvironment(parsed, rtx));
                    match rrx.recv_timeout(Duration::from_secs(5)) {
                        Ok(Ok(text)) => { let _ = resp_tx.send(text); }
                        Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                        Err(_) => {}
                    }
                }
                Err(e) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
            }
            true
        }
        "set-hook" => {
            // The one shot route's parser: `-u` unsets, and one quoted
            // command argument is stored without its quotes.
            if let Some(req) = set_hook_request(args) {
                let _ = tx.send(req);
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "show-hooks" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ShowHooks(rtx));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "server-info" | "info" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ServerInfo(rtx));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "list-commands" | "lscm" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::ListCommands(rtx));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "dump-state" | "dump" => {
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::DumpState(rtx, false, client_id));
            if let Ok(text) = rrx.recv_timeout(Duration::from_secs(5)) {
                let _ = resp_tx.send(text);
            }
            true
        }
        "zoom-pane" | "resizep -Z" => {
            let _ = tx.send(CtrlReq::ZoomPane);
            let _ = resp_tx.send(String::new());
            true
        }
        "last-window" | "last" => {
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::LastWindow { resp: Some(resp_s) });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "last-pane" | "lastp" if last_pane_input_toggle(args).is_some() => {
            let off = last_pane_input_toggle(args).unwrap_or(true);
            let (resp_s, resp_r) = mpsc::channel();
            tx.send_unwrapped(CtrlReq::SetPaneInputOff {
                target: tx.target().cloned(),
                off,
                last: true,
                resp: Some(resp_s),
            });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "last-pane" | "lastp" => {
            let (resp_s, resp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::LastPane { resp: resp_s });
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "next-window" | "next" => {
            let _ = tx.send(CtrlReq::NextWindow);
            let _ = resp_tx.send(String::new());
            true
        }
        "previous-window" | "prev" => {
            let _ = tx.send(CtrlReq::PrevWindow);
            let _ = resp_tx.send(String::new());
            true
        }
        "rotate-window" | "rotatew" => {
            // Bare `rotate-window` is -U in tmux (only -D takes the other
            // branch); testing for -U made the default rotate the wrong way.
            let upward = !args.iter().any(|a| *a == "-D");
            let _ = tx.send(CtrlReq::RotateWindow(upward));
            let _ = resp_tx.send(String::new());
            true
        }
        "break-pane" | "breakp" => {
            // Control-mode / in-TUI path: same parser, so `-d`, `-s`, `-n`,
            // `-a`, `-b`, `-P` and `-F` mean the same thing on every route.
            let (req, print) = parse_break_pane_args(args, raw_target);
            let (bp_s, bp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::BreakPaneReq { req, print, resp: bp_s });
            match bp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Ok(text)) => { let _ = resp_tx.send(text); }
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                Err(_) => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "respawn-pane" | "respawnp" => {
            let workdir = args.windows(2).find(|w| w[0] == "-c").map(|w| w[1].to_string());
            let empty = args.iter().any(|a| *a == "-E");
            let kill = args.iter().any(|a| *a == "-k") || empty;
            // Honor `-- <shell-command>` (issue #399): teammate launch delivery.
            let command = args.iter().position(|a| *a == "--")
                .map(|i| args[i + 1..].join(" "))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            // Control mode: a refused respawn must come back as %error, not as
            // a successful %end (the refusal used to kill the server outright).
            let (resp_s, resp_r) = mpsc::channel();
            let env_sets = env_flag_values(&args);
            let _ = tx.send(CtrlReq::RespawnPane(workdir, kill, command, empty, resp_s, env_sets));
            match resp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "wait-for" | "wait" => {
            let op = if args.iter().any(|a| *a == "-L") { WaitForOp::Lock }
                     else if args.iter().any(|a| *a == "-U") { WaitForOp::Unlock }
                     else if args.iter().any(|a| *a == "-S") { WaitForOp::Signal }
                     else { WaitForOp::Wait };
            if let Some(channel) = args.iter().find(|a| !a.starts_with('-')) {
                let _ = tx.send(CtrlReq::WaitFor(channel.to_string(), op));
            }
            let _ = resp_tx.send(String::new());
            true
        }
        "refresh-client" | "refresh" => {
            // %pause / %continue lines from -A, written inside this
            // command's %begin/%end block as tmux does.
            let mut body = String::new();
            // Parse -B name:what:format (subscription management)
            let mut i = 0;
            while i < args.len() {
                if args[i] == "-B" {
                    if let Some(spec) = args.get(i + 1) {
                        // Format: "name:what:format" or "name:" (remove)
                        let spec = spec.trim_matches('"');
                        if let Some(colon1) = spec.find(':') {
                            let name = spec[..colon1].to_string();
                            let rest = &spec[colon1 + 1..];
                            if rest.is_empty() {
                                // Remove subscription: "name:"
                                let _ = tx.send(CtrlReq::ControlUnsubscribe {
                                    client_id,
                                    name,
                                });
                            } else if let Some(colon2) = rest.find(':') {
                                let target = rest[..colon2].to_string();
                                let format = rest[colon2 + 1..].to_string();
                                let _ = tx.send(CtrlReq::ControlSubscribe {
                                    client_id,
                                    name,
                                    target,
                                    format,
                                });
                            }
                        }
                    }
                    i += 2;
                    continue;
                }
                // Parse -f flags (e.g. pause-after=N)
                if args[i] == "-f" {
                    if let Some(flag_val) = args.get(i + 1) {
                        let flag_val = flag_val.trim_matches('"').trim_matches('\'');
                        // A comma separated list; `!flag` clears a flag
                        // (server-client.c server_client_set_flags and
                        // server_client_control_flags).
                        for flag in flag_val.split(',') {
                            if let Some(change) = crate::control::parse_pause_after_flag(flag) {
                                let _ = tx.send(CtrlReq::ControlSetPauseAfter {
                                    client_id,
                                    pause_after_secs: change,
                                });
                            } else if flag == "ignore-size" || flag == "!ignore-size" {
                                // #825: the flag was accepted and dropped.
                                let _ = tx.send(CtrlReq::SetClientIgnoreSize(
                                    client_id,
                                    !flag.starts_with('!'),
                                ));
                            }
                        }
                    }
                    i += 2;
                    continue;
                }
                // Parse -A '%N:continue' (resume paused pane)
                if args[i] == "-A" {
                    if let Some(spec) = args.get(i + 1) {
                        let spec = spec.trim_matches('"').trim_matches('\'');
                        // Format: %N:continue or %N:pause
                        if let Some(colon) = spec.find(':') {
                            let pane_spec = &spec[..colon];
                            let action = &spec[colon + 1..];
                            let pane = pane_spec
                                .strip_prefix('%')
                                .and_then(|p| p.parse::<usize>().ok());
                            if let Some(pid) = pane {
                                let (reply, due) = mpsc::channel::<bool>();
                                let sent = if action == "continue" {
                                    tx.send(CtrlReq::ControlContinuePane {
                                        client_id,
                                        pane_id: pid,
                                        reply,
                                    }).is_ok()
                                } else if action == "pause" {
                                    tx.send(CtrlReq::ControlPausePane {
                                        client_id,
                                        pane_id: pid,
                                        reply,
                                    }).is_ok()
                                } else {
                                    false
                                };
                                if sent && due.recv_timeout(Duration::from_secs(5)).unwrap_or(false) {
                                    body.push_str(&format!("%{} %{}\n", action, pid));
                                }
                            }
                        }
                    }
                    i += 2;
                    continue;
                }
                // Parse control client viewport sizes. tmux accepts a default
                // `WxH`/`W,H`, a per-window `@id:WxH`, and `@id:` to clear it.
                if args[i] == "-C" {
                    match args.get(i + 1) {
                        Some(spec) => match crate::resize_window::parse_control_client_size(spec) {
                            Ok((window_id, size)) => {
                                let _ = tx.send(CtrlReq::ControlClientResize {
                                    client_id,
                                    window_id,
                                    size,
                                });
                            }
                            Err(error) => {
                                let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", error));
                                return true;
                            }
                        },
                        None => {
                            let _ = resp_tx.send("\u{0001}ERR\u{0001}missing size argument".to_string());
                            return true;
                        }
                    }
                    i += 2;
                    continue;
                }
                i += 1;
            }
            let _ = resp_tx.send(body);
            true
        }
        // run-shell, if-shell, swap-window and join-pane were "unknown
        // command" on this route (issue #793 follow up). Their output and
        // errors use the same %begin/%end/%error block as every command.
        "run-shell" | "run" => {
            let (background, delay, shell_cmd) = match run_shell_command_text(args) {
                Ok(parsed) => parsed,
                Err(e) => {
                    let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e));
                    return true;
                }
            };
            let shell_cmd = expand_shell_command_for(tx, shell_cmd, target_failed);
            let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd);
            if shell_cmd.is_empty() {
                // `-d` with no command only waits (cmd-run-shell.c:125).
                match delay {
                    Some(delay) => {
                        if !background {
                            std::thread::sleep(delay);
                        }
                        let _ = resp_tx.send(String::new());
                    }
                    None => {
                        let _ = resp_tx.send("\u{0001}ERR\u{0001}usage: run-shell [-b] [-d delay] shell-command".to_string());
                    }
                }
                return true;
            }
            let mut c = crate::commands::build_run_shell_command(&shell_cmd);
            if background {
                // -b does not wait; only a spawn failure is reported.
                crate::commands::null_job_output(&mut c);
                if let Some(delay) = delay {
                    // Started after the delay; the block ends now.
                    std::thread::spawn(move || {
                        std::thread::sleep(delay);
                        let _ = c.spawn();
                    });
                    let _ = resp_tx.send(String::new());
                    return true;
                }
                match c.spawn() {
                    Ok(_) => { let _ = resp_tx.send(String::new()); }
                    Err(e) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}run-shell: {}: {}", shell_cmd, e)); }
                }
            } else {
                if let Some(delay) = delay {
                    std::thread::sleep(delay);
                }
                match c.output() {
                    Ok(out) => { let _ = resp_tx.send(run_shell_output_text(&out)); }
                    Err(e) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}run-shell: {}", e)); }
                }
            }
            true
        }
        "if-shell" | "if" => {
            let (format_mode, positional) = if_shell_parts(args);
            if positional.len() < 2 {
                let _ = resp_tx.send("\u{0001}ERR\u{0001}usage: if-shell [-bF] [-t target-pane] shell-command command [command]".to_string());
                return true;
            }
            let client = Some(crate::types::ClientSel::Id(client_id));
            let success = if_shell_condition(tx, &positional[0], format_mode, target_failed, client);
            let chosen = if success { positional.get(1) } else { positional.get(2) };
            let Some(chosen) = chosen else {
                let _ = resp_tx.send(String::new());
                return true;
            };
            // tmux inserts the branch right after the if-shell item
            // (cmd-if-shell.c cmdq_insert_after), and a control client sees
            // its output inside the same guard. Each command of the branch
            // runs through this dispatcher, its output collected in order; the
            // first error ends the branch and becomes the block's %error.
            let mut body = String::new();
            for sub in crate::config::split_chained_commands_pub(chosen) {
                let toks = parse_command_line(&sub);
                let Some(name) = toks.first().map(|s| s.as_str()) else { continue };
                let sub_args: Vec<&str> = toks[1..].iter().map(|s| s.as_str()).collect();
                let sub_raw = sub_args.windows(2).find(|w| w[0] == "-t").map(|w| w[1]);
                let pt = sub_raw.map(parse_target).unwrap_or_default();
                let filtered = without_outer_target(name, &sub_args);
                // The branch's own -t is resolved like any command's: a target
                // naming nothing is its error, and a good one is what its
                // requests run against.
                let mut sub_target: Option<crate::types::TempTarget> = None;
                let mut sub_failed = false;
                if sub_raw.is_some() {
                    let mut spec = crate::types::TempTarget::from_parsed(&pt);
                    spec.session = explicit_target_session(name, sub_raw);
                    spec.pane_check_only = pt.pane.is_some() && pane_checked_not_focused(name, pt.pane_is_id);
                    match validate_target(tx.inner, spec) {
                        Ok(r) if r.win.is_some() || r.pane.is_some() => sub_target = Some(r),
                        Ok(_) => {}
                        Err(_) if target_can_fail(name) => sub_failed = true,
                        Err(e) => {
                            body.push_str(&e);
                            let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", body));
                            return true;
                        }
                    }
                }
                let sub_tx = TargetedSender::new(tx.inner, sub_target);
                let (stx, srx) = mpsc::channel::<String>();
                let answered = dispatch_control_command(
                    name, &filtered, &sub_tx, stx, pt.pane, pt.pane_is_id, sub_raw, client_id, sub_failed,
                );
                if !answered { continue; }
                let text = srx.recv_timeout(Duration::from_secs(15)).unwrap_or_default();
                if let Some(err) = text.strip_prefix("\u{0001}ERR\u{0001}") {
                    body.push_str(err);
                    let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", body));
                    return true;
                }
                body.push_str(&text);
                if !text.is_empty() && !text.ends_with('\n') { body.push('\n'); }
            }
            let _ = resp_tx.send(body.trim_end_matches('\n').to_string());
            true
        }
        "swap-window" | "swapw" => {
            let src = flag_value(args, "-s");
            // The -t was peeled off into raw_target; a bare operand is the
            // legacy destination form, as on the one shot route.
            let dst = raw_target.map(|s| s.to_string()).or_else(|| {
                args.iter().enumerate()
                    .find(|(i, a)| !a.starts_with('-') && (*i == 0 || args[*i - 1] != "-s"))
                    .map(|(_, a)| a.to_string())
            });
            let Some(dst) = dst else {
                let _ = resp_tx.send("\u{0001}ERR\u{0001}can't find window: ".to_string());
                return true;
            };
            let (sw_s, sw_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::SwapWindow { src, dst, detach: args.iter().any(|a| *a == "-d"), resp: sw_s });
            match sw_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "join-pane" | "joinp" | "move-pane" | "movep" => {
            let src_raw = flag_value(args, "-s");
            let src = src_raw.as_deref()
                .map(|sv| crate::types::TempTarget::from_parsed(&parse_target(sv)))
                .unwrap_or_default();
            let dst = raw_target
                .map(|t| crate::types::TempTarget::from_parsed(&parse_target(t)))
                .unwrap_or_default();
            let (jp_s, jp_r) = mpsc::channel();
            let _ = tx.send(CtrlReq::JoinPane {
                src_raw,
                src,
                dst,
                horizontal: args.contains(&"-h"),
                detach: args.contains(&"-d"),
                before: args.contains(&"-b"),
                resp: jp_s,
                same_window: matches!(cmd, "move-pane" | "movep"),
                size: crate::commands::join_size_arg(&args),
            });
            match jp_r.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        // set-buffer, paste-buffer and delete-buffer were "unknown command"
        // here (issue #813) although the CLI route has them; a client that
        // pastes with `set-buffer` + `paste-buffer -p` could not paste.
        "set-buffer" | "setb" => {
            match crate::commands::parse_set_buffer_args(args).and_then(|set| match set {
                Some(set) => run_set_buffer(tx, set, raw_target),
                None => Ok(()),
            }) {
                Ok(()) => { let _ = resp_tx.send(String::new()); }
                Err(err) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", err)); }
            }
            true
        }
        "paste-buffer" | "pasteb" => {
            // The one shot route's parser and executor (issue #684); `-t` was
            // peeled off into raw_target.
            let mut pb_args = crate::commands::parse_paste_buffer_args(args);
            if pb_args.target.is_none() {
                pb_args.target = raw_target.map(|s| s.to_string());
            }
            let (rtx, rrx) = mpsc::channel::<Option<String>>();
            let _ = tx.send(CtrlReq::PasteBuffer(pb_args, rtx));
            match rrx.recv_timeout(Duration::from_secs(5)) {
                Ok(Some(msg)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", msg)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "delete-buffer" | "deleteb" => {
            // tmux: "unknown buffer: NAME" for a -b that names nothing, and
            // "no buffer" with no automatic buffer (cmd-set-buffer.c).
            let (rtx, rrx) = mpsc::channel();
            let _ = tx.send(CtrlReq::DeleteBufferCmd(flag_value(args, "-b"), rtx));
            match rrx.recv_timeout(Duration::from_secs(5)) {
                Ok(Err(e)) => { let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", e)); }
                _ => { let _ = resp_tx.send(String::new()); }
            }
            true
        }
        "run-command" | "runcmd" => {
            let full_cmd = args.join(" ");
            let (rtx, rrx) = mpsc::channel::<String>();
            let _ = tx.send(CtrlReq::RunCommand(full_cmd, rtx));
            if let Ok(resp) = rrx.recv_timeout(Duration::from_secs(15)) {
                let _ = resp_tx.send(resp);
            } else {
                let _ = resp_tx.send("timeout".to_string());
            }
            true
        }
        // iTerm2 sends "phony-command" as a tmux ping/keepalive on entering
        // gateway mode (see iTerm2 TmuxController.m kickOffTmuxForRestoration).
        // Real tmux returns success with no output; we mimic that.
        "phony-command" => {
            let _ = resp_tx.send(String::new());
            true
        }
        // tmux cmd-copy-mode.c puts the target pane in copy mode whichever
        // client asks, a control client included (#827); the control client
        // hears it as %pane-mode-changed (control::sync_window_changes).
        // Same request, flags and -t routing as the one-shot route.
        "copy-mode" => {
            let _ = tx.send(CtrlReq::CopyModeCmd(crate::copy_mode::CopyModeFlags::parse(args)));
            let _ = resp_tx.send(String::new());
            true
        }
        "resize-window" | "resizew" => {
            let response = match crate::resize_window::parse_resize_window(args, raw_target) {
                Ok(request) => {
                    let (resize_tx, resize_rx) = mpsc::channel();
                    if tx.send(CtrlReq::ResizeWindow(request, resize_tx)).is_err() {
                        Err("server unavailable".to_string())
                    } else {
                        resize_rx
                            .recv_timeout(Duration::from_secs(5))
                            .unwrap_or_else(|_| Err("resize-window timed out".to_string()))
                    }
                }
                Err(error) => Err(error),
            };
            match response {
                Ok(()) => {
                    let _ = resp_tx.send(String::new());
                }
                Err(error) => {
                    let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}{}", error));
                }
            }
            true
        }
        _ => {
            // Unknown command — emit %error like tmux does, not %end.
            // The leading "\u{0001}ERR\u{0001}" sentinel tells the dispatch
            // wrapper to use format_error instead of format_end.
            let _ = resp_tx.send(format!("\u{0001}ERR\u{0001}unknown command: {}", cmd));
            true
        }
    }
}

#[cfg(test)]
#[path = "../../tests-rs/test_issue793_followup.rs"]
mod tests_issue793_followup;

#[cfg(test)]
#[path = "../../tests-rs/test_issue476_bindkey_quoting.rs"]
mod tests_issue476_bindkey_quoting;

#[cfg(test)]
#[path = "../../tests-rs/test_issue799_cc_send_keys_reply.rs"]
mod tests_issue799_cc_send_keys_reply;

#[cfg(test)]
#[path = "../../tests-rs/test_send_keys_literal_byte.rs"]
mod tests_send_keys_literal_byte;

#[cfg(test)]
#[path = "../../tests-rs/test_refresh_client_flags.rs"]
mod tests_refresh_client_flags;

#[cfg(test)]
#[path = "../../tests-rs/test_pr740_run_shell_reader.rs"]
mod tests_pr740_run_shell_reader;

#[cfg(test)]
#[path = "../../tests-rs/test_set_option_control.rs"]
mod tests_set_option_control;

#[cfg(test)]
#[path = "../../tests-rs/test_pane_border_indicator_control.rs"]
mod tests_pane_border_indicator_control;

#[cfg(test)]
#[path = "../../tests-rs/test_issue583_pane_scope_target.rs"]
mod tests_issue583_pane_scope_target;

#[cfg(test)]
#[path = "../../tests-rs/test_issue690_hook_once.rs"]
mod tests_issue690_hook_once;

#[cfg(test)]
#[path = "../../tests-rs/test_issue691_hook_table.rs"]
mod tests_issue691_hook_table;

#[cfg(test)]
#[path = "../../tests-rs/test_issue693_targets.rs"]
mod tests_issue693_targets;

#[cfg(test)]
#[path = "../../tests-rs/test_client_read_timeout.rs"]
mod tests_client_read_timeout;

#[cfg(test)]
#[path = "../../tests-rs/test_reply_stream_error.rs"]
mod tests_reply_stream_error;

#[cfg(test)]
#[path = "../../tests-rs/test_issue734_followup_new_session_reply.rs"]
mod tests_issue734_followup_new_session_reply;

#[cfg(test)]
#[path = "../../tests-rs/test_issue813_cc_buffers_cmdlist.rs"]
mod tests_issue813_cc_buffers_cmdlist;

#[cfg(test)]
#[path = "../../tests-rs/test_set_hook_quoted_command.rs"]
mod tests_set_hook_quoted_command;
