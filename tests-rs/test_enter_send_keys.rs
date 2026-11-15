//! Scripted named Enter uses pane transport without interactive bookkeeping.

use crate::input::pane_input_fixture::{fixture, pane};
use crate::types::{AppState, CtrlReq, LayoutKind, Mode, Node};
use std::time::Instant;

fn send(app: &mut AppState, keys: &[&str], literal: bool) {
    let mut lp = super::SessionLoop::new(app.command_aliases.clone());
    let pty = portable_pty::native_pty_system();
    let req = CtrlReq::SendKeys(keys.iter().map(|s| s.to_string()).collect(), literal);
    let flow = super::super::dispatch::dispatch(app, &mut lp, req, &*pty).unwrap();
    assert!(matches!(flow, super::Flow::Continue));
}

fn shift_record() -> Vec<u8> {
    crate::input::win32_input_key_seq(13, crate::platform::mouse_inject::vk_to_scan(13), 13, 16).into_bytes()
}

#[test]
fn scripted_enter_preserves_shift_and_ctrl_with_aliases() {
    for (name, expected) in [
        ("S-Enter", shift_record()), ("s-return", shift_record()),
        ("S-CR", shift_record()), ("C-Enter", b"\n".to_vec()),
        ("Enter", b"\r".to_vec()), ("M-Enter", b"\x1b\r".to_vec()),
    ] {
        let (mut app, rec) = fixture(true);
        pane(&mut app).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
        assert!(rec.take().is_empty());
        assert!(pane(&mut app).last_special_key.is_none());
        send(&mut app, &[name], false);
        assert_eq!(rec.take(), expected, "{name}");
        assert!(pane(&mut app).last_special_key.is_none());
        assert!(pane(&mut app).last_text_input.is_none());
    }
}

#[test]
fn scripted_sync_encodes_enter_for_each_pane() {
    let (mut app, rec) = fixture(true);
    pane(&mut app).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
    let (mut other, vt) = fixture(true);
    pane(&mut other).term.lock().unwrap().screen_mut().set_extended_keys_mode(2);
    let native_root = app.windows.remove(0).root;
    let vt_root = other.windows.remove(0).root;
    let (mut app, _) = fixture(true);
    app.windows[0].root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![50, 50], children: vec![native_root, vt_root] };
    app.windows[0].active_path = vec![0];
    app.sync_input = true;
    crate::server::options::apply_set_option(&mut app, "extended-keys-format", "csi-u", false).unwrap();
    assert!(rec.take().is_empty() && vt.take().is_empty());
    send(&mut app, &["S-Enter"], false);
    assert_eq!(rec.take(), shift_record());
    assert_eq!(vt.take(), b"\x1b[13;2u");
}

#[test]
fn scripted_literal_names_and_bytes_remain_literal() {
    for text in ["S-Enter", "\x1b\r"] {
        let (mut app, rec) = fixture(true);
        pane(&mut app).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
        assert!(rec.take().is_empty());
        send(&mut app, &[text], true);
        assert_eq!(rec.take(), text.as_bytes());
        assert!(!pane(&mut app).win32_input_latched);
    }
}

#[test]
fn text_followed_by_an_enter_alias_has_no_extra_separator() {
    for name in ["Enter", "Return", "CR", "return", "cr"] {
        let (mut app, rec) = fixture(true);
        assert!(rec.take().is_empty());
        send(&mut app, &["secret", name], false);
        assert_eq!(rec.take(), b"secret\r", "{name}");
    }
}

#[test]
fn scripted_names_must_match_the_entire_token() {
    for text in [" Enter ", " S-Enter "] {
        let (mut app, rec) = fixture(true);
        assert!(rec.take().is_empty());
        send(&mut app, &[text], false);
        assert_eq!(rec.take(), text.as_bytes());
    }
}

#[test]
fn scripted_enter_respects_input_off_and_mode_consumption() {
    let (mut app, rec) = fixture(true);
    pane(&mut app).input_off = true;
    assert!(rec.take().is_empty());
    send(&mut app, &["S-Enter"], false);
    assert!(rec.take().is_empty());
    pane(&mut app).input_off = false;
    app.mode = Mode::ClockMode;
    send(&mut app, &["Enter"], false);
    assert!(matches!(app.mode, Mode::Passthrough));
    assert!(rec.take().is_empty());
}
