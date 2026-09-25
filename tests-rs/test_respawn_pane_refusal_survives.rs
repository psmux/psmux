//! A ROUTINE command refusal must never terminate the psmux server.
//!
//! MEASURED defect (isolated PSMUX_DATA_DIR, two windows, master at e7013a3):
//!
//!   BEFORE: rsp_probe: 2 windows (created Tue Sep  8 03:04:00 2026)
//!   --- psmux respawn-pane   (NO -k, pane is LIVE => routine refusal) ---
//!   client rc=0 out=[]
//!   session alive AFTER routine refusal : False
//!   AFTER : psmux: no server running on ...\psmux_rsp_probe
//!
//! `respawn-pane` on a LIVE pane without `-k` is the documented tmux refusal
//! (spawn.c: `pane <session>:<window>.<pane> still active`, reported by
//! cmd-respawn-pane.c as `respawn pane failed: %s` at exit 1). psmux's
//! `respawn_active_pane` returned that as an `io::Error`, and the server event
//! loop applied a bare `?` to it. `run_server` is `-> io::Result<()>`, so the
//! refusal unwound the whole event loop, the server EXITED, and every window
//! and pane in the session was destroyed. The client had no reply channel on
//! that path, so it printed nothing and exited 0: the caller was told a
//! command had succeeded while it was silently deleting their session.
//!
//! `respawn-window` shares `respawn_active_pane` and shared the `?`. It always
//! passes `kill = true`, so the "still active" refusal cannot fire there, but
//! the spawn failures below it (a `-c` directory that does not exist, a command
//! that will not start) are just as routine and were just as fatal.
//!
//! The fix gives both arms the per-request reply channel that `move-window` and
//! `kill-window` already use: the error goes back to the ORIGINATING request,
//! the status bar shows it for attached clients, and the loop keeps running.
//!
//! End to end coverage lives in `tests/test_respawn_pane_refusal_survives.ps1`.
//! Registered from `src/window_ops.rs`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use ratatui::layout::Rect;

/// Build a valid Pane wrapping a throwaway PTY, tagged with `id`.
fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
    let child = crate::util::StubChild::exited();
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
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

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: format!("w{id}"),
        id,
        area: Rect::new(0, 0, 120, 30),
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

/// One session, `windows` windows, one live PTY-backed pane in each. The last
/// window is active, so the message must name ITS index, not window 0's.
fn app_with_windows(session: &str, windows: usize) -> AppState {
    let mut app = AppState::new(session.to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    for w in 0..windows {
        let mut win = make_window(w);
        win.root = Node::Split {
            kind: LayoutKind::Horizontal,
            sizes: vec![100],
            children: vec![Node::Leaf(make_pane(100 + w, 40, 160))],
        };
        win.active_path = vec![0];
        app.windows.push(win);
    }
    app.active_idx = windows - 1;
    app
}

fn active_pane_is_dead(app: &AppState) -> bool {
    let win = &app.windows[app.active_idx];
    crate::tree::active_pane(&win.root, &win.active_path)
        .map(|p| p.dead)
        .expect("active pane")
}

// ─────────────────────────────────────────────────────────────────────────
// 1. The refusal itself: an ERROR VALUE, worded like tmux.
// ─────────────────────────────────────────────────────────────────────────

/// `respawn-pane` with no `-k` on a live pane is refused, and the refusal
/// arrives as a returned `Err` — a value the caller can route back to the
/// requesting client — carrying tmux's own wording so the CLI can print it
/// verbatim.
#[test]
fn live_pane_without_k_is_refused_as_a_value() {
    let mut app = app_with_windows("rsp_unit", 2);
    let err = crate::window_ops::respawn_active_pane(&mut app, None, None, false, None, false)
        .expect_err("a live pane without -k must be refused");
    // tmux spawn.c: xasprintf(cause, "pane %s:%d.%u still active", ...)
    assert_eq!(
        err.to_string(),
        "pane rsp_unit:1.0 still active",
        "the refusal must name session:window.pane the way tmux does, so the \
         CLI can print `respawn pane failed: <cause>` unchanged"
    );
}

/// The refusal must not have touched anything: the pane is still alive and both
/// windows are still there. A refusal with side effects would be worse than the
/// silent exit-0 it replaces.
#[test]
fn a_refusal_leaves_the_session_untouched() {
    let mut app = app_with_windows("rsp_unit", 2);
    let before_windows = app.windows.len();
    let before_active = app.active_idx;
    assert!(crate::window_ops::respawn_active_pane(&mut app, None, None, false, None, false).is_err());
    assert_eq!(app.windows.len(), before_windows, "no window may be lost to a refusal");
    assert_eq!(app.active_idx, before_active);
    assert!(!active_pane_is_dead(&app), "the refused pane must still be running");
}

/// The legitimate paths still work, so the guard cannot be "fixed" by refusing
/// everything: `-k` on a live pane goes through, and a DEAD pane goes through
/// without `-k` (that is the whole point of respawn-pane).
#[test]
fn k_on_a_live_pane_and_a_bare_respawn_of_a_dead_pane_both_pass_the_guard() {
    // -k on a live pane: allowed. `-E` keeps this unit test from spawning a
    // real shell; the shell path is covered end to end by the .ps1.
    let mut app = app_with_windows("rsp_unit", 1);
    crate::window_ops::respawn_active_pane(&mut app, None, None, true, None, true)
        .expect("-k on a live pane must be allowed");

    // A dead pane, no -k: allowed.
    let mut app = app_with_windows("rsp_unit", 1);
    {
        let win = &mut app.windows[0];
        if let Some(p) = crate::window_ops::active_pane_mut(&mut win.root, &win.active_path) {
            p.dead = true;
        }
    }
    crate::window_ops::respawn_active_pane(&mut app, None, None, false, None, true)
        .expect("a dead pane must be respawnable without -k");
}

// ─────────────────────────────────────────────────────────────────────────
// 2. The event loop must never turn a routine error into server death.
//
//    This is the generic guard, not a respawn-pane guard. `run_server` is
//    `-> io::Result<()>`, so ANY `?` inside it is a potential session killer;
//    the audit below is the classification of every one that exists.
// ─────────────────────────────────────────────────────────────────────────

/// True if the line applies `?` to a call or a value. Matches `foo()?;`,
/// `foo()? {`, `x?.y`, `foo()?,` alike — the shape is "a `?` right after an
/// identifier, `)` or `]`, and not the start of a word". A `?` sitting inside a
/// string or a `Option<...>` never has that left neighbour.
fn has_try_operator(line: &str) -> bool {
    let b = line.as_bytes();
    b.iter().enumerate().skip(1).any(|(i, &c)| {
        c == b'?'
            && {
                let prev = b[i - 1];
                prev.is_ascii_alphanumeric() || prev == b'_' || prev == b')' || prev == b']'
            }
            && b.get(i + 1)
                .map(|&n| !(n.is_ascii_alphanumeric() || n == b'_'))
                .unwrap_or(true)
    })
}

/// Source of the server event loop, from `fn run_server` to end of file.
fn run_server_body() -> &'static str {
    let src = include_str!("../src/server/mod.rs");
    let start = src.find("pub fn run_server(").expect("run_server");
    &src[start..]
}

/// The exact regression: `respawn_active_pane(...)?` inside `run_server`. Both
/// the `RespawnPane` and the `RespawnWindow` arm had one, and either was enough
/// to delete a whole session on a routine refusal.
#[test]
fn no_respawn_call_in_the_event_loop_uses_the_question_mark() {
    let offenders: Vec<&str> = run_server_body()
        .lines()
        .filter(|l| l.contains("respawn_active_pane(") && l.trim_end().ends_with("?;"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a respawn refusal is a per-request error, not a server fault; `?` here \
         unwinds run_server and destroys the session:\n{}",
        offenders.join("\n")
    );
}

/// Both respawn arms must answer their requester. Without a reply channel the
/// client cannot tell a refusal from a success, which is exactly how the
/// server-killing `?` stayed invisible (rc 0, empty output, session gone).
#[test]
fn both_respawn_arms_report_the_error_to_the_requesting_client() {
    let body = run_server_body();
    for (arm, message) in [
        ("CtrlReq::RespawnPane(", "respawn pane failed: "),
        ("CtrlReq::RespawnWindow(", "respawn window failed: "),
    ] {
        let at = body.find(arm).unwrap_or_else(|| panic!("{arm} arm not found"));
        let window = &body[at..(at + 2200).min(body.len())];
        assert!(
            window.contains("resp.send(Err("),
            "{arm} must return its refusal to the originating request"
        );
        assert!(
            window.contains(message),
            "{arm} must word its refusal like tmux: `{message}<cause>`"
        );
        assert!(
            window.contains("app.status_message = Some("),
            "{arm} must also surface the refusal to attached clients, which \
             have no reply stream (same as the move-window arm)"
        );
    }
}

/// The audit, pinned. Every `?` reachable inside `run_server` is listed here
/// with the reason it is allowed to unwind the server. A `?` on anything else
/// fails this test, so the next person adding a fallible call to an event-loop
/// arm has to classify it as routine (reply to the client) or genuinely fatal
/// (add it here) rather than silently shipping another session killer.
///
/// Classification of the survivors:
///   * `TcpListener::bind` / `local_addr` — startup only; with no control
///     socket there is no server to keep running. GENUINELY FATAL.
///   * `capture_active_pane_*` — every user-reachable miss (no such pane, a
///     poisoned parser lock) already returns `Ok(None)`; the `io::Result` is
///     vestigial. NOT user-reachable.
///   * `dump_layout_json*` / `list_windows_json*` / `list_tree_json` —
///     `serde_json` on plain owned structs. NOT user-reachable.
///   * `send_{text,key,bytes,paste}_to_active` — every branch swallows its
///     write and lock failures and returns `Ok(())`. NOT user-reachable.
///   * `map_err` / `ok_or_else` / `resolve_window_spec` — inside the
///     `WindowDump` and `SwapWindow` closures, which return their own
///     `Result` to the arm. These `?` do not leave the arm.
///   * `reap_children` — the child-process sweep, not a client request.
#[test]
fn every_question_mark_in_the_event_loop_is_a_classified_one() {
    const CLASSIFIED: &[&str] = &[
        "TcpListener::bind",
        ".local_addr()",
        "capture_active_pane_text",
        "capture_active_pane_styled",
        "capture_active_pane_range",
        "dump_layout_json",
        "dump_layout_json_fast",
        "list_windows_json",
        "list_windows_json_with_tabs",
        "list_tree_json",
        "send_text_to_active",
        "send_key_to_active",
        "send_bytes_to_active",
        "send_paste_to_active",
        ".map_err(",
        ".ok_or_else(",
        "resolve_window_spec",
        "reap_children",
    ];
    let question_marks: Vec<&str> = run_server_body()
        .lines()
        .map(|l| l.trim())
        // A `?` applied to a call or a value, not `?` inside a string/comment.
        .filter(|l| !l.starts_with("//"))
        .filter(|l| has_try_operator(l))
        .collect();
    // Self-check: if the scan ever stops finding the `?`s it is supposed to
    // classify, it would pass vacuously and guard nothing.
    assert!(
        question_marks.len() > 60,
        "the sweep found only {} `?` lines in run_server — it is no longer \
         reading the event loop and this test has stopped guarding anything",
        question_marks.len()
    );
    let unclassified: Vec<String> = question_marks
        .into_iter()
        .filter(|l| !CLASSIFIED.iter().any(|c| l.contains(c)))
        .map(|l| l.to_string())
        .collect();
    assert!(
        unclassified.is_empty(),
        "unclassified `?` inside run_server. `run_server` is -> io::Result<()>, \
         so each of these ends the server and destroys every window and pane in \
         the session. If the failure is ROUTINE (bad target, wrong state, \
         refusal, not found) reply to the originating request instead; if it is \
         genuinely fatal, add it to CLASSIFIED with the reason:\n{}",
        unclassified.join("\n")
    );
}
