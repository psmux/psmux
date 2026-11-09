// Issue #269 - PROOF OF FIX: OSC 9;4 (Windows Terminal progress indicator)
// sequences are now captured by the vt100 emulator and exposed via
// `Screen::progress()` so the psmux server can forward them to the host
// terminal as part of dump-state, and the client can re-emit them.
//
// Before the fix, the OSC 9;4 dispatch arm did not exist; every sequence
// fell through to the empty default `unhandled_osc` callback. After the
// fix:
//   1. `Screen` carries an `osc94_progress: Option<(u8, u8)>` field.
//   2. `osc_dispatch` pattern-matches `[b"9", b"4", rest @ ..]`, reads the
//      fields as tmux's `input_osc_9` does, and stores the pair.
//   3. The `Callbacks::set_progress` hook is invoked so a custom callback
//      can react if needed.
//   4. `Screen::progress()` returns `Some((state, value))` once any OSC 9;4
//      has been received (including state=0 "hide"), so the server can
//      forward "clear" too.
//
// These tests exercise the SAME parser psmux uses (vt100::Parser) and prove:
//   - OSC 0 / 2 (titles) still round-trip correctly (regression guard).
//   - OSC 7 (path) still works (regression guard).
//   - OSC 9;4 with all 5 states is captured and surfaces via Screen::progress().
//   - The literal '9;4' bytes are not visible in pane contents (still a
//     valid OSC sequence consumed by the state machine, not displayed).
//   - Other channels (title, path, bell, squelch) are untouched.
//   - BEL-terminated and ST-terminated forms both work.
//   - Cross-chunk feeding of OSC 9;4 is correctly stitched.
//
// Run with: cargo test --test test_issue269_osc94_dropped -- --nocapture

const ST: &[u8] = b"\x1b\\";

fn osc94(state: u8, progress: u8) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"\x1b]9;4;");
    v.extend_from_slice(state.to_string().as_bytes());
    v.push(b';');
    v.extend_from_slice(progress.to_string().as_bytes());
    v.extend_from_slice(ST);
    v
}

fn osc0_title(title: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"\x1b]0;");
    v.extend_from_slice(title.as_bytes());
    v.extend_from_slice(ST);
    v
}

fn osc2_title(title: &str) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"\x1b]2;");
    v.extend_from_slice(title.as_bytes());
    v.extend_from_slice(ST);
    v
}

fn fresh_parser() -> vt100::Parser {
    vt100::Parser::new(24, 80, 0)
}

// =============================================================================
// PART A: Regression guard — OSC 0 / OSC 2 / OSC 7 still work.
// =============================================================================

#[test]
fn baseline_osc0_title_is_captured() {
    let mut p = fresh_parser();
    p.process(&osc0_title("hello-osc-0"));
    assert_eq!(p.screen().title(), "hello-osc-0");
}

#[test]
fn baseline_osc2_title_is_captured() {
    let mut p = fresh_parser();
    p.process(&osc2_title("hello-osc-2"));
    assert_eq!(p.screen().title(), "hello-osc-2");
}

#[test]
fn baseline_osc7_path_is_captured() {
    let mut p = fresh_parser();
    p.process(b"\x1b]7;file:///c:/foo\x1b\\");
    assert!(p.screen().path().is_some());
    let got = p.screen().path().unwrap();
    assert!(
        got.contains("c:/foo") || got.contains("c%3A/foo") || got.contains("foo"),
        "OSC 7 path missing the path portion, got: {:?}",
        got
    );
}

// =============================================================================
// PART B: The fix — OSC 9;4 is now captured and exposed via Screen::progress()
// =============================================================================

#[test]
fn fix_initial_progress_is_none() {
    let p = fresh_parser();
    assert_eq!(
        p.screen().progress(),
        None,
        "Fresh Screen must report None before any OSC 9;4 is received"
    );
}

#[test]
fn fix_osc94_default_state_is_captured() {
    // state=1 (default), progress=50 — the most common case.
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    assert_eq!(
        p.screen().progress(),
        Some((1, 50)),
        "OSC 9;4;1;50 must surface as Screen::progress() == Some((1, 50))"
    );
}

#[test]
fn fix_osc94_error_state_is_captured() {
    let mut p = fresh_parser();
    p.process(&osc94(2, 75));
    assert_eq!(p.screen().progress(), Some((2, 75)));
}

#[test]
fn fix_osc94_indeterminate_state_is_captured() {
    let mut p = fresh_parser();
    p.process(&osc94(3, 0));
    assert_eq!(p.screen().progress(), Some((3, 0)));
}

#[test]
fn fix_osc94_warning_state_is_captured() {
    let mut p = fresh_parser();
    p.process(&osc94(4, 90));
    assert_eq!(p.screen().progress(), Some((4, 90)));
}

#[test]
fn fix_osc94_hide_state_is_captured() {
    // state=0 (hide) MUST also surface so the client can clear the host
    // terminal's progress indicator. If we returned None here, a "clear"
    // sequence would be silently dropped on the forward path.
    let mut p = fresh_parser();
    p.process(&osc94(0, 0));
    assert_eq!(
        p.screen().progress(),
        Some((0, 0)),
        "state=0 (hide) MUST be captured so the clear is forwarded to host"
    );
}

#[test]
fn fix_osc94_overwrites_previous_state() {
    // Sequential OSC 9;4 calls should overwrite, not stack.
    let mut p = fresh_parser();
    p.process(&osc94(1, 25));
    assert_eq!(p.screen().progress(), Some((1, 25)));
    p.process(&osc94(1, 50));
    assert_eq!(p.screen().progress(), Some((1, 50)));
    p.process(&osc94(2, 60));
    assert_eq!(p.screen().progress(), Some((2, 60)));
    p.process(&osc94(0, 0));
    assert_eq!(p.screen().progress(), Some((0, 0)), "clear path");
}

#[test]
fn fix_osc94_ignores_out_of_range_state() {
    // tmux's input_osc_9 ignores a state past 4 (input.c:3030 at tag 3.8).
    let mut p = fresh_parser();
    p.process(&osc94(1, 25));
    p.process(&osc94(99, 50));
    assert_eq!(p.screen().progress(), Some((1, 25)));
}

#[test]
fn fix_osc94_ignores_out_of_range_progress() {
    // And a value past 100.
    let mut p = fresh_parser();
    p.process(&osc94(1, 25));
    p.process(&osc94(1, 200));
    assert_eq!(p.screen().progress(), Some((1, 25)));
}

#[test]
fn fix_osc94_with_bel_terminator_also_captured() {
    // OSC may end with BEL (0x07) instead of ST. Both must work.
    let bytes = b"\x1b]9;4;1;50\x07";
    let mut p = fresh_parser();
    p.process(bytes);
    assert_eq!(p.screen().progress(), Some((1, 50)));
    assert!(
        !p.screen_mut().take_audible_bell(),
        "BEL terminator of an OSC must NOT count as audible bell"
    );
}

#[test]
fn fix_chunked_osc94_is_stitched() {
    // Real PTY data arrives in chunks. The OSC may be split anywhere.
    let mut p = fresh_parser();
    p.process(b"\x1b]9;4;1;");
    assert_eq!(p.screen().progress(), None, "before terminator: not yet committed");
    p.process(b"50\x1b\\");
    assert_eq!(p.screen().progress(), Some((1, 50)), "after terminator: committed");
}

// =============================================================================
// PART C: Side-effect isolation — OSC 9;4 must not pollute other channels.
// =============================================================================

#[test]
fn fix_osc94_does_not_appear_in_screen_contents() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    let contents = p.screen().contents();
    assert!(!contents.contains("9;4"), "literal '9;4' leaked into contents: {:?}", contents);
    assert!(!contents.contains("\x1b]"), "ESC ] leaked into contents");
}

#[test]
fn fix_osc94_does_not_set_title() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    assert_eq!(p.screen().title(), "", "OSC 9;4 must not write title");
}

#[test]
fn fix_osc94_does_not_set_path() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    assert_eq!(p.screen().path(), None, "OSC 9;4 must not write path");
}

#[test]
fn fix_osc94_does_not_set_squelch_cleared() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    assert!(!p.screen_mut().take_squelch_cleared(), "OSC 9;4 vs OSC 9999 must be distinct");
}

#[test]
fn fix_osc94_does_not_ring_bell() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    assert!(!p.screen_mut().take_audible_bell());
}

#[test]
fn fix_osc94_state_machine_ready_for_next_sequence() {
    let mut p = fresh_parser();
    p.process(&osc94(1, 50));
    p.process(b"hello");
    assert!(p.screen().contents().contains("hello"));
}

#[test]
fn fix_osc94_does_not_set_alternate_screen() {
    let mut p = fresh_parser();
    let before = p.screen().alternate_screen();
    p.process(&osc94(1, 50));
    assert_eq!(p.screen().alternate_screen(), before);
}

// =============================================================================
// PART D: Side-by-side proof — title AND progress now both round-trip.
// =============================================================================

#[test]
fn fix_side_by_side_osc0_and_osc94_both_round_trip() {
    let mut p = fresh_parser();
    p.process(&osc0_title("set-by-osc0"));
    p.process(&osc94(2, 75));

    assert_eq!(p.screen().title(), "set-by-osc0");
    assert_eq!(p.screen().progress(), Some((2, 75)));
    // No cross-pollution.
    assert!(!p.screen().contents().contains("set-by-osc0"));
    assert!(!p.screen().contents().contains("9;4"));
}

#[test]
fn fix_progress_barrage_yields_final_state() {
    // Real-world: an app that emits a stream of progress sequences.
    // The final reported state must equal the last OSC 9;4 it sent.
    let mut p = fresh_parser();
    let barrage = [
        (3u8, 0u8),    // start indeterminate
        (1, 10),
        (1, 25),
        (1, 50),
        (1, 75),
        (1, 100),
        (0, 0),        // hide
    ];
    for (s, v) in &barrage {
        p.process(&osc94(*s, *v));
    }
    assert_eq!(p.screen().progress(), Some((0, 0)), "final state from barrage");
}

// =============================================================================
// PART E: Regression guards for orderings.
// =============================================================================

#[test]
fn regression_guard_osc0_then_osc94_then_osc2() {
    let mut p = fresh_parser();
    p.process(&osc0_title("first-title"));
    assert_eq!(p.screen().title(), "first-title");
    p.process(&osc94(1, 50));
    assert_eq!(p.screen().title(), "first-title", "OSC 9;4 must not clobber title");
    assert_eq!(p.screen().progress(), Some((1, 50)));
    p.process(&osc2_title("second-title"));
    assert_eq!(p.screen().title(), "second-title");
    assert_eq!(p.screen().progress(), Some((1, 50)), "OSC 2 must not clobber progress");
}

#[test]
fn regression_guard_chunked_osc94_does_not_break_subsequent_osc0() {
    let mut p = fresh_parser();
    p.process(b"\x1b]9;4;1;");
    p.process(b"50\x1b\\");
    p.process(&osc0_title("post-chunked-osc94"));
    assert_eq!(p.screen().title(), "post-chunked-osc94");
    assert_eq!(p.screen().progress(), Some((1, 50)));
}
