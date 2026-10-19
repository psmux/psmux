// Issue #858: a Ctrl+V press that reached the client decides what its release
// means.  Nothing paste shaped between them: the terminal did not paste, so no
// clipboard read, and an unbound press is owed to the pane as C-v.
use super::*;

#[test]
fn release_with_no_press_keeps_the_clipboard_read() {
    // Windows Terminal's own Ctrl+V binding swallows the press; only the
    // release reaches the client, and that route still reads the clipboard.
    let mut p = CtrlVPress::default();
    assert!(!p.pending());
    assert_eq!(p.release(), None);
}

#[test]
fn bound_press_then_release_is_no_paste_and_owes_nothing() {
    // `bind -n C-v split-window -h`: the binding ran on the press.
    let mut p = CtrlVPress::default();
    assert!(!p.press());
    assert!(p.pending());
    assert_eq!(p.release(), Some(CtrlVRelease::NoPaste { send_cv: false }));
    assert!(!p.pending(), "the release ends the press");
}

#[test]
fn unbound_press_then_release_owes_the_pane_c_v() {
    let mut p = CtrlVPress::default();
    p.press();
    p.defer();
    assert_eq!(p.release(), Some(CtrlVRelease::NoPaste { send_cv: true }));
}

#[test]
fn characters_after_the_press_are_a_paste_and_cancel_the_c_v() {
    // A host that delivers the press and then the clipboard as key records.
    let mut p = CtrlVPress::default();
    p.press();
    p.defer();
    p.note_paste_input();
    assert!(!p.take_before_key(), "the held C-v was the paste gesture");
    assert!(!p.take_due(Duration::from_millis(0)));
    assert_eq!(p.release(), Some(CtrlVRelease::Pasted));
}

#[test]
fn paste_input_with_no_press_pending_changes_nothing() {
    let mut p = CtrlVPress::default();
    p.note_paste_input();
    assert_eq!(p.release(), None);
}

#[test]
fn held_c_v_goes_out_when_the_window_runs_out() {
    let mut p = CtrlVPress::default();
    p.press();
    p.defer();
    assert!(!p.take_due(Duration::from_secs(60)), "not before the window");
    assert!(p.take_due(Duration::from_millis(0)));
    assert!(!p.take_due(Duration::from_millis(0)), "only once");
    // The release afterwards owes nothing more and still reads no clipboard.
    assert_eq!(p.release(), Some(CtrlVRelease::NoPaste { send_cv: false }));
}

#[test]
fn held_c_v_goes_out_before_an_unrelated_key() {
    let mut p = CtrlVPress::default();
    p.press();
    p.defer();
    assert!(p.take_before_key());
    assert!(!p.take_before_key());
    assert_eq!(p.release(), Some(CtrlVRelease::NoPaste { send_cv: false }));
}

#[test]
fn key_repeat_sends_the_earlier_c_v_and_holds_the_new_one() {
    let mut p = CtrlVPress::default();
    p.press();
    p.defer();
    assert!(p.press(), "the first press is owed when the repeat arrives");
    p.defer();
    assert_eq!(p.release(), Some(CtrlVRelease::NoPaste { send_cv: true }));
}

#[test]
fn defer_without_a_press_does_nothing() {
    let mut p = CtrlVPress::default();
    p.defer();
    assert!(!p.take_before_key());
    assert!(!p.take_due(Duration::from_millis(0)));
}
