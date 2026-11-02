use super::*;
use std::cell::Cell;
use std::io::{BufReader, Read};
use std::rc::Rc;
use std::time::Instant;

// A socket that never has input: every read is a read timeout, returned at
// once. The real socket returns it after its 10 ms budget, so each read here
// stands for one wake up of the connection thread.
struct Idle(Rc<Cell<u32>>);

impl Read for Idle {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        self.0.set(self.0.get() + 1);
        Err(io::ErrorKind::TimedOut.into())
    }
}

#[test]
fn a_pending_wait_sleeps_on_its_reply_instead_of_spinning_on_the_socket() {
    let reads = Rc::new(Cell::new(0));
    let mut r = BufReader::new(Idle(reads.clone()));
    let (tx, rx) = mpsc::channel();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        tx.send(Ok(())).unwrap();
    });
    let started = Instant::now();
    let result = WaitInput::default().wait(&mut r, rx, |e| e.kind() == io::ErrorKind::TimedOut);
    release.join().unwrap();
    assert_eq!(result, Some(Ok(())));
    // 500 ms at one look per WAIT_POLL is about 10 reads. Polling the socket
    // alone made every read timeout a wake up: thousands here, 100 a second
    // per pending wait on the real 10 ms socket budget.
    assert!(reads.get() <= 20, "{} socket reads in {:?}", reads.get(), started.elapsed());
}

#[test]
fn the_release_still_wakes_a_pending_wait_at_once() {
    let mut r = BufReader::new(Idle(Rc::new(Cell::new(0))));
    let (tx, rx) = mpsc::channel();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(120));
        let sent = Instant::now();
        tx.send(Ok(())).unwrap();
        sent
    });
    let result = WaitInput::default().wait(&mut r, rx, |e| e.kind() == io::ErrorKind::TimedOut);
    let woke = Instant::now();
    let sent = release.join().unwrap();
    assert_eq!(result, Some(Ok(())));
    assert!(woke.duration_since(sent) < Duration::from_millis(30), "woke {:?} after release", woke.duration_since(sent));
}

#[test]
fn only_a_list_that_waits_uses_the_completion_exchange() {
    let words = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    // A list that does not wait keeps the exchange an older server accepts.
    assert!(!list_waits(&[words("display-message -p A"), words("display-message -p B")]));
    assert!(!list_waits(&[words("new-window -d"), words("split-window -d")]));
    assert!(!list_waits(&[words("display-message wait-for")]));
    assert!(list_waits(&[words("display-message -p A"), words("wait-for done")]));
    assert!(list_waits(&[words("wait -L lock"), words("display-message -p A")]));
}
