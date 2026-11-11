//! Issue #1036 follow up: the horizontal wheel as tmux's buttons 6 and 7.
//!
//! tmux 3.8 has no WheelLeft/WheelRight key. SGR buttons 66 and 67 are
//! MOUSE_BUTTON_6/7 (tmux.h), outside MOUSE_WHEEL(), so they never scroll:
//! server_client_check_mouse makes them MouseDown6Pane / MouseDown7Pane, which
//! have no default binding, and an unbound one is forwarded to the pane only
//! when its program enabled mouse reporting.
//!
//! These tests pin the pure parts: the Win32 record a native console app gets
//! for 66/67, that the horizontal record takes the same wheel channel as the
//! vertical one, and how MouseDown6Pane / MouseDown7Pane resolve. The E2E half
//! lives in tests/test_issue1036_hwheel_bindings.ps1.

use super::*;
use crossterm::event::KeyModifiers;

fn app() -> crate::types::AppState {
    let mut app = crate::types::AppState::new("test_session".to_string());
    crate::config::populate_default_bindings(&mut app);
    app
}

fn hkey(right: bool, mods: KeyModifiers) -> (crossterm::event::KeyCode, KeyModifiers) {
    crate::mouse_keys::key(
        crate::mouse_keys::EV_MOUSE_DOWN,
        if right { 7 } else { 6 },
        crate::mouse_keys::LOC_PANE,
        mods,
    )
}

#[test]
fn hwheel_record_is_left_negative_right_positive() {
    let left = (((-120i16) as i32) << 16) as u32;
    let right = ((120i32) << 16) as u32;
    assert_eq!(hwheel_record(66), Some((left, mouse_inject::MOUSE_HWHEELED)));
    assert_eq!(hwheel_record(67), Some((right, mouse_inject::MOUSE_HWHEELED)));
    // Shift, meta and ctrl ride along as SGR modifier bits.
    for m in [4u8, 8, 16, 4 | 8 | 16] {
        assert_eq!(hwheel_record(66 + m), Some((left, mouse_inject::MOUSE_HWHEELED)), "66+{}", m);
        assert_eq!(hwheel_record(67 + m), Some((right, mouse_inject::MOUSE_HWHEELED)), "67+{}", m);
    }
}

#[test]
fn hwheel_record_leaves_every_other_button_alone() {
    // Vertical wheel, clicks, releases, motion and the extended buttons 8 to 11
    // keep whatever they did before: no horizontal record.
    for b in [0u8, 1, 2, 3, 32, 34, 35, 64, 65, 64 + 4, 65 + 16, 96, 98, 99, 128, 129, 130, 131] {
        assert_eq!(hwheel_record(b), None, "button {}", b);
    }
}

#[test]
fn horizontal_flag_is_its_own_bit() {
    use mouse_inject::{MOUSE_HWHEELED, MOUSE_MOVED, MOUSE_WHEELED};
    assert_eq!(MOUSE_HWHEELED, 0x0008, "winconsole.h MOUSE_HWHEELED");
    assert_eq!(MOUSE_HWHEELED & (MOUSE_WHEELED | MOUSE_MOVED), 0);
}

#[test]
fn horizontal_wheel_takes_the_wheel_record_channel() {
    let _lock = crate::util::lock_test_env();
    let saved = std::env::var("PSMUX_FAKE_WIN_BUILD").ok();
    std::env::set_var("PSMUX_FAKE_WIN_BUILD", "26200");
    let h = record_bypass_applies(mouse_inject::MOUSE_HWHEELED);
    let v = record_bypass_applies(mouse_inject::MOUSE_WHEELED);
    let click = record_bypass_applies(0);
    let moved = record_bypass_applies(mouse_inject::MOUSE_MOVED);
    match saved {
        Some(s) => std::env::set_var("PSMUX_FAKE_WIN_BUILD", s),
        None => std::env::remove_var("PSMUX_FAKE_WIN_BUILD"),
    }
    assert!(h, "a horizontal notch must reach a record reading app (#277 channel)");
    assert!(v, "vertical wheel unchanged");
    assert!(!click && !moved, "clicks and motion unchanged on a modern build");
}

#[test]
fn buttons_6_and_7_have_no_default_and_forward_like_tmux() {
    let app = app();
    for right in [false, true] {
        assert_eq!(crate::mouse_keys::resolve(&app, hkey(right, KeyModifiers::NONE), false),
            crate::mouse_keys::Dispatch::Forward, "root, right={}", right);
        assert_eq!(crate::mouse_keys::resolve(&app, hkey(right, KeyModifiers::NONE), true),
            crate::mouse_keys::Dispatch::Drop, "copy mode, right={}", right);
    }
}

#[test]
fn bound_buttons_6_and_7_run_the_binding() {
    let mut app = app();
    crate::config::parse_config_content(&mut app, "bind -n MouseDown6Pane set -g @left yes");
    crate::config::parse_config_content(&mut app, "bind -n M-MouseDown7Pane set -g @right yes");
    crate::config::parse_config_content(&mut app, "bind -T copy-mode MouseDown7Pane send -X cursor-right");
    assert_eq!(crate::mouse_keys::resolve(&app, hkey(false, KeyModifiers::NONE), false),
        crate::mouse_keys::Dispatch::User("set -g @left yes".into()));
    assert_eq!(crate::mouse_keys::resolve(&app, hkey(true, KeyModifiers::ALT), false),
        crate::mouse_keys::Dispatch::User("set -g @right yes".into()));
    // The unmodified right button is still unbound in root.
    assert_eq!(crate::mouse_keys::resolve(&app, hkey(true, KeyModifiers::NONE), false),
        crate::mouse_keys::Dispatch::Forward);
    // copy-mode table first while the pane is in copy mode.
    assert!(matches!(crate::mouse_keys::resolve(&app, hkey(true, KeyModifiers::NONE), true),
        crate::mouse_keys::Dispatch::User(_)));
}

#[test]
fn handle_pane_hwheel_is_inert_with_mouse_off() {
    // No windows at all: with the mouse off it must return before touching
    // any pane, like handle_pane_wheel's mouse off early exit.
    let mut app = app();
    app.mouse_enabled = false;
    handle_pane_hwheel(&mut app, 1, true, Some((1, 1)), 0);
    handle_pane_hwheel(&mut app, 1, false, None, 8);
}
