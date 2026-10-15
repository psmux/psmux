//! #744: a clipboard paste made while one of the client's own prompts was open
//! went into the pane underneath as well.
//!
//! Issue #290 gave the `Event::Paste` route a question to ask, `is an overlay
//! open`, and a function to answer it with, `route_paste_to_overlay`. The
//! Ctrl+V clipboard read-back route never asked: it always pushed
//! `send-paste`, which the server delivers to the pane. One Ctrl+V therefore
//! ran two deliveries, the key records into the prompt and the clipboard into
//! the pane.
//!
//! Two things had to change together. The read-back now goes through the same
//! function, and the overlays record the characters they take, so that the
//! read-back of the same Ctrl+V recognises the text as already delivered and
//! does not put it in the prompt twice.
//!
//! Registered from src/client.rs.

use super::{route_paste_to_overlay, PasteGesture};

/// Nothing is open, so a paste has nowhere to go but the pane. The second
/// field is the command prompt's cursor, counted in characters from the END of
/// its line, the way the shared prompt editor counts.
fn no_overlay() -> (String, usize, String) {
    (String::new(), 0, String::new())
}

#[test]
fn with_no_overlay_open_nothing_is_consumed() {
    let (mut cmd, cur, mut rename) = no_overlay();
    let consumed = route_paste_to_overlay(
        "ls -la",
        false, &mut cmd, cur,
        false, &mut rename,
    );
    assert!(!consumed, "the caller has to fall back to send-paste");
    assert_eq!(cmd, "");
}

#[test]
fn the_command_prompt_takes_it_at_the_cursor() {
    let (mut cmd, _, mut rename) = no_overlay();
    cmd.push_str("kill-session");
    let cur = 7usize; // "session" follows the cursor, so it sits after "kill-"
    let consumed = route_paste_to_overlay(
        "XX",
        true, &mut cmd, cur,
        false, &mut rename,
    );
    assert!(consumed, "an open prompt keeps the paste");
    assert_eq!(cmd, "kill-XXsession");
    // The cursor is still 7 characters from the end, which is right after the
    // text it inserted: the tail it counts from did not change.
}

#[test]
fn the_rename_prompt_takes_it_too() {
    // The window index prompt is the command prompt since #821, so the test
    // above covers it.
    let (mut cmd, cur, mut rename) = no_overlay();
    assert!(route_paste_to_overlay("win", false, &mut cmd, cur,
        true, &mut rename));
    assert_eq!(rename, "win");
}

#[test]
fn a_gesture_that_delivered_nothing_blocks_nothing() {
    // The read-back of a Ctrl+V whose characters never arrived has to go
    // through: this is the case where the prompt would otherwise get nothing
    // at all.
    let g = PasteGesture::default();
    assert!(!g.blocks("ls -la"));
}

#[test]
fn characters_taken_by_an_overlay_block_the_read_back() {
    // What #744 was: the characters went into the prompt, nothing was
    // recorded, and the read-back did not know they had been delivered.
    let mut g = PasteGesture::default();
    for c in "ls -la".chars() {
        g.record_char(c);
    }
    assert!(g.blocks("ls -la"), "the same text must not be delivered twice");
    assert!(
        g.blocks("something else entirely"),
        "whatever comes back right after this gesture's characters is that paste"
    );
}

#[test]
fn recording_a_character_matches_recording_it_as_text() {
    let mut by_char = PasteGesture::default();
    by_char.record_char('a');
    let mut by_text = PasteGesture::default();
    by_text.record("a");
    assert_eq!(by_char.blocks("a"), by_text.blocks("a"));
}

#[test]
fn a_multi_byte_character_is_recorded_whole() {
    // record_char encodes into a 4 byte buffer; a 3 byte character and a
    // 4 byte one both have to come back as themselves.
    let mut g = PasteGesture::default();
    g.record_char('\u{65e5}');
    assert!(g.blocks("\u{65e5}"));

    let mut g = PasteGesture::default();
    g.record_char('\u{1f600}');
    assert!(g.blocks("\u{1f600}"));
}

#[test]
fn a_finished_gesture_blocks_nothing_again() {
    let mut g = PasteGesture::default();
    g.record_char('a');
    assert!(g.blocks("a"));
    g.finish();
    assert!(!g.blocks("a"), "the next Ctrl+V starts clean");
}
