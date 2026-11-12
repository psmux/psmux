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


#[test]
fn reply_required_only_for_commands_that_always_print() {
    for l in [
        "display-message -p -- \"#{session_name}\"\n",
        "display -pt main x\n",
        "list-sessions\n",
        "list-windows -a -F \"#{window_id}\"\n",
        "list-panes -F \"#{pane_id}\"\n",
        "list-keys\n",
        "capture-pane -p\n",
        "new-session -d -P -F \"#{session_name}\" -s x\n",
        "split-window -dP\n",
    ] {
        assert!(reply_required(l), "{l:?} always prints on success");
    }
    for l in [
        "set-option -g status off\n",
        "display-message hello\n",
        "list-windows -f \"#{==:#{window_name},x}\"\n",
        "list-keys -T copy-mode\n",
        "capture-pane\n",
        "new-session -d -s x\n",
        "send-keys -t main -- -p\n",
        "display-message -p a ; set -g status off\n",
        "list-buffers\n",
        "show-options -w\n",
        "\n",
    ] {
        assert!(!reply_required(l), "{l:?} may succeed silently");
    }
}

#[test]
fn ack_then_reset_with_no_reply_is_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = [0u8; 256];
        let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
        let _ = s.read(&mut buf);
        let _ = s.write_all(b"OK\n");
        // Abort: linger 0 makes the close a reset.
        socket_with_zero_linger(&s);
        drop(s);
    });
    let got = exchange_on_port(port, "k", None, "display-message -p x\n", Some(Duration::from_secs(3)));
    server.join().unwrap();
    let err = got.expect_err("an ack followed by a reset is a lost reply");
    assert!(err.to_string().contains("no reply from server") || err.to_string().contains("no response"), "{err}");
}

#[cfg(windows)]
fn socket_with_zero_linger(s: &std::net::TcpStream) {
    use std::os::windows::io::AsRawSocket;
    #[repr(C)]
    struct Linger { l_onoff: u16, l_linger: u16 }
    #[link(name = "ws2_32")]
    extern "system" {
        fn setsockopt(s: usize, level: i32, name: i32, val: *const u8, len: i32) -> i32;
    }
    const SOL_SOCKET: i32 = 0xffff;
    const SO_LINGER: i32 = 0x0080;
    let l = Linger { l_onoff: 1, l_linger: 0 };
    unsafe {
        setsockopt(s.as_raw_socket() as usize, SOL_SOCKET, SO_LINGER,
            &l as *const Linger as *const u8, std::mem::size_of::<Linger>() as i32);
    }
}
#[cfg(not(windows))]
fn socket_with_zero_linger(_s: &std::net::TcpStream) {}

#[test]
fn only_single_read_only_commands_are_retried() {
    for l in [
        "display-message -p -- \"#{session_name}\"\n",
        "list-panes -F \"#{pane_id}\"\n",
        "list-buffers\n",
        "show-options -g status\n",
        "showenv\n",
        "capture-pane -p -t main\n",
        "has-session -t main\n",
    ] {
        assert!(read_only_retryable(l), "{l:?} only reads");
    }
    for l in [
        "display-message hello\n",
        "capture-pane -t main\n",
        "set-option -g status off\n",
        "send-keys -t main -- -p\n",
        "new-session -d -P -s x\n",
        "kill-session -t x\n",
        "list-panes ; kill-server\n",
        "display-message -p a \; set -g status off\n",
    ] {
        assert!(!read_only_retryable(l), "{l:?} must never run twice");
    }
}

/// A server that drops the first `drop_first` connections without a byte and
/// answers the rest; returns how many requests it saw.
fn flaky_server(drop_first: usize, reply: &'static str) -> (u16, std::thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let _ = listener.set_nonblocking(false);
    let h = std::thread::spawn(move || {
        let mut seen = 0usize;
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut s, _)) => {
                    s.set_nonblocking(false).unwrap();
                    seen += 1;
                    let mut buf = [0u8; 512];
                    let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
                    let _ = s.read(&mut buf);
                    if seen > drop_first {
                        let _ = s.write_all(reply.as_bytes());
                        let _ = s.shutdown(std::net::Shutdown::Write);
                        let _ = s.set_read_timeout(Some(Duration::from_millis(100)));
                        while matches!(s.read(&mut buf), Ok(n) if n > 0) {}
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
        seen
    });
    (port, h)
}

#[test]
fn a_lost_read_only_reply_is_retried_once_on_a_fresh_connection() {
    let (port, server) = flaky_server(1, "OK\nmain\n");
    let got = exchange_with_read_only_retry(port, "k", None, "display-message -p x\n", Some(Duration::from_secs(2)));
    assert_eq!(got.unwrap(), "main\n");
    assert_eq!(server.join().unwrap(), 2, "exactly one retry");
}

#[test]
fn a_mutating_command_is_never_sent_twice() {
    let (port, server) = flaky_server(1, "OK\n");
    let got = exchange_with_read_only_retry(port, "k", None, "new-window -d\n", Some(Duration::from_secs(2)));
    assert!(got.is_err(), "the lost reply is reported");
    assert_eq!(server.join().unwrap(), 1, "never re-run a command that changes state");
}

#[test]
fn a_read_only_command_is_retried_at_most_once() {
    let (port, server) = flaky_server(5, "OK\nx\n");
    let got = exchange_with_read_only_retry(port, "k", None, "list-panes\n", Some(Duration::from_secs(2)));
    assert!(got.is_err());
    assert_eq!(server.join().unwrap(), 2);
}
