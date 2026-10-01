// Root cause of the flaky `session_id_is_unique` test: `allocate_session_id`
// did a non-atomic read-modify-write on the `.psmux/next_session_id` counter
// file with no synchronization. Two concurrent callers (parallel test threads,
// or two sessions starting at once across processes) both read the same
// `current`, both return it, and both write `current + 1` -> duplicate ids.
//
// The fix serializes the read-modify-write with a process-global mutex plus a
// cross-process advisory lock file. This test hammers `allocate_session_id`
// from many threads at once and asserts every id is distinct, which reliably
// failed before the fix.

use super::*;

#[test]
fn allocate_session_id_is_unique_under_concurrency() {
    // allocate_session_id reads PSMUX_DATA_DIR on every call. Tests that point
    // it somewhere else (test_issue698_counter_lock_missing_dir does, at an
    // empty directory) hold lock_test_env while they do, so this test must hold
    // it too, or half its ids come from a fresh counter that restarts at 0.
    // Measured: from an empty data dir this failed 19 of 20 runs alongside the
    // #698 tests and 0 of 20 alone.
    let _env = crate::util::lock_test_env();
    // 16 x 32 allocations land in whatever data directory this process points
    // at, and on a developer's machine that is the live one: the counter their
    // own sessions number themselves from advanced by 512 every time the suite
    // ran. The lock is already held, so the directory can be moved for the
    // duration and put back, which is the shape test_issue698 uses.
    //
    // The restore is a Drop guard so it also runs when a worker thread panics
    // and `join().expect` unwinds out of this test.
    struct Restore(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
                None => std::env::remove_var("PSMUX_DATA_DIR"),
            }
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let dir = std::env::temp_dir().join(format!("psmux_sid_race_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::create_dir_all(&dir);
    let restore = Restore(std::env::var_os("PSMUX_DATA_DIR"), dir.clone());
    std::env::set_var("PSMUX_DATA_DIR", &dir);

    const THREADS: usize = 16;
    const PER_THREAD: usize = 32;

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let mut handles = Vec::new();
    for _ in 0..THREADS {
        let b = barrier.clone();
        handles.push(std::thread::spawn(move || {
            // Release all threads simultaneously to maximize the race window.
            b.wait();
            let mut ids = Vec::with_capacity(PER_THREAD);
            for _ in 0..PER_THREAD {
                ids.push(allocate_session_id());
            }
            ids
        }));
    }

    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().expect("thread panicked"));
    }

    // Put the data directory back before asserting, so a failure here cannot
    // leak the override into whatever test runs next.
    drop(restore);

    let total = all.len();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        total,
        "allocate_session_id handed out duplicate ids under concurrency: {} unique of {} allocated",
        sorted.len(),
        total
    );
}

/// Child half of the cross process test below. It does nothing unless the
/// parent launched it with PSMUX_SID_WORKER set, so running the suite with
/// --ignored cannot trip it by accident.
#[test]
#[ignore]
fn session_id_alloc_worker() {
    let Some(out) = std::env::var_os("PSMUX_SID_WORKER_OUT") else { return };
    let n: usize = std::env::var("PSMUX_SID_WORKER_N").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    let go = std::path::PathBuf::from(std::env::var_os("PSMUX_SID_WORKER_GO").expect("go file"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !go.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let mut ids = String::new();
    for _ in 0..n {
        ids.push_str(&format!("{}
", allocate_session_id()));
    }
    std::fs::write(out, ids).expect("worker output");
}

/// Several PROCESSES allocating from one data directory must never share an id.
///
/// Inside one process SESSION_ID_ALLOC serialises everything, so only a cross
/// process test reaches the advisory lock file. On Windows a create_new on a lock
/// file another process is deleting fails with PermissionDenied (delete
/// pending), measured at about 1 in 18 contended attempts with 8 processes. The
/// acquire loop must treat that as contention and wait; proceeding instead lets
/// two processes read the same counter, and the proceeding caller's Drop then
/// deletes the lock file the real holder owns.
#[test]
fn allocate_session_id_is_unique_across_processes() {
    let _env = crate::util::lock_test_env();
    const PROCS: usize = 8;
    const PER_PROC: usize = 150;
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("psmux_sid_xproc_{}_{}", std::process::id(), unique));
    std::fs::create_dir_all(&dir).expect("data dir");
    let go = dir.join("go");
    let exe = std::env::current_exe().expect("test exe");
    let mut children = Vec::new();
    for i in 0..PROCS {
        let child = std::process::Command::new(&exe)
            .args(["--exact", "session::tests_session_id_alloc_race::session_id_alloc_worker", "--ignored", "--test-threads=1", "--quiet"])
            .env("PSMUX_DATA_DIR", &dir)
            .env("PSMUX_SID_WORKER_N", PER_PROC.to_string())
            .env("PSMUX_SID_WORKER_GO", &go)
            .env("PSMUX_SID_WORKER_OUT", dir.join(format!("ids{}.txt", i)))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn worker");
        children.push(child);
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(&go, "go").expect("go file");
    for mut c in children {
        let st = c.wait().expect("worker wait");
        assert!(st.success(), "a worker process failed: {:?}", st);
    }
    let mut all = Vec::new();
    for i in 0..PROCS {
        let text = std::fs::read_to_string(dir.join(format!("ids{}.txt", i))).expect("worker ids");
        all.extend(text.lines().filter_map(|l| l.trim().parse::<usize>().ok()));
    }
    let _ = std::fs::remove_dir_all(&dir);
    let total = all.len();
    assert_eq!(total, PROCS * PER_PROC, "every worker must report all its ids");
    all.sort_unstable();
    all.dedup();
    assert_eq!(
        all.len(),
        total,
        "{} processes handed out duplicate session ids: {} unique of {}",
        PROCS,
        all.len(),
        total
    );
}
