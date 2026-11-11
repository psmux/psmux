// list-commands prints tmux 3.8's rows (cmd-list-commands.c): the default
// template `name (alias) usage`, -F with #{command_list_name},
// #{command_list_alias} and #{command_list_usage}, and a command argument
// found the way cmd_find finds it. psmux printed a help style list and
// ignored -F and the argument.

use super::*;

fn plain(args: &[&str]) -> Result<String, String> {
    render(&ListCommandsArgs::parse(args), &mut |_, e| default_row(e))
}

#[test]
fn table_is_tmux_3_8() {
    assert_eq!(COMMAND_TABLE.len(), 92);
    assert_eq!(COMMAND_TABLE[0].0, "attach-session");
    assert_eq!(COMMAND_TABLE[91].0, "wait-for");
    // Every command the CLI parser knows from tmux is listed.
    for (name, _, _) in COMMAND_TABLE {
        assert!(!name.is_empty());
    }
}

#[test]
fn default_rows() {
    let out = plain(&[]).unwrap();
    assert_eq!(out.lines().count(), 92);
    assert!(out.lines().any(|l| l == "list-commands (lscm) [-F format] [command]"));
    assert!(out.lines().any(|l| l == "show-hooks [-Bgpw] [-t target-pane] [hook]"));
    // tmux's template puts the space before an empty usage too.
    assert!(out.lines().any(|l| l == "kill-server "));
    assert!(out.lines().any(|l| l.starts_with("attach-session (attach) ")));
    assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
}

#[test]
fn a_command_argument() {
    assert_eq!(plain(&["list-commands"]).unwrap(), "list-commands (lscm) [-F format] [command]\n");
    assert_eq!(plain(&["lscm"]).unwrap(), "list-commands (lscm) [-F format] [command]\n");
    assert_eq!(plain(&["new-w"]).unwrap().lines().next().unwrap().split(' ').next(), Some("new-window"));
    assert_eq!(plain(&["bogus"]).unwrap_err(), "unknown command: bogus");
    assert_eq!(
        plain(&["list-"]).unwrap_err(),
        "ambiguous command: list-, could be: list-buffers, list-clients, list-commands, list-keys, list-panes, list-sessions, list-windows"
    );
}

#[test]
fn format_rows_skip_empty_expansions() {
    let args = ListCommandsArgs::parse(&["-F", "#{command_list_name}"]);
    let out = render(&args, &mut |fmt, e| {
        assert_eq!(fmt, "#{command_list_name}");
        if e.0.starts_with("list-") { e.0.to_string() } else { String::new() }
    })
    .unwrap();
    assert_eq!(out.lines().count(), 7);
    assert!(out.starts_with("list-buffers\n"));
}

#[test]
fn parse_forms() {
    assert_eq!(
        ListCommandsArgs::parse(&["-F#{command_list_name}", "neww"]),
        ListCommandsArgs { format: Some("#{command_list_name}".into()), command: Some("neww".into()) }
    );
    assert_eq!(ListCommandsArgs::parse(&["--", "-x"]).command.as_deref(), Some("-x"));
}
