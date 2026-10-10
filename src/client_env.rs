//! Hand a claiming client's environment to the warm server it claims (#659).
//!
//! psmux keeps a parked `__warm__` server per namespace so that the next
//! `new-session` is instant.  That standby is spawned by whichever process
//! happened to need a server first (an earlier session's server, a shell
//! started before psmux was installed, `psmux` reached through WSL interop),
//! and claiming it only renames the session: the standby's PROCESS
//! environment is whatever it was born with, forever.
//!
//! On the cold path the server is spawned by the client itself
//! (`platform::spawn_server_hidden` passes a null `lpEnvironment`, so the
//! child inherits the client's block verbatim), which makes "the server's
//! environment is the environment of the shell the user ran psmux in" an
//! invariant everything downstream relies on: `run-shell` children, hooks,
//! plugin scripts and every pane spawned later inherit it.  The warm pool is
//! an optimisation that quietly breaks that invariant, and the visible
//! symptom is a plugin that cannot find psmux itself:
//!
//! ```text
//! psmux : The term 'psmux' is not recognized as the name of a cmdlet, ...
//! ```
//!
//! tmux has the same class of staleness for its server-wide base environment
//! (`tmux.c:418` fills `global_environ` once, from whoever started the
//! server) and mitigates it with the `update-environment` option, applied
//! from the CLIENT's environment at new-session and attach (`environ.c:186`,
//! `cmd-new-session.c:283`, `cmd-attach-session.c:135`).  In tmux, though,
//! the server is always started by a real client, so the base environment is
//! always SOME user shell's.  psmux's warm pool is the part tmux does not
//! have, so the fix is to restore psmux's own invariant at claim time: the
//! claimed standby adopts the claiming client's environment, which is exactly
//! what a cold spawn would have given it.
//!
//! The payload travels as a Windows environment block (UTF-16LE,
//! `KEY=VALUE\0` entries, double-NUL terminated) in a file next to the warm
//! server's handoff files, because an environment is far too large and too
//! full of separators to survive the single-line control protocol.

use std::ffi::{OsStr, OsString};

/// Variables the claimed server keeps as its OWN, never taking the client's
/// value and never dropping them when the client does not have them.
///
/// * `PSMUX_DATA_DIR` names the directory this server's `.port`/`.key`/`.pid`
///   files already live in.  Re-pointing it mid-life would orphan them.
/// * `PSMUX_TARGET_SESSION` is this server's identity for its own children,
///   and the claim arm re-asserts it immediately afterwards anyway.  A client
///   started from inside ANOTHER psmux pane carries the outer session's value,
///   which must never become this server's.
const SERVER_OWNED_VARS: &[&str] = &["PSMUX_DATA_DIR", "PSMUX_TARGET_SESSION"];

/// A payload with fewer entries than this, or without a usable `PATH`, is
/// treated as corrupt and ignored: keeping the standby's stale environment is
/// survivable, spawning shells with a half-empty one is not.
const MIN_SANE_ENTRIES: usize = 4;

/// What an adoption would do, computed without touching the process
/// environment so it can be unit-tested.
#[derive(Debug, Default, PartialEq)]
pub struct EnvPlan {
    /// Variables to set (or overwrite) on the server.
    pub set: Vec<(OsString, OsString)>,
    /// Variables the server currently has and the client does not.
    pub remove: Vec<OsString>,
}

impl EnvPlan {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.remove.is_empty()
    }

    /// Does this plan set or remove `name` (case insensitive)?
    pub fn touches(&self, name: &str) -> bool {
        let n = name.to_uppercase();
        self.set.iter().any(|(k, _)| norm_key(k) == n)
            || self.remove.iter().any(|k| norm_key(k) == n)
    }
}

/// Windows environment variable names are case insensitive, so every lookup
/// goes through one normalised key.
fn norm_key(k: &OsStr) -> String {
    k.to_string_lossy().to_uppercase()
}

/// Is this a variable the claimed server keeps as its own?
pub fn is_server_owned(name: &OsStr) -> bool {
    let n = norm_key(name);
    SERVER_OWNED_VARS.iter().any(|k| *k == n)
}

/// Can this name be handed to `std::env::set_var` / `remove_var` at all?
///
/// Those functions PANIC rather than fail, and this code walks two
/// environments it did not author: the server's own (`vars_os`) and a block
/// read off disk.  Measured on rustc 1.96.0 / Windows 11 with a standalone
/// probe:
///
/// ```text
/// vars_os really hands out cmd.exe's per drive variables: ["=C:", "=ExitCode"]
/// set_var("=C:", ..)            panicked: false   (a LEADING '=' is legal)
/// set_var("A=B", ..)            panicked: true    (os error 87)
/// set_var("", ..)               panicked: true
/// set_var("X", "a\0b")          panicked: true
/// ```
///
/// So an interior `=`, an empty name and a NUL are the shapes that must never
/// reach the API. The cmd.exe `=C:` entries are legal but pointless to carry,
/// and this filter drops them too, which also stops a claim from deleting the
/// server's own per drive working directories.
pub fn is_settable(name: &OsStr) -> bool {
    let wide = to_wide(name);
    !wide.is_empty()
        && !wide.contains(&(b'=' as u16))
        && !wide.contains(&0)
}

/// Encode `vars` as a Windows environment block: UTF-16LE `KEY=VALUE` entries,
/// each NUL terminated, the whole block terminated by one more NUL.
pub fn encode_env_block<I>(vars: I) -> Vec<u8>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut units: Vec<u16> = Vec::new();
    for (k, v) in vars {
        let kw = to_wide(&k);
        // An empty name, or a name starting with '=' (cmd.exe's "=C:" drive
        // variables), is unusable in a block and is what makes CreateProcessW
        // reject the WHOLE block with error 87. Drop those entries here rather
        // than hand the server something it cannot spawn a shell with.
        if kw.is_empty() || kw[0] == b'=' as u16 || kw.contains(&0) {
            continue;
        }
        units.extend_from_slice(&kw);
        units.push(b'=' as u16);
        units.extend(to_wide(&v).into_iter().take_while(|&c| c != 0));
        units.push(0);
    }
    units.push(0);
    let mut bytes = Vec::with_capacity(units.len() * 2);
    for u in units {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    bytes
}

/// Decode what [`encode_env_block`] produced.  Returns `None` for a payload
/// that is not a whole number of UTF-16 units.
pub fn decode_env_block(bytes: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    if bytes.len() % 2 != 0 {
        return None;
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut out = Vec::new();
    for entry in units.split(|&u| u == 0) {
        if entry.is_empty() {
            continue;
        }
        // The name ends at the first '=' that is not the first character.
        let eq = match entry.iter().skip(1).position(|&u| u == b'=' as u16) {
            Some(i) => i + 1,
            None => continue,
        };
        out.push((from_wide(&entry[..eq]), from_wide(&entry[eq + 1..])));
    }
    Some(out)
}

/// Would this payload leave the server able to spawn a shell at all?
pub fn is_sane(entries: &[(OsString, OsString)]) -> bool {
    if entries.len() < MIN_SANE_ENTRIES {
        return false;
    }
    entries
        .iter()
        .any(|(k, v)| norm_key(k) == "PATH" && !v.is_empty())
}

/// Work out how to turn `current` (the server's environment) into `incoming`
/// (the claiming client's), leaving the server-owned variables alone.
///
/// Returns an empty plan when `incoming` is not sane, which is how a corrupt
/// or truncated payload becomes a no-op instead of a broken server.
pub fn plan_adoption(
    current: &[(OsString, OsString)],
    incoming: &[(OsString, OsString)],
) -> EnvPlan {
    let mut plan = EnvPlan::default();
    if !is_sane(incoming) {
        return plan;
    }
    let incoming_keys: std::collections::HashSet<String> =
        incoming.iter().map(|(k, _)| norm_key(k)).collect();
    for (k, _) in current {
        if is_server_owned(k) || !is_settable(k) {
            continue;
        }
        if !incoming_keys.contains(&norm_key(k)) {
            plan.remove.push(k.clone());
        }
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let current_map: std::collections::HashMap<String, &OsString> =
        current.iter().map(|(k, v)| (norm_key(k), v)).collect();
    for (k, v) in incoming {
        // set_var panics on a value with an interior NUL too. A decoded block
        // cannot hold one (NUL is its separator), but the file is read off
        // disk, so this stays a check rather than an assumption.
        if is_server_owned(k) || !is_settable(k) || to_wide(v).contains(&0) {
            continue;
        }
        let nk = norm_key(k);
        if !seen.insert(nk.clone()) {
            continue;
        }
        // Setting a variable to the value it already has is a no-op with a
        // cost, and the common case (same machine, same user) is that most of
        // the block is identical.
        if current_map.get(&nk).map(|cur| cur.as_os_str() == v.as_os_str()) == Some(true) {
            continue;
        }
        plan.set.push((k.clone(), v.clone()));
    }
    plan
}

/// Apply a plan to this process's environment.
pub fn apply_plan(plan: &EnvPlan) {
    for k in &plan.remove {
        std::env::remove_var(k);
    }
    for (k, v) in &plan.set {
        std::env::set_var(k, v);
    }
}

/// Read the block `path` holds, adopt it, and delete the file.  Returns the
/// plan that was applied (empty when the file is missing or unusable).
pub fn adopt_from_file(path: &str) -> EnvPlan {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return EnvPlan::default(),
    };
    let _ = std::fs::remove_file(path);
    let incoming = match decode_env_block(&bytes) {
        Some(e) => e,
        None => return EnvPlan::default(),
    };
    let current: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let plan = plan_adoption(&current, &incoming);
    apply_plan(&plan);
    plan
}

/// Client side: write this process's environment where the server it is about
/// to claim can read it.  Returns the path on success.
pub fn write_own_environment(path: &str) -> Option<String> {
    let block = encode_env_block(std::env::vars_os());
    match std::fs::write(path, &block) {
        Ok(()) => Some(path.to_string()),
        Err(_) => None,
    }
}

/// Path of the handoff file for a warm server, next to its `.port`/`.key`.
pub fn claim_env_file(warm_base: &str) -> String {
    crate::paths::psmux_dir_file(format!("{}.claimenv.{}", warm_base, std::process::id()))
}

#[cfg(windows)]
fn to_wide(s: &OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().collect()
}

#[cfg(not(windows))]
fn to_wide(s: &OsStr) -> Vec<u16> {
    s.to_string_lossy().encode_utf16().collect()
}

#[cfg(windows)]
fn from_wide(u: &[u16]) -> OsString {
    use std::os::windows::ffi::OsStringExt;
    OsString::from_wide(u)
}

#[cfg(not(windows))]
fn from_wide(u: &[u16]) -> OsString {
    OsString::from(String::from_utf16_lossy(u))
}

#[cfg(test)]
#[path = "../tests-rs/test_issue659_claim_environment.rs"]
mod test_issue659_claim_environment;
