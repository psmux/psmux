//! Issue #611: Shift+Enter degrades to plain Enter on a loaded host.
//!
//! Two defects, both measured against real consoles before anything was
//! changed.
//!
//! 1. `augment_enter_shift` decided Enter's modifiers by polling
//!    `GetAsyncKeyState(VK_SHIFT)` when the client got round to PROCESSING the
//!    event, not when the event was GENERATED.  A poll cannot answer a
//!    question about the past, so on a busy host the Shift was already
//!    released and Shift+Enter became Enter (submit, not newline).  Worse, the
//!    poll also overwrote modifiers the event DID carry: a genuine `Alt+Enter`
//!    (which is what ConPTY makes of the bytes `1b 0d`) had its ALT stripped
//!    whenever the physical Shift happened to still be down.
//!
//!    Measured on Windows 11 26200, crossterm 0.29:
//!
//!    ```text
//!      real Shift+Enter typed into Windows Terminal, at the ConPTY child:
//!        REC DOWN vk=0x0D scan=0x1C uChar=0x000D ctrl=0x0010 [SHIFT]
//!      crossterm reports:
//!        KEY code=Enter mods=KeyModifiers(SHIFT) kind=Press
//!    ```
//!
//!    The event carries the answer, so the poll is now consulted only for a
//!    completely unmodified Enter, where nothing else is left.
//!
//! 2. An ESC immediately followed by a CR (what VS Code's xterm.js and the
//!    Windows Terminal `sendInput` keybinding from Claude Code's
//!    `/terminal-setup` both send for Shift+Enter) was forwarded to the pane
//!    as two independent keys.  Feeding a pseudoconsole showed why the pair
//!    can arrive as two key events at all:
//!
//!    ```text
//!      HEX 1b 0d  (one write)  -> REC DOWN vk=0x0D uChar=0x000D ctrl=0x0002 [LALT]
//!      HEX 1b     then
//!      HEX 0d     (two writes) -> REC DOWN vk=0x1B ... then REC DOWN vk=0x0D ctrl=0x0000
//!    ```
//!
//!    and the pane child then received `1b` and `0d` in two separate reads,
//!    so a readline style app saw a lone ESC (cancel) followed by CR (submit).
//!    `EscCoalesce` folds the pair back into one `Alt+Enter`, exactly the way
//!    tmux's `tty_keys_next` folds `\033` plus a byte into one key with
//!    `KEYC_META` (tty-keys.c) which `input_key_write` then writes as the
//!    `\033` prefix followed by the key's own bytes (input-keys.c).

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
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

// ─────────────────────────────────────────────────────────────────────────────
// Claim A: the event's modifiers beat the hardware poll
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod event_time_modifiers {
    use super::*;
    use crate::platform::augment_enter_shift_with;

    /// A Shift+Enter that Windows Terminal reported properly must survive
    /// untouched, and must never reach the poll at all.
    #[test]
    fn shift_from_the_record_is_kept_without_polling() {
        let mut k = key_of(&press(KeyCode::Enter, KeyModifiers::SHIFT));
        let mut polled = false;
        augment_enter_shift_with(&mut k, || {
            polled = true;
            false
        });
        assert_eq!(k.modifiers, KeyModifiers::SHIFT);
        assert!(!polled, "the poll must not run when the event already answered");
    }

    /// The regression this issue is about: `1b 0d` becomes an `Alt+Enter`
    /// record, and the old code stripped that ALT whenever the stale poll said
    /// Shift was down.  ALT is what `encode_key_event` turns into `\x1b\r`, so
    /// losing it loses the newline.
    #[test]
    fn alt_from_the_record_survives_a_shift_poll() {
        let mut k = key_of(&press(KeyCode::Enter, KeyModifiers::ALT));
        augment_enter_shift_with(&mut k, || true); // poll claims Shift is down
        assert!(
            k.modifiers.contains(KeyModifiers::ALT),
            "a poll must not strip a modifier the event carried; got {:?}",
            k.modifiers
        );
        assert_eq!(k.modifiers, KeyModifiers::ALT);
    }

    /// Ctrl+Shift+Enter keeps both: the poll used to drop CONTROL as a
    /// "phantom" whenever Ctrl had already been released.
    #[test]
    fn ctrl_shift_from_the_record_is_not_rewritten() {
        let mut k = key_of(&press(
            KeyCode::Enter,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        augment_enter_shift_with(&mut k, || false);
        assert_eq!(k.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT);
    }

    /// The fallback is still there for terminals that cannot encode a modified
    /// Return and just send `\r`: with nothing in the event, the poll is the
    /// only source left.
    #[test]
    fn bare_enter_still_falls_back_to_the_poll() {
        let mut k = key_of(&press(KeyCode::Enter, KeyModifiers::NONE));
        augment_enter_shift_with(&mut k, || true);
        assert_eq!(k.modifiers, KeyModifiers::SHIFT);

        let mut k = key_of(&press(KeyCode::Enter, KeyModifiers::NONE));
        augment_enter_shift_with(&mut k, || false);
        assert_eq!(k.modifiers, KeyModifiers::NONE);
    }

    /// Nothing but Enter is touched.
    #[test]
    fn other_keys_are_left_alone() {
        for code in [KeyCode::Esc, KeyCode::Tab, KeyCode::Char('a')] {
            let mut k = key_of(&press(code, KeyModifiers::NONE));
            augment_enter_shift_with(&mut k, || true);
            assert_eq!(k.modifiers, KeyModifiers::NONE, "code {:?}", code);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Claim B: ESC followed by CR reaches the pane as one atomic Alt+Enter
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn esc_then_enter_merges_into_one_alt_enter() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();

    // The Escape is held, not forwarded.
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    // Its own key release must not close the window.
    assert!(c
        .feed(release(KeyCode::Esc, KeyModifiers::NONE), t0)
        .is_none());

    let out = c
        .feed(
            press(KeyCode::Enter, KeyModifiers::NONE),
            t0 + Duration::from_millis(5),
        )
        .expect("the pair must produce one event");
    let k = key_of(&out);
    assert_eq!(k.code, KeyCode::Enter);
    assert!(
        k.modifiers.contains(KeyModifiers::ALT),
        "merged key must carry ALT so encode_key_event writes \\x1b\\r; got {:?}",
        k.modifiers
    );
    assert_eq!(k.kind, KeyEventKind::Press);
    assert!(c.pop().is_none(), "nothing may be left over");
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
}

/// `encode_key_event` is what actually puts bytes on the wire, so assert the
/// merged event really becomes the two bytes the reporter needs.
#[cfg(windows)]
#[test]
fn the_merged_key_encodes_to_esc_cr() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    let out = c
        .feed(
            press(KeyCode::Enter, KeyModifiers::NONE),
            t0 + Duration::from_millis(1),
        )
        .unwrap();
    let bytes = crate::input::encode_key_event(&key_of(&out)).expect("Enter must encode");
    assert_eq!(bytes, b"\x1b\r".to_vec(), "got {:02x?}", bytes);
}

/// A Shift+Enter that the terminal DID encode must still produce the same two
/// bytes, so both routes agree.
#[cfg(windows)]
#[test]
fn shift_enter_from_the_record_also_encodes_to_esc_cr() {
    let k = key_of(&press(KeyCode::Enter, KeyModifiers::SHIFT));
    assert_eq!(
        crate::input::encode_key_event(&k).unwrap(),
        b"\x1b\r".to_vec()
    );
}

#[test]
fn a_lone_escape_is_delivered_after_the_window() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());

    // Inside the window it is still held back.
    assert!(c.expire(t0 + Duration::from_millis(ESC_COALESCE_MS - 1)).is_none());
    assert_eq!(
        c.deadline_ms(t0 + Duration::from_millis(10)),
        Some(ESC_COALESCE_MS - 10)
    );

    let out = c
        .expire(t0 + Duration::from_millis(ESC_COALESCE_MS))
        .expect("the window has run out, the Escape must come through");
    assert_eq!(key_of(&out).code, KeyCode::Esc);
    assert_eq!(key_of(&out).modifiers, KeyModifiers::NONE);
    // Exactly one, never two.
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
    assert!(c.pop().is_none());
}

#[test]
fn escape_then_another_key_keeps_both_in_order() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());

    let first = c
        .feed(
            press(KeyCode::Char('k'), KeyModifiers::NONE),
            t0 + Duration::from_millis(2),
        )
        .expect("the held Escape must come out first");
    assert_eq!(key_of(&first).code, KeyCode::Esc);
    let second = c.pop().expect("then the key that ended the window");
    assert_eq!(key_of(&second).code, KeyCode::Char('k'));
    assert!(c.pop().is_none());
}

/// An Enter that arrives after the window has already been retired is a plain
/// Enter, exactly as tmux delivers it once `escape-time` has elapsed.
#[test]
fn enter_after_the_window_is_not_merged() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    let esc = c
        .expire(t0 + Duration::from_millis(ESC_COALESCE_MS))
        .unwrap();
    assert_eq!(key_of(&esc).code, KeyCode::Esc);

    let out = c
        .feed(
            press(KeyCode::Enter, KeyModifiers::NONE),
            t0 + Duration::from_millis(ESC_COALESCE_MS + 1),
        )
        .unwrap();
    assert_eq!(key_of(&out).modifiers, KeyModifiers::NONE);
}

/// Ctrl+Enter has its own byte (0x0a) and must not collect a spurious ALT from
/// a preceding Escape.
#[test]
fn ctrl_enter_after_escape_is_not_merged() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    let first = c
        .feed(
            press(KeyCode::Enter, KeyModifiers::CONTROL),
            t0 + Duration::from_millis(2),
        )
        .unwrap();
    assert_eq!(key_of(&first).code, KeyCode::Esc);
    let second = c.pop().unwrap();
    assert_eq!(key_of(&second).modifiers, KeyModifiers::CONTROL);
}

/// A modified Escape (Alt+Esc, Ctrl+Esc) is a key in its own right and is
/// never held.
#[test]
fn a_modified_escape_is_passed_straight_through() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    let out = c
        .feed(press(KeyCode::Esc, KeyModifiers::ALT), t0)
        .expect("must not be held");
    assert_eq!(key_of(&out).modifiers, KeyModifiers::ALT);
    assert!(c.deadline_ms(t0).is_none());
}

/// Everything that is not an Escape flows through untouched, so the coalescer
/// cannot add latency to ordinary typing.
#[test]
fn ordinary_keys_are_never_held() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    for code in [
        KeyCode::Char('a'),
        KeyCode::Enter,
        KeyCode::Tab,
        KeyCode::Backspace,
        KeyCode::Up,
    ] {
        let out = c.feed(press(code, KeyModifiers::NONE), t0);
        assert!(out.is_some(), "code {:?} must pass straight through", code);
        assert!(c.deadline_ms(t0).is_none());
    }
}

/// Disabled (the Unix build, where crossterm's own parser already folds
/// `\x1b\r`), nothing is held at all.
#[test]
fn disabled_coalescer_is_a_pass_through() {
    let mut c = EscCoalesce::new(false);
    let t0 = Instant::now();
    let out = c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).unwrap();
    assert_eq!(key_of(&out).code, KeyCode::Esc);
    assert!(c.deadline_ms(t0).is_none());
    assert!(c.expire(t0 + Duration::from_secs(1)).is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// The third defect this issue surfaced: `send-keys S-Enter` delivered NOTHING
//
// Measured by writing each candidate into a real pseudoconsole:
//   ESC [ 13;2 ~  -> zero input records
//   ESC [ 13;3 ~  -> zero input records
//   1b 0d         -> REC DOWN vk=0x0D uChar=0x000D ctrl=0x0002 [LALT]
// so the sequence `send-keys S-Enter` used to write was silently discarded by
// the ConPTY input parser before any pane child could see it, which is the
// same failure mode #610 found for Ctrl+Backspace.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(windows)]
mod send_keys_parity {
    use super::*;
    use crate::input::{encode_key_event, parse_modified_special_key};

    /// The binding path and the keystroke path have to agree byte for byte,
    /// otherwise `bind-key x send-keys S-Enter` does something different from
    /// pressing Shift+Enter.
    #[test]
    fn send_keys_names_match_the_typed_key_bytes() {
        for (name, mods) in [
            ("S-Enter", KeyModifiers::SHIFT),
            ("M-Enter", KeyModifiers::ALT),
            ("C-Enter", KeyModifiers::CONTROL),
            ("C-S-Enter", KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            ("C-M-Enter", KeyModifiers::CONTROL | KeyModifiers::ALT),
        ] {
            let by_name = parse_modified_special_key(name).expect(name);
            let typed = encode_key_event(&key_of(&press(KeyCode::Enter, mods))).expect(name);
            assert_eq!(
                by_name.as_bytes(),
                typed.as_slice(),
                "send-keys {} sends {:02x?} but the typed key sends {:02x?}",
                name,
                by_name.as_bytes(),
                typed
            );
        }
    }

    #[test]
    fn shift_and_alt_enter_are_esc_cr_not_a_discarded_csi() {
        assert_eq!(parse_modified_special_key("S-Enter").unwrap(), "\x1b\r");
        assert_eq!(parse_modified_special_key("M-Enter").unwrap(), "\x1b\r");
        assert_eq!(parse_modified_special_key("S-Return").unwrap(), "\x1b\r");
        assert_eq!(parse_modified_special_key("M-CR").unwrap(), "\x1b\r");
    }

    /// Guard the neighbours: only Enter moved.
    #[test]
    fn other_modified_special_keys_are_unchanged() {
        assert_eq!(parse_modified_special_key("C-Left").unwrap(), "\x1b[1;5D");
        assert_eq!(parse_modified_special_key("S-Right").unwrap(), "\x1b[1;2C");
        assert_eq!(parse_modified_special_key("S-Tab").unwrap(), "\x1b[9;2~");
        assert_eq!(parse_modified_special_key("C-Delete").unwrap(), "\x1b[3;5~");
        assert_eq!(parse_modified_special_key("Enter"), None);
    }
}

/// Non-key events must not be swallowed while an Escape is held.
#[test]
fn a_resize_while_holding_flushes_the_escape_first() {
    let mut c = EscCoalesce::new(true);
    let t0 = Instant::now();
    assert!(c.feed(press(KeyCode::Esc, KeyModifiers::NONE), t0).is_none());
    let first = c.feed(Event::Resize(80, 24), t0 + Duration::from_millis(1)).unwrap();
    assert_eq!(key_of(&first).code, KeyCode::Esc);
    assert!(matches!(c.pop(), Some(Event::Resize(80, 24))));
}
