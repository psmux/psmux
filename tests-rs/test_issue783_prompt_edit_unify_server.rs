//! Issue #783: the server side command prompt goes through the shared prompt
//! editor too.
//!
//! `Mode::CommandPrompt` was the same editor written a third time, and it
//! failed the other way round from the client's overlay: its text arm carried
//! no Ctrl guard, so an unhandled `C-b` typed a bare `b` into the line, and it
//! advanced its byte cursor by one per character typed, which left the index
//! inside a multi byte character.
//!
//! This path is not reachable from a client today: neither
//! `send_key_to_active` nor `send_text_to_active` has a `Mode::CommandPrompt`
//! branch, and `handle_key` has no caller outside tests. The prompt is still
//! set by `command-prompt` on the server side, so these tests pin what it does
//! with a key when something does reach it.
//!
//! Registered from src/input.rs.

use super::*;


/// A server with nothing but a command prompt open on `text`, the cursor at
/// the end.
fn prompt_app(text: &str) -> AppState {
    let mut app = AppState::new("promptedit".to_string());
    app.mode = Mode::CommandPrompt {
        input: text.to_string(),
        cursor: text.len(),
    };
    app
}

fn press(app: &mut AppState, code: KeyCode, mods: KeyModifiers) {
    handle_key(app, KeyEvent::new(code, mods)).expect("handle_key");
}

fn press_ctrl(app: &mut AppState, c: char) {
    press(app, KeyCode::Char(c), KeyModifiers::CONTROL);
}

fn prompt_state(app: &AppState) -> (String, usize) {
    match &app.mode {
        Mode::CommandPrompt { input, cursor } => (input.clone(), *cursor),
        _ => panic!("the prompt is no longer open"),
    }
}

#[test]
fn server_prompt_control_keys_edit_instead_of_typing_a_letter() {
    // The text arm had no Ctrl guard, so each of these inserted its own letter.
    let mut app = prompt_app("abc");
    press_ctrl(&mut app, 'b');
    assert_eq!(prompt_state(&app), ("abc".to_string(), 2), "C-b is one left");
    press_ctrl(&mut app, 'f');
    assert_eq!(prompt_state(&app), ("abc".to_string(), 3));
    press_ctrl(&mut app, 'h');
    assert_eq!(prompt_state(&app), ("ab".to_string(), 2));
    press_ctrl(&mut app, 'a');
    assert_eq!(prompt_state(&app), ("ab".to_string(), 0));
    press_ctrl(&mut app, 'd');
    assert_eq!(prompt_state(&app), ("b".to_string(), 0));
    press_ctrl(&mut app, 'e');
    assert_eq!(prompt_state(&app), ("b".to_string(), 1));
}

#[test]
fn server_prompt_c_u_clears_the_whole_line() {
    let mut app = prompt_app("one two");
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    press_ctrl(&mut app, 'u');
    assert_eq!(prompt_state(&app), (String::new(), 0));
}

#[test]
fn server_prompt_c_w_cuts_the_word_before_the_cursor() {
    let mut app = prompt_app("ls one two");
    press_ctrl(&mut app, 'w');
    assert_eq!(prompt_state(&app), ("ls one ".to_string(), 7));
}

#[test]
fn server_prompt_c_c_and_c_g_cancel_the_prompt() {
    for c in ['c', 'g'] {
        let mut app = prompt_app("half typed");
        press_ctrl(&mut app, c);
        assert!(
            matches!(app.mode, Mode::Passthrough),
            "C-{} must end the prompt, not type a letter",
            c
        );
    }
}

#[test]
fn server_prompt_moves_the_byte_cursor_by_characters() {
    // Two characters, six bytes. The old arms moved the index by one, which
    // left it inside a character: the next key indexed the line with it.
    let mut app = prompt_app("日本");
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日本".to_string(), 3));
    press(&mut app, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日本".to_string(), 0));
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日本".to_string(), 3));
    press(&mut app, KeyCode::Delete, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日".to_string(), 3));
}

#[test]
fn server_prompt_types_two_multi_byte_characters_in_a_row() {
    // `*cursor += 1` after inserting a three byte character left the index at
    // 1, and the next insert at a byte that is not a character boundary
    // panicked.
    let mut app = prompt_app("");
    press(&mut app, KeyCode::Char('日'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('本'), KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日本".to_string(), 6));
    press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("日".to_string(), 3));
}

#[test]
fn server_prompt_history_steps_with_up_down_and_c_p_c_n() {
    let mut app = prompt_app("");
    app.command_history = vec!["first".to_string(), "second".to_string()];
    app.command_history_idx = app.command_history.len();

    press(&mut app, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), ("second".to_string(), 6));
    press_ctrl(&mut app, 'p');
    assert_eq!(prompt_state(&app), ("first".to_string(), 5));
    press_ctrl(&mut app, 'p');
    assert_eq!(prompt_state(&app), ("first".to_string(), 5), "stops at the oldest");
    press_ctrl(&mut app, 'n');
    assert_eq!(prompt_state(&app), ("second".to_string(), 6));
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(prompt_state(&app), (String::new(), 0), "and then an empty line");
}

#[test]
fn server_prompt_keeps_a_key_it_has_no_use_for() {
    for (code, mods) in [
        (KeyCode::Char('l'), KeyModifiers::CONTROL),
        (KeyCode::Char('x'), KeyModifiers::CONTROL),
        (KeyCode::Tab, KeyModifiers::NONE),
        (KeyCode::PageUp, KeyModifiers::NONE),
        (KeyCode::F(5), KeyModifiers::NONE),
    ] {
        let mut app = prompt_app("ls");
        press(&mut app, code, mods);
        assert_eq!(
            prompt_state(&app),
            ("ls".to_string(), 2),
            "{:?} with {:?} must leave the line alone",
            code,
            mods
        );
    }
}

#[test]
fn server_prompt_takes_an_altgr_character_as_text() {
    // Windows reports AltGr as Ctrl+Alt, so `@` on a German layout arrives as
    // Ctrl+Alt+@. The Ctrl guard added here must not swallow it.
    let mut app = prompt_app("mail");
    press(
        &mut app,
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    );
    assert_eq!(prompt_state(&app), ("mail@".to_string(), 5));
}
