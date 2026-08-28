// Issue #604: "Mouse support seems to conflicts in some scenarios" (fekir).
//
// Two symptoms, both reproduced live on this tree with a real attached client,
// real nvim and real MOUSE_EVENT injection (tests/test_issue604_wsl_nvim_mouse.ps1):
//
//   A) psmux -> wsl.exe -> nvim: clicking did not move the cursor.  0 of 4
//      injected clicks reached nvim, while the same 4 clicks reached nvim.exe
//      in a psmux pane and reached nvim inside WSL with no psmux in the path.
//
//   B) merely MOVING the pointer over a psmux pane running nvim made the
//      cursor flicker.  60 pointer samples produced 60 unsolicited
//      ESC[<35;col;rowM reports into nvim plus 60 full state frames and a
//      redraw storm, against 0 of anything while idle.
//
// ROOT CAUSE OF A (self-inflicted, measured with PSMUX_PANE_RAW=1):
//   For a VT bridge child (wsl.exe/ssh.exe) psmux injects the SGR report with
//   WriteConsoleInputW.  platform::mouse_inject::send_vt_sequence wrapped that
//   write in SetConsoleMode(set) / SetConsoleMode(restore) that also toggled
//   ENABLE_QUICK_EDIT_MODE.  conhost derives "is this client tracking the
//   mouse" from the console input mode, so it mirrored every toggle back up the
//   ConPTY into the PANE'S OUTPUT as `ESC[?1003;1006h` immediately followed by
//   `ESC[?1003;1006l`.  psmux's own vt100 parser applied both, and DECRST 1003
//   clears the mouse protocol outright, so the trailing DECRST wiped the DECSET
//   1002 that nvim had really asked for.  The pane's mode went ButtonMotion ->
//   None across the very first forwarded event and the bridge gate in
//   inject_mouse_combined suppressed every click after that.
//
// ROOT CAUSE OF B (tmux parity):
//   Bare motion was gated on DECSET 1002 OR 1003.  1002 is BUTTON-event
//   tracking: report motion only while a button is held.  Only 1003 asks for
//   motion with no button.  tmux, input-keys.c:723 and :737:
//
//       if (MOUSE_DRAG(m->b) && (s->mode & MOTION_MOUSE_MODES) == 0)
//               return (0);
//       ...
//       if (MOUSE_DRAG(m->sgr_b) &&
//           MOUSE_RELEASE(m->sgr_b) &&
//           (~s->mode & MODE_MOUSE_ALL))
//               return (0);
//
//   and tmux does not even ask the OUTER terminal for bare motion unless a pane
//   wants 1003 (tty.c:897).  Every pointer sample was additionally treated like
//   a keystroke by the client, forcing a full dump-state round trip.
//
// Registered from src/window_ops.rs so it can call the pub(crate) gates.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn parser_with(decset: &[u8]) -> Arc<Mutex<vt100::Parser>> {
    let mut p = vt100::Parser::new(10, 60, 0);
    p.process(decset);
    Arc::new(Mutex::new(p))
}

fn make_pane(term: Arc<Mutex<vt100::Parser>>) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows: 10, cols: 60, pixel_width: 0, pixel_height: 0 })
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
        last_rows: 10,
        last_cols: 60,
        id: 0,
        title: "nvim".to_string(),
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
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PART 1, SYMPTOM A: the mechanism that broke the click.
//
// This is the parser truth that made psmux's own console-mode churn fatal.
// It is not a bug in the parser: tmux clears every mouse mode on DECRST
// 1000/1001/1002/1003 too (input.c:1955).  It is the reason psmux must not
// provoke conhost into emitting that pair in the first place.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue604_conhost_decset_round_trip_wipes_the_apps_button_motion() {
    let mut p = vt100::Parser::new(10, 60, 0);

    // nvim inside wsl.exe asks for button-event tracking with SGR encoding.
    p.process(b"\x1b[?1002h\x1b[?1006h");
    assert_eq!(p.screen().mouse_protocol_mode(), vt100::MouseProtocolMode::ButtonMotion,
        "precondition: the application's own DECSET 1002 must register");

    // Exactly what PSMUX_PANE_RAW=1 captured coming back down the pane after a
    // single injected mouse event, one pair per event.
    p.process(b"\x1b[?1003;1006h\x1b[?1003;1006l");

    assert_eq!(p.screen().mouse_protocol_mode(), vt100::MouseProtocolMode::None,
        "a conhost round trip leaves the pane looking like it never asked for the \
         mouse, which is why send_vt_sequence must not toggle the console mode (#604)");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 2, SYMPTOM B: bare motion is 1003 only (tmux MODE_MOUSE_ALL).
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue604_bare_motion_requires_any_motion_mode() {
    use vt100::MouseProtocolMode as M;
    assert!(!mode_reports_bare_motion(M::None),
        "an app that never asked gets no motion");
    assert!(!mode_reports_bare_motion(M::Press),
        "DECSET 9 is press only");
    assert!(!mode_reports_bare_motion(M::PressRelease),
        "DECSET 1000 is press and release only");
    assert!(!mode_reports_bare_motion(M::ButtonMotion),
        "DECSET 1002 reports motion only WHILE A BUTTON IS HELD (tmux input-keys.c:737)");
    assert!(mode_reports_bare_motion(M::AnyMotion),
        "DECSET 1003 is the only mode that asks for motion with no button");
}

#[test]
fn issue604_button_motion_pane_gets_no_bare_motion() {
    // `nvim -u NONE` on 0.8+ has mouse=nvi and emits exactly this.
    let pane = make_pane(parser_with(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h"));
    assert!(!pane_wants_bare_motion(&pane),
        "moving the pointer over nvim must send nothing at all (#604 symptom B)");
}

#[test]
fn issue604_any_motion_pane_still_gets_bare_motion() {
    let pane = make_pane(parser_with(b"\x1b[?1003h\x1b[?1006h"));
    assert!(pane_wants_bare_motion(&pane),
        "an app that asked for 1003 must still receive bare motion (no #60/#296 regression)");
}

#[test]
fn issue604_button_motion_pane_still_gets_clicks() {
    // The narrowing is about MOTION only.  Clicks keep their own gate, which is
    // the whole point of symptom A: nvim under wsl must receive them.
    let pane = make_pane(parser_with(b"\x1b[?1049h\x1b[?1002h\x1b[?1006h"));
    assert!(pane_wants_click(&pane),
        "a 1002 app must still receive clicks, that is symptom A");
}

#[test]
fn issue604_pane_with_no_mouse_protocol_gets_neither() {
    let pane = make_pane(parser_with(b""));
    assert!(!pane_wants_bare_motion(&pane));
    assert!(!pane_wants_click(&pane),
        "#598 must not regress: an app that never asked for the mouse gets nothing");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 3, SYMPTOM B: the client must not mistake a pointer move for a key.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue604_bare_motion_commands_are_recognised() {
    use crate::client::is_bare_motion_cmd;

    // SGR button 35 = motion bit 32 + "no button held" low bits 3.
    assert!(is_bare_motion_cmd("pane-mouse 4 35 12 3 M\n"),
        "a pointer move inside a pane");
    assert!(is_bare_motion_cmd("mouse-move 12 3\n"),
        "a pointer move outside every pane");

    assert!(!is_bare_motion_cmd("pane-mouse 4 0 12 3 M\n"),
        "a left press is a real event and must keep the echo fast path");
    assert!(!is_bare_motion_cmd("pane-mouse 4 0 12 3 m\n"),
        "a left release too");
    assert!(!is_bare_motion_cmd("pane-mouse 4 32 12 3 M\n"),
        "a DRAG (button 32, button held) is a real event");
    assert!(!is_bare_motion_cmd("pane-mouse 4 64 12 3 M\n"),
        "the wheel is a real event");
    assert!(!is_bare_motion_cmd("send-key a\n"),
        "an ordinary keystroke");
    assert!(!is_bare_motion_cmd("send-text 35\n"),
        "text that merely contains 35 is not a mouse verb");
    assert!(!is_bare_motion_cmd("mouse-move-fake 1 2\n"),
        "the verb must match exactly");
}
