// A copy mode key table binding to `command-prompt` left copy mode and showed
// no prompt.
//
// tmux writes most of its own copy mode keys this way (key-bindings.c:582 to
// :704): `:` is `command-prompt -p'(goto line)' { send -X goto-line -- '%%' }`.
// The prompt opens on the status line while the pane stays in copy mode, and
// the command built from the template runs against that pane, still in copy
// mode (cmd-command-prompt.c:186, :238).
//
// psmux's server `command-prompt` arm replaced the copy mode in `app.mode`
// with `Mode::CommandPrompt`, which no client draws, so a binding copied from
// `list-keys -T copy-mode-vi` replaced a working built in with a dead key.
//
// Driven through the same dispatchers the live server uses, over a stub PTY
// pane tree. Registered from src/copy_prompt.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const ROWS: u16 = 10;
const COLS: u16 = 40;
const LINES: usize = 60;

fn make_pane(id: usize) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    });
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 200)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term,
        last_rows: ROWS,
        last_cols: COLS,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false, input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, COLS, ROWS),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
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

/// One pane holding `LINES` numbered lines, in vi copy mode at the live bottom,
/// with `config` lines applied the way a config file applies them.
fn copy_app(config: &[&str]) -> AppState {
    let mut app = AppState::new("cpbind".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = "vi".to_string();
    app.user_options.insert("copy-mode-line-numbers".to_string(), "absolute".to_string());
    for line in config {
        crate::config::parse_config_line(&mut app, line);
    }
    let pane = make_pane(0);
    {
        let mut parser = pane.term.lock().expect("parser lock");
        for i in 1..=LINES {
            parser.process(format!("line-{i}\r\n").as_bytes());
        }
    }
    let mut win = make_window(0);
    win.root = Node::Leaf(pane);
    app.windows.push(win);
    app.active_idx = 0;
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((ROWS / 2, 0));
    app
}

fn hsize(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let pane = crate::tree::active_pane(&win.root, &win.active_path).expect("fixture pane");
    let parser = pane.term.lock().expect("parser lock");
    parser.screen().scrollback_filled()
}

fn status(app: &AppState) -> String {
    app.status_message.as_ref().map(|(m, _, _)| m.clone()).unwrap_or_default()
}

/// Type `text` one character at a time, the way the client forwards keys.
fn type_chars(app: &mut AppState, text: &str) {
    for c in text.chars() {
        crate::input::send_text_to_active(app, &c.to_string()).unwrap();
    }
}

fn enter(app: &mut AppState) {
    crate::input::send_key_to_active(app, "enter").unwrap();
}

fn is_prompt(app: &AppState) -> bool {
    matches!(app.mode, Mode::CopyCommandPrompt(_))
}

/// The exact line psmux's `list-keys -T copy-mode-vi` prints for `:`.
const LISTED_COLON: &str =
    "bind-key -T copy-mode-vi : command-prompt -p'(goto line)' { send -X goto-line -- '%%' }";

// ───────────────────────── parsing the binding ─────────────────────────

#[test]
fn tmux_default_goto_binding_parses_to_label_and_brace_body() {
    let pa = parse_args("-p'(goto line)' { send -X goto-line -- '%%' }");
    assert_eq!(pa.prompts.as_deref(), Some("(goto line)"));
    assert_eq!(pa.template.as_deref(), Some("send -X goto-line -- '%%'"));
    assert!(!pa.single && !pa.numeric);
}

#[test]
fn tmux_34_list_keys_shapes_parse() {
    // What tmux 3.4 `list-keys -T copy-mode-vi` prints for `/`, `f` and `1`.
    let pa = parse_args("-T search -p \"(search down)\" { send-keys -X search-forward \"%%\" }");
    assert_eq!(pa.prompts.as_deref(), Some("(search down)"));
    assert_eq!(pa.template.as_deref(), Some("send-keys -X search-forward \"%%\""));
    let pa = parse_args("-1 -p \"(jump forward)\" { send-keys -X jump-forward \"%%\" }");
    assert!(pa.single);
    assert_eq!(pa.prompts.as_deref(), Some("(jump forward)"));
    let pa = parse_args("-N -I 1 -p (repeat) { send-keys -N \"%%\" }");
    assert!(pa.numeric);
    assert_eq!(pa.inputs.as_deref(), Some("1"));
    assert_eq!(pa.template.as_deref(), Some("send-keys -N \"%%\""));
}

#[test]
fn glued_flags_and_single_string_template() {
    let pa = parse_args("-1p'(jump to forward)' \"send -X jump-to-forward '%%'\"");
    assert!(pa.single);
    assert_eq!(pa.prompts.as_deref(), Some("(jump to forward)"));
    assert_eq!(pa.template.as_deref(), Some("send -X jump-to-forward '%%'"));
}

#[test]
fn no_template_is_a_bare_command_prompt() {
    let pa = parse_args("");
    assert_eq!(pa.template, None);
    let app = copy_app(&[]);
    let p = build(&app, "");
    assert_eq!(p.status_text(), ":", "a bare prompt reads `:` with no space (cmd-command-prompt.c:119)");
}

#[test]
fn label_without_p_names_the_command() {
    let app = copy_app(&[]);
    let p = build(&app, "{ send -X goto-line -- '%%' }");
    assert_eq!(p.status_text(), "(send) ");
}

// ───────────────────────── %% replacement ─────────────────────────

#[test]
fn percent_percent_is_replaced_once() {
    assert_eq!(template_replace("goto-line '%%'", "500", 1), "goto-line '500'");
    // tmux replaces only the first %% (cmd.c:877).
    assert_eq!(template_replace("a %% b %%", "x", 1), "a x b %%");
}

#[test]
fn numbered_placeholders_follow_their_index() {
    let t = template_replace("x '%1' '%2' '%1'", "A", 1);
    assert_eq!(t, "x 'A' '%2' 'A'");
    assert_eq!(template_replace(&t, "B", 2), "x 'A' 'B' 'A'");
}

#[test]
fn a_typed_quote_survives_the_command_parser() {
    for typed in ["it's", "a \"b\" c", "back\\slash", "x y"] {
        let sq = template_replace("send -X search-forward '%%'", typed, 1);
        let parts = crate::commands::parse_command_line(&sq);
        assert_eq!(parts.last().map(String::as_str), Some(typed), "single quoted: {sq}");
        let dq = template_replace("send -X search-forward \"%%%\"", typed, 1);
        let parts = crate::commands::parse_command_line(&dq);
        assert_eq!(parts.last().map(String::as_str), Some(typed), "double quoted: {dq}");
    }
}

#[test]
fn final_command_applies_every_answer() {
    let cmd = final_command(Some("send -X search-forward '%1-%2'"), &["line".into(), "5".into()]);
    assert_eq!(cmd, "send -X search-forward 'line-5'");
    // `%1%` is the double quoted spelling of %1 (cmd.c:872), so `%1%2` eats
    // the second placeholder's percent sign in tmux too.
    let cmd = final_command(Some("x '%1%2'"), &["a".into(), "b".into()]);
    assert_eq!(cmd, "x 'a2'");
    assert_eq!(final_command(None, &["send -X history-top".into()]), "send -X history-top");
}

// ───────────────────────── the binding, live ─────────────────────────

#[test]
fn colon_bound_from_list_keys_opens_a_prompt_and_stays_in_copy_mode() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert!(is_prompt(&app), "the binding must open a copy mode prompt");
    assert!(app.mode.in_copy(), "the pane must stay in copy mode while the prompt is open");
    assert_eq!(status(&app), "(goto line) ");
    type_chars(&mut app, "12");
    assert_eq!(status(&app), "(goto line) 12");
}

#[test]
fn colon_bound_from_list_keys_matches_the_built_in() {
    // The built in `:` and the printed binding must land on the same line.
    let mut builtin = copy_app(&[]);
    crate::input::send_text_to_active(&mut builtin, ":").unwrap();
    type_chars(&mut builtin, "12");
    enter(&mut builtin);

    let mut bound = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut bound, ":").unwrap();
    type_chars(&mut bound, "12");
    enter(&mut bound);

    assert!(matches!(bound.mode, Mode::CopyMode), "Enter must leave the prompt, not copy mode");
    assert_eq!(bound.copy_scroll_offset, hsize(&bound) - 11, "line 12 is hsize - 11 rows back");
    assert_eq!(bound.copy_scroll_offset, builtin.copy_scroll_offset);
    assert_eq!(status(&bound), "", "the prompt clears once accepted");
}

#[test]
fn colon_bound_in_one_send_text_types_into_the_prompt() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":12").unwrap();
    assert_eq!(status(&app), "(goto line) 12");
    enter(&mut app);
    assert_eq!(app.copy_scroll_offset, hsize(&app) - 11);
}

#[test]
fn a_runtime_bind_with_a_quoted_template_works() {
    let mut app = copy_app(&["bind -T copy-mode-vi X command-prompt -p 'line?' \"send -X goto-line -- '%%'\""]);
    crate::input::send_text_to_active(&mut app, "X").unwrap();
    assert_eq!(status(&app), "line? ");
    type_chars(&mut app, "1");
    enter(&mut app);
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_scroll_offset, hsize(&app));
}

#[test]
fn escape_cancels_and_keeps_copy_mode() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    type_chars(&mut app, "3");
    crate::input::send_key_to_active(&mut app, "escape").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_scroll_offset, 0);
    assert_eq!(status(&app), "");
}

#[test]
fn copy_mode_keys_are_inert_while_the_prompt_is_open() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    type_chars(&mut app, "q");
    assert!(is_prompt(&app), "`q` is text for the prompt, not cancel");
    assert_eq!(status(&app), "(goto line) q");
}

#[test]
fn backspace_edits_the_prompt() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    type_chars(&mut app, "12");
    crate::input::send_key_to_active(&mut app, "bspace").unwrap();
    assert_eq!(status(&app), "(goto line) 1");
}

#[test]
fn search_template_searches_and_stays_in_copy_mode() {
    let mut app = copy_app(&[
        "bind -T copy-mode-vi / command-prompt -T search -p'(search down)' { send -X search-forward -- '%%' }",
    ]);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    assert_eq!(status(&app), "(search down) ");
    type_chars(&mut app, "line-3");
    enter(&mut app);
    assert!(matches!(app.mode, Mode::CopyMode), "search runs in copy mode and stays there");
    assert_eq!(app.copy_search_query, "line-3");
}

#[test]
fn search_backward_with_spaces_in_the_term() {
    let mut app = copy_app(&[
        "bind -T copy-mode-vi ? command-prompt -p'(search up)' { send -X search-backward \"%%\" }",
    ]);
    crate::input::send_text_to_active(&mut app, "?").unwrap();
    type_chars(&mut app, "a b");
    enter(&mut app);
    assert_eq!(app.copy_search_query, "a b");
    assert!(!app.copy_search_forward);
}

#[test]
fn single_key_prompt_jumps_at_once() {
    let mut app = copy_app(&[
        "bind -T copy-mode-vi f command-prompt -1p'(jump forward)' { send -X jump-forward -- '%%' }",
    ]);
    let (row, col) = app.copy_pos.unwrap();
    assert_eq!(col, 0);
    crate::input::send_text_to_active(&mut app, "f").unwrap();
    assert_eq!(status(&app), "(jump forward) ");
    crate::input::send_text_to_active(&mut app, "-").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode), "-1 closes after one key");
    assert_eq!(app.copy_pos, Some((row, 4)), "jumped onto the `-` of line-N");
    assert_eq!(app.copy_last_jump.map(|j| j.1), Some('-'), "`;` can repeat it");
}

#[test]
fn single_key_prompt_in_one_send_text_leaves_the_rest_to_copy_mode() {
    let mut app = copy_app(&[
        "bind -T copy-mode-vi t command-prompt -1p'(jump to forward)' { send -X jump-to-forward -- '%%' }",
    ]);
    let (row, _) = app.copy_pos.unwrap();
    crate::input::send_text_to_active(&mut app, "t-l").unwrap();
    // `t-` parks just before the dash, then `l` is cursor-right.
    assert_eq!(app.copy_pos, Some((row, 4)));
    assert!(matches!(app.mode, Mode::CopyMode));
}

#[test]
fn a_count_before_the_key_reaches_the_command() {
    // `ab-cd-ef` on the cursor line so a second dash exists.
    let mut app = copy_app(&[
        "bind -T copy-mode-vi f command-prompt -1p'(jump forward)' { send -X jump-forward -- '%%' }",
    ]);
    {
        let win = &app.windows[0];
        let pane = crate::tree::active_pane(&win.root, &win.active_path).unwrap();
        pane.term.lock().unwrap().process(b"ab-cd-ef");
    }
    let row = ROWS - 1;
    app.copy_pos = Some((row, 0));
    crate::input::send_text_to_active(&mut app, "2f-").unwrap();
    assert_eq!(app.copy_pos, Some((row, 5)), "2f- reaches the second dash");
    assert_eq!(app.copy_count, None, "the count is spent");
}

#[test]
fn numeric_prompt_sets_the_repeat_count_for_the_next_key() {
    // tmux's own `5` in copy-mode-vi (key-bindings.c:665).
    let mut app = copy_app(&[
        "bind -T copy-mode-vi 3 command-prompt -N -I 3 -p (repeat) { send-keys -N \"%%\" }",
    ]);
    let (row, _) = app.copy_pos.unwrap();
    crate::input::send_text_to_active(&mut app, "3").unwrap();
    assert_eq!(status(&app), "(repeat) 3", "-I seeds the prompt");
    // `k` is not a digit: it answers the prompt and is then itself.
    crate::input::send_text_to_active(&mut app, "k").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_pos.map(|p| p.0), Some(row - 3), "k moved three rows");
}

#[test]
fn multiple_prompts_fill_numbered_placeholders() {
    let mut app = copy_app(&[
        "bind -T copy-mode-vi Z command-prompt -p 'a,b' \"send -X search-forward '%1-%2'\"",
    ]);
    crate::input::send_text_to_active(&mut app, "Z").unwrap();
    assert_eq!(status(&app), "a ");
    type_chars(&mut app, "line");
    enter(&mut app);
    assert_eq!(status(&app), "b ", "the second prompt follows the first");
    type_chars(&mut app, "5");
    enter(&mut app);
    assert_eq!(app.copy_search_query, "line-5");
}

#[test]
fn bare_command_prompt_runs_the_typed_command_in_copy_mode() {
    let mut app = copy_app(&["bind -T copy-mode-vi Q command-prompt"]);
    crate::input::send_text_to_active(&mut app, "Q").unwrap();
    assert_eq!(status(&app), ":");
    type_chars(&mut app, "send -X history-top");
    enter(&mut app);
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_scroll_offset, hsize(&app));
}

#[test]
fn initial_text_is_format_expanded() {
    let mut app = copy_app(&["bind -T copy-mode-vi Y command-prompt -I '#{window_name}' -p 'w' \"send -X search-forward '%%'\""]);
    crate::input::send_text_to_active(&mut app, "Y").unwrap();
    assert_eq!(status(&app), "w w");
}

// ───────────────────────── what must not change ─────────────────────────

#[test]
fn built_in_colon_is_still_the_goto_prompt() {
    let mut app = copy_app(&[]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert!(matches!(app.mode, Mode::CopyGoto { .. }));
}

#[test]
fn built_in_search_keys_still_open_the_search_prompt() {
    let mut app = copy_app(&[]);
    crate::input::send_text_to_active(&mut app, "/").unwrap();
    assert!(matches!(app.mode, Mode::CopySearch { forward: true, .. }));
}

#[test]
fn unbinding_colon_leaves_it_dead() {
    let mut app = copy_app(&["unbind -T copy-mode-vi :"]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    assert!(matches!(app.mode, Mode::CopyMode), "an unbound key does nothing, as in tmux");
}

#[test]
fn command_prompt_outside_copy_mode_is_unchanged() {
    let mut app = copy_app(&[]);
    app.mode = Mode::Passthrough;
    crate::commands::execute_command_string(&mut app, "command-prompt -I abc").unwrap();
    assert!(matches!(app.mode, Mode::CommandPrompt { .. }));
}

#[test]
fn a_second_prompt_does_not_replace_the_open_one() {
    let mut app = copy_app(&[LISTED_COLON]);
    crate::input::send_text_to_active(&mut app, ":").unwrap();
    type_chars(&mut app, "7");
    crate::commands::execute_command_string(&mut app, "command-prompt -p other 'send -X history-top'").unwrap();
    assert_eq!(status(&app), "(goto line) 7");
}
