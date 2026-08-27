//! Issue #597 follow-up: the Win32 `MOUSE_EVENT` record bypass past the wheel
//! on Windows builds below `CONPTY_MOUSE_MIN_BUILD`.
//!
//! The reporter measured, on real Windows 10 19045, that a crossterm app in a
//! psmux pane receives every wheel notch (the record bypass works) while a node
//! child reading VT bytes receives nothing (conhost's inbound VT parser drops
//! the SGR report psmux writes into the pane's ConPTY input pipe).  The record
//! channel was wheel-only, so on that build a record-reading app had the wheel
//! and no clicks at all.
//!
//! These tests pin the routing decision table and the build seam that lets it
//! be exercised on a modern host.  They are pure: `record_bypass_applies` takes
//! the event flags and reads only the build number, so the whole table can be
//! asserted without a pane, a console or a child process.  The E2E half lives
//! in `tests/test_issue597_legacy_mouse_bypass.ps1`.

use super::*;

/// Restores the build seam on drop, so a failing assertion inside a closure
/// still cleans up instead of leaking `PSMUX_FAKE_WIN_BUILD` into every later
/// test in the process.
struct BuildRestore(Option<String>);

impl Drop for BuildRestore {
    fn drop(&mut self) {
        match self.0.as_deref() {
            Some(v) => std::env::set_var("PSMUX_FAKE_WIN_BUILD", v),
            None => std::env::remove_var("PSMUX_FAKE_WIN_BUILD"),
        }
    }
}

/// Pin the reported Windows build for the duration of the closure.  Uses the
/// shared env lock so this never races the other env-touching suites.
fn with_build<T>(build: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let _restore = BuildRestore(std::env::var("PSMUX_FAKE_WIN_BUILD").ok());
    match build {
        Some(v) => std::env::set_var("PSMUX_FAKE_WIN_BUILD", v),
        None => std::env::remove_var("PSMUX_FAKE_WIN_BUILD"),
    }
    f()
}

const WHEEL: u32 = mouse_inject::MOUSE_WHEELED;
const MOVED: u32 = mouse_inject::MOUSE_MOVED;
/// Press and release both arrive with no event flags at all; the button state
/// is what tells them apart, and this decision does not look at it.
const CLICK: u32 = 0;

// ---------------------------------------------------------------------------
// The build predicate
// ---------------------------------------------------------------------------

#[test]
fn win10_builds_need_the_record_bypass() {
    // 19041 and 19045 are the two conhost generations #457 measured, and 19045
    // is the reporter's box in #597.
    for build in ["19041", "19045"] {
        with_build(Some(build), || {
            assert!(
                crate::ssh_input::conpty_needs_mouse_record_bypass(),
                "build {build} is below CONPTY_MOUSE_MIN_BUILD and must take the record bypass"
            );
        });
    }
}

#[test]
fn server_2022_needs_the_record_bypass() {
    // 20348 is Windows Server 2022, the #573 host.  It sits below the threshold
    // too, so its panes are on the same dead pipe channel.
    with_build(Some("20348"), || {
        assert!(crate::ssh_input::conpty_needs_mouse_record_bypass());
    });
}

#[test]
fn the_threshold_itself_does_not_need_the_bypass() {
    // Strictly below, not at or below.  CONPTY_MOUSE_MIN_BUILD is the first
    // build whose conhost handles the inbound report, so it keeps the pipe.
    let at = crate::ssh_input::CONPTY_MOUSE_MIN_BUILD.to_string();
    with_build(Some(&at), || {
        assert!(
            !crate::ssh_input::conpty_needs_mouse_record_bypass(),
            "CONPTY_MOUSE_MIN_BUILD ({at}) is the first GOOD build, not the last bad one"
        );
    });
    with_build(Some(&(crate::ssh_input::CONPTY_MOUSE_MIN_BUILD - 1).to_string()), || {
        assert!(crate::ssh_input::conpty_needs_mouse_record_bypass());
    });
}

#[test]
fn modern_builds_keep_the_pipe_only_routing() {
    for build in ["22631", "26100", "26200"] {
        with_build(Some(build), || {
            assert!(
                !crate::ssh_input::conpty_needs_mouse_record_bypass(),
                "build {build} must keep today's routing untouched"
            );
        });
    }
}

#[test]
fn an_unparseable_override_falls_back_to_the_real_build() {
    // Garbage in the seam must not be read as "build 0", which would silently
    // switch every host onto the bypass.  `windows_build_number` ignores a value
    // it cannot parse, so the answer is whatever this host really is.
    with_build(Some("not-a-number"), || {
        let real = crate::ssh_input::windows_build_number();
        let expected = real.map_or(false, |b| b < crate::ssh_input::CONPTY_MOUSE_MIN_BUILD);
        assert_eq!(crate::ssh_input::conpty_needs_mouse_record_bypass(), expected);
    });
}

#[test]
fn the_override_is_the_only_thing_that_moves_the_answer() {
    // Without the seam the predicate must agree with the host's real build, so
    // the diagnostic override cannot be mistaken for the mechanism itself.
    with_build(None, || {
        let real = crate::ssh_input::windows_build_number();
        let expected = real.map_or(false, |b| b < crate::ssh_input::CONPTY_MOUSE_MIN_BUILD);
        assert_eq!(crate::ssh_input::conpty_needs_mouse_record_bypass(), expected);
    });
}

#[test]
fn force_mouse_does_not_move_the_record_bypass() {
    // PSMUX_FORCE_MOUSE is about the CLIENT-to-terminal direction over SSH.
    // This predicate is about the pane's conhost in the opposite direction.
    // Wiring them together would turn a user's `PSMUX_FORCE_MOUSE=1` on Server
    // 2022 into "and also stop injecting the records that are the only thing
    // delivering the mouse there".
    let _lock = crate::util::lock_test_env();
    let saved_force = std::env::var(crate::ssh_input::FORCE_MOUSE_ENV).ok();
    let saved_build = std::env::var("PSMUX_FAKE_WIN_BUILD").ok();
    std::env::set_var("PSMUX_FAKE_WIN_BUILD", "19045");
    for force in ["0", "1"] {
        std::env::set_var(crate::ssh_input::FORCE_MOUSE_ENV, force);
        assert!(
            crate::ssh_input::conpty_needs_mouse_record_bypass(),
            "PSMUX_FORCE_MOUSE={force} must not change the pane-side record routing"
        );
    }
    match saved_force {
        Some(v) => std::env::set_var(crate::ssh_input::FORCE_MOUSE_ENV, v),
        None => std::env::remove_var(crate::ssh_input::FORCE_MOUSE_ENV),
    }
    match saved_build {
        Some(v) => std::env::set_var("PSMUX_FAKE_WIN_BUILD", v),
        None => std::env::remove_var("PSMUX_FAKE_WIN_BUILD"),
    }
}

// ---------------------------------------------------------------------------
// The routing decision table
// ---------------------------------------------------------------------------

#[test]
fn modern_build_routes_only_the_wheel_through_the_record_channel() {
    // The 22523+ rows of the table, which this change must leave exactly as
    // they were.  Press, release, drag and bare motion all take the pipe alone.
    with_build(Some("26200"), || {
        assert!(record_bypass_applies(WHEEL), "wheel keeps its #277 record on every build");
        assert!(!record_bypass_applies(CLICK), "click press/release stays pipe-only on 22523+");
        assert!(!record_bypass_applies(MOVED), "drag and bare motion stay pipe-only on 22523+");
    });
}

#[test]
fn legacy_build_routes_every_event_through_the_record_channel() {
    // The 19045 rows: the pipe is dead there, so every event that earned a
    // report also gets the record.  This is the whole fix.
    with_build(Some("19045"), || {
        assert!(record_bypass_applies(WHEEL));
        assert!(record_bypass_applies(CLICK), "BUG #597: a click never reached a record reader on Win10");
        assert!(record_bypass_applies(MOVED), "BUG #597: a drag never reached a record reader on Win10");
    });
}

#[test]
fn the_wheel_row_is_build_independent() {
    // #277 shipped the wheel record unconditionally.  Whatever the build seam
    // says, the wheel must keep it, or this change would regress #277 on the
    // very builds it was written for.
    for build in ["19041", "19045", "20348", "22523", "26200"] {
        with_build(Some(build), || {
            assert!(
                record_bypass_applies(WHEEL),
                "the wheel lost its record bypass on build {build}"
            );
        });
    }
}

#[test]
fn the_decision_never_looks_at_the_button_state() {
    // Press (FROM_LEFT_1ST_BUTTON_PRESSED) and release (0) carry the same
    // event flags, so both must route identically.  A rule that keyed off the
    // button state would deliver the press and swallow the release, which is
    // worse than delivering neither: a TUI would be left with the button
    // stuck down.
    with_build(Some("19045"), || {
        assert_eq!(record_bypass_applies(CLICK), record_bypass_applies(CLICK));
        assert!(record_bypass_applies(CLICK));
    });
}

#[test]
fn a_wheel_that_also_moved_still_takes_the_record() {
    // Defensive: some injectors set MOUSE_MOVED alongside MOUSE_WHEELED.  The
    // wheel bit is tested with a mask, not equality, so the combination is
    // still a wheel.
    with_build(Some("26200"), || {
        assert!(record_bypass_applies(WHEEL | MOVED));
    });
}
