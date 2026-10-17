// Issue #846: option reads must answer what tmux answers, so a program can use
// them as a probe.
//
// tmux refuses a name its options table does not know, unless it starts with
// `@`: options_match finds nothing and set-option and show-options both fail
// with "invalid option: NAME" (cmd-set-option.c:269, cmd-show-options.c:117).
// psmux stored any such name (a hyphenated one in user_options, a plain one in
// the ENVIRONMENT map) and read it back, so `show -gv extended-keys` reported
// whatever text was last written and `set -gu zzzbogusprobe` could not clear
// a plain name at all. And `#{NAME}` used a second hand written option table
// that had forgotten allow-passthrough, so the format expanded empty while
// `show -gv allow-passthrough` printed `off`.

use crate::server::option_catalog::{
    is_valid_option_name, validate_option_value, OPTION_CATALOG, TMUX_OPTION_NAMES,
};
use crate::server::options::{apply_set_option, get_option_value, reset_option_to_default};

fn fresh_app() -> crate::types::AppState {
    crate::types::AppState::new("issue846_probe".to_string())
}

/// cursor-style and cursor-blink read the process environment, which a
/// sibling test may set; hold the shared env lock and clear them.
struct CursorEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    saved: Vec<(&'static str, Option<String>)>,
}

impl CursorEnv {
    fn take() -> Self {
        let lock = crate::util::lock_test_env();
        let saved = ["PSMUX_CURSOR_STYLE", "PSMUX_CURSOR_BLINK"]
            .into_iter()
            .map(|k| (k, std::env::var(k).ok()))
            .collect();
        Self { _lock: lock, saved }
    }
}

impl Drop for CursorEnv {
    fn drop(&mut self) {
        for (k, v) in &self.saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[test]
fn every_catalog_option_formats_as_show_prints_it() {
    let _env = CursorEnv::take();
    let app = fresh_app();
    let mut mismatches = Vec::new();
    for def in OPTION_CATALOG {
        let shown = get_option_value(&app, def.name);
        let formatted = crate::format::expand_format(&format!("#{{{}}}", def.name), &app);
        let looked_up = crate::format::lookup_option_pub(def.name, &app);
        if formatted != shown || looked_up.as_deref() != Some(shown.as_str()) {
            mismatches.push(format!(
                "{}: show -gv '{}' but #{{}} '{}' (lookup {:?})",
                def.name, shown, formatted, looked_up
            ));
        }
    }
    assert!(mismatches.is_empty(), "format and show disagree:\n{}", mismatches.join("\n"));
}

#[test]
fn every_catalog_option_formats_as_show_prints_it_after_a_set() {
    // The same walk after a non default write, so an arm that only happens
    // to agree with the default is caught too.
    let _env = CursorEnv::take();
    let mut app = fresh_app();
    let mut mismatches = Vec::new();
    for def in OPTION_CATALOG {
        let value = match def.name {
            "allow-passthrough" => "on",
            "extended-keys" => "always",
            "extended-keys-format" => "csi-u",
            _ => continue,
        };
        apply_set_option(&mut app, def.name, value, false).unwrap();
        let shown = get_option_value(&app, def.name);
        let formatted = crate::format::expand_format(&format!("#{{{}}}", def.name), &app);
        if shown != value || formatted != value {
            mismatches.push(format!("{}: set {} show {} format {}", def.name, value, shown, formatted));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn allow_passthrough_format_is_not_empty() {
    let app = fresh_app();
    assert_eq!(crate::format::expand_format("#{allow-passthrough}", &app), "off");
}

#[test]
fn extended_keys_reads_the_tmux_defaults() {
    // options-table.c:402 (default_num 0 = off) and :411 (default_num 1 of
    // csi-u, xterm = xterm).
    let app = fresh_app();
    assert_eq!(get_option_value(&app, "extended-keys"), "off");
    assert_eq!(get_option_value(&app, "extended-keys-format"), "xterm");
}

#[test]
fn extended_keys_takes_only_tmux_choices() {
    for v in ["off", "on", "always"] {
        assert!(validate_option_value("extended-keys", v).is_ok(), "{}", v);
    }
    assert_eq!(
        validate_option_value("extended-keys", "yes").unwrap_err(),
        "unknown value: yes"
    );
    for v in ["csi-u", "xterm"] {
        assert!(validate_option_value("extended-keys-format", v).is_ok(), "{}", v);
    }
    assert!(validate_option_value("extended-keys-format", "kitty").is_err());
}

#[test]
fn extended_keys_set_then_unset_returns_to_off() {
    let mut app = fresh_app();
    apply_set_option(&mut app, "extended-keys", "on", false).unwrap();
    assert_eq!(get_option_value(&app, "extended-keys"), "on");
    reset_option_to_default(&mut app, "extended-keys");
    assert_eq!(get_option_value(&app, "extended-keys"), "off");
    assert!(!app.user_options.contains_key("extended-keys"));
}

#[test]
fn unknown_plain_name_is_refused_and_not_stored_in_the_environment() {
    let mut app = fresh_app();
    let err = apply_set_option(&mut app, "zzzbogusprobe", "1", false).unwrap_err();
    assert_eq!(err, "invalid option: zzzbogusprobe");
    assert!(!app.environment.contains_key("zzzbogusprobe"));
    assert!(!app.user_options.contains_key("zzzbogusprobe"));
}

#[test]
fn unknown_hyphenated_name_is_refused() {
    let mut app = fresh_app();
    let err = apply_set_option(&mut app, "bogus-hyphen-probe", "x", false).unwrap_err();
    assert_eq!(err, "invalid option: bogus-hyphen-probe");
    assert!(!app.user_options.contains_key("bogus-hyphen-probe"));
}

#[test]
fn tmux_option_psmux_does_not_implement_is_kept_as_text_never_env() {
    // `backspace` is a plain tmux name; it used to land in the environment
    // map, which every new pane receives.
    let mut app = fresh_app();
    apply_set_option(&mut app, "backspace", "C-?", false).unwrap();
    assert_eq!(get_option_value(&app, "backspace"), "C-?");
    assert!(!app.environment.contains_key("backspace"));
    reset_option_to_default(&mut app, "backspace");
    assert_eq!(get_option_value(&app, "backspace"), "");
}

#[test]
fn user_options_and_array_subscripts_stay_valid() {
    for name in ["@plugin", "@x", "status-format[1]", "command-alias[0]", "terminal-overrides[2]", "user-keys[0]"] {
        assert!(is_valid_option_name(name), "{}", name);
    }
    for name in ["zzzbogusprobe", "bogus-hyphen", "", "[0]", "status-format[1", "utf8", "mode-mouse"] {
        assert!(!is_valid_option_name(name), "{}", name);
    }
}

#[test]
fn every_catalog_and_tmux_table_name_is_valid() {
    for def in OPTION_CATALOG {
        assert!(is_valid_option_name(def.name), "{}", def.name);
    }
    for name in TMUX_OPTION_NAMES {
        assert!(is_valid_option_name(name), "{}", name);
    }
}

#[test]
fn set_option_validate_refuses_unknown_names_like_tmux() {
    let check = |args: &[&str]| crate::cli::parse_set_option_args(args).validate(false);
    assert_eq!(check(&["-g", "zzzbogusprobe", "1"]).unwrap_err(), "invalid option: zzzbogusprobe");
    assert_eq!(check(&["-gu", "zzzbogusprobe"]).unwrap_err(), "invalid option: zzzbogusprobe");
    assert_eq!(check(&["-g", "bogus-hyphen", "x"]).unwrap_err(), "invalid option: bogus-hyphen");
    // -q: tmux's silent success (cmd-set-option.c:263 `goto out`).
    assert!(check(&["-gq", "zzzbogusprobe", "1"]).is_ok());
    assert!(check(&["-g", "extended-keys", "on"]).is_ok());
    assert!(check(&["-g", "@anything", "x"]).is_ok());
    assert!(check(&["-g", "visual-silence", "on"]).is_ok());
}

#[test]
fn config_line_with_unknown_name_warns_and_stores_nothing() {
    let mut app = fresh_app();
    crate::config::parse_config_content(&mut app, "set -g zzzbogusprobe 1\nset -g bogus-hyphen x\n");
    assert!(!app.environment.contains_key("zzzbogusprobe"));
    assert!(!app.user_options.contains_key("bogus-hyphen"));
    assert!(app.config_warnings.iter().any(|w| w.contains("unknown option 'zzzbogusprobe'")),
        "{:?}", app.config_warnings);
    assert!(app.config_warnings.iter().any(|w| w.contains("unknown option 'bogus-hyphen'")),
        "{:?}", app.config_warnings);
}

#[test]
fn config_line_quiet_unknown_name_is_silent() {
    let mut app = fresh_app();
    crate::config::parse_config_content(&mut app, "set -gq zzzbogusprobe 1\n");
    assert!(!app.environment.contains_key("zzzbogusprobe"));
    assert!(!app.config_warnings.iter().any(|w| w.contains("zzzbogusprobe")), "{:?}", app.config_warnings);
}

#[test]
fn config_line_extended_keys_applies() {
    let mut app = fresh_app();
    crate::config::parse_config_content(&mut app, "set -g extended-keys on\nset -s extended-keys-format csi-u\n");
    assert_eq!(get_option_value(&app, "extended-keys"), "on");
    assert_eq!(get_option_value(&app, "extended-keys-format"), "csi-u");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}
