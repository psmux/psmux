//! Registry layout of shared mode (#798 step 2, settles #844 for this mode).
//!
//! Sessions: exactly today's per session files, same names, same contents.
//! Each session hosted by a shared server has its own `<base>.port` (its own
//! loopback listener inside the one process), `<base>.key` (its own key),
//! `<base>.sid` and `<base>.pid` (the shared server's pid, the same for all of
//! its sessions). `<base>` is `<ns>__<session>` under `-L ns` and `<session>`
//! in the default namespace, as today. Nothing that reads the registry needs
//! to know a session lives in a shared server: `ls`, `attach`, `has-session`,
//! `kill-session`, `kill-server`, `$TMUX` and older CLIs all work unchanged,
//! and because every session keeps its own port, `$TMUX` still names one
//! session and never needs a tie break.
//!
//! Namespace marker: one file per shared server, in a directory of its own so
//! it can never be mistaken for a session file:
//!
//! * `<data>/shared/default.server` for the default namespace,
//! * `<data>/shared/L-<ns>.server` for `-L <ns>`.
//!
//! The `L-` prefix keeps the default namespace's marker apart from every `-L`
//! name (`-L default` is `L-default.server`), the collision #844 points out
//! for a marker named after the namespace alone. The marker holds the server's
//! `pid`, its process `creation` time (so a recycled pid is never mistaken for
//! the server), and the `port` and `key` of the server's control listener,
//! which is where a CLI sends `new-session` for that namespace. Older psmux
//! versions never look in `shared/`.

use std::path::{Path, PathBuf};

/// The namespace marker of one shared server.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Marker {
    pub pid: u32,
    pub creation: u64,
    pub port: u16,
    pub key: String,
}

/// The directory that holds the markers, inside a data directory.
pub fn marker_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("shared")
}

/// The marker's file name for a namespace (`None` = the default one).
pub fn marker_file_name(ns: Option<&str>) -> String {
    match ns {
        None => "default.server".to_string(),
        Some(n) => format!("L-{}.server", n),
    }
}

/// The marker's path inside a data directory.
pub fn marker_path_in(data_dir: &Path, ns: Option<&str>) -> PathBuf {
    marker_dir_in(data_dir).join(marker_file_name(ns))
}

/// The marker's path in this process's data directory.
pub fn marker_path(ns: Option<&str>) -> PathBuf {
    marker_path_in(Path::new(&crate::paths::psmux_dir()), ns)
}

/// The marker's text.
pub fn format_marker(m: &Marker) -> String {
    format!(
        "mode=shared\npid={}\ncreation={}\nport={}\nkey={}\n",
        m.pid, m.creation, m.port, m.key
    )
}

/// Parse a marker's text. `None` for anything incomplete or malformed.
pub fn parse_marker(text: &str) -> Option<Marker> {
    let mut pid = None;
    let mut creation = None;
    let mut port = None;
    let mut key = None;
    let mut shared = false;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "mode" => shared = v == "shared",
            "pid" => pid = v.parse::<u32>().ok(),
            "creation" => creation = v.parse::<u64>().ok(),
            "port" => port = v.parse::<u16>().ok().filter(|p| *p != 0),
            "key" => key = Some(v.to_string()).filter(|k| !k.is_empty()),
            _ => {}
        }
    }
    if !shared {
        return None;
    }
    Some(Marker { pid: pid?, creation: creation?, port: port?, key: key? })
}

/// Write the marker so a reader never sees half of it: a temporary file in
/// the same directory, then a rename over the old one.
pub fn write_marker(path: &Path, m: &Marker) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("server.{}.tmp", m.pid));
    std::fs::write(&tmp, format_marker(m))?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Remove the marker, but only if it is still this server's.
pub fn remove_marker_if_ours(path: &Path, pid: u32) {
    let ours = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| parse_marker(&t))
        .is_some_and(|m| m.pid == pid);
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// True when the process the marker names is still that same process.
pub fn marker_process_alive(m: &Marker) -> bool {
    crate::platform::process_kill::process_creation_time(m.pid) == Some(m.creation)
}

/// The live shared server of a namespace, if there is one.
pub fn live_marker(ns: Option<&str>) -> Option<Marker> {
    live_marker_at(&marker_path(ns))
}

/// The marker at `path` when the process it names is still that same
/// process. A stale marker (its process gone, or its pid recycled) is NOT
/// removed here (#897 A): between reading it and removing it, another CLI may
/// have started the namespace's server and renamed ITS marker into place, and
/// the removal would then unpublish that live server, so every later
/// `new-session` waits for a marker that never comes. The next server's
/// `write_marker` renames over a stale file, which is all the cleanup it needs.
pub fn live_marker_at(path: &Path) -> Option<Marker> {
    let m = parse_marker(&std::fs::read_to_string(path).ok()?)?;
    marker_process_alive(&m).then_some(m)
}
