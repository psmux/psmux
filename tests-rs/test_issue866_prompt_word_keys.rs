// Issue #866: the prompts move by word on M-f, C-Right, M-b and C-Left, and
// C-w splits words the same way, with the word-separators option, as tmux's
// prompt does (status_prompt_forward_word and status_prompt_backward_word,
// status.c:1320 and :1399, the keys at :1558, :1598 and :1620, tag 3.7c).
// Discussion #782, items 3 and 8.
//
// Every expected position is what tmux 3.7c showed at its own command prompt
// with the default word-separators, measured through a nested client: the
// line was typed, the key pressed 1 to 6 times from the start (forward) or
// the end (backward), then `|` typed so the cursor showed. C-w was pressed
// twice from the end.
//
// Registered from src/copy_mode.rs.

use super::*;
use super::test_copy_mode_snapshot::app_with_pane;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// tmux's default word-separators (options-table.c:1064 to :1072).
const TMUX_SEPS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~";

/// The cursor, as a character index from the start, after each of `steps`
/// presses of `key`, starting at the start of `text` or at its end.
fn walk(text: &str, from_start: bool, key: &str, steps: usize) -> Vec<usize> {
    let mut input = text.to_string();
    let len = input.chars().count();
    let mut back = if from_start { len } else { 0 };
    let mut out = Vec::new();
    for _ in 0..steps {
        assert_eq!(prompt_edit(&mut input, &mut back, &mut None, TMUX_SEPS, key, || None), PromptEdit::Edited);
        assert_eq!(input, text, "{key} moves, it does not edit");
        out.push(len - back);
    }
    out
}

/// The line after each of two C-w presses from the end.
fn cut_twice(text: &str, seps: &str) -> (String, String) {
    let mut input = text.to_string();
    let mut back = 0usize;
    let mut saved = None;
    prompt_edit(&mut input, &mut back, &mut saved, seps, "C-w", || None);
    let first = input.clone();
    prompt_edit(&mut input, &mut back, &mut saved, seps, "C-w", || None);
    (first, input)
}

const LS: &str = "ls /usr/local";
const MIXED: &str = "foo-bar_baz qux.quux  a@b:c";
const CJK: &str = "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} abc\u{65e5}\u{672c} def";
const SPACES: &str = "  lead  spaces,  end  ";

// ── the editor ──────────────────────────────────────────────────────────────

#[test]
fn m_f_and_c_right_stop_where_tmux_does() {
    let cases: &[(&str, [usize; 6])] = &[
        (LS, [2, 4, 7, 8, 13, 13]),
        (MIXED, [3, 4, 11, 15, 16, 20]),
        (CJK, [8, 14, 18, 18, 18, 18]),
        (SPACES, [6, 14, 15, 20, 22, 22]),
    ];
    for (text, want) in cases {
        for key in ["M-f", "C-Right"] {
            assert_eq!(walk(text, true, key, 6), want.to_vec(), "{key} over [{text}]");
        }
    }
}

#[test]
fn m_b_and_c_left_stop_where_tmux_does() {
    let cases: &[(&str, [usize; 6])] = &[
        (LS, [8, 7, 4, 3, 0, 0]),
        (MIXED, [26, 25, 24, 23, 22, 16]),
        (CJK, [15, 9, 0, 0, 0, 0]),
        (SPACES, [17, 14, 8, 2, 0, 0]),
    ];
    for (text, want) in cases {
        for key in ["M-b", "C-Left"] {
            assert_eq!(walk(text, false, key, 6), want.to_vec(), "{key} over [{text}]");
        }
    }
}

#[test]
fn c_w_cuts_back_to_where_m_b_goes() {
    // tmux's capture showed these lines with their trailing spaces trimmed.
    let cases = [
        (LS, "ls /usr/", "ls /usr"),
        (MIXED, "foo-bar_baz qux.quux  a@b:", "foo-bar_baz qux.quux  a@b"),
        (
            CJK,
            "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} abc\u{65e5}\u{672c} ",
            "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8} ",
        ),
        (SPACES, "  lead  spaces,  ", "  lead  spaces"),
    ];
    for (text, first, second) in cases {
        assert_eq!(cut_twice(text, TMUX_SEPS), (first.to_string(), second.to_string()), "C-w over [{text}]");
    }
}

#[test]
fn c_w_keeps_what_it_cut_for_c_y() {
    let mut input = LS.to_string();
    let mut back = 0usize;
    let mut saved = None;
    prompt_edit(&mut input, &mut back, &mut saved, TMUX_SEPS, "C-w", || None);
    assert_eq!(saved.as_deref(), Some("local"));
    prompt_edit(&mut input, &mut back, &mut saved, TMUX_SEPS, "C-y", || None);
    assert_eq!(input, LS);
}

#[test]
fn with_no_separators_only_spaces_split_words() {
    assert_eq!(cut_twice(LS, ""), ("ls ".to_string(), String::new()));
    let mut input = MIXED.to_string();
    let mut back = MIXED.chars().count();
    prompt_edit(&mut input, &mut back, &mut None, "", "M-f", || None);
    assert_eq!(MIXED.chars().count() - back, 11);
}

#[test]
fn the_word_keys_work_from_inside_a_word_and_on_an_empty_line() {
    // From the `s` of `/usr`, M-b goes to the start of `usr` and M-f to its end.
    let mut input = LS.to_string();
    let mut back = LS.len() - 5;
    prompt_edit(&mut input, &mut back, &mut None, TMUX_SEPS, "M-b", || None);
    assert_eq!(LS.len() - back, 4);
    let mut back = LS.len() - 5;
    prompt_edit(&mut input, &mut back, &mut None, TMUX_SEPS, "M-f", || None);
    assert_eq!(LS.len() - back, 7);
    let mut empty = String::new();
    let mut back = 0usize;
    for key in ["M-f", "M-b", "C-Right", "C-Left", "C-w"] {
        assert_eq!(prompt_edit(&mut empty, &mut back, &mut None, TMUX_SEPS, key, || None), PromptEdit::Edited);
        assert_eq!((empty.as_str(), back), ("", 0), "{key} on an empty line");
    }
}

#[test]
fn the_lowercase_names_send_keys_uses_are_the_same_keys() {
    for (upper, lower, from_start) in [("M-f", "m-f", true), ("C-Right", "c-right", true), ("M-b", "m-b", false), ("C-Left", "c-left", false)] {
        assert_eq!(walk(LS, from_start, upper, 3), walk(LS, from_start, lower, 3), "{lower}");
    }
}

// ── the prompts that reach it ───────────────────────────────────────────────

#[test]
fn the_client_command_prompt_moves_by_word() {
    use crate::client::{command_prompt_key, PromptKeyAction};
    let mut buf = LS.to_string();
    let mut back = 0usize;
    let act = command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Char('b'),
        KeyModifiers::ALT, || None);
    assert_eq!(act, PromptKeyAction::Edited);
    assert_eq!((buf.as_str(), back), (LS, 5), "M-b goes to the start of `local`");
    command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Left, KeyModifiers::CONTROL, || None);
    assert_eq!(back, 6, "C-Left goes back over `/`");
    command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Right, KeyModifiers::CONTROL, || None);
    assert_eq!(back, 5, "C-Right goes to the end of `/`");
    command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Char('f'), KeyModifiers::ALT, || None);
    assert_eq!(back, 0, "M-f goes to the end of `local`");
}

#[test]
fn the_client_command_prompt_cuts_a_path_component_on_ctrl_w() {
    use crate::client::command_prompt_key;
    let mut buf = LS.to_string();
    let mut back = 0usize;
    command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Char('w'), KeyModifiers::CONTROL, || None);
    assert_eq!(buf, "ls /usr/");
}

#[test]
fn the_client_command_prompt_drops_an_alt_key_it_has_no_use_for() {
    use crate::client::{command_prompt_key, PromptKeyAction};
    let mut buf = "ab".to_string();
    let mut back = 1usize;
    let act = command_prompt_key(&mut buf, &mut back, &mut None, TMUX_SEPS, KeyCode::Char('x'),
        KeyModifiers::ALT, || None);
    assert_eq!(act, PromptKeyAction::Edited, "the key stops at the prompt");
    assert_eq!((buf.as_str(), back), ("ab", 1), "and types nothing, as tmux drops it");
}

#[test]
fn the_rename_overlay_cuts_a_path_component_on_ctrl_w() {
    let mut buf = LS.to_string();
    let mut back = 0usize;
    let closes = crate::client::rename_overlay_ctrl_key(&mut buf, &mut back, &mut None, TMUX_SEPS, 'w', || None);
    assert!(!closes);
    assert_eq!(buf, "ls /usr/");
}

fn search_state(app: &AppState) -> (String, usize) {
    match app.mode {
        Mode::CopySearch { ref input, .. } => (input.clone(), app.copy_prompt_back),
        _ => panic!("the search prompt closed"),
    }
}

#[test]
fn the_copy_mode_search_prompt_moves_by_word_with_the_option() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.word_separators = TMUX_SEPS.to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_text_to_active(&mut app, LS).unwrap();
    // The client sends Alt+b and Ctrl+Left by these names (send-key M-b).
    crate::input::send_key_to_active(&mut app, "M-b").unwrap();
    assert_eq!(search_state(&app), (LS.to_string(), 5));
    crate::input::send_key_to_active(&mut app, "C-Left").unwrap();
    assert_eq!(search_state(&app), (LS.to_string(), 6));
    // From before `/local`, C-w cuts `usr`.
    crate::input::send_key_to_active(&mut app, "C-w").unwrap();
    assert_eq!(search_state(&app), ("ls //local".to_string(), 6));
}

#[test]
fn the_goto_line_prompt_moves_by_word() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.word_separators = TMUX_SEPS.to_string();
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_text_to_active(&mut app, "12").unwrap();
    crate::input::send_key_to_active(&mut app, "M-b").unwrap();
    assert_eq!(app.copy_prompt_back, 2);
}

#[test]
fn a_prompt_opened_by_a_copy_mode_binding_moves_by_word() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.word_separators = TMUX_SEPS.to_string();
    crate::config::parse_config_line(&mut app,
        "bind -T copy-mode-vi Z command-prompt -p 'p' \"send -X search-forward -- '%%'\"");
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    crate::input::send_text_to_active(&mut app, LS).unwrap();
    crate::input::send_key_to_active(&mut app, "C-w").unwrap();
    match app.mode {
        Mode::CopyCommandPrompt(ref p) => assert_eq!(p.input, "ls /usr/"),
        _ => panic!("the prompt closed"),
    }
    crate::input::send_key_to_active(&mut app, "M-b").unwrap();
    assert_eq!(app.copy_prompt_back, 1);
}

#[test]
fn the_server_command_prompt_moves_by_word_and_does_not_type_alt_keys() {
    let mut app = AppState::new("promptwords".to_string());
    app.word_separators = TMUX_SEPS.to_string();
    app.mode = Mode::CommandPrompt { input: LS.to_string(), cursor: LS.len() };
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT)).unwrap();
    match &app.mode {
        Mode::CommandPrompt { input, cursor } => assert_eq!((input.as_str(), *cursor), (LS, 8)),
        _ => panic!("the prompt closed"),
    }
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)).unwrap();
    match &app.mode {
        Mode::CommandPrompt { input, .. } => assert_eq!(input, LS, "Alt+x is not typed"),
        _ => panic!("the prompt closed"),
    }
}

#[test]
fn the_server_sends_the_option_to_the_client() {
    let mut app = AppState::new("promptwords".to_string());
    app.word_separators = "-/".to_string();
    let sf = crate::server::helpers::expand_status_formats(&app, "");
    assert_eq!(sf.client_render_options.word_separators.as_deref(), Some("-/"));
    let json = serde_json::to_string(&sf.client_render_options).unwrap();
    let back: crate::render_state::ClientRenderOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(back.word_separators.as_deref(), Some("-/"));
}
