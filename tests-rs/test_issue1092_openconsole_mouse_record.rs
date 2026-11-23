//! Issue #1092: a pane on the console host of `PSMUX_CONPTY_DIR` (OpenConsole)
//! on a build below `CONPTY_MOUSE_MIN_BUILD` gets each mouse event once.
//!
//! On those builds psmux injects a `MOUSE_EVENT` record for every mouse event
//! (#597), because the inbox conhost drops the SGR report psmux writes into the
//! pane's input pipe. OpenConsole does not drop it, and with the app's VT
//! input off it turns the report into a record itself, so the injected one was
//! a second copy. Measured on 19045 with OpenConsole 1.24, a record reader
//! got 3 notches as 6 wheel records, 1 click as 2 presses and 2 releases, and
//! 1 drag of 8 steps as 2 presses and 16 moves. With VT input on (Bubble Tea)
//! the host passes the report on as text and the record is the only copy.
//! The E2E half is the OpenConsole section of `tests/test_wheel_record_once.ps1`
//! and `tests/test_issue1092_openconsole_mouse_record.ps1`.

use super::*;
use crate::ssh_input::{host_drops_mouse_report, CONPTY_MOUSE_MIN_BUILD};

const WHEEL: u32 = mouse_inject::MOUSE_WHEELED;
const HWHEEL: u32 = mouse_inject::MOUSE_HWHEELED;
const MOVED: u32 = mouse_inject::MOUSE_MOVED;
const CLICK: u32 = 0;
/// A record reader's console: mouse input on, VT input off.
const READER: u32 = ENABLE_MOUSE_INPUT;
const READER_VTI: u32 = ENABLE_MOUSE_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT;

fn duplicate(flags: u32, drops: bool, vti: Option<bool>, mode: Option<u32>) -> bool {
    wheel_record_is_duplicate(flags, drops, vti, mode) || mouse_record_is_duplicate(flags, drops, vti, mode)
}

#[test]
fn inbox_host_below_the_build_drops_the_report() {
    for build in [19041, 19045, CONPTY_MOUSE_MIN_BUILD - 1] {
        assert!(host_drops_mouse_report(Some(build), false), "inbox host on {build}");
    }
}

#[test]
fn supplied_host_never_drops_the_report() {
    for build in [19041, 19045, CONPTY_MOUSE_MIN_BUILD - 1, CONPTY_MOUSE_MIN_BUILD, 26300] {
        assert!(!host_drops_mouse_report(Some(build), true), "OpenConsole on {build}");
    }
}

#[test]
fn current_and_unknown_builds_are_unchanged() {
    for supplied in [false, true] {
        assert!(!host_drops_mouse_report(Some(CONPTY_MOUSE_MIN_BUILD), supplied));
        assert!(!host_drops_mouse_report(Some(26300), supplied));
        assert!(!host_drops_mouse_report(None, supplied));
    }
}

#[test]
fn openconsole_on_windows10_drops_the_second_record_of_every_event() {
    let drops = host_drops_mouse_report(Some(19045), true);
    for flags in [WHEEL, CLICK, MOVED] {
        assert!(duplicate(flags, drops, Some(false), Some(READER)), "flags {flags:#x}");
    }
}

#[test]
fn inbox_host_on_windows10_keeps_every_record() {
    // There the record is the only copy (#597).
    let drops = host_drops_mouse_report(Some(19045), false);
    for flags in [WHEEL, HWHEEL, CLICK, MOVED] {
        assert!(!duplicate(flags, drops, Some(false), Some(READER)), "flags {flags:#x}");
    }
}

#[test]
fn openconsole_keeps_the_record_for_a_vt_input_reader() {
    // Bubble Tea: the host hands the report over as text (#277).
    let drops = host_drops_mouse_report(Some(19045), true);
    for flags in [WHEEL, CLICK, MOVED] {
        assert!(!duplicate(flags, drops, Some(true), Some(READER_VTI)), "flags {flags:#x}");
    }
}

#[test]
fn openconsole_keeps_the_horizontal_wheel_record() {
    // The host converts no SGR 66/67, so MOUSE_HWHEELED is the only copy.
    let drops = host_drops_mouse_report(Some(19045), true);
    assert!(!duplicate(HWHEEL, drops, Some(false), Some(READER)));
}

#[test]
fn unknown_console_state_keeps_the_record() {
    let drops = host_drops_mouse_report(Some(19045), true);
    for flags in [CLICK, MOVED] {
        assert!(!mouse_record_is_duplicate(flags, drops, None, Some(READER)));
        assert!(!mouse_record_is_duplicate(flags, drops, Some(false), None));
        assert!(!mouse_record_is_duplicate(flags, drops, Some(false), Some(0x01A0)));
    }
}

#[test]
fn the_wheel_is_left_to_the_wheel_check() {
    // mouse_record_is_duplicate never answers for a wheel notch, so the
    // wheel keeps the rules test_wheel_record_once pins.
    for drops in [false, true] {
        assert!(!mouse_record_is_duplicate(WHEEL, drops, Some(false), Some(READER)));
        assert!(!mouse_record_is_duplicate(HWHEEL, drops, Some(false), Some(READER)));
    }
}

#[test]
fn this_process_reads_the_inbox_host() {
    // No PSMUX_CONPTY_DIR in the test process, so the answer is the build's
    // alone, the same as conpty_needs_mouse_record_bypass.
    let _lock = crate::util::lock_test_env();
    assert_eq!(
        crate::ssh_input::pane_host_drops_mouse_report(),
        crate::ssh_input::conpty_needs_mouse_record_bypass()
    );
}
