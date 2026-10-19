use super::ControlConsoleReader;
use std::{collections::VecDeque, io};

fn batches(input: Vec<Vec<u16>>) -> impl FnMut(&mut [u16]) -> io::Result<usize> {
    let mut input = VecDeque::from(input);
    move |buf| {
        let batch = input.pop_front().unwrap_or_default();
        buf[..batch.len()].copy_from_slice(&batch);
        Ok(batch.len())
    }
}

#[test]
fn split_surrogate_is_not_eof() {
    let mut reader = ControlConsoleReader::default();
    assert!(reader.high_surrogate.is_none());
    let mut read = batches(vec![vec![0xD83D], vec![0xDE80, b'x' as u16]]);
    let mut buf = [0; 4096];
    let n = reader.read_with(&mut buf, &mut read).unwrap();
    assert_eq!(&buf[..n], "🚀x".as_bytes());
    assert!(reader.high_surrogate.is_none());
    assert_eq!(reader.read_with(&mut buf, &mut read).unwrap(), 0);
}

#[test]
fn raw_control_characters_are_not_eof() {
    let mut reader = ControlConsoleReader::default();
    assert!(reader.high_surrogate.is_none());
    let mut read = batches(vec![vec![0x1A], vec![0]]);
    let mut buf = [0; 4096];
    for expected in [0x1A, 0] {
        let n = reader.read_with(&mut buf, &mut read).unwrap();
        assert_eq!(&buf[..n], &[expected]);
    }
    assert_eq!(reader.read_with(&mut buf, &mut read).unwrap(), 0);
}

#[test]
fn unfinished_surrogate_at_eof_is_replaced() {
    let mut reader = ControlConsoleReader::default();
    assert!(reader.high_surrogate.is_none());
    let mut read = batches(vec![vec![0xD83D]]);
    let mut buf = [0; 4096];
    let n = reader.read_with(&mut buf, &mut read).unwrap();
    assert_eq!(&buf[..n], "�".as_bytes());
    assert!(reader.high_surrogate.is_none());
    assert_eq!(reader.read_with(&mut buf, &mut read).unwrap(), 0);
}

#[test]
fn pending_surrogate_is_carried_across_returned_text() {
    let mut reader = ControlConsoleReader::default();
    assert!(reader.high_surrogate.is_none());
    let mut read = batches(vec![vec![b'a' as u16, 0xD83D], vec![0xDE80]]);
    let mut buf = [0; 4096];
    let n = reader.read_with(&mut buf, &mut read).unwrap();
    assert_eq!(&buf[..n], b"a");
    assert_eq!(reader.high_surrogate, Some(0xD83D));
    let n = reader.read_with(&mut buf, &mut read).unwrap();
    assert_eq!(&buf[..n], "🚀".as_bytes());
    assert!(reader.high_surrogate.is_none());
}

#[test]
fn maximum_utf8_expansion_fits_the_output_buffer() {
    let mut reader = ControlConsoleReader::default();
    let input = vec![0x0800; 4096 / 3];
    assert_eq!(char::from_u32(input[0] as u32).unwrap().len_utf8(), 3);
    let expected = String::from_utf16(&input).unwrap();
    assert!(expected.len() <= 4096);
    let mut read = batches(vec![input]);
    let mut buf = [0; 4096];
    let n = reader.read_with(&mut buf, &mut read).unwrap();
    assert_eq!(&buf[..n], expected.as_bytes());
}

#[test]
fn console_errors_propagate() {
    let mut reader = ControlConsoleReader::default();
    let mut buf = [0; 4096];
    let err = reader
        .read_with(&mut buf, |_| {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        })
        .unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Interrupted);
}
