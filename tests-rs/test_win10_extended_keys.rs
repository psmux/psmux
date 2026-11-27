// Issue #1116: the Windows 10 inbox conhost drops every extended key form
// (`CSI 27;m;k~` and `CSI k;m u`), so a program that asked for an extended
// key mode (#860) read nothing for C-a, M-a, C-1 or S-Enter.  On an inbox host
// before build 22000 psmux keeps the legacy bytes; later builds and a host
// supplied with PSMUX_CONPTY_DIR keep the extended form.

use super::*;
use crate::input::pane_input_fixture::{fixture, pane};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const S: KeyModifiers = KeyModifiers::SHIFT;
const M: KeyModifiers = KeyModifiers::ALT;
const C: KeyModifiers = KeyModifiers::CONTROL;

struct HostBuild;
impl HostBuild {
    fn set(build: Option<u32>) -> HostBuild {
        TEST_WIN32_INPUT_HOST_BUILD.with(|b| b.set(build));
        HostBuild
    }
}
impl Drop for HostBuild {
    fn drop(&mut self) {
        TEST_WIN32_INPUT_HOST_BUILD.with(|b| b.set(None));
    }
}

/// Keys whose bytes an extended key mode changes, as a typed key and as the
/// send-keys name.
const KEYS: &[(KeyCode, KeyModifiers, &str)] = &[
    (KeyCode::Char('a'), C, "C-a"),
    (KeyCode::Char('a'), M, "M-a"),
    (KeyCode::Char('1'), C, "C-1"),
    (KeyCode::Char('i'), C, "C-i"),
    (KeyCode::Enter, S, "S-Enter"),
    (KeyCode::Backspace, C, "C-BSpace"),
];

/// What a VT reader pane in `mode` gets for each key, typed and sent.
fn delivered(mode: u8, xterm: bool) -> Vec<(Vec<u8>, Vec<u8>)> {
    KEYS.iter().map(|&(code, mods, name)| {
        let (mut app, rec) = fixture(false);
        crate::server::options::apply_set_option(&mut app, "extended-keys-format", if xterm { "xterm" } else { "csi-u" }, false).unwrap();
        pane(&mut app).term.lock().unwrap().screen_mut().set_extended_keys_mode(mode);
        forward_key_to_active(&mut app, KeyEvent::new(code, mods)).unwrap();
        let typed = rec.take();
        send_key_to_active(&mut app, name).unwrap();
        (typed, rec.take())
    }).collect()
}

#[test]
fn only_the_windows_10_inbox_host_is_one() {
    assert!(windows10_inbox_host(Some(19045), false));
    assert!(windows10_inbox_host(Some(19041), false));
    assert!(windows10_inbox_host(Some(WIN32_INPUT_INBOX_MIN_BUILD - 1), false));
    assert!(!windows10_inbox_host(Some(19045), true), "OpenConsole on Windows 10");
    assert!(!windows10_inbox_host(Some(WIN32_INPUT_INBOX_MIN_BUILD), false));
    assert!(!windows10_inbox_host(Some(26300), false));
    assert!(!windows10_inbox_host(None, false), "unknown build");
}

#[test]
fn an_extended_pane_gets_the_legacy_bytes_on_windows_10() {
    let legacy = {
        let _b = HostBuild::set(Some(19045));
        delivered(0, true)
    };
    let _b = HostBuild::set(Some(19045));
    for mode in [1, 2] {
        for xterm in [true, false] {
            let got = delivered(mode, xterm);
            for (i, &(_, _, name)) in KEYS.iter().enumerate() {
                assert_eq!(got[i], legacy[i], "mode={mode} xterm={xterm} key={name}");
            }
        }
    }
}

#[test]
fn an_extended_pane_keeps_the_extended_form_on_windows_11() {
    let _b = HostBuild::set(Some(26300));
    let got = delivered(2, true);
    for (i, &(code, mods, name)) in KEYS.iter().enumerate() {
        let want = crate::extended_keys::encode(2, true, code, mods).unwrap();
        assert_eq!(got[i].0, want, "typed {name}");
        let (ncode, nmods) = crate::config::parse_key_string(name).unwrap();
        let want = crate::extended_keys::encode_name(2, true, ncode, nmods).unwrap();
        assert_eq!(got[i].1, want, "sent {name}");
    }
}

#[test]
fn the_send_keys_route_keeps_the_legacy_name_on_windows_10() {
    for (build, extended) in [(Some(19045), false), (Some(26300), true), (None, true)] {
        let _b = HostBuild::set(build);
        let (mut app, _rec) = fixture(false);
        pane(&mut app).term.lock().unwrap().screen_mut().set_extended_keys_mode(2);
        for &(_, _, name) in KEYS {
            assert_eq!(active_extended_named_key_bytes(&mut app, name).is_some(), extended, "build={build:?} key={name}");
        }
    }
}
