// Issue #795: argv normalizers rewrote literal VALUE data.
//
// `normalize_flag_equals` (#196) split every `-X=value` token and
// `normalize_attached_target_flag` (#571) split every `-t...` token, at any
// position, so `set @k -x=foo` stored `-x foo` at rc 0 and `set @k -tfoo` was
// refused with "empty value". tmux's args_parse stops at the first operand,
// so both passes now rewrite only the command's flag region. A lone `--` or
// `-t` value right after the option name is also stored verbatim.

use super::*;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// The full client chain from main.rs.
fn chain(args: &[&str]) -> Vec<String> {
    normalize_attached_target_flag(normalize_attached_global_args(normalize_flag_equals(argv(args))))
}

#[test]
fn set_option_values_survive_the_client_chain() {
    for v in ["-x=foo", "-u=1", "-tfoo", "-t=foo", "-t x", "--", "-target"] {
        assert_eq!(
            chain(&["psmux", "set-option", "-t", "s", "@k", v]),
            argv(&["psmux", "set-option", "-t", "s", "@k", v]),
            "value {v:?} was rewritten"
        );
    }
}

#[test]
fn values_after_dashdash_are_never_rewritten() {
    assert_eq!(
        chain(&["psmux", "set-option", "-t", "s", "--", "@k", "-x=foo"]),
        argv(&["psmux", "set-option", "-t", "s", "--", "@k", "-x=foo"])
    );
    assert_eq!(
        chain(&["psmux", "send-keys", "-t", "a", "--", "-t=x", "-tliteral"]),
        argv(&["psmux", "send-keys", "-t", "a", "--", "-t=x", "-tliteral"])
    );
}

#[test]
fn set_environment_and_aliases_keep_values() {
    for cmd in ["set-environment", "setenv"] {
        assert_eq!(
            chain(&["psmux", cmd, "-t", "s", "N", "-x=foo"]),
            argv(&["psmux", cmd, "-t", "s", "N", "-x=foo"])
        );
        assert_eq!(
            chain(&["psmux", cmd, "-t", "s", "N", "-tfoo"]),
            argv(&["psmux", cmd, "-t", "s", "N", "-tfoo"])
        );
    }
    assert_eq!(
        chain(&["psmux", "setw", "-t", "s", "@w", "-x=foo"]),
        argv(&["psmux", "setw", "-t", "s", "@w", "-x=foo"])
    );
}

#[test]
fn a_flag_value_is_not_rewritten_either() {
    // tmux consumes a required value whatever it looks like.
    assert_eq!(
        chain(&["psmux", "new-window", "-n", "-x=foo"]),
        argv(&["psmux", "new-window", "-n", "-x=foo"])
    );
    assert_eq!(
        chain(&["psmux", "new-window", "-n", "-tfoo", "-d"]),
        argv(&["psmux", "new-window", "-n", "-tfoo", "-d"])
    );
}

#[test]
fn real_flags_still_split_issue_196_and_571() {
    assert_eq!(
        chain(&["psmux", "has-session", "-t=mysession"]),
        argv(&["psmux", "has-session", "-t", "mysession"])
    );
    assert_eq!(
        chain(&["psmux", "kill-session", "-tvictimA"]),
        argv(&["psmux", "kill-session", "-t", "victimA"])
    );
    assert_eq!(
        chain(&["psmux", "set-option", "-t=s", "@k", "-t=v"]),
        argv(&["psmux", "set-option", "-t", "s", "@k", "-t=v"])
    );
    assert_eq!(
        chain(&["psmux", "set-option", "-ts", "@k", "-tv"]),
        argv(&["psmux", "set-option", "-t", "s", "@k", "-tv"])
    );
    assert_eq!(
        chain(&["psmux", "capture-pane", "-t=dev:0.1", "-S=0", "-E=100", "-p"]),
        argv(&["psmux", "capture-pane", "-t", "dev:0.1", "-S", "0", "-E", "100", "-p"])
    );
    // global options before the subcommand, with a detached -L value
    assert_eq!(
        chain(&["psmux", "-L", "ls", "has-session", "-t=x"]),
        argv(&["psmux", "-L", "ls", "has-session", "-t", "x"])
    );
    assert_eq!(
        chain(&["psmux", "-Lns", "kill-session", "-tx"]),
        argv(&["psmux", "-L", "ns", "kill-session", "-t", "x"])
    );
}

#[test]
fn boolean_letter_equals_is_left_whole() {
    // `-p=x` for display-message's boolean -p is not a flag spelling tmux
    // has; splitting it would turn `x` into a stray message word.
    assert_eq!(
        chain(&["psmux", "display-message", "-p=x"]),
        argv(&["psmux", "display-message", "-p=x"])
    );
}

#[test]
fn chained_commands_each_get_their_own_flag_region() {
    assert_eq!(
        chain(&["psmux", "set", "@k", "-x=foo", ";", "has-session", "-t=s"]),
        argv(&["psmux", "set", "@k", "-x=foo", ";", "has-session", "-t", "s"])
    );
    // PowerShell passes `\;` through unchanged
    assert_eq!(
        chain(&["psmux", "set", "@k", "-tv", "\\;", "kill-session", "-tx"]),
        argv(&["psmux", "set", "@k", "-tv", "\\;", "kill-session", "-t", "x"])
    );
}

#[test]
fn command_first_lines_from_the_server_routes() {
    // connection.rs passes a parsed line whose first token is the command.
    assert_eq!(
        normalize_flag_equals(argv(&["set-option", "-t", "s", "@k", "-x=foo"])),
        argv(&["set-option", "-t", "s", "@k", "-x=foo"])
    );
    assert_eq!(
        normalize_flag_equals(argv(&["has-session", "-t=s"])),
        argv(&["has-session", "-t", "s"])
    );
}

#[test]
fn set_option_parser_keeps_lone_dashdash_and_dash_t_values() {
    let p = parse_set_option_args(&["-t", "s", "@k", "--"]);
    assert_eq!(p.positionals, ["@k", "--"]);
    assert_eq!(p.target, Some("s"));
    assert!(p.validate(false).is_ok());

    let p = parse_set_option_args(&["@k", "-t"]);
    assert_eq!(p.positionals, ["@k", "-t"]);
    assert!(!p.missing_target);
    assert!(p.validate(false).is_ok());

    for v in ["-tfoo", "-t=foo", "-t x", "-target", "-x=foo"] {
        let p = parse_set_option_args(&["-t", "s", "@k", v]);
        assert_eq!(p.positionals, ["@k", v]);
        assert_eq!(p.target, Some("s"));
    }

    // An attached target in the flag region is the target, not more flags
    // (the server routes see `-ts` unsplit: `if-shell true 'set -ts @k v'`).
    let p = parse_set_option_args(&["-ts", "@k", "-tv"]);
    assert_eq!(p.target, Some("s"));
    assert_eq!(p.flag_chars, "t");
    assert_eq!(p.positionals, ["@k", "-tv"]);
    assert!(p.validate(false).is_ok());
    let p = parse_set_option_args(&["-gtS", "@k", "v"]);
    assert_eq!(p.target, Some("S"));
    assert_eq!(p.flag_chars, "gt");
    assert_eq!(p.positionals, ["@k", "v"]);

    // The psmux extensions on shapes tmux refuses still work.
    let p = parse_set_option_args(&["mouse", "on", "-t", "target"]);
    assert_eq!(p.target, Some("target"));
    assert_eq!(p.positionals, ["mouse", "on"]);
    let p = parse_set_option_args(&["@value", "--", "-t", "target"]);
    assert_eq!(p.target, None);
    assert_eq!(p.positionals, ["@value", "-t", "target"]);
    assert!(parse_set_option_args(&["mouse", "off", "-t"]).missing_target);
}
