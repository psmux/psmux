//! Connection-handler checks for the disposable VM only.
use super::*;
use std::io::{BufRead, Read, Write};

fn aliases() -> crate::command_alias::SharedAliases {
    let mut aliases = crate::command_alias::CommandAliases::default();
    for (i, alias) in [
        "__wait-reply-supported=set-option -g @probe-mutated yes",
        "AUTH=display-message -p 1",
        "PSMUX_WAIT_REPLY_V1=display-message -p 1",
    ].iter().enumerate() {
        aliases.set(&format!("command-alias[{}]", i), alias, false).unwrap();
    }
    assert!(aliases.contains_key("AUTH"));
    std::sync::Arc::new(std::sync::RwLock::new(aliases))
}

#[test]
fn auth_capability_never_dispatches_configured_application_aliases() {
    for (key, auth, ack) in [
        ("k".to_string(), "AUTH k\n".to_string(), "OK\n"),
        (
            "k".to_string(),
            format!("AUTH k {}\n", crate::wait_for::AUTH_CAPABILITY),
            crate::wait_for::NEGOTIATION_ACK,
        ),
        (
            format!("k {}", crate::wait_for::AUTH_CAPABILITY),
            format!("AUTH k {}\n", crate::wait_for::AUTH_CAPABILITY),
            "OK\n", // A plain key ending in the token is not negotiation.
        ),
    ] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, requests) = mpsc::channel();
        let configured = aliases();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, tx, &key, configured);
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(auth.as_bytes()).unwrap();
        let mut reader = io::BufReader::new(stream);
        let mut reply = String::new();
        reader.read_line(&mut reply).unwrap();
        assert_eq!(reply, ack);
        reader
            .get_ref()
            .shutdown(std::net::Shutdown::Write)
            .unwrap();
        reply.clear();
        reader.read_to_string(&mut reply).unwrap();
        assert!(reply.is_empty(), "AUTH must not produce application output");
        server.join().unwrap();
        for req in requests.try_iter() {
            assert!(
                matches!(req, CtrlReq::WaitForClientLost(_)),
                "no application request may be dispatched"
            );
        }
    }
}

#[test]
fn negotiated_auth_makes_eof_cancel_a_pending_waiter_without_a_preamble() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, requests) = mpsc::channel();
    let configured = aliases();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, tx, "k", configured);
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "AUTH k {}", crate::wait_for::AUTH_CAPABILITY).unwrap();
    let mut reader = io::BufReader::new(stream);
    let mut ack = String::new();
    reader.read_line(&mut ack).unwrap();
    assert_eq!(ack, crate::wait_for::NEGOTIATION_ACK);
    reader.get_mut().write_all(b"wait-for lost\n").unwrap();
    let CtrlReq::WaitFor {
        client_id,
        channel,
        op,
        reply,
    } = requests.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("expected the actual wait-for request, not an alias request");
    };
    assert_eq!(channel, "lost");
    assert_eq!(op, crate::wait_for::WaitForOp::Wait);
    // The reply sender is still live: EOF, not normal completion, ends the wait.
    reader
        .get_ref()
        .shutdown(std::net::Shutdown::Write)
        .unwrap();
    let CtrlReq::WaitForClientLost(lost_id) =
        requests.recv_timeout(Duration::from_secs(5)).unwrap()
    else {
        panic!("expected pending-waiter cleanup");
    };
    assert_eq!(lost_id, client_id);
    assert!(
        reply.send(Ok(())).is_err(),
        "disconnected waiter drops its receiver"
    );
    assert_eq!(reader.read_to_string(&mut String::new()).unwrap(), 0);
    server.join().unwrap();
}

#[test]
fn lost_wait_reply_cannot_emit_completion_even_after_command_list_output() {
    for command in ["wait-for lost\n", "display-message -p BEFORE ; wait-for lost\n"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, requests) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle_connection(stream, tx, "k", aliases());
        });
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        writeln!(stream, "AUTH k {}", crate::wait_for::AUTH_CAPABILITY).unwrap();
        let mut reader = io::BufReader::new(stream);
        let mut ack = String::new();
        reader.read_line(&mut ack).unwrap();
        assert_eq!(ack, crate::wait_for::NEGOTIATION_ACK);
        reader.get_mut().write_all(command.as_bytes()).unwrap();
        loop {
            match requests.recv_timeout(Duration::from_secs(5)).unwrap() {
                CtrlReq::DisplayMessage(reply, ..) => { reply.send("BEFORE".to_string()).unwrap(); }
                CtrlReq::WaitFor { reply, .. } => { drop(reply); break; }
                _ => {}
            }
        }
        let mut wire = Vec::new();
        reader.read_to_end(&mut wire).unwrap();
        if command.starts_with("display") {
            assert_eq!(wire.first(), Some(&b'D'), "the preceding output really reached the wire");
        } else {
            assert!(wire.is_empty());
        }
        assert_eq!(crate::wait_for::read_response(&mut io::Cursor::new(wire)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof);
        server.join().unwrap();
    }
}

#[test]
fn overlimit_input_cancels_the_waiter_without_a_success_receipt() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, requests) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, tx, "k", aliases());
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(stream, "AUTH k {}", crate::wait_for::AUTH_CAPABILITY).unwrap();
    let mut reader = io::BufReader::new(stream);
    reader.read_line(&mut String::new()).unwrap();
    reader.get_mut().write_all(b"wait-for full\n").unwrap();
    let CtrlReq::WaitFor { reply, client_id, .. } = requests.recv_timeout(Duration::from_secs(5)).unwrap()
    else { panic!("expected pending wait"); };
    reader.get_mut().write_all(&vec![b'x'; crate::wait_for::MAX_BUFFERED_INPUT + 1]).unwrap();
    let CtrlReq::WaitForClientLost(lost) = requests.recv_timeout(Duration::from_secs(5)).unwrap()
    else { panic!("expected waiter cleanup"); };
    assert_eq!(lost, client_id);
    assert!(reply.send(Ok(())).is_err());
    let mut wire = Vec::new();
    reader.read_to_end(&mut wire).unwrap();
    assert!(String::from_utf8_lossy(&wire).contains("too much input"));
    assert!(crate::wait_for::read_response(&mut io::Cursor::new(wire)).is_err());
    server.join().unwrap();
}

#[test]
fn cloned_reply_streams_keep_complete_frames_serialized() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let client = TcpStream::connect(addr).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let (server, _) = listener.accept().unwrap();
    let mut first = ReplyStream::new(server);
    first.3 = Some(std::sync::Arc::new(std::sync::Mutex::new(())));
    let mut second = first.try_clone().unwrap();
    let writer = std::thread::spawn(move || {
        for _ in 0..8 { second.write_all(&vec![b'b'; 70000]).unwrap(); }
    });
    let original = std::thread::spawn(move || {
        for _ in 0..8 { first.write_all(&vec![b'a'; 70000]).unwrap(); }
        writer.join().unwrap();
        first.complete(&WaitInput::default()).unwrap();
    });
    let output = crate::wait_for::read_response(&mut io::BufReader::new(client)).unwrap();
    assert_eq!(output.len(), 16 * 70000);
    assert_eq!(output.bytes().filter(|b| *b == b'a').count(), 8 * 70000);
    assert_eq!(output.bytes().filter(|b| *b == b'b').count(), 8 * 70000);
    original.join().unwrap();
}

#[test]
fn negotiated_wait_error_has_a_terminal_receipt() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, _requests) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, tx, "k", aliases());
    });
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    writeln!(stream, "AUTH k {}", crate::wait_for::AUTH_CAPABILITY).unwrap();
    let mut reader = io::BufReader::new(stream);
    reader.read_line(&mut String::new()).unwrap();
    reader.get_mut().write_all(b"wait-for -Z bad ; wait-for never\n").unwrap();
    assert_eq!(crate::wait_for::read_response(&mut reader).unwrap(), "ERROR: unknown flag -Z\n");
    server.join().unwrap();
}
