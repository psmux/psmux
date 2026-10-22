// Issue #842: a file run by source-file dropped every command but copy-mode,
// new-window and the select-pane / last-pane input toggles, and the ones it
// ran were sent back to the server to run after the whole file, out of order.
// `new-window -n win-1` then `split-window -h` made win-1 and never split it.
//
// tmux parses the whole file and runs its command list right after the
// source-file item, one command at a time (cmd-source-file.c:119, cfg.c:155
// to :204). source_runtime now keeps the lines from the first command on, in
// file order, for the source job to run (src/server/source_job.rs); the
// options and bindings before that first command still apply in place.
//
// These tests pin which lines are kept and how. The job itself is pinned in
// tests-rs/test_issue842_source_job.rs, and the keys end to end in
// tests/test_issue842_source_file_commands.ps1.

use super::*;
use crate::types::{DeferredConfigLine, DeferredKind};

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
    let mut app = AppState::new("sf".to_string());
    app.windows.push(make_window("shell", 0));
    app
}

/// A file of its own for each test, removed when the guard drops.
struct TempConf(std::path::PathBuf);
impl TempConf {
    fn new(tag: &str, content: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "psmux_test_sfc_{}_{}_{}.conf",
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

fn command(s: &str) -> DeferredKind {
    DeferredKind::Command(s.to_string())
}

fn directive(s: &str) -> DeferredKind {
    DeferredKind::Directive(s.to_string())
}

// ── the reported file ───────────────────────────────────────────────────────

#[test]
fn the_reported_file_keeps_both_commands_in_order() {
    let f = TempConf::new("report", "new-window -n \"win-1\"\nsplit-window -h\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("new-window -n \"win-1\""), command("split-window -h")]);
    assert_eq!(lines[0].line, Some(1));
    assert_eq!(lines[1].line, Some(2));
    assert_eq!(lines[0].file, f.path());
    assert!(app.source_defer.is_none(), "the context ends with the source");
}

#[test]
fn every_command_the_parser_used_to_drop_is_kept() {
    let f = TempConf::new(
        "dropped",
        "resize-pane -Z\nsend-keys 'echo hi' Enter\nselect-layout even-horizontal\ndisplay-message hello\n",
    );
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(
        kinds(&lines),
        vec![
            command("resize-pane -Z"),
            command("send-keys 'echo hi' Enter"),
            command("select-layout even-horizontal"),
            command("display-message hello"),
        ]
    );
}

#[test]
fn copy_mode_and_the_input_toggles_join_the_queue_too() {
    // They used to run in place, ahead of any command before them.
    let f = TempConf::new("special", "new-window\ncopy-mode\nselect-pane -d\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("new-window"), command("copy-mode"), command("select-pane -d")]);
}

// ── options and bindings around the first command ───────────────────────────

#[test]
fn lines_before_the_first_command_apply_now_and_the_rest_wait() {
    let f = TempConf::new(
        "order",
        "set -g status-left BEFORE\nsplit-window\nset -g status-right AFTER\n",
    );
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(app.status_left, "BEFORE");
    assert_ne!(app.status_right, "AFTER", "an option after a command waits for that command");
    assert_eq!(kinds(&lines), vec![command("split-window"), directive("set -g status-right AFTER")]);
    assert_eq!(lines[1].line, Some(3));
}

#[test]
fn a_file_of_options_only_keeps_nothing() {
    let f = TempConf::new("opts", "set -g status-left ONLY\nbind x kill-pane\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert!(lines.is_empty());
    assert_eq!(app.status_left, "ONLY");
}

// ── nesting ─────────────────────────────────────────────────────────────────

#[test]
fn a_nested_file_with_a_command_holds_back_the_rest_of_the_parent() {
    let child = TempConf::new("child", "split-window\nset -g status-right CHILD\n");
    let parent = TempConf::new(
        "parent",
        &format!("set -g status-left PARENT\nsource-file {}\nset -g history-limit 4321\n", child.path()),
    );
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &parent.path()).lines;
    assert_eq!(app.status_left, "PARENT");
    assert_ne!(app.history_limit, 4321);
    assert_eq!(
        kinds(&lines),
        vec![
            command("split-window"),
            directive("set -g status-right CHILD"),
            directive("set -g history-limit 4321"),
        ]
    );
    assert_eq!(lines[0].file, child.path());
    assert_eq!(lines[1].line, Some(2));
    assert_eq!(lines[2].file, parent.path());
    assert_eq!(lines[2].line, Some(3));
}

#[test]
fn a_source_file_after_a_command_waits_its_turn() {
    let f = TempConf::new("srcafter", "split-window\nsource-file other.conf\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("split-window"), directive("source-file other.conf")]);
}

// ── conditions ──────────────────────────────────────────────────────────────

#[test]
fn a_one_line_if_shell_branch_is_a_command() {
    let f = TempConf::new("ifline", "if -F 1 'split-window -h'\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("split-window -h")]);
}

#[test]
fn a_block_after_a_command_waits_whole() {
    // Its condition must see what the commands before it made, so it is not
    // evaluated at parse time.
    let f = TempConf::new("block", "split-window\nif -F 1 {\n  new-window -n inner\n}\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].kind, command("split-window"));
    match &lines[1].kind {
        DeferredKind::Block(text) => {
            assert!(text.starts_with("if -F 1 {"), "{}", text);
            assert!(text.contains("new-window -n inner"), "{}", text);
        }
        other => panic!("expected a block, got {:?}", other),
    }
    assert_eq!(lines[1].line, Some(2), "a block is reported at its first line");
}

#[test]
fn a_false_percent_if_keeps_nothing() {
    let f = TempConf::new("pctif", "%if 0\nsplit-window\n%endif\n");
    let mut app = app_with_window();
    assert!(source_runtime(&mut app, &f.path()).lines.is_empty());
}

#[test]
fn an_unknown_command_is_reported_and_not_kept() {
    let f = TempConf::new("unknown", "split-window\nbogus-command x\n");
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &f.path()).lines;
    assert_eq!(kinds(&lines), vec![command("split-window")]);
    assert!(
        app.config_warnings.iter().any(|w| w.ends_with(":2: unknown command: bogus-command")),
        "{:?}",
        app.config_warnings
    );
}

// ── where nothing is kept ───────────────────────────────────────────────────

#[test]
fn a_server_with_no_window_keeps_nothing() {
    let f = TempConf::new("nowin", "split-window\n");
    let mut app = AppState::new("sf".to_string());
    assert!(source_runtime(&mut app, &f.path()).lines.is_empty());
}

#[test]
fn a_warm_standby_keeps_nothing() {
    let f = TempConf::new("warm", "split-window\n");
    let mut app = app_with_window();
    app.session_name = "__warm__".to_string();
    assert!(source_runtime(&mut app, &f.path()).lines.is_empty());
}

#[test]
fn a_plain_source_file_has_no_queue() {
    // Menus and the other callers of source_file keep their
    // behaviour: no context, nothing kept.
    let f = TempConf::new("plain", "split-window\nset -g status-left PLAIN\n");
    let mut app = app_with_window();
    source_file(&mut app, &f.path());
    assert!(app.source_defer.is_none());
    assert_eq!(app.status_left, "PLAIN");
}

// ── warnings ────────────────────────────────────────────────────────────────

#[test]
fn warnings_with_no_command_still_reach_the_status_line() {
    let f = TempConf::new("warnonly", "bogus-command\n");
    let mut app = app_with_window();
    assert!(source_runtime(&mut app, &f.path()).lines.is_empty());
    let msg = app.status_message.as_ref().map(|m| m.0.clone()).unwrap_or_default();
    assert!(msg.contains("unknown command: bogus-command"), "{}", msg);
}

#[test]
fn with_commands_kept_the_status_line_waits_for_the_job() {
    let f = TempConf::new("warnwait", "bogus-command\nsplit-window\n");
    let mut app = app_with_window();
    assert_eq!(source_runtime(&mut app, &f.path()).lines.len(), 1);
    assert!(app.status_message.is_none(), "the job reports once the commands have run");
    assert_eq!(app.config_warnings.len(), 1);
}

// ── replaying a kept line ───────────────────────────────────────────────────

fn kept(file: &str, line: usize, kind: DeferredKind) -> DeferredConfigLine {
    DeferredConfigLine { file: file.to_string(), line: Some(line), kind, ancestors: Vec::new() }
}

#[test]
fn a_replayed_line_warns_at_its_own_file_and_line() {
    let mut app = app_with_window();
    app.config_warnings = vec!["earlier".to_string()];
    let result = replay_deferred_line(&mut app, &kept("f.conf", 7, directive("bogus-command x")));
    assert!(result.warnings.iter().any(|w| w == "f.conf:7: unknown command: bogus-command"), "{:?}", result.warnings);
    assert_eq!(app.config_warnings, vec!["earlier".to_string()], "the server's batch is left alone");
    assert!(app.source_defer.is_none());
}

#[test]
fn a_replayed_option_applies() {
    let mut app = app_with_window();
    let result = replay_deferred_line(&mut app, &kept("f.conf", 3, directive("set -g status-left REPLAYED")));
    assert!(result.deferred.is_empty());
    assert_eq!(app.status_left, "REPLAYED");
}

#[test]
fn a_replayed_source_file_hands_back_its_commands() {
    let child = TempConf::new("rchild", "new-window\nsplit-window\n");
    let mut app = app_with_window();
    let result = replay_deferred_line(
        &mut app,
        &kept("parent.conf", 2, directive(&format!("source-file {}", child.path()))),
    );
    assert_eq!(kinds(&result.deferred), vec![command("new-window"), command("split-window")]);
    assert_eq!(result.deferred[1].file, child.path());
    assert_eq!(result.deferred[1].line, Some(2));
}

#[test]
fn a_replayed_block_runs_its_condition_then_hands_back_the_branch() {
    let mut app = app_with_window();
    let text = "if -F 1 {\nsplit-window\n} {\nnew-window\n}\n".to_string();
    let result = replay_deferred_line(&mut app, &kept("f.conf", 4, DeferredKind::Block(text)));
    assert_eq!(kinds(&result.deferred), vec![command("split-window")]);
}

// ── which lines the parser applies itself ───────────────────────────────────

#[test]
fn the_directive_list_matches_the_parser_chain() {
    for l in [
        "set-option -g x y", "set -g x y", "setw -g x y", "set-window-option -g x y",
        "bind-key x a", "bind x a", "unbind-key x", "unbind x",
        "source-file f", "source f", "run-shell true", "run true",
        "if-shell true a", "if true a", "set-hook -g h a",
        "set-environment X 1", "setenv X 1",
    ] {
        assert!(is_config_directive(l), "{} is applied by the parser", l);
    }
    for l in ["split-window -h", "new-window", "setb x", "select-pane -d", "copy-mode", "display-message x"] {
        assert!(!is_config_directive(l), "{} is a command", l);
    }
}

// ── nesting and files that source themselves ───────────────────────────────

#[test]
fn a_file_that_sources_itself_is_refused_before_any_line_runs() {
    let f = TempConf::new("self", "set -g status-left SELF\nsplit-window\n");
    let content = format!("set -g status-left SELF\nsplit-window\nsource-file {}\n", f.path());
    std::fs::write(&f.0, content).unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.lines.is_empty(), "nothing runs: {:?}", parsed.lines);
    assert_ne!(app.status_left, "SELF", "not even the options before the cycle");
    assert_eq!(parsed.errors.len(), 1);
    assert!(parsed.errors[0].contains("is already being sourced"), "{}", parsed.errors[0]);
}

#[test]
fn a_cycle_through_another_file_is_refused_too() {
    let a = TempConf::new("cyc_a", "");
    let b = TempConf::new("cyc_b", "");
    std::fs::write(&a.0, format!("set -g status-left A\nsource-file {}\n", b.path())).unwrap();
    std::fs::write(&b.0, format!("new-window\nsource-file {}\n", a.path())).unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &a.path());
    assert!(parsed.lines.is_empty());
    assert_ne!(app.status_left, "A");
    assert!(parsed.errors[0].contains(" -> "), "the message shows the chain: {}", parsed.errors[0]);
}

#[test]
fn the_same_file_from_two_branches_is_not_a_cycle() {
    let d = TempConf::new("dia_d", "set -g @d 1\n");
    let b = TempConf::new("dia_b", &format!("source-file {}\n", d.path()));
    let c = TempConf::new("dia_c", &format!("source-file {}\n", d.path()));
    let a = TempConf::new("dia_a", &format!("source-file {}\nsource-file {}\nset -g status-left DIAMOND\n", b.path(), c.path()));
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &a.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(app.status_left, "DIAMOND");
}

#[test]
fn a_cycle_through_a_format_path_is_stopped_when_it_runs() {
    // The path is only known once the format is expanded, so the check
    // before the parse cannot see it; source_file refuses it.
    let f = TempConf::new("fmt", "");
    std::fs::write(&f.0, format!("set -g @self '{}'\nsource-file -F '#{{@self}}'\nset -g status-right FMT\n", f.path().replace('\\', "/"))).unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains("is already being sourced"), "{}", parsed.errors[0]);
    assert!(parsed.errors[0].contains(":2: "), "reported at the source-file line: {}", parsed.errors[0]);
    assert_eq!(app.status_right, "FMT", "the rest of the file still runs, once");
}

#[test]
fn nesting_deeper_than_tmuxs_limit_is_too_many_nested_files() {
    // f0 sources f1, ..., f50: 51 files, one more than tmux's 50.
    let files: Vec<TempConf> = (0..=50).map(|i| TempConf::new(&format!("deep{}", i), "")).collect();
    for i in 0..50 {
        std::fs::write(&files[i].0, format!("source-file {}\n", files[i + 1].path())).unwrap();
    }
    std::fs::write(&files[50].0, "set -g status-left DEEPEST\n").unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &files[0].path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].ends_with("too many nested files"), "{}", parsed.errors[0]);
    assert_ne!(app.status_left, "DEEPEST");
}

#[test]
fn fifty_levels_are_fine() {
    let files: Vec<TempConf> = (0..50).map(|i| TempConf::new(&format!("fifty{}", i), "")).collect();
    for i in 0..49 {
        std::fs::write(&files[i].0, format!("source-file {}\n", files[i + 1].path())).unwrap();
    }
    std::fs::write(&files[49].0, "set -g status-left FIFTIETH\n").unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &files[0].path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(app.status_left, "FIFTIETH");
}

#[test]
fn a_kept_line_remembers_the_files_it_was_read_in() {
    let child = TempConf::new("anc_child", "split-window\nset -g @x 1\n");
    let parent = TempConf::new("anc_parent", &format!("source-file {}\n", child.path()));
    let mut app = app_with_window();
    let lines = source_runtime(&mut app, &parent.path()).lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].ancestors, vec![source_key(&parent.path()), source_key(&child.path())]);
}

#[test]
fn a_replayed_source_file_of_an_ancestor_is_refused() {
    let f = TempConf::new("replay_self", "split-window\n");
    let mut app = app_with_window();
    let mut item = kept(&f.path(), 2, directive(&format!("source-file {}", f.path())));
    item.ancestors = vec![source_key(&f.path())];
    let result = replay_deferred_line(&mut app, &item);
    assert!(result.deferred.is_empty(), "{:?}", result.deferred);
    assert_eq!(result.warnings.len(), 1);
    assert!(result.warnings[0].contains("is already being sourced"), "{}", result.warnings[0]);
}

#[test]
fn a_file_is_known_by_one_key_however_it_is_spelled() {
    let f = TempConf::new("spell", "");
    let path = f.path();
    let key = source_key(&path);
    let dir = f.0.parent().unwrap().to_path_buf();
    let name = f.0.file_name().unwrap().to_string_lossy().into_owned();
    let dotdot = dir.join("sub").join("..").join(&name).display().to_string();
    for spelling in [path.to_uppercase(), path.replace('\\', "/"), dotdot] {
        assert_eq!(source_key(&spelling), key, "{} names {}", spelling, path);
    }
    let other = TempConf::new("spell_other", "");
    assert_ne!(source_key(&other.path()), key);
}

// ── review follow up: the arguments, the cycle check, errors ───────────────

#[test]
fn the_flags_are_read_and_the_path_kept_as_written() {
    let s = parse_source_spec(r"-q C:\Users\me\.tmux.conf");
    assert!(s.quiet && !s.format && !s.parse_only);
    assert_eq!(s.paths, [r"C:\Users\me\.tmux.conf"], "backslashes stay");
    let s = parse_source_spec("-qF '#{@dir}/x.conf'");
    assert!(s.quiet && s.format);
    assert_eq!(s.paths, ["#{@dir}/x.conf"]);
    assert_eq!(parse_source_spec("-t %1 a.conf").paths, ["a.conf"], "-t's value is not the path");
    assert_eq!(parse_source_spec("-t%1 a.conf").paths, ["a.conf"]);
    assert_eq!(parse_source_spec("-qt %1 a.conf").paths, ["a.conf"]);
    assert!(parse_source_spec("-n a.conf").parse_only);
    assert_eq!(parse_source_spec("\"C:\\My Dir\\a.conf\"").paths, ["C:\\My Dir\\a.conf"]);
    assert_eq!(parse_source_spec("-- -odd.conf").paths, ["-odd.conf"]);
    assert_eq!(parse_source_spec("a.conf"), SourceSpec { paths: vec!["a.conf".into()], ..Default::default() });
}

#[test]
fn a_reload_key_bound_in_a_block_is_not_a_cycle() {
    // The `source-file` runs only when the key is pressed.
    let f = TempConf::new("bindblock", "");
    std::fs::write(
        &f.0,
        format!("bind r {{\n  source-file {}\n  display-message reloaded\n}}\nset -g status-left BLOCK\n", f.path()),
    )
    .unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(app.status_left, "BLOCK");
}

#[test]
fn a_source_file_under_a_false_percent_if_is_not_a_cycle() {
    let f = TempConf::new("pctif_self", "");
    std::fs::write(&f.0, format!("%if 0\nsource-file {}\n%endif\nset -g status-left PCTIF\n", f.path())).unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(app.status_left, "PCTIF");
}

#[test]
fn a_source_file_in_a_false_if_shell_block_is_not_a_cycle() {
    let f = TempConf::new("ifblock_self", "");
    std::fs::write(
        &f.0,
        format!("if-shell false {{\n  source-file {}\n}} {{\n  set -g status-right ELSE\n}}\nset -g status-left IFBLOCK\n", f.path()),
    )
    .unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_eq!(app.status_left, "IFBLOCK");
    assert_eq!(app.status_right, "ELSE", "the else block still runs");
}

#[test]
fn a_cycle_that_does_run_through_a_true_block_is_stopped_when_it_runs() {
    // Not seen up front, since a block may or may not run; source_file
    // refuses it once it does.
    let f = TempConf::new("ifblock_true", "");
    std::fs::write(&f.0, format!("if-shell -F 1 {{\n  source-file {}\n}}\nset -g status-left AFTER\n", f.path())).unwrap();
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains("is already being sourced"), "{}", parsed.errors[0]);
    assert_eq!(app.status_left, "AFTER", "read once, then the rest of the file");
}

#[test]
fn only_lines_that_always_run_are_followed() {
    let lines = unconditional_lines(
        "source-file a\nbind r {\n  source-file b\n}\n%if 1\nsource-file c\n%endif\nif true {\n source-file d\n} {\n source-file e\n}\nsource-file \\\n  f\n",
    );
    let sources: Vec<&String> = lines.iter().filter(|l| l.starts_with("source-file")).collect();
    assert_eq!(sources.len(), 2, "{:?}", lines);
    assert_eq!(sources[0], "source-file a");
    assert!(sources[1].starts_with("source-file") && sources[1].ends_with(" f"), "{:?}", sources[1]);
}

#[test]
fn a_missing_nested_file_is_an_error_for_the_client() {
    let f = TempConf::new("missing", "set -g status-left M1\nsource-file C:\\psmux-no-such-dir\\none.conf\nset -g status-right M3\n");
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains(":2: cannot source"), "{}", parsed.errors[0]);
    assert_eq!(app.status_right, "M3", "the rest of the file still runs");
}

#[test]
fn a_missing_file_with_q_is_skipped_quietly() {
    let f = TempConf::new("missing_q", "source-file -q C:\\psmux-no-such-dir\\none.conf\nset -g status-left QUIET\n");
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(app.status_left, "QUIET");
}

#[test]
fn a_glob_that_matches_nothing_is_an_error_unless_q() {
    let f = TempConf::new("glob_none", "source-file C:\\psmux-no-such-dir\\*.conf\nsource-file -q C:\\psmux-no-such-dir\\*.conf\n");
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains(":1: No such file or directory"), "{}", parsed.errors[0]);
}

#[test]
fn an_invalid_option_is_an_error_for_the_client() {
    let f = TempConf::new("badopt", "set -g no-such-option 1\nset -g status-left AFTER_BAD\n");
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains(":1: "), "{}", parsed.errors[0]);
    assert_eq!(app.status_left, "AFTER_BAD");
}

#[test]
fn parse_only_applies_nothing() {
    let child = TempConf::new("nchild", "set -g status-left PARSED\n");
    let f = TempConf::new("nparent", &format!("source-file -n {}\n", child.path()));
    let mut app = app_with_window();
    let parsed = source_runtime(&mut app, &f.path());
    assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
    assert_ne!(app.status_left, "PARSED");
}
