// `select-pane -d` / `select-pane -e`: tmux's per-pane PANE_INPUTOFF.
//
// psmux parsed both flags into "disable-input" / "enable-input" and the server
// ignored them, while `#{pane_input_off}` was hard coded to 0, so a script
// that fenced a pane off (`select-pane -d -t %N`) went on typing into it, and
// the `-t` permanently selected that pane on the way.
//
// tmux 3.5a, the behaviour pinned here:
//   cmd-select-pane.c   -d sets / -e clears the flag on the target (or, with
//                       -l, the last) pane and returns: no selection, no zoom
//                       change, no after-select-pane hook.
//   window.c            window_pane_key drops a key for an input-off pane
//                       (sync copies included) after a pane MODE has had it;
//                       window_pane_copy_key skips an input-off sibling.
//   cmd-paste-buffer.c  writes nothing to an input-off pane, -d still deletes.
//   format.c            #{pane_input_off} is the flag.
//
// The tests drive the real handlers over a pane tree with no psmux server and
// no session.  Registered from src/server/mod.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::server::connection::select_pane_input_toggle;
use crate::types::{Node, TempTarget};

/// A pane writer that keeps what was written.
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

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

fn make_pane(id: usize, out: Captured) -> crate::types::Pane {
    let (master, _sink) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer: Box::new(out),
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(vt100::Parser::new(10, 40, 100))),
        last_rows: 10,
        last_cols: 40,
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

fn make_window(id: usize, panes: Vec<crate::types::Pane>) -> crate::types::Window {
    let n = panes.len();
    crate::types::Window {
        root: Node::Split {
            kind: LayoutKind::Vertical,
            sizes: vec![100 / n as u16; n],
            children: panes.into_iter().map(Node::Leaf).collect(),
        },
        active_path: vec![0],
        name: format!("w{id}"),
        id,
        area: Rect::new(0, 0, 40, 10),
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

/// Window 0 (@10) holds %1 %2 %3, window 1 (@11) holds %4 %5.  Active:
/// window 0, pane %1; window 1's active pane is %4.  `outs[id]` is what pane
/// %id was sent.
struct Fixture {
    app: AppState,
    outs: std::collections::HashMap<usize, Captured>,
}

fn fixture() -> Fixture {
    let mut outs = std::collections::HashMap::new();
    let mut pane = |id: usize| {
        let c = Captured::default();
        outs.insert(id, c.clone());
        make_pane(id, c)
    };
    let w0 = make_window(10, vec![pane(1), pane(2), pane(3)]);
    let w1 = make_window(11, vec![pane(4), pane(5)]);
    let mut app = AppState::new("inputoff".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    crate::config::populate_default_bindings(&mut app);
    app.windows.push(w0);
    app.windows.push(w1);
    app.active_idx = 0;
    Fixture { app, outs }
}

impl Fixture {
    fn sent(&self, id: usize) -> String {
        self.outs[&id].text()
    }
    fn input_off(&self, id: usize) -> bool {
        let mut found = None;
        for w in &self.app.windows {
            if let Some(path) = crate::tree::find_path_by_id(&w.root, id) {
                found = crate::tree::active_pane(&w.root, &path).map(|p| p.input_off);
            }
        }
        found.expect("pane exists")
    }
    fn set(&mut self, id: usize, off: bool) {
        crate::tree::find_pane_mut_by_id_global(&mut self.app, id).expect("pane").input_off = off;
    }
    fn focus(&self) -> (usize, usize, Vec<usize>) {
        let w = &self.app.windows[self.app.active_idx];
        (
            self.app.active_idx,
            crate::tree::get_active_pane_id(&w.root, &w.active_path).unwrap(),
            self.app.windows[1].active_path.clone(),
        )
    }
    fn fmt(&self, id: usize) -> String {
        crate::format::expand_format_for_pane_by_id("#{pane_input_off}", &self.app, id)
    }
}

fn pane_id(id: usize) -> TempTarget {
    TempTarget { pane: Some(id), pane_is_id: true, ..Default::default() }
}

#[test]
fn the_flags_parse_like_cmd_select_pane() {
    // (off, last)
    assert_eq!(select_pane_input_toggle(&["-d"], None), Some((true, false)));
    assert_eq!(select_pane_input_toggle(&["-e"], Some("%2")), Some((false, false)));
    // -e is tested first in tmux, so it wins.
    assert_eq!(select_pane_input_toggle(&["-d", "-e"], None), Some((false, false)));
    // -l moves the toggle to the last pane.
    assert_eq!(select_pane_input_toggle(&["-l", "-d"], None), Some((true, true)));
    assert_eq!(select_pane_input_toggle(&["-l", "-e"], None), Some((false, true)));
    // Not a toggle: no flag, plain -l, -m/-M (tmux returns before -e/-d), and
    // the navigation forms psmux keeps running as navigation.
    assert_eq!(select_pane_input_toggle(&[], None), None);
    assert_eq!(select_pane_input_toggle(&["-l"], None), None);
    assert_eq!(select_pane_input_toggle(&["-m", "-d"], None), None);
    assert_eq!(select_pane_input_toggle(&["-U", "-d"], None), None);
    assert_eq!(select_pane_input_toggle(&["-d"], Some(":.+")), None);
}

#[test]
fn disable_and_enable_flip_the_target_pane_and_select_nothing() {
    let mut f = fixture();
    let before = f.focus();
    let last_before = f.app.last_pane_path.clone();
    let mru_before = f.app.windows[0].pane_mru.clone();

    set_pane_input_off(&mut f.app, Some(&pane_id(2)), true, false).unwrap();
    assert!(f.input_off(2));
    assert!(!f.input_off(1), "the active pane is not the target");
    assert_eq!(f.focus(), before, "-d must not select the pane");
    assert_eq!(f.app.last_pane_path, last_before);
    assert_eq!(f.app.windows[0].pane_mru, mru_before);
    assert_eq!(f.fmt(2), "1");
    assert_eq!(f.fmt(1), "0");

    // A pane in ANOTHER window: neither the active window nor that window's
    // own active pane may move (the temporary -t focus would have moved the
    // latter, which is why the request carries its target).
    set_pane_input_off(&mut f.app, Some(&pane_id(5)), true, false).unwrap();
    assert!(f.input_off(5));
    assert_eq!(f.focus(), before);
    assert_eq!(f.fmt(5), "1");

    set_pane_input_off(&mut f.app, Some(&pane_id(2)), false, false).unwrap();
    assert!(!f.input_off(2));
    assert_eq!(f.fmt(2), "0");
    assert!(f.input_off(5), "-e clears only its own target");
    assert_eq!(f.focus(), before);
}

#[test]
fn no_target_means_the_active_pane_and_a_window_target_its_active_pane() {
    let mut f = fixture();
    set_pane_input_off(&mut f.app, None, true, false).unwrap();
    assert!(f.input_off(1));
    let w1 = TempTarget { win: Some(1), ..Default::default() };
    set_pane_input_off(&mut f.app, Some(&w1), true, false).unwrap();
    assert!(f.input_off(4));
    assert!(!f.input_off(5));
    assert_eq!(f.focus(), (0, 1, vec![0]));
}

#[test]
fn last_flag_toggles_the_last_pane() {
    let mut f = fixture();
    // Three panes, never switched: tmux's "no last pane".
    assert_eq!(set_pane_input_off(&mut f.app, None, true, true).unwrap_err(), "no last pane");
    f.app.last_pane_path = vec![2];
    set_pane_input_off(&mut f.app, None, true, true).unwrap();
    assert!(f.input_off(3));
    assert!(!f.input_off(1));
    assert_eq!(f.focus(), (0, 1, vec![0]));
    assert_eq!(f.app.last_pane_path, vec![2], "the last pane stays the last pane");
    // Another window has only tmux's two-pane fallback: %4 active, so %5.
    let w1 = TempTarget { win: Some(1), ..Default::default() };
    set_pane_input_off(&mut f.app, Some(&w1), true, true).unwrap();
    assert!(f.input_off(5));
}

#[test]
fn a_missing_target_is_tmux_error_and_changes_nothing() {
    let mut f = fixture();
    assert_eq!(set_pane_input_off(&mut f.app, Some(&pane_id(99)), true, false).unwrap_err(), "can't find pane: %99");
    assert!((1..=5).all(|id| !f.input_off(id)));
}

#[test]
fn the_in_server_command_route_targets_without_selecting() {
    // Key bindings, hooks, menus and the command prompt.
    let mut f = fixture();
    crate::commands::execute_command_string(&mut f.app, "select-pane -d -t %3").unwrap();
    assert!(f.input_off(3));
    assert_eq!(f.focus(), (0, 1, vec![0]));
    crate::commands::execute_command_string(&mut f.app, "select-pane -t %3 -e").unwrap();
    assert!(!f.input_off(3));
    crate::commands::execute_command_string(&mut f.app, "select-pane -d").unwrap();
    assert!(f.input_off(1));
    assert_eq!(f.focus(), (0, 1, vec![0]));
}

#[test]
fn keys_text_bytes_and_pastes_write_nothing_to_an_input_off_pane() {
    let mut f = fixture();
    f.set(1, true);
    send_text_to_active(&mut f.app, "echo typed\r").unwrap();
    send_key_to_active(&mut f.app, "enter").unwrap();
    send_key_to_active(&mut f.app, "C-c").unwrap();
    send_key_to_active(&mut f.app, "M-a").unwrap();
    // `send-keys -H` with a byte run that is not UTF-8 (its own write path).
    send_bytes_to_active(&mut f.app, &[0xff, 0x41]).unwrap();
    send_paste_to_active(&mut f.app, "pasted\n").unwrap();
    crate::input::send_paste_buffer_to_active(&mut f.app, "pasted").unwrap();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    crate::input::forward_key_to_active(&mut f.app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)).unwrap();
    crate::input::forward_key_to_active(&mut f.app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
    assert_eq!(f.sent(1), "", "an input-off pane took input");
    assert!(f.app.windows[0].root_pane_ids_for_test().iter().all(|id| f.sent(*id).is_empty()));

    // -e and the same text arrives.
    f.set(1, false);
    send_text_to_active(&mut f.app, "echo typed\r").unwrap();
    assert_eq!(f.sent(1), "echo typed\r");
}

#[test]
fn the_interactive_route_signals_are_not_stamped_on_an_input_off_pane() {
    let mut f = fixture();
    f.set(1, true);
    crate::input::stamp_interactive_text(&mut f.app);
    crate::input::stamp_interactive_key(&mut f.app, "Enter");
    let w = &f.app.windows[0];
    let p = crate::tree::active_pane(&w.root, &w.active_path).unwrap();
    assert!(p.last_text_input.is_none() && p.last_special_key.is_none());
}

#[test]
fn paste_buffer_writes_nothing_but_d_still_deletes_the_buffer() {
    let mut f = fixture();
    f.set(1, true);
    f.app.paste_buffers = vec!["from the buffer\n".to_string(), "older".to_string()];
    let pb = crate::commands::PasteBufferArgs { delete: true, ..Default::default() };
    assert_eq!(crate::commands::run_paste_buffer(&mut f.app, &pb).unwrap(), None, "not an error in tmux");
    assert_eq!(f.sent(1), "");
    assert_eq!(f.app.paste_buffers, vec!["older".to_string()], "-d deletes the buffer anyway");

    // `-t` an input-off pane that is not the active one; the active pane is
    // not written to either.
    f.set(1, false);
    f.set(2, true);
    let pb = crate::commands::PasteBufferArgs { target: Some("%2".to_string()), ..Default::default() };
    crate::commands::run_paste_buffer(&mut f.app, &pb).unwrap();
    assert_eq!(f.sent(2), "");
    assert_eq!(f.sent(1), "");
    // ...and to an input-on pane it still pastes.
    let pb = crate::commands::PasteBufferArgs { target: Some("%3".to_string()), ..Default::default() };
    crate::commands::run_paste_buffer(&mut f.app, &pb).unwrap();
    assert_eq!(f.sent(3), "older");
}

#[test]
fn a_pane_mode_still_takes_keys_while_input_is_off() {
    let mut f = fixture();
    f.app.mode_keys = "vi".to_string();
    f.set(1, true);
    crate::copy_mode::enter_copy_mode(&mut f.app);
    assert!(f.app.mode.in_copy());
    // A copy mode key: opens the search prompt and types into it.
    send_text_to_active(&mut f.app, "/abc").unwrap();
    match &f.app.mode {
        Mode::CopySearch { input, .. } => assert_eq!(input, "abc"),
        _ => panic!("the copy mode key did not reach the mode"),
    }
    send_key_to_active(&mut f.app, "escape").unwrap();
    assert!(matches!(f.app.mode, Mode::CopyMode));
    // `send-keys -X` is a mode command too.
    run_send_keys_x(&mut f.app, "cancel", 1).unwrap();
    assert!(!f.app.mode.in_copy());
    assert_eq!(f.sent(1), "", "nothing leaked to the program");
}

#[test]
fn synchronize_panes_skips_an_input_off_sibling_and_reaches_the_others() {
    let mut f = fixture();
    f.app.sync_input = true;
    f.set(2, true);
    send_text_to_active(&mut f.app, "t").unwrap();
    send_key_to_active(&mut f.app, "enter").unwrap();
    send_bytes_to_active(&mut f.app, &[0xff]).unwrap();
    send_paste_to_active(&mut f.app, "p").unwrap();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    crate::input::forward_key_to_active(&mut f.app, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)).unwrap();
    assert_eq!(f.sent(2), "", "window_pane_copy_key skips an input-off pane");
    assert!(!f.sent(1).is_empty());
    assert_eq!(f.sent(1), f.sent(3), "every input-on pane gets the same input");
    assert!(f.sent(1).starts_with("t\r"));
    assert!(f.sent(1).ends_with("pk"));
    assert_eq!(f.sent(4), "", "sync is per window");
}

#[test]
fn synchronize_panes_from_an_input_off_pane_reaches_nobody() {
    // window_pane_key returns for the input-off pane BEFORE window_pane_copy_key.
    let mut f = fixture();
    f.app.sync_input = true;
    f.set(1, true);
    send_text_to_active(&mut f.app, "t").unwrap();
    send_key_to_active(&mut f.app, "enter").unwrap();
    send_paste_to_active(&mut f.app, "p").unwrap();
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    crate::input::forward_key_to_active(&mut f.app, KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE)).unwrap();
    for id in 1..=3 {
        assert_eq!(f.sent(id), "", "pane %{id}");
    }
}

#[test]
fn a_mouse_event_for_the_program_is_dropped() {
    let mut f = fixture();
    let pane = crate::tree::find_pane_mut_by_id_global(&mut f.app, 1).unwrap();
    pane.input_off = true;
    crate::window_ops::inject_mouse_combined(pane, 3, 2, 0, true, 0, 0, "w10");
    assert_eq!(f.sent(1), "");
    let pane = crate::tree::find_pane_mut_by_id_global(&mut f.app, 1).unwrap();
    pane.input_off = false;
    crate::window_ops::inject_mouse_combined(pane, 3, 2, 0, true, 0, 0, "w10");
    assert_eq!(f.sent(1), "\x1b[<0;4;3M", "the same click reaches an input-on pane");
}

trait PaneIds {
    fn root_pane_ids_for_test(&self) -> Vec<usize>;
}

impl PaneIds for crate::types::Window {
    fn root_pane_ids_for_test(&self) -> Vec<usize> {
        crate::tree::pane_paths(&self.root)
            .iter()
            .filter_map(|p| crate::tree::get_active_pane_id(&self.root, p))
            .collect()
    }
}
