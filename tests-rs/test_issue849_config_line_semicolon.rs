// Issue #849: a `;` in a config line ends a command. tmux reads a file with
// its string grammar, where an unquoted `;` ends the token it is in and the
// command with it (cmd-parse.y:1314 and :1675, tmux.1:528 "Each command is
// terminated by a newline or a semicolon"). psmux handed the whole line to the
// parser, so `set -g @a1 on ; set -g @a2 on` set @a1 to `on ; set -g @a2 on`.
//
// These tests pin where a line is split and what each route then runs. The
// routes end to end are in tests/test_issue849_config_line_semicolon.ps1.

use super::*;
use crate::types::{DeferredConfigLine, DeferredKind};

fn split(s: &str) -> Vec<String> {
    split_line_commands(s)
}

fn app() -> AppState {
    AppState::new("semi".to_string())
}

fn opt(app: &AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

fn bound(app: &AppState, table: &str, key: &str) -> Option<String> {
    let k = normalize_key_for_binding(parse_key_name(key)?);
    app.key_tables.get(table)?.iter().find(|b| b.key == k).map(|b| crate::commands::format_action(&b.action))
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// A server with one window, as a runtime source-file finds it.
fn app_with_window() -> AppState {
    let mut app = AppState::new("semi".to_string());
    app.windows.push(make_window("shell", 0));
    app
}

/// A file of its own for each test, removed when the guard drops.
struct TempConf(std::path::PathBuf);
impl TempConf {
    fn new(tag: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "psmux_test_semi_{}_{}_{}.conf",
            std::process::id(),
            tag,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::write(&path, content).unwrap();
        TempConf(path)
    }
    fn path(&self) -> String {
        self.0.display().to_string()
    }
}
impl Drop for TempConf {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn kinds(lines: &[DeferredConfigLine]) -> Vec<DeferredKind> {
    lines.iter().map(|l| l.kind.clone()).collect()
}

// ── where a line is split ───────────────────────────────────────────────────

#[test]
fn a_semicolon_between_words_ends_a_command() {
    assert_eq!(split("set -g @a1 on ; set -g @a2 on"), ["set -g @a1 on", "set -g @a2 on"]);
}

#[test]
fn a_semicolon_at_the_end_of_a_word_or_inside_one_ends_a_command_too() {
    assert_eq!(split("set -g @a1 on; set -g @a2 on"), ["set -g @a1 on", "set -g @a2 on"]);
    assert_eq!(split("set -g @a1 on;set -g @a2 on;set -g @a3 on"), ["set -g @a1 on", "set -g @a2 on", "set -g @a3 on"]);
    // A `#` inside a word is not a comment.
    assert_eq!(split("set -g @a1 a#b;set -g @a2 on"), ["set -g @a1 a#b", "set -g @a2 on"]);
}

#[test]
fn a_quoted_semicolon_is_part_of_the_value() {
    assert_eq!(split(r#"set -g @a1 "on;x" ; set -g @a2 on"#), [r#"set -g @a1 "on;x""#, "set -g @a2 on"]);
    assert_eq!(split("set -g @a1 'on ; x'"), ["set -g @a1 'on ; x'"]);
    assert_eq!(split(r#"set -g @a1 "a \" ; b""#), [r#"set -g @a1 "a \" ; b""#], "an escaped quote does not close the string");
}

#[test]
fn an_escaped_semicolon_stays_in_the_line() {
    assert_eq!(split(r"bind x set -g @a1 on \; set -g @a2 on"), [r"bind x set -g @a1 on \; set -g @a2 on"]);
    assert_eq!(split(r"set -g @a1 on\;x"), [r"set -g @a1 on\;x"]);
}

#[test]
fn a_semicolon_inside_a_block_belongs_to_the_block() {
    assert_eq!(split("bind x { set -g @a1 on ; set -g @a2 on }"), ["bind x { set -g @a1 on ; set -g @a2 on }"]);
    assert_eq!(
        split("set -g @a0 on ; bind x { set -g @a1 on ; set -g @a2 on }"),
        ["set -g @a0 on", "bind x { set -g @a1 on ; set -g @a2 on }"]
    );
}

#[test]
fn a_semicolon_in_a_comment_is_part_of_the_comment() {
    assert_eq!(split("set -g @a1 on # c ; set -g @a2 on"), ["set -g @a1 on # c ; set -g @a2 on"]);
    assert_eq!(split("set -g @a1 on ; # c ; set -g @a2 on"), ["set -g @a1 on", "# c ; set -g @a2 on"]);
}

#[test]
fn empty_commands_are_dropped() {
    assert_eq!(split("set -g @a1 on ;"), ["set -g @a1 on"]);
    assert_eq!(split("set -g @a1 on ;; set -g @a2 on"), ["set -g @a1 on", "set -g @a2 on"]);
}

#[test]
fn a_line_that_starts_with_a_semicolon_is_left_for_the_parser_to_reject() {
    assert_eq!(split("; set -g @a2 on"), ["; set -g @a2 on"]);
}

#[test]
fn directives_and_semicolon_keys_are_left_alone() {
    assert_eq!(split("%if #{==:a;b,a;b}"), ["%if #{==:a;b,a;b}"]);
    assert_eq!(split("bind -n M-; display hi"), ["bind -n M-; display hi"]);
    assert_eq!(split("bind ^; last-pane ; set -g @a1 on"), ["bind ^; last-pane", "set -g @a1 on"]);
}

#[test]
fn a_line_with_no_semicolon_comes_back_as_it_was() {
    assert_eq!(split("  set -g status-right 'x'  "), ["  set -g status-right 'x'  "]);
}

// ── what a file then runs ───────────────────────────────────────────────────

#[test]
fn every_option_on_a_line_is_set() {
    let mut app = app();
    parse_config_content(&mut app, "set -g @a1 on ; set -g @a2 on\nset -g @a3 x;set -ga @a3 y\n");
    assert_eq!(opt(&app, "@a1").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@a2").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@a3").as_deref(), Some("xy"), "in order");
}

#[test]
fn a_quoted_value_keeps_its_semicolon() {
    let mut app = app();
    parse_config_content(&mut app, "set -g status-right \"x;y\" ; set -g @a2 'c ; d'\n");
    assert_eq!(app.status_right, "x;y");
    assert_eq!(opt(&app, "@a2").as_deref(), Some("c ; d"));
}

#[test]
fn a_continued_line_is_split_after_it_is_joined() {
    let mut app = app();
    parse_config_content(&mut app, "set -g @a1 on ; \\\nset -g @a2 on\n");
    assert_eq!(opt(&app, "@a1").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@a2").as_deref(), Some("on"));
}

#[test]
fn a_binding_takes_the_commands_up_to_the_first_plain_semicolon() {
    // tmux binds `set -g @a1 on` and runs `set -g @a2 on` at once.
    let mut app = app();
    parse_config_content(&mut app, "bind x set -g @a1 on ; set -g @a2 on\n");
    assert_eq!(bound(&app, "prefix", "x").as_deref(), Some("set -g @a1 on"));
    assert_eq!(opt(&app, "@a1"), None);
    assert_eq!(opt(&app, "@a2").as_deref(), Some("on"));
}

#[test]
fn a_binding_still_holds_a_list_written_with_escaped_semicolons_or_a_block() {
    let mut app = app();
    parse_config_content(&mut app, "bind x set -g @a1 on \\; set -g @a2 on\nbind y { set -g @a1 on ; set -g @a2 on }\n");
    assert_eq!(bound(&app, "prefix", "x").as_deref(), Some("set -g @a1 on \\; set -g @a2 on"));
    assert_eq!(bound(&app, "prefix", "y").as_deref(), Some("set -g @a1 on \\; set -g @a2 on"));
    assert_eq!(opt(&app, "@a1"), None);
}

#[test]
fn an_if_shell_branch_runs_every_command_on_it() {
    let mut app = app();
    parse_config_content(&mut app, "if-shell true \"set -g @a1 on ; set -g @a2 on\"\n");
    assert_eq!(opt(&app, "@a1").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@a2").as_deref(), Some("on"));
}

#[test]
fn an_if_shell_block_runs_every_command_in_it() {
    let mut app = app();
    parse_config_content(&mut app, "if-shell true { set -g @a1 on ; set -g @a2 on }\n");
    assert_eq!(opt(&app, "@a1").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@a2").as_deref(), Some("on"));
    let mut other = AppState::new("semi".to_string());
    parse_config_content(&mut other, "if-shell false { set -g @a1 on } { set -g @a2 on ; set -g @a3 on }\n");
    assert_eq!(opt(&other, "@a1"), None);
    assert_eq!(opt(&other, "@a3").as_deref(), Some("on"));
}

#[test]
fn a_command_after_an_option_on_the_same_line_waits_its_turn_in_a_runtime_source() {
    let f = TempConf::new("runtime", "set -g @a1 on ; new-window -n a ; set -g @a2 on\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(opt(&app, "@a1").as_deref(), Some("on"), "before the first command, in place");
    assert_eq!(
        kinds(&lines),
        vec![
            DeferredKind::Command("new-window -n a".to_string()),
            DeferredKind::Directive("set -g @a2 on".to_string()),
        ]
    );
    assert!(lines.iter().all(|l| l.line == Some(1)), "every command keeps the line it was on");
}

#[test]
fn a_cycle_after_a_semicolon_is_found_before_anything_runs() {
    let f = TempConf::new("cycle", "");
    std::fs::write(&f.0, format!("set -g @a1 on ; source-file {}\n", crate::util::quote_arg(&f.path()))).unwrap();
    assert!(find_source_cycle(&f.path()).is_some());
}
