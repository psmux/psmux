//! Issue #902: ConEmu writes a paste into the real console with
//! WriteConsoleInput, and there is no ConPTY to strip the bracketed paste
//! markers, so on the console input route they reached the pane as keys: a
//! paste of `Hello, psmux!` showed `200~Hello, psmux!201~` (ConEmu 230724,
//! `PSMUX_INPUT_DEBUG=1`).
//!
//! The VT input route decodes the markers, but under ConEmu the real console
//! re-encodes key records as legacy VT there, measured with
//! `PSMUX_SSH_DEBUG=1`: Ctrl+Enter arrived as 0x0a (C-j), Ctrl+Backspace as
//! 0x1f (C-_) and Shift+Enter as a plain CR. So ConEmu keeps the console route
//! and the client does not ask it for bracketed paste.

use super::*;

/// Every variable these tests read, cleared so each test sees only what it
/// sets, then put back. Holds the shared env lock so it never races another
/// env test.
const ENV_VARS: &[&str] = &[
    "ConEmuPID",
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "TERMINAL_EMULATOR",
    "TERM_PROGRAM",
    "WEZTERM_PANE",
    "ALACRITTY_LOG",
    "PSMUX_VT_INPUT",
];

fn with_clean_env<T>(setup: impl FnOnce(), f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = ENV_VARS
        .iter()
        .map(|k| (*k, std::env::var_os(k)))
        .collect();
    for k in ENV_VARS {
        std::env::remove_var(k);
    }
    setup();
    let out = f();
    for (k, v) in saved {
        match v {
            Some(val) => std::env::set_var(k, val),
            None => std::env::remove_var(k),
        }
    }
    out
}

#[test]
fn conemu_pid_marks_a_conemu_console() {
    // The value is the PID of the ConEmu GUI; presence is the signal.
    with_clean_env(
        || std::env::set_var("ConEmuPID", "15604"),
        || assert!(is_conemu_host()),
    );
    with_clean_env(|| {}, || assert!(!is_conemu_host()));
}

#[test]
fn conemu_stays_on_the_console_input_route() {
    // The VT route loses Ctrl+Enter, Ctrl+Backspace and Shift+Enter under
    // ConEmu, so ConEmuPID must not become a VT route signal.
    with_clean_env(
        || std::env::set_var("ConEmuPID", "15604"),
        || assert!(!needs_vt_input(), "a ConEmu client must keep the console route"),
    );
}

#[test]
fn conemu_on_the_console_route_is_not_asked_for_bracketed_paste() {
    // Nothing on this route decodes the markers, so they would reach the
    // pane as `200~` and `201~`.
    assert!(!host_bracketed_paste(false, true));
}

#[test]
fn a_route_that_decodes_the_markers_still_asks_conemu() {
    // PSMUX_VT_INPUT=1 in ConEmu, or the pipe route: the VT parser reads the
    // markers and sends the paste in one piece, as tmux does.
    assert!(host_bracketed_paste(true, true));
}

#[test]
fn other_hosts_are_still_asked_for_bracketed_paste() {
    // Under ConPTY (Windows Terminal and the rest) conhost strips the markers
    // on the console route, and the VT and pipe routes decode them, so
    // nothing changes outside ConEmu.
    assert!(host_bracketed_paste(false, false));
    assert!(host_bracketed_paste(true, false));
}
