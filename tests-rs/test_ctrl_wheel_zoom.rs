// Ctrl+Wheel font zoom: only a wheel notch with exactly Ctrl held is taken
// from the pane and handed to the terminal.  Every other mouse event must keep
// its old meaning, so this pins the whole set the client still forwards.

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use std::path::{Path, PathBuf};

use crate::platform::terminal_zoom::{scroll_to_zoom_enabled, scroll_to_zoom_setting, wheel_zoom_direction, wt_settings_path};

#[test]
fn zoom_is_off_only_when_the_terminal_option_is_false() {
    assert!(scroll_to_zoom_enabled(r#"{ "experimental.scrollToZoom": true }"#));
    assert!(scroll_to_zoom_enabled("{\r\n    \"copyOnSelect\": false,\r\n    \"experimental.scrollToZoom\" :\r\n true,\r\n}"));
    assert!(!scroll_to_zoom_enabled(r#"{ "experimental.scrollToZoom": false }"#));
    assert!(!scroll_to_zoom_enabled("{\r\n    \"experimental.scrollToZoom\" :\r\n false,\r\n}"));
    // Missing key: Windows Terminal's default, which zooms.
    assert!(scroll_to_zoom_enabled(r#"{ "copyOnSelect": false }"#));
    assert!(scroll_to_zoom_enabled(""));
    assert!(scroll_to_zoom_enabled("not json at all"));
    assert_eq!(scroll_to_zoom_setting(r#"{ "copyOnSelect": false }"#), None);
    assert_eq!(scroll_to_zoom_setting(r#"{ "experimental.scrollToZoom": true }"#), Some(true));
    assert_eq!(scroll_to_zoom_setting(r#"{ "experimental.scrollToZoom": false }"#), Some(false));
}

#[test]
fn commented_out_option_does_not_count() {
    assert_eq!(scroll_to_zoom_setting("{\n // \"experimental.scrollToZoom\": true,\n \"a\": 1 }"), None);
    assert_eq!(scroll_to_zoom_setting("{ /* \"experimental.scrollToZoom\": true */ \"a\": 1 }"), None);
    // A commented-out false does not switch the zoom off.
    assert!(scroll_to_zoom_enabled("{\n // \"experimental.scrollToZoom\": false,\n \"a\": 1 }"));
    assert!(scroll_to_zoom_enabled("{ /* \"experimental.scrollToZoom\": false */ \"a\": 1 }"));
    // A `//` inside a string is not a comment.
    assert!(!scroll_to_zoom_enabled("{ \"u\": \"http://x\", \"experimental.scrollToZoom\": false }"));
}

#[test]
fn last_occurrence_wins() {
    assert!(!scroll_to_zoom_enabled(r#"{ "experimental.scrollToZoom": true, "experimental.scrollToZoom": false }"#));
    assert!(scroll_to_zoom_enabled(r#"{ "experimental.scrollToZoom": false, "experimental.scrollToZoom": true }"#));
}

// Windows paths: `\` is not a separator anywhere else.
#[cfg(windows)]
#[test]
fn settings_path_follows_the_install() {
    let local = Path::new(r"C:\Users\u\AppData\Local");
    let stable = Path::new(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.24.12741.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe");
    let preview = Path::new(r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminalPreview_1.25.1.0_x64__8wekyb3d8bbwe\WindowsTerminal.exe");
    let unpackaged = Path::new(r"C:\no-such-dir-psmux-test\terminal\WindowsTerminal.exe");
    assert_eq!(
        wt_settings_path(stable, local),
        PathBuf::from(r"C:\Users\u\AppData\Local\Packages\Microsoft.WindowsTerminal_8wekyb3d8bbwe\LocalState\settings.json"),
    );
    assert_eq!(
        wt_settings_path(preview, local),
        PathBuf::from(r"C:\Users\u\AppData\Local\Packages\Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe\LocalState\settings.json"),
    );
    assert_eq!(
        wt_settings_path(unpackaged, local),
        PathBuf::from(r"C:\Users\u\AppData\Local\Microsoft\Windows Terminal\settings.json"),
    );
}

fn ev(kind: MouseEventKind, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent { kind, column: 3, row: 4, modifiers }
}

#[test]
fn ctrl_wheel_is_zoom() {
    assert_eq!(wheel_zoom_direction(&ev(MouseEventKind::ScrollUp, KeyModifiers::CONTROL)), Some(true));
    assert_eq!(wheel_zoom_direction(&ev(MouseEventKind::ScrollDown, KeyModifiers::CONTROL)), Some(false));
}

#[test]
fn every_other_wheel_keeps_scrolling() {
    let others = [
        KeyModifiers::empty(),
        KeyModifiers::SHIFT,
        KeyModifiers::ALT,
        KeyModifiers::CONTROL | KeyModifiers::ALT, // AltGr
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
    ];
    for m in others {
        for kind in [MouseEventKind::ScrollUp, MouseEventKind::ScrollDown] {
            assert_eq!(wheel_zoom_direction(&ev(kind, m)), None, "{:?} {:?}", kind, m);
        }
    }
}

#[test]
fn non_wheel_events_with_ctrl_are_untouched() {
    let kinds = [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Moved,
        MouseEventKind::ScrollLeft,
        MouseEventKind::ScrollRight,
    ];
    for kind in kinds {
        for m in [KeyModifiers::empty(), KeyModifiers::CONTROL] {
            assert_eq!(wheel_zoom_direction(&ev(kind, m)), None, "{:?} {:?}", kind, m);
        }
    }
}
