// Issue #829: a pane spawned by a one-shot CLI command gets the caller's PATH
// (tmux spawn.c: an unattached client's PATH replaces the session's). The CLI
// carries it as `-e PATH=...`; the server drops it again when it is the PATH
// the pane inherits anyway, so the warm pool stays usable.
use super::*;
use std::ffi::OsStr;

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn spawning_commands_carry_the_caller_path() {
    for cmd in ["new-window", "neww", "split-window", "splitw", "split-pane", "splitp",
                "new-pane", "newp", "respawn-pane", "respawnp", "respawn-window", "respawnw"] {
        assert_eq!(
            client_path_env_value(&s(&[cmd, "-t", "x:"]), Some(OsStr::new("C:\\a;C:\\b"))),
            Some("PATH=C:\\a;C:\\b".to_string()),
            "{cmd}"
        );
    }
}

#[test]
fn non_spawning_commands_carry_nothing() {
    // tmux builds run-shell and popup environments from the session alone.
    for cmd in ["run-shell", "display-popup", "send-keys", "kill-window", "new-session"] {
        assert_eq!(client_path_env_value(&s(&[cmd]), Some(OsStr::new("C:\\a"))), None, "{cmd}");
    }
}

#[test]
fn an_explicit_e_path_is_kept() {
    let p = Some(OsStr::new("C:\\caller"));
    assert_eq!(client_path_env_value(&s(&["new-window", "-e", "PATH=C:\\mine"]), p), None);
    assert_eq!(client_path_env_value(&s(&["new-window", "-e", "Path=C:\\mine"]), p), None);
    assert_eq!(client_path_env_value(&s(&["split-window", "-ePATH=C:\\mine"]), p), None);
    // Another -e does not stop the caller's PATH.
    assert_eq!(
        client_path_env_value(&s(&["new-window", "-e", "FOO=1"]), p),
        Some("PATH=C:\\caller".to_string())
    );
    // An -e after `--` belongs to the pane's command.
    assert_eq!(
        client_path_env_value(&s(&["respawn-pane", "--", "prog", "-e", "PATH=x"]), p),
        Some("PATH=C:\\caller".to_string())
    );
}

#[test]
fn no_or_empty_caller_path_carries_nothing() {
    assert_eq!(client_path_env_value(&s(&["new-window"]), None), None);
    assert_eq!(client_path_env_value(&s(&["new-window"]), Some(OsStr::new(""))), None);
}

#[test]
fn with_client_path_inserts_after_the_name() {
    let got = with_client_path(&s(&["new-window", "-n", "w", "prog"]), Some(OsStr::new("C:\\p")));
    assert_eq!(got, s(&["new-window", "-e", "PATH=C:\\p", "-n", "w", "prog"]));
    let untouched = s(&["kill-window", "-t", "1"]);
    assert_eq!(with_client_path(&untouched, Some(OsStr::new("C:\\p"))), untouched);
}

#[test]
fn the_wire_line_keeps_a_path_ending_in_semicolon_one_argument() {
    let cmd = with_client_path(&s(&["new-window"]), Some(OsStr::new("C:\\a;C:\\b;")));
    let line = command_list_wire_line(&cmd, &[s(&["split-window"])]);
    assert_eq!(line, "new-window -e \"PATH=C:\\\\a;C:\\\\b;\" ; split-window");
}

#[test]
fn server_drops_a_path_the_pane_inherits_anyway() {
    let env = vec![("PATH".to_string(), "C:\\same".to_string())];
    assert!(crate::pane::client_path_override(&env, Some("C:\\same")).is_empty());
    // Windows names are case insensitive.
    let env = vec![("Path".to_string(), "C:\\same".to_string())];
    assert!(crate::pane::client_path_override(&env, Some("C:\\same")).is_empty());
}

#[test]
fn server_keeps_a_different_path_and_other_variables() {
    let env = vec![
        ("PATH".to_string(), "C:\\new;C:\\same".to_string()),
        ("FOO".to_string(), "C:\\same".to_string()),
    ];
    assert_eq!(crate::pane::client_path_override(&env, Some("C:\\same")), env);
    // PATH removed from the session (`set-environment -r PATH`): keep it.
    let env = vec![("PATH".to_string(), "C:\\same".to_string())];
    assert_eq!(crate::pane::client_path_override(&env, None), env);
}
