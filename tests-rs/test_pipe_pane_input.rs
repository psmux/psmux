use super::*;
use crate::pane::{self, InjectPaste, PaneInputSink, PasteInjectResult, PIPE_INPUT_BUDGET};
use std::io::{Cursor, Write};
use std::sync::{mpsc, Condvar};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);
impl Gate {
    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
    fn wait(&self) -> io::Result<()> {
        let (open, timeout) = self.0.1.wait_timeout_while(
            self.0.0.lock().unwrap(), WAIT * 3, |open| !*open,
        ).unwrap();
        if !*open && timeout.timed_out() { return Err(io::ErrorKind::TimedOut.into()); }
        Ok(())
    }
}
struct Release(Gate);
impl Drop for Release { fn drop(&mut self) { self.0.open(); } }

struct GatedWriter {
    gate: Gate,
    entered: mpsc::Sender<()>,
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: bool,
}
impl Write for GatedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self.entered.send(());
        self.gate.wait()?;
        if self.fail { return Err(io::ErrorKind::BrokenPipe.into()); }
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
fn until(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < deadline, "condition did not complete");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn queued(gate: Gate, entered: mpsc::Sender<()>, bytes: Arc<Mutex<Vec<u8>>>) -> Box<dyn PaneInputSink> {
    let injected = bytes.clone();
    pane::spawn_pane_write_queue_with(
        Box::new(GatedWriter { gate, entered, bytes, fail: false }),
        Arc::new(move |_, text, _| {
            let mut out = injected.lock().unwrap();
            out.extend_from_slice(b"\x1b[200~");
            out.extend_from_slice(text.as_bytes());
            out.extend_from_slice(b"\x1b[201~");
            PasteInjectResult { consumed: text.len() + 12, complete: true }
        }),
    )
}

#[test]
fn pipe_budget_includes_inflight_until_actual_write() {
    assert_eq!(PIPE_INPUT_BUDGET, 64 * 1024);
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let (tx, rx) = mpsc::channel();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut writer = queued(gate.clone(), tx, bytes.clone());
    assert!(matches!(writer.try_queue_pipe(vec![0; PIPE_INPUT_BUDGET + 1]), Err(TrySendError::Full(_))));
    writer.try_queue_pipe(vec![b'a'; PIPE_INPUT_BUDGET]).unwrap();
    rx.recv_timeout(WAIT).unwrap(); // The entire budget has left the queue.
    let extra = vec![b'b'];
    assert!(matches!(writer.try_queue_pipe(extra.clone()), Err(TrySendError::Full(b)) if b == extra));
    // Interactive acceptance remains unbounded and nonblocking while pipe-full.
    writer.write_all(b"key").unwrap();
    gate.open();
    until(|| bytes.lock().unwrap().len() == PIPE_INPUT_BUDGET + 3);
    writer.try_queue_pipe(extra).unwrap();
    until(|| bytes.lock().unwrap().len() == PIPE_INPUT_BUDGET + 4);
    let out = bytes.lock().unwrap();
    assert_eq!(&out[..PIPE_INPUT_BUDGET], &vec![b'a'; PIPE_INPUT_BUDGET]);
    assert_eq!(&out[PIPE_INPUT_BUDGET..], b"keyb");
}

#[test]
fn full_pump_keeps_one_chunk_stops_reads_and_resumes_fifo_with_paste() {
    assert_eq!(PIPE_INPUT_BUDGET % INPUT_CHUNK, 0);
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let (tx, rx) = mpsc::channel();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut writer = queued(gate.clone(), tx, bytes.clone());
    let first = vec![b'a'; INPUT_CHUNK];
    writer.try_queue_pipe(first.clone()).unwrap();
    rx.recv_timeout(WAIT).unwrap();
    writer.write_all(b"key").unwrap();
    writer.queue_inject_paste(InjectPaste { pid: 0, text: "paste".into(), normalize: false })
        .unwrap_or_else(|_| panic!("paste refused"));
    let rest = vec![b'b'; PIPE_INPUT_BUDGET - INPUT_CHUNK];
    for chunk in rest.chunks(INPUT_CHUNK) { writer.try_queue_pipe(chunk.to_vec()).unwrap(); }
    let tail: Vec<u8> = (0..INPUT_CHUNK * 5 + 17).map(|i| (i % 251) as u8).collect();
    let mut source = Cursor::new(tail.clone());
    let mut pending = None;
    assert!(pump_input(&mut source, &mut pending, &mut *writer, |_| Ok(true)).unwrap());
    assert_eq!(source.position(), INPUT_CHUNK as u64);
    assert_eq!(pending.as_ref().unwrap(), &tail[..INPUT_CHUNK]);
    for _ in 0..20 {
        assert!(pump_input(&mut source, &mut pending, &mut *writer, |_| panic!("readiness checked while full")).unwrap());
        assert_eq!(source.position(), INPUT_CHUNK as u64);
    }
    gate.open();
    until(|| !pump_input(&mut source, &mut pending, &mut *writer, |_| Ok(true)).unwrap());
    assert!(pending.is_none());
    let mut expected = first;
    expected.extend_from_slice(b"key\x1b[200~paste\x1b[201~");
    expected.extend(rest);
    expected.extend(tail);
    until(|| bytes.lock().unwrap().len() == expected.len());
    assert_eq!(*bytes.lock().unwrap(), expected);
}

struct Broken(mpsc::Sender<()>);
impl Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> { Err(io::ErrorKind::BrokenPipe.into()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl Drop for Broken { fn drop(&mut self) { let _ = self.0.send(()); } }

#[test]
fn failed_writer_stops_quiet_pipe_but_holds_inner_until_pane_drop() {
    for pipe in [false, true] {
        let (tx, rx) = mpsc::channel();
        let mut writer = pane::spawn_pane_write_queue(Box::new(Broken(tx)));
        if pipe { writer.try_queue_pipe(b"fail".to_vec()).unwrap(); }
        else { writer.write_all(b"fail").unwrap(); }
        until(|| matches!(writer.try_queue_pipe(Vec::new()), Err(TrySendError::Disconnected(_))));
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let mut source = Cursor::new(b"never read");
        assert_eq!(pump_input(&mut source, &mut None, &mut *writer, |_| panic!("broken pipe read")).unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        assert_eq!(source.position(), 0);
        // Ordinary keys retain their accepted/discarded behavior after failure.
        writer.write_all(b"still accepted").unwrap();
        drop(writer);
        rx.recv_timeout(WAIT).unwrap();
    }
}

#[test]
fn input_readiness_and_eof_drain_exact_bytes() {
    let gate = Gate::default(); gate.open();
    let (tx, _) = mpsc::channel();
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let mut writer = queued(gate, tx, bytes.clone());
    let expected = vec![0, 255, b'\r', b'\n', 27, b'X'];
    let mut source = Cursor::new(expected.clone());
    let mut pending = None;
    assert!(pump_input(&mut source, &mut pending, &mut *writer, |_| Ok(false)).unwrap());
    assert_eq!(source.position(), 0);
    assert!(pump_input(&mut source, &mut pending, &mut *writer, |_| Ok(true)).unwrap());
    assert!(!pump_input(&mut source, &mut pending, &mut *writer, |_| Ok(true)).unwrap());
    until(|| bytes.lock().unwrap().len() == expected.len());
    assert_eq!(*bytes.lock().unwrap(), expected);
}

struct Recorded(Arc<Mutex<Vec<u8>>>);
impl Write for Recorded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[test]
fn blocked_tee_stop_and_replacement_do_not_wait_or_remove_new_owner() {
    let gate = Gate::default();
    let _release = Release(gate.clone());
    let (entered_tx, entered_rx) = mpsc::channel();
    let old: PipeWriter = Arc::new(Mutex::new(Box::new(GatedWriter {
        gate: gate.clone(), entered: entered_tx, bytes: Arc::default(), fail: true,
    })));
    let registry = Arc::new(Mutex::new(vec![(1, old.clone())]));
    let count = Arc::new(AtomicUsize::new(1));
    let (r, c) = (registry.clone(), count.clone());
    let tee = std::thread::spawn(move || tee_output(&r, &c, 1, b"blocked"));
    entered_rx.recv_timeout(WAIT).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let other = Arc::new(Mutex::new(Vec::new()));
    let replacement: PipeWriter = Arc::new(Mutex::new(Box::new(Recorded(captured.clone()))));
    let (r, c, new, other_bytes) = (registry.clone(), count.clone(), replacement.clone(), other.clone());
    let stop = std::thread::spawn(move || {
        remove_writer(&r, &c, &old);
        r.lock().unwrap().push((1, new));
        r.lock().unwrap().push((2, Arc::new(Mutex::new(Box::new(Recorded(other_bytes))))));
        c.fetch_add(2, Ordering::Relaxed);
        tee_output(&r, &c, 2, b"other pane");
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(WAIT).expect("stop/replacement blocked behind tee");
    gate.open();
    stop.join().unwrap(); tee.join().unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 2);
    assert_eq!(&*other.lock().unwrap(), b"other pane");
    assert!(registry.lock().unwrap().iter().any(|(id, w)| *id == 1 && Arc::ptr_eq(w, &replacement)));
    let raw = [0, 255, b'\r', b'\n', 27];
    tee_output(&registry, &count, 1, &raw);
    assert_eq!(&*captured.lock().unwrap(), &raw);
}

struct Eof(mpsc::Sender<()>);
impl Write for Eof {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> { Ok(bytes.len()) }
    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}
impl Drop for Eof { fn drop(&mut self) { let _ = self.0.send(()); } }

#[test]
fn reader_eof_closes_output_before_pipe_state_is_reaped() {
    let (tx, rx) = mpsc::channel();
    let sink: PipeWriter = Arc::new(Mutex::new(Box::new(Eof(tx))));
    let state = crate::types::PipePaneState {
        pane_id: 0, process: None, input: None, pending: None,
        output: Some(Arc::downgrade(&sink)),
    };
    let mut registry = vec![(0, sink)];
    assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    registry.clear(); // The reader has written its tail and unregisters at EOF.
    rx.recv_timeout(WAIT).expect("state kept child stdin open after reader EOF");
    drop(state); // Expired weak owner: no global registry access.
}

#[test]
fn leading_options_leave_command_flags_and_target_opaque() {
    for (args, expected) in [
        (vec!["cat > file"], (false, true, false, "cat > file")),
        (vec!["-I", "producer"], (true, false, false, "producer")),
        (vec!["-IO", "-o", "pwsh", "-I", "-O", "-o", "-t", "own"], (true, true, true, "pwsh -I -O -o -t own")),
        (vec!["--", "-I"], (false, true, false, "-I")),
        (vec!["-O"], (false, true, false, "")),
    ] {
        let got = options(&args);
        assert_eq!(got, (expected.0, expected.1, expected.2, expected.3.into()));
    }
    for command in ["pipe-pane", "pipep"] {
        let args = ["-IO", "-t", "%1", "pwsh", "-t", "own"];
        assert_eq!(crate::cli::outer_target_scan_end(command, &args), 3);
    }
}
