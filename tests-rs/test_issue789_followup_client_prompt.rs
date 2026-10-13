// #789 follow up: `command-prompt -k` and `-1` on the client's prompt, the one
// a prefix table binding opens.
//
// The client's flag parser dropped both flags, so `bind K command-prompt -k -p
// key "..."` opened an ordinary prompt: Left did nothing and Enter ran the
// template with an empty answer. tmux answers a -k prompt with the key's name
// (key_string_lookup_key, key-string.c:327, called at status.c:1439) and a -1
// prompt with the first character (status.c:1459 to :1470). tmux's own default
// `prefix /` binding is `command-prompt -k -p key { list-keys -1N "%%" }`.
//
// The names below were measured in tmux 3.4 through a nested client.
//
// Registered from src/client.rs.

use super::*;

fn spec_of(args: &str) -> CommandPromptSpec {
    parse_command_prompt_args(args)
}

#[test]
fn k_and_1_are_read_as_flags() {
    let s = spec_of("-k -p key \"set -g @k '%%'\"");
    assert!(s.key && !s.single);
    assert_eq!(s.label.as_deref(), Some("key"));
    assert_eq!(s.template.as_deref(), Some("set -g @k '%%'"));
    let s = spec_of("-1 -p one \"rename-window %%\"");
    assert!(s.single && !s.key);
    // Clustered, as tmux's own `/` binding writes it.
    let s = spec_of("-kpkey \"list-keys -1N '%%'\"");
    assert!(s.key);
    assert_eq!(s.label.as_deref(), Some("key"));
    let s = spec_of("-p x \"rename-window %%\"");
    assert!(!s.key && !s.single);
}

#[test]
fn a_k_prompt_names_keys_the_way_tmux_does() {
    let none = KeyModifiers::NONE;
    let ctrl = KeyModifiers::CONTROL;
    let alt = KeyModifiers::ALT;
    let shift = KeyModifiers::SHIFT;
    let cases: &[(KeyCode, KeyModifiers, &str)] = &[
        (KeyCode::Left, none, "Left"), (KeyCode::Right, none, "Right"),
        (KeyCode::Up, none, "Up"), (KeyCode::Down, none, "Down"),
        (KeyCode::Home, none, "Home"), (KeyCode::End, none, "End"),
        (KeyCode::PageUp, none, "PPage"), (KeyCode::PageDown, none, "NPage"),
        (KeyCode::Insert, none, "IC"), (KeyCode::Delete, none, "DC"),
        (KeyCode::F(1), none, "F1"), (KeyCode::F(12), none, "F12"),
        (KeyCode::Enter, none, "Enter"), (KeyCode::Esc, none, "Escape"),
        (KeyCode::Backspace, none, "BSpace"), (KeyCode::Tab, none, "Tab"),
        // The console reports Shift+Tab as BackTab WITH Shift.
        (KeyCode::BackTab, shift, "BTab"),
        (KeyCode::Char(' '), none, "Space"),
        (KeyCode::Char('a'), ctrl, "C-a"), (KeyCode::Char('a'), alt, "M-a"),
        (KeyCode::Char('a'), ctrl | alt, "C-M-a"),
        (KeyCode::Left, ctrl, "C-Left"), (KeyCode::Left, shift, "S-Left"),
        (KeyCode::Left, alt, "M-Left"), (KeyCode::Up, ctrl | shift, "C-S-Up"),
        (KeyCode::F(5), ctrl, "C-F5"), (KeyCode::F(5), shift, "S-F5"),
        (KeyCode::Char('a'), none, "a"),
        // Shift is part of the character: tmux names it `A`, not `S-A`.
        (KeyCode::Char('A'), shift, "A"),
        (KeyCode::Char('1'), none, "1"),
    ];
    for (code, mods, want) in cases {
        assert_eq!(prompt_key_answer_name(*code, *mods), *want, "{code:?} {mods:?}");
    }
}
