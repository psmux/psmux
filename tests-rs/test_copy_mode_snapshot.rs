// Copy mode must run on a SNAPSHOT of the pane's screen, the way tmux's
// copy-mode grid does.
//
// Live reproduction (QPC-timed 100ms polling of
// `#{pane_in_mode}|#{scroll_position}|#{history_size}`) while the pane was in
// copy mode and the app inside it (pi) was redrawing, with no resize and no
// second client involved:
//
//   mode=1 scroll=27   hist=5334
//   mode=1 scroll=30   hist=5334     <- the user scrolled 30 lines up
//   mode=1 scroll=1067 hist=1163     <- app redraw: retained history collapses
//   mode=1 scroll=3476 hist=3476     <- view stranded on the oldest retained
//                                       line (the top) until the user hit Esc
//
// psmux froze the *live* parser in place, so rows entering scrollback kept
// pushing the offset while the retained history churned underneath it; once
// the anchored rows were evicted the offset clamped to the retained depth and
// the view had nowhere to come back to.  tmux avoids the whole class by giving
// copy mode its own copy of the pane's grid, which is what these tests pin.

use super::*;

fn tcp_pair() -> (std::net::TcpStream, std::net::TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let accept = std::thread::spawn(move || listener.accept().expect("accept").0);
    let client = std::net::TcpStream::connect(addr).expect("connect");
    let server = accept.join().expect("join accept thread");
    (client, server)
}

/// One window, one proxy pane: a real `Pane` with a real vt100 parser, but no
/// child process behind it (the pane mirrors a TCP stream nobody writes to).
pub(crate) fn app_with_pane() -> AppState {
    let mut app = AppState::new("copysnap".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = ratatui::layout::Rect { x: 0, y: 0, width: 80, height: 24 };

    let (reader, _peer_reader) = tcp_pair();
    let (writer, _peer_writer) = tcp_pair();
    let pane = crate::proxy_pane::create_proxy_pane(
        reader,
        writer,
        "127.0.0.1:1".to_string(),
        "test-key".to_string(),
        "test-session".to_string(),
        0,
        None,
        "test-pane".to_string(),
        24,
        80,
        7,
        None,
    )
    .expect("create proxy pane");

    let window = crate::types::Window {
        root: crate::types::Node::Leaf(pane),
        active_path: vec![],
        name: "w0".to_string(),
        id: 0,
        area: app.last_window_area,
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![7],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    };
    app.windows.clear();
    app.windows.push(window);
    app.active_idx = 0;
    app
}

pub(crate) fn pane_of(app: &AppState) -> Option<&crate::types::Pane> {
    let win = app.windows.get(app.active_idx)?;
    crate::tree::active_pane(&win.root, &win.active_path)
}

pub(crate) fn view_term(app: &AppState) -> std::sync::Arc<std::sync::Mutex<vt100::Parser>> {
    pane_of(app).expect("active pane").term.clone()
}

pub(crate) fn parked_live_term(app: &AppState) -> Option<std::sync::Arc<std::sync::Mutex<vt100::Parser>>> {
    pane_of(app).expect("active pane").live_term.clone()
}

pub(crate) fn feed(term: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>, prefix: &str, from: usize, to: usize) {
    if let Ok(mut parser) = term.lock() {
        for i in from..to {
            parser.process(format!("{prefix} {i}\r\n").as_bytes());
        }
    }
}

pub(crate) fn filled(term: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>) -> usize {
    term.lock().map(|p| p.screen().scrollback_filled()).unwrap_or(0)
}

/// Copy mode shows a snapshot; the live parser is parked and stays reachable.
#[test]
fn entering_copy_mode_installs_a_snapshot_and_parks_the_live_screen() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 40);

    crate::copy_mode::enter_copy_mode(&mut app);

    let view = view_term(&app);
    assert!(
        !std::sync::Arc::ptr_eq(&view, &live),
        "copy mode must not read the live screen directly"
    );
    let parked = parked_live_term(&app).expect("the live parser must be parked");
    assert!(
        std::sync::Arc::ptr_eq(&parked, &live),
        "the parked parser must be the one the PTY reader keeps feeding"
    );
    assert_eq!(
        filled(&view),
        filled(&live),
        "the snapshot must start out holding exactly what the pane had"
    );
}

/// The regression: output arriving while copy mode is up must not be able to
/// strand the view on the oldest retained line.
#[test]
fn pane_output_while_in_copy_mode_cannot_strand_the_view() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 200);

    crate::copy_mode::enter_copy_mode(&mut app);
    crate::copy_mode::scroll_copy_up(&mut app, 30);
    let view = view_term(&app);
    let anchored = app.copy_scroll_offset;
    let retained_before = filled(&view);
    assert!(anchored > 0, "fixture must actually be scrolled up");
    assert!(
        anchored <= retained_before,
        "the anchor must be reachable to start with"
    );

    // The application keeps redrawing: thousands of rows into the live screen,
    // far more than the retained history holds.
    feed(&live, "after", 0, 5000);

    assert_eq!(
        filled(&view),
        retained_before,
        "the copy-mode screen must not churn while the user reads it"
    );
    assert_eq!(
        app.copy_scroll_offset, anchored,
        "the anchor must not be rewritten by the pane's output"
    );
    assert!(
        app.copy_scroll_offset <= filled(&view),
        "the anchor must still be reachable: this is the state that used to \
         read scroll_position == history_size, pinned at the top"
    );
}

/// Leaving copy mode shows the live screen, including everything printed while
/// copy mode was up, and drops the snapshot.
#[test]
fn leaving_copy_mode_shows_everything_that_arrived() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 40);

    crate::copy_mode::enter_copy_mode(&mut app);
    feed(&live, "while-in-copy-mode", 0, 40);
    crate::copy_mode::exit_copy_mode(&mut app);

    assert!(parked_live_term(&app).is_none(), "the snapshot must be released");
    let view = view_term(&app);
    assert!(
        std::sync::Arc::ptr_eq(&view, &live),
        "the pane must be back on the live parser"
    );
    let contents = view.lock().map(|p| p.screen().contents()).unwrap_or_default();
    assert!(
        contents.contains("while-in-copy-mode 39"),
        "output printed during copy mode must be visible afterwards"
    );
    assert_eq!(
        view.lock().map(|p| p.screen().scrollback()).unwrap_or(99),
        0,
        "the live screen must come back unscrolled"
    );
}

/// The reconciliation used by the per-frame path fixes both directions: a pane
/// left holding a snapshot outside copy mode, and copy mode running without
/// one.
#[test]
fn sync_copy_snapshot_reconciles_both_directions() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 40);

    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(parked_live_term(&app).is_some());

    // A path that leaves copy mode without calling exit_copy_mode: the next
    // frame must notice and restore the live screen.
    app.mode = Mode::Passthrough;
    crate::copy_mode::sync_copy_snapshot(&mut app);
    assert!(
        parked_live_term(&app).is_none(),
        "a stale snapshot must be released"
    );
    assert!(std::sync::Arc::ptr_eq(&view_term(&app), &live));

    // ... and the other way around.
    app.mode = Mode::CopyMode;
    crate::copy_mode::sync_copy_snapshot(&mut app);
    assert!(
        parked_live_term(&app).is_some(),
        "copy mode must be given a snapshot"
    );
}

/// Resizing while copy mode is up must reach the parked live screen too, or it
/// comes back with a stale geometry when copy mode exits.
#[test]
fn resizing_while_in_copy_mode_resizes_both_screens() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 60);

    crate::copy_mode::enter_copy_mode(&mut app);
    let view = view_term(&app);

    app.last_window_area = ratatui::layout::Rect { x: 0, y: 0, width: 100, height: 30 };
    crate::tree::resize_all_panes(&mut app);

    let view_size = view.lock().map(|p| p.screen().size()).ok();
    let live_size = live.lock().map(|p| p.screen().size()).ok();
    assert_eq!(
        view_size, live_size,
        "both screens must end up the same size (view {view_size:?}, live {live_size:?})"
    );

    crate::copy_mode::exit_copy_mode(&mut app);
    let after = view_term(&app).lock().map(|p| p.screen().size()).ok();
    assert_eq!(
        after, live_size,
        "the live screen must not come back with a stale geometry"
    );
}

/// `capture-pane` inspects the pane, not the copy-mode view: while the user
/// browses a snapshot, capture must return the live screen. tmux reads
/// `wp->base` for a capture and never the copy-mode grid, so a plugin that
/// snapshots the pane (continuum, iTerm2 attach) keeps seeing the real
/// terminal instead of whatever the user happens to be reading.
#[test]
fn capture_pane_reads_the_live_screen_while_copy_mode_shows_a_snapshot() {
    let mut app = app_with_pane();
    assert!(!app.windows.is_empty(), "fixture must provide a window and a pane");
    assert!(pane_of(&app).is_some(), "fixture must provide an active pane");
    let live = view_term(&app);
    feed(&live, "before", 0, 40);

    crate::copy_mode::enter_copy_mode(&mut app);
    // Output arrives while the user is browsing the snapshot.
    feed(&live, "arrived", 0, 40);

    let captured = crate::copy_mode::capture_active_pane_text(&mut app, None, false)
        .expect("capture-pane must not fail")
        .expect("capture-pane must return text");
    assert!(
        captured.contains("arrived 39"),
        "capture-pane must read the live pane while copy mode shows a snapshot; got:\n{captured}"
    );
    let view = view_term(&app)
        .lock()
        .map(|p| p.screen().contents())
        .unwrap_or_default();
    assert!(
        !view.contains("arrived 39"),
        "the copy-mode view itself must stay frozen (got live content in the view)"
    );
}

/// `clear-history` must clear the pane's LIVE scrollback and leave copy mode,
/// the way tmux does (cmd-capture-pane.c:418: `window_pane_reset_mode_all(wp)`
/// then `grid_clear_history(wp->base.grid)`).
///
/// With the snapshot installed, `pane.term` is the frozen view, so a
/// `clear-history` that clears `pane.term` wipes the screen the user is reading
/// and leaves the live scrollback intact. Measured on the release binary before
/// the fix: 502 retained lines, `#{history_size}` 0 while in copy mode, and 502
/// again the moment copy mode was left, with `capture-pane -S -200` still
/// returning 229 scrollback lines.
#[test]
fn clear_history_leaves_copy_mode_and_clears_the_live_screen() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "before", 0, 400);
    assert!(filled(&live) > 100, "the live pane needs a scrollback to clear");

    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(matches!(app.mode, Mode::CopyMode), "must be in copy mode");
    assert!(parked_live_term(&app).is_some(), "a snapshot must be installed");

    crate::window_ops::clear_active_pane_history(&mut app, false);

    assert!(
        !matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }),
        "clear-history must leave copy mode (tmux window_pane_reset_mode_all)"
    );
    assert!(
        parked_live_term(&app).is_none(),
        "the snapshot must be gone, so the pane shows its live screen again"
    );
    assert_eq!(
        filled(&view_term(&app)),
        0,
        "the LIVE scrollback must be the one that got cleared"
    );
}

/// `#{history_size}` and `#{history_bytes}` answer from the LIVE grid even
/// while copy mode shows a snapshot, as tmux's format_cb_history_size does
/// (format.c: `ft->wp->base.grid->hsize`, never the copy-mode grid). A script
/// polling the variable during copy mode must see history keep growing.
#[test]
fn history_size_reports_the_live_grid_while_copy_mode_shows_a_snapshot() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "before", 0, 60);
    let before = crate::format::expand_format("#{history_size}", &app)
        .parse::<usize>()
        .expect("history_size is a number");
    assert!(before > 0, "the pane needs retained history before copy mode");

    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(parked_live_term(&app).is_some(), "a snapshot must be installed");
    let frozen = filled(&view_term(&app));
    feed(&live, "arrived", 0, 60);

    let during = crate::format::expand_format("#{history_size}", &app)
        .parse::<usize>()
        .expect("history_size is a number");
    assert!(
        during > before,
        "history_size must follow the live grid in copy mode: before {before}, during {during}"
    );
    assert_eq!(
        filled(&view_term(&app)),
        frozen,
        "the snapshot itself must not have grown"
    );
    let bytes = crate::format::expand_format("#{history_bytes}", &app)
        .parse::<usize>()
        .expect("history_bytes is a number");
    assert!(bytes > 0, "history_bytes must read the live grid too");
}
