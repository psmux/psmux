use std::io;

use crate::format::expand_format_for_window;
use crate::render_state::ClientRenderOptions;
use crate::types::{AppState, Node, Window};
use crate::util::WinInfo;

/// Collect all leaf pane paths in tree order (for next/prev pane cycling).
pub(crate) fn collect_pane_paths_server(
    node: &Node,
    path: &mut Vec<usize>,
    panes: &mut Vec<Vec<usize>>,
) {
    match node {
        Node::Leaf(_) => {
            panes.push(path.clone());
        }
        Node::Split { children, .. } => {
            for (i, c) in children.iter().enumerate() {
                path.push(i);
                collect_pane_paths_server(c, path, panes);
                path.pop();
            }
        }
    }
}

/// Resolve a `switch-client -T <table>` request against the live key tables.
///
/// Issue #640. tmux's `cmd_switch_client_exec` calls
/// `key_bindings_get_table(tablename, 0)` — create = 0 — and errors with
/// "table %s doesn't exist" when the table has no bindings, so a typo in the
/// table name is reported rather than latching a table that swallows the next
/// key forever.
///
/// `Ok(None)` means "back to the default table": `root` is what
/// `server_client_set_key_table(c, NULL)` resolves to, and it is stored as
/// `None` so `#{client_key_table}` keeps reporting `root`/`copy-mode-vi` from
/// the pane's own mode. `root` and `prefix` are always live in tmux, so they
/// are accepted even when the user deleted every binding in them.
pub(crate) fn resolve_switch_client_table(
    app: &AppState,
    table: &str,
) -> Result<Option<String>, String> {
    if table == "root" {
        return Ok(None);
    }
    if table == "prefix" || app.key_tables.contains_key(table) {
        return Ok(Some(table.to_string()));
    }
    Err(format!("table {} doesn't exist", table))
}

/// Serialize key_tables into a compact JSON array for syncing to the client.
/// Format: [{"t":"prefix","k":"x","c":"split-window -v","r":false}, ...]
pub(crate) fn serialize_bindings_json(app: &AppState) -> String {
    use crate::commands::format_action;
    use crate::config::format_key_binding;
    let mut out = String::from("[");
    let mut first = true;
    for (table_name, binds) in &app.key_tables {
        for bind in binds {
            if !first {
                out.push(',');
            }
            first = false;
            let key_str = json_escape_string(&format_key_binding(&bind.key));
            let cmd_str = json_escape_string(&format_action(&bind.action));
            let tbl_str = json_escape_string(table_name);
            out.push_str(&format!(
                "{{\"t\":\"{}\",\"k\":\"{}\",\"c\":\"{}\",\"r\":{}}}",
                tbl_str, key_str, cmd_str, bind.repeat
            ));
        }
    }
    out.push(']');
    out
}

/// Escape a string for embedding inside a JSON double-quoted value.
/// Handles backslashes, double-quotes, and control characters.
/// Append the copy-mode-line-numbers state fields to a JSON object buffer that
/// currently ends with `}`. Emits nothing when the option is unset or `off`.
/// Ships the option value, the active pane's scrollback size (for absolute /
/// hybrid numbering), and the optional gutter styles.
pub(crate) fn append_copy_ln_json(app: &AppState, buf: &mut String) {
    let Some(cln) = app.user_options.get("copy-mode-line-numbers") else { return; };
    if cln == "off" || !buf.ends_with('}') { return; }
    let hsize = app.windows.get(app.active_idx)
        .and_then(|win| crate::tree::active_pane(&win.root, &win.active_path))
        .and_then(|p| p.term.lock().ok().map(|g| g.screen().scrollback_filled()))
        .unwrap_or(0);
    buf.pop();
    buf.push_str(",\"copy_mode_line_numbers\":\"");
    buf.push_str(&json_escape_string(cln));
    buf.push_str("\",\"copy_hsize\":");
    buf.push_str(&hsize.to_string());
    if let Some(st) = app.user_options.get("copy-mode-line-number-style") {
        buf.push_str(",\"copy_mode_line_number_style\":\"");
        buf.push_str(&json_escape_string(st));
        buf.push('"');
    }
    if let Some(st) = app.user_options.get("copy-mode-current-line-number-style") {
        buf.push_str(",\"copy_mode_current_line_number_style\":\"");
        buf.push_str(&json_escape_string(st));
        buf.push('"');
    }
    buf.push('}');
}

/// Append the active window's floating-pane overlays to a JSON object buffer
/// that currently ends with `}`. Emits nothing when there are no floats.
pub(crate) fn append_floats_json(app: &AppState, buf: &mut String) {
    if !buf.ends_with('}') { return; }
    let frag = crate::popup::serialize_floats_json(app);
    if frag.is_empty() { return; }
    buf.pop();
    buf.push_str(&frag);
    buf.push('}');
}

pub(crate) fn json_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Every status/style format that has to be expanded to build one render frame.
///
/// This exists because the DumpState handler and the server auto-push block in
/// `server/mod.rs` need the identical set of expansions, and for a while they
/// each had their own copy of the list. One copy was wrapped in an
/// [`crate::format::AsyncFormatGuard`] and the other was not, so `#()` in the
/// status bar spawned a process synchronously on the server event loop — the
/// same thread that delivers keystrokes to ConPTY — on every pane output burst.
/// With a `cmd /c` helper measured at 88ms and a loop that targets 1ms
/// iterations while PTY data flows, that alone was seconds of input lag per
/// second of typing.
///
/// The fix is structural: one list, one guard, owned by the function. Adding a
/// new render path can no longer reintroduce the bug, because there is nothing
/// left to remember to do.
pub(crate) struct StatusFormats {
    pub status_style: String,
    pub status_left: String,
    pub status_right: String,
    pub pane_border_style: String,
    pub pane_active_border_style: String,
    pub pane_border_hover_style: String,
    pub window_status_separator: String,
    pub window_status_style: String,
    pub window_status_current_style: String,
    pub mode_style: String,
    pub message_style: String,
    pub client_render_options: ClientRenderOptions,
    /// Pre-built JSON array for the multi-line status bar.
    pub status_format_json: String,
    /// Expanded `set-titles-string`, or `None` when `set-titles` is off. The
    /// client turns this into an OSC 0 for its host terminal.
    pub host_title: Option<String>,
}

/// Expand every per-frame status/style format in one guarded pass.
///
/// `status_style` is passed in rather than read from `app` because both callers
/// hold it in a metadata cache that is only rebuilt on structural change.
///
/// Infallible by design: this runs once per rendered frame inside
/// `run_server`, where an `Err` would end the server and every pane with it.
/// A render option that escaped catalog validation degrades to its default
/// instead (the same fallback the client applies to a missing field).
pub(crate) fn expand_status_formats(
    app: &AppState,
    status_style: &str,
) -> StatusFormats {
    use crate::format::expand_format;
    // The one guard. Everything below expands #() asynchronously against the
    // TTL cache instead of blocking the event loop.
    let _async_fmt = crate::format::AsyncFormatGuard::new();
    // #648: pane-border-indicators is a WINDOW option. The frame draws the
    // active window's borders, so the active window's own entry wins over the
    // server wide one.
    let pane_border_indicators = crate::server::options::window_local_option(
        app,
        app.active_idx,
        "pane-border-indicators",
    )
    .or_else(|| {
        app.user_options
            .get("pane-border-indicators")
            .map(String::as_str)
    })
    .unwrap_or(crate::pane_border::INDICATORS_DEFAULT);
    let pane_border_indicators =
        crate::pane_border::PaneBorderIndicators::parse(pane_border_indicators)
            .unwrap_or_else(|e| {
                if crate::debug_log::server_log_enabled() {
                    crate::debug_log::server_log(
                        "render",
                        &format!("{e}; falling back to the default"),
                    );
                }
                crate::pane_border::PaneBorderIndicators::default()
            });
    StatusFormats {
        status_style: expand_format(status_style, app),
        status_left: expand_format(&app.status_left, app),
        status_right: expand_format(&app.status_right, app),
        pane_border_style: expand_format(&app.pane_border_style, app),
        pane_active_border_style: expand_format(&app.pane_active_border_style, app),
        pane_border_hover_style: expand_format(&app.pane_border_hover_style, app),
        window_status_separator: expand_format(&app.window_status_separator, app),
        window_status_style: expand_format(&app.window_status_style, app),
        window_status_current_style: expand_format(&app.window_status_current_style, app),
        mode_style: expand_format(&app.mode_style, app),
        message_style: expand_format(&app.message_style, app),
        client_render_options: ClientRenderOptions {
            status_left_style: Some(expand_format(&app.status_left_style, app)),
            status_right_style: Some(expand_format(&app.status_right_style, app)),
            window_status_activity_style: Some(expand_format(
                &app.window_status_activity_style,
                app,
            )),
            window_status_bell_style: Some(expand_format(
                &app.window_status_bell_style,
                app,
            )),
            window_status_last_style: Some(expand_format(
                &app.window_status_last_style,
                app,
            )),
            window_style: Some(expand_format(
                app.user_options.get("window-style").map(String::as_str).unwrap_or(""),
                app,
            )),
            window_active_style: Some(expand_format(
                app.user_options
                    .get("window-active-style")
                    .map(String::as_str)
                    .unwrap_or(""),
                app,
            )),
            pane_border_indicators: Some(pane_border_indicators),
            // Only travels when the user actually set the option, so the
            // default configuration adds nothing to every render frame.
            codepoint_widths: if app.codepoint_widths.is_empty() {
                None
            } else {
                Some(app.codepoint_widths.clone())
            },
        },
        status_format_json: {
            let mut sf = String::from("[");
            for (i, fmt_str) in app.status_format.iter().enumerate() {
                if i > 0 { sf.push(','); }
                sf.push('"');
                sf.push_str(&json_escape_string(&expand_format(fmt_str, app)));
                sf.push('"');
            }
            sf.push(']');
            sf
        },
        // set-titles-string was expanded outside the guard on BOTH paths, so it
        // blocked even where the rest of the bar did not. It belongs here.
        host_title: if app.set_titles {
            let fmt = if app.set_titles_string.is_empty() {
                "#S:#I:#W"
            } else {
                app.set_titles_string.as_str()
            };
            Some(expand_format(fmt, app))
        } else {
            None
        },
    }
}

/// Append typed client-render options to a buffer ending in `}`.
///
/// # Errors
///
/// Returns an error if `buf` is not a JSON object or the options cannot be
/// serialized as one.
pub(crate) fn append_client_render_options_json(
    buf: &mut String,
    options: &ClientRenderOptions,
) -> io::Result<()> {
    if !buf.ends_with('}') {
        return Err(io::Error::other(
            "render-state JSON does not end with an object delimiter",
        ));
    }
    let encoded = serde_json::to_string(options).map_err(io::Error::other)?;
    let fields = encoded
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| io::Error::other("client render options are not an object"))?;
    if fields.is_empty() {
        return Ok(());
    }
    buf.pop();
    buf.push(',');
    buf.push_str(fields);
    buf.push('}');
    Ok(())
}

/// Build windows JSON with pre-expanded tab_text for each window.
/// The tab_text is the fully expanded window-status-format / window-status-current-format.
pub(crate) fn list_windows_json_with_tabs(app: &AppState) -> io::Result<String> {
    // This expands window-status-format once per window, so synchronous #()
    // work here would multiply with the window count.
    let _async_fmt = crate::format::AsyncFormatGuard::new();
    let mut v: Vec<WinInfo> = Vec::new();
    for (i, w) in app.windows.iter().enumerate() {
        let is_active = i == app.active_idx;
        // #648: window-status-format, its -current- twin and the five window
        // status styles are WINDOW options. Each window's own entry wins over
        // the server wide one; `window_local_option` borrows, so a window that
        // set nothing (every window, until someone writes to one) costs no
        // allocation on this per-frame path.
        let local = |name: &str| crate::server::options::window_local_option(app, i, name);
        let fmt = if is_active {
            local("window-status-current-format")
                .unwrap_or(&app.window_status_current_format)
        } else {
            local("window-status-format").unwrap_or(&app.window_status_format)
        };
        let tab = expand_format_for_window(fmt, app, i);
        let owned = |name: &str| local(name).map(|value| expand_format_for_window(value, app, i));
        v.push(WinInfo {
            id: w.id,
            name: w.name.clone(),
            active: is_active,
            activity: w.activity_flag,
            bell: w.bell_flag,
            last: i == app.last_window_idx,
            tab_text: tab,
            idx: app.win_display_index(i),
            ws_style: owned("window-status-style"),
            wsc_style: owned("window-status-current-style"),
            wsa_style: owned("window-status-activity-style"),
            wsb_style: owned("window-status-bell-style"),
            wsl_style: owned("window-status-last-style"),
        });
    }
    serde_json::to_string(&v)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, format!("json error: {e}")))
}

/// Sum data_version counters across all panes in the active window.
/// Whether the automatic rename walk (a process table snapshot per window) is
/// due for a pane, tmux `names.c:66` style: only when the window has produced
/// output since the last check, and at most once per throttle interval.
///
/// The check runs inside the `dump-state` request handler, so before #658 it
/// ran only when a client asked for a frame, which an idle client never did.
/// The #658 idle floor makes an attached client ask once a second, and without
/// this gate every one of those requests walked the process table for every
/// window: the cross terminal gate measured server plus client idle CPU at
/// 3.7 percent of a core against 0.78 the same morning on the build before the
/// floor (sweep 2026-09-16_02-02-11, T7). tmux answers the same question with
/// `PANE_CHANGED`: no output since the last check means nothing to rename.
///
/// `last_output` is the window's last output stamp (advanced by
/// `check_window_activity`, which runs earlier in the same request), and
/// `last_check` is the pane's stamp from the previous walk.
pub(crate) fn window_name_check_due(
    last_output: std::time::Instant,
    last_check: std::time::Instant,
    throttle_ms: u128,
) -> bool {
    if last_output <= last_check {
        return false;
    }
    last_check.elapsed().as_millis() >= throttle_ms
}

pub(crate) fn combined_data_version(app: &AppState) -> u64 {
    let mut v = 0u64;
    fn walk(node: &Node, v: &mut u64) {
        match node {
            Node::Leaf(p) => {
                *v = v.wrapping_add(p.data_version.load(std::sync::atomic::Ordering::Acquire));
            }
            Node::Split { children, .. } => {
                for c in children {
                    walk(c, v);
                }
            }
        }
    }
    if let Some(win) = app.windows.get(app.active_idx) {
        walk(&win.root, &mut v);
    }
    // #658: which window those counters came from is part of the identity of
    // the frame they describe. Without this term the sum for window 0 and the
    // sum for window 1 are the same number whenever their panes are equally
    // idle, so the version guard on the "NC" fast path could not tell a window
    // switch from no change at all. Mixed into a distinct field so it cannot
    // cancel against a pane counter.
    v = v.wrapping_add((app.active_idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    // Include per-window status flags so non-active windows changing their
    // bell/activity/silence state forces a frame emission. Without this, the
    // status bar shows the bell or activity indicator only after some
    // incidental repaint trigger like a mouse move or window switch (#162).
    for (i, w) in app.windows.iter().enumerate() {
        let bits = (w.bell_flag as u64) | ((w.activity_flag as u64) << 1) | ((w.silence_flag as u64) << 2);
        v = v.wrapping_add(bits.wrapping_mul(0x50011).wrapping_add(i as u64));
    }
    // Include mode discriminant so overlay state changes (PopupMode, MenuMode,
    // ConfirmMode, PaneChooser, ClockMode) always invalidate the cached version.
    // Without this, the NC optimization could return stale frames that lack
    // overlay fields, causing overlays to not render on the client.
    let mode_tag: u64 = match &app.mode {
        crate::types::Mode::Passthrough => 0,
        crate::types::Mode::Prefix { .. } => 1,
        crate::types::Mode::CopyMode => 2,
        crate::types::Mode::CopySearch { .. } => 3,
        crate::types::Mode::ClockMode => 4,
        crate::types::Mode::PopupMode { .. } => 5,
        crate::types::Mode::ConfirmMode { .. } => 6,
        crate::types::Mode::MenuMode { .. } => 7,
        crate::types::Mode::PaneChooser { .. } => 8,
        crate::types::Mode::BufferChooser { .. } => 9,
        _ => 10,
    };
    v = v.wrapping_add(mode_tag.wrapping_mul(0x1_0000_0000));
    // Include zoom state so toggling zoom always invalidates the cached
    // frame, even when no PTY data has changed (issue #125).
    // Check per-window zoom state — each window tracks zoom independently.
    for (wi, w) in app.windows.iter().enumerate() {
        if w.zoom_saved.is_some() {
            v = v.wrapping_add(0x8000_0000_0000_u64.wrapping_add(wi as u64));
        }
    }
    // Include client prefix state so the status bar re-renders
    // immediately when the prefix key is pressed/released (issue #126).
    if app.client_prefix_active {
        v = v.wrapping_add(0x4000_0000_0000);
    }
    // Include copy mode cursor position and scroll offset so cursor
    // movement and scrolling in copy mode always invalidate the cached
    // frame.  Without this, keyboard navigation in copy mode produces
    // no visible change because the server returns NC (no change).
    if let Some((r, c)) = app.copy_pos {
        v = v.wrapping_add((r as u64).wrapping_mul(0x10001).wrapping_add(c as u64));
    }
    v = v.wrapping_add((app.copy_scroll_offset as u64).wrapping_mul(0x20003));
    if let Some((ar, ac)) = app.copy_anchor {
        v = v.wrapping_add((ar as u64).wrapping_mul(0x30007).wrapping_add(ac as u64));
    }
    // Include status_message content so the search prompt refreshes per
    // keystroke while the user is typing in copy-mode search (#335).
    if let Some((ref msg, _, _)) = app.status_message {
        v = v.wrapping_add((msg.len() as u64).wrapping_mul(0x40009));
        if let Some(b) = msg.as_bytes().last() {
            v = v.wrapping_add(*b as u64);
        }
    }
    v
}

/// Per-window data version for activity detection
pub(crate) fn window_data_version(win: &Window) -> u64 {
    let mut v = 0u64;
    fn walk(node: &Node, v: &mut u64) {
        match node {
            Node::Leaf(p) => {
                *v = v.wrapping_add(p.data_version.load(std::sync::atomic::Ordering::Acquire));
            }
            Node::Split { children, .. } => {
                for c in children {
                    walk(c, v);
                }
            }
        }
    }
    walk(&win.root, &mut v);
    v
}

/// Check non-active windows for output activity and set their activity_flag.
/// Also checks bell_pending on all panes and sets window bell_flag,
/// and checks monitor-silence timeout to set silence_flag.
pub(crate) fn check_window_activity(app: &mut AppState) -> Vec<&'static str> {
    let active = app.active_idx;
    let bell_action = app.bell_action.clone();
    let mut triggered_hooks: Vec<&'static str> = Vec::new();
    let mut forward_bell = false;

    // #648: monitor-activity and monitor-silence are WINDOW options, so each
    // window's alert rules come from its own table with the session-wide value
    // as the parent, rather than once for the whole server as it used to be.
    let attached = app.attached_clients > 0;
    let global_monitor_activity = app.monitor_activity;
    let global_monitor_silence = app.monitor_silence;

    for (i, win) in app.windows.iter_mut().enumerate() {
        // Resolved per window inside the loop from the window's own table, so
        // this tick path allocates nothing.
        let monitor_activity =
            crate::server::options::win_flag(win, "monitor-activity", global_monitor_activity);
        let monitor_silence_secs =
            crate::server::options::win_number(win, "monitor-silence", global_monitor_silence);
        // ── Bell detection: check all panes for pending bells ──
        let has_bell = check_pane_bells(&win.root);
        if has_bell && i != active {
            // Apply bell-action: "any" = always, "current" = only active (skip),
            // "other" = only non-active (this path), "none" = never
            match bell_action.as_str() {
                "any" | "other" => {
                    if !win.bell_flag {
                        win.bell_flag = true;
                        triggered_hooks.push("alert-bell");
                    }
                    forward_bell = true;
                }
                _ => {} // "none" or "current" — don't flag non-active windows
            }
        } else if has_bell && i == active {
            match bell_action.as_str() {
                "any" | "current" => {
                    if !win.bell_flag {
                        win.bell_flag = true;
                        triggered_hooks.push("alert-bell");
                    }
                    forward_bell = true;
                }
                _ => {}
            }
        }

        // ── Activity detection ──
        if i == active && attached {
            // Active window with a client viewing it: alerts are seen the
            // moment they happen, so clear the flags (tmux clears alerts when
            // the window is current in an attached session). #559: a DETACHED
            // session must NOT take this path — tmux keeps accumulating
            // alert flags (including monitor-silence) on the current window
            // of a detached session, and scripts read them via list-windows.
            win.activity_flag = false;
            win.bell_flag = false;
            win.silence_flag = false;
            // #559: the old order assigned last_seen_version BEFORE comparing,
            // so the comparison below was always false and last_output_time
            // never advanced for the active window. Switching away from a
            // just-active window then tripped monitor-silence instantly
            // because its last_output_time was stale.
            let cur = window_data_version(win);
            if cur != win.last_seen_version {
                win.last_output_time = std::time::Instant::now();
            }
            win.last_seen_version = cur;
            continue;
        }
        let cur = window_data_version(win);
        if cur != win.last_seen_version {
            if monitor_activity && !win.activity_flag {
                win.activity_flag = true;
                triggered_hooks.push("alert-activity");
            }
            win.last_output_time = std::time::Instant::now();
            win.silence_flag = false; // Reset silence on new output
            win.last_seen_version = cur;
        }

        // ── Silence detection ──
        if monitor_silence_secs > 0 {
            let elapsed = win.last_output_time.elapsed().as_secs();
            if elapsed >= monitor_silence_secs && !win.silence_flag {
                win.silence_flag = true;
                triggered_hooks.push("alert-silence");
            }
        }
    }
    if forward_bell {
        app.bell_forward = true;
    }
    triggered_hooks
}

/// Propagate OSC 0/2 titles from the vt100 parser to pane.title for all windows.
/// tmux updates pane_title immediately when the child sends an OSC 0 or OSC 2
/// escape sequence, gated by the allow-set-title option. In psmux, the vt100
/// parser stores the title but we must explicitly copy it to pane.title.
/// Returns true if any pane title changed (i.e. state is dirty).
pub(crate) fn propagate_osc_titles(app: &mut AppState) -> bool {
    let allow_set_title = app.allow_set_title;
    if !allow_set_title {
        return false;
    }
    let mut dirty = false;
    for win in app.windows.iter_mut() {
        propagate_osc_titles_in_tree(&mut win.root, &mut dirty);
    }
    dirty
}

/// Read the active pane's most recent OSC 9;4 progress indicator state.
/// Returns `Some((state, value))` when a progress sequence has been received,
/// where state ∈ 0..=4 (0=hide, 1=default, 2=error, 3=indeterminate, 4=warning)
/// and value ∈ 0..=100. Used by the dump-state builder so the client can
/// re-emit OSC 9;4 to the host terminal (issue #269).
pub(crate) fn active_pane_progress(app: &AppState) -> Option<(u8, u8)> {
    let win = app.windows.get(app.active_idx)?;
    let pane = crate::tree::active_pane(&win.root, &win.active_path)?;
    if pane.dead {
        return None;
    }
    let parser = pane.term.lock().ok()?;
    parser.screen().progress()
}

/// Ingest one staged pane OSC 52 payload: paste buffer plus client forward.
///
/// tmux parity (input.c input_osc_52): a pane initiated OSC 52 is BOTH
/// forwarded to the host terminal AND added to the paste buffer stack via
/// paste_add, and tmux does this server side during input parsing whether or
/// not a client is attached. The buffer add here is therefore unconditional.
/// The one-shot `clipboard_osc52` forward slot is OVERWRITTEN with the
/// newest payload: a clipboard collapse must keep the latest write, and in a
/// detached session the slot would otherwise wedge on the first never
/// delivered payload and serve stale content when a client finally attaches
/// (every payload still lands in the buffer stack regardless).
///
/// Called from the dump-state builders (attached clients, per frame) and
/// from the main loop's 100ms housekeeping tick (detached sessions).
pub(crate) fn drain_osc52(app: &mut AppState) {
    if app.set_clipboard == "off" {
        return;
    }
    let Some((_sel, b64)) = take_pane_clipboard(app) else { return };
    let Ok(b64_str) = std::str::from_utf8(&b64) else { return };
    let Some(text) = crate::util::base64_decode(b64_str) else { return };
    app.paste_buffers.insert(0, text.clone());
    if app.paste_buffers.len() > 10 {
        app.paste_buffers.pop();
    }
    app.clipboard_osc52 = Some(text);
}

/// Drain a pending OSC 52 clipboard payload from any pane in the tree.
/// Returns the first `(selector, base64_data)` found and clears it on the
/// source pane.  Lets a child process inside any pane (e.g. Claude Code's
/// `/copy`) ask the host terminal to copy text — the dump-state builder
/// stages the result onto `App.clipboard_osc52`, the client re-emits OSC
/// 52 on its own stdout, and the host terminal performs the copy.
pub(crate) fn take_pane_clipboard(app: &AppState) -> Option<(Vec<u8>, Vec<u8>)> {
    for win in &app.windows {
        if let Some(payload) = drain_clipboard_in_node(&win.root) {
            return Some(payload);
        }
    }
    None
}

fn drain_clipboard_in_node(node: &Node) -> Option<(Vec<u8>, Vec<u8>)> {
    match node {
        Node::Leaf(p) => {
            if p.dead {
                return None;
            }
            let mut parser = p.term.lock().ok()?;
            parser.screen_mut().take_clipboard()
        }
        Node::Split { children, .. } => {
            for c in children {
                if let Some(r) = drain_clipboard_in_node(c) {
                    return Some(r);
                }
            }
            None
        }
    }
}

fn propagate_osc_titles_in_tree(node: &mut Node, dirty: &mut bool) {
    match node {
        Node::Leaf(p) => {
            if p.dead || p.title_locked {
                return;
            }
            if let Ok(parser) = p.term.lock() {
                let osc = parser.screen().title();
                if !osc.is_empty() {
                    let osc_owned = osc.to_string();
                    drop(parser);
                    if p.title != osc_owned {
                        p.title = osc_owned;
                        *dirty = true;
                    }
                }
            }
        }
        Node::Split { children, .. } => {
            for c in children {
                propagate_osc_titles_in_tree(c, dirty);
            }
        }
    }
}

/// Walk a pane tree and check/consume bell_pending flags.
/// Returns true if any pane had a pending bell.
fn check_pane_bells(node: &Node) -> bool {
    match node {
        Node::Leaf(p) => p
            .bell_pending
            .swap(false, std::sync::atomic::Ordering::AcqRel),
        Node::Split { children, .. } => {
            let mut any = false;
            for c in children {
                if check_pane_bells(c) {
                    any = true;
                }
            }
            any
        }
    }
}

/// Injects ESC[row;colR into any pane whose reader thread detected ESC[6n.
/// pwsh re-issues the CPR query after lock/unlock; without this response it
/// blocks indefinitely since the preemptive write at spawn time is long gone.
pub(crate) fn drain_cpr_pending(node: &mut crate::types::Node) {
    use std::io::Write as _;
    match node {
        crate::types::Node::Leaf(p) => {
            if p.cpr_pending
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                let (r, c) = p
                    .term
                    .lock()
                    .map(|g| g.screen().cursor_position())
                    .unwrap_or((0, 0));
                let response = format!("\x1b[{};{}R", r + 1, c + 1);
                let _ = p.writer.write_all(response.as_bytes());
                let _ = p.writer.flush();
            }
        }
        crate::types::Node::Split { children, .. } => {
            for c in children {
                drain_cpr_pending(c);
            }
        }
    }
}

/// Issue #597: write any DA1/DA2/DSR/DECRQM answers this pane's parser thread
/// composed into the pane's PTY input.
///
/// The PTY writer, not `mouse_inject::send_vt_response`: at pane startup the
/// party waiting for the DA1 answer is the console host itself (OpenConsole
/// opens with `ESC[1t ESC[c` and parks the child's console connect until the
/// answer arrives on the input pipe), and a console input record would never
/// reach it.  It is the same path `drain_cpr_pending` above uses for ESC[6n,
/// which is measured working under both the inbox host and OpenConsole.
pub(crate) fn drain_device_replies(node: &mut crate::types::Node) {
    use std::io::Write as _;
    match node {
        crate::types::Node::Leaf(p) => {
            if let Some(bytes) = crate::types::take_device_replies(p.id) {
                let _ = p.writer.write_all(&bytes);
                let _ = p.writer.flush();
            }
        }
        crate::types::Node::Split { children, .. } => {
            for c in children {
                drain_device_replies(c);
            }
        }
    }
}

/// Issue #473: format an RGB triple as the xterm 16-bit-per-channel reply
/// payload (`rgb:RRRR/GGGG/BBBB`), scaling 8-bit values by duplication.
fn x11_rgb((r, g, b): (u8, u8, u8)) -> String {
    format!("rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}")
}

/// Issue #473: answer terminal color queries detected in a pane's output.
///
/// `bits` is the pane's drained `color_query_pending` bitmask.  Delivery is
/// split by sequence type because ConPTY treats them differently on the
/// child-input path (verified on Win11 26200, WT 1.24):
///   * CSI replies (`?997;Nn`) pass through a normal pipe write intact — the
///     same path the ESC[6n CPR responder uses.
///   * Complete OSC replies written to the pseudoconsole input pipe are
///     consumed by ConPTY before the child sees them, so they are injected
///     as console KEY_EVENT records via WriteConsoleInputW instead
///     (`send_vt_response`).
///
/// When injection fails on Windows the OSC reply is LOST, and since #597 the
/// code says so rather than writing it to the pipe anyway.  A reporter on
/// 19045 whose injection was failing saw the colour replies never arrive and
/// read the pipe write as a fallback that was also broken.  Measured on 26200
/// with the `PSMUX_FAKE_INJECT_FAIL` seam and again with a plain write to a
/// live pane's input pipe: a `PLAIN` payload and a CSI reply arrive byte for
/// byte, a complete OSC arrives as nothing at all, and a DCS arrives as a bare
/// `ESC` with its body eaten.  So the pipe was never a second channel for an
/// OSC on this platform, and for a DCS it is worse than none: a lone `ESC` is
/// an Escape keypress to whatever is reading the pane.  The pipe write is kept
/// only where it is the sole channel and is known to work: no child pid, and
/// non-Windows, where nothing filters the pipe.
///
/// ConPTY also consumes the OSC 10;?/11;? QUERIES on the output path, so they
/// normally never reach psmux.  Applications that need the full picture
/// (GitHub Copilot CLI) issue fg/bg/palette queries as one burst; when the
/// palette burst is observed (index 0 queried), the fg/bg replies they are
/// simultaneously waiting for are included as well.
pub(crate) fn answer_color_queries(
    bits: u32,
    writer: &mut dyn std::io::Write,
    child_pid: Option<u32>,
    colors: &crate::types::HostColors,
) {
    answer_color_queries_for_pane(bits, writer, child_pid, colors, None);
}

/// Issue #685: as [`answer_color_queries`], with the asking pane's own OSC 4
/// palette taking precedence over the host terminal's, the way tmux answers
/// from `wp->palette` first.
pub(crate) fn answer_color_queries_for_pane(
    bits: u32,
    writer: &mut dyn std::io::Write,
    child_pid: Option<u32>,
    colors: &crate::types::HostColors,
    pane_palette: Option<[Option<(u8, u8, u8)>; 16]>,
) {
    if bits == 0 { return; }
    let (scheme, osc) = build_color_replies_for_pane(bits, colors, pane_palette);
    if let Some(scheme) = scheme {
        let _ = writer.write_all(scheme.as_bytes());
        let _ = writer.flush();
    }
    if osc.is_empty() { return; }
    let mut delivered = false;
    if let Some(pid) = child_pid {
        delivered = crate::platform::mouse_inject::send_vt_reply(pid, &osc);
    }
    if delivered { return; }
    match osc_delivery_fallback(child_pid.is_some()) {
        OscFallback::Pipe => {
            let _ = writer.write_all(osc.as_bytes());
            let _ = writer.flush();
        }
        OscFallback::Lost => {
            crate::platform::mouse_inject::log_lost_reply(child_pid, "OSC colour", osc.len());
        }
    }
}

/// What to do with an OSC reply that console injection could not deliver.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum OscFallback {
    /// Write it to the pane's input pipe: the only channel there is, and one
    /// that carries an OSC everywhere except in front of a ConPTY.
    Pipe,
    /// Nothing left to try.  ConPTY consumes an OSC written to a pane's input
    /// pipe (measured), so writing it there would deliver nothing and can leak
    /// a stray control byte into the pane.  Say it was lost instead.
    Lost,
}

/// Issue #597: pick the fallback for an OSC reply injection could not deliver.
///
/// `had_pid` is false when psmux never learned the pane child's process id, in
/// which case injection was never attempted and the pipe is all there is.
pub(crate) fn osc_delivery_fallback(had_pid: bool) -> OscFallback {
    if cfg!(windows) && had_pid { OscFallback::Lost } else { OscFallback::Pipe }
}

/// Build the reply strings for a color-query bitmask: the CSI scheme reply
/// (`?997;Nn`, separate because it may be pipe-written) and the concatenated
/// OSC 10/11/4 replies, in query order.
pub(crate) fn build_color_replies(
    bits: u32,
    colors: &crate::types::HostColors,
) -> (Option<String>, String) {
    build_color_replies_for_pane(bits, colors, None)
}

/// Issue #685: the same reply, but answering palette queries from the asking
/// pane's OWN OSC 4 palette when it has an entry for that index.
///
/// tmux does exactly this: `input_osc_4` (`input.c:2947`) calls
/// `colour_palette_get` on the pane's palette and replies from it, and only
/// when the pane has no entry does it forward the question to the real
/// terminal (`input_add_request`, `INPUT_REQUEST_PALETTE`).  Without the
/// override a pane that had just set index 4 to `#000080` would be told the
/// outer terminal's `#0037DA`, which is the same mismatch #685 is about.
pub(crate) fn build_color_replies_for_pane(
    bits: u32,
    colors: &crate::types::HostColors,
    pane_palette: Option<[Option<(u8, u8, u8)>; 16]>,
) -> (Option<String>, String) {
    // Light/dark scheme query: CSI ?996n → CSI ?997;1n (dark) / ?997;2n (light).
    let scheme = if bits & crate::types::COLOR_QUERY_SCHEME != 0 {
        Some(format!("\x1b[?997;{}n", if colors.is_dark() { 1 } else { 2 }))
    } else {
        None
    };
    let mut osc = String::new();
    let burst = bits & 1 != 0; // palette index 0 queried → full-burst app
    if (bits & crate::types::COLOR_QUERY_FG != 0 || burst) && colors.fg.is_some() {
        osc.push_str(&format!("\x1b]10;{}\x1b\\", x11_rgb(colors.fg.unwrap())));
    }
    if (bits & crate::types::COLOR_QUERY_BG != 0 || burst) && colors.bg.is_some() {
        osc.push_str(&format!("\x1b]11;{}\x1b\\", x11_rgb(colors.bg.unwrap())));
    }
    for i in 0..16usize {
        if bits & (1u32 << i) != 0 {
            // The pane's own entry wins; the host's is the fallback.
            let own = pane_palette.and_then(|p| p[i]);
            if let Some(rgb) = own.or(colors.palette[i]) {
                osc.push_str(&format!("\x1b]4;{};{}\x1b\\", i, x11_rgb(rgb)));
            }
        }
    }
    (scheme, osc)
}

/// Issue #556: best-effort synchronous answer, called from the pane READER
/// thread the moment a color query is detected in the ConPTY output stream.
///
/// The server-loop path adds a coalescing wait (1-8ms) plus a loop tick on
/// top of ConPTY's forward latency; on hosts whose conhost forwards OSC
/// 10;?/11;? to us (older builds), that total lands the reply AFTER the
/// app's startup probe window has closed — yazi then re-parses the reply as
/// an interactive `shell` action (issue #556).  Answering here, straight off
/// the read, is the earliest point psmux can physically respond.
///
/// Everything (scheme + OSC replies) is injected as one
/// `WriteConsoleInputW` batch so the replies arrive in query order.  Returns
/// true when delivered (or nothing needed answering); false means the caller
/// must fall back to the pending-bits → server-loop pipe path.
pub(crate) fn answer_color_queries_sync(
    bits: u32,
    child_pid: Option<u32>,
    colors: &crate::types::HostColors,
    pane_id: usize,
) -> bool {
    if bits == 0 { return true; }
    let (scheme, osc) =
        build_color_replies_for_pane(bits, colors, crate::types::pane_palette(pane_id));
    let combined = format!("{}{}", scheme.as_deref().unwrap_or(""), osc);
    if combined.is_empty() { return true; }
    match child_pid {
        Some(pid) => crate::platform::mouse_inject::send_vt_reply(pid, &combined),
        None => false,
    }
}

/// Issue #473: walk a pane tree and answer any pending terminal color queries.
/// Mirrors `drain_cpr_pending`.
pub(crate) fn drain_color_queries(node: &mut crate::types::Node, colors: &crate::types::HostColors) {
    match node {
        crate::types::Node::Leaf(p) => {
            let bits = p.color_query_pending.swap(0, std::sync::atomic::Ordering::AcqRel);
            if bits != 0 {
                let own = crate::types::pane_palette(p.id);
                answer_color_queries_for_pane(bits, &mut *p.writer, p.child_pid, colors, own);
            }
        }
        crate::types::Node::Split { children, .. } => {
            for c in children {
                drain_color_queries(c, colors);
            }
        }
    }
}

/// Complete list of supported tmux-compatible commands (for list-commands).
pub(crate) const TMUX_COMMANDS: &[&str] = &[
    "attach-session (attach)",
    "bind-key (bind)",
    "break-pane (breakp)",
    "capture-pane (capturep)",
    "choose-buffer (chooseb)",
    "choose-client",
    "choose-session",
    "choose-tree",
    "choose-window",
    "clear-history (clearhist)",
    "clear-prompt-history (clearphist)",
    "clock-mode",
    "command-prompt",
    "confirm-before (confirm)",
    "copy-mode",
    "customize-mode",
    "delete-buffer (deleteb)",
    "detach-client (detach)",
    "display-menu (menu)",
    "display-message (display)",
    "display-panes (displayp)",
    "display-popup (popup)",
    "find-window (findw)",
    "has-session (has)",
    "if-shell (if)",
    "join-pane (joinp)",
    "kill-pane (killp)",
    "kill-server",
    "kill-session",
    "kill-window (killw)",
    "last-pane (lastp)",
    "last-window (last)",
    "link-window (linkw)",
    "list-buffers (lsb)",
    "list-clients (lsc)",
    "list-commands (lscm)",
    "list-keys (lsk)",
    "list-panes (lsp)",
    "list-sessions (ls)",
    "list-windows (lsw)",
    "load-buffer (loadb)",
    "lock-client (lockc)",
    "lock-server (lock)",
    "lock-session (locks)",
    "move-pane (movep)",
    "move-window (movew)",
    "new-session (new)",
    "new-window (neww)",
    "next-layout (nextl)",
    "next-window (next)",
    "paste-buffer (pasteb)",
    "pipe-pane (pipep)",
    "previous-layout (prevl)",
    "previous-window (prev)",
    "refresh-client (refresh)",
    "rename-session (rename)",
    "rename-window (renamew)",
    "resize-pane (resizep)",
    "resize-window (resizew)",
    "respawn-pane (respawnp)",
    "respawn-window (respawnw)",
    "rotate-window (rotatew)",
    "run-shell (run)",
    "save-buffer (saveb)",
    "select-layout (selectl)",
    "select-pane (selectp)",
    "select-window (selectw)",
    "send-keys (send)",
    "send-prefix",
    "server-info (info)",
    "set-buffer (setb)",
    "set-environment (setenv)",
    "set-hook",
    "set-option (set)",
    "set-window-option (setw)",
    "show-buffer (showb)",
    "show-environment (showenv)",
    "show-hooks",
    "show-messages (showmsgs)",
    "show-options (show)",
    "show-prompt-history (showphist)",
    "show-window-options (showw)",
    "source-file (source)",
    "split-window (splitw)",
    "start-server (start)",
    "suspend-client (suspendc)",
    "swap-pane (swapp)",
    "swap-window (swapw)",
    "switch-client (switchc)",
    "unbind-key (unbind)",
    "unlink-window (unlinkw)",
    "wait-for (wait)",
];

#[cfg(test)]
#[path = "../../tests-rs/test_live_client_styles.rs"]
mod tests_live_client_styles;

#[cfg(test)]
#[path = "../../tests-rs/test_render_path_async_format.rs"]
mod tests_render_path_async_format;

#[cfg(test)]
#[path = "../../tests-rs/test_issue633_followup_render_fallback.rs"]
mod tests_issue633_followup_render_fallback;

#[cfg(test)]
#[path = "../../tests-rs/test_issue556_color_reply_order.rs"]
mod tests_issue556_color_reply_order;

#[cfg(test)]
#[path = "../../tests-rs/test_issue559_monitor_silence.rs"]
mod tests_issue559_monitor_silence;

#[cfg(test)]
#[path = "../../tests-rs/test_issue658_rename_walk_activity_gate.rs"]
mod tests_issue658_rename_walk_activity_gate;
