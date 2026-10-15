// Issue #809: the registry wide listings (list-sessions, list-windows -a,
// list-panes -a) printed a refusal as a row and a timed out read as an empty
// row. read_registry_reply classifies the raw reply; these cases need no server.
use super::*;
use std::io::{self, BufReader, Read};

fn classify(bytes: &[u8]) -> RegistryReply {
    read_registry_reply(&mut BufReader::new(bytes))
}

/// A reader that hands out its bytes and then fails like a read timeout.
struct ThenTimeout(Vec<u8>);
impl Read for ThenTimeout {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.0.is_empty() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
        }
        let n = buf.len().min(self.0.len());
        buf[..n].copy_from_slice(&self.0[..n]);
        self.0.drain(..n);
        Ok(n)
    }
}

#[test]
fn reply_after_the_ack_is_a_line() {
    assert_eq!(classify(b"OK\nalpha\n"), RegistryReply::Line("alpha\n".into()));
}

#[test]
fn empty_expansion_after_the_ack_is_still_a_row() {
    // tmux prints an empty line for a format that expands to nothing.
    assert_eq!(classify(b"OK\n\n"), RegistryReply::Line("\n".into()));
}

#[test]
fn payload_starting_with_error_after_the_ack_is_data() {
    assert_eq!(classify(b"OK\nERROR: x\n"), RegistryReply::Line("ERROR: x\n".into()));
}

#[test]
fn invalid_key_refusal_is_refused() {
    assert_eq!(classify(b"ERROR: Invalid session key\n"), RegistryReply::Refused("Invalid session key".into()));
}

#[test]
fn auth_required_refusal_is_refused() {
    assert_eq!(classify(b"ERROR: Authentication required\n"), RegistryReply::Refused("Authentication required".into()));
}

#[test]
fn any_reply_without_the_ack_is_refused() {
    assert_eq!(classify(b"ERROR: something new\n"), RegistryReply::Refused("something new".into()));
}

#[test]
fn closed_without_a_reply_is_no_reply() {
    assert_eq!(classify(b""), RegistryReply::NoReply);
    assert_eq!(classify(b"OK\n"), RegistryReply::NoReply);
}

#[test]
fn timeout_before_or_after_the_ack_is_no_reply() {
    assert_eq!(read_registry_reply(&mut BufReader::new(ThenTimeout(vec![]))), RegistryReply::NoReply);
    assert_eq!(read_registry_reply(&mut BufReader::new(ThenTimeout(b"OK\n".to_vec()))), RegistryReply::NoReply);
}
