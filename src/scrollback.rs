//! Scrollback client: `attach -f scrollback` and `new-session -f scrollback`
//! (`-f passthrough` is another name for it).
//!
//! The normal client draws the session in frames and places every cell with
//! a cursor move, so the host terminal never sees a line scroll off its top
//! and its own scrollback stays empty: a phone SSH app has nothing to scroll
//! back through. This client hands the host terminal the active pane's own
//! output bytes instead, as a plain shell session over SSH would, so the
//! terminal's scrollback fills and its own scrolling works.
//!
//! It is a control mode client underneath: the pane's output arrives as
//! `%output`, keys go back as `send-keys -H`, and the host terminal's size is
//! the client size (`refresh-client -C`). On attach, and again whenever
//! another pane becomes active, the pane's lines are printed first (on attach
//! with its history, so the scrollback holds what was written while nobody
//! was attached), the cursor is put where the pane has it, and from the end
//! of that capture on the live output is written as it arrives. The host
//! terminal shows the one active pane, with no status line and no borders.
//! `prefix d` detaches; the prefix pressed twice sends it to the pane once.
//! `read-only` and `ignore-size` go to the server as `refresh-client -f`,
//! so the server refuses a read only client's keys and other clients see
//! its flags.

use std::collections::VecDeque;
use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// The pane facts a capture is printed with.
const FACTS_FORMAT: &str =
    "#{pane_id} #{cursor_x} #{cursor_y} #{cursor_flag} #{alternate_on} #{history_size} #{pane_height}";

/// How often the host terminal's size is compared with the client size.
const SIZE_POLL: Duration = Duration::from_millis(250);
/// Where a pane's cursor is and how many lines it holds, from `FACTS_FORMAT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneFacts {
    pub pane_id: usize,
    pub cursor_x: u16,
    pub cursor_y: u16,
    pub cursor_visible: bool,
    pub alternate: bool,
    pub history: usize,
    pub height: usize,
}

pub fn parse_facts(line: &str) -> Option<PaneFacts> {
    let mut it = line.split_whitespace();
    Some(PaneFacts {
        pane_id: it.next()?.strip_prefix('%')?.parse().ok()?,
        cursor_x: it.next()?.parse().ok()?,
        cursor_y: it.next()?.parse().ok()?,
        cursor_visible: it.next()? != "0",
        alternate: it.next()? == "1",
        history: it.next()?.parse().ok()?,
        height: it.next()?.parse().ok()?,
    })
}

/// The bytes that put a pane's captured lines on the host terminal.
///
/// The lines are written one per row, each row cleared before it is written,
/// so the last `height` of them end up as the terminal's rows and the history
/// before them scrolls into the terminal's scrollback. `capture-pane` leaves
/// off trailing empty rows, so missing rows are added as empty ones. The
/// cursor is then moved, relative to the pane's last row, to where the pane
/// has it. This lines up with the live output when the host terminal is as
/// tall as the pane, which the client size makes it for a single pane window.
pub fn render_capture(lines: &[String], f: &PaneFacts, history: bool, leaving_alternate: bool) -> String {
    let want = f.height + if history && !f.alternate { f.history } else { 0 };
    let skip = lines.len().saturating_sub(want);
    let mut rows: Vec<&str> = lines[skip..].iter().map(|s| s.as_str()).collect();
    rows.resize(want, "");

    let mut s = String::from("\x1b[0m");
    if leaving_alternate {
        s.push_str("\x1b[?1049l");
    }
    if f.alternate {
        s.push_str("\x1b[?1049h\x1b[H");
    } else {
        s.push_str("\r\n");
    }
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            s.push_str("\r\n");
        }
        s.push_str("\x1b[2K");
        s.push_str(row);
        s.push_str("\x1b[0m");
    }
    s.push('\r');
    let up = f.height.saturating_sub(1).saturating_sub(f.cursor_y as usize);
    if up > 0 {
        s.push_str(&format!("\x1b[{}A", up));
    }
    if f.cursor_x > 0 {
        s.push_str(&format!("\x1b[{}C", f.cursor_x));
    }
    s.push_str(if f.cursor_visible { "\x1b[?25h" } else { "\x1b[?25l" });
    s
}

/// The pane id and payload of an `%output` or `%extended-output` line.
pub fn parse_output(line: &str) -> Option<(usize, &str)> {
    if let Some(rest) = line.strip_prefix("%output %") {
        let (id, data) = rest.split_once(' ')?;
        return Some((id.parse().ok()?, data));
    }
    let rest = line.strip_prefix("%extended-output %")?;
    let (id, rest) = rest.split_once(' ')?;
    let (_, data) = rest.split_once(" : ")?;
    Some((id.parse().ok()?, data))
}

/// Undo `control::escape_output`: `\ooo` is one byte in octal.
pub fn unescape_output(data: &str) -> Vec<u8> {
    let b = data.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..=i + 3].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// What a typed chunk does besides the keys it sends to the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    None,
    Detach,
}

/// Split typed bytes into the bytes for the pane and a detach. After the
/// prefix, `d` detaches (anything typed after it in the same chunk is
/// dropped), the prefix again sends one prefix, and any other byte is sent
/// together with the prefix before it. `after_prefix` carries a prefix typed
/// at the end of one chunk into the next.
pub fn route_keys(input: &[u8], prefix: Option<u8>, after_prefix: &mut bool) -> (Vec<u8>, KeyAction) {
    let mut keys = Vec::with_capacity(input.len());
    for &b in input {
        match prefix {
            Some(p) if *after_prefix => {
                *after_prefix = false;
                if b == b'd' {
                    return (keys, KeyAction::Detach);
                }
                if b != p {
                    keys.push(p);
                }
                keys.push(b);
            }
            Some(p) if b == p => *after_prefix = true,
            _ => keys.push(b),
        }
    }
    (keys, KeyAction::None)
}

/// The `refresh-client -f` that hands the server the client flags it acts
/// on: `read-only` makes the server refuse this client's keys and commands
/// and shows in `list-clients`, `ignore-size` leaves the window size to the
/// other clients.
pub fn client_flags_command(flags: &crate::cli::ClientFlags) -> Option<String> {
    let mut names = Vec::new();
    if flags.read_only {
        names.push("read-only");
    }
    if flags.ignore_size {
        names.push("ignore-size");
    }
    (!names.is_empty()).then(|| format!("refresh-client -f {}", names.join(",")))
}

pub fn send_keys_command(keys: &[u8]) -> String {
    let mut s = String::from("send-keys -H");
    for b in keys {
        s.push_str(&format!(" {:02x}", b));
    }
    s
}

/// What the reader does with the reply to a command; replies come back in
/// the order the commands were sent.
enum Reply {
    Ignore,
    Facts,
    Capture { history: bool },
    Lines(mpsc::Sender<Vec<String>>),
}

/// The socket's write half and the replies still owed on it, under one lock
/// so a command and its expected reply are queued together.
struct Link {
    out: TcpStream,
    replies: VecDeque<Reply>,
}

fn send(link: &Mutex<Link>, command: &str, reply: Reply) -> io::Result<()> {
    let mut l = link.lock().unwrap_or_else(|e| e.into_inner());
    l.replies.push_back(reply);
    l.out.write_all(command.as_bytes())?;
    l.out.write_all(b"\n")?;
    l.out.flush()
}

fn ask_capture(link: &Mutex<Link>, history: bool) -> io::Result<()> {
    send(link, &format!("display-message -p '{}'", FACTS_FORMAT), Reply::Facts)?;
    send(link, if history { "capture-pane -p -e -S -" } else { "capture-pane -p -e" }, Reply::Capture { history })
}

/// The prefix key as the one byte a terminal sends for it, if it is one.
/// `-A` gives the session's own prefix where it sets one and the global one
/// otherwise; `-g` alone would miss a session level prefix.
fn prefix_byte(link: &Mutex<Link>) -> Option<u8> {
    let (tx, rx) = mpsc::channel();
    send(link, "show-options -Av prefix", Reply::Lines(tx)).ok()?;
    let name = rx.recv_timeout(Duration::from_secs(2)).ok()?.into_iter().next()?;
    let (code, mods) = crate::config::parse_key_name(&name)?;
    let bytes = crate::input::encode_key_event(&crossterm::event::KeyEvent::new(code, mods))?;
    match bytes.as_slice() {
        [b] => Some(*b),
        _ => None,
    }
}

/// What the reader knows about the pane on the host terminal.
#[derive(Default)]
struct View {
    facts: Option<PaneFacts>,
    /// The pane whose output is written: set once its capture is printed.
    live: Option<usize>,
    alternate: bool,
}

/// How the client ends: the reader puts the terminal back and exits.
struct Ending {
    console: Arc<crate::platform::RawVtConsole>,
    session: String,
    detaching: AtomicBool,
}

impl Ending {
    fn finish(&self, view: &View) -> ! {
        let mut s = String::from(
            "\x1b[0m\x1b[?25h\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[?2004l",
        );
        if view.alternate {
            s.push_str("\x1b[?1049l");
        }
        if self.detaching.load(Ordering::Acquire) {
            s.push_str(&format!("\r\n[detached (from session {})]\r\n", self.session));
        } else {
            s.push_str("\r\n[exited]\r\n");
        }
        let mut out = io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
        self.console.restore();
        std::process::exit(0)
    }
}

/// Is `line` the `%end` or `%error` closing the block opened by `begin`?
/// The guard lines carry the same time, number and flags (tmux control.c).
fn closes_block(line: &str, begin: &str) -> Option<bool> {
    let tail = begin.strip_prefix("%begin")?;
    if line.strip_prefix("%end") == Some(tail) {
        Some(true)
    } else if line.strip_prefix("%error") == Some(tail) {
        Some(false)
    } else {
        None
    }
}

fn answers_client(begin: &str) -> bool {
    begin.rsplit(' ').next() == Some("1")
}

fn read_server(mut reader: io::BufReader<TcpStream>, link: Arc<Mutex<Link>>, ending: Arc<Ending>) -> ! {
    let mut out = io::stdout();
    let mut view = View::default();
    let mut block: Option<(String, Vec<String>)> = None;
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&raw);
        let line = text.trim_end_matches(&['\r', '\n'][..]);

        if let Some((begin, body)) = block.as_mut() {
            match closes_block(line, begin) {
                None => body.push(line.to_string()),
                Some(ok) => {
                    let (begin, body) = block.take().unwrap_or_default();
                    // Flags 0 mark a block the server sent on its own (the
                    // reply to the implicit attach); only flags 1 answer a
                    // command of this client.
                    if !answers_client(&begin) {
                        continue;
                    }
                    let reply = link.lock().unwrap_or_else(|e| e.into_inner())
                        .replies.pop_front().unwrap_or(Reply::Ignore);
                    match reply {
                        Reply::Ignore => {}
                        Reply::Facts => view.facts = body.first().and_then(|l| parse_facts(l)),
                        Reply::Capture { history } => {
                            if let Some(f) = view.facts {
                                let lines = if ok { body } else { Vec::new() };
                                let s = render_capture(&lines, &f, history, view.alternate && !f.alternate);
                                let _ = out.write_all(s.as_bytes());
                                let _ = out.flush();
                                view.live = Some(f.pane_id);
                                view.alternate = f.alternate;
                            }
                        }
                        Reply::Lines(tx) => {
                            let _ = tx.send(if ok { body } else { Vec::new() });
                        }
                    }
                }
            }
            continue;
        }
        if line.starts_with("%begin ") {
            block = Some((line.to_string(), Vec::new()));
        } else if let Some((pane, data)) = parse_output(line) {
            if view.live == Some(pane) {
                let _ = out.write_all(&unescape_output(data));
                let _ = out.flush();
            }
        } else if line.starts_with("%exit") {
            break;
        } else if line.starts_with("%window-pane-changed ") || line.starts_with("%session-window-changed ") {
            // Another pane is shown from its capture on; its output before
            // that capture is already in it.
            view.live = None;
            let _ = ask_capture(&link, false);
        }
    }
    ending.finish(&view)
}

pub fn run(flags: crate::cli::ClientFlags) -> io::Result<()> {
    let session = std::env::var("PSMUX_SESSION_NAME").unwrap_or_else(|_| "default".to_string());
    let shown = std::env::var("PSMUX_SESSION_DISPLAY_NAME").unwrap_or_else(|_| session.clone());

    let console = Arc::new(crate::platform::RawVtConsole::enable());
    let (reader, out) = crate::session::open_control_connection(&session, "CONTROL_QUIET")?;
    let link = Arc::new(Mutex::new(Link { out, replies: VecDeque::new() }));
    let ending = Arc::new(Ending { console: console.clone(), session: shown, detaching: AtomicBool::new(false) });
    {
        let link = link.clone();
        let ending = ending.clone();
        std::thread::spawn(move || read_server(reader, link, ending));
    }

    // The flags go first, so `ignore-size` is set before the size, then the
    // size, so the first capture is taken at it.
    if let Some(command) = client_flags_command(&flags) {
        send(&link, &command, Reply::Ignore)?;
    }
    let mut size = crossterm::terminal::size().ok();
    if let Some((w, h)) = size {
        send(&link, &format!("refresh-client -C {}x{}", w, h), Reply::Ignore)?;
    }
    let prefix = prefix_byte(&link);
    ask_capture(&link, true)?;

    {
        let link = link.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(SIZE_POLL);
            let now = crossterm::terminal::size().ok();
            if now != size {
                size = now;
                if let Some((w, h)) = now {
                    if send(&link, &format!("refresh-client -C {}x{}", w, h), Reply::Ignore).is_err() {
                        return;
                    }
                }
            }
        });
    }

    let mut stdin = io::stdin().lock();
    let mut buf = [0u8; 4096];
    let mut after_prefix = false;
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let (keys, action) = route_keys(&buf[..n], prefix, &mut after_prefix);
        if !keys.is_empty() {
            send(&link, &send_keys_command(&keys), Reply::Ignore)?;
        }
        if action == KeyAction::Detach {
            ending.detaching.store(true, Ordering::Release);
            send(&link, "detach-client", Reply::Ignore)?;
        }
    }
    // The terminal went away (an SSH connection dropped): the session keeps
    // running. The reader exits once the server confirms the detach.
    let _ = send(&link, "detach-client", Reply::Ignore);
    std::thread::sleep(Duration::from_secs(2));
    console.restore();
    Ok(())
}

#[cfg(test)]
#[path = "../tests-rs/test_scrollback_client.rs"]
mod tests;
