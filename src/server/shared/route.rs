//! In process routing between the sessions of one shared server (#798 step 3,
//! item 1). Pure: no sockets, no state.
//!
//! A per session server holds one session, so a command run from inside it
//! (a hook, or a line the server sends its own listener) whose target names
//! another session can only fail with tmux's "can't find session". The shared
//! server holds every session of the namespace, so, as in tmux, such a command
//! runs in the session its target names. These functions decide which one;
//! `server.rs` then hands the command to that session's handler or request
//! queue inside the process, never over TCP to the sibling's listener (which
//! the loop thread would have to serve while it waits).

/// What routing needs to know about one session of the server.
#[derive(Clone, Copy, Debug)]
pub struct SessionRef<'a> {
    /// The server's key for the session (its session id).
    pub sid: usize,
    pub name: &'a str,
    /// The registry base, `<ns>__<name>` or `<name>`.
    pub base: &'a str,
}

/// The session `spec` names among `sessions`, by tmux's rules
/// (cmd-find.c `cmd_find_get_session`): `$N` is the session id; an exact name
/// (or the registry base a resolved `$N` carries) wins; otherwise a prefix of
/// exactly one session's name. `=name` allows only the exact match. Two
/// sessions sharing the prefix is no match (tmux: ambiguous), and so is none.
pub fn find_session(spec: &str, sessions: &[SessionRef<'_>]) -> Option<usize> {
    let exact_only = spec.starts_with('=');
    let s = crate::cli::strip_exact_match_prefix(spec.trim());
    if s.is_empty() {
        return None;
    }
    if let Some(id) = s.strip_prefix('$') {
        let id: usize = id.parse().ok()?;
        return sessions.iter().find(|r| r.sid == id).map(|r| r.sid);
    }
    if let Some(r) = sessions.iter().find(|r| r.name == s || r.base == s) {
        return Some(r.sid);
    }
    if exact_only {
        return None;
    }
    let mut hits = sessions.iter().filter(|r| r.name.starts_with(s));
    match (hits.next(), hits.next()) {
        (Some(r), None) => Some(r.sid),
        _ => None,
    }
}

/// Commands whose `-t` is not a target inside the session they run in, so the
/// line is never moved for it: switch-client's is the DESTINATION (handled in
/// the loop, see `switch_client_destination`), detach-client's and the other
/// client commands' a client, new-session's a group, set-buffer's a client,
/// and the move/link/join/swap commands may name two sessions at once (still
/// refused in shared mode).
const NOT_ROUTED: &[&str] = &[
    "switch-client", "switchc", "detach-client", "detach",
    "new-session", "new", "attach-session", "attach", "a", "at",
    "refresh-client", "refresh", "lock-client", "lockc",
    "suspend-client", "suspendc", "show-messages", "showmsgs",
    "display-panes", "displayp", "set-buffer", "setb",
    "link-window", "linkw", "move-window", "movew",
    "join-pane", "joinp", "move-pane", "movep",
    "swap-window", "swapw", "swap-pane", "swapp", "break-pane", "breakp",
];

/// Commands whose `-t` is a SESSION in tmux (CMD_FIND_SESSION), so a bare word
/// naming a session is that session. For every other command only the
/// explicit forms (`sess:...`, `$N`) name a session: a bare word may be a
/// window of the current session, which a connection thread cannot see.
const SESSION_TARGET: &[&str] = &[
    "kill-session", "kill-ses", "has-session", "has",
    "list-windows", "lsw",
    "set-environment", "setenv", "show-environment", "showenv",
];

/// The `-t` value of one command's words: `-t VALUE` or `-tVALUE`. Flag
/// clusters (`-dt VALUE`) are not read, so such a line is never moved.
fn target_value(words: &[String]) -> Option<&str> {
    let mut i = 1;
    while i < words.len() {
        let w = words[i].as_str();
        if w == "--" {
            return None;
        }
        if w == "-t" {
            return words.get(i + 1).map(String::as_str);
        }
        if let Some(v) = w.strip_prefix("-t") {
            if !v.is_empty() {
                return Some(v);
            }
        }
        i += 1;
    }
    None
}

/// The session part a command line's target names explicitly, if the line
/// is one command whose target must be in the session it runs in. A `;`
/// list is not moved (its commands could name different sessions).
pub fn session_spec_of_line(line: &str) -> Option<String> {
    let parts = crate::config::split_chained_commands_pub(line.trim());
    if parts.len() != 1 {
        return None;
    }
    let words = crate::commands::parse_command_line(&parts[0]);
    let name = words.first()?.as_str();
    if NOT_ROUTED.contains(&name) {
        return None;
    }
    let t = target_value(&words)?.trim();
    let bare = crate::cli::strip_exact_match_prefix(t);
    if bare.starts_with('%') || bare.starts_with('@') {
        return None;
    }
    if let Some((session, _)) = t.split_once(':') {
        return (!crate::cli::strip_exact_match_prefix(session).is_empty()).then(|| session.to_string());
    }
    if bare.starts_with('$') || SESSION_TARGET.contains(&name) {
        return Some(t.to_string());
    }
    None
}

/// The session a command line arriving on session `own`'s listener must run
/// in instead, when its target names another session of this server.
pub fn sibling_for_line(line: &str, own: usize, sessions: &[SessionRef<'_>]) -> Option<usize> {
    let spec = session_spec_of_line(line)?;
    find_session(&spec, sessions).filter(|sid| *sid != own)
}

/// The kind of id a target names on its own (`%N` or `@N`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdKind {
    Pane,
    Window,
}

/// The session a `switch-client -t RAW` sent to session `own` must select a
/// window or pane in before the client moves there: one whose session part
/// names another session of this server together with a window or pane, or
/// a bare `%N` / `@N` (ids are server wide) that `own` does not hold but a
/// sibling does. None leaves the request to the session's own handler, which
/// switches for a session target and selects for one of its own.
/// `holds(sid, kind, id)` says whether session `sid` holds that id.
pub fn switch_client_destination(
    raw: &str,
    own: usize,
    sessions: &[SessionRef<'_>],
    holds: impl Fn(usize, IdKind, usize) -> bool,
) -> Option<usize> {
    let pt = crate::cli::parse_target(raw);
    let names_part = pt.window.is_some() || pt.window_name.is_some() || pt.pane.is_some();
    if let Some(s) = pt.session.as_deref().filter(|s| !s.is_empty()) {
        if !names_part {
            return None;
        }
        let spec = if raw.trim_start().starts_with('=') { format!("={}", s) } else { s.to_string() };
        return find_session(&spec, sessions).filter(|sid| *sid != own);
    }
    let (kind, id) = match (pt.pane, pt.window) {
        (Some(p), _) if pt.pane_is_id => (IdKind::Pane, p),
        (None, Some(w)) if pt.window_is_id => (IdKind::Window, w),
        _ => return None,
    };
    if holds(own, kind, id) {
        return None;
    }
    sessions.iter().map(|r| r.sid).find(|sid| *sid != own && holds(*sid, kind, id))
}

/// `raw` with its session part removed (`B:1.2` is `:1.2`), so the
/// destination session's own handler reads it as a target of its own.
pub fn within_session(raw: &str) -> String {
    match raw.split_once(':') {
        Some((_, rest)) => format!(":{}", rest),
        None => raw.to_string(),
    }
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue798_shared_routing.rs"]
mod tests_issue798_shared_routing;
