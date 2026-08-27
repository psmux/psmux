// Issue #608: psmux's own processes ran at Normal with no foreground boost.
//
// The measurement that motivated this lives in the E2E suite
// (tests/test_issue608_priority.ps1). What is pinned HERE is the parsing and
// precedence around it, because every one of those rules is a place where a
// typo could silently leave the class where it was and look like it worked:
//
//   * only three values are accepted, and the aliases for the middle one
//   * everything else is rejected rather than coerced
//   * PSMUX_PRIORITY outranks the `priority` option
//   * an unusable PSMUX_PRIORITY falls back to the option, then to the default
//   * the option round trips through apply_set_option and show-options
//
// Env-mutating tests serialise through crate::util::lock_test_env(), the single
// process-wide lock, because std::env::set_var is process global and a reader
// in another module can otherwise observe a half-swapped environment.

use crate::platform::{
    current_process_priority, normalize_priority, resolve_priority, set_process_priority,
    DEFAULT_PRIORITY, PRIORITY_VALUES,
};

fn fresh_app() -> crate::types::AppState {
    crate::types::AppState::new("i608_probe".to_string())
}

/// Set or clear PSMUX_PRIORITY for the duration of a closure and always put the
/// previous value back, so one failing case cannot leak into the next.
fn with_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let saved = std::env::var("PSMUX_PRIORITY").ok();
    match value {
        Some(v) => std::env::set_var("PSMUX_PRIORITY", v),
        None => std::env::remove_var("PSMUX_PRIORITY"),
    }
    let out = f();
    match saved {
        Some(v) => std::env::set_var("PSMUX_PRIORITY", v),
        None => std::env::remove_var("PSMUX_PRIORITY"),
    }
    out
}

// ── value parsing ───────────────────────────────────────────────────────────

#[test]
fn the_three_documented_values_are_accepted() {
    assert_eq!(normalize_priority("normal"), Some("normal"));
    assert_eq!(normalize_priority("above-normal"), Some("above-normal"));
    assert_eq!(normalize_priority("high"), Some("high"));
}

#[test]
fn spelling_of_above_normal_is_forgiving_but_canonicalised() {
    // A user who writes it the Win32 way, or with no separator, gets the
    // option they meant. show-options must still report one spelling.
    for spelling in ["above-normal", "above_normal", "abovenormal", "ABOVE-NORMAL"] {
        assert_eq!(
            normalize_priority(spelling),
            Some("above-normal"),
            "{spelling:?} should canonicalise to above-normal"
        );
    }
}

#[test]
fn surrounding_whitespace_and_case_do_not_matter() {
    assert_eq!(normalize_priority("  HIGH  "), Some("high"));
    assert_eq!(normalize_priority("\tNormal\n"), Some("normal"));
}

#[test]
fn everything_else_is_rejected() {
    // realtime is the important one: it outranks kernel threads and a wedged
    // psmux at that class can make a machine unusable, so it is not offered at
    // any spelling. idle and below-normal would make #608 worse, not better.
    for bad in [
        "realtime", "real-time", "idle", "below-normal", "belownormal", "above", "highest", "1",
        "on", "", "   ", "abovenormalx", "nice",
    ] {
        assert_eq!(normalize_priority(bad), None, "{bad:?} must be rejected");
    }
}

#[test]
fn rejected_values_never_reach_set_process_priority() {
    // Fail closed on the parse, fail open on the syscall: a value we do not
    // understand must not turn into a call with a guessed class.
    assert!(!set_process_priority("realtime"));
    assert!(!set_process_priority("bogus"));
    assert!(!set_process_priority(""));
}

#[test]
fn the_advertised_value_list_matches_what_is_accepted() {
    // PRIORITY_VALUES is what the CLI prints when it rejects something. If it
    // drifts from the parser, the error message tells users to type a value
    // that does not work.
    for v in PRIORITY_VALUES {
        assert_eq!(normalize_priority(v), Some(*v), "{v:?} is advertised but not accepted");
    }
    assert_eq!(PRIORITY_VALUES.len(), 3);
    assert!(PRIORITY_VALUES.contains(&DEFAULT_PRIORITY));
}

// ── precedence ──────────────────────────────────────────────────────────────

#[test]
fn with_no_env_and_no_option_the_default_applies() {
    with_env(None, || {
        assert_eq!(resolve_priority(None, false), DEFAULT_PRIORITY);
    });
}

#[test]
fn the_option_is_used_when_the_env_is_unset() {
    with_env(None, || {
        assert_eq!(resolve_priority(Some("high"), false), "high");
        assert_eq!(resolve_priority(Some("normal"), false), "normal");
    });
}

#[test]
fn the_env_outranks_the_option() {
    // The escape hatch: a user who configured something unusable must be able
    // to climb out of it from the shell they start psmux in.
    with_env(Some("normal"), || {
        assert_eq!(resolve_priority(Some("high"), false), "normal");
    });
    with_env(Some("high"), || {
        assert_eq!(resolve_priority(Some("normal"), false), "high");
    });
}

#[test]
fn an_unusable_env_falls_back_to_the_option_then_the_default() {
    with_env(Some("realtime"), || {
        assert_eq!(resolve_priority(Some("high"), false), "high");
        assert_eq!(resolve_priority(None, false), DEFAULT_PRIORITY);
    });
}

#[test]
fn an_empty_env_is_treated_as_unset_not_as_an_error() {
    // `set PSMUX_PRIORITY=` in cmd leaves an empty string behind. That is a
    // user clearing the variable, not asking for a nameless class.
    with_env(Some(""), || {
        assert_eq!(resolve_priority(Some("high"), false), "high");
        assert_eq!(resolve_priority(None, false), DEFAULT_PRIORITY);
    });
    with_env(Some("   "), || {
        assert_eq!(resolve_priority(None, false), DEFAULT_PRIORITY);
    });
}

#[test]
fn an_unusable_option_falls_back_to_the_default() {
    with_env(None, || {
        assert_eq!(resolve_priority(Some("realtime"), false), DEFAULT_PRIORITY);
        assert_eq!(resolve_priority(Some(""), false), DEFAULT_PRIORITY);
    });
}

// ── option round trip ───────────────────────────────────────────────────────

#[test]
fn a_fresh_appstate_reports_the_documented_default() {
    // Deliberately NOT environment sensitive: the catalog parity test compares
    // this against OPTION_CATALOG without holding the env lock.
    assert_eq!(
        crate::server::options::get_option_value(&fresh_app(), "priority"),
        DEFAULT_PRIORITY
    );
}

#[test]
fn set_option_round_trips_through_show_options() {
    with_env(None, || {
        let mut app = fresh_app();
        for want in ["normal", "high", "above-normal"] {
            crate::server::options::apply_set_option(&mut app, "priority", want, false);
            assert_eq!(
                crate::server::options::get_option_value(&app, "priority"),
                want,
                "set-option priority {want} did not read back"
            );
        }
    });
}

#[test]
fn set_option_canonicalises_an_alias_before_storing_it() {
    with_env(None, || {
        let mut app = fresh_app();
        crate::server::options::apply_set_option(&mut app, "priority", "ABOVE_NORMAL", false);
        assert_eq!(
            crate::server::options::get_option_value(&app, "priority"),
            "above-normal"
        );
    });
}

#[test]
fn set_option_refuses_a_bad_value_and_leaves_the_previous_one() {
    with_env(None, || {
        let mut app = fresh_app();
        crate::server::options::apply_set_option(&mut app, "priority", "high", false);
        for bad in ["realtime", "idle", "bogus", "42"] {
            crate::server::options::apply_set_option(&mut app, "priority", bad, false);
            assert_eq!(
                crate::server::options::get_option_value(&app, "priority"),
                "high",
                "set-option priority {bad} should have been refused"
            );
        }
    });
}

#[test]
fn set_option_reports_the_env_override_not_the_requested_value() {
    // show-options must describe what the process is ACTUALLY running at.
    // Reporting the configured value while the env forced another one would
    // make a support conversation about #608 impossible.
    with_env(Some("normal"), || {
        let mut app = fresh_app();
        crate::server::options::apply_set_option(&mut app, "priority", "high", false);
        assert_eq!(
            crate::server::options::get_option_value(&app, "priority"),
            "normal"
        );
    });
}

#[test]
fn the_config_file_path_warns_on_a_bad_value_and_keeps_the_default() {
    // The config path is a separate match from apply_set_option, and the
    // generic catalog check only validates numbers and booleans, so a choice
    // option with no hand written arm would fall through silently.
    with_env(None, || {
        let mut app = fresh_app();
        crate::config::parse_option_value(&mut app, "priority", "realtime", true);
        assert_eq!(
            crate::server::options::get_option_value(&app, "priority"),
            DEFAULT_PRIORITY,
            "a bad config value must not change the option"
        );
        assert!(
            app.config_warnings.iter().any(|w| w.contains("priority")),
            "a bad config value must warn, got {:?}",
            app.config_warnings
        );
    });
}

#[test]
fn the_config_file_path_accepts_a_good_value() {
    with_env(None, || {
        let mut app = fresh_app();
        crate::config::parse_option_value(&mut app, "priority", "high", true);
        assert_eq!(
            crate::server::options::get_option_value(&app, "priority"),
            "high"
        );
        assert!(
            !app.config_warnings.iter().any(|w| w.contains("priority")),
            "a good config value must not warn, got {:?}",
            app.config_warnings
        );
    });
}

// ── the claim wire contract ─────────────────────────────────────────────────
//
// A warm standby sets its class at its own startup, from the environment of
// whichever server generation spawned it. That predates the claiming client, so
// `PSMUX_PRIORITY` in the user's shell can only reach the already running
// process through the claim. These pin the wire shape that carries it, because
// the failure mode was silent: the escape hatch worked on a cold spawn and did
// nothing on a warm claim, which is the path most users take.

fn tokenize(cmd: &str) -> Vec<String> {
    crate::commands::parse_command_line(cmd)
}

#[test]
fn claim_with_cwd_and_priority_keeps_both_positionals_in_place() {
    let cmd = format!(
        "claim-session {} {} -p {}",
        crate::util::quote_arg("my session"),
        crate::util::quote_arg("C:\\Users\\My Name\\Documents"),
        crate::util::quote_arg("normal")
    );
    let toks = tokenize(&cmd);
    let refs: Vec<&str> = toks.iter().skip(1).map(|s| s.as_str()).collect();
    let (pos, prio) = crate::util::parse_claim_args(&refs);
    assert_eq!(pos, ["my session", "C:\\Users\\My Name\\Documents"]);
    assert_eq!(prio.as_deref(), Some("normal"));
}

#[test]
fn claim_without_cwd_does_not_mistake_the_priority_for_a_directory() {
    // The whole reason -p is a flag. As a third positional it would land at
    // index 1 here and be applied as the client's working directory.
    let cmd = format!(
        "claim-session {} -p {}",
        crate::util::quote_arg("sess"),
        crate::util::quote_arg("high")
    );
    let toks = tokenize(&cmd);
    let refs: Vec<&str> = toks.iter().skip(1).map(|s| s.as_str()).collect();
    let (pos, prio) = crate::util::parse_claim_args(&refs);
    assert_eq!(pos, ["sess"], "the priority must not occupy the cwd slot");
    assert_eq!(pos.get(1), None, "there must be no cwd when none was sent");
    assert_eq!(prio.as_deref(), Some("high"));
}

#[test]
fn an_old_client_that_sends_no_priority_still_parses() {
    // Mixed versions: a claim with no -p must keep working and report None, so
    // the server leaves its class alone rather than guessing.
    let cmd = format!(
        "claim-session {} {}",
        crate::util::quote_arg("sess"),
        crate::util::quote_arg("D:\\Projects\\")
    );
    let toks = tokenize(&cmd);
    let refs: Vec<&str> = toks.iter().skip(1).map(|s| s.as_str()).collect();
    let (pos, prio) = crate::util::parse_claim_args(&refs);
    assert_eq!(pos, ["sess", "D:\\Projects\\"]);
    assert_eq!(prio, None);
}

#[test]
fn a_cwd_with_awkward_characters_survives_alongside_the_flag() {
    for cwd in [
        "C:\\Program Files (x86)\\App",
        "C:\\R&D\\project",
        "\\\\server\\share\\folder",
        "C:\\",
    ] {
        let cmd = format!(
            "claim-session {} {} -p {}",
            crate::util::quote_arg("s1"),
            crate::util::quote_arg(cwd),
            crate::util::quote_arg("above-normal")
        );
        let toks = tokenize(&cmd);
        let refs: Vec<&str> = toks.iter().skip(1).map(|s| s.as_str()).collect();
        let (pos, prio) = crate::util::parse_claim_args(&refs);
        assert_eq!(pos.get(1).map(|s| s.as_str()), Some(cwd), "cwd {cwd:?} was corrupted");
        assert_eq!(prio.as_deref(), Some("above-normal"));
    }
}

#[test]
fn the_value_the_client_sends_follows_the_documented_precedence() {
    // claim_priority_arg is what the three send sites put after -p. It must
    // resolve the same way the server would, so a claimed session and a cold
    // spawned one land on the same class for the same environment.
    with_env(Some("normal"), || {
        assert_eq!(crate::platform::claim_priority_arg(), "normal");
    });
    with_env(Some("high"), || {
        assert_eq!(crate::platform::claim_priority_arg(), "high");
    });
    with_env(Some("realtime"), || {
        // Unusable env falls through to config, then to the default. There is
        // no config priority in the test environment, so the default stands.
        assert_eq!(crate::platform::claim_priority_arg(), DEFAULT_PRIORITY);
    });
}

#[test]
fn a_claim_value_the_server_cannot_parse_is_ignored_not_guessed() {
    // The server arm only applies the value when normalize_priority accepts it.
    // A garbled or hostile -p must leave the class where it was.
    for bad in ["realtime", "", "idle", "42"] {
        assert!(
            normalize_priority(bad).is_none(),
            "{bad:?} must not be applied by the claim arm"
        );
    }
}

// ── the syscall itself ──────────────────────────────────────────────────────

#[test]
#[cfg(windows)]
fn setting_the_class_takes_effect_and_can_be_put_back() {
    // Runs in the test process, so it must restore what it found or every
    // later test in this binary runs at whatever class it happened to leave.
    let _lock = crate::util::lock_test_env();
    let before = current_process_priority();
    for want in ["normal", "above-normal"] {
        assert!(set_process_priority(want), "SetPriorityClass({want}) refused");
        assert_eq!(current_process_priority(), Some(want));
    }
    if let Some(b) = before {
        set_process_priority(b);
    } else {
        set_process_priority("normal");
    }
}
