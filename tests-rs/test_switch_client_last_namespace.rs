// switch-client under -L: the server resolved -t/-n/-p/-l against the DEFAULT
// namespace's sessions. Under `-L ns` every base reads `ns__name`, so `-l`
// never found the client's last session ("no last session"), and `-n`/`-p`,
// finding no current session in the list, sent the client into the default
// namespace's first session. tmux's switch-client only sees the sessions of
// the server it runs on (session_next_session(tc->session, ...),
// `tc->last_session` in cmd-switch-client.c).

use super::*;

fn v(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

#[test]
fn last_session_is_found_among_namespaced_bases() {
    let all = v(&["ns__a", "ns__b"]);
    assert_eq!(
        resolve_switch_client('l', "", "ns__b", &all, Some("ns__a"), Some("ns")),
        Some("ns__a".to_string())
    );
}

#[test]
fn last_session_that_is_gone_or_current_is_none() {
    let all = v(&["ns__a", "ns__b"]);
    assert_eq!(resolve_switch_client('l', "", "ns__b", &all, Some("ns__gone"), Some("ns")), None);
    assert_eq!(resolve_switch_client('l', "", "ns__b", &all, Some("ns__b"), Some("ns")), None);
    assert_eq!(resolve_switch_client('l', "", "ns__b", &all, None, Some("ns")), None);
}

#[test]
fn next_and_previous_stay_in_the_list_they_are_given() {
    let all = v(&["ns__a", "ns__b", "ns__c"]);
    assert_eq!(resolve_switch_client('n', "", "ns__a", &all, None, Some("ns")), Some("ns__b".into()));
    assert_eq!(resolve_switch_client('n', "", "ns__c", &all, None, Some("ns")), Some("ns__a".into()));
    assert_eq!(resolve_switch_client('p', "", "ns__a", &all, None, Some("ns")), Some("ns__c".into()));
    assert_eq!(resolve_switch_client('p', "", "ns__b", &all, None, Some("ns")), Some("ns__a".into()));
}

#[test]
fn target_takes_the_short_name_or_the_base() {
    let all = v(&["ns__alpha", "ns__beta"]);
    assert_eq!(resolve_switch_client('t', "beta", "ns__alpha", &all, None, Some("ns")), Some("ns__beta".into()));
    assert_eq!(resolve_switch_client('t', "ns__beta", "ns__alpha", &all, None, Some("ns")), Some("ns__beta".into()));
    assert_eq!(resolve_switch_client('t', "be", "ns__alpha", &all, None, Some("ns")), Some("ns__beta".into()));
    assert_eq!(resolve_switch_client('t', "zeta", "ns__alpha", &all, None, Some("ns")), None);
}

#[test]
fn the_server_lists_its_own_namespace() {
    // The listing the handler now uses: only `ns__` bases under -L ns, only
    // unprefixed ones in the default namespace, never a warm standby.
    let _g = crate::util::lock_test_env();
    let dir = std::env::temp_dir().join(format!("psmux_scl_unit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for b in ["ns__a", "ns__b", "dflt", "other__x", "ns____warm__", "__warm__"] {
        std::fs::write(dir.join(format!("{}.port", b)), "1").unwrap();
    }
    let prev = std::env::var_os("PSMUX_DATA_DIR");
    std::env::set_var("PSMUX_DATA_DIR", &dir);
    let ns = list_session_names_ns(Some("ns"));
    let dflt = list_session_names_ns(None);
    match prev {
        Some(p) => std::env::set_var("PSMUX_DATA_DIR", p),
        None => std::env::remove_var("PSMUX_DATA_DIR"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(ns, v(&["ns__a", "ns__b"]));
    assert_eq!(dflt, v(&["dflt"]));
    // What `-n` from ns__a resolved to with the unscoped list: the default
    // namespace's session.
    assert_eq!(resolve_switch_client('n', "", "ns__a", &dflt, None, Some("ns")), Some("dflt".into()));
    assert_eq!(resolve_switch_client('n', "", "ns__a", &ns, None, Some("ns")), Some("ns__b".into()));
}

// ── choose-tree shows session names, not registry bases ─────────────────────

#[test]
fn tree_header_shows_the_session_name_under_l() {
    // tmux adds each session to the tree under s->name
    // (window_tree_build_session, window-tree.c). Under -L ns the registry
    // base reads `ns__name`.
    assert_eq!(
        crate::client::tree_session_header(Some("ns"), "ns__alpha", 2, true),
        "alpha: 2 windows (attached)"
    );
    assert_eq!(crate::client::tree_session_header(Some("ns"), "ns__beta", 1, false), "beta: 1 windows");
    assert_eq!(crate::client::tree_session_header(None, "alpha", 1, false), "alpha: 1 windows");
}

// ── the default L binding ───────────────────────────────────────────────────

#[test]
fn prefix_l_is_switch_client_last_by_default() {
    // tmux: bind -N 'Switch to the last client' L { switch-client -l }
    // (key-bindings.c). Uppercase L, distinct from l (last-window).
    let d = crate::help::PREFIX_DEFAULTS;
    assert!(d.iter().any(|(k, c)| *k == "L" && *c == "switch-client -l"));
    assert!(d.iter().any(|(k, c)| *k == "l" && *c == "last-window"));
    assert_eq!(d.iter().filter(|(k, _)| *k == "L").count(), 1);
}
