// Issue #474: home/data-dir resolution must not follow a shell-rewritten HOME.
//
// MSYS2 login shells unset USERPROFILE and export HOME=/home/<user>. The old
// home_dir() fell straight back to HOME, resolving the data dir to a path
// where no registry lives; the startup orphan reaper then saw every live
// psmux server as untracked and terminated them all (any `psmux <cmd>` run
// from an MSYS2 shell killed every session on the machine).
//
// The fix orders resolution: USERPROFILE, then the Windows profile API, then
// HOMEDRIVE+HOMEPATH, then HOME as the last resort. These tests pin that
// order. Env-mutating tests serialise through crate::util::lock_test_env().

use super::*;

struct EnvSnapshot {
    vars: Vec<(&'static str, Option<String>)>,
}

impl EnvSnapshot {
    fn take() -> Self {
        // PSMUX_DATA_DIR too: psmux_dir_follows_resolved_home clears it, and the
        // snapshot is what puts it back.
        let names = ["USERPROFILE", "HOME", "HOMEDRIVE", "HOMEPATH", "PSMUX_DATA_DIR"];
        Self {
            vars: names.iter().map(|n| (*n, std::env::var(n).ok())).collect(),
        }
    }
}

impl Drop for EnvSnapshot {
    fn drop(&mut self) {
        for (name, val) in &self.vars {
            match val {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
    }
}

#[test]
fn userprofile_wins_when_set() {
    let _lock = crate::util::lock_test_env();
    let _snap = EnvSnapshot::take();
    std::env::set_var("USERPROFILE", "C:\\up_test_home");
    std::env::set_var("HOME", "/home/somebody");
    assert_eq!(home_dir(), "C:\\up_test_home");
}

#[test]
fn msys2_style_home_does_not_hijack_resolution() {
    // The exact issue #474 environment: USERPROFILE unset, HOME a POSIX path.
    // Resolution must land on the real profile dir (via the Windows profile
    // API), NOT on the meaningless "/home/user" string.
    let _lock = crate::util::lock_test_env();
    let _snap = EnvSnapshot::take();
    std::env::remove_var("USERPROFILE");
    std::env::remove_var("HOMEDRIVE");
    std::env::remove_var("HOMEPATH");
    std::env::set_var("HOME", "/home/somebody");
    let home = home_dir();
    assert_ne!(home, "/home/somebody", "HOME must not win over the profile API");
    assert!(
        std::path::Path::new(&home).is_dir(),
        "resolved home must be a real directory, got {home:?}"
    );
}

#[test]
fn empty_userprofile_treated_as_unset() {
    let _lock = crate::util::lock_test_env();
    let _snap = EnvSnapshot::take();
    std::env::set_var("USERPROFILE", "");
    std::env::set_var("HOME", "/home/somebody");
    let home = home_dir();
    assert_ne!(home, "", "empty USERPROFILE must fall through");
    assert_ne!(home, "/home/somebody");
}

#[test]
fn psmux_dir_follows_resolved_home() {
    let _lock = crate::util::lock_test_env();
    let _snap = EnvSnapshot::take();
    // What the data directory resolves to when nothing overrides it, so the
    // override is cleared for the duration: the suite is meant to be run with
    // PSMUX_DATA_DIR pointed at a scratch directory, and this test failed there.
    std::env::remove_var("PSMUX_DATA_DIR");
    std::env::set_var("USERPROFILE", "C:\\up_test_home");
    assert_eq!(psmux_dir(), "C:\\up_test_home\\.psmux");
}
