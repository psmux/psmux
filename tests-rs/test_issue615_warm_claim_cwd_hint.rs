// Issue #615 follow up: `split-window -c <dir>` intermittently answered
// `#{pane_current_path}` with the psmux SERVER's own working directory.
//
// MEASURED (two full sweeps, identical product code; the second one under
// load).  Both ran the same line of tests/test_issue615_wsl_pane_path.ps1:
//
//   PASS  pwsh split-window -c 'C:\Users' -> new pane_current_path='C:\Users'
//                                            real cwd='C:\Users'
//   FAIL  pwsh split-window -c 'C:\Users' -> new pane_current_path=
//             'C:\Users\godwin\Documents\workspace\psmux'  real cwd='C:\Users'
//
// The child always landed in the right directory.  Only the REPORTED one was
// wrong, and the wrong value was the directory the server was started from.
//
// REPRODUCED to 32/32 with four back-to-back `split-window -c C:\Users` per
// session (the pool then hands out spares that are still booting), and traced
// inside the format path:
//
//   pane=2 child_pid=Some(47444) announced=None peb_fg=Some("C:\\")
//          peb_own=Some("C:\\") server_cwd=C:\        <- reported C:\
//   pane=2 child_pid=Some(47444) announced=None peb_fg=Some("C:\\Users")
//          peb_own=Some("C:\\Users") server_cwd=C:\   <- same pane, 3s later
//
// ROOT CAUSE.  A pane claimed from the warm pool is an ALREADY RUNNING shell,
// spawned by the pool in the server's own working directory.  A running
// process's cwd cannot be set from outside, so `-c <dir>` is honoured by typing
// a `cd` into it (`pane::silent_rehome`).  That line runs instantly on an idle
// box and seconds later when the machine is loaded and the spare is still
// coming up.  Until it does, the pane's PEB reading is the POOL's directory,
// and `#{pane_current_path}` reported it verbatim.
//
// TMUX PARITY.  tmux stores the requested directory on the pane (`wp->cwd`,
// spawn.c) and the child is `chdir`ed into it before `exec`, so
// `format_cb_current_path` (format.c:957) is right from the instant the pane
// exists; where the reading fails it returns NULL, never the server's
// directory.  psmux cannot chdir a running spare, so it does the other half of
// what tmux does: it remembers the directory it was asked for.
//
// FIX.  `Pane::cwd_hint` carries the requested directory plus the reading taken
// the instant before the `cd` was written.  `#{pane_current_path}` reports the
// request for exactly as long as the process has not moved off that reading,
// and latches onto the live reading the moment it does, so a pane that never
// went through a rehome behaves bit for bit as before, a user's own `cd` is
// tracked immediately, and a later `cd` back into the pool's directory reports
// the pool's directory rather than resurrecting the hint.
//
// Registered from src/pane.rs so it can reach `pub(crate)` spawn internals.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ROWS: u16 = 10;
const COLS: u16 = 60;

// ── A pane with no real ConPTY behind it ────────────────────────────────────

#[derive(Debug)]
struct DummyWriter;
impl crate::pane::PaneInputSink for DummyWriter {}

impl std::io::Write for DummyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { Ok(buf.len()) }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[derive(Debug)]
struct DummyChild;
impl portable_pty::Child for DummyChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> { Ok(None) }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        Ok(portable_pty::ExitStatus::with_exit_code(0))
    }
    fn process_id(&self) -> Option<u32> { None }
    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> { None }
}
impl portable_pty::ChildKiller for DummyChild {
    fn kill(&mut self) -> std::io::Result<()> { Ok(()) }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(DummyChild)
    }
}

struct DummyMaster;
impl portable_pty::MasterPty for DummyMaster {
    fn resize(&self, _size: portable_pty::PtySize) -> Result<(), anyhow::Error> { Ok(()) }
    fn get_size(&self) -> Result<portable_pty::PtySize, anyhow::Error> {
        Ok(portable_pty::PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 })
    }
    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
        Ok(Box::new(std::io::empty()))
    }
    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
        Ok(Box::new(DummyWriter))
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> { None }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> { None }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> { None }
}

fn make_pane(child_pid: Option<u32>) -> crate::types::Pane {
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: Box::new(DummyMaster),
        writer: Box::new(DummyWriter),
        child: Box::new(DummyChild),
        term: Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0))),
        last_rows: ROWS,
        last_cols: COLS,
        id: 0,
        title: "pane".to_string(),
        title_locked: false,
        child_pid,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false, input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn app_with(pane: crate::types::Pane) -> crate::types::AppState {
    let mut app = crate::types::AppState::new("i615".to_string());
    let mut win = crate::types::Window {
        root: crate::types::Node::Leaf(pane),
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, COLS, ROWS),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![0],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    };
    win.active_path = vec![];
    app.windows.push(win);
    app.active_idx = 0;
    app
}

fn current_path(app: &crate::types::AppState) -> String {
    crate::format::expand_format("#{pane_current_path}", app)
}

/// A real, live process parked in `dir`, so the PEB reading the format path
/// takes is a genuine one rather than a stub. Killed by `Drop`.
#[cfg(windows)]
struct Parked(std::process::Child);
#[cfg(windows)]
impl Drop for Parked {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[cfg(windows)]
/// Wait until `pid`'s own reading of its working directory agrees with `dir`.
///
/// The PEB walk behind that reading is racy for a process that has just been
/// created: the header of this file measured the same effect on a booting warm
/// spare (`peb_own=Some("C:\\")` on one sweep, `Some("C:\\Users")` three
/// seconds later).  On a loaded CI runner `ping.exe` can still report the
/// directory it was created in for a moment after `current_dir` was applied,
/// which makes any test that asserts what happens "once the reading has moved"
/// fail while nothing is wrong.  Wait for the move, and keep a deadline so a
/// reading that never arrives still fails loudly instead of hanging.
fn wait_for_cwd_reading(pid: u32, dir: &std::path::Path) {
    let want = dir.to_string_lossy().into_owned();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let live = crate::platform::process_info::get_process_cwd(pid);
        if live
            .as_deref()
            .map_or(false, |cwd| crate::util::same_dir(cwd, &want))
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the parked process never reported {want:?} (last reading {live:?})"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(windows)]
fn park_process_in(dir: &std::path::Path) -> Parked {
    let child = std::process::Command::new("ping.exe")
        .args(["-n", "60", "127.0.0.1"])
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn parked process");
    wait_for_cwd_reading(child.id(), dir);
    Parked(child)
}

fn hint(pid: Option<u32>, requested: &str, stale: Option<&str>) -> crate::types::CwdHint {
    crate::types::CwdHint {
        pid,
        requested: requested.to_string(),
        stale: stale.map(|s| s.to_string()),
        settled: AtomicBool::new(false),
    }
}

// ── PART 1: the reported defect ─────────────────────────────────────────────

/// The sweep failure, in one assertion: the pane's process is still sitting in
/// the directory the pool spawned it in, and `#{pane_current_path}` must answer
/// with the directory the split ASKED for, never with the server's own.
#[test]
#[cfg(windows)]
fn warm_claimed_pane_reports_the_requested_dir_not_the_pool_dir() {
    let pool_dir = std::env::current_dir().expect("cwd");
    let pool = pool_dir.to_string_lossy().into_owned();
    let requested = std::env::temp_dir().to_string_lossy().trim_end_matches('\\').to_string();
    assert!(!crate::util::same_dir(&pool, &requested), "the two dirs must differ for this test to mean anything");

    let parked = park_process_in(&pool_dir);
    let pid = parked.0.id();
    let mut pane = make_pane(Some(pid));
    pane.cwd_hint = Some(hint(Some(pid), &requested, Some(&pool)));
    let app = app_with(pane);

    let got = current_path(&app);
    assert!(
        crate::util::same_dir(&got, &requested),
        "reported {got:?}, expected the requested {requested:?} (the pool/server dir is {pool:?})"
    );
    assert!(
        !crate::util::same_dir(&got, &pool),
        "reported the server's own working directory {pool:?}, which is the #615 sweep failure"
    );
}

/// The other half of the failure mode: nothing readable behind the pane at all.
/// tmux answers NULL there; psmux used to answer with the server's directory,
/// which is the one value that is guaranteed to be wrong.
#[test]
fn unreadable_pane_reports_the_requested_dir_not_the_server_cwd() {
    let server = std::env::current_dir().expect("cwd").to_string_lossy().into_owned();
    let requested = r"C:\Users";
    let mut pane = make_pane(None);
    pane.cwd_hint = Some(hint(None, requested, Some(&server)));
    let app = app_with(pane);

    assert_eq!(current_path(&app), requested);
}

// ── PART 2: no regression for panes that never went through a rehome ────────

/// A pane with no hint is exactly what it was before: the live reading, and the
/// server-cwd fallback only when there is nothing else at all.
#[test]
#[cfg(windows)]
fn a_pane_without_a_hint_still_reports_the_live_reading() {
    let dir = std::env::temp_dir();
    let parked = park_process_in(&dir);
    let pane = make_pane(Some(parked.0.id()));
    let app = app_with(pane);

    let got = current_path(&app);
    let want = dir.to_string_lossy().into_owned();
    assert!(
        crate::util::same_dir(&got, &want),
        "reported {got:?}, expected the process's own directory {want:?}"
    );
}

/// The user typed their own `cd` before the injected one landed: the reading has
/// moved off the pool directory, so it is live from that moment on.
#[test]
#[cfg(windows)]
fn a_reading_that_moved_wins_over_the_request() {
    let dir = std::env::temp_dir();
    let parked = park_process_in(&dir);
    let pid = parked.0.id();
    let mut pane = make_pane(Some(pid));
    // The hint claims the pane should still be sitting in C:\ ; the process is
    // demonstrably somewhere else.
    pane.cwd_hint = Some(hint(Some(pid), r"C:\Users", Some(r"C:\")));
    let app = app_with(pane);

    let got = current_path(&app);
    assert!(
        crate::util::same_dir(&got, &dir.to_string_lossy()),
        "reported {got:?}, expected the live reading {dir:?}"
    );
}

/// Once the pane has been seen to move, the hint is retired for good: a later
/// `cd` back into the pool's directory reports the pool's directory.
#[test]
#[cfg(windows)]
fn a_settled_hint_never_comes_back() {
    let pool_dir = std::env::temp_dir();
    let parked = park_process_in(&pool_dir);
    let pid = parked.0.id();
    let mut pane = make_pane(Some(pid));
    // stale MATCHES where the process is, so only the latch can keep the
    // request from being reported.
    let h = hint(Some(pid), r"C:\Users", Some(&pool_dir.to_string_lossy()));
    h.settled.store(true, Ordering::Relaxed);
    pane.cwd_hint = Some(h);
    let app = app_with(pane);

    let got = current_path(&app);
    assert!(
        crate::util::same_dir(&got, &pool_dir.to_string_lossy()),
        "reported {got:?}, expected the live reading {pool_dir:?}; a settled hint must not be consulted"
    );
}

/// A hint belongs to the process it was taken for. `respawn-pane` and friends
/// put a different child behind the pane; the hint must not follow.
#[test]
#[cfg(windows)]
fn a_hint_from_a_previous_child_is_ignored() {
    let dir = std::env::temp_dir();
    let parked = park_process_in(&dir);
    let pid = parked.0.id();
    let mut pane = make_pane(Some(pid));
    pane.cwd_hint = Some(hint(Some(pid.wrapping_add(1)), r"C:\Users", Some(&dir.to_string_lossy())));
    let app = app_with(pane);

    let got = current_path(&app);
    assert!(
        crate::util::same_dir(&got, &dir.to_string_lossy()),
        "reported {got:?} from another child's hint, expected the live reading {dir:?}"
    );
}

// ── PART 3: the producer side ───────────────────────────────────────────────

/// `silent_rehome` is the only place a psmux pane is asked to move itself, and
/// it is where the hint has to be recorded: the requested directory, plus the
/// reading that is about to become stale.
#[test]
#[cfg(windows)]
fn silent_rehome_records_the_request_and_the_pre_cd_reading() {
    let pool_dir = std::env::current_dir().expect("cwd");
    let parked = park_process_in(&pool_dir);
    let pid = parked.0.id();
    let mut pane = make_pane(Some(pid));

    crate::pane::silent_rehome(&mut pane, r"C:\Users\", crate::pane::RehomeSyntax::PowerShell);

    let h = pane.cwd_hint.as_ref().expect("silent_rehome must record a cwd hint");
    assert_eq!(h.pid, Some(pid));
    // Trailing separator dropped: the value has to compare and print like a
    // reading taken out of a live process.
    assert_eq!(h.requested, r"C:\Users");
    let stale = h.stale.as_deref().expect("the pre-cd reading");
    assert!(
        crate::util::same_dir(stale, &pool_dir.to_string_lossy()),
        "recorded stale reading {stale:?}, expected the process's directory {pool_dir:?}"
    );
    assert!(!h.settled.load(Ordering::Relaxed));
}

/// A rehome with no readable process still records the server's directory as
/// the stale one, because that is where the pool spawns its spares, so the
/// guarantee "never answer with the server's own directory" holds even when the
/// reading cannot be taken at claim time.
#[test]
fn silent_rehome_falls_back_to_the_server_dir_as_the_stale_reading() {
    let server = std::env::current_dir().expect("cwd").to_string_lossy().into_owned();
    let mut pane = make_pane(None);

    crate::pane::silent_rehome(&mut pane, r"C:\Users", crate::pane::RehomeSyntax::PowerShell);

    let h = pane.cwd_hint.as_ref().expect("cwd hint");
    assert_eq!(h.pid, None);
    assert_eq!(h.requested, r"C:\Users");
    assert!(crate::util::same_dir(h.stale.as_deref().expect("stale"), &server));
}

// ── PART 4: the directory comparison the whole thing rests on ───────────────

#[test]
fn normalize_dir_keeps_a_drive_root_and_drops_other_trailing_separators() {
    assert_eq!(crate::util::normalize_dir_for_display(r"C:\Users\"), r"C:\Users");
    assert_eq!(crate::util::normalize_dir_for_display(r"C:\Users\\"), r"C:\Users");
    assert_eq!(crate::util::normalize_dir_for_display(r"C:\"), r"C:\");
    assert_eq!(crate::util::normalize_dir_for_display("C:/Users/x"), r"C:\Users\x");
    // Case is the user's, never rewritten.
    assert_eq!(crate::util::normalize_dir_for_display(r"c:\USERS"), r"c:\USERS");
}

#[test]
fn same_dir_ignores_separator_case_and_trailing_slash_on_windows() {
    assert!(crate::util::same_dir(r"C:\Users", r"c:\users\"));
    assert!(crate::util::same_dir(r"C:\Users", "C:/Users"));
    assert!(crate::util::same_dir(r"C:\", r"C:\"));
    assert!(!crate::util::same_dir(r"C:\Users", r"C:\Users\godwin"));
    assert!(!crate::util::same_dir(r"C:\Users", r"C:\Windows"));
}
