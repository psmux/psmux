// Issue #693: five tmux parity gaps in target handling, found by the #691
// survey.  Every one of them reproduced three times on master c467dab before a
// line of product code was read.
//
//  1. `link-window -s <sess>:0 -t <sess>:5` did nothing at rc=0.  Two causes in
//     one command: `-s` was read as
//     `w[1].trim_start_matches(':').parse::<usize>()`, which cannot read a
//     session qualified source, and `-t` was stripped by
//     `without_outer_target` and then refused by the generic temp focus, which
//     will not focus an index no window holds yet.  In tmux that `-t` is
//     `CMD_FIND_WINDOW_INDEX` (cmd-move-window.c:83, the branch link-window
//     shares with move-window) and need NOT exist, exactly like break-pane's
//     in #689.
//
//  2. `unlink-window -t <sess>:1` ignored its `-t` and removed
//     `app.active_idx`.  It only looked right because the temp focus moved the
//     active window onto the target first; `unlink-window -t <sess>:9`, a
//     window that does not exist, exited 0 having done nothing instead of
//     tmux's `can't find window: 9`.  tmux acts on `target->wl`
//     (cmd-kill-window.c:75-83).
//
//  3. `select-pane -t s:1.0` switched the current WINDOW.  tmux's
//     cmd-select-pane.c calls `window_set_active_pane(w, wp, 1)` on the TARGET
//     window (:274) and never `session_select`, so the session's current
//     window does not move:
//
//         parked on window 0, select-pane -t s:1.1
//         before  current=0  panes 0.0* 1.0* 1.1
//         after   current=1  panes 0.0* 1.0  1.1*     <- window moved
//         tmux            current=0
//
//  4. `select-window -t +1`, `-t !`, `-t {end}`, `-t -`, `-t +` died on the
//     CLI with `no server running on session '<ns>__+1'`, and `-t +1` did
//     reach the server only because Rust's `usize` parser accepts a leading
//     `+`, so it was read as the literal index 1:
//
//         parked 0 : select-window -t +1 -> 1   (want 1)
//         parked 1 : select-window -t +1 -> 1   (want 2)
//         parked 2 : select-window -t +1 -> 1   (want 3)
//
//     tmux maps the braced spellings through `cmd_find_window_table`
//     (cmd-find.c:51-58) and resolves the rest in
//     `cmd_find_get_window_with_session` (cmd-find.c:364-457).  That resolver
//     exists in psmux as `AppState::resolve_window_spec` and move-window and
//     swap-window have used it since #602; select-window simply never called
//     it.
//
//  5. `select-pane -l` with no last pane exited 0 in silence.  tmux errors
//     `no last pane` at exit 1 (cmd-select-pane.c:176), and before that falls
//     back to the sibling when the window has exactly two panes and neither
//     was ever visited (:167-172) - psmux had an index flipping guess there.
//
// Registered from src/server/connection.rs.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};

use crate::types::{AppState, LayoutKind, Node, WindowTarget};
use ratatui::layout::Rect;
use std::time::Instant;

// ---------------------------------------------------------------- fixtures

fn make_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
    let pty = portable_pty::native_pty_system();
    let pair = pty
        .openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .expect("openpty");
    let mut cmd = portable_pty::CommandBuilder::new("cmd.exe");
    cmd.arg("/c");
    cmd.arg("exit");
    let child = crate::util::spawn_pty_child(&*pair.slave, cmd).expect("spawn dummy");
    let writer = pair.master.take_writer().expect("writer");
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master: pair.master,
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
        mouse_input_cache: None, win32_input_latched: false,
        scroll_fg_cache: None, mouse_proto_owner: None, wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None, live_term: None,
        pane_style: None, pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize, name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
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

/// One window per entry, each with `panes` panes side by side.  Pane ids are
/// unique across the whole session, the way a real server allocates them.
fn app_with_windows(panes_per_window: &[usize]) -> AppState {
    let mut app = AppState::new("issue693".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    let mut next_pane_id = 1usize;
    for (w, n) in panes_per_window.iter().enumerate() {
        let mut win = make_window(w + 1, &format!("w{}", w));
        let children: Vec<Node> = (0..*n)
            .map(|_| {
                let p = Node::Leaf(make_pane(next_pane_id, 40, 160 / (*n).max(1) as u16));
                next_pane_id += 1;
                p
            })
            .collect();
        let sizes = vec![(100 / (*n).max(1)) as u16; *n];
        win.root = Node::Split { kind: LayoutKind::Horizontal, sizes, children };
        win.active_path = vec![0];
        app.windows.push(win);
        app.window_indices.push(w);
    }
    app.active_idx = 0;
    app
}

fn active_pane_index(app: &AppState, win_pos: usize) -> usize {
    crate::tree::pane_index_in_window(
        &app.windows[win_pos].root, &app.windows[win_pos].active_path,
    ).unwrap()
}

// ------------------------------------------ item 1: link-window flag parsing

#[test]
fn issue693_link_window_reads_a_session_qualified_source() {
    // The reporter's command.  `-s t693_link:0` used to go through
    // `trim_start_matches(':').parse::<usize>()`, which yields None for
    // anything carrying a session, so the source silently became the ACTIVE
    // window.  It is a RAW spec now, resolved by the shared #602 resolver.
    let la = parse_link_window_args(&["-s", "sess:0", "-t", "sess:5"], None);
    assert_eq!(la.src.as_deref(), Some("sess:0"), "-s must survive whole");
    assert_eq!(la.dst.as_deref(), Some("sess:5"), "-t must survive whole");
    assert!(!la.detach && !la.kill && !la.after && !la.before);
}

#[test]
fn issue693_link_window_takes_the_outer_target_when_the_args_lost_it() {
    // `without_outer_target` strips `-t` before the arm sees the line, so the
    // value the generic parser peeled off has to be handed back in.  This is
    // break-pane's contract from #689, applied to link-window.
    let la = parse_link_window_args(&["-d"], Some("sess:5"));
    assert_eq!(la.dst.as_deref(), Some("sess:5"));
    assert!(la.detach);

    // An explicit -t still in the args wins over the outer one.
    let la = parse_link_window_args(&["-t", "sess:7"], Some("sess:5"));
    assert_eq!(la.dst.as_deref(), Some("sess:7"));
}

#[test]
fn issue693_link_window_parses_tmuxs_whole_flag_set() {
    // cmd-move-window.c:49, `"abdks:t:"`.
    let la = parse_link_window_args(&["-d", "-k", "-a", "-s", "1", "-t", "2"], None);
    assert!(la.detach, "-d");
    assert!(la.kill, "-k");
    assert!(la.after, "-a");
    assert!(!la.before);
    assert_eq!(la.src.as_deref(), Some("1"));
    assert_eq!(la.dst.as_deref(), Some("2"));

    let la = parse_link_window_args(&["-b"], None);
    assert!(la.before && !la.after);
}

#[test]
fn issue693_bare_link_window_names_nothing_and_that_is_the_default() {
    // tmux's `.source = { 's', CMD_FIND_WINDOW, 0 }` defaults to the current
    // window, and a `-t` naming only a session leaves the index at -1, the
    // next free slot (cmd-find.c:351).  None for both says exactly that.
    let la = parse_link_window_args(&[], None);
    assert_eq!(la.src, None);
    assert_eq!(la.dst, None);
}

#[test]
fn issue693_link_window_quoted_values_are_unwrapped() {
    let la = parse_link_window_args(&["-s", "\"my sess:0\"", "-t", "\"5\""], None);
    assert_eq!(la.src.as_deref(), Some("my sess:0"));
    assert_eq!(la.dst.as_deref(), Some("5"));
}

// -------------------------------------- item 2: unlink-window target parsing

#[test]
fn issue693_unlink_window_keeps_its_target() {
    // It used to carry NOTHING to the server, so the arm removed
    // `app.active_idx` whatever the -t said.
    assert_eq!(parse_unlink_window_target(&["-t", "sess:1"], None).as_deref(), Some("sess:1"));
    assert_eq!(parse_unlink_window_target(&[], Some("sess:1")).as_deref(), Some("sess:1"));
    assert_eq!(parse_unlink_window_target(&["-t", "@2"], None).as_deref(), Some("@2"));
    assert_eq!(parse_unlink_window_target(&["-k"], None), None,
        "a bare unlink-window still means the current window");
}

#[test]
fn issue693_unlink_window_resolves_the_window_it_names_not_the_active_one() {
    // Windows 0, 1, 2 with the session parked on 2, tmux's
    // cmd-kill-window.c:75-83 unlinks `target->wl`.
    let app = app_with_windows(&[1, 1, 1]);
    let mut app = app;
    app.active_idx = 2;
    let pos = app.resolve_window_spec("issue693:1", false).unwrap().pos().unwrap();
    assert_eq!(pos, 1, "the -t decides, not active_idx ({})", app.active_idx);
}

#[test]
fn issue693_unlink_window_target_that_names_no_window_is_an_error() {
    // The one shape the temp focus could not hide: `-t sess:9` exited 0
    // having done nothing, where tmux says `can't find window: 9` and exits 1.
    let app = app_with_windows(&[1, 1, 1]);
    let err = app.resolve_window_spec("issue693:9", false).unwrap_err();
    assert_eq!(err, "can't find window: 9", "tmux's wording, not a silent no-op");
}

// ------------------------------- item 4: which -t select-window resolves

#[test]
fn issue693_every_symbolic_window_spec_goes_to_the_resolver() {
    // cmd-find.c:51-58 maps the braced spellings; :390-417 does the offsets;
    // :418-441 does `!`, `^`, `$`.  All of them are one resolver's job.
    for t in ["+", "-", "+1", "-2", "!", "^", "$",
              "{start}", "{last}", "{end}", "{next}", "{previous}",
              "0", "12", "@3", "sess:1", ":2", "sess:{end}"] {
        assert_eq!(
            select_window_spec(Some(t)).as_deref(), Some(t),
            "select-window -t {} names a WINDOW and must be resolved server side", t
        );
    }
}

#[test]
fn issue693_a_bare_name_is_still_a_session_and_an_empty_target_is_a_no_op() {
    // psmux runs one server per session, so the session fallback tmux does at
    // cmd-find.c:348 is the CLI's routing step.  #693 does not change it.
    for t in ["work", "my.session", "0abc", "dev-box"] {
        assert_eq!(select_window_spec(Some(t)), None,
            "{} is a session name, which is how psmux has always read it", t);
    }
    // `sess:` with nothing after it is that session's current window.
    assert_eq!(select_window_spec(Some("sess:")), None);
    assert_eq!(select_window_spec(Some("")), None);
    assert_eq!(select_window_spec(None), None);
}

#[test]
fn issue693_select_window_emits_one_spec_request_for_a_symbolic_target() {
    // #690's rule holds: one select-window, one window request.
    for t in ["+1", "!", "{end}", "-", "@2", "sess:3"] {
        let (reqs, resp) = select_window_requests(&[], Some(t), None, false, None);
        assert_eq!(reqs.len(), 1, "-t {} must emit exactly one request", t);
        match &reqs[0] {
            CtrlReq::SelectWindowSpec { spec, .. } => assert_eq!(spec, t),
            other => panic!("-t {} took the wrong carrier: {:?}", t, std::mem::discriminant(other)),
        }
        assert!(resp.is_some(), "-t {} must be able to report can't find window", t);
    }
}

#[test]
fn issue693_the_relative_flags_still_beat_the_target() {
    // cmd-select-window.c tests -n, then -p, then -l before the -t target.
    // Routing the target through the resolver must not have jumped that queue.
    let (n, _) = select_window_requests(&["-n"], Some("+1"), Some(0), false, None);
    assert!(matches!(n[0], CtrlReq::NextWindow));
    let (p, _) = select_window_requests(&["-p"], Some("{end}"), Some(0), false, None);
    assert!(matches!(p[0], CtrlReq::PrevWindow));
    let (l, _) = select_window_requests(&["-l"], Some("!"), Some(0), false, None);
    assert!(matches!(l[0], CtrlReq::LastWindow));
}

#[test]
fn issue693_a_session_only_target_still_emits_nothing() {
    // `select-window -t othersess` is routed by session and selects nothing
    // here, which is what it did before.  Turning it into
    // "can't find window: othersess" would have been a regression.
    let (reqs, resp) = select_window_requests(&[], Some("work"), None, false, None);
    assert!(reqs.is_empty(), "a session target must not become a window error");
    assert!(resp.is_none());
}

#[test]
fn issue693_the_cli_lets_every_symbolic_form_through_to_the_server() {
    // The CLI read a bare `!` or `{end}` as a SESSION name and died with
    // "no server running on session '<ns>__!'" without a byte reaching the
    // server.  move-window and swap-window were coerced by #602; select-window
    // joins them.
    for t in ["+", "-", "+2", "-3", "!", "^", "$", "{end}", "{last}", "{next}"] {
        assert_eq!(
            crate::cli::coerce_bare_window_target("select-window", t),
            format!(":{}", t),
            "select-window -t {} must reach the server as a window spec", t
        );
    }
    // And a bare name is still a session, on every one of the three commands.
    for cmd in ["select-window", "move-window", "swap-window"] {
        assert_eq!(crate::cli::coerce_bare_window_target(cmd, "work"), "work");
    }
}

// ------------------------------- item 4: what the resolver makes of them

#[test]
fn issue693_an_offset_steps_from_the_current_window_not_from_zero() {
    // The measured bug: `-t +1` landed on window 1 from every starting
    // window, because a leading `+` parses as a usize in Rust and the spec was
    // read as the literal index 1.
    let mut app = app_with_windows(&[1, 1, 1, 1]);
    for park in 0..4usize {
        app.active_idx = park;
        let want = (park + 1) % 4;
        assert_eq!(
            app.resolve_window_spec("+1", false).unwrap(), WindowTarget::Pos(want),
            "from window {}, +1 is window {}", park, want
        );
    }
    app.active_idx = 0;
    assert_eq!(app.resolve_window_spec("-1", false).unwrap(), WindowTarget::Pos(3),
        "-1 wraps to the end, tmux winlink_previous_by_number");
}

#[test]
fn issue693_the_symbols_resolve_the_way_cmd_find_window_table_spells_them() {
    let mut app = app_with_windows(&[1, 1, 1]);
    app.active_idx = 0;
    app.last_window_idx = 1;
    for (spec, want) in [
        ("^", 0usize), ("{start}", 0),
        ("$", 2), ("{end}", 2),
        ("!", 1), ("{last}", 1),
        ("+", 1), ("{next}", 1),
        ("-", 2), ("{previous}", 2),
    ] {
        assert_eq!(
            app.resolve_window_spec(spec, false).unwrap(), WindowTarget::Pos(want),
            "-t {} must resolve to window {}", spec, want
        );
    }
}

// ---------------------- item 3: a pane target must not move the current window

#[test]
fn issue693_setting_a_pane_in_another_window_leaves_the_current_window_alone() {
    // tmux: window_set_active_pane(w, wp, 1) on the TARGET window
    // (cmd-select-pane.c:274), no session_select anywhere in the file.
    let mut app = app_with_windows(&[1, 2]);
    app.active_idx = 0;
    assert_eq!(active_pane_index(&app, 1), 0, "window 1 starts on its pane 0");

    let moved = crate::tree::set_window_active_pane_by_index(&mut app, 1, 1);

    assert!(moved, "window 1's active pane must have changed");
    assert_eq!(app.active_idx, 0, "the SESSION's current window must not move (#693 item 3)");
    assert_eq!(active_pane_index(&app, 1), 1, "window 1 is now on its pane 1");
    assert_eq!(active_pane_index(&app, 0), 0, "window 0 is untouched");
}

#[test]
fn issue693_a_pane_id_in_another_window_reaches_it_the_same_way() {
    // A bare `%id` is CMD_FIND_PANE and names its own window, so it must obey
    // the same rule as an explicit `sess:N.M`.
    let mut app = app_with_windows(&[1, 2]);
    app.active_idx = 0;
    // ids: window 0 has %1, window 1 has %2 and %3.
    assert_eq!(crate::tree::find_window_pos_of_pane_id(&app, 3), Some(1));

    let moved = crate::tree::set_window_active_pane_by_id(&mut app, 1, 3);

    assert!(moved);
    assert_eq!(app.active_idx, 0, "a %id in another window must not switch windows either");
    assert_eq!(active_pane_index(&app, 1), 1);
}

#[test]
fn issue693_selecting_the_pane_that_is_already_active_moves_nothing() {
    // cmd-select-pane.c:269, `if (wp == w->active) return`, which is also the
    // gate on after-select-pane that #691 installed.
    let mut app = app_with_windows(&[1, 2]);
    app.active_idx = 0;
    assert!(!crate::tree::set_window_active_pane_by_index(&mut app, 1, 0),
        "window 1's pane 0 is already its active pane");
    assert!(!crate::tree::set_window_active_pane_by_index(&mut app, 1, 9),
        "a pane index that does not exist moves nothing");
    assert_eq!(app.active_idx, 0);
}

// --------------------------------------------- item 5: what "last pane" means

#[test]
fn issue693_a_single_pane_window_has_no_last_pane() {
    // The reproduction: a fresh one pane window, `select-pane -l` exited 0 in
    // silence.  tmux: cmdq_error "no last pane", CMD_RETURN_ERROR
    // (cmd-select-pane.c:176).
    let app = app_with_windows(&[1]);
    assert_eq!(crate::server::last_pane_path(&app), None,
        "one pane, never visited: tmux says no last pane");
}

#[test]
fn issue693_a_two_pane_window_falls_back_to_the_sibling() {
    // cmd-select-pane.c:167-172: lastwp NULL AND window_count_panes == 2 takes
    // the neighbour, which is what `split-window -d` then `select-pane -l`
    // hits.  psmux had an index flipping guess (`*idx = (*idx + 1) % 2`) in
    // one request and nothing at all in the other.
    let app = app_with_windows(&[2]);
    assert_eq!(crate::server::last_pane_path(&app), Some(vec![1]),
        "the sibling of a two pane window is the last pane");
}

#[test]
fn issue693_three_panes_and_no_history_is_still_no_last_pane() {
    // The count is exactly 2 in tmux, not "more than one".
    let app = app_with_windows(&[3]);
    assert_eq!(crate::server::last_pane_path(&app), None,
        "three panes and no history: tmux errors rather than guessing");
}

#[test]
fn issue693_a_remembered_pane_wins_and_a_stale_one_does_not_count() {
    let mut app = app_with_windows(&[3]);
    app.last_pane_path = vec![2];
    assert_eq!(crate::server::last_pane_path(&app), Some(vec![2]),
        "the remembered pane is the last pane");

    // A path no longer in the tree (its pane was killed) is not a last pane.
    app.last_pane_path = vec![7];
    assert_eq!(crate::server::last_pane_path(&app), None);

    // Nor is the pane we are already on.
    app.last_pane_path = vec![0];
    assert_eq!(crate::server::last_pane_path(&app), None,
        "the active pane cannot be its own last pane");
}
