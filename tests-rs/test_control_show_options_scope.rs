// A control client's show-options answered every named session option with
// the session's own value, whatever `-g` said, and without its name unless
// `-v` was given. After `set -g prefix C-b` and `set -t t prefix C-a`, a
// control client's `show -gv prefix` said C-a while the CLI said C-b, and
// `show -g prefix` printed a bare `C-a` where tmux prints `prefix C-b`
// (options.c options_scope_from_flags:1101, cmd-show-options.c).
//
// These tests drive the control dispatcher with a stand-in for the server
// thread that answers the session option requests the way run_server does:
// the global table holds C-b, the session overrides it with C-a.

use super::*;

const GLOBAL: &str = "C-b";
const SESSION: &str = "C-a";

/// The control reply to `show-options <args>`, and every scope the stand-in
/// was asked for (`true` = the global table).
fn show(args: &[&str], session_inherits: bool) -> (String, Vec<bool>) {
    let (request_tx, request_rx) = mpsc::channel::<CtrlReq>();
    let (response_tx, response_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut scopes = Vec::new();
        while let Ok(request) = request_rx.recv_timeout(Duration::from_secs(10)) {
            match request {
                CtrlReq::SessionOptionQuery(resp, _, global) => {
                    scopes.push(global);
                    let value = if global || session_inherits { GLOBAL } else { SESSION };
                    let _ = resp.send((value.to_string(), !global && session_inherits));
                }
                CtrlReq::ShowOptionValue(resp, _) => {
                    let value = if session_inherits { GLOBAL } else { SESSION };
                    let _ = resp.send(value.to_string());
                }
                CtrlReq::ShowOptions(resp) => {
                    let _ = resp.send(format!("prefix {}\nstatus-left SESSION_SL", SESSION));
                }
                CtrlReq::ScopeSessionListing(resp, _, global, _) => {
                    scopes.push(global);
                    let _ = resp.send(format!("prefix {}\nstatus-left GLOBAL_SL", GLOBAL));
                }
                _ => {}
            }
        }
        scopes
    });
    assert!(dispatch_control_command(
        "show-options",
        args,
        &crate::server::connection::TargetedSender::new(&request_tx, None),
        response_tx,
        None,
        false,
        None,
        0, &Default::default(), false, &Default::default(),
    ));
    let reply = response_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    drop(request_tx);
    (reply, server.join().unwrap())
}

#[test]
fn global_value_query_reads_the_global_table() {
    let (reply, scopes) = show(&["-gv", "prefix"], false);
    assert_eq!(reply, GLOBAL);
    assert_eq!(scopes, [true]);
}

#[test]
fn split_global_value_flags_read_the_global_table() {
    let (reply, _) = show(&["-g", "-v", "prefix"], false);
    assert_eq!(reply, GLOBAL);
}

#[test]
fn global_named_query_prints_the_name() {
    let (reply, _) = show(&["-g", "prefix"], false);
    assert_eq!(reply, "prefix C-b");
}

#[test]
fn session_value_query_keeps_the_session_value() {
    let (reply, scopes) = show(&["-v", "prefix"], false);
    assert_eq!(reply, SESSION);
    assert!(scopes.is_empty(), "no -g or -A: the session's own value");
}

#[test]
fn session_named_query_prints_the_name() {
    let (reply, _) = show(&["prefix"], false);
    assert_eq!(reply, "prefix C-a");
}

#[test]
fn inherited_query_marks_an_inherited_value() {
    let (reply, scopes) = show(&["-A", "prefix"], true);
    assert_eq!(reply, "prefix* C-b");
    assert_eq!(scopes, [false]);
}

#[test]
fn inherited_query_leaves_an_own_value_unmarked() {
    let (reply, _) = show(&["-A", "prefix"], false);
    assert_eq!(reply, "prefix C-a");
}

#[test]
fn global_listing_answers_for_the_global_table() {
    let (reply, scopes) = show(&["-g"], false);
    assert!(reply.contains("prefix C-b"), "{reply}");
    assert!(reply.contains("GLOBAL_SL"), "{reply}");
    assert_eq!(scopes, [true]);
}

#[test]
fn global_values_listing_answers_for_the_global_table() {
    let (reply, _) = show(&["-gv"], false);
    assert_eq!(reply, "C-b\nGLOBAL_SL");
}

#[test]
fn session_listing_is_unscoped() {
    let (reply, scopes) = show(&[], false);
    assert!(reply.contains("prefix C-a"), "{reply}");
    assert!(scopes.is_empty());
}
