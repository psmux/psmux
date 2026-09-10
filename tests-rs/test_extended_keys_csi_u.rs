//! Extended keys (`CSI u`) never reached a pane; their bytes did, as text.
//!
//! A terminal cannot report Shift+Enter in legacy VT — CR is CR whether or not
//! Shift is down — so every terminal that can report it uses the
//! modifyOtherKeys / fixterms form `CSI 13;2u`.  That is what `set -s
//! extended-keys always` asks a terminal for, and what a Windows Terminal
//! `sendInput` keybinding writes.
//!
//! Windows never delivered it.  psmux's local input source reads console
//! KEY_EVENT records, not bytes, and conhost's input parser does not know
//! `CSI u`: it flushes the unrecognised sequence into the input buffer one
//! record per byte.  Measured with `examples/csi_u_diag.rs` under Windows
//! Terminal on Windows 11 26200, one press of Shift+Enter delivers seven
//! records in a SINGLE `ReadConsoleInputW`:
//!
//! ```text
//!   DOWN vk=0x00 scan=0x00 uChar=0x001b        <- ESC
//!   DOWN vk=0x00 scan=0x00 uChar=0x005b  '['
//!   DOWN vk=0x00 scan=0x00 uChar=0x0031  '1'
//!   DOWN vk=0x00 scan=0x00 uChar=0x0033  '3'
//!   DOWN vk=0x00 scan=0x00 uChar=0x003b  ';'
//!   DOWN vk=0x00 scan=0x00 uChar=0x0032  '2'
//!   DOWN vk=0x00 scan=0x00 uChar=0x0075  'u'
//! ```
//!
//! Two facts in there decide the whole design.
//!
//! 1. The ESC does not survive.  It has no virtual key code and a `uChar`
//!    inside the control range, so crossterm hands that record to
//!    `ToUnicodeEx`, which answers nothing for a synthesised record, and drops
//!    the event (`event/sys/windows/parse.rs`).  psmux is handed six bare
//!    characters; its paste heuristic sees three or more ASCII characters
//!    inside 20 ms, calls it a paste, and the child application receives the
//!    TEXT `[13;2u` instead of a newline.  So the `[` is the only introducer
//!    left to key off.
//!
//! 2. They arrive in one batch.  Every byte behind the `[` is ALREADY queued
//!    when the `[` is handed over, so the decision needs no timer and no guess
//!    about typing speed: pull what is there, and a `[` a person typed comes
//!    back empty on the first pull and goes straight through.
//!
//! tmux does not have this problem because `tty-keys.c` parses its client tty
//! as bytes.  Reassembly is unconditional there and here: `extended-keys`
//! governs what tmux WRITES to a pane, never what it accepts from a terminal.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{EscCoalesce, ESC_COALESCE_MS};

fn press(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Press,
        state: KeyEventState::empty(),
    })
}

fn release(code: KeyCode, mods: KeyModifiers) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers: mods,
        kind: KeyEventKind::Release,
        state: KeyEventState::empty(),
    })
}

fn key_of(ev: &Event) -> KeyEvent {
    match ev {
        Event::Key(k) => *k,
        other => panic!("expected a key event, got {:?}", other),
    }
}

/// A console batch: the events the terminal has already delivered.  Feeding the
/// first one and letting the coalescer pull the rest is exactly what the live
/// input source does.
struct Batch(VecDeque<Event>);

impl Batch {
    /// Build the batch a console produces for `seq`, minus the leading ESC that
    /// crossterm drops.
    fn from_sequence(seq: &str) -> Self {
        let mut q = VecDeque::new();
        let mut chars = seq.chars();
        assert_eq!(chars.next(), Some('\u{1b}'), "a CSI sequence starts with ESC");
        for ch in chars {
            q.push_back(press(KeyCode::Char(ch), KeyModifiers::NONE));
        }
        Batch(q)
    }

    fn of(events: Vec<Event>) -> Self {
        Batch(events.into())
    }

    /// Feed the batch's first event to `c`, with the remainder available to
    /// pull, and return what came out.
    fn feed_into(&mut self, c: &mut EscCoalesce, now: Instant) -> Option<Event> {
        let first = self.0.pop_front().expect("a batch is never empty");
        let rest = &mut self.0;
        c.feed_with(first, now, || rest.pop_front())
    }

    fn is_drained(&self) -> bool {
        self.0.is_empty()
    }
}

/// Everything `c` will hand the client, in order, after `first` is fed with
/// `rest` available to pull — and whatever was left unpulled.
///
/// The leftover is not a loss: those events are still sitting in the terminal's
/// own queue and the input loop reads them on its next turn.  What must never
/// happen is an event being pulled and then not handed on, because THAT one the
/// caller can no longer re-read.
fn drain(
    c: &mut EscCoalesce,
    first: Event,
    rest: Vec<Event>,
    now: Instant,
) -> (Vec<Event>, Vec<Event>) {
    let mut queued: VecDeque<Event> = rest.into();
    let mut out = Vec::new();
    if let Some(ev) = c.feed_with(first, now, || queued.pop_front()) {
        out.push(ev);
    }
    while let Some(ev) = c.pop() {
        out.push(ev);
    }
    (out, queued.into_iter().collect())
}

fn codes(events: &[Event]) -> Vec<KeyCode> {
    events.iter().map(|e| key_of(e).code).collect()
}

/// Run a byte sequence through the VT parser — the input path SSH, WezTerm and
/// the JetBrains terminals use, where psmux reads real bytes instead of console
/// records.
fn parse_vt(seq: &str) -> Vec<Event> {
    let mut p = super::VtParser::new();
    let mut events = Vec::new();
    for ch in seq.chars() {
        p.feed(ch, &mut |evt| events.push(evt));
    }
    events
}

// ─────────────────────────────────────────────────────────────────────────────
// The reported key
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn csi_13_2_u_becomes_one_shift_enter() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();

    let mut batch = Batch::from_sequence("\x1b[13;2u");
    let out = batch
        .feed_into(&mut c, t0)
        .expect("the sequence must produce a key");

    let k = key_of(&out);
    assert_eq!(k.code, KeyCode::Enter);
    assert_eq!(k.modifiers, KeyModifiers::SHIFT);
    assert_eq!(k.kind, KeyEventKind::Press);

    assert!(batch.is_drained(), "the whole sequence must be consumed");
    assert!(c.pop().is_none(), "nothing may be left over");
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
}

/// The whole point of the exercise: the key the reporter's terminal sends has
/// to leave psmux as the two bytes the pane needs.  `\x1b\r` is what psmux
/// already writes for a Shift+Enter it recognises, and what Claude Code reads
/// as "insert a newline" — a real Alt+Enter produces a newline there too.
#[cfg(windows)]
#[test]
fn the_reassembled_key_encodes_to_esc_cr() {
    let mut c = EscCoalesce::new(true);
    let out = Batch::from_sequence("\x1b[13;2u")
        .feed_into(&mut c, Instant::now())
        .unwrap();
    let bytes = crate::input::encode_key_event(&key_of(&out)).expect("Enter must encode");
    assert_eq!(bytes, b"\x1b\r".to_vec(), "got {:02x?}", bytes);
}

/// A console that DOES deliver the ESC must not end up sending it on ahead of
/// the key: the held Escape is the sequence's introducer and goes with it.
#[test]
fn a_surviving_escape_is_consumed_by_the_sequence() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();

    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());

    let mut batch = Batch::from_sequence("\x1b[13;2u");
    let out = batch.feed_into(&mut c, t0).expect("must produce a key");
    assert_eq!(key_of(&out).code, KeyCode::Enter);
    assert_eq!(key_of(&out).modifiers, KeyModifiers::SHIFT);

    assert!(c.pop().is_none(), "the Escape must not follow the key out");
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
    assert!(c.deadline_ms(t0).is_none());
}

/// Every modifier combination the parameter can carry, so the decode is not
/// just "2 happens to mean Shift".
#[test]
fn the_modifier_parameter_is_decoded() {
    for (param, want) in [
        (1u8, KeyModifiers::NONE),
        (2, KeyModifiers::SHIFT),
        (3, KeyModifiers::ALT),
        (5, KeyModifiers::CONTROL),
        (6, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
        (7, KeyModifiers::CONTROL | KeyModifiers::ALT),
        (
            8,
            KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
        ),
    ] {
        let mut c = EscCoalesce::new(true);
        let out = Batch::from_sequence(&format!("\x1b[13;{}u", param))
            .feed_into(&mut c, Instant::now())
            .expect("must produce a key");
        assert_eq!(key_of(&out).modifiers, want, "parameter {}", param);
        assert_eq!(key_of(&out).code, KeyCode::Enter);
    }
}

/// A code point with no modifier parameter at all is still a key.
#[test]
fn a_bare_code_point_needs_no_modifier_parameter() {
    let mut c = EscCoalesce::new(true);
    let out = Batch::from_sequence("\x1b[13u")
        .feed_into(&mut c, Instant::now())
        .expect("must produce a key");
    assert_eq!(key_of(&out).code, KeyCode::Enter);
    assert_eq!(key_of(&out).modifiers, KeyModifiers::NONE);
}

/// The named keys must arrive as their `KeyCode`, not as a `Char` holding the
/// same code point: the client's key tables, its paste heuristic and
/// `encode_key_event` all match on `KeyCode::Enter`, `Tab`, `Backspace`.
#[test]
fn control_code_points_map_to_named_keys() {
    for (code, want) in [
        (8u32, KeyCode::Backspace),
        (9, KeyCode::Tab),
        (13, KeyCode::Enter),
        (27, KeyCode::Esc),
        (127, KeyCode::Backspace),
        (32, KeyCode::Char(' ')),
        (97, KeyCode::Char('a')),
    ] {
        let mut c = EscCoalesce::new(true);
        let out = Batch::from_sequence(&format!("\x1b[{};5u", code))
            .feed_into(&mut c, Instant::now())
            .expect("must produce a key");
        assert_eq!(key_of(&out).code, want, "code point {}", code);
    }
}

/// The release records a console may interleave with the presses must not break
/// the sequence apart.
#[test]
fn key_releases_inside_a_sequence_are_ignored() {
    let mut events = Vec::new();
    for ch in ['[', '1', '3', ';', '2', 'u'] {
        events.push(press(KeyCode::Char(ch), KeyModifiers::NONE));
        events.push(release(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let mut c = EscCoalesce::new(true);
    let out = Batch::of(events)
        .feed_into(&mut c, Instant::now())
        .expect("the sequence must still complete");
    assert_eq!(key_of(&out).code, KeyCode::Enter);
    assert_eq!(key_of(&out).modifiers, KeyModifiers::SHIFT);
}

/// A Kitty protocol RELEASE report is dropped rather than forwarded.  psmux
/// never asks a terminal for release reporting, and forwarding one would be
/// worse than dropping it: a release of Enter is exactly what the client's
/// WezTerm workaround promotes back into a press, which would double the key.
#[test]
fn a_kitty_release_report_is_dropped() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let mut batch = Batch::from_sequence("\x1b[13;2:3u");
    assert!(batch.feed_into(&mut c, t0).is_none());
    assert!(batch.is_drained());
    assert!(c.pop().is_none());
    assert!(c.deadline_ms(t0).is_none(), "nothing may still be held");
}

// ─────────────────────────────────────────────────────────────────────────────
// Everything that is NOT an extended key still behaves exactly as before
// ─────────────────────────────────────────────────────────────────────────────

/// The regression this design exists to avoid: `[` is an ordinary character in
/// every editor, and typing one must cost nothing.  With an empty queue behind
/// it the pull comes back empty at once and the character goes straight out —
/// no hold, nothing queued, no deadline armed.
#[test]
fn a_typed_bracket_passes_straight_through() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();

    let (out, left) = drain(&mut c, press(KeyCode::Char('['), KeyModifiers::NONE), vec![], t0);
    assert_eq!(codes(&out), vec![KeyCode::Char('[')]);
    assert!(left.is_empty());
    assert!(c.deadline_ms(t0).is_none(), "nothing may be held");
}

/// `[` followed by ordinary typing keeps every character, in order.
///
/// The recovery stops pulling the moment the characters cannot be a sequence,
/// so `b` is still in the terminal's queue and the input loop reads it next
/// turn.  `a` was pulled, and therefore had to come back out here.
#[test]
fn a_bracket_followed_by_typing_keeps_every_character() {
    let mut c = EscCoalesce::new(true);
    let (out, left) = drain(
        &mut c,
        press(KeyCode::Char('['), KeyModifiers::NONE),
        vec![
            press(KeyCode::Char('a'), KeyModifiers::NONE),
            press(KeyCode::Char('b'), KeyModifiers::NONE),
        ],
        Instant::now(),
    );
    assert_eq!(codes(&out), vec![KeyCode::Char('['), KeyCode::Char('a')]);
    assert_eq!(codes(&left), vec![KeyCode::Char('b')], "still readable");
}

/// A sequence psmux does not decode is handed back byte for byte, in order.
/// `CSI 1;5A` is Ctrl+Up — conhost translates that one itself, so it never
/// arrives as loose characters, and recognising it here would only add a second
/// decoder for input that cannot reach this code.
#[test]
fn an_undecoded_final_byte_replays_every_character() {
    let mut c = EscCoalesce::new(true);
    let mut batch = Batch::from_sequence("\x1b[1;5A");
    let first = batch
        .feed_into(&mut c, Instant::now())
        .expect("the abandoned sequence must start coming out");
    let mut out = vec![first];
    while let Some(ev) = c.pop() {
        out.push(ev);
    }
    assert_eq!(
        codes(&out),
        vec![
            KeyCode::Char('['),
            KeyCode::Char('1'),
            KeyCode::Char(';'),
            KeyCode::Char('5'),
            KeyCode::Char('A'),
        ]
    );
}

/// A held Escape in front of an abandoned sequence still comes out first.
#[test]
fn a_held_escape_precedes_an_abandoned_sequence() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());

    let (out, left) = drain(
        &mut c,
        press(KeyCode::Char('['), KeyModifiers::NONE),
        vec![press(KeyCode::Char('a'), KeyModifiers::NONE)],
        t0,
    );
    assert_eq!(
        codes(&out),
        vec![KeyCode::Esc, KeyCode::Char('['), KeyCode::Char('a')]
    );
    assert!(left.is_empty());
    assert!(c.deadline_ms(t0).is_none(), "the Escape is no longer held");
}

/// A non-character event inside a sequence abandons it, and nothing is lost.
#[test]
fn a_foreign_event_inside_a_sequence_replays_it() {
    let mut c = EscCoalesce::new(true);
    let (out, left) = drain(
        &mut c,
        press(KeyCode::Char('['), KeyModifiers::NONE),
        vec![
            press(KeyCode::Char('1'), KeyModifiers::NONE),
            Event::Resize(80, 24),
        ],
        Instant::now(),
    );
    assert_eq!(out.len(), 3);
    assert_eq!(key_of(&out[0]).code, KeyCode::Char('['));
    assert_eq!(key_of(&out[1]).code, KeyCode::Char('1'));
    assert!(matches!(out[2], Event::Resize(80, 24)));
    assert!(left.is_empty());
}

/// A run of characters long enough to be a stream rather than a key is
/// abandoned rather than collected forever, and nothing that was pulled to
/// reach that conclusion is lost.
#[test]
fn an_overlong_parameter_run_is_abandoned() {
    const SENT: usize = 64;
    let mut c = EscCoalesce::new(true);
    let rest: Vec<Event> = (0..SENT)
        .map(|_| press(KeyCode::Char('1'), KeyModifiers::NONE))
        .collect();
    let (out, left) = drain(
        &mut c,
        press(KeyCode::Char('['), KeyModifiers::NONE),
        rest,
        Instant::now(),
    );
    assert!(
        out.len() < SENT,
        "the run must be abandoned, not collected whole"
    );
    assert_eq!(
        out.len() + left.len(),
        SENT + 1,
        "the `[` plus every character sent, split between handed on and still readable"
    );
    assert_eq!(key_of(&out[0]).code, KeyCode::Char('['));
    assert!(c.deadline_ms(Instant::now()).is_none());
}

/// A terminal query reply (`CSI ? … u`) is not a key.
#[test]
fn a_private_parameter_sequence_is_not_a_key() {
    let mut c = EscCoalesce::new(true);
    let mut batch = Batch::from_sequence("\x1b[?1u");
    let first = batch.feed_into(&mut c, Instant::now()).expect("replayed");
    let mut out = vec![first];
    while let Some(ev) = c.pop() {
        out.push(ev);
    }
    assert_eq!(
        codes(&out),
        vec![
            KeyCode::Char('['),
            KeyCode::Char('?'),
            KeyCode::Char('1'),
            KeyCode::Char('u'),
        ]
    );
}

/// A `[` that carries Ctrl or Alt is a key in its own right (Ctrl+[ IS Escape
/// on a real keyboard) and must never open a sequence.
#[test]
fn a_modified_bracket_opens_nothing() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    for mods in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        let out = c
            .feed_with(press(KeyCode::Char('['), mods), t0, || {
                panic!("a modified `[` must not pull")
            })
            .expect("must pass straight through");
        assert_eq!(key_of(&out).modifiers, mods);
        assert_eq!(key_of(&out).code, KeyCode::Char('['));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The Escape behaviour this shares its state with (#611) is unchanged
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn esc_then_enter_still_merges() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    let out = c
        .feed(press(KeyCode::Enter, KeyModifiers::NONE), t0)
        .expect("the pair must produce one event");
    assert!(key_of(&out).modifiers.contains(KeyModifiers::ALT));
    assert_eq!(key_of(&out).code, KeyCode::Enter);
}

#[test]
fn a_lone_escape_still_comes_out_after_the_window() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    assert!(c
        .expire(t0 + Duration::from_millis(ESC_COALESCE_MS - 1))
        .is_none());
    let out = c
        .expire(t0 + Duration::from_millis(ESC_COALESCE_MS))
        .expect("the window has run out");
    assert_eq!(key_of(&out).code, KeyCode::Esc);
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
}

/// Disabled (the Unix build, where crossterm's own VT parser decodes `CSI u`
/// before psmux ever sees it), nothing is held or reassembled.
#[test]
fn disabled_coalescer_does_not_reassemble() {
    let mut c = EscCoalesce::new(false);
    let t0 = Instant::now();
    let out = c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).unwrap();
    assert_eq!(key_of(&out).code, KeyCode::Esc);
    let out = c
        .feed_with(press(KeyCode::Char('['), KeyModifiers::NONE), t0, || {
            panic!("a disabled coalescer must not pull")
        })
        .unwrap();
    assert_eq!(key_of(&out).code, KeyCode::Char('['));
    assert!(c.deadline_ms(t0).is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// The SSH / VT input path had the same gap, with a worse symptom
// ─────────────────────────────────────────────────────────────────────────────

/// Over SSH — and under WezTerm and the JetBrains terminals, which
/// `needs_vt_input()` routes the same way — psmux reads real bytes and runs its
/// own VT parser.  That parser knew every CSI final byte except `u`, so an
/// extended key fell through to its `_ => {}` discard and the keystroke was
/// lost outright: a terminal-side Shift+Enter binding did nothing at all, with
/// not even stray text to show for it.
#[test]
fn the_vt_parser_decodes_csi_u_too() {
    let out = parse_vt("\x1b[13;2u");
    assert_eq!(out.len(), 1, "one key, got {:?}", out);
    assert_eq!(key_of(&out[0]).code, KeyCode::Enter);
    assert_eq!(key_of(&out[0]).modifiers, KeyModifiers::SHIFT);
}

/// The same named-key mapping as the console path, so the two never disagree
/// about what `CSI 9;5u` is.
#[test]
fn the_vt_parser_agrees_on_named_keys() {
    for (seq, want, mods) in [
        ("\x1b[9;5u", KeyCode::Tab, KeyModifiers::CONTROL),
        ("\x1b[127;5u", KeyCode::Backspace, KeyModifiers::CONTROL),
        ("\x1b[13u", KeyCode::Enter, KeyModifiers::NONE),
        ("\x1b[97;5u", KeyCode::Char('a'), KeyModifiers::CONTROL),
    ] {
        let out = parse_vt(seq);
        assert_eq!(out.len(), 1, "{:?} produced {:?}", seq, out);
        assert_eq!(key_of(&out[0]).code, want, "{:?}", seq);
        assert_eq!(key_of(&out[0]).modifiers, mods, "{:?}", seq);
    }
}
