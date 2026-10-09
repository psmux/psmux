// A key table set by `switch-client -T` must route the NEXT key, whichever
// route set it: a root (-n) binding, the command prompt, the CLI. tmux has one
// c->keytable that all of them write (cmd-switch-client.c:96) and that
// server_client_handle_key reads. psmux dispatches keys in the attached client,
// so it only honoured a table latched by a prefix binding the client had
// dispatched itself; every other route changed #{client_key_table} and nothing
// else, and the next key went to root.

use super::*;
use crate::types::AppState;

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

// --- the key-table session option -------------------------------------------
//
// tmux options-table.c "key-table" (session, default "root"); server-client.c
// server_client_get_key_table returns it and server_client_set_key_table(c,
// NULL) puts the client back on it after every key. A key not bound in it is
// passed to the pane without trying root; switch-client -T still overrides it
// for one key; the prefix still arms from it.

fn app_with_window() -> AppState {
    let mut app = AppState::new("kt_probe".to_string());
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: "w0".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app
}

#[test]
fn key_table_option_defaults_to_root_and_is_listed() {
    let app = AppState::new("kt".to_string());
    assert_eq!(crate::server::options::get_option_value(&app, "key-table"), "root");
    assert_eq!(crate::server::helpers::default_key_table(&app), "root");
    let listed = crate::server::option_catalog::build_option_list(&app);
    let row = listed.iter().find(|(n, _, _)| n == "key-table").expect("key-table in show-options");
    assert_eq!(row.1, "root");
    assert_eq!(crate::server::option_catalog::default_for("key-table"), Some("root"));
}

#[test]
fn key_table_option_set_at_runtime_and_from_config() {
    let mut app = AppState::new("kt".to_string());
    crate::server::options::apply_set_option(&mut app, "key-table", "mytbl", false).unwrap();
    assert_eq!(crate::server::options::get_option_value(&app, "key-table"), "mytbl");
    assert_eq!(crate::server::helpers::default_key_table(&app), "mytbl");

    let mut app2 = AppState::new("kt2".to_string());
    crate::config::parse_config_content(&mut app2, "set -g key-table cfgtbl\n");
    assert_eq!(crate::server::helpers::default_key_table(&app2), "cfgtbl");
}

#[test]
fn empty_key_table_is_root() {
    let mut app = AppState::new("kt".to_string());
    app.user_options.insert("key-table".to_string(), String::new());
    assert_eq!(crate::server::helpers::default_key_table(&app), "root");
}

#[test]
fn client_key_table_format_reports_the_default_table() {
    let mut app = app_with_window();
    assert_eq!(crate::format::expand_format("#{client_key_table}", &app), "root");
    crate::server::options::apply_set_option(&mut app, "key-table", "mytbl", false).unwrap();
    assert_eq!(crate::format::expand_format("#{client_key_table}", &app), "mytbl");
    // A one key latch still wins over the default.
    app.current_key_table = Some("other".to_string());
    assert_eq!(crate::format::expand_format("#{client_key_table}", &app), "other");
    // The prefix still wins over both.
    app.client_prefix_active = true;
    assert_eq!(crate::format::expand_format("#{client_key_table}", &app), "prefix");
}

#[test]
fn switch_client_t_resolves_against_the_default_table() {
    use crate::server::helpers::resolve_switch_client_table as resolve;
    let mut app = AppState::new("kt".to_string());
    app.user_options.insert("key-table".to_string(), "mytbl".to_string());
    // Naming the default table is the unlatched state.
    assert_eq!(resolve(&app, "mytbl"), Ok(None));
    // root always exists and, not being the default, latches for one key.
    assert_eq!(resolve(&app, "root"), Ok(Some("root".to_string())));
    assert_eq!(resolve(&app, "prefix"), Ok(Some("prefix".to_string())));
    assert!(resolve(&app, "NOSUCH").is_err());
    // With the default left at root, root is still the unlatched state.
    let plain = AppState::new("kt2".to_string());
    assert_eq!(resolve(&plain, "root"), Ok(None));
}

#[test]
fn state_frame_carries_a_non_root_default_table_only() {
    let mut buf = String::from("{\"a\":1}");
    crate::server::helpers::append_default_key_table_json("root", &mut buf);
    assert_eq!(buf, "{\"a\":1}");
    crate::server::helpers::append_default_key_table_json("mytbl", &mut buf);
    let v: serde_json::Value = serde_json::from_str(&buf).expect("valid JSON");
    assert_eq!(v["default_key_table"], "mytbl");
}

#[test]
fn client_sync_follows_the_default_table() {
    let mut s = KeyTableSync::default();
    assert_eq!(s.default_table(), "root");
    s.set_default(Some("mytbl"));
    assert_eq!(s.default_table(), "mytbl");
    // A binding that names the default table leaves nothing latched...
    assert_eq!(s.latch_for(Some("mytbl".to_string())), None);
    // ...while root is a real one key latch when it is not the default.
    assert_eq!(s.latch_for(Some("root".to_string())), Some("root".to_string()));
    s.set_default(None);
    assert_eq!(s.default_table(), "root");
    assert_eq!(s.latch_for(Some("root".to_string())), None);
}

#[test]
fn client_sync_treats_the_default_as_no_latch_from_the_server() {
    let mut s = KeyTableSync::default();
    s.set_default(Some("mytbl"));
    // The server reports a latch of root (switch-client -T root from the CLI).
    assert_eq!(s.on_server_table(Some("root")), Some(Some("root".to_string())));
    // And back to the default.
    assert_eq!(s.on_server_table(None), Some(None));
}
