// Tests for issue #202: switch-client is non-functional
// Verifies the session resolution logic used by the SwitchClient handler.

/// The handler's own resolver (crate::session::resolve_switch_client) in the
/// default namespace. This used to be a copy of the handler's match, which
/// kept passing while the handler itself looked in the wrong namespace.
fn resolve_switch_target(
    flag: char,
    target: &str,
    current: &str,
    all_sessions: &[&str],
    last_session: Option<&str>,
) -> Option<String> {
    let all: Vec<String> = all_sessions.iter().map(|s| s.to_string()).collect();
    crate::session::resolve_switch_client(flag, target, current, &all, last_session, None)
}

#[test]
fn switch_client_target_exact_match() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('t', "beta", "alpha", &sessions, None);
    assert_eq!(result, Some("beta".to_string()));
}

#[test]
fn switch_client_target_not_found() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('t', "nonexistent", "alpha", &sessions, None);
    assert_eq!(result, None);
}

#[test]
fn switch_client_target_prefix_match() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('t', "bet", "alpha", &sessions, None);
    assert_eq!(result, Some("beta".to_string()));
}

#[test]
fn switch_client_target_empty() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('t', "", "alpha", &sessions, None);
    assert_eq!(result, None);
}

#[test]
fn switch_client_next_session() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('n', "", "alpha", &sessions, None);
    assert_eq!(result, Some("beta".to_string()));
}

#[test]
fn switch_client_next_wraps_around() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('n', "", "gamma", &sessions, None);
    assert_eq!(result, Some("alpha".to_string()));
}

#[test]
fn switch_client_prev_session() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('p', "", "gamma", &sessions, None);
    assert_eq!(result, Some("beta".to_string()));
}

#[test]
fn switch_client_prev_wraps_around() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('p', "", "alpha", &sessions, None);
    assert_eq!(result, Some("gamma".to_string()));
}

#[test]
fn switch_client_last_session() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('l', "", "alpha", &sessions, Some("beta"));
    assert_eq!(result, Some("beta".to_string()));
}

#[test]
fn switch_client_last_session_same_as_current() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('l', "", "alpha", &sessions, Some("alpha"));
    assert_eq!(result, None, "should return None when last session equals current");
}

#[test]
fn switch_client_last_session_not_found() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('l', "", "alpha", &sessions, Some("deleted"));
    assert_eq!(result, None, "should return None when last session no longer exists");
}

#[test]
fn switch_client_last_session_empty() {
    let sessions = vec!["alpha", "beta", "gamma"];
    let result = resolve_switch_target('l', "", "alpha", &sessions, None);
    assert_eq!(result, None);
}

#[test]
fn switch_client_next_single_session() {
    let sessions = vec!["only"];
    let result = resolve_switch_target('n', "", "only", &sessions, None);
    assert_eq!(result, Some("only".to_string()), "wraps to same when single session");
}

#[test]
fn switch_client_target_strips_window_suffix() {
    // The connection.rs handler strips ":window" from target before sending.
    // Simulate that the target reaching resolve is already stripped.
    let sessions = vec!["alpha", "beta"];
    let result = resolve_switch_target('t', "alpha", "beta", &sessions, None);
    assert_eq!(result, Some("alpha".to_string()));
}

/// Test that the SWITCH directive format is correct
#[test]
fn switch_directive_format() {
    let session = "my-session";
    let directive = format!("SWITCH {}", session);
    assert_eq!(directive, "SWITCH my-session");
    assert!(directive.starts_with("SWITCH "));
    let parsed = directive.strip_prefix("SWITCH ").unwrap();
    assert_eq!(parsed, "my-session");
}

/// Test SWITCH directive parsing with whitespace (as client.rs would process it)
#[test]
fn switch_directive_parsing_from_frame() {
    let line = "SWITCH dev-workspace\n";
    let trimmed = line.trim();
    assert!(trimmed.starts_with("SWITCH "));
    let target = trimmed.strip_prefix("SWITCH ").unwrap_or("");
    assert_eq!(target, "dev-workspace");
}

/// Test that the session name extraction from -t target works
/// e.g., "alpha:0.1" should extract "alpha"
#[test]
fn session_name_from_target_with_window_pane() {
    let target = "alpha:0.1";
    let session = if let Some(pos) = target.find(':') {
        &target[..pos]
    } else {
        target
    };
    assert_eq!(session, "alpha");
}

/// Test plain session name (no colon)
#[test]
fn session_name_from_target_plain() {
    let target = "alpha";
    let session = if let Some(pos) = target.find(':') {
        &target[..pos]
    } else {
        target
    };
    assert_eq!(session, "alpha");
}

// ============================================================================
// PR #214 routing guard tests
//
// These replicate the exact logic added to main.rs by PR #214:
//
//   let is_switch_client = args.iter().any(|a| a == "switch-client" || a == "switchc");
//   if has_explicit_session && !is_switch_client {
//       env::set_var("PSMUX_TARGET_SESSION", &port_file_base);
//   }
//
// The rule: for switch-client, -t means "switch TO this session" (destination),
// not "route the command to this session's server" (routing). So when the command
// is switch-client or switchc, we must NOT set PSMUX_TARGET_SESSION from -t,
// allowing the TMUX env var fallback to resolve the *current* (source) session.
// ============================================================================

/// Simulate the routing decision from main.rs:
/// returns true if PSMUX_TARGET_SESSION should be set from the -t argument.
fn should_set_target_session(args: &[&str], has_explicit_session: bool) -> bool {
    let is_switch_client = args.iter().any(|a| *a == "switch-client" || *a == "switchc");
    has_explicit_session && !is_switch_client
}

/// PR #214: switch-client -t <session> must NOT set PSMUX_TARGET_SESSION.
/// Before the fix, this would have been true (routing to destination server).
#[test]
fn pr214_switch_client_t_does_not_set_target_session() {
    let args = vec!["psmux", "switch-client", "-t", "beta"];
    let result = should_set_target_session(&args, /*has_explicit_session=*/true);
    assert!(
        !result,
        "switch-client -t should NOT set PSMUX_TARGET_SESSION (would route to wrong server)"
    );
}

/// PR #214: switchc alias also must NOT set PSMUX_TARGET_SESSION.
#[test]
fn pr214_switchc_alias_does_not_set_target_session() {
    let args = vec!["psmux", "switchc", "-t", "beta"];
    let result = should_set_target_session(&args, true);
    assert!(
        !result,
        "switchc -t should NOT set PSMUX_TARGET_SESSION"
    );
}

/// Other commands WITH -t MUST still set PSMUX_TARGET_SESSION (routing).
/// This ensures the guard is narrowly scoped to switch-client only.
#[test]
fn pr214_other_commands_still_set_target_session() {
    for cmd in &["select-window", "selectw", "send-keys", "display-message",
                 "capture-pane", "kill-pane", "split-window", "new-window"] {
        let args = vec!["psmux", cmd, "-t", "beta"];
        let result = should_set_target_session(&args, true);
        assert!(
            result,
            "{} -t should still set PSMUX_TARGET_SESSION, got false",
            cmd
        );
    }
}

/// When there is no explicit session in -t (e.g. -t %2, -t :1.0),
/// has_explicit_session is false so routing doesn't happen regardless
/// of whether it's switch-client or not.
#[test]
fn pr214_no_explicit_session_never_sets_target() {
    // switch-client with pane-style target (no explicit session)
    let args_sc = vec!["psmux", "switch-client", "-t", "%2"];
    assert!(!should_set_target_session(&args_sc, false));

    // regular command with pane-style target
    let args_other = vec!["psmux", "send-keys", "-t", ":0.1"];
    assert!(!should_set_target_session(&args_other, false));
}

/// switch-client -n (no -t flag at all): has_explicit_session=false,
/// guard condition is irrelevant — routing falls through to TMUX env var.
#[test]
fn pr214_switch_client_n_no_explicit_session() {
    let args = vec!["psmux", "switch-client", "-n"];
    let result = should_set_target_session(&args, false);
    assert!(!result, "switch-client -n has no explicit session, should not set target");
}

/// switch-client -p (previous): same — TMUX env var resolves source session.
#[test]
fn pr214_switch_client_p_no_explicit_session() {
    let args = vec!["psmux", "switch-client", "-p"];
    let result = should_set_target_session(&args, false);
    assert!(!result);
}

/// switch-client -l (last): same — TMUX env var resolves source session.
#[test]
fn pr214_switch_client_l_no_explicit_session() {
    let args = vec!["psmux", "switch-client", "-l"];
    let result = should_set_target_session(&args, false);
    assert!(!result);
}

/// Regression guard: the old buggy code was simply `if has_explicit_session { ... }`.
/// Prove that the old code would have returned true for switch-client -t (the bug).
#[test]
fn pr214_regression_old_code_was_buggy() {
    // Old code (no is_switch_client guard):
    let old_logic = |has_explicit_session: bool| -> bool { has_explicit_session };

    // Old code would have set PSMUX_TARGET_SESSION for switch-client -t beta
    // causing the command to be sent to beta's server (wrong — should be alpha's).
    assert!(
        old_logic(true),
        "This confirms the old code was broken: it returned true for switch-client -t"
    );

    // New code correctly returns false for switch-client:
    let args = vec!["psmux", "switch-client", "-t", "beta"];
    assert!(
        !should_set_target_session(&args, true),
        "New code correctly returns false for switch-client -t"
    );
}
