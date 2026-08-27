// Issue #607: copy mode leaked onto brand new panes and windows, and onto the
// pane you land on after killing a copy-mode pane.
//
// In tmux a mode belongs to one pane: `window_pane_create` starts every pane
// with an empty mode stack (window.c:1317 `TAILQ_INIT(&wp->modes)`) and
// `window_lost_pane` (window.c:1060) only moves `w->active`, never the
// survivor's modes. Measured on tmux 3.4 with `#{pane_in_mode}` per pane:
// splitting while %0 is in copy mode gives `%0=1 %1=0`, and killing a
// copy-mode %1 gives `%0=0` when %0 was clean and `%0=1` when it was not.
//
// psmux keeps the LIVE copy cursor in `AppState` (so `AppState::mode` is
// really "the mode of whichever pane is active") and parks every other pane's
// mode in `Pane::copy_state`. Focus commands keep the two in step through
// `switch_with_copy_save`. Pane creation, window creation and pane death also
// change which pane is active, and they used to do it without telling the copy
// layer, so `Mode::CopyMode` simply stayed put and became the new pane's mode.
//
// These tests drive the ownership handover the fix installs
// (`park_mode_on_active_pane` / `retarget_mode_to_active_pane`) over a real
// two-pane tree, and pin the per-pane reporting that goes with it.

use super::*;

use crate::copy_mode::{park_mode_on_active_pane, retarget_mode_to_active_pane, active_pane_id};
use crate::types::Mode;

fn rig(name: &str) -> Option<AppState> {
    let mut app = AppState::new(name.to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };
    if app.windows.is_empty() { return None; }
    Some(app)
}

fn in_copy(app: &AppState) -> bool {
    matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. })
}

/// A pane's OWN mode, the way `#{pane_in_mode}` now reports it: the focused
/// pane's is live in `AppState`, every other pane's is parked on the pane.
fn pane_in_copy(app: &AppState, id: usize) -> bool {
    if active_pane_id(app) == Some(id) {
        return in_copy(app);
    }
    app.windows.iter().any(|w| {
        crate::tree::find_path_by_id(&w.root, id)
            .and_then(|p| crate::tree::active_pane(&w.root, &p))
            .map_or(false, |p| p.copy_state.is_some())
    })
}

/// A second pane with no child process, the same thing `new-window -E` builds.
/// Returns None when a pseudo console cannot be opened at all, so the test is
/// skipped rather than failed on a machine that cannot make one.
fn empty_pane(app: &mut AppState) -> Option<(usize, Node)> {
    let id = app.next_pane_id;
    let pane = crate::popup::create_empty_pane(24, 80, id)?;
    app.next_pane_id += 1;
    Some((id, Node::Leaf(pane)))
}

/// Do to the tree exactly what `split_active_with_env` does, with the copy
/// mode handover the fix wraps around it.
fn split_active(app: &mut AppState) -> Option<(usize, usize)> {
    let old_id = active_pane_id(app)?;
    let (new_id, leaf) = empty_pane(app)?;
    let prev = active_pane_id(app);
    park_mode_on_active_pane(app);
    let win = &mut app.windows[app.active_idx];
    crate::tree::replace_leaf_with_split(&mut win.root, &win.active_path, LayoutKind::Horizontal, leaf);
    let mut new_path = win.active_path.clone();
    new_path.push(1);
    win.active_path = new_path;
    crate::tree::touch_mru(&mut win.pane_mru, new_id);
    retarget_mode_to_active_pane(app, prev);
    Some((old_id, new_id))
}

fn focus_pane(app: &mut AppState, id: usize) {
    crate::copy_mode::switch_with_copy_save(app, |app| {
        if let Some(path) = crate::tree::find_path_by_id(&app.windows[app.active_idx].root, id) {
            app.windows[app.active_idx].active_path = path;
        }
    });
}

/// The kill half of the fix: `kill_pane_at_path` picks the survivor, then the
/// survivor's own mode becomes the live one.
fn kill_pane(app: &mut AppState, id: usize) {
    let prev = active_pane_id(app);
    let win = &mut app.windows[app.active_idx];
    let Some(path) = crate::tree::find_path_by_id(&win.root, id) else { return };
    crate::tree::kill_leaf(&mut win.root, &path);
    crate::tree::remove_from_mru(&mut win.pane_mru, id);
    let survivor = win.pane_mru.iter()
        .find_map(|&pid| crate::tree::find_path_by_id(&win.root, pid))
        .unwrap_or_else(|| crate::tree::first_leaf_path(&win.root));
    win.active_path = survivor;
    retarget_mode_to_active_pane(app, prev);
}

#[test]
fn a_split_does_not_open_the_new_pane_in_copy_mode() {
    let Some(mut app) = rig("i607_split") else { return };
    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(in_copy(&app), "fixture must start in copy mode");

    let Some((old_id, new_id)) = split_active(&mut app) else { return };

    assert_eq!(active_pane_id(&app), Some(new_id), "the split focuses the new pane");
    assert!(!pane_in_copy(&app, new_id), "the brand new pane must not be in copy mode");
    assert!(!in_copy(&app), "the live mode must follow the newly focused pane");
    assert!(pane_in_copy(&app, old_id), "the pane copy mode was entered in must keep it");
}

#[test]
fn a_new_window_does_not_open_in_copy_mode() {
    let Some(mut app) = rig("i607_newwin") else { return };
    crate::copy_mode::enter_copy_mode(&mut app);
    let old_id = active_pane_id(&app).unwrap();

    let prev = active_pane_id(&app);
    park_mode_on_active_pane(&mut app);
    let Some((new_id, leaf)) = empty_pane(&mut app) else { return };
    app.windows.push(Window {
        root: leaf,
        active_path: vec![],
        name: "w1".to_string(),
        id: app.next_win_id,
        area: app.client_area,
        window_size: None,
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![new_id],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app.next_win_id += 1;
    app.active_idx = app.windows.len() - 1;
    retarget_mode_to_active_pane(&mut app, prev);

    assert!(!in_copy(&app), "the new window must not open in copy mode");
    assert!(!pane_in_copy(&app, new_id), "the new window's pane must not be in copy mode");
    assert!(pane_in_copy(&app, old_id), "the original pane must keep its copy mode");
}

#[test]
fn killing_a_copy_mode_pane_leaves_a_clean_neighbour_clean() {
    let Some(mut app) = rig("i607_killclean") else { return };
    let Some((old_id, new_id)) = split_active(&mut app) else { return };
    // Only the new pane goes into copy mode; the neighbour never does.
    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(pane_in_copy(&app, new_id));
    assert!(!pane_in_copy(&app, old_id));

    kill_pane(&mut app, new_id);

    assert_eq!(active_pane_id(&app), Some(old_id), "focus lands on the survivor");
    assert!(!in_copy(&app), "the survivor must not inherit the dead pane's copy mode");
    assert!(!pane_in_copy(&app, old_id));
}

#[test]
fn killing_a_copy_mode_pane_leaves_a_copy_mode_neighbour_in_copy_mode() {
    let Some(mut app) = rig("i607_killboth") else { return };
    let Some((old_id, new_id)) = split_active(&mut app) else { return };
    // Put BOTH panes in copy mode, which is only expressible once a mode
    // belongs to its pane.
    focus_pane(&mut app, old_id);
    crate::copy_mode::enter_copy_mode(&mut app);
    focus_pane(&mut app, new_id);
    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(pane_in_copy(&app, old_id), "both panes must be in copy mode for this case");
    assert!(pane_in_copy(&app, new_id));

    kill_pane(&mut app, new_id);

    assert_eq!(active_pane_id(&app), Some(old_id));
    assert!(in_copy(&app), "the survivor must keep ITS OWN copy mode");
    assert!(pane_in_copy(&app, old_id));
}

#[test]
fn a_focus_change_never_moves_copy_mode_between_panes() {
    let Some(mut app) = rig("i607_focus") else { return };
    let Some((old_id, new_id)) = split_active(&mut app) else { return };
    crate::copy_mode::enter_copy_mode(&mut app);

    focus_pane(&mut app, old_id);
    assert!(!pane_in_copy(&app, old_id), "focus must not carry copy mode with it");
    assert!(pane_in_copy(&app, new_id), "the pane that owns copy mode keeps it");

    focus_pane(&mut app, new_id);
    assert!(!pane_in_copy(&app, old_id));
    assert!(pane_in_copy(&app, new_id));
}

#[test]
fn retargeting_to_the_same_pane_does_not_clobber_the_live_copy_cursor() {
    // `kill-pane -t` on a pane in ANOTHER window leaves focus where it was.
    // The handover must notice that and leave the live cursor alone, rather
    // than restoring the older parked copy over it.
    let Some(mut app) = rig("i607_noclobber") else { return };
    crate::copy_mode::enter_copy_mode(&mut app);
    let id = active_pane_id(&app).unwrap();
    app.copy_pos = Some((7, 11));
    app.copy_anchor = Some((2, 3));

    retarget_mode_to_active_pane(&mut app, Some(id));

    assert!(in_copy(&app), "the mode must survive a same-pane retarget");
    assert_eq!(app.copy_pos, Some((7, 11)), "the live copy cursor must not be rewound");
    assert_eq!(app.copy_anchor, Some((2, 3)), "the live selection anchor must not be rewound");
}

#[test]
fn pane_in_mode_answers_for_the_target_pane_not_the_focused_one() {
    let Some(mut app) = rig("i607_fmt") else { return };
    crate::copy_mode::enter_copy_mode(&mut app);
    let Some((old_id, new_id)) = split_active(&mut app) else { return };

    // old_id is in copy mode and is NOT focused; new_id is focused and is not.
    let old_fmt = crate::format::expand_format_for_pane_by_id("#{pane_in_mode}", &app, old_id);
    let new_fmt = crate::format::expand_format_for_pane_by_id("#{pane_in_mode}", &app, new_id);
    assert_eq!(old_fmt, "1", "the unfocused copy-mode pane must report 1");
    assert_eq!(new_fmt, "0", "the focused non-copy-mode pane must report 0");

    let old_mode = crate::format::expand_format_for_pane_by_id("#{pane_mode}", &app, old_id);
    let new_mode = crate::format::expand_format_for_pane_by_id("#{pane_mode}", &app, new_id);
    assert_eq!(old_mode, "copy-mode");
    assert_eq!(new_mode, "");
}
