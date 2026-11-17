use vt100_psmux as vt100;

fn drain(p: &mut vt100::Parser) -> Vec<Vec<u8>> {
    let events = p.screen_mut().take_host_events();
    assert!(events.iter().all(|e| !e.passthrough));
    events.into_iter().map(|e| e.bytes).collect()
}

#[test]
fn exact_allowlisted_bytes_survive_every_split_and_snapshot() {
    let input = b"\x1b]9;hello;world\x07\x1b]777;notify;title;body\x1b\\\x1b]99;i=a:d=0:p=title;chunk\x1b\\\x1b]99;i=a:d=1:p=body:e=1;dGV4dA==\x07\x1b]7501;set;pid=1;state=working\x1b\\\x1b]9001;done\x07";
    let expected: Vec<Vec<u8>> = input
        .split_inclusive(|b| *b == 0x07)
        .map(|b| b.to_vec())
        .collect();
    let mut whole = vt100::Parser::new(4, 80, 0);
    whole.process(input);
    let reference = drain(&mut whole);
    assert_eq!(reference.len(), 6);
    assert_eq!(reference.concat(), input);
    assert!(!expected.is_empty());
    for cut in 0..=input.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&input[..cut]);
        p.process(&input[cut..]);
        assert!(p.snapshot().screen_mut().take_host_events().is_empty());
        assert_eq!(drain(&mut p), reference, "split {cut}");
        assert!(drain(&mut p).is_empty());
        assert!(p.screen().contents().is_empty());
    }
    let mut p = vt100::Parser::new(4, 80, 0);
    for byte in input.chunks(1) {
        p.process(byte);
    }
    assert_eq!(drain(&mut p), reference);
}

#[test]
fn exclusions_and_cancellation_do_not_escape() {
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(b"\x1b]9;4;1;35\x07\x1b]9;9;C:\\work\x07\x1b]52;c;YQ==\x07\x1b]777;query;x\x07\x1b]7501;?\x07\x1b]1234;unknown\x07\x1bPqnot-an-osc\x07\x1b\\");
    assert!(drain(&mut p).is_empty());
    assert_eq!(p.screen().progress(), Some((1, 35)));
    p.process(b"\x1b]9;cancelled\x18\x1b]9;discarded\x1b]9;valid\x07");
    assert_eq!(drain(&mut p), vec![b"\x1b]9;valid\x07".to_vec()]);
}

#[test]
fn semicolon_rich_and_long_osc_are_not_reconstructed_from_vte_params() {
    let mut p = vt100::Parser::new(4, 80, 0);
    let input = format!("\x1b]777;notify;title;{}\x1b\\", "word;".repeat(1000)).into_bytes();
    p.process(&input);
    assert_eq!(drain(&mut p), vec![input]);
}

#[test]
fn oversized_and_flooded_events_drop_whole_sequences_then_recover() {
    let mut p = vt100::Parser::new(4, 80, 0);
    p.process(b"\x1b]9;");
    p.process(&vec![b'x'; vt100::MAX_HOST_EVENT_BYTES]);
    p.process(b"\x07\x1b]9;next\x07");
    assert_eq!(drain(&mut p), vec![b"\x1b]9;next\x07".to_vec()]);
    for _ in 0..300 {
        p.process(b"\x1b]9;x\x07");
    }
    let events = drain(&mut p);
    assert_eq!(events.len(), 256);
    assert!(events.iter().all(|e| e == b"\x1b]9;x\x07"));
    p.process(b"\x1b]9;after\x07");
    assert_eq!(drain(&mut p), vec![b"\x1b]9;after\x07".to_vec()]);
}

#[test]
fn cancelled_control_strings_recover_at_the_same_escape_as_vte() {
    for introducer in [b'P', b'_', b'^', b'X'] {
        for cancel in [&b"\x18"[..], &b"\x1a"[..], &b""[..]] {
            let mut bytes = vec![0x1b, introducer];
            bytes.extend_from_slice(b"discarded");
            bytes.extend_from_slice(cancel);
            bytes.extend_from_slice(b"\x1b]9;valid\x07");
            for cut in 0..=bytes.len() {
                let mut p = vt100::Parser::new(4, 80, 0);
                p.process(&bytes[..cut]);
                p.process(&bytes[cut..]);
                assert_eq!(
                    drain(&mut p),
                    vec![b"\x1b]9;valid\x07".to_vec()],
                    "intro {introducer}, cut {cut}"
                );
            }
        }
    }
}

#[test]
fn numeric_osc9_subcommands_are_never_native_notifications() {
    for token in [
        "0",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "10",
        "11",
        "12",
        "0006",
        " 7 ",
        "\t006\r\n",
        "9999999999999999999999999999",
        "5hello",
        "4hello",
        "12suffix",
        " 7abc",
        "007text",
    ] {
        let input = format!("\x1b]9;{token};command-arguments\x07");
        for cut in 0..=input.len() {
            let mut p = vt100::Parser::new(4, 80, 0);
            p.process(&input.as_bytes()[..cut]);
            p.process(&input.as_bytes()[cut..]);
            assert!(
                drain(&mut p).is_empty(),
                "numeric subcommand {token:?}, split {cut}"
            );
        }
    }
    for body in [
        "ordinary message",
        "body;with;semicolons",
        "message;6;not-a-command",
    ] {
        let input = format!("\x1b]9;{body}\x07").into_bytes();
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&input);
        assert_eq!(drain(&mut p), vec![input]);
    }
}

#[test]
fn native_osc9_requires_ascii_control_free_text() {
    for body in [
        &b"\x006;macro"[..],
        &b"\x007;command"[..],
        &b"\x016;macro"[..],
        &b"\x1f7;command"[..],
        &b"\t7;command"[..],
        &b"hello\0world"[..],
        &b"hello\x1fworld"[..],
        &b"hello\x7fworld"[..],
    ] {
        let mut input = b"\x1b]9;".to_vec();
        input.extend_from_slice(body);
        input.push(7);
        let mut p = vt100::Parser::new(4, 80, 0);
        p.process(&input);
        assert!(drain(&mut p).is_empty(), "control-bearing body {body:?}");
    }
}

#[test]
fn conhost_notifications_survive_every_split_and_snapshot() {
    // The upstream conhost wrapper splits at a doubled ESC before process_vt.
    let input = b"\x1b\x1b]9;notice\x07\x1b]777;notify;title;body\x1b\\";
    let expected = vec![
        b"\x1b]9;notice\x07".to_vec(),
        b"\x1b]777;notify;title;body\x1b\\".to_vec(),
    ];
    for chunk_size in 1..=input.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.set_conhost_c0_cells(true);
        for chunk in input.chunks(chunk_size) {
            p.process(chunk);
        }
        assert!(p.snapshot().screen_mut().take_host_events().is_empty());
        assert_eq!(drain(&mut p), expected, "chunk size {chunk_size}");
        assert!(drain(&mut p).is_empty());
    }
    for cut in 0..=input.len() {
        let mut p = vt100::Parser::new(4, 80, 0);
        p.set_conhost_c0_cells(true);
        p.process(&input[..cut]);
        p.process(&input[cut..]);
        assert_eq!(drain(&mut p), expected, "split {cut}");
    }
}
