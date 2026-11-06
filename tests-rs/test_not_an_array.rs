// set-option on an element of an option that is no array.
//
// tmux refuses `set -g 'status-left[x]' v` with "not an array:
// status-left[x]" (cmd-set-option.c:295-298 at tag 3.8: an array key on an
// `@` option or on an option that is not OPTIONS_TABLE_IS_ARRAY), with every
// flag, -q and -u included, and sets nothing. psmux accepted the name and
// kept the value as text under `status-left[x]`, where nothing read it.
// show-options and formats are left alone: tmux reads the option itself
// there (`show -g 'status-left[0]'` prints status-left's value).

use crate::server::option_catalog::{
    is_valid_option_name, not_an_array_error, read_name, TMUX_OPTION_NAMES,
};

#[test]
fn an_element_of_an_option_that_is_no_array_is_refused() {
    for name in [
        "status-left[x]",
        "status-left[0]",
        "escape-time[0]",
        "mode-keys[1]",
        "remain-on-exit[0]",
        // A name tmux knows that psmux keeps as text.
        "visual-silence[0]",
        // An @ option is never an array, set or not.
        "@user[0]",
        "@u[x]",
    ] {
        assert_eq!(not_an_array_error(name), Some(format!("not an array: {}", name)), "{}", name);
    }
}

#[test]
fn arrays_hooks_and_plain_names_pass() {
    for name in [
        "status-left",
        "@user",
        "command-alias[9]",
        "codepoint-widths[0]",
        "pane-colours[1]",
        "pane-colors[1]",
        "status-format[1]",
        "terminal-features[0]",
        "terminal-overrides[foo]",
        "update-environment[20]",
        "user-keys[0]",
        // Hooks are arrays in tmux (options-table.c:246-273).
        "after-new-window[5]",
        "pane-exited[0]",
        "client-attached[x]",
        "window-zoomed[0]",
        // No element at all: options_parse refuses the name, which is
        // is_valid_option_name's business, not this one.
        "status-left[]",
        "status-left[0",
    ] {
        assert_eq!(not_an_array_error(name), None, "{}", name);
    }
}

#[test]
fn every_event_hook_is_a_tmux_option_name() {
    for name in ["alert-activity", "client-dark-theme", "pane-shell-prompt", "window-zoomed"] {
        assert!(TMUX_OPTION_NAMES.contains(&name), "{}", name);
        assert_eq!(not_an_array_error(&format!("{}[0]", name)), None, "{}", name);
    }
}

#[test]
fn set_option_refuses_it_with_every_flag() {
    for flags in ["-g", "-gq", "-ga", "-go", "-gu", "-s", "-w", "-p", "-pu"] {
        let args = [flags, "status-left[x]", "v"];
        let args: &[&str] = if flags.ends_with('u') { &args[..2] } else { &args };
        let parsed = crate::cli::parse_set_option_args(args);
        assert_eq!(
            parsed.validate(false),
            Err("not an array: status-left[x]".to_string()),
            "set {:?}",
            args
        );
    }
    let parsed = crate::cli::parse_set_option_args(&["-g", "mode-keys[0]", "vi"]);
    assert_eq!(parsed.validate(true), Err("not an array: mode-keys[0]".to_string()));
    let parsed = crate::cli::parse_set_option_args(&["-g", "@u[0]", "v"]);
    assert_eq!(parsed.validate(false), Err("not an array: @u[0]".to_string()));
}

#[test]
fn an_unknown_name_stays_an_invalid_option() {
    let parsed = crate::cli::parse_set_option_args(&["-g", "nosuch[0]", "v"]);
    assert_eq!(parsed.validate(false), Err("invalid option: nosuch[0]".to_string()));
    // -q silences that one (cmd-set-option.c:262-270), not "not an array".
    let parsed = crate::cli::parse_set_option_args(&["-gq", "nosuch[0]", "v"]);
    assert_eq!(parsed.validate(false), Ok(()));
}

#[test]
fn an_array_element_still_validates() {
    for args in [
        ["-g", "status-format[1]", "ok"],
        ["-g", "command-alias[9]", "zz=list-keys"],
        ["-g", "after-new-window[6]", "set -g @h6 yes"],
    ] {
        let parsed = crate::cli::parse_set_option_args(&args);
        assert_eq!(parsed.validate(false), Ok(()), "set {:?}", args);
    }
}

fn fresh_app() -> crate::types::AppState {
    crate::types::AppState::new("not_an_array".to_string())
}

#[test]
fn a_config_line_warns_and_sets_nothing() {
    let mut app = fresh_app();
    let before = crate::server::options::get_option_value(&app, "status-left");
    for line in [
        "set -g 'status-left[x]' fromconf",
        "set -gq 'status-left[0]' fromconf",
        "set -ga 'status-left[0]' more",
        "set -g '@u[0]' v",
    ] {
        app.config_warnings.clear();
        crate::config::parse_config_line(&mut app, line);
        let name = line.split('\'').nth(1).unwrap();
        assert!(
            app.config_warnings.iter().any(|w| w.contains(&format!("not an array: {}", name))),
            "{}: {:?}",
            line,
            app.config_warnings
        );
    }
    assert_eq!(crate::server::options::get_option_value(&app, "status-left"), before);
    assert!(
        !app.user_options.keys().any(|k| k.contains('[')),
        "{:?}",
        app.user_options.keys().collect::<Vec<_>>()
    );
}

#[test]
fn a_subscript_options_parse_refuses_is_no_option() {
    // options.c:759-784 at tag 3.8: no key, text after the `]`, no `]`.
    for name in ["status-left[]", "status-left[0]x", "status-left[0", "@u[]", "@u[0", "[0]"] {
        assert!(!is_valid_option_name(name), "{}", name);
    }
    for name in ["status-left[0]", "status-left[x]", "@u[0]", "@u"] {
        assert!(is_valid_option_name(name), "{}", name);
    }
    let parsed = crate::cli::parse_set_option_args(&["-g", "status-left[]", "v"]);
    assert_eq!(parsed.validate(false), Err("invalid option: status-left[]".to_string()));
}

#[test]
fn show_and_formats_read_the_option_itself() {
    assert_eq!(read_name("status-left[0]"), "status-left");
    assert_eq!(read_name("@u[x]"), "@u");
    assert_eq!(read_name("status-format[1]"), "status-format[1]");
    assert_eq!(read_name("after-new-window[5]"), "after-new-window[5]");
    assert_eq!(read_name("status-left"), "status-left");

    let mut app = fresh_app();
    crate::config::parse_config_line(&mut app, "set -g status-left 'left '");
    crate::config::parse_config_line(&mut app, "set -g escape-time 25");
    crate::config::parse_config_line(&mut app, "set -g @u hello");
    let get = crate::server::options::get_option_value;
    assert_eq!(get(&app, "status-left[0]"), "left ");
    assert_eq!(get(&app, "status-left[x]"), "left ");
    assert_eq!(get(&app, "escape-time[3]"), "25");
    assert_eq!(
        crate::server::options::pane_user_option(&app, None, None, "@u[0]"),
        Some("hello".to_string())
    );
    assert_eq!(
        crate::server::options::lookup_user_option(
            &app,
            "@u[0]",
            "",
            crate::server::options::UserOptionTable::Session
        ),
        Some(("hello".to_string(), false))
    );
    // A user option that is not set stays unset under any key.
    assert_eq!(crate::server::options::pane_user_option(&app, None, None, "@nope[0]"), None);
    assert_eq!(
        crate::format::expand_format_for_window("#{status-left[0]}|#{escape-time[0]}|#{@u[0]}", &app, 0),
        "left |25|hello"
    );
}

#[test]
fn a_config_line_on_an_array_element_has_no_warning() {
    let mut app = fresh_app();
    crate::config::parse_config_line(&mut app, "set -g 'status-format[1]' second");
    crate::config::parse_config_line(&mut app, "set -g @u hello");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(app.status_format.get_index(1), Some("second"));
}

#[test]
fn a_set_hook_config_line_on_an_element_of_no_array_sets_nothing() {
    // set-hook is set-option in tmux (cmd-set-option.c:65, 295-298), so
    // `set-hook -g status-left[0] cmd` is "not an array" and no hook is kept.
    let mut app = fresh_app();
    for line in [
        "set-hook -g 'status-left[0]' 'display x'",
        "set-hook -g @u[0] 'display x'",
        "set-hook -ga status-left[1] 'display x'",
        "set-hook -gu @u[0]",
    ] {
        app.config_warnings.clear();
        crate::config::parse_config_line(&mut app, line);
        assert!(
            app.config_warnings.iter().any(|w| w.contains("not an array: ")),
            "{}: {:?}",
            line,
            app.config_warnings
        );
    }
    assert!(!app.hooks.keys().any(|k| k.contains('[') && !k.starts_with("after-")), "{:?}", app.hooks.keys().collect::<Vec<_>>());
    crate::config::parse_config_line(&mut app, "set-hook -g after-new-window[3] 'display x'");
    crate::config::parse_config_line(&mut app, "set-hook -g pane-exited[2] 'display y'");
    assert!(app.hooks.contains_key("after-new-window[3]"), "{:?}", app.hooks.keys().collect::<Vec<_>>());
    assert!(app.hooks.contains_key("pane-exited[2]"), "{:?}", app.hooks.keys().collect::<Vec<_>>());
}
