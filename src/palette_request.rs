//! A palette query the pane has no entry for, asked of the outer terminal.
//!
//! tmux answers `OSC 4 ; N ; ?` from the pane's palette, and when the pane has
//! no entry for N it asks the outer terminal of a client showing the pane
//! (input_add_request, input.c:3584 at tag 3.8). While that question is out,
//! every later reply of the pane waits behind it (input_reply, input.c:1176),
//! and the answer, or 500 ms (INPUT_REQUEST_TIMEOUT), lets them go
//! (input_request_reply, input.c:3668; input_request_timer_callback,
//! input.c:3521). The client takes any OSC 4 reply off its input instead of
//! reading it as keys (tty_keys_palette, tty-keys.c:1830).
//!
//! psmux answers the sixteen ANSI colours from the values the client read off
//! its terminal at startup (#473), so only an index of 16 or more with no pane
//! entry is asked. The pieces, in the order a question travels:
//! - the pane's parser thread keeps a [`Requests`] queue and puts each
//!   question on a global list ([`push_request`]), then waits on its inbox;
//! - the server loop hands each to the most recently active client
//!   ([`route`]) as a `PALETTE-QUERY N` directive;
//! - the client writes `ESC ] 4 ; N ; ? ESC \` to its terminal, or answers at
//!   once that there is none when its terminal never answered OSC 4
//!   ([`outer_answers`]);
//! - the client's input routes take the reply off the key stream
//!   ([`parse_reply`], [`push_client_reply`]) and send `host-palette N RRGGBB`;
//! - the server loop matches it to the client's oldest question ([`answer`])
//!   and puts it in the pane's inbox ([`deliver`]), which wakes the parser
//!   thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::pane::{PaneReply, ReplyFallback};

/// How long a question waits for its answer, and a reply behind it waits to go
/// (INPUT_REQUEST_TIMEOUT, input.c:75).
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

/// The first index asked of the outer terminal. Below it the client's
/// startup values answer.
pub(crate) const FIRST_ASKED: u8 = 16;

/// The most questions one pane keeps open. A pane that floods the queries
/// gets no more asked until some are answered or time out, so the work and
/// memory stay bounded by this, not by its output.
const MAX_OPEN: usize = vt100::MAX_PENDING_QUERIES;

type Rgb = (u8, u8, u8);

/// One reply in the order a batch's queries were asked: ready now, or a
/// question for the outer terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReplyItem {
    Reply(PaneReply),
    Ask { index: u8, bel: bool },
}

#[derive(Debug)]
enum Entry {
    /// A question out to the outer terminal (INPUT_REQUEST_PALETTE).
    Request { index: u8, bel: bool, at: Instant },
    /// A reply waiting behind one (INPUT_REQUEST_QUEUE).
    Queued { reply: PaneReply, at: Instant },
}

impl Entry {
    fn at(&self) -> Instant {
        match self {
            Entry::Request { at, .. } | Entry::Queued { at, .. } => *at,
        }
    }
}

/// A pane's questions and the replies waiting behind them, in order
/// (`ictx->requests`).
#[derive(Debug, Default)]
pub(crate) struct Requests {
    entries: VecDeque<Entry>,
}

impl Requests {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn open(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e, Entry::Request { .. })).count()
    }

    /// Take one batch's replies in the order asked. Returns the replies to
    /// deliver now and the indexes to ask. A reply waits when anything is
    /// already waiting (input_reply with `add`), except the console host's own
    /// first DA1, which is not the child's and has its host blocked on it.
    pub(crate) fn add(&mut self, items: Vec<ReplyItem>, now: Instant) -> (Vec<PaneReply>, Vec<u8>) {
        let mut out = Vec::new();
        let mut asks = Vec::new();
        for item in items {
            match item {
                ReplyItem::Ask { index, bel } => {
                    if self.open() >= MAX_OPEN {
                        continue;
                    }
                    self.entries.push_back(Entry::Request { index, bel, at: now });
                    asks.push(index);
                }
                ReplyItem::Reply(reply) => {
                    if reply.console_host || self.entries.is_empty() {
                        out.push(reply);
                    } else {
                        self.entries.push_back(Entry::Queued { reply, at: now });
                    }
                }
            }
        }
        (out, asks)
    }

    /// The outer terminal's answer for `index`, or `None` when there is none
    /// to come. As input_request_reply walks the pane's list: the questions
    /// before the one answered are dropped unanswered, the replies before and
    /// after it go out, up to the next open question.
    pub(crate) fn answer(&mut self, index: u8, rgb: Option<Rgb>) -> Vec<PaneReply> {
        let Some(found) = self
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Request { index: i, .. } if *i == index))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut complete = false;
        let mut at = 0usize;
        while let Some(front) = self.entries.front() {
            if complete && !matches!(front, Entry::Queued { .. }) {
                break;
            }
            let Some(entry) = self.entries.pop_front() else { break };
            match entry {
                Entry::Queued { reply, .. } => out.push(reply),
                Entry::Request { index, bel, .. } if at == found => {
                    if let Some(rgb) = rgb {
                        out.extend(palette_answer(index, rgb, bel));
                    }
                    complete = true;
                }
                Entry::Request { .. } => {}
            }
            at += 1;
        }
        out
    }

    /// Drop what has waited past [`REQUEST_TIMEOUT`], letting the replies
    /// among it go (input_request_timer_callback).
    pub(crate) fn expire(&mut self, now: Instant) -> Vec<PaneReply> {
        let mut out = Vec::new();
        let mut kept = VecDeque::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if now.duration_since(entry.at()) < REQUEST_TIMEOUT {
                kept.push_back(entry);
            } else if let Entry::Queued { reply, .. } = entry {
                out.push(reply);
            }
        }
        self.entries = kept;
        out
    }

    /// When the oldest entry times out.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.entries.iter().map(Entry::at).min().map(|at| at + REQUEST_TIMEOUT)
    }
}

/// The reply to `index` from the outer terminal's colour, ended as the query
/// was. There is no colour path to fall back to for it (its bits hold the
/// sixteen ANSI colours), so a failed injection loses it.
fn palette_answer(index: u8, rgb: Rgb, bel: bool) -> Option<PaneReply> {
    crate::server::helpers::palette_reply(usize::from(index), &crate::types::HostColors::empty(), Some(rgb), bel)
        .map(|text| PaneReply { text, fallback: ReplyFallback::Lost("OSC 4"), console_host: false })
}

// ── From the parser threads to the server loop ──────────────────────────────

/// Questions waiting for the server loop: (pane id, index).
static REQUESTS: Mutex<Vec<(usize, u8)>> = Mutex::new(Vec::new());

/// Cheap gate for [`REQUESTS`]: one atomic a pass while nobody asks.
pub(crate) static REQUEST_PENDING: AtomicBool = AtomicBool::new(false);

/// A client that can be asked is attached. The parser threads read it so a
/// pane with nobody to ask answers nothing at once, as tmux does when
/// input_add_request finds no client. The server loop keeps it up to date.
pub(crate) static FORWARD: AtomicBool = AtomicBool::new(false);

/// Put a pane's question on the list and wake the server loop.
pub(crate) fn push_request(pane_id: usize, index: u8) {
    if let Ok(mut q) = REQUESTS.lock() {
        // Each pane keeps at most MAX_OPEN open; this bounds a server whose
        // loop has stopped taking them.
        if q.len() >= 4 * MAX_OPEN {
            q.remove(0);
        }
        q.push((pane_id, index));
    }
    REQUEST_PENDING.store(true, Ordering::Release);
    crate::types::wake_server_loop();
}

pub(crate) fn take_requests() -> Vec<(usize, u8)> {
    REQUESTS.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

// ── From the server loop back to a pane ─────────────────────────────────────

type Wake = Arc<(Mutex<Vec<u8>>, Condvar)>;

struct Inbox {
    pane_id: usize,
    answers: Vec<(u8, Option<Rgb>)>,
    ready: Arc<AtomicBool>,
    wake: Wake,
}

/// One inbox per pane that has asked, so few that a list beats a map.
static INBOXES: Mutex<Vec<Inbox>> = Mutex::new(Vec::new());

/// The pane's inbox, opened on its first question. `wake` is the parser
/// thread's staging condvar, which [`deliver`] notifies; the returned flag is
/// raised with each answer.
pub(crate) fn open_inbox(pane_id: usize, wake: &Wake) -> Arc<AtomicBool> {
    let mut boxes = match INBOXES.lock() {
        Ok(b) => b,
        Err(_) => return Arc::new(AtomicBool::new(false)),
    };
    if let Some(b) = boxes.iter().find(|b| b.pane_id == pane_id) {
        return b.ready.clone();
    }
    let ready = Arc::new(AtomicBool::new(false));
    boxes.push(Inbox { pane_id, answers: Vec::new(), ready: ready.clone(), wake: wake.clone() });
    ready
}

pub(crate) fn close_inbox(pane_id: usize) {
    if let Ok(mut boxes) = INBOXES.lock() {
        boxes.retain(|b| b.pane_id != pane_id);
    }
}

pub(crate) fn take_answers(pane_id: usize) -> Vec<(u8, Option<Rgb>)> {
    let Ok(mut boxes) = INBOXES.lock() else { return Vec::new() };
    match boxes.iter_mut().find(|b| b.pane_id == pane_id) {
        Some(b) => {
            b.ready.store(false, Ordering::Release);
            std::mem::take(&mut b.answers)
        }
        None => Vec::new(),
    }
}

/// Hand a pane the answer to its question and wake its parser thread.
pub(crate) fn deliver(pane_id: usize, index: u8, rgb: Option<Rgb>) {
    let Ok(mut boxes) = INBOXES.lock() else { return };
    if let Some(b) = boxes.iter_mut().find(|b| b.pane_id == pane_id) {
        if b.answers.len() < MAX_OPEN {
            b.answers.push((index, rgb));
        }
        b.ready.store(true, Ordering::Release);
        b.wake.1.notify_all();
    }
}

// ── The server loop ─────────────────────────────────────────────────────────

/// The client to ask: the attached one, not a control client, with the latest
/// activity, as input_add_request picks it.
fn chosen_client(app: &crate::types::AppState) -> Option<u64> {
    app.client_registry
        .values()
        .filter(|c| !c.is_control)
        .max_by_key(|c| c.last_activity)
        .map(|c| c.id)
}

/// Whether the pane is one a client of this app shows: in a window, a
/// floating pane, or the popup. A warm spare is in no window yet.
fn shows_pane(app: &crate::types::AppState, pane_id: usize) -> bool {
    if crate::tree::find_pane_by_id_global(app, pane_id).is_some() {
        return true;
    }
    if app.windows.iter().any(|w| w.floating.iter().any(|f| f.pane.id == pane_id)) {
        return true;
    }
    matches!(&app.mode, crate::types::Mode::PopupMode { popup_pane: Some(p), .. } if p.id == pane_id)
}

/// Ask a client for each question of a pane this app shows. Returns the
/// questions for panes it does not show, for another session to take or for
/// [`finish`].
pub(crate) fn route(app: &mut crate::types::AppState, requests: Vec<(usize, u8)>) -> Vec<(usize, u8)> {
    let mut other = Vec::new();
    for (pane_id, index) in requests {
        if !shows_pane(app, pane_id) {
            other.push((pane_id, index));
            continue;
        }
        let asked = chosen_client(app).filter(|&cid| {
            crate::types::send_directive_to_client(cid, &format!("{}{}", DIRECTIVE, index))
        });
        match asked {
            Some(cid) => app.palette_requests.entry(cid).or_default().push_back((pane_id, index)),
            None => deliver(pane_id, index, None),
        }
    }
    other
}

/// Answer, with none, questions nobody can be asked.
pub(crate) fn finish(requests: Vec<(usize, u8)>) {
    for (pane_id, index) in requests {
        deliver(pane_id, index, None);
    }
}

/// A client's answer. Its questions are answered in the order asked, so one
/// for another index before it is dropped, as input_request_reply drops it.
/// Here it is answered with none at once rather than left to time out.
pub(crate) fn answer(app: &mut crate::types::AppState, client_id: u64, index: u8, rgb: Option<Rgb>) {
    let Some(q) = app.palette_requests.get_mut(&client_id) else { return };
    let Some(pos) = q.iter().position(|&(_, i)| i == index) else { return };
    for (pane_id, i) in q.drain(..=pos) {
        deliver(pane_id, i, if i == index { rgb } else { None });
    }
    if q.is_empty() {
        app.palette_requests.remove(&client_id);
    }
}

/// A client went away: its questions will not be answered
/// (input_cancel_requests).
pub(crate) fn cancel_client(app: &mut crate::types::AppState, client_id: u64) {
    if let Some(q) = app.palette_requests.remove(&client_id) {
        for (pane_id, index) in q {
            deliver(pane_id, index, None);
        }
    }
}

/// Whether this app has a client to ask, for [`FORWARD`].
pub(crate) fn can_ask(app: &crate::types::AppState) -> bool {
    app.client_registry.values().any(|c| !c.is_control)
}

// ── The client ──────────────────────────────────────────────────────────────

/// The directive that asks a client, followed by the index.
pub(crate) const DIRECTIVE: &str = "PALETTE-QUERY ";

pub(crate) fn parse_directive(line: &str) -> Option<u8> {
    line.trim().strip_prefix(DIRECTIVE)?.trim().parse().ok()
}

/// What the client writes to its terminal for `index`.
pub(crate) fn query_bytes(index: u8) -> String {
    format!("\x1b]4;{};?\x1b\\", index)
}

/// Whether this client's terminal is worth asking. One that was asked for
/// the sixteen ANSI colours at startup and answered none of them will not
/// answer now either, so the client says so at once instead of letting the
/// pane wait the full timeout. A client that never asked (the pipe route, or
/// a psmux nested in a psmux pane) asks, as tmux does.
pub(crate) fn outer_answers() -> bool {
    match crate::types::HOST_COLORS_SPEC.get() {
        None => true,
        Some(None) => crate::util::psmux_drawn_terminal(),
        Some(Some(spec)) => crate::types::HostColors::from_spec(spec).palette.iter().any(Option::is_some),
    }
}

/// An OSC 4 reply's text after `ESC ]` and before its terminator, as
/// tty_keys_palette reads it: `4;N;colour`.
pub(crate) fn parse_reply(body: &str) -> Option<(u8, Rgb)> {
    let rest = body.strip_prefix("4;")?;
    let (index, colour) = rest.split_once(';')?;
    Some((index.parse().ok()?, crate::types::parse_x11_color(colour)?))
}

/// Replies the client's input routes took off the key stream, for its main
/// loop to send.
static CLIENT_REPLIES: Mutex<Vec<(u8, Rgb)>> = Mutex::new(Vec::new());

pub(crate) fn push_client_reply(index: u8, rgb: Rgb) {
    if let Ok(mut q) = CLIENT_REPLIES.lock() {
        if q.len() < MAX_OPEN {
            q.push((index, rgb));
        }
    }
}

pub(crate) fn take_client_replies() -> Vec<(u8, Rgb)> {
    CLIENT_REPLIES.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// The line a client sends with an answer, or with none.
pub(crate) fn reply_line(index: u8, rgb: Option<Rgb>) -> String {
    match rgb {
        Some((r, g, b)) => format!("host-palette {} {:02x}{:02x}{:02x}\n", index, r, g, b),
        None => format!("host-palette {} none\n", index),
    }
}

/// The server's reading of that line's arguments.
pub(crate) fn parse_reply_args(args: &[&str]) -> Option<(u8, Option<Rgb>)> {
    let index = args.first()?.parse().ok()?;
    let colour = *args.get(1)?;
    if colour == "none" {
        return Some((index, None));
    }
    if colour.len() != 6 || !colour.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&colour[i..i + 2], 16).ok();
    Some((index, Some((byte(0)?, byte(2)?, byte(4)?))))
}
