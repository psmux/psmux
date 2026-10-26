// A control client's #{client_width} and #{client_height} never followed
// `refresh-client -C WxH`: the registry kept the size it was registered with,
// so after `-C 50x20` and `-C 90x25` the client still said 80x25 while the
// window became 90x25. tmux sets the client's own tty size there
// (cmd-refresh-client.c:101 tty_set_size) and a control client draws no
// status line (status.c:117 status_line_size), so its height is the height it
// set, with no status rows added.

use super::*;

fn app_with_control_client() -> AppState {
    let mut app = AppState::new("t".to_string());
    app.client_area = ratatui::layout::Rect::new(0, 0, 80, 24);
    assert!(app.register_client(7, true));
    app
}

#[test]
fn refresh_size_is_the_control_clients_own_size() {
    let mut app = app_with_control_client();
    app.set_control_client_size(7, 50, 20);
    assert_eq!(expand_format("#{client_width}x#{client_height}", &app), "50x20");
    app.set_control_client_size(7, 90, 25);
    assert_eq!(expand_format("#{client_width}x#{client_height}", &app), "90x25");
}

#[test]
fn control_client_height_adds_no_status_rows() {
    let mut app = app_with_control_client();
    app.status_visible = true;
    app.status_lines = 2;
    app.set_control_client_size(7, 90, 25);
    assert_eq!(expand_format("#{client_height}", &app), "25");
}

#[test]
fn an_unknown_client_id_changes_nothing() {
    let mut app = app_with_control_client();
    app.set_control_client_size(99, 10, 10);
    assert_eq!(app.client_registry[&7].width, 80);
    assert!(!app.client_registry.contains_key(&99));
}

#[test]
fn a_tui_client_still_counts_its_status_rows() {
    let mut app = AppState::new("t".to_string());
    app.client_area = ratatui::layout::Rect::new(0, 0, 80, 29);
    assert!(app.register_client(3, false));
    app.status_visible = true;
    app.status_lines = 1;
    assert_eq!(expand_format("#{client_height}", &app), "30");
}
