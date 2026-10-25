// Regression tests for the refresh-client control-only flag rejection
// (f62b3cb + d80093a).
//
// refresh-client -C/-B/-A/-f only make sense for a control-mode client;
// tmux rejects them with "not a control client" for anyone else. The
// one-shot CLI path used to silently drop every flag, letting callers
// believe a size or subscription was applied when nothing happened. The
// fix rejects the flags server-side with tmux's error text, which the CLI
// then surfaces (error to stderr, non-zero exit).
//
// The server-side rejection lives in `handle_connection`, the one-shot CLI
// connection handler. These tests drive it over real loopback sockets and
// assert the exact wire text for every control-only flag, plus that a
// flag-free refresh-client is still forwarded to the server loop.

use super::*;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread::JoinHandle;
use std::time::Duration;

const TEST_KEY: &str = "unit-test-key";

/// Serve one connection through the real one-shot handler.
fn spawn_handler(listener: TcpListener, tx: mpsc::Sender<CtrlReq>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("handler: accept");
        handle_connection(
            stream,
            tx,
            TEST_KEY,
            Default::default(),
        );
    })
}

/// Connect like the one-shot CLI client: AUTH, expect OK, return the socket.
fn connect_authenticated(addr: std::net::SocketAddr) -> TcpStream {
    let mut client = TcpStream::connect(addr).expect("client connect");
    client.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    write!(client, "AUTH {TEST_KEY}\n").unwrap();
    let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
    let mut ok = String::new();
    reader.read_line(&mut ok).expect("read OK");
    assert_eq!(ok, "OK\n", "handler must authenticate the client");
    client
}

/// Every control-only refresh-client flag is rejected on the one-shot CLI
/// path with tmux's "not a control client" error carrying the flag name —
/// the text the CLI propagates to its exit path.
///
/// All four take a VALUE in tmux (cmd-refresh-client.c `.args` is
/// `A:B:cC:Df:r:F:lLRSt:U`), so they are exercised the way a real client
/// sends them. The bare forms are a different error entirely — see
/// `bare_control_only_flags_are_a_parse_error` below.
#[test]
fn control_only_flags_are_rejected_with_the_tmux_error_text() {
    for (flag, value) in [
        ("-C", "80,24"),
        ("-B", "sub:%1:#{pane_id}"),
        ("-A", "%1:on"),
        ("-f", "no-pause"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let (tx, _rx) = mpsc::channel::<CtrlReq>();
        let handler = spawn_handler(listener.try_clone().unwrap(), tx);

        let mut client = connect_authenticated(listener.local_addr().unwrap());
        write!(client, "refresh-client {flag} {value}\n").unwrap();
        client.shutdown(Shutdown::Write).unwrap();

        let mut resp = String::new();
        client.read_to_string(&mut resp).expect("read response");
        handler.join().expect("handler thread");

        assert_eq!(
            resp,
            format!("ERROR: refresh-client {flag}: not a control client\n"),
            "the one-shot CLI path must reject {flag} instead of silently dropping it"
        );
    }
}

/// Issue #635: the same four flags with NO value never reach the
/// control-client check at all. tmux's args_parse_flags fails the parse first
/// ("-C expects an argument") and the command never runs, so the server must
/// answer that instead of acting on a flag whose value the caller forgot.
#[test]
fn bare_control_only_flags_are_a_parse_error() {
    for flag in ["-C", "-B", "-A", "-f"] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let (tx, _rx) = mpsc::channel::<CtrlReq>();
        let handler = spawn_handler(listener.try_clone().unwrap(), tx);

        let mut client = connect_authenticated(listener.local_addr().unwrap());
        write!(client, "refresh-client {flag}\n").unwrap();
        client.shutdown(Shutdown::Write).unwrap();

        let mut resp = String::new();
        client.read_to_string(&mut resp).expect("read response");
        handler.join().expect("handler thread");

        assert_eq!(
            resp,
            format!("ERROR: {flag} expects an argument\n"),
            "a dangling {flag} must fail the parse, not fall through to the command"
        );
    }
}

/// A refresh-client without control-only flags is still forwarded to the
/// server loop (the rejection must not break the flag-free path).
#[test]
fn flag_free_refresh_client_is_forwarded_to_the_server() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    let handler = spawn_handler(listener.try_clone().unwrap(), tx);

    let mut client = connect_authenticated(listener.local_addr().unwrap());
    write!(client, "refresh-client\n").unwrap();
    client.shutdown(Shutdown::Write).unwrap();

    let mut resp = String::new();
    client.read_to_string(&mut resp).expect("read response");
    handler.join().expect("handler thread");

    assert_eq!(resp, "", "flag-free refresh-client must produce no error");
    let request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("the request must reach the server loop");
    assert!(
        matches!(request, CtrlReq::RefreshClient),
        "flag-free refresh-client must be forwarded unchanged"
    );
}

/// Unknown flags are not control-only and must not be rejected: the
/// rejection must key off exactly -C/-B/-A/-f.
#[test]
fn non_control_flags_are_not_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let (tx, rx) = mpsc::channel::<CtrlReq>();
    let handler = spawn_handler(listener.try_clone().unwrap(), tx);

    let mut client = connect_authenticated(listener.local_addr().unwrap());
    write!(client, "refresh-client -S\n").unwrap();
    client.shutdown(Shutdown::Write).unwrap();

    let mut resp = String::new();
    client.read_to_string(&mut resp).expect("read response");
    handler.join().expect("handler thread");

    assert_eq!(resp, "", "-S must not be rejected as control-only");
    let request = rx.recv_timeout(Duration::from_secs(2)).expect("request forwarded");
    assert!(matches!(request, CtrlReq::RefreshClient));
}
