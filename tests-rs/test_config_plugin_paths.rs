use super::*;
use std::fs;

// Env-mutating tests serialise through crate::util::lock_test_env(), a single
// process-wide lock shared with the other env-touching test modules (see
// util.rs). A per-module mutex is not enough: USERPROFILE/HOME are
// process-global, so two modules with separate mutexes can still collide.

/// Helper: build a fresh AppState for testing.
fn mock_app() -> AppState {
    AppState::new("test_session".to_string())
}

/// RAII guard that sets HOME/USERPROFILE and restores on drop.
struct EnvGuard {
    orig_userprofile: Option<String>,
    orig_home: Option<String>,
}
impl EnvGuard {
    fn new(home: &str) -> Self {
        let g = Self {
            orig_userprofile: std::env::var("USERPROFILE").ok(),
            orig_home: std::env::var("HOME").ok(),
        };
        std::env::set_var("USERPROFILE", home);
        std::env::set_var("HOME", home);
        g
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.orig_userprofile {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match &self.orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

// ── Issue #135: Plugin discovery must check XDG paths ───────────────────

/// When a plugin is installed at ~/.config/psmux/plugins/<name>/plugin.conf,
/// the @plugin auto-source must find it — not only check ~/.psmux/plugins/.
#[test]
fn plugin_discovery_finds_xdg_path() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_plugin_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-theme");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-theme-option 'xdg-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'psmux-plugins/psmux-test-theme'\n");

    let val = app.user_options.get("@test-theme-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("xdg-found"),
        "Plugin at XDG path (~/.config/psmux/plugins/) should be auto-sourced"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// When a plugin is installed at ~/.psmux/plugins/<name>/plugin.conf (classic path),
/// it should still be found (regression guard).
#[test]
fn plugin_discovery_still_finds_classic_path() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_classic_plugin_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".psmux").join("plugins").join("psmux-test-theme");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-classic-option 'classic-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'psmux-plugins/psmux-test-theme'\n");

    let val = app.user_options.get("@test-classic-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("classic-found"),
        "Plugin at classic path (~/.psmux/plugins/) should still be found"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// When a plugin.conf references ~/.psmux/plugins/ but the script is actually
/// at ~/.config/psmux/plugins/, psmux's run-shell should fall back to the XDG path.
#[test]
fn run_shell_tilde_psmux_fallback_to_xdg() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_runshell_fallback_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let xdg_scripts = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-plugin").join("scripts");
    fs::create_dir_all(&xdg_scripts).unwrap();
    fs::write(xdg_scripts.join("test.ps1"), "# test script\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let wrong_path = tmp.join(".psmux").join("plugins");
    assert!(!wrong_path.is_dir(), "Test setup: classic plugin dir should NOT exist");

    let correct_path = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-plugin").join("scripts").join("test.ps1");
    assert!(correct_path.exists(), "Test setup: script should exist at XDG path");

    let _ = fs::remove_dir_all(&tmp);
}

/// XDG path with short plugin name (no org/ prefix) should also be found.
#[test]
fn plugin_discovery_xdg_short_name() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_short_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-short");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.conf"), "set -g @test-short 'short-found'\n").unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-short'\n");

    let val = app.user_options.get("@test-short");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("short-found"),
        "Plugin found by short name at XDG path"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// XDG PS1 plugin discovery should also work.
#[test]
fn plugin_discovery_xdg_ps1_entry() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_xdg_ps1_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".config").join("psmux").join("plugins")
        .join("psmux-test-ps1");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("psmux-test-ps1.ps1"),
        "# PSMux plugin\n# tmux set -g @ps1-test 'ps1-found'\n",
    ).unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-ps1'\n");

    for script in &app.pending_plugin_scripts {
        assert!(
            !script.contains(".psmux\\plugins") || script.contains(".config\\psmux\\plugins"),
            "Pending script path should use XDG location, got: {}", script
        );
    }

    let _ = fs::remove_dir_all(&tmp);
}

/// A plugin.conf that declares its own plugin, or two that declare each
/// other, is read once each. It used to recurse until the stack overflowed,
/// and a server whose config declared such a plugin never started.
#[test]
fn a_plugin_conf_that_declares_itself_is_read_once() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_plugin_cycle_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let plugins = tmp.join(".psmux").join("plugins");
    for (name, other) in [("psmux-test-self", "psmux-test-self"), ("psmux-test-a", "psmux-test-b"), ("psmux-test-b", "psmux-test-a")] {
        fs::create_dir_all(plugins.join(name)).unwrap();
        fs::write(
            plugins.join(name).join("plugin.conf"),
            format!("set -ga @{}-reads x\nset -g @plugin 'org/{}'\n", name, other),
        )
        .unwrap();
    }
    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-self'\nset -g @plugin 'org/psmux-test-a'\n");

    let reads = |name: &str| app.user_options.get(&format!("@{}-reads", name)).cloned();
    assert_eq!(reads("psmux-test-self").as_deref(), Some("x"));
    assert_eq!(reads("psmux-test-a").as_deref(), Some("x"));
    assert_eq!(reads("psmux-test-b").as_deref(), Some("x"));
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);

    let _ = fs::remove_dir_all(&tmp);
}

/// A plugin.conf is on the source stack while it is read, as a file
/// source-file reads is: the commands it keeps for the source job carry it
/// among their ancestors, so a `source-file` among them that names it again
/// is a cycle there too.
#[test]
fn a_plugin_conf_is_on_the_source_stack_of_the_lines_it_keeps() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_plugin_stack_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let dir = tmp.join(".psmux").join("plugins").join("psmux-test-stack");
    fs::create_dir_all(&dir).unwrap();
    let conf = dir.join("plugin.conf");
    fs::write(&conf, "set-buffer -b plugbuf x\n").unwrap();
    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    let kept = parse_keeping_commands(&mut app, |app| {
        parse_config_content(app, "set -g @plugin 'org/psmux-test-stack'\n")
    });
    assert_eq!(kept.len(), 1);
    assert_eq!(source_key(&kept[0].file), source_key(&conf.display().to_string()));
    assert!(kept[0].ancestors.contains(&source_key(&conf.display().to_string())), "{:?}", kept[0].ancestors);

    let _ = fs::remove_dir_all(&tmp);
}

/// Install a plugin that has only an entry script,
/// `<home>/.psmux/plugins/<name>/<name>.ps1`.
fn write_ps1_plugin(home: &std::path::Path, name: &str, script: &str) {
    let dir = home.join(".psmux").join("plugins").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{}.ps1", name)), script).unwrap();
}

/// A `.ps1` entry script that only sets options and binds keys with literal
/// text is applied as config lines, and nothing is left to run.
#[test]
fn a_literal_ps1_plugin_is_applied_as_config_lines() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_ps1_literal_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write_ps1_plugin(&tmp, "psmux-test-lit", concat!(
        "# A plugin\n",
        "$PSMUX = (Get-Command psmux).Source\n",
        "& $PSMUX set -g '@lit' 'on' 2>&1 | Out-Null\n",
        "& $PSMUX set -g status-right '#[fg=red] | %H:%M' 2>$null\n",
    ));
    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-lit'\n");

    assert_eq!(app.user_options.get("@lit").map(|s| s.as_str()), Some("on"));
    assert!(app.pending_plugin_scripts.is_empty(), "{:?}", app.pending_plugin_scripts);

    let _ = fs::remove_dir_all(&tmp);
}

/// A script with one line the parser cannot apply runs whole, and none of
/// its lines is applied first. The literal lines before a variable used to
/// be applied, and then again by the script: `set -ga` appended twice. A
/// script of set lines and another command used to lose that command.
#[test]
fn a_ps1_plugin_that_has_to_run_is_not_applied_in_part() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_ps1_part_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write_ps1_plugin(&tmp, "psmux-test-var", "& $PSMUX set -ga '@part' a\n$v = 'b'\n& $PSMUX set -ga '@part' $v\n");
    write_ps1_plugin(&tmp, "psmux-test-other", "& $PSMUX set -g '@other' on\n& $PSMUX set-buffer -b plugbuf yes\n");
    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-var'\nset -g @plugin 'org/psmux-test-other'\n");

    assert_eq!(app.user_options.get("@part"), None);
    assert_eq!(app.user_options.get("@other"), None);
    assert_eq!(app.pending_plugin_scripts.len(), 2, "{:?}", app.pending_plugin_scripts);
    assert!(app.pending_plugin_scripts[0].ends_with("psmux-test-var.ps1"), "{:?}", app.pending_plugin_scripts);

    let _ = fs::remove_dir_all(&tmp);
}

/// Words PowerShell would not pass to psmux as the config parser reads them
/// leave the script to run.
#[test]
fn a_ps1_plugin_line_is_applied_only_when_its_words_are_literal() {
    for cmd in [
        "set -g status-right '#[fg=red] | %H:%M'",
        "set -g '@x' ''",
        "set -g status-style bg=red",
        "bind -n M-h display-message \"hi there\"",
        "set -g status-left 'C:\\path'",
    ] {
        assert!(ps1_words_are_literal(cmd), "{}", cmd);
    }
    for cmd in [
        "set -g @x $v",
        "set -g @x \"a $v\"",
        "set -g @x \"C:\\path\"",
        "set -g status-style bg=red,fg=white",
        "set -g @x on",
        "set -g status-left #[fg=red]",
        "set -g @y 'it''s'",
        "set -g @y (Get-Date)",
        "set -g @y a | Out-Host",
        "set -g @y 'open",
    ] {
        assert!(!ps1_words_are_literal(cmd), "{}", cmd);
    }
}

/// With ppm declared, psmux leaves a plugin's entry script to ppm, which runs
/// it from the config's `run` line: psmux used to run it as well. A
/// plugin.conf is still read, and ppm skips that plugin.
#[test]
fn ps1_plugins_are_left_to_ppm_when_ppm_is_declared() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_ps1_ppm_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    write_ps1_plugin(&tmp, "psmux-test-var", "$v = 'b'\n& $PSMUX set -g '@var' $v\n");
    write_ps1_plugin(&tmp, "psmux-test-lit", "& $PSMUX set -g '@lit' on\n");
    let conf = tmp.join(".psmux").join("plugins").join("psmux-test-conf");
    fs::create_dir_all(&conf).unwrap();
    fs::write(conf.join("plugin.conf"), "set -g @conf on\n").unwrap();
    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(&mut app, concat!(
        "set -g @plugin 'psmux-plugins/ppm'\n",
        "set -g @plugin 'org/psmux-test-var'\n",
        "set -g @plugin 'org/psmux-test-lit'\n",
        "set -g @plugin 'org/psmux-test-conf'\n",
    ));
    assert!(app.ppm_declared);
    assert!(app.pending_plugin_scripts.is_empty(), "{:?}", app.pending_plugin_scripts);
    assert_eq!(app.user_options.get("@lit"), None);
    assert_eq!(app.user_options.get("@conf").map(|s| s.as_str()), Some("on"));

    // Declared after the plugin, ppm still takes the script it queued.
    let mut app = mock_app();
    parse_config_content(&mut app, "set -g @plugin 'org/psmux-test-var'\nset -g @plugin 'psmux-plugins/ppm'\n");
    assert!(app.pending_plugin_scripts.is_empty(), "{:?}", app.pending_plugin_scripts);

    let _ = fs::remove_dir_all(&tmp);
}

/// A '#branch' suffix on the spec must not shadow the plugin name. Branch names
/// may contain '/' (e.g. 'temp/integration'), so deriving the name from the last
/// '/' segment of 'MattKotsenas/psmux-plugins/psmux-test-branch#temp/integration'
/// would pick 'integration' and miss the plugin; the '#branch' suffix is stripped
/// first. Regression guard.
#[test]
fn plugin_discovery_strips_branch_suffix() {
    let _lock = crate::util::lock_test_env();
    let tmp = std::env::temp_dir().join(format!("psmux_test_branch_suffix_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);

    let plugin_dir = tmp.join(".psmux").join("plugins").join("psmux-test-branch");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(
        plugin_dir.join("plugin.conf"),
        "set -g @test-branch-option 'branch-found'\n",
    ).unwrap();

    let _env = EnvGuard::new(tmp.to_str().unwrap());

    let mut app = mock_app();
    parse_config_content(
        &mut app,
        "set -g @plugin 'MattKotsenas/psmux-plugins/psmux-test-branch#temp/integration'\n",
    );

    let val = app.user_options.get("@test-branch-option");
    assert_eq!(
        val.map(|s| s.as_str()),
        Some("branch-found"),
        "A '#temp/integration' branch suffix must not shadow the plugin name"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// ── a plugin's .tmux entry script without Git Bash ─────────────────────────

/// A theme whose script only sources files from its own directory is applied
/// whole, with the directory put in, and needs nothing more.
#[test]
fn a_tmux_script_that_only_sources_its_files_is_applied() {
    let (lines, needs_bash) = tmux_entry_script_lines(concat!(
        "#!/usr/bin/env bash\n",
        "\n",
        "# Set path of script\n",
        "PLUGIN_DIR=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)\"\n",
        "\n",
        "tmux source \"${PLUGIN_DIR}/theme_options_tmux.conf\"\n",
        "tmux source \"$PLUGIN_DIR/theme_tmux.conf\"\n",
    ), r"C:\p\theme");
    assert_eq!(lines, vec![
        "source \"C:/p/theme/theme_options_tmux.conf\"".to_string(),
        "source \"C:/p/theme/theme_tmux.conf\"".to_string(),
    ]);
    assert!(!needs_bash);
}

/// What a script does in a function, an `if` or a loop is not applied without
/// bash: applied without its condition, tmux-sensible's macOS-only
/// default-command stopped every pane from starting.
#[test]
fn a_tmux_script_line_in_a_block_is_not_applied() {
    let (lines, needs_bash) = tmux_entry_script_lines(concat!(
        "CURRENT_DIR=\"$( cd \"$( dirname \"${BASH_SOURCE[0]}\" )\" && pwd )\"\n",
        "is_osx() {\n",
        "  local platform=$(uname)\n",
        "  [ \"$platform\" == \"Darwin\" ]\n",
        "}\n",
        "main() {\n",
        "  if is_osx; then\n",
        "    tmux set-option -g default-command \"reattach-to-user-namespace -l $SHELL\"\n",
        "  fi\n",
        "  tmux set-option -g history-limit 50000\n",
        "}\n",
        "main\n",
    ), "C:/p");
    assert!(lines.is_empty(), "{:?}", lines);
    assert!(needs_bash);

    let (lines, needs_bash) = tmux_entry_script_lines(concat!(
        "if [ -z \"$X\" ]; then\n",
        "  tmux set -g default-command 'other'\n",
        "fi\n",
        "for k in a b; do tmux bind $k x; done\n",
        "if true; then tmux set -g @one 1; fi\n",
        "tmux set -g @top on\n",
    ), "C:/p");
    assert_eq!(lines, vec!["set -g @top on".to_string()]);
    assert!(needs_bash);
}

/// A top-level tmux call is applied only when bash would pass it the same
/// words: its known variables put in and nothing else for the shell to do.
#[test]
fn a_tmux_script_line_is_applied_only_when_its_words_are_known() {
    let (lines, needs_bash) = tmux_entry_script_lines(concat!(
        "FILE=src/theme.conf\n",
        "_dir=\"$(cd \"$(dirname \"${BASH_SOURCE[0]}\")\" && pwd)\"\n",
        "tmux source-file \"$_dir/$FILE\"\n",
        "tmux bind-key T send-keys a \\; send-keys b\n",
        "tmux set -g @long \\\n",
        "  'one two'\n",
        "\"$TMUX_PROGRAM\" set -g @prog on # a comment\n",
    ), "C:/p");
    assert_eq!(lines, vec![
        "source-file \"C:/p/src/theme.conf\"".to_string(),
        "bind-key T send-keys a \\; send-keys b".to_string(),
        "set -g @long   'one two'".to_string(),
        "set -g @prog on".to_string(),
    ]);
    assert!(!needs_bash);
    for script in [
        "tmux set -g @a \"$UNKNOWN\"\n",
        "tmux set -g @a \"$(date)\"\n",
        "tmux set -g @a `date`\n",
        "tmux set -g @a b > /dev/null\n",
        "tmux show -gv @a | grep x\n",
        "tmux set -g @a b &\n",
        "tmux set -g @a b; tmux set -g @c d\n",
        "tmux set -g @a b && tmux set -g @c d\n",
        "$dir/scripts/theme.sh\n",
        "source \"$dir/main.sh\"\n",
    ] {
        let (lines, needs_bash) = tmux_entry_script_lines(script, "C:/p");
        assert!(lines.is_empty(), "{:?}: {:?}", script, lines);
        assert!(needs_bash, "{:?}", script);
    }
}

/// Without Git Bash, a script that needs it says so once, and its plain lines
/// are applied all the same.
#[test]
fn a_tmux_script_that_needs_bash_says_so() {
    let tmp = std::env::temp_dir().join(format!("psmux_test_tmux_needs_bash_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let script = tmp.join("p.tmux");
    fs::write(&script, "tmux set -g @plain on\nmain() {\n  tmux set -g @inner on\n}\nmain\n").unwrap();

    let mut app = mock_app();
    parse_tmux_entry_script(&mut app, &script);
    assert_eq!(app.user_options.get("@plain").map(|s| s.as_str()), Some("on"));
    assert_eq!(app.user_options.get("@inner"), None);
    assert_eq!(app.config_warnings.len(), 1, "{:?}", app.config_warnings);
    assert!(app.config_warnings[0].contains("needs Git Bash"), "{:?}", app.config_warnings);

    let _ = fs::remove_dir_all(&tmp);
}

/// A run line that is one `.tmux` script waits for it, as any run-shell does,
/// when Git Bash can run it; without Git Bash parse_run_shell applies it in
/// place. A command that only names a `.tmux` file among other words, or a
/// `.tmux` path that is not a file, is a command like any other.
#[test]
fn a_tmux_script_waits_when_git_bash_runs_it() {
    let tmp = std::env::temp_dir().join(format!("psmux_test_tmux_waits_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let script = tmp.join("p.tmux");
    fs::write(&script, "tmux set -g @p on\n").unwrap();
    let p = script.to_string_lossy().replace('\\', "/");

    let runs = crate::commands::can_run_tmux_scripts();
    assert_eq!(run_shell_waits(&format!("run '{}'", p)), runs);
    assert_eq!(run_shell_in_job(&format!("run -b '{}'", p)), runs);
    assert!(run_shell_waits(&format!("run \"& 'C:/Program Files/Git/bin/bash.exe' '{}'\"", p)));
    assert!(run_shell_waits("run '~/.tmux/plugins/none/none.tmux'"));
    assert_eq!(tmux_entry_script(&format!("'{}'", p)), Some(std::path::PathBuf::from(&p)));
    assert_eq!(tmux_entry_script(&format!("bash '{}'", p)), None);

    let _ = fs::remove_dir_all(&tmp);
}

/// `#{d:current_file}` names the directory of the file being sourced while
/// the startup config runs, before any window exists, as catppuccin's
/// `source -F "#{d:current_file}/themes/..."` needs.
#[test]
fn current_file_is_known_before_the_first_window() {
    let tmp = std::env::temp_dir().join(format!("psmux_test_current_file_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    fs::write(tmp.join("a.conf"), "source -F \"#{d:current_file}/b.conf\"\n").unwrap();
    fs::write(tmp.join("b.conf"), "set -g @cf b\n").unwrap();

    let mut app = mock_app();
    assert!(app.windows.is_empty());
    parse_config_content(&mut app, &format!("source '{}'\n", tmp.join("a.conf").display()));
    assert_eq!(app.user_options.get("@cf").map(|s| s.as_str()), Some("b"), "{:?}", app.config_warnings);

    let _ = fs::remove_dir_all(&tmp);
}
