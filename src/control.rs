use crate::types::{AppState, ControlNotification, Node, LayoutKind, Window};
use ratatui::layout::Rect;

/// Compute tmux's 16-bit rotating checksum over the layout body.
/// Matches `layout_checksum()` in tmux's layout-custom.c.
fn layout_checksum(s: &str) -> u16 {
    let mut csum: u16 = 0;
    for b in s.bytes() {
        csum = (csum >> 1) | ((csum & 1) << 15);
        csum = csum.wrapping_add(b as u16);
    }
    csum
}

/// Recursively serialise a Node tree into tmux's custom layout format body
/// (without the leading checksum). Format reference (tmux/layout-custom.c):
///   Leaf:           WxH,X,Y,paneid
///   Side-by-side:   WxH,X,Y{child1,child2,...}     (LayoutKind::Horizontal)
///   Stacked:        WxH,X,Y[child1,child2,...]     (LayoutKind::Vertical)
///
/// `saved` is a zoomed window's `zoom_saved`: the real split sizes along the
/// zoomed pane's path, which the live tree has replaced with 100/0 while the
/// window is zoomed.  `path` is the child index path of `node`.
fn append_layout_body(node: &Node, area: Rect, saved: &[(Vec<usize>, Vec<u16>)], path: &mut Vec<usize>, out: &mut String) {
    match node {
        Node::Leaf(pane) => {
            out.push_str(&format!("{}x{},{},{},{}", area.width, area.height, area.x, area.y, pane.id));
        }
        Node::Split { kind, sizes, children } => {
            if children.is_empty() {
                out.push_str(&format!("{}x{},{},{}", area.width, area.height, area.x, area.y));
                return;
            }
            let sizes = saved.iter()
                .find(|(p, sz)| p == path && sz.len() == children.len())
                .map(|(_, sz)| sz)
                .unwrap_or(sizes);
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let is_horizontal = matches!(*kind, LayoutKind::Horizontal);
            let rects = crate::tree::split_with_gaps(is_horizontal, &effective_sizes, area);
            out.push_str(&format!("{}x{},{},{}", area.width, area.height, area.x, area.y));
            let (open, close) = if is_horizontal { ('{', '}') } else { ('[', ']') };
            out.push(open);
            for (i, child) in children.iter().enumerate() {
                if i > 0 { out.push(','); }
                let r = rects.get(i).copied().unwrap_or(area);
                path.push(i);
                append_layout_body(child, r, saved, path, out);
                path.pop();
            }
            out.push(close);
        }
    }
}

fn checksummed(body: String) -> String {
    format!("{:04x},{}", layout_checksum(&body), body)
}

/// A window's #{window_layout} (`<csum>,<body>`): the real split, also while
/// the window is zoomed (tmux format_cb_window_layout dumps saved_layout_root
/// when it is set).  `area` is the target window's stored geometry.
pub fn window_layout_string(window: &Window, area: Rect) -> String {
    let mut body = String::new();
    let saved: &[(Vec<usize>, Vec<u16>)] = window.zoom_saved.as_deref().unwrap_or(&[]);
    append_layout_body(&window.root, area, saved, &mut Vec::new(), &mut body);
    checksummed(body)
}

/// A window's #{window_visible_layout}: what is on screen.  While zoomed that
/// is the zoomed pane alone at the window size (tmux window_zoom makes
/// layout_root layout_init(w, wp), and format_cb_window_visible_layout dumps
/// layout_root); otherwise it is the same as #{window_layout}.
pub fn window_visible_layout_string(window: &Window, area: Rect) -> String {
    if window.zoom_saved.is_some() {
        if let Some(id) = crate::tree::get_active_pane_id(&window.root, &window.active_path) {
            return checksummed(format!("{}x{},{},{},{}", area.width, area.height, area.x, area.y, id));
        }
    }
    let mut body = String::new();
    append_layout_body(&window.root, area, &[], &mut Vec::new(), &mut body);
    checksummed(body)
}

/// The `%layout-change` for window `win_idx`, as tmux
/// control_window_layout_changed_cb sends it: "#{window_id} #{window_layout}
/// #{window_visible_layout} #{window_raw_flags}".
pub fn layout_change(app: &AppState, win_idx: usize, area: Rect) -> Option<ControlNotification> {
    let window = app.windows.get(win_idx)?;
    Some(ControlNotification::LayoutChange {
        window_id: window.id,
        layout: window_layout_string(window, area),
        visible_layout: window_visible_layout_string(window, area),
        flags: crate::format::window_raw_flags(app, win_idx),
    })
}

/// A pane of the window with this id was killed. If the window stays, tmux
/// sends `%layout-change` (layout_close_pane, notify_window
/// "window-layout-changed"), then `%window-pane-changed` when the killed pane
/// was the window's active one (window_remove_pane picks the new active pane
/// after the layout closed). A window that closed gets `%window-close` from
/// the window list diff instead.
pub fn emit_pane_closed(app: &AppState, window_id: usize, killed_active: bool) {
    if app.control_clients.is_empty() {
        return;
    }
    if let Some(idx) = app.windows.iter().position(|w| w.id == window_id) {
        if let Some(n) = layout_change(app, idx, app.windows[idx].area) {
            emit_notification(app, n);
        }
        let w = &app.windows[idx];
        if killed_active {
            if let Some(pane_id) = crate::tree::get_active_pane_id(&w.root, &w.active_path) {
                emit_notification(app, ControlNotification::WindowPaneChanged { window_id, pane_id });
            }
        }
    }
}

/// Format a control mode notification as a tmux wire-compatible line.
pub fn format_notification(notif: &ControlNotification) -> String {
    match notif {
        ControlNotification::Output { pane_id, data } => {
            format!("%output %{} {}", pane_id, escape_output(data))
        }
        ControlNotification::WindowAdd { window_id } => {
            format!("%window-add @{}", window_id)
        }
        ControlNotification::WindowClose { window_id } => {
            format!("%window-close @{}", window_id)
        }
        ControlNotification::WindowRenamed { window_id, name } => {
            format!("%window-renamed @{} {}", window_id, name)
        }
        ControlNotification::WindowPaneChanged { window_id, pane_id } => {
            format!("%window-pane-changed @{} %{}", window_id, pane_id)
        }
        ControlNotification::LayoutChange { window_id, layout, visible_layout, flags } => {
            // tmux sends: %layout-change @WID layout visible_layout flags
            format!("%layout-change @{} {} {} {}", window_id, layout, visible_layout, flags)
        }
        ControlNotification::SessionChanged { session_id, name } => {
            format!("%session-changed ${} {}", session_id, name)
        }
        ControlNotification::SessionRenamed { name } => {
            format!("%session-renamed {}", name)
        }
        ControlNotification::SessionWindowChanged { session_id, window_id } => {
            format!("%session-window-changed ${} @{}", session_id, window_id)
        }
        ControlNotification::SessionsChanged => {
            "%sessions-changed".to_string()
        }
        ControlNotification::PaneModeChanged { pane_id } => {
            format!("%pane-mode-changed %{}", pane_id)
        }
        ControlNotification::ClientDetached { client } => {
            format!("%client-detached {}", client)
        }
        ControlNotification::Continue { pane_id } => {
            format!("%continue %{}", pane_id)
        }
        ControlNotification::Pause { pane_id } => {
            format!("%pause %{}", pane_id)
        }
        ControlNotification::ExtendedOutput { pane_id, age_ms, data } => {
            format!("%extended-output %{} {} : {}", pane_id, age_ms, escape_output(data))
        }
        // The writer thread normally turns this into %output or
        // %extended-output through ControlOutputGate::dequeue.
        ControlNotification::PaneOutput { pane_id, data, .. } => {
            format!("%output %{} {}", pane_id, escape_output(data))
        }
        ControlNotification::SubscriptionChanged { name, session_id, window_id, window_index, pane_id, value } => {
            format!("%subscription-changed {} ${} @{} {} %{} : {}", name, session_id, window_id, window_index, pane_id, value)
        }
        ControlNotification::Exit { reason } => {
            if let Some(r) = reason {
                format!("%exit {}", r)
            } else {
                "%exit".to_string()
            }
        }
        ControlNotification::Detached => "%exit".to_string(),
        ControlNotification::PasteBufferChanged { name } => {
            format!("%paste-buffer-changed {}", name)
        }
        ControlNotification::PasteBufferDeleted { name } => {
            format!("%paste-buffer-deleted {}", name)
        }
        ControlNotification::ClientSessionChanged { client, session_id, name } => {
            format!("%client-session-changed {} ${} {}", client, session_id, name)
        }
        ControlNotification::Message { text } => {
            format!("%message {}", text)
        }
    }
}

/// Escape non-printable bytes as octal \\NNN sequences (tmux compatible).
/// Printable ASCII (0x20..=0x7E), space, and tab are passed through.
/// Backslash is escaped as \\134 (octal) per the tmux protocol.
pub fn escape_output(data: &str) -> String {
    let mut out = String::with_capacity(data.len());
    for b in data.bytes() {
        match b {
            b'\\' => out.push_str("\\134"),
            0x20..=0x7E => out.push(b as char),
            b'\t' => out.push('\t'),
            _ => {
                out.push_str(&format!("\\{:03o}", b));
            }
        }
    }
    out
}

/// Format the %begin header for a command response.
pub fn format_begin(timestamp: i64, cmd_number: u64) -> String {
    format!("%begin {} {} 1", timestamp, cmd_number)
}

/// Format the %end footer for a successful command response.
pub fn format_end(timestamp: i64, cmd_number: u64) -> String {
    format!("%end {} {} 1", timestamp, cmd_number)
}

/// Format the %error footer for a failed command response.
pub fn format_error(timestamp: i64, cmd_number: u64) -> String {
    format!("%error {} {} 1", timestamp, cmd_number)
}

// The reply to the command on the control client's own command line
// (`-C attach`, `-C new-session -A`) is flagged 0, because the item did not
// come from the client's stdin: tmux sets the flag from CMDQ_STATE_CONTROL,
// which only control.c's stdin reader puts on an item (cmd-queue.c
// cmdq_fire_command, issue #857).

/// `%begin` of the reply to the command line command (flag 0).
pub fn format_initial_begin(timestamp: i64, cmd_number: u64) -> String {
    format!("%begin {} {} 0", timestamp, cmd_number)
}

/// `%end` of the reply to the command line command (flag 0).
pub fn format_initial_end(timestamp: i64, cmd_number: u64) -> String {
    format!("%end {} {} 0", timestamp, cmd_number)
}

/// `%error` of the reply to a failed command line command (flag 0).
pub fn format_initial_error(timestamp: i64, cmd_number: u64) -> String {
    format!("%error {} {} 0", timestamp, cmd_number)
}

/// The whole reply block a control connection opens with, before any
/// notification: tmux answers the command line command first. `-CC` puts the
/// DCS opener directly in front of it with no newline (control.c
/// control_start), which iTerm2 needs to see before `%begin`.
pub fn initial_reply_block(timestamp: i64, dcs: bool) -> String {
    format!(
        "{}{}\n{}\n",
        if dcs { "\x1bP1000p" } else { "" },
        format_initial_begin(timestamp, 1),
        format_initial_end(timestamp, 1),
    )
}

/// Emit a control notification to all connected control mode clients.
/// Never drops: the channel is unbounded, and a client that falls behind is
/// handled by its output gate (pause-after, too far behind) instead (#875).
pub fn emit_notification(app: &AppState, notif: ControlNotification) {
    for client in app.control_clients.values() {
        if let ControlNotification::Output { pane_id, .. } = &notif {
            if client.paused_panes.contains(pane_id) {
                continue;
            }
        }
        let _ = client.notification_tx.send(notif.clone());
    }
}

/// Send a notification to a single control client by id (no-op if unknown).
pub fn emit_to_client(app: &AppState, client_id: u64, notif: ControlNotification) {
    if let Some(client) = app.control_clients.get(&client_id) {
        let _ = client.notification_tx.send(notif);
    }
}

/// The output rings of the panes in the window trees, the panes a control
/// client is sent `%output` for.
pub fn tree_rings(app: &AppState) -> Vec<(usize, crate::control_ring::Ring)> {
    let mut rings = Vec::new();
    for win in &app.windows {
        crate::tree::for_each_pane(&win.root, &mut |pane: &crate::types::Pane| {
            rings.push((pane.id, pane.output_ring.clone()));
        });
    }
    rings
}

/// A control client registered: its rings start being consumed, and if it
/// is the first one, what they held from before is thrown away (#875).
pub fn start_consuming_output(app: &AppState) {
    crate::control_ring::CONTROL_CLIENTS_PRESENT.store(true, std::sync::atomic::Ordering::Release);
    let rings = tree_rings(app);
    let refs: Vec<&crate::control_ring::Ring> = rings.iter().map(|r| &r.1).collect();
    crate::control_ring::set_consumed(&refs, true);
}

/// Take the panes' new output and queue it as `%output` for every control
/// client, once per server loop pass.
///
/// Nothing is dropped here: the client's channel is unbounded and its output
/// gate tracks how far behind it is, so a client that falls behind is paused
/// (pause-after) or cut off with `%exit too far behind`, as tmux does (#826).
/// When every attached client is a control client that already has
/// `CONTROL_QUEUE_HIGH` unwritten, the rings are left alone; they fill, and
/// the pane readers wait, which holds the programs on their writes, as tmux
/// stops reading a pane (server_client_check_pane_buffer, #875).
pub fn flush_pane_output(app: &mut AppState) {
    use std::sync::atomic::Ordering;
    let present = !app.control_clients.is_empty();
    let was = crate::control_ring::CONTROL_CLIENTS_PRESENT.swap(present, Ordering::AcqRel);
    if !present {
        if was {
            // The last control client left: stop consuming, so no reader
            // waits for a drain that will not come.
            crate::control_ring::set_consumed(&[], false);
        }
        return;
    }
    let rings = tree_rings(app);
    {
        let refs: Vec<&crate::control_ring::Ring> = rings.iter().map(|r| &r.1).collect();
        crate::control_ring::set_consumed(&refs, false);
    }
    let tui_attached = app.client_registry.values().any(|c| !c.is_control);
    let all_behind = !tui_attached
        && app.control_clients.values()
            .all(|c| c.output_gate.queued_bytes() >= CONTROL_QUEUE_HIGH);
    if all_behind {
        return;
    }
    let mut pane_outputs: Vec<(usize, String)> = Vec::new();
    for (pane_id, ring) in &rings {
        let bytes = crate::control_ring::drain(ring);
        if bytes.is_empty() {
            continue;
        }
        let data = String::from_utf8_lossy(&bytes).into_owned();
        for chunk in crate::control_ring::chunks(data) {
            pane_outputs.push((*pane_id, chunk));
        }
    }
    crate::control_ring::notify_drained();
    if pane_outputs.is_empty() {
        return;
    }
    // The client's output gate tracks how old the oldest unwritten output of
    // each pane is (tmux control_check_age): past pause-after the pane is
    // paused for that client and its backlog dropped, and a client without
    // pause-after that falls CONTROL_MAXIMUM_AGE behind is disconnected
    // (#826). The writer thread decides between %output and
    // %extended-output and measures the age.
    let now = std::time::Instant::now();
    for (pane_id, data) in &pane_outputs {
        for client in app.control_clients.values_mut() {
            if client.paused_panes.contains(pane_id) {
                continue;
            }
            match client.output_gate.enqueue_sized(*pane_id, now, data.len()) {
                GateEnqueue::Send(seq) => {
                    let sent = client.notification_tx.send(ControlNotification::PaneOutput {
                        pane_id: *pane_id,
                        data: data.clone(),
                        seq,
                        queued_at: now,
                    });
                    if sent.is_err() {
                        client.output_gate.unqueue_sized(*pane_id, seq, data.len());
                    }
                }
                GateEnqueue::Paused => {
                    // Wakes the writer, which prints the %pause.
                    let _ = client.notification_tx.send(ControlNotification::Pause { pane_id: *pane_id });
                }
                GateEnqueue::Skip | GateEnqueue::TooFarBehind => {}
            }
        }
    }
}

/// Emit the initial state burst to a freshly-attached control mode client.
/// This mirrors what real tmux sends right after `-CC attach` so that
/// integrations like iTerm2's tmux mode can bootstrap their UI without
/// having to poll. Without this, iTerm2 sits forever on `-CC attach`
/// because it expects %session-changed / %window-add up front.
pub fn emit_initial_state(app: &AppState, client_id: u64) {
    let _ = client_id;
    // Match real tmux's initial burst order exactly. tmux 3.x sends, in this order:
    //   %window-add @<id>          (one per window)
    //   %sessions-changed
    //   %session-changed $<id> <name>
    // followed only later by %output and any %layout-change as windows are
    // queried by the client. iTerm2's TmuxGateway *only* leaves the
    // "not accepting notifications" state once it sees %session-changed (which
    // is the one notification that bypasses the acceptNotifications_ gate),
    // so it must come AFTER %window-add (which would otherwise be dropped)
    // and we must NOT pre-send %layout-change / %session-window-changed /
    // %window-pane-changed — those are gated on acceptNotifications_=YES,
    // which only happens after the kickoff command sequence completes.
    //
    // Sending extra notifications before the gate opens is harmless (they're
    // silently dropped) but the order session-changed must come last is
    // required because that's what flips iTerm into the writable state and
    // triggers openWindowsInitial + kickOffTmuxForRestoration.

    // 1. %window-add @id for each window.
    for win in &app.windows {
        emit_to_client(
            app,
            client_id,
            ControlNotification::WindowAdd { window_id: win.id },
        );
    }

    // 2. %sessions-changed.
    emit_to_client(app, client_id, ControlNotification::SessionsChanged);

    // 3. %session-changed $id name — this is the one that wakes iTerm up.
    emit_to_client(
        app,
        client_id,
        ControlNotification::SessionChanged {
            session_id: app.session_id,
            name: app.session_name.clone(),
        },
    );
}

/// Detach a control client the way tmux does (server_client_detach): it
/// stops being a control client of this session at once, and its connection
/// writes a bare `%exit` and closes once the command in flight has answered.
/// Returns false for a client that is not a control client, which the caller
/// then disconnects as before.
///
/// Closing the stream here instead, as the TUI path does, cut the control
/// client off with no `%exit`, so it reported `server exited unexpectedly`
/// for a plain detach (#825).
pub fn detach_control_client(app: &mut AppState, client_id: u64) -> bool {
    match app.control_clients.remove(&client_id) {
        Some(client) => {
            // Dropping the client's sender after this ends its writer, so
            // the connection closes even when the channel is full.
            let _ = client.notification_tx.send(ControlNotification::Detached);
            true
        }
        None => false,
    }
}

/// Check if any control mode clients are connected.
pub fn has_control_clients(app: &AppState) -> bool {
    !app.control_clients.is_empty()
}

/// The session's window ids and its current window id, taken before a
/// command runs so the window notifications can name what really changed.
pub struct WindowSnapshot {
    pub ids: Vec<usize>,
    pub current: Option<usize>,
    /// Every pane in a mode, with the mode's name (`pane_modes`).
    pub pane_modes: Vec<(usize, &'static str)>,
}

pub fn window_snapshot(app: &AppState) -> WindowSnapshot {
    WindowSnapshot {
        ids: app.windows.iter().map(|w| w.id).collect(),
        current: app.windows.get(app.active_idx).map(|w| w.id),
        pane_modes: pane_modes(app),
    }
}

/// The panes that are in a mode and which mode, by the same rule as
/// #{pane_mode}: the focused pane's mode is `app.mode`, every other pane's is
/// the copy mode parked in its `copy_state`.
pub fn pane_modes(app: &AppState) -> Vec<(usize, &'static str)> {
    let focused = crate::copy_mode::active_pane_id(app);
    let focused_mode = if app.mode.in_copy() {
        Some("copy-mode")
    } else if matches!(app.mode, crate::types::Mode::ClockMode) {
        Some("clock-mode")
    } else {
        None
    };
    let mut out = Vec::new();
    for win in &app.windows {
        crate::tree::for_each_pane(&win.root, &mut |p| {
            let mode = if Some(p.id) == focused {
                focused_mode
            } else if p.copy_state.is_some() {
                Some("copy-mode")
            } else {
                None
            };
            if let Some(m) = mode { out.push((p.id, m)); }
        });
    }
    out.sort_unstable();
    out
}

/// `%pane-mode-changed %N` for every pane whose mode changed between two
/// snapshots: entered, left, or swapped for another (tmux fires
/// pane-mode-changed from window_pane_set_mode and window_pane_reset_mode,
/// control-notify.c control_pane_mode_changed_cb).
pub fn pane_mode_notifications(
    before: &[(usize, &'static str)],
    after: &[(usize, &'static str)],
) -> Vec<ControlNotification> {
    let mut ids: Vec<usize> = before.iter().filter(|e| !after.contains(e)).map(|e| e.0)
        .chain(after.iter().filter(|e| !before.contains(e)).map(|e| e.0))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter().map(|pane_id| ControlNotification::PaneModeChanged { pane_id }).collect()
}

/// The window notifications between two snapshots, in tmux's order: a window
/// that left the session first (session.c session_detach fires window-unlinked
/// before it picks a new current window), then the current window change, then
/// a window that joined (spawn.c selects the new window before it fires
/// window-linked).
pub fn window_change_notifications(
    before: &WindowSnapshot,
    after: &WindowSnapshot,
    session_id: usize,
) -> Vec<ControlNotification> {
    let mut out = Vec::new();
    for id in before.ids.iter().filter(|id| !after.ids.contains(id)) {
        out.push(ControlNotification::WindowClose { window_id: *id });
    }
    if let Some(cur) = after.current {
        if before.current != Some(cur) {
            out.push(ControlNotification::SessionWindowChanged { session_id, window_id: cur });
        }
    }
    for id in after.ids.iter().filter(|id| !before.ids.contains(id)) {
        out.push(ControlNotification::WindowAdd { window_id: *id });
    }
    out
}

/// Announce what changed in the window list since the last call, whatever
/// changed it: a request, a key binding, a hook, or a pane that exited or was
/// killed from outside. These used to come from the command's hook and named
/// whatever window was current when it fired, so `kill-window -t @2` with @1
/// current announced `%window-close @1` (#811). Call it only where no -t temp
/// focus is applied.
pub fn sync_window_changes(app: &mut AppState) {
    let now = window_snapshot(app);
    if let Some(before) = app.control_windows_seen.take() {
        if !app.control_clients.is_empty() {
            for notif in window_change_notifications(&before, &now, app.session_id) {
                emit_notification(app, notif);
            }
            for notif in pane_mode_notifications(&before.pane_modes, &now.pane_modes) {
                emit_notification(app, notif);
            }
        }
    }
    app.control_windows_seen = Some(now);
}

/// One item of `refresh-client -f`: `Some(Some(secs))` sets pause-after,
/// `Some(None)` clears it, `None` is not a pause-after flag. Follows
/// server-client.c server_client_control_flags: bare `pause-after` is an age
/// of 0, `pause-after=N` reads N like `%u`, and a leading `!` clears the flag.
pub fn parse_pause_after_flag(flag: &str) -> Option<Option<u64>> {
    let flag = flag.trim();
    if flag == "no-pause" {
        return Some(None); // psmux's older spelling of !pause-after
    }
    let (not, name) = match flag.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, flag),
    };
    let secs = if name == "pause-after" {
        0
    } else {
        let digits: String = name
            .strip_prefix("pause-after=")?
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<u64>().ok()?
    };
    Some(if not { None } else { Some(secs) })
}

/// tmux's limit for a control client that does not use pause-after: once its
/// oldest unwritten pane output is this old the client is dropped with
/// `%exit too far behind` (control.c CONTROL_MAXIMUM_AGE, in milliseconds).
pub const CONTROL_MAXIMUM_AGE_MS: u64 = 300_000;

/// Unwritten output for one control client past which the server stops
/// taking pane output from the PTYs, when every attached client is a control
/// client this far behind: the panes' programs then wait on their writes, as
/// tmux stops reading a pane (server_client_check_pane_buffer, #875).
pub const CONTROL_QUEUE_HIGH: usize = 1 << 20;

/// psmux's ceiling on one control client's unwritten output. tmux has none
/// and relies on the age rules alone, but a TUI client keeps the panes
/// flowing, so a control client that stops reading would otherwise hold
/// up to five minutes of output in memory. Past this the client is treated
/// as too old: its pane is paused (pause-after) or it is dropped with
/// `%exit too far behind`. Either way it is told; nothing is lost silently.
pub const CONTROL_QUEUE_MAX: usize = 256 << 20;

#[derive(Default)]
struct GatePane {
    /// Output handed to the writer and not written yet: (sequence, queued at).
    pending: std::collections::VecDeque<(u64, std::time::Instant)>,
    /// Paused for this client (tmux CONTROL_PANE_PAUSED).
    paused: bool,
    /// Queued output with a sequence at or below this was discarded by a
    /// pause and is dropped when the writer reaches it.
    discard_upto: u64,
}

#[derive(Default)]
struct GateState {
    /// `refresh-client -f pause-after=N` age in milliseconds (None = unset).
    pause_after_ms: Option<u64>,
    panes: std::collections::HashMap<usize, GatePane>,
    next_seq: u64,
    /// Panes paused by the server thread whose `%pause` line the writer
    /// still has to print.
    pending_pauses: Vec<usize>,
    /// The client fell more than CONTROL_MAXIMUM_AGE behind.
    too_far_behind: bool,
    /// Bytes of pane output handed to the writer and not written yet (#875).
    queued_bytes: usize,
}

/// What the server loop does with a new chunk of pane output for one client.
#[derive(Debug, PartialEq, Eq)]
pub enum GateEnqueue {
    /// Queue it as `ControlNotification::PaneOutput` with this sequence.
    Send(u64),
    /// The pane is paused (or the client is going away): drop it.
    Skip,
    /// This chunk paused the pane: drop it and wake the writer.
    Paused,
    /// The client is too far behind: drop it; the client is disconnected.
    TooFarBehind,
}

/// What the writer thread does with a queued chunk when it reaches it.
#[derive(Debug, PartialEq, Eq)]
pub enum GateWrite {
    /// `%output` (pause-after unset) or `%extended-output` with this age.
    Write { age_ms: u64, extended: bool },
    /// Discarded by a pause, or the pane is paused: write nothing.
    Drop,
    /// This chunk is older than pause-after: write `%pause %N` instead.
    Pause,
    /// Older than CONTROL_MAXIMUM_AGE without pause-after: disconnect.
    TooFarBehind,
}

/// Per control client output bookkeeping shared by the server loop (which
/// queues pane output) and the connection's writer thread (which writes it).
///
/// Mirrors tmux control.c: the age that matters is the age of the OLDEST
/// output for a pane that the client has not been sent yet
/// (control_check_age). With pause-after set, a pane whose backlog is older
/// than the pause age is paused for that client, its backlog is discarded and
/// nothing more is queued for it until `refresh-client -A '%N:continue'`.
/// Without pause-after a client more than CONTROL_MAXIMUM_AGE behind is
/// disconnected with "too far behind". Either way the backlog for a stalled
/// client stops growing (#826).
#[derive(Default)]
pub struct ControlOutputGate {
    state: std::sync::Mutex<GateState>,
    /// The client's socket, shut down when it falls too far behind so a
    /// writer blocked on a client that stopped reading is released.
    stream: std::sync::Mutex<Option<std::net::TcpStream>>,
}

fn gate_age_ms(then: std::time::Instant, now: std::time::Instant) -> u64 {
    now.checked_duration_since(then).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl ControlOutputGate {
    pub fn new(stream: Option<std::net::TcpStream>) -> Self {
        ControlOutputGate { state: Default::default(), stream: std::sync::Mutex::new(stream) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// tmux control_check_age for an age already measured: true when the
    /// client is behind by more than it may be.
    fn too_old(st: &GateState, age: u64) -> bool {
        // control.c: `if (cb->t >= t) return (0);` (no age, not behind).
        if age == 0 {
            return false;
        }
        match st.pause_after_ms {
            Some(limit) => age >= limit,
            None => age >= CONTROL_MAXIMUM_AGE_MS,
        }
    }

    fn pause_locked(st: &mut GateState, pane_id: usize) {
        let last = st.next_seq;
        let p = st.panes.entry(pane_id).or_default();
        p.paused = true;
        p.pending.clear();
        p.discard_upto = last;
    }

    fn mark_too_far_behind(st: &mut GateState) {
        st.too_far_behind = true;
        st.panes.clear();
    }

    /// Close the client's connection: its writer (possibly blocked on a
    /// client that stopped reading) and its command loop both end, and the
    /// connection deregisters the client.
    pub fn shutdown_stream(&self) {
        if let Some(s) = self.stream.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    /// The server loop has new output for `pane_id` (control_write_output).
    pub fn enqueue(&self, pane_id: usize, now: std::time::Instant) -> GateEnqueue {
        self.enqueue_sized(pane_id, now, 0)
    }

    /// `enqueue` for a chunk of `bytes` bytes, counted until the writer
    /// reports it with `written`.
    pub fn enqueue_sized(&self, pane_id: usize, now: std::time::Instant, bytes: usize) -> GateEnqueue {
        let mut st = self.lock();
        if st.too_far_behind {
            return GateEnqueue::Skip;
        }
        if st.panes.get(&pane_id).map_or(false, |p| p.paused) {
            return GateEnqueue::Skip;
        }
        let oldest = st.panes.get(&pane_id).and_then(|p| p.pending.front().map(|e| e.1));
        let over = st.queued_bytes.saturating_add(bytes) > CONTROL_QUEUE_MAX;
        if oldest.is_some() || over {
            if over || oldest.map_or(false, |t| Self::too_old(&st, gate_age_ms(t, now))) {
                if st.pause_after_ms.is_some() {
                    Self::pause_locked(&mut st, pane_id);
                    st.pending_pauses.push(pane_id);
                    return GateEnqueue::Paused;
                }
                // The writer is most likely blocked on this client, so
                // the %exit line cannot reach it; just drop the connection.
                Self::mark_too_far_behind(&mut st);
                drop(st);
                self.shutdown_stream();
                return GateEnqueue::TooFarBehind;
            }
        }
        st.next_seq += 1;
        let seq = st.next_seq;
        st.queued_bytes += bytes;
        st.panes.entry(pane_id).or_default().pending.push_back((seq, now));
        GateEnqueue::Send(seq)
    }

    /// The writer is done with a chunk of `bytes` bytes queued with
    /// `enqueue_sized`, written or dropped.
    pub fn written(&self, bytes: usize) {
        let mut st = self.lock();
        st.queued_bytes = st.queued_bytes.saturating_sub(bytes);
    }

    /// Bytes of pane output queued for this client and not written yet.
    pub fn queued_bytes(&self) -> usize {
        self.lock().queued_bytes
    }

    /// The chunk `seq` could not be queued after all (channel gone).
    pub fn unqueue(&self, pane_id: usize, seq: u64) {
        self.unqueue_sized(pane_id, seq, 0)
    }

    /// `unqueue` for a chunk queued with `enqueue_sized`.
    pub fn unqueue_sized(&self, pane_id: usize, seq: u64, bytes: usize) {
        let mut st = self.lock();
        st.queued_bytes = st.queued_bytes.saturating_sub(bytes);
        if let Some(p) = st.panes.get_mut(&pane_id) {
            if p.pending.back().map(|e| e.0) == Some(seq) {
                p.pending.pop_back();
            }
        }
    }

    /// The writer reached chunk `seq` (control_write_pending).
    pub fn dequeue(
        &self,
        pane_id: usize,
        seq: u64,
        queued_at: std::time::Instant,
        now: std::time::Instant,
    ) -> GateWrite {
        let mut st = self.lock();
        if st.too_far_behind {
            return GateWrite::Drop;
        }
        let pause_set = st.pause_after_ms.is_some();
        let paused = match st.panes.get_mut(&pane_id) {
            None => return GateWrite::Drop,
            Some(p) => {
                if seq <= p.discard_upto {
                    return GateWrite::Drop;
                }
                if p.pending.front().map(|e| e.0) == Some(seq) {
                    p.pending.pop_front();
                }
                p.paused
            }
        };
        if paused {
            return GateWrite::Drop;
        }
        let age = gate_age_ms(queued_at, now);
        if Self::too_old(&st, age) {
            if pause_set {
                Self::pause_locked(&mut st, pane_id);
                return GateWrite::Pause;
            }
            // The writer prints `%exit too far behind`, then closes.
            Self::mark_too_far_behind(&mut st);
            return GateWrite::TooFarBehind;
        }
        GateWrite::Write { age_ms: age, extended: pause_set }
    }

    /// `%pause` lines the server thread decided on, for the writer to print.
    pub fn take_pending_pauses(&self) -> Vec<usize> {
        std::mem::take(&mut self.lock().pending_pauses)
    }

    pub fn is_too_far_behind(&self) -> bool {
        self.lock().too_far_behind
    }

    /// `refresh-client -f pause-after[=N]` (Some) / `!pause-after` (None).
    pub fn set_pause_after(&self, ms: Option<u64>) {
        self.lock().pause_after_ms = ms;
    }

    pub fn pause_after_ms(&self) -> Option<u64> {
        self.lock().pause_after_ms
    }

    pub fn is_paused(&self, pane_id: usize) -> bool {
        self.lock().panes.get(&pane_id).map_or(false, |p| p.paused)
    }

    /// `refresh-client -A '%N:pause'` (control_pause_pane). True when the
    /// pane was not paused, so a `%pause` line is due. The command's reply
    /// carries that line inside its %begin/%end block, as tmux writes it,
    /// so it is not queued for the writer.
    pub fn pause_pane(&self, pane_id: usize) -> bool {
        let mut st = self.lock();
        if st.panes.get(&pane_id).map_or(false, |p| p.paused) {
            return false;
        }
        Self::pause_locked(&mut st, pane_id);
        true
    }

    /// `refresh-client -A '%N:continue'` (control_continue_pane). True when
    /// the pane was paused, so a `%continue` line is due.
    pub fn continue_pane(&self, pane_id: usize) -> bool {
        let mut st = self.lock();
        let last = st.next_seq;
        match st.panes.get_mut(&pane_id) {
            Some(p) if p.paused => {
                p.paused = false;
                p.pending.clear();
                p.discard_upto = last;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue826_cc_pause_after.rs"]
mod tests_issue826_cc_pause_after;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_output_printable() {
        assert_eq!(escape_output("hello world"), "hello world");
    }

    #[test]
    fn test_escape_output_backslash() {
        // tmux escapes backslash as octal \134
        assert_eq!(escape_output("a\\b"), "a\\134b");
    }

    #[test]
    fn test_escape_output_control_chars() {
        // \r = 0x0D = octal 015, \n = 0x0A = octal 012
        assert_eq!(escape_output("a\r\nb"), "a\\015\\012b");
    }

    #[test]
    fn test_escape_output_tab_passthrough() {
        assert_eq!(escape_output("a\tb"), "a\tb");
    }

    #[test]
    fn test_escape_output_high_bytes() {
        // U+FFFD replacement character = UTF-8 bytes ef bf bd = octal 357 277 275
        let data = String::from_utf8_lossy(b"x\xffy").to_string();
        assert_eq!(escape_output(&data), "x\\357\\277\\275y");
    }

    #[test]
    fn test_format_begin_end_error() {
        assert_eq!(format_begin(1700000000, 1), "%begin 1700000000 1 1");
        assert_eq!(format_end(1700000000, 1), "%end 1700000000 1 1");
        assert_eq!(format_error(1700000000, 1), "%error 1700000000 1 1");
    }

    // Issue #857: the reply to the command line command is flagged 0, and
    // only -CC puts the DCS opener in front of it, with no newline between.
    #[test]
    fn issue857_initial_reply_block_is_flagged_zero() {
        assert_eq!(format_initial_begin(1700000000, 1), "%begin 1700000000 1 0");
        assert_eq!(format_initial_end(1700000000, 1), "%end 1700000000 1 0");
        assert_eq!(format_initial_error(1700000000, 1), "%error 1700000000 1 0");
        assert_eq!(
            initial_reply_block(1700000000, false),
            "%begin 1700000000 1 0\n%end 1700000000 1 0\n"
        );
        assert_eq!(
            initial_reply_block(1700000000, true),
            "\x1bP1000p%begin 1700000000 1 0\n%end 1700000000 1 0\n"
        );
    }

    #[test]
    fn test_format_notification_window_add() {
        let line = format_notification(&ControlNotification::WindowAdd { window_id: 3 });
        assert_eq!(line, "%window-add @3");
    }

    #[test]
    fn test_format_notification_output() {
        let line = format_notification(&ControlNotification::Output {
            pane_id: 1,
            data: "hello\r\n".to_string(),
        });
        assert_eq!(line, "%output %1 hello\\015\\012");
    }

    #[test]
    fn test_format_notification_exit() {
        let line = format_notification(&ControlNotification::Exit { reason: None });
        assert_eq!(line, "%exit");
        let line = format_notification(&ControlNotification::Exit {
            reason: Some("too far behind".to_string()),
        });
        assert_eq!(line, "%exit too far behind");
    }

    #[test]
    fn test_format_notification_session_renamed() {
        let line = format_notification(&ControlNotification::SessionRenamed {
            name: "my-session".to_string(),
        });
        assert_eq!(line, "%session-renamed my-session");
    }

    #[test]
    fn test_format_notification_layout_change() {
        let line = format_notification(&ControlNotification::LayoutChange {
            window_id: 2,
            layout: "5e08,120x30,0,0,1".to_string(),
            visible_layout: "5e08,120x30,0,0,1".to_string(),
            flags: "*".to_string(),
        });
        // tmux format: %layout-change @WID layout visible_layout flags
        assert_eq!(line, "%layout-change @2 5e08,120x30,0,0,1 5e08,120x30,0,0,1 *");
    }

    #[test]
    fn test_format_notification_window_close() {
        let line = format_notification(&ControlNotification::WindowClose { window_id: 7 });
        assert_eq!(line, "%window-close @7");
    }

    #[test]
    fn test_format_notification_window_renamed() {
        let line = format_notification(&ControlNotification::WindowRenamed {
            window_id: 0,
            name: "editor".to_string(),
        });
        assert_eq!(line, "%window-renamed @0 editor");
    }

    #[test]
    fn test_format_notification_session_changed() {
        let line = format_notification(&ControlNotification::SessionChanged {
            session_id: 0,
            name: "main".to_string(),
        });
        assert_eq!(line, "%session-changed $0 main");
    }

    #[test]
    fn test_format_notification_session_window_changed() {
        let line = format_notification(&ControlNotification::SessionWindowChanged {
            session_id: 0,
            window_id: 5,
        });
        assert_eq!(line, "%session-window-changed $0 @5");
    }

    #[test]
    fn test_format_notification_window_pane_changed() {
        let line = format_notification(&ControlNotification::WindowPaneChanged {
            window_id: 2,
            pane_id: 4,
        });
        assert_eq!(line, "%window-pane-changed @2 %4");
    }

    #[test]
    fn test_format_notification_continue_pause() {
        assert_eq!(format_notification(&ControlNotification::Continue { pane_id: 1 }), "%continue %1");
        assert_eq!(format_notification(&ControlNotification::Pause { pane_id: 1 }), "%pause %1");
    }

    #[test]
    fn test_format_notification_client_detached() {
        let line = format_notification(&ControlNotification::ClientDetached { client: "client0".to_string() });
        assert_eq!(line, "%client-detached client0");
    }

    #[test]
    fn test_has_control_clients_empty() {
        let app = AppState::new("test".to_string());
        assert!(!has_control_clients(&app));
    }

    #[test]
    fn test_has_control_clients_with_client() {
        let mut app = AppState::new("test".to_string());
        let (tx, _rx) = std::sync::mpsc::channel();
        app.control_clients.insert(1, crate::types::ControlClient {
            client_id: 1,
            cmd_counter: 0,
            echo_enabled: true,
            notification_tx: tx,
            paused_panes: std::collections::HashSet::new(),
            subscriptions: std::collections::HashMap::new(),
            subscription_values: std::collections::HashMap::new(),
            subscription_last_check: std::collections::HashMap::new(),
            output_gate: Default::default(),
            size: None,
            window_sizes: std::collections::HashMap::new(),
        });
        assert!(has_control_clients(&app));
    }

    #[test]
    fn test_emit_notification_to_clients() {
        let mut app = AppState::new("test".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        app.control_clients.insert(1, crate::types::ControlClient {
            client_id: 1,
            cmd_counter: 0,
            echo_enabled: false,
            notification_tx: tx,
            paused_panes: std::collections::HashSet::new(),
            subscriptions: std::collections::HashMap::new(),
            subscription_values: std::collections::HashMap::new(),
            subscription_last_check: std::collections::HashMap::new(),
            output_gate: Default::default(),
            size: None,
            window_sizes: std::collections::HashMap::new(),
        });
        emit_notification(&app, ControlNotification::WindowAdd { window_id: 5 });
        let notif = rx.try_recv().unwrap();
        assert!(matches!(notif, ControlNotification::WindowAdd { window_id: 5 }));
    }

    #[test]
    fn test_emit_notification_skips_paused_pane() {
        let mut app = AppState::new("test".to_string());
        let (tx, rx) = std::sync::mpsc::channel();
        let mut paused = std::collections::HashSet::new();
        paused.insert(3usize);
        app.control_clients.insert(1, crate::types::ControlClient {
            client_id: 1,
            cmd_counter: 0,
            echo_enabled: false,
            notification_tx: tx,
            paused_panes: paused,
            subscriptions: std::collections::HashMap::new(),
            subscription_values: std::collections::HashMap::new(),
            subscription_last_check: std::collections::HashMap::new(),
            output_gate: Default::default(),
            size: None,
            window_sizes: std::collections::HashMap::new(),
        });
        // Output for paused pane 3 should be dropped
        emit_notification(&app, ControlNotification::Output { pane_id: 3, data: "test".into() });
        assert!(rx.try_recv().is_err(), "paused pane output should not be sent");
        // Output for different pane should go through
        emit_notification(&app, ControlNotification::Output { pane_id: 5, data: "ok".into() });
        assert!(rx.try_recv().is_ok(), "non-paused pane output should be sent");
    }

    #[test]
    fn test_escape_output_empty() {
        assert_eq!(escape_output(""), "");
    }

    #[test]
    fn test_escape_output_mixed() {
        // Mix of printable, backslash, control, and tab
        assert_eq!(escape_output("a\\b\tc\x01d"), "a\\134b\tc\\001d");
    }

    #[test]
    fn test_format_notification_extended_output() {
        let line = format_notification(&ControlNotification::ExtendedOutput {
            pane_id: 2,
            age_ms: 150,
            data: "hello\r\n".to_string(),
        });
        assert_eq!(line, "%extended-output %2 150 : hello\\015\\012");
    }

    #[test]
    fn test_format_notification_subscription_changed() {
        let line = format_notification(&ControlNotification::SubscriptionChanged {
            name: "mysub".to_string(),
            session_id: 0,
            window_id: 1,
            window_index: 0,
            pane_id: 3,
            value: "pwsh".to_string(),
        });
        assert_eq!(line, "%subscription-changed mysub $0 @1 0 %3 : pwsh");
    }

    #[test]
    fn test_format_notification_paste_buffer_changed() {
        let line = format_notification(&ControlNotification::PasteBufferChanged {
            name: "buffer0".to_string(),
        });
        assert_eq!(line, "%paste-buffer-changed buffer0");
    }

    #[test]
    fn test_format_notification_paste_buffer_deleted() {
        let line = format_notification(&ControlNotification::PasteBufferDeleted {
            name: "buffer1".to_string(),
        });
        assert_eq!(line, "%paste-buffer-deleted buffer1");
    }

    #[test]
    fn test_format_notification_client_session_changed() {
        let line = format_notification(&ControlNotification::ClientSessionChanged {
            client: "/dev/pts/0".to_string(),
            session_id: 2,
            name: "work".to_string(),
        });
        assert_eq!(line, "%client-session-changed /dev/pts/0 $2 work");
    }

    #[test]
    fn test_format_notification_message() {
        let line = format_notification(&ControlNotification::Message {
            text: "hello world".to_string(),
        });
        assert_eq!(line, "%message hello world");
    }

    #[test]
    fn test_format_notification_sessions_changed() {
        let line = format_notification(&ControlNotification::SessionsChanged);
        assert_eq!(line, "%sessions-changed");
    }

    #[test]
    fn test_format_notification_pane_mode_changed() {
        let line = format_notification(&ControlNotification::PaneModeChanged { pane_id: 7 });
        assert_eq!(line, "%pane-mode-changed %7");
    }

    #[test]
    fn test_format_notification_extended_output_with_escape() {
        let line = format_notification(&ControlNotification::ExtendedOutput {
            pane_id: 0,
            age_ms: 5000,
            data: "line1\\line2".to_string(),
        });
        assert_eq!(line, "%extended-output %0 5000 : line1\\134line2");
    }

    #[test]
    fn test_format_notification_subscription_changed_empty_value() {
        let line = format_notification(&ControlNotification::SubscriptionChanged {
            name: "test_sub".to_string(),
            session_id: 1,
            window_id: 2,
            window_index: 3,
            pane_id: 4,
            value: String::new(),
        });
        assert_eq!(line, "%subscription-changed test_sub $1 @2 3 %4 : ");
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_issue811_window_notifications.rs"]
mod tests_issue811_window_notifications;

#[cfg(test)]
#[path = "../tests-rs/test_issue825_control_detach.rs"]
mod tests_issue825_control_detach;

#[cfg(test)]
#[path = "../tests-rs/test_issue824_827_cc.rs"]
mod tests_issue824_827_cc;
