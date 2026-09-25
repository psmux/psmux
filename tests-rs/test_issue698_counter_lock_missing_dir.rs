//! Issue #698: a missing data directory was read as lock contention.
//!
//! `AppState::new` allocates a session id, which takes `CounterLock` on
//! `<data dir>/next_session_id.lock`. Creating that file fails with `NotFound`
//! when the directory is not there yet, which is what a first run on a machine
//! looks like, and the acquire loop treated every failure as "someone else
//! holds it": 2000 sleeps of 1ms, then it gave up and proceeded. Measured on
//! Windows 11 that is 3.2s to 3.4s per allocation, on a `new-session` and on
//! every one of the suite's own `AppState::new` calls.
//!
//! Nothing reported it because the result was still correct. What was not
//! correct is the counter: `next_session_id` could not be written into a
//! directory that does not exist, so the id never advanced either.

use super::*;
use std::time::Instant;

/// A data directory path that does not exist and is not shared with any other
/// test in this process.
fn unused_data_dir(tag: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("psmux_issue698_{tag}_{}_{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn issue698_a_missing_data_dir_does_not_spend_the_lock_budget() {
    let _env = crate::util::lock_test_env();
    let saved = std::env::var_os("PSMUX_DATA_DIR");
    let dir = unused_data_dir("budget");
    assert!(!dir.exists(), "precondition: the data directory must not be there");
    std::env::set_var("PSMUX_DATA_DIR", &dir);

    let started = Instant::now();
    let first = allocate_session_id();
    let elapsed = started.elapsed();
    let second = allocate_session_id();

    // Restore the process-global override before asserting, so a failure here
    // cannot leak it into whatever test runs next.
    match saved {
        Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
        None => std::env::remove_var("PSMUX_DATA_DIR"),
    }
    let counter_written = dir.join("next_session_id").exists();
    let _ = std::fs::remove_dir_all(&dir);

    // The old loop could not finish in under 2000ms by construction, so a
    // second is a wide margin that still fails the moment it comes back.
    assert!(
        elapsed < Duration::from_secs(1),
        "allocating a session id with no data directory took {elapsed:?};          the 2000ms acquire budget is being spent on a NotFound again"
    );
    assert!(
        counter_written,
        "the acquire has to create the data directory, otherwise the counter          file it guards cannot be written at all"
    );
    assert_eq!(
        second,
        first + 1,
        "the counter must persist across allocations, which it cannot do while          its directory is missing"
    );
}

#[test]
fn issue698_a_lock_held_by_someone_else_is_still_waited_for() {
    let _env = crate::util::lock_test_env();
    let saved = std::env::var_os("PSMUX_DATA_DIR");
    let dir = unused_data_dir("held");
    std::fs::create_dir_all(&dir).expect("data dir");
    std::env::set_var("PSMUX_DATA_DIR", &dir);

    // A lock file with a current timestamp is a live holder, not a stale one.
    let lock = dir.join("next_session_id.lock");
    std::fs::write(&lock, "999999").expect("lock file");

    let started = Instant::now();
    let _id = allocate_session_id();
    let waited = started.elapsed();

    match saved {
        Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
        None => std::env::remove_var("PSMUX_DATA_DIR"),
    }
    let _ = std::fs::remove_dir_all(&dir);

    // AlreadyExists is the one failure waiting can resolve, so this path must
    // keep sleeping through its budget rather than return at once.
    assert!(
        waited >= Duration::from_secs(1),
        "a lock held by another process was not waited for; it returned in {waited:?}"
    );
}
