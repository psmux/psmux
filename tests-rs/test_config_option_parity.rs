// A `set -g NAME VALUE` line in a config FILE must land exactly where the same
// `set -g` typed at runtime lands. The config parser keeps its own option
// match (config::parse_option_value) beside the runtime one
// (server::options::apply_set_option); any catalog option the config match
// forgot fell through to the "unknown option" arm, which warned and parked the
// value in user_options where nothing reads it. `alternate-screen` was the
// visible case: `set -g alternate-screen off` in psmux.conf warned and left
// `show -gv alternate-screen` at `on`, while the runtime set worked.
//
// The sweep test walks the whole option catalog plus the validation only
// names, so the next option added to one match and not the other fails here.

use super::*;

fn app() -> AppState {
    AppState::new("cfg_parity_test".to_string())
}

/// A valid value different from the catalog default for every option.
/// Options whose setter changes process wide state (the scheduling class,
/// the bold-is-bright colour rewrite, the codepoint width table) are left
/// out so this test cannot disturb its neighbours.
fn non_default_value(name: &str) -> Option<&'static str> {
    Some(match name {
        "priority" | "bold-is-bright" | "codepoint-widths" => return None,
        "escape-time" => "123",
        "focus-events" => "on",
        "history-limit" => "4321",
        "alternate-screen" => "off",
        "set-clipboard" => "off",
        "default-shell" => "cmd.exe",
        "default-terminal" => "screen-256color",
        "copy-command" => "clip.exe",
        "terminal-overrides" => "xterm*:smcup@:rmcup@",
        "exit-empty" => "off",
        "prefix" => "C-a",
        "prefix2" => "C-q",
        "base-index" => "1",
        "pane-base-index" => "1",
        "display-time" => "1234",
        "display-panes-time" => "2345",
        "repeat-time" => "600",
        "mouse" => "off",
        "scroll-enter-copy-mode" => "off",
        "mouse-drag-enter-copy-mode" => "on",
        "pwsh-mouse-selection" => "on",
        "mouse-selection" => "off",
        "mouse-selection-force" => "on",
        "paste-detection" => "off",
        "mode-keys" => "vi",
        "copy-mode-line-numbers" => "relative",
        "copy-mode-line-number-style" => "fg=red",
        "copy-mode-current-line-number-style" => "fg=blue",
        "status" => "off",
        "status-position" => "top",
        "status-interval" => "7",
        "status-justify" => "centre",
        "status-left" => "L#S",
        "status-right" => "RR",
        "status-left-length" => "22",
        "status-right-length" => "33",
        "status-style" => "bg=blue",
        "status-left-style" => "fg=red",
        "status-right-style" => "fg=cyan",
        "message-style" => "bg=red",
        "message-command-style" => "bg=magenta",
        "mode-style" => "bg=cyan",
        "bell-action" => "none",
        "visual-bell" => "on",
        "activity-action" => "none",
        "silence-action" => "none",
        "monitor-silence" => "9",
        "destroy-unattached" => "on",
        "renumber-windows" => "on",
        "set-titles" => "on",
        "set-titles-string" => "X#S",
        "word-separators" => " ,",
        "allow-passthrough" => "on",
        "allow-rename" => "off",
        "allow-set-title" => "on",
        "update-environment" => "FOO BAR",
        "synchronize-panes" => "on",
        "choose-tree-preview" => "on",
        "prediction-dimming" => "on",
        "allow-predictions" => "on",
        "warm" => "off",
        "warm-pool-size" => "3",
        "cursor-style" => "block",
        "cursor-blink" => "on",
        "claude-code-fix-tty" => "off",
        "claude-code-force-interactive" => "off",
        "automatic-rename" => "off",
        "monitor-activity" => "on",
        "remain-on-exit" => "on",
        "aggressive-resize" => "on",
        "main-pane-width" => "50",
        "main-pane-height" => "40",
        "window-size" => "largest",
        "window-status-format" => "W#I",
        "window-status-current-format" => "C#I",
        "window-status-separator" => "|",
        "window-status-style" => "fg=red",
        "window-status-current-style" => "fg=blue",
        "window-status-activity-style" => "bold",
        "window-status-bell-style" => "underscore",
        "window-status-last-style" => "italics",
        "pane-border-indicators" => "arrows",
        "pane-border-style" => "fg=red",
        "pane-active-border-style" => "fg=blue",
        "pane-border-lines" => "double",
        "pane-border-hover-style" => "fg=cyan",
        "message-limit" => "77",
        "history-file-limit" => "88",
        other => panic!(
            "option '{}' has no sweep value: add one to non_default_value so the \
             config/runtime parity sweep covers it",
            other
        ),
    })
}

fn all_option_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = crate::server::option_catalog::OPTION_CATALOG
        .iter()
        .map(|d| d.name)
        .collect();
    names.extend(
        crate::server::option_catalog::VALIDATION_ONLY_OPTIONS
            .iter()
            .map(|d| d.name),
    );
    names
}

fn config_set(name: &str, value: &str) -> AppState {
    let mut a = app();
    crate::config::parse_config_content(&mut a, &format!("set -g {} \"{}\"\n", name, value));
    a
}

fn runtime_set(name: &str, value: &str) -> AppState {
    let mut a = app();
    crate::server::options::apply_set_option(&mut a, name, value, false)
        .unwrap_or_else(|e| panic!("runtime set -g {} {} refused: {}", name, value, e));
    a
}

#[test]
fn config_file_alternate_screen_off_is_applied() {
    let a = config_set("alternate-screen", "off");
    assert!(a.config_warnings.is_empty(), "unexpected warnings: {:?}", a.config_warnings);
    assert!(!a.allow_alternate_screen, "config `set -g alternate-screen off` was ignored");
    assert_eq!(crate::server::options::get_option_value(&a, "alternate-screen"), "off");
}

#[test]
fn config_file_setw_alternate_screen_off_is_applied() {
    // tmux types alternate-screen a WINDOW|PANE flag (options-table.c), so a
    // tmux config commonly spells it `setw -g`.
    let mut a = app();
    crate::config::parse_config_content(&mut a, "setw -g alternate-screen off\n");
    assert!(a.config_warnings.is_empty(), "unexpected warnings: {:?}", a.config_warnings);
    assert!(!a.allow_alternate_screen, "config `setw -g alternate-screen off` was ignored");
}

#[test]
fn config_file_alternate_screen_back_on_is_applied() {
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -g alternate-screen off\nset -g alternate-screen on\n",
    );
    assert!(a.config_warnings.is_empty(), "unexpected warnings: {:?}", a.config_warnings);
    assert!(a.allow_alternate_screen);
}

#[test]
fn config_file_validation_only_options_do_not_warn() {
    // message-limit is a real tmux server option (options-table.c); the
    // runtime set accepts both of these silently, so the config must too.
    for (name, value) in [("message-limit", "77"), ("history-file-limit", "88")] {
        let a = config_set(name, value);
        assert!(
            a.config_warnings.is_empty(),
            "config set -g {} {} warned: {:?}",
            name, value, a.config_warnings
        );
        assert_eq!(crate::server::options::get_option_value(&a, name), value);
    }
}

#[test]
fn config_file_invalid_alternate_screen_value_still_warns() {
    let a = config_set("alternate-screen", "sideways");
    assert!(
        a.config_warnings.iter().any(|w| w.contains("alternate-screen")),
        "a bad boolean must still be reported, got: {:?}",
        a.config_warnings
    );
}

#[test]
fn every_catalog_option_config_matches_runtime() {
    let mut failures = Vec::new();
    for name in all_option_names() {
        let Some(value) = non_default_value(name) else { continue };
        let cfg = config_set(name, value);
        let rt = runtime_set(name, value);
        let cfg_value = crate::server::options::get_option_value(&cfg, name);
        let rt_value = crate::server::options::get_option_value(&rt, name);
        if !cfg.config_warnings.is_empty() {
            failures.push(format!("{}: config warned {:?}", name, cfg.config_warnings));
        }
        if cfg_value != rt_value {
            failures.push(format!(
                "{}: config gave [{}] but runtime gave [{}]",
                name, cfg_value, rt_value
            ));
        }
        if rt_value != value {
            failures.push(format!("{}: runtime gave [{}], wanted [{}]", name, rt_value, value));
        }
    }
    assert!(failures.is_empty(), "config/runtime divergence:\n{}", failures.join("\n"));
}
