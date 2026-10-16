// Issue #839: after rename-session, the attached client kept the registry base
// it attached with. choose-buffer looked the old base's .port and .key up and
// found nothing, so prefix = showed no list; prefix ( and ) could not find the
// session among the live ones; choose-tree and choose-session no longer knew
// which row was the client's own.
//
// In tmux the client holds its session by pointer (server_client_set_session,
// server-client.c), and cmd-rename-session.c only replaces s->name on that
// same session, so a rename never changes which session a client is in. Its
// paste buffers are server wide (window_buffer_build, window-buffer.c), with
// no session lookup at all.
//
// The client now follows the live name the server puts in every frame
// (session_base_after_rename), and choose-buffer asks over the port and key
// the client attached with (fetch_buffer_entries), which a rename keeps. The
// keys end to end are in tests/test_issue839_rename_session_identity.ps1.

use super::*;

// ── the registry base follows the frame's live name ─────────────────────────

#[test]
fn a_rename_moves_the_base_to_the_new_name() {
    assert_eq!(session_base_after_rename("sesA", "sesX"), "sesX");
}

#[test]
fn a_namespaced_base_keeps_its_namespace() {
    // The frame carries the short name; the server's base is `<ns>__<name>`
    // (AppState::port_file_base).
    assert_eq!(session_base_after_rename("ns__sesA", "sesX"), "ns__sesX");
}

#[test]
fn the_namespace_is_prefixed_the_way_the_server_does_it() {
    // RenameSession builds `format!("{}__{}", socket_name, name)` without
    // looking at the name, so `ns__x` under `-L ns` lives at `ns__ns__x`.
    assert_eq!(session_base_after_rename("ns__a", "ns__x"), "ns__ns__x");
}

#[test]
fn the_same_name_leaves_the_base_alone() {
    assert_eq!(session_base_after_rename("sesA", "sesA"), "sesA");
    assert_eq!(session_base_after_rename("ns__sesA", "sesA"), "ns__sesA");
}

#[test]
fn a_case_only_rename_is_a_rename() {
    // Issue #774: the registry heals to the spelling the rename asked for.
    assert_eq!(session_base_after_rename("sesA", "SESA"), "SESA");
    assert_eq!(session_base_after_rename("ns__sesA", "SESA"), "ns__SESA");
}

#[test]
fn an_empty_name_leaves_the_base_alone() {
    assert_eq!(session_base_after_rename("sesA", ""), "sesA");
    assert_eq!(session_base_after_rename("ns__sesA", ""), "ns__sesA");
}

#[test]
fn a_warm_name_in_the_frame_leaves_the_base_alone() {
    // A standby that has not been claimed yet still reports `__warm__`.
    assert_eq!(session_base_after_rename("ns__sesA", "__warm__"), "ns__sesA");
    assert_eq!(session_base_after_rename("__warm__", "__warm__"), "__warm__");
}

#[test]
fn a_claimed_standby_takes_the_session_name() {
    // session_namespace answers None for a warm base, so the namespace of
    // `<ns>____warm__` has to come from the warm suffix.
    assert_eq!(session_base_after_rename("__warm__", "work"), "work");
    assert_eq!(session_base_after_rename("ns____warm__", "work"), "ns__work");
}

#[test]
fn the_namespace_never_changes() {
    for (base, name) in [("sesA", "b"), ("ns__sesA", "b"), ("ns__sesA", "SESA"), ("x__y", "z")] {
        let after = session_base_after_rename(base, name);
        assert_eq!(
            crate::session::session_namespace(&after),
            crate::session::session_namespace(base),
            "{} renamed to {} gave {}", base, name, after
        );
    }
}

// ── choose-buffer asks the server the client is attached to ─────────────────

/// Answer one connection with `reply`, after reading the AUTH and command
/// lines, and return the port to connect to. The stream is dropped after the
/// reply, because the multi line read goes to end of stream.
fn one_reply_server(reply: &'static str) -> u16 {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut rd = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = rd.read_line(&mut line);
            line.clear();
            let _ = rd.read_line(&mut line);
            let mut w = stream;
            let _ = w.write_all(reply.as_bytes());
        }
    });
    port
}

#[test]
fn every_buffer_line_becomes_a_row_in_order() {
    let port = one_reply_server("OK\nbuffer1: 3 bytes: \"abc\"\nBF1: 19 bytes: \"rename buffer alpha\"\n");
    let rows = fetch_buffer_entries(port, "k");
    assert_eq!(
        rows,
        vec![
            ("buffer1".to_string(), 3, "abc".to_string()),
            ("BF1".to_string(), 19, "rename buffer alpha".to_string()),
        ]
    );
}

#[test]
fn an_error_reply_gives_no_rows() {
    let port = one_reply_server("ERROR: Invalid session key\n");
    assert!(fetch_buffer_entries(port, "k").is_empty());
}

#[test]
fn no_buffers_gives_no_rows() {
    let port = one_reply_server("OK\n");
    assert!(fetch_buffer_entries(port, "k").is_empty());
}

#[test]
fn a_closed_port_gives_no_rows() {
    // Bind and drop, so nothing listens on the port any more.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    assert!(fetch_buffer_entries(port, "k").is_empty());
}
