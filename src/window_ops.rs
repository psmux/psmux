use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{PtySize, native_pty_system};
use ratatui::prelude::*;

use crate::types::{AppState, Mode, Pane, Node, LayoutKind, DragState, Window, FocusDir};
use crate::tree::{active_pane, active_pane_mut, compute_rects, compute_split_borders,
    split_sizes_at, adjust_split_sizes, get_split_mut, resize_all_panes};
use crate::pane::{detect_shell, build_default_shell, set_tmux_env};
use crate::copy_mode::{enter_copy_mode, exit_copy_mode, scroll_copy_up, scroll_copy_down, scroll_pane_scrollback, yank_selection};
use crate::platform::mouse_inject;

/// Mouse debug logger — writes to ~/.psmux/mouse_debug.log when
/// PSMUX_MOUSE_DEBUG=1 is set.
fn mouse_log(msg: &str) {
    use std::sync::LazyLock;
    static ENABLED: LazyLock<bool> = LazyLock::new(|| {
        std::env::var("PSMUX_MOUSE_DEBUG").unwrap_or_default() == "1"
    });
    if !*ENABLED { return; }

    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    if n > 2000 { return; }

    let path = format!("{}/mouse_debug.log", crate::paths::psmux_dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", chrono::Local::now().format("%H:%M:%S%.3f"), msg);
    }
}

/// Convert screen coordinates to 0-based pane-local coordinates.
/// No border offset — panes are borderless (tmux-style).
fn pane_inner_cell_0based(area: Rect, abs_x: u16, abs_y: u16) -> (i16, i16) {
    let col = abs_x as i16 - area.x as i16;
    let row = abs_y as i16 - area.y as i16;
    (col, row)
}

/// The row `pane-border-status` takes out of every pane's layout slot for its
/// label, resolved once from the server's own options.
///
/// tmux never needs this: `layout_fix_panes` bakes the label row into
/// `wp->yoff`/`wp->sy` once (layout.c), and then every mouse event — press,
/// drag, release, motion, wheel — converts through the single `cmd_mouse_at`
/// (cmd.c), so press and release cannot disagree.  psmux converts in two
/// places instead: the client turns a screen cell into a pane cell with
/// `client::pane_content_inner` before sending `pane-mouse`, while the verbs
/// that carry RAW screen coordinates (`mouse-up`, `mouse-drag`, `mouse-down`,
/// `mouse-move`, `scroll-up`/`scroll-down`) are converted here.  This side
/// used to subtract the layout slot's top rather than the content's, so with
/// `pane-border-status top` a press landed on the row under the pointer and
/// the matching release and drag landed one row lower (#669).  Both sides now
/// call the same helper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct PaneLabelRow {
    /// "top", "bottom" or "off" — already collapsed to the three cases
    /// `pane_content_inner` distinguishes, so no option string is borrowed.
    status: &'static str,
}

impl PaneLabelRow {
    /// Resolve from the live options, including the #414 default: only an
    /// explicitly emptied `pane-border-format` turns the label row off.
    pub(crate) fn from_options(app: &AppState) -> Self {
        let status = match app.user_options.get("pane-border-status").map(String::as_str) {
            Some("top") => "top",
            Some("bottom") => "bottom",
            _ => "off",
        };
        let format_empty = matches!(
            app.user_options.get("pane-border-format").map(String::as_str),
            Some("")
        );
        Self { status: if format_empty { "off" } else { status } }
    }

    /// The pane's content rect inside its layout slot.
    pub(crate) fn content(self, area: Rect) -> Rect {
        crate::client::pane_content_inner(
            area,
            self.status,
            crate::client::DEFAULT_PANE_BORDER_FORMAT,
        )
    }

    /// Screen cell → 0-based content cell, the conversion the client performs
    /// for `pane-mouse`.  The row is floored at 0 exactly as the client floors
    /// it, so a click on the label row itself still reports the pane's first
    /// content row rather than a negative one.
    pub(crate) fn cell_0based(self, area: Rect, abs_x: u16, abs_y: u16) -> (i16, i16) {
        let (col, row) = pane_inner_cell_0based(self.content(area), abs_x, abs_y);
        (col, row.max(0))
    }
}

/// Convert screen coordinates to 1-based pane-local coordinates.
fn pane_inner_cell(area: Rect, abs_x: u16, abs_y: u16) -> (u16, u16) {
    let col = abs_x.saturating_sub(area.x) + 1;
    let row = abs_y.saturating_sub(area.y) + 1;
    (col, row)
}

/// Map mouse coordinates from a client's terminal space to the server's effective
/// layout space.  When a client's terminal is larger or smaller than the effective
/// size used for layout computation, raw pixel coordinates don't match pane boundaries.
/// This ratio-based mapping is a "good enough" fallback for any interaction not yet
/// handled by client-side semantic commands.
fn map_client_coords(app: &AppState, x: u16, y: u16) -> (u16, u16) {
    let cid = match app.latest_client_id {
        Some(id) => id,
        None => return (x, y),
    };
    let (cw, ch) = match app.client_sizes.get(&cid) {
        Some(&size) => size,
        None => return (x, y),
    };
    let ew = app.last_window_area.width;
    let eh = app.last_window_area.height;
    if cw == ew && ch == eh {
        return (x, y);
    }
    let mx = if cw > 0 { ((x as u32) * (ew as u32) / (cw as u32)) as u16 } else { x };
    let my = if ch > 0 { ((y as u32) * (eh as u32) / (ch as u32)) as u16 } else { y };
    (mx.min(ew.saturating_sub(1)), my.min(eh.saturating_sub(1)))
}

/// Write a mouse event to the child PTY using the encoding the child requested.
pub fn write_mouse_event_remote(master: &mut dyn std::io::Write, button: u8, col: u16, row: u16, press: bool, enc: vt100::MouseProtocolEncoding) {
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            let ch = if press { 'M' } else { 'm' };
            let _ = write!(master, "\x1b[<{};{};{}{}", button, col, row, ch);
            let _ = master.flush();
        }
        _ => {
            if press {
                let cb = (button + 32) as u8;
                let cx = ((col as u8).min(223)) + 32;
                let cy = ((row as u8).min(223)) + 32;
                let _ = master.write_all(&[0x1b, b'[', b'M', cb, cx, cy]);
                let _ = master.flush();
            }
        }
    }
}

/// Inject a mouse event into a pane via Windows Console API (WriteConsoleInputW).
///
/// For native Windows console apps: WriteConsoleInputW injects MOUSE_EVENT records
/// that ReadConsoleInput returns.  This works for apps like pstop, Far Manager, etc.
fn inject_mouse(pane: &mut Pane, col: i16, row: i16, button_state: u32, event_flags: u32) -> bool {
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    if let Some(pid) = pane.child_pid {
        mouse_inject::send_mouse_event(pid, col, row, button_state, event_flags, false)
    } else {
        false
    }
}

/// Returns true if the window's foreground process is a VT bridge (wsl, ssh)
/// that needs VT mouse injection instead of Console API mouse injection.
fn is_vt_bridge(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("wsl") || lower.contains("ssh")
}

/// Wheel-forwarding gate: is the pane's child on the alternate screen?
///
/// The alternate screen is an authoritative wheel signal. The broader
/// mouse-protocol signal needs ownership attribution because PSReadLine can
/// enable it on behalf of the shell.
pub(crate) fn pane_in_alt_screen(pane: &Pane) -> bool {
    if let Ok(parser) = pane.term.lock() {
        return parser.screen().alternate_screen();
    }
    false
}

/// Attribute the pane's mouse protocol to whoever was foreground when it was
/// enabled (#548 follow-up).  Called from the server data tick, so the
/// process-table walk happens at ENABLEMENT time — sampling lazily at wheel
/// time would misattribute PSReadLine's lingering spurious tracking to
/// whatever app happens to be foreground when the user first scrolls.
///
/// Rules:
///   - protocol off               -> owner cleared
///   - protocol newly on/changed  -> `app_owned = (foreground_is_shell ==
///     Some(false))`.  A confirmed non-shell foreground at enablement means
///     the app itself asked for the mouse (Copilot CLI, the #570 echo
///     child); the shell (or an inconclusive probe) means PSReadLine-style
///     spurious tracking and the wheel stays on copy-mode semantics.
///   - protocol unchanged         -> owner kept (one walk per transition)
pub(crate) fn update_mouse_proto_owner(pane: &mut Pane) {
    let mode = match pane.term.lock() {
        Ok(parser) => parser.screen().mouse_protocol_mode(),
        Err(_) => return,
    };
    if mode == vt100::MouseProtocolMode::None {
        pane.mouse_proto_owner = None;
        // #613: a mode->None transition is NOT automatically the application
        // withdrawing.  Under ConPTY the pane's mouse protocol is driven by
        // conhost, which reports the console input mode word upstream, so a
        // wholesale rewrite of that word by an unrelated child arrives here as
        // an indistinguishable `ESC[?1003;1006l`.  Measured on this tree:
        // starting `node -e "process.stdin.setRawMode(true)"` inside a pane
        // whose TUI had ENABLE_MOUSE_INPUT on produced exactly that sequence,
        // with the TUI never told and never asked.
        //
        // So the latch is only dropped when the console looks like an
        // application narrowed its own mode.  When it carries libuv's raw-mode
        // signature the standing authorization survives, which is what tmux
        // does implicitly by keeping the mouse modes on the pane's own screen.
        //
        // The console query is skipped entirely unless a latch exists, so the
        // common case (every shell pane, on every data tick) stays free.
        if pane.wheel_auth.is_some() && !console_mode_is_libuv_raw(pane) {
            mouse_log("  -> wheel latch DROPPED: mouse protocol withdrawn by the pane app (#613)");
            pane.wheel_auth = None;
        }
        return;
    }
    let changed = match pane.mouse_proto_owner {
        Some((last, _)) => last != mode,
        None => true,
    };
    if changed {
        let fg_shell = pane
            .child_pid
            .and_then(crate::platform::process_info::foreground_is_shell);
        // #613: the process walk is not always ready in time.  Claude Code is
        // launched through a `claude` shim, so at the instant its DECSET is
        // parsed the pane's deepest leaf can still be the launcher — measured
        // here, with the whole sequence in one burst:
        //
        //   ESC[?1004h ESC[?1049h ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h
        //   -> mouse proto AnyMotion on pane 1 (fg_shell=Some(true) app_owned=false)
        //
        // and because the attribution is sampled ONCE per transition and the
        // mode never changes again, that wrong answer is frozen for the life of
        // the pane.  `#{mouse_any_flag}` reads no forever, which is exactly what
        // the reporter measured across five panes and three versions.
        //
        // The alternate screen settles it without reopening #548.  PSReadLine's
        // spurious tracking is enabled by the shell at a PROMPT, on the main
        // screen; an application that turns the mouse on while the pane is on
        // the alternate screen is not the shell.  This is the same
        // `alternate_on` term tmux carries in its default WheelUpPane binding
        // (key-bindings.c:510), used here for attribution rather than for
        // forwarding, and it cannot resurrect the #598 audience: htop and codex
        // never cause a mouse-protocol transition at all, so this branch is
        // unreachable for them.  A protocol that was already on before the app
        // reached the alternate screen produces no transition either, so an
        // inherited mode is still attributed where it was earned.
        let app_owned = fg_shell == Some(false) || pane_in_alt_screen(pane);
        mouse_log(&format!(
            "  -> mouse proto {:?} on pane {} (fg_shell={:?} alt={} app_owned={})",
            mode, pane.id, fg_shell, pane_in_alt_screen(pane), app_owned));
        pane.mouse_proto_owner = Some((mode, app_owned));
        if app_owned {
            latch_wheel_auth(pane);
        }
    }
}

/// libuv's raw-mode console input word: `uv_tty_set_mode(UV_TTY_MODE_RAW)`
/// ASSIGNS `ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT` rather than
/// clearing individual bits, and restores nothing when the process exits
/// (issue #613).
///
/// This is the fingerprint that tells "some node process raw-moded the shared
/// console" apart from "the application narrowed its own input mode".  An app
/// that decides it no longer wants the mouse clears one bit and keeps the rest
/// of its word (measured: `0x03B0` -> `0x03A0`); libuv leaves exactly `0x0208`.
pub(crate) const LIBUV_RAW_INPUT_MODE: u32 = 0x0008 | 0x0200;

/// Does the pane's child console carry [`LIBUV_RAW_INPUT_MODE`] right now?
///
/// A probe failure answers `false`, i.e. "treat it as a real withdrawal": the
/// safe side of #598 is always to suppress, never to type into an application
/// that did not ask.
fn console_mode_is_libuv_raw(pane: &mut Pane) -> bool {
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    match pane.child_pid.and_then(mouse_inject::query_console_input_mode) {
        Some(mode) => mode == LIBUV_RAW_INPUT_MODE,
        None => false,
    }
}

/// Record that this pane's application has asked for the mouse, anchored to
/// the process that asked (issue #613).
///
/// The anchor is the pane's foreground LEAF pid, not the pane root: the latch
/// has to expire when the application exits, or a wheel notch at the shell
/// prompt afterwards would be typed into the shell instead of entering copy
/// mode (#360).  The pane root outlives every application in the pane, so it
/// is only the fallback when the process walk cannot resolve a leaf.
///
/// Latching requires a CONFIRMED non-shell foreground OR the alternate screen,
/// the same rule `update_mouse_proto_owner` uses for `app_owned`.  PSReadLine
/// enables mouse tracking on the shell's behalf at a MAIN screen prompt, and a
/// shell prompt must never earn a standing authorization from it.
pub(crate) fn latch_wheel_auth(pane: &mut Pane) {
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    let Some(root) = pane.child_pid else { return };
    if crate::platform::process_info::foreground_is_shell(root) != Some(false)
        && !pane_in_alt_screen(pane)
    {
        return;
    }
    let owner = crate::platform::process_info::foreground_leaf_pid(root).unwrap_or(root);
    if matches!(pane.wheel_auth, Some(a) if a.owner_pid == owner) {
        return;
    }
    mouse_log(&format!("  -> wheel latch EARNED by pid {} in pane {} (#613)", owner, pane.id));
    pane.wheel_auth = Some(crate::types::WheelAuth { owner_pid: owner, alive_cache: None });
}

/// Does the pane's standing wheel authorization still hold (issue #613)?
///
/// Held while the process that earned it is alive AND still inside this pane's
/// process tree.  The tree check is what makes PID reuse harmless: a recycled
/// pid belonging to some unrelated program elsewhere on the machine is not
/// this pane's application and must not keep its authorization alive.
///
/// This is the last resort in the wheel gate: it is consulted only after both
/// live signals have already answered no, so a pane that never earned a latch
/// is exactly as silent as #598 made it.
pub(crate) fn wheel_auth_holds(pane: &mut Pane) -> bool {
    let Some(auth) = pane.wheel_auth else { return false };
    if let Some((ts, alive)) = auth.alive_cache {
        if ts.elapsed().as_secs() < 2 {
            return alive;
        }
    }
    let root = pane.child_pid;
    let alive = root.is_some_and(|r| {
        crate::platform::process_info::pid_in_pane_tree(r, auth.owner_pid)
    });
    if !alive {
        mouse_log(&format!("  -> wheel latch EXPIRED: owner pid {} left pane {} (#613)",
            auth.owner_pid, pane.id));
        pane.wheel_auth = None;
        return false;
    }
    pane.wheel_auth = Some(crate::types::WheelAuth {
        owner_pid: auth.owner_pid,
        alive_cache: Some((std::time::Instant::now(), true)),
    });
    true
}

/// The explicit opt-in for a pane whose application can never earn the gate
/// (issue #613).
///
/// The latch above rescues every pane that once satisfied a signal.  It cannot
/// rescue a pane that never did: measured on this tree, a node TUI that writes
/// `ESC[?1000h ESC[?1002h ESC[?1003h ESC[?1006h` and then enters raw mode can
/// have conhost swallow the DECSET entirely, so psmux sees no mouse protocol
/// at any point in the pane's life while the application reads SGR reports
/// perfectly well.  For that shape there is nothing to latch onto and the user
/// has to say so.
///
/// Two spellings, deliberately ordered narrowest first:
///
///   1. `set-option -p -t %N @mouse-force on`  — one pane, which is the scope
///      the damage #598 prevents is decided at.
///   2. `PSMUX_FORCE_WHEEL=1` in the server's environment — server wide, the
///      shape PR #614 proposed, kept as the last resort for a user who wants
///      it everywhere and does not want to set it per pane.
///
/// Both are off by default and neither relaxes anything else: #457's build
/// gate and #573's `PSMUX_FORCE_MOUSE` govern the opposite direction (whether
/// psmux may write mouse DECSET OUT to the terminal), and are untouched.
pub(crate) fn wheel_forced(pane: &Pane) -> bool {
    if let Some(v) = pane.pane_options.get("@mouse-force") {
        return matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "on" | "true" | "yes");
    }
    crate::ssh_input::wheel_gate_forced()
}

/// The wheel-forwarding decision (#548 + #570): forward when the pane is on
/// the alternate screen (nvim, htop, less — tmux `alternate_on`) OR when a
/// mouse protocol is active AND was enabled by the application itself (tmux
/// `mouse_any_flag`, minus PSReadLine's spurious enablement which
/// `update_mouse_proto_owner` attributes to the shell).  Everything else
/// gets copy-mode scrollback, exactly like tmux over a plain pane.
pub(crate) fn pane_wheel_forward(pane: &Pane) -> bool {
    pane_in_alt_screen(pane) || matches!(pane.mouse_proto_owner, Some((_, true)))
}

/// Gate for mouse click/button forwarding.
///
/// A click is forwarded when the pane's terminal state indicates mouse input:
///   1. it enabled a mouse protocol (DECSET 1000/1002/1003 — VT apps like vim,
///      and modern crossterm/ratatui apps, which emit DECSET 1000/1006 when
///      they turn mouse capture on); or
///   2. it is on the alternate screen (fullscreen apps on modern ConPTY).
///
/// This deliberately does not use screen-content or console-mode heuristics.
pub(crate) fn pane_wants_click(pane: &Pane) -> bool {
    if let Ok(parser) = pane.term.lock() {
        let screen = parser.screen();
        if screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None {
            return true;
        }
        if screen.alternate_screen() {
            return true;
        }
    }
    false
}

/// Strict check for BARE motion events: the pointer moved with no button held
/// (SGR button 35).  Returns true only when the child asked for any-event
/// tracking, DECSET 1003.
///
/// This does not use alt-screen or screen-content heuristics. Sending
/// unsolicited SGR motion sequences to apps that haven't
/// enabled mouse tracking (e.g. nvim without `set mouse=a`, or any TUI app
/// that only uses alt-screen for rendering) corrupts their input and makes
/// them appear hung.  (fixes #296)
///
/// DECSET 1002 is NOT enough (#604).  1002 is button-event tracking: report
/// motion only WHILE A BUTTON IS HELD.  Only 1003 asks for motion with no
/// button.  tmux draws the line in exactly the same place: a bare motion
/// report reaches a pane only under `MODE_MOUSE_ALL` (input-keys.c:737):
///
///     if (MOUSE_DRAG(m->sgr_b) &&
///         MOUSE_RELEASE(m->sgr_b) &&
///         (~s->mode & MODE_MOUSE_ALL))
///             return (0);
///
/// (SGR button 35 is the motion bit 32 plus the "no button" low bits 3, so it
/// is both MOUSE_DRAG and MOUSE_RELEASE.)  tmux does not even ask the OUTER
/// terminal for bare motion unless a pane wants 1003 (tty.c:897):
///
///     if (mode & MODE_MOUSE_ALL)
///             tty_puts(tty, "\033[?1000h\033[?1002h\033[?1003h");
///     else if (mode & MODE_MOUSE_BUTTON)
///             tty_puts(tty, "\033[?1000h\033[?1002h");
///
/// psmux accepted 1002 here, so merely moving the pointer over a pane running
/// `nvim -u NONE` (mouse=nvi, which enables 1002+1006 and never 1003) sprayed
/// an `ESC[<35;col;rowM` report at nvim for every pointer sample (#604).
pub(crate) fn pane_wants_bare_motion(pane: &Pane) -> bool {
    if let Ok(parser) = pane.term.lock() {
        mode_reports_bare_motion(parser.screen().mouse_protocol_mode())
    } else {
        false
    }
}

/// The mouse-mode half of `pane_wants_bare_motion`, split out so the tmux rule
/// itself can be asserted without building a live pane (#604).
pub(crate) fn mode_reports_bare_motion(mode: vt100::MouseProtocolMode) -> bool {
    mode == vt100::MouseProtocolMode::AnyMotion
}

/// Is this client mouse event a bare pointer move that nothing will act on?
///
/// The client sends `pane-mouse <id> 35 <col> <row> M` for every pointer
/// sample that lands inside a pane.  When the pane never asked for any-event
/// tracking the event forwards nothing, focuses nothing and selects nothing,
/// so the server state is byte for byte what it already was.  Pushing a frame
/// for it repaints the whole client screen at pointer-motion rate, which is
/// the flicker in #604: measured on this tree, sweeping the pointer 60 times
/// across a pane produced 60 full state frames and a redraw storm, against 0
/// while idle.
///
/// Copy mode is deliberately excluded: there a motion event does move the
/// selection cursor, so it is not inert.
pub fn pane_mouse_is_inert_motion(app: &AppState, pane_id: usize, button: u8) -> bool {
    if button != 35 {
        return false;
    }
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        return false;
    }
    let Some(win) = app.windows.get(app.active_idx) else { return true };
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    for (path, _) in &rects {
        if crate::tree::get_active_pane_id(&win.root, path) == Some(pane_id) {
            return match active_pane(&win.root, path) {
                Some(p) => !pane_wants_bare_motion(p),
                None => true,
            };
        }
    }
    true
}

/// Detect whether a pane has a VT bridge descendant (wsl.exe, ssh.exe, etc.)
/// by walking the process tree.  Result is cached for 2 seconds per pane
/// to avoid expensive CreateToolhelp32Snapshot on every mouse event.
fn detect_vt_bridge(pane: &mut Pane) -> bool {
    // Check cache first (2 second TTL)
    if let Some((ts, cached)) = pane.vt_bridge_cache {
        if ts.elapsed().as_secs() < 2 {
            return cached;
        }
    }
    // Ensure child_pid is resolved
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    let result = if let Some(pid) = pane.child_pid {
        crate::platform::process_info::has_vt_bridge_descendant(pid)
    } else {
        false
    };
    pane.vt_bridge_cache = Some((std::time::Instant::now(), result));
    result
}

/// `ENABLE_MOUSE_INPUT` — the console can hand the child MOUSE_EVENT records.
pub(crate) const ENABLE_MOUSE_INPUT: u32 = 0x0010;
/// `ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT` — the cooked-input pair.  A console
/// still carrying either of these is line buffered and echoing, which no
/// application that reads `INPUT_RECORD`s ever leaves in place.
pub(crate) const COOKED_INPUT_MODE: u32 = 0x0002 | 0x0004;
/// `ENABLE_VIRTUAL_TERMINAL_INPUT` — conhost stops parsing the child's input
/// and passes VT bytes through verbatim, so the child reassembles sequences
/// itself.  The paste route gate (issue #684) keys on this bit.
pub(crate) const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

/// The pane child's whole console input mode word, cached for 2 seconds.
///
/// One probe answers both mouse questions below, and the AttachConsole /
/// CreateFileW dance behind it is expensive enough to be worth sharing.
fn console_input_mode(pane: &mut Pane) -> Option<u32> {
    if let Some((ts, cached)) = pane.mouse_input_cache {
        if ts.elapsed().as_secs() < 2 {
            return cached;
        }
    }
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    let result = pane.child_pid.and_then(mouse_inject::query_console_input_mode);
    pane.mouse_input_cache = Some((std::time::Instant::now(), result));
    result
}

/// Detect whether the child's console has ENABLE_MOUSE_INPUT (0x0010) set.
///
/// When true, the console CAN deliver MOUSE_EVENT records to the child
/// (crossterm/ratatui apps like pstop, claude read them).  This is the
/// permissive answer to "may the wheel be forwarded at all": the bit is set in
/// the inherited Windows default, so a `true` here does NOT establish that the
/// child asked for the mouse.  Use [`detect_record_reader`] for that.
///
/// Result is cached for 2 seconds per pane.
fn detect_mouse_input(pane: &mut Pane) -> bool {
    console_input_mode(pane).map_or(false, |m| m & ENABLE_MOUSE_INPUT != 0)
}

/// Did the child DELIBERATELY configure its console to read `INPUT_RECORD`s?
///
/// `ENABLE_MOUSE_INPUT` on its own cannot answer this.  The mode a freshly
/// spawned console inherits is Windows' documented default, `0x01F7`, and that
/// word already contains `ENABLE_MOUSE_INPUT`, so every pane child looks like a
/// record reader from the moment it starts, before it has run a line of its own
/// code.  Measured on this tree, pane children with no `SetConsoleMode` call of
/// their own between them:
///
/// ```text
///   pwsh reading via [Console]::ReadKey  0x01F7  mouse=1 line=1 echo=1
///   Far Manager                          0x01B8  mouse=1 line=0 echo=0
///   pstop (crossterm)                    0x0098  mouse=1 line=0 echo=0
/// ```
///
/// What separates them is the rest of the word: a record reader always takes
/// the console out of cooked mode first, because line buffering and echo would
/// swallow the very keystrokes it wants as records.  So the discriminator is
/// `ENABLE_MOUSE_INPUT` set AND [`COOKED_INPUT_MODE`] clear.
///
/// `ENABLE_QUICK_EDIT_MODE` looks like it would work too (both readers above
/// clear it, the default sets it) but it must NOT be used: psmux clears that
/// very bit itself.  `mouse_inject::send_mouse_event` sets
/// `ENABLE_MOUSE_INPUT | ENABLE_EXTENDED_FLAGS` and clears
/// `ENABLE_QUICK_EDIT_MODE` on the child console before every injected record
/// and never restores it, so one forwarded wheel notch turns any pane into a
/// quick-edit-free "record reader".  Measured: the pwsh reader above went
/// `0x01F6 -> 0x01B6` across a single wheel event.  psmux never touches
/// `ENABLE_LINE_INPUT` or `ENABLE_ECHO_INPUT`, which is why the cooked pair is
/// the signal that survives its own mouse traffic.
pub(crate) fn detect_record_reader(pane: &mut Pane) -> bool {
    console_input_mode(pane).map_or(false, mode_is_deliberate_record_reader)
}

/// Does this pane's child read its input as a VT byte stream (issue #684)?
///
/// The paste route gate asks this before it may deliver `ESC[200~` as
/// `KEY_EVENT` records.  A byte stream reader (node, nvim, a crossterm app
/// running with `ENABLE_VIRTUAL_TERMINAL_INPUT`) reassembles the markers out of
/// the characters conhost hands it; an `INPUT_RECORD` reader cannot, and shows
/// them as the literal characters `[200~` instead, which is issue #98.
///
/// The discriminator is the `ENABLE_VIRTUAL_TERMINAL_INPUT` bit ALONE, and
/// deliberately NOT [`detect_record_reader`].  That heuristic answers a
/// different question ("may psmux flip this child's mode?") where erring toward
/// "record reader" is the safe side; here the safe side is the other one, and
/// the heuristic gets this wrong for exactly the children the gate is for.
/// Measured on this tree with the #684 recorder, whose two shapes differ only
/// in the bit that matters:
///
/// ```text
///   byte stream reader (VTI on, cooked off)   0x03F0  mouse=1 cooked=0 vti=1
///   record reader      (VTI off, cooked off)  0x01F8  mouse=1 cooked=0 vti=0
/// ```
///
/// `mode_is_deliberate_record_reader` is "mouse set and cooked clear", so it
/// calls BOTH of those record readers: `ENABLE_MOUSE_INPUT` is part of the
/// inherited `0x01F7` default and a byte stream reader has no reason to clear
/// it.  Gating the paste on it kept every pane on the pipe, which is the one
/// outcome issue #684 needs to change.  `ENABLE_VIRTUAL_TERMINAL_INPUT` is the
/// direct signal instead: with it set conhost stops parsing and passes the
/// bytes through, which is precisely the child that can reassemble `ESC[200~`.
///
/// A deliberate record reader never carries the bit.  crossterm's Windows
/// backend reads `INPUT_RECORD`s and never sets it (issue #98's Helix), and
/// psmux cannot set it behind its back either, because [`ensure_vti`] refuses
/// to flip a `detect_record_reader` pane (issue #623).
///
/// Unlike [`ensure_vti`] this only ever QUERIES.  A paste has no business
/// changing a child's console mode, so a child that has not set VTI itself
/// simply keeps the pipe.  It reads the cached mode word `console_input_mode`
/// already maintains for the mouse questions, so a paste costs no extra attach.
#[cfg(windows)]
pub(crate) fn pane_reads_vt_bytes(pane: &mut Pane) -> bool {
    console_input_mode(pane).map_or(false, |m| m & ENABLE_VIRTUAL_TERMINAL_INPUT != 0)
}

/// The pure classification behind [`detect_record_reader`], split out so it can
/// be pinned by unit tests against the modes measured from real applications.
pub(crate) fn mode_is_deliberate_record_reader(mode: u32) -> bool {
    mode & ENABLE_MOUSE_INPUT != 0 && mode & COOKED_INPUT_MODE == 0
}

/// Ensure the child's console has ENABLE_VIRTUAL_TERMINAL_INPUT set before
/// writing an SGR mouse sequence to its PTY input pipe.
///
/// Root cause of #277/#245: conhost silently drops VT bytes written to the
/// ConPTY master (`write_mouse_to_pty`) instead of delivering them to the
/// child as literal characters when the child's console has VTI off — which
/// is the default for a freshly spawned shell or TUI app that hasn't called
/// `SetConsoleMode` yet.  Without this, every wheel event forwarded to an
/// alt-screen/mouse-tracking app (nvim, vim, opencode, custom SGR readers)
/// vanishes before it reaches the child at all.
///
/// Cached for 2 seconds per pane (same TTL as the other mouse-inject
/// detectors) to avoid the AttachConsole/CreateFileW/SetConsoleMode dance on
/// every wheel tick once VTI is confirmed on.
///
/// # Never for a record reader (issue #623)
///
/// The flip is permanent, since nothing here restores the mode afterwards, and
/// it is the console's mode rather than psmux's, so it changes how conhost
/// treats EVERY later byte psmux writes to that ConPTY, keys included.  With
/// VTI off conhost PARSES the input stream and hands the child real
/// INPUT_RECORDs, so the three bytes psmux sends for F1 (`\x1bOP`) arrive as
/// one `VK_F1` press.  With VTI on conhost stops parsing and passes the bytes
/// through verbatim, so the same F1 arrives as the characters ESC, 'O', 'P'.
///
/// An application that reads `INPUT_RECORD`s cannot use those (Far Manager
/// ignores them), and the first thing it does when it wakes to read them is
/// re-apply its own console mode, which clears VTI again, so the key after the
/// lost one works.  That is exactly the reported symptom: inside psmux, Far's
/// F1 opened no help until it was pressed a second time.
///
/// Such a child does not need the flip in the first place.  It configured its
/// console to receive `MOUSE_EVENT` records, which is conhost's cue to turn the
/// very SGR report this function is preparing into one, and the record channel
/// in `inject_mouse_combined` delivers the wheel to it directly.
///
/// The gate has to be [`detect_record_reader`], NOT `ENABLE_MOUSE_INPUT` on its
/// own.  That bit is part of the console mode a child INHERITS (`0x01F7`), so
/// gating on it alone excused every freshly spawned pane app from the flip,
/// including the #277/#245 class this function exists for: an application that
/// writes DECSET 1000/1002/1003/1006 and then reads the SGR bytes itself never
/// calls `SetConsoleMode` at all, so it still carries the inherited word and was
/// misread as a record reader.  conhost then dropped every SGR report psmux
/// wrote, and the Win32 record injected alongside is no help to it because a
/// byte reader discards `MOUSE_EVENT` records.  Measured: the wheel stopped
/// reaching such a pane entirely.  A real record reader is recognised by the
/// rest of its mode word instead (see `detect_record_reader`).
fn ensure_vti(pane: &mut Pane) {
    if let Some((ts, cached)) = pane.vti_mode_cache {
        if cached || ts.elapsed().as_secs() < 2 {
            return;
        }
    }
    // Issue #623: leave a record reader's console mode alone.  Re-checked on
    // the same 2 second cadence as the rest of the probe (and
    // `console_input_mode` has its own cache), so a pane that later switches
    // to a VT reading app still gets the flip it needs for #277.
    if detect_record_reader(pane) {
        pane.vti_mode_cache = Some((std::time::Instant::now(), false));
        return;
    }
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    if let Some(pid) = pane.child_pid {
        let enabled = mouse_inject::query_vti_enabled(pid).unwrap_or(false)
            || mouse_inject::ensure_vti_enabled(pid);
        pane.vti_mode_cache = Some((std::time::Instant::now(), enabled));
    }
}

/// Classify the pane's foreground process for the scroll-wheel
/// alternate-scroll decision (issue #277): is it a confirmed non-shell
/// program, and if so what is it called?
///
/// Returns `(non_shell_fg, foreground_exe_name)`.  `non_shell_fg` is only
/// true on a *confirmed* `Some(false)` from
/// `platform::process_info::foreground_is_shell` — never on `None` (probe
/// failure) or `Some(true)` (confirmed shell) — the same tri-state gating
/// #381/#285 established for Ctrl+C routing, so a normal shell prompt keeps
/// entering copy mode on wheel-up (#360) and a probe hiccup never changes
/// behavior. `foreground_exe_name` lets the caller special-case legacy
/// DOS-heritage pagers (`more.com`) that don't consume arrow keys.
///
/// Cached for 2 seconds per pane (same TTL/rationale as `detect_vt_bridge` /
/// `detect_mouse_input` / `ensure_vti` above) since this walks the full
/// system process snapshot twice (`foreground_is_shell` +
/// `get_foreground_process_name`) — too expensive to redo on every wheel
/// tick during a fast scroll flick.
fn scroll_foreground_classify(pane: &mut Pane) -> (bool, Option<String>) {
    if let Some((ts, non_shell, ref name)) = pane.scroll_fg_cache {
        if ts.elapsed().as_secs() < 2 {
            return (non_shell, name.clone());
        }
    }
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    let (non_shell, name) = match pane.child_pid {
        Some(pid) => {
            let non_shell = crate::platform::process_info::foreground_is_shell(pid)
                .map_or(false, |is_shell| !is_shell);
            let name = crate::platform::process_info::get_foreground_process_name(pid);
            (non_shell, name)
        }
        None => (false, None),
    };
    pane.scroll_fg_cache = Some((std::time::Instant::now(), non_shell, name.clone()));
    (non_shell, name)
}

/// Helper: inject SGR mouse via WriteConsoleInputW KEY_EVENT records.
///
/// Used ONLY for WSL/SSH bridge children where the PTY pipe doesn't reach
/// the remote TUI.  For native ConPTY children, use write_mouse_to_pty().
fn inject_sgr_mouse(pane: &mut Pane, col: i16, row: i16, vt_button: u8, press: bool) -> bool {
    let vt_col = (col + 1).max(1) as u16;
    let vt_row = (row + 1).max(1) as u16;
    let ch = if press { 'M' } else { 'm' };
    let sgr_seq = format!("\x1b[<{};{};{}{}", vt_button, vt_col, vt_row, ch);
    mouse_log(&format!("  -> Console VT injection (KEY_EVENTs): seq={:?}", sgr_seq));
    if pane.child_pid.is_none() {
        pane.child_pid = mouse_inject::get_child_pid(&*pane.child);
    }
    if let Some(pid) = pane.child_pid {
        let ok = mouse_inject::send_vt_sequence(pid, sgr_seq.as_bytes());
        mouse_log(&format!("  -> Console VT inject result: {}", ok));
        ok
    } else {
        false
    }
}

/// Write a SGR mouse event to the pane's PTY master pipe.
///
/// This is the same mechanism Windows Terminal uses: write VT SGR mouse
/// escape sequences directly to the ConPTY input pipe.  ConPTY/conhost
/// then automatically:
///  - Translates SGR → MOUSE_EVENT records for apps using ReadConsoleInputW
///    (crossterm/ratatui: pstop, claude, opencode, etc.)
///  - Passes VT through for apps reading text/VT input (nvim, vim)
///
/// This works universally for ALL native ConPTY children — no need to
/// distinguish between crossterm vs nvim.  (fixes #60)
fn write_mouse_to_pty(pane: &mut Pane, col: i16, row: i16, vt_button: u8, press: bool) {
    use std::io::Write as _;
    let vt_col = (col + 1).max(1) as u16;
    let vt_row = (row + 1).max(1) as u16;
    let ch = if press { b'M' } else { b'm' };
    // Stack-allocated buffer — avoids heap allocation per mouse event.
    // Max SGR sequence: ESC[<btn;col;rowM = ~20 bytes worst case.
    let mut buf = [0u8; 32];
    let len = {
        let mut cursor = std::io::Cursor::new(&mut buf[..]);
        let _ = write!(cursor, "\x1b[<{};{};{}{}", vt_button, vt_col, vt_row, ch as char);
        cursor.position() as usize
    };
    mouse_log(&format!("  -> PTY pipe SGR mouse: seq={:?}", std::str::from_utf8(&buf[..len]).unwrap_or("?")));
    let _ = pane.writer.write_all(&buf[..len]);
    let _ = pane.writer.flush();
}

/// Does this native ConPTY mouse event get the Win32 `MOUSE_EVENT` record in
/// addition to the SGR write into the pane's ConPTY input pipe?
///
/// Split out from `inject_mouse_combined` so the whole routing table can be
/// asserted without building a live pane (#597).  Two rules, and only two:
///
///   * the wheel always gets the record, on every build.  That is #277, and it
///     is what makes the wheel work for Bubble Tea style apps whose VTI
///     console turns the pipe's SGR into KEY_EVENT text.
///   * every other event gets it only on a build whose conhost cannot deliver
///     the pipe write at all (`conpty_needs_mouse_record_bypass`), where the
///     record is the only channel that reaches the child.
///
/// The caller gates (`pane_wants_click`, `pane_wants_bare_motion`, the #598
/// wheel gate) run BEFORE this and are untouched: this decides how a report
/// travels, never whether one is owed.
pub(crate) fn record_bypass_applies(event_flags: u32) -> bool {
    event_flags & mouse_inject::MOUSE_WHEELED != 0
        || crate::ssh_input::conpty_needs_mouse_record_bypass()
}

/// Inject a mouse event into a pane using the best available method.
///
/// Architecture (mirrors Windows Terminal):
///
///   For native ConPTY children, write SGR mouse escape sequences directly
///   to the PTY master pipe (pane.writer).  This is the same mechanism
///   Windows Terminal uses.  ConPTY/conhost handles all translation:
///   - Apps using ReadConsoleInputW (crossterm/ratatui) get MOUSE_EVENT records
///   - Apps reading VT input (nvim/vim) get the SGR sequences directly
///
///   For WSL/SSH bridge children, bypass ConPTY using WriteConsoleInputW
///   with KEY_EVENT records, delivering escape sequences to the bridge
///   process (wsl.exe/ssh.exe) which relays them to the Linux PTY.
///
///   At shell prompts (no TUI), no mouse forwarding is needed — the shell
///   doesn't handle mouse events.  Callers should handle shell-level
///   behavior (right-click=paste, scroll=copy-mode) before calling this.
pub(crate) fn inject_mouse_combined(pane: &mut Pane, col: i16, row: i16, vt_button: u8, press: bool,
                          _button_state: u32, _event_flags: u32, win_name: &str) {
    let vt_bridge = detect_vt_bridge(pane);

    if vt_bridge {
        // WSL/SSH bridge — bypass ConPTY, inject as KEY_EVENT records.
        // The bridge (wsl.exe, ssh.exe) relays these to the Linux PTY.
        //
        // Gate on mouse_protocol_mode (tmux + Windows Terminal parity):
        // Only forward mouse events when the remote app has explicitly
        // enabled mouse tracking (DECSET 1000/1002/1003).  For VT bridge
        // children, VT escape sequences pass through unmodified, so
        // mouse_protocol_mode() accurately reflects the remote app's
        // actual mouse tracking state.
        //
        // Without this gate, SGR mouse sequences are injected as KEY_EVENT
        // records → ssh.exe/wsl.exe relays them as literal text → the
        // remote shell prints raw escape sequences at the prompt.
        // This is the root cause of issue #77 (mouse events leak as raw
        // text into SSH panes).
        let mode = pane.term.lock().ok()
            .map_or(vt100::MouseProtocolMode::None, |t| t.screen().mouse_protocol_mode());
        let wants = mode != vt100::MouseProtocolMode::None;
        if !wants {
            // The mode is part of the message on purpose: #604 was a case where
            // the remote app HAD enabled 1002 and psmux itself knocked the mode
            // back to None, which is indistinguishable from "never asked"
            // unless the log says which one it saw.
            mouse_log(&format!("inject_mouse_combined: col={} row={} vt_btn={} press={} win={} vt_bridge=true mode={:?} -> SUPPRESSED (remote has no mouse tracking)",
                col, row, vt_button, press, win_name, mode));
            return;
        }
        mouse_log(&format!("inject_mouse_combined: col={} row={} vt_btn={} press={} win={} vt_bridge=true mode={:?} -> WriteConsoleInputW KEY_EVENT injection",
            col, row, vt_button, press, win_name, mode));
        inject_sgr_mouse(pane, col, row, vt_button, press);
    } else {
        // Native ConPTY child — write SGR mouse to PTY pipe.
        // This is the same mechanism Windows Terminal uses.
        // ConPTY translates SGR → MOUSE_EVENT for crossterm apps,
        // and passes VT through for nvim/vim.
        mouse_log(&format!("inject_mouse_combined: col={} row={} vt_btn={} press={} win={} -> PTY pipe SGR mouse (Windows Terminal method)",
            col, row, vt_button, press, win_name));
        // tmux parity for the wheel (#598): a mouse REPORT is only ever
        // written to a pane whose application actually enabled a mouse
        // protocol.  tmux enforces that in input_key_mouse (input-keys.c):
        //
        //     if (m->ignore || (s->mode & ALL_MOUSE_MODES) == 0)
        //             return;
        //
        // The `alternate_on` term in tmux's default WheelUpPane binding
        // (key-bindings.c: `if -F '#{||:#{alternate_on},#{pane_in_mode},
        // #{mouse_any_flag}}' { send -M } { copy-mode -e }`) only decides
        // "do not fall through to copy-mode".  On the alternate screen with
        // no mouse mode, `send -M` writes nothing at all.
        //
        // psmux forwarded on the alternate screen ALONE, so a full-screen app
        // that never asked for the mouse had raw `ESC[<64;col;rowM` typed into
        // it and read the report as keystrokes: htop opened its "Search: "
        // prompt and filled it with the digits and separators of the report,
        // and codex lost its transcript (#598).
        //
        // Both delivery channels have to obey the gate.  The PTY pipe is the
        // obvious one, but the Win32 MOUSE_EVENT record injected below is not
        // self gating as it looks: WriteConsoleInputW puts the record straight
        // into the input buffer, and a child reading with
        // ENABLE_VIRTUAL_TERMINAL_INPUT gets conhost's VT translation of that
        // record, which is the same `ESC[<64;col;rowM`.  Measured on this
        // tree: suppressing only the pipe write still leaked the report.
        // Clicks and drags keep their existing `pane_wants_click` gate; only
        // the wheel path is in scope here.
        //
        // "Did the application ask for the mouse" has two authoritative
        // answers on Windows, and the raw `mouse_protocol_mode()` is NOT one
        // of them: a freshly spawned console already has ENABLE_MOUSE_INPUT
        // in its inherited input mode, so conhost emits `ESC[?1003;1006h`
        // upstream before any application has run, and PSReadLine re-enables
        // tracking on its own (#360/#548).  Measured on this tree: a pane
        // whose child had explicitly CLEARED ENABLE_MOUSE_INPUT still showed
        // `?1003;1006h` in the raw pane stream.  The two signals that do mean
        // the application asked:
        //
        //   1. `mouse_proto_owner` — a DECSET transition that
        //      `update_mouse_proto_owner` attributed to a confirmed non-shell
        //      foreground.  This is the accurate signal for VT panes whose
        //      DECSET actually survives (bridges, passthrough).
        //   2. ENABLE_MOUSE_INPUT on the child console RIGHT NOW.  Console
        //      input mode lives on the shared input buffer, so this reports
        //      whatever the current foreground app last asked for, which is
        //      exactly how a real Windows TUI (crossterm, ratatui, libuv)
        //      registers.
        //
        // The console query costs an AttachConsole dance (2 second cache), so
        // it is only run for the wheel, and only after the cheap
        // `mouse_proto_owner` check has already failed.  Clicks, drags and
        // motion never reach it.
        //
        // BOTH of those signals resolve to the same console input mode word,
        // and that word belongs to the console rather than to the application
        // that set it (#613).  `uv_tty_set_mode` assigns
        // `ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT` over the whole
        // word and restores nothing on exit, so ANY node process anywhere in
        // the pane's tree — a subagent, an MCP server, one tool invocation —
        // takes ENABLE_MOUSE_INPUT away and makes conhost publish the loss as
        // `ESC[?1003;1006l`, which clears `mouse_proto_owner` too.  Measured
        // here: a pane whose TUI held `0x03B0` forwarded the wheel, and after
        // one `node -e "process.stdin.setRawMode(true)"` it read `0x0208` and
        // forwarded nothing, permanently, with the TUI never consulted.
        //
        // tmux cannot have this bug: its authorization is `s->mode &
        // ALL_MOUSE_MODES` on the pane's OWN screen (input-keys.c:805), set by
        // the app's DECSET and cleared only by its DECRST or a respawn.  So
        // psmux keeps a pane-owned record of the same thing — `wheel_auth`,
        // anchored to the process that earned it — and consults it last, after
        // both live signals have already said no.  A pane that never earned an
        // authorization is exactly as silent as #598 made it.
        //
        // This site keeps the PERMISSIVE `detect_mouse_input`, while the #623
        // gate in `ensure_vti` uses the strict `detect_record_reader`, and the
        // asymmetry is deliberate: here a `true` only lets the wheel THROUGH,
        // so the inherited `ENABLE_MOUSE_INPUT` erring towards delivery costs
        // nothing (the `mouse_protocol_mode` check above has already run, and
        // the `wheel_auth` latch is the real backstop).  There a `true`
        // WITHHOLDS the VTI flip, and the same inherited bit would then silence
        // the #277 apps for good.
        if _event_flags & mouse_inject::MOUSE_WHEELED != 0 && !wheel_forced(pane) {
            if matches!(pane.mouse_proto_owner, Some((_, true))) || detect_mouse_input(pane) {
                // A live signal answered yes: refresh the latch so the
                // authorization outlives the next child that raw-modes the
                // console.
                latch_wheel_auth(pane);
            } else if !wheel_auth_holds(pane) {
                mouse_log("  -> wheel report SUPPRESSED on both channels: pane app enabled no \
                           mouse protocol (tmux input_key_mouse parity, #598)");
                return;
            }
        }

        // #277/#245: conhost silently drops the SGR bytes below unless the
        // child's console already has ENABLE_VIRTUAL_TERMINAL_INPUT set —
        // ensure it first so the sequence actually reaches VT-reading apps
        // (nvim, vim, opencode) instead of vanishing before the child sees it.
        ensure_vti(pane);
        write_mouse_to_pty(pane, col, row, vt_button, press);

        // Also inject a Win32 MOUSE_EVENT record.
        //
        // Some TUI frameworks (Bubble Tea / Go apps like opencode) enable
        // VT input mode (ENABLE_VIRTUAL_TERMINAL_INPUT) for keyboard but
        // read mouse events as MOUSE_EVENT records via ReadConsoleInput.
        // When VTI is on, ConPTY passes the SGR mouse sequence through
        // as KEY_EVENT text instead of converting to MOUSE_EVENT, so the
        // app's ReadConsoleInput loop never sees a mouse event.
        //
        // The Win32 MOUSE_EVENT injection bypasses ConPTY entirely and
        // delivers the event directly to the child's console input buffer.
        //
        // On 22523 and above this stays wheel only, exactly as #277 shipped
        // it: the pipe write above already reaches the child there, and
        // widening the record channel would risk a second copy of every
        // click for apps where conhost does convert SGR to MOUSE_EVENT.
        //
        // Below 22523 the record channel is the ONLY one that works, so it
        // covers clicks, releases, drags and motion as well (#597).  Those
        // builds are the ones CONPTY_MOUSE_MIN_BUILD already documents as
        // unable to hand an inbound SGR mouse report to the child, so the
        // pipe write cannot be the duplicate: the reporter measured a
        // crossterm app receiving every wheel notch through this record
        // channel on real 19045 while a node child reading VT bytes received
        // nothing at all.  There is no double delivery to create, only the
        // clicks that were missing.  Every event that gets here has already
        // passed its own caller gate (pane_wants_click, or
        // pane_wants_bare_motion for DECSET 1003, or the wheel gate above),
        // so this widens the CHANNEL, never the audience: an application
        // that never asked for the mouse still receives nothing (#598).
        //
        // tmux parity: tmux writes SGR bytes and has no record channel at
        // all.  The MOUSE_EVENT bypass is a Windows only extension that
        // exists because conhost sits between psmux and the pane child.
        if record_bypass_applies(_event_flags) {
            let reason = if _event_flags & mouse_inject::MOUSE_WHEELED != 0 {
                "wheel, fixes #277"
            } else {
                "build below CONPTY_MOUSE_MIN_BUILD, #597"
            };
            mouse_log(&format!("  -> also injecting Win32 MOUSE_EVENT ({})", reason));
            inject_mouse(pane, col, row, _button_state, _event_flags);
        }
    }
}

/// Temporarily unzoom for an operation, saving the zoom state so it can be
/// restored via `pop_zoom()` afterwards (tmux push/pop semantics).
/// Returns true if zoom was active and was suspended.
pub fn push_zoom(app: &mut AppState) -> bool {
    if app.windows[app.active_idx].zoom_saved.is_some() {
        // Mark that we had zoom active, unzoom, but DON'T clear zoom_saved
        // — we move it to a temp slot so pop_zoom can re-apply it.
        unzoom_if_zoomed(app);
        true
    } else {
        false
    }
}

/// Re-apply zoom after a push_zoom operation (tmux push/pop semantics).
/// Only re-zooms if `was_zoomed` is true.
pub fn pop_zoom(app: &mut AppState, was_zoomed: bool) {
    if was_zoomed && app.windows[app.active_idx].zoom_saved.is_none() {
        toggle_zoom(app);
    }
}

/// If zoom is currently active, unzoom (restore saved sizes) and resize panes.
/// Returns true if zoom was active and was cancelled.
pub fn unzoom_if_zoomed(app: &mut AppState) -> bool {
    if let Some(saved) = app.windows[app.active_idx].zoom_saved.take() {
        let win = &mut app.windows[app.active_idx];
        for (p, sz) in saved.into_iter() {
            if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &p) { *sizes = sz; }
        }
        resize_all_panes(app);
        true
    } else {
        false
    }
}

pub fn toggle_zoom(app: &mut AppState) {
    let win = &mut app.windows[app.active_idx];
    if win.zoom_saved.is_none() {
        let mut saved: Vec<(Vec<usize>, Vec<u16>)> = Vec::new();
        for depth in 0..win.active_path.len() {
            let p = win.active_path[..depth].to_vec();
            if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &p) {
                let idx = win.active_path.get(depth).copied().unwrap_or(0);
                saved.push((p.clone(), sizes.clone()));
                for i in 0..sizes.len() { sizes[i] = if i == idx { 100 } else { 0 }; }
            }
        }
        win.zoom_saved = Some(saved);
    } else {
        if let Some(saved) = app.windows[app.active_idx].zoom_saved.take() {
            let win = &mut app.windows[app.active_idx];
            for (p, sz) in saved.into_iter() {
                if let Some(Node::Split { sizes, .. }) = get_split_mut(&mut win.root, &p) { *sizes = sz; }
            }
        }
    }
    // Resize all panes so child PTYs are notified of the new dimensions.
    // Without this, zoomed panes keep their pre-zoom size and child apps
    // (neovim, bottom, etc.) render in only half the screen. (issue #35)
    resize_all_panes(app);
}

pub fn remote_mouse_down(app: &mut AppState, x: u16, y: u16) {
    let (x, y) = map_client_coords(app, x, y);
    // Status bar tab clicks are handled client-side via select-window.
    // Only handle pane focus and border resize here.
    let status_row = app.last_window_area.y + app.last_window_area.height;
    if y == status_row {
        return;
    }

    // Floating panes sit above the tiled layout: a click inside a float grabs
    // it (tmux moves/resizes floats by dragging) and gives it focus. The
    // bottom-right edge starts a resize; the body starts a move.
    {
        let ox = app.last_window_area.x;
        let oy = app.last_window_area.y;
        let hit = {
            let win = &app.windows[app.active_idx];
            win.floating.iter().enumerate().rev().find_map(|(i, fp)| {
                let x0 = ox + fp.x; let y0 = oy + fp.y;
                if x >= x0 && x < x0 + fp.w && y >= y0 && y < y0 + fp.h {
                    Some((i, x0, y0, fp.w, fp.h))
                } else { None }
            })
        };
        if let Some((i, x0, y0, w, h)) = hit {
            let on_edge = x >= x0 + w.saturating_sub(1) || y >= y0 + h.saturating_sub(1);
            let mode = if on_edge {
                crate::types::FloatDragMode::Resize
            } else {
                crate::types::FloatDragMode::Move { dx: x - x0, dy: y - y0 }
            };
            app.windows[app.active_idx].floating_focus = Some(i);
            app.float_drag = Some(crate::types::FloatDrag { index: i, mode });
            return;
        }
    }

    let label = PaneLabelRow::from_options(app);
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    let mut active_area: Option<Rect> = None;
    for (path, area) in rects.iter() {
        if area.contains(ratatui::layout::Position { x, y }) {
            win.active_path = path.clone();
            // Update MRU for clicked pane (tmux parity #70)
            if let Some(pid) = crate::tree::get_active_pane_id(&win.root, path) {
                crate::tree::touch_mru(&mut win.pane_mru, pid);
            }
            active_area = Some(*area);
        }
    }

    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        app.copy_anchor = None;
        app.copy_pos_published = None;
        if let Some(area) = active_area {
            let (row, col) = copy_cell_for_area(label.content(area), x, y);
            app.copy_pos = Some((row, col));
            app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
            app.copy_mouse_down_cell = Some((row, col));
        }
        return;
    }

    let mut on_border = false;
    // Skip border detection when zoomed — no visible borders (#82)
    let mut borders: Vec<(Vec<usize>, LayoutKind, usize, u16, u16)> = Vec::new();
    if win.zoom_saved.is_none() {
        compute_split_borders(&win.root, app.last_window_area, &mut borders);
    }
    let tol = 1u16;
    for (path, kind, idx, pos, total_px) in borders.iter() {
        match kind {
            LayoutKind::Horizontal => {
                if x >= pos.saturating_sub(tol) && x <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: *pos, start_y: y, left_initial: left, _right_initial: right, total_pixels: *total_px }); } on_border = true; break; }
            }
            LayoutKind::Vertical => {
                if y >= pos.saturating_sub(tol) && y <= pos + tol { if let Some((left,right)) = split_sizes_at(&win.root, path.clone(), *idx) { app.drag = Some(DragState { split_path: path.clone(), kind: *kind, index: *idx, start_x: x, start_y: *pos, left_initial: left, _right_initial: right, total_pixels: *total_px }); } on_border = true; break; }
            }
        }
    }

    // Forward left-click only when active pane wants mouse input.
    if !on_border {
        if let Some(area) = active_area {
            let (col, row) = label.cell_0based(area, x, y);
            let win_name = win.name.clone();
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                if pane_wants_click(active) {
                    inject_mouse_combined(active, col, row, 0, true,
                        mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED, 0, &win_name);
                }
            }
        }
    }
}

pub fn remote_mouse_drag(app: &mut AppState, x: u16, y: u16) {
    let (x, y) = map_client_coords(app, x, y);

    // A floating-pane drag moves or resizes the grabbed float, following the
    // cursor. Runs before any tiled handling and short-circuits it.
    if let Some(fd) = app.float_drag {
        let ox = app.last_window_area.x;
        let oy = app.last_window_area.y;
        let win_w = app.last_window_area.width.max(10);
        let win_h = app.last_window_area.height.max(10);
        let win = &mut app.windows[app.active_idx];
        if let Some(fp) = win.floating.get_mut(fd.index) {
            match fd.mode {
                crate::types::FloatDragMode::Move { dx, dy } => {
                    let nx = x.saturating_sub(ox).saturating_sub(dx);
                    let ny = y.saturating_sub(oy).saturating_sub(dy);
                    let (cx, cy) = crate::floating::clamp_into(nx, ny, fp.w, fp.h, win_w, win_h);
                    fp.x = cx; fp.y = cy;
                }
                crate::types::FloatDragMode::Resize => {
                    let x0 = ox + fp.x;
                    let y0 = oy + fp.y;
                    fp.w = (x.saturating_sub(x0) + 1).max(3).min(win_w);
                    fp.h = (y.saturating_sub(y0) + 1).max(3).min(win_h);
                    let (cx, cy) = crate::floating::clamp_into(fp.x, fp.y, fp.w, fp.h, win_w, win_h);
                    fp.x = cx; fp.y = cy;
                    let inner_h = fp.h.saturating_sub(2).max(1);
                    let inner_w = fp.w.saturating_sub(2).max(1);
                    if fp.pane.last_rows != inner_h || fp.pane.last_cols != inner_w {
                        let _ = fp.pane.master.resize(portable_pty::PtySize { rows: inner_h, cols: inner_w, pixel_width: 0, pixel_height: 0 });
                        if let Ok(mut parser) = fp.pane.term.lock() { parser.screen_mut().set_size(inner_h, inner_w); }
                        fp.pane.last_rows = inner_h;
                        fp.pane.last_cols = inner_w;
                    }
                }
            }
        }
        return;
    }

    let label = PaneLabelRow::from_options(app);
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        // Prefer the pane under the pointer; fall back to the active
        // (copy-mode) pane so a drag that leaves every pane rect still
        // extends the selection and can auto-scroll at the edges.
        let target = rects.iter()
            .find(|(_, area)| area.contains(ratatui::layout::Position { x, y }))
            .or_else(|| rects.iter().find(|(p, _)| *p == win.active_path))
            .map(|(p, a)| (p.clone(), label.content(*a)));
        if let Some((path, area)) = target {
            win.active_path = path;
            let (row, col) = copy_cell_for_area(area, x, y);
            if app.copy_anchor.is_none() {
                // Only start selection when mouse moves to a different cell
                // than the click position. Prevents micro-drag jitter (#199).
                if app.copy_pos == Some((row, col)) {
                    return;
                }
                app.copy_anchor = Some(app.copy_pos.unwrap_or((row, col)));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
            app.copy_pos = Some((row, col));
            // The endpoint's own offset, recorded before the edge scroll below
            // moves the view: it is what makes this screen row mean a content
            // line.  Without it a drag that hit an edge copied a different
            // range than the one that was painted.
            app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
            // tmux parity (#62): dragging on/past the pane's first or last
            // row scrolls the view so the selection continues into scrollback.
            if y <= area.y {
                app.copy_mouse_down_cell = None;
                scroll_copy_up(app, 1);
            } else if y + 1 >= area.y + area.height {
                app.copy_mouse_down_cell = None;
                scroll_copy_down(app, 1);
            }
        }
        return;
    }

    if let Some(d) = &app.drag {
        adjust_split_sizes(&mut win.root, d, x, y);
    } else {
        // Forward drag only when active pane wants mouse input.
        if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
            let (col, row) = label.cell_0based(area, x, y);
            let win_name = win.name.clone();
            if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
                if pane_wants_click(active) {
                    inject_mouse_combined(active, col, row, 32, true,
                        mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED, mouse_inject::MOUSE_MOVED, &win_name);
                }
            }
        }
    }
}

pub fn remote_mouse_up(app: &mut AppState, x: u16, y: u16) {
    let (x, y) = map_client_coords(app, x, y);
    // End any floating-pane drag.
    if app.float_drag.is_some() {
        app.float_drag = None;
        return;
    }
    let label = PaneLabelRow::from_options(app);
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        // The release cell, kept for the #199 click guard only.  It must not
        // become the selection endpoint: tmux's `window_copy_drag_release`
        // only clears the drag state, and the cursor moves on drag or
        // bare-motion events alone, so the endpoint stays whatever the last
        // drag left.  Taking the release cell here yanked a character the user
        // never saw highlighted (see `handle_pane_mouse`).
        let mut release_cell = None;
        if let Some((path, area)) = rects.iter().find(|(_, area)| area.contains(ratatui::layout::Position { x, y })) {
            win.active_path = path.clone();
            release_cell = Some(copy_cell_for_area(label.content(*area), x, y));
            // Only a release with no drag endpoint of its own may position the
            // cursor — a press that belonged to another pane, or that arrived
            // before copy mode opened (#669).  Once a drag has anchored a
            // selection, the endpoint is the last drag cell.
            if app.copy_anchor.is_none() {
                app.copy_pos = release_cell;
            }
        }
        // If mouse-up is within 1 cell of mouse-down, it was a plain click
        // (any anchor set by jittery drag events is spurious). Clear it. (#199)
        // Mouse jitter during a click can shift the cursor by 1 cell.
        let click_origin = app.copy_mouse_down_cell.take();
        if let (Some((dr, dc)), Some((ur, uc))) = (click_origin, release_cell.or(app.copy_pos)) {
            let row_diff = (dr as i32 - ur as i32).unsigned_abs();
            let col_diff = (dc as i32 - uc as i32).unsigned_abs();
            if row_diff <= 1 && col_diff <= 1 {
                app.copy_anchor = None;
                app.copy_pos = Some((dr, dc)); // snap to the original click position
                app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
                return;
            }
        }
        // Raw clients (mouse-down/drag/up) forward the terminal's reports
        // verbatim and cannot re-report what they painted, so a slip reported
        // together with the release — a phone's coarse last report, coalesced
        // motion — is dropped here by falling back to the endpoint of the last
        // frame: no frame carried that cell, so it was never highlighted.  The
        // release cell itself never extends the selection either way.
        if let Some(published) = app.copy_pos_published {
            app.copy_pos = Some(published);
            app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
        }
        // Auto-yank if a real selection exists, else clear the stale anchor.
        // Compare CONTENT positions (screen row minus the scroll offset it
        // was recorded at, as yank_selection does), not screen cells: edge
        // auto-scroll can park the release on the anchor's screen cell
        // while the selection spans many scrolled lines.
        if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
            let a_abs = a.0 as i64 - app.copy_anchor_scroll_offset as i64;
            let p_abs = p.0 as i64 - app.copy_pos_scroll_offset.unwrap_or(app.copy_scroll_offset) as i64;
            if (a_abs, a.1) != (p_abs, p.1) {
                let _ = yank_selection(app);
            }
            // tmux parity (#62): ending a drag cancels copy mode and
            // returns to the live view (copy-pipe-and-cancel) even when
            // the selection came up empty, matching the pane-mouse
            // release path.
            exit_copy_mode(app);
        }
        return;
    }

    // If we were dragging a border, resize all panes to match new layout
    let was_dragging = app.drag.is_some();
    app.drag = None;
    if was_dragging {
        resize_all_panes(app);
        return;
    }

    // Forward mouse release only when active pane wants mouse input.
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let (col, row) = label.cell_0based(area, x, y);
        let win_name = win.name.clone();
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if pane_wants_click(active) {
                inject_mouse_combined(active, col, row, 0, false,
                    0, 0, &win_name);
            }
        }
    }
}

/// Forward a non-left mouse button press/release to the child.
pub fn remote_mouse_button(app: &mut AppState, x: u16, y: u16, button: u8, press: bool) {
    let (x, y) = map_client_coords(app, x, y);
    let label = PaneLabelRow::from_options(app);
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let (col, row) = label.cell_0based(area, x, y);
        let win_name = win.name.clone();
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if pane_wants_click(active) {
                let sgr_btn = match button {
                    1 => 1u8, // middle
                    2 => 2u8, // right
                    _ => 0u8,
                };
                let button_state = if press {
                    match button {
                        1 => mouse_inject::FROM_LEFT_2ND_BUTTON_PRESSED,
                        2 => mouse_inject::RIGHTMOST_BUTTON_PRESSED,
                        _ => 0,
                    }
                } else {
                    0
                };
                inject_mouse_combined(active, col, row, sgr_btn, press,
                    button_state, 0, &win_name);
            }
        }
    }
}

/// Forward bare mouse motion (hover) to the child PTY.
///
/// Only forwarded when the child has EXPLICITLY enabled any-event motion
/// tracking (`pane_wants_bare_motion`, DECSET 1003). Screen-content heuristics
/// false-positive on a filled screen with a NON-shell foreground
/// (podman/docker interactive containers, discussion #349), spraying raw
/// SGR motion bytes (35;x;yM...) into the container tty as visible garbage.
/// SGR button 35 = bare motion with no button held (WT parity).
/// Windows Terminal encodes hover as WM_MOUSEMOVE -> button 3 + 0x20 = 35.
///
/// Same-coordinate events are suppressed (Windows Terminal parity: the
/// terminal only sends motion when coordinates actually change).
///
/// Returns true when a report was actually written into a pane.  The caller
/// uses that to decide whether the move is worth a redraw: a bare pointer
/// move that reaches no pane changes nothing on screen, and repainting for it
/// is what made the cursor flicker while the mouse was merely moving (#604).
pub fn remote_mouse_motion(app: &mut AppState, x: u16, y: u16) -> bool {
    let (x, y) = map_client_coords(app, x, y);
    // WT parity: suppress same-coordinate duplicates
    if app.last_hover_pos == Some((x, y)) {
        return false;
    }
    app.last_hover_pos = Some((x, y));

    let label = PaneLabelRow::from_options(app);
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);

    // Forward hover only when the child explicitly enabled motion tracking.
    // This avoids leaking raw SGR motion bytes (ESC[<35;...) into shell-style
    // prompts such as claudecode input boxes and container ttys (#349).
    mouse_log(&format!("remote_mouse_motion: x={} y={}", x, y));

    if let Some(area) = rects.iter().find(|(path, _)| *path == win.active_path).map(|(_, a)| *a) {
        let (col, row) = label.cell_0based(area, x, y);
        let win_name = win.name.clone();
        if let Some(active) = active_pane_mut(&mut win.root, &win.active_path) {
            if pane_wants_bare_motion(active) {
                inject_mouse_combined(active, col, row, 35, true,
                    0, mouse_inject::MOUSE_MOVED, &win_name);
                return true;
            }
        }
    }
    false
}

fn wheel_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 1-based pane-local coordinates (no border offset).
    let col = x.saturating_sub(area.x).min(area.width.saturating_sub(1)).saturating_add(1);
    let row = y.saturating_sub(area.y).min(area.height.saturating_sub(1)).saturating_add(1);
    (col, row)
}

fn copy_cell_for_area(area: Rect, x: u16, y: u16) -> (u16, u16) {
    // Convert global terminal coordinates to 0-based pane-local coordinates (no border offset).
    let col = x.saturating_sub(area.x).min(area.width.saturating_sub(1));
    let row = y.saturating_sub(area.y).min(area.height.saturating_sub(1));
    (row, col)
}

fn remote_scroll_wheel(app: &mut AppState, x: u16, y: u16, up: bool) {
    let (x, y) = map_client_coords(app, x, y);
    let mode_str = match &app.mode {
        Mode::Passthrough => "Passthrough",
        Mode::CopyMode => "CopyMode",
        Mode::CopySearch { .. } => "CopySearch",
        _ => "Other",
    };
    mouse_log(&format!("remote_scroll_wheel: x={} y={} up={} mode={}", x, y, up, mode_str));

    // Ignore scroll in popup mode — don't enter copy-mode (#110)
    if matches!(app.mode, Mode::PopupMode { .. }) {
        mouse_log("  -> popup mode, ignoring scroll");
        return;
    }

    // Handle scroll while already in copy mode
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        mouse_log("  -> already in copy mode, scrolling within");
        if up {
            scroll_copy_up(app, 3);
        } else {
            scroll_copy_down(app, 3);
            // Auto-exit copy mode when scrolled back to live output
            if app.copy_scroll_offset == 0 && app.copy_anchor.is_none() {
                exit_copy_mode(app);
            }
        }
        return;
    }

    // Determine target pane, switch focus, and check if child is a TUI app
    // that should receive scroll events.
    //
    // Wheel gate: alternate_screen ONLY (see pane_in_alt_screen).
    let (child_in_alt_screen, target_area_opt, sgr_btn, button_state) = {
        let win = &mut app.windows[app.active_idx];
        let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
        compute_rects(&win.root, app.last_window_area, &mut rects);

        let mut target_area: Option<Rect> = None;
        for (path, area) in &rects {
            if area.contains(ratatui::layout::Position { x, y }) {
                win.active_path = path.clone();
                target_area = Some(*area);
                break;
            }
        }
        if target_area.is_none() {
            target_area = rects
                .iter()
                .find(|(path, _)| *path == win.active_path)
                .map(|(_, area)| *area);
        }

        let alt = active_pane(&win.root, &win.active_path)
            .map_or(false, |p| pane_wheel_forward(p));
        let sgr_btn: u8 = if up { 64 } else { 65 };
        let wheel_delta: i16 = if up { 120 } else { -120 };
        let bs = ((wheel_delta as i32) << 16) as u32;
        (alt, target_area, sgr_btn, bs)
    };

    {
        let win = &app.windows[app.active_idx];
        let p = active_pane(&win.root, &win.active_path);
        mouse_log(&format!(
            "  -> wheel gate: forward={} alt_screen={} proto_owner={:?}",
            child_in_alt_screen,
            p.map_or(false, pane_in_alt_screen),
            p.and_then(|p| p.mouse_proto_owner),
        ));
    }

    if child_in_alt_screen {
        // Forward scroll to child TUI app (alternate screen = real TUI)
        mouse_log("  -> forwarding scroll to child TUI (alt screen)");
        let label = PaneLabelRow::from_options(app);
        let win = &mut app.windows[app.active_idx];
        let (col, row) = target_area_opt.map_or((0, 0), |area| label.cell_0based(area, x, y));
        let win_name = win.name.clone();
        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
            inject_mouse_combined(p, col, row, sgr_btn, true,
                button_state, mouse_inject::MOUSE_WHEELED, &win_name);
        }
    } else if up && app.scroll_enter_copy_mode {
        // Shell prompt — auto-enter copy mode and scroll up (tmux parity)
        mouse_log("  -> entering copy mode (shell scroll-up)");
        enter_copy_mode(app);
        scroll_copy_up(app, 3);
    } else if !app.scroll_enter_copy_mode {
        // scroll-enter-copy-mode off: scroll scrollback directly (#193)
        mouse_log("  -> direct scrollback (scroll-enter-copy-mode off)");
        scroll_pane_scrollback(app, 3, up);
    } else {
        mouse_log("  -> scroll-down at shell (no-op)");
    }
}

pub fn remote_scroll_up(app: &mut AppState, x: u16, y: u16) { remote_scroll_wheel(app, x, y, true); }
pub fn remote_scroll_down(app: &mut AppState, x: u16, y: u16) { remote_scroll_wheel(app, x, y, false); }

/// Handle a semantic mouse event from the client.
/// The client has already determined the target pane and computed pane-relative
/// coordinates, so no coordinate translation is needed.
pub fn handle_pane_mouse(app: &mut AppState, pane_id: usize, button: u8, col: i16, row: i16, press: bool) {
    // Find the pane by ID and focus it
    let win = &mut app.windows[app.active_idx];
    let mut found_path: Option<Vec<usize>> = None;
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    for (path, _area) in &rects {
        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, path) {
            if pid == pane_id {
                found_path = Some(path.clone());
                break;
            }
        }
    }

    let Some(path) = found_path else { return; };

    // Focus the target pane only on actual clicks (not drag/hover).
    // tmux behavior: click-to-focus, not focus-follows-mouse.
    let is_click = matches!(button, 0 | 1 | 2) && press;
    if is_click && win.active_path != path {
        win.active_path = path.clone();
        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &path) {
            crate::tree::touch_mru(&mut win.pane_mru, pid);
        }
    }

    // Handle copy mode: position cursor with pane-relative coordinates
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        // Visible pane extent, for clamping and edge detection.  The client
        // sends drag/release coordinates unclamped, so an out-of-range row
        // is the signal that the pointer crossed a pane edge.
        let (rows_v, cols_v) = {
            let w = &app.windows[app.active_idx];
            active_pane(&w.root, &w.active_path)
                .map(|p| (p.last_rows, p.last_cols))
                .unwrap_or((0, 0))
        };
        let max_r = (rows_v.saturating_sub(1)) as i16;
        let max_c = (cols_v.saturating_sub(1)) as i16;
        let r = row.clamp(0, max_r.max(0)) as u16;
        let c = col.clamp(0, max_c.max(0)) as u16;
        if button == 0 && press {
            // Left press: position cursor, clear selection
            app.copy_anchor = None;
            app.copy_pos = Some((r, c));
            app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
            app.copy_mouse_down_cell = Some((r, c));
            // A new gesture starts with nothing published; a frame that carries
            // this selection publishes its endpoint for the raw mouse release
            // to fall back on (see `sync_copy_freeze`).
            app.copy_pos_published = None;
        } else if button == 32 {
            // Left drag: extend selection, but ignore same-cell micro-jitter
            // (#199) — only while the RAW coordinates are still inside the
            // pane.  A drag past an edge clamps onto the click cell too
            // (press on the first row, drag straight up), but the pointer
            // leaving the pane is a real drag that must start the selection
            // and auto-scroll below, never jitter.
            let raw_in_range = row >= 0 && row <= max_r && col >= 0 && col <= max_c;
            if app.copy_anchor.is_none() {
                if raw_in_range && app.copy_pos == Some((r, c)) {
                    return; // same cell as click, ignore jitter
                }
                app.copy_anchor = Some(app.copy_pos.unwrap_or((r, c)));
                app.copy_anchor_scroll_offset = app.copy_scroll_offset;
                app.copy_selection_mode = crate::types::SelectionMode::Char;
            }
            app.copy_pos = Some((r, c));
            // Recorded BEFORE the edge auto-scroll below: the endpoint's own
            // view offset is what makes its screen row mean a content line
            // (see `copy_pos_scroll_offset`).
            app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
            // tmux parity (#62): dragging on/past the pane's first or last
            // row scrolls the view so the selection keeps growing into
            // scrollback; speed rises with distance past the edge.  The
            // client re-sends the last drag every 50ms while the pointer
            // dwells there, which is what makes the scroll continuous.
            // Once the view scrolls this is definitely a drag, so drop the
            // pending click cell: the release-side #199 guard compares
            // screen cells, which scrolling invalidates — a release on the
            // same screen cell would otherwise be mistaken for a click and
            // discard the selection.
            if row <= 0 {
                app.copy_mouse_down_cell = None;
                scroll_copy_up(app, 1 + ((-row) as usize / 2).min(4));
            } else if row >= max_r {
                app.copy_mouse_down_cell = None;
                scroll_copy_down(app, 1 + ((row - max_r) as usize / 2).min(4));
            }
        } else if button == 0 && !press {
            // Left release: finalize the drag.  The endpoint is NOT moved to
            // the release cell.  tmux's `window_copy_drag_release` only clears
            // the drag state; the copy-mode cursor is moved by drag
            // (`window_copy_drag_update`) and bare-motion
            // (`window_copy_move_mouse`) events alone.
            //
            // Taking the release cell yanks a character the user never saw
            // selected: the highlight is painted from `copy_pos` (the last
            // drag cell) and `exit_copy_mode` clears it in this same call,
            // before any dump can paint the release cell.  So a terminal that
            // reports the button release one cell past its last motion sample
            // — a fast flick, coalesced motion, a phone or SSH client —
            // copied a cell that was never highlighted.  Keeping the last drag
            // cell is by construction exactly what was last painted.
            //
            // The release cell is still what the #199 click guard compares
            // against (`r`/`c`), so a press/release pair within one cell stays
            // a click.  A release with no drag endpoint of its own (a press
            // that belonged to another pane, or that arrived before copy mode
            // opened, #669) may still position the cursor.
            if app.copy_anchor.is_none() {
                app.copy_pos = Some((r, c));
                app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
            }
            if let Some((dr, dc)) = app.copy_mouse_down_cell.take() {
                if (dr as i32 - r as i32).unsigned_abs() <= 1
                    && (dc as i32 - c as i32).unsigned_abs() <= 1
                {
                    app.copy_anchor = None;
                    app.copy_pos = Some((dr, dc));
                    app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
                    return;
                }
            }
            // The endpoint is the newest drag the client reported, and it is
            // not second-guessed here: only the client knows which cell it
            // painted, and a client whose last motion never made it to the
            // screen re-reports the cell it did paint immediately before this
            // release (`copy_release_repin`, client.rs).  The release itself
            // never moves the endpoint.
            //
            // Auto-yank if a real selection exists.  Compare CONTENT
            // positions (screen row minus the scroll offset it was recorded
            // at, as yank_selection does), not screen cells: edge
            // auto-scroll can park the release on the anchor's screen cell
            // while the selection spans many scrolled lines.
            if let (Some(a), Some(p)) = (app.copy_anchor, app.copy_pos) {
                let a_abs = a.0 as i64 - app.copy_anchor_scroll_offset as i64;
                let p_abs = p.0 as i64 - app.copy_pos_scroll_offset.unwrap_or(app.copy_scroll_offset) as i64;
                if (a_abs, a.1) != (p_abs, p.1) {
                    let _ = yank_selection(app);
                }
                // tmux parity (#62): ending a drag cancels copy mode and
                // returns to the live view (copy-pipe-and-cancel), even
                // when the selection came up empty — e.g. a drag past the
                // top of a pane with no scrollback, where the edge scroll
                // was a no-op and the pointer clamped back onto the
                // anchor.  An anchor here always means a real drag: plain
                // clicks were snapped back by the guards above.
                exit_copy_mode(app);
            }
        }
        return;
    }

    // Forward mouse event to PTY if pane wants it.
    //
    // Bare motion (SGR button 35, no button held) requires the child to have
    // EXPLICITLY enabled any-event tracking (pane_wants_bare_motion, DECSET
    // 1003, because 1002 only asks for motion while a button is held, #604).
    // Clicks/drags (buttons 0/1/2/32) use pane_wants_click, which avoids
    // screen-content heuristics that false-positive on a
    // filled screen with a non-shell foreground (podman/docker interactive
    // containers, discussion #349), which forwarded left/right clicks as
    // "0;x;yM0;x;ym" garbage into the container tty (comment 17754744). Real
    // mouse support (VT DECSET apps, alt-screen apps, and native crossterm
    // apps via ENABLE_MOUSE_INPUT — the #285 case) is preserved.
    let win = &mut app.windows[app.active_idx];
    let win_name = win.name.clone();
    if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
        let wants = if button == 35 { pane_wants_bare_motion(pane) } else { pane_wants_click(pane) };
        if wants {
            let button_state = match (button, press) {
                (0, true) => mouse_inject::FROM_LEFT_1ST_BUTTON_PRESSED,
                (1, true) => mouse_inject::FROM_LEFT_2ND_BUTTON_PRESSED,
                (2, true) => mouse_inject::RIGHTMOST_BUTTON_PRESSED,
                _ => 0,
            };
            let event_flags = if button == 32 || button == 35 { mouse_inject::MOUSE_MOVED } else { 0 };
            inject_mouse_combined(pane, col, row, button, press, button_state, event_flags, &win_name);
        }
    }
}

/// Hand a normal-mode client-side drag selection off to copy mode.
///
/// The client sends this when a drag at the shell prompt reaches the pane's
/// top edge — or, when the view is direct-scrolled (scroll-enter-copy-mode
/// off, #193), its bottom edge: the visible screen has run out, so the
/// selection must continue into scrollback (tmux parity — a prompt drag is
/// a copy-mode selection).  `anchor` is the cell where the drag started and
/// `cur` the current pointer cell, both pane-relative 0-based; out-of-range
/// values are clamped.  From here on the client keeps sending regular
/// `pane-mouse 32` drags, which auto-scroll at the edges (see
/// handle_pane_mouse).
pub fn copy_drag_begin(app: &mut AppState, pane_id: usize, anchor_col: i16, anchor_row: i16,
                       col: i16, row: i16, rect_sel: bool) {
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. } | Mode::PopupMode { .. }) {
        return;
    }

    // Find and focus the pane the drag started in.
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    let mut found_path: Option<Vec<usize>> = None;
    for (path, _area) in &rects {
        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, path) {
            if pid == pane_id {
                found_path = Some(path.clone());
                break;
            }
        }
    }
    let Some(path) = found_path else { return; };
    if win.active_path != path {
        win.active_path = path.clone();
        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &path) {
            crate::tree::touch_mru(&mut win.pane_mru, pid);
        }
    }
    let (rows_v, cols_v) = active_pane(&win.root, &path)
        .map(|p| (p.last_rows, p.last_cols))
        .unwrap_or((0, 0));
    if rows_v == 0 || cols_v == 0 { return; }

    // enter_copy_mode keeps a direct-scrolled view (scroll-enter-copy-mode
    // off, #193): the drag was made over the content currently on screen,
    // and copy_scroll_offset starts at the parser's live scrollback, so the
    // anchor below lands at the offset the user was looking at.
    enter_copy_mode(app);
    let max_r = (rows_v - 1) as i16;
    let max_c = (cols_v - 1) as i16;
    app.copy_anchor = Some((anchor_row.clamp(0, max_r) as u16, anchor_col.clamp(0, max_c) as u16));
    app.copy_anchor_scroll_offset = app.copy_scroll_offset;
    app.copy_selection_mode = if rect_sel {
        crate::types::SelectionMode::Rect
    } else {
        crate::types::SelectionMode::Char
    };
    app.copy_pos = Some((row.clamp(0, max_r) as u16, col.clamp(0, max_c) as u16));
    app.copy_pos_scroll_offset = Some(app.copy_scroll_offset);
    // A drag is in progress, not a click: the release must yank, never
    // snap back through the #199 click guard.
    app.copy_mouse_down_cell = None;
    // This gesture's selection has not been published to the client yet; the
    // next frame that carries it publishes the endpoint the release yanks to.
    app.copy_pos_published = None;
    // The handoff fires with the pointer at/past an edge — start scrolling
    // immediately so the selection keeps growing (the bottom edge only
    // moves when the view was scrolled back; at offset 0 it is a no-op).
    if row <= 0 {
        scroll_copy_up(app, 1 + ((-row) as usize / 2).min(4));
    } else if row >= max_r {
        scroll_copy_down(app, 1 + ((row - max_r) as usize / 2).min(4));
    }
}

/// Handle a semantic scroll event targeted at a specific pane.
///
/// `at` is the pointer's pane-relative 0-based (col, row).  It is `None` only
/// when the request came from a client too old to send it, in which case the
/// pane centre is used as before.
pub fn handle_pane_scroll(app: &mut AppState, pane_id: usize, up: bool, at: Option<(i16, i16)>) {
    // Every exit from this function is logged under PSMUX_MOUSE_DEBUG=1.  The
    // wheel has many ways to end up doing nothing (mouse off, popup open, a
    // pane the gate hands to its child, wheel-down at a live prompt), and with
    // no trace here a silent notch was indistinguishable from a lost event —
    // exactly what made #629 unfalsifiable from a user's log.  `pane-scroll`
    // is the verb the client sends whenever the pointer is inside a pane, so
    // this is the branch users actually hit; `remote_scroll_wheel` (the
    // pointer-outside-any-pane fallback) has had the same tracing all along.
    mouse_log(&format!(
        "handle_pane_scroll: pane={} up={} at={:?} mode={} mouse_enabled={}",
        pane_id, up, at,
        match &app.mode {
            Mode::Passthrough => "Passthrough",
            Mode::CopyMode => "CopyMode",
            Mode::CopySearch { .. } => "CopySearch",
            Mode::PopupMode { .. } => "PopupMode",
            _ => "Other",
        },
        app.mouse_enabled,
    ));

    // Server request dispatch already applies this gate. Keep it here too so
    // alternate/direct callers cannot scroll or enter copy mode with mouse off.
    if !app.mouse_enabled {
        mouse_log("  -> mouse off, ignoring scroll");
        return;
    }

    // Ignore scroll in popup mode (#110)
    if matches!(app.mode, Mode::PopupMode { .. }) {
        mouse_log("  -> popup mode, ignoring scroll");
        return;
    }

    // Handle scroll while already in copy mode (coordinates irrelevant)
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        mouse_log("  -> already in copy mode, scrolling within");
        if up {
            scroll_copy_up(app, 3);
        } else {
            scroll_copy_down(app, 3);
            if app.copy_scroll_offset == 0 && app.copy_anchor.is_none() {
                exit_copy_mode(app);
            }
        }
        return;
    }

    // Focus the target pane
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    for (path, _area) in &rects {
        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, path) {
            if pid == pane_id {
                win.active_path = path.clone();
                break;
            }
        }
    }

    // Wheel gate: alternate_screen ONLY — the known-good pre-3.3.5 semantics.
    // Gating on mouse_protocol_mode (what #360's check kept) routed the wheel
    // into pi's focused input box via PSReadLine's spurious mouse tracking;
    // see pane_in_alt_screen for the full rationale.
    let alt = active_pane(&win.root, &win.active_path)
        .map_or(false, |p| pane_wheel_forward(p));
    {
        let p = active_pane(&win.root, &win.active_path);
        mouse_log(&format!(
            "  -> wheel gate: forward={} alt_screen={} proto_owner={:?}",
            alt,
            p.map_or(false, pane_in_alt_screen),
            p.and_then(|p| p.mouse_proto_owner),
        ));
    }

    if alt {
        // Forward scroll to TUI app
        mouse_log("  -> forwarding scroll to child TUI (pane owns the mouse)");
        let win = &mut app.windows[app.active_idx];
        let win_name = win.name.clone();
        let sgr_btn: u8 = if up { 64 } else { 65 };
        let wheel_delta: i16 = if up { 120 } else { -120 };
        let button_state = ((wheel_delta as i32) << 16) as u32;
        // Report the wheel at the pointer's real position so the app can scroll
        // the window actually under the cursor (#570).  A TUI with its own split
        // layout (a file tree beside an editor, an embedded terminal) routes the
        // wheel by column/row, so reporting a fixed point made every notch scroll
        // whichever of its windows happened to cover that point.
        //
        // Only when the client did not send a position (older build) fall back to
        // the pane centre: some TUI frameworks (Bubble Tea) ignore events at (0,0)
        // when that is outside their scrollable viewport.
        let pane_area = rects.iter()
            .find(|(p, _)| *p == win.active_path)
            .map(|(_, a)| *a);
        let (col, row) = at.unwrap_or_else(|| {
            pane_area.map_or((5, 5), |a| ((a.width / 2) as i16, (a.height / 2) as i16))
        });
        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
            inject_mouse_combined(pane, col, row, sgr_btn, true,
                button_state, mouse_inject::MOUSE_WHEELED, &win_name);
        }
        return;
    }

    // Not mouse-aware and not on psmux's tracked alternate screen.  tmux
    // parity: a main-screen program without mouse tracking gets copy-mode
    // scrollback.  Alternate-scroll (DECSET 1007, wheel → arrow keys) only
    // ever applies to ALTERNATE-screen panes in tmux/xterm; the forward
    // branch above already covers those via wheel injection.
    //
    // Do NOT blanket-translate the wheel into arrow keys for every non-shell
    // foreground.  Interactive inline TUIs (pi, Claude Code, …) run on the
    // main screen without mouse tracking, and their focused input box reads
    // Up/Down as prompt-history navigation: the general alternate-scroll
    // branch this replaced sent 3×arrows per wheel notch, so the wheel over
    // pi cycled prompt history instead of scrolling the transcript
    // (regression 3.3.4→3.3.5+).
    //
    // The one legacy pager that genuinely needs help stays as an exact
    // allowlist. `more.com` is a DOS-heritage getch()-style reader that
    // parses no ANSI escape sequences at all, so arrow keys are a silent
    // no-op for it — proven by direct keystroke testing (Enter advances one
    // line, Space one page, arrows do nothing). It's also forward-only by
    // design (MS docs: no backward paging), so only wheel-down gets the
    // Enter-advance treatment; wheel-up falls through to copy-mode entry,
    // which already works because psmux's own scrollback buffer captured
    // everything `more` printed regardless of what `more` can rewind to.
    //
    // `non_shell_fg` is gated on a *confirmed* `Some(false)` from the
    // process-identity check in platform::process_info::foreground_is_shell
    // (the same tri-state helper Ctrl+C routing uses for issue #381/#285) —
    // never on `None` (probe failure) or `Some(true)` (confirmed shell).
    // This deliberately avoids content-based fullscreen heuristics, which
    // misclassify a normal shell whose
    // screen happens to be full (prompt at the bottom) as a TUI app, which
    // is exactly the false positive #360 fixed for copy-mode entry. Process
    // identity has no such ambiguity — a real shell binary is never
    // misreported as "not a shell". The legacy-pager exe-name check is
    // similarly precise (an exact allowlist, not a guess), so Enter is only
    // ever forwarded to the one program confirmed to want it — never to an
    // arbitrary non-shell foreground, where an unsolicited Enter could
    // submit a REPL/confirmation prompt the user didn't intend to trigger.
    let (non_shell_fg, fg_name) = active_pane_mut(&mut win.root, &win.active_path)
        .map_or((false, None), scroll_foreground_classify);
    // `get_foreground_process_name` returns the exe stem without extension
    // (e.g. "more" for more.com/more.exe), matching how `is_shell_exe`'s own
    // allowlist ("pwsh", "cmd", ...) is written — verified directly via
    // PSMUX_MOUSE_DEBUG against a live `more.com` child (fg_name=Some("more")).
    let is_legacy_pager = non_shell_fg && fg_name.as_deref()
        .map_or(false, |n| n.eq_ignore_ascii_case("more"));

    if is_legacy_pager && !up {
        // `more.com`: Enter is its "advance one line" key.
        mouse_log("  -> legacy pager (more.com), sending Enter x3");
        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
            for _ in 0..3 {
                crate::input::write_key_seq(pane, b"\r");
            }
            let _ = pane.writer.flush();
        }
    } else if up && app.scroll_enter_copy_mode {
        // Everything else (shell prompt, inline TUI like pi, `more.com`
        // wheel-up, or a probe failure) — enter copy mode and scroll
        // psmux's own buffer, exactly like tmux.
        mouse_log("  -> entering copy mode (shell scroll-up)");
        enter_copy_mode(app);
        scroll_copy_up(app, 3);
    } else if !app.scroll_enter_copy_mode {
        // scroll-enter-copy-mode off: scroll scrollback directly (#193)
        mouse_log("  -> direct scrollback (scroll-enter-copy-mode off)");
        scroll_pane_scrollback(app, 3, up);
    } else {
        mouse_log("  -> scroll-down at live prompt (no-op, tmux parity)");
    }
}

/// Set split sizes at a given tree path during border drag.
pub fn handle_split_set_sizes(app: &mut AppState, path: &[usize], sizes: &[u16]) {
    let win = &mut app.windows[app.active_idx];
    let mut cur: &mut Node = &mut win.root;
    for &idx in path.iter() {
        match cur {
            Node::Split { children, .. } => {
                if idx < children.len() {
                    cur = &mut children[idx];
                } else {
                    return;
                }
            }
            Node::Leaf(_) => return,
        }
    }
    if let Node::Split { sizes: node_sizes, children, .. } = cur {
        if sizes.len() == children.len() && sizes.len() == node_sizes.len() {
            *node_sizes = sizes.to_vec();
        }
    }
}

/// Finalize a border resize: apply PTY resizes to match the new layout.
pub fn handle_split_resize_done(app: &mut AppState) {
    resize_all_panes(app);
}

pub fn swap_pane(app: &mut AppState, dir: FocusDir) -> bool {
    let win = &mut app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, app.last_window_area, &mut rects);
    
    let mut active_idx = None;
    for (i, (path, _)) in rects.iter().enumerate() { 
        if *path == win.active_path { active_idx = Some(i); break; } 
    }
    let Some(ai) = active_idx else { return false; };
    let (_, arect) = &rects[ai];
    
    // Collect pane IDs for MRU-based tie-breaking (issue #70)
    let pane_ids: Vec<usize> = rects.iter().map(|(path, _)| {
        crate::tree::get_active_pane_id(&win.root, path).unwrap_or(usize::MAX)
    }).collect();
    // Pick the pane to swap with.  tmux defines `swap-pane -U`/`-D` by pane
    // *index* order, not spatial geometry: -U swaps with the previous pane and
    // -D with the next pane in the window's pane list, wrapping at the ends
    // (cmd-swap-pane.c uses TAILQ_PREV / TAILQ_NEXT with wrap to LAST / FIRST).
    // `rects` is already in pane-index order (DFS leaf order, same as
    // pane_index_in_window), so prev/next is simply ai-1 / ai+1 with wrap.
    // `-L`/`-R` have no tmux swap-pane equivalent (they are psmux extensions),
    // so they keep the spatial neighbour search. (issue #400)
    let n = rects.len();
    let target = match dir {
        FocusDir::Up   => Some(if ai == 0 { n - 1 } else { ai - 1 }),
        FocusDir::Down => Some(if ai + 1 >= n { 0 } else { ai + 1 }),
        _ => crate::input::find_best_pane_in_direction(&rects, ai, arect, dir, &pane_ids, &win.pane_mru)
            .or_else(|| crate::input::find_wrap_target(&rects, ai, arect, dir, &pane_ids, &win.pane_mru)),
    }.filter(|&ni| ni != ai); // single-pane window: nothing to swap with
    let mut swapped = false;
    if let Some(ni) = target {
        let active_path = rects[ai].0.clone();
        let target_path = rects[ni].0.clone();
        // Actually exchange the two panes in the layout tree (keeping the split
        // sizes) instead of merely moving focus.  This is the real swap-pane
        // behaviour expected from tmux.
        if crate::tree::swap_nodes(&mut win.root, &active_path, &target_path) {
            // Focus follows the pane that was just moved into the new slot.
            win.active_path = target_path;
            if let Some(focused_id) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                crate::tree::touch_mru(&mut win.pane_mru, focused_id);
            }
            swapped = true;
        }
    }
    // Resize the moved panes' PTYs to fit their new slots (tmux re-lays-out
    // after a swap).  Without this the program keeps its old terminal size.
    if swapped { crate::tree::resize_all_panes(app); }
    swapped
}

/// Swap the active pane with the pane at an explicit tree `path`
/// (used by `swap-pane -t <target>`).  Geometry is preserved; focus follows
/// the moved pane to its new slot.
pub fn swap_pane_with_path(app: &mut AppState, target_path: Vec<usize>) -> bool {
    let swapped = {
        let win = &mut app.windows[app.active_idx];
        let active_path = win.active_path.clone();
        if active_path == target_path { false }
        else {
            if crate::tree::swap_nodes(&mut win.root, &active_path, &target_path) {
                win.active_path = target_path;
                if let Some(focused_id) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                    crate::tree::touch_mru(&mut win.pane_mru, focused_id);
                }
                true
            } else { false }
        }
    };
    // Resize moved panes to fit their new slots (see swap_pane).
    if swapped { crate::tree::resize_all_panes(app); }
    swapped
}

/// Swap two explicit panes named by their tree paths (`swap-pane -s <src>
/// -t <dst>`, issue #442).  Neither pane needs to be the active one.  Geometry
/// is preserved (the layout nodes exchange slots).  Focus follows tmux
/// `cmd-swap-pane.c`: without `-d` the `-t` (dst) pane becomes active — after
/// the swap it occupies the src slot, so focus lands on `src_path`.  With `-d`
/// the previously active pane keeps focus, following it to its new slot.
pub fn swap_pane_between(app: &mut AppState, src_path: Vec<usize>, dst_path: Vec<usize>, detach: bool) -> bool {
    let swapped = {
        let win = &mut app.windows[app.active_idx];
        if src_path == dst_path { false }
        else {
            // Remember the focused pane id so `-d` can keep focus on it even if
            // it was one of the two panes being swapped.
            let active_id = crate::tree::get_active_pane_id(&win.root, &win.active_path);
            if crate::tree::swap_nodes(&mut win.root, &src_path, &dst_path) {
                if detach {
                    if let Some(aid) = active_id {
                        if let Some(p) = crate::tree::find_path_by_id(&win.root, aid) {
                            win.active_path = p;
                        }
                    }
                } else {
                    // tmux default: the -t pane becomes active; it now sits in
                    // the src slot after the exchange.
                    win.active_path = src_path.clone();
                }
                if let Some(fid) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                    crate::tree::touch_mru(&mut win.pane_mru, fid);
                }
                true
            } else { false }
        }
    };
    // Resize moved panes to fit their new slots (see swap_pane).
    if swapped { crate::tree::resize_all_panes(app); }
    swapped
}

/// Resolve a tmux-style position token (e.g. `{top-right}`) to the path of the
/// pane occupying that corner/edge of the current window.  Layout-independent:
/// always finds whatever pane currently sits there.
pub fn pane_path_at_position(app: &AppState, token: &str) -> Option<Vec<usize>> {
    if app.windows.is_empty() { return None; }
    let area = app.last_window_area;
    let win = &app.windows[app.active_idx];
    let mut rects: Vec<(Vec<usize>, Rect)> = Vec::new();
    compute_rects(&win.root, area, &mut rects);
    resolve_position_token(token, area, &rects)
}

/// Map a tmux-style position token to the path of the pane covering that
/// corner/edge point.  Pure geometry, separated out so it can be unit-tested.
pub fn resolve_position_token(token: &str, area: Rect, rects: &[(Vec<usize>, Rect)]) -> Option<Vec<usize>> {
    if area.width == 0 || area.height == 0 { return None; }
    let x0 = area.x;
    let y0 = area.y;
    let xmax = area.x + area.width - 1;
    let ymax = area.y + area.height - 1;
    let xmid = area.x + area.width / 2;
    let ymid = area.y + area.height / 2;
    let (px, py) = match token {
        "{top-left}"     => (x0, y0),
        "{top-right}"    => (xmax, y0),
        "{bottom-left}"  => (x0, ymax),
        "{bottom-right}" => (xmax, ymax),
        "{top}"          => (xmid, y0),
        "{bottom}"       => (xmid, ymax),
        "{left}"         => (x0, ymid),
        "{right}"        => (xmax, ymid),
        _ => return None,
    };
    rects.iter()
        .find(|(_, r)| px >= r.x && px < r.x + r.width && py >= r.y && py < r.y + r.height)
        .map(|(p, _)| p.clone())
}

#[cfg(test)]
mod position_token_tests {
    use super::resolve_position_token;
    use ratatui::layout::Rect;
    fn layout() -> (Rect, Vec<(Vec<usize>, Rect)>) {
        // ABTOP top-left, SMALL bottom-left, BIG right (mirrors the user's panel).
        let area = Rect { x: 0, y: 0, width: 160, height: 40 };
        let rects = vec![
            (vec![0, 0], Rect { x: 0,  y: 0,  width: 79, height: 19 }),
            (vec![0, 1], Rect { x: 0,  y: 20, width: 79, height: 20 }),
            (vec![1],    Rect { x: 80, y: 0,  width: 80, height: 40 }),
        ];
        (area, rects)
    }
    #[test]
    fn top_right_finds_big_pane() {
        let (area, rects) = layout();
        assert_eq!(resolve_position_token("{top-right}", area, &rects), Some(vec![1]));
        assert_eq!(resolve_position_token("{bottom-right}", area, &rects), Some(vec![1]));
        assert_eq!(resolve_position_token("{right}", area, &rects), Some(vec![1]));
    }
    #[test]
    fn corners_left() {
        let (area, rects) = layout();
        assert_eq!(resolve_position_token("{top-left}", area, &rects), Some(vec![0, 0]));
        assert_eq!(resolve_position_token("{bottom-left}", area, &rects), Some(vec![0, 1]));
    }
    #[test]
    fn unknown_token_is_none() {
        let (area, rects) = layout();
        assert_eq!(resolve_position_token("{active}", area, &rects), None);
    }
}

#[cfg(test)]
mod window_ops_tests {
    use super::swap_pane_with_path;
    use crate::proxy_pane::create_proxy_pane;
    use crate::types::{AppState, LayoutKind, Mode, Node, Window};
    use ratatui::layout::Rect;
    use std::net::{TcpListener, TcpStream};

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind listener");
        let addr = listener.local_addr().expect("listener addr");
        let accept_thr = std::thread::spawn(move || listener.accept().expect("accept").0);
        let client = TcpStream::connect(addr).expect("connect");
        let server = accept_thr.join().expect("join accept thread");
        (client, server)
    }

    fn proxy_pane(id: usize, rows: u16, cols: u16) -> crate::types::Pane {
        let (reader, _peer1) = tcp_pair();
        let (writer, _peer2) = tcp_pair();
        create_proxy_pane(
            reader,
            writer,
            "127.0.0.1:1".to_string(),
            "test-key".to_string(),
            "test-session".to_string(),
            id as u64,
            None,
            format!("pane-{}", id),
            rows,
            cols,
            id,
            None,
        ).expect("create proxy pane")
    }

    fn make_window_with_two_panes(left_id: usize, right_id: usize) -> Window {
        Window {
            root: Node::Split {
                kind: LayoutKind::Horizontal,
                sizes: vec![1, 1],
                children: vec![Node::Leaf(proxy_pane(left_id, 10, 5)), Node::Leaf(proxy_pane(right_id, 10, 5))],
            },
            active_path: vec![0],
            name: "w0".to_string(),
            id: 0,
            area: Rect::new(0, 0, 10, 5),
            window_size: None,
            window_options: Default::default(),
            activity_flag: false,
            bell_flag: false,
            silence_flag: false,
            last_output_time: std::time::Instant::now(),
            last_seen_version: 0,
            manual_rename: false,
            layout_index: 0,
            pane_mru: vec![right_id, left_id],
            zoom_saved: None,
            linked_from: None,
            floating: Vec::new(),
            floating_focus: None,
        }
    }

    fn make_scrollback_app(mouse_enabled: bool) -> AppState {
        let pane = proxy_pane(41, 8, 40);
        let history = (0..80)
            .map(|line| format!("history-{line}\r\n"))
            .collect::<String>();
        pane.term
            .lock()
            .expect("term lock")
            .process(history.as_bytes());

        let mut app = AppState::new("mouse-scrollback".to_string());
        app.mouse_enabled = mouse_enabled;
        app.scroll_enter_copy_mode = true;
        app.last_window_area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 8,
        };
        app.windows.push(Window {
            root: Node::Leaf(pane),
            active_path: vec![],
            name: "w0".to_string(),
            id: 0,
            area: app.client_area,
            window_size: None,
            window_options: Default::default(),
            activity_flag: false,
            bell_flag: false,
            silence_flag: false,
            last_output_time: std::time::Instant::now(),
            last_seen_version: 0,
            manual_rename: false,
            layout_index: 0,
            pane_mru: vec![41],
            zoom_saved: None,
            linked_from: None,
            floating: Vec::new(),
            floating_focus: None,
        });
        app
    }

    #[test]
    fn swap_with_path_updates_mru_for_focused_pane_after_swap() {
        let mut app = AppState::new("swap-mru".to_string());
        app.last_window_area = Rect { x: 0, y: 0, width: 10, height: 10 };
        app.windows.push(make_window_with_two_panes(11, 22));
        app.active_idx = 0;

        let swapped = swap_pane_with_path(&mut app, vec![1]);
        assert!(swapped, "swap should succeed");
        assert_eq!(app.windows[0].active_path, vec![1], "focus should follow moved active pane");
        assert_eq!(app.windows[0].pane_mru.first().copied(), Some(11), "MRU should be the focused pane id after swap");
    }

    #[test]
    fn wheel_up_enters_copy_mode_and_repeated_wheel_scrolls_further() {
        let mut app = make_scrollback_app(true);

        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::CopyMode));
        let first_offset = app.copy_scroll_offset;
        assert!(
            first_offset > 0,
            "first wheel report must move into history"
        );

        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(
            app.copy_scroll_offset > first_offset,
            "repeated wheel reports must continue scrolling"
        );
    }

    #[test]
    fn mouse_off_ignores_wheel_without_entering_copy_mode() {
        let mut app = make_scrollback_app(false);

        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::Passthrough));
        assert_eq!(app.copy_scroll_offset, 0);
    }

    #[test]
    fn copy_drag_at_top_edge_scrolls_into_history() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None); // wheel into copy mode
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;

        // Press at row 2, then drag to the pane's top row: the view scrolls.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 0, true);
        assert_eq!(app.copy_anchor, Some((2, 5)));
        assert_eq!(app.copy_pos, Some((0, 5)));
        let after_edge = app.copy_scroll_offset;
        assert!(after_edge > base, "drag on the top row must scroll up");

        // Re-sending the same drag (the client's 50ms dwell repeat) keeps scrolling.
        super::handle_pane_mouse(&mut app, 41, 32, 5, 0, true);
        assert!(app.copy_scroll_offset > after_edge, "dwell repeat must keep scrolling");

        // Distance past the edge scrolls faster than the top row itself.
        let o = app.copy_scroll_offset;
        super::handle_pane_mouse(&mut app, 41, 32, 5, -8, true);
        assert!(app.copy_scroll_offset >= o + 5, "far past the edge must scroll faster");
        assert_eq!(app.copy_pos, Some((0, 5)), "cursor clamps to the top row");
    }

    #[test]
    fn copy_drag_at_bottom_edge_scrolls_back_down() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;
        assert!(base >= 6, "two wheel reports must scroll six lines");

        // Press at row 3, drag to the last visible row (7 of 8): scrolls down.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 3, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        assert!(app.copy_scroll_offset < base, "drag on the last row must scroll down");

        // Past the bottom edge: cursor clamps, scroll continues faster.
        let o = app.copy_scroll_offset;
        super::handle_pane_mouse(&mut app, 41, 32, 5, 12, true);
        assert!(app.copy_scroll_offset < o);
        assert_eq!(app.copy_pos, Some((7, 5)), "cursor clamps to the last row");
    }

    #[test]
    fn copy_drag_straight_up_from_top_row_scrolls() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None); // wheel into copy mode
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;

        // Press ON the top row, then drag straight up out of the pane with
        // no horizontal movement: the clamped position lands back on the
        // click cell, but crossing the edge is a real drag, not #199
        // jitter — the selection must start and the view must scroll.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 0, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, -1, true);
        assert_eq!(app.copy_anchor, Some((0, 5)), "edge drag must start the selection");
        assert!(app.copy_scroll_offset > base, "edge drag must scroll despite the same clamped cell");

        // Dwell repeats at the same raw position keep scrolling.
        let o = app.copy_scroll_offset;
        super::handle_pane_mouse(&mut app, 41, 32, 5, -1, true);
        assert!(app.copy_scroll_offset > o, "dwell repeat must keep scrolling");
    }

    #[test]
    fn copy_drag_straight_down_from_bottom_row_scrolls() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;
        assert!(base >= 6, "two wheel reports must scroll six lines");

        // Press ON the last visible row (7 of 8), drag straight down past
        // the pane: same clamped cell, but the view must scroll back down.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 7, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 8, true);
        assert_eq!(app.copy_anchor, Some((7, 5)), "edge drag must start the selection");
        assert!(app.copy_scroll_offset < base, "edge drag must scroll back down");
    }

    #[test]
    fn empty_edge_drag_release_exits_copy_mode() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None); // wheel into copy mode
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;

        // A drag journey that ends exactly where it started: up to the top
        // edge (scrolls one line), down to the bottom edge (scrolls back),
        // then back onto the anchor's content position.  Nothing is selected,
        // but the drag still ends — tmux's copy-pipe-and-cancel cancels
        // copy mode either way, it never leaves the user stranded there.
        //
        // The return to the anchor is a DRAG, not the release: the endpoint
        // follows drag/motion events only (tmux's window_copy_drag_release
        // just clears the drag state), so the release cell cannot stand in
        // for the motion that brought the pointer back.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 0, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        assert_eq!(app.copy_scroll_offset, base, "the two edge scrolls must cancel out");
        super::handle_pane_mouse(&mut app, 41, 32, 5, 2, true); // back onto the anchor
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, false);
        assert!(app.paste_buffers.is_empty(), "an empty selection must not yank");
        assert!(matches!(app.mode, Mode::Passthrough), "an empty drag must still exit copy mode");
        assert_eq!(app.copy_scroll_offset, 0);
    }

    #[test]
    fn legacy_mouse_release_after_edge_scroll_yanks_and_exits() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None); // wheel into copy mode
        assert!(matches!(app.mode, Mode::CopyMode));
        let base = app.copy_scroll_offset;

        // Legacy screen-coordinate protocol (mouse-down/drag/up x y):
        // press at row 2, drag onto the top row — the view edge-scrolls.
        super::remote_mouse_down(&mut app, 5, 2);
        super::remote_mouse_drag(&mut app, 5, 0);
        assert!(app.copy_scroll_offset > base, "drag on the top row must scroll up");

        // Release on the SAME screen cell as the press: the scroll made
        // this a real multi-line selection, so it must yank (content
        // positions), not be discarded by a screen-cell anchor == pos
        // comparison.
        super::remote_mouse_up(&mut app, 5, 2);
        assert!(!app.paste_buffers.is_empty(), "release must yank the selection");
        assert!(app.paste_buffers[0].contains('\n'), "yank must span the scrolled lines");
        // A mouse yank cancels copy mode and returns to live view (#62),
        // same as the pane-mouse release path.
        assert!(matches!(app.mode, Mode::Passthrough), "mouse yank must exit copy mode");
        assert_eq!(app.copy_scroll_offset, 0);
    }

    #[test]
    fn client_entered_copy_mode_drag_anchors_at_press_and_yanks() {
        // mouse-drag-enter-copy-mode: the client sends `copy-enter`, replays
        // the press at the button-down cell, then drags. The selection must
        // anchor at the press cell (not at the pane cursor) and yank on
        // release, then leave copy mode like every other mouse drag (#62).
        let mut app = make_scrollback_app(true);
        crate::copy_mode::enter_copy_mode(&mut app);
        assert!(matches!(app.mode, Mode::CopyMode));

        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, true); // press at (col 5, row 2)
        super::handle_pane_mouse(&mut app, 41, 32, 9, 3, true); // drag to (col 9, row 3)
        assert_eq!(app.copy_anchor, Some((2, 5)), "anchor must be the press cell");

        super::handle_pane_mouse(&mut app, 41, 0, 9, 3, false); // release
        assert_eq!(app.paste_buffers.len(), 1, "release must yank the selection");
        assert!(matches!(app.mode, Mode::Passthrough), "a mouse yank exits copy mode");
    }

    /// Yank a single-row drag on the pane-mouse protocol, releasing on
    /// `release_col` after dragging only as far as column 9.
    fn yank_drag_released_at(release_col: i16) -> String {
        let mut app = make_scrollback_app(true);
        crate::copy_mode::enter_copy_mode(&mut app);
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, true); // press at (col 5, row 2)
        super::handle_pane_mouse(&mut app, 41, 32, 9, 2, true); // last drag: (col 9, row 2)
        super::handle_pane_mouse(&mut app, 41, 0, release_col, 2, false);
        assert!(
            matches!(app.mode, Mode::Passthrough),
            "the release must still finalize the drag"
        );
        app.paste_buffers
            .first()
            .cloned()
            .expect("a real drag must yank on release")
    }

    #[test]
    fn a_release_past_the_last_drag_must_not_extend_the_selection() {
        // The copied text used to come out one character longer than the
        // highlighted selection.  The endpoint is painted from `copy_pos`, the
        // last DRAG cell, and `exit_copy_mode` clears it in the same call as
        // the yank — so a release cell that trailed the last motion sample was
        // never highlighted, yet still landed in the buffer.  tmux's
        // `window_copy_drag_release` only clears the drag state (parity: the
        // endpoint follows drag and bare-motion events, never the release).
        let dragged_to = yank_drag_released_at(9);
        let released_one_past = yank_drag_released_at(10);
        assert_eq!(
            released_one_past, dragged_to,
            "a release cell the drag never reported must not be selected"
        );
        assert_eq!(dragged_to, "ry-75", "the drag must select exactly cols 5..=9");

        // The other direction: a release that pulls back is not a retraction
        // either (col 7 is two cells off the press, so this stays a drag and
        // not a #199 click).  Retractions arrive as drag events, which do
        // move the endpoint.
        assert_eq!(
            yank_drag_released_at(7), dragged_to,
            "the release may not shrink the selection either"
        );
    }

    #[test]
    fn a_legacy_mouse_up_past_the_last_drag_must_not_extend_the_selection() {
        // Same bug on the screen-coordinate protocol (mouse-down/drag/up x y),
        // which converts the release against the pane's content rect itself.
        fn yank_up_at(release_x: u16) -> String {
            let mut app = make_scrollback_app(true);
            crate::copy_mode::enter_copy_mode(&mut app);
            super::remote_mouse_down(&mut app, 5, 2);
            super::remote_mouse_drag(&mut app, 9, 2);
            super::remote_mouse_up(&mut app, release_x, 2);
            app.paste_buffers
                .first()
                .cloned()
                .expect("a real drag must yank on release")
        }
        assert_eq!(
            yank_up_at(10),
            yank_up_at(9),
            "a release past the last drag must not add a character"
        );
    }

    /// The client re-reports the endpoint it painted just before a release
    /// (`copy_release_repin`, client.rs).  A motion can reach the server — and
    /// be written into a frame — and still never reach the screen, because the
    /// release is handled before that frame is drawn: the highlight ends on
    /// the previous cell while the server's frame says otherwise.  Only the
    /// client can tell the two apart, so its re-pin is what the yank follows.
    ///
    /// This is the reported bug: highlight ends on `.` (col 4 here), the
    /// buffer ends on the `9` after it (col 5).
    #[test]
    fn a_release_after_a_client_repin_yanks_the_painted_cell() {
        let mut app = make_scrollback_app(true);
        crate::copy_mode::enter_copy_mode(&mut app);
        super::handle_pane_mouse(&mut app, 41, 0, 0, 2, true); // press at col 0
        super::handle_pane_mouse(&mut app, 41, 32, 4, 2, true); // drag to col 4
        let _ = crate::layout::dump_layout_json(&mut app).expect("a frame"); // painted: cols 0..=4
        super::handle_pane_mouse(&mut app, 41, 32, 5, 2, true); // the slip
        let _ = crate::layout::dump_layout_json(&mut app).expect("a frame"); // written, never drawn
        super::handle_pane_mouse(&mut app, 41, 32, 4, 2, true); // re-pin: what the client painted
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, false); // release
        assert_eq!(
            app.paste_buffers.first().map(String::as_str),
            Some("histo"),
            "the copy must match the highlight, not the cell the frame carried"
        );
    }

    /// Control for the test above: with no re-pin the newest drag stands, even
    /// when no frame carried it — which is exactly why the client has to send
    /// one.  Pinned so the reason for `copy_release_repin` cannot be dropped
    /// silently on this side.
    #[test]
    fn a_release_without_a_repin_follows_the_last_drag() {
        let mut app = make_scrollback_app(true);
        crate::copy_mode::enter_copy_mode(&mut app);
        super::handle_pane_mouse(&mut app, 41, 0, 0, 2, true);
        super::handle_pane_mouse(&mut app, 41, 32, 4, 2, true);
        let _ = crate::layout::dump_layout_json(&mut app).expect("a frame");
        super::handle_pane_mouse(&mut app, 41, 32, 5, 2, true);
        let _ = crate::layout::dump_layout_json(&mut app).expect("a frame");
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, false);
        assert_eq!(
            app.paste_buffers.first().map(String::as_str),
            Some("histor"),
            "without a re-pin the newest drag stands"
        );
    }

    /// The raw protocol has no client that can say what it painted, so the
    /// same-batch slip is dropped against the published endpoint there.
    #[test]
    fn a_legacy_mouse_up_slip_no_frame_carried_must_not_widen_the_yank() {
        fn yank_after(slip_frame: bool) -> String {
            let mut app = make_scrollback_app(true);
            crate::copy_mode::enter_copy_mode(&mut app);
            super::remote_mouse_down(&mut app, 5, 2);
            super::remote_mouse_drag(&mut app, 9, 2);
            let _ = crate::layout::dump_layout_json(&mut app).expect("a frame"); // col 9 published
            super::remote_mouse_drag(&mut app, 10, 2); // the slip
            if slip_frame {
                let _ = crate::layout::dump_layout_json(&mut app).expect("a frame");
            }
            super::remote_mouse_up(&mut app, 10, 2);
            app.paste_buffers
                .first()
                .cloned()
                .expect("a real drag must yank on release")
        }
        assert_ne!(
            yank_after(false),
            yank_after(true),
            "a slip no frame carried must not add a character on the raw protocol"
        );
    }

    #[test]
    fn emacs_mode_bare_g_does_not_jump_to_history_top() {
        // tmux binds history-top to `g` in copy-mode-vi only; the emacs table
        // uses M-<. An ungated `g` meant that any stray `g` — typing a prompt
        // into a pane that was still in copy mode, e.g. after a thumb scroll on
        // a phone — threw the view to the very top until the user pressed Esc.
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::CopyMode));
        let before = app.copy_scroll_offset;
        app.mode_keys = "emacs".to_string();
        crate::input::send_text_to_active(&mut app, "g").expect("send-text g");
        assert_eq!(app.copy_scroll_offset, before, "emacs 'g' must not move the view");
        assert!(matches!(app.mode, Mode::CopyMode), "emacs 'g' must stay in copy mode");
    }

    #[test]
    fn vi_mode_bare_g_still_jumps_to_history_top() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        app.mode_keys = "vi".to_string();
        crate::input::send_text_to_active(&mut app, "g").expect("send-text g");
        assert!(
            app.copy_scroll_offset > 20,
            "vi 'g' must still reach history-top (got {})",
            app.copy_scroll_offset
        );
    }

    #[test]
    fn emacs_mode_alt_less_than_jumps_to_history_top() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        app.mode_keys = "emacs".to_string();
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('<'),
            crossterm::event::KeyModifiers::ALT,
        );
        crate::input::handle_key(&mut app, key).expect("handle_key M-<");
        assert!(
            app.copy_scroll_offset > 20,
            "M-< must reach history-top in emacs mode (got {})",
            app.copy_scroll_offset
        );
    }

    #[test]
    fn named_alt_less_than_reaches_history_top_in_both_tables() {
        // Alt keys reach copy mode as NAMED keys (`send-key M-<`), not through
        // handle_key's char arm, so the named dispatcher needs its own arm.
        // Without it `send-keys M-<` was silently swallowed while `send-keys
        // M-v` (page up) worked, measured on a 380 line scrollback.
        for mode_keys in ["emacs", "vi"] {
            let mut app = make_scrollback_app(true);
            super::handle_pane_scroll(&mut app, 41, true, None);
            app.mode_keys = mode_keys.to_string();
            let before = app.copy_scroll_offset;
            crate::input::send_key_to_active(&mut app, "M-<").expect("send-key M-<");
            assert!(
                app.copy_scroll_offset > before,
                "M-< must reach history-top under mode-keys {mode_keys} (got {})",
                app.copy_scroll_offset
            );
            crate::input::send_key_to_active(&mut app, "M->").expect("send-key M->");
            assert_eq!(
                app.copy_scroll_offset, 0,
                "M-> must reach history-bottom under mode-keys {mode_keys}"
            );
        }
    }

    #[test]
    fn enter_copy_mode_preserves_direct_scrolled_view() {
        let mut app = make_scrollback_app(true);
        // scroll-enter-copy-mode off (#193): the wheel scrolls the pane's
        // parser directly, without entering copy mode.
        app.scroll_enter_copy_mode = false;
        super::handle_pane_scroll(&mut app, 41, true, None);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::Passthrough), "direct scroll must not enter copy mode");
        assert_eq!(app.copy_scroll_offset, 0);

        // Keyboard entry (prefix-[) over the scrolled view: copy mode must
        // anchor its offset at the view on screen, not reset to 0 while
        // the parser stays scrolled.
        crate::copy_mode::enter_copy_mode(&mut app);
        assert!(matches!(app.mode, Mode::CopyMode));
        assert_eq!(app.copy_scroll_offset, 6, "copy mode must keep the direct-scrolled view");
    }

    #[test]
    fn copy_drag_release_after_edge_scroll_yanks_and_exits() {
        let mut app = make_scrollback_app(true);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::CopyMode));

        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 0, true); // scrolls the view
        // Release on the SAME screen cell as the press: scrolling made this
        // a real multi-line drag, so it must yank rather than be snapped
        // back to a click — the #199 click guard works on the release cell
        // against the press cell, and the scroll invalidated that pair.
        super::handle_pane_mouse(&mut app, 41, 0, 5, 2, false);
        assert!(!app.paste_buffers.is_empty(), "release must yank the selection");
        // Char-mode selection from the press cell (row 2, col 5)@offset 3 to
        // the last DRAG cell (row 0, col 5)@offset 3 — the drag's OWN offset,
        // recorded before its edge scroll moved the view to offset 4.  Content
        // rows -3..-1: history-70/71 and the live row history-72, sliced at
        // col 5 on both ends.  The release cell is not the endpoint, so it
        // neither extends nor shortens this.  This is also exactly the range
        // the frame paints (see `a_direction_mismatched_drag_paints_and_yanks_
        // the_same_range` for the pairing half of that invariant).
        assert_eq!(app.paste_buffers[0], "ry-70\nhistory-71\nhistor",
            "yank must span from the anchor to the last drag cell");
        // A mouse yank cancels copy mode and returns to live view (#62).
        assert!(matches!(app.mode, Mode::Passthrough), "mouse yank must exit copy mode");
        assert_eq!(app.copy_scroll_offset, 0);
    }

    /// A drag whose row and column directions differ must paint and yank the
    /// SAME range.  The frame pairs each column with its own row (tmux: the
    /// selection runs from the anchor cell to the endpoint cell); independent
    /// min/max on rows and columns used to paint one range and copy another,
    /// which is the reported "sometimes the selection and the copy disagree"
    /// — it happened in every drag direction, on any selection whose endpoint
    /// sat on a different row.
    #[test]
    fn a_direction_mismatched_drag_paints_and_yanks_the_same_range() {
        let mut app = make_scrollback_app(true);
        crate::copy_mode::enter_copy_mode(&mut app);
        super::handle_pane_mouse(&mut app, 41, 0, 6, 4, true);  // press: row 4, col 6
        super::handle_pane_mouse(&mut app, 41, 32, 2, 1, true); // drag : row 1, col 2

        // What the client is told to paint: the top row keeps the ENDPOINT's
        // column (2) and the bottom row the anchor's (6).
        let frame = crate::layout::dump_layout_json(&mut app).expect("a frame");
        let num = |key: &str| -> i64 {
            let pat = format!("\"{key}\"");
            let at = frame.find(&pat).unwrap_or_else(|| panic!("{key} in {frame}"));
            let rest = &frame[at + pat.len()..];
            let colon = rest.find(':').expect("colon");
            rest[colon + 1..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .expect("a number")
        };
        assert_eq!(num("sel_start_row"), 1, "painted selection starts on the drag row");
        assert_eq!(num("sel_start_col"), 2, "...with the DRAG's column");
        assert_eq!(num("sel_end_row"), 4, "painted selection ends on the press row");
        assert_eq!(num("sel_end_col"), 6, "...with the PRESS's column");

        super::handle_pane_mouse(&mut app, 41, 0, 2, 1, false); // release on the drag cell
        assert_eq!(
            app.paste_buffers.first().map(String::as_str),
            Some("story-74\nhistory-75\nhistory-76\nhistory"),
            "the copy must be the range the frame above points at (the frame's\n             rows are client-relative, the yank's are parser-relative, so this\n             pins the text)"
        );
    }

    #[test]
    fn copy_drag_begin_from_direct_scroll_preserves_offset_and_scrolls_down() {
        let mut app = make_scrollback_app(true);
        // scroll-enter-copy-mode off (#193): the wheel scrolls the pane's
        // parser directly, without entering copy mode.
        app.scroll_enter_copy_mode = false;
        super::handle_pane_scroll(&mut app, 41, true, None);
        super::handle_pane_scroll(&mut app, 41, true, None);
        assert!(matches!(app.mode, Mode::Passthrough), "direct scroll must not enter copy mode");
        assert_eq!(app.copy_scroll_offset, 0);

        // A drag selection over the scrolled view reached the bottom row
        // (7 of 8): the client hands off with the bottom-edge position.
        super::copy_drag_begin(&mut app, 41, 5, 3, 5, 7, false);
        assert!(matches!(app.mode, Mode::CopyMode));
        // The direct-scrolled view (offset 6) is preserved and anchors the
        // selection; the bottom edge starts scrolling toward the live view.
        assert_eq!(app.copy_anchor, Some((3, 5)));
        assert_eq!(app.copy_anchor_scroll_offset, 6, "anchor must keep the direct-scroll offset");
        assert_eq!(app.copy_scroll_offset, 5, "bottom-edge handoff must scroll down one line");

        // Dwell drags on the bottom row keep scrolling toward the live view.
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        assert_eq!(app.copy_scroll_offset, 4);

        // At the live bottom the scroll clamps instead of wrapping.
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        super::handle_pane_mouse(&mut app, 41, 32, 5, 7, true);
        assert_eq!(app.copy_scroll_offset, 0);
    }

    #[test]
    fn copy_drag_begin_hands_prompt_drag_off_to_copy_mode() {
        let mut app = make_scrollback_app(true);
        assert!(matches!(app.mode, Mode::Passthrough));

        // A prompt drag that started at (col 10, row 5) crossed the top
        // edge (row -1): the client hands the selection off to copy mode.
        super::copy_drag_begin(&mut app, 41, 10, 5, 10, -1, false);
        assert!(matches!(app.mode, Mode::CopyMode));
        assert_eq!(app.copy_anchor, Some((5, 10)));
        assert_eq!(app.copy_pos, Some((0, 10)), "current cell clamps to the top row");
        assert!(app.copy_scroll_offset > 0, "handoff at the top edge starts scrolling");

        // Follow-up dwell drags keep scrolling.
        let o = app.copy_scroll_offset;
        super::handle_pane_mouse(&mut app, 41, 32, 10, -1, true);
        assert!(app.copy_scroll_offset > o);

        // A second handoff while already in copy mode is ignored.
        let anchor = app.copy_anchor;
        super::copy_drag_begin(&mut app, 41, 0, 0, 0, 0, false);
        assert_eq!(app.copy_anchor, anchor);
    }
}

pub fn resize_pane_vertical(app: &mut AppState, amount: i16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == LayoutKind::Vertical {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    if idx + 1 < sizes.len() {
                        let new_size = (sizes[idx] as i16 + amount).max(1) as u16;
                        let diff = new_size as i16 - sizes[idx] as i16;
                        sizes[idx] = new_size;
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        // tmux parity (#81): last child has no bottom border.
                        // Resize the previous sibling with the same amount so
                        // the border moves in the arrow direction.
                        let new_size = (sizes[idx - 1] as i16 + amount).max(1) as u16;
                        let diff = new_size as i16 - sizes[idx - 1] as i16;
                        sizes[idx - 1] = new_size;
                        sizes[idx] = (sizes[idx] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

pub fn resize_pane_horizontal(app: &mut AppState, amount: i16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == LayoutKind::Horizontal {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    if idx + 1 < sizes.len() {
                        let new_size = (sizes[idx] as i16 + amount).max(1) as u16;
                        let diff = new_size as i16 - sizes[idx] as i16;
                        sizes[idx] = new_size;
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        // tmux parity (#81): last child has no right border.
                        // Resize the previous sibling with the same amount so
                        // the border moves in the arrow direction.
                        let new_size = (sizes[idx - 1] as i16 + amount).max(1) as u16;
                        let diff = new_size as i16 - sizes[idx - 1] as i16;
                        sizes[idx - 1] = new_size;
                        sizes[idx] = (sizes[idx] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

/// Absolute resize: set the active pane's share to an exact size.
/// axis is "x" (width/horizontal) or "y" (height/vertical).
pub fn resize_pane_absolute(app: &mut AppState, axis: &str, target: u16) {
    let win = &mut app.windows[app.active_idx];
    if win.active_path.is_empty() { return; }
    let target_kind = if axis == "x" { LayoutKind::Horizontal } else { LayoutKind::Vertical };
    for depth in (0..win.active_path.len()).rev() {
        let parent_path = win.active_path[..depth].to_vec();
        if let Some(Node::Split { kind, sizes, .. }) = get_split_mut(&mut win.root, &parent_path) {
            if *kind == target_kind {
                let idx = win.active_path[depth];
                if idx < sizes.len() {
                    let old = sizes[idx];
                    let new = target.max(1);
                    let diff = new as i16 - old as i16;
                    sizes[idx] = new;
                    // Absorb the difference from a neighbour
                    if idx + 1 < sizes.len() {
                        sizes[idx + 1] = (sizes[idx + 1] as i16 - diff).max(1) as u16;
                    } else if idx > 0 {
                        sizes[idx - 1] = (sizes[idx - 1] as i16 - diff).max(1) as u16;
                    }
                }
                return;
            }
        }
    }
}

/// `rotate-window`: move every pane one slot along the window's pane order.
///
/// tmux (`cmd-rotate-window.c`) never touches the layout tree.  It re-points
/// each pane at the NEXT pane's layout cell and then gives the pane that
/// cell's geometry, PTY included:
///
/// ```text
///     wp->layout_cell = wp2->layout_cell;
///     wp->xoff = wp2->xoff; wp->yoff = wp2->yoff;
///     window_pane_resize(wp, wp2->sx, wp2->sy);
/// ```
///
/// Two things follow, and psmux used to get both wrong (#645):
///
///  1. Only the OCCUPANTS move.  Every cell keeps its position and its size,
///     so the window's shape is identical before and after.  psmux rotated the
///     root split's direct CHILDREN instead, which on a nested layout carried
///     whole subtrees into slots sized for something else (a 34/4/10 column of
///     three panes came back as 39/8/1).  Rotating the leaves, which is what a
///     cell permutation is once the cells live in the tree, leaves every
///     split's `sizes` and the tree shape alone.
///
///  2. Each moved pane is RESIZED to the cell it landed in.  psmux left every
///     pane at the size of the cell it had just left, so `pane_height` and
///     `pane_width` disagreed with `pane_top`/`pane_bottom`/`window_layout`,
///     `split-window` refused a visually tall pane as "too small", and the
///     child console kept the old row count.  `resize_all_panes` is the same
///     path `resize-pane` takes, which is why `resize-pane -U 0` healed it.
///
/// `upward` is tmux's `-U`, which is also its default: the pane in the first
/// cell moves to the last cell and everyone else moves up one.  `-D` is the
/// reverse.  Focus stays on the same CELL, matching tmux, which re-points
/// `w->active` at the pane that moved into the active pane's cell.
pub fn rotate_panes(app: &mut AppState, upward: bool) {
    if app.active_idx >= app.windows.len() { return; }
    let rotated = {
        let win = &mut app.windows[app.active_idx];
        let mut leaves: Vec<(usize, Vec<usize>)> = Vec::new();
        crate::tree::collect_leaf_paths_pub(&win.root, &mut Vec::new(), &mut leaves);
        if leaves.len() < 2 {
            false
        } else {
            // Swapping two LEAVES never changes the tree's shape, so the paths
            // collected up front stay valid for the whole chain of swaps.
            let paths: Vec<Vec<usize>> = leaves.into_iter().map(|(_, p)| p).collect();
            let n = paths.len();
            if upward {
                // [A,B,C] -> [B,C,A]: cell 0 takes the second pane.
                for i in 0..n - 1 {
                    crate::tree::swap_nodes(&mut win.root, &paths[i], &paths[i + 1]);
                }
            } else {
                // [A,B,C] -> [C,A,B]: cell 0 takes the last pane.
                for i in (0..n - 1).rev() {
                    crate::tree::swap_nodes(&mut win.root, &paths[i], &paths[i + 1]);
                }
            }
            true
        }
    };
    // tmux's window_pane_resize, deferred to one pass over the window: every
    // pane takes the size of the cell it now occupies, and its PTY with it.
    if rotated { crate::tree::resize_all_panes(app); }
}

pub fn break_pane_to_window(app: &mut AppState) {
    let src_idx = app.active_idx;
    let src_path = app.windows[src_idx].active_path.clone();
    
    // Extract the active pane from the current window using tree operations
    let src_root = std::mem::replace(&mut app.windows[src_idx].root,
        Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] });
    let (remaining, extracted) = crate::tree::extract_node(src_root, &src_path);
    
    if let Some(pane_node) = extracted {
        let src_empty = remaining.is_none();
        if let Some(rem) = remaining {
            app.windows[src_idx].root = rem;
            app.windows[src_idx].active_path = crate::tree::first_leaf_path(&app.windows[src_idx].root);
        }
        
        // Determine the window name from the pane
        let win_name = match &pane_node {
            Node::Leaf(p) => p.title.clone(),
            _ => format!("win {}", app.windows.len() + 1),
        };
        
        // Create new window containing the extracted pane
        let initial_mru = crate::tree::collect_pane_ids(&pane_node);
        app.windows.push(Window {
            root: pane_node,
            active_path: vec![],
            name: win_name,
            id: app.next_win_id,
            area: app.client_area,
            window_size: None,
            window_options: Default::default(),
            activity_flag: false,
            bell_flag: false,
            silence_flag: false,
            last_output_time: std::time::Instant::now(),
            last_seen_version: 0,
            manual_rename: false,
            layout_index: 0,
            pane_mru: initial_mru,
            zoom_saved: None,
            linked_from: None,
            floating: Vec::new(),
            floating_focus: None,
        });
        app.next_win_id += 1;
        app.on_window_appended();

        if src_empty {
            app.windows.remove(src_idx);
            app.on_window_removed(src_idx);
        }

        // Switch to the new window
        app.active_idx = app.windows.len() - 1;
    } else {
        // Extraction failed — restore
        if let Some(rem) = remaining {
            app.windows[src_idx].root = rem;
        }
    }
}

/// `clear-history`: drop the active pane's scrollback.
///
/// tmux (cmd-capture-pane.c:418) resets every mode on the pane first and then
/// calls `grid_clear_history(wp->base.grid)`, i.e. the LIVE grid, with copy
/// mode gone. That ordering is load bearing: while copy mode is up `pane.term`
/// is the frozen snapshot, so clearing it would wipe the screen the user is
/// reading and leave the live scrollback untouched.
pub fn clear_active_pane_history(app: &mut AppState) {
    if matches!(app.mode, Mode::CopyMode | Mode::CopySearch { .. }) {
        exit_copy_mode(app);
    }
    let history_limit = app.history_limit;
    let win = &mut app.windows[app.active_idx];
    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
        p.leave_copy_snapshot();
        if let Ok(mut parser) = p.term.lock() {
            *parser = vt100::Parser::new(p.last_rows, p.last_cols, history_limit);
        }
    }
}

pub fn respawn_active_pane(app: &mut AppState, pty_system_ref: Option<&dyn portable_pty::PtySystem>, workdir: Option<&str>, kill: bool, command: Option<&str>, empty: bool) -> io::Result<()> {
    // tmux semantics: without -k, respawn only works on dead panes.
    // With -k, kill the running process first and respawn.
    {
        let win = &app.windows[app.active_idx];
        if let Some(pane) = crate::tree::active_pane(&win.root, &win.active_path) {
            if !pane.dead && !kill {
                // tmux spawn.c: "pane <session>:<window>.<pane> still active".
                // This is a ROUTINE, user-caused refusal, not a server fault:
                // the caller must get it back as a per-request error (the
                // `?` that used to carry it out of `run_server` destroyed the
                // whole session on a mistyped respawn-pane).
                let pane_idx = crate::tree::pane_index_in_window(&win.root, &win.active_path)
                    .unwrap_or(0);
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "pane {}:{}.{} still active",
                        app.session_name,
                        app.win_display_index(app.active_idx),
                        pane_idx
                    ),
                ));
            }
        }
    }
    // If -k and pane is alive, kill the child process first
    if kill {
        let win = &mut app.windows[app.active_idx];
        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
            if !pane.dead {
                crate::platform::process_kill::kill_process_tree(&mut pane.child);
                pane.dead = true;
            }
        }
    }

    // -E: respawn as an EMPTY pane (no command). Replace the pane in place with
    // a childless empty pane, keeping its id/title/size, and return.
    if empty {
        let win = &mut app.windows[app.active_idx];
        if let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) {
            let (r, c, id, title) = (pane.last_rows, pane.last_cols, pane.id, pane.title.clone());
            if let Some(mut ep) = crate::popup::create_empty_pane(r.max(1), c.max(1), id) {
                ep.title = title;
                *pane = ep;
            }
        }
        return Ok(());
    }

    // Reuse provided PTY system or create one as fallback
    let owned_pty;
    let pty_system: &dyn portable_pty::PtySystem = if let Some(ps) = pty_system_ref {
        ps
    } else {
        owned_pty = native_pty_system();
        &*owned_pty
    };
    // Expand format variables like #{pane_current_path} at spawn time (#111).
    // Must happen before the mutable borrow of app.windows below.
    let expanded_shell = crate::format::expand_format(&app.default_shell, &app);

    let win = &mut app.windows[app.active_idx];
    let Some(pane) = active_pane_mut(&mut win.root, &win.active_path) else { return Ok(()); };
    let pane_id = pane.id;
    // tmux spawn.c: a respawn with no command of its own reuses the arguments
    // already stored on the pane ("Replace the stored arguments if there are
    // new ones. If not, the existing ones will be used (they will only exist
    // for respawn)"), so `respawn-pane -k` on a pane created with a command
    // re-runs THAT command, and only a pane created with the default shell
    // comes back as a shell. Issue #580: this also keeps
    // `#{pane_start_command}` honest across a respawn.
    let new_start_command = command.map(|c| crate::pane::start_command_raw(Some(c)));
    let inherited = if command.is_none() && !pane.start_command.is_empty() {
        Some(pane.start_command.clone())
    } else {
        None
    };
    let command = command.or(inherited.as_deref());

    let size = PtySize { rows: pane.last_rows, cols: pane.last_cols, pixel_width: 0, pixel_height: 0 };
    let pair = pty_system.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    // Issue #399: honor an explicit `-- <command>` (e.g. Claude Code agent-teams
    // respawning a pane with the teammate launch command). Without a command,
    // fall back to the configured default shell (original behavior).
    let mut shell_cmd = if command.is_some() {
        crate::pane::build_command(command, app.env_shim, app.allow_predictions)
    } else if !expanded_shell.is_empty() {
        build_default_shell(&expanded_shell, app.env_shim, app.allow_predictions)
    } else {
        detect_shell()
    };
    set_tmux_env(&mut shell_cmd, pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    crate::pane::set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    crate::pane::apply_user_environment(&mut shell_cmd, &app.environment);
    if let Some(dir) = workdir {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        let expanded = dir.replace("~/", &format!("{}/", home))
            .replace("~\\", &format!("{}\\", home));
        shell_cmd.cwd(std::path::Path::new(&expanded));
    }
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    // Close the slave handle immediately – required for ConPTY.
    drop(pair.slave);
    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, app.history_limit)));
    let term_reader = term.clone();
    let reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(crate::pane::CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();

    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);
    crate::pane::spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), pane_id, child_pid);
    pane.output_ring = output_ring;

    let mut pty_writer = crate::pane::spawn_pane_write_queue(pair.master.take_writer().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    crate::pane::conpty_preemptive_dsr_response(&mut *pty_writer);

    pane.master = pair.master;
    pane.writer = pty_writer;
    pane.child = child;
    pane.term = term;
    pane.live_term = None;
    pane.data_version = data_version;
    pane.cursor_shape = cursor_shape;
    pane.bell_pending = bell_pending;
    pane.cpr_pending = cpr_pending;
    pane.color_query_pending = color_query_pending;
    // Every other spawn site stores the fresh child pid; leaving this None
    // blanked #{pane_pid} and sent #{pane_current_command} to its
    // foreground-window fallback for the rest of the pane's life.
    pane.child_pid = child_pid;
    // #580: a respawn that carried its own command replaces what the pane
    // reports as `#{pane_start_command}`; a bare respawn keeps the recorded
    // one (it just re-ran it above), and a bare respawn of a default-shell
    // pane leaves it empty. Same rule as tmux spawn.c.
    if let Some(c) = new_start_command {
        pane.start_command = c;
    }
    pane.vt_bridge_cache = None;
    pane.vti_mode_cache = None;
    pane.mouse_input_cache = None;
    pane.scroll_fg_cache = None;
    // Fresh ConPTY, fresh input state machine: the win32-input-mode latch that
    // eats a bare ESC (#588) does not carry over to the respawned child.
    pane.win32_input_latched = false;
    pane.dead = false;
    pane.spawned_at = Some(std::time::Instant::now());

    Ok(())
}

/// Respawn a fresh default shell into a SPECIFIC pane (by window index + tree
/// path) that crashed shortly after spawn. Mirrors `respawn_active_pane`'s spawn
/// core but always uses the default shell and targets an arbitrary pane. Used by
/// the opt-in `@heal-crashed-panes` self-heal for issue #450, where a pwsh whose
/// PSReadLine is not the active reader FailFasts on its first ConPTY read right
/// after a warm-pane transplant, leaving a broken/empty window.
pub fn heal_respawn_pane(
    app: &mut AppState,
    pty_system_ref: &dyn portable_pty::PtySystem,
    win_idx: usize,
    path: &Vec<usize>,
) -> io::Result<()> {
    // Expand format vars (e.g. #{pane_current_path}) before the mutable borrow.
    let expanded_shell = crate::format::expand_format(&app.default_shell, &app);

    let Some(win) = app.windows.get_mut(win_idx) else { return Ok(()); };
    let Some(pane) = active_pane_mut(&mut win.root, path) else { return Ok(()); };
    let pane_id = pane.id;
    let size = PtySize { rows: pane.last_rows.max(1), cols: pane.last_cols.max(1), pixel_width: 0, pixel_height: 0 };

    let pair = pty_system_ref.openpty(size).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("openpty error: {e}")))?;
    let mut shell_cmd = if !expanded_shell.is_empty() {
        build_default_shell(&expanded_shell, app.env_shim, app.allow_predictions)
    } else {
        detect_shell()
    };
    set_tmux_env(&mut shell_cmd, pane_id, app.control_port, app.socket_name.as_deref(), &app.session_name, app.claude_code_fix_tty, app.claude_code_force_interactive);
    crate::pane::set_host_colors_env(&mut shell_cmd, app.host_colors.as_ref());
    crate::pane::apply_user_environment(&mut shell_cmd, &app.environment);
    let child = pair.slave.spawn_command(shell_cmd).map_err(|e| io::Error::new(io::ErrorKind::Other, format!("spawn shell error: {e}")))?;
    drop(pair.slave);
    let child_pid = crate::platform::mouse_inject::get_child_pid(&*child);

    let term: Arc<Mutex<vt100::Parser>> = Arc::new(Mutex::new(vt100::Parser::new(size.rows, size.cols, app.history_limit)));
    let term_reader = term.clone();
    let reader = pair.master.try_clone_reader().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("clone reader error: {e}")))?;
    let data_version = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let dv_writer = data_version.clone();
    let cursor_shape = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(crate::pane::CURSOR_SHAPE_UNSET));
    let cs_writer = cursor_shape.clone();
    let bell_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let bell_writer = bell_pending.clone();
    let cpr_pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cpr_writer = cpr_pending.clone();
    let color_query_pending = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let cq_writer = color_query_pending.clone();
    let output_ring = std::sync::Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
    crate::pane::spawn_reader_thread(reader, term_reader, dv_writer, cs_writer, bell_writer, cpr_writer, cq_writer, output_ring.clone(), pane_id, child_pid);

    let mut pty_writer = crate::pane::spawn_pane_write_queue(pair.master.take_writer().map_err(|e| io::Error::new(io::ErrorKind::Other, format!("take writer error: {e}")))?);
    crate::pane::conpty_preemptive_dsr_response(&mut *pty_writer);

    // Re-acquire the pane and swap in the fresh shell.
    let Some(win) = app.windows.get_mut(win_idx) else { return Ok(()); };
    let Some(pane) = active_pane_mut(&mut win.root, path) else { return Ok(()); };
    pane.master = pair.master;
    pane.writer = pty_writer;
    pane.child = child;
    pane.term = term;
    pane.live_term = None;
    pane.data_version = data_version;
    pane.cursor_shape = cursor_shape;
    pane.bell_pending = bell_pending;
    pane.cpr_pending = cpr_pending;
    pane.color_query_pending = color_query_pending;
    pane.output_ring = output_ring;
    pane.child_pid = child_pid;
    pane.vt_bridge_cache = None;
    pane.vti_mode_cache = None;
    pane.mouse_input_cache = None;
    pane.scroll_fg_cache = None;
    // Fresh ConPTY, fresh input state machine: the win32-input-mode latch that
    // eats a bare ESC (#588) does not carry over to the respawned child.
    pane.win32_input_latched = false;
    pane.dead = false;
    pane.spawned_at = Some(std::time::Instant::now());
    Ok(())
}

#[cfg(test)]
#[path = "../tests-rs/test_issue81_resize_direction.rs"]
mod test_issue81_resize_direction;

#[cfg(test)]
#[path = "../tests-rs/test_issue400_swap_pane_index_order.rs"]
mod test_issue400_swap_pane_index_order;

#[cfg(test)]
#[path = "../tests-rs/test_issue442_swap_pane_source.rs"]
mod test_issue442_swap_pane_source;

#[cfg(test)]
#[path = "../tests-rs/test_discussion349_podman_motion_leak.rs"]
mod test_discussion349_podman_motion_leak;

#[cfg(test)]
#[path = "../tests-rs/test_issue604_wsl_nvim_mouse.rs"]
mod test_issue604_wsl_nvim_mouse;

#[cfg(test)]
#[path = "../tests-rs/test_issue597_legacy_mouse_bypass.rs"]
mod test_issue597_legacy_mouse_bypass;

#[cfg(test)]
#[path = "../tests-rs/test_issue613_wheel_gate_durability.rs"]
mod test_issue613_wheel_gate_durability;

#[cfg(test)]
#[path = "../tests-rs/test_issue621_wheel_stdin_block.rs"]
mod test_issue621_wheel_stdin_block;

#[cfg(test)]
#[path = "../tests-rs/test_issue623_record_reader_gate.rs"]
mod test_issue623_record_reader_gate;

#[cfg(test)]
#[path = "../tests-rs/test_issue629_ssh_vt_wheel.rs"]
mod test_issue629_ssh_vt_wheel;

#[cfg(test)]
#[path = "../tests-rs/test_respawn_pane_refusal_survives.rs"]
mod test_respawn_pane_refusal_survives;

#[cfg(test)]
#[path = "../tests-rs/test_issue645_rotate_geometry.rs"]
mod test_issue645_rotate_geometry;

#[cfg(test)]
#[path = "../tests-rs/test_issue657_wheel_nonshell_full_screen.rs"]
mod test_issue657_wheel_nonshell_full_screen;

#[cfg(test)]
#[path = "../tests-rs/test_issue669_border_status_mouse_rows.rs"]
mod test_issue669_border_status_mouse_rows;
