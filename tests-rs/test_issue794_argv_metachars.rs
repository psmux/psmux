// #794: a `-- prog args` pane command runs as an argument vector, straight to
// the program, whatever its words hold.
//
// build_command sent any `--` argv holding `& | < > ; ` $ ( ) %` to
// `<shell> -Command` as one flattened string ("the respawn wire path loses
// quotes"), so a program under a directory named `a&b`, `a(b)` or with an
// argument `x&y` died at once, and one under `a$b` ran as a child of pwsh.
// tmux execvp's an argv whatever it holds (spawn.c, job.c with argc > 1). The
// wire form also decoded with a tokenizer that knows no escapes, so a word
// holding a `'` and a `"` or a `\"` came back changed.
//
// Measured with copies of a probe that records its own argv, under
// directories named plain, a&b, a(b), a$b, a`b, a b, a'b, a;b, a%b%, a^b, and
// fifteen arguments holding the same characters: on a68df0aa every
// new-window, split-window, respawn-pane and respawn-window pane was dead or
// stuck in pwsh; with this change every one is the program itself, argv exact.
//
// Registered from src/pane.rs.

use super::*;

/// Words that a shell, a quote stripping tokenizer or cmd.exe would change.
fn tricky_words() -> Vec<String> {
    [
        "/k", "x&y", "(p)", "$HOME", "`t", "sp ace", "q'uote", "dq\"x", "C:\\dir\\",
        "C:\\sp ace\\", "a\\\\\"b", "%PATH%", "c^d", "e|f", "g<h>i", "", ";", "\\;",
        "both'\"quotes", "tab\there", "line\nbreak", "日本語", "-NoProfile",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[test]
fn the_wire_form_gives_back_every_word() {
    let words = tricky_words();
    let tail = requote_argv(&words);
    assert_eq!(decode_argv(&tail), words, "{tail}");
    let form = encode_argv_form(&words);
    assert_eq!(decode_argv(form.strip_prefix("--").unwrap()), words);
}

#[test]
fn the_stored_start_command_respawns_the_same_words() {
    let mut argv = vec!["C:\\q\\a&b\\probe.exe".to_string()];
    argv.extend(tricky_words());
    let stored = start_command_raw_argv(&argv);
    assert_eq!(decode_argv(stored.strip_prefix("--").unwrap()), argv);
}

#[test]
fn the_start_command_displays_like_tmux() {
    // tmux cmd_stringify_argv: double quotes for " #';${}%" and tabs,
    // single quotes for a space or a `"` otherwise, else bare.
    let stored = encode_argv_form(&["C:\\a$b\\p.exe", "/k", "x&y", "$HOME", "sp ace", "dq\"x", "%PATH%"]);
    assert_eq!(
        start_command_display(&stored),
        "\"C:\\a$b\\p.exe\" /k x&y \"$HOME\" \"sp ace\" 'dq\"x' \"%PATH%\""
    );
}

#[cfg(windows)]
mod windows {
    use super::*;

    fn argv_of(cmd: &str) -> Vec<String> {
        build_command(Some(cmd), true, false)
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_metacharacter_in_the_program_path_does_not_bring_in_a_shell() {
        for dir in ["a&b", "a(b)", "a$b", "a`b", "a;b", "a%b%", "a^b", "a'b", "a b"] {
            let prog = format!("C:\\q794\\{dir}\\probe.exe");
            let argv = argv_of(&encode_argv_form(&[prog.as_str(), "/k"]));
            assert_eq!(argv, vec![prog.clone(), "/k".to_string()], "{dir}");
        }
    }

    #[test]
    fn a_metacharacter_in_an_argument_does_not_either() {
        let mut words = vec!["C:\\q794\\plain\\probe.exe".to_string()];
        words.extend(tricky_words());
        assert_eq!(argv_of(&encode_argv_form(&words)), words);
    }

    #[test]
    fn the_unix_launch_idioms_keep_their_translation() {
        // `env VAR=v prog` and `/bin/bash -c '...'` have no Windows program;
        // build_command's string route translates them (#399), as before.
        let argv = argv_of(&encode_argv_form(&["env", "A=1", "C:\\q\\p.exe", "x"]));
        assert_ne!(argv.first().map(String::as_str), Some("env"));
        let argv = argv_of(&encode_argv_form(&["/bin/bash", "-c", "export A=1; C:/q/p.exe"]));
        assert_ne!(argv.first().map(String::as_str), Some("/bin/bash"));
    }

    #[test]
    fn a_single_word_keeps_the_string_route() {
        // tmux: one word is a shell-command string. A path that exists is
        // still started directly by that route; this one does not exist.
        let argv = argv_of("-- definitely-not-a-program-794");
        assert_ne!(argv.first().map(String::as_str), Some("definitely-not-a-program-794"));
    }

    #[test]
    fn a_batch_script_runs_under_cmd_with_its_own_line() {
        let script = "C:\\q794\\a&b\\probe.cmd".to_string();
        let words: Vec<String> = vec![script.clone(), "x&y".into(), "sp ace".into(), "%PATH%".into()];
        let b = build_command(Some(&encode_argv_form(&words)), true, false);
        let argv: Vec<String> = b.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string());
        assert_eq!(argv, vec![comspec, "/d".into(), "/e:ON".into(), "/v:OFF".into(), "/c".into(), "%PSMUX_BATCH_LINE%".into()]);
        assert_eq!(
            b.get_env(BATCH_LINE_ENV).map(|v| v.to_string_lossy().into_owned()),
            Some("\"C:\\q794\\a&b\\probe.cmd\" \"x&y\" \"sp ace\" \"%PATH%\"".to_string())
        );
    }

    #[test]
    fn the_batch_line_follows_rusts_rules() {
        let line = batch_command_line(
            "C:\\s.cmd",
            &["plain".into(), "".into(), "dq\"x".into(), "C:\\d\\".into(), "a\\\"b".into(),
              "c^d".into(), "-x".into(), "a/b:c".into(), "line\nbreak".into()],
        );
        assert_eq!(
            line,
            "\"C:\\s.cmd\" plain \"\" \"dq\"\"x\" \"C:\\d\\\\\" \"a\\\\\"\"b\" \"c^d\" -x a/b:c \"line break\""
        );
    }
}

#[test]
fn display_popup_takes_an_argv_after_dashes() {
    let req = crate::popup::parse_display_popup_args(&["-E", "--", "C:\\a&b\\p.exe", "x&y", "sp ace"]).unwrap();
    assert_eq!(req.command, encode_argv_form(&["C:\\a&b\\p.exe", "x&y", "sp ace"]));
    // Without `--`, or with one word, the words stay a command string.
    let req = crate::popup::parse_display_popup_args(&["-E", "echo", "hi"]).unwrap();
    assert_eq!(req.command, "echo hi");
    let req = crate::popup::parse_display_popup_args(&["-E", "--", "htop"]).unwrap();
    assert_eq!(req.command, "htop");
}
