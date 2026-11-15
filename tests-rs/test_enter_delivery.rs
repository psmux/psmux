//! Enter transport is chosen per receiving pane, before modifiers become bytes.

use super::pane_input_fixture::{fixture, pane};
use super::{forward_key_to_active, send_key_to_active, send_text_to_active, win32_input_key_seq};
use crate::types::{FloatingPane, LayoutKind, Node};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

const N: KeyModifiers = KeyModifiers::NONE;
const S: KeyModifiers = KeyModifiers::SHIFT;
const M: KeyModifiers = KeyModifiers::ALT;
const C: KeyModifiers = KeyModifiers::CONTROL;

fn shift_record() -> Vec<u8> {
    win32_input_key_seq(13, crate::platform::mouse_inject::vk_to_scan(13), 13, 16).into_bytes()
}

#[test]
fn local_and_named_enter_preserve_modifiers_and_negotiated_format() {
    for local in [false, true] {
        for host in [false, true] {
            for mode in [None, Some(0x01F0), Some(0x01E4), Some(0x03F0), Some(0x01F7)] {
                for extended in [0, 1, 2] {
                    for xterm in [false, true] {
                        for (name, mods, m) in [
                            ("enter", N, 1), ("S-Enter", S, 2), ("M-Enter", M, 3),
                            ("M-S-Enter", M | S, 4), ("C-Enter", C, 5),
                            ("C-S-Enter", C | S, 6), ("C-M-Enter", C | M, 7),
                            ("C-M-S-Enter", C | M | S, 8),
                        ] {
                            let (mut app, recorder) = fixture(host);
                            crate::server::options::apply_set_option(&mut app, "extended-keys-format", if xterm { "xterm" } else { "csi-u" }, false).unwrap();
                            let p = pane(&mut app);
                            p.mouse_input_cache = Some((Instant::now(), mode));
                            p.term.lock().unwrap().screen_mut().set_extended_keys_mode(extended);
                            assert!(recorder.take().is_empty());
                            assert!(!p.win32_input_latched);
                            assert_eq!(p.term.lock().unwrap().screen().extended_keys_mode(), extended);
                            if local {
                                forward_key_to_active(&mut app, KeyEvent::new(KeyCode::Enter, mods)).unwrap();
                            } else {
                                send_key_to_active(&mut app, name).unwrap();
                            }
                            let record = host && matches!(mode, Some(0x01F0) | Some(0x01E4)) && mods == S;
                            let negotiated = mode == Some(0x03F0) && extended > 0 && m > 1
                                && !(extended == 1 && mods.contains(M) && !mods.contains(C));
                            let expected = if negotiated {
                                if xterm { format!("\x1b[27;{m};13~") } else { format!("\x1b[13;{m}u") }.into_bytes()
                            } else if record {
                                shift_record()
                            } else if m == 1 {
                                b"\r".to_vec()
                            } else if mods == C {
                                b"\n".to_vec()
                            } else if mods.contains(C) {
                                format!("\x1b[13;{m}~").into_bytes()
                            } else {
                                b"\x1b\r".to_vec()
                            };
                            assert_eq!(recorder.take(), expected, "local={local} host={host} mode={mode:?} ext={extended} xterm={xterm} key={name}");
                            assert_eq!(pane(&mut app).win32_input_latched, record);
                        }
                    }
                }
            }
        }
    }
}

fn mixed_panes() -> (crate::types::AppState, super::pane_input_fixture::Recorder, super::pane_input_fixture::Recorder) {
    let (mut app, native) = fixture(true);
    pane(&mut app).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
    let (mut other, vt) = fixture(true);
    pane(&mut other).term.lock().unwrap().screen_mut().set_extended_keys_mode(2);
    let first = app.windows.remove(0).root;
    let second = other.windows.remove(0).root;
    let (mut shell, _) = fixture(true);
    shell.windows[0].root = Node::Split { kind: LayoutKind::Horizontal, sizes: vec![50, 50], children: vec![first, second] };
    shell.windows[0].active_path = vec![0];
    shell.sync_input = true;
    crate::server::options::apply_set_option(&mut shell, "extended-keys-format", "csi-u", false).unwrap();
    (shell, native, vt)
}

#[test]
fn synchronized_enter_uses_each_panes_protocol_and_skips_disabled_siblings() {
    for local in [false, true] {
        let (mut app, native, vt) = mixed_panes();
        assert!(native.take().is_empty() && vt.take().is_empty());
        if local { forward_key_to_active(&mut app, KeyEvent::new(KeyCode::Enter, S)).unwrap(); }
        else { send_key_to_active(&mut app, "S-Enter").unwrap(); }
        assert_eq!(native.take(), shift_record());
        assert_eq!(vt.take(), b"\x1b[13;2u");
        if let Node::Split { children, .. } = &mut app.windows[0].root {
            if let Node::Leaf(p) = &mut children[1] { p.input_off = true; }
        }
        if local { forward_key_to_active(&mut app, KeyEvent::new(KeyCode::Enter, S)).unwrap(); }
        else { send_key_to_active(&mut app, "S-Enter").unwrap(); }
        assert_eq!(native.take(), shift_record());
        assert!(vt.take().is_empty());
        pane(&mut app).input_off = true;
        if local { forward_key_to_active(&mut app, KeyEvent::new(KeyCode::Enter, S)).unwrap(); }
        else { send_key_to_active(&mut app, "S-Enter").unwrap(); }
        assert!(native.take().is_empty() && vt.take().is_empty());
    }
}

#[test]
fn synchronized_ctrl_enter_without_a_console_pid_keeps_the_lf_fallback() {
    let (mut app, recorder) = fixture(false);
    app.sync_input = true;
    assert!(pane(&mut app).child_pid.is_none());
    assert!(recorder.take().is_empty());
    forward_key_to_active(&mut app, KeyEvent::new(KeyCode::Enter, C)).unwrap();
    assert_eq!(recorder.take(), b"\n");
}

#[test]
fn named_enter_targets_the_focused_float_without_stamping_interactive_signals() {
    let (mut app, tiled) = fixture(true);
    let (mut other, floating) = fixture(true);
    pane(&mut other).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
    let Node::Leaf(p) = other.windows.remove(0).root else { unreachable!() };
    app.windows[0].floating.push(FloatingPane { pane: p, x: 0, y: 0, w: 20, h: 10, border: String::new(), id: 2, title: String::new(), position: None, styles: Default::default() });
    app.windows[0].floating_focus = Some(0);
    assert!(tiled.take().is_empty() && floating.take().is_empty());
    send_key_to_active(&mut app, "S-Return").unwrap();
    assert!(tiled.take().is_empty());
    assert_eq!(floating.take(), shift_record());
    let p = &app.windows[0].floating[0].pane;
    assert!(p.last_special_key.is_none() && p.last_text_input.is_none());
}

#[test]
fn literal_escape_cr_is_not_reinterpreted_as_shift_enter() {
    let (mut app, recorder) = fixture(true);
    pane(&mut app).mouse_input_cache = Some((Instant::now(), Some(0x01E4)));
    assert!(recorder.take().is_empty());
    send_text_to_active(&mut app, "\x1b\r").unwrap();
    assert_eq!(recorder.take(), b"\x1b\r");
    assert!(!pane(&mut app).win32_input_latched);
}
