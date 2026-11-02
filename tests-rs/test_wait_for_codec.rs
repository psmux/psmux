use super::*;
use std::io::Cursor;

#[test]
fn receipt_is_distinct_from_arbitrary_output() {
    for output in ["", "C", "D\0\0\0\x01C", "OK\n", "before\nC\nafter", "no final newline", "雪"] {
        let mut wire = Vec::new();
        write_output(&mut wire, output.as_bytes()).unwrap();
        assert!(read_response(&mut Cursor::new(&wire)).is_err(), "payload is not a receipt: {output:?}");
        write_complete(&mut wire, &WaitInput::default()).unwrap();
        assert_eq!(read_response(&mut Cursor::new(wire)).unwrap(), output);
    }
}

#[test]
fn every_truncation_before_receipt_is_an_error_even_after_output() {
    let mut wire = Vec::new();
    write_output(&mut wire, b"BEFORE\n").unwrap();
    write_output(&mut wire, b"AFTER\n").unwrap();
    write_complete(&mut wire, &WaitInput::default()).unwrap();
    for length in 0..wire.len() {
        assert!(read_response(&mut Cursor::new(&wire[..length])).is_err(), "length {length}");
    }
    assert_eq!(read_response(&mut Cursor::new(&wire)).unwrap(), "BEFORE\nAFTER\n");
}

#[test]
fn large_output_is_split_into_bounded_frames() {
    let output = "x".repeat(MAX_CHUNK * 2 + 1);
    let mut wire = Vec::new();
    write_output(&mut wire, output.as_bytes()).unwrap();
    let mut pos = 0;
    for expected in [MAX_CHUNK, MAX_CHUNK, 1] {
        assert_eq!(wire[pos], b'D');
        let length = u32::from_be_bytes(wire[pos + 1..pos + 5].try_into().unwrap()) as usize;
        assert_eq!(length, expected);
        pos += 5 + length;
    }
    assert_eq!(pos, wire.len());
    write_complete(&mut wire, &WaitInput::default()).unwrap();
    assert_eq!(read_response(&mut Cursor::new(wire)).unwrap(), output);
}

#[test]
fn invalid_frame_lengths_and_tags_are_rejected() {
    for length in [0u32, MAX_CHUNK as u32 + 1, u32::MAX] {
        let mut wire = vec![b'D'];
        wire.extend_from_slice(&length.to_be_bytes());
        assert_eq!(read_response(&mut Cursor::new(wire)).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
    assert_eq!(read_response(&mut Cursor::new(b"OK\n")).unwrap_err().kind(), io::ErrorKind::InvalidData);
}

#[test]
fn completed_command_errors_remain_payload_for_cli_classification() {
    let mut wire = Vec::new();
    write_output(&mut wire, b"before\nERROR: channel mutex not locked\n").unwrap();
    write_complete(&mut wire, &WaitInput::default()).unwrap();
    let reply = read_response(&mut Cursor::new(wire)).unwrap();
    assert_eq!(crate::cli::command_list_reply(&reply),
        ("before\n".to_string(), vec!["channel mutex not locked".to_string()]));
}
