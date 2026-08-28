// Issue #613: "a node child's raw mode strips ENABLE_MOUSE_INPUT and the pane's
// wheel goes silent for good" (457shop).
//
// MEASURED ON THIS TREE, with a real attached client, real MOUSE_EVENT injection
// and the console mode read the way the reporter read it (AttachConsole +
// CreateFileW("CONIN$") + GetConsoleMode), in tests/test_issue613_wheel_gate_durability.ps1:
//
//   PROBE[A] before=0x03B0 mouse=True     WHEEL up x1 -> 1 SGR chunk
//     RECV <ESC>[<64;31;15M  |  1B 5B 3C 36 34 3B 33 31 3B 31 35 4D
//   MODE longnode-running    0x0208 mouse=False        (a node child took raw mode)
//   WHEEL up x1 -> 0 SGR chunks
//   MODE after-longnode-killed 0x0208 mouse=False      (killed; libuv restored nothing)
//   WHEEL up x1 -> 0   WHEEL down x2 -> 0   WHEEL up x5 -> 0
//
// The pane's own application never changed its mind and never learned anything
// had happened.  Its console was rewritten out from under it.
//
// ROOT CAUSE.  The #598 gate accepts two answers to "did this application ask
// for the mouse", and BOTH of them resolve to the same console input mode word:
//
//   window_ops::detect_mouse_input        reads ENABLE_MOUSE_INPUT off the
//                                         child console right now
//   window_ops::update_mouse_proto_owner  is driven by mouse_protocol_mode(),
//                                         which under ConPTY is conhost
//                                         republishing that same word upstream
//                                         as ESC[?1003;1006h / ESC[?1003;1006l
//
// That word belongs to the CONSOLE, not to the process that set it.  libuv's
// uv_tty_set_mode(UV_TTY_MODE_RAW) ASSIGNS ENABLE_WINDOW_INPUT |
// ENABLE_VIRTUAL_TERMINAL_INPUT over the whole word instead of clearing single
// bits, and puts nothing back on exit, so one `node` child anywhere in the
// pane's process tree destroys both signals at once.  Measured with
// PSMUX_PANE_RAW=1: the strip surfaced in the pane's output as
//
//   [console-mouse child started]           <ESC>[?1003;1006h
//   [after node child stripped the bit]     <ESC>[?1003;1006l
//
// TMUX PARITY.  tmux cannot have this bug.  Its authorization is
// `s->mode & ALL_MOUSE_MODES` (tmux.h:698) on the pane's OWN `struct screen`,
// checked in input_key_mouse (input-keys.c:805):
//
//     if (m->ignore || (s->mode & ALL_MOUSE_MODES) == 0)
//             return;
//
// set only by the application's DECSET (input.c:2053-2062), cleared only by its
// DECRST (input.c:1959) or by screen_reinit when the pane respawns
// (screen.c:115).  There is no console in the path, so no third process can
// revoke it.
//
// FIX.  `Pane::wheel_auth`, a pane-owned latch anchored to the pid that earned
// the authorization.  It is consulted LAST, only after both live signals have
// already answered no, so a pane that never earned anything is exactly as
// silent as #598 made it.  It is dropped when its owner leaves the pane, and on
// a mouse-protocol withdrawal that does not carry libuv's raw-mode fingerprint.
//
// Registered from src/window_ops.rs so it can reach the pub(crate) gates.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn parser_with(decset: &[u8]) -> Arc<Mutex<vt100::Parser>> {
    let mut p = vt100::Parser::new(10, 60, 0);
    p.process(decset);
    Arc::new(Mutex::new(p))
}

/// A pane with no live child process, so every process/console probe fails.
/// That is deliberate: it pins what the gate does when it cannot confirm
/// anything, which must always be the #598 safe side.
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
        title: "node".to_string(),
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
// PART 1: the fingerprint that tells a wholesale overwrite from a decision.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue613_libuv_raw_word_is_exactly_window_input_plus_vt_input() {
    const ENABLE_WINDOW_INPUT: u32 = 0x0008;
    const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
    assert_eq!(
        LIBUV_RAW_INPUT_MODE,
        ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT,
        "libuv's uv_tty_set_mode(UV_TTY_MODE_RAW) assigns exactly these two bits; \
         0x0208 is what every affected pane in #613 measured"
    );
    assert_eq!(LIBUV_RAW_INPUT_MODE, 0x0208,
        "the reporter's five silent panes all read 0x0208 and the two that scrolled read 0x0298");
}

#[test]
fn issue613_a_considered_narrowing_is_not_the_libuv_fingerprint() {
    const ENABLE_MOUSE_INPUT: u32 = 0x0010;
    // The model TUI in the E2E suite registers 0x03B0.  An application that
    // decides it no longer wants the mouse clears ONE bit and keeps the rest of
    // its word; that must still count as a withdrawal, or `:set mouse=` in nvim
    // would keep receiving reports it stopped asking for.
    let registered: u32 = 0x03B0;
    let withdrawn = registered & !ENABLE_MOUSE_INPUT;
    assert_eq!(withdrawn, 0x03A0, "precondition: clearing the mouse bit alone");
    assert_ne!(withdrawn, LIBUV_RAW_INPUT_MODE,
        "BUG #613: a narrowed word must not be mistaken for libuv's wholesale overwrite, \
         or an app that genuinely turned the mouse off would keep the latch");
    assert_ne!(registered, LIBUV_RAW_INPUT_MODE);
    // And the mouse-on word from the reporter's two working panes is not it either.
    assert_ne!(0x0298_u32, LIBUV_RAW_INPUT_MODE);
}

// ─────────────────────────────────────────────────────────────────────────
// PART 2: the latch, and the #598 safe side it must never cross.
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn issue613_a_pane_that_never_asked_has_no_latch_to_hold() {
    // htop, codex, a plain shell: the #598 audience.  Nothing earned, nothing
    // held, and asking must not invent one.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    assert!(pane.wheel_auth.is_none(), "a fresh pane starts unauthorized");
    assert!(!wheel_auth_holds(&mut pane),
        "BUG: an unearned pane must stay silent, which is the whole of #598");
    assert!(pane.wheel_auth.is_none(), "asking must not create an authorization");
}

#[test]
fn issue613_latching_needs_a_confirmed_non_shell_foreground() {
    // Same tri-state rule as `update_mouse_proto_owner`'s `app_owned`.  With no
    // child pid the foreground probe cannot confirm anything, and an
    // inconclusive probe must never earn a standing authorization: PSReadLine
    // enables mouse tracking on the shell's behalf (#360/#548).
    let mut pane = make_pane(parser_with(b"\x1b[?1000h\x1b[?1006h"));
    assert!(pane.child_pid.is_none(), "precondition: no resolvable foreground");
    latch_wheel_auth(&mut pane);
    assert!(pane.wheel_auth.is_none(),
        "BUG: an unconfirmed foreground must not earn a wheel authorization");
}

#[test]
fn issue613_a_decset_on_the_alternate_screen_is_attributed_to_the_app() {
    // The measured Claude Code shape: ESC[?1004h ESC[?1049h ESC[?1000h
    // ESC[?1002h ESC[?1003h ESC[?1006h in one burst, at an instant when the
    // pane's deepest leaf is still the `claude` launcher.  Before the fix that
    // produced `app_owned=false`, and because the attribution is sampled once
    // per transition and the mode never changes again, the wrong answer was
    // frozen for the life of the pane (`#{mouse_any_flag}` = no forever).
    let mut pane = make_pane(parser_with(b"\x1b[?1004h\x1b[?1049h\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h"));
    assert!(pane_in_alt_screen(&pane), "precondition: the app took the alternate screen");
    assert!(pane.child_pid.is_none(),
        "precondition: the foreground probe cannot confirm a non-shell, exactly as during \
         the launcher race");
    update_mouse_proto_owner(&mut pane);
    assert_eq!(pane.mouse_proto_owner, Some((vt100::MouseProtocolMode::AnyMotion, true)),
        "BUG #613: a mouse DECSET raised while the pane is on the alternate screen is not \
         PSReadLine's; PSReadLine enables tracking at a MAIN screen prompt");
}

#[test]
fn issue613_a_decset_on_the_main_screen_still_needs_a_confirmed_app() {
    // The #548/#360 guard, unchanged: on the main screen an unconfirmed
    // foreground is PSReadLine's spurious tracking and the wheel must keep
    // copy-mode semantics.
    let mut pane = make_pane(parser_with(b"\x1b[?1000h\x1b[?1006h"));
    assert!(!pane_in_alt_screen(&pane), "precondition: main screen");
    update_mouse_proto_owner(&mut pane);
    assert_eq!(pane.mouse_proto_owner.map(|(_, owned)| owned), Some(false),
        "BUG: an unconfirmed main-screen foreground must stay attributed to the shell");
    assert!(pane.wheel_auth.is_none(), "and must not earn a standing authorization");
}

#[test]
fn issue613_a_live_latch_survives_being_asked_repeatedly() {
    // The cached answer is what a wheel BURST rides on: the reporter scrolls,
    // and every notch must not pay for a process walk.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    pane.wheel_auth = Some(crate::types::WheelAuth {
        owner_pid: 4242,
        alive_cache: Some((Instant::now(), true)),
    });
    for notch in 0..8 {
        assert!(wheel_auth_holds(&mut pane),
            "BUG #613: notch {notch} lost the authorization the pane already earned");
    }
    assert_eq!(pane.wheel_auth.map(|a| a.owner_pid), Some(4242),
        "the owner must not drift while the latch holds");
}

#[test]
fn issue613_a_dead_owner_drops_the_latch() {
    // The latch is the app's, not the pane's: when the app exits, a wheel notch
    // at the shell prompt has to go back to copy mode (#360), not be typed into
    // the shell.  PID 0 is the idle process and is never a pane descendant, so
    // the tree check refuses it.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    pane.child_pid = Some(std::process::id());
    pane.wheel_auth = Some(crate::types::WheelAuth {
        owner_pid: 0,
        alive_cache: Some((Instant::now() - Duration::from_secs(5), true)),
    });
    assert!(!wheel_auth_holds(&mut pane),
        "BUG: an owner outside the pane's process tree must not keep the latch");
    assert!(pane.wheel_auth.is_none(), "the expired latch must be cleared, not re-checked forever");
}

#[test]
fn issue613_stale_cache_is_re_examined_rather_than_trusted() {
    // A cache entry older than the 2 second TTL must not answer on its own,
    // otherwise a latch could outlive its owner by however long nobody scrolled.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    pane.child_pid = None; // no tree to be in -> the re-examination says no
    pane.wheel_auth = Some(crate::types::WheelAuth {
        owner_pid: 4242,
        alive_cache: Some((Instant::now() - Duration::from_secs(30), true)),
    });
    assert!(!wheel_auth_holds(&mut pane),
        "BUG: a 30 second old 'alive' answer must be re-examined, not believed");
}

#[test]
fn issue613_withdrawal_that_is_not_libuv_shaped_drops_the_latch() {
    // `update_mouse_proto_owner` sees mode == None.  With no child pid the
    // console cannot be shown to carry libuv's fingerprint, and the safe side
    // of #598 is always to suppress: treat it as the application withdrawing.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    pane.wheel_auth = Some(crate::types::WheelAuth { owner_pid: 4242, alive_cache: None });
    pane.mouse_proto_owner = Some((vt100::MouseProtocolMode::AnyMotion, true));
    update_mouse_proto_owner(&mut pane);
    assert!(pane.mouse_proto_owner.is_none(), "precondition: the protocol is off");
    assert!(pane.wheel_auth.is_none(),
        "an unattributable withdrawal must drop the latch, never keep it on a guess");
}

#[test]
fn issue613_an_active_protocol_leaves_the_latch_alone() {
    // The DECRST branch is the only one allowed to clear.  A pane whose app
    // still has a mouse protocol on must keep whatever it earned.
    let mut pane = make_pane(parser_with(b"\x1b[?1003h\x1b[?1006h"));
    pane.wheel_auth = Some(crate::types::WheelAuth { owner_pid: 4242, alive_cache: None });
    update_mouse_proto_owner(&mut pane);
    assert_eq!(pane.wheel_auth.map(|a| a.owner_pid), Some(4242),
        "BUG: a live mouse protocol must not clear the pane's authorization");
}

// ─────────────────────────────────────────────────────────────────────────
// PART 3: the explicit opt-in for a pane that can never earn anything.
//
// Measured: a node TUI that writes ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h
// and then enters raw mode can have conhost swallow the DECSET entirely, so
// psmux sees no mouse protocol at any point while the app reads SGR fine.
// ─────────────────────────────────────────────────────────────────────────

fn set_or_clear(key: &str, value: Option<&str>) {
    match value {
        Some(v) => std::env::set_var(key, v),
        None => std::env::remove_var(key),
    }
}

/// Restores every env seam on drop, so a failing assertion inside the closure
/// still cleans up instead of leaking an override into the #457/#573 suites.
struct EnvRestore {
    wheel: Option<String>,
    mouse: Option<String>,
    build: Option<String>,
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        set_or_clear(crate::ssh_input::FORCE_WHEEL_ENV, self.wheel.as_deref());
        set_or_clear("PSMUX_FORCE_MOUSE", self.mouse.as_deref());
        set_or_clear("PSMUX_FAKE_WIN_BUILD", self.build.as_deref());
    }
}

fn with_env<T>(
    wheel: Option<&str>,
    mouse: Option<&str>,
    build: Option<&str>,
    f: impl FnOnce() -> T,
) -> T {
    let _lock = crate::util::lock_test_env();
    let _restore = EnvRestore {
        wheel: std::env::var(crate::ssh_input::FORCE_WHEEL_ENV).ok(),
        mouse: std::env::var("PSMUX_FORCE_MOUSE").ok(),
        build: std::env::var("PSMUX_FAKE_WIN_BUILD").ok(),
    };
    set_or_clear(crate::ssh_input::FORCE_WHEEL_ENV, wheel);
    set_or_clear("PSMUX_FORCE_MOUSE", mouse);
    set_or_clear("PSMUX_FAKE_WIN_BUILD", build);
    f()
}

#[test]
fn issue613_nothing_is_forced_by_default() {
    let pane = make_pane(parser_with(b"\x1b[?1049h"));
    with_env(None, None, None, || {
        assert!(!wheel_forced(&pane),
            "the #598 gate must stay in force for anyone who did not opt in");
    });
}

#[test]
fn issue613_pane_option_opens_the_gate_for_that_pane_only() {
    let mut forced = make_pane(parser_with(b"\x1b[?1049h"));
    let untouched = make_pane(parser_with(b"\x1b[?1049h"));
    for raw in ["on", "1", "true", "yes", "  ON  ", "True"] {
        forced.pane_options.insert("@mouse-force".to_string(), raw.to_string());
        with_env(None, None, None, || {
            assert!(wheel_forced(&forced),
                "BUG #613: set -p @mouse-force {raw:?} must authorize this pane's wheel");
            assert!(!wheel_forced(&untouched),
                "BUG: @mouse-force on one pane must not reach another; the #598 damage \
                 is decided per pane");
        });
    }
}

#[test]
fn issue613_pane_option_off_keeps_the_gate_even_when_the_env_var_is_set() {
    // The narrower scope wins in BOTH directions: a user who forced the whole
    // server must still be able to exempt the one pane running htop.
    let mut pane = make_pane(parser_with(b"\x1b[?1049h"));
    for raw in ["off", "0", "false", "no", "maybe", "2", "", "   "] {
        pane.pane_options.insert("@mouse-force".to_string(), raw.to_string());
        with_env(Some("1"), None, None, || {
            assert!(!wheel_forced(&pane),
                "BUG: @mouse-force {raw:?} on this pane must beat a server wide \
                 PSMUX_FORCE_WHEEL=1, and an unrecognised value is not consent");
        });
    }
}

#[test]
fn issue613_env_var_is_the_server_wide_last_resort() {
    let pane = make_pane(parser_with(b"\x1b[?1049h"));
    for raw in ["1", "on", "true", "yes", "  ON  ", "True"] {
        with_env(Some(raw), None, None, || {
            assert!(wheel_forced(&pane),
                "BUG #613: PSMUX_FORCE_WHEEL={raw:?} must authorize the wheel (PR #614)");
        });
    }
    for raw in ["0", "off", "false", "no", "", "   ", "maybe", "2"] {
        with_env(Some(raw), None, None, || {
            assert!(!wheel_forced(&pane),
                "PSMUX_FORCE_WHEEL={raw:?} is not consent and must keep the gate");
        });
    }
}

#[test]
fn issue613_force_mouse_does_not_open_the_wheel_gate() {
    // The two overrides travel in opposite directions: PSMUX_FORCE_MOUSE (#573)
    // governs whether psmux may write mouse DECSET OUT to the terminal, this one
    // governs whether a report psmux already holds may be delivered INTO a pane.
    // A host that needed the first says nothing about the second.
    let pane = make_pane(parser_with(b"\x1b[?1049h"));
    with_env(None, Some("1"), None, || {
        assert!(!wheel_forced(&pane),
            "PSMUX_FORCE_MOUSE must not silently widen the pane-direction gate");
        assert!(!crate::ssh_input::wheel_gate_forced());
    });
}

#[test]
fn issue613_force_wheel_does_not_relax_the_457_build_gate() {
    // And the reverse, which is the direction that can kill a pane: #457's build
    // gate exists because an inbound SGR report fast-fails Win10-era conhost and
    // takes the pane process down.  Opting into the wheel must not reach it.
    with_env(Some("1"), None, Some("20348"), || {
        assert!(!crate::ssh_input::conpty_mouse_supported(),
            "PSMUX_FORCE_WHEEL must not relax the #457 build gate");
    });
}
