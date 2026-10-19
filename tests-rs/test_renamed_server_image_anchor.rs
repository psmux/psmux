//! A server started from a copied or renamed binary (`pr856_master.exe`,
//! `psmux-dev.exe`) runs under that image name. The pid anchor used to accept
//! only `psmux`, `tmux` and `pmux`, so its own CLI read the live server as a
//! PID recycled by another program: `new-session -d` exited 0 and every later
//! command said "no server running". The `.pid` file records the process
//! creation time, which pins identity exactly, so an unknown image name whose
//! creation time matches is the server that wrote the file.

use super::*;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn temp_pid_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    p.push(format!("psmux_renamed_anchor_{}_{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&p);
    let pid = p.join(format!("ns__{tag}.pid"));
    std::fs::write(&pid, b"4300:1000").expect("write pid");
    pid
}

#[test]
#[cfg(windows)]
fn renamed_image_with_matching_creation_time_is_live() {
    let pid_path = temp_pid_path("renamed");
    let verdict = pid_anchor_verdict_for(
        &pid_path,
        4300,
        Some(1000),
        |_| Some("pr856_master".to_string()),
        |_| Some(1000),
    );
    assert_eq!(verdict, Some(true), "the process that wrote the .pid is our server whatever its name");
}

#[test]
#[cfg(windows)]
fn unknown_image_with_other_creation_time_is_recycled() {
    let pid_path = temp_pid_path("recycled");
    let verdict = pid_anchor_verdict_for(
        &pid_path,
        4300,
        Some(1000),
        |_| Some("notepad".to_string()),
        |_| Some(2000),
    );
    assert_eq!(verdict, Some(false), "a PID reused by another program must still read as dead");
}

#[test]
#[cfg(windows)]
fn unknown_image_without_recorded_creation_is_recycled() {
    let pid_path = temp_pid_path("legacy");
    let verdict = pid_anchor_verdict_for(&pid_path, 4300, None, |_| Some("notepad".to_string()), |_| Some(1000));
    assert_eq!(verdict, Some(false), "without a recorded creation time only the known names are trusted");
}

#[test]
#[cfg(windows)]
fn unknown_image_whose_creation_cannot_be_read_is_recycled() {
    let pid_path = temp_pid_path("unreadable");
    let verdict = pid_anchor_verdict_for(&pid_path, 4300, Some(1000), |_| Some("notepad".to_string()), |_| None);
    assert_eq!(verdict, Some(false));
}

#[test]
#[cfg(windows)]
fn own_image_stem_names_the_running_test_binary() {
    let stem = own_image_stem().expect("current exe has a stem");
    assert!(!stem.is_empty() && stem == stem.to_ascii_lowercase());
}
