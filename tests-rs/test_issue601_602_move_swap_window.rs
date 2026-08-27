// Issues #601 and #602: move-window / swap-window window target resolution.
//
// Measured on 3.3.8 (7657bb3) against tmux 3.4 on the same layouts:
//
//   #602a  move-window -s S:2 -t S:9   with 0:main* 1:lazygit 2:claude
//          psmux -> 1:lazygit 2:claude 9:main*     (moved the ACTIVE window)
//          tmux  -> 0:main- 1:lazygit 9:claude*
//          `-s` had nowhere to go: CtrlReq::MoveWindow carried the
//          destination alone, and the handler always moved `active_idx`.
//
//   #602b  swap-window -t +1  with 0:main 2:lazygit 9:claude 10:d 11:c* 12:b 13:a
//          psmux -> swapped 11 and 2      tmux -> swapped 11 and 12
//          `+1` survived Rust's usize parser (which accepts a leading '+')
//          as the plain 1, and an index that named no window then fell back
//          to a raw Vec position. `-t -1` and `-t {last}` never reached the
//          server at all: the CLI read them as SESSION names.
//
// The fix is one resolver, `AppState::resolve_window_spec`, shared by the
// server, the CLI and the in-process command prompt. `index_ok` is tmux's
// CMD_FIND_WINDOW_INDEX: move-window's `-t` sets it (so `+N` is arithmetic on
// the display index and an unused number is a free destination), every other
// window target clears it (so `+N` steps through the ordered window list and
// an unused number is "can't find window: N").
//
// tmux 3.4 ground truth, `tmux -L parity602`, quoted per case below.

use super::*;

fn make_window(name: &str, id: usize) -> Window {
    Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// Build a session from `(display index, name)` pairs, already sorted.
fn app_from(spec: &[(usize, &str)]) -> AppState {
    let mut app = AppState::new("t601".to_string());
    for (i, (_, name)) in spec.iter().enumerate() {
        app.windows.push(make_window(name, i + 1));
    }
    app.window_indices = spec.iter().map(|(idx, _)| *idx).collect();
    app.window_base_index = 0;
    app
}

/// `list-windows -F '#{window_index}:#{window_name}<active><last>'`.
fn render(app: &AppState) -> String {
    app.windows.iter().enumerate().map(|(i, w)| {
        format!("{}:{}{}{}", app.window_indices[i], w.name,
            if i == app.active_idx { "*" } else { "" },
            if i == app.last_window_idx { "-" } else { "" })
    }).collect::<Vec<_>>().join(" ")
}

/// 0:main 2:lazygit 9:claude 10:d 11:c 12:b 13:a, the reporter's layout.
fn gapped() -> AppState {
    app_from(&[(0, "main"), (2, "lazygit"), (9, "claude"), (10, "d"),
               (11, "c"), (12, "b"), (13, "a")])
}

// ---------------------------------------------------------------------------
// #602b: relative targets without CMD_FIND_WINDOW_INDEX step the ORDERED LIST
// ---------------------------------------------------------------------------

#[test]
fn plus_one_is_the_next_window_in_the_list_not_index_plus_one() {
    // tmux: current 2 of `0:main 2:lazygit* 9:claude 10:d`; `+1` is 9, because
    // index 3 does not exist. The gap is the whole point:
    //   swap-window -t +1  ->  0:main 2:claude* 9:lazygit 10:d
    let mut app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude"), (10, "d")]);
    app.active_idx = 1; // display 2
    assert_eq!(app.resolve_window_spec("+1", false), Ok(WindowTarget::Pos(2)),
        "+1 must be the NEXT window in the list (display 9), not display 3");
    // The old code parsed "+1" as the unsigned 1 and, finding no display index
    // 1, fell back to Vec position 1 -- the window it started from's neighbour.
    assert_ne!(app.resolve_window_spec("+1", false), Ok(WindowTarget::Pos(1)));
}

#[test]
fn plus_and_minus_counts_step_that_many_places() {
    // tmux, current 11 of 0 2 9 10 11 12 13:
    //   swap-window -t +1 -> 11:b* 12:c   (11 and 12)
    //   swap-window -t +2 -> 11:a* 13:c   (11 and 13)
    //   swap-window -t -1 -> 10:c 11:d*   (11 and 10)
    let mut app = gapped();
    app.active_idx = 4; // display 11
    assert_eq!(app.resolve_window_spec("+1", false), Ok(WindowTarget::Pos(5)));
    assert_eq!(app.resolve_window_spec("+2", false), Ok(WindowTarget::Pos(6)));
    assert_eq!(app.resolve_window_spec("-1", false), Ok(WindowTarget::Pos(3)));
    assert_eq!(app.resolve_window_spec("-2", false), Ok(WindowTarget::Pos(2)));
    // Bare +/- mean a count of one.
    assert_eq!(app.resolve_window_spec("+", false), Ok(WindowTarget::Pos(5)));
    assert_eq!(app.resolve_window_spec("-", false), Ok(WindowTarget::Pos(3)));
}

#[test]
fn offsets_wrap_like_winlink_next_by_number() {
    // tmux's winlink_next_by_number falls back to RB_MIN at the end of the
    // list, so `+1` from the last window is the first one.
    let mut app = gapped();
    app.active_idx = 6; // display 13, the last
    assert_eq!(app.resolve_window_spec("+1", false), Ok(WindowTarget::Pos(0)));
    app.active_idx = 0;
    assert_eq!(app.resolve_window_spec("-1", false), Ok(WindowTarget::Pos(6)));
}

#[test]
fn plus_one_on_a_single_window_session_is_that_window() {
    // tmux: `swap-window -t +1` with one window exits 0 and does nothing.
    let app = app_from(&[(0, "main")]);
    assert_eq!(app.resolve_window_spec("+1", false), Ok(WindowTarget::Pos(0)));
}

#[test]
fn symbolic_targets_resolve() {
    // tmux, current 2 and last 9 of 0:main 2:lazygit* 9:claude- 10:d:
    //   swap-window -t '{last}'  ->  0:main 2:claude* 9:lazygit- 10:d
    let mut app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude"), (10, "d")]);
    app.active_idx = 1;
    app.last_window_idx = 2;
    assert_eq!(app.resolve_window_spec("{last}", false), Ok(WindowTarget::Pos(2)));
    assert_eq!(app.resolve_window_spec("!", false), Ok(WindowTarget::Pos(2)));
    assert_eq!(app.resolve_window_spec("^", false), Ok(WindowTarget::Pos(0)));
    assert_eq!(app.resolve_window_spec("{start}", false), Ok(WindowTarget::Pos(0)));
    assert_eq!(app.resolve_window_spec("$", false), Ok(WindowTarget::Pos(3)));
    assert_eq!(app.resolve_window_spec("{end}", false), Ok(WindowTarget::Pos(3)));
    assert_eq!(app.resolve_window_spec("{next}", false), Ok(WindowTarget::Pos(2)));
    assert_eq!(app.resolve_window_spec("{previous}", false), Ok(WindowTarget::Pos(0)));
}

#[test]
fn a_session_prefix_is_dropped_and_names_resolve() {
    let mut app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude")]);
    app.active_idx = 0;
    assert_eq!(app.resolve_window_spec("sess:9", false), Ok(WindowTarget::Pos(2)));
    assert_eq!(app.resolve_window_spec(":9", false), Ok(WindowTarget::Pos(2)));
    assert_eq!(app.resolve_window_spec("claude", false), Ok(WindowTarget::Pos(2)));
    // `sess:` with no window part is the session's current window.
    assert_eq!(app.resolve_window_spec("sess:", false), Ok(WindowTarget::Pos(0)));
}

#[test]
fn an_index_that_names_no_window_is_an_error_not_a_vec_position() {
    // THE #602 fallback. Display index 1 does not exist here; Vec position 1
    // (display 2, lazygit) does, and the old `win_pos(d).unwrap_or(d)` handed
    // that back, so `swap-window -t 1` silently swapped a window nobody named.
    let app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude")]);
    assert_eq!(app.resolve_window_spec("1", false),
        Err("can't find window: 1".to_string()));
    // tmux prints only the window part of a qualified target.
    assert_eq!(app.resolve_window_spec("sess:77", false),
        Err("can't find window: 77".to_string()));
    assert_eq!(app.resolve_window_spec("nosuch", false),
        Err("can't find window: nosuch".to_string()));
}

// ---------------------------------------------------------------------------
// #602b: move-window's -t DOES set CMD_FIND_WINDOW_INDEX
// ---------------------------------------------------------------------------

#[test]
fn move_window_target_is_index_arithmetic_and_accepts_free_slots() {
    // tmux: current 2 of `0:main 2:lazygit* 9:claude 10:d`
    //   move-window -t +1  ->  0:main 3:lazygit* 9:claude 10:d
    // i.e. 2 + 1 = 3, an index, NOT the next window in the list.
    let mut app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude"), (10, "d")]);
    app.active_idx = 1;
    assert_eq!(app.resolve_window_spec("+1", true), Ok(WindowTarget::FreeIndex(3)));
    assert_eq!(app.resolve_window_spec("-1", true), Ok(WindowTarget::FreeIndex(1)));
    // An unused number is a destination, not an error.
    assert_eq!(app.resolve_window_spec("77", true), Ok(WindowTarget::FreeIndex(77)));
    // A used one still resolves to the window that holds it.
    assert_eq!(app.resolve_window_spec("9", true), Ok(WindowTarget::Pos(2)));
    // `-N` below base-index is out of range, as in tmux's `n > s->curw->idx`.
    app.active_idx = 0;
    assert!(app.resolve_window_spec("-1", true).is_err());
}

// ---------------------------------------------------------------------------
// #602a: move-window honours -s
// ---------------------------------------------------------------------------

#[test]
fn move_window_moves_the_source_not_the_active_window() {
    // tmux, `0:main* 1:lazygit 2:claude-`:
    //   move-window -s p:2 -t p:9  ->  0:main- 1:lazygit 9:claude*
    // 3.3.8 gave `1:lazygit 2:claude 9:main*`: main, the ACTIVE window, moved.
    let mut app = app_from(&[(0, "main"), (1, "lazygit"), (2, "claude")]);
    app.active_idx = 0;
    app.last_window_idx = 2;
    app.move_window(Some("p:2"), Some("p:9"), false, false, false, false).unwrap();
    assert_eq!(render(&app), "0:main- 1:lazygit 9:claude*");
}

#[test]
fn move_window_without_d_selects_the_moved_window() {
    // The `!dflag` select flag tmux passes to server_link_window: the moved
    // window becomes current and the old current becomes the last window.
    let mut app = app_from(&[(0, "main"), (1, "lazygit"), (2, "claude")]);
    app.active_idx = 0;
    app.move_window(Some("2"), Some("9"), false, false, false, false).unwrap();
    assert_eq!(app.active_idx, 2, "claude is current after the move");
    assert_eq!(app.windows[app.active_idx].name, "claude");
    assert_eq!(app.windows[app.last_window_idx].name, "main");
}

#[test]
fn move_window_with_d_leaves_the_current_window_alone() {
    let mut app = app_from(&[(0, "main"), (1, "lazygit"), (2, "claude")]);
    app.active_idx = 0;
    app.move_window(Some("2"), Some("9"), true, false, false, false).unwrap();
    assert_eq!(app.windows[app.active_idx].name, "main");
    assert_eq!(app.window_indices, vec![0, 1, 9]);
}

#[test]
fn move_window_to_an_occupied_index_is_index_in_use() {
    // tmux: `move-window -s p:2 -t p:1` with 1 taken prints "index in use: 1"
    // and exits 1, leaving the list untouched. 3.3.8 exited 0 having done
    // nothing at all, so scripts could not tell success from silence.
    let mut app = app_from(&[(0, "main"), (1, "b"), (2, "c")]);
    let before = render(&app);
    assert_eq!(app.move_window(Some("2"), Some("1"), false, false, false, false),
        Err("index in use: 1".to_string()));
    assert_eq!(render(&app), before, "a refused move must not change anything");
}

#[test]
fn move_window_with_an_unresolvable_source_is_an_error() {
    // tmux: `move-window -s p:88 -t p:5` -> "can't find window: 88", rc 1.
    // 3.3.8 ignored -s entirely and moved the ACTIVE window to 5 at rc 0.
    let mut app = app_from(&[(0, "main"), (1, "b"), (2, "c")]);
    let before = render(&app);
    assert_eq!(app.move_window(Some("p:88"), Some("p:5"), false, false, false, false),
        Err("can't find window: 88".to_string()));
    assert_eq!(render(&app), before);
}

#[test]
fn move_window_r_renumbers_contiguously() {
    // tmux: `0:main 5:b* 9:c-` + `move-window -r` -> `0:main 1:b* 2:c-`.
    let mut app = app_from(&[(0, "main"), (5, "b"), (9, "c")]);
    app.active_idx = 1;
    app.last_window_idx = 2;
    app.move_window(None, None, false, true, false, false).unwrap();
    assert_eq!(render(&app), "0:main 1:b* 2:c-");
}

#[test]
fn move_window_a_and_b_shuffle_to_make_room() {
    // tmux winlink_shuffle_up: -a inserts AFTER the target, pushing it and
    // everything above one index higher.
    let mut app = app_from(&[(0, "main"), (1, "b"), (2, "c")]);
    app.active_idx = 2; // c
    app.move_window(Some("2"), Some("0"), true, false, true, false).unwrap();
    assert_eq!(app.window_indices, vec![0, 1, 2]);
    assert_eq!(app.windows.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
        vec!["main", "c", "b"], "-a puts c straight after main");
}

// ---------------------------------------------------------------------------
// #601b: swap-window and the active window's identity
// ---------------------------------------------------------------------------

#[test]
fn swap_leaves_the_current_window_number_alone() {
    // Reported as a bug, but it is exactly what tmux does: swap-window swaps
    // the two winlinks' window pointers and leaves their indices (and so the
    // session's current winlink) untouched. tmux 3.4, current 0 of
    // `0:main* 1:lazygit 2:claude-`:
    //   swap-window -t p:2  ->  0:claude* 1:lazygit 2:main-
    // The active MARKER stays on index 0 and now names claude.
    let mut app = app_from(&[(0, "main"), (1, "lazygit"), (2, "claude")]);
    app.active_idx = 0;
    app.last_window_idx = 2;
    let tpos = app.resolve_window_spec("2", false).unwrap().pos().unwrap();
    app.windows.swap(app.active_idx, tpos);
    assert_eq!(render(&app), "0:claude* 1:lazygit 2:main-");
}

#[test]
fn swap_with_gaps_keeps_the_indices_in_place() {
    // tmux, current 2 of `0:main 2:lazygit* 9:claude 10:d-`:
    //   swap-window -t p:9  ->  0:main 2:claude* 9:lazygit 10:d-
    let mut app = app_from(&[(0, "main"), (2, "lazygit"), (9, "claude"), (10, "d")]);
    app.active_idx = 1;
    app.last_window_idx = 3;
    let tpos = app.resolve_window_spec("9", false).unwrap().pos().unwrap();
    app.windows.swap(app.active_idx, tpos);
    assert_eq!(render(&app), "0:main 2:claude* 9:lazygit 10:d-");
}

// ---------------------------------------------------------------------------
// Index bookkeeping the move relies on
// ---------------------------------------------------------------------------

#[test]
fn the_last_window_flag_follows_its_window_through_a_resort() {
    // `last_window_idx` is a Vec POSITION. move-window re-sorts the Vec by
    // display index, so without re-resolving it by window id the `-` flag
    // lands on whichever window slid into the old slot.
    let mut app = app_from(&[(0, "main"), (1, "b"), (2, "c")]);
    app.active_idx = 0;
    app.last_window_idx = 1; // b
    // Move c below b: the Vec order becomes main, c, b.
    app.move_window(Some("2"), Some("0"), true, false, true, false).unwrap();
    assert_eq!(app.windows[app.last_window_idx].name, "b",
        "the last-window flag must still name b, not whatever took position 1");
}

#[test]
fn move_window_to_its_own_index_is_a_no_op_not_an_error() {
    let mut app = app_from(&[(0, "main"), (1, "b"), (2, "c")]);
    let before = render(&app);
    assert_eq!(app.move_window(Some("1"), Some("1"), true, false, false, false), Ok(()));
    assert_eq!(render(&app), before);
}
