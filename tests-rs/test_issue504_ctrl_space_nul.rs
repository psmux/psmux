// Issue #504: Ctrl+Space prefix does not work in Alacritty on Windows.
//
// Established by measurement before any code was read (see
// tests/test_issue504_ctrl_space_nul.ps1 for the end-to-end proof):
//
//   * Alacritty on Windows sends 0x20 for Ctrl+Space, i.e. a plain Space with
//     no Ctrl flag.  That is an upstream Alacritty defect and nothing psmux can
//     detect: the record is identical to an unmodified Space.
//   * Alacritty's documented workaround, binding Ctrl+Space to send a literal
//     NUL, DOES reach the console.  ConPTY encodes that NUL as
//     `vk=VK_2 (0x32), UnicodeChar=0, ctrl=CTRL|SHIFT`, because NUL is
//     classically typed as Ctrl+@ (Shift+2 on a US layout).  crossterm resolves
//     the zero UnicodeChar through the keyboard layout and hands psmux
//     `Char('2')` with CONTROL.
//   * psmux never folded that back to C-Space, so the workaround was dead.
//
// tmux folds an incoming NUL to C-Space in tty-keys.c:
//     /* C-Space is special. */
//     if ((key & KEYC_MASK_KEY) == C0_NUL)
//             key = ' ' | KEYC_CTRL | (key & KEYC_META);
//
// These tests lock in the same fold, and lock in that it does NOT swallow
// neighbouring keys or break an existing `bind-key C-2`.

use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

const CTRL: KeyModifiers = KeyModifiers::CONTROL;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const ALT: KeyModifiers = KeyModifiers::ALT;
const NONE: KeyModifiers = KeyModifiers::NONE;

fn ctrl_space() -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(' '), CTRL)
}

// ---------------------------------------------------------------------------
// The fold itself
// ---------------------------------------------------------------------------

#[test]
fn conpty_nul_record_folds_to_ctrl_space() {
    // Exactly what crossterm hands us for the NUL that Alacritty's workaround
    // sends: VK_2 with a zero UnicodeChar and CTRL|SHIFT held.
    let got = fold_nul_to_ctrl_space((KeyCode::Char('2'), CTRL | SHIFT));
    assert_eq!(
        got,
        ctrl_space(),
        "the ConPTY NUL record must fold to C-Space, else `set -g prefix C-Space` is dead (#504)"
    );
}

#[test]
fn ctrl_2_without_shift_folds_to_ctrl_space() {
    // A physical Ctrl+2 press produces the same record minus the SHIFT bit.
    // In terminal terms Ctrl+2 IS NUL, so it folds too (tmux parity).
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('2'), CTRL)), ctrl_space());
}

#[test]
fn literal_nul_char_folds_to_ctrl_space() {
    // A raw 0x00 that reaches the key layer as a character.
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('\0'), NONE)), ctrl_space());
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('\0'), CTRL)), ctrl_space());
}

#[test]
fn ctrl_space_itself_is_unchanged() {
    // Terminals that report Ctrl+Space properly (conhost, Windows Terminal)
    // must keep working untouched.
    assert_eq!(fold_nul_to_ctrl_space(ctrl_space()), ctrl_space());
}

// ---------------------------------------------------------------------------
// The fold must not be greedy
// ---------------------------------------------------------------------------

#[test]
fn plain_2_is_not_folded() {
    // Typing "2" must stay "2".  This is the regression that would break every
    // user who types a digit.
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('2'), NONE)), (KeyCode::Char('2'), NONE));
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('2'), SHIFT)), (KeyCode::Char('2'), SHIFT));
}

#[test]
fn alt_2_and_ctrl_alt_2_are_not_folded() {
    // Alt+2 is a window-selection key in many configs, and Ctrl+Alt is how
    // Windows reports AltGr — neither is NUL.
    assert_eq!(fold_nul_to_ctrl_space((KeyCode::Char('2'), ALT)), (KeyCode::Char('2'), ALT));
    assert_eq!(
        fold_nul_to_ctrl_space((KeyCode::Char('2'), CTRL | ALT)),
        (KeyCode::Char('2'), CTRL | ALT)
    );
}

#[test]
fn other_ctrl_digits_are_not_folded() {
    // Only Ctrl+2 is NUL.  Ctrl+1, Ctrl+3 .. Ctrl+9 must pass through.
    for c in ['1', '3', '4', '5', '6', '7', '8', '9', '0'] {
        assert_eq!(
            fold_nul_to_ctrl_space((KeyCode::Char(c), CTRL)),
            (KeyCode::Char(c), CTRL),
            "Ctrl+{} must not fold to C-Space",
            c
        );
    }
}

#[test]
fn ctrl_letters_are_not_folded() {
    // The default prefix C-b in particular must be untouched.
    for c in ['a', 'b', 'c', 'z'] {
        assert_eq!(
            fold_nul_to_ctrl_space((KeyCode::Char(c), CTRL)),
            (KeyCode::Char(c), CTRL),
            "Ctrl+{} must not fold to C-Space",
            c
        );
    }
}

#[test]
fn non_char_keys_are_untouched() {
    for code in [KeyCode::Enter, KeyCode::Tab, KeyCode::Esc, KeyCode::Up, KeyCode::F(2)] {
        assert_eq!(fold_nul_to_ctrl_space((code, CTRL)), (code, CTRL));
    }
}

// ---------------------------------------------------------------------------
// Binding lookup: the fold is applied symmetrically
// ---------------------------------------------------------------------------

#[test]
fn normalize_folds_nul_for_binding_lookup() {
    // An incoming NUL and a `C-Space` binding must land on the same tuple.
    let incoming = normalize_key_for_binding((KeyCode::Char('2'), CTRL | SHIFT));
    let bound = normalize_key_for_binding(parse_key_string("C-Space").expect("C-Space parses"));
    assert_eq!(incoming, bound, "NUL must match a registered C-Space binding (#504)");
}

#[test]
fn existing_ctrl_2_binding_still_fires() {
    // Symmetry guard: because the fold runs on registered keys too, a user who
    // wrote `bind-key C-2` keeps working — their binding and the incoming key
    // both normalize to C-Space.
    let incoming = normalize_key_for_binding((KeyCode::Char('2'), CTRL));
    let bound = normalize_key_for_binding(parse_key_string("C-2").expect("C-2 parses"));
    assert_eq!(incoming, bound, "an existing `bind-key C-2` must not stop firing");
}

#[test]
fn c_space_and_c_2_are_the_same_binding_key() {
    // Consequence of the above, stated directly: on Windows these two names
    // denote the same physical key, exactly as they do for a terminal.
    let a = normalize_key_for_binding(parse_key_string("C-Space").unwrap());
    let b = normalize_key_for_binding(parse_key_string("C-2").unwrap());
    assert_eq!(a, b);
}

#[test]
fn unrelated_bindings_still_resolve_distinctly() {
    // Guard against the fold collapsing keys it should not touch.
    let names = ["C-b", "C-a", "C-1", "C-3", "M-2", "b", "2"];
    for i in 0..names.len() {
        for j in (i + 1)..names.len() {
            let a = normalize_key_for_binding(parse_key_string(names[i]).unwrap());
            let b = normalize_key_for_binding(parse_key_string(names[j]).unwrap());
            assert_ne!(a, b, "'{}' and '{}' must stay distinct bindings", names[i], names[j]);
        }
    }
}

// ---------------------------------------------------------------------------
// The reporter's configuration, end to end through the config parser
// ---------------------------------------------------------------------------

#[test]
fn prefix_c_space_from_config_matches_a_nul_key() {
    let mut app = AppState::new("t504".to_string());
    parse_config_content(&mut app, "set -g prefix C-Space\n");
    assert_eq!(app.prefix_key, Some(ctrl_space()), "`set -g prefix C-Space` must store C-Space");

    // The key the terminal actually delivers, after the client-side fold.
    let delivered = fold_nul_to_ctrl_space((KeyCode::Char('2'), CTRL | SHIFT));
    assert_eq!(
        Some(delivered), app.prefix_key,
        "the delivered NUL must equal the configured prefix, or the prefix never arms (#504)"
    );
}

#[test]
fn prefix_c_b_is_unaffected_by_the_fold() {
    // The default prefix must not be perturbed by any of this.
    let mut app = AppState::new("t504b".to_string());
    parse_config_content(&mut app, "set -g prefix C-b\n");
    assert_eq!(app.prefix_key, Some((KeyCode::Char('b'), CTRL)));
    assert_ne!(Some(fold_nul_to_ctrl_space((KeyCode::Char('2'), CTRL))), app.prefix_key);
}
