// Issue #1031: with `mouse-drag-enter-copy-mode on`, a left drag from the live
// view enters copy mode and, about 300 ms in, turns into a double click: the
// selection becomes the word under the press, is copied, and copy mode exits
// while the button is still down.
//
// The client sends the press when the button goes down, then on the first
// drag sends `copy-enter` and the press again so the selection anchors at the
// press cell. The server ran every left press through the click timer, so the
// replay was the second click of a double click.
//
// tmux (server-client.c, tag 3.8): only a real button down sets
// CLIENT_DOUBLECLICK / CLIENT_TRIPLECLICK, and the MouseDrag1Pane binding's
// `copy-mode -M` starts the drag from the press it already saw, with no new
// down event. So a drag is one click, and only a second real press inside
// KEYC_CLICK_TIMEOUT makes it a double click.
//
// The client now marks the replay (`pane-mouse ... R`) and
// `handle_client_pane_mouse` keeps it out of the click timer. These tests run
// the server side of the gesture through that function and then let the click
// window run out, which the existing drag test never did.

use super::make_scrollback_app;
use crate::types::Mode;

const PANE: usize = 41;

/// Let the click window run out now and run the server loop's timer.
fn expire_click_window(app: &mut crate::types::AppState) {
    if let Some((p, at, n, _)) = app.click_state {
        app.click_state = Some((p, at, n, std::time::Instant::now() - std::time::Duration::from_millis(1)));
    }
    crate::window_ops::click_timer_tick(app);
}

/// The #1031 gesture up to the point where the button is still held: real
/// press at the live view, copy-enter, the replayed press, two drags.
fn drag_into_copy_mode(app: &mut crate::types::AppState, replay: bool) {
    crate::window_ops::handle_client_pane_mouse(app, PANE, 0, 5, 2, true, false);
    crate::copy_mode::enter_copy_mode(app);
    crate::window_ops::handle_client_pane_mouse(app, PANE, 0, 5, 2, true, replay);
    crate::window_ops::handle_client_pane_mouse(app, PANE, 32, 9, 3, true, false);
    crate::window_ops::handle_client_pane_mouse(app, PANE, 32, 12, 5, true, false);
}

#[test]
fn a_replayed_press_does_not_make_the_drag_a_double_click() {
    let mut app = make_scrollback_app(true);
    drag_into_copy_mode(&mut app, true);
    assert!(matches!(app.mode, Mode::CopyMode));
    assert_eq!(app.copy_anchor, Some((2, 5)), "the drag anchors at the press cell");

    expire_click_window(&mut app);

    assert!(app.click_finish_at.is_none(), "no DoubleClick1Pane may run during the drag");
    assert!(matches!(app.mode, Mode::CopyMode), "copy mode must survive the click window");
    assert_eq!(app.copy_anchor, Some((2, 5)), "the selection must still start at the press cell");
    assert!(app.paste_buffers.is_empty(), "nothing is copied while the button is down");

    crate::window_ops::handle_client_pane_mouse(&mut app, PANE, 0, 12, 5, false, false);
    assert_eq!(app.paste_buffers.len(), 1, "the release yanks the dragged selection");
    let text = app.paste_buffers[0].data.clone();
    assert!(text.contains('\n'), "the yank spans the dragged rows, not one word: {text:?}");
    assert!(matches!(app.mode, Mode::Passthrough), "a mouse yank leaves copy mode");
}

#[test]
fn a_replay_counted_as_a_click_is_the_1031_double_click() {
    // The mechanism before the fix: the same gesture with the replay counted
    // as a press runs DoubleClick1Pane when the click window runs out.
    let mut app = make_scrollback_app(true);
    drag_into_copy_mode(&mut app, false);
    expire_click_window(&mut app);
    assert!(app.click_finish_at.is_some(), "a counted replay is a second click");
}

#[test]
fn a_real_double_click_still_selects_the_word() {
    let mut app = make_scrollback_app(true);
    for _ in 0..2 {
        crate::window_ops::handle_client_pane_mouse(&mut app, PANE, 0, 5, 2, true, false);
        crate::window_ops::handle_client_pane_mouse(&mut app, PANE, 0, 5, 2, false, false);
    }
    expire_click_window(&mut app);
    assert!(app.click_finish_at.is_some(), "two real presses are a double click");
    assert!(matches!(app.mode, Mode::CopyMode), "the double click selects in copy mode");
}

#[test]
fn a_real_triple_click_still_selects_the_line() {
    let mut app = make_scrollback_app(true);
    for _ in 0..3 {
        crate::window_ops::handle_client_pane_mouse(&mut app, PANE, 0, 5, 2, true, false);
        crate::window_ops::handle_client_pane_mouse(&mut app, PANE, 0, 5, 2, false, false);
    }
    assert!(app.click_state.is_none(), "the third press ends the click sequence");
    assert!(app.click_finish_at.is_some(), "TripleClick1Pane ran at the third press");
    assert!(matches!(app.mode, Mode::CopyMode));
}
