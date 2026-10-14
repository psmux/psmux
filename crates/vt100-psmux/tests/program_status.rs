use vt100_psmux::{Parser, ProgramState, TerminalQuery};

fn enabled() -> Parser {
    let mut parser = Parser::default();
    parser.screen_mut().set_program_status_enabled(true);
    parser
}

fn report(parser: &mut Parser, body: &str) {
    parser.process(format!("\x1b]7501;{body}\x1b\\").as_bytes());
}

#[test]
fn reports_and_detection_survive_every_split_and_terminator() {
    for end in ["\x07", "\x1b\\"] {
        let input = format!("\x1b]7501;state=blocked:kind=permission:progress=0:app=pi:msg=SGVsbG8={end}\x1b]7501;?{end}\x1b[c");
        for split in 0..=input.len() {
            let mut parser = enabled();
            parser.screen_mut().capture_queries(true);
            assert!(parser.screen().program_status().get("").is_none());
            parser.process(&input.as_bytes()[..split]);
            parser.process(&input.as_bytes()[split..]);
            let status = parser.screen().program_status().get("").unwrap();
            assert_eq!(status.state, ProgramState::Blocked);
            assert_eq!(status.kind.unwrap().as_str(), "permission");
            assert_eq!(status.progress, Some(0));
            assert_eq!(status.msg.as_deref(), Some("Hello"));
            assert_eq!(parser.screen_mut().take_queries(), vec![TerminalQuery::ProgramStatus, TerminalQuery::Da1]);
        }
    }
}

#[test]
fn replacement_inheritance_and_hierarchical_clear() {
    let mut parser = enabled();
    report(&mut parser, "state=working:app=cargo:msg=SGk=");
    report(&mut parser, "state=idle:id=build:app=test");
    report(&mut parser, "state=blocked:id=build/unit:kind=question");
    report(&mut parser, "state=done:id=builder");
    let statuses = parser.screen().program_status();
    assert_eq!(statuses.inherited_app("build/unit"), Some("test"));
    assert_eq!(statuses.inherited_app("builder"), Some("cargo"));
    assert_eq!(statuses.attention().unwrap().id, "build/unit");
    report(&mut parser, "state=done");
    assert_eq!(parser.screen().program_status().get("").unwrap().app, None);
    assert_eq!(parser.screen().program_status().get("").unwrap().msg, None);
    assert_eq!(parser.screen().program_status().inherited_app("builder"), None);
    report(&mut parser, "state=clear:id=build");
    let statuses = parser.screen().program_status();
    assert_eq!(statuses.records().count(), 2);
    assert!(statuses.get("build/unit").is_none());
    assert!(statuses.get("builder").is_some());
    report(&mut parser, "state=clear");
    assert_eq!(parser.screen().program_status().records().count(), 0);
}

#[test]
fn invalid_reports_leave_existing_records_and_seen_unchanged() {
    let mut parser = enabled();
    report(&mut parser, "state=done:app=pi");
    parser.screen_mut().program_status_mut().acknowledge();
    let before = parser.screen().program_status().get("").unwrap().clone();
    let generation = parser.screen().program_status().generation();
    for body in [
        "state=future", "app=other", "state=working:id=", "state=working:id=a//b",
        "state=working:id=/bad", "state=working:id=bad!", "state=working:id=a/b/c/d/e/f/g/h/i",
        "state=working:msg=Zg===", "state=working:msg=Z", "state=working:msg=Zh==",
        "state=working:msg=/w==", "state=working:msg=Cg==", "state=working:title=woA=",
        "state=working:msg=AA==:msg=SGk=", "state=clear:msg=AA==",
    ] {
        report(&mut parser, body);
        assert_eq!(parser.screen().program_status().get("").unwrap(), &before, "{body}");
        assert!(parser.screen().program_status().seen(""), "{body}");
        assert_eq!(parser.screen().program_status().generation(), generation, "{body}");
    }
}

#[test]
fn malformed_unknown_duplicate_and_state_specific_fields() {
    let mut parser = enabled();
    report(&mut parser, " state = working :state=blocked:kind=auth:progress=100:other=value:bad:empty!=x");
    let root = parser.screen().program_status().get("").unwrap();
    assert_eq!(root.state, ProgramState::Blocked);
    assert_eq!(root.kind.unwrap().as_str(), "auth");
    assert_eq!(root.progress, Some(100));
    report(&mut parser, "state=done:kind=auth:progress=12:app=bad/path:msg=SGk");
    let root = parser.screen().program_status().get("").unwrap();
    assert_eq!(root.kind, None);
    assert_eq!(root.progress, None);
    assert_eq!(root.app, None);
    assert_eq!(root.msg.as_deref(), Some("Hi"));
    report(&mut parser, "state=working:kind=unknown:progress=101");
    assert_eq!(parser.screen().program_status().get("").unwrap().progress, None);
}

#[test]
fn acknowledgements_preserve_retransmissions_but_not_a_new_run_in_one_batch() {
    let mut parser = enabled();
    report(&mut parser, "state=done");
    assert!(parser.screen().program_status().attention().is_some());
    parser.screen_mut().program_status_mut().acknowledge();
    assert!(parser.screen().program_status().attention().is_none());
    report(&mut parser, "state=done");
    assert!(parser.screen().program_status().attention().is_none());
    parser.process(b"\x1b]7501;state=working\x07\x1b]7501;state=done\x07");
    assert_eq!(parser.screen().program_status().attention().unwrap().state, ProgramState::Done);
    report(&mut parser, "state=error:id=failed");
    assert_eq!(parser.screen().program_status().attention().unwrap().state, ProgramState::Error);
    report(&mut parser, "state=blocked:id=question");
    parser.screen_mut().program_status_mut().acknowledge();
    assert_eq!(parser.screen().program_status().attention().unwrap().state, ProgramState::Blocked);
}

#[test]
fn alternate_screen_soft_reset_prompt_exit_and_ris_lifetimes() {
    let mut parser = enabled();
    for (state, id) in [("idle", "idle"), ("working", "work"), ("blocked", "block"), ("done", "done"), ("error", "error")] {
        report(&mut parser, &format!("state={state}:id={id}"));
    }
    parser.process(b"\x1b[?1049h\x1b[!p\x1b[?1049l");
    assert_eq!(parser.screen().program_status().records().count(), 5);
    parser.process(b"\x1b]133;A\x07");
    assert_eq!(parser.screen().program_status().records().count(), 2);
    report(&mut parser, "state=blocked");
    parser.screen_mut().program_status_mut().expire();
    assert_eq!(parser.screen().program_status().records().count(), 2);
    let before = parser.screen().program_status().generation();
    parser.process(b"\x1bc");
    assert_eq!(parser.screen().program_status().records().count(), 0);
    assert!(parser.screen().program_status().generation() > before);
}

#[test]
fn eviction_is_least_recent_update_and_drops_seen_state() {
    let mut parser = enabled();
    for id in 0..64 { report(&mut parser, &format!("state=done:id=r{id}")); }
    assert_eq!(parser.screen().program_status().records().count(), 64);
    parser.screen_mut().program_status_mut().acknowledge();
    report(&mut parser, "state=done:id=r0");
    report(&mut parser, "state=done:id=r64");
    assert!(parser.screen().program_status().get("r0").is_some());
    assert!(parser.screen().program_status().get("r1").is_none());
    report(&mut parser, "state=done:id=r1");
    assert!(!parser.screen().program_status().seen("r1"));
    assert_eq!(parser.screen().program_status().records().count(), 64);
}

#[test]
fn limits_are_checked_before_mutation_including_overwritten_values() {
    let mut parser = enabled();
    report(&mut parser, "state=done");
    for body in [
        format!("state=working:app={}", "a".repeat(33)),
        format!("state=working:id={}", "a".repeat(33)),
        format!("state=working:{}=x", "k".repeat(17)),
        format!("state=working:title={}", "YWFh".repeat(65)),
        format!("state=working:msg={}:msg=SGk=", "YWFh".repeat(684)),
        format!("state=working:msg={}", "YWFh".repeat(683)),
        format!("state=working:id={}:id=valid", "a".repeat(33)),
        format!("state=working:id={}", vec!["a".repeat(32); 4].join("/")),
    ] {
        report(&mut parser, &body);
        assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Done);
    }
    report(&mut parser, &format!("state=working:title={}:msg={}", "YWFh".repeat(64), "YWFh".repeat(682) + "YWE="));
    assert_eq!(parser.screen().program_status().get("").unwrap().title.as_ref().unwrap().len(), 192);
    assert_eq!(parser.screen().program_status().get("").unwrap().msg.as_ref().unwrap().len(), 2048);
    for end in ["\x07", "\x1b\\"] {
        let prefix = "\x1b]7501;state=done:unknown=";
        let input = format!("{prefix}{}{end}", "x".repeat(4096 - prefix.len() - end.len()));
        assert_eq!(input.len(), 4096);
        parser.process(input.as_bytes());
        assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Done);
        let input = input.replacen("state=done", "state=error", 1);
        assert_eq!(input.len(), 4097);
        parser.process(input.as_bytes());
        assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Done);
    }
}

#[test]
fn id_boundaries_and_last_value_wins_without_root_fallback() {
    let mut parser = enabled();
    report(&mut parser, "state=done");
    let id = ["a".repeat(32), "b".repeat(32), "c".repeat(32), "d".repeat(29)].join("/");
    assert_eq!(id.len(), 128);
    report(&mut parser, &format!("state=blocked:id={id}:app={}", "a".repeat(32)));
    assert_eq!(parser.screen().program_status().get(&id).unwrap().app.as_ref().unwrap().len(), 32);
    report(&mut parser, &format!("state=error:id={id}e"));
    assert_eq!(parser.screen().program_status().get(&id).unwrap().state, ProgramState::Blocked);
    report(&mut parser, "state=working:id=a/b/c/d/e/f/g/h");
    assert!(parser.screen().program_status().get("a/b/c/d/e/f/g/h").is_some());
    report(&mut parser, "state=working:id=bad!:id=valid");
    assert!(parser.screen().program_status().get("valid").is_some());
    parser.process(b"\x1b]7501;state=error:id=\xff\x07");
    assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Done);
    parser.process(b"\x1b]7501;state=idle:unknown=\xff\x07");
    assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Idle);
    report(&mut parser, "state=done:unknown=bad;value:other=valid");
    assert_eq!(parser.screen().program_status().get("").unwrap().state, ProgramState::Done);
}

#[test]
fn reinitialization_cannot_reuse_a_previous_semantic_generation() {
    let mut parser = enabled();
    report(&mut parser, "state=done");
    let cached_generation = parser.screen().program_status().generation();
    assert_eq!(cached_generation, 1);
    parser.screen_mut().reinit_keep_history();
    report(&mut parser, "state=blocked");
    assert_ne!(parser.screen().program_status().generation(), cached_generation);
    assert_eq!(parser.screen().program_status().attention().unwrap().state, ProgramState::Blocked);
}

#[test]
fn malformed_utf8_does_not_bypass_any_hard_field_limit() {
    for (key, limit) in [("app", 32), ("title", 256), ("msg", 2732), ("id", 128), ("abcdefghijklmnop", 16)] {
        let mut parser = enabled();
        report(&mut parser, "state=done");
        parser.screen_mut().program_status_mut().acknowledge();
        let generation = parser.screen().program_status().generation();
        let mut input = b"\x1b]7501;state=working:".to_vec();
        if key == "abcdefghijklmnop" {
            input.extend_from_slice(key.as_bytes());
            input.extend_from_slice(b"q=\xff");
        } else {
            input.extend_from_slice(key.as_bytes());
            input.push(b'=');
            input.extend(std::iter::repeat(b'a').take(limit));
            input.push(0xff);
        }
        input.push(7);
        assert!(input.len() < 4096);
        parser.process(&input);
        let statuses = parser.screen().program_status();
        assert_eq!(statuses.get("").unwrap().state, ProgramState::Done, "{key}");
        assert!(statuses.seen(""), "{key}");
        assert_eq!(statuses.generation(), generation, "{key}");
    }
}

#[test]
fn snapshots_neither_replay_nor_capture_queries_and_ris_preserves_order() {
    let mut parser = enabled();
    parser.screen_mut().capture_queries(true);
    parser.process(b"\x1b]7501;?\x07");
    let mut snapshot = parser.snapshot();
    assert!(snapshot.screen_mut().take_queries().is_empty());
    snapshot.process(b"\x1b]7501;?\x07\x1b[c");
    assert!(snapshot.screen_mut().take_queries().is_empty());
    parser.process(b"\x1bc\x1b[c");
    assert_eq!(parser.screen_mut().take_queries(), vec![TerminalQuery::ProgramStatus, TerminalQuery::Da1]);
}
