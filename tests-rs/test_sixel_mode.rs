// The `sixel` option takes off, on and auto, and auto is the default.
//
// tmux has no such option: it draws an image only when the terminal's DA1
// reply lists sixel (tty-keys.c:1528, tty.c:2191 in tmux 3.8), which is what
// auto does. on draws whatever the terminal said, as tmux does when
// `terminal-features` lists `sixel` (tty-term.c:619), and off never draws.
use crate::rendering::{frame_sixel_mode, sixel_mode_draws};
use crate::server::option_catalog::{default_for, validate_option_value};
use crate::types::SixelMode;

#[test]
fn each_value_against_each_reply() {
    let cases = [
        (SixelMode::Off, Some(true), false),
        (SixelMode::Off, Some(false), false),
        (SixelMode::Off, None, false),
        (SixelMode::On, Some(true), true),
        (SixelMode::On, Some(false), true),
        (SixelMode::On, None, true),
        (SixelMode::Auto, Some(true), true),
        (SixelMode::Auto, Some(false), false),
        (SixelMode::Auto, None, false),
    ];
    for (mode, host, draws) in cases {
        assert_eq!(sixel_mode_draws(mode, host), draws, "{:?} with reply {:?}", mode, host);
    }
}

#[test]
fn the_default_is_auto() {
    assert_eq!(default_for("sixel"), Some("auto"));
    assert_eq!(crate::types::AppState::new("s".to_string()).sixel, SixelMode::Auto);
}

#[test]
fn the_three_values_and_the_old_flag_spellings_parse() {
    assert_eq!(SixelMode::parse("auto"), Some(SixelMode::Auto));
    for on in ["on", "true", "1", "yes", "ON"] {
        assert_eq!(SixelMode::parse(on), Some(SixelMode::On), "{}", on);
    }
    for off in ["off", "false", "0", "no"] {
        assert_eq!(SixelMode::parse(off), Some(SixelMode::Off), "{}", off);
    }
    assert_eq!(SixelMode::parse("always"), None);
    for value in SixelMode::CHOICES {
        assert_eq!(SixelMode::parse(value).map(SixelMode::as_str), Some(*value));
    }
}

#[test]
fn any_other_value_is_refused() {
    assert_eq!(validate_option_value("sixel", "auto"), Ok(()));
    assert_eq!(validate_option_value("sixel", "on"), Ok(()));
    assert_eq!(
        validate_option_value("sixel", "always"),
        Err("unknown value: always".to_string()),
    );
}

#[test]
fn set_and_show_round_trip() {
    use crate::server::options::{apply_set_option, get_option_value};
    let mut app = crate::types::AppState::new("s".to_string());
    for value in ["off", "on", "auto"] {
        apply_set_option(&mut app, "sixel", value, false).unwrap();
        assert_eq!(get_option_value(&app, "sixel"), value);
    }
    apply_set_option(&mut app, "sixel", "yes", false).unwrap();
    assert_eq!(get_option_value(&app, "sixel"), "on");
}

#[test]
fn a_toggle_swaps_on_and_off_and_leaves_auto() {
    // tmux toggles a choice only between its first two values
    // (options_from_string_choice in options.c).
    use crate::server::options::toggled_value;
    assert_eq!(toggled_value("sixel", "on"), "off");
    assert_eq!(toggled_value("sixel", "off"), "on");
    assert_eq!(toggled_value("sixel", "auto"), "auto");
    assert_eq!(toggled_value("mouse", "on"), "off");
}

#[test]
fn a_frame_from_an_older_server_keeps_working() {
    assert_eq!(frame_sixel_mode(Some("on"), true), SixelMode::On);
    assert_eq!(frame_sixel_mode(Some("auto"), true), SixelMode::Auto);
    assert_eq!(frame_sixel_mode(Some("off"), false), SixelMode::Off);
    // No sixel_mode: the old flag's on drew unless the terminal said no.
    assert_eq!(frame_sixel_mode(None, true), SixelMode::Auto);
    assert_eq!(frame_sixel_mode(None, false), SixelMode::Off);
}
