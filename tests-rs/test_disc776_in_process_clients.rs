// Discussion #776: the in process command path (config lines, hooks,
// display-menu items, confirm-before) answered `list-clients` with one made up
// `/dev/pts/0` row and `choose-client` with a status line claiming psmux has a
// single client model. psmux registers every attached client in
// `client_registry`, and the wire path already lists them all (#724).

use super::*;
use crate::types::ClientInfo;

fn client(id: u64, pid: u32) -> ClientInfo {
    ClientInfo {
        id,
        width: 120,
        height: 29,
        connected_at: std::time::Instant::now(),
        last_activity: std::time::Instant::now(),
        tty_name: crate::types::client_tty_name(id, Some(pid)),
        is_control: false,
        last_session: None,
        pid,
        readonly: false,
        focused: false,
    }
}

fn app_with_clients(pids: &[u32]) -> AppState {
    let mut app = AppState::new("work".to_string());
    for (i, pid) in pids.iter().enumerate() {
        let id = i as u64 + 1;
        app.client_registry.insert(id, client(id, *pid));
    }
    app
}

#[test]
fn in_process_list_clients_lists_every_registered_client() {
    let app = app_with_clients(&[4100, 4200]);
    let out = generate_list_clients(&app);
    let rows: Vec<&str> = out.lines().collect();
    assert_eq!(rows.len(), 2, "one row per client: {out:?}");
    assert!(rows[0].starts_with("/dev/pts/4100:"), "{out:?}");
    assert!(rows[1].starts_with("/dev/pts/4200:"), "{out:?}");
    assert!(!out.contains("/dev/pts/0:"), "no made up client: {out:?}");
}

#[test]
fn in_process_list_clients_matches_the_wire_rows() {
    let app = app_with_clients(&[777]);
    let wire = crate::format::format_list_clients(
        &app, crate::format::default_list_clients_format(), None);
    assert_eq!(generate_list_clients(&app), wire);
}

#[test]
fn in_process_list_clients_is_empty_with_no_client() {
    // tmux prints nothing for a session with no attached clients (#434).
    let app = AppState::new("work".to_string());
    assert_eq!(generate_list_clients(&app), "");
}

#[test]
fn in_process_choose_client_shows_the_clients_not_a_single_client_claim() {
    let mut app = app_with_clients(&[5100, 5200]);
    execute_command_string(&mut app, "choose-client").unwrap();
    let msg = app.status_message.as_ref().map(|m| m.0.clone()).unwrap_or_default();
    assert!(!msg.contains("single-client"), "status line: {msg:?}");
    match &app.mode {
        Mode::PopupMode { command, output, .. } => {
            assert_eq!(command, "choose-client");
            assert!(output.contains("/dev/pts/5100:") && output.contains("/dev/pts/5200:"), "{output:?}");
        }
        _ => panic!("choose-client should open the client list popup"),
    }
}
