// Issue #860: the extended key mode tmux keeps per pane, and the encoder
// that writes a key the way tmux does for that mode.
//
// tmux references: input.c INPUT_CSI_MODSET (1526) and INPUT_CSI_MODOFF
// (1545) for the state machine; input-keys.c input_key (690),
// input_key_mode1 (573) and input_key_extended (426) for the encoder.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const C: KeyModifiers = KeyModifiers::CONTROL;
const M: KeyModifiers = KeyModifiers::ALT;
const S: KeyModifiers = KeyModifiers::SHIFT;
const N: KeyModifiers = KeyModifiers::NONE;

fn x(mode: u8, code: KeyCode, mods: KeyModifiers) -> Option<String> {
    encode(mode, true, code, mods).map(|b| String::from_utf8(b).unwrap())
}
fn u(mode: u8, code: KeyCode, mods: KeyModifiers) -> Option<String> {
    encode(mode, false, code, mods).map(|b| String::from_utf8(b).unwrap())
}
fn s(v: &str) -> Option<String> {
    Some(v.to_string())
}

// ---------------------------------------------------------------- encoder

#[test]
fn standard_mode_never_changes_a_key() {
    // Mode 0 is the legacy path for every key, so the caller's bytes stay
    // byte for byte what they were before the fix.
    let keys = [
        (KeyCode::Char(' '), C), (KeyCode::Char('['), C), (KeyCode::Char('a'), C),
        (KeyCode::Char('a'), M), (KeyCode::Char('1'), C), (KeyCode::Enter, S),
        (KeyCode::Enter, C), (KeyCode::Tab, C), (KeyCode::Backspace, C),
        (KeyCode::BackTab, S), (KeyCode::Char('\0'), N), (KeyCode::Char('\u{1}'), N),
    ];
    for (code, mods) in keys {
        assert_eq!(encode(0, true, code, mods), None, "{:?} {:?}", code, mods);
        assert_eq!(encode(0, false, code, mods), None, "{:?} {:?}", code, mods);
    }
}

#[test]
fn ctrl_space_per_mode() {
    // The issue: mode 2 is the only one that can carry Ctrl+Space; mode 1
    // keeps NUL exactly as tmux's input_key_mode1 does.
    assert_eq!(x(2, KeyCode::Char(' '), C), s("\x1b[27;5;32~"));
    assert_eq!(u(2, KeyCode::Char(' '), C), s("\x1b[32;5u"));
    assert_eq!(x(1, KeyCode::Char(' '), C), None);
    // crossterm's other spelling of Ctrl+Space: a bare NUL character.
    assert_eq!(x(2, KeyCode::Char('\0'), N), s("\x1b[27;5;32~"));
}

#[test]
fn mode2_reports_every_modified_key() {
    assert_eq!(x(2, KeyCode::Char('['), C), s("\x1b[27;5;91~"));
    assert_eq!(x(2, KeyCode::Char('i'), C), s("\x1b[27;5;105~"));
    assert_eq!(x(2, KeyCode::Char('m'), C), s("\x1b[27;5;109~"));
    assert_eq!(x(2, KeyCode::Char('a'), C), s("\x1b[27;5;97~"));
    assert_eq!(x(2, KeyCode::Char('a'), M), s("\x1b[27;3;97~"));
    assert_eq!(x(2, KeyCode::Char('a'), C | M), s("\x1b[27;7;97~"));
    assert_eq!(x(2, KeyCode::Char('A'), C | S), s("\x1b[27;6;65~"));
    assert_eq!(x(2, KeyCode::Char('/'), C), s("\x1b[27;5;47~"));
    assert_eq!(x(2, KeyCode::Char('1'), C), s("\x1b[27;5;49~"));
    assert_eq!(x(2, KeyCode::Enter, S), s("\x1b[27;2;13~"));
    assert_eq!(x(2, KeyCode::Enter, C), s("\x1b[27;5;13~"));
    assert_eq!(x(2, KeyCode::Tab, C), s("\x1b[27;5;9~"));
    assert_eq!(x(2, KeyCode::Esc, M), s("\x1b[27;3;27~"));
    assert_eq!(x(2, KeyCode::Backspace, C), s("\x1b[27;5;127~"));
    // crossterm's raw control byte spelling of Ctrl+A.
    assert_eq!(x(2, KeyCode::Char('\u{1}'), N), s("\x1b[27;5;97~"));
}

#[test]
fn c_i_and_tab_c_m_and_enter_are_distinct_only_in_mode2() {
    assert_eq!(x(2, KeyCode::Tab, N), None, "Tab stays 0x09");
    assert_eq!(x(2, KeyCode::Enter, N), None, "Enter stays CR");
    assert_ne!(x(2, KeyCode::Char('i'), C), None);
    assert_ne!(x(2, KeyCode::Char('m'), C), None);
    assert_eq!(x(1, KeyCode::Char('i'), C), None, "mode 1 keeps C-i as 0x09");
    assert_eq!(x(1, KeyCode::Char('m'), C), None, "mode 1 keeps C-m as CR");
}

#[test]
fn mode1_keeps_the_classic_control_keys() {
    // input_key_mode1: Meta without Ctrl, and Ctrl with ' ', '/', '@', '^',
    // '2'..'8' or '@'..'~', stay VT10x.
    for c in [' ', '/', '@', '^', '2', '5', '8', '[', 'a', 'z', '~', 'A'] {
        assert_eq!(x(1, KeyCode::Char(c), C), None, "C-{c}");
    }
    assert_eq!(x(1, KeyCode::Char('a'), M), None);
    assert_eq!(x(1, KeyCode::Char('a'), C | M), None);
    assert_eq!(x(1, KeyCode::Enter, M), None, "M-Enter is ESC CR in mode 1");
    // Everything else is extended.
    assert_eq!(x(1, KeyCode::Char('1'), C), s("\x1b[27;5;49~"));
    assert_eq!(x(1, KeyCode::Char('9'), C), s("\x1b[27;5;57~"));
    assert_eq!(x(1, KeyCode::Char('.'), C), s("\x1b[27;5;46~"));
    assert_eq!(x(1, KeyCode::Enter, S), s("\x1b[27;2;13~"));
    assert_eq!(x(1, KeyCode::Enter, C), s("\x1b[27;5;13~"));
    assert_eq!(x(1, KeyCode::Backspace, C), s("\x1b[27;5;127~"));
}

#[test]
fn backtab_is_shift_tab_in_mode2_only() {
    assert_eq!(x(2, KeyCode::BackTab, S), s("\x1b[27;2;9~"));
    assert_eq!(x(2, KeyCode::BackTab, N), s("\x1b[27;2;9~"));
    assert_eq!(x(1, KeyCode::BackTab, S), None, "CSI Z in mode 1");
}

#[test]
fn keys_with_builtin_sequences_are_never_extended() {
    // Arrows, the editing block and function keys keep CSI 1;m x and
    // CSI n;m ~ in every mode (tmux input_key_defaults).
    for mode in [1, 2] {
        for code in [KeyCode::Up, KeyCode::Left, KeyCode::Home, KeyCode::End, KeyCode::PageUp,
                     KeyCode::Delete, KeyCode::Insert, KeyCode::F(1), KeyCode::F(5), KeyCode::F(12)] {
            for mods in [N, C, S, M, C | S] {
                assert_eq!(x(mode, code, mods), None, "{:?} {:?} mode {}", code, mods, mode);
            }
        }
    }
}

#[test]
fn plain_and_shifted_text_is_never_extended() {
    for mode in [1, 2] {
        assert_eq!(x(mode, KeyCode::Char('a'), N), None);
        assert_eq!(x(mode, KeyCode::Char('A'), S), None);
        assert_eq!(x(mode, KeyCode::Char('é'), N), None);
        assert_eq!(x(mode, KeyCode::Esc, N), None);
        assert_eq!(x(mode, KeyCode::Backspace, N), None);
    }
}

#[test]
fn altgr_characters_stay_text() {
    // AltGr reaches psmux as Ctrl+Alt with the produced character.
    for c in ['@', '\\', '{', '}', '[', ']', '|', '~', '€'] {
        assert_eq!(x(2, KeyCode::Char(c), C | M), None, "AltGr {c}");
    }
}

#[test]
fn csi_u_format() {
    assert_eq!(u(2, KeyCode::Char('['), C), s("\x1b[91;5u"));
    assert_eq!(u(2, KeyCode::Char('a'), M), s("\x1b[97;3u"));
    assert_eq!(u(2, KeyCode::Enter, S), s("\x1b[13;2u"));
    assert_eq!(u(1, KeyCode::Char('1'), C), s("\x1b[49;5u"));
}

#[test]
fn mode_names_match_tmux_pane_key_mode() {
    assert_eq!(mode_name(0), "VT10x");
    assert_eq!(mode_name(1), "Ext 1");
    assert_eq!(mode_name(2), "Ext 2");
}

// ------------------------------------------------------- state machine

#[test]
fn modset_ignored_while_option_off() {
    for level in [0, 1, 2] {
        assert_eq!(vt100::xtmodkeys_set_mode(0, 0, 4, level), 0);
        assert_eq!(vt100::xtmodkeys_set_mode(2, 0, 4, level), 2, "off leaves an existing mode alone");
    }
}

#[test]
fn modset_with_option_on() {
    assert_eq!(vt100::xtmodkeys_set_mode(0, 1, 4, 1), 1);
    assert_eq!(vt100::xtmodkeys_set_mode(0, 1, 4, 2), 2);
    assert_eq!(vt100::xtmodkeys_set_mode(2, 1, 4, 0), 0, "level 0 clears");
    assert_eq!(vt100::xtmodkeys_set_mode(2, 1, 4, 1), 1);
    assert_eq!(vt100::xtmodkeys_set_mode(0, 1, 5, 2), 0, "only resource 4 is modifyOtherKeys");
}

#[test]
fn modset_with_option_always() {
    assert_eq!(vt100::xtmodkeys_set_mode(1, 2, 4, 0), 1, "always keeps at least mode 1");
    assert_eq!(vt100::xtmodkeys_set_mode(1, 2, 4, 2), 2);
    assert_eq!(vt100::xtmodkeys_set_mode(2, 2, 4, 1), 1);
}

#[test]
fn modoff_returns_to_the_base_mode() {
    assert_eq!(vt100::xtmodkeys_off_mode(2, 0, 4), 0);
    assert_eq!(vt100::xtmodkeys_off_mode(2, 1, 4), 0);
    assert_eq!(vt100::xtmodkeys_off_mode(2, 2, 4), 1);
    assert_eq!(vt100::xtmodkeys_off_mode(2, 1, 3), 2, "other resources do nothing");
}

// The parser plumbing, independent of the process wide option (other tests
// may set it): sequences that do not depend on it, and the explicit carry.

#[test]
fn parser_routes_xtmodkeys_and_ignores_other_resources() {
    let mut p = vt100::Parser::new(5, 20, 0);
    let base = p.screen().extended_keys_mode();
    p.process(b"\x1b[>5;2m");
    assert_eq!(p.screen().extended_keys_mode(), base, "resource 5 is not modifyOtherKeys");
    p.screen_mut().set_extended_keys_mode(2);
    p.process(b"\x1b[>5n");
    assert_eq!(p.screen().extended_keys_mode(), 2, "CSI > 5 n leaves the key mode");
    p.process(b"\x1b[>4n");
    assert_ne!(p.screen().extended_keys_mode(), 2, "CSI > 4 n leaves mode 2");
}

#[test]
fn ris_resets_the_key_mode() {
    let mut p = vt100::Parser::new(5, 20, 0);
    p.screen_mut().set_extended_keys_mode(2);
    p.process(b"\x1bc");
    assert_ne!(p.screen().extended_keys_mode(), 2, "RIS is a new screen");
}

#[test]
fn xtmodkeys_is_not_sgr() {
    // `CSI > 4 ; 2 m` ends in `m` like SGR; it must not bold or colour text.
    let mut p = vt100::Parser::new(5, 20, 0);
    p.process(b"\x1b[>4;1mX");
    let cell = p.screen().cell(0, 0).unwrap();
    assert_eq!(cell.contents(), "X");
    assert!(!cell.bold());
    assert!(!cell.underline());
}
