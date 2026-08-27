// Issue #605: `psmux attach` printed a raw winsock error.
//
//   > ta
//   psmux: No connection could be made because the target machine actively
//   refused it. (os error 10061)                                took 4s
//
// The registry named a server (`<data dir>/<session>.port`) that was no longer
// listening. Two things went wrong at once:
//
//  1. The CLI attach gate asked `probe_session_alive`, whose connect arm read
//     `Err(e) if e.kind() == ConnectionRefused => false, Err(_) => true`. On
//     Windows a connect to an UNBOUND loopback port is not answered promptly
//     with WSAECONNREFUSED: the SYN is dropped and retransmitted, so the
//     refusal takes roughly two seconds to surface. The gate's 500ms probe
//     therefore saw a plain timeout and voted "alive" for a server that did not
//     exist. `session::probe_session_liveness` already documents the correct
//     rule for the same situation: on loopback ANY connect failure means
//     nothing usable is listening.
//
//  2. Having passed the gate, the client connected for real and let the
//     resulting `std::io::Error` propagate out of `run_remote`, where `main`
//     prints every escaping error verbatim as `psmux: <display>`. Whatever
//     winsock said became the user-visible message: 10061 on the reporter's
//     Windows Server 2022 box, `connection timed out` where the retransmit
//     window lands a few milliseconds the other side of the client's budget.
//
// tmux 3.4, measured against a server killed with its socket left behind:
//
//     tmux -L parity605 attach            -> "no sessions"                rc 1, 12ms
//     tmux -L parity605 attach -t p1      -> "no sessions"                rc 1, 25ms
//     tmux -L parity605 has-session -t p1 -> "no server running on <path>" rc 1,  5ms
//
// Never an OS error, never a multi-second stall. These tests pin the message
// psmux now produces on that path and the strictness of the gate that leads to
// it. The end to end proof lives in tests/test_issue605_stale_port_attach.ps1.

use super::*;

/// The exact tmux wording, and an `io::ErrorKind` that says "gone" rather than
/// leaking a transport-level kind that a caller might retry.
#[test]
fn no_such_session_uses_tmux_wording() {
    let _guard = crate::util::lock_test_env();
    std::env::remove_var("PSMUX_SESSION_DISPLAY_NAME");

    let e = no_such_session("work");
    assert_eq!(e.kind(), io::ErrorKind::NotFound);
    assert_eq!(e.to_string(), "can't find session: work");
}

/// `main` prints `psmux: {e}`, so the rendered line must be the whole message
/// a tmux user expects and must not carry any operating system vocabulary.
#[test]
fn no_such_session_carries_no_os_error_text() {
    let _guard = crate::util::lock_test_env();
    std::env::remove_var("PSMUX_SESSION_DISPLAY_NAME");

    let rendered = format!("psmux: {}", no_such_session("dev"));
    assert_eq!(rendered, "psmux: can't find session: dev");

    for leak in [
        "os error",
        "10061",
        "10060",
        "actively refused",
        "target machine",
        "connection timed out",
        "ConnectionRefused",
    ] {
        assert!(
            !rendered.contains(leak),
            "raw transport wording {leak:?} leaked into the user-facing line: {rendered}"
        );
    }
}

/// A `-L` namespace is stored on disk as `<ns>__<session>`. The attach gate
/// publishes the spelling the user typed; the client message must use it, not
/// the internal name.
#[test]
fn no_such_session_prefers_the_display_name() {
    let _guard = crate::util::lock_test_env();

    std::env::set_var("PSMUX_SESSION_DISPLAY_NAME", "dev");
    let e = no_such_session("myns__dev");
    assert_eq!(e.to_string(), "can't find session: dev");

    // An empty override is not an override: fall back to the real name rather
    // than printing `can't find session: ` with nothing after the colon.
    std::env::set_var("PSMUX_SESSION_DISPLAY_NAME", "");
    let e = no_such_session("myns__dev");
    assert_eq!(e.to_string(), "can't find session: myns__dev");

    std::env::remove_var("PSMUX_SESSION_DISPLAY_NAME");
    let e = no_such_session("plain");
    assert_eq!(e.to_string(), "can't find session: plain");
}

/// The first attach must not sit through a Windows SYN retransmit before it can
/// say the session is gone. A live loopback server completes the handshake in
/// about a millisecond, so the budget only has to beat the retransmit, and the
/// reconnect budget must stay the longer of the two.
#[test]
fn attach_connect_budget_is_shorter_than_the_loopback_retransmit() {
    // Measured on Windows 11 and Windows Server 2022: a connect to an unbound
    // loopback port surfaces its refusal at about 2.04s.
    let observed_refusal = Duration::from_millis(2000);
    assert!(
        ATTACH_CONNECT_TIMEOUT < observed_refusal,
        "the attach budget ({ATTACH_CONNECT_TIMEOUT:?}) must expire before the \
         retransmit window, or a dead port stalls the attach for seconds"
    );
    assert!(
        ATTACH_CONNECT_TIMEOUT >= Duration::from_millis(250),
        "too tight: a loaded machine could miss a live server's handshake"
    );
    assert!(
        RECONNECT_CONNECT_TIMEOUT >= ATTACH_CONNECT_TIMEOUT,
        "a reconnect knows the server existed a moment ago and may be busy, so \
         it must not be the stricter of the two"
    );
}
