// A one-shot connection ends a command list at the first command that fails,
// as tmux's cmd-queue.c removes the rest of the list after CMD_RETURN_ERROR.
// The loop learns of the failure from ReplyStream, which notes every
// formatted `ERROR...` reply written since it last asked.

use super::*;
use std::io::Write;

fn reply_stream() -> (ReplyStream, std::net::TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (server, _) = listener.accept().unwrap();
    (ReplyStream::new(server), client)
}

#[test]
fn an_error_reply_is_noted_once() {
    let (mut s, _peer) = reply_stream();
    assert!(!s.take_error());
    let e = "can't find pane: %99";
    let _ = writeln!(s, "ERROR: {e}");
    assert!(s.take_error());
    assert!(!s.take_error(), "taking the note clears it");
}

#[test]
fn switch_clients_error_form_counts() {
    let (mut s, _peer) = reply_stream();
    let reply = "ERROR can't find session: x";
    let _ = writeln!(s, "{reply}");
    assert!(s.take_error());
}

#[test]
fn ordinary_output_is_not_an_error() {
    let (mut s, _peer) = reply_stream();
    let text = "one";
    let _ = writeln!(s, "{text}");
    let _ = writeln!(s, "OK");
    // Pane text goes out with write_all and is never read as a reply.
    let _ = s.write_all(b"ERROR in the pane's own output\n");
    assert!(!s.take_error());
}
