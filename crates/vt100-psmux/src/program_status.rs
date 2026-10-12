//! OSC 7501 records. Updates are validated before the store is touched.

/// A state reported by a program, independent of attention presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramState {
    /// At rest.
    Idle,
    /// Running.
    Working,
    /// Waiting for user input.
    Blocked,
    /// Finished successfully.
    Done,
    /// Failed and stopped.
    Error,
}

impl ProgramState {
    /// Protocol spelling of this state.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Error => "error",
        }
    }
}

/// Why a blocked program needs the user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramKind {
    /// Approval.
    Permission,
    /// An answer.
    Question,
    /// Credentials.
    Auth,
}

impl ProgramKind {
    /// Protocol spelling of this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// One application's last report for an id. Text is literal, untrusted UTF-8.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProgramStatus {
    /// Empty for the root record.
    pub id: String,
    /// Application-reported state.
    pub state: ProgramState,
    /// Present only for blocked records.
    pub kind: Option<ProgramKind>,
    /// Present only for working or blocked records; None is indeterminate.
    pub progress: Option<u8>,
    /// Explicit app; inheritance is resolved by the store.
    pub app: Option<String>,
    /// Literal label, never markup.
    pub title: Option<String>,
    /// Literal message, never markup.
    pub msg: Option<String>,
}

#[derive(Clone, Debug)]
struct Record {
    status: ProgramStatus,
    seen: bool,
    changed: std::time::Instant,
}

/// The live terminal's records, including shared acknowledgement state.
#[derive(Clone, Debug, Default)]
pub struct ProgramStatuses {
    records: Vec<Record>,
    generation: u64,
}

impl ProgramStatuses {
    /// Maximum live records, including the root.
    pub const CAPACITY: usize = 64;

    /// The terminal's live records.
    pub fn records(&self) -> impl Iterator<Item = &ProgramStatus> {
        self.records.iter().map(|r| &r.status)
    }

    /// Find the root or a child by id.
    pub fn get(&self, id: &str) -> Option<&ProgramStatus> {
        self.records.iter().find(|r| r.status.id == id).map(|r| &r.status)
    }

    /// Changes to records or acknowledgement; ordinary output does not advance it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the current result has been acknowledged.
    pub fn seen(&self, id: &str) -> bool {
        self.records.iter().any(|r| r.status.id == id && r.seen)
    }

    /// Resolve explicit app from the record or its nearest existing ancestor.
    pub fn inherited_app(&self, id: &str) -> Option<&str> {
        let mut ancestor = id;
        loop {
            if let Some(app) = self.get(ancestor).and_then(|r| r.app.as_deref()) {
                return Some(app);
            }
            if ancestor.is_empty() { return None; }
            ancestor = ancestor.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
    }

    /// Blocked, unseen error, then unseen done; newest changed record breaks ties.
    pub fn attention(&self) -> Option<&ProgramStatus> {
        self.records.iter().filter_map(|r| {
            let priority = match r.status.state {
                ProgramState::Blocked => 3,
                ProgramState::Error if !r.seen => 2,
                ProgramState::Done if !r.seen => 1,
                _ => return None,
            };
            Some(((priority, r.changed), &r.status))
        }).max_by_key(|(key, _)| *key).map(|(_, status)| status)
    }

    /// Time the selected attention record last changed, for cross-pane ordering.
    pub fn attention_changed(&self) -> Option<std::time::Instant> {
        let id = &self.attention()?.id;
        self.records.iter().find(|r| &r.status.id == id).map(|r| r.changed)
    }

    /// Mark every current completion/error seen without changing reported state.
    pub fn acknowledge(&mut self) {
        let mut changed = false;
        for record in &mut self.records {
            if matches!(record.status.state, ProgramState::Done | ProgramState::Error) && !record.seen {
                record.seen = true;
                changed = true;
            }
        }
        if changed { self.generation = self.generation.wrapping_add(1); }
    }

    /// Prompt start or attached-process exit expires idle, working and blocked.
    pub fn expire(&mut self) {
        self.retain(|r| matches!(r.state, ProgramState::Done | ProgramState::Error));
    }

    pub(crate) fn clear(&mut self) {
        self.retain(|_| false);
    }

    fn retain(&mut self, keep: impl Fn(&ProgramStatus) -> bool) {
        let before = self.records.len();
        self.records.retain(|r| keep(&r.status));
        if self.records.len() != before { self.generation = self.generation.wrapping_add(1); }
    }

    pub(crate) fn report(&mut self, body: &[u8]) {
        let Some((status, clear)) = parse(body) else { return; };
        if clear {
            self.retain(|r| !(status.id.is_empty() || r.id == status.id ||
                r.id.strip_prefix(&status.id).is_some_and(|rest| rest.starts_with('/'))));
            return;
        }
        let previous = self.records.iter().position(|r| r.status.id == status.id)
            .map(|index| self.records.remove(index));
        let (seen, changed) = if let Some(previous) = previous.filter(|r| r.status == status) {
            (previous.seen, previous.changed)
        } else {
            self.generation = self.generation.wrapping_add(1);
            (false, std::time::Instant::now())
        };
        if self.records.len() == Self::CAPACITY { self.records.remove(0); }
        self.records.push(Record { status, seen, changed });
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 32 && value.bytes().all(|b|
        b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn text(value: &str, encoded_limit: usize, decoded_limit: usize) -> Option<String> {
    if value.len() > encoded_limit { return None; }
    let body = value.trim_end_matches('=');
    let padding = value.len() - body.len();
    let remainder = body.len() % 4;
    if remainder == 1 || padding > 2 ||
        (padding != 0 && (value.len() % 4 != 0 || padding != (4 - remainder) % 4)) {
        return None;
    }
    let mut output = Vec::with_capacity(body.len() * 3 / 4);
    let mut bits = 0u32;
    let mut count = 0;
    for b in body.bytes() {
        let digit = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | u32::from(digit);
        count += 6;
        if count >= 8 {
            count -= 8;
            output.push(((bits >> count) & 255) as u8);
            bits &= (1 << count) - 1;
        }
    }
    if bits != 0 || output.len() > decoded_limit { return None; }
    let output = String::from_utf8(output).ok()?;
    if output.chars().any(char::is_control) { return None; }
    Some(output)
}

fn trimmed_field(bytes: &[u8]) -> &[u8] {
    if let Ok(text) = std::str::from_utf8(bytes) { return text.trim().as_bytes(); }
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |i| i + 1);
    &bytes[start..end]
}

fn parse(body: &[u8]) -> Option<(ProgramStatus, bool)> {
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;
    // Check even overwritten values before committing the final report.
    for pair in body.split(|b| *b == b':') {
        let Some(eq) = pair.iter().position(|b| *b == b'=') else { continue; };
        let key = trimmed_field(&pair[..eq]);
        let value = trimmed_field(&pair[eq + 1..]);
        if key.len() > 16 { return None; }
        match key {
            b"msg" if value.len() > 2732 => return None,
            b"title" if value.len() > 256 => return None,
            b"app" if value.len() > 32 => return None,
            b"id" if value.len() > 128 || value.split(|b| *b == b'/').count() > 8 ||
                value.split(|b| *b == b'/').any(|segment| segment.len() > 32) => return None,
            _ => {}
        }
        let Ok(key) = std::str::from_utf8(key) else { continue; };
        let Ok(value) = std::str::from_utf8(value) else {
            if key == "id" { id = Some("/"); }
            continue;
        };
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_lowercase()) ||
            !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b)) {
            // Preserve an invalid id until a later id overrides it; never address the root.
            if key == "id" { id = Some("/"); }
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => id = Some(value),
            "kind" => kind = match value {
                "permission" => Some(ProgramKind::Permission),
                "question" => Some(ProgramKind::Question),
                "auth" => Some(ProgramKind::Auth),
                _ => None,
            },
            "progress" => progress = value.parse::<u8>().ok().filter(|v| *v <= 100),
            "app" => app = identifier(value).then(|| value.to_owned()),
            "title" => title = Some(text(value, 256, 192)?),
            "msg" => msg = Some(text(value, 2732, 2048)?),
            _ => {}
        }
    }
    if let Some(id) = id {
        if id.split('/').count() > 8 || !id.split('/').all(identifier) { return None; }
    }
    let id = id.unwrap_or("");
    let clear = state == Some("clear");
    let state = match state? {
        "idle" | "clear" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "blocked" => ProgramState::Blocked,
        "done" => ProgramState::Done,
        "error" => ProgramState::Error,
        _ => return None,
    };
    if state != ProgramState::Blocked { kind = None; }
    if !matches!(state, ProgramState::Working | ProgramState::Blocked) { progress = None; }
    Some((ProgramStatus { id: id.to_owned(), state, kind, progress, app, title, msg }, clear))
}

/// Queries emitted by the existing parser, in terminal stream order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalQuery {
    /// OSC 7501 detection.
    ProgramStatus,
    /// Primary device attributes.
    Da1,
    /// Secondary device attributes.
    Da2,
    /// Device status.
    DsrStatus,
    /// Private mode status.
    DecrqmPrivate(u16),
    /// ANSI mode status.
    DecrqmAnsi(u16),
}

/// The fixed capability reply contains no application-controlled text.
pub const PROGRAM_STATUS_REPLY: &str = "\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";
