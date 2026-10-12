// A one-shot CLI command list (`psmux a \; b`) runs every command, in order.
// The CLI dispatched on the first command's name only, and most of its arms
// rebuild the request from the flags they know, so `select-pane -t p \;
// set-option -t p @c x` dropped the set-option, and `set-option ... \;
// select-pane -d` read `-d` as a set-option flag. split_cli_command_list
// decides what runs first and what is queued after it.

use super::*;

fn split(argv: &[&str]) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    split_cli_command_list(argv)
}

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn select_pane_keeps_the_commands_after_it() {
    assert_eq!(
        split(&["select-pane", "-t", "p", ";", "set-option", "-t", "p", "@c", "after"]),
        Some((
            v(&["select-pane", "-t", "p"]),
            vec![v(&["set-option", "-t", "p", "@c", "after"])],
        ))
    );
}

#[test]
fn a_lone_backslash_semicolon_separates_too() {
    // What a Windows shell passes for `\;`, and what the server's own
    // splitter reads as a separator.
    assert_eq!(
        split(&["select-pane", "-T", "ttl", "-t", "p", "\\;", "set-option", "@b", "x"]),
        Some((
            v(&["select-pane", "-T", "ttl", "-t", "p"]),
            vec![v(&["set-option", "@b", "x"])],
        ))
    );
}

#[test]
fn the_first_command_does_not_see_the_next_commands_flags() {
    assert_eq!(
        split(&["set-option", "-t", "p", "@d", "first", ";", "select-pane", "-d", "-t", "p"]),
        Some((
            v(&["set-option", "-t", "p", "@d", "first"]),
            vec![v(&["select-pane", "-d", "-t", "p"])],
        ))
    );
}

#[test]
fn every_queued_command_keeps_its_order() {
    let (first, queued) = split(&[
        "set-buffer", "-b", "lb", "echo X", ";", "select-pane", "-t", "p", ";",
        "paste-buffer", "-b", "lb", "-t", "p",
    ])
    .expect("a command list");
    assert_eq!(first, v(&["set-buffer", "-b", "lb", "echo X"]));
    assert_eq!(
        queued,
        vec![
            v(&["select-pane", "-t", "p"]),
            v(&["paste-buffer", "-b", "lb", "-t", "p"]),
        ]
    );
}

#[test]
fn a_single_command_is_dispatched_whole() {
    assert_eq!(split(&["select-pane", "-t", "p"]), None);
    assert_eq!(split(&["select-pane", "-t", "p", ";"]), None, "nothing queued after a trailing separator");
    assert_eq!(split(&[]), None);
}

#[test]
fn a_semicolon_inside_an_argument_is_data() {
    // Only a whole `;` / `\;` argument separates; a value that contains or
    // ends with one is the value (#499).
    assert_eq!(split(&["set-option", "@k", "a ; b"]), None);
    assert_eq!(split(&["set-option", "@k", "x;"]), None);
    assert_eq!(split(&["send-keys", "-t", "p", "a\\;"]), None);
}

#[test]
fn empty_commands_between_separators_are_dropped() {
    assert_eq!(
        split(&["select-pane", ";", ";", "set-option", "@x", "1"]),
        Some((v(&["select-pane"]), vec![v(&["set-option", "@x", "1"])]))
    );
}

#[test]
fn a_line_that_starts_with_a_separator_is_left_alone() {
    assert_eq!(split(&[";", "set-option", "@x", "1"]), None);
}

#[test]
fn commands_that_carry_a_command_list_keep_their_whole_argv() {
    for argv in [
        &["bind-key", "x", "split-window", "\\;", "select-pane", "-D"][..],
        &["bind", "x", "split-window", ";", "select-pane", "-D"][..],
        &["set-hook", "-g", "after-new-window", "display", "a", "\\;", "display", "b"][..],
        &["confirm-before", "kill-window", "\\;", "display", "b"][..],
        &["start-server", ";", "set", "-g", "exit-empty", "off"][..],
        &["attach", "-t", "p", ";", "display", "b"][..],
    ] {
        assert_eq!(split(argv), None, "{:?}", argv);
    }
}

#[test]
fn new_session_run_shell_and_if_shell_are_split() {
    // Each runs through its own arm first; the rest of the list follows.
    assert_eq!(
        split(&["new-session", "-d", "-s", "q", ";", "new-window", "-t", "q"]),
        Some((v(&["new-session", "-d", "-s", "q"]), vec![v(&["new-window", "-t", "q"])]))
    );
    assert_eq!(
        split(&["if-shell", "true", "display a", "\\;", "set", "@b", "1"]),
        Some((v(&["if-shell", "true", "display a"]), vec![v(&["set", "@b", "1"])]))
    );
    assert_eq!(
        split(&["run-shell", "echo a", ";", "display", "b"]),
        Some((v(&["run-shell", "echo a"]), vec![v(&["display", "b"])]))
    );
}

// Which commands share the routed session's connection.

#[test]
fn target_values_are_read_spelled_apart_attached_and_clustered() {
    assert_eq!(command_list_target_values(&v(&["set", "-t", "x", "@a", "1"])), v(&["x"]));
    assert_eq!(command_list_target_values(&v(&["display", "-tx:1", "-p", "hi"])), v(&["x:1"]));
    assert_eq!(command_list_target_values(&v(&["join-pane", "-dh", "-s", "y:1", "-t", "x"])), v(&["y:1", "x"]));
    assert_eq!(command_list_target_values(&v(&["neww", "-dt", "y"])), v(&["y"]));
    assert!(command_list_target_values(&v(&["send-keys", "--", "-t", "y"])).is_empty());
}

#[test]
fn a_command_on_the_routed_session_shares_its_connection() {
    for argv in [
        &["set-option", "@a", "1"][..],
        &["set-option", "-t", "x", "@a", "1"][..],
        &["set-option", "-t", "=x", "@a", "1"][..],
        &["display-message", "-t", "x:1.0", "-p", "hi"][..],
        &["select-window", "-t", ":2"][..],
        &["select-pane", "-t", ".1"][..],
        &["list-windows"][..],
    ] {
        assert!(command_list_shares_session(&v(argv), None, "x"), "{:?}", argv);
    }
    // The routed base carries the -L namespace; a target names the short name.
    assert!(command_list_shares_session(&v(&["set", "-t", "x", "@a", "1"]), Some("ns"), "ns__x"));
}

#[test]
fn a_command_that_leaves_the_routed_session_is_routed_alone() {
    for argv in [
        // Another session: on this connection it acted on x (set, send-keys,
        // new-window all landed in the wrong session).
        &["set-option", "-t", "y", "@a", "1"][..],
        &["send-keys", "-t", "y:0", "echo", "Enter"][..],
        &["new-window", "-dt", "y"][..],
        // Ids whose owner only the CLI resolves (#627).
        &["select-pane", "-t", "%3"][..],
        &["select-window", "-t", "@2"][..],
        &["display", "-t", "$1", "-p", "x"][..],
        // Client side or namespace wide work.
        &["ls"][..],
        &["list-sessions"][..],
        &["kill-server"][..],
        &["has-session", "-t", "x"][..],
        &["kill-session", "-t", "x"][..],
        &["run-shell", "echo hi"][..],
        &["if-shell", "true", "display a"][..],
        &["switch-client", "-t", "x"][..],
        // Not in the tmux catalog: its CLI arm decides whether it exists.
        &["bogus-command"][..],
    ] {
        assert!(!command_list_shares_session(&v(argv), None, "x"), "{:?}", argv);
    }
}

// ─── the line the list goes to the server as ────────────────────────────────

fn wire(argv: &[&str]) -> String {
    let (first, queued) = split(argv).expect("a command list");
    command_list_wire_line(&first, &queued)
}

#[test]
fn the_wire_line_joins_the_commands_with_a_separator() {
    assert_eq!(
        wire(&["select-pane", "-t", "p", "\\;", "set-option", "-t", "p", "@c", "after"]),
        "select-pane -t p ; set-option -t p @c after"
    );
}

#[test]
fn the_server_splits_the_wire_line_back_into_the_same_commands() {
    let argv = [
        "set-buffer", "-b", "lb", "echo X", ";", "select-pane", "-e", "-t", "%1", ";",
        "paste-buffer", "-p", "-b", "lb", "-t", "%1", ";", "select-pane", "-d", "-t", "%1",
    ];
    let (first, queued) = split(&argv).expect("a command list");
    let line = command_list_wire_line(&first, &queued);
    let back: Vec<Vec<String>> = crate::config::split_chained_commands_pub(&line)
        .iter()
        .map(|c| crate::commands::parse_command_line(c))
        .collect();
    let mut want = vec![first];
    want.extend(queued);
    assert_eq!(back, want);
}

#[test]
fn a_value_with_a_semicolon_stays_one_argument_on_the_wire() {
    let line = wire(&["set-option", "@k", "a ; b", ";", "set-option", "@j", "x;y", ";", "display", ""]);
    let back: Vec<Vec<String>> = crate::config::split_chained_commands_pub(&line)
        .iter()
        .map(|c| crate::commands::parse_command_line(c))
        .collect();
    assert_eq!(
        back,
        vec![v(&["set-option", "@k", "a ; b"]), v(&["set-option", "@j", "x;y"]), v(&["display", ""])]
    );
}

// ─── the reply ──────────────────────────────────────────────────────────────

#[test]
fn the_reply_keeps_output_and_collects_errors() {
    assert_eq!(command_list_reply("one\ntwo\n"), ("one\ntwo\n".to_string(), vec![]));
    assert_eq!(
        command_list_reply("one\nERROR: can't find pane: %99\n"),
        ("one\n".to_string(), vec!["can't find pane: %99".to_string()])
    );
    // switch-client answers `ERROR <msg>` without the colon.
    assert_eq!(
        command_list_reply("ERROR can't find session: x\n"),
        (String::new(), vec!["can't find session: x".to_string()])
    );
    assert_eq!(command_list_reply(""), (String::new(), vec![]));
}
