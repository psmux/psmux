// Tests for the ctrl-wheel-zoom option (PR #791).
//
// Handing Ctrl+Wheel back to Windows Terminal as a font zoom means psmux
// synthesizes global keystrokes, and tmux has no such behaviour (it delivers
// the notch to a C-WheelUpPane / C-WheelDownPane binding, and drops it when
// nothing is bound).  So it is opt in: off unless the user turns it on.

use super::*;

fn mock_app() -> crate::types::AppState {
    crate::types::AppState::new("test_session".to_string())
}

#[test]
fn defaults_to_off() {
    let app = mock_app();
    assert!(!app.ctrl_wheel_zoom, "Ctrl+Wheel must keep reaching the pane by default");
}

#[test]
fn config_turns_it_on_and_off() {
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g ctrl-wheel-zoom on");
    assert!(app.ctrl_wheel_zoom);
    parse_config_content(&mut app, "set -g ctrl-wheel-zoom off");
    assert!(!app.ctrl_wheel_zoom);
}

#[test]
fn show_options_reports_it() {
    let mut app = mock_app();
    assert_eq!(crate::server::options::get_option_value(&app, "ctrl-wheel-zoom"), "off");
    app.ctrl_wheel_zoom = true;
    assert_eq!(crate::server::options::get_option_value(&app, "ctrl-wheel-zoom"), "on");
}

#[test]
fn set_option_and_toggle() {
    let mut app = mock_app();
    crate::server::options::apply_set_option(&mut app, "ctrl-wheel-zoom", "on", false)
        .expect("apply_set_option must accept the option");
    assert!(app.ctrl_wheel_zoom);
    assert!(crate::server::options::is_boolean_option("ctrl-wheel-zoom"));
    assert!(crate::server::options::missing_value_toggles("ctrl-wheel-zoom"));
}

#[test]
fn set_u_restores_the_catalog_default() {
    let def = crate::server::option_catalog::option_definition("ctrl-wheel-zoom")
        .expect("the option must be in the catalog");
    assert_eq!(def.default, "off");
    let mut app = mock_app();
    app.ctrl_wheel_zoom = true;
    crate::server::options::reset_option_to_default(&mut app, "ctrl-wheel-zoom");
    assert!(!app.ctrl_wheel_zoom);
}
