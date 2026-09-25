// Issue #629: "Mouse scroll wheel not working over SSH (Kitty -> Windows)"
// (rishabhupadhyay097).  macOS kitty -> ssh -> Windows 11, PowerShell 5.1,
// TERM=xterm-256color, psmux 3.3.8.  The wheel reached the client and decoded
// cleanly, and then nothing happened:
//
//   send_mouse_enable: writing mouse-enable VT sequences to stdout
//   stdin console mode: 0x0298 VTI=true MOUSE=true
//   MOUSE via VT parser: Mouse(MouseEvent { kind: ScrollUp, column: 25, row: 30,
//                                           modifiers: KeyModifiers(0x0) })
//
// Clicks and pane selection over the same link worked, and
// `set -g scroll-enter-copy-mode off` made the wheel visible again.
//
// WHAT THE WHEEL PATH ACTUALLY DEPENDS ON.  How the client RECEIVED the notch
// (a Win32 MOUSE_EVENT record, or SGR bytes decoded by the VT parser, which is
// the only path an SSH login has) is settled entirely inside the client: both
// shapes converge on the same `pane-scroll`/`scroll-up` verb before the server
// sees anything.  The one thing that decides whether a notch scrolls psmux's
// own scrollback or is handed to the pane's child is `pane_wheel_forward`, and
// a pane handed the wheel it cannot use is silent — which is exactly what a
// PowerShell 5.1 prompt does with an SGR wheel report.
//
// At v3.3.8, the tag the reporter is running, that gate was
// `pane_wants_scroll_forward` (window_ops.rs:263 at v3.3.8):
//
//     if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None { return true; }
//     if screen.alternate_screen() { return true; }
//     false
//
// `mouse_protocol_mode()` is not a statement by the pane's application.  Under
// ConPTY it is conhost republishing the pane's CONSOLE INPUT MODE word upstream
// as `ESC[?1003;1006h` (see the #613 notes next door, and platform.rs), and the
// mouse bit is set in the inherited word on an ordinary console — measured on
// this tree, a plain PowerShell pane inherits 0x01F7, which already carries
// ENABLE_MOUSE_INPUT.  So a shell prompt could satisfy a gate meant for nvim,
// and the wheel was injected into a shell that ignores it.  That is #598, fixed
// after v3.3.8 by 1357bc1, which added the ownership term this file pins.
//
// TMUX PARITY.  tmux forwards the wheel only when the pane's own screen carries
// a mouse mode the APPLICATION set (`s->mode & ALL_MOUSE_MODES`, checked in
// input_key_mouse, input-keys.c:805; set only from the application's DECSET in
// input.c:2053).  No console word is in that path, so a shell prompt can never
// take the forward branch and the wheel always reaches copy mode.  psmux gets
// the same answer by attributing the transition (`update_mouse_proto_owner`)
// instead of trusting the mode word.
//
// Registered from src/window_ops.rs so it can reach the pub(crate) gates.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn parser_with(bytes: &[u8]) -> Arc<Mutex<vt100::Parser>> {
    let mut p = vt100::Parser::new(10, 60, 0);
    p.process(bytes);
    Arc::new(Mutex::new(p))
}

/// A pane with no live child, so every process/console probe fails.  The gate
/// must reach its verdict from pane-owned state alone.
fn make_pane(term: Arc<Mutex<vt100::Parser>>) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows: 10, cols: 60, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = crate::util::spawn_pty_child(&*pair.slave, cmd).expect("spawn dummy");
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
        title: "powershell".to_string(),
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

/// The gate psmux shipped at v3.3.8, reproduced verbatim so the regression this
/// file guards is stated rather than described.  Nothing in the tree calls it.
fn v338_pane_wants_scroll_forward(pane: &crate::types::Pane) -> bool {
    if let Ok(parser) = pane.term.lock() {
        let screen = parser.screen();
        if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None {
            return true;
        }
        if screen.alternate_screen() {
            return true;
        }
    }
    false
}

// ─────────────────────────────────────────────────────────────────────────
// PART 1: the reporter's pane.  A PowerShell prompt on the MAIN screen whose
// console word conhost republished as a mouse protocol.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue629_shell_owned_mouse_protocol_does_not_divert_the_wheel() {
    // `ESC[?1003;1006h` is the shape conhost emits for the pane's inherited
    // console input mode — not something the shell asked for.  #598's fix
    // records who earned it; a shell prompt earns nothing.
    let mut pane = make_pane(parser_with(b"\x1b[?1003h\x1b[?1006h"));
    pane.mouse_proto_owner = Some((vt100::MouseProtocolMode::AnyMotion, false));

    assert!(
        v338_pane_wants_scroll_forward(&pane),
        "precondition: this is the pane v3.3.8 handed the wheel to, which is why \
         the reporter's notches vanished into PowerShell"
    );
    assert!(
        !pane_wheel_forward(&pane),
        "BUG #629: a mouse protocol the SHELL did not ask for must not divert the \
         wheel; tmux forwards only on the application's own DECSET, so a shell \
         prompt always gets copy-mode scrollback"
    );
}

#[test]
fn issue629_a_plain_shell_pane_never_forwards() {
    // No protocol, no alternate screen: the ordinary prompt an SSH user scrolls.
    let pane = make_pane(parser_with(b""));
    assert!(!v338_pane_wants_scroll_forward(&pane));
    assert!(!pane_wheel_forward(&pane), "a bare prompt must always reach copy mode");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 2: the audiences the #629 answer must not cost anything.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue629_an_application_that_asked_still_gets_the_wheel() {
    // nvim `set mouse=a`, Claude Code, any crossterm app: the transition was
    // attributed to a confirmed non-shell foreground (#573/#597 audience).
    let mut pane = make_pane(parser_with(b"\x1b[?1000h\x1b[?1006h"));
    pane.mouse_proto_owner = Some((vt100::MouseProtocolMode::Press, true));
    assert!(
        pane_wheel_forward(&pane),
        "an app-owned mouse protocol must keep receiving SGR wheel reports"
    );
}

#[test]
fn issue629_alternate_screen_still_forwards() {
    // less, htop, nvim: tmux's own `alternate_on` term.  #598 narrowed who gets
    // the wheel; it never took it away from the alternate screen.
    let pane = make_pane(parser_with(b"\x1b[?1049h"));
    assert!(pane_in_alt_screen(&pane), "precondition: pane is on the alternate screen");
    assert!(pane_wheel_forward(&pane), "an alternate-screen pane keeps the wheel");
}

#[test]
fn issue629_the_two_gate_terms_are_independent() {
    // Ownership alone and alt-screen alone each suffice; neither is required of
    // the other.  This is what keeps the #629 answer from re-opening #598: the
    // fix removed a term, it did not add a condition to the surviving ones.
    let mut owned = make_pane(parser_with(b""));
    owned.mouse_proto_owner = Some((vt100::MouseProtocolMode::AnyMotion, true));
    assert!(!pane_in_alt_screen(&owned));
    assert!(pane_wheel_forward(&owned), "ownership alone forwards");

    let alt = make_pane(parser_with(b"\x1b[?1049h"));
    assert!(alt.mouse_proto_owner.is_none());
    assert!(pane_wheel_forward(&alt), "alternate screen alone forwards");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 3: the reporter's workaround, and why it pointed at the right branch.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue629_scroll_enter_copy_mode_off_is_not_a_wheel_gate() {
    // `set -g scroll-enter-copy-mode off` restored a visible scroll for the
    // reporter, which is only possible below the gate: both of its branches
    // (`enter_copy_mode` + `scroll_copy_up`, and `scroll_pane_scrollback`) are
    // reached only when the wheel was NOT forwarded.  So the option cannot
    // rescue a pane the gate diverted, and the gate is where #629 lives.  Pin
    // that the option is nowhere in the forwarding decision.
    let mut diverted = make_pane(parser_with(b"\x1b[?1049h"));
    let before = pane_wheel_forward(&diverted);
    diverted.pane_options.insert("scroll-enter-copy-mode".to_string(), "off".to_string());
    assert_eq!(
        before,
        pane_wheel_forward(&diverted),
        "scroll-enter-copy-mode must not influence who receives the wheel"
    );
}
