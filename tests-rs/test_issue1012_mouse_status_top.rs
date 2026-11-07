// Issue #1012: with `status-position top`, mouse RELEASE and DRAG reached the
// pane one row below the PRESS (two rows with `status 2`) (dhaern).
//
// MEASURED on master 338ecf92, attached 80x24 client, real MOUSE_EVENT
// records, SGR echo child; press at screen (10,5), drag to (10,6):
//
//   status-position top     press <ESC>[<0;11;5M  release <ESC>[<0;11;6m
//                           drag  <ESC>[<32;11;7M release <ESC>[<0;11;7m
//   status 2, top           press <ESC>[<0;11;4M  release <ESC>[<0;11;6m
//   tmux 3.4, top           press ;5  release ;5  drag ;6  release ;6
//
// WHY. The client converts a press against pane rects that already sit below
// a top status bar and sends `pane-mouse`. Release, drag, wheel and motion go
// out as RAW screen rows (`mouse-up X Y`, ...), and the server mapped them
// against a window area that starts at row 0, so the status rows above the
// window were never taken off. `map_client_coords` now subtracts them first.

use super::{map_client_coords, window_row_from_screen};
use crate::types::AppState;

#[test]
fn bottom_status_leaves_the_row_alone() {
    assert_eq!(window_row_from_screen(5, true, 1, "bottom"), 5);
    assert_eq!(window_row_from_screen(5, true, 2, "bottom"), 5);
}

#[test]
fn top_status_takes_its_rows_off() {
    assert_eq!(window_row_from_screen(5, true, 1, "top"), 4);
    assert_eq!(window_row_from_screen(5, true, 2, "top"), 3);
    assert_eq!(window_row_from_screen(5, true, 5, "top"), 0);
}

#[test]
fn hidden_status_reserves_nothing_even_on_top() {
    assert_eq!(window_row_from_screen(5, false, 1, "top"), 5);
    assert_eq!(window_row_from_screen(5, false, 2, "top"), 5);
}

#[test]
fn a_row_on_the_top_status_bar_floors_at_zero() {
    assert_eq!(window_row_from_screen(0, true, 1, "top"), 0);
    assert_eq!(window_row_from_screen(1, true, 2, "top"), 0);
}

#[test]
fn map_client_coords_applies_the_top_status_offset() {
    let mut app = AppState::new("issue1012".to_string());
    app.status_visible = true;
    app.status_lines = 1;
    app.status_position = "bottom".to_string();
    assert_eq!(map_client_coords(&app, 10, 5), (10, 5));
    app.status_position = "top".to_string();
    assert_eq!(map_client_coords(&app, 10, 5), (10, 4));
    app.status_lines = 2;
    assert_eq!(map_client_coords(&app, 10, 5), (10, 3));
    app.status_visible = false;
    assert_eq!(map_client_coords(&app, 10, 5), (10, 5));
}
