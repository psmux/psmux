// #798 step 3 item 1: which session of a shared server a command line, or a
// switch-client target, belongs to (src/server/shared/route.rs). Pure: no
// server is started.

use super::*;

fn refs() -> Vec<SessionRef<'static>> {
    vec![
        SessionRef { sid: 1, name: "A", base: "ns__A" },
        SessionRef { sid: 2, name: "B", base: "ns__B" },
        SessionRef { sid: 3, name: "build", base: "ns__build" },
        SessionRef { sid: 4, name: "bugs", base: "ns__bugs" },
    ]
}

#[test]
fn find_session_follows_tmux_order() {
    let r = refs();
    assert_eq!(find_session("B", &r), Some(2));
    assert_eq!(find_session("=B", &r), Some(2));
    assert_eq!(find_session("$3", &r), Some(3));
    assert_eq!(find_session("ns__bugs", &r), Some(4));
    // a unique prefix, and an ambiguous one
    assert_eq!(find_session("bui", &r), Some(3));
    assert_eq!(find_session("bu", &r), None);
    // `=` is exact only
    assert_eq!(find_session("=bui", &r), None);
    assert_eq!(find_session("nope", &r), None);
    assert_eq!(find_session("$99", &r), None);
    assert_eq!(find_session("", &r), None);
}

#[test]
fn explicit_session_targets_route_to_the_sibling() {
    let r = refs();
    assert_eq!(sibling_for_line("new-window -d -t B: -n x", 1, &r), Some(2));
    assert_eq!(sibling_for_line("rename-window -t B:0 x", 1, &r), Some(2));
    assert_eq!(sibling_for_line("send-keys -t =B:1.2 ls Enter", 1, &r), Some(2));
    assert_eq!(sibling_for_line("set -t B: @x 1", 1, &r), Some(2));
    assert_eq!(sibling_for_line("display -p -t$2: x", 1, &r), Some(2));
    assert_eq!(sibling_for_line("split-window -t '$3:1'", 1, &r), Some(3));
}

#[test]
fn own_session_and_unknown_sessions_stay() {
    let r = refs();
    assert_eq!(sibling_for_line("new-window -t A:", 1, &r), None);
    assert_eq!(sibling_for_line("new-window -t nope:", 1, &r), None);
    assert_eq!(sibling_for_line("new-window -t bu:", 1, &r), None);
    // no target, or one that names no session
    assert_eq!(sibling_for_line("new-window -d", 1, &r), None);
    assert_eq!(sibling_for_line("select-window -t :1", 1, &r), None);
    assert_eq!(sibling_for_line("select-pane -t %5", 1, &r), None);
    assert_eq!(sibling_for_line("select-window -t @5", 1, &r), None);
}

#[test]
fn a_bare_word_routes_only_for_session_target_commands() {
    let r = refs();
    assert_eq!(sibling_for_line("kill-session -t B", 1, &r), Some(2));
    assert_eq!(sibling_for_line("has-session -t B", 1, &r), Some(2));
    assert_eq!(sibling_for_line("list-windows -t B", 1, &r), Some(2));
    assert_eq!(sibling_for_line("setenv -t B FOO bar", 1, &r), Some(2));
    // a window target: B may be a window of the current session
    assert_eq!(sibling_for_line("select-window -t B", 1, &r), None);
    assert_eq!(sibling_for_line("new-window -t B", 1, &r), None);
}

#[test]
fn commands_whose_target_is_not_in_their_session_never_route() {
    let r = refs();
    for line in [
        "switch-client -t B:1",
        "detach-client -t B:1",
        "new-session -d -t B:",
        "move-window -s A:1 -t B:",
        "join-pane -s A:1 -t B:0",
        "swap-pane -s A:1 -t B:0",
        "link-window -s A:1 -t B:",
        "break-pane -t B:",
        "set-buffer -t B: x",
    ] {
        assert_eq!(sibling_for_line(line, 1, &r), None, "{line}");
    }
}

#[test]
fn lists_and_clusters_are_left_alone() {
    let r = refs();
    assert_eq!(sibling_for_line("new-window -t B: ; new-window -t A:", 1, &r), None);
    assert_eq!(sibling_for_line("new-window -dt B:", 1, &r), None);
    assert_eq!(sibling_for_line("new-window -- -t B:", 1, &r), None);
}

#[test]
fn switch_client_destination_needs_a_window_or_pane_of_a_sibling() {
    let r = refs();
    let none = |_: usize, _: IdKind, _: usize| false;
    assert_eq!(switch_client_destination("B:1", 1, &r, none), Some(2));
    assert_eq!(switch_client_destination("B:SW.1", 1, &r, none), Some(2));
    assert_eq!(switch_client_destination("=B:1", 1, &r, none), Some(2));
    // a session alone is the session's own handler's switch
    assert_eq!(switch_client_destination("B", 1, &r, none), None);
    assert_eq!(switch_client_destination("B:", 1, &r, none), None);
    // the own session selects in place
    assert_eq!(switch_client_destination("A:1", 1, &r, none), None);
    assert_eq!(switch_client_destination("nope:1", 1, &r, none), None);
}

#[test]
fn switch_client_destination_finds_the_owner_of_a_server_wide_id() {
    let r = refs();
    // pane %7 and window @3 are in B; pane %1 is in A
    let holds = |sid: usize, kind: IdKind, id: usize| match (sid, kind, id) {
        (2, IdKind::Pane, 7) | (2, IdKind::Window, 3) | (1, IdKind::Pane, 1) => true,
        _ => false,
    };
    assert_eq!(switch_client_destination("%7", 1, &r, holds), Some(2));
    assert_eq!(switch_client_destination("@3", 1, &r, holds), Some(2));
    assert_eq!(switch_client_destination("%1", 1, &r, holds), None);
    assert_eq!(switch_client_destination("%99", 1, &r, holds), None);
}

#[test]
fn within_session_drops_the_session_part() {
    assert_eq!(within_session("B:1.2"), ":1.2");
    assert_eq!(within_session("=B:SW"), ":SW");
    assert_eq!(within_session("%7"), "%7");
    assert_eq!(within_session("@3"), "@3");
}

#[test]
fn server_start_args_of_a_binding_new_session() {
    let a: Vec<String> = ["psmux", "server", "-s", "N1", "-L", "ns", "-d", "C:\\w", "-n", "win", "-x", "100", "-y", "40", "-e", "K=V", "-c", "cmd /k echo"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let (ns, req) = crate::server::shared::cli::server_start_args(&a).expect("parsed");
    assert_eq!(ns.as_deref(), Some("ns"));
    assert_eq!(req.name.as_deref(), Some("N1"));
    assert_eq!(req.start_dir.as_deref(), Some("C:\\w"));
    assert_eq!(req.window_name.as_deref(), Some("win"));
    assert_eq!((req.width, req.height), (Some(100), Some(40)));
    assert_eq!(req.env, vec![("K".to_string(), "V".to_string())]);
    assert_eq!(req.command.as_deref(), Some("cmd /k echo"));
    // the shared server itself, and a server without a session name
    let shared: Vec<String> = ["psmux", "server", "--shared-server"].iter().map(|s| s.to_string()).collect();
    assert!(crate::server::shared::cli::server_start_args(&shared).is_none());
    let other: Vec<String> = ["psmux", "new-session", "-s", "x"].iter().map(|s| s.to_string()).collect();
    assert!(crate::server::shared::cli::server_start_args(&other).is_none());
}
