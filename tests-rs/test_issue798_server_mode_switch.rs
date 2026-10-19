// Issue #798 step 1: the server mode switch. Pure parser tests only; nothing
// here reads the process environment, a config file or starts a server.

use super::*;

#[test]
fn unset_and_empty_mean_per_session() {
    assert_eq!(resolve(None, None), Ok(ServerMode::PerSession));
    assert_eq!(resolve(Some(""), None), Ok(ServerMode::PerSession));
    assert_eq!(resolve(Some("   "), None), Ok(ServerMode::PerSession));
    assert_eq!(parse_value(""), Ok(None));
    assert_eq!(parse_value(" \t"), Ok(None));
}

#[test]
fn accepted_values_ignore_case_and_spaces() {
    assert_eq!(parse_value("per-session"), Ok(Some(ServerMode::PerSession)));
    assert_eq!(parse_value(" Per-Session "), Ok(Some(ServerMode::PerSession)));
    assert_eq!(parse_value("shared"), Ok(Some(ServerMode::Shared)));
    assert_eq!(parse_value("SHARED"), Ok(Some(ServerMode::Shared)));
    assert_eq!(parse_value("\"shared\""), Ok(Some(ServerMode::Shared)));
}

#[test]
fn unknown_values_are_errors_never_a_fallback() {
    for bad in ["bogus", "per_session", "persession", "on", "1", "share", "shared2"] {
        assert_eq!(parse_value(bad), Err(()), "{bad}");
        let err = resolve(Some(bad), None).unwrap_err();
        assert!(err.starts_with("PSMUX_SERVER_MODE: unknown value"), "{err}");
        assert!(err.contains(bad), "{err}");
        assert!(err.contains("per-session or shared"), "{err}");
    }
}

#[test]
fn env_outranks_config() {
    let cfg = Some(("cfg", "set -s server-mode shared\n"));
    assert_eq!(resolve(Some("per-session"), cfg), Ok(ServerMode::PerSession));
    let cfg = Some(("cfg", "set -s server-mode per-session\n"));
    assert_eq!(resolve(Some("shared"), cfg), Ok(ServerMode::Shared));
    // An empty env value is unset, so the config decides.
    let cfg = Some(("cfg", "set -s server-mode shared\n"));
    assert_eq!(resolve(Some(""), cfg), Ok(ServerMode::Shared));
    // A bad env value is an error even when the config is fine.
    let cfg = Some(("cfg", "set -s server-mode per-session\n"));
    assert!(resolve(Some("bogus"), cfg).is_err());
}

#[test]
fn config_forms() {
    let m = |t: &str| mode_from_config_text(t, "cfg");
    assert_eq!(m(""), Ok(None));
    assert_eq!(m("set -g mouse on\n"), Ok(None));
    assert_eq!(m("set -s server-mode shared"), Ok(Some(ServerMode::Shared)));
    assert_eq!(m("set-option -s server-mode shared"), Ok(Some(ServerMode::Shared)));
    assert_eq!(m("set server-mode shared"), Ok(Some(ServerMode::Shared)));
    assert_eq!(m("  set -sq server-mode 'shared'  "), Ok(Some(ServerMode::Shared)));
    assert_eq!(m("set -s server-mode shared # trailing"), Ok(Some(ServerMode::Shared)));
    assert_eq!(m("# set -s server-mode shared"), Ok(None));
    assert_eq!(m("set -g mouse on ; set -s server-mode shared"), Ok(Some(ServerMode::Shared)));
    // Last line wins, as when the file is applied top to bottom.
    assert_eq!(
        m("set -s server-mode shared\nset -s server-mode per-session\n"),
        Ok(Some(ServerMode::PerSession))
    );
    assert_eq!(
        m("set -s server-mode shared\nset -su server-mode\n"),
        Ok(Some(ServerMode::PerSession))
    );
    // A similar name is not this option.
    assert_eq!(m("set -s server-mode-x shared"), Ok(None));
    assert_eq!(m("set -g @server-mode shared"), Ok(None));
}

#[test]
fn config_bad_value_is_an_error_naming_the_file() {
    let err = mode_from_config_text("set -s server-mode bogus\n", "C:/x/.psmux.conf").unwrap_err();
    assert!(err.contains("server-mode: unknown value 'bogus'"), "{err}");
    assert!(err.contains("C:/x/.psmux.conf"), "{err}");
    assert!(mode_from_config_text("set -s server-mode\n", "cfg").is_err());
    assert!(resolve(None, Some(("cfg", "set -s server-mode nope"))).is_err());
}

#[test]
fn running_per_session_server_only_accepts_its_own_mode() {
    assert_eq!(set_on_running_per_session_server("per-session"), Ok(()));
    assert_eq!(set_on_running_per_session_server(" PER-SESSION "), Ok(()));
    let err = set_on_running_per_session_server("shared").unwrap_err();
    assert!(err.contains("cannot change on a running server"), "{err}");
    assert!(set_on_running_per_session_server("bogus").unwrap_err().contains("unknown value"));
    assert!(set_on_running_per_session_server("").is_err());
    assert_eq!(RUNNING_PER_SESSION, ServerMode::PerSession.as_str());
}

#[test]
fn option_is_known_but_not_listed() {
    use crate::server::option_catalog as cat;
    assert!(cat::is_valid_option_name("server-mode"));
    assert!(cat::is_known_option("server-mode"));
    // `show -s` and the customize list are unchanged: it is not a catalog entry.
    assert!(!cat::server_option_names().contains(&"server-mode"));
    assert!(cat::option_definition("server-mode").is_none());
    assert!(!cat::is_server_option("server-mode"));
}
