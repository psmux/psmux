//! Issue #774: session names are case-sensitive in tmux, but psmux located a
//! session by opening `<data dir>\<name>.port`, and the data directory is
//! case-insensitive on Windows. `-t =REPRO-CASE` therefore reached the server
//! of `repro-case`, and `kill-session -t =REPRO-CASE` killed it.
//!
//! The fix reads the spelling the directory entry was stored under and holds
//! the requested name to it. These tests exercise that registry layer against a
//! private data directory; no server is started.

use super::*;

struct DataDir {
    dir: std::path::PathBuf,
    saved: Option<std::ffi::OsString>,
    _env: std::sync::MutexGuard<'static, ()>,
}

impl DataDir {
    fn new(tag: &str) -> Self {
        let env = crate::util::lock_test_env();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir()
            .join(format!("psmux_issue774_{tag}_{}_{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let saved = std::env::var_os("PSMUX_DATA_DIR");
        std::env::set_var("PSMUX_DATA_DIR", &dir);
        DataDir { dir, saved, _env: env }
    }

    fn touch(&self, name: &str, body: &str) {
        std::fs::write(self.dir.join(name), body).unwrap();
    }

    fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();
        v.sort();
        v
    }

    /// Whether this directory folds case, which is what the bug needs.
    fn folds_case(&self) -> bool {
        self.touch("probe_case.tmp", "");
        let folds = self.dir.join("PROBE_CASE.TMP").exists();
        let _ = std::fs::remove_file(self.dir.join("probe_case.tmp"));
        folds
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
            None => std::env::remove_var("PSMUX_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn issue774_spelling_comparison_is_exact_for_the_session_name() {
    assert!(!session_spelling_differs("repro-case", "repro-case"));
    assert!(session_spelling_differs("REPRO-CASE", "repro-case"));
    assert!(session_spelling_differs("Work", "work"));
    // The -L namespace prefix is not part of the session name.
    assert!(!session_spelling_differs("NS__repro-case", "ns__repro-case"));
    assert!(session_spelling_differs("ns__REPRO-CASE", "ns__repro-case"));
}

#[test]
fn issue774_stored_file_name_reports_the_spelling_on_disk() {
    let d = DataDir::new("stored");
    d.touch("repro-case.port", "1");
    let lower = d.dir.join("repro-case.port");
    assert_eq!(stored_file_name(lower.to_str().unwrap()).as_deref(), Some("repro-case.port"));
    if d.folds_case() {
        let upper = d.dir.join("REPRO-CASE.port");
        assert_eq!(
            stored_file_name(upper.to_str().unwrap()).as_deref(),
            Some("repro-case.port"),
            "a lookup under another spelling must report the stored one"
        );
    }
    assert_eq!(stored_file_name(d.dir.join("missing.port").to_str().unwrap()), None);
    assert_eq!(stored_file_name(d.dir.join("*.port").to_str().unwrap()), None);
}

#[test]
fn issue774_a_case_variant_target_is_not_the_stored_session() {
    let d = DataDir::new("variant");
    d.touch("repro-case.port", "1");
    d.touch("ns__Work.port", "1");
    assert_eq!(registry_case_variant("repro-case"), None, "exact spelling is that session");
    assert_eq!(registry_case_variant("absent"), None, "no entry is not a variant");
    assert_eq!(registry_case_variant("ns__Work"), None);
    if d.folds_case() {
        assert_eq!(registry_case_variant("REPRO-CASE").as_deref(), Some("repro-case"));
        assert_eq!(registry_case_variant("Repro-Case").as_deref(), Some("repro-case"));
        assert_eq!(registry_case_variant("ns__work").as_deref(), Some("ns__Work"));
        assert_eq!(registry_case_variant("NS__Work"), None, "namespace part is not compared");
    }
}

#[test]
fn issue774_heal_gives_registry_entries_the_owner_spelling() {
    let d = DataDir::new("heal");
    if !d.folds_case() {
        return;
    }
    d.touch("STALE.port", "1");
    d.touch("STALE.key", "k");
    d.touch("STALE.sid", "3");
    heal_registry_spelling("stale");
    assert_eq!(d.names(), vec!["stale.key", "stale.port", "stale.sid"]);
    assert_eq!(std::fs::read_to_string(d.dir.join("stale.key")).unwrap(), "k");
    assert_eq!(registry_case_variant("stale"), None);
}

#[test]
fn issue774_rename_blocker_refuses_a_live_session_in_the_slot() {
    let d = DataDir::new("rename");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let live_port = listener.local_addr().unwrap().port();
    d.touch("other.port", &live_port.to_string());
    d.touch("mine.port", "1");

    // Renaming to the exact name of a live session: refused (tmux
    // `duplicate session`).
    assert_eq!(rename_blocker("mine", "other").as_deref(), Some("other"));
    // Same name: tmux returns without doing anything, nothing blocks.
    assert_eq!(rename_blocker("mine", "mine"), None);
    // A free name is not blocked.
    assert_eq!(rename_blocker("mine", "free"), None);
    if d.folds_case() {
        // A case variant of a live session lands on its files: refused.
        assert_eq!(rename_blocker("mine", "OTHER").as_deref(), Some("other"));
        // A case variant of our own name lands on our own files: allowed.
        assert_eq!(rename_blocker("mine", "MINE"), None);
    }
    // Nobody listening behind the entry: stale, not a blocker.
    drop(listener);
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);
    d.touch("gone.port", &dead_port.to_string());
    assert_eq!(rename_blocker("mine", "gone"), None);
}
