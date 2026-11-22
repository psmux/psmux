//! Issue #1089: a program that reads its mouse as console records gets the
//! mouse on a build below `CONPTY_MOUSE_MIN_BUILD`.
//!
//! On 22523 and above the console host publishes a program's
//! `ENABLE_MOUSE_INPUT` upstream as `ESC[?1003;1006h`, so a record reader
//! reaches the wheel, click and motion gates with a mouse protocol of its own.
//! The Windows 10 inbox host (19045) publishes nothing, so there the gates
//! never opened for it: the wheel went to copy mode and clicks went nowhere,
//! although the #597 record channel delivers them.  The `pane_takes_*` gates
//! read the console mode instead on those builds.
//!
//! The console mode is seeded into the pane's 2 second cache, so nothing here
//! needs a console or a child process.  The E2E half is
//! `tests/test_issue1089_record_reader_mouse.ps1`.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct BuildRestore(Option<String>);

impl Drop for BuildRestore {
    fn drop(&mut self) {
        match self.0.as_deref() {
            Some(v) => std::env::set_var("PSMUX_FAKE_WIN_BUILD", v),
            None => std::env::remove_var("PSMUX_FAKE_WIN_BUILD"),
        }
    }
}

fn with_build<T>(build: &str, f: impl FnOnce() -> T) -> T {
    let _lock = crate::util::lock_test_env();
    let _restore = BuildRestore(std::env::var("PSMUX_FAKE_WIN_BUILD").ok());
    std::env::set_var("PSMUX_FAKE_WIN_BUILD", build);
    f()
}

/// A pane with no live child whose console mode is `mode` (None: no console
/// could be asked), as `console_input_mode` would have cached it.
fn pane_with_mode(bytes: &[u8], mode: Option<u32>) -> crate::types::Pane {
    let mut p = vt100::Parser::new(10, 60, 0);
    p.process(bytes);
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize { rows: 10, cols: 60, pixel_width: 0, pixel_height: 0 });
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child: crate::util::StubChild::exited(),
        term: Arc::new(Mutex::new(p)),
        last_rows: 10,
        last_cols: 60,
        id: 0,
        title: "pane".to_string(),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        dead_info: None,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: Some((Instant::now(), mode)),
        win32_input_latched: false, input_off: false,
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

/// (wheel, click, bare motion) through the gates the event routes call.
fn takes(pane: &mut crate::types::Pane) -> (bool, bool, bool) {
    (pane_takes_wheel(pane), pane_takes_click(pane), pane_takes_bare_motion(pane))
}

/// Console modes measured on 19045 and 26300 (probe r1_modes): a record
/// reader in its plain and its VT input shape.
const RECORD_READERS: [(u32, &str); 4] = [
    (0x01B0, "native_mouse_child: mouse on, cooked off"),
    (0x03B0, "the Bubble Tea shape: the same with VT input on"),
    (0x01B8, "Far Manager and vim"),
    (0x0098, "crossterm (pstop)"),
];

/// Console modes that are not a record reader, measured the same way.
const NOT_READERS: [(u32, &str); 5] = [
    (0x01E4, "a pwsh, Windows PowerShell or default shell prompt"),
    (0x01E7, "a cmd prompt, Read-Host"),
    (0x01F7, "[Console]::ReadKey, and the inherited default"),
    (0x0208, "node in raw mode (Claude Code), nvim"),
    (0x0000, "cmd's pause"),
];

#[test]
fn a_record_reader_gets_the_mouse_on_windows_10() {
    for build in ["19041", "19045", "20348"] {
        with_build(build, || {
            for (mode, what) in RECORD_READERS {
                let mut pane = pane_with_mode(b"", Some(mode));
                assert_eq!(takes(&mut pane), (true, true, true),
                    "build {build}, {what} ({mode:#06X}): the wheel, clicks and motion must reach it");
            }
        });
    }
}

#[test]
fn the_old_gates_alone_shut_it_out() {
    // What the routes asked before: on 19045 the host publishes neither a mouse
    // protocol nor the alternate screen, so every gate said no.
    with_build("19045", || {
        let pane = pane_with_mode(b"", Some(0x01B0));
        assert!(!pane_wheel_forward(&pane));
        assert!(!pane_wants_click(&pane));
        assert!(!pane_wants_bare_motion(&pane));
    });
}

#[test]
fn shells_and_vt_programs_are_not_record_readers() {
    // The wheel over these must still enter copy mode, and no click or motion
    // report may be typed into them.
    with_build("19045", || {
        for (mode, what) in NOT_READERS {
            let mut pane = pane_with_mode(b"", Some(mode));
            assert_eq!(takes(&mut pane), (false, false, false),
                "{what} ({mode:#06X}) must keep the mouse for psmux");
        }
    });
}

#[test]
fn no_console_answer_is_not_a_record_reader() {
    with_build("19045", || {
        let mut pane = pane_with_mode(b"", None);
        assert_eq!(takes(&mut pane), (false, false, false));
    });
}

#[test]
fn newer_builds_still_decide_from_the_published_protocol() {
    // From 22523 the host publishes the record reader's mode, so the console
    // mode is not consulted at all: with nothing published, nothing passes.
    for build in ["22523", "26100", "26300"] {
        with_build(build, || {
            let mut pane = pane_with_mode(b"", Some(0x01B0));
            assert_eq!(takes(&mut pane), (false, false, false),
                "build {build} must keep its gates exactly as they were");
        });
    }
}

#[test]
fn the_old_signals_still_open_the_gates_on_windows_10() {
    // A DECSET 1003 or the alternate screen that does reach psmux (a VT bridge
    // such as ssh or wsl, or OpenConsole through PSMUX_CONPTY_DIR) keeps
    // working whatever the console mode says.
    with_build("19045", || {
        let mut any = pane_with_mode(b"\x1b[?1003h", Some(0x0208));
        assert_eq!(takes(&mut any), (false, true, true), "1003 opens clicks and motion; the wheel also needs ownership");
        let mut alt = pane_with_mode(b"\x1b[?1049h", Some(0x0208));
        assert_eq!(takes(&mut alt), (true, true, false), "the alternate screen opens the wheel and clicks");
    });
}

/// Each leaf's (id, wants_mouse) in tree order, from both serialisers: the
/// fast one the server pushes to clients and the serde one, which must agree.
/// (leaf id, wants_mouse) pairs in tree order.
type Marks = Vec<(usize, bool)>;

fn frame_marks(app: &mut crate::types::AppState) -> (Marks, Marks) {
    fn marks(layout: &crate::layout::LayoutJson) -> Marks {
        match layout {
            crate::layout::LayoutJson::Leaf { id, wants_mouse, .. } => vec![(*id, *wants_mouse)],
            crate::layout::LayoutJson::Split { children, .. } => children.iter().flat_map(marks).collect(),
        }
    }
    let fast = crate::layout::dump_layout_json_fast(app).expect("fast dump");
    let slow = crate::layout::dump_layout_json(app).expect("serde dump");
    (
        marks(&serde_json::from_str(&fast).expect("fast dump parses")),
        marks(&serde_json::from_str(&slow).expect("serde dump parses")),
    )
}

/// A window of two panes: 1 a record reader (0x01B0), 2 a pwsh prompt (0x01E4).
fn app_reader_beside_prompt(mouse: bool) -> crate::types::AppState {
    use crate::types::{LayoutKind, Node, Window};
    let mut left = pane_with_mode(b"", Some(0x01B0));
    left.id = 1;
    let mut right = pane_with_mode(b"", Some(0x01E4));
    right.id = 2;
    let mut app = crate::types::AppState::new("record-reader-frame".to_string());
    app.mouse_enabled = mouse;
    app.last_window_area = ratatui::layout::Rect::new(0, 0, 60, 10);
    app.windows.push(Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![1, 1], children: vec![Node::Leaf(left), Node::Leaf(right)] },
        active_path: vec![0],
        name: "w0".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 60, 10),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![1, 2],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app.active_idx = 0;
    app
}

#[test]
fn the_frame_tells_the_client_a_record_reader_wants_the_mouse() {
    // The client decides on a left press whether the drag after it is the
    // program's or psmux's selection, from the leaf's wants_mouse.  Without
    // the mark a record reader got the press and lost the drag.
    with_build("19045", || {
        let mut app = app_reader_beside_prompt(true);
        let (fast, slow) = frame_marks(&mut app);
        assert_eq!(fast, vec![(1, true), (2, false)]);
        assert_eq!(slow, fast, "the serde dump marks the same panes");
    });
}

#[test]
fn the_frame_marks_nothing_with_the_mouse_off_or_on_newer_builds() {
    with_build("19045", || {
        let mut app = app_reader_beside_prompt(false);
        let (fast, slow) = frame_marks(&mut app);
        assert_eq!(fast, vec![(1, false), (2, false)], "mouse off: no console is asked");
        assert_eq!(slow, fast);
    });
    with_build("26300", || {
        let mut app = app_reader_beside_prompt(true);
        let (fast, slow) = frame_marks(&mut app);
        assert_eq!(fast, vec![(1, false), (2, false)], "22523 and above: the published protocol alone decides");
        assert_eq!(slow, fast);
    });
}
#[test]
fn a_no_goes_stale_after_100_ms_and_a_yes_does_not() {
    // A program sets its console mode as it starts, so a "no" cached at the
    // shell prompt just before must not hold its first notch for psmux.
    with_build("19045", || {
        let mut said_no = pane_with_mode(b"", Some(0x01E4));
        said_no.mouse_input_cache = Some((Instant::now() - Duration::from_millis(150), Some(0x01E4)));
        assert!(!pane_takes_wheel(&mut said_no), "no console to ask: still no");
        let (ts, _) = said_no.mouse_input_cache.expect("asked again and cached");
        assert!(ts.elapsed() < Duration::from_millis(100), "a 150 ms old no was asked again");

        let mut fresh_no = pane_with_mode(b"", Some(0x01E4));
        let at = Instant::now() - Duration::from_millis(20);
        fresh_no.mouse_input_cache = Some((at, Some(0x01E4)));
        assert!(!pane_takes_wheel(&mut fresh_no));
        assert_eq!(fresh_no.mouse_input_cache.map(|(t, _)| t), Some(at), "a 20 ms old no is kept");

        let mut said_yes = pane_with_mode(b"", Some(0x01B0));
        let at = Instant::now() - Duration::from_millis(500);
        said_yes.mouse_input_cache = Some((at, Some(0x01B0)));
        assert!(pane_takes_wheel(&mut said_yes), "a 500 ms old yes still holds");
        assert_eq!(said_yes.mouse_input_cache.map(|(t, _)| t), Some(at), "and is not asked again");
    });
}

#[test]
fn only_the_active_leaf_asks_its_console() {
    with_build("19045", || {
        let mut unasked = pane_with_mode(b"", None);
        unasked.mouse_input_cache = None;
        assert!(!leaf_record_reader(&mut unasked, true, false));
        assert!(unasked.mouse_input_cache.is_none(), "an inactive leaf must not query");
        assert!(!leaf_record_reader(&mut unasked, true, true));
        assert!(unasked.mouse_input_cache.is_some(), "the active leaf does");

        // The active leaf asks again even for a "no" taken a moment ago: the
        // frame carrying a program's first screen must see its new mode.
        let mut just_said_no = pane_with_mode(b"", Some(0x01E4));
        let at = Instant::now() - Duration::from_millis(5);
        just_said_no.mouse_input_cache = Some((at, Some(0x01E4)));
        assert!(!leaf_record_reader(&mut just_said_no, true, true));
        assert_ne!(just_said_no.mouse_input_cache.map(|(t, _)| t), Some(at), "a 5 ms old no was asked again");

        let mut known = pane_with_mode(b"", Some(0x01B0));
        known.mouse_input_cache = Some((Instant::now() - Duration::from_secs(30), Some(0x01B0)));
        assert!(leaf_record_reader(&mut known, true, false), "an inactive leaf keeps its pane's last answer");
        assert!(!leaf_record_reader(&mut known, false, false), "and marks nothing when the frame does not");
    });
}