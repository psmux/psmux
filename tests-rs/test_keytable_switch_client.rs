// A key table set by `switch-client -T` must route the NEXT key, whichever
// route set it: a root (-n) binding, the command prompt, the CLI. tmux has one
// c->keytable that all of them write (cmd-switch-client.c:96) and that
// server_client_handle_key reads. psmux dispatches keys in the attached client,
// so it only honoured a table latched by a prefix binding the client had
// dispatched itself; every other route changed #{client_key_table} and nothing
// else, and the next key went to root.

use super::*;

// --- root bindings latch like prefix bindings -------------------------------

#[test]
fn root_binding_switch_client_t_reports_a_latch() {
    // `bind -n F6 switch-client -T mytbl`: the client now runs root bindings
    // through dispatch_binding_commands, which must report the latch.
    match dispatch_binding_commands("switch-client -T mytbl") {
        BindingDispatch::Commands { cmds, latch } => {
            assert_eq!(cmds, vec!["switch-client -T mytbl".to_string()]);
            assert_eq!(latch.as_deref(), Some("mytbl"));
        }
        other => panic!("expected Commands, got {:?}", other),
    }
}

#[test]
fn root_binding_chain_keeps_every_command_and_the_latch() {
    match dispatch_binding_commands("set -g @a 1 \\; switch-client -T mytbl") {
        BindingDispatch::Commands { cmds, latch } => {
            assert_eq!(cmds.len(), 2, "both commands of the chain are kept: {:?}", cmds);
            assert_eq!(latch.as_deref(), Some("mytbl"));
        }
        other => panic!("expected Commands, got {:?}", other),
    }
}

#[test]
fn root_binding_without_t_latches_nothing() {
    match dispatch_binding_commands("set -g @a 1") {
        BindingDispatch::Commands { latch, .. } => assert!(latch.is_none()),
        other => panic!("expected Commands, got {:?}", other),
    }
}

// --- KeyTableSync: a table the server set is adopted -------------------------

#[test]
fn server_side_table_change_is_adopted() {
    // `psmux switch-client -T mytbl` from the CLI or the command prompt.
    let mut s = KeyTableSync::default();
    assert_eq!(s.on_server_table(None), None, "root at start, nothing to adopt");
    assert_eq!(s.on_server_table(Some("mytbl")), Some(Some("mytbl".to_string())));
    // Same value in the next frame is not a new change.
    assert_eq!(s.on_server_table(Some("mytbl")), None);
}

#[test]
fn server_side_return_to_root_is_adopted() {
    let mut s = KeyTableSync::default();
    s.on_server_table(Some("mytbl"));
    assert_eq!(s.on_server_table(None), Some(None));
}

#[test]
fn root_is_the_same_as_no_table() {
    let mut s = KeyTableSync::default();
    assert_eq!(s.on_server_table(Some("root")), None);
}

#[test]
fn echo_of_own_latch_is_not_readopted() {
    // The client latched F6 itself and told the server.
    let mut s = KeyTableSync::default();
    s.note_own_write(Some("mytbl"));
    assert_eq!(s.on_server_table(Some("mytbl")), None);
}

#[test]
fn stale_frame_does_not_clear_a_fresh_local_latch() {
    // F6 latched locally; a frame produced before the server saw it still says
    // root. That must not drop the latch.
    let mut s = KeyTableSync::default();
    s.note_own_write(Some("mytbl"));
    assert_eq!(s.on_server_table(None), None);
    assert_eq!(s.on_server_table(Some("mytbl")), None);
}

#[test]
fn fast_second_key_is_not_rearmed_by_an_in_flight_frame() {
    // F6 then x typed faster than a round trip: the client latched, then
    // consumed the latch and reset to root, before the frame reporting mytbl
    // arrived. Adopting that frame would latch mytbl again and swallow the key
    // after x.
    let mut s = KeyTableSync::default();
    s.note_own_write(Some("mytbl")); // F6
    s.note_own_write(None); // x: reset ahead of its binding
    assert_eq!(s.on_server_table(Some("mytbl")), None, "in-flight echo ignored");
    assert_eq!(s.on_server_table(None), None, "final echo ignored");
    // An external change afterwards is still adopted.
    assert_eq!(s.on_server_table(Some("other")), Some(Some("other".to_string())));
}

#[test]
fn sticky_table_rearm_is_an_echo() {
    // bind -T MOVE h select-pane -L \; switch-client -T MOVE: the client
    // resets to root then re-latches MOVE in one batch.
    let mut s = KeyTableSync::default();
    s.note_own_write(Some("MOVE"));
    s.on_server_table(Some("MOVE"));
    s.note_own_write(None);
    s.note_own_write(Some("MOVE"));
    assert_eq!(s.on_server_table(Some("MOVE")), None);
    assert_eq!(s.on_server_table(Some("MOVE")), None);
}

#[test]
fn external_change_after_own_writes_settle_is_adopted() {
    let mut s = KeyTableSync::default();
    s.note_own_write(Some("mytbl"));
    s.on_server_table(Some("mytbl"));
    s.note_own_write(None);
    s.on_server_table(None);
    assert_eq!(s.on_server_table(Some("mytbl")), Some(Some("mytbl".to_string())),
        "a later CLI switch-client -T to the same table is a new change");
}

#[test]
fn own_write_queue_is_bounded() {
    let mut s = KeyTableSync::default();
    for _ in 0..1000 {
        s.note_own_write(Some("NOSUCH"));
    }
    assert!(s.own_writes.len() <= 32);
}

// --- the server publishes its table in the state frame ----------------------

#[test]
fn state_frame_carries_the_latched_table() {
    let mut buf = String::from("{\"a\":1}");
    crate::server::helpers::append_key_table_json(Some("my\"tbl"), &mut buf);
    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(v["key_table"], "my\"tbl");
}

#[test]
fn state_frame_omits_root() {
    let mut buf = String::from("{\"a\":1}");
    crate::server::helpers::append_key_table_json(None, &mut buf);
    assert_eq!(buf, "{\"a\":1}");
}
