// A palette question asked of the outer terminal (crate::palette_request).
//
// The pane side follows tmux's input request list (input.c at tag 3.8): a
// reply after an open question waits behind it (input_reply with `add`), the
// answer lets the replies before it and after it go up to the next open
// question (input_request_reply), and 500 ms lets anything go that has waited
// that long (input_request_timer_callback).

use super::*;
use crate::console_tap::{match_osc4_reply, KeyRec, Osc4Reply};
use crate::pane::{PaneReply, ReplyFallback};

fn reply(text: &str) -> PaneReply {
    PaneReply { text: text.to_string(), fallback: ReplyFallback::Pipe, console_host: false }
}

fn texts(replies: &[PaneReply]) -> Vec<&str> {
    replies.iter().map(|r| r.text.as_str()).collect()
}

fn ask(index: u8, bel: bool) -> ReplyItem {
    ReplyItem::Ask { index, bel }
}

fn ready(text: &str) -> ReplyItem {
    ReplyItem::Reply(reply(text))
}

#[test]
fn with_nothing_open_a_batch_goes_out_as_it_is() {
    let mut q = Requests::default();
    let (now, asks) = q.add(vec![ready("a"), ready("b")], Instant::now());
    assert_eq!(texts(&now), vec!["a", "b"]);
    assert!(asks.is_empty());
    assert!(q.is_empty());
    assert_eq!(q.deadline(), None);
}

#[test]
fn a_reply_after_a_question_waits_for_its_answer() {
    let t = Instant::now();
    let mut q = Requests::default();
    let (now, asks) = q.add(vec![ready("before"), ask(20, true), ready("DA1")], t);
    assert_eq!(texts(&now), vec!["before"]);
    assert_eq!(asks, vec![20]);
    // A later batch waits too, behind the open question.
    let (now, _) = q.add(vec![ready("later")], t);
    assert!(now.is_empty());
    let out = q.answer(20, Some((0, 0, 0xd7)));
    assert_eq!(texts(&out), vec!["\x1b]4;20;rgb:0000/0000/d7d7\x07", "DA1", "later"]);
    assert!(q.is_empty());
}

#[test]
fn the_answer_ends_the_way_its_query_did_and_cannot_fall_back() {
    let mut q = Requests::default();
    q.add(vec![ask(196, false)], Instant::now());
    let out = q.answer(196, Some((0xff, 0, 0)));
    assert_eq!(texts(&out), vec!["\x1b]4;196;rgb:ffff/0000/0000\x1b\\"]);
    assert_eq!(out[0].fallback, ReplyFallback::Lost("OSC 4"));
}

#[test]
fn replies_go_up_to_the_next_open_question() {
    let t = Instant::now();
    let mut q = Requests::default();
    let (_, asks) = q.add(vec![ask(20, true), ready("x"), ask(21, true), ready("y")], t);
    assert_eq!(asks, vec![20, 21]);
    let out = q.answer(20, Some((1, 2, 3)));
    assert_eq!(texts(&out), vec!["\x1b]4;20;rgb:0101/0202/0303\x07", "x"]);
    let out = q.answer(21, Some((4, 5, 6)));
    assert_eq!(texts(&out), vec!["\x1b]4;21;rgb:0404/0505/0606\x07", "y"]);
    assert!(q.is_empty());
}

#[test]
fn an_answer_for_a_later_question_drops_the_ones_before_it() {
    // input_request_reply frees the requests before the one answered.
    let t = Instant::now();
    let mut q = Requests::default();
    q.add(vec![ask(20, true), ready("x"), ask(21, true), ready("y")], t);
    let out = q.answer(21, Some((4, 5, 6)));
    assert_eq!(texts(&out), vec!["x", "\x1b]4;21;rgb:0404/0505/0606\x07", "y"]);
    assert!(q.is_empty());
    // A late answer for the dropped one finds nothing.
    assert!(q.answer(20, Some((1, 2, 3))).is_empty());
}

#[test]
fn no_answer_lets_the_replies_go_with_no_colour() {
    let mut q = Requests::default();
    q.add(vec![ask(20, true), ready("DA1")], Instant::now());
    assert_eq!(texts(&q.answer(20, None)), vec!["DA1"]);
    assert!(q.is_empty());
}

#[test]
fn half_a_second_lets_everything_go_that_has_waited_that_long() {
    let t = Instant::now();
    let mut q = Requests::default();
    q.add(vec![ask(20, true), ready("DA1")], t);
    assert_eq!(q.deadline(), Some(t + REQUEST_TIMEOUT));
    assert!(q.expire(t + REQUEST_TIMEOUT - Duration::from_millis(1)).is_empty());
    assert_eq!(texts(&q.expire(t + REQUEST_TIMEOUT)), vec!["DA1"]);
    assert!(q.is_empty());
    // A reply that came later times out later, as each entry has its time.
    let mut q = Requests::default();
    q.add(vec![ask(20, true)], t);
    q.add(vec![ready("late")], t + Duration::from_millis(300));
    assert!(q.expire(t + REQUEST_TIMEOUT).is_empty());
    assert!(!q.is_empty());
    assert_eq!(texts(&q.expire(t + Duration::from_millis(800))), vec!["late"]);
}

#[test]
fn the_console_hosts_own_da1_never_waits() {
    let mut q = Requests::default();
    let host = PaneReply { text: "host".into(), fallback: ReplyFallback::Pipe, console_host: true };
    let (now, _) = q.add(vec![ask(20, true), ReplyItem::Reply(host), ready("child")], Instant::now());
    assert_eq!(texts(&now), vec!["host"]);
    assert_eq!(texts(&q.answer(20, None)), vec!["child"]);
}

#[test]
fn a_pane_keeps_a_bounded_number_of_questions_open() {
    let mut q = Requests::default();
    let many: Vec<ReplyItem> = (0..MAX_OPEN + 10).map(|i| ask(16 + (i % 200) as u8, true)).collect();
    let (_, asks) = q.add(many, Instant::now());
    assert_eq!(asks.len(), MAX_OPEN);
}

#[test]
fn pane_items_ask_only_for_a_high_index_with_no_entry() {
    use crate::pane::{pane_items, scan_device_queries, DecrqmState};
    let queries = scan_device_queries(b"\x1b]4;1;?\x07\x1b]4;20;?\x07\x1b]4;21;#112233\x07\x1b]4;21;?\x1b\\\x1b[c");
    let items = pane_items(&queries, &DecrqmState::default(), &crate::types::HostColors::campbell(), &mut false, true);
    assert_eq!(items.len(), 4);
    assert!(matches!(&items[0], ReplyItem::Reply(r) if r.text == "\x1b]4;1;rgb:c5c5/0f0f/1f1f\x07"));
    assert_eq!(items[1], ask(20, true));
    assert!(matches!(&items[2], ReplyItem::Reply(r) if r.text == "\x1b]4;21;rgb:1111/2222/3333\x1b\\"));
    assert!(matches!(&items[3], ReplyItem::Reply(r) if r.text.starts_with("\x1b[?")));
    // With nobody to ask, index 20 has no reply, as before.
    let none = pane_items(&queries, &DecrqmState::default(), &crate::types::HostColors::campbell(), &mut false, false);
    assert_eq!(none.len(), 3);
    assert!(none.iter().all(|i| matches!(i, ReplyItem::Reply(_))));
}

#[test]
fn the_wire_forms_read_back() {
    assert_eq!(parse_directive("PALETTE-QUERY 20"), Some(20));
    assert_eq!(parse_directive("PALETTE-QUERY 256"), None);
    assert_eq!(parse_directive("SWITCH x"), None);
    assert_eq!(query_bytes(20), "\x1b]4;20;?\x1b\\");
    assert_eq!(reply_line(20, Some((0, 0, 0xd7))), "host-palette 20 0000d7\n");
    assert_eq!(reply_line(20, None), "host-palette 20 none\n");
    assert_eq!(parse_reply_args(&["20", "0000d7"]), Some((20, Some((0, 0, 0xd7)))));
    assert_eq!(parse_reply_args(&["20", "none"]), Some((20, None)));
    assert_eq!(parse_reply_args(&["20", "xyz"]), None);
    assert_eq!(parse_reply_args(&["300", "none"]), None);
}

#[test]
fn a_terminal_reply_reads_as_tty_keys_palette_reads_it() {
    assert_eq!(parse_reply("4;20;rgb:0000/0000/d7d7"), Some((20, (0, 0, 0xd7))));
    assert_eq!(parse_reply("4;255;rgb:ee/ee/ee"), Some((255, (0xee, 0xee, 0xee))));
    assert_eq!(parse_reply("4;1;#aa0000"), Some((1, (0xaa, 0, 0))));
    assert_eq!(parse_reply("4;20;?"), None);
    assert_eq!(parse_reply("10;rgb:aaaa/aaaa/aaaa"), None);
    assert_eq!(parse_reply("4;256;rgb:00/00/00"), None);
}

#[test]
fn a_client_answers_the_question_it_was_asked_first() {
    let mut app = crate::types::AppState::new("palette".to_string());
    // Two questions out to client 7; no inbox, so deliver goes nowhere.
    app.palette_requests.entry(7).or_default().extend([(1, 20), (2, 21)]);
    answer(&mut app, 7, 21, Some((1, 2, 3)));
    assert!(app.palette_requests.get(&7).is_none(), "21 answered and 20 before it dropped");
    app.palette_requests.entry(7).or_default().extend([(1, 20), (2, 21)]);
    answer(&mut app, 7, 30, None);
    assert_eq!(app.palette_requests.get(&7).map(|q| q.len()), Some(2), "an answer nobody asked for changes nothing");
    cancel_client(&mut app, 7);
    assert!(app.palette_requests.is_empty());
}

#[test]
fn an_answer_reaches_the_pane_that_asked() {
    let pane = 9_100_004usize;
    let wake: std::sync::Arc<(Mutex<Vec<u8>>, Condvar)> = Default::default();
    let ready = open_inbox(pane, &wake);
    deliver(pane, 20, Some((1, 2, 3)));
    deliver(pane + 1, 21, None);
    assert!(ready.load(Ordering::Acquire));
    assert_eq!(take_answers(pane), vec![(20, Some((1, 2, 3)))]);
    assert!(!ready.load(Ordering::Acquire));
    close_inbox(pane);
    deliver(pane, 20, None);
    assert!(take_answers(pane).is_empty());
}

fn down(u: u16) -> Option<KeyRec> {
    Some(KeyRec { key_down: true, vk: 0, u_char: u, ctrl_state: 0 })
}
fn up(u: u16) -> Option<KeyRec> {
    Some(KeyRec { key_down: false, vk: 0, u_char: u, ctrl_state: 0 })
}
fn records(s: &str) -> Vec<Option<KeyRec>> {
    s.encode_utf16().map(down).collect()
}

#[test]
fn the_console_route_takes_a_whole_reply() {
    let st = records("\x1b]4;20;rgb:0000/0000/d7d7\x1b\\");
    assert_eq!(
        match_osc4_reply(&st),
        Osc4Reply::Whole { records: st.len(), body: "4;20;rgb:0000/0000/d7d7".to_string() }
    );
    let bel = records("\x1b]4;1;rgb:aa/00/00\x07");
    assert_eq!(match_osc4_reply(&bel), Osc4Reply::Whole { records: bel.len(), body: "4;1;rgb:aa/00/00".to_string() });
    // Key up records between the characters are skipped and taken with it.
    let mut with_ups = Vec::new();
    for r in records("\x1b]4;9;#010203\x07") {
        with_ups.push(r);
        with_ups.push(up(r.map(|k| k.u_char).unwrap_or(0)));
    }
    assert_eq!(
        match_osc4_reply(&with_ups),
        Osc4Reply::Whole { records: with_ups.len() - 1, body: "4;9;#010203".to_string() }
    );
    // What follows the reply stays for crossterm.
    let mut then_key = records("\x1b]4;20;rgb:00/00/d7\x07");
    let n = then_key.len();
    then_key.extend(records("ab"));
    assert!(matches!(match_osc4_reply(&then_key), Osc4Reply::Whole { records, .. } if records == n));
}

#[test]
fn the_console_route_waits_for_a_reply_still_arriving() {
    assert_eq!(match_osc4_reply(&records("\x1b")), Osc4Reply::Partial);
    assert_eq!(match_osc4_reply(&records("\x1b]4;20;rgb:00")), Osc4Reply::Partial);
    assert_eq!(match_osc4_reply(&records("\x1b]4;20;rgb:00/00/d7\x1b")), Osc4Reply::Partial);
}

#[test]
fn the_console_route_leaves_keys_and_other_sequences_alone() {
    // Not OSC 4.
    assert_eq!(match_osc4_reply(&records("\x1b]10;rgb:aa/aa/aa\x07")), Osc4Reply::No);
    assert_eq!(match_osc4_reply(&records("\x1b[A")), Osc4Reply::No);
    // A key someone pressed has a virtual key.
    let mut pressed = records("\x1b]4;");
    pressed.push(Some(KeyRec { key_down: true, vk: 0x41, u_char: u16::from(b'a'), ctrl_state: 0 }));
    assert_eq!(match_osc4_reply(&pressed), Osc4Reply::No);
    // A mouse or focus record in the middle.
    let mut mixed = records("\x1b]4;20");
    mixed.push(None);
    assert_eq!(match_osc4_reply(&mixed), Osc4Reply::No);
    // Longer than tmux reads.
    let long = format!("\x1b]4;20;{}\x07", "a".repeat(200));
    assert_eq!(match_osc4_reply(&records(&long)), Osc4Reply::No);
}
