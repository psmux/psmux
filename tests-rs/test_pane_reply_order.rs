// A pane's queries are answered one reply per query, in the order they were
// asked, the way tmux answers each one as its parser meets it (input.c:1176 in
// tmux 3.8).  Issue #930 put the colour and XTVERSION replies of one read in
// query order; these cover every query psmux answers, DA1 and the window
// reports included, which used to go out on another path after them.
use super::*;
use crate::types::{HostColors, COLOR_QUERY_BG, COLOR_QUERY_FG, COLOR_QUERY_SCHEME};

fn texts(bytes: &[u8]) -> Vec<String> {
    let queries = scan_device_queries(bytes);
    let st = DecrqmState { rows: 30, cols: 120, ..Default::default() };
    pane_replies(&queries, &st, &HostColors::campbell(), &mut false)
        .into_iter()
        .map(|r| r.text)
        .collect()
}

fn xt() -> String { xtversion_reply() }
const DA1: &str = "\x1b[?1;2;4c";
const FG: &str = "\x1b]10;rgb:cccc/cccc/cccc\x1b\\";
const BG: &str = "\x1b]11;rgb:0c0c/0c0c/0c0c\x1b\\";
const DARK: &str = "\x1b[?997;1n";
const CELL: &str = "\x1b[6;20;10t";

#[test]
fn da1_asked_first_is_answered_first() {
    // Limit one the maintainer left on #931: the injected replies used to
    // come before the DA1 reply whatever the order asked.
    assert_eq!(texts(b"\x1b[c\x1b[>q\x1b]11;?\x1b\\"), vec![DA1.to_string(), xt(), BG.to_string()]);
}

#[test]
fn a_startup_probe_is_answered_in_its_own_order() {
    // What yazi sends at start up.
    assert_eq!(
        texts(b"\x1b[>q\x1b]11;?\x1b\\\x1b[16t\x1b[c"),
        vec![xt(), BG.to_string(), CELL.to_string(), DA1.to_string()]
    );
}

#[test]
fn colour_replies_follow_the_order_asked() {
    // Limit two: the colour replies used to come in a fixed order, the scheme
    // first and the palette by index.
    assert_eq!(texts(b"\x1b]11;?\x07\x1b[?996n"), vec![BG.to_string(), DARK.to_string()]);
    assert_eq!(
        texts(b"\x1b]4;2;?\x07\x1b]4;1;?\x07"),
        vec!["\x1b]4;2;rgb:1313/a1a1/0e0e\x1b\\".to_string(), "\x1b]4;1;rgb:c5c5/0f0f/1f1f\x1b\\".to_string()]
    );
}

#[test]
fn a_query_asked_twice_is_answered_twice() {
    assert_eq!(texts(b"\x1b[>q\x1b[c\x1b[>q"), vec![xt(), DA1.to_string(), xt()]);
}

#[test]
fn the_copilot_burst_gets_the_colours_the_host_dropped() {
    // The inbox console host drops OSC 10 and 11, so only `?996n` and the
    // palette reach psmux; the foreground and background replies go where the
    // burst asked for them, before index 0.
    let mut burst = String::from("\x1b[?996n");
    for i in 0..16 { burst.push_str(&format!("\x1b]4;{};?\x1b\\", i)); }
    let out = texts(burst.as_bytes());
    assert_eq!(out.len(), 19);
    assert_eq!(&out[..3], &[DARK.to_string(), FG.to_string(), BG.to_string()]);
    for (i, reply) in out[3..].iter().enumerate() {
        assert!(reply.starts_with(&format!("\x1b]4;{};rgb:", i)), "{reply:?}");
    }
}

#[test]
fn the_burst_does_not_answer_a_colour_twice() {
    // OpenConsole passes OSC 10 and 11 on, and then each is answered where
    // it was asked, once.
    let out = texts(b"\x1b]10;?\x07\x1b]11;?\x07\x1b]4;0;?\x07");
    assert_eq!(out.len(), 3);
    assert_eq!(&out[..2], &[FG.to_string(), BG.to_string()]);
    let out = texts(b"\x1b]11;?\x07\x1b]4;0;?\x07\x1b]4;0;?\x07");
    assert_eq!(out.iter().filter(|r| r.starts_with("\x1b]10;")).count(), 1);
    assert_eq!(out.iter().filter(|r| r.starts_with("\x1b]11;")).count(), 1);
    assert_eq!(out[1], FG, "the missing foreground goes before index 0");
}

#[test]
fn a_single_palette_index_gets_exactly_its_reply() {
    assert_eq!(texts(b"\x1b]4;5;?\x07"), vec!["\x1b]4;5;rgb:8888/1717/9898\x1b\\".to_string()]);
}

#[test]
fn the_panes_own_palette_wins() {
    assert_eq!(texts(b"\x1b]4;4;#000080\x07\x1b]4;4;?\x07"), vec!["\x1b]4;4;rgb:0000/0000/8080\x1b\\".to_string()]);
}

#[test]
fn every_query_pair_of_one_osc_4_is_answered_in_order() {
    // Master answered only the first pair, as its byte scanner matched
    // `ESC ] 4 ; 1 ; ?` and nothing after it.
    assert_eq!(
        texts(b"\x1b]4;2;?;1;?\x1b\\"),
        vec!["\x1b]4;2;rgb:1313/a1a1/0e0e\x1b\\".to_string(), "\x1b]4;1;rgb:c5c5/0f0f/1f1f\x1b\\".to_string()]
    );
}

#[test]
fn a_palette_reply_is_the_colour_when_asked_not_when_the_batch_ends() {
    // Set red, ask, set blue, ask, in one batch: tmux answers red then blue.
    assert_eq!(
        texts(b"\x1b]4;1;rgb:ff/00/00\x07\x1b]4;1;?\x07\x1b]4;1;rgb:00/00/ff\x07\x1b]4;1;?\x07"),
        vec!["\x1b]4;1;rgb:ffff/0000/0000\x1b\\".to_string(), "\x1b]4;1;rgb:0000/0000/ffff\x1b\\".to_string()]
    );
}

#[test]
fn each_reply_keeps_the_way_it_went_before_when_injection_fails() {
    let queries = scan_device_queries(b"\x1b[c\x1b[>q\x1b[?996n\x1b]10;?\x07\x1b]4;3;?\x07\x1b[14t");
    let out = pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), &mut false);
    let ways: Vec<ReplyFallback> = out.iter().map(|r| r.fallback).collect();
    assert_eq!(
        ways,
        vec![
            ReplyFallback::Pipe,
            ReplyFallback::Lost("XTVERSION"),
            ReplyFallback::Colour(COLOR_QUERY_SCHEME),
            ReplyFallback::Colour(COLOR_QUERY_FG),
            ReplyFallback::Colour(1 << 3),
            ReplyFallback::Pipe,
        ]
    );
    let bg = pane_replies(&scan_device_queries(b"\x1b]11;?\x07"), &DecrqmState::default(), &HostColors::campbell(), &mut false);
    assert_eq!(bg[0].fallback, ReplyFallback::Colour(COLOR_QUERY_BG));
}

#[test]
fn an_unknown_colour_has_no_reply() {
    let out = pane_replies(&scan_device_queries(b"\x1b]10;?\x07\x1b]4;1;?\x07\x1b[>q"), &DecrqmState::default(), &HostColors::empty(), &mut false);
    assert_eq!(out.into_iter().map(|r| r.text).collect::<Vec<_>>(), vec![xt()]);
}

/// For each reply, whether it answers the console host (the conhost or
/// OpenConsole of the pane) rather than the child.
fn console_hosts(bytes: &[u8], open: &mut bool) -> Vec<bool> {
    let queries = scan_device_queries(bytes);
    pane_replies(&queries, &DecrqmState::default(), &HostColors::campbell(), open)
        .into_iter()
        .map(|r| r.console_host)
        .collect()
}

#[test]
fn the_first_da1_of_a_pane_is_the_console_hosts() {
    // OpenConsole's opening `ESC[1t ESC[c`, then the child's own DA1 later.
    let mut open = true;
    assert_eq!(console_hosts(b"\x1b[1t\x1b[c\x1b[?1004h\x1b[?9001h", &mut open), vec![true]);
    assert!(!open);
    assert_eq!(console_hosts(b"\x1b[>q\x1b[c", &mut open), vec![false, false]);
}

#[test]
fn only_one_da1_is_taken_as_the_console_hosts() {
    let mut open = true;
    assert_eq!(console_hosts(b"\x1b[>q\x1b[c\x1b[c", &mut open), vec![false, true, false]);
    let mut open = true;
    assert_eq!(console_hosts(b"\x1b[>q\x1b[16t", &mut open), vec![false, false]);
    assert!(open, "a batch without DA1 leaves the console host's DA1 still to come");
}

#[test]
fn a_flood_of_colour_queries_gets_a_bounded_injection() {
    // Master folded a read's colour queries into one bit each, so a flood was
    // answered once a colour.  Answering every query must still stay bounded:
    // the injection holds the console lock every pane shares.
    let flood = b"\x1b]11;?\x07".repeat(8000);
    let out = texts(&flood);
    assert_eq!(out.len(), vt100::MAX_PENDING_QUERIES);
    let bytes: usize = out.iter().map(String::len).sum();
    assert!(bytes <= vt100::MAX_PENDING_QUERIES * BG.len(), "{bytes} bytes");
}
