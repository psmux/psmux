// Issue #606: `set-option -g repeat-time N` in a config file was rejected with
// "unknown option 'repeat-time'".
//
// The option existed everywhere except in `parse_option_value`: the server's
// set-option honoured it, the catalog described it and the repeat window in
// src/input.rs consumed it, but a `.tmux.conf` line fell through to the
// hyphenated catch-all, so the value landed in `user_options`, the parser
// warned, and `repeat_time_ms` kept its 500 ms default. The reporter's
// `repeat-time 0` therefore never disabled `bind-key -r` repeat.
//
// These tests pin the plumbing on every route the config parser owns plus the
// server-side bound. The live repeat window (real keystrokes into an attached
// client) is covered by tests/test_issue606_repeat_time.ps1.

use super::*;

use crate::server::options::{apply_set_option, get_option_value, REPEAT_TIME_MAX_MS};

fn mock_app() -> AppState {
    AppState::new("repeat606".to_string())
}

// ---------------------------------------------------------------------------
// The config route: this is the arm that was missing.
// ---------------------------------------------------------------------------

#[test]
fn config_sets_repeat_time() {
    let mut app = mock_app();
    parse_option_value(&mut app, "repeat-time", "250", true);
    assert_eq!(
        app.repeat_time_ms, 250,
        "a config `set -g repeat-time 250` must reach repeat_time_ms"
    );
}

#[test]
fn config_never_warns_unknown_option() {
    let mut app = mock_app();
    parse_option_value(&mut app, "repeat-time", "250", true);
    assert!(
        !app.config_warnings.iter().any(|w| w.contains("unknown option")),
        "repeat-time must not be reported as unknown, got: {:?}",
        app.config_warnings
    );
}

#[test]
fn config_does_not_stash_repeat_time_in_user_options() {
    // The catch-all used to store it as an opaque string, which is why
    // `show-options -g` printed `repeat-time "250"` while the real option
    // still read 500.
    let mut app = mock_app();
    parse_option_value(&mut app, "repeat-time", "250", true);
    assert!(
        !app.user_options.contains_key("repeat-time"),
        "repeat-time must be a real option, not a user_options passthrough"
    );
}

#[test]
fn config_zero_disables_repeat() {
    let mut app = mock_app();
    parse_option_value(&mut app, "repeat-time", "0", true);
    assert_eq!(app.repeat_time_ms, 0, "0 must be storable, it disables repeat");
}

#[test]
fn config_accepts_the_upper_bound() {
    let mut app = mock_app();
    parse_option_value(&mut app, "repeat-time", "2000000", true);
    assert_eq!(app.repeat_time_ms, 2_000_000);
}

#[test]
fn config_rejects_a_value_above_the_bound() {
    let mut app = mock_app();
    let before = app.repeat_time_ms;
    parse_option_value(&mut app, "repeat-time", "2000001", true);
    assert_eq!(app.repeat_time_ms, before, "out of range value must not be stored");
    assert!(
        app.config_warnings.iter().any(|w| w.contains("value is too large: 2000001")),
        "tmux warns 'value is too large', got: {:?}",
        app.config_warnings
    );
}

#[test]
fn config_rejects_a_negative_value() {
    let mut app = mock_app();
    let before = app.repeat_time_ms;
    parse_option_value(&mut app, "repeat-time", "-5", true);
    assert_eq!(app.repeat_time_ms, before, "negative value must not be stored");
    assert!(
        app.config_warnings.iter().any(|w| w.contains("value is too small: -5")),
        "tmux warns 'value is too small', got: {:?}",
        app.config_warnings
    );
}

#[test]
fn config_non_numeric_warns_once_about_the_type() {
    let mut app = mock_app();
    let before = app.repeat_time_ms;
    parse_option_value(&mut app, "repeat-time", "abc", true);
    assert_eq!(app.repeat_time_ms, before);
    let typed: Vec<_> = app
        .config_warnings
        .iter()
        .filter(|w| w.contains("repeat-time"))
        .collect();
    assert_eq!(
        typed.len(),
        1,
        "a non-numeric value must warn exactly once, got: {:?}",
        app.config_warnings
    );
    assert!(
        typed[0].contains("expected a number"),
        "the single warning must be the type warning, got: {:?}",
        typed
    );
}

#[test]
fn full_config_line_reaches_the_option() {
    // End to end through the line parser, the reporter's literal .tmux.conf.
    let _lock = crate::util::lock_test_env();
    let mut app = mock_app();
    parse_config_content(&mut app, "set-option -g repeat-time 0\n");
    assert_eq!(app.repeat_time_ms, 0);
    assert!(
        !app.config_warnings.iter().any(|w| w.contains("repeat-time")),
        "the reporter's exact line must parse clean, got: {:?}",
        app.config_warnings
    );
}

#[test]
fn short_form_config_line_reaches_the_option() {
    let _lock = crate::util::lock_test_env();
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g repeat-time 1250\n");
    assert_eq!(app.repeat_time_ms, 1250);
}

// ---------------------------------------------------------------------------
// The server route: already worked, but was unbounded.
// ---------------------------------------------------------------------------

#[test]
fn set_option_stores_repeat_time() {
    let mut app = mock_app();
    apply_set_option(&mut app, "repeat-time", "3000", false);
    assert_eq!(app.repeat_time_ms, 3000);
}

#[test]
fn set_option_round_trips_through_show_options() {
    let mut app = mock_app();
    apply_set_option(&mut app, "repeat-time", "1750", false);
    assert_eq!(
        get_option_value(&app, "repeat-time"),
        "1750",
        "show-options -v repeat-time must report the stored value"
    );
}

#[test]
fn set_option_refuses_an_out_of_range_value() {
    // The command prompt and raw TCP routes never see the CLI guard, so the
    // bound has to live here too.
    let mut app = mock_app();
    apply_set_option(&mut app, "repeat-time", "1750", false);
    apply_set_option(&mut app, "repeat-time", "5000000", false);
    assert_eq!(
        app.repeat_time_ms, 1750,
        "an out of range value must leave the previous one in place"
    );
    apply_set_option(&mut app, "repeat-time", "-1", false);
    assert_eq!(app.repeat_time_ms, 1750, "a negative value must be refused too");
}

#[test]
fn default_matches_tmux() {
    let app = mock_app();
    assert_eq!(app.repeat_time_ms, 500, "tmux's options-table default is 500 ms");
    assert_eq!(get_option_value(&app, "repeat-time"), "500");
}

#[test]
fn bound_matches_tmux_options_table() {
    assert_eq!(
        REPEAT_TIME_MAX_MS, 2_000_000,
        "tmux options-table.c declares repeat-time with maximum 2000000"
    );
}

#[test]
fn catalog_describes_repeat_time() {
    let def = crate::server::option_catalog::OPTION_CATALOG
        .iter()
        .find(|d| d.name == "repeat-time")
        .expect("repeat-time must be in the option catalog");
    assert_eq!(def.option_type, "number");
    assert_eq!(def.scope, "session");
    assert_eq!(def.default, "500");
}

// ---------------------------------------------------------------------------
// The consumer: src/input.rs Mode::Prefix leaves the prefix table once
// `elapsed >= app.repeat_time_ms`. The window is driven by a real Instant, so
// this pins the decision the dispatcher makes rather than the dispatcher
// itself; the live behaviour is proven by the injector section of
// tests/test_issue606_repeat_time.ps1.
// ---------------------------------------------------------------------------

#[test]
fn repeat_window_expiry_follows_the_option() {
    fn expired(repeat_time_ms: u64, elapsed_ms: u64) -> bool {
        elapsed_ms >= repeat_time_ms
    }
    let mut app = mock_app();

    parse_option_value(&mut app, "repeat-time", "0", true);
    assert!(
        expired(app.repeat_time_ms, 0),
        "repeat-time 0 must expire immediately, so no key ever repeats"
    );

    parse_option_value(&mut app, "repeat-time", "500", true);
    assert!(expired(app.repeat_time_ms, 1500), "500 ms window is gone after 1500 ms");
    assert!(!expired(app.repeat_time_ms, 200), "500 ms window is alive after 200 ms");

    parse_option_value(&mut app, "repeat-time", "3000", true);
    assert!(
        !expired(app.repeat_time_ms, 1500),
        "3000 ms window must survive a 1500 ms gap"
    );
}
