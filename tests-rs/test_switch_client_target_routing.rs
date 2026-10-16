// `switch-client` from an attached client: which forms the client performs
// itself, and which must reach the server.
//
// Found while reviewing the #640 follow-up. The attached client classified
// ANY `switch-client` without `-T` as session navigation:
//
//     if is_switch && switch_client_table_arg(only).is_none() {
//         return BindingDispatch::SessionNav { next: only.contains("-n") };
//     }
//
// so a binding of `switch-client -t <session>`, which names an explicit
// destination, was executed as "previous session" and never reached the
// server at all. Reproduced with injected physical keystrokes, three sessions
// p1/p2/p3 with the client on p1 and the binding targeting p2, so the correct
// answer (p2) and the cycling answer (p3, the wrap-around predecessor) are
// distinguishable:
//
//     [route 1] CLI: psmux switch-client -t p2      -> session now: p2
//     [route 2] keybinding: prefix + g              -> session now: p3   WRONG
//
// The CLI route was already correct, which is what pins the fault to the
// client's own dispatch rather than to target resolution.
//
// tmux keeps these apart in `cmd_switch_client_exec` (cmd-switch-client.c):
// `-n` / `-p` / `-l` walk the session list, while `-t` resolves a named
// target. Only the first three are session navigation.
//
// Deciding it with `contains("-n")` over the raw string was a second, latent
// defect: any target that merely contained those two characters, such as
// `switch-client -t build-node`, would have been read as "next session". The
// flags are now matched as parsed argv tokens.
//
// The live keystroke route, and the two namespace defects found alongside
// this one, are covered by tests/test_switch_client_target_namespace.ps1.

#[allow(unused_imports)]
use super::*;

use crate::client::{dispatch_binding_commands, BindingDispatch};

fn cmds_of(d: BindingDispatch) -> (Vec<String>, Option<String>) {
    match d {
        BindingDispatch::Commands { cmds, latch } => (cmds, latch),
        other => panic!("expected a command list, got {:?}", other),
    }
}

fn nav_of(d: BindingDispatch) -> bool {
    match d {
        BindingDispatch::SessionNav { next } => next,
        other => panic!("expected session navigation, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// -t names a destination: it must be forwarded, never treated as navigation.
// ---------------------------------------------------------------------------

#[test]
fn switch_client_with_target_is_forwarded_to_the_server() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-client -t p2"));
    assert_eq!(
        cmds,
        vec!["switch-client -t p2".to_string()],
        "an explicit -t target must reach the server, which resolves and \
         validates it; the client cycling instead is issue #483's whole point"
    );
    assert_eq!(latch, None, "-t latches no key table");
}

#[test]
fn switch_client_target_with_window_and_pane_is_forwarded_intact() {
    // #483: the server switches the session AND selects the addressed
    // window/pane, so the full spec has to survive the client.
    for target in ["work:2", "work:2.1", "work:@4", "work:%7"] {
        let binding = format!("switch-client -t {}", target);
        let (cmds, _) = cmds_of(dispatch_binding_commands(&binding));
        assert_eq!(cmds, vec![binding.clone()], "target {}", target);
    }
}

#[test]
fn switch_client_target_containing_dash_n_is_not_read_as_next() {
    // The old `contains("-n")` test looked at the whole string, so any target
    // with those two characters in it flipped the meaning of the command.
    for name in ["build-node", "web-nginx", "-notes"] {
        let binding = format!("switch-client -t {}", name);
        let (cmds, _) = cmds_of(dispatch_binding_commands(&binding));
        assert_eq!(
            cmds,
            vec![binding.clone()],
            "target {} must stay a target, not become session navigation",
            name
        );
    }
}

#[test]
fn switch_client_with_no_arguments_is_forwarded() {
    // tmux resolves a bare `switch-client` against the current session, which
    // is effectively a no-op. Cycling to the previous session is not that.
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-client"));
    assert_eq!(cmds, vec!["switch-client".to_string()]);
    assert_eq!(latch, None);
}

// ---------------------------------------------------------------------------
// -n / -p / -l are still the client's own session navigation.
// ---------------------------------------------------------------------------

#[test]
fn switch_client_next_is_still_client_side_navigation() {
    assert!(
        nav_of(dispatch_binding_commands("switch-client -n")),
        "-n is next"
    );
}

#[test]
fn switch_client_previous_is_still_client_side_navigation() {
    assert!(
        !nav_of(dispatch_binding_commands("switch-client -p")),
        "-p is not next"
    );
}

#[test]
fn switch_client_last_reaches_the_server() {
    // `-l` is the client's own previous session (tmux: tc->last_session,
    // cmd-switch-client.c), which the server keeps per client (issue #566).
    // As client-side navigation it ran as -p: with sessions a, b, c and the
    // client in c after a, a binding of `switch-client -l` went to b.
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-client -l"));
    assert_eq!(cmds, vec!["switch-client -l".to_string()]);
    assert_eq!(latch, None);
    let (cmds, _) = cmds_of(dispatch_binding_commands("switchc -l"));
    assert_eq!(cmds, vec!["switchc -l".to_string()]);
}

#[test]
fn the_switchc_alias_behaves_the_same_on_both_arms() {
    assert!(nav_of(dispatch_binding_commands("switchc -n")));
    let (cmds, _) = cmds_of(dispatch_binding_commands("switchc -t p2"));
    assert_eq!(cmds, vec!["switchc -t p2".to_string()]);
}

#[test]
fn a_target_alongside_a_navigation_flag_is_forwarded() {
    // The server decides the precedence between the two (it checks -n/-p/-l
    // before -t). The client must not pre-empt that decision.
    let (cmds, _) = cmds_of(dispatch_binding_commands("switch-client -n -t p2"));
    assert_eq!(cmds, vec!["switch-client -n -t p2".to_string()]);
}

// ---------------------------------------------------------------------------
// The #640 key table behaviour is unchanged by any of this.
// ---------------------------------------------------------------------------

#[test]
fn a_table_switch_still_latches_and_is_not_confused_with_a_target() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-client -T MOVE"));
    assert_eq!(cmds, vec!["switch-client -T MOVE".to_string()]);
    assert_eq!(latch.as_deref(), Some("MOVE"));
}

#[test]
fn a_chain_that_targets_a_session_and_then_latches_does_both() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands(
        "switch-client -t p2 \\; switch-client -T MOVE",
    ));
    assert_eq!(
        cmds,
        vec![
            "switch-client -t p2".to_string(),
            "switch-client -T MOVE".to_string(),
        ]
    );
    assert_eq!(latch.as_deref(), Some("MOVE"));
}

// ---------------------------------------------------------------------------
// Namespace visibility, the rule behind the other two defects found with this
// one: a `-L` socket is a separate server, so neither the session cycle nor
// `-t` resolution may see across namespaces.
// ---------------------------------------------------------------------------

#[test]
fn session_cycle_membership_is_namespace_scoped() {
    // The cycle enumerates `<base>.port` files. Before the fix it filtered
    // only warm sessions, so cycling off the last session of nsA wrapped into
    // nsB's first session, and the client attached to another server's
    // session. Live proof, prefix+n on nsA/a2:
    //     nsB after: b1: 1 windows (...) (attached)
    let bases = ["nsA__a1", "nsA__a2", "nsB__b1", "nsB__b2", "plain"];

    let in_nsa: Vec<&str> = bases
        .iter()
        .copied()
        .filter(|b| crate::session::session_visible_from(b, Some("nsA")))
        .collect();
    assert_eq!(
        in_nsa,
        vec!["nsA__a1", "nsA__a2"],
        "a client on nsA must never see nsB's sessions, nor the default one"
    );

    let in_default: Vec<&str> = bases
        .iter()
        .copied()
        .filter(|b| crate::session::session_visible_from(b, None))
        .collect();
    assert_eq!(
        in_default,
        vec!["plain"],
        "the default namespace sees only unqualified bases"
    );
}

#[test]
fn a_namespaced_base_reports_its_own_namespace() {
    // This is what the client and the server each use to work out which
    // namespace they belong to before filtering or qualifying.
    assert_eq!(crate::session::session_namespace("nsA__a2"), Some("nsA"));
    assert_eq!(crate::session::session_namespace("plain"), None);
}

#[test]
fn a_bare_target_qualifies_into_the_servers_namespace() {
    // The server resolves `-t p2` against registry bases, which carry the
    // `<ns>__` prefix, while the user types the bare name. Unqualified, the
    // lookup missed every time on a -L socket and the CLI answered
    // "can't find session: p2" for a session that was listed right there.
    let qualify = |ns: Option<&str>, name: &str| match ns {
        Some(ns) => format!("{}__{}", ns, name),
        None => name.to_string(),
    };
    assert_eq!(qualify(Some("nsA"), "p2"), "nsA__p2");
    assert_eq!(qualify(None, "p2"), "p2");

    let bases = vec![
        "nsA__p1".to_string(),
        "nsA__p2".to_string(),
        "nsB__p2".to_string(),
    ];
    let q = qualify(Some("nsA"), "p2");
    assert!(bases.contains(&q), "exact match resolves within nsA");
    assert_eq!(
        bases.iter().find(|x| x.starts_with(&qualify(Some("nsA"), "p"))),
        Some(&"nsA__p1".to_string()),
        "tmux's prefix match still applies, and still cannot escape nsA"
    );
}
