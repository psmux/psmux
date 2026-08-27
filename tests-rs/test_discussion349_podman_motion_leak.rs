// Discussion #349: mouse motion sequences leak as raw text ("35;128;51M...")
// into an interactive podman container terminal once command output fills the
// screen.
//
// ROOT CAUSE (reproduced live with a real podman alpine container):
//   The attached client forwards every bare mouse move as
//   "pane-mouse <id> 35 <col> <row> M".  The server handlers
//   (handle_pane_mouse / remote_mouse_motion) gated ALL buttons, including
//   bare motion 35, on a permissive screen-content heuristic. It
//   false-positives on a filled screen whose
//   foreground is a NON-shell (podman.exe is neither a shell nor wsl/ssh, so
//   the #381 shell gate does not apply).  psmux then wrote SGR any-motion
//   bytes (ESC[<35;x;yM) into podman's pty on every mouse move; the container
//   shell never enabled mouse tracking, so the tty echoed them as garbage.
//
// CONTRACT:
//   Bare motion (button 35) is gated on pane_wants_bare_motion(), which requires the
//   child to have EXPLICITLY enabled any-event tracking (DECSET 1003; #604
//   narrowed this from 1002/1003, see the 1002 case below).
//   Clicks and drags use pane_wants_click(); wheel uses pane_wheel_forward().
//
// Registered from src/window_ops.rs so it can call the pub(crate) helpers.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A vt100 screen FILLED the way `ls` output fills it inside a container:
/// lines scrolled off the top, bottom rows full, prompt (cursor) on the last
/// row. This is the exact screen state the reporter describes ("everything is
/// fine for a few seconds... a couple ls commands... then it happens").
fn filled_parser(rows: u16, cols: u16) -> Arc<Mutex<vt100::Parser>> {
    let mut p = vt100::Parser::new(rows, cols, 0);
    let mut bytes = Vec::new();
    for i in 1..=(rows as usize + 4) {
        bytes.extend_from_slice(format!("-rw-r--r-- 1 root root {i} file{i}\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"/ # ");
    p.process(&bytes);
    Arc::new(Mutex::new(p))
}

/// Same filled screen, but the child has explicitly enabled AnyMotion
/// (DECSET 1003) or ButtonMotion (DECSET 1002) mouse tracking first.
fn filled_parser_with_decset(rows: u16, cols: u16, decset: &[u8]) -> Arc<Mutex<vt100::Parser>> {
    let p = filled_parser(rows, cols);
    p.lock().unwrap().process(decset);
    p
}

/// Assemble a Pane around a given parser. Only the fields the mouse gates
/// read are meaningful; the pty plumbing is a throwaway cmd.exe.
fn make_pane(term: Arc<Mutex<vt100::Parser>>, rows: u16, cols: u16) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = pair.slave.spawn_command(cmd).expect("spawn dummy");
    let writer = pair.master.take_writer().expect("writer");
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: pair.master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
        id: 0,
        title: "podman".to_string(),
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
        scroll_fg_cache: None, mouse_proto_owner: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PART 1: A filled container screen without an explicit mouse protocol must
// not receive hover events.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn discussion349_filled_container_screen_does_not_get_hover() {
    let term = filled_parser(10, 60);
    let pane = make_pane(term, 10, 60);

    let hover = pane_wants_bare_motion(&pane);
    assert!(!hover,
        "bare motion must not be forwarded: the child never enabled \
         DECSET 1002/1003, so pane_wants_bare_motion must be false");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 2 — NO REGRESSION for genuine hover consumers: a child that DID
// enable motion tracking still receives motion after the fix.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn discussion349_decset_1003_any_motion_still_gets_hover() {
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1003h\x1b[?1006h");
    let pane = make_pane(term, 10, 60);
    assert!(pane_wants_bare_motion(&pane),
        "a child that enabled AnyMotion (DECSET 1003) must still receive bare motion (#60)");
}

// Narrowed by #604.  DECSET 1002 is BUTTON-event tracking: report motion only
// WHILE A BUTTON IS HELD.  Only 1003 asks for motion with no button, and tmux
// draws the line in the same place (input-keys.c:737 discards a bare motion
// report unless `s->mode & MODE_MOUSE_ALL`).  psmux treated 1002 as consent for
// bare motion, so simply moving the pointer over `nvim -u NONE` (mouse=nvi,
// which enables 1002+1006 and never 1003) sprayed an ESC[<35;col;rowM report at
// nvim for every pointer sample.
#[test]
fn discussion349_decset_1002_button_motion_gets_no_bare_motion() {
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1002h\x1b[?1006h");
    let pane = make_pane(term, 10, 60);
    assert!(!pane_wants_bare_motion(&pane),
        "ButtonMotion (DECSET 1002) asks for motion only while a button is held, \
         so a BARE pointer move must not be forwarded (#604)");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 3 — the #296 property holds on this path too: alt-screen alone
// (a TUI that renders fullscreen but never asked for mouse) gets NO motion.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn discussion349_alt_screen_without_mouse_protocol_gets_no_hover() {
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1049h");
    let pane = make_pane(term, 10, 60);
    assert!(!pane_wants_bare_motion(&pane),
        "alt-screen without an explicit mouse protocol must not receive bare motion (#296)");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 4: CLICK gate (discussion #349 follow-up, comment 17754744).
//
// Clicks use pane_wants_click(), whose reliable signals are an enabled mouse
// protocol or the alternate screen.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn discussion349_clicks_not_forwarded_on_filled_container_screen() {
    // THE FIX: a filled container screen (no mouse protocol, not alt-screen,
    // no native ENABLE_MOUSE_INPUT) must NOT receive clicks. This is the exact
    // state behind the reporter's "0;37;26M0;37;26m" leak.
    let term = filled_parser(10, 60);
    let pane = make_pane(term, 10, 60);
    assert!(!pane_wants_click(&pane),
        "FIX CONTRACT: clicks must NOT be forwarded to a plain (container) shell \
         that never enabled mouse tracking (discussion #349 comment 17754744)");
}

#[test]
fn discussion349_clicks_forwarded_when_mouse_protocol_enabled() {
    // A VT app that enabled a mouse protocol (DECSET 1000/1002/1003) still
    // receives clicks after the fix.
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1000h\x1b[?1006h");
    let pane = make_pane(term, 10, 60);
    assert!(pane_wants_click(&pane),
        "a child that enabled a mouse protocol must still receive clicks (vim/htop)");
}

#[test]
fn discussion349_clicks_forwarded_on_alt_screen() {
    // A fullscreen app on modern ConPTY (alt-screen passes through) still
    // receives clicks.
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1049h");
    let pane = make_pane(term, 10, 60);
    assert!(pane_wants_click(&pane),
        "an alt-screen (fullscreen) app must still receive clicks");
}

#[test]
fn discussion349_button_motion_app_still_gets_clicks() {
    let term = filled_parser_with_decset(10, 60, b"\x1b[?1002h\x1b[?1006h");
    let pane = make_pane(term, 10, 60);
    assert!(pane_wants_click(&pane),
        "a ButtonMotion (DECSET 1002) app must still receive clicks");
}
