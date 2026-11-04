// user-keys on the VT and pipe input routes (crate::user_keys): VtParser
// holds the characters that start a `user-keys` sequence and turns a whole
// sequence into the key UserN, and everything else decodes as before. Each
// parser here gets its own fixed table, so no other test sees it.

use super::*;
use crossterm::event::{Event, KeyCode, KeyModifiers};

const TMUX_EXAMPLE: &str = "\x1b[5;30012~"; // tmux.1's user-keys example

fn parse_with(table: &[(u32, &str)], input: &str) -> (Vec<Event>, VtParser) {
    let mut p = VtParser::with_user_keys(table);
    let mut events = Vec::new();
    for ch in input.chars() {
        p.feed(ch, &mut |e| events.push(e));
    }
    (events, p)
}

fn keys(events: &[Event]) -> Vec<(KeyCode, KeyModifiers)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Key(k) => Some((k.code, k.modifiers)),
            _ => None,
        })
        .collect()
}

fn user(n: u32) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(crate::user_keys::char_for(n).unwrap()), KeyModifiers::NONE)
}

#[test]
fn a_whole_sequence_is_the_user_key() {
    let (events, p) = parse_with(&[(0, TMUX_EXAMPLE)], TMUX_EXAMPLE);
    assert_eq!(keys(&events), vec![user(0)]);
    assert!(!p.has_pending_escape(), "nothing is left held");
    assert_eq!(p.state, PS::Ground);
}

#[test]
fn the_same_sequence_decodes_as_before_without_user_keys() {
    let mut p = VtParser::new();
    let mut events = Vec::new();
    for ch in TMUX_EXAMPLE.chars() {
        p.feed(ch, &mut |e| events.push(e));
    }
    assert!(!keys(&events).contains(&user(0)), "{:?}", events);
}

#[test]
fn other_keys_still_decode_with_user_keys_set() {
    let table = [(0, TMUX_EXAMPLE), (1, "\x1bOz")];
    for (input, want) in [
        ("\x1b[A", (KeyCode::Up, KeyModifiers::NONE)),
        ("\x1b[5~", (KeyCode::PageUp, KeyModifiers::NONE)),
        ("\x1bOP", (KeyCode::F(1), KeyModifiers::NONE)),
        ("a", (KeyCode::Char('a'), KeyModifiers::NONE)),
    ] {
        let (events, _) = parse_with(&table, input);
        assert_eq!(keys(&events), vec![want], "{:?}", input);
    }
    // A sequence that shares its first characters with a user key, then
    // leaves it: the held characters go through the state machine in order.
    let (events, _) = parse_with(&table, "\x1b[5;2~");
    assert_eq!(keys(&events), vec![(KeyCode::PageUp, KeyModifiers::SHIFT)]);
}

#[test]
fn a_lone_escape_is_held_then_flushed_as_escape() {
    let (events, mut p) = parse_with(&[(0, TMUX_EXAMPLE)], "\x1b");
    assert!(events.is_empty());
    assert!(p.has_pending_escape(), "the reader waits escape-time for more");
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![(KeyCode::Esc, KeyModifiers::NONE)]);
    assert!(!p.has_pending_escape());
}

#[test]
fn a_partial_sequence_that_times_out_goes_on_as_typed() {
    // `\x1bOz` is user key 1 only once the `z` arrives; `\x1bO` alone, when
    // nothing follows, goes to the state machine (an SS3 waiting for its
    // final byte), and the next key completes it as before.
    let (events, mut p) = parse_with(&[(1, "\x1bOz")], "\x1bO");
    assert!(events.is_empty());
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    p.feed('P', &mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![(KeyCode::F(1), KeyModifiers::NONE)]);
}

#[test]
fn a_sequence_another_one_extends_waits_then_is_the_shorter_key() {
    let table = [(3, "\x1bOz"), (4, "\x1bOzz")];
    let (events, mut p) = parse_with(&table, "\x1bOz");
    assert!(events.is_empty(), "the longer one may still come");
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![user(3)]);
    let (events, _) = parse_with(&table, "\x1bOzz");
    assert_eq!(keys(&events), vec![user(4)]);
}

#[test]
fn back_to_back_user_keys_and_text() {
    let table = [(0, TMUX_EXAMPLE), (2, "\x1b[99~")];
    let input = format!("a{TMUX_EXAMPLE}\x1b[99~b");
    let (events, _) = parse_with(&table, &input);
    assert_eq!(
        keys(&events),
        vec![(KeyCode::Char('a'), KeyModifiers::NONE), user(0), user(2), (KeyCode::Char('b'), KeyModifiers::NONE)]
    );
}

#[test]
fn a_user_key_with_no_escape() {
    // tmux takes any string; a plain one matches typed text too.
    let ch = |c| (KeyCode::Char(c), KeyModifiers::NONE);
    let (events, p) = parse_with(&[(5, "zq")], "azqz");
    assert_eq!(keys(&events), vec![ch('a'), user(5)]);
    assert!(p.has_pending_escape(), "the last z is held as the start of another zq");
    let (events, _) = parse_with(&[(5, "zq")], "azqzx");
    assert_eq!(keys(&events), vec![ch('a'), user(5), ch('z'), ch('x')]);
}

#[test]
fn nothing_inside_a_bracketed_paste_is_a_user_key() {
    let input = format!("\x1b[200~x{TMUX_EXAMPLE}y\x1b[201~");
    let (events, _) = parse_with(&[(0, TMUX_EXAMPLE)], &input);
    assert!(!keys(&events).contains(&user(0)), "{:?}", events);
    let pasted: Vec<&Event> = events.iter().filter(|e| matches!(e, Event::Paste(_))).collect();
    assert_eq!(pasted.len(), 1, "{:?}", events);
}

#[test]
fn what_is_held_is_bounded_by_the_longest_sequence() {
    let (_, p) = parse_with(&[(0, TMUX_EXAMPLE)], "\x1b[5;3001");
    assert!(p.uk_held.chars().count() <= TMUX_EXAMPLE.chars().count());
    let (_, p) = parse_with(&[(0, TMUX_EXAMPLE)], "\x1b[5;30012345678");
    assert!(p.uk_held.is_empty(), "a character that leaves every sequence releases the rest");
}

#[test]
fn escape_and_a_sequence_is_the_meta_user_key() {
    // tmux reads ESC and then a key as that key with Meta (tty_keys_next), so
    // ESC and a user-keys sequence is M-UserN, the key `bind -n M-User2` names.
    let meta = |n| (user(n).0, KeyModifiers::ALT);
    let (events, p) = parse_with(&[(2, "\x1b[99~")], "\x1b\x1b[99~");
    assert_eq!(keys(&events), vec![meta(2)]);
    assert!(!p.has_pending_escape());
    let (events, _) = parse_with(&[(5, "zq")], "\x1bzq");
    assert_eq!(keys(&events), vec![meta(5)]);
    // Alt and a letter, and ESC ESC, decode as before.
    let (events, _) = parse_with(&[(2, "\x1b[99~")], "\x1bx");
    assert_eq!(keys(&events), vec![(KeyCode::Char('x'), KeyModifiers::ALT)]);
    let (_, mut p) = parse_with(&[(2, "\x1b[99~")], "\x1b\x1b");
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    let mut q = VtParser::new();
    let mut before = Vec::new();
    for ch in "\x1b\x1b".chars() {
        q.feed(ch, &mut |e| before.push(e));
    }
    q.flush_escape(&mut |e| before.push(e));
    assert_eq!(keys(&flushed), keys(&before));
}

#[test]
fn a_user_key_takes_over_a_sequence_psmux_knows() {
    // tty_keys_add replaces the key of a sequence already in the tree, so a
    // user key wins over the built-in one (tty-keys.c:442-448).
    let (events, _) = parse_with(&[(7, "\x1b[5;2~")], "\x1b[5;2~");
    assert_eq!(keys(&events), vec![user(7)]);
}

#[test]
fn a_sequence_that_starts_inside_a_failed_one_is_still_found() {
    // With abc and bx set, abx is the text a and then the key of bx: the
    // held text gives up only its first character and the rest is matched
    // again, as tmux's tty_keys_next takes one key off its buffer at a time.
    let ch = |c| (KeyCode::Char(c), KeyModifiers::NONE);
    let table = [(0, "abc"), (1, "bx")];
    let (events, p) = parse_with(&table, "abx");
    assert_eq!(keys(&events), vec![ch('a'), user(1)]);
    assert!(!p.has_pending_escape());
    // The same when the held text times out: ab is a then the start of bx,
    // which waits once more and is then the text b.
    let (events, mut p) = parse_with(&table, "ab");
    assert!(events.is_empty());
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![ch('a')]);
    assert!(p.has_pending_escape(), "b may still be the start of bx");
    p.flush_escape(&mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![ch('a'), ch('b')]);
    // And a whole key inside the timed out text is that key.
    let (_, mut p) = parse_with(&[(0, "abc"), (2, "b")], "ab");
    let mut flushed = Vec::new();
    p.flush_escape(&mut |e| flushed.push(e));
    assert_eq!(keys(&flushed), vec![ch('a'), user(2)]);
}

#[test]
fn a_long_sequence_that_fails_late_gives_its_text_back_in_order() {
    // aaa...ab set and aaa...ax typed: each start is tried again in a loop,
    // not by recursion, and every character comes out once, in order.
    let n = 300;
    let seq = format!("{}b", "a".repeat(n));
    let input = format!("{}x", "a".repeat(n));
    let (events, p) = parse_with(&[(0, seq.as_str())], &input);
    let ch = |c| (KeyCode::Char(c), KeyModifiers::NONE);
    let mut want = vec![ch('a'); n];
    want.push(ch('x'));
    assert_eq!(keys(&events), want);
    assert!(!p.has_pending_escape());
}
