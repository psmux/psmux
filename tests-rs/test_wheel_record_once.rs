//! One wheel notch, one wheel record.
//!
//! For a pane app that reads its mouse as console records, psmux writes the
//! notch to the pane's ConPTY pipe as SGR AND injects a MOUSE_WHEELED record
//! (#277). The record exists for consoles with VT input on, where the host
//! hands the SGR over as text. A deliberate record reader keeps VT input off
//! (#623), and then a host from CONPTY_MOUSE_MIN_BUILD on converts the SGR
//! into a wheel record by itself: measured 5 notches -> 10 records (5 with
//! delta 128 from the host, 5 with delta 120 from psmux) on inbox conhost and
//! OpenConsole alike. `wheel_record_is_duplicate` decides when the injected
//! record must be left out. The E2E half is tests/test_wheel_record_once.ps1.

use super::*;

const WHEEL: u32 = mouse_inject::MOUSE_WHEELED;
const HWHEEL: u32 = mouse_inject::MOUSE_HWHEELED;
/// The record reader shape measured above: mouse input on, cooked off, no VTI.
const READER: u32 = 0x01B0;
/// Bubble Tea: keys as VT, mouse still as records.
const READER_VTI: u32 = 0x01B0 | ENABLE_VIRTUAL_TERMINAL_INPUT;

#[test]
fn record_reader_without_vti_gets_the_host_copy_only() {
    assert!(wheel_record_is_duplicate(WHEEL, false, Some(false), Some(READER)));
    // Any mode with mouse input on and VTI off converts, cooked or not.
    assert!(wheel_record_is_duplicate(WHEEL, false, Some(false), Some(0x01F7)));
}

#[test]
fn vti_on_keeps_the_record_issue_277() {
    // VTI on (by ensure_vti or by the app): the host passes the SGR as text,
    // the record is the only wheel event a record reading app gets.
    assert!(!wheel_record_is_duplicate(WHEEL, false, Some(true), Some(READER_VTI)));
    // The #623 branch caches "off" without asking; the live mode says VTI on.
    assert!(!wheel_record_is_duplicate(WHEEL, false, Some(false), Some(READER_VTI)));
}

#[test]
fn legacy_builds_keep_the_record_issue_597() {
    // Below CONPTY_MOUSE_MIN_BUILD the pipe report never reaches the child.
    assert!(!wheel_record_is_duplicate(WHEEL, true, Some(false), Some(READER)));
}

#[test]
fn horizontal_wheel_keeps_its_record() {
    // The host converts no SGR 66/67, so MOUSE_HWHEELED is the only copy.
    assert!(!wheel_record_is_duplicate(HWHEEL, false, Some(false), Some(READER)));
}

#[test]
fn unknown_state_keeps_the_record() {
    assert!(!wheel_record_is_duplicate(WHEEL, false, None, Some(READER)));
    assert!(!wheel_record_is_duplicate(WHEEL, false, Some(false), None));
}

#[test]
fn no_mouse_input_keeps_the_record() {
    // Without ENABLE_MOUSE_INPUT the host makes no record of its own; the
    // injection (which sets the bit itself) stays the delivery path.
    assert!(!wheel_record_is_duplicate(WHEEL, false, Some(false), Some(0x01A0)));
}

#[test]
fn clicks_and_motion_are_not_wheel() {
    assert!(!wheel_record_is_duplicate(0, false, Some(false), Some(READER)));
    assert!(!wheel_record_is_duplicate(mouse_inject::MOUSE_MOVED, false, Some(false), Some(READER)));
}
