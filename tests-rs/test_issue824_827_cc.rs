// Issues #824 and #827, control mode parity.
//
// #824: while a window is zoomed, #{window_layout} is the real split and
// #{window_visible_layout} the zoomed pane alone at the window size (tmux
// format.c format_cb_window_layout dumps saved_layout_root), and
// %layout-change carries #{window_raw_flags}, Z included
// (control-notify.c control_window_layout_changed_cb).
// #827: #{pane_tabs} lists the default tab stops (screen.c
// screen_reset_tabs); a layout string's cells come back exactly
// (layout-custom.c layout_parse); a pane entering or leaving a mode is
// announced as %pane-mode-changed (control_pane_mode_changed_cb).
//
// Registered from src/control.rs.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    let child = crate::util::StubChild::exited();
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        dead_info: None,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

/// One window, 120x30, panes 1 and 2 side by side 59/60, pane 2 active.
fn app_with_split() -> AppState {
    let mut app = AppState::new("i824".to_string());
    let area = Rect::new(0, 0, 120, 30);
    app.last_window_area = area;
    app.client_area = area;
    let win = crate::types::Window {
        root: Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![50, 50],
            children: vec![Node::Leaf(make_pane(1, 30, 59)), Node::Leaf(make_pane(2, 30, 60))],
        },
        active_path: vec![1],
        name: "w".to_string(),
        id: 1,
        area,
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![2, 1],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    };
    app.windows.push(win);
    app.window_indices.push(0);
    app.active_idx = 0;
    app
}

fn csum(body: &str) -> String {
    format!("{:04x},{}", layout_checksum(body), body)
}

#[test]
fn zoomed_window_keeps_the_split_and_shows_the_zoomed_pane() {
    let mut app = app_with_split();
    let split = csum("120x30,0,0{59x30,0,0,1,60x30,60,0,2}");
    let area = app.windows[0].area;
    assert_eq!(window_layout_string(&app.windows[0], area), split);
    assert_eq!(window_visible_layout_string(&app.windows[0], area), split);

    crate::window_ops::toggle_zoom(&mut app);
    assert!(app.windows[0].zoom_saved.is_some());
    // The live tree is squeezed while zoomed; the dumps are not.
    assert_eq!(window_layout_string(&app.windows[0], area), split);
    assert_eq!(window_visible_layout_string(&app.windows[0], area), csum("120x30,0,0,2"));
    let line = format_notification(&layout_change(&app, 0, area).unwrap());
    assert_eq!(line, format!("%layout-change @1 {} {} *Z", split, csum("120x30,0,0,2")));
    assert_eq!(crate::format::expand_format_for_window("#{window_layout}", &app, 0), split);
    assert_eq!(crate::format::expand_format_for_window("#{window_visible_layout}", &app, 0), csum("120x30,0,0,2"));

    crate::window_ops::toggle_zoom(&mut app);
    assert!(app.windows[0].zoom_saved.is_none());
    let line = format_notification(&layout_change(&app, 0, area).unwrap());
    assert_eq!(line, format!("%layout-change @1 {} {} *", split, split));
}

#[test]
fn layout_change_flags_follow_the_window() {
    let mut app = app_with_split();
    app.windows[0].bell_flag = true;
    let area = app.windows[0].area;
    let line = format_notification(&layout_change(&app, 0, area).unwrap());
    assert!(line.ends_with(" *!"), "{line}");
    assert!(layout_change(&app, 5, area).is_none());
}

#[test]
fn pane_tabs_are_every_eight_columns_from_eight() {
    assert_eq!(crate::format::pane_tab_stops(100), "8,16,24,32,40,48,56,64,72,80,88,96");
    assert_eq!(crate::format::pane_tab_stops(120), "8,16,24,32,40,48,56,64,72,80,88,96,104,112");
    assert_eq!(crate::format::pane_tab_stops(9), "8");
    assert_eq!(crate::format::pane_tab_stops(8), "");
    assert_eq!(crate::format::pane_tab_stops(0), "");
}

/// The cells a parsed layout split gives its children in `area`.
fn parsed_cells(layout: &str, area: Rect) -> Vec<(u16, u16)> {
    let node = crate::layout::parse_layout_string(layout).expect("parse");
    let crate::layout::LayoutNode::Split { kind, children, .. } = node else { panic!("not a split") };
    let weights = crate::layout::layout_split_weights(kind, &children);
    crate::tree::split_with_gaps(matches!(kind, LayoutKind::Horizontal), &weights, area)
        .iter().map(|r| (r.width, r.height)).collect()
}

#[test]
fn layout_string_cells_come_back_exactly() {
    // The issue's case: 29/70 came back 28/71.
    assert_eq!(parsed_cells("69a0,100x30,0,0{29x30,0,0,1,70x30,30,0,3}", Rect::new(0, 0, 100, 30)), vec![(29, 30), (70, 30)]);
    // Every split of a 100 column window, and of a 30 row one.
    for a in 2..=97u16 {
        let s = format!("0000,100x30,0,0{{{a}x30,0,0,1,{}x30,{},0,2}}", 99 - a, a + 1);
        assert_eq!(parsed_cells(&s, Rect::new(0, 0, 100, 30)), vec![(a, 30), (99 - a, 30)], "{s}");
    }
    for a in 1..=28u16 {
        let s = format!("0000,100x30,0,0[100x{a},0,0,1,100x{},0,{},2]", 29 - a, a + 1);
        assert_eq!(parsed_cells(&s, Rect::new(0, 0, 100, 30)), vec![(100, a), (100, 29 - a)], "{s}");
    }
    // Three way.
    assert_eq!(parsed_cells("0000,120x30,0,0{41x30,0,0,1,7x30,42,0,2,70x30,50,0,3}", Rect::new(0, 0, 120, 30)), vec![(41, 30), (7, 30), (70, 30)]);
}

#[test]
fn pane_mode_changes_name_the_pane() {
    let before = vec![(1usize, "copy-mode")];
    let after = vec![(1usize, "copy-mode"), (3usize, "copy-mode")];
    let lines: Vec<String> = pane_mode_notifications(&before, &after).iter().map(format_notification).collect();
    assert_eq!(lines, vec!["%pane-mode-changed %3".to_string()]);
    let lines: Vec<String> = pane_mode_notifications(&after, &[]).iter().map(format_notification).collect();
    assert_eq!(lines, vec!["%pane-mode-changed %1".to_string(), "%pane-mode-changed %3".to_string()]);
    let lines: Vec<String> = pane_mode_notifications(&[(2, "copy-mode")], &[(2, "clock-mode")]).iter().map(format_notification).collect();
    assert_eq!(lines, vec!["%pane-mode-changed %2".to_string()]);
    assert!(pane_mode_notifications(&after, &after).is_empty());
}

#[test]
fn pane_modes_reads_the_focused_and_the_parked_panes() {
    let mut app = app_with_split();
    assert!(pane_modes(&app).is_empty());
    app.mode = crate::types::Mode::CopyMode;
    assert_eq!(pane_modes(&app), vec![(2, "copy-mode")]);
    app.mode = crate::types::Mode::ClockMode;
    assert_eq!(pane_modes(&app), vec![(2, "clock-mode")]);
}
