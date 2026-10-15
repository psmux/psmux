// Issue #828: a -t request must meet the TARGET pane's own mode.
//
// psmux keeps the focused pane's copy or clock mode in `AppState::mode` and
// parks every other pane's (copy in `Pane::copy_state`, clock in
// `AppState::clock_panes`).  The temporary -t focus switched panes without
// moving the modes, so the focused pane's mode answered for every target:
// `send-keys -t s:1 -l text` fed window 0's copy or clock mode, and
// `#{pane_in_mode}` read 1 for every window.  tmux sends a key to the mode
// of the pane it targets (window.c `window_pane_key`) and reports the mode
// per pane (format.c `format_cb_pane_in_mode`).
// Registered from src/server/temp_target.rs; fixture from the #793 tests.

use super::*;
use super::tests::{app, active};
use crate::types::{Mode, TempTarget};

fn pane(id: usize) -> TempTarget {
    TempTarget { pane: Some(id), pane_is_id: true, ..Default::default() }
}

fn in_mode(app: &AppState, pane_id: usize) -> String {
    crate::format::expand_format_for_pane_by_id("#{pane_in_mode}", app, pane_id)
}

fn pane_mode(app: &AppState, pane_id: usize) -> String {
    crate::format::expand_format_for_pane_by_id("#{pane_mode}", app, pane_id)
}

#[test]
fn copy_mode_of_the_focused_pane_does_not_follow_a_target() {
    let mut app = app();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((3, 7));
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    assert_eq!(active(&app), (1, 4));
    assert!(matches!(app.mode, Mode::Passthrough), "the target %4 is not in a mode");
    assert_eq!(in_mode(&app, 4), "0");
    assert_eq!(in_mode(&app, 1), "1", "%1 keeps its parked copy mode");
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(active(&app), (0, 1));
    assert!(matches!(app.mode, Mode::CopyMode), "%1 has its copy mode back");
    assert_eq!(app.copy_pos, Some((3, 7)), "with its own cursor");
    assert_eq!(in_mode(&app, 1), "1");
    assert_eq!(in_mode(&app, 4), "0");
}

#[test]
fn copy_mode_entered_under_a_target_stays_on_that_pane() {
    let mut app = app();
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    crate::copy_mode::enter_copy_mode(&mut app);
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(active(&app), (0, 1));
    assert!(matches!(app.mode, Mode::Passthrough), "the focused %1 never entered copy mode");
    assert_eq!(in_mode(&app, 1), "0");
    assert_eq!(in_mode(&app, 4), "1");
    // The next request aimed at %4 meets its copy mode again.
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    crate::copy_mode::exit_copy_mode(&mut app);
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(in_mode(&app, 4), "0", "leaving it under the target clears it");
    assert!(matches!(app.mode, Mode::Passthrough));
}

#[test]
fn clock_mode_belongs_to_one_pane() {
    let mut app = app();
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    app.mode = Mode::ClockMode;
    restore_temp_focus(&mut app, &mut saved);
    assert!(matches!(app.mode, Mode::Passthrough), "the focused %1 is not in clock mode");
    assert_eq!(pane_mode(&app, 4), "clock-mode");
    assert_eq!(in_mode(&app, 4), "1");
    assert_eq!(in_mode(&app, 1), "0");
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    assert!(matches!(app.mode, Mode::ClockMode), "a request aimed at %4 meets its clock");
    restore_temp_focus(&mut app, &mut saved);
    assert_eq!(pane_mode(&app, 4), "clock-mode", "and the clock stays there");
}

#[test]
fn a_target_on_the_focused_pane_leaves_its_mode_alone() {
    let mut app = app();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((2, 2));
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(1)).unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    restore_temp_focus(&mut app, &mut saved);
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_pos, Some((2, 2)));
    let w = &app.windows[0];
    let p = crate::tree::active_pane(&w.root, &w.active_path).unwrap();
    // enter_copy_mode marks the pane (save_copy_state_to_pane); that marker
    // is how #{pane_in_mode} and the layout see the pane's own mode.
    assert!(p.copy_state.is_some(), "the focused pane still carries its copy mode marker");
}

#[test]
fn a_client_overlay_is_not_moved_or_replaced_by_a_target() {
    let mut app = app();
    // %4 has a parked copy mode; a menu is open over the session.
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    crate::copy_mode::enter_copy_mode(&mut app);
    restore_temp_focus(&mut app, &mut saved);
    app.mode = Mode::MenuMode { menu: crate::types::Menu { title: String::new(), items: Vec::new(), selected: 0, x: None, y: None } };
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    assert!(matches!(app.mode, Mode::MenuMode { .. }), "the menu stays open under the target");
    restore_temp_focus(&mut app, &mut saved);
    assert!(matches!(app.mode, Mode::MenuMode { .. }));
    assert_eq!(in_mode(&app, 4), "1", "%4 keeps its parked copy mode");
}

#[test]
fn a_copy_prompt_the_user_is_typing_into_is_left_alone() {
    let mut app = app();
    crate::copy_mode::enter_copy_mode(&mut app);
    app.mode = Mode::CopyGoto { input: "12".into() };
    let mut saved = None;
    apply_temp_target(&mut app, &mut saved, &pane(4)).unwrap();
    restore_temp_focus(&mut app, &mut saved);
    assert!(matches!(&app.mode, Mode::CopyGoto { input } if input == "12"));
    assert_eq!(active(&app), (0, 1));
}
