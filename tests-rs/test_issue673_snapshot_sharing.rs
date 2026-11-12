// Issue #673, both halves.
//
// PART 1, memory.  Copy mode reads a snapshot of the pane's screen (#671,
// tmux's `window_copy_init`), and psmux built that snapshot by deep-cloning the
// whole parser, scrollback included.  Measured against the merged tree on a 200
// column pane with `history-limit 60000` filled to `history_size 49953`:
//
//     before copy mode   210.8 MB working set / 206.0 MB private
//     in copy mode       406.1 MB private       (+200.1 MB)
//     live history       189.7 MB  (#{history_bytes})
//
// so entering copy mode cost 105.5% of the history it was about to read, and 30
// enter/leave cycles left the server resting at 1.307x where it started.
//
// A history row is immutable: it becomes history at the moment it leaves the
// visible grid, and after that it is only ever trimmed off the old end or
// dropped with the whole history.  `Grid::set_size` deliberately resizes only
// the visible rows, so a history row even keeps the width #641 compacted it to.
// So the snapshot can SHARE the rows through an `Arc` and copy only the visible
// screen, which is what these tests pin: shared not duplicated, a trim on the
// live grid leaves the snapshot intact, and output after the snapshot does not
// leak into it.
//
// PART 2, scope.  `sync_copy_snapshot` reconciled by `app.mode` and the active
// pane only, but psmux allows more than one pane in copy mode at once (#607).
// A pane put into copy mode with `copy-mode -t` and then left in the background
// had its snapshot torn down on the very next frame, after which its own output
// pushed the view it was anchored to: measured end to end, the non active
// pane's `#{scroll_position}` fell from 785 to 0 while it printed.

use super::*;

use super::test_copy_mode_snapshot::{app_with_pane, feed, filled, pane_of, view_term};

fn tcp_pair() -> (std::net::TcpStream, std::net::TcpStream) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind listener");
    let addr = listener.local_addr().expect("listener addr");
    let accept = std::thread::spawn(move || listener.accept().expect("accept").0);
    let client = std::net::TcpStream::connect(addr).expect("connect");
    let server = accept.join().expect("join accept thread");
    (client, server)
}

/// How many of a parser's history rows are held by more than one grid.
fn shared(term: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>) -> usize {
    term.lock()
        .map(|p| p.screen().shared_scrollback_rows())
        .unwrap_or(0)
}

/// The text of the row `offset` lines above the live bottom, read through the
/// scrollback offset the way the copy-mode view reads it.
fn row_at_offset(
    term: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>,
    offset: usize,
) -> String {
    let mut parser = term.lock().expect("lock parser");
    let was = parser.screen().scrollback();
    parser.screen_mut().set_scrollback(offset);
    let text = parser.screen().contents();
    parser.screen_mut().set_scrollback(was);
    text.lines().next().unwrap_or_default().to_string()
}

/// The whole point: the snapshot must share the history rows rather than copy
/// them.  Sharing is observable through the `Arc` strong counts.
#[test]
fn entering_copy_mode_shares_history_rows_instead_of_duplicating_them() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "history", 0, 500);

    let retained = filled(&live);
    assert!(retained > 100, "fixture must build a real history, got {retained}");
    assert_eq!(
        shared(&live),
        0,
        "nothing shares the pane's history before copy mode is entered"
    );

    crate::copy_mode::enter_copy_mode(&mut app);
    let view = view_term(&app);

    assert_eq!(
        filled(&view),
        retained,
        "the snapshot must hold the same history the pane had"
    );
    assert_eq!(
        shared(&view),
        retained,
        "every history row in the snapshot must be shared with the live grid, \
         not copied: this is the {retained} row deep clone that cost 200 MB"
    );
    assert_eq!(
        shared(&live),
        retained,
        "the live grid must see the same rows as shared"
    );
}

/// Sharing must not make the two grids the same grid: the snapshot is still a
/// frozen view, so output arriving afterwards must not reach it.
#[test]
fn output_after_the_snapshot_does_not_leak_into_it() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "before", 0, 500);

    crate::copy_mode::enter_copy_mode(&mut app);
    let view = view_term(&app);
    let retained = filled(&view);
    let top_of_snapshot = row_at_offset(&view, retained);

    // The application keeps printing under the reader.
    feed(&live, "after", 0, 4000);

    assert_eq!(
        filled(&view),
        retained,
        "the snapshot's history must not grow with the live pane's output"
    );
    assert!(
        filled(&live) > 0,
        "the live grid must have taken the new output"
    );
    assert_eq!(
        row_at_offset(&view, retained),
        top_of_snapshot,
        "the row the snapshot is anchored to must still read the same text"
    );
    assert!(
        top_of_snapshot.contains("before"),
        "the snapshot must still be showing the pre copy mode content, got {top_of_snapshot:?}"
    );
    assert!(
        !top_of_snapshot.contains("after"),
        "output printed after the snapshot leaked into it: {top_of_snapshot:?}"
    );
}

/// The live grid trimming its oldest rows away must not disturb the snapshot's
/// view of them.  The snapshot holds its own references, so a row the live grid
/// has evicted stays alive and readable for as long as the reader needs it.
#[test]
fn trimming_the_live_grid_leaves_the_snapshot_intact() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "old", 0, 400);

    crate::copy_mode::enter_copy_mode(&mut app);
    let view = view_term(&app);
    let retained = filled(&view);
    let oldest = row_at_offset(&view, retained);
    assert!(
        oldest.contains("old"),
        "fixture must anchor on a row that will be trimmed, got {oldest:?}"
    );

    // Shrink the LIVE history hard: every row the snapshot is reading is now
    // gone from the live grid.
    if let Ok(mut parser) = live.lock() {
        parser.screen_mut().set_scrollback_len(5);
    }
    assert!(
        filled(&live) <= 5,
        "the live grid must really have been trimmed, got {}",
        filled(&live)
    );

    assert_eq!(
        filled(&view),
        retained,
        "the snapshot must keep every row it had when copy mode started"
    );
    assert_eq!(
        row_at_offset(&view, retained),
        oldest,
        "a row trimmed from the live grid must still read correctly in the snapshot"
    );
    // Those rows are now the snapshot's alone.
    assert_eq!(
        shared(&view),
        filled(&live),
        "only the rows the live grid still holds may count as shared"
    );
}

/// Clearing the live history is the other way rows go away, and it must not
/// reach the snapshot either.  `clear-history` in copy mode clears the LIVE
/// grid (tmux cmd-capture-pane.c:418).
#[test]
fn clearing_the_live_history_leaves_the_snapshot_intact() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "kept", 0, 300);

    crate::copy_mode::enter_copy_mode(&mut app);
    let view = view_term(&app);
    let retained = filled(&view);
    let oldest = row_at_offset(&view, retained);

    // How an application clears the pane's history from inside: CSI 3 J, the
    // sequence `clear` sends.  It reaches the LIVE parser, never the snapshot.
    if let Ok(mut parser) = live.lock() {
        parser.process(b"[3J");
    }
    assert_eq!(filled(&live), 0, "the live history must be gone");

    assert_eq!(
        filled(&view),
        retained,
        "the snapshot must survive a clear-history on the live grid"
    );
    assert_eq!(
        row_at_offset(&view, retained),
        oldest,
        "the snapshot's oldest row must still read correctly after the live clear"
    );
    assert_eq!(
        shared(&view),
        0,
        "with the live history cleared the snapshot owns its rows outright"
    );
}

/// Leaving copy mode must release everything: the snapshot goes, and the live
/// grid stops sharing its rows with anyone.
#[test]
fn leaving_copy_mode_releases_the_snapshot_and_its_rows() {
    let mut app = app_with_pane();
    let live = view_term(&app);
    feed(&live, "line", 0, 400);
    let retained = filled(&live);

    crate::copy_mode::enter_copy_mode(&mut app);
    assert_eq!(shared(&live), retained, "rows must be shared while copy mode is up");

    crate::copy_mode::exit_copy_mode(&mut app);

    let after = view_term(&app);
    assert!(
        std::sync::Arc::ptr_eq(&after, &live),
        "leaving copy mode must put the live parser back"
    );
    assert!(
        pane_of(&app).expect("active pane").live_term.is_none(),
        "the parked live parser must be handed back, not leaked"
    );
    assert!(
        pane_of(&app).expect("active pane").copy_state.is_none(),
        "the pane's own copy state must be cleared"
    );
    assert_eq!(
        shared(&live),
        0,
        "with the snapshot dropped nothing may still be holding the history rows"
    );
}

// --- part 2: more than one pane in copy mode ---------------------------------

/// Add a second proxy pane beside the first, splitting the window, and return
/// the ids of both panes in tree order.
fn split_in_two(app: &mut AppState) -> (usize, usize) {
    let (reader, _peer_reader) = tcp_pair();
    let (writer, _peer_writer) = tcp_pair();
    // Keep the peers alive for the life of the test: a proxy pane whose socket
    // dies reads EOF and the pane goes with it.
    std::mem::forget(_peer_reader);
    std::mem::forget(_peer_writer);
    let second = crate::proxy_pane::create_proxy_pane(
        reader,
        writer,
        "127.0.0.1:1".to_string(),
        "test-key".to_string(),
        "test-session".to_string(),
        0,
        None,
        "test-pane-2".to_string(),
        24,
        80,
        8,
        None,
    )
    .expect("create second proxy pane");

    let win = &mut app.windows[0];
    let first = std::mem::replace(
        &mut win.root,
        crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![50, 50],
            children: vec![],
        },
    );
    let first_id = match &first {
        crate::types::Node::Leaf(p) => p.id,
        _ => unreachable!("fixture root must be a single leaf"),
    };
    let second_id = second.id;
    if let crate::types::Node::Split { children, .. } = &mut win.root {
        children.push(first);
        children.push(crate::types::Node::Leaf(second));
    }
    win.active_path = vec![0];
    (first_id, second_id)
}

fn term_of_pane(app: &AppState, id: usize) -> std::sync::Arc<std::sync::Mutex<vt100::Parser>> {
    let mut found = None;
    let win = &app.windows[0];
    crate::tree::for_each_pane(&win.root, &mut |p: &crate::types::Pane| {
        if p.id == id {
            found = Some(p.term.clone());
        }
    });
    found.expect("pane must exist")
}

fn has_snapshot(app: &AppState, id: usize) -> bool {
    let mut found = false;
    let win = &app.windows[0];
    crate::tree::for_each_pane(&win.root, &mut |p: &crate::types::Pane| {
        if p.id == id {
            found = p.live_term.is_some();
        }
    });
    found
}

/// The scope regression: two panes in copy mode at once must BOTH hold a
/// snapshot, and the reconciler must not tear down the background one.
#[test]
fn two_panes_in_copy_mode_both_keep_their_snapshot() {
    let mut app = app_with_pane();
    let (first, second) = split_in_two(&mut app);

    // Pane one enters copy mode and then loses focus, which is what
    // `copy-mode -t` on a background pane amounts to.
    app.windows[0].active_path = vec![0];
    let first_live = term_of_pane(&app, first);
    feed(&first_live, "one", 0, 300);
    crate::copy_mode::enter_copy_mode(&mut app);
    assert!(has_snapshot(&app, first), "the pane that just entered must hold a snapshot");

    // Focus moves to pane two, which then enters copy mode itself.
    crate::copy_mode::switch_with_copy_save(&mut app, |app| {
        app.windows[0].active_path = vec![1];
    });
    let second_live = term_of_pane(&app, second);
    feed(&second_live, "two", 0, 300);
    crate::copy_mode::enter_copy_mode(&mut app);

    // The per-frame reconciliation runs.  This is where the background pane
    // used to lose its snapshot.
    crate::copy_mode::sync_copy_snapshot(&mut app);

    assert!(
        has_snapshot(&app, second),
        "the active pane in copy mode must hold a snapshot"
    );
    assert!(
        has_snapshot(&app, first),
        "the NON active pane in copy mode must hold its own snapshot too: \
         sync_copy_snapshot used to call leave_copy_snapshot on it every frame"
    );

    // And the background pane's view must survive its own output.
    let first_view = term_of_pane(&app, first);
    let retained = filled(&first_view);
    let top = row_at_offset(&first_view, retained);
    feed(&first_live, "noise", 0, 2000);
    crate::copy_mode::sync_copy_snapshot(&mut app);

    assert!(
        has_snapshot(&app, first),
        "the background pane must still hold its snapshot after its own output"
    );
    assert_eq!(
        filled(&first_view),
        retained,
        "the background pane's frozen view must not churn under its own output"
    );
    assert_eq!(
        row_at_offset(&first_view, retained),
        top,
        "the background pane's anchored row must still read the same text"
    );
    assert!(
        top.contains("one") && !top.contains("noise"),
        "the background pane's view leaked new output: {top:?}"
    );
}

/// A pane that is not in copy mode must not be given a snapshot, however the
/// reconciler is called: a frozen background pane would stop redrawing.
#[test]
fn a_pane_not_in_copy_mode_is_never_snapshotted() {
    let mut app = app_with_pane();
    let (first, second) = split_in_two(&mut app);

    app.windows[0].active_path = vec![0];
    crate::copy_mode::enter_copy_mode(&mut app);
    crate::copy_mode::sync_copy_snapshot(&mut app);
    assert!(has_snapshot(&app, first));
    assert!(
        !has_snapshot(&app, second),
        "a pane that never entered copy mode must stay on its live screen"
    );

    // Leaving releases the first pane too, and repeated reconciliation is
    // idempotent.
    crate::copy_mode::exit_copy_mode(&mut app);
    crate::copy_mode::sync_copy_snapshot(&mut app);
    crate::copy_mode::sync_copy_snapshot(&mut app);
    assert!(!has_snapshot(&app, first), "leaving must release the snapshot");
    assert!(!has_snapshot(&app, second));
}
