// A prompt opened by a copy mode key table binding edits through the shared
// prompt editor.
//
// `copy_prompt::feed` had a line editor of its own: characters were appended,
// BSpace removed the last one, C-u cleared the line and every other key was
// dropped, so the cursor could not move and C-a, C-e, C-k, C-w, C-t and the
// arrows did nothing. tmux sends this prompt's keys through the same
// `status_prompt_key` switch as any other prompt's (process_key, status.c:1483,
// tag 3.7c); only -k, -N and -1 leave before it (status.c:1438 to :1470).
//
// Up and Down walk the history of the prompt's type (status.c:1625 to :1644).
// A -T search prompt shares the copy mode search history, as tmux's own `/`
// and `?` are -T search prompts (key-bindings.c:597, :610).
//
// Registered from src/copy_mode.rs.

use super::*;
use super::test_copy_mode_snapshot::app_with_pane;

/// A prompt bound to `Z` in copy-mode-vi whose answer becomes a search, so
/// Enter shows what the whole line held.
const BIND_Z: &str = "bind -T copy-mode-vi Z command-prompt -p 'p' \"send -X search-forward -- '%%'\"";

fn prompt_app(binding: &str) -> AppState {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    crate::config::parse_config_line(&mut app, binding);
    crate::copy_mode::enter_copy_mode(&mut app);
    app
}

/// Open the `Z` prompt and type `text` into it.
fn open_and_type(text: &str) -> AppState {
    let mut app = prompt_app(BIND_Z);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    assert!(matches!(app.mode, Mode::CopyCommandPrompt(_)), "Z must open the bound prompt");
    for c in text.chars() {
        crate::input::send_text_to_active(&mut app, &c.to_string()).unwrap();
    }
    app
}

fn keys(app: &mut AppState, names: &[&str]) {
    for k in names {
        crate::input::send_key_to_active(app, k).unwrap();
    }
}

fn input(app: &AppState) -> String {
    match app.mode {
        Mode::CopyCommandPrompt(ref p) => p.input.clone(),
        _ => panic!("the prompt closed"),
    }
}

fn status(app: &AppState) -> String {
    app.status_message.as_ref().map(|m| m.0.clone()).unwrap_or_default()
}

#[test]
fn left_moves_the_cursor_and_a_character_goes_in_there() {
    let mut app = open_and_type("abc");
    keys(&mut app, &["left", "left"]);
    crate::input::send_text_to_active(&mut app, "X").unwrap();
    assert_eq!(input(&app), "aXbc");
    assert_eq!(status(&app), "p aXbc", "the status line shows the edit at once");
}

#[test]
fn c_a_and_c_e_go_to_the_start_and_the_end() {
    let mut app = open_and_type("bc");
    keys(&mut app, &["C-a"]);
    crate::input::send_text_to_active(&mut app, "a").unwrap();
    keys(&mut app, &["C-e"]);
    crate::input::send_text_to_active(&mut app, "d").unwrap();
    assert_eq!(input(&app), "abcd");
}

#[test]
fn home_end_and_right_move_the_cursor_too() {
    let mut app = open_and_type("ac");
    keys(&mut app, &["home", "right"]);
    crate::input::send_text_to_active(&mut app, "b").unwrap();
    keys(&mut app, &["end"]);
    crate::input::send_text_to_active(&mut app, "d").unwrap();
    assert_eq!(input(&app), "abcd");
}

#[test]
fn bspace_deletes_the_character_before_the_cursor() {
    let mut app = open_and_type("abXc");
    keys(&mut app, &["left", "bspace"]);
    assert_eq!(input(&app), "abc");
}

#[test]
fn delete_deletes_the_character_under_the_cursor() {
    let mut app = open_and_type("abXc");
    keys(&mut app, &["left", "left", "delete"]);
    assert_eq!(input(&app), "abc");
}

#[test]
fn c_k_c_w_and_c_t_edit_at_the_cursor() {
    let mut app = open_and_type("abc def");
    keys(&mut app, &["left", "left", "C-k"]);
    assert_eq!(input(&app), "abc d", "C-k cuts from the cursor to the end");
    keys(&mut app, &["C-w"]);
    assert_eq!(input(&app), "abc ", "C-w cuts the word before the cursor");
    let mut app = open_and_type("abc");
    keys(&mut app, &["left", "C-t"]);
    assert_eq!(input(&app), "acb", "C-t transposes the characters at the cursor (#785)");
}

#[test]
fn c_u_still_clears_the_line() {
    let mut app = open_and_type("abc");
    keys(&mut app, &["left", "C-u"]);
    assert_eq!(input(&app), "");
    crate::input::send_text_to_active(&mut app, "x").unwrap();
    assert_eq!(input(&app), "x", "the cursor is back at the end");
}

#[test]
fn enter_runs_the_whole_line_not_up_to_the_cursor() {
    let mut app = open_and_type("abd");
    keys(&mut app, &["left"]);
    crate::input::send_text_to_active(&mut app, "c").unwrap();
    keys(&mut app, &["C-a", "enter"]);
    assert!(matches!(app.mode, Mode::CopyMode), "Enter answers the prompt and stays in copy mode");
    assert_eq!(app.copy_search_query, "abcd");
}

#[test]
fn characters_are_inserted_whole() {
    let mut app = open_and_type("日本語");
    keys(&mut app, &["left"]);
    crate::input::send_text_to_active(&mut app, "X").unwrap();
    assert_eq!(input(&app), "日本X語");
    keys(&mut app, &["bspace"]);
    assert_eq!(input(&app), "日本語");
}

#[test]
fn the_cursor_is_drawn_where_it_is() {
    let mut app = open_and_type("abc");
    keys(&mut app, &["left"]);
    // "p abc": the label is two bytes, and the cursor sits before `c`.
    assert_eq!(crate::copy_mode::copy_prompt_cursor(&app.mode, app.copy_prompt_back), Some((2, 4)));
    let mut app = open_and_type("日本語");
    keys(&mut app, &["left"]);
    assert_eq!(
        crate::copy_mode::copy_prompt_cursor(&app.mode, app.copy_prompt_back),
        Some((2, 2 + "日本".len())),
        "the offset is in bytes, before the last character"
    );
}

#[test]
fn a_prompt_opens_with_its_cursor_at_the_end() {
    // Left twice and Escape leave nothing behind: the next time the prompt
    // opens, typing goes after the -I text.
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -I 'ab' -p 'p' \"send -X search-forward -- '%%'\"");
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["left", "left", "escape"]);
    assert!(matches!(app.mode, Mode::CopyMode));
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    crate::input::send_text_to_active(&mut app, "c").unwrap();
    assert_eq!(input(&app), "abc");
}

#[test]
fn the_next_prompt_opens_with_its_cursor_at_the_end() {
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -I 'a,bc' -p 'p,q' \"send -X search-forward -- '%1-%2'\"");
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["C-a", "enter"]);
    assert_eq!(status(&app), "q bc");
    crate::input::send_text_to_active(&mut app, "d").unwrap();
    assert_eq!(input(&app), "bcd");
    keys(&mut app, &["enter"]);
    assert_eq!(app.copy_search_query, "a-bcd");
}

#[test]
fn up_and_down_change_nothing_without_t_search() {
    // tmux recalls the command history here; psmux keeps that history in the
    // client, so a prompt of any type but search has none to recall.
    let mut app = open_and_type("abc");
    app.copy_search_history = vec!["older".to_string()];
    keys(&mut app, &["left", "up", "down", "C-p"]);
    crate::input::send_text_to_active(&mut app, "X").unwrap();
    assert_eq!(input(&app), "abXc");
}

/// `Z` as a `-T search` prompt, with `history` already searched for.
fn search_prompt(history: &[&str]) -> AppState {
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -T search -p 'p' \"send -X search-forward -- '%%'\"");
    app.copy_search_history = history.iter().map(|s| s.to_string()).collect();
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    app
}

#[test]
fn t_search_up_and_down_walk_the_search_history() {
    let mut app = search_prompt(&["one", "two"]);
    keys(&mut app, &["up"]);
    assert_eq!(input(&app), "two", "Up starts from the newest");
    assert_eq!(status(&app), "p two");
    keys(&mut app, &["up"]);
    assert_eq!(input(&app), "one");
    keys(&mut app, &["up"]);
    assert_eq!(input(&app), "one", "and stops at the oldest");
    keys(&mut app, &["down"]);
    assert_eq!(input(&app), "two");
    keys(&mut app, &["down"]);
    assert_eq!(input(&app), "", "past the newest is an empty line");
    keys(&mut app, &["C-p"]);
    assert_eq!(input(&app), "two", "C-p is Up");
}

#[test]
fn a_recalled_line_has_its_cursor_at_the_end() {
    let mut app = search_prompt(&["abc"]);
    keys(&mut app, &["up"]);
    crate::input::send_text_to_active(&mut app, "d").unwrap();
    assert_eq!(input(&app), "abcd");
}

#[test]
fn t_search_enter_adds_to_the_history_the_built_in_prompt_reads() {
    let mut app = search_prompt(&["one"]);
    crate::input::send_text_to_active(&mut app, "two").unwrap();
    keys(&mut app, &["enter"]);
    assert_eq!(app.copy_search_history, vec!["one".to_string(), "two".to_string()]);
    // The built in `/` prompt shares it, as tmux's `/` is -T search too.
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    keys(&mut app, &["up"]);
    match app.mode {
        Mode::CopySearch { ref input, .. } => assert_eq!(input, "two"),
        _ => panic!("/ did not open the search prompt"),
    }
}

#[test]
fn t_search_reads_what_the_built_in_prompt_added() {
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -T search -p 'p' \"send -X search-forward -- '%%'\"");
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_text_to_active(&mut app, "abc").unwrap();
    keys(&mut app, &["enter"]);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["up"]);
    assert_eq!(input(&app), "abc");
}

#[test]
fn an_empty_line_and_a_repeat_are_not_added() {
    // The template ignores the answer, so an empty one runs nothing that
    // opens a prompt of its own.
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -T search -p 'p' \"send -X history-top\"");
    app.copy_search_history = vec!["one".to_string()];
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["enter"]);
    assert!(matches!(app.mode, Mode::CopyMode));
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    crate::input::send_text_to_active(&mut app, "one").unwrap();
    keys(&mut app, &["enter"]);
    assert_eq!(app.copy_search_history, vec!["one".to_string()]);
}

#[test]
fn a_prompt_without_t_search_adds_nothing() {
    let mut app = open_and_type("abc");
    keys(&mut app, &["enter"]);
    assert!(app.copy_search_history.is_empty());
}

#[test]
fn up_starts_from_the_newest_each_time_the_prompt_opens() {
    let mut app = search_prompt(&["one", "two"]);
    keys(&mut app, &["up", "up", "escape"]);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["up"]);
    assert_eq!(input(&app), "two");
}

#[test]
fn bspace_exit_needs_an_empty_line_not_a_cursor_at_the_start() {
    // status.c:1520 tests the size of the line.
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -e -p 'p' \"send -X search-forward -- '%%'\"");
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    crate::input::send_text_to_active(&mut app, "ab").unwrap();
    keys(&mut app, &["C-a", "bspace"]);
    assert_eq!(input(&app), "ab", "nothing before the cursor, so nothing to delete");
    keys(&mut app, &["C-e", "bspace", "bspace"]);
    assert_eq!(input(&app), "");
    keys(&mut app, &["bspace"]);
    assert!(matches!(app.mode, Mode::CopyMode), "an empty -e prompt closes on BSpace");
}

#[test]
fn a_single_key_prompt_is_not_edited() {
    // -1 leaves before the editing switch (status.c:1459): the next key that
    // types something is the answer.
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -1 -p 'p' \"send -X search-forward -- '%%'\"");
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    keys(&mut app, &["left", "C-a"]);
    assert_eq!(input(&app), "");
    crate::input::send_text_to_active(&mut app, "x").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_search_query, "x");
}

#[test]
fn handle_key_maps_the_arrows_too() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = open_and_type("ac");
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).unwrap();
    crate::input::handle_key(&mut app, KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)).unwrap();
    assert_eq!(input(&app), "abc");
}

#[test]
fn a_key_that_changes_nothing_still_redraws_the_prompt() {
    // The server clears the status line before it hands a key on
    // (CtrlReq::SendKey), so the prompt has to put itself back.
    let mut app = open_and_type("abc");
    for k in ["up", "C-a", "left", "pageup"] {
        app.status_message = None;
        keys(&mut app, &[k]);
        assert_eq!(status(&app), "p abc", "{k} took the prompt off the status line");
    }
    let mut app = prompt_app("bind -T copy-mode-vi Z command-prompt -1 -p 'p' \"send -X search-forward -- '%%'\"");
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    app.status_message = None;
    keys(&mut app, &["left"]);
    assert_eq!(status(&app), "p ", "a -1 prompt stays on the status line too");
}
