use crate::TerminalQuery;

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
        _ => None,
    }
}

pub(crate) fn osc_query(params: &[&[u8]], program_status: bool) -> Option<TerminalQuery> {
    match params {
        [b"7501", b"?"] if program_status => Some(TerminalQuery::ProgramStatus),
        _ => None,
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
        self.pending |= osc_query(params, self.program_status).is_some();
    }
}
