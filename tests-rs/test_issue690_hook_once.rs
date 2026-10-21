// Issue #690: one `select-window` fired `after-select-window` TWICE.
//
// The reproduction is four commands and a buffer count:
//
//     psmux -L h new-session -d -s s -x 80 -y 24
//     psmux -L h new-window -t s
//     psmux -L h set-hook -g after-select-window "set-buffer HOOKFIRED"
//     psmux -L h select-window -t s:0
//     psmux -L h list-buffers
//         buffer0: 9 bytes: "HOOKFIRED"
//         buffer1: 9 bytes: "HOOKFIRED"       <- one command, two firings
//
// It counted 2 on all four routes (the CLI, the raw control socket, a
// bind-key binding injected into an attached client, and the prefix `:`
// prompt), three runs each, because the doubling was on the server side of
// all of them: `select-window -t <index>` sent TWO control requests for the
// same window, a permanent `FocusWindow` from the generic `-t` focus block
// and a `SelectWindow` from the command's own arm.  Every request carries its
// own hook slot in the server loop, so each one fired the hook.  The trace
// PSMUX_HOOK_DEBUG writes named both carriers:
//
//     === FIRE after-select-window req=FocusWindow pid=3600 ===
//     === FIRE after-select-window req=SelectWindow pid=3600 ===
//
// tmux fires a command's after hook once, from the command queue:
// cmd-queue.c `cmdq_fire_command` calls
// `cmdq_insert_hook(s, item, &fs, "after-%s", name)` once, after the
// command's exec returns, and `notify_winlink("window-selected")` in notify.c
// drives the notification hooks once as well.
//
// These tests pin the rule the fix put in one function: every `select-window`
// form emits EXACTLY ONE window selecting request, and it is the right one.
// The end to end counts, on all four routes, live in
// tests\test_issue690_hook_once.ps1.

use super::*;

/// One request, whatever the form: this is the property #690 broke.
///
/// The `None` in the second position is the RAW `-t`, which issue #693 item 4
/// added so `select-window` can resolve `+1`, `!` and `{end}` through the same
/// resolver move-window uses (tmux cmd-find.c:51-58 and :390-417). Passing
/// None here keeps every case below on the pre-resolved window/id/name inputs
/// #690 is about; the raw spec forms have their own file.
fn count(args: &[&str], win: Option<usize>, is_id: bool, name: Option<&str>) -> usize {
    select_window_requests(args, None, win, is_id, name).0.len()
}

#[test]
fn issue690_index_target_emits_one_request() {
    // `select-window -t s:0`, the reporter's command.  The outer -t is gone by
    // the time the arm sees the args, so this is what it gets.
    let reqs = select_window_requests(&[], None, Some(0), false, None).0;
    assert_eq!(reqs.len(), 1, "one select-window must emit one window request");
    assert!(
        matches!(reqs[0], CtrlReq::SelectWindow(0)),
        "a plain index target belongs to SelectWindow, which also fires before-select-window"
    );
}

#[test]
fn issue690_index_target_does_not_also_focus() {
    // The second firing came from a FocusWindow for the same window.  No form
    // may produce one alongside the select.
    for (args, win, is_id, name) in [
        (&[][..], Some(0), false, None),
        (&[][..], Some(3), false, None),
        (&["-T"][..], Some(1), false, None),
    ] {
        let reqs = select_window_requests(args, None, win, is_id, name).0;
        assert_eq!(reqs.len(), 1, "args {:?} emitted {} requests", args, reqs.len());
        assert!(
            !reqs.iter().any(|r| matches!(r, CtrlReq::FocusWindow(_))),
            "select-window must not send FocusWindow as well as SelectWindow (#690)"
        );
    }
}

#[test]
fn issue690_window_id_target_stays_an_id() {
    // #497: an @id must never be re-sent as an index.
    let reqs = select_window_requests(&[], None, Some(2), true, None).0;
    assert_eq!(reqs.len(), 1);
    assert!(matches!(reqs[0], CtrlReq::FocusWindowById(2)));
}

#[test]
fn issue690_window_name_target_focuses_by_name() {
    let reqs = select_window_requests(&[], None, None, false, Some("editor")).0;
    assert_eq!(reqs.len(), 1);
    match &reqs[0] {
        CtrlReq::FocusWindowByName(name) => assert_eq!(name, "editor"),
        _ => panic!("a window name target must focus by name, not by index"),
    }
}

#[test]
fn issue690_positional_index_wins_over_target() {
    // psmux accepts tmux's window number positionally.  It decides alone.
    let reqs = select_window_requests(&["2"], None, Some(1), false, None).0;
    assert_eq!(reqs.len(), 1);
    assert!(matches!(reqs[0], CtrlReq::SelectWindow(2)));
}

#[test]
fn issue690_relative_flags_are_the_whole_operation() {
    // cmd-select-window.c tests -n, then -p, then -l, and only falls through
    // to the -t target when none is given.  psmux used to select the target
    // AND move, which is two selections and two firings for one command.
    let next = select_window_requests(&["-n"], None, Some(0), false, None).0;
    assert_eq!(next.len(), 1);
    assert!(matches!(next[0], CtrlReq::NextWindow));

    let prev = select_window_requests(&["-p"], None, Some(0), false, None).0;
    assert_eq!(prev.len(), 1);
    assert!(matches!(prev[0], CtrlReq::PrevWindow));

    let last = select_window_requests(&["-l"], None, Some(0), false, None).0;
    assert_eq!(last.len(), 1);
    assert!(matches!(last[0], CtrlReq::LastWindow { .. }));

    // ... and with a session-only -t, which is how the suites spell it.
    assert_eq!(count(&["-p"], None, false, Some("sess")), 1);
    assert_eq!(count(&["-n"], None, false, Some("sess")), 1);
    assert_eq!(count(&["-l"], None, false, Some("sess")), 1);
}

#[test]
fn issue690_relative_beats_an_id_target_too() {
    // An @id target with -n must not focus the id and then move.
    let reqs = select_window_requests(&["-n"], None, Some(4), true, None).0;
    assert_eq!(reqs.len(), 1);
    assert!(matches!(reqs[0], CtrlReq::NextWindow));
}

#[test]
fn issue690_no_target_emits_nothing() {
    assert_eq!(count(&[], None, false, None), 0);
    // `-T` with no target names the current window, which it toggles away
    // from (cmd-select-window.c:131-138): one request, still at most one.
    assert_eq!(count(&["-T"], None, true, None), 1);
}

#[test]
fn issue690_every_form_emits_at_most_one() {
    // The invariant, over the whole shape space the arm can see.
    for args in [
        &[][..], &["-n"][..], &["-p"][..], &["-l"][..], &["0"][..], &["7"][..],
        &["-T"][..], &["-n", "3"][..],
    ] {
        for win in [None, Some(0usize), Some(5usize)] {
            for is_id in [false, true] {
                for name in [None, Some("win")] {
                    let n = select_window_requests(args, None, win, is_id, name).0.len();
                    assert!(
                        n <= 1,
                        "args {:?} win {:?} is_id {} name {:?} emitted {} window requests, #690 is exactly that",
                        args, win, is_id, name, n
                    );
                }
            }
        }
    }
}
