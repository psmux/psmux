//! A pane program's clipboard query, `OSC 52 ; selection ; ?`, answered as
//! tmux's `input_osc_52_reply` answers it (input.c:3341 at tag 3.8) from what
//! the server has: the newest automatic paste buffer, or the system clipboard
//! when `get-clipboard` is `request` or `both`.
//!
//! The pane's reader thread makes the other replies of a batch, but cannot see
//! the paste buffers or the options, so a batch holding a clipboard query
//! comes here whole. The server loop puts the clipboard's reply in, or leaves
//! it out, and delivers the batch with one injection, in the order asked
//! (#930).
//!
//! A system clipboard another window holds open (#428) is read on a thread
//! for up to a second, as a paste of it waits (`PASTE_GESTURE_TIMEOUT`), and
//! the pane's later batches wait behind it, so the server loop never waits
//! and the replies keep their order. tmux likewise answers when its terminal
//! does (`tty_clipboard_query`, tty.c:3207 at tag 3.8), but its 5 s covers a
//! round trip to the terminal; here the pane's other replies wait too, as
//! keys wait behind a paste, so the wait is the paste's.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::clipboard::ClipboardBusy;

use crate::pane::{PaneReply, ReplyFallback};
use crate::types::AppState;

/// The values `get-clipboard` takes (options-table.c:87 at tag 3.8).
pub(crate) const GET_CLIPBOARD_CHOICES: &[&str] = &["off", "buffer", "request", "both"];

/// Batches a pane may have waiting at once. A program that asks faster than
/// the server loop answers loses the rest, logged, rather than growing this
/// without bound.
const MAX_WAITING: usize = 64;

/// One batch of a pane's replies, holding a clipboard reply or queued behind
/// one of the same pane's that does.
pub(crate) struct Batch {
    pub pane_id: usize,
    pub child_pid: Option<u32>,
    pub replies: Vec<PaneReply>,
    pub color_query_pending: Arc<AtomicU32>,
    /// The read of a busy clipboard this batch waits for.
    read: Option<Read>,
}

impl Batch {
    pub(crate) fn new(pane_id: usize, child_pid: Option<u32>, replies: Vec<PaneReply>, color_query_pending: Arc<AtomicU32>) -> Self {
        Self { pane_id, child_pid, replies, color_query_pending, read: None }
    }
}

/// A read of the system clipboard on its own thread, waiting for the window
/// that holds it open, as `commands::clipboard_text_or_wait` waits for a paste.
struct Read {
    rx: Receiver<Option<String>>,
    deadline: Instant,
}

impl Read {
    fn start(read: impl FnOnce() -> Option<String> + Send + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read());
            crate::types::wake_server_loop();
        });
        let deadline = Instant::now() + crate::clipboard::PASTE_GESTURE_TIMEOUT + Duration::from_millis(250);
        Self { rx, deadline }
    }

    /// What it read, `Some(None)` when it gave up, or `None` while it waits.
    fn poll(&self) -> Option<Option<String>> {
        match self.rx.try_recv() {
            Ok(text) => Some(text),
            Err(TryRecvError::Empty) if Instant::now() < self.deadline => None,
            Err(_) => Some(None),
        }
    }
}

static WAITING: Mutex<Vec<Batch>> = Mutex::new(Vec::new());
static ANY_WAITING: AtomicBool = AtomicBool::new(false);

/// Whether `replies` hold a clipboard reply, so the batch has to come here.
pub(crate) fn wanted(replies: &[PaneReply]) -> bool {
    replies.iter().any(|r| matches!(r.fallback, ReplyFallback::Clipboard { .. }))
}

/// Whether a batch of `pane_id`'s waits here, so its next one has to queue
/// behind it. One atomic load while none waits.
pub(crate) fn waits(pane_id: usize) -> bool {
    ANY_WAITING.load(Ordering::Acquire)
        && WAITING.lock().is_ok_and(|waiting| waiting.iter().any(|b| b.pane_id == pane_id))
}

/// Hand a batch to the server loop. The caller wakes it.
pub(crate) fn queue(batch: Batch) {
    let Ok(mut waiting) = WAITING.lock() else { return };
    if waiting.iter().filter(|b| b.pane_id == batch.pane_id).count() >= MAX_WAITING {
        crate::platform::mouse_inject::log_lost_reply(batch.child_pid, "OSC 52", 0);
        return;
    }
    waiting.push(batch);
    ANY_WAITING.store(true, Ordering::Release);
}

/// What a clipboard query gets.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The reply's data, `None` for no reply.
    Now(Option<String>),
    /// Another window holds the system clipboard open: read it on a thread.
    Busy,
}

/// What `OSC 52 ; ?` is answered with, `Answer::Now(None)` for no reply:
///
/// - Nothing unless `set-clipboard` is `on`, as tmux reads OSC 52 only on
///   `on` (input.c:3379). `external`, the default, gives no reply: answering
///   there would hand the user's copies to any program that prints to a
///   pane (a file being shown, a remote host over ssh) without the user
///   having opted in, which tmux never does.
/// - Nothing when `get-clipboard` is `off`.
/// - `buffer`, the default: the newest automatic buffer, none without one
///   (`paste_get_top`, which skips named buffers).
/// - `request` and `both`: the system clipboard's text, as `refresh-client -l`
///   reads it, where tmux asks the most recently used client's terminal; none
///   with no client attached, as tmux has no terminal to ask then, and none
///   when the clipboard holds no text. `both` keeps it as a new buffer too.
///   `Answer::Busy` when another window holds the clipboard open.
pub(crate) fn answer(
    app: &mut AppState,
    read_clipboard: impl FnOnce() -> Result<Option<String>, ClipboardBusy>,
) -> Answer {
    if app.set_clipboard != "on" {
        return Answer::Now(None);
    }
    match crate::server::options::get_option_value(app, "get-clipboard").as_str() {
        "buffer" => Answer::Now(app.paste_top().map(str::to_string)),
        mode @ ("request" | "both") => {
            if app.attached_clients == 0 {
                return Answer::Now(None);
            }
            let Ok(text) = read_clipboard() else { return Answer::Busy };
            let Some(text) = text.filter(|t| !t.is_empty()) else { return Answer::Now(None) };
            if mode == "both" {
                app.paste_add(text.clone());
            }
            Answer::Now(Some(text))
        }
        _ => Answer::Now(None),
    }
}

/// The reply as tmux's `input_reply_clipboard` writes it (input.c:3484):
/// `ESC ] 52 ;`, the selection if there was one, `;`, the data in base64, and
/// the end the query had.
pub(crate) fn reply(selection: u8, data: &str, bel: bool) -> String {
    let mut out = String::from("\x1b]52;");
    if selection != 0 {
        out.push(char::from(selection));
    }
    out.push(';');
    out.push_str(&crate::util::base64_encode(data));
    out.push_str(if bel { "\x07" } else { "\x1b\\" });
    out
}

/// Answer and deliver the waiting batches of the panes `owns` says are this
/// server's. A clipboard write earlier in a batch goes to the buffers first,
/// so a query after it reads it back, as tmux's in-order parser does. A batch
/// waiting for a busy clipboard stays, and so do the same pane's after it.
pub(crate) fn serve(app: &mut AppState, owns: impl Fn(&AppState, usize) -> bool) {
    if !ANY_WAITING.load(Ordering::Acquire) {
        return;
    }
    let mine: Vec<Batch> = {
        let Ok(mut waiting) = WAITING.lock() else { return };
        let (mine, rest): (Vec<Batch>, Vec<Batch>) = waiting.drain(..).partition(|b| owns(app, b.pane_id));
        *waiting = rest;
        ANY_WAITING.store(!waiting.is_empty(), Ordering::Release);
        mine
    };
    if mine.is_empty() {
        return;
    }
    crate::server::helpers::drain_osc52(app);
    let mut held: Vec<Batch> = Vec::new();
    for mut batch in mine {
        if held.iter().any(|b| b.pane_id == batch.pane_id) {
            held.push(batch);
            continue;
        }
        if wanted(&batch.replies) {
            // Every clipboard query in the batch reads the same thing.
            let data = data_for(app, &mut batch, crate::clipboard::try_read_from_system_clipboard, || {
                crate::clipboard::read_from_system_clipboard_waiting(crate::clipboard::PASTE_GESTURE_TIMEOUT)
            });
            let Some(data) = data else {
                held.push(batch);
                continue;
            };
            batch.replies.retain_mut(|r| {
                let ReplyFallback::Clipboard { selection, bel } = r.fallback else { return true };
                match &data {
                    Some(text) => {
                        r.text = reply(selection, text, bel);
                        true
                    }
                    None => false,
                }
            });
        }
        if crate::pane::deliver_replies(batch.pane_id, batch.child_pid, &batch.replies, &batch.color_query_pending) {
            crate::types::PTY_DATA_READY.store(true, Ordering::Release);
        }
    }
    if !held.is_empty() {
        // Back in front of any queued meanwhile, which came after them.
        if let Ok(mut waiting) = WAITING.lock() {
            held.append(&mut waiting);
            *waiting = held;
            ANY_WAITING.store(true, Ordering::Release);
        }
    }
}

/// The data a batch's clipboard replies carry, `Some(None)` for no reply, or
/// `None` while a read of a busy clipboard goes on: `read` tries once, and
/// when the clipboard is busy `wait_read` waits for it on a thread. The
/// options are read again when that ends, as they may have changed meanwhile.
fn data_for(
    app: &mut AppState,
    batch: &mut Batch,
    read: impl FnOnce() -> Result<Option<String>, ClipboardBusy>,
    wait_read: impl FnOnce() -> Option<String> + Send + 'static,
) -> Option<Option<String>> {
    if let Some(waiting) = &batch.read {
        let text = waiting.poll()?;
        batch.read = None;
        return match answer(app, || Ok(text)) {
            Answer::Now(data) => Some(data),
            Answer::Busy => Some(None),
        };
    }
    match answer(app, read) {
        Answer::Now(data) => Some(data),
        Answer::Busy => {
            batch.read = Some(Read::start(wait_read));
            None
        }
    }
}

/// Whether `pane_id` is a pane of `app`'s, a float included.
pub(crate) fn has_pane(app: &AppState, pane_id: usize) -> bool {
    app.windows.iter().any(|window| {
        let mut found = false;
        crate::pane_colours::window_panes(window, |pane| found |= pane.id == pane_id);
        found
    })
}

/// Drop the waiting batches of panes no server owns, so a pane that went
/// away cannot keep one here.
pub(crate) fn drop_orphans(live: impl Fn(usize) -> bool) {
    if !ANY_WAITING.load(Ordering::Acquire) {
        return;
    }
    if let Ok(mut waiting) = WAITING.lock() {
        waiting.retain(|b| live(b.pane_id));
        ANY_WAITING.store(!waiting.is_empty(), Ordering::Release);
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_clipboard_read.rs"]
mod tests;
