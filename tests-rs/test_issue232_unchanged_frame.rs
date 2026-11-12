// #232 suite regression: anything that queried a session once a second (a
// `list-sessions`, a `show-options`, a `display-message -p` from a script)
// made the server push its attached clients a frame per request, most of them
// byte for byte copies of the screen the client already had. The status-interval
// suite read those as a second frame tens of milliseconds after each real one
// (p50 frame interval 28 ms, "status-interval=5: got 8 frames in 7s"). tmux
// redraws a client only when what it shows changed.

use super::*;

fn frame(tag: &str) -> String {
    format!("{{\"layout\":{{}},\"status_right\":\"{}\"}}", tag)
}

fn with_bell(tag: &str) -> String {
    let mut f = frame(tag);
    crate::frame_oneshot::append(
        &mut f,
        &crate::frame_oneshot::OneShots { bell: true, ..Default::default() },
    );
    f
}

#[test]
fn an_unchanged_frame_is_not_owed_again() {
    let k = frame_key(&frame("a"));
    assert!(k.is_some());
    assert!(frame_owed(None, k, false), "the first frame is always sent");
    assert!(!frame_owed(k, k, false), "the same content twice is sent once");
    assert!(frame_owed(k, frame_key(&frame("b")), false), "changed content is sent");
}

#[test]
fn an_unread_older_frame_is_still_replaced() {
    // The slot holds a stale frame the writer has not taken: the current
    // one must replace it even if the client was handed this content before.
    let k = frame_key(&frame("a"));
    assert!(frame_owed(k, k, true));
}

#[test]
fn a_frame_with_a_one_shot_is_always_sent() {
    assert_eq!(frame_key(&with_bell("a")), None);
    assert!(frame_owed(frame_key(&with_bell("a")), frame_key(&with_bell("a")), false));
}

/// Other tests push frames to every registered slot at the same time, so
/// these look only for the frames they pushed themselves.
fn holds(slot: &FrameSlot, want: &str) -> bool {
    slot.lock().unwrap().as_deref() == Some(want)
}

#[test]
fn the_server_slot_skips_a_repeat_of_the_frame_it_just_delivered() {
    let _guard = super::tests_pr267_backpressure::BP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (poke, _wake) = std::sync::mpsc::channel();
    let id = u64::MAX - 0x0232_0001;
    let slot = register_frame_channel(id, poke);
    let a = frame("issue232-slot-a");
    push_frame(&a);
    assert!(holds(&slot, &a));
    slot.lock().unwrap().take(); // the writer sends it
    push_frame(&a);
    let repeat = holds(&slot, &a);
    let b = frame("issue232-slot-b");
    push_frame(&b);
    let changed = holds(&slot, &b);
    slot.lock().unwrap().take();
    let bell = with_bell("issue232-slot-b");
    push_frame(&bell);
    let rang = holds(&slot, &bell);
    deregister_frame_channel(id);
    assert!(!repeat, "an identical frame was pushed a second time");
    assert!(changed, "a changed frame must still be pushed");
    assert!(rang, "a bell rides even on an unchanged screen");
}

#[test]
fn the_dump_state_requester_is_not_pushed_its_own_reply() {
    let _guard = super::tests_pr267_backpressure::BP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (poke, _wake) = std::sync::mpsc::channel();
    let id = u64::MAX - 0x0232_0002;
    let slot = register_frame_channel(id, poke);
    let a = frame("issue232-answered-a");
    push_frame_answered(&a, Some(id));
    let pushed_back = holds(&slot, &a);
    push_frame(&a);
    let repeated = holds(&slot, &a);
    deregister_frame_channel(id);
    assert!(!pushed_back, "the reply already carried this frame");
    assert!(!repeated, "and it is still the client's current screen");
}

/// A frame overwritten in the slot before the writer took it was never
/// delivered. Going back to that content must send it again, and the key
/// that counts is always the frame the writer will actually write.
#[test]
fn content_overwritten_before_delivery_is_sent_again() {
    let _guard = super::tests_pr267_backpressure::BP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (poke, _wake) = std::sync::mpsc::channel();
    let id = u64::MAX - 0x0232_0004;
    let slot = register_frame_channel(id, poke);
    let a = frame("issue232-overwrite-a");
    let b = frame("issue232-overwrite-b");
    push_frame(&a);
    slot.lock().unwrap().take(); // client has A
    push_frame(&b); // B waits, unread
    push_frame(&a); // back to A before the writer ran
    let after_revert = holds(&slot, &a);
    slot.lock().unwrap().take(); // writer sends A
    push_frame(&b); // B was never delivered: it must go now
    let b_sent = holds(&slot, &b);
    deregister_frame_channel(id);
    assert!(after_revert, "the unread B must be replaced by A, not left on the wire last");
    assert!(b_sent, "B was overwritten before delivery, so it is owed again");
}

#[test]
fn refresh_client_gets_the_same_frame_again() {
    let _guard = super::tests_pr267_backpressure::BP_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (poke, _wake) = std::sync::mpsc::channel();
    let id = u64::MAX - 0x0232_0003;
    let slot = register_frame_channel(id, poke);
    let a = frame("issue232-refresh-a");
    push_frame(&a);
    slot.lock().unwrap().take();
    invalidate_delivered_frames();
    push_frame(&a);
    let redrawn = holds(&slot, &a);
    deregister_frame_channel(id);
    assert!(redrawn, "refresh-client, attach and resize redraw even an unchanged screen");
}

#[test]
fn read_only_queries_owe_no_frame() {
    let (tx, _rx) = std::sync::mpsc::channel::<String>();
    assert!(CtrlReq::SessionInfo(tx.clone()).is_read_only_query());
    assert!(CtrlReq::ShowOptions(tx.clone()).is_read_only_query());
    assert!(CtrlReq::ListKeys(tx.clone()).is_read_only_query());
    let (btx, _brx) = std::sync::mpsc::channel::<bool>();
    assert!(CtrlReq::HasSession(btx).is_read_only_query());
    // What amesh polls with: list-panes -a -F, display-message -p, capture-pane.
    assert!(CtrlReq::ListAllPanesFormat(tx.clone(), "#{pane_id}".into()).is_read_only_query());
    assert!(CtrlReq::DisplayMessage(tx.clone(), "#{cursor_x}".into(), None, false, None, None, DisplayFlags::default())
        .is_read_only_query());
    let (vtx, _vrx) = std::sync::mpsc::channel();
    assert!(CtrlReq::ValidateTarget { target: TempTarget::default(), resp: vtx }.is_read_only_query());
    // display-message without -p puts its text on the status line.
    assert!(!CtrlReq::DisplayMessage(tx.clone(), "hi".into(), None, true, None, None, DisplayFlags::default())
        .is_read_only_query());
    // These change what a client draws.
    assert!(!CtrlReq::RenameWindow("x".into()).is_read_only_query());
    assert!(!CtrlReq::NextWindow.is_read_only_query());
}
