// Issue #847: source-file takes one or more paths, each of which may be a
// glob, and reads them in order (tmux.1:1631 to :1656, cmd-source-file.c:42,
// :203 to :242 and :126 to :128). psmux read one: the CLI kept the last path,
// the server's dispatcher and control mode the first, and a `source-file a b`
// line in a file was read as the single path `a b`, which cannot be opened.
//
// These tests pin how the arguments are split and how the paths are read.
// The routes end to end are in tests/test_issue847_source_file_paths.ps1.

use super::*;

/// A directory of its own for each test, removed when the guard drops.
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "psmux_test_sfp_{}_{}_{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
    /// Write `name` (which may hold a subdirectory) and return its path.
    fn file(&self, name: &str, content: &str) -> String {
        let p = self.0.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, content).unwrap();
        p.display().to_string()
    }
    fn path(&self, name: &str) -> String {
        self.0.join(name).display().to_string()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A server with no window yet, where a source applies in place.
fn app() -> AppState {
    AppState::new("sfp".to_string())
}

fn opt(app: &AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

fn q(p: &str) -> String {
    crate::util::quote_arg(p)
}

// ── the arguments ───────────────────────────────────────────────────────────

#[test]
fn every_path_is_kept_in_order() {
    let s = parse_source_spec("a.conf b.conf c.conf");
    assert_eq!(s.paths, ["a.conf", "b.conf", "c.conf"]);
    let s = parse_source_spec("-q a.conf b.conf");
    assert!(s.quiet);
    assert_eq!(s.paths, ["a.conf", "b.conf"]);
    let s = parse_source_spec("-qt %1 a.conf b.conf");
    assert_eq!(s.paths, ["a.conf", "b.conf"], "-t's value is not a path");
    let s = parse_source_spec("-- -odd.conf b.conf");
    assert_eq!(s.paths, ["-odd.conf", "b.conf"]);
}

#[test]
fn a_quoted_path_with_a_space_is_one_path_and_keeps_its_backslashes() {
    let s = parse_source_spec(r#""C:\My Dir\a.conf" C:\b.conf 'C:\c d.conf'"#);
    assert_eq!(s.paths, [r"C:\My Dir\a.conf", r"C:\b.conf", r"C:\c d.conf"]);
}

#[test]
fn the_flags_after_the_first_path_are_paths() {
    // tmux stops reading flags at the first argument (args_parse).
    let s = parse_source_spec("a.conf -q");
    assert!(!s.quiet);
    assert_eq!(s.paths, ["a.conf", "-q"]);
}

#[test]
fn split_arguments_read_the_same_as_text() {
    // The dispatcher and control mode split the line themselves.
    let s = source_spec_from_args(&["-q", "missing.conf", "a.conf"]);
    assert!(s.quiet);
    assert_eq!(s.paths, ["missing.conf", "a.conf"]);
    assert_eq!(s, parse_source_spec("-q missing.conf a.conf"));
    assert!(source_spec_from_args::<&str>(&[]).paths.is_empty());
    assert!(source_spec_from_args(&["-q"]).paths.is_empty(), "a flag is not a path");
}

#[test]
fn the_wire_form_reads_back_unchanged() {
    for spec in [
        SourceSpec { paths: vec!["a.conf".into()], ..Default::default() },
        SourceSpec {
            paths: vec![r"C:\My Dir\a.conf".into(), "-odd.conf".into(), r"C:\x\".into(), "it's.conf".into()],
            quiet: true,
            format: true,
            parse_only: false,
        },
        SourceSpec { paths: vec!["#{@dir}/x.conf".into(), "b".into()], format: true, ..Default::default() },
    ] {
        assert_eq!(parse_source_spec(&spec.wire()), spec, "{}", spec.wire());
    }
}

#[test]
fn an_unquoted_path_with_spaces_that_exists_is_still_one_path() {
    // Before paths were split this read the file, so a config that names its
    // file this way keeps working.
    let d = TempDir::new("legacy");
    let p = d.file("dir with space/s.conf", "set -g @sp on\n");
    assert_eq!(parse_source_spec(&p).paths, [p.clone()]);
    assert_eq!(parse_source_spec(&format!("-q {}", p)).paths, [p.clone()]);
    let mut app = app();
    source_file(&mut app, &p);
    assert_eq!(opt(&app, "@sp").as_deref(), Some("on"));
}

#[test]
fn unquoted_words_that_name_no_one_file_are_separate_paths() {
    let d = TempDir::new("split");
    let a = d.file("a.conf", "");
    let b = d.file("b.conf", "");
    assert_eq!(parse_source_spec(&format!("{} {}", a, b)).paths, [a.clone(), b.clone()]);
    // Quoted, the words are never joined back.
    let p = d.file("x y.conf", "");
    let s = parse_source_spec(&format!("\"{}\" {}", p, b));
    assert_eq!(s.paths, [p, b]);
}

// ── reading the paths ───────────────────────────────────────────────────────

#[test]
fn both_files_are_read() {
    let d = TempDir::new("both");
    let a = d.file("a.conf", "set -g @a on\n");
    let b = d.file("b.conf", "set -g @b on\n");
    let mut app = app();
    source_file(&mut app, &format!("{} {}", q(&a), q(&b)));
    assert_eq!(opt(&app, "@a").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@b").as_deref(), Some("on"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn the_files_are_read_in_the_order_given() {
    let d = TempDir::new("order");
    // Both append, so the value shows that both ran and in which order.
    let xa = d.file("xa.conf", "set -ga @x a\n");
    let xb = d.file("xb.conf", "set -ga @x b\n");
    let mut app = app();
    source_file(&mut app, &format!("{} {}", q(&xa), q(&xb)));
    assert_eq!(opt(&app, "@x").as_deref(), Some("ab"));
    app.user_options.remove("@x");
    source_file(&mut app, &format!("{} {}", q(&xb), q(&xa)));
    assert_eq!(opt(&app, "@x").as_deref(), Some("ba"));
}

#[test]
fn a_missing_path_is_reported_and_the_rest_are_still_read() {
    let d = TempDir::new("missing");
    let a = d.file("a.conf", "set -g @a on\n");
    let b = d.file("b.conf", "set -g @b on\n");
    let missing = d.path("missing.conf");
    let mut app = app();
    source_runtime(&mut app, &format!("{} {} {}", q(&a), q(&missing), q(&b)));
    assert_eq!(opt(&app, "@a").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@b").as_deref(), Some("on"));
    assert_eq!(app.config_warnings.len(), 1, "{:?}", app.config_warnings);
    assert!(app.config_warnings[0].contains("missing.conf"), "{}", app.config_warnings[0]);
}

#[test]
fn every_missing_path_is_reported() {
    let d = TempDir::new("missing2");
    let m1 = d.path("missing1.conf");
    let m2 = d.path("missing2.conf");
    let mut app = app();
    source_runtime(&mut app, &format!("{} {}", q(&m1), q(&m2)));
    assert_eq!(app.config_warnings.len(), 2, "{:?}", app.config_warnings);
    assert!(app.config_warnings[0].contains("missing1.conf"));
    assert!(app.config_warnings[1].contains("missing2.conf"));
}

#[test]
fn with_q_a_missing_path_is_skipped_without_a_word() {
    let d = TempDir::new("quiet");
    let a = d.file("a.conf", "set -g @a on\n");
    let b = d.file("b.conf", "set -g @b on\n");
    let missing = d.path("missing.conf");
    let mut app = app();
    source_runtime(&mut app, &format!("-q {} {} {}", q(&a), q(&missing), q(&b)));
    assert_eq!(opt(&app, "@a").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@b").as_deref(), Some("on"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn a_glob_and_a_plain_path_are_both_read() {
    let d = TempDir::new("glob");
    d.file("g/1.conf", "set -g @g1 on\n");
    d.file("g/2.conf", "set -g @g2 on\n");
    let b = d.file("b.conf", "set -g @b on\n");
    let pattern = d.path("g/*.conf");
    let mut app = app();
    source_file(&mut app, &format!("{} {}", q(&pattern), q(&b)));
    for name in ["@g1", "@g2", "@b"] {
        assert_eq!(opt(&app, name).as_deref(), Some("on"), "{}", name);
    }
}

#[test]
fn a_line_in_a_file_reads_every_path_on_it() {
    let d = TempDir::new("line");
    let a = d.file("a.conf", "set -g @a on\n");
    let s = d.file("dir with space/s.conf", "set -g @sp on\n");
    let outer = d.file("outer.conf", &format!("source-file {} \"{}\"\n", a, s));
    let mut app = app();
    source_file(&mut app, &outer);
    assert_eq!(opt(&app, "@a").as_deref(), Some("on"));
    assert_eq!(opt(&app, "@sp").as_deref(), Some("on"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
}

#[test]
fn a_runtime_source_keeps_each_files_commands_after_the_previous_files() {
    let d = TempDir::new("runtime");
    let nw = d.file("nw.conf", "new-window -n nw\n");
    let sp = d.file("sp.conf", "split-window\n");
    let mut app = AppState::new("sfp".to_string());
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
    let lines = source_runtime(&mut app, &format!("{} {}", q(&nw), q(&sp))).lines;
    let got: Vec<(String, crate::types::DeferredKind)> =
        lines.iter().map(|l| (l.file.clone(), l.kind.clone())).collect();
    assert_eq!(
        got,
        vec![
            (nw.clone(), crate::types::DeferredKind::Command("new-window -n nw".into())),
            (sp.clone(), crate::types::DeferredKind::Command("split-window".into())),
        ]
    );
}

#[test]
fn a_cycle_through_the_second_path_is_found_before_anything_runs() {
    let d = TempDir::new("cycle");
    let a = d.file("a.conf", "set -g @a on\n");
    let outer_path = d.path("outer.conf");
    std::fs::write(&outer_path, format!("set -g @outer on\nsource-file {} {}\n", q(&a), q(&outer_path))).unwrap();
    let mut app = app();
    let parsed = source_runtime(&mut app, &outer_path);
    assert_eq!(parsed.errors.len(), 1, "{:?}", parsed.errors);
    assert!(parsed.errors[0].contains("is already being sourced"), "{}", parsed.errors[0]);
    assert_eq!(opt(&app, "@outer"), None, "nothing in the file ran");
}

#[test]
fn a_reload_is_any_path_naming_the_startup_config() {
    let d = TempDir::new("startup");
    let start = d.file("startup.conf", "");
    let other = d.file("plugin.conf", "");
    let mut app = app();
    app.startup_config_file = Some(start.clone());
    assert!(is_startup_config(&app, &format!("{} {}", q(&other), q(&start))));
    assert!(is_startup_config(&app, &format!("-q {}", q(&start))), "-q is a flag, not part of the path");
    assert!(is_startup_config(&app, &format!("-q -- {}", q(&start))));
    assert!(!is_startup_config(&app, &q(&other)));
}
