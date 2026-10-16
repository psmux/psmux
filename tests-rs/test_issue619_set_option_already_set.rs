// Issue #619, the two residual set-option parity gaps found while fixing item 2.
//
// GAP A: `set-option -o <opt> <val>` on an option that is ALREADY set was
// refused in complete silence, at exit code 0, with empty stdout and empty
// stderr, on the CLI route, the raw TCP route, the config-file route and the
// in-TUI command prompt alike. tmux fails it and names the option
// (cmd-set-option.c):
//
//     /* With -o, check this option is not already set. */
//     if (!args_has(args, 'u') && args_has(args, 'o')) {
//             if (array_key == NULL)
//                     already = (o != NULL);
//             ...
//             if (already) {
//                     if (args_has(args, 'q'))
//                             goto out;
//                     cmdq_error(item, "already set: %s", argument);
//                     goto fail;
//             }
//     }
//
// so without `-q` it is `already set: <name>` and CMD_RETURN_ERROR, and with
// `-q` it is a silent success. Silence in BOTH cases makes `-o` useless for the
// one job it exists to do: a caller seeding a default it does not want to
// clobber cannot tell "I set it" from "the user already had it".
//
// GAP B: `set-option -u` did not restore the table default. tmux funnels every
// unset through options_remove_or_default (options.c ~1457):
//
//     if (o->tableentry != NULL &&
//         (oo == global_options || oo == global_s_options || oo == global_w_options))
//             options_default(oo, o->tableentry);
//     else
//             options_remove(o);
//
// so a table option goes back to its options-table default and only a user
// option, which has no table entry, is removed. psmux open coded the unset in
// three places and each copy was wrong in its own way: the server request loop
// carried a hand written restore table with a `_ => {}` catch all (so
// `set -s default-terminal xterm-256color` then `set -su default-terminal`
// still read xterm-256color, and status-left came back as `psmux:#I` where a
// fresh server reports `[#S] `), the config parser wrote an EMPTY value instead
// of the default, and the plugin drain loop only erased the explicit-set mark.
//
// Measured on the unfixed tree (eb627b4), CLI route, 42 options probed:
// 37 did not come back to the fresh-server value.
//
// The fix is one shared `reset_option_to_default` driven by OPTION_CATALOG,
// which tests-rs/test_option_default_parity.rs already pins to a freshly
// constructed AppState. The audit below iterates that catalog rather than a
// hand list, so a newly catalogued option is covered the day it is added.
//
// The live CLI, raw TCP, config-file and attached Win32 client routes are
// covered by tests/test_issue619_set_option_already_set.ps1.

#[allow(unused_imports)]
use super::*;

use crate::config::{parse_config_content, parse_config_line};
use crate::server::option_catalog::OPTION_CATALOG;
use crate::server::options::get_option_value;
use crate::types::AppState;

fn mock_app() -> AppState {
    AppState::new("srv619b".to_string())
}

// ---------------------------------------------------------------------------
// GAP A: `-o` on an option that is already set reports "already set: <name>".
// ---------------------------------------------------------------------------

#[test]
fn only_if_unset_on_a_set_option_records_already_set() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 11\nset -go escape-time 22\n");

    assert_eq!(
        app.escape_time_ms, 11,
        "the -o must not overwrite a value the user already set"
    );
    assert!(
        app.config_warnings
            .iter()
            .any(|w| w.contains("already set: escape-time")),
        "the refusal must be reported, not swallowed. warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn only_if_unset_on_a_set_user_option_records_already_set() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @i619b one\nset -go @i619b two\n");

    assert_eq!(app.user_options.get("@i619b").map(String::as_str), Some("one"));
    assert!(
        app.config_warnings
            .iter()
            .any(|w| w.contains("already set: @i619b")),
        "a @user option refusal must be reported too. warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn only_if_unset_with_quiet_is_a_silent_no_op() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 11\nset -goq escape-time 22\n");

    assert_eq!(app.escape_time_ms, 11, "-q still must not overwrite");
    assert!(
        !app.config_warnings
            .iter()
            .any(|w| w.contains("already set")),
        "tmux's `if (args_has(args, 'q')) goto out;` means -q exits 0 in silence. \
         warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn only_if_unset_on_an_unset_option_applies_and_says_nothing() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set -go escape-time 22");

    assert_eq!(app.escape_time_ms, 22, "-o on a fresh option must apply");
    assert!(
        !app.config_warnings
            .iter()
            .any(|w| w.contains("already set")),
        "an applied -o is not an error. warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn only_if_unset_after_unset_applies_and_says_nothing() {
    // The #619 sequence itself: the refusal must not come back now that it is
    // reported. `-u` clears the option, so the following `-o` finds nothing set.
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "set -g escape-time 5\nset -gu escape-time\nset -go escape-time 77\n",
    );

    assert_eq!(app.escape_time_ms, 77);
    assert!(
        !app.config_warnings
            .iter()
            .any(|w| w.contains("already set")),
        "warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn unset_in_the_same_token_disarms_the_guard_without_an_error() {
    // tmux skips the -o guard whenever -u is present, so `-guo` is an unset and
    // never an "already set" failure.
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g status-left PRE\nset -guo status-left NEVER\n");

    assert_ne!(app.status_left, "NEVER", "-u wins over -o");
    assert!(
        !app.config_warnings
            .iter()
            .any(|w| w.contains("already set")),
        "an unset is not a refused -o. warnings were {:?}",
        app.config_warnings
    );
}

#[test]
fn already_set_reaches_the_tui_as_a_status_message() {
    // The in-TUI command prompt runs commands through this same parser
    // (commands.rs routes set-option to config::parse_config_line), and a
    // command that failed has to say so on the status line rather than look
    // like it worked.
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g escape-time 11");
    app.status_message = None;
    parse_config_line(&mut app, "set -go escape-time 22");

    let msg = app
        .status_message
        .as_ref()
        .map(|(m, _, _)| m.clone())
        .unwrap_or_default();
    assert_eq!(
        msg, "already set: escape-time",
        "the command prompt must show the refusal"
    );
}

#[test]
fn already_set_with_quiet_shows_no_status_message() {
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g escape-time 11");
    app.status_message = None;
    parse_config_line(&mut app, "set -goq escape-time 22");

    assert!(
        app.status_message.is_none(),
        "-q asked for silence, got {:?}",
        app.status_message.as_ref().map(|(m, _, _)| m.clone())
    );
}

#[test]
fn already_set_is_reported_for_setw_and_the_server_scope_spelling() {
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "setw -g window-status-separator XX\nsetw -go window-status-separator YY\n",
    );
    assert_eq!(app.window_status_separator, "XX");
    assert!(app
        .config_warnings
        .iter()
        .any(|w| w.contains("already set: window-status-separator")));

    let mut app2 = mock_app();
    parse_config_content(
        &mut app2,
        "set -s default-terminal screen-256color\nset -so default-terminal tmux-256color\n",
    );
    assert_eq!(get_option_value(&app2, "default-terminal"), "screen-256color");
    assert!(app2
        .config_warnings
        .iter()
        .any(|w| w.contains("already set: default-terminal")));
}

// ---------------------------------------------------------------------------
// GAP B: `-u` restores the value a fresh server reports.
// ---------------------------------------------------------------------------

/// Options whose value cannot be compared against a fresh AppState in-process.
/// Each entry needs a reason; the list is deliberately tiny.
fn audit_exempt(name: &str) -> Option<&'static str> {
    match name {
        // Resolved at read time to the first shell that exists on this machine,
        // so it is host dependent by design (same exemption as
        // test_option_default_parity.rs).
        "default-shell" | "default-command" => Some("host dependent shell resolution"),
        // Backed by a process-global environment variable rather than AppState,
        // so a parallel test that touches PSMUX_CURSOR_* would decide the
        // result. The unset path is exercised through the catalog like any
        // other option; only the comparison is skipped.
        "cursor-style" | "cursor-blink" => Some("process-global env var, not AppState"),
        // Applying it changes the SCHEDULING CLASS of the test process itself.
        "priority" => Some("mutates the live process priority class"),
        _ => None,
    }
}

/// A value guaranteed to differ from `def.default`, so the probe really moves
/// the option before the unset is asked to move it back.
fn probe_value(def: &crate::server::option_catalog::OptionDef) -> String {
    match def.option_type {
        crate::server::option_catalog::OptionType::Number(_) => {
            let cur: i64 = def.default.trim().parse().unwrap_or(0);
            // Stay inside every bound psmux enforces (repeat-time caps at
            // 2000000) while still differing from the default.
            (cur + 7).to_string()
        }
        crate::server::option_catalog::OptionType::Boolean => {
            if def.default == "on" { "off".to_string() } else { "on".to_string() }
        }
        crate::server::option_catalog::OptionType::Choice(
            crate::server::option_catalog::ChoiceKind::PaneBorderIndicators,
        ) => {
            if def.default == "colour" { "arrows".to_string() } else { "colour".to_string() }
        }
        _ => {
            if def.default == "i619probe" {
                "i619probe2".to_string()
            } else {
                "i619probe".to_string()
            }
        }
    }
}

#[test]
fn unset_restores_the_fresh_server_value_for_every_catalog_option() {
    let _lock = crate::util::lock_test_env();
    let fresh = mock_app();

    let mut broken: Vec<String> = Vec::new();
    let mut compared = 0usize;
    let mut probed = 0usize;

    for def in OPTION_CATALOG.iter() {
        if audit_exempt(def.name).is_some() {
            continue;
        }
        let want = get_option_value(&fresh, def.name);

        // Driven through the config parser rather than by calling the restore
        // helper directly, so this measures the same route a user's .tmux.conf
        // and the in-TUI command prompt take, and so the file still compiles
        // against the unfixed tree for a before/after comparison.
        let mut app = mock_app();
        let probe = probe_value(def);
        parse_config_line(&mut app, &format!("set -g {} {}", def.name, probe));
        let moved = get_option_value(&app, def.name) != want;
        if moved {
            probed += 1;
        }

        parse_config_line(&mut app, &format!("set -gu {}", def.name));
        let got = get_option_value(&app, def.name);
        compared += 1;

        if got != want {
            broken.push(format!(
                "\n  {}\n      probe written   : {:?} (took effect: {})\n      after set -u    : {:?}\n      fresh server    : {:?}",
                def.name, probe, moved, got, want
            ));
        }
        assert!(
            !app.user_set_options.contains(def.name),
            "{}: -u must also clear the explicit-set mark, or a following -o is refused",
            def.name
        );
    }

    assert!(
        broken.is_empty(),
        "{} of {} catalogued options do not come back to the value a fresh server reports \
         after `set-option -u` ({} of them actually moved under the probe). \
         tmux restores the options-table default here (options_remove_or_default):{}",
        broken.len(),
        compared,
        probed,
        broken.join("")
    );
}

#[test]
fn the_audit_probes_a_meaningful_number_of_options() {
    // Guards the audit above against quietly degrading into a no-op if the
    // catalog is gutted or the probe values stop taking effect.
    let fresh = mock_app();
    let mut moved = 0usize;
    for def in OPTION_CATALOG.iter() {
        if audit_exempt(def.name).is_some() {
            continue;
        }
        let want = get_option_value(&fresh, def.name);
        let mut app = mock_app();
        parse_config_line(&mut app, &format!("set -g {} {}", def.name, probe_value(def)));
        if get_option_value(&app, def.name) != want {
            moved += 1;
        }
    }
    assert!(
        moved >= 25,
        "the unset audit only manages to move {} options away from their default; \
         it is supposed to span at least 25 across int, bool, string, style, window \
         and server scopes",
        moved
    );
}

#[test]
fn unset_removes_a_user_option_rather_than_defaulting_it() {
    // tmux: a @user option has no table entry, so options_remove_or_default
    // takes the options_remove branch.
    let mut app = mock_app();
    parse_config_line(&mut app, "set -g @i619audit hello");
    assert_eq!(app.user_options.get("@i619audit").map(String::as_str), Some("hello"));

    parse_config_line(&mut app, "set -gu @i619audit");
    assert!(
        app.user_options.get("@i619audit").is_none(),
        "a user option must be removed outright, not blanked in place"
    );
}

#[test]
fn unset_removes_window_style_which_has_no_catalog_default() {
    // window-style and window-active-style live only in user_options and have
    // no typed field, so removal IS the restore (#617 behaviour, preserved).
    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "set -g window-style bg=black\nset -g window-active-style bg=blue\n",
    );
    parse_config_line(&mut app, "set -gu window-style");
    parse_config_line(&mut app, "set -gu window-active-style");
    assert!(app.user_options.get("window-style").is_none());
    assert!(app.user_options.get("window-active-style").is_none());
}

#[test]
fn config_route_restores_the_default_not_an_empty_value() {
    // The config parser used to call parse_option_value(app, key, "") on -u, so
    // `set -gu escape-time` in a .tmux.conf left the old number where the CLI
    // restored the default, and `set -gu status-style` produced a styleless status bar.
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g escape-time 5\nset -gu escape-time\n");
    // The default is 10 ms since the #841 follow up (tmux 3.5+), 500 before.
    assert_eq!(app.escape_time_ms, crate::types::DEFAULT_ESCAPE_TIME_MS, "config -u must restore the default");

    let mut app2 = mock_app();
    parse_config_content(&mut app2, "set -g status-style fg=red\nset -gu status-style\n");
    assert_eq!(
        get_option_value(&app2, "status-style"),
        "bg=green,fg=black",
        "config -u must restore the stock status style, not blank it"
    );

    let mut app3 = mock_app();
    parse_config_content(&mut app3, "set -g status-left AAA\nset -gu status-left\n");
    assert_eq!(
        app3.status_left, "[#S] ",
        "the old server table restored `psmux:#I`, which no fresh server ever reports"
    );

    let mut app4 = mock_app();
    parse_config_content(
        &mut app4,
        "set -s default-terminal screen-256color\nset -su default-terminal\n",
    );
    assert_eq!(
        get_option_value(&app4, "default-terminal"),
        "xterm-256color",
        "the server scope spelling must restore too (#618 + #619)"
    );
}

#[test]
fn unset_then_only_if_unset_still_applies_after_the_default_restore() {
    // The #619 item-2 contract, re-checked now that -u writes a real default
    // instead of an empty string: the explicit-set mark must still be gone.
    for (opt, first, second, read) in [
        ("escape-time", "5", "77", "77"),
        ("status-left", "AAA", "ZZZ", "ZZZ"),
        ("history-limit", "1234", "4321", "4321"),
        ("status-style", "fg=red", "fg=blue", "fg=blue"),
    ] {
        let mut app = mock_app();
        parse_config_content(
            &mut app,
            &format!(
                "set -g {o} {f}\nset -gu {o}\nset -go {o} {s}\n",
                o = opt,
                f = first,
                s = second
            ),
        );
        assert_eq!(
            get_option_value(&app, opt),
            read,
            "{}: -go after -gu must still apply",
            opt
        );
        assert!(
            !app.config_warnings
                .iter()
                .any(|w| w.contains("already set")),
            "{}: the -o was refused; warnings {:?}",
            opt,
            app.config_warnings
        );
    }
}
