use std::io::{self, ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, SystemTime};
use std::env;

const STALE_PORT_PROBE_ATTEMPTS: usize = 3;
const STALE_PORT_CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
const STALE_PORT_RETRY_DELAY: Duration = Duration::from_millis(25);
/// How long to wait for the server's AUTH ack (`OK` / `ERROR`) when verifying
/// that the listener on a port file's port is actually *our* psmux server.
const STALE_PORT_AUTH_READ_TIMEOUT: Duration = Duration::from_millis(120);
/// Grace window subtracted from the system boot time before treating a
/// registry file as "written before this boot". Absorbs clock jitter and the
/// inherent imprecision of deriving boot wall-time from uptime, so a server
/// that wrote its port file moments after boot is never falsely reaped.
const BOOT_TIME_MARGIN: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PortProbeResult {
    Alive,
    Stale,
    Inconclusive,
}

/// Returns true if this port-file base name belongs to a warm (standby) server.
/// Warm sessions should be hidden from user-facing lists and never auto-attached.
pub fn is_warm_session(base: &str) -> bool {
    base == "__warm__" || base.ends_with("____warm__")
}

/// The `-L` socket namespace a registry base name belongs to.
///
/// A namespaced session is stored as `<ns>__<name>`, so the namespace is the
/// text before the first `__`. A bare name has none, and neither does the
/// default namespace's own warm helper `__warm__` (its `__` is not a
/// separator). Used to work out which server family a client belongs to from
/// the full name it was attached with.
pub fn session_namespace(full: &str) -> Option<&str> {
    if is_warm_session(full) {
        return None;
    }
    match full.split_once("__") {
        Some((ns, _)) if !ns.is_empty() => Some(ns),
        _ => None,
    }
}

/// Whether the registry base `base` belongs to namespace `ns`.
///
/// This is the file-name convention in one place: a `-L` namespace writes
/// `<ns>__<session>`, the default namespace writes a bare `<session>`, and the
/// default namespace's own warm helper `__warm__` is the single bare name that
/// contains `__`. Membership, unlike [`session_visible_from`], counts the warm
/// helpers: they are real servers, so a namespace-wide sweep must reach them.
pub fn registry_base_in_namespace(base: &str, ns: Option<&str>) -> bool {
    match ns {
        Some(n) => base.starts_with(&format!("{}__", n)),
        None => !base.contains("__") || base == "__warm__",
    }
}

/// What a `kill-server` is allowed to touch.
///
/// tmux's `kill-server` kills the server on the selected socket and nothing
/// else; sessions on other sockets are untouched. psmux keeps every `-L`
/// namespace in one registry directory, so the socket boundary has to be
/// applied by name — that is [`KillScope::Namespace`]. [`KillScope::All`] is
/// the psmux-only `kill-server -a`, the deliberate machine-wide sweep that used
/// to be what a bare `kill-server` did (#649).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillScope<'a> {
    /// Every registry entry in the data dir, every namespace (`-a`/`--all`).
    All,
    /// One socket: `Some(ns)` for `-L ns`, `None` for the default namespace.
    Namespace(Option<&'a str>),
}

impl KillScope<'_> {
    /// Whether a registry base name (a `.port`/`.pid` file stem) is in scope.
    pub fn covers(&self, base: &str) -> bool {
        match self {
            KillScope::All => true,
            KillScope::Namespace(ns) => registry_base_in_namespace(base, *ns),
        }
    }
}

/// Whether the registry base `base` is visible from namespace `ns`.
///
/// tmux parity: a `-L` socket is a separate server, and a client on one
/// socket can never see sessions of another. psmux keeps every namespace in
/// one registry directory, so listings that enumerate `.port` files must
/// apply this rule. `ls`, `$N` resolution and the reaper already did; the
/// interactive session and tree choosers did not, and a session from another
/// namespace (`amx-6c9b6ad63d__main`) sorted itself between the default
/// namespace's own sessions and shifted every `j`/`k` step in the picker.
pub fn session_visible_from(base: &str, ns: Option<&str>) -> bool {
    match ns {
        Some(n) => base.starts_with(&format!("{}__", n)),
        None => !base.contains("__"),
    }
}

/// Find the next available numeric session name (tmux-compatible).
/// tmux uses a monotonically incrementing counter, but since psmux has
/// no persistent server state, we scan existing port files and pick
/// the lowest non-negative integer not already in use.
/// When `ns_prefix` is Some("foo"), names are checked as "foo__0", "foo__1", etc.
pub fn next_session_name(ns_prefix: Option<&str>) -> String {
    let Some(psmux_dir) = crate::paths::psmux_dir_opt() else {
        return "0".to_string();
    };
    let mut used: std::collections::HashSet<u32> = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
        for entry in entries.flatten() {
            if let Some(fname) = entry.file_name().to_str() {
                if let Some((base, ext)) = fname.rsplit_once('.') {
                    if ext != "port" { continue; }
                    if is_warm_session(base) { continue; }
                    // Extract the session name part (after namespace prefix if any)
                    let session_part = if let Some(pfx) = ns_prefix {
                        let full_pfx = format!("{}__", pfx);
                        if base.starts_with(&full_pfx) {
                            &base[full_pfx.len()..]
                        } else {
                            continue; // different namespace
                        }
                    } else {
                        if base.contains("__") { continue; } // namespaced session
                        base
                    };
                    if let Ok(n) = session_part.parse::<u32>() {
                        used.insert(n);
                    }
                }
            }
        }
    }
    let mut id = 0u32;
    while used.contains(&id) {
        id += 1;
    }
    id.to_string()
}

/// Serializes session-id allocation within this process. Without it, two
/// threads (e.g. concurrent `new-session` handling, or the test harness running
/// tests in parallel) can both read the same value from the counter file before
/// either writes back, and hand out duplicate ids.
static SESSION_ID_ALLOC: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Best-effort cross-process advisory lock backed by an atomically-created lock
/// file. Separate psmux server processes share the same `next_session_id`
/// counter, so the in-process mutex alone is not enough; this closes the
/// read-modify-write gap across processes too. Released on drop. A lock left by
/// a crashed process is taken over once it is clearly stale; the guarded
/// critical section is sub-millisecond, so the staleness bound never steals a
/// live lock.
struct CounterLock {
    path: String,
}

impl CounterLock {
    const STALE_AFTER: Duration = Duration::from_secs(5);

    fn acquire(path: String) -> Self {
        let mut made_dir = false;
        for _ in 0..2000 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return CounterLock { path };
                }
                // Another process holds it. This is the one failure waiting can
                // resolve, so it is the only one that sleeps.
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    // Take over a stale lock left behind by a crashed holder.
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().map(|d| d >= Self::STALE_AFTER).unwrap_or(false))
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                // The data directory is not there yet, which is what a first run
                // on a machine looks like. Create it once and try again: no
                // amount of waiting makes a missing directory appear, and the
                // counter file this lock guards cannot be written without it
                // either.
                Err(e) if e.kind() == ErrorKind::NotFound && !made_dir => {
                    made_dir = true;
                    match std::path::Path::new(&path).parent() {
                        Some(parent) if std::fs::create_dir_all(parent).is_ok() => {}
                        _ => return CounterLock { path },
                    }
                }
                // Anything else, a denied directory for instance, is a state
                // this loop cannot change. Proceed rather than spend the whole
                // budget discovering that.
                Err(_) => return CounterLock { path },
            }
        }
        // Only reachable under real contention now, and never observed there:
        // the critical section is microseconds. Proceed rather than hang
        // session creation indefinitely.
        CounterLock { path }
    }
}

impl Drop for CounterLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Allocate a globally unique session ID by reading and incrementing
/// the persistent counter file `.psmux/next_session_id`.
///
/// The read-modify-write is serialized within the process by `SESSION_ID_ALLOC`
/// and across processes by an advisory lock file, so concurrent callers can
/// never observe the same `current` and return duplicate ids.
pub fn allocate_session_id() -> usize {
    let _guard = SESSION_ID_ALLOC.lock().unwrap_or_else(|e| e.into_inner());
    let counter_path = crate::paths::psmux_dir_file("next_session_id");
    let _xlock = CounterLock::acquire(format!("{}.lock", counter_path));
    let current = std::fs::read_to_string(&counter_path)
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let _ = std::fs::write(&counter_path, (current + 1).to_string());
    current
}

/// Write a `.sid` file recording the session ID for this session.
pub fn write_session_id_file(port_file_base: &str, session_id: usize) {
    let sid_path = crate::paths::sid_file(port_file_base);
    let _ = std::fs::write(&sid_path, session_id.to_string());
}

/// Remove the `.sid` file for a session. Also removes the twin `.pid` file
/// (issue #448): both are per-session identity sentinels written together by
/// `ensure_session_registry_files`, and every session-teardown site already
/// calls this, so piggybacking `.pid` cleanup here keeps the registry consistent
/// without touching each teardown call site.
pub fn remove_session_id_file(port_file_base: &str) {
    let sid_path = crate::paths::sid_file(port_file_base);
    let _ = std::fs::remove_file(&sid_path);
    remove_session_pid_file(port_file_base);
    // The `.act` activity stamp (issue #603) is per-session registry state
    // exactly like `.sid`/`.pid`, so it leaves with them. Left behind, a dead
    // session's stamp survived kill-server (seen as a stray
    // `<ns>__<name>.act` after `-L <ns> kill-server` in test_mouse_hover) and
    // would keep ranking a session that no longer exists.
    remove_session_activity_file(port_file_base);
}

/// Remove the `.act` activity stamp for a session (issue #603).
pub fn remove_session_activity_file(port_file_base: &str) {
    if let Some(dir) = crate::paths::psmux_dir_opt() {
        remove_session_activity_file_in(std::path::Path::new(&dir), port_file_base);
    }
}

/// Registry-directory-parameterized [`remove_session_activity_file`].
pub fn remove_session_activity_file_in(dir: &std::path::Path, port_file_base: &str) {
    let _ = std::fs::remove_file(dir.join(format!("{}.act", port_file_base)));
}

/// Move a session's `.act` activity stamp from `old_base` to `new_base` on
/// rename (issue #603). tmux keeps `activity_time` on the session struct, so a
/// rename never touches it; psmux's copy is keyed by name and has to follow.
/// Without this a renamed session fell back to its `.port` mtime and lost its
/// place in bare CLI routing. Best effort: a session that was never stamped
/// has nothing to carry.
pub fn carry_session_activity_file(old_base: &str, new_base: &str) {
    if let Some(dir) = crate::paths::psmux_dir_opt() {
        carry_session_activity_file_in(std::path::Path::new(&dir), old_base, new_base);
    }
}

/// Registry-directory-parameterized [`carry_session_activity_file`].
pub fn carry_session_activity_file_in(dir: &std::path::Path, old_base: &str, new_base: &str) {
    if old_base == new_base {
        return;
    }
    let old = dir.join(format!("{}.act", old_base));
    let new = dir.join(format!("{}.act", new_base));
    if old.exists() {
        let _ = std::fs::rename(&old, &new);
    }
}

/// Write a `.pid` file recording the OS process ID of the server that owns this
/// session (issue #448). The stale-port cleanup only knew a server by its TCP
/// port; a wedged server that stopped listening but hasn't exited could not be
/// targeted by identity at all. The PID gives every registry entry a stable
/// process anchor.
pub fn write_session_pid_file(port_file_base: &str, pid: u32) {
    let pid_path = crate::paths::pid_file(port_file_base);
    // `pid:creation_filetime` — same body as ensure_session_registry_files, so a
    // freshly renamed session is force-kill-identifiable before the next re-ensure.
    let creation = crate::platform::process_kill::process_creation_time(pid).unwrap_or(0);
    let _ = std::fs::write(&pid_path, format_pid_file_contents(pid, creation));
}

/// Remove the `.pid` file for a session.
pub fn remove_session_pid_file(port_file_base: &str) {
    let pid_path = crate::paths::pid_file(port_file_base);
    let _ = std::fs::remove_file(&pid_path);
}

/// Record that server process `pid` belongs to THIS data dir (issue #510).
///
/// Keyed by PID rather than by session, and deliberately independent of the
/// session registry lifecycle: the whole point is to still identify a server as
/// ours after its `.port`/`.pid` entries are gone, which is exactly the state a
/// spawn-race duplicate or a registry wipe leaves behind. Body is the same
/// `pid:creation_filetime` as the `.pid` sentinel so a reused PID cannot
/// inherit a dead process's claim.
pub fn write_server_marker(pid: u32) {
    let dir = crate::paths::server_marker_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let creation = crate::platform::process_kill::process_creation_time(pid).unwrap_or(0);
    let _ = std::fs::write(
        crate::paths::server_marker_file(pid),
        format_pid_file_contents(pid, creation),
    );
}

/// Server processes `psmux_dir` claims, as `pid -> recorded creation filetime`.
///
/// The PID is read from the file body rather than its name so a truncated or
/// hand-copied marker cannot assert a claim over an arbitrary PID.
pub fn read_owned_server_pids(psmux_dir: &Path) -> std::collections::HashMap<u32, Option<u64>> {
    let mut owned = std::collections::HashMap::new();
    let Ok(entries) = std::fs::read_dir(psmux_dir.join("servers")) else {
        return owned;
    };
    for entry in entries.flatten() {
        let Ok(body) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some((pid, creation)) = parse_pid_file_contents(&body) {
            owned.insert(pid, creation);
        }
    }
    owned
}

/// Resolve a tmux session ID (`$N`) to the port file base name of the
/// session that owns that ID. Returns `None` if no session has that ID.
pub fn resolve_session_by_id(id: usize) -> Option<String> {
    let psmux_dir = crate::paths::psmux_dir_opt()?;
    if let Ok(entries) = std::fs::read_dir(&psmux_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "sid").unwrap_or(false) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(file_id) = content.trim().parse::<usize>() {
                        if file_id == id {
                            if let Some(base) = path.file_stem().and_then(|s| s.to_str()) {
                                // Verify the session is actually alive
                                let port_path = crate::paths::port_file(base);
                                if std::path::Path::new(&port_path).exists() {
                                    return Some(base.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Clean up any stale port files (where server is not actually running)
pub fn cleanup_stale_port_files() {
    let Some(psmux_dir) = crate::paths::psmux_dir_opt() else {
        return;
    };
    cleanup_stale_port_files_in(Path::new(&psmux_dir));
}

fn cleanup_stale_port_files_in(psmux_dir: &Path) {
    cleanup_stale_port_files_in_with(psmux_dir, probe_session_for_cleanup);
}

/// Registry file extensions that only ever exist as satellites of a `.port`
/// entry. Anything else in the data dir (`next_session_id`, its `.lock`, debug
/// logs, the `instances/` and `servers/` subdirectories) is never touched.
const ORPHAN_REGISTRY_EXTS: &[&str] = &["sid", "key", "pid", "spawnlock", "act"];

/// How long a `.port`-less registry file must sit untouched before it is
/// considered abandoned (issue #530).
///
/// `ensure_session_registry_files` writes `.sid`/`.key`/`.pid` BEFORE the
/// `.port` beacon, so a perfectly healthy server that is still coming up
/// briefly looks exactly like an orphan, and it is only during that window that
/// this bound is load-bearing (the 5s registry self-heal re-writes a file only
/// when its contents changed, so a live server's mtimes do NOT keep advancing).
/// Once the server is up its `.port` is present and the whole set is skipped
/// outright. One minute is therefore far beyond any legitimate window while
/// still clearing the backlog on the next CLI invocation.
const ORPHAN_REGISTRY_GRACE: Duration = Duration::from_secs(60);

/// Most files a single sweep may remove.
///
/// psmux CLI commands are invoked constantly by scripts and prompts, and the
/// backlog this fix targets can run to thousands of files. Deleting all of it
/// inside one arbitrary invocation — `psmux -V`, say — makes a trivial command
/// do a surprising amount of destructive work and stalls it on I/O. The budget
/// keeps any one invocation cheap and bounded; the backlog drains across
/// successive sweeps instead, which is just as effective and far less abrupt.
const ORPHAN_REGISTRY_SWEEP_BUDGET: usize = 256;

/// Minimum interval between sweeps, tracked by [`ORPHAN_REGISTRY_SWEEP_STAMP`].
///
/// Without this, every psmux invocation pays a full directory walk. Orphans are
/// produced only by session teardown, so there is nothing to gain from looking
/// more often than this.
const ORPHAN_REGISTRY_SWEEP_INTERVAL: Duration = Duration::from_secs(300);

/// Marker file recording when the last sweep ran. Leading dot, so it has no
/// extension and can never be mistaken for a registry satellite.
const ORPHAN_REGISTRY_SWEEP_STAMP: &str = ".registry_sweep";

/// Delete per-session registry files whose `.port` entry is already gone
/// (issue #530).
///
/// Every other sweep in this module enumerates `.port` files and deletes the
/// siblings it finds. That makes the `.port` file the sole entry point to the
/// registry, so the moment one is removed while a sibling survives, the
/// survivor becomes permanently unreachable: no code path ever looks at it
/// again, and nothing can ever delete it. Teardown paths that remove
/// `.port`/`.key`/`.pid` but not `.sid` therefore leak one file per session
/// forever.
///
/// The cost is not only disk. `resolve_session_by_id` scans every `.sid` file
/// in the directory to map `$N` to a session, so each leaked file is paid for
/// on every lookup.
pub fn prune_orphaned_registry_files() {
    let Some(psmux_dir) = crate::paths::psmux_dir_opt() else {
        return;
    };
    let psmux_dir = Path::new(&psmux_dir);
    if !registry_sweep_due(psmux_dir, ORPHAN_REGISTRY_SWEEP_INTERVAL) {
        return;
    }
    prune_orphaned_registry_files_in(psmux_dir);
}

/// Whether a sweep is due, re-stamping the marker when it is.
///
/// The stamp is written BEFORE the sweep runs, not after: if the process is
/// killed partway through, the next invocation waits a full interval rather
/// than immediately retrying the same work.
fn registry_sweep_due(psmux_dir: &Path, interval: Duration) -> bool {
    let stamp = psmux_dir.join(ORPHAN_REGISTRY_SWEEP_STAMP);
    let swept_recently = stamp
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|age| age < interval)
        .unwrap_or(false); // missing or unreadable stamp -> sweep
    if swept_recently {
        return false;
    }
    let _ = std::fs::write(&stamp, b"");
    true
}

fn prune_orphaned_registry_files_in(psmux_dir: &Path) -> usize {
    let satellites = prune_orphaned_registry_files_in_with(
        psmux_dir,
        ORPHAN_REGISTRY_GRACE,
        ORPHAN_REGISTRY_SWEEP_BUDGET,
        pid_owns_live_server,
    );
    // Namespace tokens are the same leak in a subdirectory: bounded by distinct
    // namespace NAMES, which is unbounded for disposable `-L` namespaces (#530).
    // Run it after the satellite sweep so it sees the `.port` set that pass left,
    // and give it whatever is left of this sweep's budget so the two passes
    // together stay bounded rather than each costing a full budget.
    satellites
        + prune_orphaned_instance_tokens_in_with(
            psmux_dir,
            ORPHAN_REGISTRY_GRACE,
            ORPHAN_REGISTRY_SWEEP_BUDGET.saturating_sub(satellites),
        )
}

/// Core of [`prune_orphaned_registry_files`], with the grace period, the
/// per-sweep budget and the liveness oracle injected so tests can drive it
/// deterministically.
///
/// Returns the number of files removed.
fn prune_orphaned_registry_files_in_with<F>(
    psmux_dir: &Path,
    grace: Duration,
    budget: usize,
    mut is_live: F,
) -> usize
where
    F: FnMut(u32) -> bool,
{
    if budget == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(psmux_dir) else {
        return 0;
    };
    let mut pruned = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !ORPHAN_REGISTRY_EXTS.contains(&ext) {
            continue;
        }
        // A surviving `.port` means this entry still belongs to the port-driven
        // sweep, which owns the liveness decision for the whole set.
        if path.with_extension("port").exists() {
            continue;
        }
        // Too young to judge: could be a server mid-startup that hasn't
        // published its port yet. Leave it for a later invocation.
        let old_enough = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .map(|age| age >= grace)
            .unwrap_or(false); // unreadable mtime -> keep
        if !old_enough {
            continue;
        }
        // If the set still names a live psmux process, the server exists but is
        // not publishing a port (still binding, or wedged). Reaping that is
        // #448's job, not ours — deleting its identity files would only make it
        // harder to find.
        if let Some(pid) = orphan_anchor_pid(&path, ext) {
            if is_live(pid) {
                continue;
            }
        }
        if std::fs::remove_file(&path).is_ok() {
            pruned += 1;
            if crate::debug_log::session_log_enabled() {
                crate::debug_log::session_log(
                    "cleanup",
                    &format!(
                        "pruned orphaned '{}': no .port sibling and no live owner",
                        path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
                    ),
                );
            }
            // Budget spent: leave the rest for the next sweep so no single
            // invocation does an unbounded amount of destructive work.
            if pruned >= budget {
                break;
            }
        }
    }
    pruned
}

/// PID that owns an orphaned registry file, when one can be determined.
///
/// A `.spawnlock` records its holder's PID directly; every other satellite is
/// anchored by the sibling `.pid` sentinel. `.sid`/`.key` orphans left by a
/// teardown that already removed the `.pid` have no anchor at all, which is
/// precisely the abandoned case.
fn orphan_anchor_pid(path: &Path, ext: &str) -> Option<u32> {
    let body = if ext == "spawnlock" {
        std::fs::read_to_string(path).ok()?
    } else {
        std::fs::read_to_string(path.with_extension("pid")).ok()?
    };
    parse_pid_file_contents(&body)
        .map(|(pid, _creation)| pid)
        .or_else(|| body.trim().parse::<u32>().ok())
}

/// True when `pid` is a live process running a psmux server image.
///
/// The process-table query is Windows-only. Elsewhere this answers "live", so
/// an orphan that still carries a PID anchor is kept rather than deleted on a
/// guess. Orphans with no anchor — the overwhelming majority, and the ones
/// #530 is about — are unaffected by the platform and prune everywhere.
fn pid_owns_live_server(pid: u32) -> bool {
    if !cfg!(windows) {
        return true;
    }
    // Snapshot-backed lookup (#650): an unopenable PID is not a dead one. A
    // server started from an SSH logon carries the elevated token sshd mints,
    // so a desktop CLI running under the UAC-filtered token is refused the
    // handle and would otherwise prune a live server's satellites.
    match crate::platform::process_info::get_process_name_or_snapshot(pid) {
        None => false,
        Some(name) => PSMUX_SERVER_IMAGE_NAMES.contains(&name.to_ascii_lowercase().as_str()),
    }
}

/// Instance-token file names (`instances/<prefix>-<hash>`) that a namespace with
/// a live server could be using.
///
/// A token file name is a hash of the namespace, so it cannot be read backwards
/// into one. The mapping is therefore built forwards from the live `.port`
/// files: a registry base is `<ns>__<session>` for a `-L` namespace and a bare
/// `<session>` for the default one, and BOTH halves may themselves contain
/// `__`, so every prefix ending at a `__` is claimed. Over-claiming is the safe
/// direction: it can only keep a token that nothing is using, never delete one
/// that a live namespace depends on.
fn live_instance_token_names(psmux_dir: &Path) -> std::collections::HashSet<std::ffi::OsString> {
    let mut live = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(psmux_dir) else {
        return live;
    };
    fn claim(
        dir: &Path,
        ns: Option<&str>,
        set: &mut std::collections::HashSet<std::ffi::OsString>,
    ) {
        if let Some(name) = crate::paths::namespace_instance_file(dir, ns).file_name() {
            set.insert(name.to_os_string());
        }
    }
    let mut any_port = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "port").unwrap_or(true) {
            continue;
        }
        any_port = true;
        let Some(base) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let bytes = base.as_bytes();
        for i in 1..bytes.len().saturating_sub(1) {
            // `_` is ASCII, so a match is always a char boundary.
            if bytes[i] == b'_' && bytes[i + 1] == b'_' {
                claim(psmux_dir, Some(&base[..i]), &mut live);
            }
        }
    }
    // Any live server at all may be a default-namespace one: a bare `<session>`
    // base is indistinguishable from a namespaced base whose split we cannot
    // pin down, so the default token is kept whenever anything is running.
    if any_port {
        claim(psmux_dir, None, &mut live);
    }
    live
}

/// Delete namespace identity tokens (issue #509's `instances/`) for namespaces
/// that have no live server left (issue #530).
///
/// #509 argued that tokens need no teardown because the next server in a
/// namespace re-mints one anyway. That holds for a fixed set of namespace
/// names; it does not for callers that mint a throwaway `-L` namespace per run,
/// which is exactly what namespaces are good for. Since a token for a dead
/// namespace is discarded and re-minted the moment that namespace is used again
/// (`ensure_namespace_instance_in` re-decides when it finds no live peer),
/// deleting it early is observationally identical and keeps the directory
/// bounded by LIVE namespaces rather than by every name ever used.
fn prune_orphaned_instance_tokens_in_with(
    psmux_dir: &Path,
    grace: Duration,
    budget: usize,
) -> usize {
    if budget == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(crate::paths::instance_dir_in(psmux_dir)) else {
        // No instances dir: skip the parent walk `live_instance_token_names`
        // would otherwise pay for nothing.
        return 0;
    };
    let live = live_instance_token_names(psmux_dir);
    let mut pruned = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name() else { continue };
        if live.contains(name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        // Same startup guard as the satellite sweep: a server establishes its
        // namespace identity before it publishes a `.port`, so a young token
        // may belong to a namespace that is still coming up.
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|age| age >= grace)
            .unwrap_or(false); // unreadable mtime -> keep
        if !old_enough {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            pruned += 1;
            if crate::debug_log::session_log_enabled() {
                crate::debug_log::session_log(
                    "cleanup",
                    &format!(
                        "pruned namespace token '{}': no live server in that namespace",
                        name.to_string_lossy()
                    ),
                );
            }
            // Same budget rule as the satellite sweep: leave the rest for the
            // next one rather than doing unbounded destructive work here.
            if pruned >= budget {
                break;
            }
        }
    }
    pruned
}

/// Image-name stems (lower-case, no extension) that count as a psmux server for
/// the orphan reaper. Only processes whose executable matches one of these are
/// ever candidates for termination — an unrelated app that happens to hold a
/// loopback listener is never touched.
const PSMUX_SERVER_IMAGE_NAMES: &[&str] = &["psmux", "tmux", "pmux"];

/// Grace period before a live server process is eligible for orphan reaping.
/// A server that just bound its socket but hasn't finished writing its `.port`
/// file yet (or a concurrent `new-session` still coming up) would otherwise look
/// untracked; requiring the process to be older than this avoids that race. The
/// spawn-race itself is fixed in #444 — this reaper is only the accumulation
/// backstop, so it can afford to skip very young processes and catch them next
/// startup instead.
const ORPHAN_REAP_MIN_AGE: Duration = Duration::from_secs(10);

/// A live psmux server process discovered by the reaper: its PID, every loopback
/// port it listens on, and its process creation time (FILETIME 100ns ticks).
#[derive(Clone, Debug, PartialEq)]
struct ServerCandidate {
    pid: u32,
    ports: Vec<u16>,
    creation_ft: u64,
}

/// Pure orphan-selection policy (unit-testable, no OS calls).
///
/// A candidate server is an orphan to reap iff ALL hold:
///  - it is not this very process (`self_pid`),
///  - its PID is not recorded in any live registry entry (`tracked_pids`),
///  - NONE of its listening ports is claimed by a registry `.port` file
///    (`tracked_ports`) — i.e. nothing references this server, so it is a
///    duplicate / lost headless server rather than a legitimate session,
///  - it was created at or before `age_cutoff_ft` (older than the grace window),
///    so a just-spawned server still writing its registry files is never reaped.
///
/// The port check is the primary anchor: a legitimate server ALWAYS has a
/// `.port` file pointing at it, so it can never be selected even if its `.pid`
/// file is missing (backward compatibility with servers started before #448).
fn select_orphan_pids(
    candidates: &[ServerCandidate],
    tracked_ports: &std::collections::HashSet<u16>,
    tracked_pids: &std::collections::HashSet<u32>,
    owned_pids: &std::collections::HashMap<u32, Option<u64>>,
    self_pid: u32,
    age_cutoff_ft: u64,
) -> Vec<u32> {
    let mut out = Vec::new();
    for c in candidates {
        if c.pid == self_pid { continue; }
        if tracked_pids.contains(&c.pid) { continue; }
        if c.ports.iter().any(|p| tracked_ports.contains(p)) { continue; }
        // Issue #510: reap only what this data dir can positively claim.
        //
        // The candidate list is machine-wide, so "no registry entry references
        // it" covers two very different processes: an orphan we started, and a
        // perfectly healthy server belonging to another USERPROFILE/HOME. The
        // old rule could not tell them apart and killed both. An absent
        // ownership marker now means "not ours, not our business" rather than
        // "orphan" - unknown must never justify termination.
        let Some(&recorded_creation) = owned_pids.get(&c.pid) else { continue; };
        // Same identity gate force_kill_targets applies: without a recorded
        // creation time there is nothing to distinguish this process from an
        // unrelated one that inherited the PID, so it is not a candidate.
        match recorded_creation {
            Some(ft) if ft == c.creation_ft => {}
            _ => continue,
        }
        // Only reap processes old enough to have finished registering.
        if age_cutoff_ft != 0 && c.creation_ft > age_cutoff_ft { continue; }
        out.push(c.pid);
    }
    out
}

/// Read the set of ports referenced by `.port` files and the set of PIDs
/// recorded in `.pid` files whose sibling `.port` still exists. A `.pid` without
/// a live `.port` is ignored so a dead-then-reused PID can't be treated as
/// tracked.
fn read_tracked_registry(psmux_dir: &Path)
    -> (std::collections::HashSet<u16>, std::collections::HashSet<u32>)
{
    let mut tracked_ports = std::collections::HashSet::new();
    let mut tracked_pids = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(psmux_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            match path.extension().and_then(|e| e.to_str()) {
                Some("port") => {
                    if let Ok(s) = std::fs::read_to_string(&path) {
                        if let Ok(p) = s.trim().parse::<u16>() { tracked_ports.insert(p); }
                    }
                }
                Some("pid") => {
                    // Only trust a PID whose session still has a live .port file.
                    let port_sibling = path.with_extension("port");
                    if port_sibling.exists() {
                        if let Ok(s) = std::fs::read_to_string(&path) {
                            // Tolerate both `pid` and `pid:creation_filetime` bodies.
                            if let Some((pid, _)) = parse_pid_file_contents(&s) { tracked_pids.insert(pid); }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    (tracked_ports, tracked_pids)
}

// --- kill-server force-kill fallback: data-dir-scoped, identity-checked -------
// tmux's kill-server is socket-scoped. psmux's bare kill-server sends a graceful
// kill-server to every session in scope; a wedged server that ignores it must be
// force-killed. The old fallback scanned every process on the machine by image
// name (psmux/pmux/tmux) and TerminateProcess'd them — machine-wide, reaching
// unrelated servers and other namespaces. These helpers replace that with a
// selection scoped by construction to this data dir's `.pid` files, gated by an
// exact process-creation-time match so a recycled pid is never killed.

/// A force-kill target read from a data dir's registry: the recorded server pid
/// and the creation FILETIME it had when it wrote the pid file. The pair is what
/// defeats pid reuse — a recycled pid will not carry the same creation time.
#[derive(Debug, PartialEq)]
pub struct PidTarget {
    pub pid: u32,
    pub creation_time: u64,
}

/// Parse a `.pid` file body. Two forms are accepted: a bare `pid` (the #448
/// liveness anchor as first written) and `pid:creation_filetime` (extended so
/// kill-server can verify identity). Returns `(pid, Option<creation_time>)`, or
/// `None` when the pid itself is unparseable. One parser so every reader
/// (`force_kill_targets`, the orphan reaper, the pid anchor) stays in step.
pub fn parse_pid_file_contents(s: &str) -> Option<(u32, Option<u64>)> {
    let s = s.trim();
    match s.split_once(':') {
        Some((pid_str, time_str)) => Some((pid_str.trim().parse().ok()?, time_str.trim().parse().ok())),
        None => Some((s.parse().ok()?, None)),
    }
}

/// The `.pid` body the server writes: `pid:creation_filetime`. Kept as one
/// function so the producer and `parse_pid_file_contents` cannot drift apart.
pub fn format_pid_file_contents(pid: u32, creation_time: u64) -> String {
    format!("{pid}:{creation_time}")
}

/// Force-kill candidates for `kill-server`'s fallback, scoped by construction to
/// a single data dir: the `pid:creation_filetime` `.pid` files in `dir`, further
/// narrowed by `scope` so the fallback reaches exactly the servers the graceful
/// pass aimed at — one namespace for a bare or `-L` kill-server, everything for
/// `-a`. Bare-pid and malformed files are skipped (no recorded creation time
/// means no identity gate, so they are not force-kill candidates). This selects
/// targets; it does not kill.
pub fn force_kill_targets(dir: &std::path::Path, scope: KillScope) -> Vec<PidTarget> {
    let mut targets = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return targets; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "pid").unwrap_or(false) {
            {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if !scope.covers(stem) { continue; }
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                // A recorded creation time is required: it is the identity gate.
                // Bare-pid files carry none, so they are not force-kill candidates.
                if let Some((pid, Some(creation_time))) = parse_pid_file_contents(&contents) {
                    targets.push(PidTarget { pid, creation_time });
                }
            }
        }
    }
    targets
}

/// The exact-match identity gate for the force-kill fallback: terminate only when
/// the live process at the pid still carries the creation time recorded in the
/// pid file. `queried` is the process's current creation FILETIME (None if it
/// could not be read); `expected` is the value from the pid file. A recycled pid
/// carries a different creation time and is rejected; an unreadable process is
/// rejected too, so the fallback fails safe and never kills on uncertainty.
pub fn confirms_identity(queried: Option<u64>, expected: u64) -> bool {
    queried == Some(expected)
}

/// A server `kill-server` is about to end: its `.port` file, the port it names
/// and the auth key that goes with it.
pub struct KillTarget {
    pub base: String,
    pub port_path: std::path::PathBuf,
    pub port: u16,
}

/// Registry entries a `kill-server` in `scope` should end, plus the stale
/// `.port` files (unreadable, so nothing to talk to) that should simply be
/// swept. `exclude_base` drops one entry — the caller's own server, when the
/// command came from inside it and must kill itself last.
///
/// Selection only: nothing is contacted or deleted here, which is what makes
/// the scope rule testable without starting a server.
pub fn kill_server_targets(
    dir: &std::path::Path,
    scope: KillScope,
    exclude_base: Option<&str>,
) -> (Vec<KillTarget>, Vec<std::path::PathBuf>) {
    let mut targets: Vec<KillTarget> = Vec::new();
    let mut stale: Vec<std::path::PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return (targets, stale) };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "port").unwrap_or(true) {
            continue;
        }
        let Some(base) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        // tmux parity (#649): a bare kill-server ends the server on the socket
        // it was invoked on and nothing else. Without this, every `-L`
        // namespace sharing the data dir died with it.
        if !scope.covers(base) {
            continue;
        }
        if exclude_base == Some(base) {
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(port_str) => match port_str.trim().parse::<u16>() {
                Ok(port) => targets.push(KillTarget {
                    base: base.to_string(),
                    port_path: path.clone(),
                    port,
                }),
                // A `.port` with no parseable port names no live server.
                Err(_) => stale.push(path.clone()),
            },
            Err(_) => stale.push(path.clone()),
        }
    }
    (targets, stale)
}

/// How many of the targets are real user sessions, i.e. not warm standbys.
/// tmux answers `no server running` when there is nothing to kill, and a warm
/// standby is an implementation detail, not a session (the same rule `ls`
/// applies), so it must not make an empty namespace look occupied.
pub fn user_session_count(targets: &[KillTarget]) -> usize {
    targets.iter().filter(|t| !is_warm_session(&t.base)).count()
}

/// Send `kill-server` to every server in `scope`, sweep their registry sets and
/// force-kill whatever ignored the graceful request. Returns the number of user
/// sessions (warm standbys excluded) that were in scope.
///
/// One implementation for every dispatch route — the CLI, the command prompt of
/// an attached client and a config file — so the blast radius cannot drift
/// between them again.
pub fn kill_servers_in_scope(
    dir: &std::path::Path,
    scope: KillScope,
    exclude_base: Option<&str>,
) -> usize {
    // Snapshot the force-kill candidates BEFORE the graceful pass removes the
    // `.pid` files, scoped the same way, so the fallback can never reach a
    // server the graceful pass was not allowed to touch.
    let fk_targets: Vec<PidTarget> = force_kill_targets(dir, scope);
    let excluded_pid = exclude_base.and_then(|b| {
        std::fs::read_to_string(crate::paths::pid_file(b))
            .ok()
            .and_then(|s| parse_pid_file_contents(&s))
            .map(|(pid, _)| pid)
    });
    let (targets, stale) = kill_server_targets(dir, scope, exclude_base);
    let killed = user_session_count(&targets);
    let handles: Vec<std::thread::JoinHandle<()>> = targets
        .into_iter()
        .map(|t| {
            std::thread::spawn(move || {
                let key = read_session_key(&t.base).unwrap_or_default();
                let addr = format!("127.0.0.1:{}", t.port);
                if let Ok(sock) = addr.parse() {
                    if let Ok(mut stream) =
                        std::net::TcpStream::connect_timeout(&sock, Duration::from_millis(500))
                    {
                        let _ = stream.set_nodelay(true);
                        let _ = write!(stream, "AUTH {}\n", key);
                        let _ = stream.flush();
                        let _ = stream.write_all(b"kill-server\n");
                        let _ = stream.flush();
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        // Wait for the server to exit (EOF = done).
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
                        let mut buf = [0u8; 64];
                        loop {
                            match std::io::Read::read(&mut stream, &mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => continue,
                            }
                        }
                    }
                }
                // Remove the whole registry set regardless. Deleting only
                // port/key/pid used to strand the `.sid`, which no sweep can
                // reach once its `.port` is gone (#530).
                remove_session_registry_files(&t.port_path);
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    // Clean up stale registry sets (whole set, including `.sid` — #530)
    for path in &stale {
        remove_session_registry_files(path);
    }
    // Force-kill any wedged server that ignored the graceful kill. The
    // identity gate (exact process-creation-time match) skips any pid that has
    // already exited or been recycled — no machine-wide, name-based scan.
    std::thread::sleep(Duration::from_millis(50));
    for t in fk_targets {
        if Some(t.pid) == excluded_pid {
            continue;
        }
        if confirms_identity(
            crate::platform::process_kill::process_creation_time(t.pid),
            t.creation_time,
        ) {
            crate::platform::process_kill::terminate_server_pid(t.pid, None);
        }
    }
    killed
}

/// `.pid` registry entries belonging to namespace `ns`, excluding `self_pid`
/// (issue #509).
///
/// Membership is decided by the file-name convention: a `-L` namespace writes
/// `<ns>__<session>.pid`, while the default namespace writes a bare
/// `<session>.pid`. The warm helper is included in both cases — it is a real
/// server, so while it lives the namespace has not gone away.
pub fn namespace_peer_pids(dir: &Path, ns: Option<&str>, self_pid: u32) -> Vec<PidTarget> {
    let mut peers = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return peers; };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "pid").unwrap_or(true) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        // A bare session name has no `__` separator; the default namespace's
        // own warm helper is the one exception. One rule, shared with
        // kill-server's scope (`registry_base_in_namespace`).
        if !registry_base_in_namespace(stem, ns) {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Some((pid, Some(creation_time))) = parse_pid_file_contents(&contents) {
                if pid != self_pid {
                    peers.push(PidTarget { pid, creation_time });
                }
            }
        }
    }
    peers
}

/// Whether the calling server is the first live server in its namespace, i.e.
/// no peer from the registry is still running under its recorded identity.
///
/// `creation_of` supplies each pid's current creation FILETIME (`None` when the
/// process is gone or unreadable) so the decision is testable without OS process
/// enumeration. A pid whose creation time no longer matches has been recycled
/// and is somebody else's process, not our peer.
pub fn is_first_server_in_namespace(
    peers: &[PidTarget],
    creation_of: impl Fn(u32) -> Option<u64>,
) -> bool {
    !peers
        .iter()
        .any(|p| confirms_identity(creation_of(p.pid), p.creation_time))
}

/// Mint a fresh namespace identity token.
fn mint_instance_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let s = RandomState::new();
    let mut h = s.build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    h.write_u64(std::process::id() as u64);
    format!("{:016x}", h.finish())
}

/// Establish this namespace's stable identity (issue #509), returning the token
/// now in force.
///
/// psmux runs one server process per session, so a per-process value such as
/// `#{pid}` changes every time a session is created and a supervisor reads that
/// as a server restart. Every server in a namespace instead reads one shared
/// token file, so it does not matter which server answers a query.
///
/// The token is minted by the namespace's first server and left alone by every
/// later one. It is re-minted only when no peer is still alive — that is a
/// genuine restart, and a supervisor must be able to see it. Reclamation is
/// therefore self-healing: a token left behind by a namespace that died is
/// replaced by the next server to start, so nothing needs to delete it on exit
/// (a server can exit via exit-empty, kill-session, or a crash).
///
/// `established` is the token this server already holds from a previous call,
/// or `None` on the first call of the process. The first-server decision — and
/// the re-mint it triggers — belongs to server STARTUP only: this function is
/// also re-run every few seconds by the registry self-heal loop, and a lone
/// server (single session, no warm helper) sees no live peers on every tick.
/// Re-deciding there would delete and re-mint its own token every interval,
/// churning the very identity this feature exists to keep stable. Once a token
/// is established, re-ensure only restores the file if it was lost — with the
/// SAME token, so even losing the file does not fake a restart.
pub fn ensure_namespace_instance_in(
    dir: &Path,
    ns: Option<&str>,
    self_pid: u32,
    creation_of: impl Fn(u32) -> Option<u64>,
    established: Option<&str>,
) -> Option<String> {
    let path = crate::paths::namespace_instance_file(dir, ns);

    match established {
        // Steady state: this server already holds the namespace's identity.
        // While it is alive the namespace cannot have restarted, so never
        // delete or re-mint — only put the established token back if the file
        // went missing.
        Some(token) => {
            if !write_token_if_missing(&path, token) {
                return None;
            }
        }
        // Startup: decide whether this is a fresh namespace or a join.
        None => {
            let peers = namespace_peer_pids(dir, ns, self_pid);
            if is_first_server_in_namespace(&peers, creation_of) {
                // The namespace this token described is gone; do not inherit its identity.
                let _ = std::fs::remove_file(&path);
            }
            if !write_token_if_missing(&path, &mint_instance_token()) {
                return None;
            }
        }
    }

    read_namespace_instance_in(dir, ns)
}

/// Create the token file with `token` unless it already exists (`create_new`,
/// so a concurrent first-start cannot clobber the winner's token). Returns
/// false only on an unexpected I/O failure.
fn write_token_if_missing(path: &Path, token: &str) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut f) => {
            use std::io::Write as _;
            let _ = write!(f, "{}", token);
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => true,
        Err(_) => false,
    }
}

/// Read a namespace's identity token, or `None` when the namespace has none.
///
/// Reading never mints: only a starting server may establish identity, so a
/// client query against an unknown namespace reports nothing rather than
/// inventing a value.
pub fn read_namespace_instance_in(dir: &Path, ns: Option<&str>) -> Option<String> {
    let path = crate::paths::namespace_instance_file(dir, ns);
    let contents = std::fs::read_to_string(path).ok()?;
    let token = contents.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// The namespace identity this server process established at startup. One
/// server process serves exactly one namespace, so a process-wide cell is the
/// correct scope; it is what makes the periodic registry re-ensure a restore
/// rather than a fresh first-server decision (see
/// [`ensure_namespace_instance_in`]).
static ESTABLISHED_INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Production wrapper for [`ensure_namespace_instance_in`] against the real data
/// directory and live process table.
pub fn ensure_namespace_instance(ns: Option<&str>, self_pid: u32) -> Option<String> {
    let dir = crate::paths::psmux_dir_opt()?;
    let token = ensure_namespace_instance_in(
        Path::new(&dir),
        ns,
        self_pid,
        |pid| crate::platform::process_kill::process_creation_time(pid),
        ESTABLISHED_INSTANCE.get().map(|s| s.as_str()),
    )?;
    let _ = ESTABLISHED_INSTANCE.set(token.clone());
    Some(token)
}

/// Production wrapper for [`read_namespace_instance_in`].
pub fn read_namespace_instance(ns: Option<&str>) -> Option<String> {
    let dir = crate::paths::psmux_dir_opt()?;
    read_namespace_instance_in(Path::new(&dir), ns)
}

/// Terminate live psmux *server* processes that no registry entry accounts for
/// (issue #448). Complements `cleanup_stale_port_files`, which only removes
/// registry files for servers already proven dead: this pass finds a live but
/// orphaned server (a spawn-race duplicate, or a crashed client's headless
/// server) and reaps the process itself, bounding the process count regardless
/// of how the duplicate arose.
pub fn reap_orphaned_servers() {
    let Some(psmux_dir) = crate::paths::psmux_dir_opt() else {
        return;
    };
    reap_orphaned_servers_in(Path::new(&psmux_dir));
}

fn reap_orphaned_servers_in(psmux_dir: &Path) {
    use crate::platform::process_kill;

    // Issue #474: no registry, no reaping. When the data dir does not exist,
    // this invocation cannot see the files that track live servers (an MSYS2
    // login shell that unset USERPROFILE resolves home elsewhere, for
    // example). Proceeding with an empty view would classify every live
    // server on the machine as an orphan and terminate them all.
    if !psmux_dir.is_dir() {
        return;
    }

    let (tracked_ports, tracked_pids) = read_tracked_registry(psmux_dir);
    let self_pid = std::process::id();

    // Capture the reuse-guard cutoff BEFORE enumerating: any process we see now
    // was created at or before this instant, so a PID reused afterwards is
    // rejected by terminate_server_pid (#447 guard).
    let now_ft = process_kill::now_process_filetime();
    // 100ns ticks in the grace window; a process is "old enough" to reap only if
    // its creation time is at or before now - grace.
    let grace_ticks = (ORPHAN_REAP_MIN_AGE.as_nanos() / 100) as u64;
    let age_cutoff_ft = now_ft.saturating_sub(grace_ticks);

    // Group loopback listeners by PID, keeping only psmux-image server processes.
    let mut by_pid: std::collections::HashMap<u32, Vec<u16>> = std::collections::HashMap::new();
    for (pid, port) in process_kill::loopback_listener_pids() {
        by_pid.entry(pid).or_default().push(port);
    }
    let mut candidates: Vec<ServerCandidate> = Vec::new();
    for (pid, ports) in by_pid {
        // Handle-only on purpose, unlike the liveness lookups (#650). A PID
        // that lands here becomes a candidate for TERMINATION, so "I cannot
        // open it, therefore I cannot claim it is mine" is the fail-safe
        // answer. Naming a process psmux cannot open would widen what the
        // reaper is willing to kill, and the reap this issue is about is the
        // registry sweep, not this one.
        let is_psmux = crate::platform::process_info::get_process_name(pid)
            .map(|n| {
                let n = n.to_ascii_lowercase();
                PSMUX_SERVER_IMAGE_NAMES.contains(&n.as_str())
            })
            .unwrap_or(false);
        if !is_psmux { continue; }
        let creation_ft = process_kill::process_creation_time(pid).unwrap_or(u64::MAX);
        candidates.push(ServerCandidate { pid, ports, creation_ft });
    }

    let owned_pids = read_owned_server_pids(psmux_dir);
    let orphans = select_orphan_pids(
        &candidates, &tracked_ports, &tracked_pids, &owned_pids, self_pid, age_cutoff_ft);
    for pid in orphans {
        if crate::debug_log::session_log_enabled() {
            crate::debug_log::session_log("reaper", &format!(
                "terminating orphaned psmux server pid {} (this data dir claims it, no registry entry references it)", pid));
        }
        process_kill::terminate_server_pid(pid, Some(now_ft));
    }

    prune_stale_server_markers(psmux_dir, &owned_pids);
}

/// Delete ownership markers whose process is gone or whose PID now belongs to
/// something else, so the directory tracks live servers rather than growing
/// without bound. Kept separate from reaping: a marker is only a claim, and
/// dropping a claim never terminates anything.
///
/// This is the sole reclamation path rather than a removal on server shutdown:
/// a server can exit down several routes (exit-empty, kill-session, a crash)
/// and a marker missed by any of them would linger forever, so the invariant is
/// "markers are reconciled against the process table", not "every exit tidies
/// up after itself". A lingering marker is harmless in the meantime — it names
/// either a dead PID or, after reuse, a process whose creation time no longer
/// matches, and neither can authorise a kill.
fn prune_stale_server_markers(
    psmux_dir: &Path,
    owned_pids: &std::collections::HashMap<u32, Option<u64>>,
) {
    for (&pid, &recorded_creation) in owned_pids {
        let live_creation = crate::platform::process_kill::process_creation_time(pid);
        let still_ours = match (live_creation, recorded_creation) {
            // Process is gone.
            (None, _) => false,
            // Same PID, different process: the original exited and the PID was
            // reused, so this claim is stale.
            (Some(live), Some(recorded)) => live == recorded,
            // No recorded creation time to compare against. Keep it: a marker
            // that cannot be identity-checked can never authorise a kill
            // either, so leaving it costs nothing.
            (Some(_), None) => true,
        };
        if !still_ours {
            let _ = std::fs::remove_file(psmux_dir.join("servers").join(pid.to_string()));
        }
    }
}

/// Windows FILETIME ticks (100ns since 1601-01-01) for a `SystemTime`.
/// Used to compare a process creation time against a registry file mtime.
fn system_time_to_filetime_ticks(t: SystemTime) -> Option<u64> {
    const UNIX_EPOCH_AS_FILETIME: u64 = 116_444_736_000_000_000;
    let since_unix = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(UNIX_EPOCH_AS_FILETIME.saturating_add((since_unix.as_nanos() / 100) as u64))
}

/// Slack allowed between a `.pid` file's last write and the recorded process's
/// creation time before the PID is considered recycled. A legitimate server is
/// always created BEFORE it writes its `.pid`, so its creation time can never
/// exceed the file mtime; the margin only absorbs filesystem/clock jitter.
const PID_REUSE_MARGIN_TICKS: u64 = 60 * 10_000_000; // 60s in 100ns ticks

/// Definitive liveness verdict from the `.pid` sentinel written next to every
/// `.port` file (issue #448) — no network round-trip.
///
/// This is what keeps CLI startup O(microseconds) per registry entry: a TCP
/// probe of a dead port can burn its full connect timeout on Windows (stealth
/// firewall behavior never sends RST on loopback for some configurations) and
/// then classify as Inconclusive, leaving the stale file to tax EVERY future
/// invocation. The process table answers instantly and definitively.
///
/// Returns:
///   Some(true)  - recorded PID is a live psmux-image process created no later
///                 than the `.pid` file was written -> genuinely our server.
///   Some(false) - PID gone, recycled by a non-psmux image, or recycled by a
///                 psmux process created long after the file -> server is dead.
///   None        - no usable `.pid` anchor (pre-#448 registry) -> caller must
///                 fall back to the network probe.
fn pid_anchor_verdict(port_path: &Path) -> Option<bool> {
    pid_anchor_verdict_with(
        port_path,
        crate::platform::process_info::get_process_name_or_snapshot,
    )
}

/// [`pid_anchor_verdict`] with the process-name lookup injected, so a test can
/// drive the "the handle path is refused but the process is right there in the
/// snapshot" case that issue #650 is about without needing a second Terminal
/// Services session and a differently-elevated token.
fn pid_anchor_verdict_with<F>(port_path: &Path, name_of: F) -> Option<bool>
where
    F: Fn(u32) -> Option<String>,
{
    // The process-table queries below are Windows-only; other platforms fall
    // back to the network probe rather than misreading stub returns as "dead".
    if !cfg!(windows) {
        return None;
    }
    let pid_path = port_path.with_extension("pid");
    // Tolerate both `pid` and `pid:creation_filetime` bodies (the latter written
    // so kill-server can verify identity); the anchor only needs the pid.
    let (pid, _creation) = parse_pid_file_contents(&std::fs::read_to_string(&pid_path).ok()?)?;
    let name = match name_of(pid) {
        // No such process anywhere on the machine — not openable AND not in the
        // process table. That is a genuinely dead PID, so the #448 fast reap
        // still fires here.
        //
        // It used to be enough for the PID to be merely unopenable, on the
        // assumption that a same-user psmux server is always openable with
        // QUERY_LIMITED_INFORMATION. It is not: a server started from an
        // OpenSSH logon lives in Terminal Services session 0 under the elevated
        // token sshd mints, and an unelevated desktop shell in session 1 is
        // refused the handle with ERROR_ACCESS_DENIED, so every desktop
        // invocation, `psmux -V` included, deleted the live server's registry
        // files (#650). See `get_process_name_or_snapshot` for the measured
        // access matrix.
        None => return Some(false),
        Some(n) => n.to_ascii_lowercase(),
    };
    if !PSMUX_SERVER_IMAGE_NAMES.contains(&name.as_str()) {
        // PID recycled by an unrelated application; our server is gone.
        return Some(false);
    }
    // PID-reuse guard (same idea as the #447 reaper guard): a psmux process
    // created well AFTER the .pid file was last written cannot be the server
    // that wrote it. When either timestamp is unavailable, err towards alive.
    if let Some(created_ft) = crate::platform::process_kill::process_creation_time(pid) {
        if let Some(mtime_ft) = std::fs::metadata(&pid_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(system_time_to_filetime_ticks)
        {
            if created_ft > mtime_ft.saturating_add(PID_REUSE_MARGIN_TICKS) {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// Resolve the session key stored alongside a `.port` file (the sibling
/// `.key`). Returns an empty string when the key file is missing, which the
/// identity probe treats as "cannot verify" (Inconclusive) rather than dead.
fn read_key_for_port_path(port_path: &Path) -> String {
    let key_path = port_path.with_extension("key");
    std::fs::read_to_string(&key_path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// Best-effort wall-clock time the system last booted, derived from uptime.
///
/// Any registry file last written before this instant cannot belong to a
/// server that has been running since the machine started — its owning
/// process died with the previous boot (e.g. an OS-update reboot). This is
/// the reliable signal for cleaning up sessions orphaned by a restart, and
/// it does not depend on the network (the old port may now be free, occupied
/// by an unrelated process, or even reused by a *different* live psmux
/// server — all of which a bare TCP probe would misclassify).
#[cfg(windows)]
fn system_boot_time() -> Option<SystemTime> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
    }
    let uptime_ms = unsafe { GetTickCount64() };
    SystemTime::now().checked_sub(Duration::from_millis(uptime_ms))
}

#[cfg(not(windows))]
fn system_boot_time() -> Option<SystemTime> {
    None
}

/// True when `mtime` is old enough (older than `boot - margin`) that the file
/// must have been written by a process from a previous boot.
fn is_pre_boot(mtime: SystemTime, boot: SystemTime, margin: Duration) -> bool {
    match boot.checked_sub(margin) {
        Some(cutoff) => mtime < cutoff,
        None => false,
    }
}

fn cleanup_stale_port_files_in_with<F>(psmux_dir: &Path, mut probe: F)
where
    F: FnMut(&str, u16) -> PortProbeResult,
{
    let boot = system_boot_time();
    if let Ok(entries) = std::fs::read_dir(psmux_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "port").unwrap_or(false) {
                // PID-anchor verdict FIRST (issue #448 sentinel): the process
                // table answers liveness instantly and definitively, so a
                // Some(true) verdict (recorded PID alive with matching
                // creation time) vetoes every heuristic below, including the
                // boot-time guard. File metadata must never be a death
                // certificate for a live server (issue #546): a live server's
                // registry mtimes are frozen at creation (the 5s self-heal
                // rewrites only on content change), so a forward wall-clock
                // step, VM save/restore, or mtime-preserving backup restore
                // pushes the frozen .port mtime behind the derived boot time
                // and the old guard-first ordering reaped the registry of a
                // running session — after which the orphan reaper killed its
                // server — on any psmux invocation at all, even `psmux -V`.
                // The genuine reboot case still reaps: after a restart, any
                // live process holding a recycled PID was created later than
                // .pid mtime + PID_REUSE_MARGIN_TICKS, which yields
                // Some(false).
                let pid_verdict = pid_anchor_verdict(&path);
                if pid_verdict == Some(true) {
                    continue; // live server; nothing to clean
                }
                // Boot-time guard: a port file last modified before this boot
                // belongs to a server that died when the machine restarted.
                // No network round-trip, and immune to the old port being
                // reused by another process this boot. Applies only when the
                // PID anchor could not positively confirm liveness
                // (Some(false) or, for pre-#448 registries, None).
                if let Some(boot) = boot {
                    if let Some(mtime) = entry.metadata().ok().and_then(|m| m.modified().ok()) {
                        if is_pre_boot(mtime, boot, BOOT_TIME_MARGIN) {
                            if crate::debug_log::session_log_enabled() {
                                crate::debug_log::session_log("cleanup", &format!(
                                    "reaping '{}': port file predates last boot (server died on restart)",
                                    registry_base(&path)));
                            }
                            remove_session_registry_files(&path);
                            continue;
                        }
                    }
                }
                // PID-anchor negative path: registry entries with a `.pid`
                // sibling never pay a network probe. Dead-port probes are not
                // just slow (they can burn the full connect timeout per
                // attempt on Windows loopback) - they are also inconclusive,
                // so stale files were never reaped and the probe tax repeated
                // on every subsequent CLI invocation.
                match pid_verdict {
                    Some(false) => {
                        if crate::debug_log::session_log_enabled() {
                            crate::debug_log::session_log("cleanup", &format!(
                                "reaping '{}': recorded server PID is dead or recycled",
                                registry_base(&path)));
                        }
                        remove_session_registry_files(&path);
                        continue;
                    }
                    _ => {} // no .pid anchor; fall through to the network probe
                }
                if let Ok(port_str) = std::fs::read_to_string(&path) {
                    if let Ok(port) = port_str.trim().parse::<u16>() {
                        let key = read_key_for_port_path(&path);
                        if probe(&key, port) == PortProbeResult::Stale {
                            if crate::debug_log::session_log_enabled() {
                                crate::debug_log::session_log("cleanup", &format!(
                                    "reaping '{}' (port {}): no psmux server authenticated as ours (stale)",
                                    registry_base(&path), port));
                            }
                            remove_session_registry_files(&path);
                        }
                    } else {
                        if crate::debug_log::session_log_enabled() {
                            crate::debug_log::session_log("cleanup", &format!(
                                "reaping '{}': unparseable port value {:?}",
                                registry_base(&path), port_str.trim()));
                        }
                        remove_session_registry_files(&path);
                    }
                }
            }
        }
    }
}

/// Display name (file stem) of a registry path, for logging.
fn registry_base(port_path: &Path) -> &str {
    port_path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
}

/// Remove a session's entire registry set, given its `.port` path.
///
/// Teardown paths must go through this rather than deleting extensions
/// individually: the `.port` file is the only entry point the sweeps know, so
/// any satellite left behind when it disappears is unreachable forever (#530).
pub(crate) fn remove_session_registry_files(port_path: &Path) {
    let _ = std::fs::remove_file(port_path);
    let key_path = port_path.with_extension("key");
    let _ = std::fs::remove_file(&key_path);
    let sid_path = port_path.with_extension("sid");
    let _ = std::fs::remove_file(&sid_path);
    // Also drop the twin .pid sentinel (issue #448) so a dead server's PID
    // never lingers to be mistaken for a live tracked process by the reaper.
    let pid_path = port_path.with_extension("pid");
    let _ = std::fs::remove_file(&pid_path);
    // And the `.act` activity stamp (issue #603): a reaped session must not
    // keep ranking in bare CLI routing.
    let act_path = port_path.with_extension("act");
    let _ = std::fs::remove_file(&act_path);
}

/// Outcome of a single AUTH handshake against the listener on a port.
#[derive(Clone, Copy, PartialEq)]
enum AuthProbe {
    /// Server accepted our session key (`OK`) — this is genuinely our session.
    Authenticated,
    /// Server explicitly rejected the key (`ERROR ...`) — a *different* psmux
    /// server has reused this port; the session this file names is dead.
    Rejected,
    /// Connected but the peer didn't complete our protocol (no reply, garbage,
    /// or an unrelated process). Identity is unverifiable from the network.
    Unknown,
}

/// Connect to `addr` and verify, via the AUTH handshake, that the listener is
/// the psmux server that owns `key`.
///
/// Returns `Err(kind)` when the connection itself fails so the caller can tell
/// "nothing is listening" (refused) from a transient network error.
fn probe_auth_identity(addr: std::net::SocketAddr, key: &str) -> Result<AuthProbe, ErrorKind> {
    let mut s = std::net::TcpStream::connect_timeout(&addr, STALE_PORT_CONNECT_TIMEOUT)
        .map_err(|e| e.kind())?;
    // A successful connect alone proves only that *something* listens. Without
    // a key we cannot prove it is ours, so leave the verdict to the boot guard.
    let key = match validate_auth_key(key) {
        Some(k) => k,
        None => return Ok(AuthProbe::Unknown),
    };
    let _ = s.set_read_timeout(Some(STALE_PORT_AUTH_READ_TIMEOUT));
    let _ = s.set_nodelay(true);
    if write!(s, "AUTH {}\n", key).is_err() {
        return Ok(AuthProbe::Unknown);
    }
    let _ = s.flush();
    let mut br = std::io::BufReader::new(std::io::Read::take(s, 4096));
    let mut line = String::new();
    match std::io::BufRead::read_line(&mut br, &mut line) {
        Ok(0) => Ok(AuthProbe::Unknown),
        Ok(_) => {
            let t = line.trim();
            if t == "OK" {
                Ok(AuthProbe::Authenticated)
            } else if t.starts_with("ERROR") {
                Ok(AuthProbe::Rejected)
            } else {
                Ok(AuthProbe::Unknown)
            }
        }
        Err(_) => Ok(AuthProbe::Unknown),
    }
}

/// Identity-aware liveness probe used by stale-port cleanup.
///
/// A bare TCP connect cannot distinguish our server from any other process
/// that grabbed the same port after a crash or reboot — that false "alive"
/// is exactly what left dead sessions showing `(not responding)` in the
/// picker. This probe instead requires the AUTH key to match:
///   - connection refused (or a full timeout with zero response) on every
///     attempt                                    -> `Stale`
///   - server accepts our key (`OK`)              -> `Alive`
///   - server rejects our key (`ERROR`, reused port) -> `Stale`
///   - anything ambiguous (no reply, slow, foreign process) -> `Inconclusive`
///
/// Only definitive signals delete a file; ambiguous ones are left for the
/// boot-time guard, so a live-but-busy server is never reaped by mistake.
///
/// Issue #7 batch D: some environments (confirmed on this host via a raw
/// `TcpClient.BeginConnect`/`WaitOne` probe against several unbound loopback
/// ports) never return an immediate ECONNREFUSED for a closed loopback
/// port — the SYN is silently dropped and the connect attempt runs the full
/// `STALE_PORT_CONNECT_TIMEOUT` before giving up, surfacing as
/// `ErrorKind::TimedOut` rather than `ConnectionRefused`. The original code
/// treated any `TimedOut` as ambiguous ("maybe a live-but-slow server"),
/// which on such a host means the network probe can NEVER return `Stale` —
/// orphaned `.port`/`.key` files with no `.pid` anchor (the only registry
/// shape old enough to still reach this probe) are kept forever. A real
/// live server on loopback completes the AUTH handshake in well under a
/// millisecond; three full timeouts in a row with no byte of response ever
/// received is just as definitive a "nothing is there" signal as an
/// instant refusal, so treat it the same — but ONLY when every attempt saw
/// refused/timed-out and nothing else (any actual response, however
/// unparseable, still keeps the conservative `Inconclusive` verdict).
fn probe_session_for_cleanup(key: &str, port: u16) -> PortProbeResult {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut saw_refused = false;
    let mut saw_timed_out = false;
    let mut saw_inconclusive = false;

    for attempt in 0..STALE_PORT_PROBE_ATTEMPTS {
        match probe_auth_identity(addr, key) {
            Ok(AuthProbe::Authenticated) => {
                if crate::debug_log::session_log_enabled() {
                    crate::debug_log::session_log("probe",
                        &format!("port {}: AUTH accepted -> alive", port));
                }
                return PortProbeResult::Alive;
            }
            Ok(AuthProbe::Rejected) => {
                if crate::debug_log::session_log_enabled() {
                    crate::debug_log::session_log("probe", &format!(
                        "port {}: AUTH rejected by a different server (reused port) -> stale", port));
                }
                return PortProbeResult::Stale;
            }
            Ok(AuthProbe::Unknown) => saw_inconclusive = true,
            Err(ErrorKind::ConnectionRefused) => saw_refused = true,
            Err(ErrorKind::TimedOut) => saw_timed_out = true,
            Err(_) => saw_inconclusive = true,
        }

        if attempt + 1 < STALE_PORT_PROBE_ATTEMPTS {
            std::thread::sleep(STALE_PORT_RETRY_DELAY);
        }
    }

    if (saw_refused || saw_timed_out) && !saw_inconclusive {
        if crate::debug_log::session_log_enabled() {
            crate::debug_log::session_log("probe", &format!(
                "port {}: refused/timed-out on every attempt (refused={}, timed_out={}) -> stale",
                port, saw_refused, saw_timed_out));
        }
        PortProbeResult::Stale
    } else {
        if crate::debug_log::session_log_enabled() {
            crate::debug_log::session_log("probe",
                &format!("port {}: no definitive answer -> inconclusive (kept)", port));
        }
        PortProbeResult::Inconclusive
    }
}

/// Read the session key from the key file
pub fn read_session_key(session: &str) -> io::Result<String> {
    let keypath = crate::paths::key_file(session);
    std::fs::read_to_string(&keypath).map(|s| s.trim().to_string())
}

/// Hard cap on a single response payload read from the server (256 KB).
///
/// The server is trusted, but the client should still bound how much memory
/// a single picker fetch can consume. A buggy or malicious peer that sends
/// an unbounded line with no `\n` would otherwise block until the read
/// timeout while filling the BufReader. 256 KB is comfortably larger than
/// any real `session-info`, `list-tree`, or `choose-buffer` payload.
pub const MAX_AUTHED_RESPONSE_BYTES: u64 = 256 * 1024;

/// Validate that a session key is well-formed for the line-oriented AUTH
/// protocol. Rejects keys containing CR, LF, or NUL — anything that could
/// terminate the AUTH line early or smuggle a second protocol frame.
///
/// Returns the trimmed key on success, `None` on rejection.
///
/// SECURITY: Without this check, a key sourced from a future caller (e.g.
/// env var, IPC, plugin) that contains `\n` could inject a second command
/// onto the AUTH line. All AUTH writers should funnel through this guard.
pub fn validate_auth_key(key: &str) -> Option<&str> {
    let k = key.trim_matches(|c: char| c == '\r' || c == '\n');
    if k.is_empty() {
        return None;
    }
    if k.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return None;
    }
    Some(k)
}

/// Send an authenticated command to a server (fire-and-forget).
///
/// Validates the key against CRLF/NUL injection. Silently no-ops on a
/// malformed key — callers are at the trust boundary already (key file
/// under user's profile), this is defense-in-depth.
pub fn send_auth_cmd(addr: &str, key: &str, cmd: &[u8]) -> io::Result<()> {
    let key = match validate_auth_key(key) {
        Some(k) => k,
        None => return Ok(()),
    };
    let sock_addr: std::net::SocketAddr = addr.parse().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    if let Ok(mut s) = std::net::TcpStream::connect_timeout(&sock_addr, Duration::from_millis(50)) {
        let _ = s.set_nodelay(true);
        let _ = write!(s, "AUTH {}\n", key);
        let _ = std::io::Write::write_all(&mut s, cmd);
        let _ = s.flush();
    }
    Ok(())
}

/// Send an authenticated command and get response.
///
/// Validates the key, caps the response at `MAX_AUTHED_RESPONSE_BYTES`,
/// and returns whatever the server sent after the AUTH ack. The `OK\n`
/// ack is **not** stripped here for backward compatibility with existing
/// callers; new code should prefer `fetch_authed_response` /
/// `fetch_authed_response_multi`.
pub fn send_auth_cmd_response(addr: &str, key: &str, cmd: &[u8]) -> io::Result<String> {
    let key = match validate_auth_key(key) {
        Some(k) => k,
        None => return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session key")),
    };
    let mut s = std::net::TcpStream::connect(addr)?;
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = write!(s, "AUTH {}\n", key);
    let _ = std::io::Write::write_all(&mut s, cmd);
    let _ = s.flush();
    let mut br = std::io::BufReader::new(std::io::Read::take(&mut s, MAX_AUTHED_RESPONSE_BYTES));
    let mut auth_line = String::new();
    let _ = std::io::BufRead::read_line(&mut br, &mut auth_line);
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(&mut br, &mut buf);
    Ok(buf)
}

/// Internal: open an authenticated connection and send a single command.
///
/// Returns a length-capped `BufReader` positioned right after the command
/// write, ready for response parsing. Centralizes:
///   - CRLF/NUL key validation (security)
///   - connect timeout, read timeout, TCP_NODELAY
///   - response size cap (`MAX_AUTHED_RESPONSE_BYTES`, DoS guard)
///   - the AUTH + command write
///
/// The size cap is applied with `Read::take` BEFORE the `BufReader` so the
/// resulting reader still exposes `BufRead`. Wrapping the other way around
/// (`BufReader::take`) loses `BufRead` because `Take` is `Read`-only.
fn open_authed(
    addr: &str,
    key: &str,
    cmd: &[u8],
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Option<std::io::BufReader<std::io::Take<std::net::TcpStream>>> {
    let key = validate_auth_key(key)?;
    let sock_addr: std::net::SocketAddr = addr.parse().ok()?;
    let mut s = std::net::TcpStream::connect_timeout(&sock_addr, connect_timeout).ok()?;
    s.set_read_timeout(Some(read_timeout)).ok()?;
    let _ = s.set_nodelay(true);
    write!(s, "AUTH {}\n", key).ok()?;
    s.write_all(cmd).ok()?;
    if !cmd.ends_with(b"\n") {
        s.write_all(b"\n").ok()?;
    }
    let _ = s.flush();
    Some(std::io::BufReader::new(std::io::Read::take(s, MAX_AUTHED_RESPONSE_BYTES)))
}

/// Read one response line from an authenticated stream, transparently
/// skipping the `OK\n` AUTH ack regardless of when it arrives.
///
/// Returns `None` on timeout, EOF, empty payload, or `ERROR:` reply.
/// Returns `Some(line)` on a valid payload (newline trimmed).
fn read_authed_line<R: std::io::BufRead>(br: &mut R) -> Option<String> {
    // First read: could be either the AUTH ack ("OK") or the payload
    // (if the ack was already pipelined into the same packet).
    let mut line = String::new();
    if std::io::BufRead::read_line(br, &mut line).ok()? == 0 {
        return None;
    }
    let trimmed = line.trim();
    if trimmed == "OK" {
        // First line WAS the ack. Read the real payload now.
        line.clear();
        if std::io::BufRead::read_line(br, &mut line).ok()? == 0 {
            return None;
        }
    }
    // Filter again in case the second line is also empty/error/OK.
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "OK" || trimmed.starts_with("ERROR:") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Read all remaining bytes from an authenticated stream, stripping a
/// leading `OK\n` AUTH ack if present.
///
/// Returns `None` on no payload, error response, or read failure.
/// Returns `Some(payload)` with the AUTH ack removed and trailing
/// whitespace stripped. Total read is capped by the underlying `Take`.
fn read_authed_all<R: std::io::Read>(rd: &mut R) -> Option<String> {
    let mut buf = String::new();
    std::io::Read::read_to_string(rd, &mut buf).ok()?;
    let body = buf.strip_prefix("OK\n").or_else(|| buf.strip_prefix("OK\r\n")).unwrap_or(&buf);
    let trimmed = body.trim();
    if trimmed.is_empty() || trimmed.starts_with("ERROR:") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Send an authenticated single-command request and return one response line.
///
/// Centralized AUTH + command + response helper used by all picker fetches.
/// Handles every known framing race for the AUTH ack:
///   - ack pipelined with payload (one packet, both lines arrive together)
///   - ack arrives first, then payload
///   - ack delayed past first read (issue #250 race)
///   - server replies only `OK` and never sends payload
///   - server replies `ERROR: ...`
///   - server hangs / connection refused / bad address
///
/// All callers get the same robust behavior; they can no longer reinvent
/// the parser per-site (which is how #250 happened).
pub fn fetch_authed_response(
    addr: &str,
    key: &str,
    cmd: &[u8],
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Option<String> {
    let mut br = open_authed(addr, key, cmd, connect_timeout, read_timeout)?;
    read_authed_line(&mut br)
}

/// Like `fetch_authed_response` but returns the entire response body
/// (multi-line payloads such as `list-tree` JSON arrays or `choose-buffer`
/// listings). The leading AUTH ack line is stripped if present.
///
/// The total payload is bounded by `MAX_AUTHED_RESPONSE_BYTES` to prevent
/// a malformed or hostile server from forcing unbounded client memory.
pub fn fetch_authed_response_multi(
    addr: &str,
    key: &str,
    cmd: &[u8],
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Option<String> {
    let mut br = open_authed(addr, key, cmd, connect_timeout, read_timeout)?;
    read_authed_all(&mut br)
}

/// Fetch a one-line `session-info` response from a session server.
///
/// Thin wrapper over `fetch_authed_response` retained for the call site
/// in `client.rs` (and the regression tests added in PR #251 for #250).
pub fn fetch_session_info(
    addr: &str,
    key: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Option<String> {
    fetch_authed_response(addr, key, b"session-info\n", connect_timeout, read_timeout)
}

/// Fan out `fetch_session_info` across many sessions in parallel.
///
/// The session picker used to call `fetch_session_info` sequentially, so
/// opening the picker with N sessions was bounded by `N * read_timeout`
/// in the worst case. With this helper, N concurrent threads share that
/// bound: total wall time is roughly `read_timeout`, regardless of N.
///
/// `inputs` is `(label, addr, key)`. Output preserves input order and
/// pairs each label with the fetched info or the supplied `fallback`
/// (typically `"<label>: (not responding)"`).
///
/// Retained for the #250 regression suite; the picker now uses
/// `classify_sessions_parallel`, which both lists and prunes in one pass.
#[allow(dead_code)]
pub fn fetch_session_infos_parallel<F>(
    inputs: Vec<(String, String, String)>,
    connect_timeout: Duration,
    read_timeout: Duration,
    fallback: F,
) -> Vec<(String, String)>
where
    F: Fn(&str) -> String + Send + Sync,
{
    if inputs.is_empty() {
        return Vec::new();
    }
    // Single session: skip thread spawn overhead entirely.
    if inputs.len() == 1 {
        let (label, addr, key) = &inputs[0];
        let info = fetch_session_info(addr, key, connect_timeout, read_timeout)
            .unwrap_or_else(|| fallback(label));
        return vec![(label.clone(), info)];
    }
    let results: Vec<(String, String)> = std::thread::scope(|scope| {
        let fallback_ref = &fallback;
        let handles: Vec<_> = inputs
            .iter()
            .map(|(label, addr, key)| {
                let label = label.clone();
                let addr = addr.clone();
                let key = key.clone();
                scope.spawn(move || {
                    let info = fetch_session_info(&addr, &key, connect_timeout, read_timeout)
                        .unwrap_or_else(|| fallback_ref(&label));
                    (label, info)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    results
}

/// Liveness verdict for one session, produced by a single bounded probe.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionLiveness {
    /// Server authenticated and returned its session-info line (the payload).
    Alive(String),
    /// Definitively gone: the connection failed (refused / unreachable — on
    /// loopback any connect failure means nothing is listening), an `ERROR`
    /// auth rejection (a different server reused the port), or connected then
    /// silent past the read timeout. Its registry files should be reaped. A
    /// genuinely live server that was momentarily too slow self-heals: it
    /// rewrites its `.port`/`.key`/`.sid` every 5s (see
    /// `ensure_session_registry_files`).
    Dead,
    /// No usable AUTH key on disk, so identity cannot be verified at all.
    /// Left in place and shown as `(not responding)` rather than deleted.
    Unreachable,
}

/// Single bounded liveness probe: connect, AUTH with the session's own key,
/// ask for `session-info`, and classify the reply. Never retries or blocks
/// beyond `connect_timeout + read_timeout`.
fn probe_session_liveness(
    addr: &str,
    key: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> SessionLiveness {
    let sock: std::net::SocketAddr = match addr.parse() {
        Ok(a) => a,
        Err(_) => return SessionLiveness::Dead,
    };
    let key = match validate_auth_key(key) {
        Some(k) => k,
        None => return SessionLiveness::Unreachable,
    };
    // On loopback a live server always completes the TCP handshake (the kernel
    // accepts into the listen backlog even before the app calls accept()), so
    // ANY connect failure means nothing usable is listening -> Dead. We do not
    // branch on the error kind: Windows does not always surface a clean
    // `ConnectionRefused` for a free port.
    let mut s = match std::net::TcpStream::connect_timeout(&sock, connect_timeout) {
        Ok(s) => s,
        Err(_) => return SessionLiveness::Dead,
    };
    let _ = s.set_read_timeout(Some(read_timeout));
    let _ = s.set_nodelay(true);
    if write!(s, "AUTH {}\n", key).is_err() || s.write_all(b"session-info\n").is_err() {
        // Connection broke right after connect -> not a healthy server.
        return SessionLiveness::Dead;
    }
    let _ = s.flush();
    let mut br = std::io::BufReader::new(std::io::Read::take(s, MAX_AUTHED_RESPONSE_BYTES));
    let mut line = String::new();
    match std::io::BufRead::read_line(&mut br, &mut line) {
        Ok(0) => SessionLiveness::Dead,
        Ok(_) => {
            let t = line.trim();
            if t.starts_with("ERROR") {
                return SessionLiveness::Dead;
            }
            if t == "OK" {
                // Ack consumed; the next line is the real payload.
                line.clear();
                match std::io::BufRead::read_line(&mut br, &mut line) {
                    Ok(0) => SessionLiveness::Dead,
                    Ok(_) => {
                        let t2 = line.trim();
                        if t2.is_empty() || t2 == "OK" || t2.starts_with("ERROR") {
                            SessionLiveness::Dead
                        } else {
                            SessionLiveness::Alive(t2.to_string())
                        }
                    }
                    Err(_) => SessionLiveness::Dead,
                }
            } else {
                // Ack pipelined with the payload in one line.
                SessionLiveness::Alive(t.to_string())
            }
        }
        Err(_) => SessionLiveness::Dead,
    }
}

/// Classify many sessions in parallel with a single bounded probe each.
///
/// Like `fetch_session_infos_parallel`, total wall time is ~one probe window
/// regardless of N (each session runs on its own thread). Returns the liveness
/// verdict per input label, preserving order, so the caller can reap the dead
/// ones and render the rest. This is what keeps the session picker responsive:
/// it replaces a sequential cleanup pass (O(N * timeout)) with one parallel
/// round-trip that both lists and prunes.
pub fn classify_sessions_parallel(
    inputs: Vec<(String, String, String)>,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Vec<(String, SessionLiveness)> {
    if inputs.is_empty() {
        return Vec::new();
    }
    if inputs.len() == 1 {
        let (label, addr, key) = &inputs[0];
        let v = probe_session_liveness(addr, key, connect_timeout, read_timeout);
        return vec![(label.clone(), v)];
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = inputs
            .iter()
            .map(|(label, addr, key)| {
                let label = label.clone();
                let addr = addr.clone();
                let key = key.clone();
                scope.spawn(move || {
                    let v = probe_session_liveness(&addr, &key, connect_timeout, read_timeout);
                    (label, v)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

/// PID-anchor liveness for the session registered under `base`, for
/// enumeration paths (e.g. CLI `list-sessions`) that would otherwise pay a
/// TCP connect timeout per dead entry. Some(false) = definitively dead
/// (reap + skip), Some(true) = live, None = no anchor (probe as usual).
pub fn registry_pid_anchor_alive(base: &str) -> Option<bool> {
    let port_path = crate::paths::port_file_opt(base)?;
    pid_anchor_verdict(Path::new(&port_path))
}

/// Reap a single session's registry files (`.port`/`.key`/`.sid`) by base name.
///
/// Used when a probe proves the session is dead. Safe against a live server:
/// it re-creates these files on its next 5s registry tick.
pub fn remove_session_registry(base: &str) {
    let Some(port_path) = crate::paths::port_file_opt(base) else {
        return;
    };
    remove_session_registry_files(Path::new(&port_path));
}

pub fn send_control(line: String) -> io::Result<()> {
    let mut target = env::var("PSMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    if env::var("PSMUX_ROUTE_DEBUG").is_ok() {
        eprintln!("[route] send_control target={:?} full={:?} argv={:?} line={:?}",
            target, env::var("PSMUX_TARGET_FULL").ok(),
            env::args().collect::<Vec<_>>(), line.trim());
    }
    // Never target a warm (standby) session — resolve to a real session instead
    if is_warm_session(&target) {
        // Extract namespace from warm session name (e.g. "foo____warm__" -> Some("foo"))
        let ns = target.strip_suffix("____warm__").map(|s| s.to_string());
        target = resolve_last_session_name_ns(ns.as_deref()).unwrap_or_else(|| "default".to_string());
    }
    let full_target = env::var("PSMUX_TARGET_FULL").ok();
    let path = crate::paths::port_file(&target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("no server running on session '{}'", target)))?.clone();
    let session_key = read_session_key(&target).unwrap_or_default();
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
    // 1s connect timeout: a busy-but-alive server must not be mistaken for a
    // dead one. The old 100ms falsely tripped callers' stale-port cleanup,
    // deleting the port file of a server that was merely slow to accept.
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(1000))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
    let _ = write!(stream, "AUTH {}\n", session_key);
    if let Some(ref ft) = full_target {
        let _ = write!(stream, "TARGET {}\n", ft);
    }
    let _ = write!(stream, "{}", line);
    // Tier 2 — confirmed EXECUTION (not just delivery): append a `session-info`
    // barrier. It round-trips through the server's single FIFO event loop, so its
    // reply proves the command above was actually applied, not merely enqueued.
    // This makes send_control synchronous and closes races where a caller inspects
    // the effect immediately after the CLI returns. For commands whose server
    // handler tears down the connection first (e.g. kill-session), the barrier is
    // simply never answered — that path is covered by the caller's verify-retry.
    let _ = write!(stream, "session-info\n");
    let _ = stream.flush();
    // Half-close the write side so the server observes EOF *after* our bytes.
    // TCP guarantees all sent data is delivered before the FIN, so the server's
    // read_line always sees the full command before its loop ends — eliminating
    // the RST-on-close race that used to silently drop fire-and-forget commands
    // (the old 50ms "drain" read was only a partial mitigation).
    let _ = stream.shutdown(std::net::Shutdown::Write);
    // Read to EOF (bounded by the read timeout): blocks until the server has
    // processed the barrier — i.e. the command has executed — or the connection
    // closes / times out.
    let mut buf = [0u8; 256];
    loop {
        match std::io::Read::read(&mut stream, &mut buf) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    Ok(())
}

pub fn send_control_with_response(line: String) -> io::Result<String> {
    let mut target = env::var("PSMUX_TARGET_SESSION").ok().unwrap_or_else(|| "default".to_string());
    if env::var("PSMUX_ROUTE_DEBUG").is_ok() {
        eprintln!("[route] send_control_with_response target={:?} full={:?} argv={:?} line={:?}",
            target, env::var("PSMUX_TARGET_FULL").ok(),
            env::args().collect::<Vec<_>>(), line.trim());
    }
    // Never target a warm (standby) session — resolve to a real session instead
    if is_warm_session(&target) {
        let ns = target.strip_suffix("____warm__").map(|s| s.to_string());
        target = resolve_last_session_name_ns(ns.as_deref()).unwrap_or_else(|| "default".to_string());
    }
    let full_target = env::var("PSMUX_TARGET_FULL").ok();
    let path = crate::paths::port_file(&target);
    let port = std::fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u16>().ok()).ok_or_else(|| io::Error::new(io::ErrorKind::Other, format!("no server running on session '{}'", target)))?.clone();
    let session_key = read_session_key(&target).unwrap_or_default();
    // Bounded connect: against a saturated listen backlog, a bare connect()
    // fails only after the ~21s Windows SYN-retransmit and surfaces as the
    // notorious `os error 10060`. A 1s timeout turns that into a fast, retryable
    // error instead of a multi-second hang printed to the user.
    let addr: std::net::SocketAddr = format!("127.0.0.1:{}", port).parse()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "bad server address"))?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(1000))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(3000)));
    let _ = write!(stream, "AUTH {}\n", session_key);
    if let Some(ref ft) = full_target {
        let _ = write!(stream, "TARGET {}\n", ft);
    }
    let _ = write!(stream, "{}", line);
    let _ = stream.flush();
    // Half-close so the server sees EOF after our request and closes the socket
    // once the reply is complete — giving a definitive Ok(0) end-of-response
    // instead of relying on an idle-gap timeout to guess the reply is done.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut buf = Vec::new();
    let mut temp = [0u8; 4096];
    let mut timed_out = false;
    loop {
        match std::io::Read::read(&mut stream, &mut temp) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&temp[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => { timed_out = true; break; }
            Err(_) => break,
        }
    }
    let result = String::from_utf8_lossy(&buf).to_string();
    // Strip the "OK\n" AUTH response prefix if present. This is protocol
    // framing, not payload — it must come off before the checks below, because
    // the server writes it before it has even read the command (issue #561).
    let result = if result.starts_with("OK\n") {
        result[3..].to_string()
    } else if result.starts_with("OK\r\n") {
        result[4..].to_string()
    } else {
        result
    };
    // A connection-level refusal is not reply data. The server writes these
    // INSTEAD of running the command (and without the OK ack), but every caller
    // simply printed the reply, so `list-windows` put "ERROR: Authentication
    // required" on STDOUT at exit 0 and a machine consumer ingested it as a
    // window record (issue #561). Classified here, at the single chokepoint,
    // rather than at the 22 call sites that print the reply.
    //
    // Matched as exact whole-payload strings on purpose, NOT as an "ERROR:"
    // prefix: capture-pane and show-buffer return arbitrary pane content, which
    // may legitimately begin with the word ERROR, and misreading that as a
    // refusal would be a worse bug than the one being fixed.
    let trimmed = result.trim();
    if trimmed == "ERROR: Authentication required" || trimmed == "ERROR: Invalid session key" {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            trimmed.trim_start_matches("ERROR:").trim().to_string(),
        ));
    }
    // A read timeout means the reply is INCOMPLETE. The half-close above makes a
    // complete reply end in a definitive server-side EOF (Ok(0)), so landing in
    // the timeout branch at all means we did not get the whole answer, whatever
    // is already in `buf`. The old guard also required `buf.is_empty()`, which
    // the OK ack made permanently false on any authenticated connection, so a
    // stall returned Ok("") and a truncation returned Ok(partial) at exit 0 —
    // indistinguishable from an empty result set and from a complete one
    // (issue #561, cases B and C).
    if timed_out {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "no response from server (timed out)"));
    }
    Ok(result)
}

/// Send a control message to a specific port with authentication
pub fn send_control_to_port(port: u16, msg: &str, session_key: &str) -> io::Result<()> {
    let addr = format!("127.0.0.1:{}", port);
    if let Ok(mut stream) = std::net::TcpStream::connect(&addr) {
        let _ = stream.set_nodelay(true);
        let _ = write!(stream, "AUTH {}\n", session_key);
        let _ = stream.write_all(msg.as_bytes());
        let _ = stream.flush();
        // Drain the OK response to prevent RST
        let mut buf = [0u8; 64];
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        let _ = std::io::Read::read(&mut stream, &mut buf);
    }
    Ok(())
}

/// Shortest gap between two `.act` writes for the same session on the
/// per-keystroke path.
///
/// tmux restamps `session.activity_time` on every single key (server-client.c
/// `server_client_handle_key`) because the value is a struct in its own address
/// space. psmux's copy is a file, so a burst of typing would otherwise be a
/// burst of writes; a one second floor keeps the ranking accurate to well
/// within any interval a human notices while costing at most one 16 byte write
/// per second per attached client.
const ACTIVITY_STAMP_MIN_GAP: Duration = Duration::from_millis(1000);

/// Last `.act` write this process performed, as (session, when). Only the
/// throttled path consults it; attach and switch always write.
static LAST_ACTIVITY_STAMP: std::sync::Mutex<Option<(String, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// The stamp written into a `.act` file: microseconds since the Unix epoch.
///
/// Microseconds, not milliseconds, because the value is compared against a
/// `.port` file's mtime, which NTFS keeps to 100ns. A millisecond stamp written
/// just after a port file can truncate to BELOW that file's mtime and lose a
/// comparison it should win. Microseconds is also the resolution tmux keeps
/// `activity_time` at (a `struct timeval`).
fn epoch_micros_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Record `session` as active right now: psmux's `session_update_activity`
/// (tmux session.c). Warm (standby) sessions are internal and never ranked, so
/// they are never stamped.
pub fn touch_session_activity(session: &str) {
    let Some(dir) = crate::paths::psmux_dir_opt() else { return };
    touch_session_activity_in(std::path::Path::new(&dir), session);
}

/// Registry-directory-parameterized variant of [`touch_session_activity`],
/// matching the `_in` convention the routing resolvers use: taking the dir
/// explicitly lets the writer be unit-tested without mutating the process-wide
/// `PSMUX_DATA_DIR`, which other tests read without holding the env lock.
pub fn touch_session_activity_in(dir: &std::path::Path, session: &str) {
    if session.is_empty() || is_warm_session(session) {
        return;
    }
    let path = dir.join(format!("{}.act", session));
    if std::fs::write(path, epoch_micros_now().to_string()).is_ok() {
        // Arm the throttle too: the stamp this just wrote IS current, so the
        // first keystroke after an attach has nothing to add.
        if let Ok(mut guard) = LAST_ACTIVITY_STAMP.lock() {
            *guard = Some((session.to_string(), std::time::Instant::now()));
        }
    }
}

/// [`touch_session_activity`] for the per-keystroke path: a no-op unless
/// `ACTIVITY_STAMP_MIN_GAP` has passed since this process last stamped this
/// same session. A different session always writes, so a client that switches
/// sessions stamps the new one immediately.
pub fn touch_session_activity_throttled(session: &str) {
    if session.is_empty() || is_warm_session(session) {
        return;
    }
    if throttled_out(session) {
        return;
    }
    touch_session_activity(session);
}

/// Registry-directory-parameterized [`touch_session_activity_throttled`].
pub fn touch_session_activity_throttled_in(dir: &std::path::Path, session: &str) {
    if session.is_empty() || is_warm_session(session) {
        return;
    }
    if throttled_out(session) {
        return;
    }
    touch_session_activity_in(dir, session);
}

/// True while this process's last stamp for `session` is still inside
/// `ACTIVITY_STAMP_MIN_GAP`.
fn throttled_out(session: &str) -> bool {
    let Ok(guard) = LAST_ACTIVITY_STAMP.lock() else { return true };
    match *guard {
        Some((ref last, when)) => last == session && when.elapsed() < ACTIVITY_STAMP_MIN_GAP,
        None => false,
    }
}

/// When `base` was last active, as ranked by bare CLI routing.
///
/// The `.act` stamp when one exists, else the `.port` file's mtime. The
/// fallback is the creation time of the session, which is exactly what tmux
/// seeds `activity_time` with for a session nobody has attached to yet
/// (session.c `session_create`: `session_update_activity(s, &s->creation_time)`),
/// and it keeps a registry written by an older psmux ranking sensibly.
fn session_activity_in(
    dir: &std::path::Path,
    base: &str,
    port_meta: Option<std::fs::Metadata>,
) -> std::time::SystemTime {
    if let Ok(text) = std::fs::read_to_string(dir.join(format!("{}.act", base))) {
        if let Ok(us) = text.trim().parse::<u64>() {
            return std::time::UNIX_EPOCH + Duration::from_micros(us);
        }
    }
    port_meta
        .and_then(|m| m.modified().ok())
        .unwrap_or(std::time::UNIX_EPOCH)
}

pub fn resolve_last_session_name() -> Option<String> {
    resolve_last_session_name_ns(None)
}

/// Resolve the most recently modified session, optionally filtered by -L namespace.
/// When `ns` is Some("foo"), only sessions with port files named "foo__*" are considered
/// and the returned name includes the prefix (e.g. "foo__dev").
/// When `ns` is None, only non-namespaced sessions (no "__" in name) are considered.
pub fn resolve_last_session_name_ns(ns: Option<&str>) -> Option<String> {
    let psmux_dir = crate::paths::psmux_dir_opt()?;
    resolve_last_session_name_ns_in(std::path::Path::new(&psmux_dir), ns)
}

/// Registry-directory-parameterized variant of [`resolve_last_session_name_ns`]:
/// the most recently ACTIVE real (non-warm) session base in namespace `ns`.
/// Taking the dir explicitly lets routing be unit-tested without mutating
/// `USERPROFILE`/`HOME`.
///
/// tmux parity (issue #603). tmux picks this session in cmd-find.c
/// `cmd_find_best_session`, whose comparator `cmd_find_session_better` gets no
/// `CMD_FIND_PREFER_UNATTACHED` flag for any ordinary command and so collapses
/// to a single `timercmp` on `activity_time`. The winner
/// is simply the session with the newest activity, and activity is restamped on
/// client attach and on every key a real client sends (server-client.c).
///
/// psmux used to answer this with the `last_session` file alone: whatever name
/// was in it won outright as long as its `.port` still existed. That file is
/// written once per attach and never again, so a session attached long ago and
/// since detached kept beating the session the user is actually sitting in. It
/// is now only a tie-break for candidates whose stamps are identical, which in
/// practice means a registry an older psmux wrote and nobody has attached to
/// since.
pub fn resolve_last_session_name_ns_in(dir: &std::path::Path, ns: Option<&str>) -> Option<String> {
    let hint = std::fs::read_to_string(dir.join("last_session"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let ns_prefix = ns.map(|n| format!("{}__", n));

    // (activity, is_last_session_hint, base): ranked in that order, newest and
    // then hinted first, with the name as a final deterministic tie-break.
    let mut best: Option<(std::time::SystemTime, bool, String)> = None;
    let Ok(rd) = std::fs::read_dir(dir) else { return None };
    for e in rd.flatten() {
        let Some(fname) = e.file_name().to_str().map(|s| s.to_string()) else { continue };
        let Some((base, ext)) = fname.rsplit_once('.') else { continue };
        if ext != "port" || is_warm_session(base) {
            continue;
        }
        // Filter by namespace: -L sessions have "ns__name" format.
        let in_ns = match ns_prefix {
            Some(ref prefix) => base.starts_with(prefix.as_str()),
            None => !base.contains("__"),
        };
        if !in_ns {
            continue;
        }
        let activity = session_activity_in(dir, base, e.metadata().ok());
        let hinted = hint.as_deref() == Some(base);
        let better = match best {
            None => true,
            Some((best_act, best_hinted, ref best_base)) => {
                activity > best_act
                    || (activity == best_act
                        && ((hinted && !best_hinted)
                            || (hinted == best_hinted && base < best_base.as_str())))
            }
        };
        if better {
            best = Some((activity, hinted, base.to_string()));
        }
    }
    best.map(|(_, _, base)| base)
}

/// Resolve the routing target session (the port-file base name) for a CLI
/// command that did not name an explicit `-t session`.
///
/// Socket-selection precedence follows tmux: `-L`/`-S` take precedence over
/// `$TMUX`, which tmux consults only when neither is given (tmux.c `main()`:
/// `if (path == NULL && label == NULL)`). psmux has no single socket, so this
/// maps to: adopt the current server named by `$TMUX` only when its session is
/// in the requested `-L` namespace; otherwise resolve within the namespace (the
/// most-recent session, else the namespaced `X__default`).
///
/// `$TMUX` (set inside every psmux pane, formatted `<socketpath>,<port>,<idx>`)
/// identifies the current server via its control `<port>`; `psmux_dir` is the
/// `.psmux` registry directory of `<base>.port` files; `l_socket_name` is the
/// `-L` namespace, if any. Returns the base to route to, or `None`.
pub fn resolve_routing_target(
    l_socket_name: Option<&str>,
    tmux_env: Option<&str>,
    psmux_dir: &std::path::Path,
) -> Option<String> {
    // Adopt the current server (named by `$TMUX`) only when it is in-namespace.
    if let Some(tmux_val) = tmux_env {
        if let Some(base) = session_base_owning_tmux_port(tmux_val, psmux_dir) {
            let in_namespace = match l_socket_name {
                Some(ns) => base.starts_with(&format!("{}__", ns)),
                None => true,
            };
            if in_namespace {
                return Some(base);
            }
        }
    }
    // Otherwise the most recent real session in the namespace,
    if let Some(name) = resolve_last_session_name_ns_in(psmux_dir, l_socket_name) {
        return Some(name);
    }
    // else a namespaced `X__default` so `-L X` never leaks to the un-namespaced
    // `default` server. With no `-L`, stay unresolved (the caller keeps "default").
    l_socket_name.map(|ns| format!("{}__default", ns))
}

/// Find the session port-file base whose control port matches the one encoded
/// in a `$TMUX` value (`<socketpath>,<port>,<idx>`). Warm (standby) sessions are
/// internal-only and never adopted.
fn session_base_owning_tmux_port(tmux_val: &str, psmux_dir: &std::path::Path) -> Option<String> {
    let port: u16 = tmux_val.split(',').nth(1)?.trim().parse().ok()?;
    for entry in std::fs::read_dir(psmux_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "port").unwrap_or(false) {
            if let Ok(port_str) = std::fs::read_to_string(&path) {
                if port_str.trim().parse::<u16>().ok() == Some(port) {
                    let base = path.file_stem().and_then(|s| s.to_str())?;
                    return if is_warm_session(base) { None } else { Some(base.to_string()) };
                }
            }
        }
    }
    None
}

pub fn resolve_default_session_name() -> Option<String> {
    if let Ok(name) = env::var("PSMUX_DEFAULT_SESSION") {
        let p = crate::paths::port_file(&name);
        if std::path::Path::new(&p).exists() { return Some(name); }
    }
    // `.psmuxrc` is a home-relative config file, not part of the data dir, so it
    // stays under home_dir(); `pmuxrc` and the port files live in the data dir.
    let home = crate::paths::home_dir();
    let candidates = [format!("{}\\.psmuxrc", home), format!("{}\\pmuxrc", crate::paths::psmux_dir())];
    for cfg in candidates.iter() {
        if let Ok(text) = std::fs::read_to_string(cfg) {
            let line = text.lines().find(|l| !l.trim().is_empty())?;
            let name = if let Some(rest) = line.strip_prefix("default-session ") { rest.trim().to_string() } else { line.trim().to_string() };
            let p = crate::paths::port_file(&name);
            if std::path::Path::new(&p).exists() { return Some(name); }
        }
    }
    None
}

pub fn reap_children_placeholder() -> io::Result<bool> { Ok(false) }

/// Return the names of all live sessions by scanning .psmux/*.port files.
pub fn list_session_names() -> Vec<String> {
    list_session_names_ns(None)
}

/// Return session names filtered by namespace (same logic as resolve_last_session_name_ns).
pub fn list_session_names_ns(ns: Option<&str>) -> Vec<String> {
    let dir = crate::paths::psmux_dir();
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if let Some(fname) = e.file_name().to_str().map(|s| s.to_string()) {
                if let Some((base, ext)) = fname.rsplit_once('.') {
                    if ext == "port" {
                        if is_warm_session(base) { continue; }
                        // Filter by namespace
                        match ns {
                            Some(prefix) => {
                                if !base.starts_with(&format!("{}__", prefix)) { continue; }
                            }
                            None => {
                                if base.contains("__") { continue; }
                            }
                        }
                        names.push(base.to_string());
                    }
                }
            }
        }
    }
    names.sort();
    names
}

/// A tree entry used by choose-tree: either a session header or a window under a session.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub session_name: String,
    pub session_port: u16,
    pub is_session_header: bool,
    pub window_index: Option<usize>,
    pub window_name: String,
    pub window_panes: usize,
    pub window_size: String,
    pub is_current_session: bool,
    pub is_active_window: bool,
}

/// The sessions the tree chooser offers to a client attached as
/// `current_session`: every live registry entry in the same `-L` namespace,
/// warm helpers excluded, sorted by name. Directory parameterised so it can
/// be tested without touching `USERPROFILE`.
pub fn tree_chooser_sessions_in(
    dir: &std::path::Path,
    current_session: &str,
) -> Vec<(String, u16, std::time::SystemTime)> {
    let ns = session_namespace(current_session);
    let mut sessions: Vec<(String, u16, std::time::SystemTime)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "port").unwrap_or(false) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    // Hide warm (standby) sessions from choose-tree
                    if is_warm_session(stem) { continue; }
                    // Another -L namespace is another server: never listed.
                    if !session_visible_from(stem, ns) { continue; }
                    if let Ok(port_str) = std::fs::read_to_string(&path) {
                        if let Ok(port) = port_str.trim().parse::<u16>() {
                            let mtime = entry.metadata()
                                .and_then(|m| m.modified())
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                            sessions.push((stem.to_string(), port, mtime));
                        }
                    }
                }
            }
        }
    }
    sessions.sort_by_key(|(name, _, _)| name.clone());
    sessions
}

/// List all running sessions and their windows for choose-tree display.
/// Queries each running server via its TCP port for window list info.
pub fn list_all_sessions_tree(current_session: &str, current_windows: &[(String, usize, String, bool, usize)]) -> Vec<TreeEntry> {
    let Some(psmux_dir) = crate::paths::psmux_dir_opt() else {
        return vec![];
    };
    let sessions = tree_chooser_sessions_in(std::path::Path::new(&psmux_dir), current_session);

    let mut tree = Vec::new();
    for (name, port, _) in &sessions {
        let is_current = name == current_session;
        // Session header
        tree.push(TreeEntry {
            session_name: name.clone(),
            session_port: *port,
            is_session_header: true,
            window_index: None,
            window_name: String::new(),
            window_panes: 0,
            window_size: String::new(),
            is_current_session: is_current,
            is_active_window: false,
        });

        if is_current {
            // Use local data for the current session (fast, no IPC)
            for (wname, panes, size, is_active, disp_idx) in current_windows.iter() {
                tree.push(TreeEntry {
                    session_name: name.clone(),
                    session_port: *port,
                    is_session_header: false,
                    window_index: Some(*disp_idx),
                    window_name: wname.clone(),
                    window_panes: *panes,
                    window_size: size.clone(),
                    is_current_session: true,
                    is_active_window: *is_active,
                });
            }
        } else {
            // Query remote session for its window list
            let key = read_session_key(name).unwrap_or_default();
            let addr = format!("127.0.0.1:{}", port);
            if let Ok(resp) = send_auth_cmd_response(&addr, &key, b"list-windows -F \"#{window_index}:#{window_name}:#{window_panes}:#{window_width}x#{window_height}:#{window_active}\"\n") {
                for line in resp.lines() {
                    let line = line.trim();
                    if line.is_empty() { continue; }
                    let parts: Vec<&str> = line.splitn(5, ':').collect();
                    if parts.len() >= 5 {
                        let wi = parts[0].parse::<usize>().unwrap_or(0);
                        let wn = parts[1].to_string();
                        let wp = parts[2].parse::<usize>().unwrap_or(1);
                        let ws = parts[3].to_string();
                        let wa = parts[4] == "1";
                        tree.push(TreeEntry {
                            session_name: name.clone(),
                            session_port: *port,
                            is_session_header: false,
                            window_index: Some(wi),
                            window_name: wn,
                            window_panes: wp,
                            window_size: ws,
                            is_current_session: false,
                            is_active_window: wa,
                        });
                    }
                }
            }
        }
    }
    tree
}

#[cfg(test)]
#[path = "../tests-rs/test_session.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_issue649_kill_server_namespace_scope.rs"]
mod tests_issue649_kill_server_namespace_scope;

#[cfg(test)]
#[path = "../tests-rs/test_issue250_root_cause.rs"]
mod tests_issue250_root_cause;

#[cfg(test)]
#[path = "../tests-rs/test_session_id_alloc_race.rs"]
mod tests_session_id_alloc_race;

#[cfg(test)]
#[path = "../tests-rs/test_issue448_orphan_reaper.rs"]
mod tests_issue448_orphan_reaper;

#[cfg(test)]
#[path = "../tests-rs/test_startup_stale_port_tax.rs"]
mod tests_startup_stale_port_tax;

#[cfg(test)]
#[path = "../tests-rs/test_l_socket_tmux_precedence.rs"]
mod tests_l_socket_tmux_precedence;

#[cfg(test)]
#[path = "../tests-rs/test_issue509_namespace_instance.rs"]
mod tests_issue509_namespace_instance;

#[cfg(test)]
#[path = "../tests-rs/test_issue510_reaper_attribution.rs"]
mod tests_issue510_reaper_attribution;

#[cfg(test)]
#[path = "../tests-rs/test_issue530_registry_pruning.rs"]
mod tests_issue530_registry_pruning;

#[cfg(test)]
#[path = "../tests-rs/test_issue603_bare_routing.rs"]
mod tests_issue603_bare_routing;

#[cfg(test)]
#[path = "../tests-rs/test_picker_namespace_filter.rs"]
mod tests_picker_namespace_filter;

#[cfg(test)]
#[path = "../tests-rs/test_issue650_cross_session_process_name.rs"]
mod tests_issue650_cross_session_process_name;

#[cfg(test)]
#[path = "../tests-rs/test_issue698_counter_lock_missing_dir.rs"]
mod tests_issue698_counter_lock_missing_dir;