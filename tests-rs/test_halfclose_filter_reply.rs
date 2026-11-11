// One-shot CLI requests must not half-close, and a connection that ends with
// no reply at all is an error. A loopback filter seen in the wild (the WFP
// network driver of an ad blocker, 8.0 series) silently aborts a connection
// whose connecting side calls shutdown(SD_SEND): the client reads zero bytes
// and a clean EOF. These tests stand in for it with an in-process listener.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// How the stand-in server treats a request.
#[derive(Clone, Copy)]
enum Peer {
    /// Like the filter: a client FIN seen before the reply loses the reply.
    DropReplyOnClientFin,
    /// Closes after reading the request, with zero bytes sent.
    Silent,
}

/// Serve one connection; returns the port and a handle giving whether the
/// client half-closed before the reply.
fn serve_once(peer: Peer, reply: &'static str) -> (u16, std::thread::JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_millis(150))).unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        let mut saw_fin = false;
        // Read until the client's FIN, or a quiet gap after a full request.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match s.read(&mut buf) {
                Ok(0) => { saw_fin = true; break; }
                Ok(n) => req.extend_from_slice(&buf[..n]),
                Err(_) => {
                    if req.iter().filter(|&&b| b == b'\n').count() >= 2 { break; }
                }
            }
        }
        if matches!(peer, Peer::DropReplyOnClientFin) && !saw_fin {
            let _ = s.write_all(reply.as_bytes());
            let _ = s.shutdown(std::net::Shutdown::Write);
            let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
            while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
        }
        saw_fin
    });
    (port, handle)
}

#[test]
fn exchange_reply_survives_a_filter_that_aborts_client_half_close() {
    let (port, server) = serve_once(Peer::DropReplyOnClientFin, "OK\nthe-reply\n");
    let got = exchange_on_port(port, "k", Some("s"), "display-message -p x\n", Some(Duration::from_secs(3)));
    let half_closed = server.join().unwrap();
    assert!(!half_closed, "the CLI half-closed its request");
    assert_eq!(got.unwrap(), "the-reply\n");
}

#[test]
fn exchange_with_no_reply_at_all_is_an_error_not_empty_output() {
    let (port, server) = serve_once(Peer::Silent, "");
    let got = exchange_on_port(port, "k", None, "display-message -p x\n", Some(Duration::from_secs(3)));
    server.join().unwrap();
    let err = got.expect_err("a connection closed with no reply must not read as an empty result");
    assert!(err.to_string().contains("no response from server"), "{err}");
}

#[test]
fn auth_cmd_response_survives_the_filter_and_reports_no_reply() {
    let (port, server) = serve_once(Peer::DropReplyOnClientFin, "OK\nclaimed OK\n");
    let got = send_auth_cmd_response(&format!("127.0.0.1:{port}"), "abc123", b"claim-session x\n");
    assert!(!server.join().unwrap(), "the warm claim half-closed its request");
    assert_eq!(got.unwrap(), "claimed OK\n");

    let (port, server) = serve_once(Peer::Silent, "");
    let got = send_auth_cmd_response(&format!("127.0.0.1:{port}"), "abc123", b"claim-session x\n");
    server.join().unwrap();
    assert!(got.is_err(), "no AUTH ack must be an error, got {got:?}");
}

#[test]
fn exchange_ends_an_unterminated_request_with_a_newline() {
    // Without a half-close the server only sees the last line once it ends.
    assert_eq!(terminated_request("list-panes"), "list-panes\n");
    assert_eq!(terminated_request("list-panes\n"), "list-panes\n");
    assert_eq!(terminated_request(""), "");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (s, _) = listener.accept().unwrap();
        let mut r = std::io::BufReader::new(s.try_clone().unwrap());
        let (mut a, mut b) = (String::new(), String::new());
        std::io::BufRead::read_line(&mut r, &mut a).unwrap();
        std::io::BufRead::read_line(&mut r, &mut b).unwrap();
        // End the reply the way the server does: half-close, drain, close.
        // A bare close can reach the client as a reset that throws the
        // unread reply away.
        let mut w = s;
        let _ = w.write_all(b"OK\ndone\n");
        let _ = w.shutdown(std::net::Shutdown::Write);
        let _ = w.set_read_timeout(Some(Duration::from_millis(100)));
        let mut sink = [0u8; 64];
        while matches!(w.read(&mut sink), Ok(n) if n > 0) {}
        b
    });
    let got = exchange_on_port(port, "k", None, "list-panes", Some(Duration::from_secs(3))).unwrap();
    assert_eq!(server.join().unwrap(), "list-panes\n");
    assert_eq!(got, "done\n");
}

