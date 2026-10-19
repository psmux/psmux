// Issue #815: C-y at the prompts inserts what C-w last cut, or else the top
// paste buffer, the way tmux's prompt does (status_prompt_paste, status.c:1186
// to :1245, called from status.c:1645, tag 3.7c). Discussion #782, item 1.
//
// The expected lines are the ones tmux 3.7c showed at its own command prompt,
// measured through a nested client with status-keys emacs. `|` and `X` are
// typed after the keys so the cursor shows in the result:
//
//   abc, C-y (buffer BUF1)                       :abcBUF1
//   abc, C-a, C-y, |                             :BUF1|abc
//   ac, Left, C-y (buffer B), X                  :aBXc
//   buffer line1<LF>line2, C-y, |                :line1|
//   x, C-y (buffer '  sp  '), |                  :x  sp  |
//   buffer a<TAB>b, C-y, |                       :a|
//   buffer x<DEL>y, C-y, |                       :x|
//   x, C-y (buffer 日本語), |                    :x日本語|
//   no buffer, q, C-y, |                         :q|
//   set-buffer AUTO, -b named NAMED, C-y, |      :AUTO|
//   one two, C-w, C-y, C-y                       :one twotwo
//   empty, C-w, C-y, | (buffer BUF1)             :|
//   abc def, Home, C-k, C-y (buffer BUF1)        :BUF1
//   aa bb, C-w, C-u, C-y                         :bb
//   aa bb, C-w, Escape, reopen, C-y              :BUF1
//   copy mode search /, s, C-y (buffer SRCH)     (search down) sSRCH
//
// Registered from src/copy_mode.rs.

use super::*;
use super::test_copy_mode_snapshot::app_with_pane;

/// Type `text` into a fresh prompt, press the named keys in order with `top`
/// as the paste buffer, then type `X` so the cursor shows in the result.
fn edit(text: &str, keys: &[&str], top: Option<&str>) -> String {
    let mut input = text.to_string();
    let mut back = 0usize;
    let mut saved = None;
    for k in keys {
        prompt_edit(&mut input, &mut back, &mut saved, "", k, || top.map(str::to_string));
    }
    prompt_insert(&mut input, back, "X");
    input
}

// ── the editor ──────────────────────────────────────────────────────────────

#[test]
fn c_y_inserts_the_top_buffer_at_the_end() {
    assert_eq!(edit("abc", &["C-y"], Some("BUF1")), "abcBUF1X");
}

#[test]
fn c_y_inserts_at_the_cursor_and_leaves_it_after_the_text() {
    assert_eq!(edit("ac", &["Left", "C-y"], Some("B")), "aBXc");
    assert_eq!(edit("abc", &["C-a", "C-y"], Some("BUF1")), "BUF1Xabc");
}

#[test]
fn only_the_first_line_of_the_buffer_goes_in() {
    assert_eq!(edit("", &["C-y"], Some("line1\nline2")), "line1X");
    // A buffer copied on Windows ends its lines with CR LF.
    assert_eq!(edit("", &["C-y"], Some("line1\r\nline2")), "line1X");
}

#[test]
fn any_control_character_or_del_ends_the_text() {
    assert_eq!(edit("", &["C-y"], Some("a\tb")), "aX");
    assert_eq!(edit("", &["C-y"], Some("x\u{7f}y")), "xX");
    assert_eq!(edit("", &["C-y"], Some("p\u{1b}[0mq")), "pX");
}

#[test]
fn spaces_at_either_end_of_the_buffer_are_kept() {
    assert_eq!(edit("x", &["C-y"], Some("  sp  ")), "x  sp  X");
}

#[test]
fn wide_characters_go_in_whole() {
    assert_eq!(edit("x", &["C-y"], Some("日本語")), "x日本語X");
    assert_eq!(edit("ab", &["Left", "C-y"], Some("日本")), "a日本Xb");
}

#[test]
fn with_no_buffer_c_y_changes_nothing() {
    assert_eq!(edit("q", &["C-y"], None), "qX");
    assert_eq!(edit("q", &["C-y"], Some("")), "qX");
}

#[test]
fn c_y_is_an_editing_key_even_when_it_inserts_nothing() {
    let mut input = "a".to_string();
    let mut back = 0usize;
    let mut saved = None;
    assert_eq!(prompt_edit(&mut input, &mut back, &mut saved, "", "C-y", || None), PromptEdit::Edited);
    assert_eq!(prompt_edit(&mut input, &mut back, &mut saved, "", "c-y", || None), PromptEdit::Edited);
}

#[test]
fn after_c_w_c_y_puts_the_cut_back_and_can_repeat_it() {
    let mut input = "one two".to_string();
    let mut back = 0usize;
    let mut saved = None;
    let no_buffer = || -> Option<String> { panic!("a cut is there, so the buffer is not asked for") };
    prompt_edit(&mut input, &mut back, &mut saved, "", "C-w", || None);
    assert_eq!(input, "one ");
    assert_eq!(saved.as_deref(), Some("two"));
    prompt_edit(&mut input, &mut back, &mut saved, "", "C-y", no_buffer);
    assert_eq!(input, "one two");
    prompt_edit(&mut input, &mut back, &mut saved, "", "C-y", no_buffer);
    assert_eq!(input, "one twotwo");
}

#[test]
fn an_empty_cut_makes_c_y_insert_nothing_rather_than_the_buffer() {
    assert_eq!(edit("", &["C-w", "C-y"], Some("BUF1")), "X");
}

#[test]
fn c_k_and_c_u_do_not_keep_what_they_remove() {
    assert_eq!(edit("abc def", &["Home", "C-k", "C-y"], Some("BUF1")), "BUF1X");
    assert_eq!(edit("abc", &["C-u", "C-y"], Some("BUF1")), "BUF1X");
}

#[test]
fn the_cut_outlives_the_line_it_came_from() {
    assert_eq!(edit("aa bb", &["C-w", "C-u", "C-y"], Some("BUF1")), "bbX");
}

#[test]
fn prompt_paste_line_stops_at_the_first_control_character() {
    assert_eq!(prompt_paste_line("abc"), "abc");
    assert_eq!(prompt_paste_line("ab\ncd"), "ab");
    assert_eq!(prompt_paste_line("\nab"), "");
    assert_eq!(prompt_paste_line(""), "");
    // C1 controls are UTF-8 characters to tmux and go in like any other.
    assert_eq!(prompt_paste_line("a\u{85}b"), "a\u{85}b");
}

// ── the prompts that reach it ───────────────────────────────────────────────

#[test]
fn the_client_command_prompt_pastes_on_ctrl_y() {
    use crate::client::{command_prompt_key, PromptKeyAction};
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut buf = "abc".to_string();
    let mut back = 0usize;
    let mut saved = None;
    let act = command_prompt_key(&mut buf, &mut back, &mut saved, "", KeyCode::Char('y'),
        KeyModifiers::CONTROL, || Some("BUF1".to_string()));
    assert_eq!(act, PromptKeyAction::Edited);
    assert_eq!(buf, "abcBUF1");
    assert_eq!(back, 0);
}

#[test]
fn the_client_command_prompt_keeps_its_c_w_cut_for_c_y() {
    use crate::client::command_prompt_key;
    use crossterm::event::{KeyCode, KeyModifiers};
    let mut buf = "one two".to_string();
    let mut back = 0usize;
    let mut saved = None;
    command_prompt_key(&mut buf, &mut back, &mut saved, "", KeyCode::Char('w'), KeyModifiers::CONTROL, || None);
    command_prompt_key(&mut buf, &mut back, &mut saved, "", KeyCode::Char('y'), KeyModifiers::CONTROL,
        || Some("BUF1".to_string()));
    assert_eq!(buf, "one two");
}

#[test]
fn the_rename_overlay_pastes_on_ctrl_y() {
    let mut buf = "ab".to_string();
    let mut back = 1usize;
    let mut saved = None;
    let closes = crate::client::rename_overlay_ctrl_key(&mut buf, &mut back, &mut saved, "", 'y',
        || Some("NAME".to_string()));
    assert!(!closes, "C-y edits the name, it does not close the overlay");
    assert_eq!(buf, "aNAMEb");
    assert_eq!(back, 1);
}

fn search_input(app: &AppState) -> String {
    match app.mode {
        Mode::CopySearch { ref input, .. } => input.clone(),
        _ => panic!("the search prompt closed"),
    }
}

#[test]
fn the_copy_mode_search_prompt_pastes_the_top_buffer() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.paste_set_stack(&["SRCH", "OLDER"]);
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_text_to_active(&mut app, "s").unwrap();
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    assert_eq!(search_input(&app), "sSRCH");
    assert_eq!(app.status_message.as_ref().map(|m| m.0.as_str()), Some("(search down) sSRCH"));
}

#[test]
fn a_cut_does_not_carry_over_to_the_next_search_prompt() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.paste_set_stack(&["BUF1"]);
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_text_to_active(&mut app, "aa bb").unwrap();
    crate::input::send_key_to_active(&mut app, "C-w").unwrap();
    crate::input::send_key_to_active(&mut app, "esc").unwrap();
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    assert_eq!(search_input(&app), "BUF1");
}

#[test]
fn the_goto_line_prompt_pastes_the_top_buffer() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.paste_set_stack(&["12"]);
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    match app.mode {
        Mode::CopyGoto { ref input } => assert_eq!(input, "12"),
        _ => panic!("the goto line prompt closed"),
    }
}

#[test]
fn a_prompt_opened_by_a_copy_mode_binding_pastes_the_top_buffer() {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    app.paste_set_stack(&["BUF1"]);
    crate::config::parse_config_line(&mut app,
        "bind -T copy-mode-vi Z command-prompt -p 'p' \"send -X search-forward -- '%%'\"");
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    crate::input::send_text_to_active(&mut app, "ab").unwrap();
    crate::input::send_key_to_active(&mut app, "C-a").unwrap();
    crate::input::send_key_to_active(&mut app, "C-y").unwrap();
    match app.mode {
        Mode::CopyCommandPrompt(ref p) => assert_eq!(p.input, "BUF1ab"),
        _ => panic!("the prompt closed"),
    }
}

// ── reading the buffer from the server ──────────────────────────────────────

/// Answer one connection with `reply`, after reading the AUTH and command
/// lines, and return the address to connect to.
fn one_reply_server(reply: &'static str) -> String {
    use std::io::{BufRead, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut rd = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = rd.read_line(&mut line);
            line.clear();
            let _ = rd.read_line(&mut line);
            let mut w = stream;
            let _ = w.write_all(reply.as_bytes());
        }
    });
    addr
}

fn fetch(reply: &'static str) -> Option<String> {
    let addr = one_reply_server(reply);
    crate::session::fetch_authed_first_line_raw(
        &addr,
        "k",
        b"show-buffer\n",
        std::time::Duration::from_millis(500),
        std::time::Duration::from_millis(500),
    )
}

#[test]
fn the_buffer_is_read_with_its_spaces() {
    assert_eq!(fetch("OK\n  sp  \nline2\n").as_deref(), Some("  sp  "));
}

#[test]
fn a_buffer_that_reads_ok_or_is_empty_is_still_a_buffer() {
    assert_eq!(fetch("OK\nOK\n").as_deref(), Some("OK"));
    assert_eq!(fetch("OK\n\n").as_deref(), Some(""));
}

#[test]
fn a_refused_connection_gives_no_buffer() {
    assert_eq!(fetch("ERROR: Invalid session key\n"), None);
    assert_eq!(fetch(""), None);
}
