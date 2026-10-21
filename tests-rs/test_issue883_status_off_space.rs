// Issue #883: `status off` must give the status row back to the panes.
//
// The client reports `terminal rows - status rows` to the server as its size,
// and the server sizes every window from that. It clamped the status rows to
// at least 1, so with the status off the panes were one row short of the
// area the client draws them in and the bottom row stayed blank. tmux takes
// `status_line_size` rows away, which is 0 when the status is off
// (status.c:113, resize.c:194).

use super::*;
use crate::client::status_rows_reserved;
use crate::types::ClientInfo;

#[test]
fn status_off_reserves_no_row() {
    assert_eq!(status_rows_reserved(false, 1), 0);
    // `status 0` stores status_lines 1 with the status hidden.
    assert_eq!(status_rows_reserved(false, 0), 0);
    // A hidden multi line status still reserves nothing.
    assert_eq!(status_rows_reserved(false, 3), 0);
}

#[test]
fn status_on_reserves_its_lines() {
    assert_eq!(status_rows_reserved(true, 1), 1);
    assert_eq!(status_rows_reserved(true, 2), 2);
    assert_eq!(status_rows_reserved(true, 3), 3);
    assert_eq!(status_rows_reserved(true, 5), 5);
    // A visible status is at least one row even if the count is 0.
    assert_eq!(status_rows_reserved(true, 0), 1);
}

#[test]
fn content_rows_follow_the_status_row_count() {
    // What the client sends for a 30 row terminal.
    let rows = |vis, n| 30u16.saturating_sub(status_rows_reserved(vis, n));
    assert_eq!(rows(false, 1), 30, "status off: the panes get every row");
    assert_eq!(rows(true, 1), 29);
    assert_eq!(rows(true, 2), 28);
    assert_eq!(rows(true, 3), 27);
}

fn app_with_client(content_rows: u16) -> AppState {
    let mut app = AppState::new("t883".to_string());
    app.client_registry.insert(
        1,
        ClientInfo {
            id: 1,
            width: 120,
            height: content_rows,
            connected_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            tty_name: crate::types::client_tty_name(1, Some(4242)),
            is_control: false,
            last_session: None,
            pid: 4242,
            readonly: false,
            focused: false,
        },
    );
    app.latest_client_id = Some(1);
    app
}

#[test]
fn client_height_is_the_terminal_height_with_status_off() {
    let mut app = app_with_client(30);
    app.status_visible = false;
    app.status_lines = 1;
    assert_eq!(expand_format("#{client_height}", &app), "30");
}

#[test]
fn client_height_counts_every_status_line() {
    // The client reported 28 content rows under a 2 line status: the
    // terminal is 30 rows.
    let mut app = app_with_client(28);
    app.status_visible = true;
    app.status_lines = 2;
    assert_eq!(expand_format("#{client_height}", &app), "30");
    let mut app = app_with_client(29);
    app.status_visible = true;
    app.status_lines = 1;
    assert_eq!(expand_format("#{client_height}", &app), "30");
}

#[test]
fn status_format_reads_back_the_line_count() {
    let mut app = app_with_client(30);
    app.status_visible = false;
    assert_eq!(expand_format("#{status}", &app), "off");
    app.status_visible = true;
    app.status_lines = 1;
    assert_eq!(expand_format("#{status}", &app), "on");
    app.status_lines = 3;
    assert_eq!(expand_format("#{status}", &app), "3");
}
