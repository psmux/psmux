//! #879: interactive Ctrl+[ bypassed the lone-Escape repair from #588.
//! It worked on a fresh pane, then stopped after a win32 input record latched
//! the pane's ConPTY. Exercise the actual attached-client dispatcher, not CLI
//! send-keys, with a recording writer and no console, process, or server.

use crate::input::{send_key_to_active, win32_input_key_seq};
use super::pane_input_fixture::{fixture, pane};

const ESCAPE_ALIASES: &[&str] = &["C-[", "C-3", "C-S-[", "C-S-3", "c-s-3"];

#[test]
fn escape_aliases_stay_raw_on_a_fresh_pane() {
    let (mut app, recorder) = fixture(true);
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), b"\x1b", "{key}");
        assert!(!pane(&mut app).win32_input_latched);
    }
}

#[test]
fn escape_aliases_follow_physical_escape_after_a_named_key_latches_the_pane() {
    let (mut app, recorder) = fixture(true);
    // Named keys already go through write_pane_input. On this host/reader
    // combination, Left writes a win32 record and latches the pane (#729).
    send_key_to_active(&mut app, "left").unwrap();
    assert!(pane(&mut app).win32_input_latched);
    recorder.take();

    send_key_to_active(&mut app, "esc").unwrap();
    let escape = recorder.take();
    assert_eq!(
        escape,
        win32_input_key_seq(27, crate::platform::mouse_inject::vk_to_scan(27), 27, 0).into_bytes()
    );
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), escape, "{key} must match physical Escape");
    }
}

#[test]
fn ordinary_control_bytes_are_unchanged_on_a_latched_pane() {
    let (mut app, recorder) = fixture(true);
    pane(&mut app).win32_input_latched = true;
    for (key, byte) in [
        ("C-w", 0x17),
        ("C-c", 0x03),
        ("C-/", 0x1f),
        ("C-S--", 0x1f),
        ("C-4", 0x1c),
    ] {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), [byte], "{key}");
    }
}

#[test]
fn an_old_host_keeps_raw_escape_even_if_the_latch_is_set() {
    let (mut app, recorder) = fixture(false);
    // #845: never send a win32 record to a host that did not request it.
    pane(&mut app).win32_input_latched = true;
    for key in ESCAPE_ALIASES {
        send_key_to_active(&mut app, key).unwrap();
        assert_eq!(recorder.take(), b"\x1b", "{key}");
    }
}
