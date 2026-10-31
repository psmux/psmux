// Issue #597: psmux answers XTVERSION (`CSI > q`) for the programs inside a pane.
//
// Measured before the fix (tests/query_probe_child.cs in a pane of a detached
// session): DA1, DA2, DSR and DECRQM all come back, because the ConPTY host
// answers those itself and never forwards them, while XTVERSION comes back with
// zero bytes.  The query is not lost on the way in -- PSMUX_PANE_RAW=1 shows
// `1b 5b 3e 30 71` sitting in the pane's raw output stream -- it simply had
// nobody to answer it.  Claude Code asks three times at startup and logs
// "no XTVERSION reply".
//
// tmux answers it from its own parser (input.c, INPUT_CSI_XDA at line 1884:
// `input_reply(ictx, 1, "\033P>|tmux %s\033\\", getversion())`, dispatched by
// the `{ 'q', ">", INPUT_CSI_XDA }` table entry at line 343), and only when the
// parameter is absent or 0.  These tests pin the same rule.

use super::*;

/// The pane's query capture (its vt100 parser), reduced to "did these bytes
/// ask for XTVERSION".
fn asks_xtversion(bytes: &[u8]) -> bool {
    scan_device_queries(bytes).contains(&DeviceQuery::Xtversion)
}

/// The same across reads, with the parser state carried from one to the next.
struct XtversionScanner(DeviceQueryScanner);

impl XtversionScanner {
    fn new() -> Self { Self(DeviceQueryScanner::new()) }
    fn scan(&mut self, bytes: &[u8]) -> bool { self.0.scan(bytes).contains(&DeviceQuery::Xtversion) }
}

// ── which queries deserve a reply ─────────────────────────────────────────

#[test]
fn answers_xtversion_with_explicit_zero() {
    assert!(asks_xtversion(b"\x1b[>0q"));
}

#[test]
fn answers_xtversion_without_parameter() {
    // xterm defaults an omitted Ps to 0, and so does tmux's input_get().
    assert!(asks_xtversion(b"\x1b[>q"));
}

#[test]
fn answers_xtversion_embedded_in_a_startup_burst() {
    // The exact bytes Claude Code 2.1.274 emits around its query, taken from a
    // PSMUX_PANE_RAW capture of a pane running `claude`.
    let burst = b"\x1b[?2004h\x1b[?2031h\x1b[?1004h\x1b[<u\x1b[>5u\x1b[>4;2m\x1b[>0q\x1b[>4m\x1b[<u";
    assert!(asks_xtversion(burst));
}

#[test]
fn answers_first_parameter_zero_in_a_list() {
    assert!(asks_xtversion(b"\x1b[>0;0q"));
}

#[test]
fn ignores_non_zero_parameter() {
    // `CSI > 1 q` is not a version request; xterm and tmux both stay silent.
    assert!(!asks_xtversion(b"\x1b[>1q"));
    assert!(!asks_xtversion(b"\x1b[>2q"));
}

#[test]
fn ignores_decscusr_which_has_no_private_marker() {
    // `CSI Ps SP q` (set cursor style) shares the final byte but not the `>`.
    assert!(!asks_xtversion(b"\x1b[5 q"));
    assert!(!asks_xtversion(b"\x1b[0 q"));
}

#[test]
fn ignores_other_private_sequences_claude_code_sends() {
    // Kitty keyboard push/pop and modifyOtherKeys end in u and m, not q.
    assert!(!asks_xtversion(b"\x1b[>5u"));
    assert!(!asks_xtversion(b"\x1b[<u"));
    assert!(!asks_xtversion(b"\x1b[>4;2m"));
    assert!(!asks_xtversion(b"\x1b[>c"));
}

#[test]
fn ignores_decrqm_so_psmux_stays_silent_like_tmux_before_3_6() {
    // DECRQM never reaches psmux anyway (ConPTY answers `CSI ? 2026 $ p` with
    // `CSI ? 2026 ; 0 $ y` itself), and the query capture must not mistake it
    // for a version query.
    assert!(!asks_xtversion(b"\x1b[?2026$p"));
    assert!(!asks_xtversion(b"\x1b[?1006$p"));
}

#[test]
fn ignores_plain_text_and_empty_input() {
    assert!(!asks_xtversion(b""));
    assert!(!asks_xtversion(b"PS C:\\Users\\godwin> q\r\n"));
    assert!(!asks_xtversion(b"\x1b[?1049l"));
}

#[test]
fn keeps_scanning_after_a_rejected_query() {
    // A `CSI > 1 q` earlier in the batch must not stop the scan before the real
    // query that follows it.
    assert_eq!(scan_device_queries(b"\x1b[>1q\x1b[>0q"), vec![DeviceQuery::Xtversion]);
}

// ── the reply bytes ───────────────────────────────────────────────────────

#[test]
fn reply_is_a_dcs_wrapped_tmux_identity() {
    let reply = xtversion_reply();
    let expected = format!("\x1bP>|tmux {}\x1b\\", crate::types::VERSION);
    assert_eq!(reply, expected, "reply bytes must match tmux's XTVERSION form");
}

#[test]
fn reply_opens_with_dcs_and_closes_with_st() {
    let reply = xtversion_reply();
    assert!(reply.starts_with("\x1bP>|"), "reply must start with DCS > | : {:?}", reply);
    assert!(reply.ends_with("\x1b\\"), "reply must end with ST : {:?}", reply);
    // No stray control bytes in between, or the asking program drops the reply.
    let body = &reply[3..reply.len() - 2];
    assert!(
        body.bytes().all(|b| b >= 0x20 && b < 0x7f),
        "reply body must be printable ASCII: {:?}",
        body
    );
}

#[test]
fn reply_carries_the_version_psmux_v_prints() {
    assert!(xtversion_reply().contains(crate::types::VERSION));
}

// ── split across reads ────────────────────────────────────────────────────

#[test]
fn scanner_detects_a_query_split_across_two_reads() {
    let mut scanner = XtversionScanner::new();
    assert!(!scanner.scan(b"some output\x1b[>"));
    assert!(scanner.scan(b"0q more output"));
}

#[test]
fn scanner_detects_a_query_split_one_byte_from_the_end() {
    let mut scanner = XtversionScanner::new();
    assert!(!scanner.scan(b"\x1b[>0"));
    assert!(scanner.scan(b"q"));
}

#[test]
fn scanner_does_not_invent_a_query_from_two_halves_of_different_sequences() {
    let mut scanner = XtversionScanner::new();
    assert!(!scanner.scan(b"\x1b[>4;2m"));
    assert!(!scanner.scan(b"hello world"));
}

#[test]
fn scanner_reports_each_new_query_once_per_batch() {
    let mut scanner = XtversionScanner::new();
    assert!(scanner.scan(b"\x1b[>0q"));
    assert!(!scanner.scan(b"plain output with no queries at all"));
    assert!(scanner.scan(b"\x1b[>q"));
}
