// XTSMGRAPHICS, `CSI ? item ; action ; value S`. tmux answers it itself
// (input_csi_dispatch_sm_graphics, input.c:2103 in tmux 3.8). The replies below
// were measured from tmux 3.8. A pane's programs are told sixel is there only
// when its console host passes DA1 on to psmux, so a pane on the inbox console
// host, which answers DA1 itself without sixel, gets a failure for every one.
use super::*;
use crate::types::HostColors;

const COLOUR_REGISTERS: &str = "\x1b[?1;0;1024S";

/// The replies to `bytes`, with `da1_open` as the pane's console host's own
/// DA1 still to come, which on the inbox console host it always is.
fn replies(bytes: &[u8], da1_open: bool) -> Vec<String> {
    let queries = scan_device_queries(bytes);
    let mut open = da1_open;
    pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), &mut open)
        .into_iter()
        .map(|r| r.text)
        .collect()
}

fn listed(bytes: &[u8]) -> Vec<String> { replies(bytes, false) }
fn not_listed(bytes: &[u8]) -> Vec<String> { replies(bytes, true) }

#[test]
fn the_colour_registers_read_as_tmux_reads_them() {
    for bytes in [
        &b"\x1b[?1;1;0S"[..],
        b"\x1b[?1;2;0S",
        b"\x1b[?1;4;0S",
        // The value is not read for these, whatever it holds.
        b"\x1b[?1;1S",
        b"\x1b[?1;1;999S",
        b"\x1b[?1;1;0:1S",
    ] {
        assert_eq!(listed(bytes), vec![COLOUR_REGISTERS], "{bytes:?}");
    }
}

#[test]
fn anything_else_fails_with_the_value_asked_as_tmux_answers() {
    for (bytes, want) in [
        (&b"\x1b[?1;3;256S"[..], "\x1b[?1;3;256S"),
        (b"\x1b[?1;0;0S", "\x1b[?1;3;0S"),
        (b"\x1b[?1;5;0S", "\x1b[?1;3;0S"),
        (b"\x1b[?2;1;0S", "\x1b[?2;3;0S"),
        (b"\x1b[?2;4;0S", "\x1b[?2;3;0S"),
        (b"\x1b[?2;1;7S", "\x1b[?2;3;7S"),
        (b"\x1b[?3;1;0S", "\x1b[?3;3;0S"),
        (b"\x1b[?4;1;0S", "\x1b[?4;3;0S"),
        (b"\x1b[?0;1;0S", "\x1b[?0;3;0S"),
    ] {
        assert_eq!(listed(bytes), vec![want], "{bytes:?}");
    }
}

#[test]
fn missing_parameters_read_as_zero() {
    for (bytes, want) in [
        (&b"\x1b[?S"[..], "\x1b[?0;3;0S"),
        (b"\x1b[?1S", "\x1b[?1;3;0S"),
        (b"\x1b[?;1;0S", "\x1b[?0;3;0S"),
        (b"\x1b[?1;;5S", "\x1b[?1;3;5S"),
    ] {
        assert_eq!(listed(bytes), vec![want], "{bytes:?}");
    }
}

#[test]
fn a_parameter_with_a_colon_reads_as_minus_one() {
    for (bytes, want) in [
        (&b"\x1b[?1:2;1;0S"[..], "\x1b[?-1;3;0S"),
        (b"\x1b[?1;1:2;0S", "\x1b[?1;3;0S"),
        (b"\x1b[?1;2:1;9S", "\x1b[?1;3;9S"),
    ] {
        assert_eq!(listed(bytes), vec![want], "{bytes:?}");
    }
}

#[test]
fn a_parameter_past_65535_reads_as_65535() {
    // The parser's parameters are 16 bits. tmux reads up to INT_MAX, so it
    // answers 70000 with 70000 and drops 3000000000; this is the one difference.
    assert_eq!(listed(b"\x1b[?70000;1;0S"), vec!["\x1b[?65535;3;0S"]);
    assert_eq!(listed(b"\x1b[?2;1;70000S"), vec!["\x1b[?2;3;65535S"]);
    assert_eq!(listed(b"\x1b[?1;1;70000S"), vec![COLOUR_REGISTERS]);
}

#[test]
fn more_than_three_parameters_get_no_reply() {
    for bytes in [&b"\x1b[?1;1;0;0S"[..], b"\x1b[?1;1;0;S"] {
        assert!(scan_device_queries(bytes).is_empty(), "{bytes:?}");
    }
}

#[test]
fn csi_s_without_the_question_mark_is_not_a_query() {
    assert!(scan_device_queries(b"\x1b[1;1;0S").is_empty());
    assert!(scan_device_queries(b"\x1b[S").is_empty());
}

#[test]
fn replies_keep_the_order_asked() {
    assert_eq!(
        listed(b"\x1b[?1;1;0S\x1b[?2;1;0S"),
        vec![COLOUR_REGISTERS, "\x1b[?2;3;0S"]
    );
    assert_eq!(
        listed(b"\x1b[?1;1;0S\x1b[c"),
        vec![COLOUR_REGISTERS, "\x1b[?1;2;4c"]
    );
    assert_eq!(
        listed(b"\x1b[c\x1b[?1;1;0S"),
        vec!["\x1b[?1;2;4c", COLOUR_REGISTERS]
    );
}

#[test]
fn a_pane_told_no_sixel_gets_a_failure_for_every_one() {
    for (bytes, want) in [
        (&b"\x1b[?1;1;0S"[..], "\x1b[?1;3;0S"),
        (b"\x1b[?1;2;0S", "\x1b[?1;3;0S"),
        (b"\x1b[?1;4;0S", "\x1b[?1;3;0S"),
        (b"\x1b[?1;1;999S", "\x1b[?1;3;999S"),
        (b"\x1b[?1;3;256S", "\x1b[?1;3;256S"),
        (b"\x1b[?2;1;0S", "\x1b[?2;3;0S"),
        (b"\x1b[?S", "\x1b[?0;3;0S"),
    ] {
        assert_eq!(not_listed(bytes), vec![want], "{bytes:?}");
    }
    assert!(not_listed(b"\x1b[?1;1;0;0S").is_empty());
}

#[test]
fn the_console_hosts_own_da1_is_what_says_sixel_is_listed() {
    // OpenConsole opens a pane with its DA1, so by the time a program can ask,
    // the pane has seen it. It stays seen for the rest of the pane's life.
    let st = DecrqmState::default();
    let colours = HostColors::campbell();
    let mut open = true;

    let first = pane_replies(&scan_device_queries(b"\x1b[1t\x1b[c"), &st, &colours, &mut open);
    assert_eq!(first.len(), 1);
    assert!(first[0].console_host);
    assert!(!open);

    for _ in 0..2 {
        let later = pane_replies(&scan_device_queries(b"\x1b[?1;1;0S"), &st, &colours, &mut open);
        assert_eq!(later[0].text, COLOUR_REGISTERS);
        assert!(!later[0].console_host, "this reply is the child's");
    }
}

#[test]
fn the_reply_falls_back_to_the_pipe_as_the_other_csi_replies_do() {
    let queries = scan_device_queries(b"\x1b[?1;1;0S");
    let got = pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), &mut false);
    assert_eq!(got[0].fallback, ReplyFallback::Pipe);
}
