//! PR #468: WezTerm on Windows is a ConPTY-based VT terminal that writes VT
//! mouse escape sequences (SGR `\x1b[<…M`) to the ConPTY input pipe, exactly
//! like JediTerm. ConPTY does not translate these into MOUSE_EVENT records, so
//! without the VT input parser the raw SGR bytes leak through as KEY_EVENT text
//! into the active pane on mouse movement/clicks.
//!
//! The fix routes WezTerm through the same VT input path already used for SSH
//! and JediTerm by detecting the env vars WezTerm always sets
//! (`TERM_PROGRAM=WezTerm` and `WEZTERM_PANE`).
//!
//! These tests exercise the pure routing decision (`needs_vt_input`) which IS
//! the entire fix. A real WezTerm session sets exactly these env vars, so this
//! is the same code path with the same inputs, not a proxy.

use super::*;

/// Env vars that can independently flip `needs_vt_input()` to true. We clear
/// ALL of them so each test isolates the single variable under test, then
/// restore the originals. Uses the shared env lock so it never races other
/// env-touching tests.
const VT_ENV_VARS: &[&str] = &[
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "TERMINAL_EMULATOR",
    "TERM_PROGRAM",
    "WEZTERM_PANE",
    // Alacritty's signal, and the explicit opt in. Cleared too, so the
    // baseline holds when the suite runs from an Alacritty window, which sets
    // ALACRITTY_LOG for every child.
    "ALACRITTY_LOG",
    "PSMUX_VT_INPUT",
];

fn with_clean_vt_env<T>(setup: impl FnOnce(), f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let saved: Vec<(&str, Option<String>)> = VT_ENV_VARS
        .iter()
        .map(|k| (*k, std::env::var(k).ok()))
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
fn baseline_no_vt_terminal_is_native_input() {
    // Sanity: with every VT-routing var cleared, a plain local terminal must
    // NOT take the VT input path (native ReadConsoleInputW / MOUSE_EVENT path).
    with_clean_vt_env(
        || {},
        || {
            assert!(
                !needs_vt_input(),
                "plain local terminal must use native input, not VT parser"
            );
        },
    );
}

#[test]
fn wezterm_term_program_routes_to_vt_input() {
    // The reporter's exact signal: WezTerm sets TERM_PROGRAM=WezTerm.
    // This is the core regression guard: on unpatched master this FAILS
    // (returns false) because WezTerm was never added to needs_vt_input(),
    // which is why SGR mouse bytes leaked into the pane.
    with_clean_vt_env(
        || std::env::set_var("TERM_PROGRAM", "WezTerm"),
        || {
            assert!(
                needs_vt_input(),
                "BUG PR#468: TERM_PROGRAM=WezTerm must route through VT input parser"
            );
        },
    );
}

#[test]
fn wezterm_pane_var_routes_to_vt_input() {
    // WezTerm also always sets WEZTERM_PANE (the pane id). Either signal alone
    // must be sufficient, since a user could unset TERM_PROGRAM.
    with_clean_vt_env(
        || std::env::set_var("WEZTERM_PANE", "0"),
        || {
            assert!(
                needs_vt_input(),
                "BUG PR#468: WEZTERM_PANE presence must route through VT input parser"
            );
        },
    );
}

#[test]
fn wezterm_both_signals_route_to_vt_input() {
    // Real WezTerm sets both at once.
    with_clean_vt_env(
        || {
            std::env::set_var("TERM_PROGRAM", "WezTerm");
            std::env::set_var("WEZTERM_PANE", "3");
        },
        || {
            assert!(needs_vt_input(), "real WezTerm (both vars) must route to VT input");
        },
    );
}

#[test]
fn non_wezterm_term_program_stays_native() {
    // No false positives: other terminals that set TERM_PROGRAM (vscode,
    // Apple_Terminal, tmux, etc.) must NOT be dragged onto the VT path, or we
    // would regress native mouse handling for the common local case.
    for other in ["vscode", "Apple_Terminal", "tmux", "wezterm", "WEZTERM", "iTerm.app"] {
        with_clean_vt_env(
            || std::env::set_var("TERM_PROGRAM", other),
            || {
                assert!(
                    !needs_vt_input(),
                    "TERM_PROGRAM={other} must NOT route to VT input (exact 'WezTerm' match only)"
                );
            },
        );
    }
}

#[test]
fn existing_jediterm_and_ssh_paths_still_work() {
    // Regression guard: the WezTerm addition must not disturb the pre-existing
    // JediTerm and SSH routing.
    with_clean_vt_env(
        || std::env::set_var("TERMINAL_EMULATOR", "JetBrains-JediTerm"),
        || assert!(needs_vt_input(), "JediTerm must still route to VT input"),
    );
    with_clean_vt_env(
        || std::env::set_var("SSH_CONNECTION", "1.2.3.4 5 6.7.8.9 22"),
        || assert!(needs_vt_input(), "SSH must still route to VT input"),
    );
}
