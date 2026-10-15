// `send-keys -X` parity with tmux 3.4 (cmd-send-keys.c, window-copy.c).
//
// Measured side by side (WSL tmux 3.4, `tmux -L cp_N -f /dev/null`, 80x24,
// `seq 1 200` in the pane) against psmux on dfb7225:
//
//   send-keys -X history-top, pane in no mode
//       tmux   "not in a mode", exit 1, pane_in_mode 0
//       psmux  exit 0, and the pane ENTERED copy mode
//   send-keys -X -N 40 scroll-up
//       tmux   scroll_position 40
//       psmux  scroll_position 1
//
// tmux hands -N to the command as `wme->prefix`; only the commands that loop
// over it repeat (history-top, rectangle-toggle and the like run once).
//
// Also covered: a copy-mode key binding written with -N, and the
// scroll-enter-copy-mode test for a `copy-mode` binding that pages up, which
// used to be a search for the text `-u`.
//
// No psmux server and no session: the tests call the server's command
// function on a pane tree. Registered from src/server/mod.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{Node, SelectionMode};

const ROWS: u16 = 24;
const COLS: u16 = 80;

fn make_pane() -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows: ROWS,
        cols: COLS,
        pixel_width: 0,
        pixel_height: 0,
    });
    let term = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 2000)));
    {
        let mut parser = term.lock().unwrap();
        for i in 1..=200 {
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

fn app(in_copy: bool) -> AppState {
    let mut app = AppState::new("skxparity".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.push(crate::types::Window {
        root: Node::Leaf(make_pane()),
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
    });
    app.active_idx = 0;
    if in_copy {
        app.mode = Mode::CopyMode;
        app.copy_pos = Some((ROWS - 1, 8));
    }
    app
}

// ══════════════ 2. -X outside a mode ══════════════

#[test]
fn send_keys_x_outside_copy_mode_is_not_in_a_mode() {
    let mut a = app(false);
    let r = run_send_keys_x(&mut a, "history-top", 1);
    assert_eq!(r, Err("not in a mode".to_string()));
    assert!(matches!(a.mode, Mode::Passthrough), "tmux never enters a mode for -X");
    assert_eq!(a.copy_scroll_offset, 0);
}

#[test]
fn send_keys_x_in_copy_mode_runs() {
    let mut a = app(true);
    assert_eq!(run_send_keys_x(&mut a, "history-top", 1), Ok(()));
    assert!(a.copy_scroll_offset > 0);
    assert_eq!(a.copy_pos, Some((0, 0)), "history-top: cy = cx = 0");
}

#[test]
fn cancel_through_send_keys_x_leaves_copy_mode_and_a_second_is_refused() {
    let mut a = app(true);
    assert_eq!(run_send_keys_x(&mut a, "cancel", 1), Ok(()));
    assert!(matches!(a.mode, Mode::Passthrough));
    assert_eq!(run_send_keys_x(&mut a, "cancel", 1), Err("not in a mode".to_string()));
}

// ══════════════ 4. the -N count ══════════════

#[test]
fn a_count_repeats_scroll_up() {
    let mut a = app(true);
    run_send_keys_x(&mut a, "scroll-up", 40).unwrap();
    assert_eq!(a.copy_scroll_offset, 40, "-N 40 scroll-up scrolls 40 lines");
    run_send_keys_x(&mut a, "scroll-up", 5).unwrap();
    assert_eq!(a.copy_scroll_offset, 45);
}

#[test]
fn a_count_repeats_cursor_motion() {
    let mut a = app(true);
    run_send_keys_x(&mut a, "cursor-up", 3).unwrap();
    assert_eq!(a.copy_pos.map(|p| p.0), Some(ROWS - 4));
}

#[test]
fn commands_without_a_count_run_once() {
    // window_copy_cmd_rectangle_toggle has no repeat loop: -N 3 toggles once.
    let mut a = app(true);
    run_send_keys_x(&mut a, "rectangle-toggle", 3).unwrap();
    assert_eq!(a.copy_selection_mode, SelectionMode::Rect);
}

#[test]
fn a_count_stops_when_the_command_leaves_copy_mode() {
    let mut a = app(true);
    assert_eq!(run_send_keys_x(&mut a, "cancel", 5), Ok(()));
    assert!(matches!(a.mode, Mode::Passthrough));
}

#[test]
fn a_copy_mode_binding_keeps_its_count() {
    // tmux's own copy-mode-vi WheelUpPane is `send-keys -X -N 5 scroll-up`.
    let mut a = app(true);
    let (tx, rx) = std::sync::mpsc::channel();
    a.control_tx = Some(tx);
    let action = crate::types::Action::Command("send-keys -X -N 5 scroll-up".to_string());
    assert!(crate::input::run_copy_mode_binding(&mut a, &action));
    match rx.try_recv() {
        Ok(CtrlReq::SendKeysXRun { cmd, count, .. }) => {
            assert_eq!(cmd, "scroll-up");
            assert_eq!(count, 5);
        }
        _ => panic!("the binding's -N count was dropped"),
    }
}

#[test]
fn the_d_key_name_is_one_send_keys_x_runs() {
    // list-keys shows D as tmux's copy-pipe-end-of-line-and-cancel, so that
    // name has to do what D does.
    let mut a = app(true);
    run_send_keys_x(&mut a, "copy-pipe-end-of-line-and-cancel", 1).unwrap();
    assert!(matches!(a.mode, Mode::Passthrough));
}

// ══════════════ 5. which copy-mode bindings page up ══════════════

#[test]
fn page_up_copy_mode_commands_are_read_by_their_flags() {
    use crate::copy_mode::is_page_up_copy_mode_command as pages_up;
    assert!(pages_up("copy-mode -u"));
    assert!(pages_up("copy-mode -Hu"), "a flag cluster counts");
    assert!(pages_up("copy-mode -uH"));
    assert!(pages_up("copy-mode -e -u"));
    assert!(pages_up("copy-mode -u \\; send-keys -X cursor-up"));
    assert!(!pages_up("copy-mode"));
    assert!(!pages_up("copy-mode -H"));
    assert!(!pages_up("copy-mode -t my-ubuntu"), "a target containing -u is not the -u flag");
    assert!(!pages_up("copy-mode -tsess-u"));
    assert!(!pages_up("new-window -u"));
    assert!(!pages_up("select-pane \\; copy-mode -u"), "only the first command decides, as before");
}

// ══════════════ copy-selection and copy-pipe variants ══════════════
//
// tmux window-copy.c, measured with tmux 3.4 (select two cells, run the
// command, read pane_in_mode / selection_present):
//   copy-selection             stays in copy mode, selection cleared
//   copy-selection-no-clear    stays, selection kept
//   copy-selection-and-cancel  leaves copy mode
//   copy-pipe / -no-clear / -and-cancel  the same three
// psmux kept the selection after copy-selection and copy-pipe, and ran
// `copy-pipe-no-clear` as copy-pipe with the pipe command "-no-clear".

fn selected(cmd: &str) -> AppState {
    let mut a = app(true);
    a.copy_pos = Some((ROWS - 3, 0));
    assert_eq!(run_send_keys_x(&mut a, "begin-selection", 1), Ok(()));
    assert_eq!(run_send_keys_x(&mut a, "cursor-right", 2), Ok(()));
    assert!(a.copy_anchor.is_some(), "a selection before {}", cmd);
    assert_eq!(run_send_keys_x(&mut a, cmd, 1), Ok(()));
    a
}

#[test]
fn copy_selection_clears_and_stays() {
    let a = selected("copy-selection");
    assert!(a.mode.in_copy());
    assert!(a.copy_anchor.is_none(), "copy-selection clears the selection");
    assert!(!a.paste_buffers.is_empty(), "and copied it");
}

#[test]
fn copy_selection_no_clear_keeps_it() {
    let a = selected("copy-selection-no-clear");
    assert!(a.mode.in_copy());
    assert!(a.copy_anchor.is_some());
}

#[test]
fn copy_selection_and_cancel_leaves() {
    let a = selected("copy-selection-and-cancel");
    assert!(!a.mode.in_copy());
}

#[test]
fn copy_pipe_variants_follow_tmux() {
    let a = selected("copy-pipe");
    assert!(a.mode.in_copy() && a.copy_anchor.is_none(), "copy-pipe clears and stays");
    let a = selected("copy-pipe-no-clear");
    assert!(a.mode.in_copy() && a.copy_anchor.is_some(), "copy-pipe-no-clear keeps the selection");
    let a = selected("copy-pipe-and-cancel");
    assert!(!a.mode.in_copy(), "copy-pipe-and-cancel leaves");
}

// ══════════════ the copy-line family and append-selection ══════════════
//
// tmux 3.4 (window_copy_do_copy_line / _end_of_line, append_selection):
// only the -and-cancel spellings leave copy mode; the others copy, clear the
// selection and keep the cursor. psmux left copy mode on copy-end-of-line
// and copy-line (copy-line also kept the selection), ignored
// copy-pipe-end-of-line, copy-line-and-cancel and both copy-pipe-line verbs,
// and kept the selection after append-selection.

fn at_line(cmd: &str) -> (AppState, Option<(u16, u16)>) {
    let mut a = app(true);
    a.copy_pos = Some((ROWS - 3, 2));
    let before = a.copy_pos;
    assert_eq!(run_send_keys_x(&mut a, cmd, 1), Ok(()));
    (a, before)
}

#[test]
fn copy_line_family_stays_unless_cancel() {
    for cmd in ["copy-line", "copy-end-of-line", "copy-pipe-line", "copy-pipe-end-of-line"] {
        let (a, before) = at_line(cmd);
        assert!(a.mode.in_copy(), "{} stays in copy mode", cmd);
        assert!(a.copy_anchor.is_none(), "{} leaves no selection", cmd);
        assert_eq!(a.copy_pos, before, "{} keeps the cursor", cmd);
        assert!(!a.paste_buffers.is_empty(), "{} copied", cmd);
    }
    for cmd in ["copy-line-and-cancel", "copy-end-of-line-and-cancel",
                "copy-pipe-line-and-cancel", "copy-pipe-end-of-line-and-cancel"] {
        let (a, _) = at_line(cmd);
        assert!(!a.mode.in_copy(), "{} leaves copy mode", cmd);
        assert!(!a.paste_buffers.is_empty(), "{} copied", cmd);
    }
}

#[test]
fn copy_line_copies_the_whole_line_and_end_of_line_the_rest() {
    let (a, _) = at_line("copy-line");
    let line = a.paste_buffers[0].data.clone();
    let (b, _) = at_line("copy-end-of-line");
    let rest = b.paste_buffers[0].data.clone();
    assert!(line.starts_with("line"), "{:?}", line);
    assert!(line.ends_with(&rest) && rest.len() + 2 == line.len(), "line {:?} rest {:?}", line, rest);
}

#[test]
fn append_selection_clears_and_stays() {
    let a = selected("append-selection");
    assert!(a.mode.in_copy() && a.copy_anchor.is_none());
    let a = selected("append-selection-and-cancel");
    assert!(!a.mode.in_copy() && a.copy_anchor.is_none());
}
