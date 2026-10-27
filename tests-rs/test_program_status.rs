use super::*;
use crate::types::{LayoutKind, Window};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8};
use std::time::Instant;

fn pane(id: usize) -> Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: 24, cols: 80, pixel_width: 0, pixel_height: 0,
    });
    Pane {
        master, writer, child: crate::util::StubChild::running(),
        term: Arc::new(Mutex::new({
            // The `program-status` option is on for these tests.
            let mut parser = vt100::Parser::default();
            parser.screen_mut().set_program_status_enabled(true);
            parser
        })),
        last_rows: 24, last_cols: 80, id, title: "test".into(), title_locked: true,
        child_pid: None, data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: Instant::now(), last_infer_title: Instant::now(), dead: false, dead_info: None,
        last_text_input: None, last_special_key: None, vt_bridge_cache: None,
        vti_mode_cache: None, mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)), bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)), color_query_pending: Arc::new(AtomicU32::new(0)),
        copy_state: None, live_term: None, pane_style: None, pane_options: Default::default(),
        squelch_until: None, output_ring: Default::default(), spawned_at: None,
        start_command: String::new(), cwd_hint: None,
    }
}

fn window(id: usize) -> Window {
    Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![50, 50],
            children: vec![Node::Leaf(pane(id * 10)), Node::Leaf(pane(id * 10 + 1))] },
        active_path: vec![0], name: "test".into(), id,
        area: ratatui::layout::Rect::new(0, 0, 160, 24), window_size: None,
        window_options: Default::default(), activity_flag: false, bell_flag: false,
        silence_flag: false, last_output_time: Instant::now(), last_seen_version: 0,
        manual_rename: true, layout_index: 0, pane_mru: vec![], zoom_saved: None,
        linked_from: None, floating: vec![], floating_focus: None,
    }
}

fn app() -> AppState {
    let mut app = AppState::new("program-status-unit-test".into());
    app.windows = vec![window(1), window(2)];
    app.window_indices = vec![0, 1];
    app.active_idx = 0;
    app
}

fn get_pane(app: &AppState, window: usize, index: usize) -> &Pane {
    crate::tree::active_pane(&app.windows[window].root, &[index]).unwrap()
}

fn report(pane: &Pane, body: &str) {
    pane.live_parser().lock().unwrap().process(format!("\x1b]7501;{body}\x07").as_bytes());
}

/// `list-windows -J`, the inspection path that carries full records.
fn listing(app: &AppState) -> serde_json::Value {
    serde_json::from_str(&crate::util::list_windows_json(app).unwrap()).unwrap()
}

/// The documented opt-in tab format (docs/program-status.md).
const RECIPE: &str = "#I:#W#{?window_flags,#{window_flags},#{?window_ps_state,, }}#{?window_ps_state, [#{window_ps_state}],}";

fn enable_status(app: &mut AppState) {
    crate::config::parse_config_content(app, &format!(
        "set -g window-status-format '{RECIPE}'\nset -g window-status-current-format '{RECIPE}'\n",
    ));
}

#[test]
fn documented_recipe_matches_stock_tabs_without_reports_and_never_doubles_a_space() {
    let mut app = app();
    let stock: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    enable_status(&mut app);
    let quiet: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    for window in 0..2 {
        assert_eq!(quiet[window]["tab_text"], stock[window]["tab_text"], "no reports: identical to stock");
    }
    report(get_pane(&app, 0, 0), "state=blocked");
    report(get_pane(&app, 1, 0), "state=error");
    let json: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    for window in 0..2 {
        let tab = json[window]["tab_text"].as_str().unwrap();
        assert!(!tab.contains("  "), "double space in {tab:?}");
    }
    assert_eq!(json[0]["tab_text"], "0:test* [blocked]");
    let second = json[1]["tab_text"].as_str().unwrap();
    assert!(matches!(second, "1:test [error]" | "1:test- [error]"), "{second:?}");
}

#[test]
fn custom_tabs_show_all_states_only_after_explicit_opt_in() {
    let mut app = app();
    crate::config::parse_config_content(&mut app, "set -g window-status-format INACTIVE:#W\nset -g window-status-current-format ACTIVE:#W\n");
    for window in 0..2 { report(get_pane(&app, window, 0), "state=blocked"); }
    let before: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    assert_eq!(before[0]["tab_text"], "ACTIVE:test");
    assert_eq!(before[1]["tab_text"], "INACTIVE:test");
    let config = "set -g window-status-format 'INACTIVE:#W:#{window_ps_state}'\nset -g window-status-current-format 'ACTIVE:#W:#{window_ps_state}'\n";
    crate::config::parse_config_content(&mut app, config);
    crate::config::parse_config_content(&mut app, config);
    for state in ["idle", "working", "done", "error", "blocked"] {
        for window in 0..2 { report(get_pane(&app, window, 0), &format!("state={state}")); }
        let json: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
        for (window, prefix) in [(0, "ACTIVE:test"), (1, "INACTIVE:test")] {
            assert_eq!(json[window]["tab_text"], format!("{prefix}:{state}"), "{state}");
            assert!(json[window].get("ps_panes").is_none(), "frames never carry records");
            assert_eq!(listing(&app)[window]["ps_panes"][0]["records"][0]["state"], state);
        }
    }
}

#[test]
fn stock_tabs_preserve_tmux_defaults_even_with_program_reports() {
    let mut app = app();
    let stock = "#I:#W#{?window_flags,#{window_flags}, }";
    assert_eq!(app.window_status_format, stock);
    assert_eq!(app.window_status_current_format, stock);
    crate::config::parse_config_content(&mut app, concat!(
        "set -g window-status-format CUSTOM_INACTIVE\n",
        "set -g window-status-current-format CUSTOM_ACTIVE\n",
    ));
    assert_eq!(app.window_status_format, "CUSTOM_INACTIVE");
    assert_eq!(app.window_status_current_format, "CUSTOM_ACTIVE");
    crate::config::parse_config_content(&mut app, "set -gu window-status-format\n");
    assert_eq!(app.window_status_format, stock);
    assert_eq!(app.window_status_current_format, "CUSTOM_ACTIVE");
    crate::config::parse_config_content(&mut app, "set -gu window-status-current-format\n");
    assert_eq!(app.window_status_format, stock);
    assert_eq!(app.window_status_current_format, stock);
    let before: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    for state in ["idle", "working", "done", "error", "blocked"] {
        for window in 0..2 { report(get_pane(&app, window, 0), &format!("state={state}")); }
        let json: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
        for window in 0..2 {
            assert_eq!(json[window]["tab_text"], before[window]["tab_text"], "{state}");
            assert_eq!(listing(&app)[window]["ps_panes"][0]["records"][0]["state"], state);
        }
    }
}

#[test]
fn background_status_changes_invalidate_frames_without_output_alerts() {
    let mut app = app();
    enable_status(&mut app);
    let before = crate::server::helpers::combined_data_version(&app);
    let mut cached_version = 0;
    let mut cached_windows = String::new();
    assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached_windows).unwrap());
    let initial_json: serde_json::Value = serde_json::from_str(&cached_windows).unwrap();
    assert!(initial_json[1].get("ps_status").is_none());
    assert!(window_status(&app.windows[1]).is_none());
    report(get_pane(&app, 1, 1), "state=done");
    assert_ne!(crate::server::helpers::combined_data_version(&app), before);
    assert_eq!(window_status(&app.windows[1]).unwrap().pane, 21);
    assert!(!app.windows[1].activity_flag);
    assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached_windows).unwrap());
    let updated_json: serde_json::Value = serde_json::from_str(&cached_windows).unwrap();
    assert_eq!(updated_json[1]["ps_status"]["state"], "done");
    assert!(updated_json[1]["tab_text"].as_str().unwrap().contains("[done]"));
    let version = crate::server::helpers::combined_data_version(&app);
    get_pane(&app, 1, 1).live_parser().lock().unwrap().process(b"ordinary output");
    assert_eq!(crate::server::helpers::combined_data_version(&app), version);
    assert!(!crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached_windows).unwrap());
    report(get_pane(&app, 1, 1), "state=clear");
    assert_ne!(crate::server::helpers::combined_data_version(&app), version);
    assert!(window_status(&app.windows[1]).is_none());
}

#[test]
fn return_acknowledges_only_current_target_results_shared_by_all_readers() {
    let mut app = app();
    report(get_pane(&app, 0, 0), "state=done");
    report(get_pane(&app, 1, 0), "state=error");
    report(get_pane(&app, 1, 1), "state=blocked");
    assert!(window_status(&app.windows[0]).is_some());
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "blocked");
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert!(get_pane(&app, 1, 0).live_parser().lock().unwrap().screen().program_status().seen(""));
    assert!(window_status(&app.windows[0]).is_some());
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "blocked");
    let seen = window_records(&app.windows[1]);
    let another_client = window_records(&app.windows[1]);
    assert_eq!(serde_json::to_value(seen).unwrap(), serde_json::to_value(another_client).unwrap());
    report(get_pane(&app, 1, 0), "state=done");
    assert!(!get_pane(&app, 1, 0).live_parser().lock().unwrap().screen().program_status().seen(""));
    crate::copy_mode::switch_with_copy_save(&mut app, |_| {});
    assert!(!get_pane(&app, 1, 0).live_parser().lock().unwrap().screen().program_status().seen(""));
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 0);
    assert!(window_status(&app.windows[0]).unwrap().seen);
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert!(get_pane(&app, 1, 0).live_parser().lock().unwrap().screen().program_status().seen(""));
}

#[test]
fn return_acknowledgement_linearizes_under_parser_contention() {
    let mut app = app();
    let parser = get_pane(&app, 1, 0).live_parser().clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (focus_tx, focus_rx) = std::sync::mpsc::channel();
    let (reported_tx, reported_rx) = std::sync::mpsc::channel();
    let (returned_tx, returned_rx) = std::sync::mpsc::channel();
    let timeout = std::time::Duration::from_secs(5);
    let reporter = std::thread::spawn(move || {
        let mut terminal = parser.lock().unwrap();
        ready_tx.send(()).unwrap();
        focus_rx.recv_timeout(timeout).unwrap();
        terminal.process(b"\x1b]7501;state=done\x07");
        reported_tx.send(()).unwrap();
        drop(terminal);
        returned_rx.recv_timeout(timeout).unwrap();
        parser.lock().unwrap().process(b"\x1b]7501;state=error\x07");
    });
    ready_rx.recv_timeout(timeout).unwrap();
    crate::copy_mode::switch_with_copy_save(&mut app, |app| {
        app.active_idx = 1;
        focus_tx.send(()).unwrap();
        reported_rx.recv_timeout(timeout).unwrap();
    });
    assert!(get_pane(&app, 1, 0).live_parser().lock().unwrap().screen().program_status().seen(""));
    returned_tx.send(()).unwrap();
    reporter.join().unwrap();
    let terminal = get_pane(&app, 1, 0).live_parser().lock().unwrap();
    assert_eq!(terminal.screen().program_status().get("").unwrap().state, vt100::ProgramState::Error);
    assert!(!terminal.screen().program_status().seen(""));
}

#[test]
fn respawn_then_immediate_report_refreshes_cached_tabs_and_records() {
    let mut app = app();
    enable_status(&mut app);
    report(get_pane(&app, 1, 0), "state=done");
    let mut cached_version = 0;
    let mut cached_windows = String::new();
    assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached_windows).unwrap());
    let before = crate::server::helpers::combined_data_version(&app);
    get_pane(&app, 1, 0).live_parser().lock().unwrap().screen_mut().reinit_keep_history();
    report(get_pane(&app, 1, 0), "state=blocked");
    assert_ne!(crate::server::helpers::combined_data_version(&app), before);
    assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached_windows).unwrap());
    let json: serde_json::Value = serde_json::from_str(&cached_windows).unwrap();
    assert_eq!(json[1]["ps_status"]["state"], "blocked");
    assert!(json[1].get("ps_panes").is_none());
    assert_eq!(listing(&app)[1]["ps_panes"][0]["records"][0]["state"], "blocked");
    assert!(json[1]["tab_text"].as_str().unwrap().contains("[blocked]"));
    assert!(!json[1]["tab_text"].as_str().unwrap().contains("[done]"));
}

#[test]
fn aggregation_prioritizes_blocked_error_done_and_recent_changes_across_panes() {
    let app = app();
    report(get_pane(&app, 1, 0), "state=error");
    report(get_pane(&app, 1, 1), "state=done");
    assert_eq!(window_status(&app.windows[1]).unwrap().pane, 20);
    report(get_pane(&app, 1, 1), "state=blocked:kind=question:id=child");
    assert_eq!(window_status(&app.windows[1]).unwrap().id, "child");
    report(get_pane(&app, 1, 0), "state=blocked:kind=auth");
    assert_eq!(window_status(&app.windows[1]).unwrap().pane, 20);
    report(get_pane(&app, 1, 1), "state=blocked:kind=question:id=child");
    assert_eq!(window_status(&app.windows[1]).unwrap().pane, 20);
}

#[test]
fn formats_and_structured_records_keep_literal_text_out_of_style_execution() {
    let mut app = app();
    enable_status(&mut app);
    report(get_pane(&app, 0, 0), "state=blocked:kind=auth:progress=0:app=pi:msg=I1tmZz1yZWRd");
    assert_eq!(crate::format::expand_format("#{pane_ps_state}|#{pane_ps_kind}|#{pane_ps_progress}|#{pane_ps_app}", &app), "blocked|auth|0|pi");
    assert_eq!(crate::format::expand_format("#{window_ps_state}|#{window_ps_seen}", &app), "blocked|0");
    let frame = crate::server::helpers::list_windows_json_with_tabs(&app).unwrap();
    let json: serde_json::Value = serde_json::from_str(&frame).unwrap();
    assert!(json[0].get("ps_panes").is_none());
    assert_eq!(listing(&app)[0]["ps_panes"][0]["records"][0]["msg"], "#[fg=red]");
    assert!(!json[0]["tab_text"].as_str().unwrap().contains("#[fg=red]"));
    assert!(json[0]["tab_text"].as_str().unwrap().contains("[blocked]"));
    let old: crate::util::WinInfo = serde_json::from_str(r#"{"id":1,"name":"old","active":true}"#).unwrap();
    assert!(old.ps_status.is_none());
    assert!(old.ps_panes.is_empty());
}

#[test]
fn copy_mode_inspects_and_acknowledges_live_records_and_respawn_withdraws_them() {
    let mut app = app();
    let path = vec![0];
    let pane = crate::tree::active_pane_mut(&mut app.windows[1].root, &path).unwrap();
    report(pane, "state=working");
    pane.live_term = Some(pane.term.clone());
    let snapshot = pane.live_parser().lock().unwrap().snapshot();
    pane.term = Arc::new(Mutex::new(snapshot));
    report(pane, "state=error");
    assert_eq!(pane.term.lock().unwrap().screen().program_status().get("").unwrap().state, vt100::ProgramState::Working);
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "error");
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert!(window_status(&app.windows[1]).unwrap().seen);
    let before = version(&app);
    let pane = crate::tree::active_pane_mut(&mut app.windows[1].root, &path).unwrap();
    pane.live_parser().lock().unwrap().screen_mut().reinit_keep_history();
    assert_ne!(version(&app), before);
    report(get_pane(&app, 1, 0), "state=error");
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "error");
}

#[test]
fn floating_and_dead_retained_panes_participate_in_status_selection() {
    let mut app = app();
    let mut floating = pane(99);
    floating.dead = true;
    report(&floating, "state=error");
    app.windows[1].floating.push(crate::types::FloatingPane {
        pane: floating, id: 99, x: 1, y: 1, w: 20, h: 10,
        border: String::new(), title: String::new(), position: None, styles: Default::default(),
    });
    assert_eq!(window_status(&app.windows[1]).unwrap().pane, 99);
    app.windows[1].floating_focus = Some(0);
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert!(window_status(&app.windows[1]).unwrap().seen);
}

#[test]
fn program_status_option_is_off_by_default_and_the_single_switch() {
    // A stock server: off, listed, and a stock parser treats OSC 7501 as
    // unknown, so the capability query draws no reply and nothing is stored.
    // This test flips the process wide switch, so it holds it (off) and the
    // guard restores it whatever happens.
    let _switch = crate::types::program_status_switch_for_test(false);
    let mut app = app();
    assert!(!app.program_status);
    assert_eq!(crate::server::options::get_option_value(&app, "program-status"), "off");
    assert_eq!(crate::server::option_catalog::default_for("program-status"), Some("off"));
    let mut stock = vt100::Parser::default();
    stock.screen_mut().capture_queries(true);
    stock.process(b"\x1b]7501;state=blocked\x07\x1b]7501;?\x07\x1b[c");
    assert!(stock.screen().program_status().records().next().is_none());
    assert_eq!(stock.screen_mut().take_queries(), vec![vt100::TerminalQuery::Da1]);

    // Opting in to the indicator format alone changes nothing.
    for pane in [get_pane(&app, 1, 0), get_pane(&app, 1, 1)] {
        pane.live_parser().lock().unwrap().screen_mut().set_program_status_enabled(false);
    }
    enable_status(&mut app);
    report(get_pane(&app, 1, 0), "state=blocked");
    assert!(window_status(&app.windows[1]).is_none());
    assert!(window_records(&app.windows[1]).is_empty());

    // The option is the switch, and set -u restores off and withdraws records.
    crate::server::options::apply_set_option(&mut app, "program-status", "on", false).unwrap();
    assert!(app.program_status);
    assert_eq!(crate::server::options::get_option_value(&app, "program-status"), "on");
    report(get_pane(&app, 1, 0), "state=blocked");
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "blocked");
    crate::server::options::reset_option_to_default(&mut app, "program-status");
    assert!(!app.program_status);
    assert_eq!(crate::server::options::get_option_value(&app, "program-status"), "off");
    assert!(window_status(&app.windows[1]).is_none());
    assert!(window_records(&app.windows[1]).is_empty());
    assert!(!crate::types::PROGRAM_STATUS_ENABLED.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn working_in_a_background_child_drives_the_window_summary_and_formats() {
    let mut app = app();
    enable_status(&mut app);
    report(get_pane(&app, 1, 0), "state=idle:app=shell");
    report(get_pane(&app, 1, 1), "state=idle:app=build");
    report(get_pane(&app, 1, 1), "state=working:id=compile:progress=0:title=SGk=:msg=I1tmZz1yZWRd");
    // A newer idle report in the selected pane must not hide background work.
    report(get_pane(&app, 1, 0), "state=idle:app=newshell");
    let summary = window_status(&app.windows[1]).unwrap();
    assert_eq!((summary.pane, summary.id.as_str(), summary.state.as_str()), (21, "compile", "working"));
    assert_eq!(summary.app.as_deref(), Some("build"));
    assert_eq!(summary.progress, Some(0));
    assert!(!summary.seen);
    assert_eq!(crate::format::expand_format_for_window(
        "#{window_ps_state}|#{window_ps_id}|#{window_ps_pane}|#{window_ps_progress}|#{window_ps_app}|#{window_ps_seen}", &app, 1),
        "working|compile|%21|0|build|0");
    assert_eq!(pane_format(get_pane(&app, 1, 1), "pane_ps_state"), "working");
    assert_eq!(pane_format(get_pane(&app, 1, 1), "pane_ps_id"), "compile");
    assert_eq!(pane_format(get_pane(&app, 1, 1), "pane_ps_app"), "build");
    let frame: serde_json::Value = serde_json::from_str(&crate::server::helpers::list_windows_json_with_tabs(&app).unwrap()).unwrap();
    assert_eq!(frame[1]["ps_status"]["state"], "working");
    assert!(frame[1]["tab_text"].as_str().unwrap().contains("[working]"));
    assert!(frame[1].get("ps_panes").is_none());
    assert!(frame[1]["ps_status"].get("title").is_none());
    assert!(frame[1]["ps_status"].get("msg").is_none());
    let raw = listing(&app);
    assert_eq!(raw[1]["ps_panes"][1]["records"].as_array().unwrap().len(), 2);
    assert_eq!(raw[1]["ps_panes"][1]["records"][1]["msg"], "#[fg=red]");
}

#[test]
fn window_and_pane_use_one_ordering_for_all_state_and_seen_combinations() {
    // Low-priority idle/seen results share a bucket; newest semantic change wins.
    let cases = [("idle", false, 0), ("done", true, 0), ("error", true, 0),
        ("working", false, 1), ("done", false, 2), ("error", false, 3), ("blocked", false, 4)];
    for (left_state, left_seen, left_rank) in cases {
        for (right_state, right_seen, right_rank) in cases {
            let app = app();
            for (index, state, seen) in [(0, left_state, left_seen), (1, right_state, right_seen)] {
                let pane = get_pane(&app, 1, index);
                report(pane, &format!("state={state}"));
                if seen { pane.live_parser().lock().unwrap().screen_mut().program_status_mut().acknowledge(); }
            }
            let expected = if left_rank > right_rank { 0 } else { 1 };
            let summary = window_status(&app.windows[1]).unwrap();
            assert_eq!(summary.pane, 20 + expected, "{left_state}/{left_seen} vs {right_state}/{right_seen}");
            let pane = get_pane(&app, 1, expected);
            assert_eq!(summary.state, pane_format(pane, "pane_ps_state"));
            assert_eq!(u8::from(summary.seen).to_string(), pane_format(pane, "pane_ps_seen"));
        }
    }
}

#[test]
fn acknowledgement_reveals_background_work_without_erasing_a_result() {
    let mut app = app();
    report(get_pane(&app, 1, 0), "state=done");
    report(get_pane(&app, 1, 1), "state=working");
    assert_eq!(window_status(&app.windows[1]).unwrap().state, "done");
    let before = version(&app);
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert_ne!(version(&app), before);
    let summary = window_status(&app.windows[1]).unwrap();
    assert_eq!(summary.state, "working");
    assert_eq!(summary.pane, 21);
    assert!(!summary.seen);
    assert_eq!(pane_format(get_pane(&app, 1, 0), "pane_ps_state"), "done");
    assert_eq!(pane_format(get_pane(&app, 1, 0), "pane_ps_seen"), "1");
    report(get_pane(&app, 1, 1), "state=clear");
    let summary = window_status(&app.windows[1]).unwrap();
    assert_eq!(summary.state, "done");
    assert!(summary.seen);
}

#[test]
fn all_summary_changes_refresh_silent_frames_including_progress_and_seen() {
    let mut app = app();
    let mut cached_version = 0;
    let mut cached = String::new();
    crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached).unwrap();
    for body in ["state=working:progress=0", "state=working:progress=75", "state=idle", "state=done"] {
        report(get_pane(&app, 1, 0), body);
        assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached).unwrap(), "{body}");
        let frame: serde_json::Value = serde_json::from_str(&cached).unwrap();
        assert!(frame[1].get("ps_status").is_some());
        assert!(!frame[1]["activity"].as_bool().unwrap());
    }
    crate::copy_mode::switch_with_copy_save(&mut app, |app| app.active_idx = 1);
    assert!(crate::server::helpers::refresh_program_status_tabs(&app, &mut cached_version, &mut cached).unwrap());
    let frame: serde_json::Value = serde_json::from_str(&cached).unwrap();
    assert_eq!(frame[1]["ps_status"]["seen"], true);
    assert_eq!(frame[1]["ps_status"]["state"], "done");
}

#[test]
fn summaries_expose_reported_state_and_seen_without_a_separate_category() {
    let app = app();
    report(get_pane(&app, 0, 0), "state=idle:app=pi");
    report(get_pane(&app, 0, 0), "state=blocked:kind=permission:id=child");
    assert_eq!(crate::format::expand_format(
        "#{pane_ps_state}|#{pane_ps_id}|#{pane_ps_kind}|#{pane_ps_seen}|#{window_ps_state}|#{window_ps_id}|#{window_ps_kind}|#{window_ps_pane}|#{window_ps_seen}", &app),
        "blocked|child|permission|0|blocked|child|permission|%10|0");
    let json = listing(&app);
    let summary = json[0]["ps_status"].as_object().unwrap();
    let mut fields: Vec<_> = summary.keys().map(String::as_str).collect();
    fields.sort_unstable();
    assert_eq!(fields, ["app", "id", "kind", "pane", "progress", "seen", "state"]);
    assert_eq!(summary["app"], "pi");
    assert_eq!(json[0]["ps_panes"][0]["records"].as_array().unwrap().len(), 2);
}
