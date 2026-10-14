//! Server-side handlers for cross-session pane forwarding.
//!
//! Extracted into its own module to keep server/mod.rs small.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc};
use std::sync::atomic::AtomicBool;

use crate::types::{AppState, ForwardedPane, Node, LayoutKind};
use crate::tree;

/// Handle `PaneForwardExtract`: extract a pane from the window tree, keep
/// its real ConPTY alive, start a TCP forwarding listener, and reply with
/// connection info so the target session can connect.
pub fn handle_pane_forward_extract(
    app: &mut AppState,
    target: &crate::types::TempTarget,
    resp: mpsc::Sender<String>,
) {
    // The window is a window INDEX (it used to be read as a position in the
    // window list) and a pane index past the end is an error (it used to
    // fall back to the active pane and move that one instead).
    let (win_idx, pane_pos) = match crate::commands::resolve_join_end(app, target) {
        Ok(r) => r,
        Err(e) => {
            let _ = resp.send(format!("ERR {}", e));
            return;
        }
    };
    let src_path = match pane_pos.and_then(|p| tree::pane_paths(&app.windows[win_idx].root).get(p).cloned()) {
        Some(p) => p,
        None => app.windows[win_idx].active_path.clone(),
    };
    // Unzoom if needed
    if let Some(saved) = app.windows[win_idx].zoom_saved.take() {
        let win = &mut app.windows[win_idx];
        for (p, sz) in saved.into_iter() {
            if let Some(Node::Split { sizes, .. }) = tree::get_split_mut(&mut win.root, &p) {
                *sizes = sz;
            }
        }
    }
    // Extract the pane node from the tree
    let src_root = std::mem::replace(
        &mut app.windows[win_idx].root,
        Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
    );
    let (remaining, extracted) = tree::extract_node(src_root, &src_path);
    let pane_node = match extracted {
        Some(n) => n,
        None => {
            if let Some(rem) = remaining {
                app.windows[win_idx].root = rem;
            }
            let _ = resp.send("ERR pane not found".to_string());
            return;
        }
    };
    // Restore remaining tree (or remove empty window)
    let src_empty = remaining.is_none();
    if let Some(rem) = remaining {
        app.windows[win_idx].root = rem;
        app.windows[win_idx].active_path = tree::first_leaf_path(&app.windows[win_idx].root);
    }
    if src_empty {
        app.windows.remove(win_idx);
        app.on_window_removed(win_idx);
        if app.active_idx >= app.windows.len() {
            app.active_idx = app.windows.len().saturating_sub(1);
        }
    }
    // Unwrap the leaf into its Pane
    let pane = match pane_node {
        Node::Leaf(p) => p,
        _ => {
            let _ = resp.send("ERR extracted non-leaf".to_string());
            return;
        }
    };
    // Capture metadata before consuming the pane
    let pid = pane.child_pid;
    let title = pane.title.clone();
    let rows = pane.last_rows;
    let cols = pane.last_cols;
    // Capture full screen state (with colors, attributes, cursor) as VT escape codes
    let screen_b64 = {
        if let Ok(parser) = pane.term.lock() {
            let buf = parser.screen().state_formatted();
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(&buf)
        } else {
            String::new()
        }
    };
    // Start TCP forwarding listener
    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            let _ = resp.send(format!("ERR bind: {}", e));
            return;
        }
    };
    let listen_port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let shutdown = Arc::new(AtomicBool::new(false));
    let fwd_id = app.next_forward_id;
    app.next_forward_id += 1;
    // The writer was already taken during pane creation and stored in pane.writer
    let pty_writer = pane.writer;
    let pane_local_id = pane.id;
    // Start the forwarding thread.
    //
    // PTY output -> TCP does NOT clone a second reader: the pane's original
    // reader thread (spawn_reader_thread) keeps sole ownership of the ConPTY
    // output pipe. A cloned reader raced it for every chunk and the tunnel
    // starved, so the proxy pane on the target session stayed permanently
    // blank. Instead the tunnel registers as a tee writer in PIPE_WRITERS
    // (the pipe-pane mechanism): the original reader forwards every chunk it
    // reads, and drops the entry when a write fails after the tunnel closes.
    // Output between the screen snapshot above and the accept below goes only
    // to the detached local parser; the snapshot already covers the visible
    // screen at extract time.
    let sd_clone = shutdown.clone();
    let accepted = Arc::new(AtomicBool::new(false));
    let tunnel_closed = Arc::new(AtomicBool::new(false));
    let (accepted_t, closed_t) = (accepted.clone(), tunnel_closed.clone());
    std::thread::spawn(move || {
        // Accept one connection for I/O forwarding
        if let Ok((stream, _)) = listener.accept() {
            accepted_t.store(true, std::sync::atomic::Ordering::Release);
            let _ = stream.set_nodelay(true);
            match stream.try_clone() {
                Ok(tcp_writer) => {
                    if let Ok(mut writers) = crate::types::PIPE_WRITERS.lock() {
                        writers.push((pane_local_id, Box::new(tcp_writer)));
                        crate::types::PIPE_PANE_COUNT
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Err(_) => {
                    closed_t.store(true, std::sync::atomic::Ordering::Release);
                    return;
                }
            }
            // Writer: TCP -> PTY input (same 64K buffer as the reader side for
            // symmetric throughput)
            let mut tcp_reader = stream;
            let mut pty_writer = pty_writer;
            let mut buf = [0u8; 65536];
            loop {
                if sd_clone.load(std::sync::atomic::Ordering::Relaxed) { break; }
                match tcp_reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if pty_writer.write_all(&buf[..n]).is_err() { break; }
                        let _ = pty_writer.flush();
                    }
                    Err(_) => break,
                }
            }
            // Tunnel is down. Fully close the socket so the next tee write
            // fails and the pane's reader thread unregisters the entry and
            // re-syncs the PIPE_PANE_COUNT gate.
            let _ = tcp_reader.shutdown(std::net::Shutdown::Both);
            // Nobody can reach the pane any more: the target dropped the
            // proxy or its server ended. The reaper ends the child.
            closed_t.store(true, std::sync::atomic::Ordering::Release);
        }
    });
    // Store forwarded pane state
    app.forwarded_panes.insert(fwd_id, ForwardedPane {
        master: pane.master,
        child: pane.child,
        listener_port: listen_port,
        pid,
        title: title.clone(),
        rows,
        cols,
        shutdown,
        accepted,
        tunnel_closed,
        created: std::time::Instant::now(),
    });
    // Send response
    let title_wire = title.replace(' ', "\x01");
    let response = format!(
        "FORWARD {} {} {} {} {} {} {}",
        fwd_id, listen_port, pid.unwrap_or(0), title_wire, rows, cols, screen_b64.len(),
    );
    if screen_b64.is_empty() {
        let _ = resp.send(response);
    } else {
        let _ = resp.send(format!("{}\n{}", response, screen_b64));
    }
}

/// Handle `PaneForwardInject`: create a proxy pane that tunnels I/O to a
/// source session's forwarded pane, then graft it into the window tree.
pub fn handle_pane_forward_inject(
    app: &mut AppState,
    source_session: String,
    source_addr: String,
    source_key: String,
    forward_id: u64,
    fwd_port: u16,
    pid: u32,
    title: String,
    rows: u16,
    cols: u16,
    screen_b64: String,
    target: &crate::types::TempTarget,
    horizontal: bool,
) -> Result<(), String> {
    // Resolve the target BEFORE connecting: the pane goes next to the pane
    // the join named (it used to go next to the active one, the target was
    // dropped on the way here), and a target that does not resolve is an
    // error, not a graft somewhere else.
    let (tgt_idx, tgt_pos) = crate::commands::resolve_join_end(app, target)?;
    // Connect to the forwarding listener on the source session
    let fwd_addr = format!("127.0.0.1:{}", fwd_port);
    let stream = match TcpStream::connect(&fwd_addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("psmux: cross-session inject connect failed: {}", e);
            return Err(format!("connect to the source pane failed: {}", e));
        }
    };
    let _ = stream.set_nodelay(true);
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("psmux: cross-session inject clone failed: {}", e);
            return Err(format!("clone failed: {}", e));
        }
    };
    let writer_stream = stream;
    // Decode screen snapshot
    let screen_snapshot = if !screen_b64.is_empty() {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(&screen_b64).ok()
    } else {
        None
    };
    // Allocate the pane id the way a cold spawn does. It used to be one more
    // than the highest id in the tree, which is not what the allocator hands
    // out next: ids of killed panes and of spares already issued sit above it,
    // so the next split or new-window could reuse the proxy's id and two panes
    // answered to the same `%N`. Pane ids are per server today, so the joined
    // pane does not keep the id it had in the source session (tmux keeps it);
    // see docs/design/multi-session-server.md.
    let pane_id = {
        let dropped = app.warm_pane.set_issued_floor(app.next_pane_id);
        if dropped > 0 {
            crate::warm_trace!(
                "pool: id floor raised to {} by a cross session join, discarded {} spare(s)",
                app.next_pane_id, dropped
            );
        }
        let id = app.next_pane_id;
        app.next_pane_id += 1;
        id
    };
    // Create the proxy pane
    let proxy_pane = match crate::proxy_pane::create_proxy_pane(
        reader_stream,
        writer_stream,
        source_addr.clone(),
        source_key,
        source_session,
        forward_id,
        if pid > 0 { Some(pid) } else { None },
        title,
        rows,
        cols,
        pane_id,
        screen_snapshot,
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("psmux: create_proxy_pane failed: {}", e);
            return Err(format!("create_proxy_pane failed: {}", e));
        }
    };
    // Start the reader thread (same as normal panes)
    let reader = match proxy_pane.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("psmux: proxy reader clone failed: {}", e);
            return Err(format!("proxy reader clone failed: {}", e));
        }
    };
    crate::pane::spawn_reader_thread(
        reader,
        proxy_pane.term.clone(),
        proxy_pane.data_version.clone(),
        proxy_pane.cursor_shape.clone(),
        proxy_pane.bell_pending.clone(),
        proxy_pane.cpr_pending.clone(),
        proxy_pane.color_query_pending.clone(),
        proxy_pane.output_ring.clone(),
        pane_id,
        proxy_pane.child_pid,
    );
    // Graft into the target window tree, next to the resolved target pane.
    if tgt_idx < app.windows.len() {
        let tgt_path = match tgt_pos.and_then(|p| tree::pane_paths(&app.windows[tgt_idx].root).get(p).cloned()) {
            Some(p) => p,
            None => app.windows[tgt_idx].active_path.clone(),
        };
        let split_kind = if horizontal { LayoutKind::Horizontal } else { LayoutKind::Vertical };
        // Cross session join-pane has no -d: the joined pane becomes active,
        // and active_path is re-anchored on it instead of on the split the
        // graft created (#725).
        if let Some(g) = tree::graft_pane(&mut app.windows[tgt_idx], &tgt_path, split_kind, Node::Leaf(proxy_pane), false, true) {
            if let Some(prev) = g.prev_active_path { app.last_pane_path = prev; }
        }
        app.active_idx = tgt_idx;
    }
    Ok(())
}

// ── Lifetime of a forwarded pane ────────────────────────────────────────
//
// A pane joined into another session keeps its real ConPTY and child here,
// in the source server, and the target holds a proxy (src/proxy_pane.rs).
// In tmux the pane simply belongs to the target from then on: the source
// session can be destroyed (its last window went, or kill-session) and the
// pane lives on. Here that means this process must keep hosting the pane
// after its own session is gone, and must not outlive the pane either.

/// How long an extracted pane waits for the target to connect its tunnel.
/// A join whose inject never arrived leaves a pane nobody can see; after this
/// it is ended rather than kept as an invisible shell.
pub(crate) const FORWARD_ACCEPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the reaper does with one forwarded pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForwardVerdict {
    /// Still in use by the target.
    Keep,
    /// The child exited by itself: drop the entry (the target learns it from
    /// `pane-forward-status`, which answers `exited` for an unknown id).
    ChildExited,
    /// The tunnel closed, or never opened in time: nobody can reach the pane,
    /// so end its child and drop the entry.
    TunnelGone,
}

pub(crate) fn forward_verdict(
    child_exited: bool,
    accepted: bool,
    tunnel_closed: bool,
    age: std::time::Duration,
) -> ForwardVerdict {
    if child_exited {
        ForwardVerdict::ChildExited
    } else if tunnel_closed || (!accepted && age >= FORWARD_ACCEPT_TIMEOUT) {
        ForwardVerdict::TunnelGone
    } else {
        ForwardVerdict::Keep
    }
}

/// Drop forwarded panes that are over (see [`ForwardVerdict`]). Returns how
/// many were dropped.
pub(crate) fn reap_forwarded_panes(app: &mut AppState) -> usize {
    use std::sync::atomic::Ordering;
    if app.forwarded_panes.is_empty() {
        return 0;
    }
    let mut over: Vec<(u64, ForwardVerdict)> = Vec::new();
    for (id, fp) in app.forwarded_panes.iter_mut() {
        let exited = matches!(fp.child.try_wait(), Ok(Some(_)));
        let v = forward_verdict(
            exited,
            fp.accepted.load(Ordering::Acquire),
            fp.tunnel_closed.load(Ordering::Acquire),
            fp.created.elapsed(),
        );
        if v != ForwardVerdict::Keep {
            over.push((*id, v));
        }
    }
    for (id, v) in &over {
        if let Some(mut fp) = app.forwarded_panes.remove(id) {
            fp.shutdown.store(true, Ordering::Relaxed);
            if *v == ForwardVerdict::TunnelGone {
                let _ = fp.child.kill();
            }
        }
    }
    over.len()
}

/// `pane-forward-status`: `exited` once the child is gone or the id is unknown.
pub(crate) fn forward_status(app: &mut AppState, fwd_id: u64) -> String {
    match app.forwarded_panes.get_mut(&fwd_id) {
        Some(fp) => match fp.child.try_wait() {
            Ok(None) => "running".to_string(),
            _ => "exited".to_string(),
        },
        None => "exited".to_string(),
    }
}

pub(crate) fn forward_resize(app: &mut AppState, fwd_id: u64, rows: u16, cols: u16) {
    if let Some(fp) = app.forwarded_panes.get(&fwd_id) {
        let _ = fp.master.resize(portable_pty::PtySize {
            rows, cols, pixel_width: 0, pixel_height: 0,
        });
    }
}

pub(crate) fn forward_kill(app: &mut AppState, fwd_id: u64) {
    if let Some(mut fp) = app.forwarded_panes.remove(&fwd_id) {
        fp.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = fp.child.kill();
    }
}

/// End every forwarded pane's child (server shutdown).
pub(crate) fn kill_all_forwarded(app: &mut AppState) {
    for (_, mut fp) in app.forwarded_panes.drain() {
        fp.shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = fp.child.kill();
    }
}

/// This session is ending (its last window went, kill-session, or
/// destroy-unattached) while it still hosts panes joined into other sessions.
///
/// The session goes away exactly as it would have: its registry files are
/// removed, its clients are detached, its own panes and spares are ended and
/// its name is released, so `ls` no longer lists it and a new session may take
/// the name. The process stays up only to serve the forwarded panes and exits
/// once the last of them is over. It used to exit on the spot, which ended the
/// shells the user was looking at in the other session.
pub(crate) fn become_forward_host(
    app: &mut AppState,
    session_guard: Option<crate::platform::SessionMutex>,
) -> ! {
    let base = app.port_file_base();
    crate::session::remove_session_registry_files(std::path::Path::new(
        &crate::paths::port_file(&base),
    ));
    // No longer a server this data dir lists, so not one the orphan reaper
    // may claim either: without its marker the reaper leaves it alone.
    let _ = std::fs::remove_file(crate::paths::server_marker_file(std::process::id()));
    if !app.control_clients.is_empty() {
        crate::control::emit_notification(
            app,
            crate::types::ControlNotification::Exit { reason: None },
        );
        std::thread::sleep(std::time::Duration::from_millis(80));
    }
    crate::types::send_directive_to_all_clients("DETACH");
    std::thread::sleep(std::time::Duration::from_millis(50));
    crate::types::shutdown_persistent_streams();
    tree::kill_all_children_batch(&mut app.windows);
    app.warm_pane.kill_all();
    crate::warm_pane_sync::reap_inflight_spares();
    // Release the session name for a new session of the same name.
    drop(session_guard);
    run_forward_host(app)
}

/// The loop of a process that only hosts forwarded panes. It answers the
/// proxies' control requests and nothing else, and exits when no forwarded
/// pane is left.
fn run_forward_host(app: &mut AppState) -> ! {
    use crate::types::CtrlReq;
    let rx = app.control_rx.take();
    loop {
        if let Some(rx) = rx.as_ref() {
            if let Ok(req) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
                let mut pending = vec![req];
                while let Ok(r) = rx.try_recv() {
                    pending.push(r);
                }
                for req in pending {
                    match req {
                        CtrlReq::PaneForwardResize(id, rows, cols) => forward_resize(app, id, rows, cols),
                        CtrlReq::PaneForwardStatus(id, resp) => {
                            let _ = resp.send(forward_status(app, id));
                        }
                        CtrlReq::PaneForwardKill(id) => forward_kill(app, id),
                        CtrlReq::KillServer | CtrlReq::KillServerScoped(_) => {
                            kill_all_forwarded(app);
                        }
                        // Anything else was meant for the session that is gone.
                        _ => {}
                    }
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        reap_forwarded_panes(app);
        if app.forwarded_panes.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::process::exit(0);
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_joinpane_proxy_ownership.rs"]
mod tests_joinpane_proxy_ownership;
