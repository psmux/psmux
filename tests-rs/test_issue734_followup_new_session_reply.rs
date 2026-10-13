// Issue #734 follow up: what a one-shot detached `new-session` sends back.
// Alone it keeps the `OK` raw socket callers read (test_issue505); inside a
// command list the placeholder is dropped, as tmux prints nothing for a
// `new-session -d` without -P (cmd-new-session.c:348), so an if-shell branch
// `new-session -d -s x ; display-message -p done` prints just `done`.

use super::one_shot_new_session_reply as reply;

#[test]
fn a_lone_plain_success_keeps_ok() {
    assert_eq!(reply("OK\n".to_string(), false), "OK\n");
}

#[test]
fn a_plain_success_inside_a_list_prints_nothing() {
    assert_eq!(reply("OK\n".to_string(), true), "");
}

#[test]
fn a_p_report_passes_through_either_way() {
    let rec = "probe:0\t%1\tC:\\s.sock\t1234\n".to_string();
    assert_eq!(reply(rec.clone(), true), rec);
    assert_eq!(reply(rec.clone(), false), rec);
}

#[test]
fn an_error_passes_through_so_it_ends_the_list() {
    let err = "ERROR: duplicate session: probe\n".to_string();
    assert_eq!(reply(err.clone(), true), err);
}
