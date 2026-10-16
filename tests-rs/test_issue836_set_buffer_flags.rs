// Issue #836: set-buffer -n stored the new name as data instead of renaming,
// a bundled -ab NAME lost its -a, and -t was read as a session or as data.
//
// tmux's template is `ab:t:n:w` (cmd-set-buffer.c:36), read by
// args_parse_flags, so flags bundle. `-n` renames the -b buffer, or the newest
// automatic one (cmd-set-buffer.c:87 to :104, paste_rename in paste.c:203).
// `-t` is a client (CMD_CLIENT_TFLAG|CMD_CLIENT_CANFAIL, :40).
//
// Only control mode parsed `-n` before (#813). The CLI, the server's command
// dispatch and execute_command_string (hooks, menus) now use the same
// parse_set_buffer_args and run_set_buffer. These tests cover the parser, the
// CLI's wire line and the hook route on an AppState. The CLI and if-shell
// routes are pinned end to end by tests/test_issue836_set_buffer_flags.ps1.

use super::*;

fn parse(args: &[&str]) -> SetBufferArgs {
    parse_set_buffer_args(args).expect("parses").expect("has data or -n")
}

fn app_with(named: &[(&str, &str)], auto_oldest_first: &[&str]) -> AppState {
    let mut app = AppState::new("t".to_string());
    for d in auto_oldest_first {
        app.paste_add(d.to_string());
    }
    for (n, d) in named {
        app.paste_set(n, d.to_string());
    }
    app
}

/// Every buffer as `name=data`, sorted.
fn buffers(app: &AppState) -> Vec<String> {
    let mut v: Vec<String> = app.paste_buffers.iter().map(|b| format!("{}={}", b.name, b.data)).collect();
    v.extend(app.named_buffers.iter().map(|(n, d)| format!("{}={}", n, d)));
    v.sort();
    v
}

#[test]
fn n_is_a_new_name_not_data() {
    let set = parse(&["-b", "old", "-n", "new"]);
    assert_eq!(set.name.as_deref(), Some("old"));
    assert_eq!(set.new_name.as_deref(), Some("new"));
    assert_eq!(set.content, "");
}

#[test]
fn bundled_flags_are_split() {
    let set = parse(&["-ab", "buffer0", "TAIL"]);
    assert!(set.append);
    assert_eq!(set.name.as_deref(), Some("buffer0"));
    assert_eq!(set.content, "TAIL");

    let set = parse(&["-bbuffer0", "-a", "GLUED"]);
    assert!(set.append);
    assert_eq!(set.name.as_deref(), Some("buffer0"));
    assert_eq!(set.content, "GLUED");

    let set = parse(&["-nrenamed"]);
    assert_eq!(set.new_name.as_deref(), Some("renamed"));

    let set = parse(&["-aw", "MORE"]);
    assert!(set.append && set.clipboard);
    assert_eq!(set.content, "MORE");
}

#[test]
fn t_value_is_a_client_not_data() {
    let set = parse(&["-t", "nosuchclient", "TDATA"]);
    assert_eq!(set.content, "TDATA");
    assert_eq!(set.name, None);
    assert_eq!(set.target.as_deref(), Some("nosuchclient"));
    assert_eq!(parse(&["-tsesC", "x"]).target.as_deref(), Some("sesC"));

    let set = parse(&["-t", "c1", "-b", "tb", "TB"]);
    assert_eq!(set.name.as_deref(), Some("tb"));
    assert_eq!(set.content, "TB");
}

#[test]
fn no_data_is_an_error_and_empty_data_is_nothing() {
    assert_eq!(parse_set_buffer_args::<&str>(&[]).err().as_deref(), Some("no data specified"));
    assert_eq!(parse_set_buffer_args(&["-b", "x"]).err().as_deref(), Some("no data specified"));
    assert!(parse_set_buffer_args(&[""]).expect("empty data is fine").is_none());
}

#[test]
fn data_after_double_dash_may_start_with_a_dash() {
    assert_eq!(parse(&["--", "-n"]).content, "-n");
    assert_eq!(parse(&["-b", "x", "--", "-ab"]).content, "-ab");
}

#[test]
fn wire_line_carries_the_rename_and_quotes_names() {
    assert_eq!(set_buffer_wire_line(&parse(&["-b", "old", "-n", "new"])), "set-buffer -b \"old\" -n \"new\"\n");
    assert_eq!(set_buffer_wire_line(&parse(&["-n", ""])), "set-buffer -n \"\"\n");
    assert_eq!(set_buffer_wire_line(&parse(&["-ab", "my buf", "x"])), "set-buffer -a -b \"my buf\" -H 78\n");
    assert_eq!(set_buffer_wire_line(&parse(&["-t", "sesC", "x"])), "set-buffer -t \"sesC\" -H 78\n");
    // The wire line parses back to the same request on the server.
    let back = parse_set_buffer_args(&["-a", "-b", "my buf", "-H", "78"]).unwrap().unwrap();
    assert!(back.append);
    assert_eq!(back.name.as_deref(), Some("my buf"));
    assert_eq!(back.content, "x");
}

#[test]
fn rename_keeps_the_data() {
    let mut app = app_with(&[("old", "OLDDATA")], &[]);
    run_set_buffer(&mut app, Some("old"), "", false, Some("new")).unwrap();
    assert_eq!(buffers(&app), vec!["new=OLDDATA"]);
}

#[test]
fn rename_without_b_takes_the_newest_automatic_buffer() {
    let mut app = app_with(&[], &["AUTO1"]);
    run_set_buffer(&mut app, None, "", false, Some("renamed")).unwrap();
    assert_eq!(buffers(&app), vec!["renamed=AUTO1"]);
}

#[test]
fn rename_onto_a_used_name_replaces_that_buffer() {
    let mut app = app_with(&[("a", "A"), ("b", "B")], &[]);
    run_set_buffer(&mut app, Some("a"), "", false, Some("b")).unwrap();
    assert_eq!(buffers(&app), vec!["b=A"]);
}

#[test]
fn rename_errors_are_tmux_messages() {
    let mut app = app_with(&[], &[]);
    assert_eq!(run_set_buffer(&mut app, Some("nosuch"), "", false, Some("y")).err().as_deref(), Some("unknown buffer: nosuch"));
    assert_eq!(run_set_buffer(&mut app, None, "", false, Some("z")).err().as_deref(), Some("no buffer"));
    let mut app = app_with(&[], &["AUTO1"]);
    assert_eq!(run_set_buffer(&mut app, None, "", false, Some("")).err().as_deref(), Some("new name is empty"));
    assert_eq!(buffers(&app), vec!["buffer0=AUTO1"]);
}

#[test]
fn hook_route_renames_and_appends() {
    let mut app = app_with(&[("old", "OLDDATA")], &[]);
    execute_command_string(&mut app, "set-buffer -b old -n new").unwrap();
    assert_eq!(buffers(&app), vec!["new=OLDDATA"]);

    let mut app = app_with(&[], &["AUTO1"]);
    execute_command_string(&mut app, "set-buffer -ab buffer0 TAIL").unwrap();
    assert_eq!(buffers(&app), vec!["buffer0=AUTO1TAIL"]);

    let mut app = app_with(&[], &[]);
    execute_command_string(&mut app, "set-buffer -t nosuchclient TDATA").unwrap();
    assert_eq!(buffers(&app), vec!["buffer0=TDATA"]);
}

#[test]
fn cross_session_refusal_names_the_session() {
    assert_eq!(
        set_buffer_cross_session_refusal("sesC"),
        "set-buffer: sesC is another session (cross-session set-buffer is not supported: each session runs in its own psmux server, with its own paste buffers)"
    );
}

#[test]
fn hook_route_reports_a_failed_rename() {
    let mut app = app_with(&[], &[]);
    execute_command_string(&mut app, "set-buffer -b nosuch -n y").unwrap();
    assert!(buffers(&app).is_empty());
    let msg = app.status_message.as_ref().map(|(m, _, _)| m.clone());
    assert_eq!(msg.as_deref(), Some("unknown buffer: nosuch"));
}
