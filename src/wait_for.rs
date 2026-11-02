//! Named signal channels and independent FIFO lock queues.
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, Read, Write};
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

pub(crate) const AUTH_CAPABILITY: &str = "PSMUX_WAIT_REPLY_V1";
pub(crate) const NEGOTIATION_ACK: &str = "\u{0001}PSMUX_WAIT_REPLY_V1\u{0001}\n";
pub(crate) const MAX_BUFFERED_INPUT: usize = 64 * 1024;
const MAX_CHUNK: usize = 64 * 1024;

// Negotiation is entirely inside AUTH, before aliases or command dispatch.
pub(crate) fn negotiate(
    reader: &mut impl io::BufRead,
    writer: &mut impl Write,
    key: &str,
) -> io::Result<()> {
    writeln!(writer, "AUTH {} {}", key, AUTH_CAPABILITY)?;
    writer.flush()?;
    let mut reply = String::new();
    reader.take(128).read_line(&mut reply)?;
    if !reply.ends_with('\n') {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete wait-for AUTH reply"));
    }
    if reply != NEGOTIATION_ACK {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
            "server did not accept wait-for completion protocol; check the session key and restart the server with the updated binary"));
    }
    Ok(())
}

// Lengths delimit payload, so even output containing the receipt is just data.
pub(crate) fn write_output(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    for chunk in bytes.chunks(MAX_CHUNK) {
        writer.write_all(b"D")?;
        writer.write_all(&(chunk.len() as u32).to_be_bytes())?;
        writer.write_all(chunk)?;
    }
    Ok(())
}

pub(crate) fn write_complete(writer: &mut impl Write, input: &WaitInput) -> io::Result<()> {
    // A command-local exit does not resolve input admitted during its wait.
    if input.failed || !input.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "unresolved wait-for input"));
    }
    writer.write_all(b"C")?;
    writer.flush()
}

pub(crate) fn read_response(reader: &mut impl Read) -> io::Result<String> {
    let mut output = Vec::new();
    let mut chunk = [0u8; MAX_CHUNK];
    loop {
        let mut tag = [0];
        reader.read_exact(&mut tag)?;
        match tag[0] {
            b'C' => return Ok(String::from_utf8_lossy(&output).into_owned()),
            b'D' => {
                let mut length = [0; 4];
                reader.read_exact(&mut length)?;
                let length = u32::from_be_bytes(length) as usize;
                if length == 0 || length > MAX_CHUNK {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid completion payload length"));
                }
                reader.read_exact(&mut chunk[..length])?;
                output.extend_from_slice(&chunk[..length]);
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid completion frame")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitForOp {
    Wait,
    Lock,
    Signal,
    Unlock,
}

/// tmux checks -S before -L before -U when flags are combined.
pub fn parse(args: &[&str]) -> Result<(String, WaitForOp), String> {
    let mut signal = false;
    let mut lock = false;
    let mut unlock = false;
    let mut options = true;
    let mut channel = None;
    for arg in args {
        if options && *arg == "--" {
            options = false;
        } else if options && arg.starts_with('-') && arg.len() > 1 {
            for flag in arg[1..].chars() {
                match flag {
                    'S' => signal = true,
                    'L' => lock = true,
                    'U' => unlock = true,
                    _ => return Err(format!("unknown flag -{}", flag)),
                }
            }
        } else {
            if channel.replace((*arg).to_string()).is_some() {
                return Err("usage: wait-for [-L | -S | -U] channel".to_string());
            }
            options = false;
        }
    }
    let name = channel.ok_or_else(|| "usage: wait-for [-L | -S | -U] channel".to_string())?;
    let op = if signal {
        WaitForOp::Signal
    } else if lock {
        WaitForOp::Lock
    } else if unlock {
        WaitForOp::Unlock
    } else {
        WaitForOp::Wait
    };
    Ok((name, op))
}

type Reply = Sender<Result<(), String>>;

struct Waiter {
    client_id: u64,
    reply: Reply,
}

#[derive(Default)]
struct WaitChannel {
    locked: bool,
    woken: bool,
    waiters: VecDeque<Waiter>,
    lockers: VecDeque<Waiter>,
}

#[derive(Default)]
pub struct WaitChannels(HashMap<String, WaitChannel>);

impl WaitChannels {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn execute(&mut self, name: String, op: WaitForOp, client_id: u64, reply: Reply) {
        if op == WaitForOp::Unlock && !self.0.get(&name).is_some_and(|c| c.locked) {
            let _ = reply.send(Err(format!("channel {} not locked", name)));
            return;
        }
        let channel = self.0.entry(name.clone()).or_default();
        match op {
            WaitForOp::Wait if !channel.woken => {
                channel.waiters.push_back(Waiter { client_id, reply });
                return;
            }
            WaitForOp::Lock if channel.locked => {
                channel.lockers.push_back(Waiter { client_id, reply });
                return;
            }
            WaitForOp::Lock => channel.locked = true,
            WaitForOp::Unlock => {
                // A cancelled receiver cannot acquire the lock. Skip it rather
                // than stranding the next live acquirer until another unlock.
                while let Some(waiter) = channel.lockers.pop_front() {
                    if waiter.reply.send(Ok(())).is_ok() {
                        let _ = reply.send(Ok(()));
                        return;
                    }
                }
                channel.locked = false;
            }
            WaitForOp::Signal => {
                if channel.waiters.is_empty() && !channel.woken {
                    channel.woken = true;
                    let _ = reply.send(Ok(()));
                    return;
                }
                for waiter in channel.waiters.drain(..) {
                    let _ = waiter.reply.send(Ok(()));
                }
            }
            WaitForOp::Wait => {}
        }
        // tmux removes a woken channel only when unlocked and without waiters.
        if !channel.locked && channel.woken && channel.waiters.is_empty() {
            self.0.remove(&name);
        }
        let _ = reply.send(Ok(()));
    }

    pub fn client_lost(&mut self, client_id: u64) {
        self.0.retain(|_, channel| {
            channel.waiters.retain(|w| w.client_id != client_id);
            channel.lockers.retain(|w| w.client_id != client_id);
            channel.locked
                || channel.woken
                || !channel.waiters.is_empty()
                || !channel.lockers.is_empty()
        });
    }
}

// Pending commands are read only to detect cancellation, never dispatched
// ahead of the suspended item. The cap includes queued and partial bytes.
#[derive(Default)]
pub(crate) struct WaitInput {
    lines: VecDeque<String>,
    partial: Vec<u8>,
    queued_bytes: usize,
    pub(crate) eof: bool,
    pub(crate) cancel_on_eof: bool,
    pub(crate) failed: bool,
}

impl WaitInput {
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.partial.is_empty()
    }

    fn read_piece(&mut self, r: &mut impl io::BufRead, pending: bool) -> io::Result<bool> {
        let bytes = r.fill_buf()?;
        if bytes.is_empty() {
            self.eof = true;
            return Ok(true);
        }
        let count = bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |i| i + 1);
        let count = if pending {
            let remaining = MAX_BUFFERED_INPUT - self.queued_bytes - self.partial.len();
            if remaining == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "too much input while wait-for is pending"));
            }
            count.min(remaining)
        } else {
            count
        };
        let complete = bytes[count - 1] == b'\n';
        self.partial.extend_from_slice(&bytes[..count]);
        r.consume(count);
        Ok(complete)
    }

    fn take_line(&mut self) -> io::Result<String> {
        String::from_utf8(std::mem::take(&mut self.partial))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub(crate) fn read_line(&mut self, r: &mut impl io::BufRead, line: &mut String) -> io::Result<usize> {
        let next = if let Some(queued) = self.lines.pop_front() {
            self.queued_bytes -= queued.len();
            queued
        } else {
            while !self.eof && !self.read_piece(r, false)? {}
            if self.eof && self.cancel_on_eof && !self.partial.is_empty() {
                self.failed = true;
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "incomplete command after wait-for"));
            }
            self.take_line()?
        };
        let n = next.len();
        line.push_str(&next);
        Ok(n)
    }

    pub(crate) fn wait(
        &mut self,
        r: &mut impl io::BufRead,
        reply: mpsc::Receiver<Result<(), String>>,
        retry: impl Fn(&io::Error) -> bool,
    ) -> Option<Result<(), String>> {
        loop {
            match reply.try_recv() {
                Ok(result) => return Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.failed = true;
                    return None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if self.eof {
                if self.cancel_on_eof {
                    self.failed = true;
                    return None;
                }
                // Ordinary one-shot callers, including source jobs, half-close
                // to end their request. Their EOF is not cancellation.
                match reply.recv_timeout(Duration::from_millis(50)) {
                    Ok(result) => return Some(result),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        self.failed = true;
                        return None;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                }
            }
            match self.read_piece(r, true) {
                Ok(_) if self.eof && self.cancel_on_eof => {
                    self.failed = true;
                    return None;
                }
                Ok(true) if !self.partial.is_empty() => {
                    match self.take_line() {
                        Ok(line) => {
                            self.queued_bytes += line.len();
                            self.lines.push_back(line);
                        }
                        Err(e) => {
                            self.failed = true;
                            return Some(Err(e.to_string()));
                        }
                    }
                }
                Ok(_) => {}
                Err(e) if retry(&e) => {}
                Err(e) => {
                    self.failed = true;
                    return Some(Err(e.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_wait_for_channels.rs"]
mod tests;
#[cfg(test)]
#[path = "../tests-rs/test_wait_for_input.rs"]
mod input_tests;
#[cfg(test)]
#[path = "../tests-rs/test_wait_for_auth.rs"]
mod auth_tests;
#[cfg(test)]
#[path = "../tests-rs/test_wait_for_codec.rs"]
mod codec_tests;
