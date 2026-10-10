// format.rs — tmux-compatible format expansion engine
//
// Supports: variables, #{?cond,t,f}, #{==:a,b}, #{!=:a,b}, #{<:a,b}, etc,
// #{s/pat/rep/flags:var}, #{b:var}, #{d:var}, #{t:var}, #{l:str},
// #{E:var}, #{T:var}, #{q:var}, #{e|op|flags:a,b}, #{m/flags:pat,str},
// #{=N:var}, #{=/N/marker:var}, #{pN:var}, #{||:a,b}, #{&&:a,b},
// #{C/flags:fmt}, chained modifiers with ';',
// -F custom format for list commands.

use std::env;
use std::cell::{Cell, RefCell};

use crate::types::{AppState, Node, LayoutKind, Pane, Mode, VERSION};
use crate::tree::{split_with_gaps, get_active_pane_id, active_pane, count_panes};
use crate::config::format_key_binding;

// Thread-local override for per-pane format expansion in list-panes.
// When set to Some(pos), pane_* variables resolve for the Nth pane (0-based)
// instead of the active pane.
thread_local! {
    static PANE_POS_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
    static BUFFER_IDX_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
    static NAMED_BUFFER_OVERRIDE: RefCell<Option<String>> = const { RefCell::new(None) };
    static CLIENT_OVERRIDE: Cell<Option<u64>> = const { Cell::new(None) };
    // Set while expanding a format the way tmux's format_expand does (no
    // strftime pass). See `expand_format_untimed`.
    static NO_STRFTIME: Cell<bool> = const { Cell::new(false) };
    // Set while a held standby answers a client (see `with_sessionless`).
    static SESSIONLESS: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with formats expanded as on a server that has no session.
///
/// A held standby (`start-server` + `exit-empty off`, issue #734) is the
/// namespace's empty server, but it carries the window and pane it will hand to
/// the next `new-session`. tmux has no session there, so a command's format has
/// no session, window or pane to draw on (CMD_FIND_CANFAIL leaves them unset in
/// cmd-display-message.c) and `#{session_name}` is empty. Expanding against the
/// standby's own state printed its internal name `__warm__` and its spare
/// window. Server scope variables (`#{pid}`, `#{host}`, options) still resolve.
pub fn with_sessionless<R>(on: bool, f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SESSIONLESS.set(self.0);
        }
    }
    let _restore = Restore(SESSIONLESS.replace(on));
    f()
}

/// Set the no session mode for the requests the server loop is about to run
/// (see `with_sessionless`); returns the previous value.
pub fn set_sessionless(on: bool) -> bool {
    SESSIONLESS.replace(on)
}

/// Whether formats are being expanded for a server with no session.
pub fn sessionless() -> bool {
    SESSIONLESS.get()
}

/// Expand `f` with the `client_*` variables answering for client `cid`.
///
/// tmux builds every format tree around one client (`format_defaults(ft, c,
/// ...)`): list-clients hands it each row's client and display-message hands it
/// `-c` (cmd-list-clients.c, cmd-display-message.c:119). psmux used to answer
/// `client0` and the SERVER's pid for every client (issue #724). `None` leaves
/// the session's best client in charge, as before.
pub fn with_format_client<R>(cid: Option<u64>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) { CLIENT_OVERRIDE.set(self.0); }
    }
    let _restore = Restore(CLIENT_OVERRIDE.replace(cid));
    f()
}

/// The client a format is expanded for: the explicit one, else tmux's best
/// client for the session (cmd_find_best_client: the most recently active).
pub fn format_client(app: &AppState) -> Option<&crate::types::ClientInfo> {
    if let Some(ci) = CLIENT_OVERRIDE.get().and_then(|id| app.client_registry.get(&id)) {
        return Some(ci);
    }
    if let Some(ci) = app.latest_client_id.and_then(|id| app.client_registry.get(&id)) {
        return Some(ci);
    }
    app.client_registry.values().max_by_key(|c| (c.last_activity, c.id))
}

/// Find a client by name or tty the way tmux's cmd_find_client does. While a
/// reconnecting client's old connection is not yet reaped, two entries share
/// its name; the newest one is the live client.
pub fn find_client_by_name(app: &AppState, target: &str) -> Option<u64> {
    app.client_registry
        .values()
        .filter(|ci| crate::cli::client_spec_matches(&ci.tty_name, target))
        .map(|ci| ci.id)
        .max()
}

/// Resolve a `display-message -c` selection to a registered client id.
pub fn resolve_client_sel(app: &AppState, sel: &crate::types::ClientSel) -> Option<u64> {
    match sel {
        crate::types::ClientSel::Id(id) => app.client_registry.contains_key(id).then_some(*id),
        crate::types::ClientSel::Name(n) => find_client_by_name(app, n),
    }
}

/// `#{client_flags}`, in the order tmux's server_client_get_flags lists them.
pub fn client_flags_string(ci: &crate::types::ClientInfo) -> String {
    let mut flags: Vec<&str> = vec!["attached"];
    if ci.focused { flags.push("focused"); }
    if ci.is_control { flags.push("control-mode"); }
    if ci.readonly { flags.push("read-only"); }
    // Every psmux client speaks UTF-8 (the client forces the console code page).
    flags.push("UTF-8");
    flags.join(",")
}

/// Seconds since the Unix epoch of a moment `ago` in the past.
fn epoch_secs_ago(ago: std::time::Duration) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    now - ago.as_secs() as i64
}

/// tmux's `LIST_CLIENTS_TEMPLATE` (cmd-list-clients.c). The `[user ...]` part,
/// shown only for a client of another uid, is left out: Windows has no uid and
/// every client of a psmux server runs as its owner.
pub fn default_list_clients_format() -> &'static str {
    "#{client_name}: #{session_name} [#{client_width}x#{client_height} #{client_termname}] #{?client_flags,(,}#{client_flags}#{?client_flags,),}"
}

/// One line per registered client, oldest first, each expanded for its own
/// client. A filter that expands false drops the row (`list-clients -f`).
pub fn format_list_clients(app: &AppState, fmt: &str, filter: Option<&str>) -> String {
    let mut clients: Vec<&crate::types::ClientInfo> = app.client_registry.values().collect();
    clients.sort_by_key(|c| c.id);
    let mut out = String::new();
    for ci in clients {
        let line = with_format_client(Some(ci.id), || {
            if let Some(f) = filter {
                if !format_truthy(&expand_format(f, app)) {
                    return None;
                }
            }
            Some(expand_format(fmt, app))
        });
        if let Some(line) = line {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// tmux's format_true: empty and "0" are false, anything else is true.
fn format_truthy(s: &str) -> bool {
    !s.is_empty() && s != "0"
}

/// Set the buffer index for per-buffer format expansion in list-buffers -F.
pub fn set_buffer_idx_override(idx: Option<usize>) {
    BUFFER_IDX_OVERRIDE.set(idx);
}

/// Set the named buffer override for per-buffer format expansion in list-buffers -F.
pub fn set_named_buffer_override(name: Option<String>) {
    NAMED_BUFFER_OVERRIDE.with(|c| *c.borrow_mut() = name);
}

// ─────────────────── tmux window_layout generation ────────────────────

/// Generate a tmux-compatible window_layout string from the pane tree.
/// Format: `<checksum>,<layout_body>`
/// Body examples:
///   Single pane:  `80x24,0,0,0`
///   Horiz split:  `80x24,0,0{40x24,0,0,0,39x24,41,0,1}`
///   Vert split:   `80x24,0,0[80x12,0,0,0,80x11,0,13,1]`
pub fn generate_window_layout(node: &Node, area: ratatui::prelude::Rect) -> String {
    let body = layout_node(node, area);
    let checksum = tmux_layout_checksum(&body);
    format!("{:04x},{}", checksum, body)
}

fn layout_node(node: &Node, area: ratatui::prelude::Rect) -> String {
    match node {
        Node::Leaf(pane) => {
            // WxH,X,Y,pane_id
            format!("{}x{},{},{},{}", area.width, area.height, area.x, area.y, pane.id)
        }
        Node::Split { kind, sizes, children } => {
            let is_horizontal = matches!(*kind, LayoutKind::Horizontal);
            let effective_sizes: Vec<u16> = if sizes.len() == children.len() {
                sizes.clone()
            } else {
                vec![(100 / children.len().max(1)) as u16; children.len()]
            };
            let rects = split_with_gaps(is_horizontal, &effective_sizes, area);
            
            let (open, close) = if is_horizontal { ('{', '}') } else { ('[', ']') };
            
            let mut inner = String::new();
            for (i, child) in children.iter().enumerate() {
                if i > 0 { inner.push(','); }
                if i < rects.len() {
                    inner.push_str(&layout_node(child, rects[i]));
                }
            }
            
            format!("{}x{},{},{}{}{}{}", area.width, area.height, area.x, area.y, open, inner, close)
        }
    }
}

/// Compute tmux layout checksum (16-bit CSUM as used by tmux src/layout-custom.c).
fn tmux_layout_checksum(layout: &str) -> u16 {
    let mut csum: u16 = 0;
    for &b in layout.as_bytes() {
        csum = (csum >> 1) | ((csum & 1) << 15); // rotate right 1 bit
        csum = csum.wrapping_add(b as u16);
    }
    csum
}

// ─────────────────────────── public API ───────────────────────────

/// Expand tmux format strings for the active window.
#[inline]
pub fn expand_format(fmt: &str, app: &AppState) -> String {
    expand_format_for_window(fmt, app, app.active_idx)
}

/// Expand a format WITHOUT the strftime pass, the way tmux's format_expand
/// (format_single, format_single_from_target) does. tmux only runs strftime
/// for format_expand_time callers (status line, display-message, pane
/// titles); the shell command of if-shell and run-shell and a set-option -F
/// value go through plain format_expand (cmd-if-shell.c:86,
/// cmd-run-shell.c:144, cmd-set-option.c). A shell command is full of `%`
/// that is not strftime: cmd's `%VAR%`, PowerShell's `%` alias, `100%`.
pub fn expand_format_untimed(fmt: &str, app: &AppState) -> String {
    let prev = NO_STRFTIME.replace(true);
    let out = expand_format(fmt, app);
    NO_STRFTIME.set(prev);
    out
}

/// The REAL (user-visible) active window index. While a temporary -t focus
/// is applied for command targeting, `active_idx` points at the target
/// window; the pre-switch index saved in `temp_focus_saved_active` is what
/// "active" means to the user, so `#{window_active}` and the `*` flag must
/// compare against it (issue #551 — `display-message -t <win>` reported
/// every targeted window as active).
fn real_active_idx(app: &AppState) -> usize {
    app.temp_focus_saved_active.unwrap_or(app.active_idx)
}

/// Expand tmux format strings for a specific window index.
pub fn expand_format_for_window(fmt: &str, app: &AppState, win_idx: usize) -> String {
    // No session means no window or pane either: the shorthands below that
    // describe them are empty, as in tmux (see `no_session_var`).
    let no_session = sessionless();
    let shorthand_idx = if no_session { usize::MAX } else { win_idx };
    let mut result = String::with_capacity(fmt.len() * 2);
    let bytes = fmt.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    // Whether the original format contains strftime %-sequences.
    // If so, we need to escape '%' in expanded variable content so chrono
    // only interprets the real strftime codes from the original format.
    let has_strftime = !NO_STRFTIME.get() && fmt.contains('%');

    while i < len {
        if bytes[i] == b'#' && i + 1 < len {
            if bytes[i + 1] == b'{' {
                // #{...} expression
                if let Some(close) = find_matching_brace(fmt, i + 2) {
                    let inner = &fmt[i + 2..close];
                    let expanded = expand_expression(inner, app, win_idx);
                    if has_strftime {
                        result.push_str(&escape_strftime_percent(&expanded));
                    } else {
                        result.push_str(&expanded);
                    }
                    i = close + 1;
                    continue;
                }
            }
            if bytes[i + 1] == b'(' {
                // #(command) — shell command execution (tmux compat)
                if let Some(end) = fmt[i + 2..].find(')') {
                    let cmd = &fmt[i + 2..i + 2 + end];
                    let output = run_shell_command(cmd, app);
                    if has_strftime {
                        result.push_str(&escape_strftime_percent(&output));
                    } else {
                        result.push_str(&output);
                    }
                    i = i + 2 + end + 1;
                    continue;
                }
            }
            if bytes[i + 1] == b',' {
                // Escaped comma inside conditional branches
                result.push(',');
                i += 2;
                continue;
            }
            // Shorthand #X
            match bytes[i + 1] {
                b'S' => {
                    if no_session {
                        // empty, like #{session_name} with no session
                    } else if has_strftime {
                        result.push_str(&escape_strftime_percent(&app.session_name));
                    } else {
                        result.push_str(&app.session_name);
                    }
                    i += 2; continue;
                }
                b'I' => {
                    if !no_session {
                        let n = if win_idx < app.windows.len() { app.win_display_index(win_idx) } else { 0 };
                        result.push_str(&n.to_string());
                    }
                    i += 2; continue;
                }
                b'W' => {
                    if let Some(w) = app.windows.get(shorthand_idx) {
                        if has_strftime {
                            result.push_str(&escape_strftime_percent(&w.name));
                        } else {
                            result.push_str(&w.name);
                        }
                    }
                    i += 2; continue;
                }
                b'T' => {
                    if let Some(w) = app.windows.get(shorthand_idx) {
                        let title = active_pane(&w.root, &w.active_path)
                            .map(|p| &p.title[..])
                            .filter(|t| !t.is_empty())
                            .unwrap_or("");
                        let title = if title.is_empty() { hostname_cached() } else { title.to_string() };
                        if has_strftime {
                            result.push_str(&escape_strftime_percent(&title));
                        } else {
                            result.push_str(&title);
                        }
                    }
                    i += 2; continue;
                }
                b'P' => {
                    if let Some(w) = app.windows.get(shorthand_idx) {
                        let active_id = get_active_pane_id(&w.root, &w.active_path).unwrap_or(0);
                        let pos = crate::tree::get_pane_position_in_window(&w.root, active_id).unwrap_or(0);
                        result.push_str(&(pos + app.pane_base_index).to_string());
                    }
                    i += 2; continue;
                }
                b'F' => {
                    if no_session {}
                    else if win_idx == real_active_idx(app) { result.push('*'); }
                    else if win_idx == app.last_window_idx { result.push('-'); }
                    i += 2; continue;
                }
                b'H' | b'h' => {
                    if has_strftime {
                        result.push_str(&escape_strftime_percent(&hostname_cached()));
                    } else {
                        result.push_str(&hostname_cached());
                    }
                    i += 2; continue;
                }
                b'D' => {
                    // tmux: #D = unique pane id (like %0, %1)
                    if let Some(w) = app.windows.get(shorthand_idx) {
                        let active_id = get_active_pane_id(&w.root, &w.active_path).unwrap_or(0);
                        if has_strftime {
                            // Escape the '%' so chrono doesn't misinterpret %0, %1, etc.
                            result.push_str(&format!("%%{}", active_id));
                        } else {
                            result.push_str(&format!("%{}", active_id));
                        }
                    }
                    i += 2; continue;
                }
                b'#' => { result.push('#'); i += 2; continue; }
                _ => {}
            }
        }
        // Advance by full UTF-8 character (not single byte) to preserve
        // multi-byte chars like ▶ (U+25B6, 3 bytes) and ◀ (U+25C0).
        if let Some(ch) = fmt[i..].chars().next() {
            result.push(ch);
            i += ch.len_utf8();
        } else {
            i += 1;
        }
    }
    // Expand strftime %-sequences only if the ORIGINAL format contained '%'
    if has_strftime && result.contains('%') {
        // Use write! to catch chrono format errors instead of panicking.
        // Expanded variable content has '%' escaped to '%%' above, so chrono
        // will only interpret the real strftime codes from the original format.
        use std::fmt::Write;
        let formatted = chrono::Local::now().format(&result);
        let mut buf = String::with_capacity(result.len() + 32);
        if write!(buf, "{}", formatted).is_ok() {
            result = buf;
        }
        // On error, keep the pre-strftime result as-is
    }
    result
}

// NOTE: this doc block used to sit here, detached, describing `run_shell_command`
// as if it were unconditionally async — which it is not, and has not been since
// d981d94. `cargo` flagged it as an unused doc comment and it was left in place;
// meanwhile it was the only documentation anyone reading this file would find,
// and it described the wrong half of the function. It now lives on
// `run_shell_command` itself, covering BOTH branches.
thread_local! {
    // When true, `#()` expansion is ASYNC (spawn a background worker, return the
    // cached value, apply the result on a later repaint). Default false = SYNC.
    static FORMAT_ASYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// RAII guard that puts `#()` expansion in ASYNC mode for its lifetime.
///
/// ONLY the periodic status-bar / render path should use this: that path runs
/// every server-loop tick, so a slow `#(command)` there must never block the
/// loop (issue #272 / PR #477). Every other caller — one-shot `display-message
/// -p '#(cmd)'`, hooks, etc. — expands `#()` synchronously so a single expansion
/// returns the command's real output immediately. PR #477 made ALL expansion
/// async, which silently broke one-shot `#()` (it returned the empty pre-first-
/// result value and never repainted). This restores tmux-parity: async only for
/// the status bar, synchronous everywhere else.
///
/// The periodic render path owns its guards inside
/// `server::helpers::expand_status_formats` and `list_windows_json_with_tabs`,
/// so a new call site cannot forget one. The state-aware window preview uses a
/// separate guard because it expands one requested window outside those
/// helpers.
pub struct AsyncFormatGuard(bool);
impl AsyncFormatGuard {
    pub fn new() -> Self {
        // Save and restore the PREVIOUS value rather than unconditionally
        // clearing on drop. If two guarded helpers ever nest, a blind
        // `set(false)` in the inner Drop would silently drop the outer region
        // back to synchronous — re-creating the exact bug this guard exists to
        // prevent, but only in the nested case and therefore much harder to
        // spot. Restoring makes nesting a non-event.
        let prev = FORMAT_ASYNC.with(|c| c.replace(true));
        AsyncFormatGuard(prev)
    }
}
impl Drop for AsyncFormatGuard {
    fn drop(&mut self) {
        let prev = self.0;
        FORMAT_ASYNC.with(|c| c.set(prev));
    }
}

/// Expand `#(command)` (tmux compatibility). Has two modes, selected by the
/// thread-local [`FORMAT_ASYNC`] flag that [`AsyncFormatGuard`] sets:
///
/// - **Sync (the default, no guard active):** run the command inline with
///   `Command::output()` and return its real stdout. Correct — and required —
///   for one-shot callers like `display-message -p '#(cmd)'`, which have no
///   later repaint to pick up an async result. It BLOCKS the calling thread for
///   as long as the child runs.
/// - **Async (under an [`AsyncFormatGuard`]):** return the last cached stdout
///   and, when it is missing or older than the TTL, spawn a background worker to
///   refresh it. Never blocks; the worker's result is delivered over
///   `format_job_tx` and applied by the drain in the server loop, which then
///   repaints. Before the first result the expansion is empty.
///
/// The async cache is `app.format_shell_cache`, keyed by command string, TTL =
/// `status-interval` seconds (1s floor). A command already in flight is not
/// re-spawned, so a burst of pushes runs it at most once per TTL (issue #272).
///
/// The sync branch writes that cache but deliberately does not read it: a
/// one-shot caller asked for the command's output *now*, and serving it a value
/// from up to `status-interval` ago would make `#(date)` and friends silently
/// stale. That asymmetry is why the guard has to be right — anything on the
/// render path that misses it does not merely lose caching, it blocks the loop.
fn run_shell_command(cmd: &str, app: &AppState) -> String {
    // Synchronous one-shot expansion (the default): run the command inline and
    // return its real output. A one-shot `display-message -p '#(cmd)'` has no
    // later repaint to pick up an async result, so it must block here.
    if !FORMAT_ASYNC.with(|c| c.get()) {
        use std::process::Command;
        use crate::platform::HideWindowCommandExt;
        let output = if cfg!(windows) {
            Command::new("cmd").args(["/C", cmd]).hide_window().output()
        } else {
            Command::new("sh").args(["-c", cmd]).output()
        };
        let value = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        };
        // Refresh the cache so a following async status render starts fresh.
        if let Ok(mut guard) = app.format_shell_cache.lock() {
            guard.insert(cmd.to_string(), crate::types::ShellEntry {
                at: std::time::Instant::now(), value: value.clone(), running: false,
            });
        }
        return value;
    }

    let ttl = std::time::Duration::from_secs(app.status_interval.max(1));

    let mut guard = match app.format_shell_cache.lock() {
        Ok(g) => g,
        Err(_) => return String::new(),
    };

    // Fast path: a fresh value, returned without spawning. Otherwise capture the
    // last output + freshness base and decide whether to spawn a refresh worker.
    let (last_value, base_at, should_spawn) = match guard.get(cmd) {
        Some(e) => {
            if e.at.elapsed() < ttl {
                return e.value.clone();
            }
            (e.value.clone(), e.at, !e.running)
        }
        None => (String::new(), std::time::Instant::now(), true),
    };

    if should_spawn {
        if let Some(tx) = app.format_job_tx.as_ref() {
            let tx = tx.clone();
            let cmd_owned = cmd.to_string();
            // Mark in-flight (keeping the old freshness base) before spawning so
            // other #() expansions in the same push don't double-spawn.
            guard.insert(cmd.to_string(), crate::types::ShellEntry { at: base_at, value: last_value.clone(), running: true });
            drop(guard);
            std::thread::spawn(move || {
                use std::process::Command;
                use crate::platform::HideWindowCommandExt;
                let output = if cfg!(windows) {
                    Command::new("cmd").args(["/C", &cmd_owned]).hide_window().output()
                } else {
                    Command::new("sh").args(["-c", &cmd_owned]).output()
                };
                let value = match output {
                    Ok(o) if o.status.success() => {
                        String::from_utf8_lossy(&o.stdout).trim().to_string()
                    }
                    _ => String::new(),
                };
                let _ = tx.send((cmd_owned, value));
            });
        }
    }

    last_value
}

/// Escape '%' to '%%' in expanded variable content so chrono's strftime
/// doesn't misinterpret user content (pane titles, pane IDs, etc.) as
/// format specifiers.
#[inline]
fn escape_strftime_percent(s: &str) -> String {
    if s.contains('%') {
        s.replace('%', "%%")
    } else {
        s.to_string()
    }
}

/// Expand format for a specific pane (used by list-panes -F, loops, etc).
pub fn expand_format_for_pane(
    fmt: &str,
    app: &AppState,
    win_idx: usize,
    pane_pos: usize,
) -> String {
    PANE_POS_OVERRIDE.set(Some(pane_pos));
    let result = expand_format_for_window(fmt, app, win_idx);
    PANE_POS_OVERRIDE.set(None);
    result
}

/// Like `expand_format_for_pane` but resolves the pane by its global ID
/// (e.g. from a bare `%N` -t target). Falls back to the active pane if no
/// pane with that id exists. (Issue #332.)
pub fn expand_format_for_pane_by_id(
    fmt: &str,
    app: &AppState,
    pane_id: usize,
) -> String {
    if let Some((win_idx, pos)) = crate::tree::find_pane_by_id_global(app, pane_id) {
        expand_format_for_pane(fmt, app, win_idx, pos)
    } else if let Some((win_idx, pos)) = app.windows.iter().enumerate().find_map(|(wi, w)| {
        // A floating pane: its position follows the tiled panes (#767).
        w.floating.iter().position(|fp| fp.id == pane_id)
            .map(|fi| (wi, count_panes(&w.root) + fi))
    }) {
        expand_format_for_pane(fmt, app, win_idx, pos)
    } else {
        expand_format(fmt, app)
    }
}

// ─────────────────── expression dispatcher ───────────────────────

/// Expand a `#{...}` expression (the content between `#{` and `}`).
fn expand_expression(expr: &str, app: &AppState, win_idx: usize) -> String {
    if expr.is_empty() {
        return String::new();
    }

    let first = expr.as_bytes()[0];

    // Conditional: #{?cond,true,false}
    if first == b'?' {
        return expand_conditional(&expr[1..], app, win_idx);
    }

    // Comparison operators at top level: #{==:fmt,fmt}, #{!=:...}, #{<:...}, etc.
    if let Some(val) = try_comparison_op(expr, app, win_idx) {
        return val;
    }

    // Boolean: #{||:a,b} and #{&&:a,b}
    if let Some(rest) = expr.strip_prefix("||:") {
        return expand_boolean_or(rest, app, win_idx);
    }
    if let Some(rest) = expr.strip_prefix("&&:") {
        return expand_boolean_and(rest, app, win_idx);
    }

    // Loop expansion: #{W:format} = iterate windows, #{P:format} = iterate panes, #{S:format} = iterate sessions
    if expr.len() >= 3 && expr.as_bytes()[1] == b':' {
        // With no session there is nothing to loop over (tmux
        // format_loop_sessions/windows/panes walk empty lists).
        if sessionless() && matches!(first, b'W' | b'P' | b'S') {
            return String::new();
        }
        match first {
            b'W' => {
                // #{W:fmt} — expand fmt once per window, join with spaces
                // #{W:fmt,current_fmt} — use fmt for non-active, current_fmt for active window
                let inner_fmt = &expr[2..];
                let args = split_at_depth0(inner_fmt, b',');
                let (normal_fmt, current_fmt) = if args.len() >= 2 {
                    (args[0].as_str(), args[1].as_str())
                } else {
                    (inner_fmt, inner_fmt)
                };
                let two_arg = args.len() >= 2;
                let mut parts = Vec::new();
                for wi in 0..app.windows.len() {
                    let fmt = if wi == real_active_idx(app) { current_fmt } else { normal_fmt };
                    parts.push(expand_format_for_window(fmt, app, wi));
                }
                // Two-argument form joins without separator (user controls layout),
                // single-argument form joins with spaces (backward compatible).
                let sep = if two_arg { "" } else { " " };
                return parts.join(sep);
            }
            b'P' => {
                // #{P:fmt} — expand fmt once per pane in the current window
                let inner_fmt = &expr[2..];
                let mut parts = Vec::new();
                if let Some(win) = app.windows.get(win_idx) {
                    let mut pane_ids = Vec::new();
                    collect_pane_ids(&win.root, &mut pane_ids);
                    // tmux format_loop_panes walks w->panes, floats included (#767).
                    pane_ids.extend(win.floating.iter().map(|fp| fp.id));
                    for (pos, _pid) in pane_ids.iter().enumerate() {
                        PANE_POS_OVERRIDE.set(Some(pos));
                        parts.push(expand_format_for_window(inner_fmt, app, win_idx));
                        PANE_POS_OVERRIDE.set(None);
                    }
                }
                return parts.join(" ");
            }
            b'S' => {
                // #{S:fmt} — expand fmt once per session (single session in psmux)
                let inner_fmt = &expr[2..];
                return expand_format_for_window(inner_fmt, app, win_idx);
            }
            _ => {}
        }
    }

    // Modifier chain: check if there's a modifier prefix
    if let Some(result) = try_expand_modifier_chain(expr, app, win_idx) {
        return result;
    }

    // Plain variable or option name. Unknown names render empty (tmux parity).
    expand_var(expr, app, win_idx)
}

// ─────────────────── modifier chain parsing ──────────────────────

/// tmux's `format_is_end` (format.c:4754).
fn tmux_is_end(b: Option<u8>) -> bool {
    matches!(b, Some(b';') | Some(b':'))
}

/// tmux's `format_skip1` (format.c:4684): the index of the first byte of `s`
/// from `i` on that is in `end` outside any nested `#{...}`, stepping over the
/// `#,` `##` `#{` `#}` `#:` escapes.
fn tmux_skip1(s: &[u8], mut i: usize, end: &[u8]) -> Option<usize> {
    let mut brackets: i32 = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'#' && s.get(i + 1) == Some(&b'{') {
            brackets += 1;
        }
        if c == b'#' && i + 1 < s.len() && b",#{}:".contains(&s[i + 1]) {
            i += 2;
            continue;
        }
        if c == b'}' {
            brackets -= 1;
        }
        if end.contains(&c) && brackets == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Where tmux's `format_build_modifiers` (format.c:4790 to :4900) ends the
/// modifier list of `expr`: the index of the `:` it stops on, or `None` when
/// tmux abandons the list.
///
/// tmux walks the list one modifier at a time and gives up on the WHOLE list
/// at the first character it does not know (format.c:4841 `break`, then :4893
/// returns NULL), so `#{t;Z:session_name}` has no modifiers at all and the
/// complete text is looked up as one name. A modifier that takes arguments
/// wraps them in an ASCII punctuation character other than `-`; any other
/// character after it (a letter, a space, a byte of a UTF-8 sequence) starts
/// one bare argument that runs to the next `:` or `;` (format.c:4855).
///
/// The letter sets are the union of tmux 3.4's and current tmux's, so a list
/// either version accepts is accepted.
fn tmux_modifier_colon(expr: &str) -> Option<usize> {
    let s = expr.as_bytes();
    let at = |i: usize| s.get(i).copied();
    let mut cp = 0usize;
    while cp < s.len() && s[cp] != b':' {
        if s[cp] == b';' {
            cp += 1;
        }
        if cp >= s.len() {
            break;
        }
        let c = s[cp];
        if b"labcdnwETSWPOVL!<>".contains(&c) && tmux_is_end(at(cp + 1)) {
            cp += 1;
            continue;
        }
        if cp + 1 < s.len()
            && matches!(&s[cp..cp + 2], b"||" | b"&&" | b"!!" | b"!=" | b"==" | b"<=" | b">=")
            && tmux_is_end(at(cp + 2))
        {
            cp += 2;
            continue;
        }
        if !b"ImCLNPSOVst=pReqWc".contains(&c) {
            break;
        }
        if tmux_is_end(at(cp + 1)) {
            cp += 1;
            continue;
        }
        let Some(sep) = at(cp + 1) else { break };
        if !sep.is_ascii_punctuation() || sep == b'-' {
            match tmux_skip1(s, cp + 1, b":;") {
                Some(e) => { cp = e; continue; }
                None => break,
            }
        }
        let last = [sep, b';', b':'];
        cp += 1;
        loop {
            if s[cp] == sep && tmux_is_end(at(cp + 1)) {
                cp += 1;
                break;
            }
            match tmux_skip1(s, cp + 1, &last) {
                Some(e) => cp = e,
                None => break,
            }
            if tmux_is_end(at(cp)) {
                break;
            }
        }
    }
    if at(cp) == Some(b':') { Some(cp) } else { None }
}

/// Try to parse and apply modifier chain(s). Returns None if expr is a plain variable.
fn try_expand_modifier_chain(expr: &str, app: &AppState, win_idx: usize) -> Option<String> {
    let bytes = expr.as_bytes();
    let first = bytes[0];

    let Some(colon_pos) = tmux_modifier_colon(expr) else {
        // Not a modifier list tmux accepts. tmux then treats the whole text as
        // one name (format.c:4893 hands back no modifiers and the key
        // unchanged), which expands as a format when it holds `#{` and is
        // looked up as a name otherwise (format_replace's final branch): an
        // unknown name, so nothing. `#{t;Z:session_name}` is empty in tmux; it
        // used to drop the unknown `Z` and apply `t`.
        let looks_like_chain = b"labcdnwETSWPOVL!<>ImCLNPSOVst=pReqWc|&;".contains(&first);
        if looks_like_chain && find_modifier_colon(expr).is_some() {
            if expr.contains("#{") {
                return Some(expand_format_for_window(expr, app, win_idx));
            }
            return Some(String::new());
        }
        return None;
    };

    // Separators before the first modifier are skipped (format.c:4813), and a
    // list with no modifiers in it (`#{:name}`, `#{;:name}`) is a plain lookup
    // of what follows the colon.
    let mod_spec = expr[..colon_pos].trim_start_matches(';');
    let target = &expr[colon_pos + 1..];
    if mod_spec.bytes().all(|b| b == b';') {
        if target.contains("#{") {
            return Some(expand_format_for_window(target, app, win_idx));
        }
        return Some(expand_var(target, app, win_idx));
    }
    let first = mod_spec.as_bytes()[0];

    // Quick check: does this look like a modifier?
    let is_modifier_start = matches!(first,
        b't' | b'b' | b'd' | b'l' | b'E' | b'T' | b'q' | b's' | b'm' | b'C' |
        b'e' | b'p' | b'=' | b'N' | b'w'
    );

    if !is_modifier_start {
        return None;
    }

    // Special: 'l' modifier with colon — #{l:string} returns literal string
    if first == b'l' {
        return Some(target.to_string());
    }

    {

        // Parse modifier chain (separated by ';')
        let modifiers = parse_modifier_chain(mod_spec);
        if modifiers.is_empty() {
            return None;
        }

        // First, check if the first modifier is one that takes the target as a
        // format to expand (e.g. comparisons, match, math — where the target is
        // "arg1,arg2" not a variable).
        let needs_raw_target = modifiers.iter().any(|m| matches!(m,
            Modifier::MathExpr { .. } | Modifier::Match { .. }
        ));

        let mut value = if needs_raw_target {
            // Expand each comma-separated part individually
            let parts = split_at_depth0(target, b',');
            parts.iter()
                .map(|p| expand_var_or_format(p, app, win_idx))
                .collect::<Vec<_>>()
                .join(",")
        } else {
            expand_var_or_format(target, app, win_idx)
        };

        // Apply modifiers in order
        for m in &modifiers {
            value = apply_modifier(m, &value, app, win_idx);
        }

        Some(value)
    }
}

/// Find the colon that separates modifiers from the target, at brace depth 0.
fn find_modifier_colon(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut depth = 0usize;

    while i < len {
        let b = bytes[i];
        if b == b'#' && i + 1 < len && bytes[i + 1] == b'{' {
            depth += 1;
            i += 2;
            continue;
        }
        if b == b'}' && depth > 0 {
            depth -= 1;
            i += 1;
            continue;
        }
        if b == b':' && depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parsed modifier representation.
#[derive(Debug, Clone)]
enum Modifier {
    Time,
    Basename,
    Dirname,
    Expand,
    ExpandTime,
    Quote,
    Substitute { pattern: String, replacement: String, case_insensitive: bool },
    Trim(i32),
    TrimWithMarker(i32, String),
    Pad(i32),
    MathExpr { op: char, floating: bool, decimals: u32 },
    Match { regex: bool, case_insensitive: bool },
    SearchContent { _regex: bool, _case_insensitive: bool },
    Width,
    /// A modifier tmux parses and then ignores (`s` without both of its
    /// arguments): it changes nothing but keeps the list valid.
    Ignored,
}

/// Parse a modifier chain string (e.g. "s|foo|bar|;=5" ) into modifiers.
fn parse_modifier_chain(spec: &str) -> Vec<Modifier> {
    let mut modifiers = Vec::new();
    let parts = split_at_depth0(spec, b';');
    for part in &parts {
        if let Some(m) = parse_single_modifier(part) {
            modifiers.push(m);
        }
    }
    modifiers
}

/// Parse one modifier segment.
fn parse_single_modifier(spec: &str) -> Option<Modifier> {
    // Char aware: a segment that starts with a multi byte character used to
    // be sliced at byte 1 and panicked the server (#712 audit). tmux stops
    // parsing modifiers at a character it does not know (format.c
    // format_build_modifiers); the `_` arm below does the same.
    let first = spec.chars().next()?;
    let rest = &spec[first.len_utf8()..];

    match first {
        't' => Some(Modifier::Time),
        'b' => Some(Modifier::Basename),
        'd' => Some(Modifier::Dirname),
        'E' => Some(Modifier::Expand),
        'T' => Some(Modifier::ExpandTime),
        'q' => Some(Modifier::Quote),
        'w' => Some(Modifier::Width),
        '=' => {
            if rest.is_empty() { return Some(Modifier::Trim(0)); }
            let sep = rest.as_bytes()[0];
            if sep == b'/' || sep == b'|' {
                let sep_ch = sep as char;
                let inner = &rest[1..];
                let parts: Vec<&str> = inner.splitn(2, sep_ch).collect();
                let n: i32 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                let marker = parts.get(1).unwrap_or(&"").to_string();
                Some(Modifier::TrimWithMarker(n, marker))
            } else {
                let n: i32 = rest.parse().unwrap_or(0);
                Some(Modifier::Trim(n))
            }
        }
        'p' => {
            let n: i32 = rest.parse().unwrap_or(0);
            Some(Modifier::Pad(n))
        }
        's' => {
            // tmux only takes an ASCII punctuation wrapper other than `-`
            // (format.c:4855); anything else is one bare argument, and `s`
            // with fewer than two arguments is skipped while the rest of the
            // list still applies (format.c:5857). `#{sXaXbX:v}` and
            // `#{s:v}` are both just `v` in tmux.
            let Some(sep) = rest.chars().next() else { return Some(Modifier::Ignored) };
            if !sep.is_ascii_punctuation() || sep == '-' { return Some(Modifier::Ignored); }
            let inner = &rest[1..];
            let parts: Vec<&str> = inner.splitn(3, sep).collect();
            let pattern = parts.first().unwrap_or(&"").to_string();
            let replacement = parts.get(1).unwrap_or(&"").to_string();
            let flags = parts.get(2).unwrap_or(&"");
            Some(Modifier::Substitute {
                pattern,
                replacement,
                case_insensitive: flags.contains('i'),
            })
        }
        'e' => {
            // Same wrapper rule as `s`: only ASCII punctuation other than `-`
            // wraps the arguments.
            let sep = rest.chars().next()?;
            if !sep.is_ascii_punctuation() || sep == '-' { return None; }
            let inner = &rest[1..];
            let parts: Vec<&str> = inner.splitn(3, sep).collect();
            let op = parts.first().and_then(|s| s.chars().next()).unwrap_or('+');
            let flags = parts.get(1).unwrap_or(&"");
            let floating = flags.contains('f');
            let decimals: u32 = parts.get(2).and_then(|s| s.parse().ok())
                .unwrap_or(if floating { 2 } else { 0 });
            Some(Modifier::MathExpr { op, floating, decimals })
        }
        'm' => {
            let regex = rest.contains('r');
            let ci = rest.contains('i');
            Some(Modifier::Match { regex, case_insensitive: ci })
        }
        'C' => {
            let regex = rest.contains('r');
            let ci = rest.contains('i');
            Some(Modifier::SearchContent { _regex: regex, _case_insensitive: ci })
        }
        _ => None,
    }
}

/// Apply a modifier to a value.
fn apply_modifier(m: &Modifier, value: &str, app: &AppState, win_idx: usize) -> String {
    match m {
        Modifier::Time => {
            if let Ok(ts) = value.parse::<i64>() {
                if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
                    let local: chrono::DateTime<chrono::Local> = dt.into();
                    return local.format("%a %b %e %H:%M:%S %Y").to_string();
                }
            }
            value.to_string()
        }
        Modifier::Basename => {
            std::path::Path::new(value)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(value)
                .to_string()
        }
        Modifier::Dirname => {
            std::path::Path::new(value)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string()
        }
        Modifier::Expand => {
            expand_format_for_window(value, app, win_idx)
        }
        Modifier::ExpandTime => {
            let expanded = expand_format_for_window(value, app, win_idx);
            if expanded.contains('%') {
                use std::fmt::Write;
                let formatted = chrono::Local::now().format(&expanded);
                let mut buf = String::with_capacity(expanded.len() + 32);
                if write!(buf, "{}", formatted).is_ok() { buf } else { expanded }
            } else {
                expanded
            }
        }
        Modifier::Quote => {
            let mut out = String::with_capacity(value.len() * 2);
            for ch in value.chars() {
                match ch {
                    '(' | ')' | '[' | ']' | '{' | '}' | '$' | '\\' | '\'' | '"'
                    | '`' | '!' | '#' | '&' | '|' | ';' | '<' | '>' | ' ' | '\t' | '\n' => {
                        out.push('\\');
                        out.push(ch);
                    }
                    _ => out.push(ch),
                }
            }
            out
        }
        Modifier::Trim(n) => {
            let n = *n;
            if n == 0 { return value.to_string(); }
            let chars: Vec<char> = value.chars().collect();
            if n > 0 {
                let len = n as usize;
                if chars.len() > len { chars[..len].iter().collect() }
                else { value.to_string() }
            } else {
                let len = (-n) as usize;
                if chars.len() > len { chars[chars.len() - len..].iter().collect() }
                else { value.to_string() }
            }
        }
        Modifier::TrimWithMarker(n, marker) => {
            let n = *n;
            if n == 0 { return value.to_string(); }
            let chars: Vec<char> = value.chars().collect();
            if n > 0 {
                let len = n as usize;
                if chars.len() > len {
                    let mut trimmed: String = chars[..len].iter().collect();
                    trimmed.push_str(marker);
                    trimmed
                } else { value.to_string() }
            } else {
                let len = (-n) as usize;
                if chars.len() > len {
                    let mut trimmed = marker.clone();
                    trimmed.extend(chars[chars.len() - len..].iter());
                    trimmed
                } else { value.to_string() }
            }
        }
        Modifier::Pad(n) => {
            let n = *n;
            let abs_n = n.unsigned_abs() as usize;
            let chars_len = value.chars().count();
            if chars_len >= abs_n { return value.to_string(); }
            let pad = abs_n - chars_len;
            let spaces: String = " ".repeat(pad);
            if n > 0 { format!("{}{}", value, spaces) }
            else { format!("{}{}", spaces, value) }
        }
        Modifier::Substitute { pattern, replacement, case_insensitive } => {
            let re_pattern = if *case_insensitive {
                format!("(?i){}", pattern)
            } else {
                pattern.clone()
            };
            match regex::Regex::new(&re_pattern) {
                Ok(re) => re.replace(value, replacement.as_str()).to_string(),
                Err(_) => value.to_string(),
            }
        }
        Modifier::MathExpr { op, floating, decimals } => {
            let parts = split_at_depth0(value, b',');
            if parts.len() < 2 { return "0".into(); }
            if *floating {
                let a: f64 = parts[0].parse().unwrap_or(0.0);
                let b: f64 = parts[1].parse().unwrap_or(0.0);
                let r = match op {
                    '+' => a + b, '-' => a - b, '*' => a * b,
                    '/' => if b != 0.0 { a / b } else { 0.0 },
                    'm' => if b != 0.0 { a % b } else { 0.0 },
                    _ => 0.0,
                };
                format!("{:.prec$}", r, prec = *decimals as usize)
            } else {
                let a: i64 = parts[0].parse().unwrap_or(0);
                let b: i64 = parts[1].parse().unwrap_or(0);
                let r = match op {
                    '+' => a + b, '-' => a - b, '*' => a * b,
                    '/' => if b != 0 { a / b } else { 0 },
                    'm' => if b != 0 { a % b } else { 0 },
                    _ => 0,
                };
                if *decimals > 0 {
                    format!("{:.prec$}", r as f64, prec = *decimals as usize)
                } else { r.to_string() }
            }
        }
        Modifier::Match { regex, case_insensitive } => {
            let parts = split_at_depth0(value, b',');
            if parts.len() < 2 { return "0".into(); }
            let pattern = &parts[0];
            let subject = &parts[1];
            if *regex {
                let re_pat = if *case_insensitive { format!("(?i){}", pattern) }
                    else { pattern.to_string() };
                match regex::Regex::new(&re_pat) {
                    Ok(re) => if re.is_match(subject) { "1".into() } else { "0".into() },
                    Err(_) => "0".into(),
                }
            } else {
                if glob_match(pattern, subject, *case_insensitive) { "1".into() }
                else { "0".into() }
            }
        }
        Modifier::SearchContent { _regex, _case_insensitive } => {
            // #{C:pattern} — Search for pattern in pane content, return line number or empty
            let pattern = value;
            if pattern.is_empty() { return String::new(); }
            if let Some(w) = app.windows.get(win_idx) {
                if let Some(p) = active_pane(&w.root, &w.active_path) {
                    if let Ok(parser) = p.term.lock() {
                        let screen = parser.screen();
                        let re_result = if *_regex {
                            let pat = if *_case_insensitive { format!("(?i){}", pattern) } else { pattern.to_string() };
                            regex::Regex::new(&pat).ok()
                        } else {
                            let escaped = regex::escape(pattern);
                            let pat = if *_case_insensitive { format!("(?i){}", escaped) } else { escaped };
                            regex::Regex::new(&pat).ok()
                        };
                        if let Some(re) = re_result {
                            for r in 0..p.last_rows {
                                let mut row_text = String::with_capacity(p.last_cols as usize);
                                for c in 0..p.last_cols {
                                    if let Some(cell) = screen.cell(r, c) {
                                        let t = cell.contents();
                                        if t.is_empty() { row_text.push(' '); } else { row_text.push_str(t); }
                                    } else { row_text.push(' '); }
                                }
                                if re.is_match(&row_text) {
                                    return r.to_string();
                                }
                            }
                        }
                    }
                }
            }
            String::new()
        }
        Modifier::Width => {
            value.chars().count().to_string()
        }
        Modifier::Ignored => value.to_string(),
    }
}

/// Expand something that could be a variable name or a format string.
fn expand_var_or_format(target: &str, app: &AppState, win_idx: usize) -> String {
    if target.contains("#{") {
        expand_format_for_window(target, app, win_idx)
    } else {
        // If it looks like a plain number or is empty, return as literal
        if target.is_empty() || target.parse::<f64>().is_ok() {
            return target.to_string();
        }
        // Use the inner form so a REAL variable that is merely empty is
        // distinguishable from a name that is not a variable at all. Testing
        // `val.is_empty()` conflated them, so `#{b:pane_path}` rendered the
        // literal text "pane_path" whenever no OSC 7 had arrived yet — every
        // modifier over an optional variable echoed its own name instead of
        // rendering nothing.
        let val = expand_var_inner(target, app, win_idx);
        if val == UNKNOWN_VAR {
            // Try as option
            if let Some(opt_val) = lookup_option(target, app) {
                return opt_val;
            }
            // Not a known variable — return as literal
            return target.to_string();
        }
        val
    }
}

/// Look up a tmux option by name.
/// Public wrapper for lookup_option so config.rs can use it for -o flag check.
pub fn lookup_option_pub(name: &str, app: &AppState) -> Option<String> {
    lookup_option(name, app)
}

fn lookup_option(name: &str, app: &AppState) -> Option<String> {
    if name.starts_with('@') {
        return app.user_options.get(name).cloned();
    }
    match name {
        "status-left" => Some(app.status_left.clone()),
        "status-right" => Some(app.status_right.clone()),
        "status" => Some(if app.status_visible { "on".into() } else { "off".into() }),
        "status-position" => Some(app.status_position.clone()),
        "status-style" => Some(app.status_style.clone()),
        "prefix" => Some(format_key_binding(&app.prefix_key)),
        "prefix2" => Some(app.prefix2_key.as_ref().map(|k| format_key_binding(k)).unwrap_or_else(|| "none".to_string())),
        "base-index" => Some(app.window_base_index.to_string()),
        "pane-base-index" => Some(app.pane_base_index.to_string()),
        "escape-time" => Some(app.escape_time_ms.to_string()),
        "history-limit" => Some(app.history_limit.to_string()),
        "mouse" => Some(if app.mouse_enabled { "on".into() } else { "off".into() }),
        "bold-is-bright" => Some(if app.bold_is_bright { "on".into() } else { "off".into() }),
        "scroll-enter-copy-mode" => Some(if app.scroll_enter_copy_mode { "on".into() } else { "off".into() }),
        "mouse-drag-enter-copy-mode" => Some(if app.mouse_drag_enter_copy_mode { "on".into() } else { "off".into() }),
        "choose-tree-preview" => Some(if app.choose_tree_preview { "on".into() } else { "off".into() }),
        "mode-keys" => Some(app.mode_keys.clone()),
        "default-command" | "default-shell" => Some(if app.default_shell.is_empty() {
            crate::pane::cached_shell().unwrap_or("pwsh.exe").to_string()
        } else {
            app.default_shell.clone()
        }),
        "word-separators" => Some(app.word_separators.clone()),
        "renumber-windows" => Some(if app.renumber_windows { "on".into() } else { "off".into() }),
        "automatic-rename" => Some(if app.automatic_rename { "on".into() } else { "off".into() }),
        "monitor-activity" => Some(if app.monitor_activity { "on".into() } else { "off".into() }),
        "remain-on-exit" => Some(if app.remain_on_exit { "on".into() } else { "off".into() }),
        "destroy-unattached" => Some(if app.destroy_unattached { "on".into() } else { "off".into() }),
        "exit-empty" => Some(if app.exit_empty { "on".into() } else { "off".into() }),
        "set-titles" => Some(if app.set_titles { "on".into() } else { "off".into() }),
        "set-titles-string" => Some(app.set_titles_string.clone()),
        "pane-border-style" => Some(app.pane_border_style.clone()),
        "pane-active-border-style" => Some(app.pane_active_border_style.clone()),
        "pane-border-hover-style" => Some(app.pane_border_hover_style.clone()),
        "window-status-format" => Some(app.window_status_format.clone()),
        "window-status-current-format" => Some(app.window_status_current_format.clone()),
        "window-status-separator" => Some(app.window_status_separator.clone()),
        "window-status-style" => Some(app.window_status_style.clone()),
        "window-status-current-style" => Some(app.window_status_current_style.clone()),
        "window-status-activity-style" => Some(app.window_status_activity_style.clone()),
        "window-status-bell-style" => Some(app.window_status_bell_style.clone()),
        "window-status-last-style" => Some(app.window_status_last_style.clone()),
        "message-style" => Some(app.message_style.clone()),
        "message-command-style" => Some(app.message_command_style.clone()),
        "mode-style" => Some(app.mode_style.clone()),
        "status-left-style" => Some(app.status_left_style.clone()),
        "status-right-style" => Some(app.status_right_style.clone()),
        "status-interval" => Some(app.status_interval.to_string()),
        "status-justify" => Some(app.status_justify.clone()),
        "display-time" => Some(app.display_time_ms.to_string()),
        "display-panes-time" => Some(app.display_panes_time_ms.to_string()),
        "focus-events" => Some(if app.focus_events { "on".into() } else { "off".into() }),
        "aggressive-resize" => Some(if app.aggressive_resize { "on".into() } else { "off".into() }),
        "synchronize-panes" => Some(if app.sync_input { "on".into() } else { "off".into() }),
        "monitor-silence" => Some(app.monitor_silence.to_string()),
        "bell-action" => Some(app.bell_action.clone()),
        "visual-bell" => Some(if app.visual_bell { "on".into() } else { "off".into() }),
        "terminal-overrides" => Some(app.terminal_overrides.join(",")),
        "claude-code-fix-tty" => Some(if app.claude_code_fix_tty { "on".into() } else { "off".into() }),
        "claude-code-force-interactive" => Some(if app.claude_code_force_interactive { "on".into() } else { "off".into() }),
        _ => {
            // Try user_options first (plugins store @cpu_percentage etc.),
            // then environment, then @name fallback for plugin compat
            // (format strings use #{cpu_percentage} without the @ prefix).
            app.user_options.get(name).cloned()
                .or_else(|| app.environment.get(name).cloned())
                .or_else(|| {
                    if !name.starts_with('@') {
                        app.user_options.get(&format!("@{}", name)).cloned()
                    } else {
                        None
                    }
                })
        }
    }
}

// ─────────────────── comparison operators ─────────────────────────

/// Try to match a comparison operator at the start of expr.
fn try_comparison_op(expr: &str, app: &AppState, win_idx: usize) -> Option<String> {
    let ops: &[(&str, fn(&str, &str) -> bool)] = &[
        ("<=:", |a, b| a <= b),
        (">=:", |a, b| a >= b),
        ("==:", |a, b| a == b),
        ("!=:", |a, b| a != b),
        ("<:", |a, b| a < b),
        (">:", |a, b| a > b),
    ];

    for &(prefix, cmp_fn) in ops {
        if let Some(rest) = expr.strip_prefix(prefix) {
            let parts = split_at_depth0(rest, b',');
            if parts.len() < 2 { return Some("0".into()); }
            let lhs = expand_var_or_format(&parts[0], app, win_idx);
            let rhs = expand_var_or_format(&parts[1], app, win_idx);
            return Some(if cmp_fn(&lhs, &rhs) { "1".into() } else { "0".into() });
        }
    }
    None
}

fn expand_boolean_or(body: &str, app: &AppState, win_idx: usize) -> String {
    let parts = split_at_depth0(body, b',');
    for part in &parts {
        let val = expand_var_or_format(part, app, win_idx);
        if is_truthy(&val) { return "1".into(); }
    }
    "0".into()
}

fn expand_boolean_and(body: &str, app: &AppState, win_idx: usize) -> String {
    let parts = split_at_depth0(body, b',');
    for part in &parts {
        let val = expand_var_or_format(part, app, win_idx);
        if !is_truthy(&val) { return "0".into(); }
    }
    "1".into()
}

#[inline]
fn is_truthy(s: &str) -> bool {
    !s.is_empty() && s != "0" && s != "off" && s != "no"
}

// ─────────────────── conditional ─────────────────────────────────

fn expand_conditional(body: &str, app: &AppState, win_idx: usize) -> String {
    let (cond, true_branch, false_branch) = split_conditional(body);

    let is_true = if let Some((lhs_str, op, rhs_str)) = find_comparison_in_cond(&cond) {
        // Expand sides as format strings (plain text passes through, #{var} expands)
        let lhs = expand_format_for_window(lhs_str, app, win_idx);
        let rhs = expand_format_for_window(rhs_str, app, win_idx);
        match op {
            "==" => lhs == rhs,
            "!=" => lhs != rhs,
            "<" => lhs < rhs,
            ">" => lhs > rhs,
            "<=" => lhs <= rhs,
            ">=" => lhs >= rhs,
            _ => false,
        }
    } else {
        // If cond already contains format markers (#), expand it directly.
        // Otherwise wrap as #{variable_name} to resolve the variable.
        let cond_val = if cond.contains('#') {
            expand_format_for_window(&cond, app, win_idx)
        } else {
            expand_format_for_window(&format!("#{{{}}}", cond), app, win_idx)
        };
        is_truthy(&cond_val)
    };

    if is_true {
        expand_format_for_window(&true_branch, app, win_idx)
    } else {
        expand_format_for_window(&false_branch, app, win_idx)
    }
}

fn find_comparison_in_cond(cond: &str) -> Option<(&str, &str, &str)> {
    let ops = ["<=", ">=", "==", "!=", "<", ">"];
    for op in ops {
        // Scan for op outside of nested #{...} blocks
        let bytes = cond.as_bytes();
        let op_bytes = op.as_bytes();
        let mut i = 0;
        let mut depth = 0usize;
        while i + op_bytes.len() <= bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == b'#' && bytes[i + 1] == b'{' {
                depth += 1;
                i += 2;
                continue;
            }
            if bytes[i] == b'}' && depth > 0 {
                depth -= 1;
                i += 1;
                continue;
            }
            if depth == 0 && &bytes[i..i + op_bytes.len()] == op_bytes {
                let lhs = &cond[..i];
                let rhs = &cond[i + op.len()..];
                if !lhs.is_empty() || !rhs.is_empty() {
                    return Some((lhs, op, rhs));
                }
            }
            i += 1;
        }
    }
    None
}

// ─────────────────── variable expansion ──────────────────────────

/// Expand a named variable.
/// Sentinel returned by [`expand_var_inner`] for a name that is not a format
/// variable at all, as distinct from a real variable whose value happens to be
/// empty.
///
/// Those two cases used to be indistinguishable — both came back as `""` — and
/// [`expand_var_or_format`] treated any empty result as "unknown name" and fell
/// back to echoing the name as a literal. The visible consequence was that a
/// modifier over an optional variable printed the variable's own name:
/// `#{b:pane_path}` rendered the text `pane_path` whenever the shell had not
/// yet sent an OSC 7. A bare `#{pane_path}` rendered correctly (empty), so the
/// bug only appeared once a modifier was involved.
///
/// Contains a NUL so it can never collide with a real expansion.
const UNKNOWN_VAR: &str = "\u{0}psmux:unknown-var";

/// Expand a plain variable name. Unknown names expand to the empty string,
/// matching tmux.
/// Every `client_*` variable, answered for ONE client: the row of a
/// list-clients, the `-c` of a display-message, else the best client (issue
/// #724). Resolved before any window lookup, because a client exists whether
/// or not the session has a window to describe.
fn expand_client_var(var: &str, app: &AppState) -> Option<String> {
    Some(match var {
        // The width and height fall back to the session's own geometry when no
        // client is attached, as they did before.
        "client_width" => match format_client(app) {
            Some(ci) => ci.width.to_string(),
            None => app.client_area.width.to_string(),
        },
        "client_height" => {
            let h = format_client(app).map(|ci| ci.height).unwrap_or(app.client_area.height);
            (h + if app.status_visible { 1 } else { 0 }).to_string()
        }
        "client_session" => if sessionless() { String::new() } else { app.session_name.clone() },
        // The session this client came from, empty when it has not switched.
        // This was aliased to client_session, so it echoed the CURRENT session
        // and could neither predict what `-l` would do nor verify what it did
        // (issue #566). tmux reports empty for a client with no last session,
        // which is what an unset value gives here.
        "client_last_session" => format_client(app)
            .and_then(|info| info.last_session.clone())
            .unwrap_or_default(),
        // tmux leaves these empty when there is no client (ft->c == NULL).
        "client_name" | "client_tty" => format_client(app).map(|ci| ci.tty_name.clone()).unwrap_or_default(),
        "client_pid" => format_client(app)
            .filter(|ci| ci.pid != 0)
            .map(|ci| ci.pid.to_string())
            .unwrap_or_default(),
        "client_readonly" => format_client(app).map(|ci| if ci.readonly { "1".into() } else { "0".into() }).unwrap_or_default(),
        "client_control_mode" => format_client(app).map(|ci| if ci.is_control { "1".into() } else { "0".into() }).unwrap_or_default(),
        "client_flags" => format_client(app).map(client_flags_string).unwrap_or_default(),
        "client_prefix" => if app.client_prefix_active || matches!(app.mode, Mode::Prefix { .. }) { "1".into() } else { "0".into() },
        "client_activity" => match format_client(app) {
            Some(ci) => epoch_secs_ago(ci.last_activity.elapsed()).to_string(),
            None => app.created_at.timestamp().to_string(),
        },
        "client_created" => match format_client(app) {
            Some(ci) => epoch_secs_ago(ci.connected_at.elapsed()).to_string(),
            None => app.created_at.timestamp().to_string(),
        },
        "client_activity_string" | "client_created_string" => {
            let at = format_client(app).map(|ci| {
                if var == "client_activity_string" { ci.last_activity.elapsed() } else { ci.connected_at.elapsed() }
            });
            match at {
                Some(ago) => (chrono::Local::now() - chrono::Duration::from_std(ago).unwrap_or_default())
                    .format("%a %b %e %H:%M:%S %Y").to_string(),
                None => app.created_at.format("%a %b %e %H:%M:%S %Y").to_string(),
            }
        }
        "client_key_table" => if app.client_prefix_active || matches!(app.mode, Mode::Prefix { .. }) {
            "prefix".into()
        } else if let Some(t) = app.current_key_table.as_ref() {
            // `switch-client -T <table>` latched a custom table (issue #640);
            // tmux reports `c->keytable->name` here.
            t.clone()
        } else {
            match app.mode {
                Mode::CopyMode => "copy-mode-vi".into(),
                // The session's `key-table` (tmux reports c->keytable, which
                // server_client_set_key_table(c, NULL) points at it).
                _ => crate::server::helpers::default_key_table(app).to_string(),
            }
        },
        "client_termname" | "client_termtype" => env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
        "client_termfeatures" => "256,RGB,hyperlinks,title".into(),
        "client_utf8" => "1".into(),
        "client_cell_width" => "8".into(),
        "client_cell_height" => "16".into(),
        "client_written" | "client_discarded" => "0".into(),
        _ => return None,
    })
}

pub fn expand_var(var: &str, app: &AppState, win_idx: usize) -> String {
    if var.starts_with("client_") {
        if let Some(v) = expand_client_var(var, app) { return v; }
    }
    let v = expand_var_inner(var, app, win_idx);
    if v == UNKNOWN_VAR { String::new() } else { v }
}

/// Whether `var` describes a session, window or pane, so that a server with
/// no session (a held standby, see `with_sessionless`) leaves it empty, the
/// way tmux's format_defaults adds no such keys without a session. Server
/// scope variables (`pid`, `host`, `socket_path`, `start_time`, `version`,
/// buffers, options) are not, and keep resolving.
fn no_session_var(var: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "session_", "window_", "pane_", "alternate_", "copy_cursor_", "cursor_",
        "keypad_", "scroll_", "search_", "selection_",
    ];
    const NAMES: &[&str] = &[
        "history_bytes", "history_size", "insert_flag", "line", "origin_flag", "wrap_flag",
        "mouse_all_flag", "mouse_any_flag", "mouse_button_flag", "mouse_standard_flag",
        "mouse_utf8_flag", "mouse_line", "mouse_word", "mouse_x", "mouse_y",
    ];
    PREFIXES.iter().any(|p| var.starts_with(p)) || NAMES.contains(&var)
}

fn expand_var_inner(var: &str, app: &AppState, win_idx: usize) -> String {
    // With no session at all nothing session scoped resolves, and tmux's
    // server_sessions counts 0.
    if sessionless() {
        if var == "server_sessions" {
            return "0".to_string();
        }
        if no_session_var(var) {
            return String::new();
        }
    }
    let win = match app.windows.get(win_idx) {
        Some(w) => w,
        None => {
            // Even without a window, some variables still resolve
            return match var {
                "session_name" => app.session_name.clone(),
                "session_windows" => app.windows.len().to_string(),
                "session_id" => format!("${}", app.session_id),
                "session_path" => std::env::current_dir()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "pid" | "server_pid" => std::process::id().to_string(),
                "server_instance" => {
                    crate::session::read_namespace_instance(app.socket_name.as_deref())
                        .unwrap_or_default()
                }
                "version" => VERSION.to_string(),
                "host" | "hostname" => hostname_cached(),
                "host_short" => { let h = hostname_cached(); h.split('.').next().unwrap_or(&h).to_string() }
                _ => {
                    // Not "unknown": with no window we simply cannot resolve a
                    // window/pane variable. Reporting UNKNOWN_VAR here would
                    // make a modifier chain echo the variable's name, so treat
                    // an unresolvable-but-real variable as empty.
                    if let Some(v) = lookup_option(var, app) { v } else { String::new() }
                }
            };
        }
    };
    // Resolve the target pane for format expansion. When PANE_POS_OVERRIDE is set
    // (during list-panes iteration), use that positional pane instead of the active pane.
    //
    // Floating panes (tmux new-pane) come AFTER the tiled panes in position
    // order, as in tmux, where window_add_pane puts a SPAWN_FLOATING pane at
    // the tail of w->panes, so list-panes and #{pane_index} count them (#767).
    // A focused float is the window's active pane (tmux makes it w->active),
    // so the tiled pane behind it is not active while the float has focus.
    let tiled_count = count_panes(&win.root);
    let focused_float = win.floating_focus.filter(|fi| *fi < win.floating.len());
    let (fmt_pane_pos, fmt_pane_is_active) = {
        let override_pos = PANE_POS_OVERRIDE.get();
        if let Some(pos) = override_pos {
            let is_active = if pos >= tiled_count {
                focused_float == Some(pos - tiled_count)
            } else {
                let active_id = get_active_pane_id(&win.root, &win.active_path);
                focused_float.is_none()
                    && crate::tree::get_nth_pane(&win.root, pos)
                        .map(|p| Some(p.id) == active_id).unwrap_or(false)
            };
            (pos, is_active)
        } else if let Some(fi) = focused_float {
            (tiled_count + fi, true)
        } else {
            let active_id = get_active_pane_id(&win.root, &win.active_path).unwrap_or(0);
            let pos = crate::tree::get_pane_position_in_window(&win.root, active_id).unwrap_or(0);
            (pos, true)
        }
    };
    // The floating pane the position names, if it names one.
    let target_float = fmt_pane_pos
        .checked_sub(tiled_count)
        .and_then(|fi| win.floating.get(fi));
    // Helper closure to get the target pane reference
    let target_pane = || -> Option<&Pane> {
        match target_float {
            Some(fp) => Some(&fp.pane),
            None => crate::tree::get_nth_pane(&win.root, fmt_pane_pos),
        }
    };
    // A floating pane's content rect (x, y, w, h) inside the window. The PTY
    // is the float's outer size minus its border, and the border is drawn
    // around it, so the content starts one cell in. `border none` is drawn
    // with no inset by the client, so it starts at the float's corner.
    let float_rect = |fp: &crate::types::FloatingPane| -> (u16, u16, u16, u16) {
        let (ox, oy) = if fp.border == "none" { (fp.x, fp.y) } else { (fp.x + 1, fp.y + 1) };
        (win.area.x + ox, win.area.y + oy, fp.pane.last_cols, fp.pane.last_rows)
    };
    // A float is not in the tiled layout, so its geometry comes from its own
    // rect rather than from compute_rects (which would never find it).
    if let Some(fp) = target_float {
        let (x, y, w, h) = float_rect(fp);
        let flag = |b: bool| -> String { if b { "1".into() } else { "0".into() } };
        let geo = match var {
            "pane_left" => Some(x.to_string()),
            "pane_top" => Some(y.to_string()),
            "pane_right" => Some((x + w).saturating_sub(1).to_string()),
            "pane_bottom" => Some((y + h).saturating_sub(1).to_string()),
            "pane_at_left" => Some(flag(x == win.area.x)),
            "pane_at_top" => Some(flag(y == win.area.y)),
            "pane_at_right" => Some(flag(x + w >= win.area.x + win.area.width)),
            "pane_at_bottom" => Some(flag(y + h >= win.area.y + win.area.height)),
            _ => None,
        };
        if let Some(v) = geo { return v; }
    }
    match var {
        // ── Session ──
        "session_name" => app.session_name.clone(),
        "session_attached" => if app.attached_clients > 0 { "1".into() } else { "0".into() },
        "session_windows" => app.windows.len().to_string(),
        "session_id" => format!("${}", app.session_id),
        "session_created" => app.created_at.timestamp().to_string(),
        "session_created_string" => app.created_at.format("%a %b %e %H:%M:%S %Y").to_string(),
        "session_activity" | "session_last_attached" => app.created_at.timestamp().to_string(),
        "session_activity_string" => app.created_at.format("%a %b %e %H:%M:%S %Y").to_string(),
        "session_group" | "session_group_list" => app.session_group.clone().unwrap_or_default(),
        "session_alerts" | "session_stack" => String::new(),
        "session_group_attached" => {
            if app.session_group.is_some() && app.attached_clients > 0 { "1".into() } else { "0".into() }
        }
        "session_group_size" => {
            if app.session_group.is_some() { "1".into() } else { "0".into() }
        }
        "session_grouped" => if app.session_group.is_some() { "1".into() } else { "0".into() },
        "session_format" | "session_many_attached" => if app.attached_clients > 1 { "1".into() } else { "0".into() },
        "session_path" => std::env::current_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default(),

        // ── Window ──
        "window_index" => app.win_display_index(win_idx).to_string(),
        "window_name" => win.name.clone(),
        "window_active" => if win_idx == real_active_idx(app) { "1".into() } else { "0".into() },
        // tmux counts floating panes too: format_cb_window_panes is
        // window_count_panes(w, 1), with_floating set (#767).
        "window_panes" => (tiled_count + win.floating.len()).to_string(),
        "window_flags" | "window_raw_flags" => {
            let mut f = String::new();
            if win_idx == real_active_idx(app) { f.push('*'); }
            else if win_idx == app.last_window_idx { f.push('-'); }
            if win.zoom_saved.is_some() { f.push('Z'); }
            if win.activity_flag { f.push('#'); }
            if win.bell_flag { f.push('!'); }
            if win.silence_flag { f.push('~'); }
            f
        }
        "window_id" => format!("@{}", win.id),
        "window_activity_flag" => if win.activity_flag { "1".into() } else { "0".into() },
        "window_zoomed_flag" => if win.zoom_saved.is_some() { "1".into() } else { "0".into() },
        "window_layout" | "window_visible_layout" => generate_window_layout(&win.root, win.area),
        "window_width" => win.area.width.to_string(),
        "window_height" => win.area.height.to_string(),
        "window_format" => "1".into(),
        "window_activity" => app.created_at.timestamp().to_string(),
        "window_silence_flag" => if win.silence_flag { "1".into() } else { "0".into() },
        "window_bell_flag" => if win.bell_flag { "1".into() } else { "0".into() },
        "window_linked" => if win.linked_from.is_some() { "1".into() } else { "0".into() },
        "window_linked_sessions" => if win.linked_from.is_some() { "1".into() } else { "0".into() },
        "window_linked_sessions_list" => String::new(),
        "window_last_flag" => if win_idx == app.last_window_idx { "1".into() } else { "0".into() },
        "window_start_flag" => if win_idx == 0 { "1".into() } else { "0".into() },
        "window_end_flag" => if win_idx == app.windows.len().saturating_sub(1) { "1".into() } else { "0".into() },
        "window_bigger" => {
            if win.area.width > app.client_area.width || win.area.height > app.client_area.height {
                "1".into()
            } else {
                "0".into()
            }
        }
        "window_cell_width" => "8".into(),
        "window_cell_height" => "16".into(),
        "window_offset_x" | "window_offset_y" | "window_stack_index" => "0".into(),

        // ── Pane ──
        "pane_index" => {
            (fmt_pane_pos + app.pane_base_index).to_string()
        }
        "pane_id" => {
            if let Some(p) = target_pane() { format!("%{}", p.id) } else { "%0".into() }
        }
        "pane_title" => {
            if let Some(p) = target_pane() {
                if !p.title.is_empty() { p.title.clone() } else { hostname_cached() }
            } else { hostname_cached() }
        }
        "pane_width" => {
            if let Some(p) = target_pane() { p.last_cols.to_string() } else { "80".into() }
        }
        "pane_height" => {
            if let Some(p) = target_pane() { p.last_rows.to_string() } else { "24".into() }
        }
        // Milliseconds since this pane last received printable text on the
        // INTERACTIVE input route (handle_key); empty if none yet. The injected
        // route (send-keys / send-paste / send-text) does NOT update it. A
        // read-only route signal — consumers own any policy on top.
        "pane_last_text_input" => {
            match target_pane().and_then(|p| p.last_text_input) {
                Some(t) => t.elapsed().as_millis().to_string(),
                None => String::new(),
            }
        }
        // The last NON-text key received on the INTERACTIVE input route
        // (handle_key), by canonical bind-key name (Escape, Enter, Up, F9,
        // C-c, M-a, ...); companion _ms gives its age in ms. Empty if none yet.
        // Like pane_last_text_input, the injected route (send-keys /
        // send-paste / send-text) does NOT update it. A read-only route signal
        // -- consumers own any policy on top.
        "pane_last_special_key" => {
            match target_pane().and_then(|p| p.last_special_key.as_ref()) {
                Some((_, name)) => name.clone(),
                None => String::new(),
            }
        }
        "pane_last_special_key_ms" => {
            match target_pane().and_then(|p| p.last_special_key.as_ref()) {
                Some((t, _)) => t.elapsed().as_millis().to_string(),
                None => String::new(),
            }
        }
        "pane_active" => if fmt_pane_is_active { "1".into() } else { "0".into() },
        // tmux format.c format_cb_pane_floating_flag: 1 for a floating pane,
        // 0 for a tiled one (#767).
        "pane_floating_flag" => if target_float.is_some() { "1".into() } else { "0".into() },
        // tmux format_cb_pane_x / _y: the pane's content offset in the window.
        "pane_x" | "pane_y" => {
            let want_x = var == "pane_x";
            if let Some(fp) = target_float {
                let (x, y, _, _) = float_rect(fp);
                return if want_x { x.to_string() } else { y.to_string() };
            }
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) {
                    return if want_x { rect.x.to_string() } else { rect.y.to_string() };
                }
            }
            "0".into()
        }
        // tmux format_cb_pane_z (window_pane_zindex): floats are numbered
        // from the topmost (0) down; a tiled pane sits under every float,
        // at the float count plus one. psmux draws `floating` in order, so
        // the LAST entry is on top.
        "pane_z" => match fmt_pane_pos.checked_sub(tiled_count) {
            Some(fi) if fi < win.floating.len() => (win.floating.len() - 1 - fi).to_string(),
            _ => (win.floating.len() + 1).to_string(),
        },
        "pane_current_command" => {
            if let Some(p) = target_pane() {
                // Shell-integration OSC is authoritative (issue #299):
                // OSC 133;C;cmdline_url=, OSC 1337;SetUserVar=WEZTERM_PROG, or
                // OSC 633;E is the definitive signal for what's running.
                if let Ok(parser) = p.term.lock() {
                    if let Some(cmd) = parser.screen().shell_command() {
                        return cmd.to_string();
                    }
                }
                if let Some(pid) = p.child_pid {
                    // Fallback: the deepest foreground descendant (what tmux
                    // reports — `cat` running under a shell, not the shell),
                    // then the pane's own process, then a generic label.
                    //
                    // The process-table snapshot behind that walk has two
                    // freshness policies, and FORMAT_ASYNC already says which
                    // one this expansion wants: it is set exactly on the
                    // per-frame render path (status bar, window tabs, window
                    // title) and clear for every one-shot expansion that
                    // becomes a command reply. A frame may be one refresh
                    // behind — the next frame corrects it, and keeping the
                    // 9-11ms system walk off the server event loop is what the
                    // keystroke-latency fix bought. A command reply may NOT:
                    // `display-message -p '#{pane_current_command}'` is asked
                    // once, and a stale table is the whole answer, which is how
                    // this format came to report `pwsh` two seconds after a
                    // command started and the command two seconds after it
                    // exited. tmux reads the tty's foreground process group at
                    // query time and is never stale, so the query route walks
                    // inline when the snapshot has expired.
                    let deepest = if FORMAT_ASYNC.with(|c| c.get()) {
                        crate::platform::process_info::get_deepest_foreground_process_name(pid)
                    } else {
                        crate::platform::process_info::get_deepest_foreground_process_name_fresh(pid)
                    };
                    deepest
                        .or_else(|| crate::platform::process_info::get_process_name(pid))
                        .unwrap_or_else(|| "shell".into())
                } else if !p.title.is_empty() {
                    p.title.clone()
                } else {
                    "shell".into()
                }
            } else { String::new() }
        }
        "pane_current_path" => {
            if let Some(p) = target_pane() {
                // What the shell last said about itself over OSC 7 / OSC 9;9,
                // if anything.  Read once: the PEB walk below must not hold the
                // parser lock.
                let announced = p
                    .term
                    .lock()
                    .ok()
                    .and_then(|t| t.screen().path().map(str::to_owned));

                // Layer 0 (issue #615): a pane running `wsl` or `ssh` has no
                // Win32 process that knows where the shell is.  wsl.exe keeps
                // the working directory it was created with forever, so the PEB
                // walk below succeeds and confidently returns the directory the
                // user was in BEFORE typing `wsl` -- which is exactly what made
                // `split-window -c "#{pane_current_path}"` open the wrong
                // folder.  When a bridge is in the tree, an announcement from
                // the shell is the only real information available, so it wins.
                //
                // The bridge walk is deliberately behind `announced`: a pane
                // with no shell integration never pays for it, and its
                // behaviour is bit-for-bit what it was before.
                if let Some(raw) = announced.as_deref() {
                    if let Some(pid) = p.child_pid {
                        if crate::platform::process_info::tree_has_vt_bridge_cached(pid) {
                            if let Some(win) = crate::wsl_path::osc_cwd_to_windows(
                                raw,
                                crate::wsl_path::default_distro(),
                            ) {
                                return win;
                            }
                        }
                    }
                }

                // Layer 1: PEB walk (authoritative for local processes -- pwsh,
                // cmd, cygwin bash and git bash all move it on `cd`).
                //
                // A pane claimed from the warm pool is the one case where that
                // reading is known to be a lie for a while: the spare is
                // already running in the directory the pool spawned it in (the
                // server's own cwd), and `-c <dir>` is honoured by typing a
                // `cd` into it, which only takes effect once the shell reaches
                // a prompt.  `cwd_hint` carries both the directory that was
                // asked for and the reading taken just before that `cd` was
                // written, so the request is reported for exactly as long as
                // the process has not moved, and `split-window -c <dir>` never
                // answers with the server's working directory (#615 follow
                // up).  Everything else keeps trusting the reading.
                {
                    let live = p
                        .child_pid
                        .and_then(crate::platform::process_info::get_foreground_cwd);
                    if let Some(hint) = p.cwd_hint.as_ref() {
                        if hint.pid == p.child_pid
                            && !hint.settled.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            match live.as_deref() {
                                // Still sitting where the pool left it: the
                                // injected `cd` has not run yet.
                                Some(cwd)
                                    if hint
                                        .stale
                                        .as_deref()
                                        .is_some_and(|s| crate::util::same_dir(s, cwd)) =>
                                {
                                    return hint.requested.clone();
                                }
                                // Nothing readable at all: the request is the
                                // only thing known about this pane.
                                None => return hint.requested.clone(),
                                // The process moved, either into the requested
                                // directory or somewhere the user went first.
                                // Either way the reading is live from now on,
                                // latched so a later `cd` back into the pool's
                                // directory cannot revive the hint.
                                Some(_) => {
                                    hint.settled
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                        }
                    }
                    if let Some(cwd) = live {
                        return cwd;
                    }
                }
                // Layer 2: the announced path, translated to a native Windows
                // path when it is a POSIX one (works where the PEB fails).
                if let Some(raw) = announced.as_deref() {
                    if let Some(win) = crate::wsl_path::osc_cwd_to_windows(
                        raw,
                        crate::wsl_path::default_distro(),
                    ) {
                        return win;
                    }
                    return raw.to_string();
                }
                // Layer 3: fallback to server CWD
                std::env::current_dir()
                    .map(|d| d.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else { String::new() }
        }
        "pane_path" => {
            // Pure OSC 7 value (tmux-compatible: only what the shell announced)
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    parser.screen().path().unwrap_or_default().to_string()
                } else { String::new() }
            } else { String::new() }
        }
        "pane_pid" => {
            if let Some(p) = target_pane() {
                p.child_pid.map(|pid| pid.to_string()).unwrap_or_default()
            } else { String::new() }
        }
        "pane_tty" => {
            if let Some(p) = target_pane() { format!("/dev/pty{}", p.id) }
            else { String::new() }
        }
        // A mode belongs to one pane, as it does in tmux (`window.c`
        // `window_pane_set_mode` pushes it onto that pane's own mode stack),
        // so both of these must answer for the TARGET pane rather than for
        // whichever pane happens to hold focus (#607).  The focused pane's
        // live mode is `AppState::mode`; every other pane's is parked in its
        // own `copy_state`.
        "pane_in_mode" => {
            let target_id = target_pane().map(|p| p.id);
            if target_id.is_some() && target_id == crate::copy_mode::active_pane_id(app) {
                if app.mode.in_copy() || matches!(app.mode, Mode::ClockMode) {
                    "1".into()
                } else {
                    "0".into()
                }
            } else if target_pane().map_or(false, |p| p.copy_state.is_some()) {
                "1".into()
            } else {
                "0".into()
            }
        }
        "pane_mode" => {
            let target_id = target_pane().map(|p| p.id);
            if target_id.is_some() && target_id == crate::copy_mode::active_pane_id(app) {
                if app.mode.in_copy() {
                    "copy-mode".into()
                } else if matches!(app.mode, Mode::ClockMode) {
                    "clock-mode".into()
                } else {
                    String::new()
                }
            } else if target_pane().map_or(false, |p| p.copy_state.is_some()) {
                "copy-mode".into()
            } else {
                String::new()
            }
        }
        "pane_synchronized" => if app.sync_input { "1".into() } else { "0".into() },
        "pane_dead" => {
            if let Some(p) = target_pane() {
                if p.dead { "1".into() } else { "0".into() }
            } else { "0".into() }
        }
        "pane_dead_signal" | "pane_dead_status" | "pane_dead_time" => "0".into(),
        "pane_format" => "1".into(),
        // select-pane -d / -e (tmux format.c format_cb_pane_input_off:
        // `wp->flags & PANE_INPUTOFF`).
        "pane_input_off" => {
            if target_pane().is_some_and(|p| p.input_off) { "1".into() } else { "0".into() }
        }
        "pane_pipe" | "pane_unseen_changes" => "0".into(),
        "pane_last" => {
            if let Some(p) = target_pane() {
                if !app.last_pane_path.is_empty() {
                    if let Some(last_p) = active_pane(&win.root, &app.last_pane_path) {
                        if last_p.id == p.id { return "1".into(); }
                    }
                }
            }
            "0".into()
        }
        "pane_marked" => {
            if let Some(p) = target_pane() {
                if let Some((mw, mp)) = app.marked_pane {
                    if mw == win_idx && mp == p.id { "1".into() } else { "0".into() }
                } else { "0".into() }
            } else { "0".into() }
        }
        "pane_marked_set" => {
            if app.marked_pane.is_some() { "1".into() } else { "0".into() }
        }
        "pane_left" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) { rect.x.to_string() } else { "0".into() }
            } else { "0".into() }
        }
        "pane_top" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) { rect.y.to_string() } else { "0".into() }
            } else { "0".into() }
        }
        "pane_right" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) { (rect.x + rect.width).saturating_sub(1).to_string() } else { "79".into() }
            } else { "79".into() }
        }
        "pane_bottom" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) { (rect.y + rect.height).saturating_sub(1).to_string() } else { "23".into() }
            } else { "23".into() }
        }
        "pane_at_top" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) {
                    if rect.y == win.area.y { "1".into() } else { "0".into() }
                } else { "1".into() }
            } else { "1".into() }
        }
        "pane_at_bottom" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) {
                    let bottom = rect.y + rect.height;
                    let win_bottom = win.area.y + win.area.height;
                    if bottom >= win_bottom { "1".into() } else { "0".into() }
                } else { "1".into() }
            } else { "1".into() }
        }
        "pane_at_left" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) {
                    if rect.x == win.area.x { "1".into() } else { "0".into() }
                } else { "1".into() }
            } else { "1".into() }
        }
        "pane_at_right" => {
            if let Some(p) = target_pane() {
                let mut rects = Vec::new();
                crate::tree::compute_rects(&win.root, win.area, &mut rects);
                if let Some((_, rect)) = rects.iter().find(|(path, _)| {
                    crate::tree::get_active_pane_id_at_path(&win.root, path) == Some(p.id)
                }) {
                    let right = rect.x + rect.width;
                    let win_right = win.area.x + win.area.width;
                    if right >= win_right { "1".into() } else { "0".into() }
                } else { "1".into() }
            } else { "1".into() }
        }
        "pane_search_string" => app.copy_search_query.clone(),
        // The command THIS pane was started with (issue #580). tmux reports
        // the pane's own spawn argv and the empty string for a pane running
        // the plain default shell (format.c format_cb_start_command ->
        // cmd_stringify_argv, which returns "" for argc == 0). Reporting
        // `app.default_shell` here made every pane in the server answer with
        // the same string, so ownership predicates like
        // `#{m:*MARKER*,#{pane_start_command}}` could never match.
        "pane_start_command" => target_pane()
            .map(|p| crate::pane::start_command_display(&p.start_command))
            .unwrap_or_default(),
        "pane_start_path" | "pane_tabs" => String::new(),
        "pane_fg" => {
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    let (r, c) = parser.screen().cursor_position();
                    if let Some(cell) = parser.screen().cell(r, c) {
                        return format_vt100_color(cell.fgcolor());
                    }
                }
            }
            "default".into()
        }
        "pane_bg" => {
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    let (r, c) = parser.screen().cursor_position();
                    if let Some(cell) = parser.screen().cell(r, c) {
                        return format_vt100_color(cell.bgcolor());
                    }
                }
            }
            "default".into()
        }

        // ── Cursor ──
        "cursor_x" => {
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    let (_, c) = parser.screen().cursor_position();
                    return c.to_string();
                }
            }
            "0".into()
        }
        "cursor_y" => {
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    let (r, _) = parser.screen().cursor_position();
                    return r.to_string();
                }
            }
            "0".into()
        }
        "cursor_character" => {
            if let Some(p) = target_pane() {
                if let Ok(parser) = p.term.lock() {
                    let (r, c) = parser.screen().cursor_position();
                    if let Some(cell) = parser.screen().cell(r, c) {
                        return cell.contents().to_string();
                    }
                }
            }
            String::new()
        }
        "cursor_flag" => "0".into(),

        // ── Mouse ──
        "mouse_x" => app.last_mouse_x.to_string(),
        "mouse_y" => app.last_mouse_y.to_string(),
        "mouse_line" => {
            if let Some(w) = app.windows.get(win_idx) {
                if let Some(p) = active_pane(&w.root, &w.active_path) {
                    if let Ok(parser) = p.term.lock() {
                        let screen = parser.screen();
                        let cols = p.last_cols;
                        // Convert screen-absolute mouse_y to pane-relative row
                        let mut rects = Vec::new();
                        crate::tree::compute_rects(&w.root, w.area, &mut rects);
                        let pane_y_offset = rects.iter()
                            .find(|(path, _)| crate::tree::get_active_pane_id_at_path(&w.root, path) == Some(p.id))
                            .map(|(_, rect)| rect.y)
                            .unwrap_or(0);
                        let row = app.last_mouse_y.saturating_sub(pane_y_offset);
                        let mut row_text = String::with_capacity(cols as usize);
                        for col in 0..cols {
                            if let Some(cell) = screen.cell(row, col) {
                                let t = cell.contents();
                                if t.is_empty() { row_text.push(' '); } else { row_text.push_str(t); }
                            } else { row_text.push(' '); }
                        }
                        return row_text.trim_end().to_string();
                    }
                }
            }
            String::new()
        }
        "mouse_word" => {
            if let Some(w) = app.windows.get(win_idx) {
                if let Some(p) = active_pane(&w.root, &w.active_path) {
                    if let Ok(parser) = p.term.lock() {
                        let screen = parser.screen();
                        let cols = p.last_cols;
                        let mut rects = Vec::new();
                        crate::tree::compute_rects(&w.root, w.area, &mut rects);
                        let (pane_x_offset, pane_y_offset) = rects.iter()
                            .find(|(path, _)| crate::tree::get_active_pane_id_at_path(&w.root, path) == Some(p.id))
                            .map(|(_, rect)| (rect.x, rect.y))
                            .unwrap_or((0, 0));
                        let row = app.last_mouse_y.saturating_sub(pane_y_offset);
                        let col = app.last_mouse_x.saturating_sub(pane_x_offset);
                        let mut row_text = String::with_capacity(cols as usize);
                        for c in 0..cols {
                            if let Some(cell) = screen.cell(row, c) {
                                let t = cell.contents();
                                if t.is_empty() { row_text.push(' '); } else { row_text.push_str(t); }
                            } else { row_text.push(' '); }
                        }
                        let chars: Vec<char> = row_text.chars().collect();
                        let ci = col as usize;
                        if ci < chars.len() && !chars[ci].is_whitespace() {
                            let seps = &app.word_separators;
                            let cls = |ch: &char| -> u8 {
                                if ch.is_whitespace() { 0 }
                                else if seps.contains(*ch) { 1 }
                                else { 2 }
                            };
                            let target = cls(&chars[ci]);
                            let mut start = ci;
                            while start > 0 && cls(&chars[start - 1]) == target { start -= 1; }
                            let mut end = ci;
                            while end + 1 < chars.len() && cls(&chars[end + 1]) == target { end += 1; }
                            return chars[start..=end].iter().collect();
                        }
                    }
                }
            }
            String::new()
        }

        // ── Copy mode ──
        "copy_cursor_x" => app.copy_pos.map(|(_, c)| c.to_string()).unwrap_or("0".into()),
        "copy_cursor_y" => app.copy_pos.map(|(r, _)| r.to_string()).unwrap_or("0".into()),
        "copy_cursor_word" => {
            // Return the word under the copy cursor
            if let (Some((r, c)), Some(w)) = (app.copy_pos, app.windows.get(win_idx)) {
                if let Some(p) = active_pane(&w.root, &w.active_path) {
                    if let Ok(parser) = p.term.lock() {
                        let screen = parser.screen();
                        let cols = p.last_cols;
                        let mut row_text = String::with_capacity(cols as usize);
                        for col in 0..cols {
                            if let Some(cell) = screen.cell(r, col) {
                                let t = cell.contents();
                                if t.is_empty() { row_text.push(' '); } else { row_text.push_str(t); }
                            } else { row_text.push(' '); }
                        }
                        let chars: Vec<char> = row_text.chars().collect();
                        let ci = c as usize;
                        if ci < chars.len() && !chars[ci].is_whitespace() {
                            let seps = &app.word_separators;
                            let cls = |ch: &char| -> u8 {
                                if ch.is_whitespace() { 0 }
                                else if seps.contains(*ch) { 1 }
                                else { 2 }
                            };
                            let target = cls(&chars[ci]);
                            let mut start = ci;
                            while start > 0 && cls(&chars[start - 1]) == target { start -= 1; }
                            let mut end = ci;
                            while end + 1 < chars.len() && cls(&chars[end + 1]) == target { end += 1; }
                            return chars[start..=end].iter().collect();
                        }
                    }
                }
            }
            String::new()
        }
        "copy_cursor_line" => {
            // Return the line under the copy cursor
            if let (Some((r, _)), Some(w)) = (app.copy_pos, app.windows.get(win_idx)) {
                if let Some(p) = active_pane(&w.root, &w.active_path) {
                    if let Ok(parser) = p.term.lock() {
                        let screen = parser.screen();
                        let cols = p.last_cols;
                        let mut row_text = String::with_capacity(cols as usize);
                        for col in 0..cols {
                            if let Some(cell) = screen.cell(r, col) {
                                let t = cell.contents();
                                if t.is_empty() { row_text.push(' '); } else { row_text.push_str(t); }
                            } else { row_text.push(' '); }
                        }
                        return row_text.trim_end().to_string();
                    }
                }
            }
            String::new()
        }
        "selection_present" | "selection_active" => if app.copy_anchor.is_some() { "1".into() } else { "0".into() },
        "selection_start_x" => app.copy_anchor.map(|(_, c)| c.to_string()).unwrap_or("0".into()),
        "selection_start_y" => app.copy_anchor.map(|(r, _)| r.to_string()).unwrap_or("0".into()),
        "selection_end_x" => app.copy_pos.map(|(_, c)| c.to_string()).unwrap_or("0".into()),
        "selection_end_y" => app.copy_pos.map(|(r, _)| r.to_string()).unwrap_or("0".into()),
        "search_present" => if !app.copy_search_query.is_empty() { "1".into() } else { "0".into() },
        "search_match" => {
            if !app.copy_search_matches.is_empty() {
                app.copy_search_matches.get(app.copy_search_idx)
                    .map(|_| app.copy_search_query.clone())
                    .unwrap_or_default()
            } else { String::new() }
        }
        "scroll_position" => app.copy_scroll_offset.to_string(),
        "scroll_region_upper" => "0".into(),
        "scroll_region_lower" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                return p.last_rows.saturating_sub(1).to_string();
            }
            "0".into()
        }

        // ── Buffer ──
        "buffer_size" => {
            // Check named buffer override first
            let named = NAMED_BUFFER_OVERRIDE.with(|c| c.borrow().clone());
            if let Some(ref name) = named {
                return app.named_buffers.get(name).map(|b| b.len().to_string()).unwrap_or("0".into());
            }
            let idx = BUFFER_IDX_OVERRIDE.get().unwrap_or(0);
            app.paste_buffers.get(idx).map(|b| b.len().to_string()).unwrap_or("0".into())
        }
        "buffer_sample" => {
            let named = NAMED_BUFFER_OVERRIDE.with(|c| c.borrow().clone());
            if let Some(ref name) = named {
                return app.named_buffers.get(name).map(|b| b.chars().take(50).collect::<String>()).unwrap_or_default();
            }
            let idx = BUFFER_IDX_OVERRIDE.get().unwrap_or(0);
            app.paste_buffers.get(idx).map(|b| b.chars().take(50).collect::<String>()).unwrap_or_default()
        }
        "buffer_name" => {
            let named = NAMED_BUFFER_OVERRIDE.with(|c| c.borrow().clone());
            if let Some(name) = named {
                return name;
            }
            let idx = BUFFER_IDX_OVERRIDE.get().unwrap_or(0);
            if idx < app.paste_buffers.len() { format!("buffer{:04}", idx) } else { String::new() }
        }
        "buffer_created" => app.created_at.timestamp().to_string(),

        // ── Client ──
        // Every client_* variable is answered by expand_client_var (#724).

        // ── Server ──
        "host" | "hostname" => hostname_cached(),
        "host_short" => { let h = hostname_cached(); h.split('.').next().unwrap_or(&h).to_string() }
        "user" | "username" => env::var("USERNAME").or_else(|_| env::var("USER")).unwrap_or_else(|_| "unknown".into()),
        // NOTE: `#{pid}`/`#{server_pid}` are SESSION-scoped on psmux, not
        // namespace-scoped as they are on tmux: psmux runs one server process
        // per session, so this is the pid of whichever session's server answered
        // the request. Use `#{server_instance}` to identify the namespace.
        "pid" | "server_pid" => std::process::id().to_string(),
        // Namespace-scoped identity (issue #509): constant for the life of this
        // `-L` namespace's server set, and different after a genuine restart.
        // Every server in the namespace reads the same token, so the value does
        // not depend on which one answered.
        "server_instance" => {
            crate::session::read_namespace_instance(app.socket_name.as_deref()).unwrap_or_default()
        }
        "version" => VERSION.to_string(),
        "start_time" => app.created_at.timestamp().to_string(),
        "socket_path" => {
            // The server that answered (#730): `<dir>/default`, `<dir>/<-L label>`,
            // or the exact `-S` path its namespace was created for.
            crate::socket_path::server_socket_path(app.socket_name.as_deref())
        }

        // ── Options as format variables ──
        "mouse" => if app.mouse_enabled { "on".into() } else { "off".into() },
        "bold-is-bright" => if app.bold_is_bright { "on".into() } else { "off".into() },
        "scroll-enter-copy-mode" => if app.scroll_enter_copy_mode { "on".into() } else { "off".into() },
        "mouse-drag-enter-copy-mode" => if app.mouse_drag_enter_copy_mode { "on".into() } else { "off".into() },
        "choose-tree-preview" => if app.choose_tree_preview { "on".into() } else { "off".into() },
        "prefix" => format_key_binding(&app.prefix_key),
        "prefix2" => app.prefix2_key.as_ref().map(|k| format_key_binding(k)).unwrap_or_else(|| "none".to_string()),
        "status" => if app.status_visible { "on".into() } else { "off".into() },
        "mode_keys" => app.mode_keys.clone(),
        "history_limit" => app.history_limit.to_string(),
        "alternate_screen" => if app.allow_alternate_screen { "on".into() } else { "off".into() },
        // history_size reports the number of rows currently held in the
        // active pane's scrollback (the *retained* count), not the
        // configured maximum (#271).  Falls back to 0 when no active pane
        // is reachable, matching tmux's behaviour for empty buffers.
        // It reads the LIVE grid even while copy mode shows a snapshot
        // (PR #671), because tmux's format_cb_history_size answers from
        // wp->base.grid->hsize whatever mode the pane is in.
        "history_size" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.live_parser().lock() {
                    return parser.screen().scrollback_filled().to_string();
                }
            }
            "0".into()
        }
        // history_bytes is how much the active pane's scrollback actually
        // occupies.  It used to be hardcoded to 0, which hid exactly the
        // growth issue #641 was about; now that a row is compacted to its used
        // width on the way into history, this number tracks retained text
        // rather than pane width, so it is worth reporting honestly.
        "history_bytes" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.live_parser().lock() {
                    return parser.screen().history_bytes().to_string();
                }
            }
            "0".into()
        }
        "alternate_on" => {
            if let Some(p) = active_pane(&win.root, &win.active_path) {
                if let Ok(parser) = p.term.lock() {
                    if parser.screen().alternate_screen() { return "1".into(); }
                }
            }
            "0".into()
        }
        "alternate_saved_x" | "alternate_saved_y" => "0".into(),

        // ── Mouse tracking flags (#662) ──
        //
        // tmux reads all six straight off the pane's own screen mode
        // (format.c:1940-2026) and its default wheel binding is written in
        // terms of them (key-bindings.c:510):
        //
        //   bind -n WheelUpPane { if -F '#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}' \
        //       { send -M } { copy-mode -e } }
        //
        // psmux tracked the same state for its own wheel gate but never
        // published it, so every one of these rendered empty and that binding
        // line could only ever take its `copy-mode -e` branch.  They are read
        // from the TARGET pane, not the active one, so `-t %N` and
        // `list-panes -F` answer per pane the way tmux does.
        "mouse_any_flag" | "mouse_standard_flag" | "mouse_button_flag"
        | "mouse_all_flag" | "mouse_utf8_flag" | "mouse_sgr_flag" => {
            let on = target_pane().and_then(|p| p.term.lock().ok()).map(|parser| {
                let screen = parser.screen();
                match var {
                    "mouse_any_flag" => screen.mouse_any_flag(),
                    "mouse_standard_flag" => screen.mouse_standard_flag(),
                    "mouse_button_flag" => screen.mouse_button_flag(),
                    "mouse_all_flag" => screen.mouse_all_flag(),
                    "mouse_utf8_flag" => screen.mouse_utf8_flag(),
                    _ => screen.mouse_sgr_flag(),
                }
            });
            if on == Some(true) { "1".into() } else { "0".into() }
        }

        // ── Misc ──
        "origin_flag" | "insert_flag" | "keypad_cursor_flag" | "keypad_flag" => "0".into(),
        "wrap_flag" => "1".into(),
        "line" | "command" | "command_list_name" | "command_list_alias" | "command_list_usage" | "config_files" => String::new(),
        "current_file" => crate::config::current_config_file(),

        // A user option resolves for THIS pane first (#728): tmux's
        // format_find reads `@name` through the pane's options table, whose
        // parent chain is the window, the session and the global tables.
        _ if var.starts_with('@') => {
            crate::server::options::pane_user_option(app, target_pane(), var)
                .unwrap_or_else(|| UNKNOWN_VAR.to_string())
        }
        // Anything else: try as option, then report "not a variable at all".
        _ => {
            if let Some(val) = lookup_option(var, app) { val }
            else { UNKNOWN_VAR.to_string() }
        }
    }
}

// ─────────────────── helper utilities ────────────────────────────

fn format_vt100_color(color: vt100::Color) -> String {
    match color {
        vt100::Color::Default => "default".into(),
        vt100::Color::Idx(i) => match i {
            0 => "black".into(),
            1 => "red".into(),
            2 => "green".into(),
            3 => "yellow".into(),
            4 => "blue".into(),
            5 => "magenta".into(),
            6 => "cyan".into(),
            7 => "white".into(),
            8 => "bright black".into(),
            9 => "bright red".into(),
            10 => "bright green".into(),
            11 => "bright yellow".into(),
            12 => "bright blue".into(),
            13 => "bright magenta".into(),
            14 => "bright cyan".into(),
            15 => "bright white".into(),
            _ => format!("colour{}", i),
        },
        vt100::Color::Rgb(r, g, b) => format!("#{:02x}{:02x}{:02x}", r, g, b),
    }
}

pub(crate) fn hostname_cached() -> String {
    use std::sync::OnceLock;
    static HOSTNAME: OnceLock<String> = OnceLock::new();
    HOSTNAME.get_or_init(|| {
        env::var("COMPUTERNAME")
            .or_else(|_| env::var("HOSTNAME"))
            .unwrap_or_default()
    }).clone()
}

fn find_matching_brace(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'}' {
            depth -= 1;
            if depth == 0 { return Some(i); }
        } else if i + 1 < bytes.len() && bytes[i] == b'#' && bytes[i + 1] == b'{' {
            depth += 1;
            i += 1;
        }
        i += 1;
    }
    None
}

fn split_at_depth0(s: &str, delim: u8) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;       // #{...} nesting depth
    let mut in_style = false;      // inside #[...] style directive
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            depth += 1;
            i += 2;
            continue;
        }
        if bytes[i] == b'}' && depth > 0 {
            depth -= 1;
            i += 1;
            continue;
        }
        // Track #[...] style directives — commas inside are NOT delimiters
        if bytes[i] == b'#' && i + 1 < bytes.len() && bytes[i + 1] == b'[' && !in_style {
            in_style = true;
            i += 2;
            continue;
        }
        if bytes[i] == b']' && in_style {
            in_style = false;
            i += 1;
            continue;
        }
        // Handle #, (escaped delimiter) – skip over without splitting
        if bytes[i] == b'#' && i + 1 < bytes.len() && bytes[i + 1] == delim && depth == 0 {
            i += 2;
            continue;
        }
        if bytes[i] == delim && depth == 0 && !in_style {
            parts.push(s[start..i].to_string());
            start = i + 1;
        }
        i += 1;
    }
    parts.push(s[start..].to_string());
    parts
}

fn split_conditional(s: &str) -> (String, String, String) {
    let parts = split_at_depth0(s, b',');
    match parts.len() {
        0 => (String::new(), String::new(), String::new()),
        1 => (parts[0].clone(), String::new(), String::new()),
        2 => (parts[0].clone(), parts[1].clone(), String::new()),
        _ => (parts[0].clone(), parts[1].clone(), parts[2..].join(",")),
    }
}

fn glob_match(pattern: &str, text: &str, case_insensitive: bool) -> bool {
    let p = if case_insensitive { pattern.to_lowercase() } else { pattern.to_string() };
    let t = if case_insensitive { text.to_lowercase() } else { text.to_string() };
    glob_match_impl(p.as_bytes(), t.as_bytes())
}

fn glob_match_impl(pattern: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;
    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == text[ti]) {
            pi += 1; ti += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star_pi = pi; star_ti = ti; pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1; star_ti += 1; ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' { pi += 1; }
    pi == pattern.len()
}

// ─────────────────── list-* format helpers ───────────────────────

/// Default format for list-windows (tmux-style one-per-line).
pub fn default_list_windows_format() -> &'static str {
    "#{window_index}: #{window_name}#{window_flags} (#{window_panes} panes) [#{window_width}x#{window_height}]"
}

/// Default format for list-panes.
pub fn default_list_panes_format() -> &'static str {
    "#{pane_index}: [#{pane_width}x#{pane_height}] [history #{history_limit}/#{history_limit}] #{pane_id} (active)"
}

/// Default format for list-sessions.
pub fn default_list_sessions_format() -> &'static str {
    "#{session_name}: #{session_windows} windows (created #{session_created_string})"
}

/// Default format for list-buffers.
pub fn default_list_buffers_format() -> &'static str {
    "#{buffer_name}: #{buffer_size} bytes: \"#{buffer_sample}\""
}

/// Format a list of windows using a format string.
pub fn format_list_windows(app: &AppState, fmt: &str) -> String {
    let mut lines = Vec::with_capacity(app.windows.len());
    for (i, _win) in app.windows.iter().enumerate() {
        lines.push(expand_format_for_window(fmt, app, i));
    }
    lines.join("\n")
}

/// Format a list of sessions using a format string. psmux is single-session
/// per server, so this returns one line for the current session (matching
/// what tmux would emit for that server's session in its list-sessions -F).
pub fn format_list_sessions(app: &AppState, fmt: &str) -> String {
    expand_format(fmt, app)
}

/// Format a list of panes for the active window.
pub fn format_list_panes(app: &AppState, fmt: &str, win_idx: usize) -> String {
    let win = match app.windows.get(win_idx) {
        Some(w) => w,
        None => return String::new(),
    };
    let mut ids = Vec::new();
    collect_pane_ids(&win.root, &mut ids);
    // Floating panes follow the tiled ones, as tmux lists them (#767): they
    // were missing, so a script walking list-panes could never find one.
    ids.extend(win.floating.iter().map(|fp| fp.id));
    ids.iter().enumerate().map(|(pos, _pid)| {
        PANE_POS_OVERRIDE.set(Some(pos));
        let line = expand_format_for_window(fmt, app, win_idx);
        PANE_POS_OVERRIDE.set(None);
        line
    }).collect::<Vec<_>>().join("\n")
}

fn collect_pane_ids(node: &Node, ids: &mut Vec<usize>) {
    match node {
        Node::Leaf(p) => ids.push(p.id),
        Node::Split { children, .. } => {
            for child in children { collect_pane_ids(child, ids); }
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_format.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_issue662_mouse_flag_formats.rs"]
mod tests_issue662_mouse_flag_formats;

#[cfg(test)]
#[path = "../tests-rs/test_issue272_format_shell_cache.rs"]
mod tests_issue272_format_shell_cache;

#[cfg(test)]
#[path = "../tests-rs/test_modifier_over_empty_var.rs"]
mod tests_modifier_over_empty_var;

#[cfg(test)]
#[path = "../tests-rs/test_issue580_pane_start_command.rs"]
mod tests_issue580_pane_start_command;

#[cfg(test)]
#[path = "../tests-rs/test_issue712_format_modifier_utf8.rs"]
mod tests_issue712_format_modifier_utf8;

#[cfg(test)]
#[path = "../tests-rs/test_format_modifier_chain_parity.rs"]
mod tests_format_modifier_chain_parity;

#[cfg(test)]
#[path = "../tests-rs/test_issue724_client_formats.rs"]
mod tests_issue724_client_formats;

#[cfg(test)]
#[path = "../tests-rs/test_issue767_floating_panes.rs"]
mod tests_issue767_floating_panes;

#[cfg(test)]
#[path = "../tests-rs/test_issue734_side_sessionless.rs"]
mod tests_issue734_side_sessionless;
