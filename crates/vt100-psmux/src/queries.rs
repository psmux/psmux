use crate::TerminalQuery;

/// The most queries a screen captures between two drains
/// ([`crate::Screen::take_queries`]).  Each is answered, so without a bound a
/// few bytes of pane output each (`ESC [ c` is three) would have the host
/// build and deliver replies without limit.  A real probe asks far fewer:
/// Copilot CLI's colour burst is 19.
pub const MAX_PENDING_QUERIES: usize = 256;

pub(crate) fn csi_query(
    params: &vte::Params,
    intermediates: &[u8],
    ignore: bool,
    action: char,
) -> Option<TerminalQuery> {
    if ignore { return None; }
    let first = params.iter().next().and_then(|p| p.first()).copied().unwrap_or(0);
    match (action, intermediates, first) {
        ('c', [], 0) => Some(TerminalQuery::Da1),
        ('c', [b'>'], 0) => Some(TerminalQuery::Da2),
        ('n', [], 5) => Some(TerminalQuery::DsrStatus),
        // The live parser puts in the value (`Screen::decrqm`).
        ('p', [b'?', b'$'], mode) if mode > 0 => Some(TerminalQuery::DecrqmPrivate { mode, value: None }),
        ('p', [b'$'], mode) if mode > 0 => Some(TerminalQuery::DecrqmAnsi { mode, value: 0 }),
        // XTWINOPS size reports, which tmux answers in
        // input_csi_dispatch_winops (input.c:2163-2186).  Issue #841: tmux
        // 3.8 asks its outer terminal `CSI 18 t CSI 14 t` whenever the tty
        // has no pixel size (always the case under WSL) and holds every lone
        // Escape for 500 ms until the `CSI 14 t` answer arrives.  The text
        // area reports 18 and 19 reach psmux only from a host that does not
        // answer them itself: the inbox console host answers them and does
        // not forward them, an OpenConsole forwards them unanswered.
        ('t', [], op @ (14 | 15 | 16 | 18 | 19)) => Some(TerminalQuery::WindowReport(op)),
        // XTVERSION, which tmux answers only for a first parameter of 0 or
        // none (INPUT_CSI_XDA, input.c:1884 in tmux 3.8): `CSI > 1 q` is not
        // a version request.
        ('q', [b'>'], 0) => Some(TerminalQuery::Xtversion),
        ('n', [b'?'], 996) => Some(TerminalQuery::ColourScheme),
        // XTSMGRAPHICS, which tmux answers whatever it holds, unless it has
        // more than three parameters (input_csi_dispatch_sm_graphics,
        // input.c:2103 in tmux 3.8).
        // `Params::len` counts subparameters too, which tmux does not.
        ('S', [b'?'], _) if params.iter().count() <= 3 => Some(graphics_attribute(params)),
        _ => None,
    }
}

/// An XTSMGRAPHICS's parameters as tmux's `input_get(ictx, n, 0, 0)` reads
/// them: a missing one is 0 and one holding a colon is -1.
fn graphics_attribute(params: &vte::Params) -> TerminalQuery {
    let mut read = params.iter().map(|p| match p {
        [n] => i32::from(*n),
        _ => -1,
    });
    let mut next = || read.next().unwrap_or(0);
    TerminalQuery::GraphicsAttribute { item: next(), action: next(), value: next() }
}

pub(crate) fn osc_query(params: &[&[u8]], program_status: bool, bel: bool) -> Option<TerminalQuery> {
    match params {
        [b"7501", b"?"] if program_status => Some(TerminalQuery::ProgramStatus),
        // The colours are filled in by the live parser; the detector only
        // needs to know a reply may be due.
        [b"10", b"?"] => Some(TerminalQuery::ForegroundColour { colour: None, bel }),
        [b"11", b"?"] => Some(TerminalQuery::BackgroundColour { colour: None, bel }),
        [b"12", b"?"] => Some(TerminalQuery::CursorColour { colour: None, bel }),
        // tmux keeps the selection characters it knows, each once, and
        // answers with the first (`input_osc_52_parse`, input.c:3371 at tag
        // 3.8), so `xc` is `c` and `x` or nothing is none.
        [b"52", selection, b"?"] => Some(TerminalQuery::Clipboard {
            selection: selection.iter().copied().find(|c| b"cpqs01234567".contains(c)).unwrap_or(0),
            bel,
        }),
        // OSC 4 can hold several queries among its sets, so the live parser
        // takes them pair by pair (`palette_pairs`), not here.
        _ => None,
    }
}

/// The `index ; colour` pairs of an `OSC 4` after the `4`, in the order
/// written, read as tmux's `input_osc_4` reads them (input.c:2935-2963 in
/// tmux 3.8): a bad index, or an index with no colour after it, ends them.
pub(crate) fn palette_pairs<'a>(
    rest: &'a [&'a [u8]],
) -> impl Iterator<Item = (u8, &'a [u8])> + 'a {
    rest.chunks_exact(2)
        .map_while(|pair| crate::palette::parse_palette_index(pair[0]).map(|i| (i, pair[1])))
}

/// Whether an OSC asks for a palette colour: a `?` for any index, as tmux's
/// `input_osc_4` takes one (input.c:2945-2958 at tag 3.8). The host answers
/// it from the pane's entry, the outer terminal's sixteen colours, or the
/// xterm 256 colour table, so every one is a query here.
fn asks_palette(params: &[&[u8]]) -> bool {
    match params {
        [b"4", rest @ ..] => palette_pairs(rest).any(|(_, spec)| spec == b"?"),
        _ => false,
    }
}

/// A DECRQSS, `DCS $ q Pt ST`, as tmux reads one (`input_dcs_dispatch`,
/// input.c:2660-2664, and `input_handle_decrqss`, input.c:2554-2617 at tag
/// 3.8). Only the start of `Pt` matters: `SP q` asks for the cursor style,
/// whatever follows it, and anything else is a setting tmux does not know.
/// A parameter before the `$` changes nothing.
///
/// tmux reads everything up to the ST as the string: a CAN, a SUB, an ESC
/// that does not begin the ST, and the sequences after it (input.c:683-702).
/// So it answers once, at that ST, and answers nothing the string holds, a
/// DA1 or another DCS among them. vte ends the DCS at the first of those and
/// parses the rest, so the string's start is read up to there and the query
/// waits for the next ST, holding back the queries met meanwhile. tmux gives
/// up on a DCS 5 seconds after it began (`input_start_ground_timer`,
/// input.c:826-832, started at input.c:2547), and so does this. A string as
/// long as `input-buffer-size` is dropped (`INPUT_DISCARD`, input.c:2641).
#[derive(Debug, Default, Clone)]
pub(crate) struct Decrqss {
    /// The first two bytes of `Pt` and its length, while vte is in the DCS.
    reading: Option<([u8; 2], usize)>,
    /// When the DCS began.
    since: Option<std::time::Instant>,
    /// vte has ended the DCS; true when it asked for the cursor style. The
    /// query is answered at the next ST.
    open: Option<bool>,
}

/// tmux's ground timer, after which an unfinished DCS is dropped.
const GROUND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl Decrqss {
    pub(crate) fn hook(&mut self, intermediates: &[u8], action: char) {
        // A DCS inside the string is part of it.
        if self.open() {
            return;
        }
        self.open = None;
        let decrqss = action == 'q' && intermediates == b"$";
        self.reading = decrqss.then_some(([0; 2], 0));
        self.since = decrqss.then(std::time::Instant::now);
    }

    pub(crate) fn put(&mut self, byte: u8) {
        if let Some((head, len)) = &mut self.reading {
            if let Some(slot) = head.get_mut(*len) {
                *slot = byte;
            }
            *len += 1;
            if *len >= crate::perform::input_buffer_size() {
                self.reading = None;
            }
        }
    }

    pub(crate) fn unhook(&mut self) {
        if let Some((head, len)) = self.reading.take() {
            self.open = Some(len >= 2 && head == *b" q");
        }
    }

    /// Whether a string is still waiting for its ST, so that what comes is
    /// part of it.
    pub(crate) fn open(&self) -> bool {
        self.open.is_some() && self.since.is_some_and(|t| t.elapsed() < GROUND_TIMEOUT)
    }

    /// The ST (`ESC \`): whether the string it ends asked for the cursor
    /// style, or `None` when it ends none.
    pub(crate) fn st(&mut self) -> Option<bool> {
        let live = self.open();
        self.open.take().filter(|_| live)
    }
}

/// Detects reply requests before synchronized output reaches the live parser.
///
/// Uses the live parser's query classification without a screen or callbacks.
/// It never generates replies or stores program status. Feed every incoming
/// batch, including outside updates, so split sequences keep their context.
#[derive(Default)]
pub struct QueryDetector {
    parser: vte::Parser,
    requests: ReplyRequests,
}

impl QueryDetector {
    /// Processes a new batch and reports whether it completes any reply request.
    pub fn scan(&mut self, bytes: &[u8]) -> bool {
        self.parser.advance(&mut self.requests, bytes);
        std::mem::take(&mut self.requests.pending)
    }

    /// Mirror the live screen's [`crate::Screen::set_program_status_enabled`]:
    /// off, an OSC 7501 query is not a reply request.
    pub fn set_program_status_enabled(&mut self, enabled: bool) {
        self.requests.program_status = enabled;
    }
}

#[derive(Default)]
struct ReplyRequests {
    pending: bool,
    program_status: bool,
    decrqss: Decrqss,
}

impl vte::Perform for ReplyRequests {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        self.pending |= csi_query(params, intermediates, ignore, action).is_some() && !self.decrqss.open();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        self.pending |= (osc_query(params, self.program_status, bell_terminated).is_some() || asks_palette(params))
            && !self.decrqss.open();
    }

    fn hook(&mut self, _params: &vte::Params, intermediates: &[u8], _ignore: bool, action: char) {
        self.decrqss.hook(intermediates, action);
    }

    fn put(&mut self, byte: u8) {
        self.decrqss.put(byte);
    }

    fn unhook(&mut self) {
        self.decrqss.unhook();
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if intermediates.is_empty() && byte == b'\\' {
            self.pending |= self.decrqss.st().is_some();
        }
    }
}
