// Issue #1058: a pane keeps its clock on screen while another pane has focus.
//
// tmux keeps clock mode per pane (window-clock.c): each pane in the mode has
// its own timer (`window_clock_timer_callback`) and is drawn whether it has
// the focus or not, so a clock can stay up beside the pane being worked in.
// psmux kept the clock of a pane the focus left in `AppState::clock_panes`
// (#828) but the frame had no per pane mark and the client drew the clock on
// the focused pane only, so moving away hid the clock and moving back showed
// it again.
//
// A left press on the clock pane closes the clock, as tmux's MouseDown1Pane
// `select-pane -t=; send -M` (key-bindings.c) hands the press to the mode and
// `window_clock_key` resets it for any key.
// Registered from src/window_ops.rs (window_ops_tests).

use super::make_window_with_two_panes;
use crate::layout::LayoutJson;
use crate::types::{AppState, Mode};
use ratatui::layout::Rect;

const LEFT: usize = 1;
const RIGHT: usize = 2;

/// One window split in two, the left pane focused and in clock mode.
fn app_with_clock_on_left() -> AppState {
    let mut app = AppState::new("clock-other-pane".to_string());
    app.mouse_enabled = true;
    app.last_window_area = Rect::new(0, 0, 10, 5);
    app.windows.push(make_window_with_two_panes(LEFT, RIGHT));
    app.active_idx = 0;
    app.mode = Mode::ClockMode;
    app
}

fn focus(app: &mut AppState, index: usize) {
    crate::copy_mode::switch_with_copy_save(app, |app| app.windows[0].active_path = vec![index]);
}

/// Each leaf's (id, clock) in tree order.
fn clocks(layout: &LayoutJson) -> Vec<(usize, bool)> {
    match layout {
        LayoutJson::Leaf { id, clock, .. } => vec![(*id, *clock)],
        LayoutJson::Split { children, .. } => children.iter().flat_map(clocks).collect(),
    }
}

/// The marks of both serialisers: the fast one the server pushes to clients
/// and the serde one (`dump-layout`), which must agree.
fn both_dumps(app: &mut AppState) -> (Vec<(usize, bool)>, Vec<(usize, bool)>) {
    let fast = crate::layout::dump_layout_json_fast(app).expect("fast dump");
    let fast: LayoutJson = serde_json::from_str(&fast).expect("fast dump parses");
    let slow = crate::layout::dump_layout_json(app).expect("serde dump");
    let slow: LayoutJson = serde_json::from_str(&slow).expect("serde dump parses");
    (clocks(&fast), clocks(&slow))
}

#[test]
fn the_focused_clock_is_marked_on_its_own_pane() {
    let mut app = app_with_clock_on_left();
    let (fast, slow) = both_dumps(&mut app);
    assert_eq!(fast, vec![(LEFT, true), (RIGHT, false)]);
    assert_eq!(slow, fast, "the serde dump marks the same panes");
}

#[test]
fn a_clock_the_focus_left_stays_marked() {
    let mut app = app_with_clock_on_left();
    focus(&mut app, 1);
    assert!(matches!(app.mode, Mode::Passthrough), "the right pane is not in a mode");
    assert_eq!(app.clock_panes, vec![LEFT]);
    let (fast, slow) = both_dumps(&mut app);
    assert_eq!(fast, vec![(LEFT, true), (RIGHT, false)], "the left pane keeps its clock on screen");
    assert_eq!(slow, fast);

    focus(&mut app, 0);
    assert!(matches!(app.mode, Mode::ClockMode), "back on the left, its clock is live again");
    assert!(app.clock_panes.is_empty());
    let (fast, _) = both_dumps(&mut app);
    assert_eq!(fast, vec![(LEFT, true), (RIGHT, false)]);
}

#[test]
fn an_ordinary_frame_carries_no_clock_field() {
    let mut app = app_with_clock_on_left();
    app.mode = Mode::Passthrough;
    let fast = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let slow = crate::layout::dump_layout_json(&mut app).expect("serde dump");
    assert!(!fast.contains("\"clock\""), "the fast dump must not grow: {fast}");
    assert!(!slow.contains("\"clock\""), "the serde dump must not grow");
    let (fast, slow) = both_dumps(&mut app);
    assert_eq!(fast, vec![(LEFT, false), (RIGHT, false)]);
    assert_eq!(slow, fast);
}

#[test]
fn the_client_redraws_while_any_pane_shows_a_clock() {
    let mut app = app_with_clock_on_left();
    focus(&mut app, 1);
    let fast = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&fast).expect("parses");
    assert!(crate::client::layout_shows_clock(&layout), "the parked clock keeps the client redrawing");
    crate::copy_mode::close_clock_of(&mut app, LEFT);
    let fast = crate::layout::dump_layout_json_fast(&mut app).expect("fast dump");
    let layout: LayoutJson = serde_json::from_str(&fast).expect("parses");
    assert!(!crate::client::layout_shows_clock(&layout));
}

#[test]
fn a_left_press_on_the_clock_pane_closes_the_clock() {
    let mut app = app_with_clock_on_left();
    focus(&mut app, 1);
    // A click on the left pane: the client sends select-pane, then the press.
    focus(&mut app, 0);
    assert!(matches!(app.mode, Mode::ClockMode));
    let closed = crate::window_ops::handle_client_pane_mouse(&mut app, LEFT, 0, 2, 1, true, false);
    assert!(closed, "the press reaches the clock, which closes");
    assert!(matches!(app.mode, Mode::Passthrough));
    assert!(app.clock_panes.is_empty());
}

#[test]
fn a_press_on_another_pane_leaves_the_clock() {
    let mut app = app_with_clock_on_left();
    focus(&mut app, 1);
    let closed = crate::window_ops::handle_client_pane_mouse(&mut app, RIGHT, 0, 2, 1, true, false);
    assert!(!closed);
    assert_eq!(app.clock_panes, vec![LEFT], "the left pane keeps its clock");
    let release = crate::window_ops::handle_client_pane_mouse(&mut app, RIGHT, 0, 2, 1, false, false);
    assert!(!release);
    assert_eq!(app.clock_panes, vec![LEFT]);
}

#[test]
fn only_a_left_press_closes_the_clock() {
    let mut app = app_with_clock_on_left();
    // The right button and a replayed press (#1031) are not MouseDown1Pane.
    assert!(!crate::window_ops::handle_client_pane_mouse(&mut app, LEFT, 2, 2, 1, true, false));
    assert!(!crate::window_ops::handle_client_pane_mouse(&mut app, LEFT, 0, 2, 1, true, true));
    assert!(matches!(app.mode, Mode::ClockMode));
}

#[test]
fn a_parked_clock_of_a_killed_pane_is_dropped() {
    let mut app = app_with_clock_on_left();
    app.clock_panes = vec![99];
    focus(&mut app, 1);
    assert_eq!(app.clock_panes, vec![LEFT], "%99 no longer exists");
}
