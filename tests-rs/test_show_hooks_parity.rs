// show-hooks prints what tmux 3.8 prints (cmd-show-options.c), checked
// against tmux 3.4 under WSL for the cases both versions share:
//   show-hooks             nothing (no hook set on the session)
//   show-hooks -g          every session hook, in table order, bare names
//                          for empty ones, `name[i] command` for set ones
//   show-hooks -gw         the window and pane hooks
//   show-hooks -w / -p     only hooks set there
//   show-hooks -g NAME     that hook, or its bare name when empty
// psmux printed "(no hooks)", ignored every flag and the name, and used
// `name -> command` rows in HashMap order.

use super::*;
use std::collections::HashMap;

fn hooks(list: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
    list.iter()
        .map(|(n, c)| (n.to_string(), c.iter().map(|s| s.to_string()).collect()))
        .collect()
}

fn show(h: &HashMap<String, Vec<String>>, args: &[&str]) -> String {
    format_show_hooks(h, &ShowHooksArgs::parse(args))
}

#[test]
fn no_hooks_prints_nothing_without_g() {
    let h = hooks(&[]);
    assert_eq!(show(&h, &[]), "");
    assert_eq!(show(&h, &["-w"]), "");
    assert_eq!(show(&h, &["-p"]), "");
    assert_eq!(show(&h, &["-gp"]), "", "-p ignores -g, as options_scope_from_flags does");
    assert!(!show(&h, &["-g"]).contains("no hooks"));
}

#[test]
fn g_lists_every_session_hook_in_table_order() {
    let h = hooks(&[]);
    let out = show(&h, &["-g"]);
    let lines: Vec<&str> = out.lines().collect();
    let session: Vec<&str> = HOOK_TABLE.iter().filter(|(_, c)| *c == HookClass::Session).map(|(n, _)| *n).collect();
    assert_eq!(lines, session);
    assert_eq!(lines.first(), Some(&"after-bind-key"), "tmux 3.4 starts with after-bind-key too");
    assert_eq!(lines.last(), Some(&"window-unlinked"));
    assert!(!lines.contains(&"pane-exited"), "pane hooks are window options in tmux");
    assert!(out.ends_with('\n') && !out.ends_with("\n\n"));
}

#[test]
fn gw_lists_window_and_pane_hooks() {
    let out = show(&hooks(&[]), &["-gw"]);
    assert!(out.lines().any(|l| l == "pane-exited"));
    assert!(out.lines().any(|l| l == "window-renamed"));
    assert!(!out.lines().any(|l| l == "after-new-window"));
    assert_eq!(out.lines().count(), 27);
}

#[test]
fn set_hooks_print_indexed_rows_in_place() {
    let h = hooks(&[
        ("after-new-window", &["display-message hi"]),
        ("pane-exited", &["display hi", "display bye"]),
    ]);
    let g = show(&h, &["-g"]);
    assert!(g.lines().any(|l| l == "after-new-window[0] display-message hi"));
    assert!(!g.lines().any(|l| l == "after-new-window"));
    // Table order: after-list-windows < after-new-window < after-paste-buffer.
    let pos = |s: &str| g.lines().position(|l| l.starts_with(s)).unwrap();
    assert!(pos("after-list-windows") < pos("after-new-window[0]"));
    assert!(pos("after-new-window[0]") < pos("after-paste-buffer"));
    let gw = show(&h, &["-gw"]);
    assert!(gw.contains("pane-exited[0] display hi\npane-exited[1] display bye\n"));
    // Without -g only what is set, nothing for the empty ones.
    assert_eq!(show(&h, &[]), "after-new-window[0] display-message hi\n");
    assert_eq!(show(&h, &["-w"]), "pane-exited[0] display hi\npane-exited[1] display bye\n");
    assert_eq!(show(&h, &["-p"]), "pane-exited[0] display hi\npane-exited[1] display bye\n");
}

#[test]
fn a_named_hook() {
    let h = hooks(&[("after-new-window", &["display-message hi"]), ("pane-exited", &["a", "b"])]);
    assert_eq!(show(&h, &["-g", "after-new-window"]), "after-new-window[0] display-message hi\n");
    assert_eq!(show(&h, &["after-new-window"]), "after-new-window[0] display-message hi\n");
    // An empty hook exists only in the global options (tmux: bare name with -g).
    assert_eq!(show(&h, &["-g", "session-created"]), "session-created\n");
    assert_eq!(show(&h, &["session-created"]), "");
    assert_eq!(show(&h, &["-g", "pane-exited[1]"]), "pane-exited[1] b\n");
    assert_eq!(show(&h, &["-g", "pane-exited[5]"]), "");
    assert_eq!(show(&h, &["-t", "a:0", "-g", "--", "pane-exited"]), "pane-exited[0] a\npane-exited[1] b\n");
}

#[test]
fn psmux_only_hooks_are_listed_after_the_table() {
    let h = hooks(&[("before-select-window", &["x"])]);
    let g = show(&h, &["-g"]);
    assert!(g.ends_with("window-unlinked\nbefore-select-window[0] x\n"));
    assert_eq!(show(&h, &[]), "before-select-window[0] x\n");
}

#[test]
fn table_matches_tmux_counts() {
    assert_eq!(HOOK_TABLE.len(), 88);
    assert_eq!(HOOK_TABLE.iter().filter(|(_, c)| *c == HookClass::Pane).count(), 19);
    assert_eq!(HOOK_TABLE.iter().filter(|(_, c)| *c == HookClass::Window).count(), 8);
    // Every table hook is a name set-hook accepts.
    for (n, _) in HOOK_TABLE {
        assert!(crate::server::option_catalog::is_valid_option_name(n), "{n}");
    }
}
