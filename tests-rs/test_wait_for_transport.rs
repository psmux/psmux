use super::*;
use std::io::{BufRead, BufReader, Read, Write};

#[test]
fn completion_transport_requires_receipt_and_keeps_write_side_open() {
    for completed in [false, true] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line, format!("AUTH k {}\n", crate::wait_for::AUTH_CAPABILITY));
            reader.get_mut().write_all(crate::wait_for::NEGOTIATION_ACK.as_bytes()).unwrap();
            for expected in ["TARGET test:1\n", "wait-for ready\n"] {
                line.clear();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line, expected);
            }
            reader.get_ref().set_read_timeout(Some(Duration::from_millis(100))).unwrap();
            let error = reader.read_line(&mut String::new()).expect_err("client must not half-close");
            assert!(matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
                || matches!(error.raw_os_error(), Some(997) | Some(10004) | Some(10060)));
            crate::wait_for::write_output(reader.get_mut(), b"OK\nC\n").unwrap();
            if completed { crate::wait_for::write_complete(reader.get_mut(), &crate::wait_for::WaitInput::default()).unwrap(); }
        });
        let result = exchange_with_completion(port, "k", Some("test:1"), "wait-for ready\n");
        if completed { assert_eq!(result.unwrap(), "OK\nC\n"); }
        else { assert_eq!(result.unwrap_err().kind(), io::ErrorKind::UnexpectedEof); }
        server.join().unwrap();
    }
}

#[test]
fn completion_response_can_arrive_after_auth_timeout() {
    let delay = COMPLETION_AUTH_TIMEOUT + Duration::from_secs(1);
    assert!(delay > COMPLETION_AUTH_TIMEOUT, "response must exceed the actual AUTH budget");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || -> io::Result<()> {
        let (stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)?;
        assert_eq!(line, format!("AUTH k {}\n", crate::wait_for::AUTH_CAPABILITY));
        reader.get_mut().write_all(crate::wait_for::NEGOTIATION_ACK.as_bytes())?;
        for expected in ["TARGET test:1\n", "wait-for slow\n"] {
            line.clear();
            reader.read_line(&mut line)?;
            assert_eq!(line, expected);
        }
        // Model a signal arriving after the handshake deadline, with no
        // earlier payload to restart a mistakenly retained read timeout.
        std::thread::sleep(delay);
        crate::wait_for::write_output(reader.get_mut(), b"completed after AUTH deadline\n")?;
        crate::wait_for::write_complete(reader.get_mut(), &crate::wait_for::WaitInput::default())
    });
    let result = exchange_with_completion(port, "k", Some("test:1"), "wait-for slow\n");
    // Join even on client failure: the delayed peer must finish before the
    // test reports a stale AUTH timeout.
    let peer_result = server.join().unwrap();
    assert_eq!(result.unwrap(), "completed after AUTH deadline\n");
    peer_result.unwrap();
}

#[test]
fn unsupported_auth_submits_nothing_and_does_not_reconnect() {
    for ack in ["OK\n", "ERROR: Invalid session key\n", ""] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (done, finished) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reader = BufReader::new(stream);
            let mut auth = String::new();
            reader.read_line(&mut auth).unwrap();
            assert_eq!(auth, format!("AUTH k {}\n", crate::wait_for::AUTH_CAPABILITY));
            reader.get_mut().write_all(ack.as_bytes()).unwrap();
            reader.get_mut().shutdown(std::net::Shutdown::Write).unwrap();
            let mut remaining = String::new();
            reader.read_to_string(&mut remaining).unwrap();
            assert!(remaining.is_empty(), "neither TARGET nor the command is sent before acceptance");
            // Keep listening until the caller returns, so a reconnect cannot
            // race the negative accept assertion.
            finished.recv_timeout(Duration::from_secs(10)).unwrap();
            listener.set_nonblocking(true).unwrap();
            assert_eq!(listener.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        });
        assert!(exchange_with_completion(port, "k", Some("test:1"), "wait-for -S ready\n").is_err());
        done.send(()).unwrap();
        server.join().unwrap();
    }
}

#[test]
fn ordinary_unbounded_exchange_keeps_auth_half_close_and_eof_contract() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut request = String::new();
        stream.read_to_string(&mut request).unwrap();
        assert_eq!(request, "AUTH k\nSOURCE-JOB\nCURRENT-FILE example.conf\nwait-for -S ready\n");
        stream.write_all(b"OK\n").unwrap();
    });
    assert_eq!(exchange_on_port(port, "k", None,
        "SOURCE-JOB\nCURRENT-FILE example.conf\nwait-for -S ready\n", None).unwrap(), "");
    server.join().unwrap();
}
