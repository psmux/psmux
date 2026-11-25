// clear-history frees the pane's history and nothing else, as tmux's
// cmd-capture-pane.c does (tag 3.8, lines 424 to 430): leave every mode, then
// grid_clear_history(wp->base.grid), and with -H screen_reset_hyperlinks.
// psmux gave the pane a new parser instead, which also blanked the screen,
// left the alternate screen and dropped the cursor, the scroll region and the
// modes the program had asked for.

use crate::copy_mode::test_copy_mode_snapshot::{app_with_pane, feed, filled, view_term};

fn process(app: &crate::types::AppState, bytes: &str) {
    if let Ok(mut parser) = view_term(app).lock() {
        parser.process(bytes.as_bytes());
    }
}

fn with_screen<T>(app: &crate::types::AppState, f: impl FnOnce(&vt100::Screen) -> T) -> T {
    let term = view_term(app);
    let parser = term.lock().expect("pane parser");
    f(parser.screen())
}

/// What a full screen program leaves behind: the alternate screen, mouse
/// tracking with SGR encoding, bracketed paste, a scroll region, some text
/// and the cursor somewhere in the middle.
const FULL_SCREEN: &str =
    "\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[HALTSCREEN text\x1b[3;8r\x1b[5;10H";

#[test]
fn the_alternate_screen_and_its_state_stay() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 60);
    process(&app, FULL_SCREEN);
    let before = with_screen(&app, |s| {
        (s.cursor_position(), s.scroll_region(), s.mouse_protocol_mode(), s.mouse_protocol_encoding(), s.contents())
    });
    assert!(filled(&view_term(&app)) > 30, "the normal screen needs a history to clear");

    crate::window_ops::clear_active_pane_history(&mut app, false);

    assert_eq!(filled(&view_term(&app)), 0, "the history is freed");
    with_screen(&app, |s| {
        assert!(s.alternate_screen(), "the pane stays on the alternate screen");
        assert!(s.bracketed_paste(), "bracketed paste stays on");
        assert_eq!(
            (s.cursor_position(), s.scroll_region(), s.mouse_protocol_mode(), s.mouse_protocol_encoding(), s.contents()),
            before,
            "the cursor, the scroll region, the mouse modes and the text stay"
        );
    });
    // The program goes on drawing where it was.
    process(&app, "X");
    with_screen(&app, |s| assert_eq!(s.cell(4, 9).map(|c| c.contents().to_string()).as_deref(), Some("X")));
}

#[test]
fn the_visible_rows_of_the_normal_screen_stay() {
    let mut app = app_with_pane();
    feed(&view_term(&app), "row", 0, 60);
    let before = with_screen(&app, |s| (s.cursor_position(), s.contents()));
    assert!(before.1.contains("row 59"));

    crate::window_ops::clear_active_pane_history(&mut app, false);

    assert_eq!(filled(&view_term(&app)), 0);
    with_screen(&app, |s| assert_eq!((s.cursor_position(), s.contents()), before));
}

#[test]
fn alternate_screen_off_stays_off() {
    let mut app = app_with_pane();
    if let Ok(mut parser) = view_term(&app).lock() {
        parser.screen_mut().set_allow_alternate_screen(false);
    }
    crate::window_ops::clear_active_pane_history(&mut app, false);
    with_screen(&app, |s| assert!(!s.allow_alternate_screen(), "the pane's alternate-screen off is kept"));
    process(&app, "\x1b[?1049h");
    with_screen(&app, |s| assert!(!s.alternate_screen()));
}

const LINKED: &str = "\x1b]8;;https://example.com/a\x1b\\link\x1b]8;;\x1b\\ plain";

#[test]
fn hyperlinks_stay_without_h() {
    let mut app = app_with_pane();
    process(&app, LINKED);
    crate::window_ops::clear_active_pane_history(&mut app, false);
    with_screen(&app, |s| {
        let id = s.cell(0, 0).expect("cell").hyperlink_id();
        assert_eq!(s.hyperlink_uri(id), Some("https://example.com/a"));
    });
}

#[test]
fn h_removes_every_hyperlink_for_good() {
    let mut app = app_with_pane();
    process(&app, LINKED);
    let old = with_screen(&app, |s| s.cell(0, 0).expect("cell").hyperlink_id());
    assert_ne!(old, 0);

    crate::window_ops::clear_active_pane_history(&mut app, true);

    with_screen(&app, |s| {
        assert_eq!(s.hyperlink_uri(old), None, "-H removes the link the visible cell had");
        assert_eq!(s.cell(0, 0).expect("cell").contents(), "l", "the text itself stays");
    });
    // The same URI set again is a new link; the old cells do not get it back
    // (tmux's hyperlink ids keep counting after hyperlinks_reset).
    process(&app, "\r\n\x1b]8;;https://example.com/a\x1b\\again\x1b]8;;\x1b\\");
    with_screen(&app, |s| {
        let new = s.cell(1, 0).expect("cell").hyperlink_id();
        assert_ne!(new, old);
        assert_eq!(s.hyperlink_uri(new), Some("https://example.com/a"));
        assert_eq!(s.hyperlink_uri(old), None);
    });
}

#[test]
fn the_command_reads_h_from_its_flags() {
    // The in process route (no server port): `clear-history -H` and the
    // cluster `-Ht <pane>` remove the links, a target named H does not.
    for (cmd, gone) in [
        ("clear-history -H", true),
        ("clearhist -Ht %7", true),
        ("clear-history -t H", false),
        ("clear-history -tH", false),
        ("clear-history", false),
    ] {
        let mut app = app_with_pane();
        process(&app, LINKED);
        let id = with_screen(&app, |s| s.cell(0, 0).expect("cell").hyperlink_id());
        crate::commands::execute_command_string(&mut app, cmd).expect(cmd);
        let uri = with_screen(&app, |s| s.hyperlink_uri(id).map(str::to_string));
        assert_eq!(uri.is_none(), gone, "{cmd}");
    }
}
