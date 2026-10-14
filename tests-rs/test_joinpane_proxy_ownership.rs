// Who owns a pane joined across sessions, and for how long.
//
// The end to end proof is tests/test_joinpane_proxy_ownership.ps1. These pin
// the two decisions it rests on without starting a server:
//   * the source side's reaper ends a forwarded pane exactly when nobody can
//     reach it any more, and never while the target still holds it;
//   * the target side's proxy reads a host that is gone as an exited child
//     (it used to read it as "still running" forever).

use super::*;
use crate::proxy_pane::{classify_forward_status, ForwardProbe, ProxyChild, FORWARD_UNKNOWN_LIMIT};
use portable_pty::Child;
use std::io::{self, BufRead, BufReader, Write};
use std::time::Duration;

const YOUNG: Duration = Duration::from_millis(10);

#[test]
fn verdict_keeps_a_pane_the_target_still_holds() {
    assert_eq!(forward_verdict(false, true, false, YOUNG), ForwardVerdict::Keep);
    assert_eq!(forward_verdict(false, true, false, Duration::from_secs(3600)), ForwardVerdict::Keep);
}

#[test]
fn verdict_waits_for_the_tunnel_until_the_accept_timeout() {
    assert_eq!(forward_verdict(false, false, false, YOUNG), ForwardVerdict::Keep);
    assert_eq!(
        forward_verdict(false, false, false, FORWARD_ACCEPT_TIMEOUT),
        ForwardVerdict::TunnelGone
    );
}

#[test]
fn verdict_ends_a_pane_whose_tunnel_closed() {
    assert_eq!(forward_verdict(false, true, true, YOUNG), ForwardVerdict::TunnelGone);
}

#[test]
fn verdict_child_exit_wins_over_everything() {
    assert_eq!(forward_verdict(true, true, true, YOUNG), ForwardVerdict::ChildExited);
    assert_eq!(forward_verdict(true, false, false, YOUNG), ForwardVerdict::ChildExited);
}

#[test]
fn status_reply_classification() {
    assert_eq!(classify_forward_status(&Ok("running\n".into())), ForwardProbe::Running);
    assert_eq!(classify_forward_status(&Ok("exited\n".into())), ForwardProbe::Exited);
    // The port now belongs to a server that does not know the key.
    assert_eq!(
        classify_forward_status(&Ok("ERROR: Invalid session key\n".into())),
        ForwardProbe::Exited
    );
    // A host that is alive but did not answer in time is not evidence.
    assert_eq!(classify_forward_status(&Ok(String::new())), ForwardProbe::Unknown);
    assert_eq!(
        classify_forward_status(&Err(io::Error::from(io::ErrorKind::ConnectionRefused))),
        ForwardProbe::Exited
    );
    assert_eq!(
        classify_forward_status(&Err(io::Error::from(io::ErrorKind::TimedOut))),
        ForwardProbe::Unknown
    );
}

#[test]
fn one_unanswered_poll_is_not_an_exit() {
    assert!(FORWARD_UNKNOWN_LIMIT > 1);
}

/// A loopback port nothing listens on.
fn dead_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

#[test]
fn proxy_child_reports_exited_when_its_host_is_gone() {
    let mut child = ProxyChild::new(format!("127.0.0.1:{}", dead_port()), "k".into(), 1, Some(1));
    let mut exited = false;
    for _ in 0..(FORWARD_UNKNOWN_LIMIT * 2) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            exited = true;
            break;
        }
    }
    assert!(exited, "a proxy whose host process is gone must read as exited");
}

#[test]
fn proxy_child_stays_alive_while_its_host_answers_running() {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let polls = (FORWARD_UNKNOWN_LIMIT * 2) as usize;
    let host = std::thread::spawn(move || {
        for _ in 0..polls {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            let _ = r.read_line(&mut line); // AUTH
            line.clear();
            let _ = r.read_line(&mut line); // pane-forward-status
            let mut w = s;
            let _ = w.write_all(b"OK\nrunning\n");
            let _ = w.flush();
        }
    });
    let mut child = ProxyChild::new(addr.to_string(), "k".into(), 1, Some(1));
    for _ in 0..polls {
        assert!(matches!(child.try_wait(), Ok(None)), "a host that answers running keeps the pane alive");
    }
    host.join().unwrap();
}
