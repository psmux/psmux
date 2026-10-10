//! Issue #708: `respawn-pane` threw the pane's history away and dropped `-e`.
//!
//! Measured on master 39ac085 (5 loops, 120x30 pane, isolated `-L`):
//!   history kept after respawn-pane of a dead pane      0/5
//!   `respawn-pane -k -e OPENRIG_TEST=hello -- <echo>`   0/5 (child printed `ENV=[] []`)
//!   `new-window -e` with the same helper (control)      5/5
//!
//! tmux 3.4 in WSL, same scenario: history_size 33 before and 33 after the
//! respawn, the marker still in `capture-pane -S -3000`, the rows that were
//! visible at death gone, the cursor at 0,0, copy mode and the alternate
//! screen left, and `-e` seen by that one process.
//!
//! tmux source: spawn.c `spawn_pane` with SPAWN_RESPAWN keeps the
//! `window_pane`, runs `window_pane_reset_mode_all` and `screen_reinit`
//! (screen.c), whose `grid_clear_lines(gd, gd->hsize, gd->sy)` clears the
//! visible rows and nothing above them. The child's environment is
//! `environ_for_session` followed by `environ_copy(sc->environ, child)`, which
//! is where cmd-respawn-pane.c puts every `-e`.
//!
//! psmux gave the pane a brand new `vt100::Parser` on every respawn, and the
//! respawn request had no field for `-e`, so the values the parser collected
//! never left `connection.rs`.
//!
//! End to end coverage: `tests/test_issue708_respawn_history_env.ps1`.
//! Registered from `src/window_ops.rs`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Mode, Node};
use ratatui::layout::Rect;

const ROWS: u16 = 10;
const COLS: u16 = 40;

fn make_pane(id: usize, term: vt100::Parser) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: ROWS, cols: COLS, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(term)),
        last_rows: ROWS,
        last_cols: COLS,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: true,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false, input_off: false,
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

fn app_with_pane(term: vt100::Parser) -> AppState {
    let mut app = AppState::new("i708".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.history_limit = 2000;
    app.last_window_area = Rect { x: 0, y: 0, width: COLS, height: ROWS };
    app.windows.push(crate::types::Window {
        root: Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![100],
            children: vec![Node::Leaf(make_pane(708, term))],
        },
        active_path: vec![0],
        name: "w".into(),
        id: 0,
        area: Rect::new(0, 0, COLS, ROWS),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app.active_idx = 0;
    app
}

/// A dead process's screen: MARKER, then enough lines to push it into
/// history, so rows `fill13..fill20` are the visible ones at death.
fn dead_screen() -> vt100::Parser {
    let mut p = vt100::Parser::new(ROWS, COLS, 2000);
    p.process(b"MARKER\r\n");
    for n in 1..=20 {
        p.process(format!("fill{n}\r\n").as_bytes());
    }
    p
}

fn all_text(p: &vt100::Parser) -> String {
    let mut s = p.screen().clone();
    let hist = s.scrollback_filled();
    let mut out = String::new();
    // Walk from the top of history down to the live screen.
    for off in (0..=hist).rev() {
        s.set_scrollback(off);
        let first = s.rows(0, COLS).next().unwrap_or_default();
        out.push_str(first.trim_end());
        out.push('\n');
    }
    s.set_scrollback(0);
    for row in s.rows(0, COLS).skip(1) {
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

fn active_term(app: &AppState) -> Arc<Mutex<vt100::Parser>> {
    let win = &app.windows[app.active_idx];
    crate::tree::active_pane(&win.root, &win.active_path).expect("pane").term.clone()
}

// ─────────────────────────────────────────────────────────────────────────
// 1. The vt100 primitive: tmux screen_reinit.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn reinit_keeps_history_and_clears_only_the_visible_rows() {
    let mut p = dead_screen();
    let hist_before = p.screen().scrollback_filled();
    assert!(hist_before > 0, "setup: MARKER must be in history");
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().scrollback_filled(), hist_before, "history_size must survive (tmux: 33 -> 33)");
    let text = all_text(&p);
    assert!(text.lines().next() == Some("MARKER"), "oldest history row must still be MARKER:\n{text}");
    assert!(!text.contains("fill20"), "rows visible at death are cleared, not pushed to history (grid_clear_lines):\n{text}");
    assert!(p.screen().contents().trim().is_empty(), "visible screen must be blank");
    assert_eq!(p.screen().cursor_position(), (0, 0), "cursor homed");
}

#[test]
fn reinit_resets_modes_alternate_screen_and_title() {
    let mut p = dead_screen();
    // application cursor, bracketed paste, mouse, hidden cursor, title, alt screen
    p.process(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b[?25l\x1b]2;old title\x07\x1b[?1049h");
    assert!(p.screen().alternate_screen());
    p.screen_mut().reinit_keep_history();
    let s = p.screen();
    assert!(!s.alternate_screen(), "tmux screen_reinit leaves the alternate screen");
    assert!(!s.application_cursor());
    assert!(!s.bracketed_paste());
    assert!(!s.hide_cursor());
    assert_eq!(s.mouse_protocol_mode(), vt100::MouseProtocolMode::None);
    assert_eq!(s.title(), "", "tmux screen_free_titles");
    assert!(all_text(&p).starts_with("MARKER\n"), "main grid history survives a death on the alt screen");
}

#[test]
fn reinit_keeps_the_pane_options_and_the_history_limit() {
    let mut p = vt100::Parser::new(ROWS, COLS, 5);
    p.screen_mut().set_allow_alternate_screen(false);
    for n in 0..30 {
        p.process(format!("l{n}\r\n").as_bytes());
    }
    p.screen_mut().set_scrollback(3);
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().scrollback_len(), 5);
    assert_eq!(p.screen().scrollback_filled(), 5);
    assert_eq!(p.screen().scrollback(), 0, "view back at the live bottom");
    assert!(!p.screen().allow_alternate_screen(), "alternate-screen off is a pane option, not process state");
    // new output keeps flowing into the same bounded history
    for n in 0..30 {
        p.process(format!("m{n}\r\n").as_bytes());
    }
    assert_eq!(p.screen().scrollback_filled(), 5);
}

#[test]
fn reinit_keeps_hyperlinks_that_history_cells_point_into() {
    let mut p = vt100::Parser::new(ROWS, COLS, 100);
    p.process(b"\x1b]8;;https://example.com/a\x1b\\LINK\x1b]8;;\x1b\\\r\n");
    for n in 0..15 {
        p.process(format!("x{n}\r\n").as_bytes());
    }
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().hyperlink_uri(1), Some("https://example.com/a"));
    // a new link after the respawn does not reuse id 1
    p.process(b"\x1b]8;;https://example.com/b\x1b\\B\x1b]8;;\x1b\\");
    assert_eq!(p.screen().hyperlink_uri(1), Some("https://example.com/a"));
    assert_eq!(p.screen().hyperlink_uri(2), Some("https://example.com/b"));
}

/// tmux keeps `wp->palette` across a respawn; only RIS, OSC 104 and
/// `send-keys -R` clear it (input.c, cmd-send-keys.c).
#[test]
fn reinit_keeps_the_osc4_palette_like_tmux() {
    let mut p = dead_screen();
    p.process(b"]4;4;rgb:00/00/80\\");
    let gen = p.screen().palette_generation();
    assert_eq!(p.screen().palette_entry(4), Some((0, 0, 0x80)));
    p.screen_mut().reinit_keep_history();
    assert_eq!(p.screen().palette_entry(4), Some((0, 0, 0x80)));
    assert_eq!(p.screen().palette_generation(), gen);
    // RIS in the new process still clears it
    p.process(b"c");
    assert_eq!(p.screen().palette_entry(4), None);
}

// ─────────────────────────────────────────────────────────────────────────
// 2. The pane side: the history moves to the new parser, the old Arc is left
//    empty for the dead reader thread.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn history_moves_out_of_the_old_parser() {
    let old = Arc::new(Mutex::new(dead_screen()));
    let fresh = crate::window_ops::reinit_parser_keep_history(&old, ROWS, COLS, 2000, true);
    assert!(all_text(&fresh).starts_with("MARKER\n"));
    // The dead process's reader may still flush into `old`; it must not be the
    // parser holding the history now.
    let left = old.lock().unwrap();
    assert_eq!(left.screen().scrollback_filled(), 0);
}

#[test]
fn a_pane_resized_while_dead_starts_at_its_new_size() {
    let old = Arc::new(Mutex::new(dead_screen()));
    let fresh = crate::window_ops::reinit_parser_keep_history(&old, 7, 30, 2000, true);
    assert_eq!(fresh.screen().size(), (7, 30));
    assert!(all_text(&fresh).contains("MARKER"));
}

#[test]
fn a_poisoned_parser_falls_back_to_an_empty_screen() {
    let old = Arc::new(Mutex::new(dead_screen()));
    let o2 = old.clone();
    let _ = std::thread::spawn(move || {
        let _g = o2.lock().unwrap();
        panic!("poison");
    })
    .join();
    let fresh = crate::window_ops::reinit_parser_keep_history(&old, ROWS, COLS, 2000, true);
    assert_eq!(fresh.screen().size(), (ROWS, COLS));
    assert_eq!(fresh.screen().scrollback_filled(), 0);
}

/// `respawn-pane -E` (no process) goes through the same spawn_pane /
/// screen_reinit in tmux, so it keeps the history too. -E needs no real
/// shell, so this drives `respawn_active_pane` itself.
#[test]
fn respawn_active_pane_keeps_history() {
    let mut app = app_with_pane(dead_screen());
    crate::window_ops::respawn_active_pane(&mut app, None, None, false, None, true, &[])
        .expect("respawn of a dead pane");
    let term = active_term(&app);
    let p = term.lock().unwrap();
    assert!(all_text(&p).starts_with("MARKER\n"), "respawn discarded the history:\n{}", all_text(&p));
    assert!(p.screen().contents().trim().is_empty(), "visible rows cleared");
}

/// tmux window_pane_reset_mode_all: a pane in copy mode leaves it on respawn,
/// and the history kept is the LIVE one, not the copy-mode snapshot.
#[test]
fn respawn_leaves_copy_mode_and_keeps_the_live_history() {
    let mut app = app_with_pane(dead_screen());
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::copy_mode::sync_copy_snapshot(&mut app);
    assert!(matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }));
    {
        let win = &app.windows[0];
        let pane = crate::tree::active_pane(&win.root, &win.active_path).unwrap();
        assert!(pane.live_term.is_some(), "setup: copy mode shows a snapshot");
    }
    crate::window_ops::respawn_active_pane(&mut app, None, None, false, None, true, &[])
        .expect("respawn from copy mode");
    assert!(!matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }), "copy mode must be left (tmux in_mode 1 -> 0)");
    let win = &app.windows[0];
    let pane = crate::tree::active_pane(&win.root, &win.active_path).unwrap();
    assert!(pane.live_term.is_none());
    assert!(pane.copy_state.is_none());
    assert!(all_text(&pane.term.lock().unwrap()).starts_with("MARKER\n"));
}

// ─────────────────────────────────────────────────────────────────────────
// 3. -e: parsed, carried, applied.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn env_flags_are_collected_in_order_like_tmux_environ_put() {
    let args = ["-k", "-e", "OPENRIG_TEST=hello", "-c", "C:\\x", "-e", "\"SECOND=a=b\"", "-e", "NOEQUALS", "--", "cmd"];
    let got = crate::server::connection::env_flag_values(&args);
    assert_eq!(
        got,
        vec![
            ("OPENRIG_TEST".to_string(), "hello".to_string()),
            ("SECOND".to_string(), "a=b".to_string()),
        ],
        "every -e KEY=VALUE, value may contain '=', entries without '=' are ignored (environ_put)"
    );
}

#[test]
fn the_respawn_requests_carry_the_env() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let env = vec![("K".to_string(), "V".to_string())];
    match crate::types::CtrlReq::RespawnPane(None, true, None, false, tx, env.clone()) {
        crate::types::CtrlReq::RespawnPane(_, _, _, _, _, e) => assert_eq!(e, env),
        _ => unreachable!(),
    }
    let (tx, _rx) = std::sync::mpsc::channel();
    match crate::types::CtrlReq::RespawnWindow(None, None, tx, env.clone()) {
        crate::types::CtrlReq::RespawnWindow(_, _, _, e) => assert_eq!(e, env),
        _ => unreachable!(),
    }
}

/// Every place that turns a respawn request into `respawn_active_pane` must
/// hand it the request's env (the gap was one dropped field, so pin the wiring).
#[test]
fn every_respawn_dispatch_forwards_the_env() {
    let conn = include_str!("../src/server/connection.rs");
    let sends: Vec<&str> = conn.lines().filter(|l| l.contains("CtrlReq::RespawnPane(") || l.contains("CtrlReq::RespawnWindow(")).collect();
    assert!(sends.len() >= 3, "respawn-pane (CLI + control mode) and respawn-window sends: {sends:?}");
    for l in &sends {
        assert!(l.contains("env_sets"), "a respawn request is sent without its -e values: {l}");
    }
    let server = include_str!("../src/server/mod.rs");
    for arm in ["CtrlReq::RespawnPane(", "CtrlReq::RespawnWindow("] {
        let at = server.find(arm).expect("arm");
        let window = &server[at..(at + 1500).min(server.len())];
        assert!(window.contains("&env_sets)"), "{arm} arm must pass env_sets to respawn_active_pane");
    }
}

/// The real thing: a respawned child process sees `-e` and it overrides the
/// session environment. Spawns cmd.exe under a ConPTY (no psmux server).
#[test]
fn respawned_child_sees_env_and_it_overrides_the_session() {
    let mut app = app_with_pane(dead_screen());
    app.environment.insert("I708_OVR".to_string(), "fromsession".to_string());
    let env = vec![
        ("I708_A".to_string(), "hello".to_string()),
        ("I708_OVR".to_string(), "fromflag".to_string()),
    ];
    // A script file keeps the probe independent of how a command string is
    // routed through a shell; the E2E suite covers the string forms.
    let script = std::env::temp_dir().join(format!("psmux_i708_{}.cmd", std::process::id()));
    std::fs::write(&script, "@echo off\r\necho ENV=[%I708_A%][%I708_OVR%]\r\n").expect("write probe");
    let script_s = script.to_string_lossy().to_string();
    crate::window_ops::respawn_active_pane(
        &mut app,
        None,
        None,
        false,
        Some(&script_s),
        false,
        &env,
    )
    .expect("respawn with a command");
    let term = active_term(&app);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut text = String::new();
    while Instant::now() < deadline {
        text = all_text(&term.lock().unwrap());
        if text.contains("ENV=[") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    {
        let win = &mut app.windows[0];
        if let Some(p) = crate::window_ops::active_pane_mut(&mut win.root, &win.active_path) {
            let _ = p.child.kill();
        }
    }
    let _ = std::fs::remove_file(&script);
    assert!(text.contains("ENV=[hello][fromflag]"), "child environment wrong:\n{text}");
    assert!(text.starts_with("MARKER\n"), "history kept under the new process:\n{text}");
}

// ─────────────────────────────────────────────────────────────────────────
// 4. One row screens. Found while fixing #708: the first cut left a 1x1
//    placeholder parser behind for the dead reader thread, and a wrapped
//    line on a one row screen panicked in `Grid::col_wrap`
//    (`prev_pos.row - scrolled` underflows), which killed the server. psmux
//    allows one row panes (#644, MIN_PTY_DIM 1), so the grid must survive it.
// ─────────────────────────────────────────────────────────────────────────

fn wrap_on(rows: u16, cols: u16, scrollback: usize) -> vt100::Parser {
    let mut p = vt100::Parser::new(rows, cols, scrollback);
    p.process(b"abcdefghijklmnopqrstuvwxyz0123456789");
    p
}

#[test]
fn one_row_with_scrollback_wraps_into_history() {
    let p = wrap_on(1, 10, 100);
    assert_eq!(p.screen().contents(), "456789", "36 chars over 10 columns: the 4th row is the visible one");
    assert_eq!(p.screen().scrollback_filled(), 3);
}

#[test]
fn one_row_without_scrollback_wraps() {
    let p = wrap_on(1, 10, 0);
    assert_eq!(p.screen().contents(), "456789", "36 chars over 10 columns: the 4th row is the visible one");
}

#[test]
fn one_by_one_screen_survives_any_output() {
    let mut p = vt100::Parser::new(1, 1, 0);
    p.process("hello\r\nworld\r\n\x1b[31mcolour\x1b[0m wide \u{4e2d} end".as_bytes());
    let _ = p.screen().contents();
}

#[test]
fn two_rows_still_mark_the_wrap() {
    let p = wrap_on(2, 10, 100);
    assert!(p.screen().row_wrapped(0), "the row above the last one wrapped into it");
}

/// The parser left behind in the old Arc takes whatever the dead process's
/// reader flushes last, at the pane's own size, without panicking.
#[test]
fn the_parser_left_for_the_old_reader_takes_late_output() {
    let old = Arc::new(Mutex::new(dead_screen()));
    let _fresh = crate::window_ops::reinit_parser_keep_history(&old, ROWS, COLS, 2000, true);
    let mut left = old.lock().unwrap();
    assert_eq!(left.screen().size(), (ROWS, COLS));
    left.process(&[b'x'; 500]);
    left.process(b"\r\nlate\r\n");
}
