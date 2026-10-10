// Copy mode parity with tmux 3.4: the key tables, history-top, `copy-mode -Hu`
// under `scroll-enter-copy-mode off`, and `copy-mode` from a sourced file.
//
// Measured side by side (WSL tmux 3.4, `tmux -L cp_N -f /dev/null`, 80x24,
// `seq 1 200` in the pane) against psmux on dfb7225:
//
//   list-keys -T copy-mode-vi        tmux 87 lines        psmux nothing
//   unbind -T copy-mode-vi V; V      tmux no selection    psmux selects
//   send-keys -X history-top         tmux cursor 0,0      psmux 8,23
//   bind -n F5 copy-mode -Hu, option off, press F5
//                                    pane gets F5 (as for plain -u)
//                                    psmux typed PageUp into the pane
//   source-file on a `copy-mode`     tmux 1 copy-mode     psmux 0
//
// The tests drive the real handlers over a pane tree with no psmux server and
// no session. Registered from src/input.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::Node;

const ROWS: u16 = 24;
const COLS: u16 = 80;
const FILL: usize = 200;

/// A pane writer that keeps what was written, so a test can see exactly which
/// bytes a key sent to the pane.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl crate::pane::PaneInputSink for Captured {}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

fn make_pane(writer: Box<dyn crate::pane::PaneInputSink>) -> crate::types::Pane {
    let (master, _sink) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    });
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 2000)));
    {
        let mut parser = term.lock().unwrap();
        for i in 1..=FILL {
            parser.process(format!("line{i}\r\n").as_bytes());
        }
    }
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term,
        last_rows: ROWS,
        last_cols: COLS,
        id: 0,
        title: "pane0".to_string(),
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
        mouse_input_cache: None, win32_input_latched: false, input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(pane: crate::types::Pane) -> crate::types::Window {
    crate::types::Window {
        root: Node::Leaf(pane),
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, COLS, ROWS),
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
    }
}

/// One pane holding `FILL` lines, NOT in copy mode, writing into `out`.
fn plain_app(mode_keys: &str, out: Captured) -> AppState {
    let mut app = AppState::new("cpparity".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.mode_keys = mode_keys.to_string();
    crate::config::populate_default_bindings(&mut app);
    app.windows.push(make_window(make_pane(Box::new(out))));
    app.active_idx = 0;
    app
}

/// The same pane already in copy mode, cursor on the bottom row (where tmux
/// parks it on entry), view at the live end.
fn copy_app(mode_keys: &str) -> AppState {
    let mut app = plain_app(mode_keys, Captured::default());
    app.mode = Mode::CopyMode;
    app.copy_scroll_offset = 0;
    app.copy_pos = Some((ROWS - 1, 8));
    app
}

fn history(app: &AppState) -> usize {
    let win = &app.windows[app.active_idx];
    let p = crate::tree::active_pane(&win.root, &win.active_path).expect("active pane");
    let parser = p.term.lock().expect("parser lock");
    parser.screen().scrollback_filled()
}

/// What `list-keys` prints (the command prompt / key binding route shows it in
/// a popup).
fn list_keys(app: &mut AppState) -> Vec<String> {
    crate::commands::execute_command_string(app, "list-keys").expect("list-keys");
    match &app.mode {
        Mode::PopupMode { output, .. } => output.lines().map(|l| l.to_string()).collect(),
        _ => panic!("list-keys did not produce its popup"),
    }
}

// ══════════════ 1. the copy-mode tables are listed ══════════════

#[test]
fn list_keys_prints_the_built_in_copy_mode_vi_keys() {
    let mut app = plain_app("vi", Captured::default());
    let lines = list_keys(&mut app);
    let vi: Vec<&String> = lines.iter().filter(|l| l.starts_with("bind-key -T copy-mode-vi ")).collect();
    // tmux lists 87; psmux lists the keys its built-in copy mode handles.
    assert!(vi.len() >= 50, "copy-mode-vi lists {} lines", vi.len());
    for want in [
        "bind-key -T copy-mode-vi v send-keys -X rectangle-toggle",
        "bind-key -T copy-mode-vi g send-keys -X history-top",
        "bind-key -T copy-mode-vi C-v send-keys -X rectangle-toggle",
        "bind-key -T copy-mode-vi C-e send-keys -X scroll-down",
        "bind-key -T copy-mode-vi Escape send-keys -X clear-selection",
    ] {
        assert!(lines.iter().any(|l| l == want), "missing: {want}");
    }
}

#[test]
fn list_keys_prints_the_emacs_copy_mode_table_with_its_own_meanings() {
    let mut app = plain_app("emacs", Captured::default());
    let lines = list_keys(&mut app);
    for want in [
        "bind-key -T copy-mode C-v send-keys -X page-down",
        "bind-key -T copy-mode C-e send-keys -X end-of-line",
        "bind-key -T copy-mode M-< send-keys -X history-top",
        "bind-key -T copy-mode q send-keys -X cancel",
    ] {
        assert!(lines.iter().any(|l| l == want), "missing: {want}");
    }
    // The emacs handler ignores g/G/J/K/C-y (vi only, as in tmux), so they
    // are not listed there.
    for absent in ["g", "G", "J", "K", "C-y"] {
        let prefix = format!("bind-key -T copy-mode {absent} ");
        assert!(!lines.iter().any(|l| l.starts_with(&prefix)), "copy-mode must not list {absent}");
    }
}

#[test]
fn a_rebind_replaces_the_listed_default_and_an_unbind_removes_it() {
    let mut app = plain_app("vi", Captured::default());
    crate::config::parse_config_line(&mut app, "bind -T copy-mode-vi y send-keys -X copy-selection");
    crate::config::parse_config_line(&mut app, "unbind -T copy-mode-vi v");
    let lines = list_keys(&mut app);
    let y: Vec<&String> = lines.iter().filter(|l| l.starts_with("bind-key -T copy-mode-vi y ")).collect();
    assert_eq!(y.len(), 1, "y listed {} times: {:?}", y.len(), y);
    assert!(y[0].ends_with("send-keys -X copy-selection"), "y shows {}", y[0]);
    assert!(!lines.iter().any(|l| l.starts_with("bind-key -T copy-mode-vi v ")), "unbound v still listed");
    assert!(lines.iter().any(|l| l == "bind-key -T copy-mode-vi V send-keys -X select-line"),
        "the other defaults stay listed");
}

// ══════════════ 1b. an unbound built-in key does nothing ══════════════

#[test]
fn an_unbound_built_in_character_key_does_nothing() {
    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -T copy-mode-vi V");
    handle_copy_mode_char(&mut app, 'V').unwrap();
    assert!(app.copy_anchor.is_none(), "unbound V still began a selection (tmux: no binding, no action)");
    // A key that is still bound keeps working.
    handle_copy_mode_char(&mut app, ' ').unwrap();
    assert!(app.copy_anchor.is_some(), "Space is still bound and must select");
}

#[test]
fn an_unbound_built_in_named_key_does_nothing() {
    let mut bound = copy_app("vi");
    send_key_to_active(&mut bound, "C-b").unwrap();
    assert!(bound.copy_scroll_offset > 0, "control: C-b pages up while bound");

    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -T copy-mode-vi C-b");
    send_key_to_active(&mut app, "C-b").unwrap();
    assert_eq!(app.copy_scroll_offset, 0, "unbound C-b still paged up");
}

#[test]
fn unbind_all_in_a_copy_table_takes_every_built_in_key() {
    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -a -T copy-mode-vi");
    handle_copy_mode_char(&mut app, 'k').unwrap();
    assert_eq!(app.copy_pos, Some((ROWS - 1, 8)), "k moved the cursor after unbind -a -T copy-mode-vi");
    let mut lines = list_keys(&mut app);
    lines.retain(|l| l.starts_with("bind-key -T copy-mode-vi "));
    assert!(lines.is_empty(), "copy-mode-vi still lists {:?}", lines);
}

#[test]
fn unbind_of_the_other_table_leaves_the_active_one_alone() {
    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -T copy-mode V");
    handle_copy_mode_char(&mut app, 'V').unwrap();
    assert!(app.copy_anchor.is_some(), "unbinding copy-mode V must not touch copy-mode-vi V");
}

#[test]
fn unbind_all_without_a_table_keeps_copy_mode_usable() {
    // tmux `unbind -a` clears the prefix table only (cmd-unbind-key.c); the
    // copy-mode keys keep working.
    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -a");
    handle_copy_mode_char(&mut app, 'V').unwrap();
    assert!(app.copy_anchor.is_some(), "copy mode lost V after a plain unbind -a");
}

#[test]
fn source_file_reload_restores_unbound_built_in_keys() {
    let mut app = copy_app("vi");
    crate::config::parse_config_line(&mut app, "unbind -T copy-mode-vi V");
    crate::config::populate_default_bindings(&mut app);
    handle_copy_mode_char(&mut app, 'V').unwrap();
    assert!(app.copy_anchor.is_some(), "a re-seed must bring V back");
}

// ══════════════ 3. history-top puts the cursor at 0,0 ══════════════

#[test]
fn vi_g_parks_the_cursor_on_the_first_cell_of_history() {
    let mut app = copy_app("vi");
    handle_copy_mode_char(&mut app, 'g').unwrap();
    assert_eq!(app.copy_scroll_offset, history(&app), "g reaches the top of history");
    assert_eq!(app.copy_pos, Some((0, 0)), "tmux window_copy_cmd_history_top sets cy = cx = 0");
}

#[test]
fn emacs_m_less_than_parks_the_cursor_on_the_first_cell_of_history() {
    let mut app = copy_app("emacs");
    send_key_to_active(&mut app, "M-<").unwrap();
    assert_eq!(app.copy_scroll_offset, history(&app));
    assert_eq!(app.copy_pos, Some((0, 0)));
}

// ══════════════ 5. copy-mode -Hu under scroll-enter-copy-mode off ══════════════

#[test]
fn a_clustered_hu_root_binding_is_skipped_like_plain_u() {
    let out = Captured::default();
    let mut app = plain_app("emacs", out.clone());
    app.scroll_enter_copy_mode = false;
    crate::config::parse_config_line(&mut app, "bind -n F5 copy-mode -Hu");
    handle_key(&mut app, KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)).unwrap();
    let bytes = out.0.lock().unwrap().clone();
    assert!(matches!(app.mode, Mode::Passthrough), "no copy mode with the option off");
    assert!(
        !bytes.windows(4).any(|w| w == b"\x1b[5~"),
        "F5 reached the pane as PageUp: the binding ran instead of being skipped ({:?})",
        String::from_utf8_lossy(&bytes)
    );
    assert!(!bytes.is_empty(), "the pressed key must reach the pane");
}

// ══════════════ 6. copy-mode from a sourced file ══════════════

#[test]
fn source_file_runs_a_copy_mode_line() {
    let mut app = plain_app("emacs", Captured::default());
    let path = std::env::temp_dir().join(format!("psmux_cpparity_{}.conf", std::process::id()));
    std::fs::write(&path, "copy-mode\n").unwrap();
    crate::config::source_file(&mut app, path.to_str().unwrap());
    let _ = std::fs::remove_file(&path);
    assert!(matches!(app.mode, Mode::CopyMode), "source-file on a `copy-mode` line must enter copy mode");
}
