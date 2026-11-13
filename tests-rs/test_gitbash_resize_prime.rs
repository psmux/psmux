// The Cygwin resize prime rides the pane's own input queue.
//
// Git bash and every Cygwin runtime program lose the first input record
// after their console is resized, so psmux follows each size change of such
// a pane with a Shift press (portable_pty CYGWIN_RESIZE_PRIME). Written
// through a second handle on the pipe, that press could land inside a paste
// or a key sequence the pane writer thread was still writing. These tests pin
// that it goes through the queue instead: behind everything queued before the
// resize, never inside it, and nowhere once the pane's writer is gone.

use super::*;

use parking_lot::{Condvar, Mutex};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PRIME: &[u8] = portable_pty::win::conpty::CYGWIN_RESIZE_PRIME;

/// A writer that blocks every write until its gate opens, recording each
/// write call separately so a split can be seen.
struct GateWriter {
    state: Arc<(Mutex<Vec<Vec<u8>>>, Condvar)>,
    open: Arc<AtomicBool>,
}

impl Write for GateWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let (lock, cv) = &*self.state;
        let mut writes = lock.lock();
        while !self.open.load(Ordering::SeqCst) {
            cv.wait(&mut writes);
        }
        writes.push(buf.to_vec());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn no_inject() -> PasteInjector {
    Arc::new(|_, _, _| PasteInjectResult { consumed: 0, complete: false })
}

fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    cond()
}

#[test]
fn resize_prime_lands_after_a_paste_queued_before_the_resize() {
    let state = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let open = Arc::new(AtomicBool::new(false));
    let mut w = spawn_pane_write_queue_with(
        Box::new(GateWriter { state: state.clone(), open: open.clone() }),
        no_inject(),
    );
    let mut hook = w.resize_prime_hook().expect("a queued writer offers the hook");

    // A bracketed paste queued as several items, larger than one coalesced
    // write, so the writer thread delivers it in more than one write call;
    // the first of them is stuck behind the gate when the resize happens.
    let body = vec![b'x'; WRITE_COALESCE_MAX + 4096];
    w.write_all(b"\x1b[200~").unwrap();
    w.write_all(&body[..WRITE_COALESCE_MAX]).unwrap();
    w.write_all(&body[WRITE_COALESCE_MAX..]).unwrap();
    w.write_all(b"\x1b[201~").unwrap();
    hook();
    w.write_all(b"echo after\r").unwrap();

    open.store(true, Ordering::SeqCst);
    state.1.notify_all();
    let mut expect = Vec::new();
    expect.extend_from_slice(b"\x1b[200~");
    expect.extend_from_slice(&body);
    expect.extend_from_slice(b"\x1b[201~");
    expect.extend_from_slice(PRIME);
    expect.extend_from_slice(b"echo after\r");
    assert!(
        wait_for(|| state.0.lock().iter().map(|v| v.len()).sum::<usize>() >= expect.len()),
        "the queue did not drain"
    );
    let writes = state.0.lock();
    assert!(writes.len() > 1, "the paste should take more than one write call");
    let got: Vec<u8> = writes.concat();
    assert_eq!(got.len(), expect.len());
    assert!(got == expect, "the prime must follow the whole paste, before the later keys");
}

#[test]
fn resize_prime_is_dropped_once_the_pane_writer_is_gone() {
    let state = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let open = Arc::new(AtomicBool::new(true));
    let w = spawn_pane_write_queue_with(
        Box::new(GateWriter { state: state.clone(), open }),
        no_inject(),
    );
    let mut hook = w.resize_prime_hook().unwrap();
    drop(w);
    hook();
    std::thread::sleep(Duration::from_millis(100));
    assert!(state.0.lock().is_empty(), "nothing may reach a pane whose writer was dropped");
}

/// Writers that cannot keep the prime in order offer no hook, so a pane
/// built on one is never primed.
#[test]
fn writer_without_a_queue_offers_no_hook() {
    let sink = std::io::sink();
    assert!(PaneInputSink::resize_prime_hook(&sink).is_none());
}
