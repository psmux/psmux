use super::*;
use crate::types::{FloatingPane, LayoutKind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ROWS: u16 = 10;
const COLS: u16 = 40;
const COPY: &str = "#{copy_cursor_x},#{copy_cursor_y},#{scroll_position}";
const SELECTION: &str = "#{selection_present},#{selection_active},#{selection_start_x},#{selection_start_y},#{selection_end_x},#{selection_end_y},#{search_present}";

fn make_pane(id: usize) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    });
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 200)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term,
        last_rows: ROWS,
        last_cols: COLS,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false,
        input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn mock_window(name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: name.to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
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

fn pane(app: &AppState, id: usize) -> &Pane {
    let win = &app.windows[0];
    crate::tree::get_nth_pane(&win.root, id - 1).unwrap()
}

fn history(p: &Pane) -> usize {
    p.live_parser().lock().unwrap().screen().scrollback_filled()
}

fn rig() -> AppState {
    let mut app = AppState::new("pane-copy-formats".into());
    app.pane_base_index = 0;
    let mut win = mock_window("copy");
    let first = make_pane(1);
    let second = make_pane(2);
    for (p, lines) in [(&first, 14), (&second, 30)] {
        let mut parser = p.term.lock().unwrap();
        for i in 0..lines {
            parser.process(format!("pane {} line {i}\r\n", p.id).as_bytes());
        }
    }
    assert!(history(&first) > 2);
    assert!(history(&second) > history(&first));
    win.root = Node::Split {
        kind: LayoutKind::Horizontal,
        sizes: vec![50, 50],
        children: vec![Node::Leaf(first), Node::Leaf(second)],
    };
    win.active_path = vec![0];
    app.windows.push(win);
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((2, 3));
    app.copy_scroll_offset = 1;
    crate::copy_mode::switch_with_copy_save(&mut app, |a| a.windows[0].active_path = vec![1]);
    crate::copy_mode::enter_copy_mode(&mut app);
    app.copy_pos = Some((4, 5));
    app.copy_scroll_offset = 7;
    crate::copy_mode::switch_with_copy_save(&mut app, |a| a.windows[0].active_path = vec![0]);
    // The active pane's parked copy deliberately lags behind its live cursor.
    assert_eq!(app.copy_pos, Some((2, 3)));
    app.copy_pos = Some((6, 8));
    app.copy_scroll_offset = 2;
    app
}

#[test]
fn pane_copy_formats_use_live_owner_and_parked_target() {
    let mut app = rig();
    assert!(app.mode.in_copy());
    assert_eq!(pane(&app, 1).copy_state.as_ref().unwrap().pos, Some((2, 3)));
    assert_eq!(pane(&app, 2).copy_state.as_ref().unwrap().pos, Some((4, 5)));
    assert_eq!(expand_format(COPY, &app), "8,6,2");
    assert_eq!(expand_format_for_pane_by_id(COPY, &app, 2), "5,4,7");
    assert_eq!(format_list_panes(&app, COPY, 0), "8,6,2\n5,4,7");
    assert_eq!(
        expand_format(COPY, &app),
        "8,6,2",
        "target override must not leak"
    );

    crate::copy_mode::switch_with_copy_save(&mut app, |a| a.windows[0].active_path = vec![1]);
    assert_eq!(app.copy_pos, Some((4, 5)));
    app.copy_pos = Some((0, 0));
    app.copy_scroll_offset = 0;
    assert_eq!(expand_format(COPY, &app), "0,0,0");
    assert_eq!(expand_format_for_pane_by_id(COPY, &app, 1), "8,6,2");
    app.copy_pos = None;
    assert_eq!(expand_format(COPY, &app), "0,0,0");
}

#[test]
fn pane_copy_formats_zero_for_target_without_copy_mode() {
    let mut app = rig();
    crate::tree::active_pane_mut(&mut app.windows[0].root, &vec![1])
        .unwrap()
        .copy_state = None;
    assert!(app.mode.in_copy());
    assert!(pane(&app, 2).copy_state.is_none());
    assert_eq!(expand_format_for_pane_by_id(COPY, &app, 2), "0,0,0");

    // Normal mode must ignore both stale globals and the active pane's parked state.
    app.mode = Mode::Passthrough;
    assert!(app.copy_pos.is_some());
    assert!(pane(&app, 1).copy_state.is_some());
    assert_eq!(expand_format(COPY, &app), "0,0,0");
}

#[test]
fn pane_copy_formats_history_uses_targets_live_parser_during_snapshot() {
    let app = rig();
    let first = pane(&app, 1);
    let second = pane(&app, 2);
    assert!(first.live_term.is_some());
    assert!(second.live_term.is_some());
    let frozen_first = first.term.lock().unwrap().screen().scrollback_filled();
    let frozen_second = second.term.lock().unwrap().screen().scrollback_filled();
    for p in [first, second] {
        p.live_parser()
            .lock()
            .unwrap()
            .process(b"new output\r\nmore output\r\n");
    }
    assert_eq!(
        first.term.lock().unwrap().screen().scrollback_filled(),
        frozen_first
    );
    assert_eq!(
        second.term.lock().unwrap().screen().scrollback_filled(),
        frozen_second
    );
    assert_eq!(history(first), frozen_first + 2);
    assert_eq!(history(second), frozen_second + 2);
    assert_ne!(history(first), history(second));
    assert_eq!(
        expand_format("#{history_size}", &app),
        history(first).to_string()
    );
    assert_eq!(
        expand_format_for_pane_by_id("#{history_size}", &app, 2),
        history(second).to_string()
    );
    assert_eq!(
        format_list_panes(&app, "#{history_size}", 0),
        format!("{}\n{}", history(first), history(second))
    );
}

#[test]
fn pane_copy_formats_resolve_floating_target_and_live_history() {
    let mut app = rig();
    let Node::Split { children, .. } = &mut app.windows[0].root else {
        panic!("two panes")
    };
    let Node::Leaf(float) = children.pop().unwrap() else {
        panic!("second pane")
    };
    let expected_history = history(&float);
    assert!(float.copy_state.is_some());
    assert!(float.live_term.is_some());
    app.windows[0].floating.push(FloatingPane {
        pane: float,
        id: 2,
        x: 1,
        y: 1,
        w: 42,
        h: 12,
        border: "single".into(),
        title: String::new(),
        position: None,
        styles: Default::default(),
    });
    for focus in [None, Some(0)] {
        app.windows[0].floating_focus = focus;
        assert_eq!(expand_format_for_pane_by_id(COPY, &app, 2), "5,4,7");
        assert_eq!(
            expand_format_for_pane_by_id("#{history_size}", &app, 2),
            expected_history.to_string()
        );
    }
    assert_eq!(expand_format(COPY, &app), "5,4,7");
    assert_eq!(
        expand_format("#{history_size}", &app),
        expected_history.to_string()
    );
}

fn selection_rig() -> AppState {
    let mut app = rig();
    app.copy_anchor = Some((6, 7));
    app.copy_search_query = "live query".into();
    let win = &mut app.windows[0];
    let first = crate::tree::active_pane_mut(&mut win.root, &vec![0]).unwrap();
    first.copy_state.as_mut().unwrap().anchor = Some((1, 2));
    first.copy_state.as_mut().unwrap().search_query.clear();
    let second = crate::tree::active_pane_mut(&mut win.root, &vec![1]).unwrap();
    second.copy_state.as_mut().unwrap().anchor = Some((3, 4));
    app
}

#[test]
fn pane_copy_formats_selection_uses_live_owner_and_parked_target() {
    let app = selection_rig();
    assert_eq!(app.copy_anchor, Some((6, 7)));
    assert_eq!(
        pane(&app, 1).copy_state.as_ref().unwrap().anchor,
        Some((1, 2))
    );
    assert_eq!(
        pane(&app, 2).copy_state.as_ref().unwrap().anchor,
        Some((3, 4))
    );
    assert!(!app.copy_search_query.is_empty());
    assert!(pane(&app, 2)
        .copy_state
        .as_ref()
        .unwrap()
        .search_query
        .is_empty());
    assert_eq!(expand_format(SELECTION, &app), "1,1,7,6,8,6,1");
    assert_eq!(
        expand_format_for_pane_by_id(SELECTION, &app, 2),
        "1,1,4,3,5,4,0"
    );
    assert_eq!(
        format_list_panes(&app, SELECTION, 0),
        "1,1,7,6,8,6,1\n1,1,4,3,5,4,0"
    );
    assert_eq!(expand_format(SELECTION, &app), "1,1,7,6,8,6,1");
}

#[test]
fn pane_copy_formats_selection_ignores_stale_state_outside_copy_mode() {
    let mut app = selection_rig();
    crate::tree::active_pane_mut(&mut app.windows[0].root, &vec![1])
        .unwrap()
        .copy_state = None;
    assert!(app.mode.in_copy());
    assert!(pane(&app, 2).copy_state.is_none());
    assert_eq!(
        expand_format_for_pane_by_id(SELECTION, &app, 2),
        "0,0,0,0,0,0,0"
    );
    assert_eq!(
        expand_format_for_pane_by_id("#{copy_cursor_word}", &app, 2),
        ""
    );
    app.mode = Mode::Passthrough;
    assert!(app.copy_anchor.is_some());
    assert!(!app.copy_search_query.is_empty());
    assert!(pane(&app, 1).copy_state.is_some());
    assert_eq!(expand_format(SELECTION, &app), "0,0,0,0,0,0,0");
    assert_eq!(expand_format("#{copy_cursor_word}", &app), "");

    for mode in [
        Mode::CopyMode,
        Mode::CopySearch {
            input: String::new(),
            forward: true,
        },
    ] {
        app.mode = mode;
        app.copy_anchor = None;
        app.copy_search_query.clear();
        assert!(app.mode.in_copy());
        assert!(app.copy_pos.is_some());
        assert_eq!(expand_format(SELECTION, &app), "0,0,0,0,8,6,0");
    }
}

#[test]
fn pane_copy_formats_selection_from_another_window_and_floating_pane() {
    let mut app = selection_rig();
    let Node::Split { children, .. } = &mut app.windows[0].root else {
        panic!("two panes")
    };
    let second = children.pop().unwrap();
    let mut other = mock_window("other");
    other.id = 1;
    other.root = second;
    app.windows.push(other);
    assert_eq!(app.active_idx, 0);
    assert_eq!(crate::copy_mode::active_pane_id(&app), Some(1));
    assert_eq!(
        expand_format_for_pane_by_id(SELECTION, &app, 2),
        "1,1,4,3,5,4,0"
    );
    assert_eq!(format_list_panes(&app, SELECTION, 1), "1,1,4,3,5,4,0");

    let Node::Leaf(second) = app.windows.pop().unwrap().root else {
        panic!("second pane")
    };
    app.windows[0].floating.push(FloatingPane {
        pane: second,
        id: 2,
        x: 1,
        y: 1,
        w: 42,
        h: 12,
        border: "single".into(),
        title: String::new(),
        position: None,
        styles: Default::default(),
    });
    assert_eq!(
        expand_format_for_pane_by_id(SELECTION, &app, 2),
        "1,1,4,3,5,4,0"
    );
    assert_eq!(expand_format(SELECTION, &app), "1,1,7,6,8,6,1");
}

#[test]
fn pane_copy_formats_word_uses_target_cursor_snapshot_and_scroll_without_changing_view() {
    let mut app = rig();
    app.copy_pos = Some((0, 2));
    app.copy_scroll_offset = 3;
    let second = crate::tree::active_pane_mut(&mut app.windows[0].root, &vec![1]).unwrap();
    let parked = second.copy_state.as_mut().unwrap();
    parked.pos = Some((1, 2));
    parked.scroll_offset = 5;
    for (id, prefix) in [(1, "live"), (2, "park")] {
        let p = pane(&app, id);
        assert!(
            p.live_term.is_some(),
            "copy snapshot must differ from the live parser"
        );
        let mut parser = p.term.lock().unwrap();
        *parser = vt100::Parser::new(ROWS, COLS, 200);
        for row in 0..20 {
            parser.process(format!("{prefix}{row}\r\n").as_bytes());
        }
        assert_eq!(parser.screen().scrollback_filled(), 11);
        parser.screen_mut().set_scrollback(1);
    }
    assert_eq!(expand_format("#{copy_cursor_word}", &app), "live8");
    assert_eq!(
        expand_format_for_pane_by_id("#{copy_cursor_word}", &app, 2),
        "park7"
    );
    assert_eq!(
        format_list_panes(&app, "#{copy_cursor_word}", 0),
        "live8\npark7"
    );
    for id in [1, 2] {
        assert_eq!(pane(&app, id).term.lock().unwrap().screen().scrollback(), 1);
    }

    app.copy_pos = None;
    assert_eq!(expand_format("#{copy_cursor_word}", &app), "");
    assert_eq!(
        expand_format_for_pane_by_id("#{copy_cursor_word}", &app, 2),
        "park7"
    );

    let Node::Split { children, .. } = &mut app.windows[0].root else {
        panic!("two panes")
    };
    let mut other = mock_window("other");
    other.id = 1;
    other.root = children.pop().unwrap();
    app.windows.push(other);
    assert_eq!(
        expand_format_for_pane_by_id("#{copy_cursor_word}", &app, 2),
        "park7"
    );
    assert_eq!(format_list_panes(&app, "#{copy_cursor_word}", 1), "park7");
    assert_eq!(expand_format("#{copy_cursor_word}", &app), "");

    let Node::Leaf(second) = app.windows.pop().unwrap().root else {
        panic!("second pane")
    };
    app.windows[0].floating.push(FloatingPane {
        pane: second,
        id: 2,
        x: 1,
        y: 1,
        w: 42,
        h: 12,
        border: "single".into(),
        title: String::new(),
        position: None,
        styles: Default::default(),
    });
    assert_eq!(
        expand_format_for_pane_by_id("#{copy_cursor_word}", &app, 2),
        "park7"
    );
    assert_eq!(
        app.windows[0].floating[0]
            .pane
            .term
            .lock()
            .unwrap()
            .screen()
            .scrollback(),
        1
    );
}
