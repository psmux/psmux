//! Single source of truth for psmux's home and data directories.
//!
//! Every `.port`/`.key`/`.sid`/warm-pool/log path routes through [`psmux_dir`]
//! (or [`psmux_dir_opt`]), so client and server can never disagree about where
//! the bookkeeping lives.

/// User home directory: `USERPROFILE`, then the Windows profile API, then
/// `HOMEDRIVE`+`HOMEPATH`, then `HOME`. Empty string when nothing resolves.
///
/// `HOME` is deliberately the LAST resort on Windows (issue #474): MSYS2
/// login shells unset `USERPROFILE` and set `HOME` to a POSIX-style path
/// (`/home/user`), so a psmux invocation from an MSYS2 shell that trusted
/// `HOME` would resolve the data dir somewhere the registry does not live.
/// The startup orphan reaper would then see every live server as untracked
/// and terminate them all. Querying the profile API keeps every invocation
/// of the same user converging on the same data dir regardless of which
/// shell launched it.
pub fn home_dir() -> String {
    if let Ok(v) = std::env::var("USERPROFILE") {
        if !v.is_empty() {
            return v;
        }
    }
    #[cfg(windows)]
    if let Some(p) = windows_profile_dir() {
        return p;
    }
    let drive = std::env::var("HOMEDRIVE").unwrap_or_default();
    let path = std::env::var("HOMEPATH").unwrap_or_default();
    if !drive.is_empty() && !path.is_empty() {
        let p = format!("{}{}", drive, path);
        if std::path::Path::new(&p).is_dir() {
            return p;
        }
    }
    std::env::var("HOME").unwrap_or_default()
}

/// The current user's profile directory straight from the OS
/// (`GetUserProfileDirectoryW` on the process token), independent of any
/// environment variable a shell may have rewritten or unset.
#[cfg(windows)]
fn windows_profile_dir() -> Option<String> {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(h: isize) -> i32;
    }
    // Advapi32, not kernel32: `OpenProcessToken` is documented under
    // Advapi32.lib / Advapi32.dll, and it is not in kernel32.lib.
    // platform.rs:1142 already declares it under advapi32.
    #[link(name = "advapi32")]
    extern "system" {
        // isize handles to match the declarations in platform.rs
        // (clashing_extern_declarations, reported on PR 682); cast at the
        // call site.
        fn OpenProcessToken(process: isize, access: u32, token: *mut isize) -> i32;
    }
    #[link(name = "userenv")]
    extern "system" {
        fn GetUserProfileDirectoryW(token: *mut c_void, buf: *mut u16, len: *mut u32) -> i32;
    }
    const TOKEN_QUERY: u32 = 0x0008;
    unsafe {
        let mut token: isize = 0;
        if OpenProcessToken(GetCurrentProcess() as isize, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = GetUserProfileDirectoryW(token as *mut c_void, buf.as_mut_ptr(), &mut len);
        CloseHandle(token);
        if ok == 0 || len == 0 {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
        if s.is_empty() { None } else { Some(s) }
    }
}

/// Validate and normalize a PSMUX_DATA_DIR override value. Panics on a
/// relative or empty value: silently resolving against the CWD would scatter
/// the registry across working directories. Kept separate from the env read so
/// the rejection can be tested without ever placing an invalid value in the
/// process-global environment, which unlocked readers may observe mid-test.
fn normalize_data_dir_override(raw: &std::ffi::OsStr) -> String {
    let path = std::path::PathBuf::from(raw);
    assert!(
        path.is_absolute() && !path.as_os_str().is_empty(),
        "PSMUX_DATA_DIR must be an absolute non-empty path"
    );
    path.to_string_lossy().trim_end_matches(['/', '\\']).to_string()
}

/// psmux data directory, without a trailing separator. Fallible variant for
/// call sites that early-exit when home is missing (`.ok()?` /
/// `Err(_) => return …`). `None` when no home directory is available.
pub fn psmux_dir_opt() -> Option<String> {
    if let Some(raw) = std::env::var_os("PSMUX_DATA_DIR") {
        return Some(normalize_data_dir_override(&raw));
    }
    let home = home_dir();
    if home.is_empty() {
        None
    } else {
        Some(format!("{}\\.psmux", home))
    }
}

/// psmux data directory, without a trailing separator. Infallible variant for
/// call sites that today use `.unwrap_or_default()`.
///
/// When no home directory is set, returns `\.psmux` — byte-identical to the
/// historical `format!("{}\.psmux", "")`. It does not panic: the historical
/// behavior is to proceed and let the filesystem call fail gracefully, and a
/// crash on an unset `USERPROFILE` would be worse.
pub fn psmux_dir() -> String {
    psmux_dir_opt().unwrap_or_else(|| "\\.psmux".to_string())
}

/// The data root reduced to a comparison key: separators unified and case
/// folded, because Windows treats `C:/Foo` and `c:\foo` as one directory and
/// anything keyed on the root must treat them as one too.
fn normalized_data_root() -> String {
    psmux_dir().replace('/', "\\").to_lowercase()
}

/// Stable per-data-root tag for names that live in the machine-wide kernel
/// object namespace (issue #599).
///
/// `PSMUX_DATA_DIR` namespaces the registry: every `.port`/`.key`/`.sid`/`.pid`
/// file, and therefore every fact the single-server guard protects, lives under
/// it. That guard's mutex, though, is a machine-wide name. Without the root in
/// the key two independent registries collide on one session name and the
/// second root's server is refused as a duplicate of a server it cannot even
/// see -- including the fixed `__warm__` name, which only the first root then
/// ever publishes. `-L` already reaches the key through `port_file_base()`;
/// this makes psmux's other namespace mechanism agree.
///
/// Derived from the RESOLVED root rather than the raw variable, so pointing
/// `PSMUX_DATA_DIR` at the default `~\.psmux` keys identically to leaving it
/// unset.
pub fn data_root_tag() -> String {
    use std::hash::{Hash, Hasher};
    // A fixed-seed hasher: the name must be identical across processes and
    // runs, so `RandomState` is deliberately NOT used here (same reasoning as
    // `namespace_instance_file`).
    let mut h = std::collections::hash_map::DefaultHasher::new();
    normalized_data_root().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Path to a fixed-name file directly under the data directory (e.g.
/// `latency.log`, `last_session`, `next_session_id`, `crash.log`).
pub fn psmux_dir_file(name: impl AsRef<str>) -> String {
    format!("{}\\{}", psmux_dir(), name.as_ref())
}

/// Path to a session's `.port` file (the TCP port its server listens on).
pub fn port_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.port", psmux_dir(), session.as_ref())
}

/// Directory holding one ownership marker per server process this data dir
/// started (issue #510).
///
/// The startup reaper enumerates candidate servers machine-wide, but its
/// registry only describes one data dir. Without a per-process record of
/// "this one is mine" it cannot tell an orphan of its own from another
/// instance's healthy server, and killing on that ambiguity destroyed live
/// sessions belonging to a different USERPROFILE/HOME. A server writes its
/// marker into its OWN data dir, so ownership is self-declared and needs no
/// inspection of other processes.
pub fn server_marker_dir() -> String {
    format!("{}\\servers", psmux_dir())
}

/// Path to one server process's ownership marker (see `server_marker_dir`).
pub fn server_marker_file(pid: u32) -> String {
    format!("{}\\{}", server_marker_dir(), pid)
}

/// Fallible variant of [`port_file`]: `None` when no data directory can be
/// determined. Lets a call site early-exit on the same condition without having
/// to know that `port_file` routes through `psmux_dir`.
pub fn port_file_opt(session: impl AsRef<str>) -> Option<String> {
    psmux_dir_opt().map(|dir| format!("{}\\{}.port", dir, session.as_ref()))
}

/// Path to a session's `.key` file (the auth key for its server).
pub fn key_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.key", psmux_dir(), session.as_ref())
}

/// Path to a session's `.sid` file (its stable session id).
pub fn sid_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.sid", psmux_dir(), session.as_ref())
}

/// Path to a session's `.pid` file (the liveness anchor: the server pid).
pub fn pid_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.pid", psmux_dir(), session.as_ref())
}

/// Path to the remembered host palette, shared by every server in this data
/// root.
///
/// A server learns its terminal's colours only when a client attaches and
/// reports them, and the palette is planted in every pane child's environment at
/// spawn time. So the spares a fresh server pre spawned are holding the wrong
/// palette the instant that first report arrives, and retiring them (#473
/// follow up) costs the user's first split a whole shell startup: measured 18 ms
/// without the report against 335 ms median and 807 ms worst with it.
///
/// The palette of a given terminal is the same every time, so remembering the
/// last one and spawning spares with it makes that first report agree and the
/// retirement never happen. A wrong guess is self correcting: the report differs,
/// the spares are retired exactly as before, and the new value is remembered.
pub fn host_colors_file() -> String {
    format!("{}\\host_colors", psmux_dir())
}

/// Path to a session's `.spawnlock` file (the warm-pool spawn lock).
pub fn spawnlock_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.spawnlock", psmux_dir(), session.as_ref())
}

/// Path to a session's `.act` file: the last-activity stamp (Unix epoch
/// microseconds, ASCII) that bare CLI routing ranks candidates by.
///
/// This is psmux's registry-visible equivalent of tmux's in-memory
/// `session.activity_time` (tmux `session.c`). tmux can consult it directly
/// because the command runs inside the server; a psmux CLI process talks to one
/// server per session, so the ranking input has to live in the shared registry
/// where it can be read without connecting to anything (issue #603).
pub fn activity_file(session: impl AsRef<str>) -> String {
    format!("{}\\{}.act", psmux_dir(), session.as_ref())
}

/// Directory holding one namespace-identity file per `-L` namespace (issue #509).
///
/// A subdirectory rather than a `<ns>.instance` sibling: session files are named
/// `<ns>__<session>.<ext>`, and the default namespace uses a bare `<session>`,
/// so any flat naming risks colliding with a legitimately-named session.
pub fn instance_dir_in(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join("instances")
}

/// Path to a namespace's identity file. `ns` is the `-L` value, or `None` for
/// the default namespace.
///
/// The file name is a readable prefix plus a hash of the *full* namespace name.
/// `-L` values come from the user and may contain characters that are illegal in
/// a filename (or that would collide once sanitised — `a/b` and `a_b` both
/// become `a_b`), so the hash, not the prefix, is what guarantees isolation.
pub fn namespace_instance_file(dir: &std::path::Path, ns: Option<&str>) -> std::path::PathBuf {
    let name = match ns {
        None => "default-0000000000000000".to_string(),
        Some(n) => {
            use std::hash::{Hash, Hasher};
            // A fixed-seed hasher: the file name must be identical across
            // processes and runs, so `RandomState` (used for the random session
            // key) is deliberately NOT used here.
            let mut h = std::collections::hash_map::DefaultHasher::new();
            n.hash(&mut h);
            let digest = h.finish();
            let prefix: String = n
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(32)
                .collect();
            format!("{}-{:016x}", prefix, digest)
        }
    };
    instance_dir_in(dir).join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests READ the process-global env (USERPROFILE/HOME/
    // PSMUX_DATA_DIR) that sibling modules mutate, so they must hold the
    // same crate-wide lock the mutators serialize on or they observe a
    // half-mutated environment in full parallel runs.
    #[test]
    fn psmux_dir_is_home_relative_dot_psmux() {
        let _lock = crate::util::lock_test_env();
        // The test environment always has a home, so the data dir resolves to
        // {home}\.psmux and the two accessors agree.
        let dir = psmux_dir();
        assert!(dir.ends_with("\\.psmux"), "got {dir:?}");
        assert_eq!(psmux_dir_opt().as_deref(), Some(dir.as_str()));
    }

    #[test]
    fn per_session_helpers_append_name_and_suffix_to_data_dir() {
        let _lock = crate::util::lock_test_env();
        let dir = psmux_dir();
        assert_eq!(port_file("foo"), format!("{}\\foo.port", dir));
        assert_eq!(key_file("foo"), format!("{}\\foo.key", dir));
        assert_eq!(sid_file("foo"), format!("{}\\foo.sid", dir));
        assert_eq!(pid_file("foo"), format!("{}\\foo.pid", dir));
        assert_eq!(spawnlock_file("foo"), format!("{}\\foo.spawnlock", dir));
    }

    #[test]
    fn psmux_dir_file_appends_fixed_name() {
        let _lock = crate::util::lock_test_env();
        assert_eq!(
            psmux_dir_file("latency.log"),
            format!("{}\\latency.log", psmux_dir())
        );
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue474_home_resolution.rs"]
mod tests_issue474_home_resolution;

#[cfg(test)]
#[path = "../tests-rs/test_data_dir_override.rs"]
mod tests_data_dir_override;
