use super::*;

/// Helper: build a fresh AppState for testing.
fn mock_app_137() -> AppState {
    AppState::new("test_137".to_string())
}

// ── Issue #137: default-terminal must map to TERM, not leak as env var ──

/// `set -g default-terminal "xterm-256color"` must store TERM in app.environment,
/// NOT default-terminal.
#[test]
fn default_terminal_maps_to_term() {
    let mut app = mock_app_137();
    parse_config_content(&mut app, "set -g default-terminal \"xterm-256color\"\n");

    assert_eq!(
        app.environment.get("TERM").map(|s| s.as_str()),
        Some("xterm-256color"),
        "default-terminal should set TERM in environment"
    );
    assert!(
        !app.environment.contains_key("default-terminal"),
        "default-terminal should NOT appear as a raw env var key"
    );
}

/// Other values for default-terminal should also map to TERM correctly.
#[test]
fn default_terminal_other_values() {
    let mut app = mock_app_137();
    parse_config_content(&mut app, "set -g default-terminal \"screen-256color\"\n");

    assert_eq!(
        app.environment.get("TERM").map(|s| s.as_str()),
        Some("screen-256color"),
        "default-terminal value should be stored as TERM"
    );
}

// ── Hyphenated tmux options must NOT leak into app.environment ──────────

/// Options like allow-rename, terminal-overrides, activity-action, etc. must
/// NOT be stored in app.environment (they'd become invalid PowerShell $env: vars).
#[test]
fn hyphenated_options_do_not_leak_to_environment() {
    let mut app = mock_app_137();
    let config = r#"
set -g allow-rename on
set -g terminal-overrides "xterm*:Tc"
set -g activity-action other
set -g silence-action none
set -g bell-action any
set -g visual-bell off
set -g update-environment "DISPLAY SSH_AUTH_SOCK"
set -g automatic-rename on
set -g synchronize-panes off
"#;
    parse_config_content(&mut app, config);

    // None of these hyphenated option names should appear in environment
    let banned_keys = [
        "allow-rename",
        "terminal-overrides",
        "activity-action",
        "silence-action",
        "bell-action",
        "visual-bell",
        "update-environment",
        "automatic-rename",
        "synchronize-panes",
    ];
    for key in &banned_keys {
        assert!(
            !app.environment.contains_key(*key),
            "Hyphenated option '{}' must NOT be in app.environment",
            key
        );
    }
}

/// Combined config: default-terminal + hyphenated options. Only TERM should
/// appear in environment.
#[test]
fn combined_config_only_term_in_environment() {
    let mut app = mock_app_137();
    let config = r#"
set -g default-terminal "xterm-256color"
set -g allow-rename on
set -g terminal-overrides "xterm*:Tc"
set -g activity-action other
set-environment MY_CUSTOM_VAR hello
set-environment EDITOR vim
"#;
    parse_config_content(&mut app, config);

    // TERM should be set from default-terminal
    assert_eq!(
        app.environment.get("TERM").map(|s| s.as_str()),
        Some("xterm-256color"),
    );
    // User-defined env vars should be present
    assert_eq!(
        app.environment.get("MY_CUSTOM_VAR").map(|s| s.as_str()),
        Some("hello"),
    );
    assert_eq!(
        app.environment.get("EDITOR").map(|s| s.as_str()),
        Some("vim"),
    );
    // Hyphenated options must NOT leak
    assert!(!app.environment.contains_key("allow-rename"));
    assert!(!app.environment.contains_key("terminal-overrides"));
    assert!(!app.environment.contains_key("activity-action"));
}

/// Env var keys that contain hyphens from any source should be rejected.
/// This is a safety check: even if some code path tries to set a hyphenated
/// key, app.environment should only contain valid identifiers.
#[test]
fn environment_has_no_hyphenated_keys_after_full_config() {
    let mut app = mock_app_137();
    let config = r##"
set -g default-terminal "xterm-256color"
set -g allow-rename on
set -g terminal-overrides "xterm*:Tc"
set -g status-keys vi
set -g clock-mode-colour blue
set -g pane-border-format "#{pane_index}"
set -g window-style "default"
set -g wrap-search on
set-environment -g VALID_KEY some_value
"##;
    parse_config_content(&mut app, config);

    for (key, _) in &app.environment {
        assert!(
            !key.contains('-'),
            "Environment key '{}' contains a hyphen; this would cause a PowerShell ParserError when injected as $env:{}",
            key,
            key,
        );
    }
}
