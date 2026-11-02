use super::*;
use std::io::{BufReader, Read};

// Script the input edge only. WaitInput and its actual reply channel are real.
enum Step {
    Bytes(&'static [u8]),
    Timeout,
    Complete,
    EofWithReply,
}

struct Input {
    steps: VecDeque<Step>,
    reply: Sender<Result<(), String>>,
}

impl Read for Input {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.steps.pop_front().expect("unexpected extra read") {
            Step::Bytes(bytes) => {
                let n = bytes.len().min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]);
                if n < bytes.len() { self.steps.push_front(Step::Bytes(&bytes[n..])); }
                Ok(n)
            }
            Step::Timeout => Err(io::ErrorKind::TimedOut.into()),
            Step::Complete => {
                self.reply.send(Ok(())).unwrap();
                Err(io::ErrorKind::TimedOut.into())
            }
            Step::EofWithReply => {
                self.reply.send(Ok(())).unwrap();
                Ok(0)
            }
        }
    }
}

fn input(steps: Vec<Step>) -> (BufReader<Input>, mpsc::Receiver<Result<(), String>>) {
    let (tx, rx) = mpsc::channel();
    (
        BufReader::new(Input {
            steps: steps.into(),
            reply: tx,
        }),
        rx,
    )
}

fn retry(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::TimedOut
}

#[test]
fn incoming_lines_and_partial_input_survive_a_suspended_wait() {
    let (mut r, reply) = input(vec![
        Step::Bytes(b"display-message -p first\ndisplay-message -p par"),
        Step::Timeout,
        Step::Complete,
        Step::Bytes(b"tial\n"),
    ]);
    let mut waiting = WaitInput::default();
    assert!(waiting.lines.is_empty());
    assert!(waiting.partial.is_empty());
    assert_eq!(waiting.wait(&mut r, reply, retry), Some(Ok(())));
    let mut line = String::new();
    assert_eq!(waiting.read_line(&mut r, &mut line).unwrap(), 25);
    assert_eq!(line, "display-message -p first\n");
    line.clear();
    waiting.read_line(&mut r, &mut line).unwrap();
    assert_eq!(line, "display-message -p partial\n");
}

#[test]
fn completion_receipt_rejects_admitted_wait_input() {
    for (suffix, queued, partial) in [
        (b"display-message -p unfinished".as_slice(), 0, true),
        (b"display-message -p extra\nwait-for other\n".as_slice(), 2, false),
    ] {
        let (mut reader, reply) = input(vec![Step::Bytes(suffix), Step::Complete]);
        let mut waiting = WaitInput::default();
        assert!(waiting.is_empty());
        assert_eq!(waiting.wait(&mut reader, reply, retry), Some(Ok(())));
        assert!(!waiting.failed, "the wait completed, but its admitted suffix remains");
        assert_eq!(waiting.lines.len(), queued);
        assert_eq!(!waiting.partial.is_empty(), partial);
        assert!(!waiting.is_empty());

        // Model the reply-producing command after the wait taking its local
        // exit while the suffix remains. Payload cannot stand in for a receipt.
        let mut wire = Vec::new();
        write_output(&mut wire, b"DONE\n").unwrap();
        let before = wire.clone();
        assert_eq!(write_complete(&mut wire, &waiting).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(wire, before, "no receipt may acknowledge discarded input");
        assert_eq!(read_response(&mut io::Cursor::new(wire)).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }
}

#[test]
fn completion_receipt_accepts_completed_wait_without_suffix() {
    let (mut reader, reply) = input(vec![Step::Complete]);
    let mut waiting = WaitInput::default();
    assert_eq!(waiting.wait(&mut reader, reply, retry), Some(Ok(())));
    assert!(!waiting.failed);
    assert!(waiting.is_empty());
    let mut wire = Vec::new();
    write_output(&mut wire, b"DONE\n").unwrap();
    write_complete(&mut wire, &waiting).unwrap();
    assert_eq!(read_response(&mut io::Cursor::new(wire)).unwrap(), "DONE\n");
}

#[test]
fn read_budget_expiry_preserves_partial_input_outside_wait() {
    let (mut r, _reply) = input(vec![
        Step::Bytes(b"display-mess"),
        Step::Timeout,
        Step::Bytes(b"age -p ok\n"),
    ]);
    let mut waiting = WaitInput::default();
    let mut line = String::new();
    assert_eq!(
        waiting.read_line(&mut r, &mut line).unwrap_err().kind(),
        io::ErrorKind::TimedOut
    );
    assert!(line.is_empty());
    assert_eq!(waiting.partial, b"display-mess");
    waiting.read_line(&mut r, &mut line).unwrap();
    assert_eq!(line, "display-message -p ok\n");
}

#[test]
fn observed_eof_cancels_even_if_a_reply_arrives_during_the_read() {
    let (mut r, reply) = input(vec![Step::EofWithReply]);
    let mut waiting = WaitInput {
        cancel_on_eof: true,
        ..WaitInput::default()
    };
    assert!(!waiting.eof);
    assert_eq!(waiting.wait(&mut r, reply, retry), None);
    assert!(waiting.eof);
    assert!(waiting.failed);
}

#[test]
fn ordinary_half_close_is_not_cancellation() {
    let (mut r, reply) = input(vec![Step::EofWithReply]);
    let mut waiting = WaitInput::default();
    assert!(!waiting.cancel_on_eof);
    assert_eq!(waiting.wait(&mut r, reply, retry), Some(Ok(())));
    assert!(waiting.eof);
    assert!(!waiting.failed);
}

#[test]
fn disconnect_is_observed_after_buffered_commands_without_executing_them() {
    let (mut r, reply) = input(vec![Step::Bytes(b"wait-for other\n"), Step::EofWithReply]);
    let mut waiting = WaitInput {
        cancel_on_eof: true,
        ..WaitInput::default()
    };
    assert!(waiting.lines.is_empty());
    assert_eq!(waiting.wait(&mut r, reply, retry), None);
    assert_eq!(waiting.lines.front().unwrap(), "wait-for other\n");
    assert!(waiting.eof);
}

#[test]
fn dropped_reply_sender_is_not_success() {
    let (tx, reply) = mpsc::channel();
    assert_eq!(reply.try_recv(), Err(mpsc::TryRecvError::Empty));
    drop(tx);
    let mut waiting = WaitInput::default();
    assert_eq!(waiting.wait(&mut io::Cursor::new(Vec::<u8>::new()), reply, retry), None);
    assert!(waiting.failed);
}

#[test]
fn exact_input_cap_still_allows_the_wait_reply() {
    static BYTES: [u8; MAX_BUFFERED_INPUT] = [b'x'; MAX_BUFFERED_INPUT];
    let (mut reader, reply) = input(vec![Step::Bytes(&BYTES), Step::Complete]);
    let mut waiting = WaitInput::default();
    assert!(waiting.is_empty());
    assert_eq!(waiting.wait(&mut reader, reply, retry), Some(Ok(())));
    assert_eq!(waiting.partial.len(), MAX_BUFFERED_INPUT);
    assert!(!waiting.failed);
}

#[test]
fn pending_input_cap_counts_queued_and_unterminated_bytes_incrementally() {
    for prefix in [Vec::new(), b"display-message -p queued\n".to_vec()] {
        let mut bytes = prefix;
        bytes.resize(MAX_BUFFERED_INPUT + 1, b'x');
        assert!(bytes.len() > MAX_BUFFERED_INPUT);
        let mut reader = BufReader::with_capacity(257, io::Cursor::new(bytes));
        let (tx, reply) = mpsc::channel();
        let mut waiting = WaitInput { cancel_on_eof: true, ..WaitInput::default() };
        let result = waiting.wait(&mut reader, reply, retry);
        assert!(result.unwrap().unwrap_err().contains("too much input"));
        assert!(waiting.failed);
        assert_eq!(waiting.queued_bytes + waiting.partial.len(), MAX_BUFFERED_INPUT);
        assert!(tx.send(Ok(())).is_err());
    }
}
