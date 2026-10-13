// #789 follow up: two things a copy mode command prompt hands to the command
// it runs.
//
// 1. `command-prompt -k` answers with the NAME of the key, which tmux writes
//    with key_string_lookup_key (key-string.c:327, called at status.c:1439):
//    `Left`, `PPage`, `NPage`, `IC`, `DC`, `F1`, `BTab`, `Space`, `C-Left`.
//    The prompt passed on the client's send-key spelling instead (`left`,
//    `pageup`, `insert`, `f1`, `btab`, and an empty name for Space).
//    Measured in tmux 3.4 through a nested client: every name below.
//
// 2. The command is parsed the way tmux parses it. tmux puts the typed text in
//    raw (cmd_template_replace, cmd.c:841) and its parser removes the quotes.
//    The in process executor split on whitespace, so `rename-window '%%'` with
//    TEST named the window `'TEST'` and `a b` named it `'a`; `rename-window --
//    '%%'` named it `--`; and the escape written for a typed `'` was applied
//    outside single quotes too, which turned `it's` into `it"'s`.
//    Measured in tmux 3.4 (cmd_template_replace is the same in 3.7c):
//      '%%'  TEST -> TEST, a b -> a b, say "hi" -> say "hi", a;b -> a;b
//      "%%"  it's -> it's, say "hi" -> say hi
//      -- '%%'  TEST -> TEST
//
// Registered from src/copy_mode.rs.

use super::*;
use super::test_copy_mode_snapshot::app_with_pane;

fn app_with(binding: &str) -> AppState {
    let mut app = app_with_pane();
    app.mode_keys = "vi".to_string();
    crate::config::parse_config_line(&mut app, binding);
    crate::copy_mode::enter_copy_mode(&mut app);
    app
}

fn window_name(app: &AppState) -> String {
    app.windows[app.active_idx].name.clone()
}

// ───────────────────────── -k key names ─────────────────────────

/// The client's send-key spellings, and what tmux answers a -k prompt with.
const KEY_NAMES: &[(&str, &str)] = &[
    ("left", "Left"), ("right", "Right"), ("up", "Up"), ("down", "Down"),
    ("home", "Home"), ("end", "End"),
    ("pageup", "PPage"), ("pagedown", "NPage"),
    ("insert", "IC"), ("delete", "DC"),
    ("f1", "F1"), ("f5", "F5"), ("f12", "F12"),
    ("tab", "Tab"), ("btab", "BTab"),
    ("enter", "Enter"), ("esc", "Escape"), ("backspace", "BSpace"),
    ("C-a", "C-a"), ("M-a", "M-a"), ("C-M-a", "C-M-a"),
    ("C-Left", "C-Left"), ("S-Left", "S-Left"), ("M-Left", "M-Left"),
    ("C-S-Up", "C-S-Up"), ("C-F5", "C-F5"), ("S-F5", "S-F5"),
    ("C-Backspace", "C-BSpace"), ("M-Escape", "M-Escape"), ("C-Space", "C-Space"),
    ("a", "a"), ("A", "A"), ("1", "1"), (";", ";"), (" ", "Space"),
];

#[test]
fn key_string_writes_every_name_the_way_tmux_does() {
    for (sent, want) in KEY_NAMES {
        assert_eq!(crate::copy_prompt::key_string(sent), *want, "send-key {sent:?}");
    }
}

#[test]
fn a_name_that_is_not_a_key_passes_through() {
    assert_eq!(crate::copy_prompt::key_string(""), "");
    assert_eq!(crate::copy_prompt::key_string("WheelUpPane"), "WheelUpPane");
}

#[test]
fn a_k_prompt_answers_with_the_tmux_name_of_the_key() {
    for (sent, want) in KEY_NAMES {
        let mut app = app_with("bind -T copy-mode-vi K command-prompt -k -p key \"rename-window 'K%%'\"");
        crate::input::send_text_to_active(&mut app, "K").unwrap();
        assert!(matches!(app.mode, Mode::CopyCommandPrompt(_)), "K must open the prompt");
        if sent.chars().count() == 1 {
            crate::input::send_text_to_active(&mut app, sent).unwrap();
        } else {
            crate::input::send_key_to_active(&mut app, sent).unwrap();
        }
        assert!(!matches!(app.mode, Mode::CopyCommandPrompt(_)), "{sent:?} answers the prompt");
        assert_eq!(window_name(&app), format!("K{want}"), "send-key {sent:?}");
    }
}

#[test]
fn space_reaches_a_k_prompt_as_its_name() {
    // The live path turns the client's `send-key space` into a typed ' '.
    let mut app = app_with("bind -T copy-mode-vi K command-prompt -k -p key \"rename-window 'K%%'\"");
    crate::input::send_text_to_active(&mut app, "K").unwrap();
    crate::input::send_key_to_active(&mut app, "space").unwrap();
    assert_eq!(window_name(&app), "KSpace");
}

// ───────────────────────── %% and the parser ─────────────────────────

fn answer(binding: &str, typed: &str) -> String {
    let mut app = app_with(binding);
    crate::input::send_text_to_active(&mut app, "Y").unwrap();
    assert!(matches!(app.mode, Mode::CopyCommandPrompt(_)), "Y must open the prompt");
    for c in typed.chars() {
        crate::input::send_text_to_active(&mut app, &c.to_string()).unwrap();
    }
    crate::input::send_key_to_active(&mut app, "enter").unwrap();
    window_name(&app)
}

const SQ: &str = "bind -T copy-mode-vi Y command-prompt -p x \"rename-window '%%'\"";
const DQ: &str = "bind -T copy-mode-vi Y command-prompt -p x \"rename-window \\\"%%\\\"\"";
const DASHES: &str = "bind -T copy-mode-vi Y command-prompt -p x \"rename-window -- '%%'\"";

#[test]
fn single_quotes_in_the_template_are_not_part_of_the_name() {
    assert_eq!(answer(SQ, "TEST"), "TEST");
    assert_eq!(answer(SQ, "a b"), "a b");
    assert_eq!(answer(SQ, "say \"hi\""), "say \"hi\"");
    assert_eq!(answer(SQ, "a;b"), "a;b");
}

#[test]
fn double_quotes_in_the_template_are_not_part_of_the_name() {
    assert_eq!(answer(DQ, "TEST"), "TEST");
    assert_eq!(answer(DQ, "a b"), "a b");
    assert_eq!(answer(DQ, "it's"), "it's");
    // The typed quotes end and reopen the template's: tmux names it `say hi`.
    assert_eq!(answer(DQ, "say \"hi\""), "say hi");
    assert_eq!(answer(DQ, "a;b"), "a;b");
}

#[test]
fn double_dash_ends_the_flags() {
    assert_eq!(answer(DASHES, "TEST"), "TEST");
    assert_eq!(answer(DASHES, "a b"), "a b");
}

#[test]
fn a_typed_quote_is_escaped_only_inside_single_quotes() {
    use crate::copy_prompt::template_replace;
    // Inside single quotes: kept, as psmux has done since #786.
    assert_eq!(
        crate::commands::parse_command_line(&template_replace("x '%%'", "it's", 1)),
        vec!["x", "it's"]
    );
    // Inside double quotes and unquoted: put in as typed, as tmux does.
    assert_eq!(template_replace("x \"%%\"", "it's", 1), "x \"it's\"");
    assert_eq!(template_replace("x %%", "it's", 1), "x it's");
    assert_eq!(template_replace("x %1", "it's", 1), "x it's");
    // A quote character inside the template's double quotes does not count.
    assert_eq!(template_replace("x \"a'b\" %%", "it's", 1), "x \"a'b\" it's");
    // An escaped double quote does not end the template's double quotes.
    assert_eq!(template_replace("x \"\\\"'%%\"", "it's", 1), "x \"\\\"'it's\"");
}

#[test]
fn the_executor_removes_quotes_like_the_command_line() {
    let mut app = app_with_pane();
    crate::commands::execute_command_string(&mut app, "rename-window 'a b'").unwrap();
    assert_eq!(window_name(&app), "a b");
    crate::commands::execute_command_string(&mut app, "rename-window \"c d\"").unwrap();
    assert_eq!(window_name(&app), "c d");
    crate::commands::execute_command_string(&mut app, "rename-window -- -dash").unwrap();
    assert_eq!(window_name(&app), "-dash");
    crate::commands::execute_command_string(&mut app, "rename-session -- 'my s'").unwrap();
    assert_eq!(app.session_name, "my s");
}

// ───────────────────────── first_operand ─────────────────────────

#[test]
fn first_operand_reads_flags_like_getopt() {
    use crate::cli::first_operand;
    assert_eq!(first_operand("rename-window", &["x"]), Some("x"));
    assert_eq!(first_operand("rename-window", &["--", "x"]), Some("x"));
    assert_eq!(first_operand("rename-window", &["--", "-x"]), Some("-x"));
    assert_eq!(first_operand("rename-window", &["-t", "s:1", "x"]), Some("x"));
    assert_eq!(first_operand("rename-window", &["-ts:1", "x"]), Some("x"));
    assert_eq!(first_operand("rename-window", &["-t", "s:1", "--", "x"]), Some("x"));
    assert_eq!(first_operand("rename-window", &["-"]), Some("-"));
    assert_eq!(first_operand("rename-window", &["--"]), None);
    assert_eq!(first_operand("rename-window", &[]), None);
}
