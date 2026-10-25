// Issue #635 (contributor report by MattKotsenas): `kill-window -t` -- a `-t`
// with NO value after it -- silently killed the CURRENT window and exited 0.
//
// It is not specific to kill-window. Measured against master with a live
// 5-window session, 71 of 71 sampled command lines with a dangling
// value-taking flag were accepted at rc 0, and several were destructive:
// `kill-window -t` removed a window, `unlink-window -t` removed a window, and
// `kill-session -t` DESTROYED the whole session (the no-target fallback reads
// PSMUX_TARGET_SESSION, which every pane child inherits, so a script running
// `psmux kill-session -t $TARGET` with `$TARGET` unset wipes the session it
// is running inside).
//
// tmux rejects this GENERICALLY in arguments.c args_parse_flags:
//
//     xasprintf(cause, "-%c expects an argument", flag);
//     return (-1);
//
// and the command never runs. These tests pin the psmux port of that: one
// table of tmux `.args` templates plus one validator, exercised here directly.

use super::*;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn check(command: &str, args: &[&str]) -> Result<(), String> {
    validate_flag_arguments(command, args)
}

fn err(command: &str, args: &[&str]) -> String {
    check(command, args).expect_err(&format!("{} {:?} should have been rejected", command, args))
}

fn ok(command: &str, args: &[&str]) {
    if let Err(e) = check(command, args) {
        panic!("{} {:?} must be accepted, got {:?}", command, args, e);
    }
}

/// Letters declared `X:` (REQUIRED value) in a tmux `.args` template.
fn required_flags(template: &str) -> Vec<char> {
    let b = template.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphanumeric()
            && b.get(i + 1) == Some(&b':')
            && b.get(i + 2) != Some(&b':')
        {
            out.push(c);
        }
    }
    out
}

/// Letters declared `X::` (OPTIONAL value).
fn optional_flags(template: &str) -> Vec<char> {
    let b = template.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphanumeric()
            && b.get(i + 1) == Some(&b':')
            && b.get(i + 2) == Some(&b':')
        {
            out.push(c);
        }
    }
    out
}

/// Letters declared with no value at all.
fn boolean_flags(template: &str) -> Vec<char> {
    let b = template.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphanumeric() && b.get(i + 1) != Some(&b':') {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// the two highest-severity cases, called out by name
// ---------------------------------------------------------------------------

#[test]
fn kill_window_dangling_t_is_rejected() {
    assert_eq!(err("kill-window", &["-t"]), "-t expects an argument");
    assert_eq!(err("killw", &["-t"]), "-t expects an argument");
}

#[test]
fn kill_session_dangling_t_is_rejected() {
    // The worst one: the fallback target is the CURRENT session.
    assert_eq!(err("kill-session", &["-t"]), "-t expects an argument");
    assert_eq!(err("kill-ses", &["-t"]), "-t expects an argument");
}

#[test]
fn other_destructive_defaults_are_rejected() {
    for command in ["kill-pane", "killp", "unlink-window", "unlinkw",
                    "respawn-pane", "respawnp", "respawn-window", "respawnw",
                    "break-pane", "breakp", "detach-client", "detach"] {
        assert_eq!(err(command, &["-t"]), "-t expects an argument", "{}", command);
    }
    // respawn-* also take -c (start directory) and -e (environment).
    assert_eq!(err("respawn-pane", &["-c"]), "-c expects an argument");
    assert_eq!(err("respawn-window", &["-e"]), "-e expects an argument");
}

// ---------------------------------------------------------------------------
// exhaustive: EVERY value-taking flag of EVERY command in the table
// ---------------------------------------------------------------------------

#[test]
fn every_required_flag_of_every_command_is_rejected_when_dangling() {
    let mut checked = 0usize;
    for (name, alias, template) in ARGS_TEMPLATES {
        for flag in required_flags(template) {
            let arg = format!("-{}", flag);
            let expected = format!("-{} expects an argument", flag);
            assert_eq!(err(name, &[&arg]), expected, "command {}", name);
            if !alias.is_empty() {
                assert_eq!(err(alias, &[&arg]), expected, "alias {}", alias);
            }
            checked += 1;
        }
    }
    // Guards against the table silently emptying out.
    assert!(checked > 200, "only {} required flags checked", checked);
}

#[test]
fn every_required_flag_is_accepted_when_it_has_a_value() {
    for (name, _alias, template) in ARGS_TEMPLATES {
        for flag in required_flags(template) {
            ok(name, &[&format!("-{}", flag), "value"]);
        }
    }
}

#[test]
fn optional_value_flags_are_never_mandatory() {
    let mut seen = 0usize;
    for (name, _alias, template) in ARGS_TEMPLATES {
        for flag in optional_flags(template) {
            ok(name, &[&format!("-{}", flag)]);
            ok(name, &[&format!("-{}", flag), "5"]);
            seen += 1;
        }
    }
    // resize-pane -D/-L/-R/-U and move-pane's four are the `::` flags tmux has.
    assert!(seen >= 8, "expected the `::` flags to be present, saw {}", seen);
}

#[test]
fn boolean_flags_are_never_rejected() {
    for (name, _alias, template) in ARGS_TEMPLATES {
        for flag in boolean_flags(template) {
            ok(name, &[&format!("-{}", flag)]);
        }
    }
}

// ---------------------------------------------------------------------------
// the original 12-command probe, pinned
// ---------------------------------------------------------------------------

#[test]
fn the_reported_twelve_commands_all_reject_a_dangling_t() {
    for command in [
        "kill-window", "rename-window", "select-window", "split-window",
        "send-keys", "display-message", "list-panes", "select-pane",
        "swap-window", "resize-pane", "capture-pane", "kill-pane",
    ] {
        assert_eq!(err(command, &["-t"]), "-t expects an argument", "{}", command);
    }
}

#[test]
fn alias_forms_reject_a_dangling_t() {
    for alias in [
        "killw", "neww", "splitw", "lsp", "lsw",
        "selectp", "selectw", "renamew", "send", "display", "capturep",
        "resizep", "killp", "swapw", "movew", "linkw", "breakp", "joinp",
        "clearhist", "respawnp", "respawnw", "lastp", "last", "next", "prev",
    ] {
        assert_eq!(err(alias, &["-t"]), "-t expects an argument", "{}", alias);
    }
}

// ---------------------------------------------------------------------------
// tmux parity on the shape of the parse
// ---------------------------------------------------------------------------

#[test]
fn a_required_value_consumes_whatever_token_follows_including_a_flag() {
    // tmux's args_parse_flag_argument takes values[*i] as the value with no
    // dash test at all when the flag is NOT optional-argument, so
    // `kill-window -t -a` means target "-a" -- tmux then reports
    // "can't find window: -a". It is NOT a parse error.
    ok("kill-window", &["-t", "-a"]);
    ok("kill-session", &["-t", "-x"]);
    // A dash-leading value must survive: `-x -5` is a legitimate size.
    ok("resize-pane", &["-x", "-5"]);
    ok("resize-window", &["-y", "-10"]);
    ok("capture-pane", &["-S", "-100"]);
    // ...but a required flag that is the LAST token is still an error even
    // when an earlier flag looked similar.
    assert_eq!(err("kill-window", &["-a", "-t"]), "-t expects an argument");
}

#[test]
fn double_dash_ends_option_parsing() {
    // Everything after `--` is payload, so a `-t` there is a literal.
    ok("send-keys", &["-t", "sess", "--", "-t"]);
    ok("send-keys", &["-t", "sess", "--", "-foo"]);
    ok("new-window", &["--", "-t"]);
    ok("run-shell", &["--", "-d"]);
    // The flags BEFORE `--` are still checked.
    assert_eq!(err("send-keys", &["-t"]), "-t expects an argument");
}

#[test]
fn flag_parsing_stops_at_the_first_positional() {
    // tmux's args_parse_flags returns 1 on the first non-dash token, so a
    // trailing `-t` after a positional is an argument, not a flag.
    ok("send-keys", &["-t", "sess", "hello", "-t"]);
    ok("rename-window", &["newname", "-t"]);
    // A bare "-" is a positional too.
    ok("send-keys", &["-t", "sess", "-"]);
    ok("send-keys", &["-"]);
}

#[test]
fn an_attached_value_satisfies_the_flag() {
    ok("kill-window", &["-tmysession"]);
    ok("list-windows", &["-Fformat"]);
    ok("new-window", &["-nname"]);
    // Clustered: -a is boolean, -t takes the rest of the token.
    ok("kill-window", &["-atmysession"]);
}

#[test]
fn clustered_short_flags_are_walked_letter_by_letter() {
    // The value-taking letter is LAST in the cluster and has no value.
    assert_eq!(err("kill-window", &["-at"]), "-t expects an argument");
    assert_eq!(err("list-panes", &["-aF"]), "-F expects an argument");
    assert_eq!(err("capture-pane", &["-pS"]), "-S expects an argument");
    // Same cluster, value supplied.
    ok("kill-window", &["-at", "sess"]);
    ok("list-panes", &["-aF", "#{pane_id}"]);
    // A cluster of booleans only is fine.
    ok("list-panes", &["-as"]);
    ok("capture-pane", &["-pJ"]);
}

#[test]
fn an_optional_value_flag_does_not_swallow_a_following_flag() {
    // tmux: `as[0] == '-' && (as[1] == '-' || isalpha(as[1]))` means "no value".
    // So `-D` keeps no value and `-t sess` is still parsed as a flag pair --
    // which is why the dangling `-t` after it must still be caught.
    ok("resize-pane", &["-D", "-t", "sess"]);
    assert_eq!(err("resize-pane", &["-D", "-t"]), "-t expects an argument");
    // A NUMBER after an optional flag IS its value, so the -t here is dangling.
    assert_eq!(err("resize-pane", &["-D", "5", "-t"]), "-t expects an argument");
}

#[test]
fn optional_value_flag_consumes_a_numeric_value() {
    ok("resize-pane", &["-D", "5"]);
    ok("resize-pane", &["-U", "10", "-t", "sess"]);
    ok("move-pane", &["-L", "-t", "sess"]);
}

// ---------------------------------------------------------------------------
// the validator must never invent new rejections
// ---------------------------------------------------------------------------

#[test]
fn unknown_commands_are_left_alone() {
    ok("not-a-command", &["-t"]);
    ok("", &["-t"]);
    ok("_render-preview", &["-t"]);
}

#[test]
fn flags_a_command_does_not_declare_are_left_alone() {
    // psmux carries flags tmux has no template for. They are treated as
    // booleans so the check can never reject something psmux accepts today.
    ok("kill-window", &["-Q"]);
    ok("list-panes", &["-9"]);
}

#[test]
fn empty_and_flagless_command_lines_are_accepted() {
    ok("kill-window", &[]);
    ok("list-sessions", &[]);
    ok("kill-window", &["-a"]);
    ok("new-window", &["-d"]);
}

#[test]
fn realistic_command_lines_still_pass() {
    ok("new-window", &["-t", "dev", "-n", "editor", "-c", "C:\\src"]);
    ok("split-window", &["-h", "-p", "30", "-t", "dev:1"]);
    ok("send-keys", &["-t", "dev:1.0", "echo hi", "Enter"]);
    ok("capture-pane", &["-p", "-S", "-1000", "-t", "dev:0.0"]);
    ok("bind-key", &["-T", "copy-mode", "C-c", "send-keys", "-X", "cancel"]);
    ok("set-option", &["-g", "-t", "dev", "status", "on"]);
    ok("display-message", &["-p", "-t", "dev", "#{session_name}"]);
    ok("list-windows", &["-a", "-F", "#{window_id}"]);
    ok("resize-pane", &["-t", "dev:0.0", "-x", "80", "-y", "24"]);
    ok("swap-window", &["-s", "1", "-t", "2"]);
    ok("join-pane", &["-s", "dev:1.0", "-t", "dev:0.0", "-h"]);
    ok("if-shell", &["-b", "-F", "#{==:1,1}", "display-message ok"]);
    ok("refresh-client", &["-C", "80,24"]);
    ok("wait-for", &["-S", "channel"]);
}

// ---------------------------------------------------------------------------
// the whole-line wrapper used by the config-file and key-binding routes
// ---------------------------------------------------------------------------

#[test]
fn command_line_wrapper_takes_the_command_from_token_zero() {
    let bad: Vec<String> = vec!["kill-window".into(), "-t".into()];
    assert_eq!(
        validate_command_line_flags(&bad).unwrap_err(),
        "-t expects an argument"
    );
    let good: Vec<String> = vec!["kill-window".into(), "-t".into(), "sess".into()];
    assert!(validate_command_line_flags(&good).is_ok());
    let empty: Vec<String> = Vec::new();
    assert!(validate_command_line_flags(&empty).is_ok());
}

// ---------------------------------------------------------------------------
// the table itself
// ---------------------------------------------------------------------------

#[test]
fn templates_resolve_through_names_aliases_and_psmux_extras() {
    assert_eq!(args_template("kill-window"), Some("af:t:"));
    assert_eq!(args_template("killw"), Some("af:t:"));
    assert_eq!(args_template("kill-session"), Some("aCgf:t:"));
    // psmux-only aliases share the canonical command's template.
    assert_eq!(args_template("kill-ses"), args_template("kill-session"));
    // tmux's default command aliases are no commands: the CLI leaves them to
    // the server's alias table, and the command they run is checked there.
    for alias in ["split-pane", "splitp", "info", "server-info", "choose-window", "choose-session"] {
        assert_eq!(args_template(alias), None, "{}", alias);
    }
    assert_eq!(args_template("nope"), None);
}

#[test]
fn flag_value_kind_reads_the_tmux_template_grammar() {
    assert_eq!(flag_value_kind("af:t:", 'a'), FlagValue::None);
    assert_eq!(flag_value_kind("af:t:", 'f'), FlagValue::Required);
    assert_eq!(flag_value_kind("af:t:", 't'), FlagValue::Required);
    assert_eq!(flag_value_kind("af:t:", 'z'), FlagValue::None);
    assert_eq!(flag_value_kind("D::L::MR::Tt:U::x:y:Z", 'D'), FlagValue::Optional);
    assert_eq!(flag_value_kind("D::L::MR::Tt:U::x:y:Z", 'M'), FlagValue::None);
    assert_eq!(flag_value_kind("D::L::MR::Tt:U::x:y:Z", 't'), FlagValue::Required);
    assert_eq!(flag_value_kind("D::L::MR::Tt:U::x:y:Z", 'x'), FlagValue::Required);
    // ':' is punctuation, never a flag letter.
    assert_eq!(flag_value_kind("af:t:", ':'), FlagValue::None);
    assert_eq!(flag_value_kind("af:t:", '-'), FlagValue::None);
}

#[test]
fn psmux_only_aliases_resolve_to_their_canonical_command() {
    // docs/tmux_args_reference.md is the list of names psmux answers to.
    for (alias, canonical) in [
        ("a", "attach-session"),
        ("at", "attach-session"),
        ("resp", "respawn-pane"),
        ("send-key", "send-keys"),
        ("show-option", "show-options"),
        ("show-window-option", "show-window-options"),
        ("warmup", "start-server"),
        ("kill-ses", "kill-session"),
    ] {
        assert_eq!(args_template(alias), args_template(canonical), "{}", alias);
    }
    assert_eq!(err("a", &["-t"]), "-t expects an argument");
    assert_eq!(err("resp", &["-c"]), "-c expects an argument");
    assert_eq!(err("send-key", &["-N"]), "-N expects an argument");
    assert_eq!(err("show-option", &["-t"]), "-t expects an argument");
}

#[test]
fn psmux_only_value_flags_are_covered_too() {
    // Flags psmux parses that tmux's own template does not declare. Listed
    // separately from ARGS_TEMPLATES so the tmux transcription stays verbatim.
    assert_eq!(err("new-window", &["-T"]), "-T expects an argument");
    assert_eq!(err("neww", &["-T"]), "-T expects an argument");
    assert_eq!(err("unbind-key", &["-t"]), "-t expects an argument");
    assert_eq!(err("unbind", &["-t"]), "-t expects an argument");
    assert_eq!(err("list-buffers", &["-t"]), "-t expects an argument");
    assert_eq!(err("list-keys", &["-t"]), "-t expects an argument");
    // ...and they still take a value happily.
    ok("new-window", &["-T", "title", "-t", "sess"]);
    ok("unbind-key", &["-t", "sess", "-T", "root", "F1"]);
    ok("list-keys", &["-t", "sess"]);
    // psmux-only COMMANDS carry templates too.
    assert_eq!(err("send-paste", &["-t"]), "-t expects an argument");
    assert_eq!(err("new-pane", &["-t"]), "-t expects an argument");
    assert_eq!(err("newp", &["-c"]), "-c expects an argument");
    assert_eq!(err("choose-tree", &["-F"]), "-F expects an argument");
}

#[test]
fn the_table_has_no_duplicate_command_names() {
    let mut names: Vec<&str> = Vec::new();
    for (name, alias, _) in ARGS_TEMPLATES {
        assert!(!names.contains(name), "duplicate command {}", name);
        names.push(name);
        if !alias.is_empty() {
            assert!(!names.contains(alias), "duplicate alias {}", alias);
            names.push(alias);
        }
    }
    for (alias, target) in EXTRA_ALIASES {
        assert!(!names.contains(alias), "extra alias {} shadows a real name", alias);
        assert!(
            args_template(target).is_some(),
            "extra alias {} points at unknown command {}",
            alias,
            target
        );
    }
}

#[test]
fn every_command_psmux_advertises_has_a_template() {
    // The user-facing catalog is the contract: anything listed there must be
    // covered, or its dangling flags stay silent.
    let mut missing: Vec<String> = Vec::new();
    for entry in crate::server::helpers::TMUX_COMMANDS {
        let name = match entry.split_once(" (") {
            Some((c, _)) => c,
            None => entry,
        };
        if args_template(name).is_none() {
            missing.push(name.to_string());
        }
    }
    assert!(missing.is_empty(), "commands with no flag template: {:?}", missing);
}
