// Option default parity.
//
// Contract: for every option in OPTION_CATALOG, the `default` string the catalog
// advertises must be the value a freshly constructed AppState actually reports.
//
// This matters because the catalog `default` field is not decorative. It is what
// customize-mode restores when the user resets an option (`d` in customize-mode,
// wire command `customize-reset-default`). If the catalog disagrees with the
// runtime initializer, resetting an option silently changes a value the user
// never touched, and `customize-mode` displays a default that is a lie.
//
// Claim under test: `status-right` diverges.

use super::*;
use crate::server::option_catalog::{default_for, OPTION_CATALOG};
use crate::server::options::get_option_value;

fn fresh_app() -> crate::types::AppState {
    crate::types::AppState::new("parity_probe".to_string())
}

/// Options whose catalog default legitimately cannot be compared verbatim against
/// a fresh AppState. Each entry needs a reason. This list is deliberately tiny;
/// anything added here is an admission that the option has no single truthful
/// default, so justify it or fix the option instead.
fn exempt(name: &str) -> Option<&'static str> {
    match name {
        // Resolved at read time to the first shell that exists on this machine
        // (pwsh, then powershell, then cmd), so it is host dependent by design.
        "default-shell" => Some("host dependent shell resolution"),
        "default-command" => Some("alias of default-shell, host dependent"),
        _ => None,
    }
}

/// Two options do not live in `AppState` at all: `cursor-style` and
/// `cursor-blink` are read straight out of the process environment
/// (`PSMUX_CURSOR_STYLE` / `PSMUX_CURSOR_BLINK`, server/options.rs), because
/// that is how the value reaches the renderer. So "what a fresh AppState
/// reports" for them is really "what this machine happens to export", and the
/// test measured the developer's shell rather than psmux: with
/// `PSMUX_CURSOR_STYLE=default` exported it failed with catalog `bar` against
/// a fresh AppState of `default`, on a tree with no defect in it. A test that
/// passes or fails on an environment variable it never set is not testing the
/// catalog.
///
/// `set -g cursor-style` calls `env::set_var`, so a sibling test in the same
/// binary can do it too. The fix covers both: take the shared env lock (the
/// one every env-touching test in this crate must hold) and read these
/// options with the variables removed, which is the state the catalog default
/// actually describes. Restored on the way out, panic or not, so nothing else
/// in the process notices.
struct PristineCursorEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl PristineCursorEnv {
    fn take() -> Self {
        let lock = crate::util::lock_test_env();
        let saved = ["PSMUX_CURSOR_STYLE", "PSMUX_CURSOR_BLINK"]
            .into_iter()
            .map(|name| {
                let previous = std::env::var(name).ok();
                std::env::remove_var(name);
                (name, previous)
            })
            .collect();
        Self { _lock: lock, saved }
    }
}

impl Drop for PristineCursorEnv {
    fn drop(&mut self) {
        for (name, previous) in &self.saved {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// The environment must not be able to decide this test's verdict.
#[test]
fn env_backed_options_report_their_catalog_default_on_any_machine() {
    let _pristine = PristineCursorEnv::take();
    let app = fresh_app();
    for name in ["cursor-style", "cursor-blink"] {
        let catalog = default_for(name).unwrap_or_else(|| panic!("{name} must be in the catalog"));
        assert_eq!(
            get_option_value(&app, name),
            catalog,
            "{name} is read from the process environment, so its default must be \
             what psmux falls back to with the variable unset",
        );
    }
}

#[test]
fn every_catalog_default_matches_a_fresh_appstate() {
    let _pristine = PristineCursorEnv::take();
    let app = fresh_app();
    let mut mismatches: Vec<String> = Vec::new();
    let mut compared = 0usize;

    for def in OPTION_CATALOG.iter() {
        if exempt(def.name).is_some() {
            continue;
        }
        let actual = get_option_value(&app, def.name);
        compared += 1;
        if actual != def.default {
            mismatches.push(format!(
                "\n  {}\n      catalog default : {:?}\n      fresh AppState  : {:?}",
                def.name, def.default, actual
            ));
        }
    }

    assert!(
        mismatches.is_empty(),
        "{} of {} catalog defaults disagree with a fresh AppState. \
         customize-mode reset would change these values for a user who never touched them:{}",
        mismatches.len(),
        compared,
        mismatches.join("")
    );
}

/// Focused regression guard for the specific option this was found on.
/// Kept separate so a future regression names the option directly.
#[test]
fn status_right_catalog_default_matches_runtime_default() {
    let app = fresh_app();
    let catalog = default_for("status-right").expect("status-right must be in OPTION_CATALOG");
    let runtime = get_option_value(&app, "status-right");

    assert_eq!(
        runtime, catalog,
        "status-right advertises one default and initializes another. \
         Resetting it in customize-mode replaces the real default \
         ({runtime:?}) with {catalog:?}."
    );
}

#[test]
fn pane_border_indicators_is_listed_as_a_window_option_with_global_default() {
    let app = fresh_app();
    assert_eq!(
        crate::server::options::get_window_option_value(
            &app,
            "pane-border-indicators",
        ),
        "colour",
    );
    assert!(
        crate::server::options::render_window_options(&app)
            .lines()
            .any(|line| line == "pane-border-indicators colour"),
    );
}

/// The catalog is what customize-mode renders. An option missing a default there
/// cannot be reset at all, so a blank default on a non-string option is a defect.
#[test]
fn catalog_defaults_are_present_for_scalar_options() {
    let mut missing: Vec<&str> = Vec::new();
    for def in OPTION_CATALOG.iter() {
        let scalar = matches!(
            def.option_type,
            crate::server::option_catalog::OptionType::Number(_)
                | crate::server::option_catalog::OptionType::Boolean
                | crate::server::option_catalog::OptionType::Choice(_)
        );
        if scalar && def.default.trim().is_empty() {
            missing.push(def.name);
        }
    }
    assert!(
        missing.is_empty(),
        "these non-string options have an empty catalog default, so customize-mode \
         cannot reset them: {missing:?}"
    );
}

#[test]
fn window_option_listing_preserves_its_supported_surface() {
    let app = fresh_app();
    let output = crate::server::options::render_window_options(&app);
    let actual: Vec<&str> = output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let expected = [
        "automatic-rename",
        "monitor-activity",
        "monitor-silence",
        "remain-on-exit",
        // A window option in tmux (options-table.c:1694, #923).
        "remain-on-exit-format",
        "window-status-format",
        "window-status-current-format",
        "window-status-separator",
        "window-status-style",
        "window-status-current-style",
        "window-status-activity-style",
        "window-status-bell-style",
        "window-status-last-style",
        "pane-border-indicators",
        "main-pane-width",
        "main-pane-height",
        "window-size",
    ];
    assert_eq!(actual, expected);
}

#[test]
fn window_option_lookup_preserves_its_supported_surface() {
    let app = fresh_app();
    for name in [
        "copy-mode-line-numbers",
        "copy-mode-line-number-style",
        "copy-mode-current-line-number-style",
        "aggressive-resize",
    ] {
        assert_eq!(
            crate::server::options::get_window_option_value(&app, name),
            "",
            "{name} is catalogued as window-scoped but is not exposed by show-window-options",
        );
    }
}
