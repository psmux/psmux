// Issue #616 follow up: under a Russian layout the Y key types `н`, and the
// confirm prompt only knew `y`, so `prefix x` could not be confirmed. These
// tests pin tmux's confirm-before (cmd-confirm-before.c): the flags
// `-b -c -p -t -y`, the default prompt, one key answering, Enter only with
// -y, and psmux's additions for non Latin layouts.

use super::*;
use crate::types::{AppState, Mode};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

// A Russian layout as Windows reports it: the Y key (VK_Y, 0x59) types `н`,
// the N key (VK_N, 0x4E) types `т`.
fn russian(c: char) -> Option<u16> {
    match c {
        'н' => Some(0x59),
        'т' => Some(0x4E),
        _ => None,
    }
}

// ── flags ────────────────────────────────────────────────────────────────

#[test]
fn no_flags_means_y_and_enter_is_no() {
    let (spec, start) = parse_confirm_args(&args("kill-pane")).unwrap();
    assert_eq!(spec, ConfirmSpec::default());
    assert_eq!(spec.key, 'y');
    assert!(!spec.default_yes);
    assert_eq!(start, 0);
}

#[test]
fn every_tmux_flag_is_read_and_left_out_of_the_command() {
    let a = args("-b -y -c z -p ok? -t client0 kill-window -a");
    let (spec, start) = parse_confirm_args(&a).unwrap();
    assert!(spec.background);
    assert!(spec.default_yes);
    assert_eq!(spec.key, 'z');
    assert_eq!(spec.prompt.as_deref(), Some("ok?"));
    assert_eq!(spec.target.as_deref(), Some("client0"));
    assert_eq!(&a[start..], &["kill-window".to_string(), "-a".to_string()]);
}

#[test]
fn flags_cluster_as_getopt_reads_them() {
    let (spec, start) = parse_confirm_args(&args("-yb kill-pane")).unwrap();
    assert!(spec.default_yes && spec.background);
    assert_eq!(start, 1);
    // A value flag takes the rest of its cluster.
    let (spec, start) = parse_confirm_args(&args("-cн kill-pane")).unwrap();
    assert_eq!(spec.key, 'н');
    assert_eq!(start, 1);
    let (spec, _) = parse_confirm_args(&args("-ycq kill-pane")).unwrap();
    assert!(spec.default_yes);
    assert_eq!(spec.key, 'q');
}

#[test]
fn a_non_latin_confirm_key_is_accepted() {
    let (spec, start) = parse_confirm_args(&args("-c н kill-pane")).unwrap();
    assert_eq!(spec.key, 'н');
    assert_eq!(start, 2);
}

#[test]
fn a_confirm_key_must_be_one_printable_character() {
    assert_eq!(parse_confirm_args(&args("-c ab kill-pane")).unwrap_err(), "invalid confirm key");
    assert_eq!(parse_confirm_args(&["-c", "\u{7}", "kill-pane"]).unwrap_err(), "invalid confirm key");
    assert!(parse_confirm_args(&args("-c")).is_err(), "-c with no value");
    assert!(parse_confirm_args(&args("-x kill-pane")).is_err(), "unknown flag");
}

#[test]
fn double_dash_ends_the_flags() {
    let a = args("-y -- -weird");
    let (spec, start) = parse_confirm_args(&a).unwrap();
    assert!(spec.default_yes);
    assert_eq!(&a[start..], &["-weird".to_string()]);
}

// ── prompt ───────────────────────────────────────────────────────────────

#[test]
fn the_default_prompt_is_tmux_s_and_names_the_confirm_key() {
    let spec = ConfirmSpec::default();
    assert_eq!(prompt_text(&spec, "kill-pane -t %1"), "Confirm 'kill-pane'? (y/n)");
    let spec = ConfirmSpec { key: 'н', ..ConfirmSpec::default() };
    assert_eq!(prompt_text(&spec, "kill-pane"), "Confirm 'kill-pane'? (н/n)");
}

#[test]
fn a_given_prompt_is_shown_as_given() {
    let spec = ConfirmSpec { prompt: Some("kill-pane #P? (y/n)".into()), ..ConfirmSpec::default() };
    assert_eq!(prompt_text(&spec, "kill-pane"), "kill-pane #P? (y/n)");
}

// ── answers ──────────────────────────────────────────────────────────────

#[test]
fn only_the_confirm_key_says_yes() {
    let none = |_| None;
    assert!(is_yes_with('y', false, ConfirmInput::Char('y'), none));
    assert!(is_yes_with('y', false, ConfirmInput::Char('Y'), none), "Caps Lock still says yes");
    for c in ['n', 'N', 'q', ' ', 'x', 'z'] {
        assert!(!is_yes_with('y', false, ConfirmInput::Char(c), none), "{c:?} is a no");
    }
    assert!(!is_yes_with('y', false, ConfirmInput::Other, none), "Escape is a no");
}

#[test]
fn enter_says_yes_only_with_dash_y() {
    let none = |_| None;
    assert!(!is_yes_with('y', false, ConfirmInput::Enter, none));
    assert!(is_yes_with('y', true, ConfirmInput::Enter, none));
}

#[test]
fn a_custom_confirm_key_replaces_y() {
    let none = |_| None;
    assert!(is_yes_with('н', false, ConfirmInput::Char('н'), none));
    assert!(is_yes_with('н', false, ConfirmInput::Char('Н'), none));
    assert!(!is_yes_with('н', false, ConfirmInput::Char('y'), none));
    assert!(is_yes_with('z', false, ConfirmInput::Char('z'), none));
    assert!(!is_yes_with('z', false, ConfirmInput::Char('y'), none));
}

#[test]
fn the_y_key_of_a_russian_layout_says_yes() {
    assert!(is_yes_with('y', false, ConfirmInput::Char('н'), russian));
    // The N key's `т` is not the Y key.
    assert!(!is_yes_with('y', false, ConfirmInput::Char('т'), russian));
    // A character no layout has a key for is a no.
    assert!(!is_yes_with('y', false, ConfirmInput::Char('ж'), russian));
}

#[test]
fn a_latin_key_never_stands_for_another_key() {
    // On QWERTZ the key where QWERTY has Y types z, under VK_Z: it is a
    // no, exactly as in tmux. Only characters outside ASCII are looked up.
    let qwertz = |c: char| match c { 'z' => Some(0x5A), 'y' => Some(0x59), _ => None };
    assert!(!is_yes_with('y', false, ConfirmInput::Char('z'), qwertz));
    let lies = |_| Some(0x59u16);
    assert!(!is_yes_with('y', false, ConfirmInput::Char('z'), lies), "ASCII is compared as typed");
}

#[test]
fn a_bare_modifier_does_not_answer() {
    use crossterm::event::ModifierKeyCode;
    assert!(!ConfirmInput::answers(KeyCode::Modifier(ModifierKeyCode::LeftShift)));
    assert!(!ConfirmInput::answers(KeyCode::CapsLock));
    assert!(ConfirmInput::answers(KeyCode::Char('n')));
    assert!(ConfirmInput::answers(KeyCode::Esc));
    assert!(ConfirmInput::answers(KeyCode::F(1)));
}

// ── a binding's line, as the attached client reads it ───────────────────

#[test]
fn the_default_x_binding_reads_as_before() {
    let p = PendingConfirm::from_line("confirm-before -p 'kill-pane #P? (y/n)' kill-pane").unwrap();
    assert_eq!(p.command, "kill-pane");
    assert_eq!(p.prompt, "kill-pane #P? (y/n)");
    assert_eq!(p.key, 'y');
    assert!(!p.default_yes);
}

#[test]
fn a_binding_with_c_and_y_keeps_its_command_intact() {
    let p = PendingConfirm::from_line("confirm -y -c н \"display-message 'a b'\"").unwrap();
    assert_eq!(p.key, 'н');
    assert!(p.default_yes);
    assert_eq!(p.prompt, "Confirm 'display-message'? (н/n)");
    // One argument is a command string, as in tmux: it runs as written.
    assert_eq!(p.command, "display-message 'a b'");
    assert_eq!(crate::commands::parse_command_line(&p.command), vec!["display-message".to_string(), "a b".to_string()]);
}

#[test]
fn several_words_are_joined_so_they_parse_back_the_same() {
    let p = PendingConfirm::from_line("confirm-before display-message 'a b'").unwrap();
    assert_eq!(crate::commands::parse_command_line(&p.command), vec!["display-message".to_string(), "a b".to_string()]);
}

#[test]
fn a_bad_binding_line_is_refused() {
    assert!(PendingConfirm::from_line("confirm-before -c ab kill-pane").is_err());
}

// ── the server's prompt (AppState) ───────────────────────────────────────

fn app() -> AppState {
    let mut app = AppState::new("t616".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn confirmed(app: &AppState) -> bool {
    app.user_options.get("@c616").map(String::as_str) == Some("yes")
}

#[test]
fn the_command_prompt_route_keeps_the_flags_out_of_the_command() {
    // Before: the flags were stored as part of the command, so a yes ran
    // `-c н -p ok? set ...`, which is no command at all.
    let mut a = app();
    crate::commands::execute_command_string(&mut a, "confirm-before -c н -p ok? \"set -g @c616 yes\"").unwrap();
    match &a.mode {
        Mode::ConfirmMode { prompt, command, key, default_yes, .. } => {
            assert_eq!(command, "set -g @c616 yes");
            assert_eq!(prompt, "ok?");
            assert_eq!(*key, 'н');
            assert!(!default_yes);
        }
        other => panic!("expected ConfirmMode, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn the_server_prompt_takes_the_confirm_key() {
    let mut a = app();
    crate::commands::execute_command_string(&mut a, "confirm-before -c н \"set -g @c616 yes\"").unwrap();
    crate::input::handle_key(&mut a, press(KeyCode::Char('н'))).unwrap();
    assert!(matches!(a.mode, Mode::Passthrough));
    assert!(confirmed(&a), "н with -c н must run the command");
}

#[test]
fn the_server_prompt_closes_on_any_other_key() {
    for code in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Enter, KeyCode::Char('n')] {
        let mut a = app();
        crate::commands::execute_command_string(&mut a, "confirm-before \"set -g @c616 yes\"").unwrap();
        crate::input::handle_key(&mut a, press(code)).unwrap();
        assert!(matches!(a.mode, Mode::Passthrough), "{code:?} closes the prompt");
        assert!(!confirmed(&a), "{code:?} must not run the command");
    }
}

#[test]
fn the_server_prompt_takes_enter_with_dash_y() {
    let mut a = app();
    crate::commands::execute_command_string(&mut a, "confirm-before -y \"set -g @c616 yes\"").unwrap();
    crate::input::handle_key(&mut a, press(KeyCode::Enter)).unwrap();
    assert!(confirmed(&a));
}

#[test]
fn a_modifier_alone_leaves_the_server_prompt_open() {
    use crossterm::event::ModifierKeyCode;
    let mut a = app();
    crate::commands::execute_command_string(&mut a, "confirm-before \"set -g @c616 yes\"").unwrap();
    crate::input::handle_key(&mut a, press(KeyCode::Modifier(ModifierKeyCode::LeftShift))).unwrap();
    assert!(matches!(a.mode, Mode::ConfirmMode { .. }));
    crate::input::handle_key(&mut a, press(KeyCode::Char('Y'))).unwrap();
    assert!(confirmed(&a));
}
