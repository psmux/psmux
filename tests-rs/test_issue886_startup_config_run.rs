// Issue #886: the startup config dropped every command that is not a config
// directive (`set`, `bind`, `source-file`, `run-shell`, `if-shell`,
// `set-hook`, `setenv`) without a word: `split-window`, `new-window`,
// `set-buffer` and `new-session` in psmux.conf did nothing, and a `run-shell`
// without `-b` never held the lines after it.
//
// tmux runs the whole config through its command queue before the first
// client's command (cfg.c:71 to :92), a run-shell without -b waiting
// (cmd-run-shell.c:123, :186 to :188). load_config now parses the way a
// runtime source-file does (issue #842): options and bindings before the
// first command apply in place, and from that command on the lines are kept
// for a source job, which the server starts once it has its window.
//
// These tests pin what the startup parse keeps. The job is the one
// tests-rs/test_issue842_source_job.rs pins, and the server end to end is in
// tests/test_issue886_startup_config_run.ps1.

use super::*;
use crate::types::DeferredKind;

/// Parse `content` as load_config parses a startup config file.
fn startup_parse(app: &mut AppState, content: &str) {
    IN_STARTUP_LOAD.with(|c| c.set(true));
    let _guard = StartupLoadGuard;
    parse_startup_config(app, content);
}

fn kept_lines(app: &AppState) -> Vec<String> {
    app.startup_source_lines
        .iter()
        .map(|l| match &l.kind {
            DeferredKind::Command(s) => format!("command: {}", s),
            DeferredKind::Directive(s) => format!("directive: {}", s),
            DeferredKind::Block(s) => format!("block: {}", s),
        })
        .collect()
}

#[test]
fn options_before_the_first_command_apply_and_the_rest_is_kept() {
    let mut app = AppState::new("s".to_string());
    startup_parse(
        &mut app,
        "set -g status-left BEFORE\nsplit-window -h\nset -g status-right AFTER\n",
    );
    assert_eq!(app.status_left, "BEFORE");
    assert_ne!(app.status_right, "AFTER", "the line after the command waits for it");
    assert_eq!(
        kept_lines(&app),
        vec!["command: split-window -h".to_string(), "directive: set -g status-right AFTER".to_string()]
    );
    assert!(app.source_defer.is_none(), "the parse leaves no context behind");
}

#[test]
fn kept_lines_carry_their_line_numbers() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "set -g status-left A\n\nset-buffer -b cfgbuf from-config\n");
    assert_eq!(app.startup_source_lines.len(), 1);
    assert_eq!(app.startup_source_lines[0].line, Some(3));
}

#[test]
fn a_run_shell_without_b_holds_the_lines_after_it() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "run-shell 'exit 0'\nset -g status-left LATER\n");
    assert_ne!(app.status_left, "LATER");
    assert_eq!(
        kept_lines(&app),
        vec!["command: run-shell 'exit 0'".to_string(), "directive: set -g status-left LATER".to_string()]
    );
}

#[test]
fn a_run_shell_with_only_a_delay_is_kept_too() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "run-shell -d 2\nset -g status-left LATER\n");
    assert_eq!(kept_lines(&app).first().map(String::as_str), Some("command: run-shell -d 2"));
    assert_ne!(app.status_left, "LATER");
}

#[test]
fn new_session_and_the_window_commands_are_kept() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "new-session -d -s cfg\nnew-window -n w\ncopy-mode\n");
    assert_eq!(
        kept_lines(&app),
        vec![
            "command: new-session -d -s cfg".to_string(),
            "command: new-window -n w".to_string(),
            "command: copy-mode".to_string(),
        ]
    );
}

#[test]
fn an_unknown_command_is_still_a_warning_and_not_kept() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "bogus-command x\nset -g status-left OK\n");
    assert!(app.startup_source_lines.is_empty());
    assert_eq!(app.status_left, "OK");
    assert!(app.config_warnings.iter().any(|w| w.contains("unknown command: bogus-command")), "{:?}", app.config_warnings);
}

#[test]
fn a_config_with_no_command_keeps_nothing() {
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "set -g status-left ONLY\nbind-key x display-message hi\n");
    assert!(app.startup_source_lines.is_empty());
    assert_eq!(app.status_left, "ONLY");
}

#[test]
fn a_warm_standby_keeps_nothing() {
    // Its one window belongs to the next session; the claim loads the config
    // again and keeps the commands then.
    let mut app = AppState::new("__warm__".to_string());
    startup_parse(&mut app, "split-window -h\nset -g status-left AFTER\n");
    assert!(app.startup_source_lines.is_empty());
    assert_eq!(app.status_left, "AFTER", "the options apply as they always have");
}

#[test]
fn a_session_a_config_made_keeps_its_lines_too() {
    // Its job runs only some of the commands (made_session_runs), and the
    // config lines after them in file order.
    let mut app = AppState::new("cfg".to_string());
    app.config_made_session = true;
    startup_parse(&mut app, "new-session -d\nset -g status-left AFTER\n");
    assert_eq!(
        kept_lines(&app),
        vec!["command: new-session -d".to_string(), "directive: set -g status-left AFTER".to_string()]
    );
}

#[test]
fn a_session_a_config_made_runs_run_shell_and_the_buffer_commands_only() {
    // tmux runs its config once per server, and the session shares that
    // server: what the config made server wide, it has too.
    for line in [
        "run-shell 'echo hi'",
        "run -b 'echo hi'",
        "set-buffer -b b x",
        "setb x",
        "load-buffer -b b C:\\x.txt",
        "loadb x",
        "delete-buffer -b b",
        "deleteb",
    ] {
        assert!(made_session_runs(line), "{}", line);
    }
    // These acted on the session that loaded the config; new-session would
    // start servers without end.
    for line in ["new-session -d -s x", "new", "split-window -h", "new-window", "paste-buffer", "display-message hi"] {
        assert!(!made_session_runs(line), "{}", line);
    }
}

#[test]
fn a_warm_standby_starts_no_run_shell() {
    // The claim loads the config again and starts it then; the standby used
    // to start it too, so it ran twice. A file the shell makes shows it; the
    // same line on a server that is not a standby shows the check can see it.
    let wait_for = |mark: &std::path::Path, secs: u64| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while std::time::Instant::now() < deadline && !mark.exists() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let made = mark.exists();
        let _ = std::fs::remove_file(mark);
        made
    };
    let config = |mark: &std::path::Path| format!("run-shell -b 'cmd /c type nul > {}'\nset -g status-left AFTER\n", mark.display());

    let shown = std::env::temp_dir().join(format!("psmux_test_scw_on_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&shown);
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, &config(&shown));
    assert!(wait_for(&shown, 20), "a server that is not a standby starts the shell");

    let mark = std::env::temp_dir().join(format!("psmux_test_scw_off_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&mark);
    let mut app = AppState::new("__warm__".to_string());
    startup_parse(&mut app, &config(&mark));
    assert!(!wait_for(&mark, 3), "the standby started the shell");
    assert_eq!(app.status_left, "AFTER");
}

#[test]
fn a_runtime_source_still_keeps_its_own_lines() {
    // The shared part (parse_keeping_commands) leaves source_runtime as it was.
    let path = std::env::temp_dir().join(format!("psmux_test_scr_{}.conf", std::process::id()));
    std::fs::write(&path, "split-window\nset -g status-left AFTER\n").unwrap();
    let mut app = AppState::new("s".to_string());
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: "shell".to_string(),
        id: 0,
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
    });
    let parsed = source_runtime(&mut app, &path.display().to_string());
    let _ = std::fs::remove_file(&path);
    assert_eq!(parsed.lines.len(), 2);
    assert!(app.startup_source_lines.is_empty(), "a runtime source is not the startup config");
}

#[test]
fn a_plugin_source_at_startup_joins_the_startup_lines() {
    // A plugin script's source-file runs before the window exists; its
    // commands run on the startup job, after the config's own.
    let path = std::env::temp_dir().join(format!("psmux_test_scp_{}.conf", std::process::id()));
    std::fs::write(&path, "set -g status-left PLUG\nset-buffer -b plugbuf x\nset -g status-right LATE\n").unwrap();
    let mut app = AppState::new("s".to_string());
    startup_parse(&mut app, "split-window -h\n");
    let errors = source_during_startup(&mut app, &path.display().to_string());
    let _ = std::fs::remove_file(&path);
    assert!(errors.is_empty(), "{:?}", errors);
    assert_eq!(app.status_left, "PLUG", "the options before its first command apply now");
    assert_eq!(
        kept_lines(&app),
        vec![
            "command: split-window -h".to_string(),
            "command: set-buffer -b plugbuf x".to_string(),
            "directive: set -g status-right LATE".to_string(),
        ]
    );
}

#[test]
fn a_plugin_source_on_a_warm_standby_keeps_nothing() {
    let path = std::env::temp_dir().join(format!("psmux_test_scw_{}.conf", std::process::id()));
    std::fs::write(&path, "set-buffer -b plugbuf x\nset -g status-right LATE\n").unwrap();
    let mut app = AppState::new("__warm__".to_string());
    source_during_startup(&mut app, &path.display().to_string());
    let _ = std::fs::remove_file(&path);
    assert!(app.startup_source_lines.is_empty());
    assert_eq!(app.status_right, "LATE");
}

#[test]
fn a_job_commands_file_is_current_only_while_its_request_runs() {
    // The server loop sets the file a source job's command came from
    // (CtrlReq::InFile) for that one request and puts the old one back.
    set_current_config_file("");
    let mut saved = Some(swap_current_config_file("C:\\cfg\\a.conf"));
    assert_eq!(current_config_file(), "C:\\cfg\\a.conf");
    restore_current_config_file(&mut saved);
    assert_eq!(current_config_file(), "");
    assert!(saved.is_none());
    // Nothing was swapped: nothing to put back.
    set_current_config_file("C:\\cfg\\b.conf");
    restore_current_config_file(&mut saved);
    assert_eq!(current_config_file(), "C:\\cfg\\b.conf");
    set_current_config_file("");
}
