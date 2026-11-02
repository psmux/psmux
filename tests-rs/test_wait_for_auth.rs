use super::{negotiate, AUTH_CAPABILITY, NEGOTIATION_ACK};
use std::io::{self, Cursor, Write};

#[test]
fn capability_ack_is_required_before_any_command() {
    let mut wire = Vec::new();
    negotiate(&mut Cursor::new(NEGOTIATION_ACK), &mut wire, "key").unwrap();
    assert_eq!(wire, format!("AUTH key {}\n", AUTH_CAPABILITY).as_bytes());
    for reply in ["OK\n", "OK\r\n", "ERROR: Invalid session key\n", "1\n"] {
        let mut wire = Vec::new();
        assert_eq!(negotiate(&mut Cursor::new(reply), &mut wire, "key").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied);
        assert_eq!(wire, format!("AUTH key {}\n", AUTH_CAPABILITY).as_bytes());
    }
}

#[test]
fn incomplete_or_oversized_auth_reply_fails() {
    for reply in ["".to_string(), "OK".to_string(), NEGOTIATION_ACK.trim_end().to_string(),
        "x".repeat(128) + "\n"] {
        let mut reader = Cursor::new(reply);
        assert_eq!(negotiate(&mut reader, &mut Vec::new(), "key").unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof);
        assert!(reader.position() <= 128);
    }
}

#[test]
fn negotiation_io_failure_is_not_compatibility_fallback() {
    struct Refuse;
    impl Write for Refuse {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> { Ok(()) }
    }
    let mut reader = Cursor::new(NEGOTIATION_ACK);
    assert_eq!(negotiate(&mut reader, &mut Refuse, "key").unwrap_err().kind(),
        io::ErrorKind::BrokenPipe);
    assert_eq!(reader.position(), 0);
}
