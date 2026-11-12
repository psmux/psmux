// prefix + ] and `paste-buffer` with an empty paste stack paste the OS clipboard
// (#428). A window holding the clipboard open for longer than the ~10 ms the
// plain read retries (a clipboard history listener, a VNC server, a graphics
// driver panel) made the paste vanish: 5 of 5 with a 100 ms holder. The read now
// waits off the server loop and the loop holds later requests behind it, so a
// key typed after the paste can neither overtake it nor be lost.

use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn app() -> AppState {
    AppState::new("paste_hold".to_string())
}

#[test]
fn a_paste_still_reading_keeps_waiting() {
    let mut app = app();
    let (_tx, rx) = mpsc::channel::<Option<String>>();
    start_pending_clipboard_paste(&mut app, rx, Instant::now() + Duration::from_secs(5), ClipboardPasteDelivery::Raw);
    assert!(!poll_pending_clipboard_paste(&mut app), "the read has not answered: requests stay held");
    assert!(app.clipboard_paste_pending.is_some());
}

#[test]
fn an_answered_read_finishes_the_paste() {
    let mut app = app();
    let (tx, rx) = mpsc::channel::<Option<String>>();
    start_pending_clipboard_paste(&mut app, rx, Instant::now() + Duration::from_secs(5), ClipboardPasteDelivery::Raw);
    tx.send(Some("CLIP".to_string())).unwrap();
    assert!(poll_pending_clipboard_paste(&mut app), "answered: the held requests may run");
    assert!(app.clipboard_paste_pending.is_none());
}

#[test]
fn an_overdue_read_gives_up_and_releases_the_held_requests() {
    let mut app = app();
    let (_tx, rx) = mpsc::channel::<Option<String>>();
    start_pending_clipboard_paste(&mut app, rx, Instant::now() - Duration::from_millis(1), ClipboardPasteDelivery::Raw);
    assert!(poll_pending_clipboard_paste(&mut app), "a clipboard held for good must not hold the keys for good");
    assert!(app.clipboard_paste_pending.is_none());
}

#[test]
fn a_reader_that_died_releases_the_held_requests() {
    let mut app = app();
    let (tx, rx) = mpsc::channel::<Option<String>>();
    drop(tx);
    start_pending_clipboard_paste(&mut app, rx, Instant::now() + Duration::from_secs(5), ClipboardPasteDelivery::Raw);
    assert!(poll_pending_clipboard_paste(&mut app));
    assert!(app.clipboard_paste_pending.is_none());
}

#[test]
fn nothing_pending_holds_nothing() {
    let mut app = app();
    assert!(poll_pending_clipboard_paste(&mut app));
}

#[test]
fn input_and_commands_wait_behind_the_paste_but_frames_do_not() {
    use crate::server::holds_behind_paste;
    use crate::types::CtrlReq;
    assert!(holds_behind_paste(&CtrlReq::SendText("x".to_string())));
    assert!(holds_behind_paste(&CtrlReq::PtyWake) == false);
}
