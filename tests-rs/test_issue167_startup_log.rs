// Issue #167 — server-startup error log helper.
//
// `run_server` writes a one-shot diagnostic file to
// `~/.psmux/server-startup.log` whenever the initial pane spawn fails.
// The detached server has no visible stderr, so without this file the
// user sees only "psmux flashed black and returned to prompt".  These
// tests pin the helper's output format so the workaround instructions
// it embeds (PSMUX_NO_PASSTHROUGH, PSMUX_BARE_ENV, local-account check,
// link to issue #167) cannot accidentally drop out of the file.

use super::*;

// All tests in this module touch the same on-disk log file and the
// process-global USERPROFILE/HOME env vars.  cargo runs tests in parallel by
// default, so without serialisation `home_missing` wipes the env vars while
// other tests are mid-write.  Serialise through crate::util::lock_test_env(),
// the SHARED process-wide lock, so we also mutually exclude with other modules
// that mutate USERPROFILE/HOME (e.g. test_config_plugin_paths) - a per-module
// mutex here left that cross-module race open.

/// The log the helper under test actually writes to.
///
/// This used to build `<USERPROFILE>\.psmux\server-startup.log` by hand, which
/// is where the helper writes only when `PSMUX_DATA_DIR` is unset. Running the
/// suite with the data directory pointed somewhere else, which is how it should
/// be run so it cannot reach a developer's live sessions, had the helper write
/// to the override and these tests read the home path, so all five failed on a
/// file that was never going to be there. Ask `paths` the same question the
/// helper asks.
fn log_path() -> std::path::PathBuf {
    std::path::PathBuf::from(crate::paths::psmux_dir()).join("server-startup.log")
}

fn cleanup() {
    let _ = std::fs::remove_file(log_path());
}

/// Points `PSMUX_DATA_DIR` at a directory of this test's own for as long as it
/// lives, and puts the previous value back on drop, including when the test
/// panics. Without it these tests wrote to, and `cleanup` then DELETED, the
/// real `~/.psmux/server-startup.log` of whoever ran `cargo test` without an
/// override. Hold `lock_test_env` for the guard's whole life.
struct IsolatedDataDir {
    saved: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
}

impl IsolatedDataDir {
    fn new(tag: &str) -> Self {
        let saved = std::env::var_os("PSMUX_DATA_DIR");
        let dir = std::env::temp_dir().join(format!("psmux_t167_{}_{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        std::env::set_var("PSMUX_DATA_DIR", &dir);
        IsolatedDataDir { saved, dir }
    }
}

impl Drop for IsolatedDataDir {
    fn drop(&mut self) {
        match self.saved.take() {
            Some(v) => std::env::set_var("PSMUX_DATA_DIR", v),
            None => std::env::remove_var("PSMUX_DATA_DIR"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn writes_a_log_file_with_the_error_message() {
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("w");
    cleanup();
    write_startup_error_log(&"CreateProcessW \"pwsh.exe\" failed: Falscher Parameter. (os error 87)", "t167");
    let body = std::fs::read_to_string(log_path()).expect("log file must exist after call");
    cleanup();

    assert!(body.contains("os error 87"),
        "log must include the verbatim OS error so users can grep it: {}", body);
    assert!(body.contains("CreateProcessW"),
        "log must include the failing API name: {}", body);
}

#[test]
fn log_includes_environment_diagnostics() {
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("env");
    cleanup();
    write_startup_error_log(&"any error", "t167");
    let body = std::fs::read_to_string(log_path()).unwrap();
    cleanup();

    // These three diagnostics are what the issue-167 conversation kept
    // asking for.  Future maintainers should NOT remove them without
    // also updating the response template.
    assert!(body.contains("env vars (count)"),
        "must report env var count: {}", body);
    assert!(body.contains("env block size (wch)"),
        "must report env block size in wide chars: {}", body);
    assert!(body.contains("Windows hard limit: 32767"),
        "must reference the Windows limit so users can compare: {}", body);
}

#[test]
fn log_includes_workaround_instructions() {
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("wk");
    cleanup();
    write_startup_error_log(&"any", "t167");
    let body = std::fs::read_to_string(log_path()).unwrap();
    cleanup();

    assert!(body.contains("PSMUX_NO_PASSTHROUGH"),
        "must surface the no-passthrough workaround: {}", body);
    assert!(body.contains("PSMUX_BARE_ENV"),
        "must surface the bare-env workaround: {}", body);
    assert!(body.contains("local Windows account") || body.contains("Microsoft account"),
        "must mention the MSA-vs-local workaround that worked for sungamma: {}", body);
    assert!(body.contains("issues/167"),
        "must link back to the tracking issue: {}", body);
}

#[test]
fn log_includes_psmux_version() {
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("ver");
    cleanup();
    write_startup_error_log(&"err", "t167");
    let body = std::fs::read_to_string(log_path()).unwrap();
    cleanup();

    let version = env!("CARGO_PKG_VERSION");
    assert!(body.contains(version),
        "must include the psmux version producing the log; expected '{}': {}",
        version, body);
}

#[test]
fn every_failure_is_kept_and_the_newest_is_the_one_read_back() {
    // This used to assert the second call OVERWROTE the first. One name serves
    // every server in the data directory, so truncating meant two servers
    // failing to start left one report between them. Both are kept now, each
    // under a header naming the session that wrote it, and the reader takes the
    // last one.
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("keep");
    cleanup();
    write_startup_error_log(&"old error message", "nsA__alpha");
    write_startup_error_log(&"NEW_MARKER_xyz_789", "nsB__beta");
    let body = std::fs::read_to_string(log_path()).unwrap();
    let read_back = super::read_fresh_startup_error(0, "nsB__beta");
    cleanup();

    assert!(body.contains("NEW_MARKER_xyz_789"), "the latest failure must be in the file");
    assert!(body.contains("old error message"), "an earlier server's report must survive");
    assert!(body.contains("=== session nsA__alpha pid"), "each report names its session");
    assert!(body.contains("=== session nsB__beta pid"), "each report names its session");
    let (err, _) = read_back.expect("the reader must find a report");
    assert!(err.contains("NEW_MARKER_xyz_789"),
        "the reader must surface the LAST report, got: {err}");
}

#[test]
fn each_client_reads_its_own_servers_report_not_the_last_one() {
    // Two servers failing at nearly the same moment append in either order, so
    // "the last report" is the other server's about half the time. Measured
    // end to end before this: 20 of 40 clients printed the other server's
    // error. The client knows the base of the server it started, and that is
    // what the header names.
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("own");
    write_startup_error_log(&"ERROR_OF_ALPHA_111", "nsA__alpha");
    write_startup_error_log(&"ERROR_OF_BETA_222", "nsB__beta");
    write_startup_error_log(&"ERROR_OF_ALPHA_333", "nsA__alpha");
    write_startup_error_log(&"ERROR_OF_GAMMA_444", "nsA__alphabet");

    let (a, _) = super::read_fresh_startup_error(0, "nsA__alpha").expect("alpha has a report");
    assert!(a.contains("ERROR_OF_ALPHA_333"), "alpha's NEWEST report, got: {a}");
    assert!(!a.contains("GAMMA"), "a base that merely starts with alpha's is not alpha: {a}");
    let (b, _) = super::read_fresh_startup_error(0, "nsB__beta").expect("beta has a report");
    assert!(b.contains("ERROR_OF_BETA_222") && !b.contains("ALPHA"),
        "beta reads its own report and nothing after it, got: {b}");
    assert!(super::read_fresh_startup_error(0, "nsC__never").is_none(),
        "a server that wrote nothing must not be handed another server's error");
}

#[test]
fn a_report_log_rotates_at_its_cap_and_the_new_report_survives() {
    // Always on and appended, so without a cap a machine that keeps failing to
    // start would grow the file for ever.
    let _g = crate::util::lock_test_env();
    let _dd = IsolatedDataDir::new("rot");
    let path = log_path();
    let p = path.to_str().unwrap();
    let rotated = format!("{}.1", p);
    let filler = "x".repeat(300);
    for i in 0..10 {
        super::append_report_at(p, "nsR__fill", &format!("filler {i} {filler}"), 2048);
    }
    let size = std::fs::metadata(p).unwrap().len();
    assert!(size < 2048 + 512, "the live file stays near its cap, got {size} bytes");
    assert!(std::path::Path::new(&rotated).exists(), "the older reports move to .1");
    super::append_report_at(p, "nsR__last", "when (epoch s): 4000000000\nerror:\n  LAST_ONE\n", 2048);
    let (err, _) = super::read_fresh_startup_error_for_at(p, 0, Some("nsR__last"))
        .expect("the newest report is readable after a rotation");
    assert!(err.contains("LAST_ONE"), "got: {err}");
}

#[test]
fn log_call_does_not_panic_when_home_is_missing() {
    let _g = crate::util::lock_test_env();
    // With USERPROFILE and HOME gone the data directory still resolves through
    // the Win32 profile API, so without a directory of its own this test wrote
    // into the real ~/.psmux/server-startup.log.
    let _dd = IsolatedDataDir::new("nohome");
    // Simulate a degenerate environment where neither USERPROFILE nor HOME
    // is set.  The helper must NOT panic; it should swallow and return.
    let saved_up = std::env::var("USERPROFILE").ok();
    let saved_h  = std::env::var("HOME").ok();
    std::env::remove_var("USERPROFILE");
    std::env::remove_var("HOME");

    // Run inside catch_unwind so a panic surfaces as a test failure
    // instead of aborting the test binary.
    let res = std::panic::catch_unwind(|| {
        write_startup_error_log(&"err with no home", "t167");
    });

    if let Some(v) = saved_up { std::env::set_var("USERPROFILE", v); }
    if let Some(v) = saved_h  { std::env::set_var("HOME", v); }

    assert!(res.is_ok(), "helper must not panic when home env is unset");
}
