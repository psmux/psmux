use super::{negotiate, output_text, read_reply, write_reply, AUTH_CAPABILITY, NEGOTIATION_ACK};
use std::process::{ExitStatus, Output};

fn output(code: u32, stdout: &[u8], stderr: &[u8]) -> Output {
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    let status = ExitStatus::from_raw(code);
    #[cfg(unix)]
    let status = ExitStatus::from_raw((code as i32) << 8);
    Output { status, stdout: stdout.to_vec(), stderr: stderr.to_vec() }
}

#[test]
fn nonzero_status_is_displayed_even_without_output() {
    for code in [1, 7, 255] {
        let out = output(code, b"", b"");
        assert!(!out.status.success());
        assert_eq!(output_text("exit failure", &out), format!("'exit failure' returned {}\n", code));
    }
}

#[test]
fn status_follows_complete_or_partial_output() {
    for bytes in [b"hello".as_slice(), b"hello\n".as_slice()] {
        let out = output(7, bytes, b"");
        assert!(!out.stdout.is_empty());
        assert_eq!(output_text("echo hello; exit 7", &out), "hello\n'echo hello; exit 7' returned 7\n");
    }
}

#[test]
fn successful_ui_output_and_stderr_default_are_preserved() {
    for (stdout, stderr, expected) in [
        ("", "", ""), ("ok", "", "ok"), ("ok\n", "", "ok\n"),
        ("", "err", "err"), ("ok", "err", "ok\nerr"),
        ("ok\n", "err\n", "ok\nerr\n"),
    ] {
        let out = output(0, stdout.as_bytes(), stderr.as_bytes());
        assert!(out.status.success());
        assert_eq!(output_text("command", &out), expected);
    }
}

#[test]
fn negotiation_requires_native_acknowledgement() {
    for reply in [
        "ERROR: Invalid session key\n", "ERROR: Invalid session key\r\n",
        "OK\n", "OK\r\n", "", "\u{0001}PSMUX_RUN_SHELL_NATIVE_V1\u{0001}",
        "ERROR: Authentication required\n", "unexpected reply\n",
    ] {
        let mut reader = std::io::Cursor::new(reply.as_bytes());
        let mut written = Vec::new();
        assert!(negotiate(&mut reader, &mut written, "key").is_err(), "accepted {reply:?}");
        assert_eq!(written, format!("AUTH key {}\n", AUTH_CAPABILITY).as_bytes());
    }
    let mut reader = std::io::Cursor::new(NEGOTIATION_ACK.as_bytes());
    assert!(negotiate(&mut reader, &mut Vec::new(), "key").is_ok());
}

#[test]
fn negotiation_write_failure_prevents_read() {
    struct Refuse;
    impl std::io::Write for Refuse {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::Error::other("refused")) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let mut reader = std::io::Cursor::new(NEGOTIATION_ACK.as_bytes());
    assert!(negotiate(&mut reader, &mut Refuse, "key").is_err());
    assert_eq!(reader.position(), 0);
}

#[test]
fn native_reply_preserves_all_bytes_and_channel_identity() {
    let all_bytes: Vec<u8> = (0..=255).collect();
    for code in [0, 7, -1] {
        for (stdout, stderr) in [
            (all_bytes.as_slice(), b"RS1 0 0 0\n\x01RUN_SHELL_EXIT\x0199\n".as_slice()),
            (b"".as_slice(), all_bytes.as_slice()), (b"".as_slice(), b"".as_slice()),
        ] {
            let mut wire = Vec::new();
            write_reply(&mut wire, code, stdout, stderr).unwrap();
            let reply = read_reply(&mut std::io::Cursor::new(&wire)).unwrap();
            assert_eq!(reply.code, code);
            assert_eq!(reply.stdout, stdout);
            assert_eq!(reply.stderr, stderr);
        }
    }
}

#[test]
fn submitted_native_reply_must_be_complete_and_framed() {
    let mut wire = Vec::new();
    write_reply(&mut wire, 7, b"hello", b"stderr").unwrap();
    for end in 0..wire.len() {
        assert!(read_reply(&mut std::io::Cursor::new(&wire[..end])).is_err(), "truncated at {end}");
    }
    for bad in [b"RS1 bad 0 0\n".as_slice(), b"RS1 0 -1 0\n".as_slice(), b"RS1 0 18446744073709551615 0\n".as_slice(), b"RS1 0 0 0\nextra".as_slice()] {
        assert!(read_reply(&mut std::io::Cursor::new(bad)).is_err());
    }
    assert_eq!(read_reply(&mut std::io::Cursor::new(&wire)).unwrap().code, 7);
}

#[cfg(unix)]
#[test]
fn signal_termination_is_displayed() {
    use std::os::unix::process::ExitStatusExt;
    let out = Output { status: ExitStatus::from_raw(15), stdout: vec![], stderr: vec![] };
    assert!(!out.status.success());
    assert_eq!(output_text("command", &out), "'command' terminated by signal 15\n");
}
