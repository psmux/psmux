//! Issue #787: Alacritty on Windows writes keys to its ConPTY as VT bytes, as
//! WezTerm does, and has no win32-input-mode, so its clients belong on the VT
//! input path. On the console route two things broke, both measured under
//! Alacritty 0.17.0 on Windows 11 26200 with `PSMUX_INPUT_DEBUG=1`:
//!
//!   Ctrl+J   conhost handed it over as Enter with CONTROL, so it went to the
//!            pane as `send-key C-Enter` and a C-j prefix never armed
//!   startup  the input pump read `5`, `5` and Alt+\ 8ms after it started,
//!            the torn tail of a colour reply, and typed them into the pane
//!
//! With `PSMUX_VT_INPUT=1` in the same window, `PSMUX_SSH_DEBUG=1` showed
//! `u_char=0x000A` decoded as Char('j') with CONTROL, the prefix armed, and
//! nothing reached the pane at startup. Mouse moves, clicks and wheel, a
//! bracketed paste, IME text and Alt+a all decoded on that path too.
//!
//! Alacritty sets no TERM_PROGRAM. The variable it always sets on Windows is
//! ALACRITTY_LOG (alacritty/src/logging.rs:204, v0.17.0); ALACRITTY_WINDOW_ID
//! is set on Unix only (alacritty_terminal/src/tty/unix.rs:230).

use super::*;

/// Every variable that can put a client on the VT input path on its own,
/// cleared so each test sees only the one it sets, then put back. Holds the
/// shared env lock so it never races another env test.
const VT_ENV_VARS: &[&str] = &[
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "TERMINAL_EMULATOR",
    "TERM_PROGRAM",
    "WEZTERM_PANE",
    "ALACRITTY_LOG",
    "PSMUX_VT_INPUT",
    // Not signals, but one test sets them, so they are put back as well.
    "TERM",
    "COLORTERM",
];

fn with_clean_vt_env<T>(setup: impl FnOnce(), f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let saved: Vec<(&str, Option<std::ffi::OsString>)> = VT_ENV_VARS
        .iter()
        .map(|k| (*k, std::env::var_os(k)))
        .collect();
    for k in VT_ENV_VARS {
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
fn alacritty_log_puts_the_client_on_the_vt_input_path() {
    // The value Alacritty 0.17.0 sets: its log file in the temp directory,
    // named after its own pid.
    with_clean_vt_env(
        || std::env::set_var("ALACRITTY_LOG", r"C:\Users\u\AppData\Local\Temp\Alacritty-12092.log"),
        || assert!(needs_vt_input(), "an Alacritty client must read its input on the VT path"),
    );
}

#[test]
fn the_value_of_alacritty_log_does_not_matter() {
    // Presence is the signal. The file is created only when something is
    // logged, so the path may not exist.
    with_clean_vt_env(
        || std::env::set_var("ALACRITTY_LOG", "x"),
        || assert!(needs_vt_input()),
    );
}

#[test]
fn the_rest_of_alacrittys_environment_is_not_a_signal() {
    // TERM and COLORTERM are what Alacritty sets besides (setup_env in
    // alacritty_terminal/src/tty/mod.rs), and many terminals set the same
    // values, so neither may route a client on its own.
    with_clean_vt_env(
        || {
            std::env::set_var("TERM", "xterm-256color");
            std::env::set_var("COLORTERM", "truecolor");
        },
        || assert!(!needs_vt_input(), "TERM and COLORTERM alone must keep the console route"),
    );
}

#[test]
fn without_any_signal_the_console_route_is_kept() {
    with_clean_vt_env(|| {}, || assert!(!needs_vt_input()));
}

#[cfg(windows)]
#[test]
fn an_alacritty_client_waits_for_the_last_colour_replies() {
    // The second half of the fix rides on the first: the colour query drain
    // waits out the settle window only on the VT input path (#646), and the
    // `55` came from returning without it.
    with_clean_vt_env(
        || std::env::set_var("ALACRITTY_LOG", "x"),
        || {
            assert_eq!(
                crate::platform::settle_window(needs_vt_input()),
                std::time::Duration::from_millis(75)
            );
        },
    );
    with_clean_vt_env(
        || {},
        || {
            assert_eq!(
                crate::platform::settle_window(needs_vt_input()),
                std::time::Duration::ZERO,
                "a host on the console route still pays nothing at startup"
            );
        },
    );
}
