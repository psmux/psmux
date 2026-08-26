// Multi-binary crate (psmux, pmux, tmux) sharing all modules —
// suppress dead_code warnings for functions only used by a subset of binaries.
#![allow(dead_code)]

mod types;
mod platform;
mod cli;
mod session;
mod tree;
mod style;
mod border_lines;
mod copy_line_numbers;
mod floating;
mod rendering;
mod config;
mod commands;
mod pane;
mod warm_pane_sync;
mod popup;
mod clipboard;
mod copy_mode;
mod input;
mod layout;
mod window_ops;
mod util;
mod format;
mod help;
mod server;
mod preview;
mod client;
mod ssh_input;
mod debug_log;
mod control;
mod resize_window;
mod proxy_pane;
mod cross_session;
mod cross_session_server;
mod paths;
mod wsl_path;

use std::io::{self, Write, Read as _, BufRead as _, IsTerminal};
use std::time::Duration;
use std::env;

#[allow(unused_imports)]
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use crossterm::terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute};
use crossterm::cursor::{EnableBlinking, DisableBlinking};
use crossterm::event::{EnableMouseCapture, DisableMouseCapture, EnableBracketedPaste, DisableBracketedPaste};

use crate::platform::enable_virtual_terminal_processing;
use crate::cli::{print_help, print_version, print_commands};
use crate::session::{cleanup_stale_port_files, reap_orphaned_servers, read_session_key, send_control,
    send_control_with_response, resolve_default_session_name,
    force_kill_targets, confirms_identity};
use crate::rendering::apply_cursor_style;
use crate::server::run_server;
use crate::client::run_remote;

/// Boolean flags the CLI accepts for set-option / set / set-window-option /
/// setw. -t carries a value and never reaches this set; every other character
/// is refused with "unknown flag" and exit 1 (#553).
///
/// tmux 3.4 spells set-option's as "aFgopqst:uUw" (cmd-set-option.c). psmux
/// omits -F there because the CLI forwards the value verbatim and the server
/// expands it, and accepts -s on setw as well, where tmux's own
/// set-window-option table ("aFgoqt:u") has no -s: psmux shares one flag guard
/// across all four spellings, and refusing -s on just the setw alias would be a
/// second, differently shaped hard failure for the tools this change exists to
/// unbreak (#618).
pub(crate) const SET_OPTION_CLI_FLAGS: &str = "agopqstuUw";

/// Boolean flags the CLI accepts for show-options and friends. tmux 3.4 uses
/// "AgHpqst:vw" (cmd-show-options.c); psmux has no -H (hooks-only listing).
pub(crate) const SHOW_OPTIONS_CLI_FLAGS: &str = "Agpqsvw";
use crate::ssh_input::{send_mouse_enable, InputSource};

/// Convert a ratatui Color to an ANSI SGR escape sequence.
fn color_to_ansi(c: ratatui::style::Color, fg: bool) -> String {
    use ratatui::style::Color;
    let base = if fg { 30 } else { 40 };
    let bright = if fg { 90 } else { 100 };
    match c {
        Color::Reset => format!("\x1b[{}m", if fg { 39 } else { 49 }),
        Color::Black => format!("\x1b[{}m", base + 0),
        Color::Red => format!("\x1b[{}m", base + 1),
        Color::Green => format!("\x1b[{}m", base + 2),
        Color::Yellow => format!("\x1b[{}m", base + 3),
        Color::Blue => format!("\x1b[{}m", base + 4),
        Color::Magenta => format!("\x1b[{}m", base + 5),
        Color::Cyan => format!("\x1b[{}m", base + 6),
        Color::Gray => format!("\x1b[{}m", base + 7),
        Color::DarkGray => format!("\x1b[{}m", bright + 0),
        Color::LightRed => format!("\x1b[{}m", bright + 1),
        Color::LightGreen => format!("\x1b[{}m", bright + 2),
        Color::LightYellow => format!("\x1b[{}m", bright + 3),
        Color::LightBlue => format!("\x1b[{}m", bright + 4),
        Color::LightMagenta => format!("\x1b[{}m", bright + 5),
        Color::LightCyan => format!("\x1b[{}m", bright + 6),
        Color::White => format!("\x1b[{}m", bright + 7),
        Color::Rgb(r, g, b) => format!("\x1b[{};2;{};{};{}m", if fg { 38 } else { 48 }, r, g, b),
        Color::Indexed(i) => format!("\x1b[{};5;{}m", if fg { 38 } else { 48 }, i),
    }
}

/// Returns Some(true) if the window spec (id, index, or name) exists in the
/// target server, Some(false) if it definitively does not, or None if the server
/// could not be queried (in which case the caller should NOT block the command).
/// Routing uses the already-set PSMUX_TARGET_SESSION (from the global -t parse).
fn cli_window_exists(window_spec: &str) -> Option<bool> {
    // Clear PSMUX_TARGET_FULL for the query: it holds the (possibly bad) target
    // window we are validating, which would otherwise scope list-windows to that
    // nonexistent window and return nothing. We want ALL windows of the session.
    let saved_full = std::env::var("PSMUX_TARGET_FULL").ok();
    std::env::remove_var("PSMUX_TARGET_FULL");
    let resp = crate::session::send_control_with_response(
        "list-windows -F #{window_id}|#{window_index}|#{window_name}\n".to_string(),
    );
    if let Some(v) = saved_full { std::env::set_var("PSMUX_TARGET_FULL", v); }
    let resp = resp.ok()?;
    let mut any = false;
    for line in resp.lines() {
        let line = line.trim();
        if line.is_empty() || line == "OK" { continue; }
        let mut parts = line.splitn(3, '|');
        let id = parts.next().unwrap_or("").trim();
        let idx = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        // Only count lines that actually look like "@<id>|<index>|<name>".
        if id.starts_with('@') && idx.parse::<usize>().is_ok() {
            any = true;
            if id == window_spec || idx == window_spec || name == window_spec { return Some(true); }
        }
    }
    if any { Some(false) } else { None }
}

/// Returns Some(true)/Some(false) for whether a "%<id>" pane id exists anywhere in
/// the target server, or None if it could not be queried. Uses `list-panes -a`
/// because pane ids are globally unique across windows (a pane index is not, so we
/// only validate the unambiguous %id form to avoid false negatives).
fn cli_pane_id_exists(pane_id: &str) -> Option<bool> {
    let saved_full = std::env::var("PSMUX_TARGET_FULL").ok();
    std::env::remove_var("PSMUX_TARGET_FULL");
    let resp = crate::session::send_control_with_response(
        "list-panes -a -F #{pane_id}\n".to_string(),
    );
    if let Some(v) = saved_full { std::env::set_var("PSMUX_TARGET_FULL", v); }
    let resp = resp.ok()?;
    let mut any = false;
    for line in resp.lines() {
        let line = line.trim();
        if !line.starts_with('%') { continue; }
        any = true;
        if line == pane_id { return Some(true); }
    }
    if any { Some(false) } else { None }
}

/// Returns Some(true)/Some(false) for whether a pane index exists in the ACTIVE
/// window of the target session, or None if it could not be queried. Used for the
/// "<session>.<index>" target form (no explicit window), which refers to the active
/// window. PSMUX_TARGET_FULL is cleared so the query is not scoped to the (possibly
/// nonexistent) target pane.
fn cli_pane_index_exists(idx_spec: &str) -> Option<bool> {
    let saved_full = std::env::var("PSMUX_TARGET_FULL").ok();
    std::env::remove_var("PSMUX_TARGET_FULL");
    let resp = crate::session::send_control_with_response(
        "list-panes -F #{pane_index}\n".to_string(),
    );
    if let Some(v) = saved_full { std::env::set_var("PSMUX_TARGET_FULL", v); }
    let resp = resp.ok()?;
    let mut any = false;
    for line in resp.lines() {
        let line = line.trim();
        if line.parse::<usize>().is_err() { continue; }
        any = true;
        if line == idx_spec { return Some(true); }
    }
    if any { Some(false) } else { None }
}

/// Returns Some(true)/Some(false) for whether pane index `idx_spec` exists in
/// the window identified by `window_spec` (@id, index, or name), or None when
/// the server could not be queried. Uses `list-panes -a` so every row carries
/// its window identity (a bare `list-panes` is scoped to the ACTIVE window,
/// which is not necessarily the target). Issue #554: needed because focus
/// commands (select-pane/select-window) use the permanent focus path, which
/// has no reply channel — the server-side FocusTargetTemp check that covers
/// targeted commands never runs for them, so a stale numeric pane index in
/// an explicit window had no error signal at all.
fn cli_pane_index_exists_in_window(window_spec: &str, idx_spec: &str) -> Option<bool> {
    let saved_full = std::env::var("PSMUX_TARGET_FULL").ok();
    std::env::remove_var("PSMUX_TARGET_FULL");
    let resp = crate::session::send_control_with_response(
        "list-panes -a -F #{window_id}|#{window_index}|#{window_name}|#{pane_index}\n".to_string(),
    );
    if let Some(v) = saved_full { std::env::set_var("PSMUX_TARGET_FULL", v); }
    let resp = resp.ok()?;
    let mut any_row = false;
    for line in resp.lines() {
        let line = line.trim();
        if line.is_empty() || line == "OK" { continue; }
        let mut parts = line.splitn(4, '|');
        let id = parts.next().unwrap_or("").trim();
        let widx = parts.next().unwrap_or("").trim();
        let name = parts.next().unwrap_or("").trim();
        let pidx = parts.next().unwrap_or("").trim();
        // Only count lines that look like "@<id>|<index>|<name>|<pane_index>".
        if !id.starts_with('@') || widx.parse::<usize>().is_err() { continue; }
        any_row = true;
        if (id == window_spec || widx == window_spec || name == window_spec) && pidx == idx_spec {
            return Some(true);
        }
    }
    if any_row { Some(false) } else { None }
}

/// Issue #545: validate the window/pane component of the global -t target for
/// commands that require the target to exist. Exits 1 with tmux's diagnostic
/// ("can't find window: X" / "can't find pane: X") when the target
/// definitively does not resolve. Conservative by design: relative/special
/// specifiers (+, -, ^, !, $, {, *, =) are never validated, and an
/// unreachable server (None from the validators) never blocks the command.
/// The server-side FocusTargetTemp check independently prevents execution
/// against the wrong window for anything not validated here.
/// Return the sessions in this namespace that contain the given `%<id>` pane id.
///
/// psmux runs one server per session and each allocates `%N` from its own
/// counter starting at 1, so `%1` exists in EVERY session simultaneously. tmux
/// makes these ids unique per server, which is what lets an unqualified `-t %N`
/// mean one specific pane there; psmux accepts the syntax but cannot honour the
/// semantics, and routing silently resolved it against whichever session was
/// most recently used (issue #569). Enumerating the namespace is what lets the
/// caller refuse an ambiguous target instead of acting on the wrong pane.
fn cli_sessions_with_pane_id(ns: Option<&str>, pane_id: &str) -> Vec<String> {
    let mut found = Vec::new();
    for base in crate::session::list_session_names_ns(ns) {
        let port = match std::fs::read_to_string(crate::paths::port_file(&base))
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
        {
            Some(p) => p,
            None => continue,
        };
        let key = match std::fs::read_to_string(crate::paths::key_file(&base)) {
            Ok(k) => k.trim().to_string(),
            Err(_) => continue,
        };
        let addr = format!("127.0.0.1:{}", port);
        // -s scopes to the whole session rather than the active window, so a
        // pane in a background window still counts.
        if let Ok(resp) = crate::session::send_auth_cmd_response(
            &addr, &key, b"list-panes -s -F #{pane_id}\n",
        ) {
            if resp.lines().any(|l| l.trim() == pane_id) {
                found.push(base);
            }
        }
    }
    found
}

/// True for a pane component that names a pane by POSITION rather than identity:
/// `+`/`-` (with an optional repeat count), `!` for the last pane, and the
/// `{...}` symbolic forms such as `{last}`, `{next}`, `{top-left}`.
///
/// These cannot be validated by the CLI, because there is no id or index to look
/// up: only the server, holding the live layout, can say which pane `+` means.
/// Treating them as unresolvable is what makes `-t :.+` work at all. The CLI used
/// to refuse to split them off as a pane component, so `:.+` was read as a WINDOW
/// literally named ".+", and every relative pane target died in the client with
/// "can't find window: .+" without a byte reaching the server, which implements
/// them correctly.
fn is_relative_pane(p: &str) -> bool {
    if p == "!" {
        return true;
    }
    if p.starts_with('{') && p.ends_with('}') && p.len() > 2 {
        return true;
    }
    if let Some(count) = p.strip_prefix(['+', '-']) {
        return count.is_empty() || count.chars().all(|c| c.is_ascii_digit());
    }
    false
}

fn cli_validate_window_pane_target(ns: Option<&str>) {
    let full = match std::env::var("PSMUX_TARGET_FULL") {
        Ok(f) if !f.is_empty() => f,
        _ => return,
    };
    let special = |s: &str| matches!(s.chars().next(),
        Some('+') | Some('-') | Some('^') | Some('!') | Some('$') | Some('{') | Some('*') | Some('='));
    // Bare "%<id>" pane target. These ids are NOT unique across a namespace (see
    // cli_sessions_with_pane_id), so an unqualified one can be genuinely
    // unanswerable. But it is only unanswerable when NOTHING decides which
    // session is meant: if the session this command is already routed to owns
    // the id, that is the answer, and refusing there would break the ordinary
    // "grab a %id from this session and use it" idiom (issue #569 first shipped
    // an unconditional refusal, which broke exactly that and failed
    // test_named_session_parity).
    //
    // So the refusal is scoped to the case the report was actually about: the
    // routed session does NOT hold the id, several others do, and resolving
    // would fall back to picking one by recency.
    if full.starts_with('%') {
        let owners = cli_sessions_with_pane_id(ns, &full);
        let routed = std::env::var("PSMUX_TARGET_SESSION").unwrap_or_default();
        let routed_owns = !routed.is_empty() && owners.iter().any(|o| *o == routed);
        if owners.len() > 1 && !routed_owns {
            // Report the names the caller can actually type: inside a `-L`
            // namespace the registry base is "<ns>__<session>" but the user
            // addresses the session by its short name.
            let names = owners
                .iter()
                .map(|b| match ns {
                    Some(p) => b.strip_prefix(&format!("{}__", p)).unwrap_or(b).to_string(),
                    None => b.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            eprintln!(
                "psmux: ambiguous pane id {}: present in sessions {}; qualify as session:window.pane",
                full, names
            );
            std::process::exit(1);
        }
        if owners.is_empty() && cli_pane_id_exists(&full) == Some(false) {
            eprintln!("psmux: can't find pane: {}", full);
            std::process::exit(1);
        }
        return;
    }
    if let Some(ci) = full.find(':') {
        let rest = &full[ci + 1..];
        if rest.is_empty() { return; }
        // Split off a pane component only when it is unambiguous (digits, %id or
        // a relative specifier after the last dot) — window names may
        // legitimately contain dots, and a wrong split would false-error on a
        // real window.
        let (win_part, pane_part): (&str, Option<&str>) = match rest.rfind('.') {
            Some(d) => {
                let p = &rest[d + 1..];
                if !p.is_empty()
                    && (p.starts_with('%')
                        || p.chars().all(|c| c.is_ascii_digit())
                        || is_relative_pane(p))
                {
                    (&rest[..d], Some(p))
                } else {
                    (rest, None)
                }
            }
            None => (rest, None),
        };
        if !win_part.is_empty() && !special(win_part) && cli_window_exists(win_part) == Some(false) {
            eprintln!("psmux: can't find window: {}", win_part);
            std::process::exit(1);
        }
        if let Some(p) = pane_part {
            if is_relative_pane(p) {
                // Nothing to look up: a relative pane names a position in the
                // live layout, which only the server can resolve.
            } else if p.starts_with('%') {
                if cli_pane_id_exists(p) == Some(false) {
                    eprintln!("psmux: can't find pane: {}", p);
                    std::process::exit(1);
                }
            } else if !win_part.is_empty()
                && !special(win_part)
                && cli_pane_index_exists_in_window(win_part, p) == Some(false)
            {
                // Numeric pane index inside an explicit window (issue #554).
                // Targeted commands get this from the server-side
                // FocusTargetTemp check, but focus commands (select-pane)
                // take the permanent focus path with no reply channel, so
                // the CLI must resolve it for an exit-code signal.
                eprintln!("psmux: can't find pane: {}", p);
                std::process::exit(1);
            }
        }
    } else if let Some(dot) = full.rfind('.') {
        // "<session>.<index>" form (no window component): the pane index
        // refers to the active window.
        let pane_part = &full[dot + 1..];
        if pane_part.starts_with('%') {
            if cli_pane_id_exists(pane_part) == Some(false) {
                eprintln!("psmux: can't find pane: {}", pane_part);
                std::process::exit(1);
            }
        } else if !pane_part.is_empty()
            && pane_part.chars().all(|c| c.is_ascii_digit())
            && cli_pane_index_exists(pane_part) == Some(false)
        {
            eprintln!("psmux: can't find pane: {}", full);
            std::process::exit(1);
        }
    }
}

/// Probe whether a session's server is still alive and responding.
/// Returns true only if we can connect+auth (server is up); a connection
/// refusal or a missing port file means the session is gone. A *timeout*
/// returns true — conservative on purpose, so a busy server is never wrongly
/// declared dead (used by kill-session to decide when the kill has landed).
fn probe_session_alive(session_name: &str) -> bool {
    probe_session_alive_inner(session_name, true)
}

/// The same probe, but a connect failure of ANY kind counts as gone.
///
/// The lenient reading (`Err(_) => true`) is right for kill-session, which is
/// waiting for a server it just asked to die. It is wrong for the attach gate.
/// Windows does not reliably answer a connect to an unbound loopback port with
/// a prompt `WSAECONNREFUSED`: the SYN is dropped and retransmitted, so the
/// refusal can take about two seconds to surface and a 500ms probe sees a plain
/// timeout instead. Reading that as "alive" let `attach` commit to a server
/// that was not there, and the client's own connect then failed with whatever
/// the OS happened to report, printed verbatim as
/// `psmux: No connection could be made because the target machine actively
/// refused it. (os error 10061)` (issue #605).
///
/// On loopback a live server always completes the handshake (the kernel accepts
/// into the listen backlog before the app calls accept), which is the same
/// reasoning `session::probe_session_liveness` already documents, so a strict
/// reading cannot starve a healthy-but-busy server.
fn probe_session_alive_strict(session_name: &str) -> bool {
    probe_session_alive_inner(session_name, false)
}

fn probe_session_alive_inner(session_name: &str, connect_failure_means_alive: bool) -> bool {
    let path = crate::paths::port_file(session_name);
    let port = match std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()) {
        Some(p) => p,
        None => return false, // no port file → gone
    };
    let addr: std::net::SocketAddr = match format!("127.0.0.1:{}", port).parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let key = read_session_key(session_name).unwrap_or_default();
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
        Ok(mut s) => {
            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = write!(s, "AUTH {}\n", key);
            let _ = write!(s, "session-info\n");
            let _ = s.flush();
            let mut buf = [0u8; 256];
            match std::io::Read::read(&mut s, &mut buf) {
                Ok(n) if n > 0 => String::from_utf8_lossy(&buf[..n]).contains("OK"),
                // Connected but silent. Something IS listening, so the attach
                // will at worst report an auth/read failure rather than a
                // connect error; treat it as alive in both modes.
                _ => true,
            }
        }
        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => false, // gone
        Err(_) => connect_failure_means_alive,
    }
}

/// Pick the key-name positional out of a `bind-key` / `unbind-key` argument
/// list, skipping the subcommand itself and the flags that take a value.
/// Returns None when there is no positional yet (the caller then leaves the
/// usage complaint to the server, as before).
fn bind_key_name_arg(cmd_args: &[&String]) -> Option<String> {
    let mut i = 1; // cmd_args[0] is the subcommand
    while i < cmd_args.len() {
        let a = cmd_args[i].as_str();
        match a {
            // Flags that consume the next argument.
            "-T" | "-N" | "-t" => { i += 2; continue; }
            // A bare "-" is a legal key name, not a flag.
            _ if a.starts_with('-') && a.chars().count() > 1 => { i += 1; continue; }
            _ => return Some(cmd_args[i].to_string()),
        }
    }
    None
}

/// tmux refuses a key name it cannot parse: cmd-bind-key.c and cmd-unbind-key.c
/// both call `cmdq_error(item, "unknown key: %s", ...)` and exit non-zero.
/// psmux used to forward anything at all to the server, which dropped it in
/// silence -- so a Cyrillic key name (issue #616) and an outright typo were
/// indistinguishable from success. Validate before the command goes on the wire.
fn reject_unknown_key_name(cmd_args: &[&String], _verb: &str) {
    let Some(key) = bind_key_name_arg(cmd_args) else { return };
    // Names tmux accepts and psmux does not model (WheelUpPane and the rest of
    // the mouse family) are NOT typos: those lines are normal in a ported
    // config and psmux has always taken them without complaint. Failing them
    // here would be a regression, not parity.
    if crate::config::is_unmodelled_tmux_key_name(&key) {
        return;
    }
    // Either parser accepting the name is enough: the config route uses
    // parse_key_name and the server route uses parse_key_string, and they
    // recognise slightly different spellings.
    if crate::config::parse_key_name(&key).is_none()
        && crate::config::parse_key_string(&key).is_none()
    {
        eprintln!("unknown key: {}", key);
        std::process::exit(1);
    }
}

fn build_send_paste_control(cmd_args: &[&str]) -> io::Result<String> {
    let mut payload: Option<&str> = None;
    let mut i = 1;
    while i < cmd_args.len() {
        match cmd_args[i] {
            "-t" => {
                i += 1;
                if i >= cmd_args.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "send-paste -t requires a target",
                    ));
                }
            }
            value if payload.is_none() => payload = Some(value),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "send-paste requires exactly one UTF-8 base64 payload",
                ));
            }
        }
        i += 1;
    }

    let payload = payload
        .filter(|value| crate::util::base64_decode(value).is_some())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "send-paste requires exactly one UTF-8 base64 payload",
            )
        })?;
    Ok(format!("send-paste {}\n", payload))
}

fn main() {
    if let Err(e) = run_main() {
        // Print a user-friendly error message instead of Rust's Debug format
        // which shows "Error: Custom { kind: Other, error: \"...\" }"  (fixes #47)
        let msg = e.to_string();
        eprintln!("psmux: {}", msg);
        std::process::exit(1);
    }
}

fn process_command_index(args: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < args.len() {
        if matches!(args[i].as_str(), "-t" | "-L" | "-f" | "-S") && i + 1 < args.len() {
            i += 2;
        } else if matches!(args[i].as_str(), "-h" | "--help" | "-V" | "-v" | "--version") {
            return Some(i);
        } else if args[i].starts_with('-') {
            i += 1;
        } else {
            return Some(i);
        }
    }
    None
}

fn process_target_position(args: &[String], command_index: usize) -> Option<usize> {
    if let Some(position) = args[1..command_index].iter().position(|arg| arg == "-t") {
        return Some(position + 1);
    }

    let command = &args[command_index];
    let command_args = &args[command_index + 1..];
    let scan_end = crate::cli::outer_target_scan_end(command, command_args);
    command_args[..scan_end]
        .iter()
        .position(|arg| arg == "-t")
        .map(|position| command_index + 1 + position)
}

#[cfg(test)]
mod process_command_arg_tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_string()).collect()
    }

    #[test]
    fn child_target_after_separator_does_not_route_client() {
        let args = strings(&[
            "psmux",
            "new-window",
            "-t",
            "outer",
            "--",
            "program",
            "-t",
            "child",
        ]);
        let command = process_command_index(&args).unwrap();
        assert_eq!(process_target_position(&args, command), Some(2));
    }

    #[test]
    fn deferred_target_does_not_route_client() {
        let args = strings(&["psmux", "bind-key", "x", "kill-window", "-t", "child"]);
        let command = process_command_index(&args).unwrap();
        assert_eq!(process_target_position(&args, command), None);
    }

    #[test]
    fn global_target_before_command_routes_client() {
        let args = strings(&["psmux", "-t", "outer", "new-window"]);
        let command = process_command_index(&args).unwrap();
        assert_eq!(process_target_position(&args, command), Some(1));
    }
}

fn run_main() -> io::Result<()> {
    // `-L=foo` first (flag_equals), then `-Lfoo` (attached globals), then
    // command-level `-tname` (attached target): running attached passes
    // first would mis-split `-L=foo` into `-L` + `=foo`.
    let args: Vec<String> = crate::cli::normalize_attached_target_flag(
        crate::cli::normalize_attached_global_args(
            crate::cli::normalize_flag_equals(env::args().collect()),
        ),
    );
    
    // Set console code page to UTF-8 early so ALL output paths (CLI commands
    // like capture-pane, list-sessions, display-message, etc.) correctly
    // render multi-byte Unicode characters instead of mojibake.
    enable_virtual_terminal_processing();

    // Clean up any stale port files at startup
    cleanup_stale_port_files();
    // Then drop registry files whose `.port` entry is already gone (issue
    // #530). The sweep above is the only thing that can reach them, and it
    // finds entries BY their `.port` file — so a satellite that outlives its
    // port is invisible to it and accumulates forever.
    crate::session::prune_orphaned_registry_files();
    // Then reap any LIVE but orphaned server processes (issue #448): duplicates
    // or crashed-client headless servers that cleanup_stale_port_files cannot
    // see because they have no registry file. Bounds the process count so
    // orphans can't accumulate to the point of exhausting Windows desktop-heap.
    reap_orphaned_servers();

    // Parse -L flag early (tmux-compatible: names the server socket for namespace isolation)
    // In psmux, -L <name> creates a namespace prefix for session port/key files.
    // Sessions under -L "foo" are stored as "foo__sessionname.port".
    // IMPORTANT: Only recognize -L as a global flag when it appears BEFORE the subcommand.
    // This avoids conflict with subcommand flags (e.g. select-pane -L, resize-pane -L).
    let mut l_socket_name: Option<String> = None;
    let mut f_config_file: Option<String> = None;
    let mut control_mode: u8 = 0; // 0=off, 1=-C (echo), 2=-CC (no echo)
    {
        let mut i = 1; // skip binary name
        while i < args.len() {
            let arg = &args[i];
            if arg == "-CC" {
                control_mode = 2;
                i += 1;
            } else if arg == "-C" {
                control_mode = 1;
                i += 1;
            } else if arg == "-L" && i + 1 < args.len() {
                l_socket_name = Some(args[i + 1].clone());
                i += 2;
            } else if arg == "-f" && i + 1 < args.len() {
                f_config_file = Some(args[i + 1].clone());
                i += 2;
            } else if (arg == "-S" || arg == "-t") && i + 1 < args.len() {
                i += 2; // skip other global flag-value pairs
            } else if arg.starts_with('-') {
                i += 1; // skip single global flags (e.g. -v, -V)
            } else {
                break; // hit the subcommand name — stop scanning for global flags
            }
        }
    }

    // Set PSMUX_CONFIG_FILE if -f was provided, so load_config() picks it up.
    if let Some(ref cf) = f_config_file {
        env::set_var("PSMUX_CONFIG_FILE", cf);
    }

    // Parse -t flag early to set target session for all commands
    // Supports session:window.pane format (e.g., "dev:0.1")
    // PSMUX_TARGET_SESSION stores the port file base name (for port file lookup)
    // PSMUX_TARGET_FULL stores the full target (session:window.pane) for the server
    //
    // Tracks whether THIS command line supplied an explicit `-t <session>`. Only
    // an explicit session target may pin routing; anything else (no -t, a pane/
    // window-only target like `-t %2`, or switch-client) must fall through to the
    // $TMUX-based resolution below so a stale PSMUX_TARGET_SESSION inherited from
    // the pane environment (e.g. a warm-pool shell frozen at `__warm__`) can never
    // hijack the current session. See issue #485.
    let command_index = process_command_index(&args);
    let target_position = command_index.and_then(|index| process_target_position(&args, index));
    let strip_target_position = command_index
        .filter(|index| !matches!(args[*index].as_str(), "detach-client" | "detach"))
        .and(target_position);
    let mut explicit_session_target = false;
    if let Some(pos) = target_position {
        if let Some(target) = args.get(pos + 1) {
            // move-window/swap-window: a bare -t that names a WINDOW (tmux
            // target-window semantics), not a session. Coerce "N" -> ":N" so the
            // generic parser treats it as a window and routing stays on the current
            // server. Every other command keeps bare = session.
            //
            // The relative and symbolic forms need the same coercion (issue
            // #602): without it `-t -1` was read as the session named "-1" and
            // died in the CLI with "no server running on session '-1'", and
            // `-t {last}` the same way, so neither ever reached the server that
            // knows what they mean. Bare NAMES still mean a session.
            let is_window_move = args.iter().any(|a|
                a == "move-window" || a == "movew" || a == "swap-window" || a == "swapw");
            let looks_like_window = |t: &str| {
                if t.parse::<usize>().is_ok() { return true; }
                if matches!(t, "!" | "^" | "$") { return true; }
                if t.starts_with('{') && t.ends_with('}') && t.len() > 2 { return true; }
                if let Some(n) = t.strip_prefix(['+', '-']) {
                    return n.is_empty() || n.chars().all(|c| c.is_ascii_digit());
                }
                false
            };
            let target: String = if is_window_move && looks_like_window(target) {
                format!(":{}", target)
            } else {
                target.to_string()
            };
            // Extract just the session name for port file lookup
            let parsed_target = crate::cli::parse_target(&target);
            let has_explicit_session = parsed_target.session.is_some();
            let session = parsed_target.session.unwrap_or_else(|| "default".to_string());
            // Store the full target for the server to parse, with $N resolved
            // to the actual session name so the server can look up port files.
            let resolved_full = if target.starts_with('$') {
                if let Some(colon_pos) = target.find(':') {
                    format!("{}{}", session, &target[colon_pos..])
                } else {
                    session.clone()
                }
            } else {
                target.to_string()
            };
            env::set_var("PSMUX_TARGET_FULL", &resolved_full);
            // Apply -L namespace prefix for port file lookup. A `$N` id has
            // already been resolved to the on disk name, which carries the
            // prefix, so do not add it a second time: `-L ns cmd -t $N` used
            // to route to `ns__ns__name`, a server that does not exist.
            let port_file_base = match l_socket_name {
                Some(ref l) if !session.starts_with(&format!("{}__", l)) => {
                    format!("{}__{}", l, session)
                }
                _ => session.clone(),
            };
            // If the -t target includes an explicit session name, use it
            // directly. Otherwise (e.g. -t %2, -t :1.0) fall through to
            // the TMUX env var resolution below so we connect to the right
            // server when invoked from inside a psmux pane.
            //
            // Exception: for switch-client, -t is the DESTINATION session,
            // not the server to route the command to. Skip setting
            // PSMUX_TARGET_SESSION so the TMUX-based fallback below resolves
            // the current (source) session for routing. PSMUX_TARGET_FULL
            // still carries the destination for the server handler.
            let is_switch_client = args.iter().any(|a| a == "switch-client" || a == "switchc");
            // detach-client's -t is a CLIENT spec (tty or %id), not a session to
            // route to. Setting PSMUX_TARGET_SESSION from it made a bare
            // `detach-client -t /dev/pts/3` route as session '/dev/pts/3' and
            // fail with "no session" (issue #565). Route it like switch-client:
            // fall through to -s / $TMUX resolution.
            let is_detach_client = args.iter().any(|a| a == "detach-client" || a == "detach");
            if has_explicit_session && !is_switch_client && !is_detach_client {
                env::set_var("PSMUX_TARGET_SESSION", &port_file_base);
                explicit_session_target = true;
            }
        }
    }
    if !explicit_session_target {
        // No explicit `-t session` on this command line: `$TMUX` (set inside
        // every psmux pane and kept pointing at the live server) is the authority
        // for which server we belong to. It must OVERRIDE any PSMUX_TARGET_SESSION
        // inherited from the pane environment, because a warm-pool shell freezes
        // that variable at `__warm__` (it names the server the pane was born in,
        // not the session it was transplanted into). Guarding on `is_err()` here
        // let that stale value win and routed queries like `display-message -p
        // '#S'` to the wrong session (issue #485). The `-L` namespace and the
        // most-recent-session fallback are applied inside resolve_routing_target.
        let psmux_dir = std::path::PathBuf::from(crate::paths::psmux_dir());
        let tmux_env = env::var("TMUX").ok();
        if let Some(name) = crate::session::resolve_routing_target(
            l_socket_name.as_deref(),
            tmux_env.as_deref(),
            &psmux_dir,
        ) {
            env::set_var("PSMUX_TARGET_SESSION", &name);
        }
    }
    
    // Keep command tails verbatim. Only the outer target consumed for routing
    // is removed; nested commands and argv after `--` retain their own `-t`.
    let cmd_args: Vec<&String> = command_index
        .map(|index| {
            args[index..]
                .iter()
                .enumerate()
                .filter(|(offset, _)| {
                    let absolute = index + *offset;
                    strip_target_position
                        .map_or(true, |target| absolute != target && absolute != target + 1)
                })
                .map(|(_, arg)| arg)
                .collect()
        })
        .unwrap_or_default();
    
    let cmd = cmd_args.first().map(|s| s.as_str()).unwrap_or("");

    // Issue #545: commands that operate on an EXISTING window/pane must fail
    // loudly (nonzero exit + stderr, tmux parity) when the -t target does not
    // resolve, instead of the server-side temp focus silently no-opping and
    // the command running against the active window. Same validators the
    // select-window/select-pane branches already use. Commands whose -t names
    // a destination that need not exist yet (move-window, break-pane, ...) or
    // that resolve their own targets are deliberately NOT listed.
    if matches!(cmd,
        "send-keys" | "send" | "capture-pane" | "capturep"
        | "rename-window" | "renamew" | "kill-pane" | "killp"
        | "clear-history" | "clearhist" | "list-panes" | "lsp"
        | "respawn-pane" | "respawnp" | "pipe-pane" | "pipep"
        | "resize-pane" | "resizep" | "display-message" | "display"
        // Issue #554: select-pane/select-window were left off this list, so
        // any ':'-bearing target (the fully-qualified form scripts use)
        // skipped validation entirely and exited 0 on a miss. Their older
        // per-arm validators are deleted; this shared one is a superset.
        | "select-pane" | "selectp" | "select-window" | "selectw")
    {
        cli_validate_window_pane_target(l_socket_name.as_deref());
    }

    // Handle help and version flags first
    match cmd {
        "-h" | "--help" | "help" => {
            print_help();
            return Ok(());
        }
        "-V" | "-v" | "--version" | "version" => {
            print_version();
            return Ok(());
        }
        "list-commands" | "lscm" => {
            print_commands();
            return Ok(());
        }
        // Hidden internal command for empirical preview rendering tests.
        // Usage: psmux _render-preview <session> <win_id> <width> <height>
        // Fetches the window-dump and renders it via the SAME render_layout_json
        // the choose-tree/choose-session preview uses, then prints the resulting
        // buffer as ANSI text to stdout. Lets us compare REAL vs PREVIEW.
        "_render-preview" => {
            if cmd_args.len() < 5 {
                eprintln!("usage: psmux _render-preview <session> <win_id> <width> <height>");
                std::process::exit(2);
            }
            let sess = cmd_args[1].clone();
            let win_id: usize = cmd_args[2].parse().expect("win_id must be a number");
            let w: u16 = cmd_args[3].parse().expect("width must be a number");
            let h: u16 = cmd_args[4].parse().expect("height must be a number");
            let layout = match crate::preview::fetch_window_dump(&sess, win_id) {
                Some(l) => l,
                None => { eprintln!("failed to fetch window-dump for {}:@{}", sess, win_id); std::process::exit(3); }
            };
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;
            use ratatui::layout::Rect;
            use ratatui::style::Color;
            let backend = TestBackend::new(w, h);
            let mut term = Terminal::new(backend).unwrap();
            term.draw(|f| {
                let area = Rect::new(0, 0, w, h);
                let active_rect = crate::client::compute_active_rect_json(&layout, area);
                let total_panes = layout.count_leaves();
                let border_style = ratatui::style::Style::default().fg(Color::DarkGray);
                let active_border_style = ratatui::style::Style::default().fg(Color::Green);
                let border_chars = crate::border_lines::border_chars(crate::border_lines::DEFAULT);
                crate::client::render_layout_json(
                    f, &layout, area,
                    false,
                    border_style,
                    active_border_style,
                    false, Color::Reset,
                    active_rect,
                    "", false, "off", "",
                    total_panes,
                    border_chars,
                    None,
                    crate::client::WindowContentStyles::default(),
                );
                let borders = crate::client::border_geometry_from_layout(
                    &layout,
                    area,
                    f.buffer_mut().area,
                    false,
                );
                let junctions = crate::rendering::fix_border_intersections(
                    f.buffer_mut(),
                    border_chars,
                    &borders,
                );
                crate::client::recolor_border_junctions(
                    f.buffer_mut(),
                    &junctions,
                    active_rect,
                    border_style,
                    active_border_style,
                );
            }).unwrap();
            // Dump the buffer as ANSI escape sequences so colors are visible.
            let buf = term.backend().buffer().clone();
            let area = buf.area;
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for y in 0..area.height {
                let mut last_fg: Option<Color> = None;
                let mut last_bg: Option<Color> = None;
                for x in 0..area.width {
                    let cell = &buf.content[(y as usize) * (area.width as usize) + (x as usize)];
                    let fg = cell.style().fg;
                    let bg = cell.style().bg;
                    if fg != last_fg || bg != last_bg {
                        let _ = write!(out, "\x1b[0m");
                        if let Some(c) = fg { let _ = write!(out, "{}", color_to_ansi(c, true)); }
                        if let Some(c) = bg { let _ = write!(out, "{}", color_to_ansi(c, false)); }
                        last_fg = fg;
                        last_bg = bg;
                    }
                    let _ = write!(out, "{}", cell.symbol());
                }
                let _ = writeln!(out, "\x1b[0m");
            }
            return Ok(());
        }
        _ => {}
    }

    match cmd {
        // kill-server MUST be handled early before any potential fall-through
        "kill-server" => {
            let psmux_dir = crate::paths::psmux_dir();
            // Compute namespace prefix for -L filtering (matches list-sessions behavior)
            let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
            // Snapshot the force-kill candidates from this data dir's .pid files
            // BEFORE the graceful pass removes them. Scoped to this dir and (with
            // -L) this namespace, so the fallback can never reach another instance.
            let fk_targets =
                force_kill_targets(std::path::Path::new(&psmux_dir), ns_prefix.as_deref());
            let mut targets: Vec<(std::path::PathBuf, u16, String)> = Vec::new();
            let mut stale_ports: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "port").unwrap_or(false) {
                        if let Some(session_name) = path.file_stem().and_then(|s| s.to_str()) {
                            // Apply -L namespace filtering:
                            // With -L: only kill sessions under that namespace
                            // Without -L: kill ALL sessions (tmux behavior)
                            if let Some(ref pfx) = ns_prefix {
                                if !session_name.starts_with(pfx.as_str()) { continue; }
                            }
                            if let Ok(port_str) = std::fs::read_to_string(&path) {
                                if let Ok(port) = port_str.trim().parse::<u16>() {
                                    let sess_key = read_session_key(session_name).unwrap_or_default();
                                    targets.push((path.clone(), port, sess_key));
                                }
                            } else {
                                stale_ports.push(path.clone());
                            }
                        }
                    }
                }
            }
            // Send kill-server to all sessions in parallel via threads
            let handles: Vec<std::thread::JoinHandle<()>> = targets.into_iter().map(|(path, port, sess_key)| {
                std::thread::spawn(move || {
                    let addr = format!("127.0.0.1:{}", port);
                    if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
                        &addr.parse().unwrap(),
                        Duration::from_millis(500),
                    ) {
                        let _ = stream.set_nodelay(true);
                        let _ = write!(stream, "AUTH {}\n", sess_key);
                        let _ = stream.flush();
                        let _ = std::io::Write::write_all(&mut stream, b"kill-server\n");
                        let _ = stream.flush();
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        // Wait for server to exit (EOF = done)
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
                        let mut buf = [0u8; 64];
                        loop {
                            match std::io::Read::read(&mut stream, &mut buf) {
                                Ok(0) => break,
                                Err(_) => break,
                                Ok(_) => continue,
                            }
                        }
                    }
                    // Remove the whole registry set regardless. Deleting only
                    // port/key/pid used to strand the `.sid`, which no sweep can
                    // reach once its `.port` is gone (#530).
                    crate::session::remove_session_registry_files(&path);
                })
            }).collect();
            // Wait for all threads to complete
            for h in handles { let _ = h.join(); }
            // Clean up stale registry sets (whole set, including `.sid` — #530)
            for path in &stale_ports {
                crate::session::remove_session_registry_files(path);
            }
            // Force-kill any wedged server that ignored the graceful kill. The
            // candidates were read from this data dir's (and, with -L, this
            // namespace's) own .pid files before the graceful pass removed them,
            // so nothing outside this instance is ever reached. The identity gate
            // (exact process-creation-time match) skips any pid that has already
            // exited or been recycled — no machine-wide, name-based scan.
            std::thread::sleep(Duration::from_millis(50));
            for t in fk_targets {
                if confirms_identity(crate::platform::process_kill::process_creation_time(t.pid), t.creation_time) {
                    crate::platform::process_kill::terminate_server_pid(t.pid, None);
                }
            }
            return Ok(());
        }
        "ls" | "list-sessions" => {
                // Parse -F (format) and -f (filter) flags
                let mut format_str: Option<String> = None;
                let mut filter_str: Option<String> = None;
                {
                    let mut i = 1;
                    while i < cmd_args.len() {
                        match cmd_args[i].as_str() {
                            "-F" => {
                                if let Some(f) = cmd_args.get(i + 1) {
                                    format_str = Some(f.to_string());
                                    i += 1;
                                }
                            }
                            s if s.starts_with("-F") && s.len() > 2 => {
                                format_str = Some(s[2..].to_string());
                            }
                            "-f" => {
                                if let Some(f) = cmd_args.get(i + 1) {
                                    filter_str = Some(f.to_string());
                                    i += 1;
                                }
                            }
                            s if s.starts_with('-') => {
                                // Unknown flag: error like tmux rather than ignoring it,
                                // so scripts (libtmux/tmuxp) see a nonzero exit.
                                eprintln!("psmux: list-sessions: unknown option '{}'", s);
                                std::process::exit(1);
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                }
                let dir = crate::paths::psmux_dir();
                // Compute namespace prefix for -L filtering
                let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
                // Servers that answered in this namespace, counted BEFORE the
                // -f filter is applied. tmux exits 1 with `no server running`
                // when there is nothing to list at all, but a filter that
                // matches nothing on a live server is an empty listing at
                // exit 0, and the two must stay distinguishable to scripts.
                let mut live_servers: usize = 0;
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        if let Some(name) = e.file_name().to_str() {
                            if let Some((base, ext)) = name.rsplit_once('.') {
                                if ext == "port" {
                                    // Skip warm (standby) sessions — internal-only
                                    if crate::session::is_warm_session(base) { continue; }
                                    // Filter by -L namespace: when -L is given, only show
                                    // sessions with that prefix; when no -L, only show
                                    // sessions without any namespace prefix
                                    if let Some(ref pfx) = ns_prefix {
                                        if !base.starts_with(pfx.as_str()) { continue; }
                                    } else {
                                        if base.contains("__") { continue; }
                                    }
                                    // PID-anchor fast path: a dead entry would
                                    // otherwise cost a full TCP connect timeout
                                    // here (dead loopback ports can time out
                                    // rather than refuse on Windows). Reap it
                                    // and move on without touching the network.
                                    if crate::session::registry_pid_anchor_alive(base) == Some(false) {
                                        crate::session::remove_session_registry(base);
                                        continue;
                                    }
                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                        if let Ok(_p) = port_str.trim().parse::<u16>() {
                                            let addr = format!("127.0.0.1:{}", port_str.trim());
                                            let conn = std::net::TcpStream::connect_timeout(
                                                &addr.parse().unwrap(),
                                                Duration::from_millis(200)
                                            );
                                            // Only prune a session that ACTIVELY refused (truly dead);
                                            // a timeout means busy-but-alive and must not be deleted.
                                            let refused = matches!(&conn, Err(e) if e.kind() == io::ErrorKind::ConnectionRefused);
                                            if let Ok(mut s) = conn {
                                                // Format expansion for variables like
                                                // pane_current_command/pane_current_path can
                                                // take 40+ ms each (OS process queries), so
                                                // 50 ms was too tight for multi-variable
                                                // format strings (e.g. libtmux's 123-field
                                                // format). 500 ms allows complex formats
                                                // while still detecting dead sessions quickly.
                                                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                                                // Read session key and authenticate
                                                let key_path = crate::paths::key_file(&base);
                                                if let Ok(key) = std::fs::read_to_string(&key_path) {
                                                    let _ = std::io::Write::write_all(&mut s, format!("AUTH {}\n", key.trim()).as_bytes());
                                                }
                                                // Use -F format if provided, otherwise session-info
                                                let query = if let Some(ref fmt) = format_str {
                                                    format!("list-sessions -F {}\n", crate::util::quote_arg(&fmt))
                                                } else {
                                                    "session-info\n".to_string()
                                                };
                                                let _ = std::io::Write::write_all(&mut s, query.as_bytes());
                                                let mut br = std::io::BufReader::new(s);
                                                let mut line = String::new();
                                                // Skip "OK" response from AUTH
                                                let _ = br.read_line(&mut line);
                                                if line.trim() == "OK" {
                                                    line.clear();
                                                    let _ = br.read_line(&mut line);
                                                }
                                                if line.trim() == "ERROR: Authentication required" {
                                                    // Auth failed, skip this session
                                                    continue;
                                                }
                                                live_servers += 1;
                                                // When -F format is provided, the server already
                                                // expanded it; use the result even if empty (tmux
                                                // prints an empty line for unknown format vars).
                                                // Only fall back to display_name when no -F was given.
                                                if format_str.is_some() || !line.trim().is_empty() {
                                                    let output = line.trim_end().to_string();
                                                    // Apply -f filter if provided.
                                                    // tmux -f accepts format expressions; support
                                                    // the common #{==:#{session_name},NAME} pattern
                                                    // as well as a plain substring fallback.
                                                    if let Some(ref flt) = filter_str {
                                                        let passes = if let Some(target) = flt
                                                            .strip_prefix("#{==:#{session_name},")
                                                            .and_then(|s| s.strip_suffix('}'))
                                                        {
                                                            // Compare port-file display name against literal
                                                            let display_name = if let Some(ref pfx) = ns_prefix {
                                                                base.strip_prefix(pfx.as_str()).unwrap_or(base)
                                                            } else {
                                                                base
                                                            };
                                                            display_name == target
                                                        } else {
                                                            // Fallback: plain substring match
                                                            output.contains(flt.as_str())
                                                        };
                                                        if !passes { continue; }
                                                    }
                                                    println!("{}", output);
                                                } else {
                                                    // Strip namespace prefix for display (e.g. "foo__dev" -> "dev")
                                                    let display_name = if let Some(ref pfx) = ns_prefix {
                                                        base.strip_prefix(pfx.as_str()).unwrap_or(base)
                                                    } else {
                                                        base
                                                    };
                                                    if let Some(ref flt) = filter_str {
                                                        let passes = if let Some(target) = flt
                                                            .strip_prefix("#{==:#{session_name},")
                                                            .and_then(|s| s.strip_suffix('}'))
                                                        {
                                                            display_name == target
                                                        } else {
                                                            display_name.contains(flt.as_str())
                                                        };
                                                        if !passes { continue; }
                                                    }
                                                    println!("{}", display_name); 
                                                }
                                            } else if refused {
                                                // Actively refused → truly dead. Remove the WHOLE
                                                // set: dropping the `.port` alone would strand its
                                                // siblings where no sweep can reach them (#530).
                                                crate::session::remove_session_registry_files(&e.path());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if live_servers == 0 {
                    // Nothing answered: no user session in this namespace (a
                    // `__warm__` standby is not a session). tmux prints
                    // `no server running on <socket>` and exits 1 here, and
                    // scripts lean on that code (`tmux ls || start`), so an
                    // empty listing at exit 0 was a portability trap.
                    match l_socket_name.as_deref() {
                        Some(l) => eprintln!("psmux: no server running on {} (-L {})", dir, l),
                        None => eprintln!("psmux: no server running on {}", dir),
                    }
                    std::process::exit(1);
                }
                return Ok(());
            }
            "a" | "at" | "attach" | "attach-session" => {
                // Search cmd_args (skips binary name + global flags). Skip
                // cmd_args[0] which is the subcommand itself ("a"/"attach"/etc),
                // otherwise argv[0] (the exe path or subcommand name) gets
                // picked up as the target session name.
                let sub_args: Vec<&String> = cmd_args.iter().skip(1).copied().collect();
                // Explicit -t target takes precedence over every fallback (issue #408).
                // The global argument scan above STRIPS -t (and its value) out of
                // cmd_args, so re-reading it from sub_args here always missed and the
                // resolution fell through to resolve_last_session_name_ns — meaning
                // `attach-session -t NAME` reattached to whatever session was used
                // last instead of NAME. Read -t from the full, unstripped argv so the
                // requested target always wins. Works for both subcommand-position
                // (`attach-session -t s2`) and global-position (`-t s2 attach-session`).
                let explicit_target = args.iter().any(|a| a == "-t")
                    || sub_args.iter().any(|a| !a.starts_with('-'));
                let name = args
                    .iter()
                    .position(|a| a == "-t")
                    .and_then(|i| args.get(i + 1))
                    .map(|target| {
                        let session = crate::cli::parse_target(target)
                            .session
                            .unwrap_or_else(|| target.clone());
                        if let Some(ref l) = l_socket_name {
                            format!("{}__{}", l, session)
                        } else {
                            session
                        }
                    })
                    .or_else(|| {
                        // Accept positional argument as target session name
                        // (e.g. "psmux attach work" without -t flag)
                        let t_val_idx = sub_args.iter().position(|a| *a == "-t").map(|i| i + 1);
                        sub_args.iter().enumerate().find_map(|(i, a)| {
                            if !a.starts_with('-') && Some(i) != t_val_idx {
                                Some(if let Some(ref l) = l_socket_name {
                                    format!("{}__{}", l, a)
                                } else {
                                    (*a).clone()
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .or_else(resolve_default_session_name)
                    .or_else(|| crate::session::resolve_last_session_name_ns(l_socket_name.as_deref()))
                    .unwrap_or_else(|| {
                        if let Some(ref l) = l_socket_name {
                            format!("{}__0", l)
                        } else {
                            "0".to_string()
                        }
                    });
                // #362: tmux runs `new-session` from the config at server start,
                // so `attach-session` works even with no server running. psmux has
                // no persistent server, so when no session exists yet and the
                // config requests a new-session, bootstrap it: delegate to our own
                // `new-session` (which loads the config — the `new-session` line is
                // a no-op during config load, so there is no recursion — then
                // creates the session and attaches in this same console). Honour
                // the config's new-session args (e.g. -s NAME) but drop -d/-D so we
                // attach rather than leave it detached.
                if crate::session::list_session_names_ns(l_socket_name.as_deref()).is_empty() {
                    if let Some(ns_args) = crate::config::config_new_session_args() {
                        let exe = env::current_exe()?;
                        let mut child = std::process::Command::new(exe);
                        if let Some(ref l) = l_socket_name { child.arg("-L").arg(l); }
                        child.arg("new-session");
                        for a in ns_args.iter().filter(|a| a.as_str() != "-d" && a.as_str() != "-D") {
                            child.arg(a);
                        }
                        let _ = child.status()?;
                        return Ok(());
                    }
                }
                // tmux refuses to attach to a session that is not there:
                // `can't find session: NAME`, exit 1. psmux resolved the name and
                // then fell straight through to the client, where the non-tty
                // guard further down printed the version and returned success, so
                // a scripted `attach -t missing` reported OK for a session that
                // never existed (#29). Probe before committing to the attach.
                //
                // The gate is STRICT here (issue #605): a connect that does not
                // complete means no server, whatever error the OS chose to
                // report. The lenient reading let a stale registry entry pass
                // the gate and the raw winsock error surfaced from the client.
                let shown = l_socket_name
                    .as_deref()
                    .and_then(|l| name.strip_prefix(&format!("{}__", l)))
                    .unwrap_or(&name)
                    .to_string();
                if !probe_session_alive_strict(&name) {
                    // Nothing is listening on the registered port. Reap the
                    // entry so the next `ls`/`attach` does not re-litigate it,
                    // unless the pid anchor still vouches for a live server.
                    if crate::session::registry_pid_anchor_alive(&name) != Some(true) {
                        crate::session::remove_session_registry(&name);
                    }
                    // tmux wording: a bare `attach` that finds nothing to attach
                    // to says `no sessions`; only an explicit target names the
                    // session it could not find. The fallback name here was
                    // never typed by the user (it came from last_session or the
                    // `0` default), so echoing it back would blame a session
                    // they never asked for.
                    if !explicit_target
                        && crate::session::list_session_names_ns(l_socket_name.as_deref()).is_empty()
                    {
                        eprintln!("psmux: no sessions");
                    } else {
                        eprintln!("psmux: can't find session: {}", shown);
                    }
                    std::process::exit(1);
                }
                env::set_var("PSMUX_SESSION_NAME", name);
                env::set_var("PSMUX_SESSION_DISPLAY_NAME", shown);
                env::set_var("PSMUX_REMOTE_ATTACH", "1");
            }
            "server" => {
                // Internal command - run headless server (used when spawning background server)
                let name = args.iter().position(|a| a == "-s").and_then(|i| args.get(i+1)).map(|s| s.clone()).unwrap_or_else(|| "default".to_string());
                // Parse -L socket name for namespace isolation
                let server_socket_name = args.iter().position(|a| a == "-L").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Check for initial command via -c flag (shell-wrapped)
                let initial_cmd = args.iter().position(|a| a == "-c").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Parse start directory via -d flag
                let srv_start_dir = args.iter().position(|a| a == "-d").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Parse window name via -n flag
                let srv_window_name = args.iter().position(|a| a == "-n").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Parse initial dimensions via -x / -y flags
                let srv_init_width = args.iter().position(|a| a == "-x").and_then(|i| args.get(i+1)).and_then(|s| s.parse::<u16>().ok());
                let srv_init_height = args.iter().position(|a| a == "-y").and_then(|i| args.get(i+1)).and_then(|s| s.parse::<u16>().ok());
                let srv_init_size = match (srv_init_width, srv_init_height) {
                    (Some(w), Some(h)) => Some((w, h)),
                    (Some(w), None) => Some((w, 24)),
                    (None, Some(h)) => Some((80, h)),
                    _ => None,
                };
                // Parse session group target via -g flag
                let srv_group_target = args.iter().position(|a| a == "-g").and_then(|i| args.get(i+1)).map(|s| s.clone());
                // Parse -e environment variables (may appear multiple times)
                let srv_env_vars = crate::util::collect_server_session_env_args(&args).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidInput, e)
                })?;
                // Check for raw command after -- (direct execution)
                let raw_cmd: Option<Vec<String>> = args.iter().position(|a| a == "--").map(|pos| {
                    args.iter().skip(pos + 1).cloned().collect()
                }).filter(|v: &Vec<String>| !v.is_empty());
                return run_server(name, server_socket_name, initial_cmd, raw_cmd, srv_start_dir, srv_window_name, srv_init_size, srv_group_target, srv_env_vars);
            }
            "new-session" | "new" => {
                // Nesting guard is applied AFTER flag parsing below, once we know
                // whether -d (detached) was requested. A detached session never
                // grabs the current terminal, so nesting it is harmless and must
                // be allowed (issue #424). Only an attaching new-session takes
                // over the terminal and warrants the nesting warning.
                // Strict getopt-style parsing for new-session flags.
                // tmux template: "Ac:dDe:EF:f:n:Ps:t:x:Xy:"
                // Flags that take a value (letter followed by ':'):
                //   -s (session name), -n (window name), -F (format),
                //   -c (start dir), -x (width), -y (height), -e (env),
                //   -f (client flags), -t (target session)
                // Boolean flags: -A, -d, -D, -E, -P, -X
                let mut session_name: Option<String> = None;
                let mut detached = false;
                let mut print_info = false;
                let mut format_str: Option<String> = None;
                let mut window_name: Option<String> = None;
                let mut start_dir: Option<String> = None;
                let mut attach_if_exists = false;
                let mut init_width: Option<u16> = None;
                let mut init_height: Option<u16> = None;
                let mut group_target: Option<String> = None;
                let mut env_vars: Vec<(String, String)> = Vec::new();
                let mut positional_args: Vec<String> = Vec::new();
                let mut raw_cmd_after_dd: Option<Vec<String>> = None;

                {
                    let mut i = 1; // skip command name (cmd_args[0])
                    while i < cmd_args.len() {
                        let a = cmd_args[i].as_str();
                        if a == "--" {
                            // Everything after -- is raw command
                            raw_cmd_after_dd = Some(cmd_args[i+1..].iter().map(|s| s.to_string()).collect());
                            break;
                        }
                        // tmux uses getopt, which allows combined short flags
                        // like `-As main` (= `-A -s main`) or `-dP` (= `-d -P`).
                        // We expand combined flags inline.
                        if !a.starts_with('-') {
                            // Positional argument — collect it and everything after
                            positional_args.extend(cmd_args[i..].iter().map(|s| s.to_string()));
                            break;
                        }

                        let chars: Vec<char> = if a.len() > 2 && !a.starts_with("--") {
                            a[1..].chars().collect()
                        } else if a.len() == 2 {
                            vec![a.chars().nth(1).unwrap()]
                        } else {
                            // Unknown long flag, skip
                            i += 1; continue;
                        };

                        let mut k = 0;
                        while k < chars.len() {
                            let c = chars[k];
                            // Value-consuming flags: when in a combined group,
                            // remaining chars after the flag letter are the value (getopt style).
                            // If no remaining chars, the value is the next cmd_args element.
                            macro_rules! consume_value {
                                () => {{
                                    if k + 1 < chars.len() {
                                        // Rest of this arg is the value (e.g., -F#{fmt})
                                        let val: String = chars[k+1..].iter().collect();
                                        (val, true)
                                    } else {
                                        // Value is the next arg
                                        i += 1;
                                        let val = if i < cmd_args.len() { cmd_args[i].to_string() } else { String::new() };
                                        (val, true)
                                    }
                                }};
                            }
                            match c {
                            's' => { let (v, _) = consume_value!(); session_name = Some(v); break; }
                            'n' => { let (v, _) = consume_value!(); window_name = Some(v); break; }
                            'F' => { let (v, _) = consume_value!(); format_str = Some(v.trim_matches('"').to_string()); break; }
                            'c' => { let (v, _) = consume_value!(); start_dir = Some(v.trim_matches('"').to_string()); break; }
                            'x' => { let (v, _) = consume_value!(); init_width = v.parse::<u16>().ok(); break; }
                            'y' => { let (v, _) = consume_value!(); init_height = v.parse::<u16>().ok(); break; }
                            'e' => {
                                let (v, _) = consume_value!();
                                match crate::util::parse_new_session_e_value_token(
                                    Some(v.as_str()),
                                ) {
                                    Ok(pair) => env_vars.push(pair),
                                    Err(msg) => {
                                        return Err(io::Error::new(io::ErrorKind::InvalidInput, msg));
                                    }
                                }
                                break;
                            }
                            'f' => { let _ = consume_value!(); break; /* skip value */ }
                            't' => { let (v, _) = consume_value!(); group_target = Some(v); break; }
                            // Boolean flags
                            'd' => { detached = true; }
                            'P' => { print_info = true; }
                            'A' => { attach_if_exists = true; }
                            'D' | 'E' | 'X' => { /* ignored for compatibility */ }
                            _ => { /* unknown flag, skip */ }
                            }
                            k += 1;
                        }
                        i += 1;
                    }
                }

                // Prevent nesting only for an ATTACHING new-session (issue #424).
                // A detached (-d) session does not take over the current terminal,
                // so it is allowed to be created from inside an existing session,
                // matching tmux (which only warns for commands that grab the pty).
                // A popup is not a pane, so a session started from one is not
                // nested — util::inside_psmux_pane() draws the same line tmux
                // draws with its all_window_panes tty walk (#537).
                if !detached
                    && env::var("PSMUX_ALLOW_NESTING").ok().as_deref() != Some("1")
                    && crate::util::inside_psmux_pane()
                {
                    eprintln!("psmux: sessions should be nested with care, unset PSMUX_SESSION to force");
                    return Ok(());
                }

                let name = session_name.unwrap_or_else(|| {
                    // tmux-compatible: auto-generate numeric name (0, 1, 2, ...)
                    crate::session::next_session_name(l_socket_name.as_deref())
                });
                // Compute port file base name: with -L namespace prefix if specified
                let port_file_base = if let Some(ref l) = l_socket_name {
                    format!("{}__{}", l, name)
                } else {
                    name.clone()
                };
                // Check for -- separator: everything after it is a raw command (direct execution)
                let raw_cmd_args: Option<Vec<String>> = raw_cmd_after_dd.filter(|v| !v.is_empty());
                // Parse initial command from positional args (legacy mode, no --)
                let initial_cmd: Option<String> = if raw_cmd_args.is_some() || positional_args.is_empty() {
                    None
                } else {
                    Some(positional_args.join(" "))
                };
                
                // Check if session already exists AND is actually running
                let psmux_dir = crate::paths::psmux_dir();
                let port_path = crate::paths::port_file(&port_file_base);
                // PID of the server we spawn on the cold path (None when we adopt
                // a warm server or attach to a remote one). The readiness gate
                // below uses it to fail fast if the freshly spawned server dies.
                let mut server_pid: Option<u32> = None;
                if std::path::Path::new(&port_path).exists() {
                    // Verify server is actually running
                    let server_alive = if let Ok(port_str) = std::fs::read_to_string(&port_path) {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            let addr = format!("127.0.0.1:{}", port);
                            std::net::TcpStream::connect_timeout(
                                &addr.parse().unwrap(),
                                Duration::from_millis(100)
                            ).is_ok()
                        } else { false }
                    } else { false };
                    
                    if server_alive {
                        if attach_if_exists {
                            // -A flag: attach to existing session instead of erroring
                            env::set_var("PSMUX_SESSION_NAME", &port_file_base);
                            env::set_var("PSMUX_REMOTE_ATTACH", "1");
                            // Skip server creation, jump straight to attach
                            // (handled at the bottom of this match block)
                        } else {
                            eprintln!("duplicate session: {}", name);
                            std::process::exit(1);
                        }
                    } else {
                        // Stale port file - remove it and continue
                        let _ = std::fs::remove_file(&port_path);
                    }
                }
                
                // If -A attached to an existing session, skip server creation
                if env::var("PSMUX_REMOTE_ATTACH").ok().as_deref() == Some("1") {
                    // Already set up for attach — skip server spawn
                } else {
                // Fast path: try to claim a pre-spawned warm server.
                // The warm server has config loaded and shell already running,
                // so claiming it avoids the full cold-start latency.
                // Only eligible when no custom command/dir is requested.
                // Skipped when PSMUX_NO_WARM=1 is set or config has 'set -g warm off'.
                // Also skipped when a custom config file is specified (-f or PSMUX_CONFIG_FILE)
                // because the warm server loaded the default config, not the custom one.
                // Also skipped when -x/-y request an explicit size: the warm server
                // was spawned with the default geometry, and tmux guarantees that
                // `new-session -d -x W -y H` yields a session of exactly that size.
                let warm_disabled = std::env::var("PSMUX_NO_WARM").map(|v| v == "1" || v == "true").unwrap_or(false)
                    || crate::config::is_warm_disabled_by_config();
                let has_custom_config = f_config_file.is_some() || std::env::var("PSMUX_CONFIG_FILE").is_ok();
                let claimed_warm = if !warm_disabled && !has_custom_config && initial_cmd.is_none() && raw_cmd_args.is_none() && start_dir.is_none() && env_vars.is_empty() && init_width.is_none() && init_height.is_none() {
                    let warm_base = if let Some(ref l) = l_socket_name {
                        format!("{}____warm__", l)
                    } else {
                        "__warm__".to_string()
                    };
                    let warm_port_path = crate::paths::port_file(&warm_base);
                    // Atomically CLAIM the warm server before connecting. The
                    // __warm__.port file is a shared handoff: under rapid
                    // new-session, several clients could read the SAME file (and
                    // OS ephemeral-port reuse can make a stale entry point at an
                    // already-claimed server), which intermittently dropped a
                    // session. Renaming the port file is atomic on the same dir,
                    // so exactly one client wins the claim; losers cold-spawn.
                    let warm_port_opt = std::fs::read_to_string(&warm_port_path)
                        .ok()
                        .and_then(|s| s.trim().parse::<u16>().ok());
                    let claim_path = format!("{}\\{}.claiming.{}", psmux_dir, warm_base, std::process::id());
                    if let Some(warm_port) = warm_port_opt {
                        if std::fs::rename(&warm_port_path, &claim_path).is_ok() {
                            let warm_addr = format!("127.0.0.1:{}", warm_port);
                            let result = if std::net::TcpStream::connect_timeout(
                                &warm_addr.parse().unwrap(),
                                Duration::from_millis(100),
                            ).is_ok() {
                                let warm_key = crate::session::read_session_key(&warm_base).unwrap_or_default();
                                if !warm_key.is_empty() {
                                    let client_cwd = std::env::current_dir()
                                        .ok()
                                        .and_then(|p| p.to_str().map(|s| s.to_string()));
                                    // -p carries this shell's PSMUX_PRIORITY (or the
                                    // config value) onto the standby, which set its
                                    // own class before this shell existed (#608).
                                    let claim_prio = crate::platform::claim_priority_arg();
                                    let claim_cmd = if let Some(ref cwd) = client_cwd {
                                        format!("claim-session {} {} -p {}\n", crate::util::quote_arg(&name), crate::util::quote_arg(cwd), crate::util::quote_arg(&claim_prio))
                                    } else {
                                        format!("claim-session {} -p {}\n", crate::util::quote_arg(&name), crate::util::quote_arg(&claim_prio))
                                    };
                                    match crate::session::send_auth_cmd_response(
                                        &warm_addr, &warm_key,
                                        claim_cmd.as_bytes(),
                                    ) {
                                        Ok(resp) if resp.contains("OK") => {
                                            if let Some(ref wn) = window_name {
                                                let new_key = crate::session::read_session_key(&port_file_base).unwrap_or_default();
                                                let _ = crate::session::send_auth_cmd(
                                                    &warm_addr, &new_key,
                                                    format!("rename-window {}\n", crate::util::quote_arg(wn)).as_bytes(),
                                                );
                                            }
                                            // Apply -e environment variables to the claimed warm session
                                            if !env_vars.is_empty() {
                                                let new_key = crate::session::read_session_key(&port_file_base).unwrap_or_default();
                                                for (k, v) in &env_vars {
                                                    let _ = crate::session::send_auth_cmd(
                                                        &warm_addr, &new_key,
                                                        format!("set-environment {} {}\n", crate::util::quote_arg(k), crate::util::quote_arg(v)).as_bytes(),
                                                    );
                                                }
                                            }
                                            true
                                        }
                                        // Explicit rejection: the server answered
                                        // but it is NOT a warm server (e.g. a stale
                                        // __warm__.port left pointing at an already-
                                        // claimed session, or OS ephemeral-port reuse
                                        // routing us to a live non-warm server). This
                                        // claim will NEVER produce our session, so do
                                        // NOT commit — fall through to a clean cold
                                        // spawn. The stale handoff file was already
                                        // consumed (renamed to .claiming then removed
                                        // below), so the bad warm pointer self-heals
                                        // and the next open is fast again. Without
                                        // this, we would wait the full port-file
                                        // timeout (~5s) for a session that never
                                        // appears and then fail the open entirely.
                                        Ok(resp) if resp.contains("ERR") => false,
                                        // We have ALREADY atomically claimed this
                                        // warm (won the .port rename) and sent
                                        // claim-session to a live server, so it WILL
                                        // become our session. A slow/missing response
                                        // here must NOT trigger a cold spawn: doing so
                                        // would create a SECOND server with the same
                                        // name (duplicate -> desynced .port/.key ->
                                        // the session appears lost). Commit to the
                                        // claim; the post-claim port-wait below
                                        // verifies completion (and errors cleanly if
                                        // the warm somehow died mid-claim).
                                        _ => true,
                                    }
                                } else { false }
                            } else {
                                // Connect failed: the warm is dead and will NOT become
                                // our session, so a cold spawn is correct (no duplicate
                                // is possible because nothing claimed this name).
                                false
                            };
                            // The server writes <session>.port on a successful
                            // claim; our renamed handoff file is now orphaned
                            // either way, so remove it. On failure this also
                            // ensures the dead/stale warm entry does not linger.
                            let _ = std::fs::remove_file(&claim_path);
                            result
                        } else {
                            // Lost the claim race (another client renamed it
                            // first) or the file vanished — fall back to cold spawn.
                            false
                        }
                    } else { false }
                } else { false };

                if !claimed_warm {
                // Cold path: spawn a background server from scratch
                let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
                let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), name.clone()];
                // Pass -L socket name to server for namespace isolation
                if let Some(ref l) = l_socket_name {
                    server_args.push("-L".into());
                    server_args.push(l.clone());
                }
                // Pass initial command if provided
                if let Some(ref init_cmd) = initial_cmd {
                    server_args.push("-c".into());
                    server_args.push(init_cmd.clone());
                }
                // Pass start directory to server
                if let Some(ref dir) = start_dir {
                    server_args.push("-d".into());
                    server_args.push(dir.clone());
                }
                // Pass window name to server
                if let Some(ref wn) = window_name {
                    server_args.push("-n".into());
                    server_args.push(wn.clone());
                }
                // Pass initial dimensions to server
                if let Some(w) = init_width {
                    server_args.push("-x".into());
                    server_args.push(w.to_string());
                }
                if let Some(h) = init_height {
                    server_args.push("-y".into());
                    server_args.push(h.to_string());
                }
                // Pass session group target to server
                if let Some(ref gt) = group_target {
                    server_args.push("-g".into());
                    server_args.push(gt.clone());
                }
                // Pass -e environment variables to server
                for (k, v) in &env_vars {
                    server_args.push("-e".into());
                    server_args.push(format!("{}={}", k, v));
                }
                // Pass raw command args (direct execution) if -- was used
                if let Some(ref raw_args) = raw_cmd_args {
                    server_args.push("--".into());
                    for a in raw_args {
                        server_args.push(a.clone());
                    }
                }
                // On Windows, mark parent's stdout/stderr as non-inheritable before
                // spawning the server. This prevents the server from inheriting
                // PowerShell's redirect pipes (which would cause the parent to hang
                // waiting for the pipe to close). The server creates its own ConPTY
                // handles so it doesn't need the parent's stdio.
                #[cfg(windows)]
                {
                    #[link(name = "kernel32")]
                    extern "system" {
                        fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
                        fn SetHandleInformation(hObject: *mut std::ffi::c_void, dwMask: u32, dwFlags: u32) -> i32;
                    }
                    const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5u32; // -11i32 as u32
                    const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4u32;  // -12i32 as u32
                    const HANDLE_FLAG_INHERIT: u32 = 0x00000001;
                    unsafe {
                        let stdout = GetStdHandle(STD_OUTPUT_HANDLE);
                        let stderr = GetStdHandle(STD_ERROR_HANDLE);
                        SetHandleInformation(stdout, HANDLE_FLAG_INHERIT, 0);
                        SetHandleInformation(stderr, HANDLE_FLAG_INHERIT, 0);
                    }
                }
                // Spawn server with a hidden console window via CreateProcessW.
                // This gives ConPTY a real console while keeping the window invisible.
                #[cfg(windows)]
                { server_pid = Some(crate::platform::spawn_server_hidden(&exe, &server_args)?); }
                #[cfg(not(windows))]
                {
                    let mut cmd = std::process::Command::new(&exe);
                    for a in &server_args { cmd.arg(a); }
                    cmd.stdin(std::process::Stdio::null());
                    cmd.stdout(std::process::Stdio::null());
                    cmd.stderr(std::process::Stdio::null());
                    let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn server: {e}")))?;
                }
                } // end if !claimed_warm (cold path)
                } // end else (not PSMUX_REMOTE_ATTACH)
                
                // Wait for server to create port file (up to 5 seconds)
                // Poll fast (10ms) — the server writes the port file early,
                // before spawning ConPTY/pwsh, so it should appear quickly.
                //
                // Wait for the server to become READY before returning, gating on
                // ACTUAL readiness rather than mere socket reachability. The server
                // writes its .port file and binds/accepts BEFORE it creates the
                // initial window and BEFORE its main request loop runs; under load
                // any of those steps can take seconds. The old tight check (5s port
                // poll + one 100ms connect) wrongly declared a slow-but-healthy
                // server dead — and even deleted its .port file, orphaning a live
                // session. Instead, loop until one of these terminal conditions:
                //
                //   READY (rc=0):
                //     - .port present + readable + a TCP connect succeeds, AND
                //     - for detached sessions, list-windows is non-empty (the
                //       initial window exists). list-windows is answered only by
                //       the main loop, which runs only after create_window, so a
                //       non-empty reply IS the "command finished server-side"
                //       signal — matching how tmux gates on command completion.
                //
                //   FAST-FAIL (rc=1), so we NEVER block on a doomed server:
                //     - .port vanished after we first saw it: the server hit a
                //       create_window failure / panic (its panic hook removes the
                //       .port) and exited. The client never deletes the .port
                //       itself — the server owns it.
                //     - the server PROCESS we spawned has died: covers a hard kill
                //       / abrupt exit / any path that skips the panic-hook cleanup
                //       and would otherwise leave a stale .port. We only consult
                //       this AFTER the readiness check fails for the iteration, so
                //       a healthy reachable server is never declared dead.
                //
                //   DEADLINE (rc=1): a hard 15s upper bound. The fast-fail signals
                //     above catch every real failure within ~20ms, so in practice
                //     this only ever fires for the pathological "process alive but
                //     create_window genuinely hangs forever" case — bounded, never
                //     an infinite hang. 15s comfortably exceeds the measured worst
                //     case window-creation latency under heavy concurrent load
                //     (~9s), so it does not reintroduce false failures.
                env::set_var("PSMUX_TARGET_SESSION", &port_file_base);
                // Wall-clock second this attempt began — used to tell a fresh
                // server-startup.log (this failure) from a stale one (issue #370).
                let attempt_start_epoch = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let ready_deadline = std::time::Instant::now() + Duration::from_secs(15);
                let mut port_seen = false;
                let mut ready = false;
                loop {
                    if std::path::Path::new(&port_path).exists() {
                        port_seen = true;
                        let connectable = std::fs::read_to_string(&port_path).ok()
                            .and_then(|s| s.trim().parse::<u16>().ok())
                            .map(|port| {
                                let addr = format!("127.0.0.1:{}", port);
                                std::net::TcpStream::connect_timeout(
                                    &addr.parse().unwrap(),
                                    Duration::from_millis(200),
                                ).is_ok()
                            })
                            .unwrap_or(false);
                        if connectable {
                            if !detached {
                                // Attached: connectivity is enough — we attach below.
                                ready = true;
                                break;
                            }
                            // Detached: also require the initial window to exist.
                            // A non-empty, non-error reply means >0 windows: the
                            // default (tmux-text) list-windows is "" for zero
                            // windows (not the `-J` JSON form, whose empty value
                            // "[]" is non-empty), and only the main loop answers
                            // it, after create_window. The non-error guard rejects
                            // a racing auth failure — see detached_list_windows_ready.
                            if let Ok(resp) = send_control_with_response("list-windows\n".to_string()) {
                                if detached_list_windows_ready(&resp) { ready = true; break; }
                            }
                        }
                    } else if port_seen {
                        // .port vanished after appearing → server cleaned up and exited.
                        break;
                    }
                    // Readiness not met this iteration → consult the process-death
                    // fast-fail signal (cold path only; None when we adopted a warm
                    // or remote server). A dead PID here means the server exited
                    // without leaving a usable session.
                    if let Some(pid) = server_pid {
                        if !crate::platform::process_is_alive(pid) { break; }
                    }
                    if std::time::Instant::now() >= ready_deadline { break; }
                    std::thread::sleep(Duration::from_millis(20));
                }
                if !ready {
                    eprintln!("psmux: failed to create session '{}'", name);
                    // Issue #370: surface the real reason instead of leaving it
                    // buried in ~/.psmux/server-startup.log. The detached server
                    // records the concrete spawn failure (e.g. a bad
                    // default-shell path) there before exiting; echo it so the
                    // user isn't left guessing why their config "silently" failed.
                    if let Some((reason, log_path)) =
                        crate::server::read_fresh_startup_error(attempt_start_epoch)
                    {
                        eprintln!("psmux: {}", reason);
                        eprintln!("psmux: full startup diagnostics in {}", log_path);
                    }
                    std::process::exit(1);
                }

                // Session came up. Surface any non-fatal config parse warnings
                // (unknown command/option, malformed value) the server recorded
                // during config load, so a typo'd ~/.psmux.conf is not silently
                // ignored (issue #370 follow-up). Printed before attaching so it
                // is visible in the terminal / scrollback.
                let cfg_warnings = crate::server::read_fresh_config_warnings(attempt_start_epoch);
                if !cfg_warnings.is_empty() {
                    eprintln!("psmux: {} config warning(s):", cfg_warnings.len());
                    for w in &cfg_warnings {
                        eprintln!("psmux:   {}", w);
                    }
                }

                if detached {
                    // The readiness wait above already confirmed the initial
                    // window exists. If -P, print the pane info before returning.
                    if print_info {
                        // Query the server for pane info using display-message
                        let fmt = if let Some(ref f) = format_str {
                            f.clone()
                        } else {
                            // tmux default: new-session -P prints "session_name:"
                            "#{session_name}:".to_string()
                        };
                        match send_control_with_response(format!("display-message -p {}\n", fmt)) {
                            Ok(resp) => { let trimmed = resp.trim(); if !trimmed.is_empty() { println!("{}", trimmed); } }
                            Err(_) => {}
                        }
                    }
                    return Ok(());
                } else {
                    // User wants attached session - set env vars to attach
                    env::set_var("PSMUX_SESSION_NAME", &port_file_base);
                    env::set_var("PSMUX_REMOTE_ATTACH", "1");
                    // Continue to attach below...
                }
            }
            "new-window" | "neww" => {
                // Strict getopt-style parsing for new-window flags.
                // tmux template: "ac:dDe:F:kn:Pt:S:"
                let mut name_arg: Option<String> = None;
                let mut detached = false;
                let mut print_info = false;
                let mut format_str: Option<String> = None;
                let mut start_dir: Option<String> = None;
                let mut title_arg: Option<String> = None;
                let mut empty_flag = false;
                let mut env_args: Vec<String> = Vec::new();
                let mut nw_positional: Vec<String> = Vec::new();
                let mut nw_saw_ddash = false;
                {
                    let mut i = 1;
                    while i < cmd_args.len() {
                        let a = cmd_args[i].as_str();
                        if a == "--" { nw_saw_ddash = true; nw_positional.extend(cmd_args[i+1..].iter().map(|s| s.to_string())); break; }
                        match a {
                            "-n" => { i += 1; if i < cmd_args.len() { name_arg = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            "-F" => { i += 1; if i < cmd_args.len() { format_str = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            s if s.starts_with("-F") && s.len() > 2 => { format_str = Some(s[2..].trim_matches('"').to_string()); }
                            "-c" => { i += 1; if i < cmd_args.len() { start_dir = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            "-T" => { i += 1; if i < cmd_args.len() { title_arg = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            // -e KEY=VALUE environment for the new pane (#489)
                            "-e" => { i += 1; if i < cmd_args.len() { env_args.push(cmd_args[i].trim_matches('"').to_string()); } }
                            "-t" | "-S" => { i += 1; /* skip value */ }
                            "-d" => { detached = true; }
                            "-P" => { print_info = true; }
                            "-E" => { empty_flag = true; }
                            "-a" | "-D" | "-k" => { /* ignored for compatibility */ }
                            _ if a.starts_with('-') => { /* unknown flag, skip */ }
                            _ => { nw_positional.extend(cmd_args[i..].iter().map(|s| s.to_string())); break; }
                        }
                        i += 1;
                    }
                }
                // cwd parity: a command-line new-window with no -c must open in
                // the CALLER's cwd, not the server's (session) cwd. tmux picks a
                // new window's cwd from three cases — an explicit -c, an attached
                // client (session start dir), or a detached client (the caller's
                // dir); the three cases are spelled out in tmux's own source. Each
                // psmux CLI call is a one-shot detached client, so default -c to
                // our current dir when the user gave none. Tradeoff: this defeats
                // the warm-pane fast path for command-line new-window (the warm
                // pane lives in the server's cwd) — interactive prefix-c, which
                // never routes through here, keeps it.
                if start_dir.is_none() {
                    if let Ok(cwd) = std::env::current_dir() {
                        start_dir = Some(cwd.to_string_lossy().into_owned());
                    }
                }
                let cmd_arg = nw_positional.join(" ");
                let cmd_arg = cmd_arg.as_str();
                let mut cmd_line = "new-window".to_string();
                if detached { cmd_line.push_str(" -d"); }
                if print_info { cmd_line.push_str(" -P"); }
                if empty_flag { cmd_line.push_str(" -E"); }
                if let Some(ref fmt) = format_str {
                    cmd_line.push_str(&format!(" -F {}", crate::util::quote_arg(&fmt)));
                }
                if let Some(name) = &name_arg {
                    cmd_line.push_str(&format!(" -n {}", crate::util::quote_arg(&name)));
                }
                if let Some(t) = &title_arg {
                    cmd_line.push_str(&format!(" -T {}", crate::util::quote_arg(&t)));
                }
                if let Some(dir) = &start_dir {
                    cmd_line.push_str(&format!(" -c {}", crate::util::quote_arg(&dir)));
                }
                for ev in &env_args {
                    cmd_line.push_str(&format!(" -e {}", crate::util::quote_arg(&ev)));
                }
                // tmux parity (#582): a multi-token `-- prog args...` argv is
                // exec'd directly by the server, so forward the tokens
                // individually behind the `--` marker instead of collapsing
                // them into one shell string.
                if nw_saw_ddash && nw_positional.len() > 1 {
                    cmd_line.push_str(" --");
                    for tok in &nw_positional {
                        cmd_line.push_str(&format!(" {}", crate::util::quote_arg(tok)));
                    }
                } else if !cmd_arg.is_empty() {
                    cmd_line.push_str(&format!(" {}", crate::util::quote_arg(&cmd_arg)));
                }
                cmd_line.push('\n');
                if print_info {
                    let resp = send_control_with_response(cmd_line)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd_line)?;
                }
                return Ok(());
            }
            "split-window" | "splitw" | "split-pane" | "splitp" => {
                // split-pane / splitp are tmux's default command-aliases for
                // split-window (options-table.c). psmux handles them as arm
                // synonyms, matching how info/server-info and choose-window/
                // choose-session are already aliased. See issue #426.
                // Strict getopt-style parsing for split-window flags.
                // tmux template: "bc:de:F:fhIl:p:Pt:vZ"
                let mut flag = "-v";
                let mut detached = false;
                let mut print_info = false;
                let mut format_str: Option<String> = None;
                let mut start_dir: Option<String> = None;
                let mut size_pct: Option<String> = None;
                let mut size_cells: Option<String> = None;
                let mut title_arg: Option<String> = None;
                let mut env_args: Vec<String> = Vec::new();
                let mut zoom_after_split = false;
                let mut sw_positional: Vec<String> = Vec::new();
                let mut sw_saw_ddash = false;
                {
                    let mut i = 1;
                    while i < cmd_args.len() {
                        let a = cmd_args[i].as_str();
                        if a == "--" { sw_saw_ddash = true; sw_positional.extend(cmd_args[i+1..].iter().map(|s| s.to_string())); break; }
                        match a {
                            "-F" => { i += 1; if i < cmd_args.len() { format_str = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            s if s.starts_with("-F") && s.len() > 2 => { format_str = Some(s[2..].trim_matches('"').to_string()); }
                            "-c" => { i += 1; if i < cmd_args.len() { start_dir = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            "-T" => { i += 1; if i < cmd_args.len() { title_arg = Some(cmd_args[i].trim_matches('"').to_string()); } }
                            "-p" => { i += 1; if i < cmd_args.len() { size_pct = Some(cmd_args[i].to_string()); size_cells = None; } }
                            "-l" => { i += 1; if i < cmd_args.len() { let v = cmd_args[i].to_string(); if v.ends_with('%') { size_pct = Some(v); size_cells = None; } else { size_cells = Some(v); size_pct = None; } } }
                            // -e KEY=VALUE environment for the new pane (#489)
                            "-e" => { i += 1; if i < cmd_args.len() { env_args.push(cmd_args[i].trim_matches('"').to_string()); } }
                            "-t" => { i += 1; /* skip value */ }
                            "-h" => { flag = "-h"; }
                            "-v" => { flag = "-v"; }
                            "-d" => { detached = true; }
                            "-P" => { print_info = true; }
                            "-Z" => { zoom_after_split = true; }
                            "-b" | "-f" | "-I" => { /* ignored for compatibility */ }
                            _ if a.starts_with('-') => { /* unknown flag, skip */ }
                            _ => { sw_positional.extend(cmd_args[i..].iter().map(|s| s.to_string())); break; }
                        }
                        i += 1;
                    }
                }
                // cwd parity (same as new-window): a command-line split with no
                // -c must open in the CALLER's cwd, not the server's (session)
                // cwd. Each psmux CLI call is a one-shot detached client, so
                // default -c to our current dir when none given.
                if start_dir.is_none() {
                    if let Ok(cwd) = std::env::current_dir() {
                        start_dir = Some(cwd.to_string_lossy().into_owned());
                    }
                }
                let cmd_arg = sw_positional.join(" ");
                let cmd_arg = cmd_arg.as_str();
                let mut cmd_line = format!("split-window {}", flag);
                if detached { cmd_line.push_str(" -d"); }
                if print_info { cmd_line.push_str(" -P"); }
                if zoom_after_split { cmd_line.push_str(" -Z"); }
                if let Some(ref fmt) = format_str {
                    cmd_line.push_str(&format!(" -F {}", crate::util::quote_arg(&fmt)));
                }
                if let Some(dir) = &start_dir {
                    cmd_line.push_str(&format!(" -c {}", crate::util::quote_arg(&dir)));
                }
                if let Some(t) = &title_arg {
                    cmd_line.push_str(&format!(" -T {}", crate::util::quote_arg(&t)));
                }
                if let Some(pct) = &size_pct {
                    cmd_line.push_str(&format!(" -p {}", pct));
                } else if let Some(cells) = &size_cells {
                    cmd_line.push_str(&format!(" -l {}", cells));
                }
                for ev in &env_args {
                    cmd_line.push_str(&format!(" -e {}", crate::util::quote_arg(&ev)));
                }
                // tmux parity (#582): a multi-token `-- prog args...` argv is
                // exec'd directly by the server; forward tokens individually
                // behind the `--` marker.
                if sw_saw_ddash && sw_positional.len() > 1 {
                    cmd_line.push_str(" --");
                    for tok in &sw_positional {
                        cmd_line.push_str(&format!(" {}", crate::util::quote_arg(tok)));
                    }
                } else if !cmd_arg.is_empty() {
                    cmd_line.push_str(&format!(" {}", crate::util::quote_arg(&cmd_arg)));
                }
                cmd_line.push('\n');
                if print_info {
                    let resp = send_control_with_response(cmd_line)?;
                    // #559: an ERROR reply must never masquerade as the new
                    // pane id. `-P -F '#{pane_id}'` is the documented way for
                    // scripts to capture the created pane; printing the error
                    // text to stdout with exit 0 hands the caller a bogus
                    // "id" it cannot distinguish from success (psmux-resurrect
                    // silently lost every split this way).
                    if resp.trim_start().starts_with("ERROR") {
                        eprint!("{}", resp);
                        std::process::exit(1);
                    }
                    print!("{}", resp);
                } else {
                    let resp = send_control_with_response(cmd_line)?;
                    if !resp.is_empty() {
                        eprint!("{}", resp);
                        std::process::exit(1);
                    }
                }
                return Ok(());
            }
            "kill-pane" | "killp" => { send_control("kill-pane\n".to_string())?; return Ok(()); }
            "capture-pane" | "capturep" => {
                // Parse optional flags - cmd_args[0] is command, start from 1
                let mut cmd = "capture-pane".to_string();
                let mut print_stdout = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(target) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", target));
                                i += 1;
                            }
                        }
                        "-S" => {
                            if let Some(start) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -S {}", start));
                                i += 1;
                            }
                        }
                        "-E" => {
                            if let Some(end) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -E {}", end));
                                i += 1;
                            }
                        }
                        "-p" => { cmd.push_str(" -p"); print_stdout = true; }
                        "-e" => { cmd.push_str(" -e"); }
                        "-J" => { cmd.push_str(" -J"); }
                        "-N" => { cmd.push_str(" -N"); }
                        "-b" => {
                            if let Some(buf) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -b {}", buf));
                                i += 1;
                            }
                        }
                        a if a.len() > 2
                            && a.starts_with('-')
                            && !a.starts_with("--")
                            && a.chars().skip(1).all(|c| matches!(c, 'p' | 'e' | 'J' | 'N')) =>
                        {
                            // POSIX cluster of capture-pane booleans (-ep, -pe, -pJ, -eJ,
                            // -epJ, ...). -t/-S/-E/-b take a value, so they are NOT
                            // eligible for clustering.
                            if a.contains('p') { cmd.push_str(" -p"); print_stdout = true; }
                            if a.contains('e') { cmd.push_str(" -e"); }
                            if a.contains('J') { cmd.push_str(" -J"); }
                            if a.contains('N') { cmd.push_str(" -N"); }
                        }
                        // Cluster ending in a value-taking flag: -pt <target>,
                        // -pet <target>, -pS <start>, etc.
                        a if a.len() > 2
                            && a.starts_with('-')
                            && !a.starts_with("--")
                            && {
                                let last = a.chars().last().unwrap_or(' ');
                                matches!(last, 't' | 'S' | 'E' | 'b')
                                    && a[1..a.len()-1].chars().all(|c| matches!(c, 'p' | 'e' | 'J' | 'N'))
                            } =>
                        {
                            let last = a.chars().last().unwrap();
                            // Expand boolean flags in the cluster
                            if a.contains('p') { cmd.push_str(" -p"); print_stdout = true; }
                            if a.contains('e') { cmd.push_str(" -e"); }
                            if a.contains('J') { cmd.push_str(" -J"); }
                            if a.contains('N') { cmd.push_str(" -N"); }
                            // Consume the next arg as the value for the trailing flag
                            if let Some(val) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -{} {}", last, val));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                if print_stdout {
                    let resp = send_control_with_response(cmd)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // send-keys - Send keys to a pane (critical for scripting)
            "send-keys" | "send" | "send-key" => {
                let mut literal = false;
                let mut has_x = false;
                let mut has_hex = false;
                let mut keys: Vec<String> = Vec::new();
                // Getopt-style parsing: -t consumes next arg, -l/-R/-X/-H are flags
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-l" => { literal = true; }
                        "-R" => { keys.push("__RESET__".to_string()); }
                        "-X" => { has_x = true; }
                        "-H" => { has_hex = true; }
                        "-t" => { i += 1; } // consume target value (already handled globally)
                        "-N" => { i += 1; } // repeat count, consume value
                        _ => { keys.push(cmd_args[i].to_string()); }
                    }
                    i += 1;
                }
                let mut cmd = "send-keys".to_string();
                if literal { cmd.push_str(" -l"); }
                if has_x { cmd.push_str(" -X"); }
                if has_hex { cmd.push_str(" -H"); }
                // Quote arguments that need it. quote_arg_if_needed escapes
                // backslashes as well as quotes inside the wrapping quotes,
                // matching what parse_command_line decodes there (#547) —
                // the old encoder escaped only `"`, so a quoted key ending
                // in `\` consumed the closing quote and swallowed the rest.
                // Unquoted values keep literal backslashes byte-exact
                // (Windows path separators).
                for k in keys {
                    cmd.push_str(&format!(" {}", crate::util::quote_arg_if_needed(&k)));
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // Base64 is the protocol boundary: unlike send-keys -l, it cannot
            // inject a second newline-delimited control command. It also avoids
            // nesting the payload inside PowerShell -EncodedCommand, which
            // crossed cmd.exe's 8191-character limit around 2 KiB of text.
            "send-paste" => {
                let normalized: Vec<&str> = cmd_args.iter().map(|arg| arg.as_str()).collect();
                send_control(build_send_paste_control(&normalized)?)?;
                return Ok(());
            }
            // select-pane - Select the active pane
            "select-pane" | "selectp" => {
                let mut cmd = "select-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-T" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -T {}", crate::util::quote_arg(&t)));
                                i += 1;
                            }
                        }
                        "-P" => {
                            if let Some(s) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -P {}", crate::util::quote_arg(&s)));
                                i += 1;
                            }
                        }
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-L" => { cmd.push_str(" -L"); }
                        "-R" => { cmd.push_str(" -R"); }
                        "-l" => { cmd.push_str(" -l"); }
                        "-Z" => { cmd.push_str(" -Z"); }
                        "-m" => { cmd.push_str(" -m"); }
                        "-M" => { cmd.push_str(" -M"); }
                        "-e" => { cmd.push_str(" -e"); }
                        "-d" => { cmd.push_str(" -d"); }
                        _ => {}
                    }
                    i += 1;
                }
                // Target validation happens in the shared
                // cli_validate_window_pane_target() before dispatch (#554) —
                // the per-arm copy it replaced skipped every ':'-bearing
                // target, so the fully-qualified sess:win.pane form was the
                // only pane-addressing form with no error signal.
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // select-window - Select a window
            "select-window" | "selectw" => {
                let mut cmd = "select-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-l" => { cmd.push_str(" -l"); }
                        "-n" => { cmd.push_str(" -n"); }
                        "-p" => { cmd.push_str(" -p"); }
                        _ => {}
                    }
                    i += 1;
                }
                // Target validation happens in the shared
                // cli_validate_window_pane_target() before dispatch (#554);
                // it covers the ':' window form this arm used to check plus
                // the pane forms it never did.
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // list-panes - List all panes
            "list-panes" | "lsp" => {
                let mut all_sessions = false;
                let mut session_scope = false;
                let mut format_str: Option<String> = None;
                let mut target_session: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { all_sessions = true; }
                        "-s" => { session_scope = true; }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                target_session = Some(t.to_string());
                                i += 1;
                            }
                        }
                        "-F" => {
                            if let Some(f) = cmd_args.get(i + 1) {
                                format_str = Some(f.to_string());
                                i += 1;
                            }
                        }
                        s if s.starts_with("-F") && s.len() > 2 => {
                            format_str = Some(s[2..].to_string());
                        }
                        _ => {}
                    }
                    i += 1;
                }

                if all_sessions {
                    // Iterate over all session port files (like list-sessions does)
                    let dir = crate::paths::psmux_dir();
                    let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for e in entries.flatten() {
                            if let Some(name) = e.file_name().to_str() {
                                if let Some((base, ext)) = name.rsplit_once('.') {
                                    if ext != "port" { continue; }
                                    if crate::session::is_warm_session(base) { continue; }
                                    if let Some(ref pfx) = ns_prefix {
                                        if !base.starts_with(pfx.as_str()) { continue; }
                                    } else if base.contains("__") { continue; }
                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                        if let Ok(_p) = port_str.trim().parse::<u16>() {
                                            let addr = format!("127.0.0.1:{}", port_str.trim());
                                            let conn = std::net::TcpStream::connect_timeout(
                                                &addr.parse().unwrap(),
                                                Duration::from_millis(500),
                                            );
                                            // Only prune a session that ACTIVELY refused (truly dead);
                                            // a timeout means busy-but-alive and must not be deleted.
                                            let refused = matches!(&conn, Err(e) if e.kind() == io::ErrorKind::ConnectionRefused);
                                            if let Ok(mut s) = conn {
                                                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                                                let key_path = crate::paths::key_file(&base);
                                                if let Ok(key) = std::fs::read_to_string(&key_path) {
                                                    let _ = std::io::Write::write_all(&mut s, format!("AUTH {}\n", key.trim()).as_bytes());
                                                }
                                                // Send list-panes -s (all panes in this session) to each server
                                                let query = if let Some(ref fmt) = format_str {
                                                    format!("list-panes -s -F {}\n", crate::util::quote_arg(&fmt))
                                                } else {
                                                    "list-panes -s\n".to_string()
                                                };
                                                let _ = std::io::Write::write_all(&mut s, query.as_bytes());
                                                let _ = s.flush();
                                                let mut br = std::io::BufReader::new(s);
                                                let mut line = String::new();
                                                let _ = std::io::BufRead::read_line(&mut br, &mut line);
                                                if line.trim() == "OK" {
                                                    line.clear();
                                                    let _ = std::io::BufRead::read_line(&mut br, &mut line);
                                                }
                                                if line.trim() == "ERROR: Authentication required" { continue; }
                                                // Print all lines from this session
                                                if !line.trim().is_empty() {
                                                    print!("{}", line);
                                                }
                                                loop {
                                                    let mut next = String::new();
                                                    match std::io::BufRead::read_line(&mut br, &mut next) {
                                                        Ok(0) => break,
                                                        Ok(_) => {
                                                            if !next.trim().is_empty() {
                                                                print!("{}", next);
                                                            }
                                                        }
                                                        Err(_) => break,
                                                    }
                                                }
                                            } else if refused {
                                                // Whole set, not just port+key (#530).
                                                crate::session::remove_session_registry_files(&e.path());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Single session: build command and send to target session
                    let mut cmd = "list-panes".to_string();
                    if session_scope { cmd.push_str(" -s"); }
                    if let Some(ref t) = target_session {
                        cmd.push_str(&format!(" -t {}", t));
                    }
                    if let Some(ref f) = format_str {
                        cmd.push_str(&format!(" -F {}", crate::util::quote_arg(f.trim_matches('"'))));
                    }
                    cmd.push('\n');
                    let resp = send_control_with_response(cmd)?;
                    print!("{}", resp);
                }
                return Ok(());
            }
            // list-windows - List all windows
            "list-windows" | "lsw" => {
                let mut all_sessions = false;
                let mut json_mode = false;
                let mut format_str: Option<String> = None;
                let mut target_session: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { all_sessions = true; }
                        "-J" => { json_mode = true; }
                        "-F" => {
                            if let Some(f) = cmd_args.get(i + 1) {
                                format_str = Some(f.to_string());
                                i += 1;
                            }
                        }
                        s if s.starts_with("-F") && s.len() > 2 => {
                            format_str = Some(s[2..].to_string());
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                target_session = Some(t.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }

                if all_sessions {
                    // Iterate over all session port files (like list-sessions does)
                    let dir = crate::paths::psmux_dir();
                    let ns_prefix = l_socket_name.as_ref().map(|l| format!("{l}__"));
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for e in entries.flatten() {
                            if let Some(name) = e.file_name().to_str() {
                                if let Some((base, ext)) = name.rsplit_once('.') {
                                    if ext != "port" { continue; }
                                    if crate::session::is_warm_session(base) { continue; }
                                    if let Some(ref pfx) = ns_prefix {
                                        if !base.starts_with(pfx.as_str()) { continue; }
                                    } else if base.contains("__") { continue; }
                                    if let Ok(port_str) = std::fs::read_to_string(e.path()) {
                                        if let Ok(_p) = port_str.trim().parse::<u16>() {
                                            let addr = format!("127.0.0.1:{}", port_str.trim());
                                            let conn = std::net::TcpStream::connect_timeout(
                                                &addr.parse().unwrap(),
                                                Duration::from_millis(500),
                                            );
                                            // Only prune a session that ACTIVELY refused (truly dead);
                                            // a timeout means busy-but-alive and must not be deleted.
                                            let refused = matches!(&conn, Err(e) if e.kind() == io::ErrorKind::ConnectionRefused);
                                            if let Ok(mut s) = conn {
                                                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                                                let key_path = crate::paths::key_file(&base);
                                                if let Ok(key) = std::fs::read_to_string(&key_path) {
                                                    let _ = std::io::Write::write_all(&mut s, format!("AUTH {}\n", key.trim()).as_bytes());
                                                }
                                                // Send list-windows to each server (without -a to avoid recursion)
                                                let query = if let Some(ref fmt) = format_str {
                                                    format!("list-windows -F {}\n", crate::util::quote_arg(&fmt))
                                                } else if json_mode {
                                                    "list-windows -J\n".to_string()
                                                } else {
                                                    "list-windows\n".to_string()
                                                };
                                                let _ = std::io::Write::write_all(&mut s, query.as_bytes());
                                                let _ = s.flush();
                                                let mut br = std::io::BufReader::new(s);
                                                let mut line = String::new();
                                                let _ = std::io::BufRead::read_line(&mut br, &mut line);
                                                if line.trim() == "OK" {
                                                    line.clear();
                                                    let _ = std::io::BufRead::read_line(&mut br, &mut line);
                                                }
                                                if line.trim() == "ERROR: Authentication required" { continue; }
                                                if !line.trim().is_empty() {
                                                    print!("{}", line);
                                                }
                                                loop {
                                                    let mut next = String::new();
                                                    match std::io::BufRead::read_line(&mut br, &mut next) {
                                                        Ok(0) => break,
                                                        Ok(_) => {
                                                            if !next.trim().is_empty() {
                                                                print!("{}", next);
                                                            }
                                                        }
                                                        Err(_) => break,
                                                    }
                                                }
                                            } else if refused {
                                                // Whole set, not just port+key (#530).
                                                crate::session::remove_session_registry_files(&e.path());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // Single session: build command and send to target session
                    let mut cmd = "list-windows".to_string();
                    if json_mode { cmd.push_str(" -J"); }
                    if let Some(ref f) = format_str {
                        cmd.push_str(&format!(" -F {}", crate::util::quote_arg(f.trim_matches('"'))));
                    }
                    if let Some(ref t) = target_session {
                        cmd.push_str(&format!(" -t {}", t));
                    }
                    cmd.push('\n');
                    let resp = send_control_with_response(cmd)?;
                    print!("{}", resp);
                }
                return Ok(());
            }
            // kill-window - Kill a window
            "kill-window" | "killw" => {
                let mut cmd = "kill-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-a" => { cmd.push_str(" -a"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                // #559: the server already answers "ERROR: can't find window"
                // for a bad -t, but the fire-and-forget send discarded it and
                // exited 0 — a silent no-op (tmux exits 1). Read the reply.
                let resp = send_control_with_response(cmd)?;
                if !resp.trim().is_empty() {
                    eprint!("{}", resp);
                    std::process::exit(1);
                }
                return Ok(());
            }
            // detach-client - Gracefully detach attached client(s) (issue #275)
            "detach-client" | "detach" => {
                let mut t_target: Option<String> = None;
                let mut s_target: Option<String> = None;
                let mut detach_all = false;
                let mut kill_parent = false;
                let mut shell_cmd: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { detach_all = true; }
                        "-P" => { kill_parent = true; }
                        "-t" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                t_target = Some(v.to_string());
                                i += 1;
                            }
                        }
                        "-s" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                s_target = Some(v.to_string());
                                i += 1;
                            }
                        }
                        "-E" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                shell_cmd = Some(v.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                // Apply -L namespace prefix to -s session lookup so users can
                // target a namespaced session by its short name.
                let session_for_routing = if let Some(s) = &s_target {
                    if let Some(ref l) = l_socket_name {
                        format!("{}__{}", l, s)
                    } else {
                        s.clone()
                    }
                } else {
                    env::var("PSMUX_TARGET_SESSION").unwrap_or_else(|_| {
                        if let Some(ref l) = l_socket_name {
                            format!("{}__{}", l, "default")
                        } else {
                            "default".to_string()
                        }
                    })
                };
                env::set_var("PSMUX_TARGET_SESSION", &session_for_routing);

                // Build the command to forward.  -s is consumed by routing; we
                // don't re-send it because the server is already this session.
                let mut server_cmd = String::from("detach-client");
                // CLI invocations have no "current attached client" to detach,
                // so we silently promote a flag-less `psmux detach-client` to
                // `-a` (detach all). With `-t` specified we leave it alone so
                // the server force-detaches just that target.
                let effective_all = detach_all || (t_target.is_none() && shell_cmd.is_none());
                if effective_all { server_cmd.push_str(" -a"); }
                if kill_parent { server_cmd.push_str(" -P"); }
                if let Some(t) = &t_target {
                    // `-t` names a CLIENT (a tty like /dev/pts/2, or %id). An
                    // unknown one must be refused, not silently ignored: tmux
                    // answers `can't find client: X` with rc 1, and before #565
                    // stopped stripping this flag the command was promoted to
                    // `-a`, so it always did SOMETHING. Without this check the
                    // flag's new correctness would come at the cost of a silent
                    // no-op, which is worse than either behaviour.
                    //
                    // Clients are listed by tty, and tty_name is derived from the
                    // client id, so `%3` and `/dev/pts/3` name the same client.
                    let wanted = match t.strip_prefix('%') {
                        Some(n) if n.chars().all(|c| c.is_ascii_digit()) => format!("/dev/pts/{}", n),
                        _ => t.clone(),
                    };
                    if let Ok(listing) = send_control_with_response("list-clients\n".to_string()) {
                        let known = listing.lines().any(|l| {
                            l.split(':').next().map(|tty| tty.trim() == wanted).unwrap_or(false)
                        });
                        if !known {
                            eprintln!("psmux: can't find client: {}", t);
                            std::process::exit(1);
                        }
                    }
                    // Quote the value so tty paths with slashes survive arg parsing.
                    server_cmd.push_str(&format!(" -t {}", crate::util::quote_arg(t)));
                }
                if let Some(c) = &shell_cmd {
                    // -E is documented but currently a no-op (we do not exec
                    // arbitrary shell commands on the server's behalf).
                    server_cmd.push_str(&format!(" -E {}", crate::util::quote_arg(c)));
                }
                server_cmd.push('\n');

                // If the target session has no port file, fall through with a
                // friendly message (matches kill-session behavior).
                if send_control(server_cmd).is_err() {
                    eprintln!("psmux: no session '{}'", session_for_routing);
                    std::process::exit(1);
                }
                return Ok(());
            }
            // kill-session - Kill a session
            "kill-session" | "kill-ses" => {
                let mut target: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                // Resolve $N session IDs via parse_target
                                let resolved = crate::cli::parse_target(t)
                                    .session
                                    .unwrap_or_else(|| t.to_string());
                                // Apply -L namespace prefix for port file lookup.
                                // A `$N` id resolves to the on disk name, which
                                // already carries the prefix, so do not add it
                                // twice: `-L ns kill-session -t $N` used to look
                                // for `ns__ns__name`, find nothing, and exit 0.
                                let namespaced = match l_socket_name {
                                    Some(ref l) if !resolved.starts_with(&format!("{}__", l)) => {
                                        format!("{}__{}", l, resolved)
                                    }
                                    _ => resolved,
                                };
                                target = Some(namespaced);
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let session_name = target.clone().unwrap_or_else(|| {
                    env::var("PSMUX_TARGET_SESSION").unwrap_or_else(|_| {
                        // Apply -L namespace prefix to default
                        if let Some(ref l) = l_socket_name {
                            format!("{}__{}", l, "default")
                        } else {
                            "default".to_string()
                        }
                    })
                });
                if let Some(ref t) = target {
                    env::set_var("PSMUX_TARGET_SESSION", t);
                }
                // Send the kill, then VERIFY the session is actually gone,
                // retrying within a deadline. psmux's loopback IPC intermittently
                // drops or defers a single command, so a one-shot send is not
                // reliable. Crucially, a mere timeout must NOT be treated as
                // "server dead" — that used to delete a live-but-busy server's
                // port file, orphaning it and triggering relaunch storms.
                let port_path = crate::paths::port_file(&session_name);
                // tmux: `kill-session -t NAME` on a name that is not a session
                // is `can't find session: NAME` at exit 1. This arm used to read
                // "no port file" as "already gone" and return success, so a
                // misspelt name, or a script killing a session it never
                // created, reported OK for a kill that did nothing.
                let shown = l_socket_name
                    .as_deref()
                    .and_then(|l| session_name.strip_prefix(&format!("{}__", l)))
                    .unwrap_or(&session_name)
                    .to_string();
                if !std::path::Path::new(&port_path).exists() {
                    eprintln!("psmux: can't find session: {}", shown);
                    std::process::exit(1);
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let was_alive = probe_session_alive(&session_name);
                if !was_alive && crate::session::registry_pid_anchor_alive(&session_name) != Some(true) {
                    // A registry with nobody behind it: the server went away and
                    // left its files. Reap them so the next `ls` does not stall
                    // on them, and tell the caller the truth, which is that
                    // there was no such session to kill.
                    crate::session::remove_session_registry(&session_name);
                    eprintln!("psmux: can't find session: {}", shown);
                    std::process::exit(1);
                }
                let mut gone = !was_alive;
                let mut refused = false;
                while !gone {
                    match send_control("kill-session\n".to_string()) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => { refused = true; break; }
                        Err(_) => {} // busy/timeout — keep trying until the deadline
                    }
                    std::thread::sleep(Duration::from_millis(150));
                    if !probe_session_alive(&session_name) { gone = true; break; }
                    if std::time::Instant::now() >= deadline { break; }
                }
                if gone || refused {
                    // Server is down (killed or actively refused) → remove the
                    // now-stale port file. (Idempotent: the server also removes
                    // its own on a clean exit.)
                    let _ = std::fs::remove_file(&port_path);
                    return Ok(());
                }
                // Still alive after the deadline → genuine failure. Exit non-zero
                // so scripts (and the watchdog) can trust $? instead of seeing a
                // false success on a command that silently did nothing.
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("kill-session: session '{}' still present after 5s", session_name),
                ));
            }
            // has-session - Check if session exists (for scripting)
            "has-session" | "has" => {
                // Get target from env (set from -t flag) or from remaining args
                let target = env::var("PSMUX_TARGET_SESSION").unwrap_or_else(|_| {
                    // Try to get session name from cmd_args
                    let mut t = "default".to_string();
                    let mut i = 1;
                    while i < cmd_args.len() {
                        if cmd_args[i].as_str() == "-t" {
                            if let Some(v) = cmd_args.get(i + 1) {
                                // Strip leading '=' prefix (tmux exact-match semantics)
                                t = v.strip_prefix('=').unwrap_or(v).to_string();
                            }
                            i += 1;
                        } else if !cmd_args[i].starts_with('-') {
                            let raw = &cmd_args[i];
                            t = raw.strip_prefix('=').unwrap_or(raw).to_string();
                            break;
                        }
                        i += 1;
                    }
                    // Apply -L namespace prefix for port file lookup
                    if let Some(ref l) = l_socket_name {
                        format!("{}__{}", l, t)
                    } else {
                        t
                    }
                });
                // Warm (standby) sessions are internal-only — treat as non-existent
                if crate::session::is_warm_session(&target) {
                    std::process::exit(1);
                }
                let path = crate::paths::port_file(&target);
                if let Ok(port_str) = std::fs::read_to_string(&path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let addr = format!("127.0.0.1:{}", port);
                        // Actually authenticate and query the server to ensure it's healthy
                        let session_key = read_session_key(&target).unwrap_or_default();
                        let conn = std::net::TcpStream::connect_timeout(
                            &addr.parse().unwrap(),
                            Duration::from_millis(500)
                        );
                        // Only prune a session that ACTIVELY refused (truly dead);
                        // a timeout means busy-but-alive and must not be deleted.
                        let refused = matches!(&conn, Err(e) if e.kind() == io::ErrorKind::ConnectionRefused);
                        if let Ok(mut s) = conn {
                            let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                            let _ = write!(s, "AUTH {}\n", session_key);
                            let _ = write!(s, "session-info\n");
                            let _ = s.flush();
                            let mut buf = [0u8; 256];
                            if let Ok(n) = std::io::Read::read(&mut s, &mut buf) {
                                if n > 0 {
                                    let resp = String::from_utf8_lossy(&buf[..n]);
                                    if resp.contains("OK") {
                                        std::process::exit(0);
                                    }
                                }
                            }
                            // Fallback: connection succeeded so session likely exists
                            std::process::exit(0);
                        } else if refused
                            || crate::session::registry_pid_anchor_alive(&target) == Some(false)
                        {
                            // Truly dead: it either refused outright, or its PID
                            // anchor names a process that is gone (a dead loopback
                            // port can time out rather than refuse on Windows).
                            // Reap the WHOLE set: dropping the `.port` alone would
                            // strand its siblings where no sweep can reach them (#530).
                            crate::session::remove_session_registry(&target);
                        }
                        // Anything else (a plain timeout against a live server that
                        // was merely slow to accept) exits 1 without touching the
                        // registry: has-session is a predicate, not a reaper (#622).
                    }
                }
                std::process::exit(1);
            }
            // rename-session - Rename a session
            "rename-session" | "rename" => {
                let mut new_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    if !cmd_args[i].starts_with('-') {
                        new_name = Some(cmd_args[i].to_string());
                        break;
                    }
                    i += 1;
                }
                if let Some(name) = new_name {
                    send_control(format!("rename-session {}\n", crate::util::quote_arg(&name)))?;
                }
                return Ok(());
            }
            // swap-pane - Swap panes
            "swap-pane" | "swapp" => {
                let mut cmd = "swap-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // resize-pane - Resize a pane
            "resize-pane" | "resizep" => {
                let mut cmd = "resize-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-L" => { cmd.push_str(" -L"); }
                        "-R" => { cmd.push_str(" -R"); }
                        "-Z" => { cmd.push_str(" -Z"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-x" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                if v.trim_end_matches('%').parse::<i32>().is_err() {
                                    eprintln!("psmux: resize-pane: -x value must be a number, got '{}'", v);
                                    std::process::exit(1);
                                }
                                cmd.push_str(&format!(" -x {}", v));
                                i += 1;
                            }
                        }
                        "-y" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                if v.trim_end_matches('%').parse::<i32>().is_err() {
                                    eprintln!("psmux: resize-pane: -y value must be a number, got '{}'", v);
                                    std::process::exit(1);
                                }
                                cmd.push_str(&format!(" -y {}", v));
                                i += 1;
                            }
                        }
                        s if s.parse::<i32>().is_ok() => {
                            cmd.push_str(&format!(" {}", s));
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // paste-buffer - Paste buffer into pane
            "paste-buffer" | "pasteb" => {
                let mut cmd = "paste-buffer".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -b {}", b));
                                i += 1;
                            }
                        }
                        "-d" => { cmd.push_str(" -d"); }
                        "-p" => { cmd.push_str(" -p"); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                // Issue #264: paste-buffer -b <missing> must error (matching
                // real tmux's "no buffer <name>"), so read the server's
                // response instead of firing-and-forgetting.
                let resp = send_control_with_response(cmd)?;
                if let Some(msg) = resp.strip_prefix("ERROR: ") {
                    eprintln!("psmux: {}", msg.trim_end());
                    std::process::exit(1);
                }
                return Ok(());
            }
            // set-buffer - Set buffer contents
            "set-buffer" | "setb" => {
                let mut buffer_name: Option<String> = None;
                let mut data: Option<String> = None;
                let mut propagate_to_clipboard = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        "-w" => { propagate_to_clipboard = true; }
                        s if !s.starts_with('-') => {
                            data = Some(s.to_string());
                        }
                        _ => {}
                    }
                    i += 1;
                }
                if propagate_to_clipboard {
                    if let Some(ref d) = data {
                        crate::clipboard::copy_to_system_clipboard(d);
                    }
                }
                let mut cmd = "set-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                // Same byte-exact wire as load-buffer: `set-buffer "a  'b'"`
                // reached the server as three bare words and came back out of
                // paste-buffer as `a b`.
                if let Some(d) = data { cmd.push_str(&format!(" -H {}", crate::util::hex_encode(d.as_bytes()))); }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // list-buffers - List paste buffers
            "list-buffers" | "lsb" => {
                let mut format_str: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-F" => {
                            if let Some(f) = cmd_args.get(i + 1) {
                                format_str = Some(f.to_string());
                                i += 1;
                            }
                        }
                        s if s.starts_with("-F") && s.len() > 2 => {
                            format_str = Some(s[2..].to_string());
                        }
                        "-t" => { i += 1; } // skip target
                        _ => {}
                    }
                    i += 1;
                }
                let cmd = if let Some(fmt) = format_str {
                    format!("list-buffers -F {}\n", fmt)
                } else {
                    "list-buffers\n".to_string()
                };
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // show-buffer - Show buffer contents
            "show-buffer" | "showb" => {
                let mut buffer_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let mut cmd = "show-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // delete-buffer - Delete a paste buffer
            "delete-buffer" | "deleteb" => {
                let mut buffer_name: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                let mut cmd = "delete-buffer".to_string();
                if let Some(b) = buffer_name { cmd.push_str(&format!(" -b {}", b)); }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // display-message - Display a message
            "display-message" | "display" => {
                // A target containing "::" is always malformed (a valid target is
                // session:window.pane with single colons), so reject it with a
                // nonzero exit rather than silently resolving to the active session.
                if let Ok(full) = std::env::var("PSMUX_TARGET_FULL") {
                    if full.contains("::") {
                        eprintln!("psmux: bad target: {}", full);
                        std::process::exit(1);
                    }
                }
                let mut message: Vec<String> = Vec::new();
                let mut target: Option<String> = None;
                let mut print_to_stdout = false;
                let mut duration_ms: Option<u64> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                target = Some(t.to_string());
                                i += 1;
                            }
                        }
                        "-p" => { print_to_stdout = true; }
                        "-d" => {
                            if let Some(val) = cmd_args.get(i + 1) {
                                duration_ms = val.parse::<u64>().ok();
                            }
                            i += 1;
                        }
                        "-I" => { i += 1; } // consume -I <input>, skip value
                        // POSIX cluster ending in value-taking flag: -pt <target>
                        a if a.len() > 2
                            && a.starts_with('-')
                            && !a.starts_with("--")
                            && {
                                let last = a.chars().last().unwrap_or(' ');
                                matches!(last, 't' | 'd' | 'I')
                                    && a[1..a.len()-1].chars().all(|c| matches!(c, 'p'))
                            } =>
                        {
                            let last = a.chars().last().unwrap();
                            if a.contains('p') { print_to_stdout = true; }
                            if let Some(val) = cmd_args.get(i + 1) {
                                match last {
                                    't' => { target = Some(val.to_string()); }
                                    'd' => { duration_ms = val.parse::<u64>().ok(); }
                                    _ => {}
                                }
                                i += 1;
                            }
                        }
                        s => { message.push(s.to_string()); }
                    }
                    i += 1;
                }
                let msg = message.join(" ");
                let mut cmd = "display-message".to_string();
                if let Some(t) = target { cmd.push_str(&format!(" -t {}", t)); }
                if print_to_stdout { cmd.push_str(" -p"); }
                if let Some(d) = duration_ms { cmd.push_str(&format!(" -d {}", d)); }
                // Quote the message to preserve literal whitespace (tabs etc)
                // that would otherwise be split by the server's command parser.
                cmd.push_str(&format!(" {}", crate::util::quote_arg(&msg)));
                cmd.push('\n');
                if print_to_stdout {
                    let resp = send_control_with_response(cmd)?;
                    print!("{}", resp);
                } else {
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // run-shell - Run a shell command
            "run-shell" | "run" => {
                let mut cmd_to_run: Vec<String> = Vec::new();
                let mut background = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => { background = true; }
                        s => { cmd_to_run.push(s.to_string()); }
                    }
                    i += 1;
                }
                let shell_cmd_str = cmd_to_run.join(" ");
                if shell_cmd_str.trim().is_empty() {
                    eprintln!("usage: run-shell [-b] shell-command");
                    std::process::exit(1);
                }
                // `#{...}` can only be resolved against live server state, which
                // this process does not have — the CLI path runs the command
                // itself rather than going through the server. So when the
                // command references a format variable, hand the whole thing to
                // the server and let it run there (connection.rs expands, then
                // executes). Without this, `psmux run-shell "x #{pane_id}"`
                // passed the helper that literal text, exactly as the bind path
                // used to.
                if shell_cmd_str.contains("#{") {
                    let mut line = String::from("run-shell");
                    if background {
                        line.push_str(" -b");
                    }
                    line.push(' ');
                    line.push_str(&shell_cmd_str);
                    line.push('\n');
                    match crate::session::send_control_with_response(line) {
                        Ok(resp) => {
                            if !resp.is_empty() {
                                io::stdout().write_all(resp.as_bytes())?;
                            }
                            return Ok(());
                        }
                        // No server reachable: fall through and run locally with
                        // the format text unexpanded. That is the pre-existing
                        // behaviour, and it beats refusing to run at all.
                        Err(e) => {
                            eprintln!("run-shell: {} (running without format expansion)", e);
                        }
                    }
                }
                let shell_cmd = crate::util::expand_run_shell_path(&shell_cmd_str);
                // Run the command using the resolved shell
                if background {
                    let mut c = crate::commands::build_run_shell_command(&shell_cmd);
                    // Report a failure to START the command. `-b` waives the
                    // output, not the error.
                    if let Err(e) = c.spawn() {
                        eprintln!("run-shell: {}: {}", shell_cmd, e);
                        std::process::exit(1);
                    }
                } else {
                    let mut c = crate::commands::build_run_shell_command(&shell_cmd);
                    let output = c.output()?;
                    io::stdout().write_all(&output.stdout)?;
                    io::stderr().write_all(&output.stderr)?;
                    std::process::exit(output.status.code().unwrap_or(0));
                }
                return Ok(());
            }
            // respawn-pane - Restart the pane's process
            "respawn-pane" | "respawnp" | "resp" => {
                let mut cmd = "respawn-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-k" => { cmd.push_str(" -k"); }
                        "-c" => {
                            if let Some(d) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -c {}", d));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => { cmd.push_str(&format!(" {}", cmd_args[i])); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // last-window - Select last used window
            "last-window" | "last" => {
                send_control("last-window\n".to_string())?;
                return Ok(());
            }
            // last-pane - Select last used pane
            "last-pane" | "lastp" => {
                send_control("last-pane\n".to_string())?;
                return Ok(());
            }
            // next-window - Move to next window
            "next-window" | "next" => {
                send_control("next-window\n".to_string())?;
                return Ok(());
            }
            // previous-window - Move to previous window
            "previous-window" | "prev" => {
                send_control("previous-window\n".to_string())?;
                return Ok(());
            }
            // rotate-window - Rotate panes in window
            "rotate-window" | "rotatew" => {
                let mut cmd = "rotate-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-D" => { cmd.push_str(" -D"); }
                        "-U" => { cmd.push_str(" -U"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // display-panes - Show pane numbers
            "display-panes" | "displayp" => {
                send_control("display-panes\n".to_string())?;
                return Ok(());
            }
            // break-pane - Break pane out to a new window
            "break-pane" | "breakp" => {
                let mut cmd = "break-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-d" => { cmd.push_str(" -d"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // join-pane - Join a pane to another window (or across sessions)
            "join-pane" | "joinp" | "move-pane" | "movep" => {
                // Parse args to detect cross-session scenario
                // Note: -t is stripped from cmd_args by the global handler above,
                // but preserved in PSMUX_TARGET_FULL env var.
                let mut source_spec = String::new();
                let mut horizontal = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-h" => horizontal = true,
                        "-v" => {} // vertical is default
                        "-d" => {} // detach (ignored at CLI level)
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                source_spec = t.to_string();
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                // Get -t from the saved env var (global handler stripped it from cmd_args)
                let target_spec = std::env::var("PSMUX_TARGET_FULL").unwrap_or_default();
                // Check if source and target reference different sessions
                let src_session = if source_spec.contains(':') {
                    source_spec.split(':').next().unwrap_or("").to_string()
                } else {
                    String::new()
                };
                let tgt_session = if target_spec.contains(':') {
                    target_spec.split(':').next().unwrap_or("").to_string()
                } else {
                    String::new()
                };
                let current_session = std::env::var("PSMUX_TARGET_SESSION")
                    .or_else(|_| std::env::var("PSMUX_SESSION"))
                    .unwrap_or_default();
                let effective_src = if src_session.is_empty() { current_session.clone() } else { src_session.clone() };
                let effective_tgt = if tgt_session.is_empty() { current_session.clone() } else { tgt_session.clone() };
                // tmux parity: require at least one of -s or -t. Reject empty invocation.
                if source_spec.is_empty() && target_spec.is_empty() {
                    eprintln!("psmux: usage: join-pane [-bdhv] [-l size | -p percentage] [-s src-pane] [-t dst-pane]");
                    std::process::exit(1);
                }
                // tmux parity: src and target panes must be in different windows.
                // Detect same-session same-window case and reject (matches tmux's
                // "source and target panes must be different" error).
                let src_after_colon_check = if source_spec.contains(':') {
                    source_spec.split(':').nth(1).unwrap_or("")
                } else { source_spec.as_str() };
                let tgt_after_colon_check = if target_spec.contains(':') {
                    target_spec.split(':').nth(1).unwrap_or("")
                } else { target_spec.as_str() };
                let same_session = effective_src == effective_tgt && !effective_src.is_empty();
                if same_session && !src_after_colon_check.is_empty() && !tgt_after_colon_check.is_empty() {
                    // Prefix with ':' so parse_target reads "0.2" as window=0,pane=2
                    // (a bare "0.2" is otherwise read as session="0", pane=2).
                    let sp_chk = crate::cli::parse_target(&format!(":{}", src_after_colon_check));
                    let tp_chk = crate::cli::parse_target(&format!(":{}", tgt_after_colon_check));
                    if let (Some(sw), Some(tw)) = (sp_chk.window, tp_chk.window) {
                        if sw == tw {
                            eprintln!("psmux: can't join a pane to its own window");
                            std::process::exit(1);
                        }
                    }
                }
                if !effective_src.is_empty() && !effective_tgt.is_empty() && effective_src != effective_tgt {
                    // Cross-session join-pane: orchestrate via TCP
                    let src_after_colon = if source_spec.contains(':') {
                        source_spec.split(':').nth(1).unwrap_or("0.0")
                    } else if !source_spec.is_empty() {
                        &source_spec
                    } else {
                        "0.0"
                    };
                    let tgt_after_colon = if target_spec.contains(':') {
                        target_spec.split(':').nth(1).unwrap_or("")
                    } else if !target_spec.is_empty() {
                        &target_spec
                    } else {
                        ""
                    };
                    let sp = crate::cli::parse_target(src_after_colon);
                    let tp = crate::cli::parse_target(tgt_after_colon);
                    match crate::cross_session::orchestrate_cross_session_join(
                        &effective_src,
                        sp.window.unwrap_or(0),
                        sp.pane.unwrap_or(0),
                        &effective_tgt,
                        tp.window,
                        tp.pane,
                        horizontal,
                    ) {
                        Ok(()) => {}
                        Err(e) => {
                            eprintln!("psmux: cross-session join-pane failed: {}", e);
                            std::process::exit(1);
                        }
                    }
                } else {
                    // Same-session join-pane: forward to server as before
                    let mut cmd = "join-pane".to_string();
                    if horizontal { cmd.push_str(" -h"); }
                    if !source_spec.is_empty() { cmd.push_str(&format!(" -s {}", source_spec)); }
                    if !target_spec.is_empty() { cmd.push_str(&format!(" -t {}", target_spec)); }
                    cmd.push('\n');
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // rename-window - Rename current window
            "rename-window" | "renamew" => {
                // cmd_args[0] is the command, cmd_args[1] should be the new name
                if let Some(name) = cmd_args.get(1) {
                    if !name.starts_with('-') {
                        send_control(format!("rename-window {}\n", crate::util::quote_arg(name)))?;
                    }
                }
                return Ok(());
            }
            // zoom-pane - Toggle pane zoom
            "zoom-pane" | "resizep -Z" => {
                send_control("zoom-pane\n".to_string())?;
                return Ok(());
            }
            // source-file - Load a configuration file
            "source-file" | "source" => {
                let mut quiet = false;
                let mut file_path: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-q" => { quiet = true; }
                        "-n" => { /* parse only, don't execute */ }
                        "-v" => { /* verbose */ }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    // Expand ~ to home directory
                    let expanded = if path.starts_with('~') {
                        let home = env::var("USERPROFILE").or_else(|_| env::var("HOME")).unwrap_or_default();
                        path.replacen('~', &home, 1)
                    } else {
                        path
                    };
                    if let Err(e) = std::fs::read_to_string(&expanded) {
                        if !quiet {
                            eprintln!("psmux: {}: {}", expanded, e);
                            std::process::exit(1);
                        }
                    } else {
                        // Send source-file command to server if attached
                        send_control(format!("source-file {}\n", crate::util::quote_arg(&expanded)))?;
                    }
                }
                return Ok(());
            }
            // list-keys - List all key bindings
            "list-keys" | "lsk" => {
                let mut table_filter: Option<String> = None;
                let mut key_filter: Option<String> = None;
                let mut cmd = "list-keys".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-T" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                table_filter = Some(t.to_string());
                                cmd.push_str(&format!(" -T {}", t));
                                i += 1;
                            }
                        }
                        "-t" => { i += 1; } // target handled globally
                        arg if !arg.starts_with('-') => {
                            // Positional: key name to filter
                            if key_filter.is_none() {
                                key_filter = Some(arg.to_string());
                            }
                            cmd.push_str(&format!(" {}", arg));
                        }
                        _ => { cmd.push_str(&format!(" {}", cmd_args[i])); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                match send_control_with_response(cmd) {
                    Ok(resp) => { print!("{}", resp); }
                    Err(_) => {
                        // No running server — emit built-in defaults filtered by -T and key.
                        // Real tmux supports this without a server for the prefix table.
                        let table = table_filter.as_deref().unwrap_or("prefix");
                        if table == "prefix" || table_filter.is_none() {
                            for (key, action) in crate::help::PREFIX_DEFAULTS {
                                if let Some(ref kf) = key_filter {
                                    if *key != kf.as_str() { continue; }
                                }
                                println!("bind-key -T prefix {} {}", key, action);
                            }
                        }
                        if table == "root" || table_filter.is_none() {
                            for (key, action) in crate::help::ROOT_DEFAULTS {
                                if let Some(ref kf) = key_filter {
                                    if *key != kf.as_str() { continue; }
                                }
                                println!("bind-key -T root {} {}", key, action);
                            }
                        }
                    }
                }
                return Ok(());
            }
            // bind-key - Bind a key to a command
            "bind-key" | "bind" => {
                reject_unknown_key_name(&cmd_args, "bind-key");
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; bind-key will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // unbind-key - Unbind a key
            "unbind-key" | "unbind" => {
                reject_unknown_key_name(&cmd_args, "unbind-key");
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; unbind-key will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // set-option / set / set-window-option / setw - Set an option
            "set-option" | "set" | "set-window-option" | "setw" => {
                // Validate that known integer-valued options receive a numeric value,
                // erroring (nonzero exit) like tmux instead of silently accepting junk.
                let (cli_pane_scope, cli_only_if_unset) = {
                    // NOTE: "lock-after-time" is intentionally excluded. Unlike the
                    // options below, psmux has no real numeric business logic for it
                    // anywhere server-side -- config.rs stores it as an opaque
                    // user_options passthrough (locking isn't implemented), so gating
                    // it here just made the CLI reject values the server itself
                    // accepts unchecked, breaking round-trip (task #7 batch A bug 1).
                    const INT_OPTS: &[&str] = &[
                        "history-limit", "escape-time", "display-time", "display-panes-time",
                        "repeat-time", "message-limit", "status-interval", "base-index",
                        "pane-base-index", "status-left-length", "status-right-length",
                        "history-file-limit",
                    ];
                    // Collect positional (non-flag) args, skipping -t/-p values.
                    // `@user-options` start with '@', not '-', so they are
                    // positionals; an explicit empty string ("") is a real
                    // value and must stay in the list (tmux accepts
                    // `set -g @foo ""`).
                    let mut positionals: Vec<&str> = Vec::new();
                    let mut flags = String::new();
                    let mut j = 1;
                    while j < cmd_args.len() {
                        let a = cmd_args[j].as_str();
                        // Flags parse only BEFORE the first positional, and
                        // `--` ends option parsing entirely (#583, the
                        // set-option sibling of the #562 send-keys fix). tmux
                        // (getopt) stops scanning at the option name, so a
                        // dash-leading VALUE is data, never a flag: `set @k
                        // -u` stores the literal "-u" instead of consuming it
                        // as the unset flag and deleting the key at rc 0.
                        if positionals.is_empty() {
                            if a == "--" {
                                positionals.extend(cmd_args[j + 1..].iter().map(|s| s.as_str()));
                                break;
                            }
                            // Only -t consumes a value. `-p` is a bare pane-scope
                            // flag (tmux parity, #580); treating it as a target
                            // flag ate the option name, so `set -p -t %3
                            // remain-on-exit failed` misparsed as an empty-value
                            // set of 'failed'.
                            if a == "-t" { j += 2; continue; }
                            if a.starts_with('-') && a.len() > 1 {
                                flags.push_str(&a[1..]);
                                j += 1;
                                continue;
                            }
                        }
                        positionals.push(a);
                        j += 1;
                    }
                    // Issue #553: reject flags psmux does not implement
                    // instead of silently accepting them and letting the
                    // write land under different semantics than requested
                    // (tmux: "unknown flag -Z", rc 1, nothing written). Same
                    // principle as dd84b97 for refresh-client. -t/-p never
                    // reach `flags` (skipped with their values above); -U is
                    // the unset alias, -w a scope flag.
                    //
                    // Issue #618: -s is the SERVER scope flag. tmux 3.2 moved
                    // default-terminal, extended-keys and friends onto the
                    // server option table and tools write them the documented
                    // way (`set-option -s default-terminal xterm-256color`).
                    // Rejecting it turned every such bootstrap into a hard
                    // failure. psmux runs one server per session and keeps a
                    // single option store, so -s selects the same store as -g
                    // rather than a genuinely cross-session one; the write
                    // lands where the caller expects it, but it is not true
                    // cross-session server-option storage.
                    for ch in flags.chars() {
                        if !SET_OPTION_CLI_FLAGS.contains(ch) {
                            eprintln!("psmux: set-option: unknown flag -{}", ch);
                            std::process::exit(1);
                        }
                    }
                    let has_unset = flags.contains('u') || flags.contains('U');
                    let has_append = flags.contains('a');
                    // Issue #535: a set-option carrying no value used to be
                    // dropped in silence: nothing set, empty stderr, exit 0.
                    // That turned a one-character mistake (PowerShell eats a
                    // bare `@name` as the splatting operator, so `set -g
                    // @pill $undefined` arrives as `set -g <text>`) into an
                    // undebuggable no-op. tmux fails these loudly, so we do
                    // too. `-q` is NOT consulted: both tmux's manual and our
                    // own -q help text scope it to unknown/ambiguous options,
                    // and tmux 3.4 still errors on `set -gq @foo`.
                    if positionals.is_empty() {
                        eprintln!("psmux: set-option: too few arguments (need at least 1)");
                        std::process::exit(1);
                    }
                    if positionals.len() == 1 && !has_unset {
                        let name = positionals[0];
                        // Boolean flags legitimately take no value: they
                        // toggle (tmux parity, #278). Everything else is an
                        // error. `-a` appends, so it always needs a value.
                        if has_append || !crate::server::options::missing_value_toggles(name) {
                            eprintln!("psmux: set-option: empty value for '{}'", name);
                            std::process::exit(1);
                        }
                    }
                    if let (Some(name), Some(val)) = (positionals.first(), positionals.get(1)) {
                        if INT_OPTS.contains(name) && val.parse::<i64>().is_err() {
                            eprintln!("psmux: set-option: value for '{}' must be a number, got '{}'", name, val);
                            std::process::exit(1);
                        }
                        // Issue #606: repeat-time is bounded in tmux
                        // (options-table.c: 0..=2000000 ms). Without this the
                        // number check above waved through 2000001 and -5
                        // alike: the first installed a 33 minute repeat
                        // window, the second was dropped server-side at exit
                        // 0 so a typo looked like it had been applied.
                        // #608: `priority` takes a fixed set of three values.
                        // The generic catalog check only validates numbers and
                        // booleans, so without this a typo returned 0 and was
                        // dropped server-side, leaving the class unchanged and
                        // the mistake invisible.
                        if *name == "priority"
                            && crate::platform::normalize_priority(val).is_none()
                        {
                            eprintln!(
                                "psmux: set-option: value for 'priority' must be one of {}, got '{}'",
                                crate::platform::PRIORITY_VALUES.join(", "),
                                val
                            );
                            std::process::exit(1);
                        }
                        if *name == "repeat-time" {
                            if let Ok(ms) = val.parse::<i64>() {
                                if ms < 0 {
                                    eprintln!("psmux: set-option: value is too small: {}", val);
                                    std::process::exit(1);
                                }
                                if ms > crate::server::options::REPEAT_TIME_MAX_MS {
                                    eprintln!("psmux: set-option: value is too large: {}", val);
                                    std::process::exit(1);
                                }
                            }
                        }
                    }
                    // `-o` on an option that is already set is an error in tmux
                    // (`already set: <name>`, exit 1) and silent at exit 0 only
                    // under `-q`, so the no-q case has to read the server's
                    // answer instead of firing and forgetting (#619 follow up).
                    // `-u` disarms the guard entirely: tmux skips it whenever
                    // `-u` is present, so that stays a plain unset.
                    let only_if_unset =
                        flags.contains('o') && !has_unset && !flags.contains('q');
                    (flags.contains('p'), only_if_unset)
                };
                let cmd_str: String = cmd_args.iter().map(|s| {
                    let s = s.as_str();
                    // An explicitly empty argument must be re-quoted, or it
                    // collapses into the joining whitespace and the server
                    // re-splits one positional short, so `set -g @foo ""`
                    // (tmux: clear the option) silently kept the old value.
                    // parse_command_line already preserves a quoted empty
                    // token, see #177.
                    //
                    // The test is any Unicode whitespace, not just an ASCII
                    // space: the server re-splits on char::is_whitespace(), so
                    // a value whose only separators were NBSPs used to arrive
                    // unquoted, get re-split, and come back collapsed AND
                    // rewritten to ASCII spaces. Whether a value survived
                    // depended on whether it happened to contain an ASCII
                    // space (#536).
                    if s.is_empty() || s.chars().any(char::is_whitespace) || s.contains('"') || s.contains('\'') {
                        // quote_arg escapes `\` as well as `"` to match what
                        // parse_command_line decodes inside double quotes;
                        // escaping only the quote collapsed every `\\` and a
                        // trailing `\` swallowed the following args (#547).
                        // Quote-bearing values must be wire-quoted too or the
                        // re-tokenizer strips the quote chars (`a"b` -> `ab`).
                        crate::util::quote_arg(s)
                    } else {
                        s.to_string()
                    }
                }).collect::<Vec<String>>().join(" ");
                // Pane scope (#580): the server validates the option and the
                // target pane and answers "ERROR: ..." on refusal. A silent
                // fire-and-forget here would recreate the exit-0 silent no-op
                // this flag's support exists to end, so read the reply.
                // Detected in the flag region above; a "-p" in the VALUE
                // position is data, not a scope flag (#583).
                let pane_scope = cli_pane_scope;
                if pane_scope {
                    let resp = send_control_with_response(format!("{}\n", cmd_str))?;
                    if resp.trim_start().starts_with("ERROR") {
                        eprintln!("psmux: {}", resp.trim_start().trim_start_matches("ERROR:").trim());
                        std::process::exit(1);
                    }
                    return Ok(());
                }
                // `set-option -o` without `-q`: the server answers "" when the
                // write landed and "ERROR: already set: <name>" when it
                // refused, which tmux reports on stderr at exit 1. Without
                // this the refusal was a silent exit 0 with empty output, so a
                // script seeding defaults could not tell "I set it" from "the
                // user already had it" (#619 follow up).
                if cli_only_if_unset {
                    match send_control_with_response(format!("{}\n", cmd_str)) {
                        Ok(resp) => {
                            let t = resp.trim();
                            if t.starts_with("ERROR") {
                                // tmux prints the bare cmdq_error text.
                                eprintln!("{}", t.trim_start_matches("ERROR:").trim());
                                std::process::exit(1);
                            }
                        }
                        Err(e) if e.to_string().contains("no session")
                            || e.to_string().contains("no server running") => {
                            eprintln!("warning: no active session; option will take effect when set inside a session or via config file");
                        }
                        Err(e) => return Err(e),
                    }
                    return Ok(());
                }
                match send_control(format!("{}\n", cmd_str)) {
                    Ok(()) => {},
                    Err(e) if e.to_string().contains("no session") => {
                        eprintln!("warning: no active session; option will take effect when set inside a session or via config file");
                    },
                    Err(e) => return Err(e),
                }
                return Ok(());
            }
            // show-options / show / show-window-options / showw - Show options
            // `show-option` / `show-window-option`: tmux resolves unambiguous
            // command-name prefixes, so the singular spellings work there and
            // tools type them (LazyVim probes `show-option -qvg ...`, #586).
            // The control-mode dispatcher already accepted them; the CLI and
            // TCP paths did not.
            "show-options" | "show" | "show-window-options" | "showw"
            | "show-option" | "show-window-option" => {
                // Issue #553: reject flags psmux does not implement instead
                // of silently accepting them (tmux: "unknown flag -Z", rc 1).
                // Accepted: -A -g -q -s -v -w plus -t <target>. Option names
                // (@x, status-style) never start with '-'.
                {
                    let mut j = 1;
                    while j < cmd_args.len() {
                        let a = cmd_args[j].as_str();
                        // `--` ends option parsing: whatever follows is the
                        // option name, dashes and all (#583; tmux accepts
                        // `show -qv -t S -- @k` at rc 0).
                        if a == "--" { break; }
                        if a == "-t" { j += 2; continue; }
                        if a.starts_with('-') && a.len() > 1 {
                            for ch in a[1..].chars() {
                                // 'p' = pane scope (#580), forwarded as-is.
                                // 's' = server scope (#618); the server narrows
                                // a bare listing to the server options.
                                if !SHOW_OPTIONS_CLI_FLAGS.contains(ch) {
                                    eprintln!("psmux: show-options: unknown flag -{}", ch);
                                    std::process::exit(1);
                                }
                            }
                        }
                        j += 1;
                    }
                }
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                let resp = send_control_with_response(format!("{}\n", cmd_str))?;
                // A refusal (e.g. show-options -p on a nonexistent pane, #580)
                // must not masquerade as an option listing at exit 0 (#559
                // family).
                if resp.trim_start().starts_with("ERROR") {
                    eprintln!("psmux: {}", resp.trim_start().trim_start_matches("ERROR:").trim());
                    std::process::exit(1);
                }
                print!("{}", resp);
                return Ok(());
            }
            // if-shell - Conditional execution
            "if-shell" | "if" => {
                let mut background = false;
                let mut condition: Option<String> = None;
                let mut cmd_true: Option<String> = None;
                let mut cmd_false: Option<String> = None;
                let mut format_mode = false;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => { background = true; }
                        "-F" => { format_mode = true; }
                        "-t" => { i += 1; } // Skip target
                        s if !s.starts_with('-') => {
                            if condition.is_none() {
                                condition = Some(s.to_string());
                            } else if cmd_true.is_none() {
                                cmd_true = Some(s.to_string());
                            } else if cmd_false.is_none() {
                                cmd_false = Some(s.to_string());
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                
                if let (Some(cond), Some(true_cmd)) = (condition, cmd_true) {
                    if background && !format_mode {
                        // -b flag: run the condition check in a background thread
                        // and dispatch the result command asynchronously (like tmux)
                        let cmd_false_bg = cmd_false.clone();
                        std::thread::spawn(move || {
                            let success = {
                                let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
                                let mut c = std::process::Command::new(&shell_prog);
                                for a in &shell_args { c.arg(a); }
                                c.arg(&cond);
                                c.stdout(std::process::Stdio::null());
                                c.stderr(std::process::Stdio::null());
                                { use crate::platform::HideWindowCommandExt; c.hide_window(); }
                                c.status().map(|s| s.success()).unwrap_or(false)
                            };
                            let cmd_to_run = if success { Some(true_cmd) } else { cmd_false_bg };
                            if let Some(cmd) = cmd_to_run {
                                let tcp_cmd = format!("{}\n", cmd);
                                let _ = send_control_with_response(tcp_cmd);
                            }
                        });
                        // Return immediately — condition runs in background
                        return Ok(());
                    }

                    let success = if format_mode {
                        // Expand format string via server before evaluating
                        let fmt_cmd = format!("display-message -p {}\n", crate::util::quote_arg(&cond));
                        let expanded = send_control_with_response(fmt_cmd).unwrap_or_default();
                        let expanded = expanded.trim_end_matches('\n');
                        !expanded.is_empty() && expanded != "0"
                    } else if cond == "true" || cond == "1" {
                        true
                    } else if cond == "false" || cond == "0" {
                        false
                    } else {
                        // Run shell command - suppress stdout/stderr so it doesn't leak to terminal
                        {
                            let (shell_prog, shell_args) = crate::commands::resolve_run_shell();
                            let mut c = std::process::Command::new(&shell_prog);
                            for a in &shell_args { c.arg(a); }
                            c.arg(&cond);
                            c.stdout(std::process::Stdio::null());
                            c.stderr(std::process::Stdio::null());
                            { use crate::platform::HideWindowCommandExt; c.hide_window(); }
                            c.status().map(|s| s.success()).unwrap_or(false)
                        }
                    };
                    
                    let cmd_to_run = if success { Some(true_cmd) } else { cmd_false };
                    
                    if let Some(cmd) = cmd_to_run {
                        // Re-quote multi-word arguments for TCP transport
                        let needs_quoting = cmd.contains(' ');
                        let tcp_cmd = if needs_quoting {
                            // The command string may contain spaces (e.g. "display-message -p hello")
                            // Send it as-is since it's already a full command line
                            format!("{}\n", cmd)
                        } else {
                            format!("{}\n", cmd)
                        };
                        // Use send_control_with_response to capture any output from the chosen command
                        let resp = send_control_with_response(tcp_cmd)?;
                        if !resp.is_empty() {
                            print!("{}", resp);
                        }
                    }
                }
                return Ok(());
            }
            // wait-for - Wait for a signal
            "wait-for" | "wait" => {
                let mut lock = false;
                let mut signal = false;
                let mut unlock = false;
                let mut channel: Option<String> = None;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-L" => { lock = true; }
                        "-S" => { signal = true; }
                        "-U" => { unlock = true; }
                        s if !s.starts_with('-') => { channel = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                
                if let Some(ch) = channel {
                    if signal {
                        send_control(format!("wait-for -S {}\n", ch))?;
                    } else if lock {
                        send_control(format!("wait-for -L {}\n", ch))?;
                    } else if unlock {
                        send_control(format!("wait-for -U {}\n", ch))?;
                    } else {
                        // Wait for channel - this blocks
                        let resp = send_control_with_response(format!("wait-for {}\n", ch))?;
                        if !resp.is_empty() {
                            print!("{}", resp);
                        }
                    }
                }
                return Ok(());
            }
            // select-layout - Select a layout for the window
            "select-layout" | "selectl" => {
                let mut layout: Option<String> = None;
                let mut next = false;
                let mut prev = false;
                let mut i = 1;
                
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-n" => { next = true; }
                        "-p" => { prev = true; }
                        "-o" => { /* last layout */ }
                        "-E" => { /* spread evenly */ }
                        "-t" => { i += 1; } // Skip target
                        s if !s.starts_with('-') => { layout = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                
                if next {
                    send_control("next-layout\n".to_string())?;
                } else if prev {
                    send_control("previous-layout\n".to_string())?;
                } else if let Some(l) = layout {
                    send_control(format!("select-layout {}\n", l))?;
                } else {
                    send_control("select-layout\n".to_string())?;
                }
                return Ok(());
            }
            // move-window - Move a window
            "move-window" | "movew" => {
                let mut cmd = "move-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { cmd.push_str(" -a"); }
                        "-b" => { cmd.push_str(" -b"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-k" => { cmd.push_str(" -k"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { cmd.push_str(&format!(" {}", s)); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                // #602: move-window was fire-and-forget, so "index in use: 1"
                // and an unresolvable -s both exited 0 with nothing done. The
                // server now answers; surface it the way swap-window does.
                let resp = send_control_with_response(cmd)?;
                if !resp.trim().is_empty() {
                    eprint!("{}", resp);
                    std::process::exit(1);
                }
                return Ok(());
            }
            // swap-window - Swap windows
            "swap-window" | "swapw" => {
                let mut cmd = "swap-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-d" => { cmd.push_str(" -d"); }
                        "-s" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -s {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { cmd.push_str(&format!(" {}", s)); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                // #559: swap-window with an unresolvable -s/-t silently did
                // nothing and exited 0. The server now validates both windows
                // and answers "ERROR: can't find window" — surface it.
                let resp = send_control_with_response(cmd)?;
                if !resp.trim().is_empty() {
                    eprint!("{}", resp);
                    std::process::exit(1);
                }
                return Ok(());
            }
            // list-clients - List all clients
            "list-clients" | "lsc" => {
                let resp = send_control_with_response("list-clients\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // switch-client - Switch the current client to another session
            "switch-client" | "switchc" => {
                let mut cmd = "switch-client".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-l" => { cmd.push_str(" -l"); }
                        "-n" => { cmd.push_str(" -n"); }
                        "-p" => { cmd.push_str(" -p"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-c" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -c {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                // #483: the server validates a -t target and replies "ERROR
                // <reason>" for an unresolvable window/pane/session so scripts
                // see a non-zero exit instead of a silent success. A missing/
                // unreachable server keeps the old fire-and-forget behavior.
                match send_control_with_response(cmd) {
                    Ok(resp) => {
                        if let Some(reason) = resp.trim().strip_prefix("ERROR ") {
                            eprintln!("{}", reason);
                            std::process::exit(1);
                        }
                    }
                    Err(_) => {}
                }
                return Ok(());
            }
            // copy-mode - Enter copy mode
            "copy-mode" => {
                let mut cmd = "copy-mode".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-u" => { cmd.push_str(" -u"); }
                        "-d" => { cmd.push_str(" -d"); }
                        "-e" => { cmd.push_str(" -e"); }
                        "-H" => { cmd.push_str(" -H"); }
                        "-q" => { cmd.push_str(" -q"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // clock-mode - Display a clock
            "clock-mode" => {
                send_control("clock-mode\n".to_string())?;
                return Ok(());
            }
            // choose-buffer - List paste buffers interactively
            "choose-buffer" | "chooseb" => {
                let resp = send_control_with_response("choose-buffer\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // set-environment / setenv - Set environment variable
            "set-environment" | "setenv" => {
                let mut cmd = "set-environment".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-g" => { cmd.push_str(" -g"); }
                        "-r" => { cmd.push_str(" -r"); }
                        "-u" => { cmd.push_str(" -u"); }
                        "-h" => { cmd.push_str(" -h"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s => { cmd.push_str(&format!(" {}", s)); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // show-environment / showenv - Show environment variables
            "show-environment" | "showenv" => {
                let mut cmd = "show-environment".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-g" => { cmd.push_str(" -g"); }
                        "-s" => { cmd.push_str(" -s"); }
                        "-h" => { cmd.push_str(" -h"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        s if !s.starts_with('-') => { cmd.push_str(&format!(" {}", s)); }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                print!("{}", resp);
                return Ok(());
            }
            // load-buffer - Load a paste buffer from a file
            "load-buffer" | "loadb" => {
                let mut buffer_name: Option<String> = None;
                let mut file_path: Option<String> = None;
                let mut propagate_to_clipboard = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        // tmux 3.2+: forward the loaded buffer to the outer
                        // terminal's system clipboard. Real tmux does this
                        // via OSC 52 to the host terminal; on Windows we
                        // have direct access to the Win32 clipboard, so
                        // just write to it. Failures are non-fatal
                        // (matches tmux's permissive behavior).
                        "-w" => { propagate_to_clipboard = true; }
                        "-" => { file_path = Some("-".to_string()); }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    let content = if path == "-" {
                        let mut input = String::new();
                        io::stdin().read_to_string(&mut input)?;
                        input
                    } else {
                        std::fs::read_to_string(&path)?
                    };
                    if propagate_to_clipboard {
                        crate::clipboard::copy_to_system_clipboard(&content);
                    }
                    let mut cmd = "set-buffer".to_string();
                    if let Some(b) = buffer_name {
                        cmd.push_str(&format!(" -b {}", b));
                    }
                    // Byte-exact transport (see `set-buffer -H` in the server).
                    // The content used to travel as bare words with newlines
                    // escaped to a literal `\n`, so the server's tokenizer ate
                    // it: quote grouping was stripped, tabs and runs of spaces
                    // collapsed to one space, a `;` split the line into two
                    // commands, and the `\n` two-char escape was never undone.
                    // `paste-buffer` then faithfully pasted the damaged text —
                    // `printf '%s' 'ARG'` arrived as `printf %s ARG`.
                    cmd.push_str(&format!(" -H {}", crate::util::hex_encode(content.as_bytes())));
                    cmd.push('\n');
                    send_control(cmd)?;
                }
                return Ok(());
            }
            // save-buffer - Save a paste buffer to a file
            "save-buffer" | "saveb" => {
                let mut buffer_name: Option<String> = None;
                let mut file_path: Option<String> = None;
                let mut append = false;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-a" => { append = true; }
                        "-b" => {
                            if let Some(b) = cmd_args.get(i + 1) {
                                buffer_name = Some(b.to_string());
                                i += 1;
                            }
                        }
                        "-" => { file_path = Some("-".to_string()); }
                        s if !s.starts_with('-') => { file_path = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(path) = file_path {
                    let mut cmd = "show-buffer".to_string();
                    if let Some(b) = buffer_name {
                        cmd.push_str(&format!(" -b {}", b));
                    }
                    cmd.push('\n');
                    let content = send_control_with_response(cmd)?;
                    if path == "-" {
                        print!("{}", content);
                    } else if append {
                        use std::fs::OpenOptions;
                        let mut file = OpenOptions::new().append(true).create(true).open(&path)?;
                        file.write_all(content.as_bytes())?;
                    } else {
                        std::fs::write(&path, &content)?;
                    }
                }
                return Ok(());
            }
            // clear-history - Clear pane history
            "clear-history" | "clearhist" => {
                let mut cmd = "clear-history".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-H" => { cmd.push_str(" -H"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // pipe-pane - Pipe pane output to a command
            "pipe-pane" | "pipep" => {
                let mut cmd = "pipe-pane".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-I" => { cmd.push_str(" -I"); }
                        "-O" => { cmd.push_str(" -O"); }
                        "-o" => { cmd.push_str(" -o"); }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        // The piped shell command is opaque and must reach the
                        // server as a single token: the server re-flattens the
                        // args with join(" ") and hands the result to a shell,
                        // so a quoted argument containing a space would otherwise
                        // arrive as several arguments (issue #563). Quoting through
                        // quote_arg_if_needed makes that join an identity and
                        // escapes backslashes to match parse_command_line.
                        s => { cmd.push_str(&format!(" {}", crate::util::quote_arg_if_needed(s))); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                // A direct file sink that could not be opened answers
                // "ERROR: ..."; exiting 0 on it recorded nothing and looked
                // identical to success (same shape as #559). Acceptance
                // answers nothing.
                let resp = send_control_with_response(cmd)?;
                if resp.trim_start().starts_with("ERROR") {
                    eprint!("{}", resp);
                    std::process::exit(1);
                }
                return Ok(());
            }
            // find-window - Search for a window
            "find-window" | "findw" => {
                let mut pattern: Option<String> = None;
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-C" | "-N" | "-T" | "-i" | "-r" | "-Z" => {}
                        "-t" => { i += 1; }
                        s if !s.starts_with('-') => { pattern = Some(s.to_string()); }
                        _ => {}
                    }
                    i += 1;
                }
                if let Some(p) = pattern {
                    let resp = send_control_with_response(format!("find-window {}\n", p))?;
                    print!("{}", resp);
                }
                return Ok(());
            }
            // list-commands - List all commands (duplicate handled above but kept for match completeness)
            "list-commands" | "lscm" => {
                print_commands();
                return Ok(());
            }
            // set-hook - Set a hook
            "set-hook" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                send_control(format!("{}\n", cmd_str))?;
                return Ok(());
            }
            // show-hooks - Show hooks
            "show-hooks" => {
                let cmd_str: String = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                let resp = send_control_with_response(format!("{}\n", cmd_str))?;
                print!("{}", resp);
                return Ok(());
            }
            // next-layout - Cycle to next layout
            "next-layout" => {
                send_control("next-layout\n".to_string())?;
                return Ok(());
            }
            // previous-layout - Cycle to previous layout
            "previous-layout" => {
                send_control("previous-layout\n".to_string())?;
                return Ok(());
            }
            // choose-tree / choose-window / choose-session — interactive TUI choosers.
            // From the CLI just forward to the server so the attached client
            // (if any) can display the chooser.  Return exit 0.
            "choose-tree" | "choose-window" | "choose-session" => {
                send_control(format!("{}\n", cmd))?;
                return Ok(());
            }
            // command-prompt - Open interactive command prompt
            "command-prompt" => {
                let mut cmd = "command-prompt".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-I" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -I {}", t));
                                i += 1;
                            }
                        }
                        "-p" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -p {}", t));
                                i += 1;
                            }
                        }
                        "-1" => { cmd.push_str(" -1"); }
                        "-N" => { cmd.push_str(" -N"); }
                        "-W" => { cmd.push_str(" -W"); }
                        "-T" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -T {}", t));
                                i += 1;
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // display-menu - Display a menu
            "display-menu" | "menu" => {
                let parts: Vec<String> = cmd_args.iter().map(|s| {
                    crate::util::quote_arg_if_needed(s)
                }).collect();
                send_control(format!("{}\n", parts.join(" ")))?;
                return Ok(());
            }
            // display-popup - Display a popup window
            "display-popup" | "popup" => {
                let parts: Vec<String> = cmd_args.iter().map(|s| {
                    crate::util::quote_arg_if_needed(s)
                }).collect();
                send_control(format!("{}\n", parts.join(" ")))?;
                return Ok(());
            }
            "new-pane" | "newp" => {
                let parts: Vec<String> = cmd_args.iter().map(|s| {
                    crate::util::quote_arg_if_needed(s)
                }).collect();
                let line = format!("{}\n", parts.join(" "));
                // -P prints the new pane id, so read the server's response.
                if cmd_args.iter().any(|a| a.as_str() == "-P") {
                    let resp = send_control_with_response(line)?;
                    print!("{}", resp);
                } else {
                    send_control(line)?;
                }
                return Ok(());
            }
            // server-info - Show server information
            "server-info" | "info" => {
                let resp = send_control_with_response("server-info\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // dump-state - print the server's live state JSON (debug / testing aid)
            "dump-state" | "dump" => {
                let resp = send_control_with_response("dump-state\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // mouse-* - forward raw mouse events to the server (debug / testing aid)
            "mouse-down" | "mouse-drag" | "mouse-up" | "mouse-down-right" | "mouse-up-right"
            | "pane-mouse" | "pane-scroll" | "copy-drag-begin" => {
                let parts: Vec<String> = cmd_args.iter().map(|s| s.to_string()).collect();
                send_control(format!("{}\n", parts.join(" ")))?;
                return Ok(());
            }
            // start-server / warmup - Pre-spawn a warm server
            "start-server" | "start" | "warmup" => {
                // Pre-spawn a warm __warm__ server so the next new-session is
                // instant.  Also triggers Windows Defender's scan cache on the
                // binary, eliminating the ~200-400ms first-run penalty.
                let warm_base = if let Some(ref l) = l_socket_name {
                    format!("{}____warm__", l)
                } else {
                    "__warm__".to_string()
                };
                let warm_port_path = crate::paths::port_file(&warm_base);
                // Check if warm server is already running
                let already_running = if std::path::Path::new(&warm_port_path).exists() {
                    if let Ok(port_str) = std::fs::read_to_string(&warm_port_path) {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            std::net::TcpStream::connect_timeout(
                                &format!("127.0.0.1:{}", port).parse().unwrap(),
                                Duration::from_millis(100),
                            ).is_ok()
                        } else { false }
                    } else { false }
                } else { false };
                if already_running {
                    return Ok(());
                }
                // Clean up stale port file if any
                let _ = std::fs::remove_file(&warm_port_path);
                // Spawn the warm server
                let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
                let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), "__warm__".into()];
                if let Some(ref l) = l_socket_name {
                    server_args.push("-L".into());
                    server_args.push(l.clone());
                }
                // Detect terminal size for the warm server
                if let Ok((tw, th)) = crossterm::terminal::size() {
                    let h = th.saturating_sub(1);
                    if tw > 0 && h > 0 {
                        server_args.push("-x".into());
                        server_args.push(tw.to_string());
                        server_args.push("-y".into());
                        server_args.push(h.to_string());
                    }
                }
                #[cfg(windows)]
                crate::platform::spawn_server_hidden(&exe, &server_args)?;
                #[cfg(not(windows))]
                {
                    let mut cmd = std::process::Command::new(&exe);
                    for a in &server_args { cmd.arg(a); }
                    cmd.stdin(std::process::Stdio::null());
                    cmd.stdout(std::process::Stdio::null());
                    cmd.stderr(std::process::Stdio::null());
                    let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn warm server: {e}")))?;
                }
                return Ok(());
            }
            // confirm-before - Ask for confirmation before running a command
            "confirm-before" | "confirm" => {
                let parts: Vec<String> = cmd_args.iter().map(|s| {
                    crate::util::quote_arg_if_needed(s)
                }).collect();
                send_control(format!("{}\n", parts.join(" ")))?;
                return Ok(());
            }
            // refresh-client - Refresh the client display
            "refresh-client" | "refresh" => {
                let mut cmd = "refresh-client".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    match cmd_args[i].as_str() {
                        "-S" => { cmd.push_str(" -S"); }
                        "-l" => { cmd.push_str(" -l"); }
                        // Control-only flags are forwarded so the server can
                        // reject them with tmux's "not a control client" error
                        // instead of the client silently dropping them.
                        "-C" | "-B" | "-A" | "-f" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" {} {}", cmd_args[i], t));
                                i += 1;
                            } else {
                                cmd.push_str(&format!(" {}", cmd_args[i]));
                            }
                        }
                        "-t" => {
                            if let Some(t) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" -t {}", t));
                                i += 1;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                cmd.push('\n');
                let resp = send_control_with_response(cmd)?;
                let trimmed = resp.trim();
                if let Some(reason) = trimmed.strip_prefix("ERROR:") {
                    eprintln!("psmux: {}", reason.trim());
                    std::process::exit(1);
                }
                if !trimmed.is_empty() {
                    print!("{}", resp);
                }
                return Ok(());
            }
            // send-prefix - Send the prefix key to the active pane
            "send-prefix" => {
                send_control("send-prefix\n".to_string())?;
                return Ok(());
            }
            // show-messages - Show message log
            "show-messages" | "showmsgs" => {
                let resp = send_control_with_response("show-messages\n".to_string())?;
                if !resp.trim().is_empty() {
                    print!("{}", resp);
                }
                return Ok(());
            }
            // suspend-client - Suspend client (no-op on Windows)
            "suspend-client" | "suspendc" => {
                // No-op on Windows — no SIGTSTP concept
                return Ok(());
            }
            // lock-client / lock-server / lock-session (no-op on Windows)
            "lock-client" | "lockc" | "lock-server" | "lock" | "lock-session" | "locks" => {
                // No-op on Windows — no terminal locking concept
                return Ok(());
            }
            // resize-window - Resize window
            "resize-window" | "resizew" => {
                let mut cmd = "resize-window".to_string();
                let mut i = 1;
                while i < cmd_args.len() {
                    let arg = cmd_args[i].as_str();
                    match arg {
                        "-x" | "-y" => {
                            if let Some(v) = cmd_args.get(i + 1) {
                                cmd.push_str(&format!(" {} {}", arg, v));
                                i += 1;
                            }
                        }
                        "-t" => { i += 1; } // target handled globally
                        _ => { cmd.push_str(&format!(" {}", arg)); }
                    }
                    i += 1;
                }
                cmd.push('\n');
                send_control(cmd)?;
                return Ok(());
            }
            // customize-mode - tmux 3.2+ customize mode
            "customize-mode" => {
                send_control("customize-mode\n".to_string())?;
                return Ok(());
            }
            // choose-client - List clients interactively
            "choose-client" => {
                // Single-client model — returns current client info
                let resp = send_control_with_response("list-clients\n".to_string())?;
                print!("{}", resp);
                return Ok(());
            }
            // respawn-window - Respawn active pane in window
            "respawn-window" | "respawnw" => {
                send_control("respawn-window\n".to_string())?;
                return Ok(());
            }
            // link-window - Link a window
            "link-window" | "linkw" => {
                let full = cmd_args.iter().map(|s| s.as_str()).collect::<Vec<&str>>().join(" ");
                send_control(format!("{}\n", full))?;
                return Ok(());
            }
            // unlink-window - Unlink a window
            "unlink-window" | "unlinkw" => {
                send_control("unlink-window\n".to_string())?;
                return Ok(());
            }
            _ => {
                // Unknown command - print error and exit
                if !cmd.is_empty() {
                    eprintln!("psmux: unknown command: {}", cmd);
                    eprintln!("Run 'psmux --help' for usage information.");
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unknown command: {}", cmd)));
                }
            }
        }
    
    // Prevent nesting: similar to tmux checking $TMUX.
    // PSMUX_ACTIVE is set on the client process itself.
    // PSMUX_SESSION is set on child panes spawned by the server.
    // Both indicate we are already inside a psmux PANE; a display-popup child
    // is not one, and tmux attaches happily from there (#537).
    // Override with PSMUX_ALLOW_NESTING=1 if nesting is intentional.
    //
    // This has to happen HERE, before the block below allocates a session name
    // and spawns (or claims) a server. It used to sit further down, after the
    // spawn and after the non-tty version fallback, which made it a refusal in
    // name only: a nested `psmux` printed "nested with care", declined to
    // attach, and left a fully spawned orphan session behind it. It was also
    // unreachable for a non-tty caller, who got the version banner instead of
    // the refusal. Nesting is a property of the environment, not of the
    // terminal, and the cheapest correct moment to refuse is before we build
    // anything.
    //
    // Control mode keeps its long-standing exemption: it returned earlier than
    // the old guard, so it was never subject to this, and an editor driving
    // `-CC` from inside a pane is a legitimate arrangement.
    if control_mode == 0
        && env::var("PSMUX_ALLOW_NESTING").ok().as_deref() != Some("1")
        && crate::util::inside_psmux_pane()
    {
        eprintln!("psmux: sessions should be nested with care, unset PSMUX_SESSION to force");
        return Ok(());
    }

    // Default behavior (bare `psmux` with no command):
    // tmux-compatible: always create a new session with the next available
    // numeric name (0, 1, 2, ...) and attach to it.
    //
    // For both control mode (-C/-CC) and TUI mode, ensure a session server
    // is running before we try to connect.  Real tmux's bare `tmux -CC`
    // starts the server and creates a session automatically; we do the same.

    if env::var("PSMUX_REMOTE_ATTACH").ok().as_deref() != Some("1") {
        let psmux_dir = crate::paths::psmux_dir();
        let session_name = env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| {
            crate::session::next_session_name(l_socket_name.as_deref())
        });
        let port_file_base = if let Some(ref l) = l_socket_name {
            format!("{}__{}", l, session_name)
        } else {
            session_name.clone()
        };
        let port_path = crate::paths::port_file(&port_file_base);

        // If a server is ALREADY alive under this exact name (e.g. PSMUX_SESSION_NAME
        // was set to target an existing session rather than to request a fresh one —
        // this is how -C/-CC control-mode clients attach), do NOT warm-claim or
        // cold-spawn a replacement. Doing so unconditionally (as this block used to)
        // clobbered the live session's port/key/sid files with those of a brand-new,
        // empty-scrollback server on every single bare/control-mode connection,
        // silently orphaning the running session. Mirrors the liveness check the
        // explicit `new-session` command path already performs above.
        let server_already_alive = std::fs::read_to_string(&port_path)
            .ok()
            .and_then(|s| s.trim().parse::<u16>().ok())
            .map(|p| std::net::TcpStream::connect_timeout(
                &format!("127.0.0.1:{}", p).parse().unwrap(),
                Duration::from_millis(100),
            ).is_ok())
            .unwrap_or(false);

        // Try warm server claim first (fast path)
        // Skipped when PSMUX_NO_WARM=1 is set or config has 'set -g warm off',
        // or when the target session is already alive (see above).
        let warm_disabled = server_already_alive
            || std::env::var("PSMUX_NO_WARM").map(|v| v == "1" || v == "true").unwrap_or(false)
            || crate::config::is_warm_disabled_by_config();
        let warm_base = if let Some(ref l) = l_socket_name {
            format!("{}____warm__", l)
        } else {
            "__warm__".to_string()
        };
        let warm_port_path = crate::paths::port_file(&warm_base);
        let mut warm_claimed = false;
        // Atomically CLAIM the warm server before connecting (see the detached
        // path above for the full rationale): renaming the shared __warm__.port
        // file is atomic, so exactly one concurrent new-session wins a given warm
        // server and the rest cold-spawn. Prevents the rapid-creation race where
        // two clients claim the same warm and one session is lost.
        let warm_port_opt = if warm_disabled { None } else {
            std::fs::read_to_string(&warm_port_path).ok().and_then(|s| s.trim().parse::<u16>().ok())
        };
        let warm_claim_path = format!("{}\\{}.claiming.{}", psmux_dir, warm_base, std::process::id());
        if let Some(port) = warm_port_opt {
            if std::fs::rename(&warm_port_path, &warm_claim_path).is_ok() {
            let warm_key = crate::session::read_session_key(&warm_base).unwrap_or_default();
            {
                {
                    let addr = format!("127.0.0.1:{}", port);
                    if let Ok(mut stream) = std::net::TcpStream::connect_timeout(
                        &addr.parse().unwrap(),
                        Duration::from_millis(500),
                    ) {
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(3000)));
                        let _ = write!(stream, "AUTH {}\n", warm_key);
                        let client_cwd = std::env::current_dir()
                            .ok()
                            .and_then(|p| p.to_str().map(|s| s.to_string()));
                        // See the new-session claim above: -p is what lets a bare
                        // `psmux` in a shell with PSMUX_PRIORITY set reach a standby
                        // that was spawned long before that shell (#608).
                        let claim_prio = crate::platform::claim_priority_arg();
                        if let Some(ref cwd) = client_cwd {
                            let _ = write!(stream, "claim-session {} {} -p {}\n", crate::util::quote_arg(&session_name), crate::util::quote_arg(cwd), crate::util::quote_arg(&claim_prio));
                        } else {
                            let _ = write!(stream, "claim-session {} -p {}\n", crate::util::quote_arg(&session_name), crate::util::quote_arg(&claim_prio));
                        }
                        let _ = stream.flush();
                        // Committed: we atomically own this warm (won the .port
                        // rename) and have sent claim-session, so it WILL become our
                        // session. Set warm_claimed NOW so a slow/missing response
                        // does not trigger a duplicate cold spawn (the duplicate was
                        // the residual cause of rapid-creation session loss). The OK
                        // read below still waits for the rename to finish (issue
                        // #136), and the port-file wait after this block covers a
                        // slow response.
                        warm_claimed = true;
                        // Use send_auth_cmd_response pattern: read AUTH
                        // "OK" line first, then read the claim-session
                        // response.  Previously a single raw read() would
                        // pick up only the AUTH "OK" and proceed before
                        // the server finished renaming port/key files,
                        // causing "auth failed" on the subsequent attach
                        // (issue #136).
                        if let Ok(reader_stream) = stream.try_clone() {
                            let mut br = std::io::BufReader::new(reader_stream);
                            let mut auth_line = String::new();
                            if std::io::BufRead::read_line(&mut br, &mut auth_line).unwrap_or(0) > 0
                                && auth_line.trim().starts_with("OK")
                            {
                                // Auth succeeded — now wait for the claim
                                // response so files are renamed before we
                                // try to attach.
                                let mut claim_line = String::new();
                                let got = std::io::BufRead::read_line(&mut br, &mut claim_line).unwrap_or(0) > 0;
                                if got && claim_line.contains("OK") {
                                    warm_claimed = true;
                                } else if got && claim_line.contains("ERR") {
                                    // Explicit rejection: this server is NOT a warm
                                    // server (stale __warm__.port -> already-claimed
                                    // session, or OS port reuse). Do NOT commit; the
                                    // handoff file was already consumed above, so the
                                    // bad warm pointer self-heals. Cold-spawn instead
                                    // of waiting ~5s for a session that never appears.
                                    warm_claimed = false;
                                }
                                // No/garbled response: leave warm_claimed = true
                                // (set above) to preserve the rapid-creation race
                                // fix — we own a live warm that will complete.
                            }
                        }
                    }
                }
            }
            }
            // Orphaned handoff file: server wrote <session>.port on success.
            let _ = std::fs::remove_file(&warm_claim_path);
        }


        if !warm_claimed && !server_already_alive {
            // Cold path: spawn a new background server
            let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("psmux"));
            let mut server_args: Vec<String> = vec!["server".into(), "-s".into(), session_name.clone()];
            // Propagate the -L socket namespace so the cold-spawned server writes
            // its port/key files under the namespaced base (<ns>__<session>) that
            // the client waits on below. Without this, bare `psmux -L <ns>` (no
            // warm server to claim → always cold path) spawned a DEFAULT-namespace
            // server, the namespaced port file never appeared, and the attach
            // timed out then failed with "handle is invalid" / exit 1.
            if let Some(ref l) = l_socket_name {
                server_args.push("-L".into());
                server_args.push(l.clone());
            }
            #[cfg(windows)]
            crate::platform::spawn_server_hidden(&exe, &server_args)?;
            #[cfg(not(windows))]
            {
                let mut cmd = std::process::Command::new(&exe);
                for a in &server_args { cmd.arg(a); }
                cmd.stdin(std::process::Stdio::null());
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
                let _child = cmd.spawn().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("failed to spawn server: {e}")))?;
            }
        }

        // Wait for the session's port file before attaching. This covers BOTH the
        // cold spawn (server writes it on startup) AND a committed warm claim whose
        // rename of __warm__.port -> <session>.port may still be completing — so the
        // attach never races ahead of the rename (issue #136).
        for _ in 0..500 {
            if std::path::Path::new(&port_path).exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        // Now attach to the session
        env::set_var("PSMUX_SESSION_NAME", &port_file_base);
        env::set_var("PSMUX_REMOTE_ATTACH", "1");
    }

    // Control mode: connect to server with CONTROL/CONTROL_NOECHO protocol
    // instead of launching the TUI client. Must be checked before the
    // is_terminal() gate since control mode reads from piped stdin.
    if control_mode > 0 {
        return run_control_mode(control_mode);
    }

    // Raw VT pipe detection: under mintty (Git Bash/MSYS2, issue #474) and
    // under `ssh -T` (the Win10 SSH mouse workaround), stdin/stdout are pipes,
    // not a console. Read and write VT bytes directly instead of using console
    // APIs or routing them through ConPTY.
    let pipe_vt = crate::ssh_input::stdin_is_vt_pipe();

    // The nesting guard used to live here. It now runs before the session
    // spawn further up, so a refused nested invocation leaves nothing behind.

    // If stdin is not a terminal (headless/non-interactive environment, e.g.
    // winget validation pipeline), print version and exit cleanly — starting
    // a TUI session would fail without an interactive console. A Cygwin pty
    // IS a terminal (a human sits on the mintty side) even though it is
    // technically a pipe. The same is true of an interactive `ssh -T` channel.
    //
    // Only a BARE invocation reaches this. Anything that already knows what it
    // was asked to do must answer for itself before here, or the version
    // becomes a success report for work that never happened.
    if !std::io::stdin().is_terminal() && !pipe_vt {
        print_version();
        return Ok(());
    }
    env::set_var("PSMUX_ACTIVE", "1");

    // Same reasoning as the server (#608), for the other half of the keystroke
    // path: this process reads the console input buffer and writes the frames,
    // and the console window belongs to the terminal host, not to us, so we
    // never inherit its foreground boost. Placed after the non-terminal bail so
    // a scripted `psmux` that only prints its version does not touch its class.
    // Fail open: a refused SetPriorityClass is ignored.
    let client_priority = crate::platform::resolve_priority(
        crate::config::priority_from_config().as_deref(),
        true,
    );
    crate::platform::set_process_priority(&client_priority);

    let mut stdout = crate::platform::create_writer();
    enable_virtual_terminal_processing();
    if pipe_vt {
        // The local wrapper (or Cygwin pty) is already raw on the terminal
        // side; enable_raw_mode would call SetConsoleMode on this pipe handle
        // and fail with ERROR_INVALID_FUNCTION.
        // crossterm's ANSI detection needs TERM set to take the pure-ANSI
        // path on Windows — mintty always sets it, but make sure.
        if env::var("TERM").is_err() {
            env::set_var("TERM", "xterm-256color");
        }
        let _ = enable_raw_mode();
    } else {
        enable_raw_mode()?;
    }

    // Issue #473: ask the host terminal for its colors (OSC 10/11/4, ?996n)
    // BEFORE the input pump starts, so the replies cannot be misread as
    // keystrokes.  The client reports the result to the server on attach,
    // letting the server answer the same queries from pane applications.
    // Skipped in pipe mode: the query/reply machinery is console-based.
    if !pipe_vt {
        let _ = crate::types::HOST_COLORS_SPEC.set(crate::platform::query_host_terminal_colors());
    }

    // Detect terminal type for input handling.
    // Use VT input parsing for SSH sessions and terminals that send VT mouse
    // sequences through ConPTY (e.g. JetBrains JediTerm).
    let use_vt_input = crate::ssh_input::needs_vt_input();

    // For standard terminals (not SSH), clear VTI flag from stdin if
    // crossterm or another layer set it. Keeps normal ReadConsoleInputW
    // behavior via proper INPUT_RECORDs.
    if !use_vt_input && !pipe_vt {
        crate::platform::disable_vti_on_stdin();
    }

    execute!(stdout, EnterAlternateScreen, EnableBlinking, EnableMouseCapture, EnableBracketedPaste)?;
    apply_cursor_style(&mut stdout)?;

    let input = if pipe_vt {
        InputSource::new_pipe()?
    } else {
        InputSource::new(use_vt_input)?
    };

    if pipe_vt {
        // Learn the real terminal size over the pipe (XTWINOPS) before the
        // first draw; the reader thread records the reply for the backend.
        // Also enable SGR mouse / focus / bracketed paste directly. With
        // `ssh -T`, these bytes reach the client terminal without ConPTY.
        crate::ssh_input::pipe_send_modes_enable();
        crate::ssh_input::request_pipe_terminal_size();
        for _ in 0..50 {
            if crate::platform::pipe_term_size().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if crate::platform::pipe_term_size().is_none() {
            // No reply (terminal doesn't speak XTWINOPS) — a sane default
            // beats failing; a later reply corrects it live.
            crate::platform::set_pipe_term_size(120, 30);
        }
    }

    let backend = crate::platform::PsmuxBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // For console-backed VT input (SSH / JetBrains), explicitly (re-)send
    // mouse-enable escape sequences. ConPTY may have consumed crossterm's
    // EnableMouseCapture output without forwarding it. Pipe mode already sent
    // its safe mode set above and must not enter this ConPTY-specific path.
    if use_vt_input && !pipe_vt {
        send_mouse_enable();
    } else if !pipe_vt {
        // Local console: write the DECSET registration explicitly instead of
        // relying solely on ConPTY synthesizing it from ENABLE_MOUSE_INPUT.
        // Windows Terminal tracks this registration and can silently drop it
        // later; the client re-arms it periodically with the same call.
        crate::ssh_input::send_mouse_keepalive();
    }

    // Loop to handle session switching without spawning new processes
    let result = loop {
        let result = run_remote(&mut terminal, &input);
        
        // Check if we should switch to another session
        if let Ok(switch_to) = env::var("PSMUX_SWITCH_TO") {
            env::remove_var("PSMUX_SWITCH_TO");
            // Remember the session THIS client is leaving, before the variable
            // that names it is overwritten. This process spans both sides of a
            // switch (one server per session, so the client re-attaches
            // elsewhere), which makes it the only place that knows the pair.
            // It is handed to the next server on the attach handshake so
            // `switch-client -l` can answer from this client's own history
            // instead of a machine-wide file that any other client may have
            // written (issue #566).
            if let Ok(leaving) = env::var("PSMUX_SESSION_NAME") {
                if !leaving.is_empty() && leaving != switch_to {
                    env::set_var("PSMUX_CLIENT_LAST_SESSION", &leaving);
                }
            }
            env::set_var("PSMUX_SESSION_NAME", &switch_to);
            // Update last_session file. This stays the routing hint it always
            // was (resolve_last_session_name_ns consumes it); it is no longer
            // what -l reads.
            let last_path = crate::paths::psmux_dir_file("last_session");
            let _ = std::fs::write(&last_path, &switch_to);
            // A switch is an attach: tmux restamps the session a client moves to
            // (`server_client_set_session` -> `session_update_activity`). Bare CLI
            // routing ranks by that stamp (issue #603).
            crate::session::touch_session_activity(&switch_to);
            // Continue loop to attach to new session
            continue;
        }
        
        break result;
    };

    // Terminal cleanup — always runs, even on error, to prevent leaked
    // SGR attributes (invisible text), stuck raw mode, or stale cursor style.
    let _ = disable_raw_mode();
    let out = terminal.backend_mut();
    // Reset all SGR attributes (fg/bg color, bold, hidden, etc.) BEFORE
    // leaving the alternate screen.  SGR state is global and NOT restored
    // by the alternate-screen save/restore mechanism (\x1b[?1049l).
    // Without this, the last ratatui frame's foreground color can persist
    // into the main screen, making typed text invisible.
    let _ = execute!(out, crossterm::style::Print("\x1b[0m"));
    // Reset cursor style to terminal default (\x1b[0 q)
    let _ = execute!(out, crossterm::style::Print("\x1b[0 q"));
    let _ = execute!(out, DisableBlinking, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

/// Run as a control mode client (psmux -C or psmux -CC).
/// Connects to the server via TCP, sends CONTROL/CONTROL_NOECHO,
/// reads commands from stdin and prints responses/notifications to stdout.
///
/// When running over SSH with a ConPTY console, Windows ConPTY silently
/// consumes DCS escape sequences (including the `\x1bP1000p` that iTerm2
/// uses to detect tmux control mode) and also interleaves its own cursor
/// positioning sequences into the output, corrupting the line-based
/// protocol.  To bypass ConPTY, the SSH client must disable PTY allocation
/// so that stdin/stdout are raw pipes: `ssh -T user@host tmux -CC`.
fn run_control_mode(mode: u8) -> io::Result<()> {
    use std::net::TcpStream;

    // Create diagnostic log FIRST, before anything else, so we can see failures.
    let psmux_dir = crate::paths::psmux_dir();
    let _ = std::fs::create_dir_all(&psmux_dir);
    let cc_log_path = crate::paths::psmux_dir_file("cc_debug.log");
    let mut log_file = std::fs::File::create(&cc_log_path).ok();
    macro_rules! cclog {
        ($($arg:tt)*) => {
            if let Some(ref mut f) = log_file {
                let _ = writeln!(f, $($arg)*);
                let _ = f.flush();
            }
        }
    }
    cclog!("=== psmux control mode log ===");
    cclog!("time: {:?}", std::time::SystemTime::now());
    cclog!("mode: {}", if mode == 1 { "CONTROL" } else { "CONTROL_NOECHO" });
    cclog!("USERPROFILE: {:?}", env::var("USERPROFILE"));
    cclog!("HOME: {:?}", env::var("HOME"));
    cclog!("log_path: {}", cc_log_path);
    cclog!("SSH_CLIENT: {:?}", env::var("SSH_CLIENT"));
    cclog!("SSH_CONNECTION: {:?}", env::var("SSH_CONNECTION"));
    cclog!("PSMUX_SESSION_NAME: {:?}", env::var("PSMUX_SESSION_NAME"));
    cclog!("PSMUX_REMOTE_ATTACH: {:?}", env::var("PSMUX_REMOTE_ATTACH"));

    // Win32 handle diagnostics
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
            fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
            fn PeekNamedPipe(
                hNamedPipe: *mut std::ffi::c_void,
                lpBuffer: *mut u8,
                nBufferSize: u32,
                lpBytesRead: *mut u32,
                lpTotalBytesAvail: *mut u32,
                lpBytesLeftThisMessage: *mut u32,
            ) -> i32;
            fn GetLastError() -> u32;
        }
        const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
        const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
        unsafe {
            let h_in = GetStdHandle(STD_INPUT_HANDLE);
            let h_out = GetStdHandle(STD_OUTPUT_HANDLE);
            let ft_in = GetFileType(h_in);
            let ft_out = GetFileType(h_out);
            // FILE_TYPE_UNKNOWN=0, FILE_TYPE_DISK=1, FILE_TYPE_CHAR=2, FILE_TYPE_PIPE=3
            cclog!("stdin_handle: 0x{:x} (file_type={})", h_in as u64, ft_in);
            cclog!("stdout_handle: 0x{:x} (file_type={})", h_out as u64, ft_out);
            // Try to peek stdin to see if pipe is alive
            let mut avail: u32 = 0;
            let peek_ok = PeekNamedPipe(h_in, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut());
            let last_err = GetLastError();
            cclog!("stdin PeekNamedPipe: ok={} avail={} last_error={}", peek_ok, avail, last_err);
        }
        cclog!("stdin_is_terminal: {}", std::io::stdin().is_terminal());
        cclog!("stdout_is_terminal: {}", std::io::stdout().is_terminal());
    }

    // Detect ConPTY + SSH: control mode over SSH requires raw pipe I/O.
    // ConPTY injects cursor-positioning escape sequences between protocol
    // lines, corrupting the tmux control protocol for iTerm2.
    //
    // Detection: if stdout IS a console handle directly, we know ConPTY is
    // active. However, when DefaultShell is pwsh, stdout is a pipe from pwsh
    // and we cannot reliably distinguish ConPTY-backed pipes from raw pipes.
    // We only block the definite case (direct console handle).
    // ConPTY raw passthrough: when stdin/stdout are consoles (e.g. ssh -t
    // allocated a PTY), put them into raw mode so ConPTY doesn't cook bytes
    // (line buffering, ECHO, NL<->CRLF) or interpret VT sequences. This
    // lets the tmux DCS protocol flow intact regardless of `ssh -T` vs
    // `ssh -t`. Some clients (e.g. iTerm2's tmux integration) close stdin
    // on the SSH session shortly after seeing the DCS opener when no PTY
    // is allocated, so supporting `ssh -t` is required for them.
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(n: u32) -> *mut std::ffi::c_void;
            fn GetConsoleMode(h: *mut std::ffi::c_void, m: *mut u32) -> i32;
            fn SetConsoleMode(h: *mut std::ffi::c_void, m: u32) -> i32;
        }
        const STD_INPUT_HANDLE: u32 = (-10i32) as u32;
        const STD_OUTPUT_HANDLE: u32 = (-11i32) as u32;
        const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
        const ENABLE_LINE_INPUT: u32 = 0x0002;
        const ENABLE_ECHO_INPUT: u32 = 0x0004;
        const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING_OUT: u32 = 0x0004;
        const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;
        unsafe {
            let h_in = GetStdHandle(STD_INPUT_HANDLE);
            let h_out = GetStdHandle(STD_OUTPUT_HANDLE);
            let mut mode_in: u32 = 0;
            let mut mode_out: u32 = 0;
            if GetConsoleMode(h_in, &mut mode_in) != 0 {
                let new_in = (mode_in & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT;
                let r = SetConsoleMode(h_in, new_in);
                cclog!("ConPTY stdin: mode 0x{:x} -> 0x{:x} (set ok={})", mode_in, new_in, r);
            }
            if GetConsoleMode(h_out, &mut mode_out) != 0 {
                let new_out = mode_out | ENABLE_VIRTUAL_TERMINAL_PROCESSING_OUT | DISABLE_NEWLINE_AUTO_RETURN;
                let r = SetConsoleMode(h_out, new_out);
                cclog!("ConPTY stdout: mode 0x{:x} -> 0x{:x} (set ok={})", mode_out, new_out, r);
            }
        }
    }

    let session_name = env::var("PSMUX_SESSION_NAME")
        .unwrap_or_else(|_| "default".to_string());
    cclog!("session: {}", session_name);

    // Read port and key
    let port_path = crate::paths::port_file(&session_name);
    let key_path = crate::paths::key_file(&session_name);
    cclog!("port_path: {}", port_path);
    cclog!("key_path: {}", key_path);
    cclog!("port_path exists: {}", std::path::Path::new(&port_path).exists());
    cclog!("key_path exists: {}", std::path::Path::new(&key_path).exists());

    let port_str = match std::fs::read_to_string(&port_path) {
        Ok(s) => { cclog!("port_str: {:?}", s.trim()); s }
        Err(e) => { cclog!("FATAL: cannot read port file: {}", e); return Err(io::Error::new(io::ErrorKind::NotFound, format!("session '{}' not found (no port file)", session_name))); }
    };
    let port: u16 = match port_str.trim().parse() {
        Ok(p) => { cclog!("port: {}", p); p }
        Err(e) => { cclog!("FATAL: corrupted port file: {}", e); return Err(io::Error::new(io::ErrorKind::InvalidData, "corrupted port file")); }
    };
    let key = match std::fs::read_to_string(&key_path) {
        Ok(k) => { cclog!("key: (read {} bytes)", k.trim().len()); k.trim().to_string() }
        Err(e) => { cclog!("FATAL: cannot read key file: {}", e); return Err(io::Error::new(io::ErrorKind::NotFound, "session key file not found")); }
    };

    // Connect
    cclog!("connecting to 127.0.0.1:{}", port);
    let mut stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
        Ok(s) => { cclog!("connected OK"); s }
        Err(e) => { cclog!("FATAL: connect failed: {}", e); return Err(io::Error::new(io::ErrorKind::ConnectionRefused, format!("cannot connect to session: {}", e))); }
    };
    let _ = stream.set_nodelay(true);

    // Auth
    write!(stream, "AUTH {}\n", key)?;
    stream.flush()?;
    cclog!("AUTH sent");

    // Read OK response
    let mut reader = io::BufReader::new(stream.try_clone()?);
    let mut ok_line = String::new();
    reader.read_line(&mut ok_line)?;
    cclog!("auth response: {:?}", ok_line.trim());
    if !ok_line.trim().starts_with("OK") {
        cclog!("FATAL: auth failed");
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, format!("auth failed: {}", ok_line.trim())));
    }

    // Send CONTROL or CONTROL_NOECHO
    let mode_str = if mode == 1 { "CONTROL" } else { "CONTROL_NOECHO" };
    let mut write_stream = reader.get_ref().try_clone()?;
    write!(write_stream, "{}\n", mode_str)?;
    write_stream.flush()?;
    cclog!("{} sent, starting I/O threads", mode_str);

    // Spawn a thread to read server responses/notifications and print to stdout
    let reader_stream = reader.get_ref().try_clone()?;
    let cc_log_path = Some(cc_log_path);
    let cc_log_out = cc_log_path.clone();
    let reader_thread = std::thread::spawn(move || {
        let mut br = io::BufReader::new(reader_stream);
        let mut line = String::new();
        let stdout = io::stdout();
        let start = std::time::Instant::now();
        let mut log_file = cc_log_out.as_ref().and_then(|p| {
            std::fs::OpenOptions::new().append(true).open(p).ok()
        });
        loop {
            line.clear();
            match br.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if let Some(ref mut f) = log_file {
                        let _ = writeln!(f, "[{:>8.3}s] OUT ({} bytes): {:?}",
                            start.elapsed().as_secs_f64(), line.len(),
                            &line[..line.len().min(200)]);
                    }
                    let mut out = stdout.lock();
                    let _ = out.write_all(line.as_bytes());
                    let _ = out.flush();
                }
            }
        }
    });

    // Read commands from stdin and send to server.
    // iTerm2's tmux integration sends \r as the command terminator by default
    // (TmuxGateway.newline = @"\r"). On Linux/macOS the PTY's ICRNL flag
    // translates \r → \n, but Windows ConPTY may not always do this.
    // Read raw bytes and translate bare \r to \n to avoid blocking the
    // server's read_line (which splits on \n only).
    let mut stdin_buf = [0u8; 4096];
    let stdin_start = std::time::Instant::now();
    let mut stdin_log_file = cc_log_path.as_ref().and_then(|p| {
        std::fs::OpenOptions::new().append(true).open(p).ok()
    });
    if let Some(ref mut f) = stdin_log_file {
        let _ = writeln!(f, "[{:>8.3}s] stdin reader started",
            stdin_start.elapsed().as_secs_f64());
        let _ = f.flush();
    }

    // Use raw Win32 ReadFile for stdin to handle SSH pipe edge cases.
    // Windows sshd may close the stdin pipe before the SSH channel is
    // fully established (race condition). We use PeekNamedPipe to
    // distinguish a genuinely broken pipe from a temporary condition.
    #[cfg(windows)]
    let stdin_handle = {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetStdHandle(nStdHandle: u32) -> *mut std::ffi::c_void;
        }
        unsafe { GetStdHandle((-10i32) as u32) }
    };
    #[cfg(not(windows))]
    let stdin_handle = ();

    let mut total_bytes_read: u64 = 0;
    let mut eof_retries: u32 = 0;
    const MAX_EOF_RETRIES: u32 = 20; // 20 * 50ms = 1 second of retries

    loop {
        // On Windows, use ReadFile directly for better diagnostics
        #[cfg(windows)]
        let read_result = {
            #[link(name = "kernel32")]
            extern "system" {
                fn ReadFile(
                    hFile: *mut std::ffi::c_void,
                    lpBuffer: *mut u8,
                    nNumberOfBytesToRead: u32,
                    lpNumberOfBytesRead: *mut u32,
                    lpOverlapped: *mut std::ffi::c_void,
                ) -> i32;
                fn GetLastError() -> u32;
                fn PeekNamedPipe(
                    hNamedPipe: *mut std::ffi::c_void, lpBuffer: *mut u8, nBufferSize: u32,
                    lpBytesRead: *mut u32, lpTotalBytesAvail: *mut u32,
                    lpBytesLeftThisMessage: *mut u32,
                ) -> i32;
            }
            let mut bytes_read: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    stdin_handle,
                    stdin_buf.as_mut_ptr(),
                    stdin_buf.len() as u32,
                    &mut bytes_read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                // ERROR_BROKEN_PIPE = 109, ERROR_NO_DATA = 232
                if err == 109 || err == 232 {
                    // Pipe is broken. Check if we should retry.
                    if total_bytes_read == 0 && eof_retries < MAX_EOF_RETRIES {
                        eof_retries += 1;
                        if let Some(ref mut f) = stdin_log_file {
                            if eof_retries <= 5 || eof_retries % 20 == 0 {
                                let _ = writeln!(f, "[{:>8.3}s] stdin pipe broken (err={}), retry {}/{}",
                                    stdin_start.elapsed().as_secs_f64(), err, eof_retries, MAX_EOF_RETRIES);
                                let _ = f.flush();
                            }
                        }
                        std::thread::sleep(Duration::from_millis(50));
                        // Re-check pipe state
                        let mut avail: u32 = 0;
                        let peek_ok = unsafe {
                            PeekNamedPipe(stdin_handle, std::ptr::null_mut(), 0,
                                std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
                        };
                        if peek_ok != 0 {
                            // Pipe is alive again!
                            if let Some(ref mut f) = stdin_log_file {
                                let _ = writeln!(f, "[{:>8.3}s] stdin pipe recovered! avail={}",
                                    stdin_start.elapsed().as_secs_f64(), avail);
                                let _ = f.flush();
                            }
                        }
                        continue;
                    }
                    if let Some(ref mut f) = stdin_log_file {
                        let _ = writeln!(f, "[{:>8.3}s] stdin pipe broken (err={}), giving up after {} retries",
                            stdin_start.elapsed().as_secs_f64(), err, eof_retries);
                        let _ = writeln!(f, "HINT: check DefaultShell and SSH client settings");
                        let _ = f.flush();
                    }
                    // Do NOT print to stderr: it travels through the SSH
                    // session and corrupts iTerm2's tmux control protocol.
                    // Diagnostics are in ~/.psmux/cc_debug.log.
                    Err(io::Error::from_raw_os_error(err as i32))
                } else {
                    if let Some(ref mut f) = stdin_log_file {
                        let _ = writeln!(f, "[{:>8.3}s] stdin ReadFile error: {}",
                            stdin_start.elapsed().as_secs_f64(), err);
                        let _ = f.flush();
                    }
                    Err(io::Error::from_raw_os_error(err as i32))
                }
            } else if bytes_read == 0 {
                // ReadFile succeeded but 0 bytes = EOF
                if total_bytes_read == 0 && eof_retries < MAX_EOF_RETRIES {
                    eof_retries += 1;
                    if let Some(ref mut f) = stdin_log_file {
                        if eof_retries <= 5 || eof_retries % 20 == 0 {
                            let _ = writeln!(f, "[{:>8.3}s] stdin EOF (0 bytes), retry {}/{}",
                                stdin_start.elapsed().as_secs_f64(), eof_retries, MAX_EOF_RETRIES);
                            let _ = f.flush();
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                if let Some(ref mut f) = stdin_log_file {
                    let _ = writeln!(f, "[{:>8.3}s] stdin EOF, giving up after {} retries",
                        stdin_start.elapsed().as_secs_f64(), eof_retries);
                    let _ = f.flush();
                }
                Ok(0usize)
            } else {
                eof_retries = 0;
                Ok(bytes_read as usize)
            }
        };

        #[cfg(not(windows))]
        let read_result = {
            use std::io::Read;
            let stdin = io::stdin();
            stdin.lock().read(&mut stdin_buf)
        };

        let n = match read_result {
            Ok(0) => break,
            Err(_) => break,
            Ok(n) => {
                total_bytes_read += n as u64;
                if let Some(ref mut f) = stdin_log_file {
                    // Log a printable ASCII dump of all bytes (replace control bytes with .)
                    // plus the byte count. Avoids 80-byte truncation in the hex dump.
                    let asc: String = stdin_buf[..n].iter()
                        .map(|&b| {
                            if b == b'\r' { "\\r".to_string() }
                            else if b == b'\n' { "\\n".to_string() }
                            else if b == b'\t' { "\\t".to_string() }
                            else if (0x20..0x7f).contains(&b) { (b as char).to_string() }
                            else { format!("\\x{:02x}", b) }
                        }).collect::<String>();
                    let _ = writeln!(f, "[{:>8.3}s] IN  ({} bytes): {}",
                        stdin_start.elapsed().as_secs_f64(), n, asc);
                    let _ = f.flush();
                }
                n
            }
        };
        // Translate bare \r to \n (iTerm2 compat), skip if already \r\n
        let mut out = Vec::with_capacity(n);
        let chunk = &stdin_buf[..n];
        for i in 0..n {
            if chunk[i] == b'\r' {
                if i + 1 < n && chunk[i + 1] == b'\n' {
                    // \r\n pair: keep as-is (the \n will be written next iteration)
                    out.push(b'\r');
                } else {
                    // Bare \r: translate to \n
                    out.push(b'\n');
                }
            } else {
                out.push(chunk[i]);
            }
        }
        if write_stream.write_all(&out).is_err() { break; }
        if write_stream.flush().is_err() { break; }
    }

    // After stdin EOF, shut down the TCP write side so the server sees
    // EOF and can clean up.  Then emit %exit + ST to stdout like real
    // tmux's client does (tmux/client.c).
    let _ = write_stream.shutdown(std::net::Shutdown::Write);
    if let Some(ref mut f) = stdin_log_file {
        let _ = writeln!(f, "[{:>8.3}s] stdin closed (total_bytes_read={}), TCP write shut down",
            stdin_start.elapsed().as_secs_f64(), total_bytes_read);
        let _ = f.flush();
    }

    // Wait briefly for the reader thread to drain remaining responses,
    // then forcibly close.  The server may take up to 5s (its read timeout)
    // to notice the client is gone.
    let handle = reader_thread;
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    std::thread::spawn(move || {
        let _ = handle.join();
        done2.store(true, std::sync::atomic::Ordering::Release);
    });
    // Drain for up to 2 seconds, then exit
    for _ in 0..40 {
        if done.load(std::sync::atomic::Ordering::Acquire) { break; }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Emit %exit and ST to stdout like real tmux's client does
    // (tmux/client.c). iTerm2 watches for %exit to leave tmux
    // integration mode cleanly.  ST (\x1b\\) terminates the DCS.
    {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        let _ = out.write_all(b"%exit\n");
        if mode == 2 {
            let _ = out.write_all(b"\x1b\\");
        }
        let _ = out.flush();
    }

    Ok(())
}

/// Returns `true` when stdout is a Windows console handle (ConPTY).
/// When stdout is a pipe (e.g. `ssh -T`), returns `false`.
#[cfg(windows)]
fn stdout_is_console() -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n: u32) -> *mut std::ffi::c_void;
        fn GetConsoleMode(h: *mut std::ffi::c_void, m: *mut u32) -> i32;
    }
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        if handle.is_null() || handle == (-1isize as *mut std::ffi::c_void) {
            return false;
        }
        let mut mode: u32 = 0;
        // GetConsoleMode succeeds only for console handles (not pipes/files)
        GetConsoleMode(handle, &mut mode) != 0
    }
}

/// Returns `true` when the process appears to be running inside an SSH session.
#[cfg(windows)]
fn is_ssh_session() -> bool {
    env::var("SSH_CLIENT").is_ok()
        || env::var("SSH_CONNECTION").is_ok()
        || env::var("SSH_TTY").is_ok()
}

/// Decide whether a detached-session readiness probe's `list-windows` reply
/// means the initial window exists. A non-empty body is >0 windows (the
/// tmux-text form is "" for zero windows) — EXCEPT a protocol-level error
/// reply, which is also non-empty. The startup .key write can race the
/// client's read, so the server may answer with an "ERROR: ..." auth failure;
/// that must NOT be mistaken for a ready window list.
fn detached_list_windows_ready(resp: &str) -> bool {
    let t = resp.trim();
    !t.is_empty() && !t.starts_with("ERROR:")
}

#[cfg(test)]
mod readiness_tests {
    use super::detached_list_windows_ready;

    #[test]
    fn nonempty_window_list_is_ready() {
        assert!(detached_list_windows_ready("0: bash* (1 panes) [80x24]\n"));
    }

    #[test]
    fn empty_reply_is_not_ready() {
        assert!(!detached_list_windows_ready(""));
        assert!(!detached_list_windows_ready("   \n"));
    }

    #[test]
    fn auth_error_reply_is_not_ready() {
        // The key read can race the server's .key write; an auth failure is
        // a non-empty reply but is NOT a ready window list.
        assert!(!detached_list_windows_ready("ERROR: Invalid session key\n"));
    }
}

#[cfg(test)]
mod sbai_7120_tests {
    use super::build_send_paste_control;

    #[test]
    fn send_paste_cli_forwards_one_base64_frame() {
        let args = vec!["send-paste", "IGxpbmUgb25lXG5saW5lIHR3byA="];

        assert_eq!(
            build_send_paste_control(&args).unwrap(),
            "send-paste IGxpbmUgb25lXG5saW5lIHR3byA=\n",
        );
    }

    #[test]
    fn send_paste_cli_rejects_missing_or_non_base64_payload() {
        assert!(build_send_paste_control(&["send-paste"]).is_err());
        assert!(build_send_paste_control(&["send-paste", "not base64"]).is_err());
        assert!(build_send_paste_control(&["send-paste", "YQ==", "Yg=="]).is_err());
        assert!(build_send_paste_control(&["send-paste", "-t"]).is_err());
    }

    #[test]
    fn send_paste_cli_preserves_optional_target_syntax() {
        assert_eq!(
            build_send_paste_control(&["send-paste", "-t", "dev:0.1", "YQ=="]).unwrap(),
            "send-paste YQ==\n",
        );
    }
}
