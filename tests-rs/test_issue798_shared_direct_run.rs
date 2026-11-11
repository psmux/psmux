use super::*;

fn session(name: &str) -> Session {
    let mut app = AppState::new(name.to_string());
    app.socket_name = Some("tdirect".to_string());
    let aliases = app.command_aliases.clone();
    Session {
        app,
        lp: SessionLoop::new(aliases),
        cwd: None,
        port: 0,
        stop: Arc::new(AtomicBool::new(false)),
        _guard: None,
    }
}

fn sessions(names: &[&str]) -> BTreeMap<usize, Session> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| (i + 1, session(name)))
        .collect()
}

fn queue(lines: &[(&str, &str)]) {
    OTHER_SESSION_LINES.with(|q| {
        let mut q = q.borrow_mut();
        q.clear();
        q.extend(lines.iter().map(|(n, l)| (n.to_string(), l.to_string())));
    });
}

fn queued() -> usize {
    OTHER_SESSION_LINES.with(|q| q.borrow().len())
}

fn option(all: &BTreeMap<usize, Session>, sid: usize, name: &str) -> Option<String> {
    all[&sid].app.user_options.get(name).cloned()
}

fn run_on_app(s: &mut Session, line: &str) -> Flow {
    let _ = crate::commands::execute_command_string(&mut s.app, line);
    Flow::Continue
}

#[test]
fn u1_queued_sets_for_another_session_run_there_in_sequence() {
    let _lock = crate::util::lock_test_env();
    let mut all = sessions(&["A", "B"]);
    let mut ended = Vec::new();
    queue(&[("B", "set -t B: @x 1"), ("B", "set -t B: @x 2")]);
    drain_other_session_lines(&mut all, &mut ended, run_on_app);
    assert_eq!(option(&all, 2, "@x").as_deref(), Some("2"));
    assert_eq!(option(&all, 1, "@x"), None);
    assert!(ended.is_empty());
    assert_eq!(queued(), 0);

    let mut all = sessions(&["A", "B"]);
    queue(&[("B", "set -t B: @x 2"), ("B", "set -t B: @x 1")]);
    drain_other_session_lines(&mut all, &mut ended, run_on_app);
    assert_eq!(option(&all, 2, "@x").as_deref(), Some("1"));
}

#[test]
fn u2_a_session_that_ends_skips_its_later_lines_and_not_the_others() {
    let _lock = crate::util::lock_test_env();
    let mut all = sessions(&["A", "B", "C"]);
    let mut ended = Vec::new();
    let mut ran = Vec::new();
    queue(&[("B", "first"), ("B", "second"), ("C", "third")]);
    drain_other_session_lines(&mut all, &mut ended, |s, line| {
        ran.push(format!("{}:{line}", s.app.session_name));
        if line == "first" {
            Flow::EndSession
        } else {
            Flow::Continue
        }
    });
    assert_eq!(ran, vec!["B:first", "C:third"]);
    assert_eq!(ended, vec![(2, "panic")]);

    let mut all = sessions(&["A", "B", "C"]);
    let mut ended = Vec::new();
    let mut ran = Vec::new();
    queue(&[("B", "first"), ("B", "second"), ("C", "third")]);
    drain_other_session_lines(&mut all, &mut ended, |s, line| {
        ran.push(format!("{}:{line}", s.app.session_name));
        Flow::Continue
    });
    assert_eq!(ran, vec!["B:first", "B:second", "C:third"]);
    assert!(ended.is_empty());
}

#[test]
fn u3_lines_for_a_missing_or_ended_session_are_dropped() {
    let _lock = crate::util::lock_test_env();
    let mut all = sessions(&["A", "B", "C"]);
    let mut ended = vec![(3, "kill-session")];
    let mut ran = Vec::new();
    queue(&[("GONE", "one"), ("C", "two"), ("B", "three")]);
    drain_other_session_lines(&mut all, &mut ended, |s, line| {
        ran.push(format!("{}:{line}", s.app.session_name));
        Flow::Continue
    });
    assert_eq!(ran, vec!["B:three"]);
    assert_eq!(ended, vec![(3, "kill-session")]);
    assert_eq!(queued(), 0);
}

#[test]
fn u4_only_a_plain_session_user_option_set_runs_directly() {
    let yes = [
        "set -t B: @x 1",
        "set-option -t B: @x 1",
        "set -q -t B: @x 1",
        "set -t B: -q @x 1",
        "set -u -t B: @x",
        "set -t B: -u @x",
        "set -t B: @a-b_1 v",
    ];
    for line in yes {
        assert!(direct_run_line(line, "B"), "{line}");
    }
    let no = [
        "set\t-t B: @x 1",
        "set  -t B: @x 1",
        " set -t B: @x 1",
        "set -t B: @x 1 ",
        "set -t B: @x a\"b\"c",
        "set -t B: @x \"a b\"",
        "set -t B: @x 'a'",
        "set \"-t\" B: @x 1",
        "setw -t B: @x 1",
        "set-window-option -t B: @x 1",
        "set -w -t B: @x 1",
        "set -g -t B: @x 1",
        "set -s -t B: @x 1",
        "set -p -t B: @x 1",
        "set -a -t B: @x 1",
        "set -o -t B: @x 1",
        "set -F -t B: @x 1",
        "set -U -t B: @x",
        "set -qt B: @x 1",
        "set -t B: -- @x 1",
        "set -t B:9 @x 1",
        "set -t B:0.1 @x 1",
        "set -t B @x 1",
        "set -t B: -t B: @x 1",
        "set -t C: @x 1",
        "set @x 1",
        "set -t B: @plugin ppm",
        "set -t B: status-left x",
        "set -t B: @x[0] v",
        "set -t B: @ v",
        "set -t B: @x",
        "set -u -t B: @x 1",
        "set -t B: @x 1 2",
        "set -t B: @x -5",
        "set -t B: @x -q",
        "set -t B: @x a;",
        "set -t B: @x a\\b",
        "set -t B: @x #a",
        "set -t B: @x 1 ; set -t B: @y 2",
        "new-window -t B:",
        "",
    ];
    for line in no {
        assert!(!direct_run_line(line, "B"), "{line}");
    }
}

#[test]
fn u5_a_line_rebuilt_for_another_session_keeps_the_direct_shape() {
    let words = ["set", "-t", "B:", "@x", "1"];
    let line = words
        .iter()
        .map(|w| crate::util::quote_arg_for_handler(w))
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(line, "set -t B: @x 1");
    assert!(direct_run_line(&line, "B"));
    let unset = ["set", "-u", "-t", "B:", "@x"];
    let line = unset
        .iter()
        .map(|w| crate::util::quote_arg_for_handler(w))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(direct_run_line(&line, "B"));
}

#[test]
fn u6_a_line_queued_while_draining_runs_at_the_next_drain() {
    let _lock = crate::util::lock_test_env();
    let mut all = sessions(&["A", "B"]);
    let mut ended = Vec::new();
    let mut ran = Vec::new();
    queue(&[("B", "outer")]);
    drain_other_session_lines(&mut all, &mut ended, |_s, line| {
        ran.push(line.to_string());
        if line == "outer" {
            OTHER_SESSION_LINES.with(|q| q.borrow_mut().push(("B".into(), "inner".into())));
        }
        Flow::Continue
    });
    assert_eq!(ran, vec!["outer"]);
    assert_eq!(queued(), 1);
    drain_other_session_lines(&mut all, &mut ended, |_s, line| {
        ran.push(line.to_string());
        Flow::Continue
    });
    assert_eq!(ran, vec!["outer", "inner"]);
    assert_eq!(queued(), 0);
}

#[test]
fn u7_a_session_with_a_line_on_the_port_keeps_its_later_lines_on_the_port() {
    let mut ported = Vec::new();
    assert!(admit(&mut ported, "B", true));
    assert!(ported.is_empty());
    assert!(!admit(&mut ported, "B", false));
    assert_eq!(ported, vec!["B".to_string()]);
    assert!(!admit(&mut ported, "B", true));
    assert!(!admit(&mut ported, "B", false));
    assert_eq!(ported, vec!["B".to_string()]);
    assert!(admit(&mut ported, "C", true));
    assert_eq!(ported, vec!["B".to_string()]);
}

#[test]
fn u8_each_drain_clears_the_sessions_on_the_port() {
    let _lock = crate::util::lock_test_env();
    let mut all = sessions(&["A", "B"]);
    let mut ended = Vec::new();
    queue(&[]);
    PORTED_SESSIONS.with(|ported| *ported.borrow_mut() = vec!["B".to_string()]);
    drain_other_session_lines(&mut all, &mut ended, run_on_app);
    assert!(PORTED_SESSIONS.with(|ported| ported.borrow().is_empty()));

    PORTED_SESSIONS.with(|ported| *ported.borrow_mut() = vec!["B".to_string()]);
    assert!(!PORTED_SESSIONS.with(|ported| admit(&mut ported.borrow_mut(), "B", true)));
    PORTED_SESSIONS.with(|ported| ported.borrow_mut().clear());
}

#[test]
fn u9_outside_the_shared_server_the_call_returns_false_and_queues_nothing() {
    let _lock = crate::util::lock_test_env();
    // A test process never runs the shared server, as a default server never does.
    assert!(!in_shared_server());
    queue(&[]);
    let app = AppState::new("A".to_string());
    let line = "set -t B: @x 1";
    let parts: Vec<&str> = line.split(' ').collect();
    for on_loop in [false, true] {
        ON_LOOP.with(|c| c.set(on_loop));
        assert!(!crate::server::shared::run_in_other_session(&app, &parts, line));
        assert_eq!(queued(), 0);
        assert!(PORTED_SESSIONS.with(|p| p.borrow().is_empty()));
    }
    ON_LOOP.with(|c| c.set(false));
}
