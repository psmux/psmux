//! The attached client's incoming line queue (socket reader to main loop).
//!
//! The socket reader thread reads one line per server message and hands it to
//! the main loop, which drains everything queued on each iteration and keeps
//! only the LAST frame it saw: frames are full snapshots, so an older one has
//! nothing a newer one lacks, except the one shot clipboard write and bell it
//! may carry, which move into the newer frame (frame_oneshot.rs). Directives (`SWITCH`, `DETACH`,
//! `DETACH-KILL-PARENT`) and `NC` are acted on one by one, in order.
//!
//! A plain unbounded `mpsc::channel` kept every frame until the main loop got
//! to it. While the main loop is stuck writing to a terminal that stopped
//! reading (a conhost selection, a stalled SSH pipe, a hung terminal), that is
//! every frame the server pushes: against a flooding pane the client grew by
//! about 330 MiB a second.
//!
//! Blocking the reader instead (a bounded channel) stops the growth but moves
//! the stall onto the server, whose per client writer gives up after its 5 s
//! write timeout and drops the connection. The server then counts the client
//! as gone: `client-detached` fires, `list-clients` loses it, and a session
//! with `destroy-unattached on` is destroyed while its user is still looking
//! at it.
//!
//! So this queue does what the main loop would have done with the backlog
//! anyway, at the moment a line arrives: a frame that lands directly behind
//! another queued frame replaces it, and an `NC` behind an `NC` is dropped.
//! Anything else keeps its place, so directives stay ordered against the
//! frames around them and the main loop sees exactly what it would have seen
//! by draining the old channel, only without the superseded frames. Memory is
//! bounded by one frame plus whatever few directives are waiting, and the
//! reader keeps the socket drained, which is what tmux does for a client whose
//! tty cannot keep up: it discards the stale output and redraws from the
//! current state (`tty_block_maybe` in tty.c), and never disconnects it.

use std::collections::VecDeque;
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

/// What kind of line the server sent, as far as coalescing cares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineKind {
    /// A full state snapshot (a JSON object). Only the newest one matters.
    Frame,
    /// "Nothing changed". Two in a row mean the same as one.
    NoChange,
    /// A directive or anything unrecognised: always delivered, in order.
    Other,
}

pub(crate) fn classify(line: &str) -> LineKind {
    let t = line.trim();
    if t == "NC" {
        LineKind::NoChange
    } else if t.starts_with('{') {
        LineKind::Frame
    } else {
        LineKind::Other
    }
}

struct State {
    items: VecDeque<(LineKind, String)>,
    sender_gone: bool,
    receiver_gone: bool,
}

struct Shared {
    state: Mutex<State>,
}

/// The reader thread's end.
pub(crate) struct FrameSender {
    shared: Arc<Shared>,
}

/// The main loop's end. `try_recv` matches `mpsc::Receiver::try_recv`:
/// queued lines first, then `Disconnected` once the reader has gone.
pub(crate) struct FrameReceiver {
    shared: Arc<Shared>,
}

pub(crate) fn frame_queue() -> (FrameSender, FrameReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            items: VecDeque::new(),
            sender_gone: false,
            receiver_gone: false,
        }),
    });
    (FrameSender { shared: shared.clone() }, FrameReceiver { shared })
}

impl FrameSender {
    /// Queue a line, coalescing it with the line behind it when that loses
    /// nothing. Returns `false` once the receiver has been dropped, so the
    /// reader thread can exit, as `mpsc::Sender::send` erroring did.
    pub(crate) fn send(&self, line: String) -> bool {
        let kind = classify(&line);
        let mut st = match self.shared.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if st.receiver_gone {
            return false;
        }
        match (st.items.back_mut(), kind) {
            // The replaced frame's clipboard write and bell ride on in the
            // newer one (frame_oneshot): they happened, the screen did not.
            (Some((LineKind::Frame, last)), LineKind::Frame) => {
                let mut line = line;
                crate::frame_oneshot::supersede(last, &mut line);
                *last = line;
            }
            (Some((LineKind::NoChange, _)), LineKind::NoChange) => {}
            _ => st.items.push_back((kind, line)),
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.shared.state.lock().map(|s| s.items.len()).unwrap_or(0)
    }
}

impl FrameReceiver {
    pub(crate) fn try_recv(&self) -> Result<String, TryRecvError> {
        let mut st = match self.shared.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match st.items.pop_front() {
            Some((_, line)) => Ok(line),
            None if st.sender_gone => Err(TryRecvError::Disconnected),
            None => Err(TryRecvError::Empty),
        }
    }
}

impl Drop for FrameSender {
    fn drop(&mut self) {
        if let Ok(mut st) = self.shared.state.lock() {
            st.sender_gone = true;
        }
    }
}

impl Drop for FrameReceiver {
    fn drop(&mut self) {
        if let Ok(mut st) = self.shared.state.lock() {
            st.receiver_gone = true;
            st.items.clear();
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_client_frame_queue.rs"]
mod tests;
