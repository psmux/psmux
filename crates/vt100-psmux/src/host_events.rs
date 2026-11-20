//! Exact, bounded bytes a pane addressed to the host terminal through an
//! enabled tmux passthrough (`allow-passthrough on` or `all`). Unwrapped
//! notification OSCs are not forwarded, as in tmux.

/// Bound each stage of delivery by bytes and event count. Overflow drops whole
/// events; it must never leave an incomplete terminal sequence at the host.
pub const MAX_HOST_EVENT_BYTES: usize = 1 << 20;
const MAX_EVENTS: usize = 256;

/// Bytes explicitly addressed to the host through tmux passthrough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEvent {
    /// An explicit tmux passthrough body, subject to on/all visibility.
    pub passthrough: bool,
    /// Exact notification sequence or unescaped passthrough body.
    pub bytes: Vec<u8>,
}

/// Events queued in any parser of this process. The server polls this on
/// every loop pass, so with nothing queued (always, while
/// `allow-passthrough` is off) it never has to lock a pane to look.
static PENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether any parser in this process holds host events not yet taken.
#[must_use]
pub fn host_events_pending() -> bool {
    PENDING.load(std::sync::atomic::Ordering::Acquire) != 0
}

fn release(n: usize) {
    if n > 0 {
        let _ = PENDING.fetch_update(
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
            |v| Some(v.saturating_sub(n)),
        );
    }
}

#[derive(Debug, Default)]
pub(crate) struct Events {
    bytes: usize,
    items: Vec<HostEvent>,
}

/// A cloned screen (copy mode snapshot, warm pane copy) starts with no
/// events: they are consumed once, from the live parser only.
impl Clone for Events {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        release(self.items.len());
    }
}

impl Events {
    pub(crate) fn push(&mut self, event: Vec<u8>, passthrough: bool) {
        if self.items.len() < MAX_EVENTS
            && event.len() <= MAX_HOST_EVENT_BYTES.saturating_sub(self.bytes)
        {
            self.bytes += event.len();
            self.items.push(HostEvent {
                bytes: event,
                passthrough,
            });
            PENDING.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    pub(crate) fn take(&mut self) -> Vec<HostEvent> {
        self.bytes = 0;
        let items = std::mem::take(&mut self.items);
        release(items.len());
        items
    }
}

/// A notification OSC that may be committed from inside a passthrough before
/// its closing ST arrives (see [`completed_notification`]).
fn notification(bytes: &[u8]) -> bool {
    let end = bytes.len() - if bytes.ends_with(b"\x07") { 1 } else { 2 };
    let body = &bytes[2..end];
    let Some(pos) = body.iter().position(|b| *b == b';') else {
        return false;
    };
    let (code, data) = (&body[..pos], &body[pos + 1..]);
    match code {
        // ConEmu interprets number-leading OSC 9 payloads as host commands;
        // some dispatch on numeric prefixes even without a semicolon.
        b"9" => {
            !data.iter().any(|b| *b < 0x20 || *b == 0x7f)
                && data
                    .iter()
                    .find(|b| !b.is_ascii_whitespace())
                    .map_or(true, |b| !b.is_ascii_digit())
        }
        b"777" => data.starts_with(b"notify;"),
        b"99" | b"9001" => true,
        b"7501" => !data.starts_with(b"?"),
        _ => false,
    }
}

/// Only a whole, single BEL-terminated notification can be committed before
/// the outer tmux ST: ConPTY may drop that ST and then remain completely idle.
pub(crate) fn completed_notification(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b]")
        && bytes.ends_with(b"\x07")
        && !bytes[2..bytes.len() - 1]
            .iter()
            .any(|b| matches!(b, 0x1b | 0x07 | 0x18 | 0x1a))
        && notification(bytes)
}
