// display-popup run from a command line waits for the popup to close and
// exits with its command's status, as tmux's does (cmd-display-menu.c:696-703
// returns CMD_RETURN_WAIT; popup_free_cb hands pd->status, which starts at
// 128 + SIGHUP, to the client, popup.c:266-283 and 609).
//
// Only the last test starts a process: a popup whose command exits with 7.

use super::*;
use crate::commands::execute_command_string;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn mock_app() -> AppState {
    let mut app = AppState::new("popwait".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn waiter(app: &mut AppState) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    wait_for_popup(app, tx);
    rx
}

#[test]
fn command_line_exit_code_comes_from_the_reply() {
    assert_eq!(command_line_outcome(""), Ok(0));
    assert_eq!(command_line_outcome("\n"), Ok(0));
    assert_eq!(command_line_outcome("7\n"), Ok(7));
    assert_eq!(command_line_outcome("129"), Ok(129));
    // A Windows status past i32::MAX goes back to the OS unchanged.
    assert_eq!(command_line_outcome("3221225786").map(|c| c as u32), Ok(3221225786));
    assert_eq!(command_line_outcome("ERROR: display-popup: bad\n"), Err("display-popup: bad".to_string()));
}

#[test]
fn status_zero_sends_nothing_and_others_send_the_number() {
    assert_eq!(popup_status_reply(0), "");
    assert_eq!(popup_status_reply(7), "7");
    assert_eq!(popup_status_reply(POPUP_CLOSED_STATUS), "129");
}

#[test]
fn a_command_line_waits_only_with_a_client_and_a_running_popup() {
    let mut app = mock_app();
    // No popup at all.
    app.attached_clients = 1;
    assert!(!command_line_waits(&app));
    // A popup with no command running (it failed to start, or the in-server
    // path without a command): nothing to wait for.
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    assert!(matches!(app.mode, Mode::PopupMode { popup_pane: None, .. }));
    assert!(!command_line_waits(&app));
}

#[test]
fn every_popup_opened_gets_a_new_serial() {
    let mut app = mock_app();
    let s0 = app.popup_serial;
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    let s1 = app.popup_serial;
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    assert!(s1 != s0 && app.popup_serial != s1);
}

#[test]
fn closing_with_minus_c_answers_the_waiter_with_129() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    let rx = waiter(&mut app);
    execute_command_string(&mut app, "display-popup -C").unwrap();
    assert_eq!(rx.try_recv().as_deref(), Ok("129"));
    assert!(app.popup_waiters.is_empty());
}

#[test]
fn a_popup_opened_over_it_answers_the_first_waiter() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    let rx = waiter(&mut app);
    execute_command_string(&mut app, "display-popup -w 20 -h 6").unwrap();
    assert_eq!(rx.try_recv().as_deref(), Ok("129"));
}

#[test]
fn a_popup_gone_by_any_other_way_is_answered_on_the_next_loop() {
    let mut app = mock_app();
    execute_command_string(&mut app, "display-popup -w 30 -h 8").unwrap();
    let rx = waiter(&mut app);
    // A key or another overlay ends the popup without going through
    // display-popup.
    app.mode = Mode::Passthrough;
    settle_popup_waiters(&mut app);
    assert_eq!(rx.try_recv().as_deref(), Ok("129"));
    assert!(app.popup_waiters.is_empty());
    // Nothing waits: a no-op.
    settle_popup_waiters(&mut app);
}

#[test]
fn a_popup_with_its_command_running_keeps_its_waiter_then_reports_the_status() {
    let mut app = mock_app();
    let pane = create_popup_pane(
        "exit 7",
        None,
        8,
        30,
        app.next_pane_id,
        "popwait",
        &app.environment.clone(),
        &[],
        None,
    );
    let Some(pane) = pane else {
        eprintln!("skipped: the popup command could not be started here");
        return;
    };
    app.popup_serial += 1;
    app.mode = Mode::PopupMode {
        command: "exit 7".into(),
        output: String::new(),
        process: None,
        width: 30,
        height: 8,
        close: crate::popup::PopupClose { exit_flags: 1, ..Default::default() },
        popup_pane: Some(pane),
        scroll_offset: 0,
        look: PopupLook::titled(""),
    };
    app.attached_clients = 1;
    assert!(command_line_waits(&app));
    let rx = waiter(&mut app);
    // While the popup is open the command line keeps waiting, and the
    // status is noted once the command has exited.
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.popup_exit.is_none() && Instant::now() < deadline {
        settle_popup_waiters(&mut app);
        assert!(rx.try_recv().is_err(), "answered while the popup is open");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(app.popup_exit, Some(7));
    // The popup closes (close-on-exit, or a key): the status goes back.
    app.mode = Mode::Passthrough;
    settle_popup_waiters(&mut app);
    assert_eq!(rx.try_recv().as_deref(), Ok("7"));
}
