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
        ('p', [b'?', b'$'], mode) if mode > 0 => Some(TerminalQuery::DecrqmPrivate(mode)),
        ('p', [b'$'], mode) if mode > 0 => Some(TerminalQuery::DecrqmAnsi(mode)),
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
        _ => None,
    }
}

pub(crate) fn osc_query(params: &[&[u8]], program_status: bool) -> Option<TerminalQuery> {
    match params {
        [b"7501", b"?"] if program_status => Some(TerminalQuery::ProgramStatus),
        [b"10", b"?"] => Some(TerminalQuery::ForegroundColour),
        [b"11", b"?"] => Some(TerminalQuery::BackgroundColour),
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

/// Whether an `OSC 4` pair asks for a colour psmux answers: a `?` for one
/// of the sixteen ANSI colours, the ones it knows the outer terminal's
/// values for.
pub(crate) fn is_palette_query(index: u8, spec: &[u8]) -> bool {
    spec == b"?" && index < 16
}

/// Whether an OSC asks for any palette colour psmux answers.
fn asks_palette(params: &[&[u8]]) -> bool {
    match params {
        [b"4", rest @ ..] => palette_pairs(rest).any(|(i, spec)| is_palette_query(i, spec)),
        _ => false,
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
}

impl vte::Perform for ReplyRequests {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        self.pending |= csi_query(params, intermediates, ignore, action).is_some();
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        self.pending |= osc_query(params, self.program_status).is_some() || asks_palette(params);
    }
}
