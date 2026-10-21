// pane-base-index is a window option (tmux options-table.c:1552-1554, read
// from w->options in window.c:1124 and :1162). `set -w` stores it on one
// window and every pane index read goes through AppState::pane_base_index_of,
// which prefers that window's own value over the global one. End to end in
// tests/test_pane_base_index_window_scope.ps1.

use super::*;

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: name.to_string(),
        id,
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

fn app2() -> AppState {
    let mut app = AppState::new("pbiw".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(make_window("zero", 0));
    app.windows.push(make_window("one", 1));
    app
}

#[test]
fn set_w_scopes_to_one_window() {
    let mut app = app2();
    execute_command_string(&mut app, "set -w -t :1 pane-base-index 1").unwrap();
    assert_eq!(app.pane_base_index_of(1), 1);
    assert_eq!(app.pane_base_index_of(0), 0, "window 0 keeps the global value");
    assert_eq!(app.pane_base_index, 0, "the global value is untouched");
}

#[test]
fn set_g_is_the_global_value_for_every_window() {
    let mut app = app2();
    execute_command_string(&mut app, "set -g pane-base-index 1").unwrap();
    assert_eq!(app.pane_base_index, 1);
    assert_eq!((app.pane_base_index_of(0), app.pane_base_index_of(1)), (1, 1));
}

#[test]
fn window_value_outranks_global_and_unset_restores_it() {
    let mut app = app2();
    execute_command_string(&mut app, "set -g pane-base-index 1").unwrap();
    execute_command_string(&mut app, "set -w -t :0 pane-base-index 5").unwrap();
    assert_eq!((app.pane_base_index_of(0), app.pane_base_index_of(1)), (5, 1));
    execute_command_string(&mut app, "set -w -t :0 -u pane-base-index").unwrap();
    assert_eq!(app.pane_base_index_of(0), 1);
}

#[test]
fn pane_base_index_is_a_window_scoped_write() {
    assert!(crate::server::options::is_window_scoped_write("pane-base-index"));
    let mut app = app2();
    app.windows[1].window_options.insert("pane-base-index".into(), "2".into());
    assert_eq!(crate::server::options::get_window_option_value_for(&app, "pane-base-index", Some(1)), "2");
    assert_eq!(crate::server::options::get_window_option_value_for(&app, "pane-base-index", Some(0)), "0");
}

#[test]
fn out_of_range_window_falls_back_to_global() {
    let mut app = app2();
    app.pane_base_index = 3;
    assert_eq!(app.pane_base_index_of(99), 3);
}
