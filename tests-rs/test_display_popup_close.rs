// display-popup closes as tmux 3.8 decides it: with no -E the popup stays
// open when its command exits, -E closes it, -EE closes it only on exit
// status 0, Escape and C-c close it, and -k lets any key close it once the
// command has exited (cmd-display-menu.c:676-690, popup.c:430-436 and
// 500-501). psmux's -K keeps it open even with -E, and q closes it after the
// exit, as it always did.

use super::*;
use crate::commands::execute_command_string;
use crate::types::{AppState, Mode};

fn rules(words: &str) -> PopupClose {
    let args: Vec<&str> = words.split_whitespace().collect();
    PopupClose::from_args(&parse_display_popup_args(&args).expect("parse"))
}

const ESC: &[u8] = b"\x1b";
const CTRL_C: &[u8] = b"\x03";

#[test]
fn the_flags_are_read() {
    assert_eq!(rules("cmd"), PopupClose { exit_flags: 0, any_key: false, keep_open: false });
    assert_eq!(rules("-E cmd").exit_flags, 1);
    assert_eq!(rules("-EE cmd").exit_flags, 2);
    assert_eq!(rules("-E -E cmd").exit_flags, 2);
    assert_eq!(rules("-EEE cmd").exit_flags, 2);
    assert!(rules("-k cmd").any_key);
    assert!(rules("-Ek cmd").any_key);
    assert!(rules("-K -E cmd").keep_open);
}

#[test]
fn when_the_command_exits() {
    // (flags, closes at exit 0, closes at exit 3)
    for (flags, zero, three) in [
        ("cmd", false, false),
        ("-E cmd", true, true),
        ("-EE cmd", true, false),
        ("-k cmd", false, false),
        ("-E -k cmd", true, true),
        ("-K cmd", false, false),
        ("-K -E cmd", false, false),
        ("-K -EE cmd", false, false),
    ] {
        let r = rules(flags);
        assert_eq!(r.closes_on_exit(0), zero, "{flags}, exit 0");
        assert_eq!(r.closes_on_exit(3), three, "{flags}, exit 3");
    }
}

#[test]
fn escape_and_ctrl_c_close_a_popup_without_e_at_once() {
    for flags in ["cmd", "-K cmd", "-k cmd"] {
        let r = rules(flags);
        assert!(r.closes_on_key(ESC, false), "{flags}: Escape while it runs");
        assert!(r.closes_on_key(CTRL_C, false), "{flags}: C-c while it runs");
        assert!(!r.closes_on_key(b"x", false), "{flags}: x while it runs");
        assert!(!r.closes_on_key(b"q", false), "{flags}: q while it runs");
    }
}

#[test]
fn under_e_escape_and_ctrl_c_go_to_the_command_while_it_runs() {
    for flags in ["-E cmd", "-EE cmd", "-K -E cmd", "-E -k cmd"] {
        let r = rules(flags);
        assert!(!r.closes_on_key(ESC, false), "{flags}: Escape while it runs");
        assert!(!r.closes_on_key(CTRL_C, false), "{flags}: C-c while it runs");
        assert!(!r.closes_on_key(b"x", false), "{flags}: x while it runs");
        // Arrow keys start with ESC too but are not Escape.
        assert!(!r.closes_on_key(b"\x1b[A", false), "{flags}: Up while it runs");
    }
}

#[test]
fn once_the_command_has_exited() {
    for flags in ["cmd", "-EE cmd", "-K -E cmd"] {
        let r = rules(flags);
        assert!(r.closes_on_key(ESC, true), "{flags}: Escape");
        assert!(r.closes_on_key(CTRL_C, true), "{flags}: C-c");
        assert!(r.closes_on_key(b"q", true), "{flags}: q");
        assert!(!r.closes_on_key(b"x", true), "{flags}: x without -k");
        assert!(!r.closes_on_key(b"\x1b[A", true), "{flags}: Up without -k");
    }
    let r = rules("-EE -k cmd");
    assert!(r.closes_on_key(b"x", true), "-k: x after the exit");
    assert!(r.closes_on_key(b"\x1b[A", true), "-k: Up after the exit");
    assert!(!r.closes_on_key(b"", true), "-k: no key");
}

#[test]
fn a_menu_item_opens_the_popup_with_the_same_rules() {
    // The in process path (menu items, confirm-before, commands the server
    // runs itself) used to read only whether -E was given and dropped -K.
    for (command, want) in [
        ("display-popup 'echo hi'", rules("cmd")),
        ("display-popup -EE 'echo hi'", rules("-EE cmd")),
        ("display-popup -K -E 'echo hi'", rules("-K -E cmd")),
        ("display-popup -k 'echo hi'", rules("-k cmd")),
    ] {
        let mut app = AppState::new("s".to_string());
        app.window_base_index = 0;
        execute_command_string(&mut app, command).unwrap();
        match &app.mode {
            Mode::PopupMode { close, .. } => assert_eq!(*close, want, "{command}"),
            _ => panic!("{command}: expected PopupMode"),
        }
    }
}
