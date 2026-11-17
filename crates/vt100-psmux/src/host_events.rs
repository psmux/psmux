//! Exact, bounded host notification bytes, independent of vte's OSC parameter limit.

/// Bound each stage of delivery by bytes and event count. Overflow drops whole
/// events; it must never leave an incomplete terminal sequence at the host.
pub const MAX_HOST_EVENT_BYTES: usize = 1 << 20;
const MAX_EVENTS: usize = 256;

/// Whether bytes were explicitly addressed to the host through tmux passthrough.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEvent {
    /// An explicit tmux passthrough body, subject to on/all visibility.
    pub passthrough: bool,
    /// Exact notification sequence or unescaped passthrough body.
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Events {
    bytes: usize,
    items: Vec<HostEvent>,
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
        }
    }

    pub(crate) fn take(&mut self) -> Vec<HostEvent> {
        self.bytes = 0;
        std::mem::take(&mut self.items)
    }
}

#[derive(Debug, Default)]
enum State {
    #[default]
    Ground,
    Escape,
    String(bool),
    Osc(bool),
}

#[derive(Debug, Default)]
pub(crate) struct Collector {
    state: State,
    bytes: Vec<u8>,
    overflow: bool,
}

impl Collector {
    pub(crate) fn feed(&mut self, input: &[u8], events: &mut Events) {
        let mut i = 0;
        while i < input.len() {
            if matches!(self.state, State::Ground) {
                let Some(off) = memchr::memchr(0x1b, &input[i..]) else {
                    break;
                };
                i += off;
            }
            let b = input[i];
            i += 1;
            match self.state {
                State::Ground => self.state = State::Escape,
                State::Escape => match b {
                    b']' => {
                        self.bytes.clear();
                        self.bytes.extend_from_slice(b"\x1b]");
                        self.overflow = false;
                        self.state = State::Osc(false);
                    }
                    b'P' | b'_' | b'^' | b'X' => self.state = State::String(false),
                    0x1b => {}
                    _ => self.state = State::Ground,
                },
                State::String(esc) => {
                    if matches!(b, 0x18 | 0x1a) || (esc && b == b'\\') {
                        self.state = State::Ground;
                    } else if esc {
                        self.state = State::Escape;
                        i -= 1;
                    } else {
                        self.state = State::String(b == 0x1b);
                    }
                }
                State::Osc(esc) => {
                    if matches!(b, 0x18 | 0x1a) || (esc && b != b'\\') {
                        self.bytes.clear();
                        self.state = if esc { State::Escape } else { State::Ground };
                        if esc {
                            i -= 1;
                        }
                        continue;
                    }
                    if !self.overflow {
                        if self.bytes.len() < crate::input_buffer_size().min(MAX_HOST_EVENT_BYTES) {
                            self.bytes.push(b);
                        } else {
                            self.bytes.clear();
                            self.overflow = true;
                        }
                    }
                    if b == 0x07 || (esc && b == b'\\') {
                        if !self.overflow && notification(&self.bytes) {
                            events.push(std::mem::take(&mut self.bytes), false);
                        }
                        self.bytes.clear();
                        self.state = State::Ground;
                    } else {
                        self.state = State::Osc(b == 0x1b);
                    }
                }
            }
        }
    }
}

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
