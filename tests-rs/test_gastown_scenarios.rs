// Tests derived from gastown's Go tmux-wrapper test suite, ported to psmux
// unit tests. Covers the applicable subset: tmux-level behaviours that psmux
// implements independently of gastown's AI-orchestration features.
//
// Reference: https://github.com/gastownhall/gastown/tree/677877bf/internal/tmux
//
// Files analysed:
//   tmux_test.go, session_creation_test.go, socket_test.go,
//   cross_socket_test.go (gastown-specific AI/dialog/theme tests omitted)

use super::*;
use crate::types::CtrlReq;

// ─── shared helpers ──────────────────────────────────────────────────────────

fn mock_app() -> AppState {
    let mut app = AppState::new("test_session".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app
}

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
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

fn mock_app_with_window() -> AppState {
    let mut app = mock_app();
    app.windows.push(make_window("shell", 0));
    app
}

// ═════════════════════════════════════════════════════════════════════════════
// is_warm_session() tests
// From gastown: TestNewSessionSet* (warm sessions are filtered from listings),
//              socket_test.go (SetGetDefaultSocket creates __warm__ sentinel),
//              cross_socket_test.go (namespaced warm servers use <ns>____warm__)
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn warm_session_exact_sentinel() {
    // The global (no-namespace) warm server is always named "__warm__"
    assert!(
        crate::session::is_warm_session("__warm__"),
        "__warm__ must be recognised as a warm session"
    );
}

#[test]
fn warm_session_namespaced_with_double_underscore() {
    // When socket_name = "foo", the warm server is "foo____warm__"
    // (double __ separator between namespace and the sentinel)
    assert!(
        crate::session::is_warm_session("foo____warm__"),
        "foo____warm__ must be recognised as a warm session"
    );
}

#[test]
fn warm_session_another_namespace() {
    assert!(
        crate::session::is_warm_session("myns____warm__"),
        "myns____warm__ must be recognised as a warm session"
    );
}

#[test]
fn warm_session_regular_name_is_not_warm() {
    assert!(
        !crate::session::is_warm_session("myapp"),
        "Regular session name must not be warm"
    );
}

#[test]
fn warm_session_empty_string_is_not_warm() {
    assert!(
        !crate::session::is_warm_session(""),
        "Empty string must not be warm"
    );
}

#[test]
fn warm_session_partial_sentinel_names_are_not_warm() {
    // Substrings of the sentinel must not match
    assert!(!crate::session::is_warm_session("warm"));
    assert!(!crate::session::is_warm_session("__warm"));
    assert!(!crate::session::is_warm_session("warm__"));
    assert!(!crate::session::is_warm_session("_warm_"));
}

#[test]
fn warm_session_namespaced_regular_session_is_not_warm() {
    // "ns__0" and "ns__mysession" are regular namespaced sessions
    assert!(!crate::session::is_warm_session("ns__0"));
    assert!(!crate::session::is_warm_session("ns__mysession"));
}

#[test]
fn warm_session_numeric_ids_are_not_warm() {
    assert!(!crate::session::is_warm_session("0"));
    assert!(!crate::session::is_warm_session("1"));
    assert!(!crate::session::is_warm_session("42"));
}

// ═════════════════════════════════════════════════════════════════════════════
// ensure_background() tests
// From gastown: TestAutoRespawnHookCmd_Format — hook commands must always be
//              dispatched as background (non-blocking) run-shell invocations.
// In psmux: fire_hooks() wraps every hook command with ensure_background()
//           before executing them, preventing "running: ..." status noise.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn ensure_background_adds_b_flag_to_run_shell() {
    assert_eq!(
        ensure_background("run-shell echo hello"),
        "run-shell -b echo hello"
    );
}

#[test]
fn ensure_background_adds_b_flag_to_run_shell_with_quoted_script() {
    assert_eq!(
        ensure_background("run-shell 'echo hook fired'"),
        "run-shell -b 'echo hook fired'"
    );
}

#[test]
fn ensure_background_does_not_double_b_flag() {
    // If -b is already present, must NOT produce "run-shell -b -b ..."
    assert_eq!(
        ensure_background("run-shell -b echo hello"),
        "run-shell -b echo hello"
    );
}

#[test]
fn ensure_background_handles_run_alias() {
    // "run" is the tmux alias for "run-shell"
    assert_eq!(ensure_background("run echo hello"), "run -b echo hello");
}

#[test]
fn ensure_background_run_alias_does_not_double_b_flag() {
    assert_eq!(ensure_background("run -b echo hello"), "run -b echo hello");
}

#[test]
fn ensure_background_non_run_shell_command_is_unchanged() {
    // Commands other than run-shell / run must not be modified
    assert_eq!(
        ensure_background("display-message hello"),
        "display-message hello"
    );
    assert_eq!(
        ensure_background("set-hook after-new-window ''"),
        "set-hook after-new-window ''"
    );
    assert_eq!(
        ensure_background("bind-key C-b send-prefix"),
        "bind-key C-b send-prefix"
    );
}

#[test]
fn ensure_background_empty_string_is_unchanged() {
    assert_eq!(ensure_background(""), "");
}

#[test]
fn ensure_background_is_idempotent() {
    // Applying ensure_background twice must not double the -b flag
    let cmd = "run-shell echo test";
    let once = ensure_background(cmd);
    let twice = ensure_background(&once);
    assert_eq!(once, twice, "ensure_background must be idempotent");
}

// ═════════════════════════════════════════════════════════════════════════════
// fire_hooks() + ensure_background integration
// From gastown: AutoRespawnHook_RespawnWorks verifies the hook fires without
//              blocking the caller.  Here we verify that psmux never sets the
//              "running: ..." status bar message when firing hooks.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn fire_hooks_with_run_shell_does_not_set_running_status() {
    // Hooks must use -b so execution is fire-and-forget.
    // The "running: ..." status is only set on the foreground (no -b) path.
    let mut app = mock_app_with_window();
    app.hooks.insert(
        "test-event".to_string(),
        vec!["run-shell echo fired".to_string()],
    );
    fire_hooks(&mut app, "test-event");
    let is_running = app.status_message
        .as_ref()
        .map(|(msg, _, _)| msg.starts_with("running:"))
        .unwrap_or(false);
    assert!(
        !is_running,
        "fire_hooks must force -b flag; 'running:' status indicates foreground path was used"
    );
}

#[test]
fn fire_hooks_nonexistent_event_is_noop() {
    let mut app = mock_app_with_window();
    fire_hooks(&mut app, "no-such-event");
    // Should not panic and must not mutate mode
    assert!(matches!(app.mode, Mode::Passthrough));
}

// ═════════════════════════════════════════════════════════════════════════════
// parse_command_line() edge cases
// From gastown: SanitizeNudgeMessage, session_creation_test parse flags,
//              send-keys -l literal mode with special characters.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn parse_cmdline_empty_string_returns_empty_vec() {
    let result = parse_command_line("");
    assert!(result.is_empty(), "empty input must produce no tokens");
}

#[test]
fn parse_cmdline_single_token() {
    let result = parse_command_line("run-shell");
    assert_eq!(result, vec!["run-shell"]);
}

#[test]
fn parse_cmdline_multiple_whitespace_separated_tokens() {
    let result = parse_command_line("bind-key C-b send-prefix");
    assert_eq!(result, vec!["bind-key", "C-b", "send-prefix"]);
}

#[test]
fn parse_cmdline_extra_whitespace_is_ignored() {
    let result = parse_command_line("  bind-key   C-b   send-prefix  ");
    assert_eq!(result, vec!["bind-key", "C-b", "send-prefix"]);
}

#[test]
fn parse_cmdline_double_quoted_arg_preserves_spaces() {
    let result = parse_command_line(r#"display-message "hello world""#);
    assert_eq!(result, vec!["display-message", "hello world"]);
}

#[test]
fn parse_cmdline_single_quoted_arg_preserves_spaces() {
    let result = parse_command_line("run-shell 'echo hello world'");
    assert_eq!(result, vec!["run-shell", "echo hello world"]);
}

#[test]
fn parse_cmdline_double_quoted_escaped_quote() {
    // Inside double quotes, \" is a literal double-quote character
    let result = parse_command_line(r#"display-message "say \"hi\"""#);
    assert_eq!(result, vec!["display-message", r#"say "hi""#]);
}

#[test]
fn parse_cmdline_windows_backslash_path_preserved_in_double_quotes() {
    // psmux is Windows-native; backslashes in paths must not be consumed
    let result = parse_command_line(r#"run-shell "C:\Users\foo\script.ps1""#);
    assert_eq!(result, vec!["run-shell", r"C:\Users\foo\script.ps1"]);
}

#[test]
fn parse_cmdline_double_backslash_collapses_to_one() {
    // Inside double quotes, \\ is a literal single backslash
    let result = parse_command_line(r#"run-shell "path\\to\\file""#);
    assert_eq!(result, vec!["run-shell", r"path\to\file"]);
}

#[test]
fn parse_cmdline_new_session_s_flag() {
    // Verify flag parsing used by the new-session command handler
    let parts = parse_command_line("new-session -s myname -d");
    assert!(parts.contains(&"new-session".to_string()));
    assert!(parts.contains(&"-s".to_string()));
    assert!(parts.contains(&"myname".to_string()));
    assert!(parts.contains(&"-d".to_string()));
}

#[test]
fn parse_cmdline_quoted_session_name_is_single_token() {
    // A session name containing spaces must be quoted and kept as one token
    let parts = parse_command_line(r#"new-session -s "my project" -d"#);
    assert!(
        parts.contains(&"my project".to_string()),
        "quoted session name must be preserved as a single token; got: {:?}",
        parts
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// generate_list_panes() — structure tests
// From gastown: TestGetPaneCommand_MultiPane (all panes listed, pane 0 reachable)
// psmux: generate_list_panes() walks the window's Node tree; with no leaf
//        panes (mock window uses empty Split root), output is empty.
//        The PopupMode routing is verified via execute_command_string.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn list_panes_empty_window_returns_empty_output() {
    // Mock windows use Node::Split with no children — no panes to list
    let app = mock_app_with_window();
    let output = generate_list_panes(&app);
    assert!(
        output.is_empty(),
        "window with no leaf panes must produce no list-panes output"
    );
}

#[test]
fn list_panes_command_sets_popup_mode() {
    // Regardless of pane count, list-panes must set PopupMode
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-panes").unwrap();
    assert!(
        matches!(app.mode, Mode::PopupMode { .. }),
        "list-panes must set PopupMode"
    );
}

#[test]
fn list_panes_popup_has_correct_title() {
    let mut app = mock_app_with_window();
    execute_command_string(&mut app, "list-panes").unwrap();
    match &app.mode {
        Mode::PopupMode { command, .. } => {
            assert_eq!(command, "list-panes");
        }
        other => panic!("expected PopupMode, got {:?}", std::mem::discriminant(other)),
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// generate_show_hooks() format tests
// From gastown: AutoRespawnHookCmd_Format — hook output must follow the
//              "hookname -> command" format (single) and
//              "hookname[N] -> command" format (multiple).
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn show_hooks_empty_produces_sentinel() {
    let app = mock_app();
    let output = generate_show_hooks(&app, &[]);
    assert_eq!(output, "", "no hooks prints nothing, as tmux does");
}

#[test]
fn show_hooks_single_command_uses_arrow_format() {
    let mut app = mock_app_with_window();
    app.hooks.insert(
        "after-new-window".to_string(),
        vec!["run-shell -b echo fired".to_string()],
    );
    let output = generate_show_hooks(&app, &[]);
    assert!(
        output.contains("after-new-window[0] run-shell -b echo fired"),
        "single-command hook uses tmux's 'name[0] cmd' format; got: {}",
        output
    );
}

#[test]
fn show_hooks_multiple_commands_use_indexed_format() {
    let mut app = mock_app_with_window();
    app.hooks.insert(
        "session-created".to_string(),
        vec![
            "run-shell -b cmd1".to_string(),
            "run-shell -b cmd2".to_string(),
        ],
    );
    let output = generate_show_hooks(&app, &[]);
    assert!(
        output.contains("session-created[0] run-shell -b cmd1"),
        "first hook command must be indexed as [0]; got: {}",
        output
    );
    assert!(
        output.contains("session-created[1] run-shell -b cmd2"),
        "second hook command must be indexed as [1]; got: {}",
        output
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// generate_list_windows() content tests
// From gastown: CheckSessionHealth queries list-windows to verify windows exist.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn list_windows_contains_window_name() {
    let mut app = mock_app();
    app.windows.push(make_window("mywindow", 0));
    let output = generate_list_windows(&app);
    assert!(
        output.contains("mywindow"),
        "list-windows output must include the window name"
    );
}

#[test]
fn list_windows_includes_all_windows() {
    let mut app = mock_app();
    app.windows.push(make_window("alpha", 0));
    app.windows.push(make_window("beta", 1));
    let output = generate_list_windows(&app);
    assert!(output.contains("alpha"), "must list first window");
    assert!(output.contains("beta"), "must list second window");
}

// ═════════════════════════════════════════════════════════════════════════════
// Namespace (socket) isolation naming convention
// From gastown: TestCrossSocketIsolation — sessions on different sockets must
//              not share port files.  In psmux, socket_name is used as a
//              namespace prefix: "<ns>__<session>" for the port file base.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn namespaced_port_file_base_uses_double_underscore_separator() {
    // The new-session handler builds: format!("{}__{}", socket_name, session_name)
    let socket_name = "project1";
    let session_name = "main";
    let port_file_base = format!("{}__{}", socket_name, session_name);
    assert_eq!(
        port_file_base, "project1__main",
        "namespaced port file base must use __ as separator"
    );
}

#[test]
fn non_namespaced_port_file_base_is_bare_session_name() {
    let socket_name: Option<&str> = None;
    let session_name = "mysession";
    let port_file_base = if let Some(sn) = socket_name {
        format!("{}__{}", sn, session_name)
    } else {
        session_name.to_string()
    };
    assert_eq!(
        port_file_base, "mysession",
        "without namespace the port file base must equal the session name"
    );
}

#[test]
fn warm_server_base_for_namespace_uses_four_underscores() {
    // spawn_warm_server() builds: format!("{}____warm__", socket_name)
    // That is: namespace + "__" + "__warm__" = four underscores total
    let socket_name = "myns";
    let warm_base = format!("{}____warm__", socket_name);
    assert_eq!(warm_base, "myns____warm__");
    // Verify is_warm_session recognises this value
    assert!(
        crate::session::is_warm_session(&warm_base),
        "warm base for namespace must be recognised as warm"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// Session listing excludes warm sessions
// From gastown: TestNewSessionSet — list-sessions must not expose warm servers.
// psmux: list_session_names() filters out entries where is_warm_session == true.
//        This is filesystem-dependent; we verify the filtering logic directly.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn warm_session_names_are_excluded_from_visible_list() {
    // Simulate what list_session_names scan would do: filter out warm entries.
    let all_sessions = vec![
        "0".to_string(),
        "myapp".to_string(),
        "__warm__".to_string(),
        "ns__0".to_string(),
        "ns____warm__".to_string(),
    ];
    let visible: Vec<&str> = all_sessions
        .iter()
        .filter(|s| !crate::session::is_warm_session(s))
        .map(|s| s.as_str())
        .collect();

    assert!(visible.contains(&"0"), "numeric session must be visible");
    assert!(visible.contains(&"myapp"), "named session must be visible");
    assert!(visible.contains(&"ns__0"), "namespaced session must be visible");
    assert!(
        !visible.contains(&"__warm__"),
        "__warm__ must be hidden from session list"
    );
    assert!(
        !visible.contains(&"ns____warm__"),
        "ns____warm__ must be hidden from session list"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// respawn-pane -k flag pipeline (regression)
// From gastown: TestAutoRespawnHook_RespawnWorks sets a pane-died hook that
// invokes "respawn-pane -k". Previously the -k flag was parsed at the CLI
// level but silently discarded by the server. Fixed: CtrlReq::RespawnPane
// now carries (Option<String>, bool) and the full pipeline respects -k.
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn pane_died_hook_respawn_k_command_round_trips() {
    // The hook stores "run-shell 'respawn-pane -k'" as the command.
    // When fire_hooks processes it, ensure_background converts it to
    // "run-shell -b 'respawn-pane -k'". The inner respawn-pane -k
    // must survive the background wrapping unmodified.
    let hook_cmd = "run-shell 'respawn-pane -k'";
    let bg = ensure_background(hook_cmd);
    assert_eq!(bg, "run-shell -b 'respawn-pane -k'");
    // Verify the inner command is preserved
    assert!(bg.contains("respawn-pane -k"), "inner respawn-pane -k must survive ensure_background");
}

#[test]
fn auto_respawn_hook_full_chain_verify() {
    // This test mirrors gastown's TestAutoRespawnHook_RespawnWorks:
    // 1. set-hook pane-died[0] "run-shell 'respawn-pane -k'"
    // 2. Let pane die
    // 3. fire_hooks("pane-died") should dispatch respawn-pane -k
    //
    // We verify the hook registration, ensure_background, and that the
    // resulting command properly includes -k.
    let mut app = mock_app_with_window();

    // Step 1: Register the pane-died hook (mirrors set-hook command handler)
    let hook_name = "pane-died";
    let hook_cmd = "run-shell 'respawn-pane -k'".to_string();
    app.hooks.entry(hook_name.to_string()).or_default().push(hook_cmd.clone());

    // Verify hook was registered
    assert!(app.hooks.contains_key(hook_name), "hook must be registered under pane-died");
    assert_eq!(app.hooks[hook_name].len(), 1);
    assert_eq!(app.hooks[hook_name][0], "run-shell 'respawn-pane -k'");

    // Step 2: Fire the hook
    fire_hooks(&mut app, hook_name);

    // Step 3: Verify no "running:" status (background execution)
    let is_running = app.status_message
        .as_ref()
        .map(|(msg, _, _)| msg.starts_with("running:"))
        .unwrap_or(false);
    assert!(
        !is_running,
        "auto-respawn hook must fire via -b (background), not foreground"
    );
}

#[test]
fn ctrl_req_respawn_pane_carries_kill_flag() {
    // Verify CtrlReq::RespawnPane enum variant stores workdir, kill, and command.
    // (issue #399: added the optional `-- <command>` field.)
    let (resp_s, _resp_r) = std::sync::mpsc::channel();
    let req_with_kill = CtrlReq::RespawnPane(Some("/tmp".to_string()), true, None, false, resp_s, Vec::new());
    match req_with_kill {
        CtrlReq::RespawnPane(wd, kill, cmd, _, _, _) => {
            assert_eq!(wd.as_deref(), Some("/tmp"));
            assert!(kill, "kill flag must be true");
            assert!(cmd.is_none(), "no -- command in this case");
        }
        _ => panic!("wrong variant"),
    }

    // issue #399: a teammate launch delivers the command via `respawn-pane -- <cmd>`.
    let (resp_s, _resp_r) = std::sync::mpsc::channel();
    let req_with_cmd = CtrlReq::RespawnPane(None, true, Some("claude --agent-id Bob".to_string()), false, resp_s, Vec::new());
    match req_with_cmd {
        CtrlReq::RespawnPane(wd, kill, cmd, _, _, _) => {
            assert!(wd.is_none());
            assert!(kill);
            assert_eq!(cmd.as_deref(), Some("claude --agent-id Bob"), "-- command must be carried");
        }
        _ => panic!("wrong variant"),
    }

    let (resp_s, _resp_r) = std::sync::mpsc::channel();
    let req_without_kill = CtrlReq::RespawnPane(None, false, None, false, resp_s, Vec::new());
    match req_without_kill {
        CtrlReq::RespawnPane(wd, kill, cmd, _, _, _) => {
            assert!(wd.is_none());
            assert!(!kill, "kill flag must be false");
            assert!(cmd.is_none());
        }
        _ => panic!("wrong variant"),
    }
}
