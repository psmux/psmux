//! The shared multi session server (issue #798), behind its switch.
//!
//! This module is the ONLY entry into shared mode. Owner ruling
//! (docs/design/multi-session-server.md, top section): the shared server is a
//! switch, everything for it lives behind that switch, and with the switch off
//! psmux runs exactly the code it ran before. So the default path only ever
//! calls the gates below, at the places a server is about to be started, and a
//! gate returns at once when the mode resolves to `per-session`.
//!
//! The switch, step 1 of #798:
//!
//! * `PSMUX_SERVER_MODE` in the environment of the process about to start a
//!   server. `per-session` (the default) or `shared`. Unset, empty or only
//!   whitespace means unset. Case and surrounding spaces are ignored.
//! * otherwise `set -s server-mode <value>` (any `set` / `set-option` flags)
//!   at the top level of the user's config file, the same file and the same
//!   lightweight scan the `warm` and `priority` options use before a server
//!   exists. The last such line wins, as it would when the file is applied.
//! * otherwise `per-session`.
//!
//! The environment outranks the config, like `PSMUX_PRIORITY` outranks
//! `priority`, so a shell can step out of a configured mode without editing a
//! file. Any other value is an error that names the value and the accepted
//! ones, exit 1, and no server is started: a typo must never quietly run the
//! other mode.
//!
//! In this step `shared` starts nothing. It reports that the shared server is
//! not in this build yet and exits 1. It never falls back to per session.

use std::io::Write;

/// The name of the environment switch.
pub const ENV_VAR: &str = "PSMUX_SERVER_MODE";
/// The name of the server option.
pub const OPTION: &str = "server-mode";
/// The accepted values, in the order an error lists them.
pub const VALUES: &[&str] = &["per-session", "shared"];

/// Which kind of server a namespace start should create.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerMode {
    /// One server process per session: everything psmux did before #798.
    PerSession,
    /// One server per namespace hosting many sessions (tmux's model).
    Shared,
}

impl ServerMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ServerMode::PerSession => "per-session",
            ServerMode::Shared => "shared",
        }
    }
}

/// One value of the switch. `Ok(None)` is "not set" (empty or whitespace).
pub fn parse_value(raw: &str) -> Result<Option<ServerMode>, ()> {
    let v = raw.trim().trim_matches('"').trim_matches('\'').trim();
    if v.is_empty() {
        return Ok(None);
    }
    match v.to_ascii_lowercase().as_str() {
        "per-session" => Ok(Some(ServerMode::PerSession)),
        "shared" => Ok(Some(ServerMode::Shared)),
        _ => Err(()),
    }
}

fn expected() -> String {
    VALUES.join(" or ")
}

fn env_error(raw: &str) -> String {
    format!("{}: unknown value '{}' (expected {})", ENV_VAR, raw.trim(), expected())
}

fn config_error(raw: &str, source: &str) -> String {
    format!(
        "{}: unknown value '{}' in {} (expected {})",
        OPTION,
        raw.trim(),
        source,
        expected()
    )
}

/// The `server-mode` a config file sets, if any. `Ok(None)` when no top level
/// line sets it. `set -u server-mode` resets it to the default.
///
/// Only top level `set` / `set-option` commands are read, a line or a `;`
/// separated piece of one at a time, exactly like the CLI's existing peeks at
/// `warm` and `priority`. Lines inside `if-shell` or a sourced file are not
/// seen here; that is documented in the design doc.
pub fn mode_from_config_text(text: &str, source: &str) -> Result<Option<ServerMode>, String> {
    let mut found: Option<ServerMode> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        for piece in line.split(';') {
            let parts: Vec<&str> = piece.split_whitespace().collect();
            if parts.len() < 2 || (parts[0] != "set" && parts[0] != "set-option") {
                continue;
            }
            let mut i = 1;
            let mut unset = false;
            while i < parts.len() && parts[i].starts_with('-') {
                if parts[i].contains('u') {
                    unset = true;
                }
                i += 1;
            }
            if parts.get(i).map(|n| n.trim_matches('"').trim_matches('\'')) != Some(OPTION) {
                continue;
            }
            if unset {
                found = Some(ServerMode::PerSession);
                continue;
            }
            let raw = parts.get(i + 1).copied().unwrap_or("");
            if raw.starts_with('#') || raw.is_empty() {
                return Err(config_error(raw, source));
            }
            match parse_value(raw) {
                Ok(Some(mode)) => found = Some(mode),
                _ => return Err(config_error(raw, source)),
            }
        }
    }
    Ok(found)
}

/// Resolve the mode from an environment value and a config file's text.
/// Pure, so the unit tests can drive every combination.
pub fn resolve(
    env_value: Option<&str>,
    config: Option<(&str, &str)>,
) -> Result<ServerMode, String> {
    if let Some(raw) = env_value {
        match parse_value(raw) {
            Ok(Some(mode)) => return Ok(mode),
            Ok(None) => {}
            Err(()) => return Err(env_error(raw)),
        }
    }
    if let Some((source, text)) = config {
        if let Some(mode) = mode_from_config_text(text, source)? {
            return Ok(mode);
        }
    }
    Ok(ServerMode::PerSession)
}

/// The mode this process would start a server in, from its own environment
/// and the user's config file.
pub fn resolve_for_this_process() -> Result<ServerMode, String> {
    let env_value = std::env::var(ENV_VAR).ok();
    // Fast exit for the explicit default: no config read at all.
    if let Some(ref raw) = env_value {
        if let Ok(Some(mode)) = parse_value(raw) {
            return Ok(mode);
        }
    }
    let config_text = crate::config::read_user_config_content();
    let source = std::env::var("PSMUX_CONFIG_FILE").unwrap_or_else(|_| "the config file".to_string());
    resolve(
        env_value.as_deref(),
        config_text.as_deref().map(|t| (source.as_str(), t)),
    )
}

/// The message `shared` gives while the shared server is not built yet.
pub fn not_available_message() -> String {
    "shared server mode is not available in this build yet (step 1 of #798); \
     unset PSMUX_SERVER_MODE or set server-mode per-session to use one server per session"
        .to_string()
}

fn die(msg: &str) -> ! {
    let _ = std::io::stdout().flush();
    eprintln!("psmux: {}", msg);
    std::process::exit(1);
}

/// Called by the CLI where it is about to start a server (or claim the warm
/// standby, which is a per session server). Returns only for `per-session`,
/// and then the caller runs exactly the code it always ran.
///
/// `shared` is entered here and never comes back: it must not be served by a
/// per session server, the warm standby included. In step 1 it reports that
/// the mode is not built yet.
pub fn gate_server_start(_ns: Option<&str>) {
    match resolve_for_this_process() {
        Ok(ServerMode::PerSession) => {}
        Ok(ServerMode::Shared) => run_shared_cli(),
        Err(e) => die(&e),
    }
}

/// The entry of shared mode on the CLI side. Step 1: not built yet.
fn run_shared_cli() -> ! {
    die(&not_available_message())
}

/// The same decision for a start a running server makes on a user's behalf
/// (`new-session` from a binding, the command prompt or a persistent client),
/// where exiting is not an option: `Err` carries the message to show, and the
/// caller starts and claims nothing. With the switch at `per-session` the
/// caller goes on exactly as before.
pub fn check_server_start() -> Result<(), String> {
    match resolve_for_this_process() {
        Ok(ServerMode::PerSession) => Ok(()),
        Ok(ServerMode::Shared) => Err(not_available_message()),
        Err(e) => Err(e),
    }
}

/// Called at the very top of the internal `psmux server` process, the one
/// place every server process starts, however it was spawned (a CLI, another
/// server's `new-session`, the warm standby refill). A process whose switch
/// says anything but `per-session` must not go on to run the per session
/// server, so it exits 1 before it writes a registry file.
pub fn gate_server_process() {
    match resolve_for_this_process() {
        Ok(ServerMode::PerSession) => {}
        Ok(ServerMode::Shared) => die(&not_available_message()),
        Err(e) => die(&e),
    }
}

/// What `show -s server-mode` answers on a per session server: the mode it
/// runs in, which is fixed for its life.
pub const RUNNING_PER_SESSION: &str = "per-session";

/// `set server-mode VALUE` on a running per session server. The mode is
/// chosen when a server starts and a running server cannot change it, but the
/// startup config (and a later `source-file` of it) naming the mode the server
/// already runs in must not warn, so that value is accepted and changes
/// nothing. It is the value check `set-option` runs (option_catalog
/// validate_option_value) on the CLI and in the server; in step 1 every running
/// server is a per session one, so judging the value there is exact. An unset
/// never reaches here (the option has no catalog default,
/// so `set -u` has nothing to apply) and leaves the mode as it is.
pub fn set_on_running_per_session_server(value: &str) -> Result<(), String> {
    match parse_value(value) {
        Ok(Some(ServerMode::PerSession)) => Ok(()),
        Ok(None) => Err(format!("unknown value: {}", value.trim())),
        Ok(Some(ServerMode::Shared)) => Err(format!(
            "{} cannot change on a running server: this server runs per-session; \
             set {} or {} before the server starts",
            OPTION, ENV_VAR, OPTION
        )),
        Err(()) => Err(format!("unknown value: {}", value.trim())),
    }
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue798_server_mode_switch.rs"]
mod tests_issue798_server_mode_switch;
