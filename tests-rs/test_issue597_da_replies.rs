// Issue #597 (follow up): psmux answers DA1, DA2, DSR 5 and DECRQM itself,
// the way tmux does, so a console host that forwards those queries instead of
// answering them does not leave them hanging.
//
// Measured on Windows 11 26200 with the `Microsoft.Windows.Console.ConPTY`
// 1.24 package loaded through `PSMUX_CONPTY_DIR`, before this change:
//
//   * every pane launch stalled about three seconds, prompt visible at
//     3777 / 3747 / 3799 ms against 629 / 676 / 627 ms on the inbox host.  A
//     PSMUX_PANE_RAW capture shows why: OpenConsole 1.24 opens the stream with
//     `1b 5b 31 74 1b 5b 63` (`ESC[1t ESC[c`) and parks the child inside its
//     console connect until the DA1 answer comes back.
//   * tests/test_issue597_xtversion_reply.ps1 scored 6 passed 3 failed, with
//     `DA1 answer changed shape: []`, `DA2 ... []` and `DECRQM_2026 ... []`,
//     empty brackets meaning zero bytes.
//
// After: prompt at 817 / 766 / 772 ms and the suite 9 of 9, with DA1
// `1b 5b 3f 31 3b 32 63`, DA2 `1b 5b 3e 38 34 3b 30 3b 30 63` and DECRQM 2026
// `ESC[?2026;0$y`, which is byte for byte what the inbox host answers.
//
// tmux parity, `input.c` in the tmux checkout:
//   1581-1595  INPUT_CSI_DA            -> input_reply(ictx, 1, "\033[?1;2c")
//   1597-1608  INPUT_CSI_DA_TWO        -> input_reply(ictx, 1, "\033[>84;0;0c")
//   1722-1737  INPUT_CSI_DSR           -> "\033[0n" for 5, "\033[%u;%uR" for 6
//   1650-1721  INPUT_CSI_QUERY_PRIVATE -> "\033[?%d;%d$y" over a mode table
//   1637-1649  INPUT_CSI_QUERY         -> "\033[%d;%d$y" for ANSI modes
//   2123-2217  input_csi_dispatch_winops: `ESC[1t` hits a `case 1:` that only
//              breaks, so tmux answers no XTWINOPS 1 and neither does psmux.

use super::*;

/// The state of a freshly started pane: nothing set, cursor visible.
fn fresh() -> DecrqmState {
    DecrqmState { cursor_visible: true, ..DecrqmState::default() }
}

fn reply_for(bytes: &[u8], st: &DecrqmState) -> String {
    scan_device_queries(bytes)
        .into_iter()
        .map(|q| device_reply(q, st))
        .collect()
}

// ── the scanner: what counts as a query ───────────────────────────────────

#[test]
fn da1_bare_is_a_query() {
    assert_eq!(scan_device_queries(b"\x1b[c"), vec![DeviceQuery::Da1]);
}

#[test]
fn da1_with_explicit_zero_is_a_query() {
    // xterm and tmux both default an omitted parameter to 0, so `CSI 0 c` and
    // `CSI c` are the same question.
    assert_eq!(scan_device_queries(b"\x1b[0c"), vec![DeviceQuery::Da1]);
}

#[test]
fn da2_is_a_query() {
    assert_eq!(scan_device_queries(b"\x1b[>c"), vec![DeviceQuery::Da2]);
    assert_eq!(scan_device_queries(b"\x1b[>0c"), vec![DeviceQuery::Da2]);
}

#[test]
fn dsr_five_is_a_query() {
    assert_eq!(scan_device_queries(b"\x1b[5n"), vec![DeviceQuery::DsrStatus]);
}

#[test]
fn decrqm_private_and_ansi_are_queries() {
    assert_eq!(
        scan_device_queries(b"\x1b[?2026$p"),
        vec![DeviceQuery::DecrqmPrivate(2026)]
    );
    assert_eq!(
        scan_device_queries(b"\x1b[4$p"),
        vec![DeviceQuery::DecrqmAnsi(4)]
    );
}

#[test]
fn the_openconsole_opening_burst_yields_exactly_one_da1() {
    // The real bytes OpenConsole 1.24 opens a pane with, from a
    // PSMUX_PANE_RAW=1 capture on this box.
    let burst = b"\x1b[1t\x1b[c\x1b[?1004h\x1b[?9001h";
    assert_eq!(scan_device_queries(burst), vec![DeviceQuery::Da1]);
}

// ── what must NOT draw a reply ────────────────────────────────────────────

#[test]
fn dsr_six_is_left_to_the_cpr_responder() {
    // `CSI 6 n` is answered by CprScanner + helpers::drain_cpr_pending, which
    // knows the pane's real cursor position.  Answering it here as well would
    // hand the asking program two replies.
    assert!(scan_device_queries(b"\x1b[6n").is_empty());
}

#[test]
fn csi_996n_is_left_to_the_colour_responder() {
    assert!(scan_device_queries(b"\x1b[?996n").is_empty());
}

#[test]
fn a_non_zero_da_parameter_stays_unanswered() {
    // tmux's `switch (input_get(ictx, 0, 0, 0))` replies only for 0.
    assert!(scan_device_queries(b"\x1b[1c").is_empty());
    assert!(scan_device_queries(b"\x1b[>1c").is_empty());
}

#[test]
fn a_device_attributes_reply_is_not_mistaken_for_a_query() {
    // The answers themselves are CSI sequences ending in `c`.  If one ever
    // came back round the loop it must not start a reply storm: both carry a
    // non-zero first parameter, which is not a request to report.
    assert!(scan_device_queries(b"\x1b[?1;2c").is_empty());
    assert!(scan_device_queries(b"\x1b[>84;0;0c").is_empty());
    assert!(scan_device_queries(b"\x1b[?61;6;7;21;22;23;24;28;32;42c").is_empty());
}

#[test]
fn xtwinops_is_not_a_query() {
    // tmux's winops `case 1:` only breaks.  psmux leaves `ESC[1t` alone too,
    // and the three second stall still went away, which is what proves DA1 was
    // the sequence OpenConsole was waiting on.
    assert!(scan_device_queries(b"\x1b[1t").is_empty());
    assert!(scan_device_queries(b"\x1b[18t").is_empty());
}

#[test]
fn ordinary_output_draws_nothing() {
    let paint = b"\x1b[?25l\x1b[2J\x1b[m\x1b[H\x1b[38;5;12mhello\x1b[0m\x1b[?25h";
    assert!(scan_device_queries(paint).is_empty());
}

#[test]
fn a_decset_is_not_a_decrqm() {
    // `$p` asks; `h` and `l` set and reset and must stay silent.
    assert!(scan_device_queries(b"\x1b[?2026h\x1b[?2026l").is_empty());
    assert!(scan_device_queries(b"\x1b[?1049h\x1b[?1049l").is_empty());
}

#[test]
fn xtversion_is_not_swept_up_by_this_scanner() {
    // `CSI > 0 q` belongs to the XTVERSION responder in the reader thread.
    assert!(scan_device_queries(b"\x1b[>0q").is_empty());
}

#[test]
fn decrqm_with_mode_zero_draws_no_reply() {
    // tmux guards both DECRQM arms with `if (m > 0)`.
    let st = fresh();
    assert_eq!(reply_for(b"\x1b[?0$p", &st), "");
    assert_eq!(reply_for(b"\x1b[0$p", &st), "");
}

// ── the reply bytes, against tmux ─────────────────────────────────────────

#[test]
fn da1_reply_is_tmux_byte_for_byte() {
    // input.c:1589 input_reply(ictx, 1, "\033[?1;2c")
    assert_eq!(reply_for(b"\x1b[c", &fresh()), "\x1b[?1;2c");
}

#[test]
fn da2_reply_is_tmux_byte_for_byte() {
    // input.c:1602 input_reply(ictx, 1, "\033[>84;0;0c")
    assert_eq!(reply_for(b"\x1b[>c", &fresh()), "\x1b[>84;0;0c");
}

#[test]
fn dsr_five_reply_is_tmux_byte_for_byte() {
    // input.c:1727 input_reply(ictx, 1, "\033[0n")
    assert_eq!(reply_for(b"\x1b[5n", &fresh()), "\x1b[0n");
}

#[test]
fn decrqm_reports_reset_for_a_mode_that_is_off() {
    // tmux value semantics: 1 set, 2 reset.
    assert_eq!(reply_for(b"\x1b[?2004$p", &fresh()), "\x1b[?2004;2$y");
    assert_eq!(reply_for(b"\x1b[?1049$p", &fresh()), "\x1b[?1049;2$y");
    assert_eq!(reply_for(b"\x1b[?1006$p", &fresh()), "\x1b[?1006;2$y");
}

#[test]
fn decrqm_reports_set_for_a_mode_that_is_on() {
    let st = DecrqmState {
        cursor_visible: true,
        bracketed_paste: true,
        alternate_screen: true,
        mouse_sgr: true,
        mouse_all: true,
        application_cursor: true,
        ..DecrqmState::default()
    };
    assert_eq!(reply_for(b"\x1b[?2004$p", &st), "\x1b[?2004;1$y");
    assert_eq!(reply_for(b"\x1b[?1049$p", &st), "\x1b[?1049;1$y");
    assert_eq!(reply_for(b"\x1b[?47$p", &st), "\x1b[?47;1$y");
    assert_eq!(reply_for(b"\x1b[?1047$p", &st), "\x1b[?1047;1$y");
    assert_eq!(reply_for(b"\x1b[?1006$p", &st), "\x1b[?1006;1$y");
    assert_eq!(reply_for(b"\x1b[?1003$p", &st), "\x1b[?1003;1$y");
    assert_eq!(reply_for(b"\x1b[?1$p", &st), "\x1b[?1;1$y");
}

#[test]
fn decrqm_reports_the_cursor_mode() {
    assert_eq!(reply_for(b"\x1b[?25$p", &fresh()), "\x1b[?25;1$y");
    let hidden = DecrqmState { cursor_visible: false, ..DecrqmState::default() };
    assert_eq!(reply_for(b"\x1b[?25$p", &hidden), "\x1b[?25;2$y");
}

#[test]
fn decrqm_reports_deccolm_permanently_reset() {
    // input.c:1656-1658 `case 3: /* DECCOLM: always reset */ n = 4;`
    assert_eq!(reply_for(b"\x1b[?3$p", &fresh()), "\x1b[?3;4$y");
}

#[test]
fn decrqm_2026_reports_reset_like_tmux() {
    // psmux implements synchronized output (`SyncGate` in pane.rs), so it
    // answers 2 (supported, currently reset) the way tmux does.
    assert_eq!(reply_for(b"\x1b[?2026$p", &fresh()), "\x1b[?2026;2$y");
}

#[test]
fn decrqm_2026_reports_set_inside_an_open_update() {
    // tmux input.c: `case 2026: n = (s->mode & MODE_SYNC) ? 1 : 2;`
    let st = DecrqmState { synchronized_output: true, ..fresh() };
    assert_eq!(reply_for(b"\x1b[?2026$p", &st), "\x1b[?2026;1$y");
}

#[test]
fn decrqm_for_an_unknown_mode_reports_zero() {
    assert_eq!(reply_for(b"\x1b[?31337$p", &fresh()), "\x1b[?31337;0$y");
    // Every ANSI mode is unknown to psmux, including tmux's one tracked mode.
    assert_eq!(reply_for(b"\x1b[4$p", &fresh()), "\x1b[4;0$y");
}

#[test]
fn several_queries_in_one_batch_answer_in_order() {
    let st = fresh();
    assert_eq!(
        reply_for(b"\x1b[c\x1b[>c\x1b[5n\x1b[?2004$p", &st),
        "\x1b[?1;2c\x1b[>84;0;0c\x1b[0n\x1b[?2004;2$y"
    );
}

// ── split reads ───────────────────────────────────────────────────────────

#[test]
fn a_query_split_across_two_reads_is_still_answered() {
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"hello\x1b[").is_empty());
    assert_eq!(s.scan(b"c world"), vec![DeviceQuery::Da1]);
}

#[test]
fn a_decrqm_split_mid_parameter_is_still_answered() {
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"\x1b[?20").is_empty());
    assert_eq!(s.scan(b"26$p"), vec![DeviceQuery::DecrqmPrivate(2026)]);
}

#[test]
fn a_query_split_on_its_last_byte_is_answered_once() {
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"\x1b[>").is_empty());
    assert_eq!(s.scan(b"c"), vec![DeviceQuery::Da2]);
    // ...and not again on the next batch, which still carries it in the tail.
    assert!(s.scan(b"plain text").is_empty());
}

#[test]
fn a_query_that_ended_last_batch_is_not_answered_twice() {
    let mut s = DeviceQueryScanner::new();
    assert_eq!(s.scan(b"\x1b[c"), vec![DeviceQuery::Da1]);
    assert!(s.scan(b"x").is_empty());
    assert!(s.scan(b"y").is_empty());
}

#[test]
fn two_identical_queries_across_a_boundary_are_both_answered() {
    // One straddles the boundary, one sits wholly in the new batch.  The
    // boundary pass and the batch pass partition the stream by start offset,
    // so neither query is dropped and neither is counted twice.
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"\x1b[").is_empty());
    assert_eq!(s.scan(b"c\x1b[c"), vec![DeviceQuery::Da1, DeviceQuery::Da1]);
}

#[test]
fn a_long_batch_between_halves_forgets_the_stale_tail() {
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"\x1b[").is_empty());
    // A batch longer than KEEP resets the carried tail, so the orphaned
    // `ESC[` can no longer combine with anything.
    assert!(s.scan(b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").is_empty());
    assert!(s.scan(b"c").is_empty());
}

#[test]
fn a_batch_of_plain_text_costs_nothing_and_answers_nothing() {
    let mut s = DeviceQueryScanner::new();
    assert!(s.scan(b"no escapes here at all").is_empty());
}

// ── the hand off queue ────────────────────────────────────────────────────

#[test]
fn a_queued_reply_waits_for_its_pane_and_is_taken_once() {
    // Pane ids well past anything the rest of the suite uses, so this stays
    // independent of whatever else is running: `take_device_replies` only ever
    // removes entries for the id it is asked about.
    let mine = 9_597_001;
    let other = 9_597_002;
    assert_eq!(crate::types::take_device_replies(mine), None);

    crate::types::push_device_reply(mine, b"\x1b[?1;2c".to_vec());
    crate::types::push_device_reply(other, b"\x1b[0n".to_vec());
    // A pane that is not reachable yet, a warm spare still on its way to the
    // pool, must not consume another pane's answer.
    assert_eq!(crate::types::take_device_replies(9_597_003), None);
    // Several answers for one pane come back concatenated, in arrival order.
    crate::types::push_device_reply(mine, b"\x1b[>84;0;0c".to_vec());
    assert_eq!(
        crate::types::take_device_replies(mine),
        Some(b"\x1b[?1;2c\x1b[>84;0;0c".to_vec())
    );
    // ...and only once.
    assert_eq!(crate::types::take_device_replies(mine), None);
    assert_eq!(crate::types::take_device_replies(other), Some(b"\x1b[0n".to_vec()));
}

#[test]
fn an_empty_reply_is_never_queued() {
    let id = 9_597_004;
    crate::types::push_device_reply(id, Vec::new());
    assert_eq!(crate::types::take_device_replies(id), None);
}
