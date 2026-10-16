//! Issue #658: a frame wake must not jump over the held Escape's deadline.
//!
//! `InputSource::read_timeout` waits on console input and a pushed frame
//! together. Its loop ends with an `esc.expire()`, and that expire is the ONLY
//! thing that releases a lone Escape once its 50 ms coalescing window is up -
//! the console arm cannot do it, because a lone Escape is precisely the case
//! where no further console event arrives. The frame arm left the loop through
//! an early `return`, so it jumped straight over the expire. While frames kept
//! arriving, which is what a busy pane does, nothing released the key.
//!
//! This pins the decision the frame arm now makes. The E2E side of it is
//! tests/test_issue658_escape_under_output.ps1.

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::ssh_input::{frame_wake_release, EscCoalesce, ESC_COALESCE_MS};

/// The coalescer with its window pinned at the 
/// [`ESC_COALESCE_MS`] ceiling, so these tests keep exercising the hold
/// whatever the live escape-time default is (10 ms since the #841 follow up).
fn pinned() -> EscCoalesce {
    EscCoalesce::with_window(true, Duration::from_millis(ESC_COALESCE_MS))
}

fn esc_key() -> Event {
    Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()))
}

fn is_escape(ev: &Event) -> bool {
    matches!(
        ev,
        Event::Key(KeyEvent { code: KeyCode::Esc, kind: KeyEventKind::Press | KeyEventKind::Repeat, .. })
    )
}

/// A coalescer holding one bare Escape that arrived at `t0`.
fn holding_an_escape(t0: Instant) -> EscCoalesce {
    let mut esc = pinned();
    // The DOWN arrives with its UP already queued: an Escape the console
    // synthesised, the only kind still held (issue #841 follow up).
    let mut up = Some(Event::Key(KeyEvent::new_with_kind(KeyCode::Esc, KeyModifiers::empty(), KeyEventKind::Release)));
    let out = esc.feed_with(esc_key(), t0, move || up.take());
    assert!(
        out.is_none(),
        "a bare Escape is held for its coalescing window, not handed straight on: \
         that hold is what turns a split ESC+CR into one Alt+Enter (#611)"
    );
    esc
}

#[test]
fn a_frame_wake_releases_an_escape_whose_window_is_up() {
    let t0 = Instant::now();
    let mut esc = holding_an_escape(t0);
    let after = t0 + Duration::from_millis(ESC_COALESCE_MS + 1);
    let released = frame_wake_release(&mut esc, after);
    assert!(
        released.as_ref().is_some_and(is_escape),
        "the Escape's 50 ms window is up, so the frame wake owes it to the caller \
         before it returns. Dropping it here is how a bare Escape was swallowed \
         for as long as a busy pane kept pushing frames"
    );
}

#[test]
fn a_frame_wake_does_not_cut_the_window_short() {
    let t0 = Instant::now();
    let mut esc = holding_an_escape(t0);
    let early = t0 + Duration::from_millis(ESC_COALESCE_MS / 2);
    assert!(
        frame_wake_release(&mut esc, early).is_none(),
        "half way through the window the Escape is still waiting to see whether a \
         CR follows it. Releasing early would undo #611"
    );
    // ...and it is still there to be released when the window really is up.
    let after = t0 + Duration::from_millis(ESC_COALESCE_MS + 1);
    assert!(frame_wake_release(&mut esc, after).as_ref().is_some_and(is_escape));
}

#[test]
fn a_frame_wake_with_nothing_held_yields_nothing() {
    let mut esc = pinned();
    assert!(
        frame_wake_release(&mut esc, Instant::now()).is_none(),
        "no key is being held, so a frame wake returns to the caller empty handed \
         exactly as it did before"
    );
}

#[test]
fn an_escape_is_only_released_once() {
    let t0 = Instant::now();
    let mut esc = holding_an_escape(t0);
    let after = t0 + Duration::from_millis(ESC_COALESCE_MS + 1);
    assert!(frame_wake_release(&mut esc, after).is_some());
    assert!(
        frame_wake_release(&mut esc, after + Duration::from_millis(10)).is_none(),
        "the next frame wake must not deliver a second Escape the user never pressed"
    );
}
