use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;
use std::collections::{HashMap, HashSet, VecDeque};

use crossterm::event::{KeyCode, KeyModifiers};
use portable_pty::MasterPty;
use ratatui::prelude::Rect;
use chrono::Local;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Git provenance captured at compile time by `build.rs`. These let a binary
/// report the exact commit it was built from, so a build compiled from an
/// arbitrary checkout is fully identifiable. Every value falls back to
/// `"unknown"` / `"false"` when the crate is built outside a git checkout.
pub const GIT_HASH: &str = env!("PSMUX_GIT_HASH");
pub const GIT_HASH_FULL: &str = env!("PSMUX_GIT_HASH_FULL");
pub const GIT_DATE: &str = env!("PSMUX_GIT_DATE");
pub const GIT_DIRTY: &str = env!("PSMUX_GIT_DIRTY");

/// Human-readable build provenance line, e.g.
///
///   `psmux 3.3.7 (a1b2c3d 2026-07-20)`
///   `psmux 3.3.7 (a1b2c3d 2026-07-20, dirty)`   ← built from a modified tree
///   `psmux 3.3.7 (unknown commit)`               ← built without a git checkout
pub fn build_version_string() -> String {
    if GIT_HASH == "unknown" {
        return format!("psmux {VERSION} (unknown commit)");
    }
    let dirty_suffix = if GIT_DIRTY == "true" { ", dirty" } else { "" };
    if GIT_DATE == "unknown" {
        format!("psmux {VERSION} ({GIT_HASH}{dirty_suffix})")
    } else {
        format!("psmux {VERSION} ({GIT_HASH} {GIT_DATE}{dirty_suffix})")
    }
}

/// Notifications emitted to control mode clients (tmux wire-compatible).
#[derive(Clone, Debug)]
pub enum ControlNotification {
    Output { pane_id: usize, data: String },
    WindowAdd { window_id: usize },
    WindowClose { window_id: usize },
    WindowRenamed { window_id: usize, name: String },
    WindowPaneChanged { window_id: usize, pane_id: usize },
    LayoutChange { window_id: usize, layout: String },
    SessionChanged { session_id: usize, name: String },
    SessionRenamed { name: String },
    SessionWindowChanged { session_id: usize, window_id: usize },
    SessionsChanged,
    PaneModeChanged { pane_id: usize },
    ClientDetached { client: String },
    Continue { pane_id: usize },
    Pause { pane_id: usize },
    /// Extended output with age information (when pause-after is active).
    ExtendedOutput { pane_id: usize, age_ms: u64, data: String },
    /// Subscription value changed notification.
    SubscriptionChanged {
        name: String,
        session_id: usize,
        window_id: usize,
        window_index: usize,
        pane_id: usize,
        value: String,
    },
    Exit { reason: Option<String> },
    PasteBufferChanged { name: String },
    PasteBufferDeleted { name: String },
    ClientSessionChanged { client: String, session_id: usize, name: String },
    Message { text: String },
}

/// Per-connection control mode client state.
pub struct ControlClient {
    pub client_id: u64,
    pub cmd_counter: u64,
    pub echo_enabled: bool,
    pub notification_tx: mpsc::SyncSender<ControlNotification>,
    pub paused_panes: HashSet<usize>,
    /// `refresh-client -B name:what:format` subscriptions.
    /// Key = subscription name, Value = (target, format_string).
    pub subscriptions: HashMap<String, (String, String)>,
    /// Last expanded value for each subscription (for change detection).
    pub subscription_values: HashMap<String, String>,
    /// Last time each subscription was checked (rate limit: 1/s per sub).
    pub subscription_last_check: HashMap<String, Instant>,
    /// `refresh-client -f pause-after=N`: pause output if client falls behind by N seconds.
    pub pause_after_secs: Option<u64>,
    /// Panes whose output is currently paused due to pause-after threshold.
    pub output_paused_panes: HashSet<usize>,
    /// Timestamp of last output sent per pane (for pause-after age tracking).
    pub pane_last_output: HashMap<usize, Instant>,
    /// Default viewport reported by `refresh-client -C`.
    pub size: Option<(u16, u16)>,
    /// Per-window viewport overrides reported as `refresh-client -C @id:WxH`.
    pub window_sizes: HashMap<usize, (u16, u16)>,
}

/// Per-client metadata stored in the server's client registry.
/// Tracks every attached PERSISTENT and CONTROL client.
#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub id: u64,
    pub width: u16,
    pub height: u16,
    pub connected_at: std::time::Instant,
    pub last_activity: std::time::Instant,
    /// Synthetic TTY name for display (e.g. "/dev/pts/1")
    pub tty_name: String,
    /// True for CONTROL/CONTROL_NOECHO clients
    pub is_control: bool,
    /// The session THIS client was in before it switched here, if any.
    ///
    /// psmux runs one server per session, so a switch tears the client off one
    /// server and re-attaches it to another; only the client process spans both
    /// and knows the pair. It reports the session it left on the attach
    /// handshake and it is recorded here. Before this existed, `switch-client
    /// -l` consulted a single data-dir-global `last_session` file that every
    /// attach overwrote, so the only value that could survive its
    /// "not the current session" filter was one written by a DIFFERENT client,
    /// and `-l` relocated this client into a session it had never visited
    /// (issue #566). `None` means this client has not switched yet, which is
    /// the honest answer rather than somebody else's history.
    pub last_session: Option<String>,
}

/// A pane's standing authorization to receive wheel reports (issue #613).
///
/// tmux keeps this on `struct screen` as `s->mode & ALL_MOUSE_MODES`: it is set
/// by the application's own DECSET (input.c:2053), cleared by its DECRST
/// (input.c:1959) or by `screen_reinit` when the pane respawns (screen.c:115),
/// and there is no third party who could revoke it because there is no console.
///
/// psmux has a console in the way, so it records WHO earned the authorization
/// and keeps it for as long as that process is alive inside the pane.  A child
/// rewriting the console mode word cannot revoke what it never granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelAuth {
    /// The pane's foreground leaf pid at the moment a mouse signal was first
    /// observed.  Liveness of THIS pid is what keeps the latch.
    pub owner_pid: u32,
    /// `(checked_at, still_alive)`, same 2 second TTL as the other
    /// mouse-inject detectors, so a wheel burst costs one process walk.
    pub alive_cache: Option<(Instant, bool)>,
}

pub struct Pane {
    pub master: Box<dyn MasterPty>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub child: Box<dyn portable_pty::Child>,
    pub term: Arc<Mutex<vt100::Parser>>,
    /// While copy mode displays this pane, `term` holds a frozen *snapshot* of
    /// the screen and this holds the live parser, which the PTY reader keeps
    /// feeding through its own `Arc` (see `Pane::enter_copy_snapshot`).
    ///
    /// Copy mode on a snapshot is what tmux does (`window_copy_init` copies the
    /// pane's grid).  Without it the frozen view sits on the live grid, so rows
    /// entering scrollback churn underneath the offset the user is anchored to;
    /// once those rows are evicted the view is stranded on the oldest retained
    /// line — psmux reproduced as `#{scroll_position} == #{history_size}` while
    /// pi redrew, stuck until Esc.
    pub live_term: Option<Arc<Mutex<vt100::Parser>>>,
    pub last_rows: u16,
    pub last_cols: u16,
    pub id: usize,
    pub title: String,
    /// When true, `infer_title_from_prompt` will not overwrite the title.
    /// Set by `select-pane -T` (explicit title). Cleared by `select-pane -T ""`.
    pub title_locked: bool,
    /// Cached child process PID for Windows console mouse injection.
    /// Lazily extracted on first mouse event.
    pub child_pid: Option<u32>,
    /// Monotonic counter incremented by the PTY reader thread each time new
    /// output is processed.  Checked by the server to know when the screen
    /// has actually changed (avoids serialising stale frames).
    pub data_version: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Timestamp of the last auto-rename foreground-process check (throttled to ~1/s).
    pub last_title_check: Instant,
    /// Timestamp of the last infer_title_from_prompt call in layout serialisation (throttled to ~2/s).
    pub last_infer_title: Instant,
    /// True when the child process has exited but remain-on-exit keeps the pane visible.
    pub dead: bool,
    /// Timestamp of the last printable keystroke routed via the INTERACTIVE
    /// text-input route (`handle_key -> forward_key_to_active`); `None` until
    /// the first one. NOT updated by the injected route (`send-keys` /
    /// `send-paste` / `send-text`). Exposed read-only as the
    /// `#{pane_last_text_input}` format variable. Lives on the pane, so it's
    /// freed with it (no separate lifecycle / file).
    pub last_text_input: Option<Instant>,
    /// The last NON-text key routed via the INTERACTIVE input route
    /// (`handle_key -> forward_key_to_active`): its canonical bind-key name
    /// (`Escape`, `Enter`, `Up`, `F9`, `C-c`, `M-a`, ...) + the `Instant` it
    /// arrived; `None` until the first one. Same route contract as
    /// `last_text_input` (NOT updated by the injected route). The text vs
    /// non-text split is `is_text_input_key`. Exposed read-only as
    /// `#{pane_last_special_key}` / `#{pane_last_special_key_ms}`.
    pub last_special_key: Option<(Instant, String)>,
    /// Cached VT bridge detection result (for mouse injection).
    /// Updated on first mouse event and refreshed every 2 seconds.
    pub vt_bridge_cache: Option<(Instant, bool)>,
    /// Cached ENABLE_VIRTUAL_TERMINAL_INPUT query result (for mouse injection).
    /// When true, the child's console input has VTI set, meaning VT mouse
    /// sequences can be delivered.  Refreshed every 2 seconds.
    pub vti_mode_cache: Option<(Instant, bool)>,
    /// Cached console INPUT MODE WORD of the pane's child (for the mouse
    /// injection heuristics).  `None` inside the tuple means the probe itself
    /// failed; the whole field being `None` means it has not run yet.
    /// Refreshed every 2 seconds.
    ///
    /// The whole word is cached rather than one bit because the two questions
    /// asked of it need different parts of it: `ENABLE_MOUSE_INPUT` alone says
    /// the console can deliver MOUSE_EVENT records (it is set in the inherited
    /// Windows default, so it does NOT mean the child asked), while the raw
    /// mode shape says the child deliberately configured itself to read them.
    pub mouse_input_cache: Option<(Instant, Option<u32>)>,
    /// True once psmux has written a WIN32 INPUT MODE key sequence
    /// (`ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`) into this pane's ConPTY input
    /// pipe — today only `send-keys C-<letter>`, which needs that form so the
    /// child sees a real `VK + LEFT_CTRL_PRESSED` record (issue #305).
    ///
    /// It has to be remembered because writing one is a ONE-WAY latch on the
    /// other side: conhost's input state machine concludes the terminal speaks
    /// win32 input mode and, from then on, stops dispatching a DANGLING `ESC`
    /// at the end of a write as the Escape key — it holds it as the possible
    /// start of a longer sequence instead.  A bare `0x1b` therefore never
    /// reaches the pane application again (issue #588: one `send-keys ... C-m`
    /// and Escape stopped working in bash, in nvim, in WSL).  Measured under a
    /// bare pseudoconsole: `1b` alone arrives as `key=Escape` before the win32
    /// sequence and is swallowed after it, while the same key written AS a
    /// win32 sequence arrives correctly either way.
    ///
    /// So once this is set, `input::write_pane_input` encodes a lone Escape in
    /// win32 form too, which is the only thing conhost still accepts.  It
    /// belongs to the pane because the latch belongs to that one ConPTY: a new
    /// window is unaffected, and `respawn_active_pane` clears it with the rest
    /// of the per-ConPTY caches.
    pub win32_input_latched: bool,
    /// Cached foreground-process classification for the scroll-wheel
    /// alternate-scroll decision (issue #277): `(timestamp, is_shell,
    /// foreground_exe_name)`. `is_shell` mirrors
    /// `platform::process_info::foreground_is_shell`'s tri-state contract
    /// (only a confirmed non-shell foreground enables alternate-scroll);
    /// `foreground_exe_name` is used to special-case legacy DOS-heritage
    /// pagers (`more.com`) that don't consume arrow keys. Refreshed every
    /// 2 seconds, same TTL as the other mouse-inject detectors above.
    pub scroll_fg_cache: Option<(Instant, bool, Option<String>)>,
    /// Wheel-forward attribution for the pane's mouse protocol (#548
    /// follow-up): `(mode, app_owned)` for the currently active DECSET
    /// 1000/1002/1003 tracking, `None` while no protocol is on.
    /// `app_owned` is sampled ONCE per protocol transition, at enablement
    /// time, from `foreground_is_shell`: PSReadLine enables tracking
    /// spuriously while the shell owns the prompt (`app_owned = false`,
    /// wheel enters copy mode like tmux over a plain pane), while a real
    /// main-screen mouse consumer (Copilot CLI, the #570 echo child)
    /// enables it after taking the foreground (`app_owned = true`, wheel
    /// is forwarded — tmux `mouse_any_flag` parity).  Updated by
    /// `window_ops::update_mouse_proto_owner` on the server data tick.
    pub mouse_proto_owner: Option<(vt100::MouseProtocolMode, bool)>,
    /// Pane-owned wheel authorization latch (issue #613).
    ///
    /// Both #598 signals — `mouse_proto_owner` and the live
    /// `ENABLE_MOUSE_INPUT` probe — resolve to the same console input mode
    /// word, and that word belongs to the CONSOLE, not to the application.
    /// Any descendant that enters raw mode assigns it wholesale (libuv writes
    /// `ENABLE_WINDOW_INPUT | ENABLE_VIRTUAL_TERMINAL_INPUT` and restores
    /// nothing), conhost reports the loss upstream as `ESC[?1003;1006l`, and
    /// the pane's wheel goes silent for good even though its application never
    /// changed its mind.
    ///
    /// This latch is psmux's stand-in for tmux's `s->mode & ALL_MOUSE_MODES`,
    /// which lives on the pane's own screen and no other process can reach.
    /// It is earned by a confirmed non-shell foreground, anchored to that
    /// process's pid, and dropped when it exits.  See
    /// `window_ops::latch_wheel_auth`.
    pub wheel_auth: Option<WheelAuth>,
    /// Last cursor shape requested by the child process via DECSCUSR (`\x1b[N q`).
    /// 0 = no override (use PSMUX_CURSOR_STYLE default), 1-6 = DECSCUSR values.
    pub cursor_shape: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// Set by the PTY reader thread when a BEL character (\x07) is detected.
    /// Consumed by the server loop to set the window's bell_flag.
    pub bell_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by the PTY reader thread when ESC[6n (Cursor Position Request) is
    /// detected in the child's output.  Consumed by the server loop, which
    /// then injects ESC[row;colR into the pane's PTY input.  This handles
    /// the case where pwsh re-issues the CPR after lock/unlock — the single
    /// preemptive write at spawn time is no longer in the pipe at that point.
    pub cpr_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Issue #473: bitmask of terminal color queries detected by the PTY
    /// reader thread in the child's output.  Bits 0-15 = OSC 4;<i>;? palette
    /// queries, bit 16 = OSC 10;? (foreground), bit 17 = OSC 11;? (background),
    /// bit 18 = CSI ?996n (light/dark scheme).  Consumed by the server loop,
    /// which injects the corresponding color responses so pane applications
    /// (GitHub Copilot CLI, vim, etc.) can detect the terminal palette.
    pub color_query_pending: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Per-pane copy mode state (tmux-style pane-local copy mode).
    /// Some(_) when this pane is in copy mode, None otherwise.
    pub copy_state: Option<CopyModeState>,
    /// Per-pane style string (set via `select-pane -P "bg=...,fg=..."`).
    /// Matches tmux's `window-style` / `window-active-style` pane option.
    /// Stored for API compatibility; ConPTY rendering doesn't support
    /// per-pane fg/bg tinting so this is not rendered yet.
    pub pane_style: Option<String>,
    /// Pane-scoped options set via `set-option -p` (issue #580, the Claude
    /// Code teammate backend's scope).  Currently wired: `remain-on-exit`
    /// (`on`/`off`/`failed`, consulted by `prune_exited` and overriding the
    /// session-global).  Unwired pane options are rejected loudly at set
    /// time rather than stored as silent no-ops.
    pub pane_options: std::collections::HashMap<String, String>,
    /// When set, the layout serialiser renders this pane as blank until
    /// the deadline passes.  Used to hide injected cd+cls commands during
    /// warm session claiming so the user never sees a flash.
    pub squelch_until: Option<Instant>,
    /// Per-pane output ring buffer for control mode %output notifications.
    /// Filled by the PTY reader thread, drained by the server loop.
    pub output_ring: Arc<Mutex<VecDeque<u8>>>,
    /// When the pane's shell was spawned (monotonic), and only for panes that
    /// carry a real, restartable default shell. `None` for popup, proxy, and
    /// empty (`-E`) panes, which must never be auto-respawned. Used by the
    /// opt-in `@heal-crashed-panes` self-heal: a shell that exits within a
    /// short grace window of spawn is treated as a crash-on-startup (e.g. the
    /// #450 pwsh `ReadLineFromFile` FailFast right after a warm-pane transplant)
    /// and respawned in place, so the user still gets a working window.
    pub spawned_at: Option<Instant>,
    /// The command operand this pane was created with, exposed read-only as
    /// `#{pane_start_command}` (issue #580). tmux keeps the spawn argv on the
    /// pane (`window_pane.argc/argv`, set in `spawn.c` only when a command was
    /// actually given) and `format_cb_start_command` stringifies it, which
    /// yields the EMPTY STRING for a pane running the plain default shell.
    /// psmux carries the operand as one string, so the same contract is: empty
    /// for a default-shell pane (including a transplanted warm-pool spare),
    /// the user's command otherwise. Set at every spawn site
    /// (`new-session`/`new-window`/`split-window`, string and `--` argv form)
    /// and replaced by `respawn-pane`/`respawn-window` ONLY when that respawn
    /// carried a command of its own (tmux spawn.c: "Replace the stored
    /// arguments if there are new ones").
    pub start_command: String,
    /// The directory this pane was ASKED to start in, kept until the shell is
    /// observed to actually be somewhere (issue #615 follow up).
    ///
    /// tmux stores the requested cwd on the pane (`window_pane.cwd`, spawn.c)
    /// and the child is `chdir`ed into it before `exec`, so `#{pane_current_path}`
    /// is right from the instant the pane exists.  psmux cannot do that for a
    /// pane claimed from the warm pool: that shell is ALREADY RUNNING in the
    /// directory the pool spawned it in (the server's own cwd), and a running
    /// process's cwd cannot be set from outside, so `-c <dir>` is honoured by
    /// typing a `cd` into it (`pane::silent_rehome`).  That line runs instantly
    /// on an idle box and seconds later when the machine is loaded and the
    /// spare is still booting.  Until it does, the PEB reading is the POOL's
    /// directory, and `#{pane_current_path}` used to report it.
    /// That is how `split-window -c <dir>` could answer with the server's own
    /// working directory, which is never a directory the user asked for.
    ///
    /// With the hint set, the requested directory is reported for exactly as
    /// long as the pane's process is still sitting in the pre-`cd` directory.
    /// The moment the reading moves, whether to the requested directory or
    /// anywhere else because the user typed their own `cd` first, the live
    /// reading takes over permanently.
    pub cwd_hint: Option<CwdHint>,
}

impl Pane {
    /// Display this pane through a copy-mode snapshot: `term` becomes a frozen
    /// copy of the current screen and the live parser is parked in
    /// `live_term`.  The PTY reader keeps feeding the live parser, so the
    /// application keeps running while the user reads.
    ///
    /// Idempotent: a second call while a snapshot is installed does nothing.
    pub fn enter_copy_snapshot(&mut self) {
        if self.live_term.is_some() {
            return;
        }
        let snapshot = match self.term.lock() {
            Ok(term) => term.snapshot(),
            // Poisoned mutex (reader thread panicked): stay on the live screen
            // rather than dropping the pane's terminal.
            Err(_) => return,
        };
        let live = Arc::clone(&self.term);
        self.term = Arc::new(Mutex::new(snapshot));
        self.live_term = Some(live);
    }

    /// Drop the snapshot and show the live screen again, with everything the
    /// application printed while copy mode was up.  Idempotent.
    pub fn leave_copy_snapshot(&mut self) {
        if let Some(live) = self.live_term.take() {
            self.term = live;
        }
    }

    /// The parser holding the pane's **live** screen.
    ///
    /// While copy mode is up `term` is the frozen snapshot the user is
    /// reading, and this is the parser the PTY reader keeps feeding.
    /// Commands that inspect the pane itself rather than the copy-mode
    /// view -- `capture-pane` and its `-e`/`-S`/`-E` variants, matching
    /// tmux, whose capture reads `wp->base` and never the copy-mode
    /// grid -- must go through here.  Outside copy mode it is `term`.
    pub fn live_parser(&self) -> &Arc<Mutex<vt100::Parser>> {
        self.live_term.as_ref().unwrap_or(&self.term)
    }

    /// The view parser plus, while a copy-mode snapshot is installed, the live
    /// parser behind it.
    ///
    /// Anything that reconfigures the terminal (resize, `history-limit`,
    /// `alternate-screen`) must reach both, or the live screen comes back with
    /// a stale geometry the moment copy mode exits.
    pub fn each_term(&self) -> impl Iterator<Item = &Arc<Mutex<vt100::Parser>>> {
        std::iter::once(&self.term).chain(self.live_term.iter())
    }

    /// Mutable counterpart of [`Pane::each_term`].
    pub fn for_each_term_mut<F: FnMut(&mut Arc<Mutex<vt100::Parser>>)>(&mut self, mut f: F) {
        f(&mut self.term);
        if let Some(live) = self.live_term.as_mut() {
            f(live);
        }
    }
}

/// A pane's requested-but-not-yet-observed working directory. See
/// [`Pane::cwd_hint`].
#[derive(Debug)]
pub struct CwdHint {
    /// The pane process the hint describes.  A `respawn-pane` (or any other
    /// path that puts a different child behind the pane) changes `child_pid`,
    /// which retires the hint without every respawn site having to know it
    /// exists.
    pub pid: Option<u32>,
    /// The directory the pane was asked for, in native form.
    pub requested: String,
    /// The reading taken the moment before the `cd` was injected: the
    /// directory the process is in until the shell moves itself. `None` when
    /// nothing could be read, in which case any successful reading is trusted.
    pub stale: Option<String>,
    /// Latched once a reading other than `stale` is seen, so a later `cd` back
    /// into `stale` cannot resurrect the hint.
    pub settled: std::sync::atomic::AtomicBool,
}

/// Pre-spawned shell ready to be transplanted into a new window instantly.
/// The shell has already loaded its profile (~470ms for pwsh), so the prompt
/// appears immediately when the user creates a new window — matching wezterm's
/// perceived "instant tab" experience.
pub struct WarmPane {
    pub master: Box<dyn MasterPty>,
    pub writer: Box<dyn std::io::Write + Send>,
    pub child: Box<dyn portable_pty::Child>,
    pub term: Arc<Mutex<vt100::Parser>>,
    pub data_version: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub cursor_shape: std::sync::Arc<std::sync::atomic::AtomicU8>,
    pub bell_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub cpr_pending: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Issue #473: color query bitmask (see `Pane::color_query_pending`).
    pub color_query_pending: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub child_pid: Option<u32>,
    pub pane_id: usize,
    pub rows: u16,
    pub cols: u16,
    pub output_ring: Arc<Mutex<VecDeque<u8>>>,
    /// When this spare's shell was created. The pool hands out the OLDEST
    /// spare first, because a spare's usefulness is entirely a function of how
    /// much of its shell startup has already elapsed: transplanting a spare
    /// that was spawned 5ms ago costs the caller the whole shell boot, exactly
    /// as a cold spawn would.
    pub spawned_at: std::time::Instant,
    /// Has this spare's shell finished starting? A spare becomes a pool member
    /// about 25ms after it is asked for (that is just `CreateProcess` plus a
    /// ConPTY) but is not worth transplanting for another ~400ms, which is how
    /// long pwsh takes to put a prompt on the screen. Handing out a spare in
    /// between is indistinguishable from a cold spawn: the window opens and
    /// sits blank for the rest of the shell's startup.
    ///
    /// Maintained by [`WarmPane::refresh_ready`]; only ready spares are ever
    /// handed to a caller.
    pub ready: bool,
    /// Readiness bookkeeping: the last `data_version` seen and when it last
    /// changed, which is how "the shell stopped writing" is detected.
    pub last_dv: u64,
    pub last_change: std::time::Instant,
    /// Trace bookkeeping (`PSMUX_WARM_TRACE=1`): whether the "spare became
    /// ready" line has already been emitted for this spare.
    pub trace_settled: bool,
    /// The host terminal palette this spare's shell was spawned with, planted
    /// on it as `PSMUX_HOST_COLORS` (`pane::set_host_colors_env`).
    ///
    /// A child's environment block cannot be changed from outside once it is
    /// running, so this is not bookkeeping — it is the palette this spare will
    /// hand to whatever process it is transplanted into, for the rest of that
    /// pane's life. The server usually learns its real palette only when a
    /// client attaches and reports it, which is after the pool has been
    /// filled, so a spare can easily be holding a palette the server no longer
    /// believes. Recorded here so such a spare can be retired instead of
    /// transplanted (see `warm_pane_sync::for_host_colors_change`).
    pub host_colors: Option<HostColors>,
}

/// How long a spare's output has to stay unchanged before its shell counts as
/// started.
///
/// Measured, not guessed: a pwsh spare writes its prompt and goes quiet about
/// 155ms after spawn, and a claim of a spare 374ms old was instant while claims
/// at 30ms and 40ms both cost ~380ms more. 250ms of quiet puts readiness at
/// ~405ms after spawn, which is exactly where the instant claims begin.
pub const WARM_READY_QUIET: std::time::Duration = std::time::Duration::from_millis(250);

/// Backstop: a spare this old counts as ready however little it has written.
///
/// Readiness is inferred from output, so a `default-shell` that prints nothing
/// at all (or never stops printing) would otherwise never be handed out and
/// every creation would cold spawn forever. Past this age the pool behaves as
/// it did before readiness existed, which is the old, merely imperfect,
/// behaviour rather than a new failure.
pub const WARM_READY_MAX_WAIT: std::time::Duration = std::time::Duration::from_millis(1500);

impl WarmPane {
    /// Recompute [`WarmPane::ready`]. Cheap enough for every loop tick: one
    /// relaxed atomic load per spare, and it returns immediately once ready.
    pub fn refresh_ready(&mut self, now: std::time::Instant) -> bool {
        if self.ready {
            return true;
        }
        let dv = self.data_version.load(std::sync::atomic::Ordering::Relaxed);
        if dv != self.last_dv {
            self.last_dv = dv;
            self.last_change = now;
        }
        let quiet = now.saturating_duration_since(self.last_change);
        if (dv > 0 && quiet >= WARM_READY_QUIET)
            || now.saturating_duration_since(self.spawned_at) >= WARM_READY_MAX_WAIT
        {
            self.ready = true;
        }
        self.ready
    }
}

/// The pool of pre spawned spare shells that `new-window` and `split-window`
/// transplant instead of paying a shell boot.
///
/// This used to be a bare `Option<WarmPane>`: a pool of depth one. Depth one
/// is enough when creations are minutes apart and hopeless when they are not,
/// because a spare is only worth anything once its shell has finished
/// starting. Claim the single spare, refill, and the refill is a brand new
/// shell; the very next creation claims that newborn and pays its whole boot.
/// That is the alternating fast/slow pattern users see when they open several
/// windows in a row.
///
/// Spares are handed out oldest first (`pop_front`) so a claim always gets the
/// most nearly booted shell available.
#[derive(Default)]
pub struct WarmPool {
    spares: VecDeque<WarmPane>,
    /// How many spares to keep while nobody is creating anything. 0 disables
    /// the pool entirely.
    pub target: usize,
    /// Spares whose spawn has been handed to the background spawner and has
    /// not landed back in `spares` yet. Counted towards the target so a burst
    /// does not queue one spawn per loop tick.
    pub inflight: usize,
    /// Until when the pool is allowed to hold more than `target`. Set by a
    /// claim that found no ready spare, which is the only honest signal that
    /// the idle depth is too shallow for what the user is doing right now.
    surge_until: Option<std::time::Instant>,
    /// When the last claim happened, ready or not. Drives surplus trimming.
    last_claim: Option<std::time::Instant>,
    /// No spare below this pane id may be handed out any more.
    ///
    /// A creation that finds the pool empty allocates a fresh id, which is above
    /// every id the in flight refills already reserved. When those refills land,
    /// their ids are in the past: handing one out would make the visible sequence
    /// go backwards, which is what produced `%2 %3 %12 %4`. They are dropped on
    /// arrival instead.
    issued_floor: usize,
}

/// Default depth of the spare shell pool (`warm-pool-size`).
///
/// Two, not one: with a single spare every other creation in a sequence claims
/// a shell that was spawned moments earlier and pays its whole boot. Two costs
/// one extra idle shell (about 45 MB of working set on Windows with pwsh) and
/// removes the alternation. Users who create windows in long bursts can raise
/// it; `set -g warm-pool-size 0` (or `PSMUX_NO_WARM=1`) turns the pool off.
pub const WARM_POOL_SIZE_DEFAULT: usize = 2;

/// Hard ceiling on `warm-pool-size`. Each spare is a real shell process plus a
/// ConPTY, so an unbounded pool is an unbounded memory leak by configuration.
pub const WARM_POOL_SIZE_MAX: usize = 8;

/// Hard ceiling on the pool while it is surging, and the multiple of `target`
/// a surge is allowed to reach.
///
/// A surge exists because depth alone cannot serve a run of creations: a spare
/// takes ~400ms to become useful, so creations arriving every 20ms outrun any
/// fixed depth, and the pool refills one shell per claim while the caller needs
/// far more than one. Surging spawns the whole batch CONCURRENTLY instead, so a
/// run of creations pays one shell startup between them all rather than one
/// each. The multiple keeps a user who deliberately set `warm-pool-size 1` for
/// memory reasons from being handed eight shells.
pub const WARM_POOL_SURGE_MAX: usize = 8;
pub const WARM_POOL_SURGE_FACTOR: usize = 4;

/// How long a surge allowance outlives the claim that triggered it. Long enough
/// to cover a human run of window opening, short enough that the memory comes
/// back while they are still looking at the result.
pub const WARM_SURGE_HOLD: std::time::Duration = std::time::Duration::from_secs(5);

/// How close together two claims have to be to count as a run.
///
/// A surge needs TWO signals, not one: a claim that found nothing ready, and
/// another claim just before it. A lone miss is not a burst, and treating it as
/// one has a measured cost: a cold `new-session` misses by definition (its only
/// spare is newborn), and surging there fired eight shell spawns alongside the
/// session's own starting shell and made startup ~100ms slower.
pub const WARM_BURST_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);

/// Resolve the boot time pool depth: `PSMUX_NO_WARM` wins (0), then
/// `PSMUX_WARM_POOL_SIZE` (clamped), then the compiled default. The config
/// option `warm-pool-size` overrides this once the config has been parsed.
pub fn default_warm_pool_size() -> usize {
    if std::env::var("PSMUX_NO_WARM").map(|v| v == "1" || v == "true").unwrap_or(false) {
        return 0;
    }
    std::env::var("PSMUX_WARM_POOL_SIZE")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .map(|n| n.min(WARM_POOL_SIZE_MAX))
        .unwrap_or(WARM_POOL_SIZE_DEFAULT)
}

impl WarmPool {
    pub fn new(target: usize) -> Self {
        Self { spares: VecDeque::new(), target, inflight: 0, surge_until: None, last_claim: None, issued_floor: 0 }
    }
    /// Claim the oldest spare whatever its state. Used by the shrink and
    /// teardown paths, which only want the process handle back.
    pub fn take(&mut self) -> Option<WarmPane> {
        self.spares.pop_front()
    }
    /// Hand a spare to a creation. Returns it and whether it was ready.
    ///
    /// Readiness decides WHICH spare is handed out, never whether a spare is
    /// handed out at all. A warming spare still beats a cold spawn: it is
    /// already some way into its shell startup and costs no `CreateProcess`.
    /// Gating the claim on readiness outright made a cold `new-session` 330ms
    /// slower, because its one and only spare is by definition newborn.
    ///
    /// What readiness buys is correct ACCOUNTING. An unready claim is reported
    /// to [`WarmPool::note_claim`] as a miss, which opens a surge window, and
    /// it is the surge that turns a run of creations from "one shell startup
    /// each" into "one shell startup between them all".
    pub fn claim(&mut self) -> (Option<WarmPane>, bool) {
        // #450: a spare's shell can die while it idles here. A corpse must
        // never be handed out, and must not sit in the pool occupying a slot
        // that would otherwise be refilled with a working shell.
        self.reap_dead();
        let now = std::time::Instant::now();
        for wp in self.spares.iter_mut() {
            wp.refresh_ready(now);
        }
        // ALWAYS the front, which `push` keeps as the lowest pane id. Two
        // reasons, and they happen to agree:
        //
        //  - Pane ids must be handed out in creation order. A spare's id is
        //    allocated when it is SPAWNED, because it is planted in the shell's
        //    environment as TMUX_PANE and a child's environment cannot be
        //    rewritten later. So the claim order IS the visible id order, and
        //    anything other than lowest-id-first shows up as shuffled ids.
        //  - The lowest id is also the earliest spawned, hence the furthest
        //    through its shell startup, which is the one worth handing over.
        //
        // Picking the first READY spare instead looked equivalent and was not:
        // spares are spawned concurrently, so they neither land nor become
        // ready in id order. Ten splits in a row came out
        // %2 %3 %4 %5 %12 %9 %11 %6 %7 %8.
        let wp = self.spares.pop_front();
        if let Some(ref w) = wp {
            // Handing this id out puts every lower id in the past, exactly as a
            // cold spawn taking `next_pane_id` does, so record it as issued.
            //
            // The pool is sorted and this took its front, so normally the id
            // handed out IS the lowest outstanding one and this changes
            // nothing. It matters when the pool ran dry: refills are spawned
            // concurrently and land in whatever order the OS finishes them, so
            // a claim that waited for an in flight spare can be handed the
            // FIRST one to land rather than the lowest, and the lower ids are
            // still on their way. Without this they landed afterwards, passed
            // `push` because the floor had never moved, and were handed to the
            // next creation: a burst of five new windows came out
            // `%2 %3 %4 %11 %9`. `set_issued_floor` was only ever called on the
            // cold spawn path, which is the one case where no spare is handed
            // out at all.
            self.note_issued(w.pane_id);
        }
        let was_ready = wp.as_ref().map(|w| w.ready).unwrap_or(false);
        (wp, was_ready)
    }
    /// How many spares have finished starting. Does not recompute readiness;
    /// call after [`WarmPool::refresh_all`] or a [`WarmPool::claim`].
    pub fn ready_len(&self) -> usize {
        self.spares.iter().filter(|w| w.ready).count()
    }
    /// Recompute readiness for every spare. Returns how many just flipped to
    /// ready, so the caller can trace the edge.
    pub fn refresh_all(&mut self, now: std::time::Instant) -> usize {
        let mut flipped = 0;
        for wp in self.spares.iter_mut() {
            if !wp.ready && wp.refresh_ready(now) {
                flipped += 1;
            }
        }
        flipped
    }
    /// Record that a creation wanted a spare. `satisfied` is false when no
    /// ready spare was available.
    ///
    /// A surge opens only when the claim follows another claim within
    /// [`WARM_BURST_WINDOW`]: one claim on its own is an isolated creation, not
    /// a run, and over-reacting to it costs startup time for no benefit (a cold
    /// `new-session` misses by definition).
    ///
    /// Given a run, three things open or hold the surge open, and #661 is the
    /// third of them:
    ///
    ///  - a MISS: the run has already outrun the idle depth.
    ///  - a claim that took the LAST READY spare. The next claim in this run is
    ///    then certain to miss, and a spare takes ~400ms to become useful, so
    ///    waiting for that miss to react guarantees one slow creation per burst.
    ///  - the run CONTINUING while a surge is serving it. The surge used to
    ///    decay from the last MISS, so a run the surge was serving well renewed
    ///    nothing: five seconds after the single miss that opened it,
    ///    [`WarmPool::trim_surplus`] killed six ready spares in the middle of a
    ///    run of splits arriving every ~265ms, and the pool fell straight back
    ///    to alternating one ready spare with none, ie the defect the pool
    ///    exists to remove. The window now decays from the last CREATION, which
    ///    is what [`WARM_SURGE_HOLD`] always described, so the surplus still
    ///    comes back five seconds after the user stops creating panes and idle
    ///    memory is unchanged.
    pub fn note_claim(&mut self, satisfied: bool) {
        let now = std::time::Instant::now();
        let following_another = self
            .last_claim
            .map(|t| now.saturating_duration_since(t) <= WARM_BURST_WINDOW)
            .unwrap_or(false);
        self.last_claim = Some(now);
        if !following_another {
            return;
        }
        // `ready_len` here is the pool AFTER this claim took its spare: the
        // claim sites call this straight after `claim`.
        if !satisfied || self.ready_len() == 0 || self.is_surging() {
            self.surge_until = Some(now + WARM_SURGE_HOLD);
        }
    }
    pub fn is_surging(&self) -> bool {
        matches!(self.surge_until, Some(t) if std::time::Instant::now() < t)
    }
    /// Expire the surge window now. Tests need this because the real one lasts
    /// [`WARM_SURGE_HOLD`], and a unit test must not sleep for five seconds.
    #[cfg(test)]
    pub fn end_surge_for_test(&mut self) {
        self.surge_until = None;
    }
    /// Forget that a claim ever happened, so the next one is not read as part of
    /// a run. Tests need this because the real gap is [`WARM_BURST_WINDOW`] and
    /// a unit test must not sleep for a second and a half.
    #[cfg(test)]
    pub fn forget_last_claim_for_test(&mut self) {
        self.last_claim = None;
    }
    /// How many spares the pool should be holding right now. `standby` caps a
    /// `__warm__` helper at one: it creates no windows of its own, so spares
    /// beyond the one its claimant will want first are pure idle memory, and a
    /// standby never surges.
    pub fn effective_target(&self, standby: bool) -> usize {
        if self.target == 0 {
            return 0;
        }
        if standby {
            return self.target.min(1);
        }
        if self.is_surging() {
            return self
                .target
                .saturating_mul(WARM_POOL_SURGE_FACTOR)
                .min(WARM_POOL_SURGE_MAX)
                .max(self.target);
        }
        self.target
    }
    /// Release spares held only because of a surge that has now expired.
    pub fn trim_surplus(&mut self) -> usize {
        if self.is_surging() {
            return 0;
        }
        self.trim_to(self.target)
    }
    /// Kill spares beyond `keep`, NEWEST first: the oldest spares are the ones
    /// whose startup is furthest along, so dropping them would throw away the
    /// 400ms the pool just spent making them useful.
    pub fn trim_to(&mut self, keep: usize) -> usize {
        let mut killed = 0;
        while self.spares.len() > keep {
            if let Some(mut wp) = self.spares.pop_back() {
                wp.child.kill().ok();
                killed += 1;
            } else {
                break;
            }
        }
        killed
    }
    /// Return a freshly spawned spare to the pool, keeping the pool sorted by
    /// pane id.
    ///
    /// Sorted, not appended: refills are spawned concurrently, so they finish
    /// and land in whatever order the OS gets round to them, which is not the
    /// order their ids were allocated in. Appending would make the front of the
    /// queue the first spare to LAND rather than the first to have been created,
    /// and [`WarmPool::claim`] hands out the front. The pool is at most
    /// [`WARM_POOL_SURGE_MAX`] deep, so the insert is a walk over a handful of
    /// entries.
    pub fn push(&mut self, mut wp: WarmPane) {
        // Its id is already in the past: a creation that found the pool empty
        // has since taken a higher one. Handing this out would walk the visible
        // sequence backwards. See `issued_floor`.
        if wp.pane_id < self.issued_floor {
            crate::warm_trace!(
                "pool: refused spare pane={} pid={:?} below floor {}",
                wp.pane_id, wp.child_pid, self.issued_floor
            );
            if let Some(pid) = wp.child_pid {
                crate::platform::process_kill::kill_pid_tree(pid);
            }
            wp.child.kill().ok();
            return;
        }
        let at = self
            .spares
            .iter()
            .position(|w| w.pane_id > wp.pane_id)
            .unwrap_or(self.spares.len());
        self.spares.insert(at, wp);
    }
    /// Record that `id` has been handed out, so every spare below it is refused
    /// on arrival by [`WarmPool::push`].
    ///
    /// Raises the floor and nothing else. The pooled spares are left alone on
    /// purpose: this is called from [`WarmPool::claim`], which has just taken
    /// the front of a pool kept sorted by id, so nothing below what it handed
    /// out is still in there. The caller that needs the pooled spares dropped
    /// too is [`WarmPool::set_issued_floor`].
    fn note_issued(&mut self, id: usize) {
        if id > self.issued_floor {
            self.issued_floor = id;
        }
    }
    /// Refuse every spare below `id` from now on, and drop the pooled ones.
    /// Called by a creation that is about to allocate `id` for itself because no
    /// spare was available. Returns how many pooled spares were discarded.
    pub fn set_issued_floor(&mut self, id: usize) -> usize {
        if id <= self.issued_floor {
            return 0;
        }
        self.issued_floor = id;
        let before = self.spares.len();
        let floor = id;
        let mut dropped = VecDeque::new();
        while let Some(mut wp) = self.spares.pop_front() {
            if wp.pane_id < floor {
                wp.child.kill().ok();
            } else {
                dropped.push_back(wp);
            }
        }
        self.spares = dropped;
        before - self.spares.len()
    }
    pub fn issued_floor(&self) -> usize {
        self.issued_floor
    }
    pub fn len(&self) -> usize {
        self.spares.len()
    }
    pub fn is_empty(&self) -> bool {
        self.spares.is_empty()
    }
    /// How many more spares should be spawned right now, counting the ones
    /// already being spawned in the background.
    pub fn deficit(&self) -> usize {
        self.deficit_for(self.target)
    }
    /// `deficit` against a target other than the configured one. Callers pass
    /// [`WarmPool::effective_target`], which is what applies the standby cap and
    /// the surge allowance.
    pub fn deficit_for(&self, target: usize) -> usize {
        target.saturating_sub(self.spares.len() + self.inflight)
    }
    pub fn iter(&self) -> std::collections::vec_deque::Iter<'_, WarmPane> {
        self.spares.iter()
    }
    pub fn iter_mut(&mut self) -> std::collections::vec_deque::IterMut<'_, WarmPane> {
        self.spares.iter_mut()
    }
    /// Kill and drop every spare. Used by every "the pool is now stale or the
    /// server is going away" site; leaving a spare behind here is how orphan
    /// shells get created.
    pub fn kill_all(&mut self) {
        let mut n = 0;
        for mut wp in self.spares.drain(..) {
            // The tree, not just the direct child: a spare's shell can already
            // have children of its own, and TerminateProcess on the parent
            // alone leaves those running (#686).
            if let Some(pid) = wp.child_pid {
                crate::platform::process_kill::kill_pid_tree(pid);
            }
            wp.child.kill().ok();
            n += 1;
        }
        if n > 0 {
            crate::warm_trace!("pool: killed {n} pooled spare(s)");
        }
    }
    /// Drop spares whose shell died while idling (#450). Returns how many were
    /// removed so the caller can decide whether to log.
    pub fn reap_dead(&mut self) -> usize {
        let before = self.spares.len();
        self.spares.retain_mut(|wp| matches!(wp.child.try_wait(), Ok(None)));
        before - self.spares.len()
    }
}

/// A pane extracted from this session for cross-session forwarding.
/// The real ConPTY stays alive here; I/O is tunneled over TCP to the target.
pub struct ForwardedPane {
    pub master: Box<dyn MasterPty>,
    pub child: Box<dyn portable_pty::Child>,
    pub listener_port: u16,
    pub pid: Option<u32>,
    pub title: String,
    pub rows: u16,
    pub cols: u16,
    /// Handle to the forwarding threads (so we can abort on kill).
    pub shutdown: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutKind { Horizontal, Vertical }

pub enum Node {
    Leaf(Pane),
    Split { kind: LayoutKind, sizes: Vec<u16>, children: Vec<Node> },
}

pub struct Window {
    pub root: Node,
    pub active_path: Vec<usize>,
    pub name: String,
    pub id: usize,
    /// Actual tmux-style window geometry, independent from any client viewport.
    pub area: Rect,
    /// Per-window `window-size` override. `resize-window` sets this to manual.
    pub window_size: Option<String>,
    /// Window scoped options written by `set-option -w -t <window>` (#648).
    ///
    /// tmux keeps one option table per window plus a `global_w_options` table,
    /// and `options_get` walks from the window's table to the global one
    /// (options.c). psmux had only the global store, so `-w` and `-g` selected
    /// the same map: a write aimed at one window landed on every window AND on
    /// the global, which is how the reporter's later panes inherited
    /// `remain-on-exit on` and stopped closing.
    ///
    /// A name present here overrides the global store for THIS window only. A
    /// missing name inherits, so `set -w -u` is just a removal and the window
    /// goes back to following the global value. Reads go through
    /// `crate::server::options::window_local_option` (borrowing, for the
    /// render loop) or `resolve_window_option` (owning, for reporting).
    pub window_options: std::collections::HashMap<String, String>,
    /// Activity flag: set when pane output is received while window is not active
    pub activity_flag: bool,
    /// Bell flag: set when a bell (\x07) is detected in a pane
    pub bell_flag: bool,
    /// Silence flag: set when no output for monitor-silence seconds
    pub silence_flag: bool,
    /// Last output timestamp for silence detection
    pub last_output_time: std::time::Instant,
    /// Last observed combined data_version for activity detection
    pub last_seen_version: u64,
    /// True when the user has manually renamed this window (auto-rename won't override).
    /// Cleared when `set automatic-rename on` is explicitly set.
    pub manual_rename: bool,
    /// Current position in the named layout cycle (0..4)
    pub layout_index: usize,
    /// Per-pane MRU (most-recently-used) order: pane IDs ordered by recency.
    /// Front = most recently focused.  Used for:
    ///  - Directional navigation tie-breaking (issue #70)
    ///  - Focus selection after kill-pane (issue #71)
    pub pane_mru: Vec<usize>,
    /// Per-window zoom state (tmux parity: each window tracks its own zoom independently).
    /// When `Some(...)`, one pane in this window is zoomed; the vec stores saved split sizes
    /// for restoration on unzoom.
    pub zoom_saved: Option<Vec<(Vec<usize>, Vec<u16>)>>,
    /// If this window is a linked reference, stores the source window ID it was linked from.
    pub linked_from: Option<usize>,
    /// Floating panes overlaid ABOVE this window's tiled layout (tmux `new-pane`).
    /// Unlike a popup (a modal `Mode`), floating panes are persistent and coexist
    /// with the tiled panes. Drawn in order, so later entries stack on top.
    pub floating: Vec<FloatingPane>,
    /// Index into `floating` of the pane currently holding input focus, if any.
    /// When `None`, input goes to the tiled active pane.
    pub floating_focus: Option<usize>,
}

/// A floating pane: a PTY-backed pane rendered as a positioned overlay above the
/// tiled layout (tmux `new-pane`). Reuses the full `Pane` infrastructure
/// (vt100 parsing, ConPTY I/O, screen serialization) exactly like popups do,
/// but is persistent, positioned, movable, and resizable rather than modal.
pub struct FloatingPane {
    pub pane: Pane,
    /// Top-left position within the window content area (0-based cols/rows).
    pub x: u16,
    pub y: u16,
    /// Outer size in cells, including the 1-cell border on each side.
    pub w: u16,
    pub h: u16,
    /// Border line style: a `pane-border-lines` value
    /// (single/double/heavy/simple/none). Empty or "single" => default single.
    pub border: String,
    /// Unique pane id (shares the `next_pane_id` space with tiled panes).
    pub id: usize,
    pub title: String,
    /// Last `-P` position keyword (top-left/centre/...), kept so the float can
    /// be re-anchored when the terminal is resized. `None` for explicit coords.
    pub position: Option<String>,
}

/// A menu item for display-menu
#[derive(Clone)]
pub struct MenuItem {
    pub name: String,
    pub key: Option<char>,
    pub command: String,
    pub is_separator: bool,
}

/// A parsed menu structure
#[derive(Clone)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub x: Option<i16>,
    pub y: Option<i16>,
}

/// Hook definition - command to run on certain events
#[derive(Clone)]
pub struct Hook {
    pub name: String,
    pub command: String,
}

// PopupPty has been removed: popups now store an actual Pane
// (see src/popup.rs for the popup-as-pane architecture).

/// Pipe pane state - process piping pane output
pub struct PipePaneState {
    pub pane_id: usize,
    pub process: Option<std::process::Child>,
    pub stdin: bool,
    pub stdout: bool,
}

/// Wait-for channel state
pub struct WaitChannel {
    pub locked: bool,
    pub waiters: Vec<mpsc::Sender<()>>,
}

pub enum Mode {
    Passthrough,
    Prefix { armed_at: Instant },
    CommandPrompt { input: String, cursor: usize },
    WindowChooser { selected: usize, tree: Vec<crate::session::TreeEntry> },
    RenamePrompt { input: String },
    RenameSessionPrompt { input: String },
    CopyMode,
    PaneChooser { opened_at: Instant },
    /// Interactive menu mode
    MenuMode { menu: Menu },
    /// Popup window running a command.
    /// Interactive popups store a real `Pane` (same type as tiled panes),
    /// inheriting all pane features: vt100 parsing, colors, PTY I/O.
    PopupMode { 
        command: String, 
        output: String, 
        process: Option<std::process::Child>,
        width: u16,
        height: u16,
        close_on_exit: bool,
        /// Optional: full Pane powering the popup (for interactive programs)
        popup_pane: Option<Pane>,
        /// Scroll offset for static text popups (lines from top)
        scroll_offset: u16,
    },
    /// Confirmation prompt before command
    ConfirmMode { 
        prompt: String, 
        command: String,
        input: String,
    },
    /// Copy-mode search input
    CopySearch {
        input: String,
        forward: bool,
    },
    /// Big clock display (tmux clock-mode)
    ClockMode,
    /// Interactive buffer chooser (prefix =)
    BufferChooser { selected: usize },
    /// Window index prompt (prefix ') — jump to window by number
    WindowIndexPrompt { input: String },
    /// Interactive option editor (tmux 3.2+ customize-mode)
    CustomizeMode {
        options: Vec<(String, String, String)>,
        selected: usize,
        scroll_offset: usize,
        editing: bool,
        edit_buffer: String,
        edit_cursor: usize,
        filter: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SelectionMode { Char, Line, Rect }

/// Per-pane copy mode state, saved/restored on pane focus changes to provide
/// tmux-style pane-local copy mode.
#[derive(Clone)]
pub struct CopyModeState {
    pub anchor: Option<(u16, u16)>,
    pub anchor_scroll_offset: usize,
    pub pos: Option<(u16, u16)>,
    pub scroll_offset: usize,
    pub selection_mode: SelectionMode,
    pub search_query: String,
    pub count: Option<usize>,
    pub search_matches: Vec<(usize, u16, u16)>,
    pub search_idx: usize,
    pub search_forward: bool,
    pub find_char_pending: Option<u8>,
    pub text_object_pending: Option<u8>,
    pub register_pending: bool,
    pub register: Option<char>,
    /// Mark and last-jump are pane-local like the rest of copy state (#498)
    pub mark: Option<(usize, u16, u16)>,
    pub last_jump: Option<(u8, char)>,
    /// true when the pane was in CopySearch (not CopyMode)
    pub in_search: bool,
    /// search input buffer (only meaningful when in_search == true)
    pub search_input: String,
    /// search direction for CopySearch
    pub search_input_forward: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusDir { Left, Right, Up, Down }

/// One cached `#(command)` expansion: the last output plus the state of any
/// in-flight background worker refreshing it.
#[derive(Clone)]
pub struct ShellEntry {
    /// When `value` was last refreshed; the TTL base for re-running the command.
    pub at: std::time::Instant,
    /// Last run's stdout (trimmed); empty on failure or before the first result.
    pub value: String,
    /// A background worker for this command is currently in flight.
    pub running: bool,
}

pub struct AppState {
    pub windows: Vec<Window>,
    pub active_idx: usize,
    /// While a temporary -t focus is applied (FocusTargetTemp), holds the
    /// REAL user-visible active window index saved before the switch.
    /// Format evaluation uses it for "is this the active window" variables
    /// (#{window_active}, the `*` flag) so `display-message -t <win>` does
    /// not report every targeted window as active (issue #551) — the target
    /// is only *temporarily* focused to serve the request. None when no
    /// temporary focus is in effect.
    pub temp_focus_saved_active: Option<usize>,
    pub mode: Mode,
    pub escape_time_ms: u64,
    pub repeat_time_ms: u64,
    /// True when prefix mode was re-armed by a repeatable binding (not initial prefix press).
    pub prefix_repeating: bool,
    pub prefix_key: (KeyCode, KeyModifiers),
    pub prefix2_key: Option<(KeyCode, KeyModifiers)>,
    pub prediction_dimming: bool,
    /// allow-predictions: when on, do not force PSReadLine PredictionSource to
    /// None after the profile loads, letting the user's own prediction settings
    /// take effect.  The pre-profile crash prevention (#109) still runs.
    /// Default: off
    pub allow_predictions: bool,
    pub drag: Option<DragState>,
    /// In-progress mouse drag on a floating pane (move/resize), if any.
    pub float_drag: Option<FloatDrag>,
    /// Most recently reported client viewport. New dynamic windows inherit it.
    pub client_area: Rect,
    pub last_window_area: Rect,
    pub mouse_enabled: bool,
    /// bold-is-bright: when on (default), rewrite crossterm's 256-indexed
    /// `38;5;N`/`48;5;N` (N<=15) back to the standard 30-37/90-97 SGR codes so
    /// the outer terminal applies "bold is bright" to the 16 basic colors
    /// (issue #425).  Turn off to pass crossterm's output through untouched,
    /// which keeps explicit 256-indexed low colors byte-accurate at the cost of
    /// losing bold-is-bright on basic colors.
    pub bold_is_bright: bool,
    /// Issue #473: the host terminal's colors as reported by the most recently
    /// attached client (or the PSMUX_HOST_COLORS override).  None until a
    /// client reports; the responder then falls back to the Campbell palette.
    pub host_colors: Option<HostColors>,
    /// scroll-enter-copy-mode: when off, mouse scroll at a shell prompt does NOT
    /// auto-enter copy mode.  Default: on (tmux parity).
    pub scroll_enter_copy_mode: bool,
    /// mouse-drag-enter-copy-mode: when on, dragging with the left button
    /// in a pane that does not track the mouse enters copy mode and selects
    /// there (tmux's MouseDragStart -> `copy-mode -M`) instead of painting
    /// the client-side selection overlay.  Default: off (keep the
    /// client-side selection, which copies on release).
    pub mouse_drag_enter_copy_mode: bool,
    /// pwsh-mouse-selection: when on, client-side drag selection behaves like
    /// Windows 11 PowerShell — pane-aware clipping, no copy-on-release (copy
    /// only on right-click), word/line selection on double/triple-click.
    /// Default: off (preserves the legacy pwsh-style copy-on-release).
    pub pwsh_mouse_selection: bool,
    /// mouse-selection: when off, psmux disables its own client-side drag
    /// selection overlay so applications running inside a pane (opencode,
    /// nvim, etc.) can implement their own mouse selection without having
    /// psmux's selection rectangle drawn on top.  Mouse events are still
    /// forwarded to the application (click-to-focus, scroll, app-level
    /// mouse tracking continue to work).  Default: on.  (issue #245)
    pub mouse_selection: bool,
    /// mouse-selection-force: when on, psmux keeps client-side drag selection
    /// even when the pane application enabled mouse tracking. Plain clicks are
    /// deferred until release and replayed to the application; drags are
    /// consumed by psmux. Default: off.
    pub mouse_selection_force: bool,
    /// paste-detection: when on (default), Ctrl+V Press is suppressed and the
    /// Windows paste detection mechanism intercepts clipboard content injected
    /// by the console host.  When off, Ctrl+V is forwarded as send-key C-v so
    /// child applications (e.g. neovim visual block mode) can receive it.
    pub paste_detection: bool,
    /// choose-tree-preview: when on, choose-session and choose-tree pickers
    /// open with the live preview pane already visible (no need to press `p`).
    /// Default: off (matches tmux which has no preview-on-by-default option).
    pub choose_tree_preview: bool,
    pub paste_buffers: Vec<String>,
    /// Named paste buffers (HashMap<name, content>). Named buffers are separate
    /// from the positional stack and are accessed via `set-buffer -b name`.
    pub named_buffers: std::collections::HashMap<String, String>,
    /// Auto-increment counter for unnamed buffer names (buffer0, buffer1, etc.)
    pub paste_next_index: u32,
    pub status_left: String,
    pub status_right: String,
    pub window_base_index: usize,
    /// Stable per-window display indices, parallel to `windows` and kept sorted
    /// ascending. `window_indices[i]` is the tmux-style number of `windows[i]`.
    /// Decoupling the display number from the Vec position lets `renumber-windows
    /// off` (the default) leave gaps when a window is killed, matching tmux.
    /// When this vec is out of sync with `windows` (e.g. mock AppState in unit
    /// tests that push windows directly), the helper methods fall back to the
    /// legacy affine mapping `pos + window_base_index`, so nothing breaks.
    pub window_indices: Vec<usize>,
    pub copy_anchor: Option<(u16,u16)>,
    /// Scroll offset when copy_anchor was set (for viewport-relative adjustment)
    pub copy_anchor_scroll_offset: usize,
    /// The view position that makes `copy_pos`'s screen row mean a specific
    /// CONTENT line, when that is not the position the view is parked on now.
    ///
    /// `None` is the ordinary case and it means "read this endpoint against the
    /// current offset".  Every route that moves the endpoint leaves it there,
    /// because it puts the endpoint on a row of the view the user is looking
    /// at: a cursor motion, `v` / `V` / `o`, a `send-keys -X` verb, a scroll.
    ///
    /// A mouse drag is the one exception, so it is the only thing that sets
    /// this.  Dragging on or past the pane's first or last row records the
    /// endpoint and THEN auto-scrolls the view (`scroll_copy_up` /
    /// `scroll_copy_down`), so for the rest of that gesture the endpoint's row
    /// belongs to the view it was measured in, not to the one now on screen.
    /// Reading it against the current offset put it on the wrong content line
    /// and the release yanked a different range than the client had painted.
    ///
    /// It used to be a plain `usize` that every endpoint write had to keep up
    /// to date.  Nine mouse handlers did and roughly ninety other call sites
    /// did not, so a keyboard selection made after scrolling resolved its
    /// endpoint against the live bottom of the buffer and copied a range
    /// displaced by the scroll offset.  `None` makes the common case correct
    /// by default and leaves the pin to the one gesture that needs it.
    pub copy_pos_scroll_offset: Option<usize>,
    pub copy_pos: Option<(u16,u16)>,
    /// Cell where mouse was pressed down in copy mode (for click vs drag detection, #199)
    pub copy_mouse_down_cell: Option<(u16,u16)>,
    /// The copy-mode endpoint of the last frame this server sent, i.e. the
    /// last cell it told a client to paint the selection at.
    ///
    /// Only the RAW mouse protocol reads this (`remote_mouse_up`): a client
    /// that forwards the terminal's reports verbatim cannot say what it
    /// painted, so a final motion that arrives together with the release — a
    /// slip while the button comes up, a phone's coarse last report, coalesced
    /// motion — is dropped by yanking to this instead of to `copy_pos`.  The
    /// semantic protocol (`pane-mouse`) does not need it: there the client
    /// re-reports the endpoint it actually painted just before the release
    /// (`copy_release_repin` in client.rs), which is the cell the user saw.
    /// `None` means no frame carried this gesture's selection, so the release
    /// falls back to `copy_pos`.
    pub copy_pos_published: Option<(u16,u16)>,
    pub copy_scroll_offset: usize,
    /// Selection mode: Char (default), Line (V), Rect (C-v)
    pub copy_selection_mode: SelectionMode,
    /// Copy-mode search query
    pub copy_search_query: String,    /// Numeric prefix count for copy-mode motions (vi-style)
    pub copy_count: Option<usize>,    /// Copy-mode search matches: (absolute_line, col_start, col_end).
    /// The line is an index into the whole pane buffer, scrollback history
    /// first and then the visible screen, so a match above the viewport is
    /// addressable (#612).
    pub copy_search_matches: Vec<(usize, u16, u16)>,
    /// Current match index in copy_search_matches
    pub copy_search_idx: usize,
    /// Search direction: true = forward (/), false = backward (?)
    pub copy_search_forward: bool,
    /// Pending find-char operation: (f=0,F=1,t=2,T=3) for next char input
    pub copy_find_char_pending: Option<u8>,
    /// Pending text-object prefix: 0 = 'a' (a-word), 1 = 'i' (inner-word)
    pub copy_text_object_pending: Option<u8>,
    /// Pending register selection: true when '"' was pressed, waiting for a-z
    pub copy_register_pending: bool,
    /// Currently selected named register (a-z), None = default unnamed
    pub copy_register: Option<char>,
    /// Copy-mode mark set by `X` (set-mark): (scroll_offset, row, col).
    /// `M-x` (jump-to-mark) swaps the cursor with it, so pressing it twice
    /// returns you to where you started, same as tmux (#498).
    pub copy_mark: Option<(usize, u16, u16)>,
    /// Last f/F/t/T jump as (kind, char), so `;` (jump-again) and `,`
    /// (jump-reverse) can repeat it (#498).
    pub copy_last_jump: Option<(u8, char)>,
    /// When true the pane keeps following live output while in copy mode
    /// instead of being anchored. Toggled by `r` (refresh-from-pane) (#498).
    pub copy_refresh_live: bool,
    /// Named registers a-z for copy-mode yank/paste
    pub named_registers: std::collections::HashMap<char, String>,
    pub display_map: Vec<(usize, Vec<usize>)>,
    /// Key tables: "prefix" (default), "root", "copy-mode-vi", "copy-mode-emacs", etc.
    pub key_tables: std::collections::HashMap<String, Vec<Bind>>,
    /// Current key table for switch-client -T (None = normal mode)
    pub current_key_table: Option<String>,
    pub control_rx: Option<mpsc::Receiver<CtrlReq>>,
    /// Sender for the same channel `control_rx` receives on, so code already
    /// running ON the server loop can queue follow-up work for a later
    /// iteration instead of recursing.
    ///
    /// Used by the copy-mode key tables: a `bind -T copy-mode-vi y send-keys -X
    /// copy-pipe-and-cancel "clip.exe"` resolves while handling a keystroke, and
    /// the `send-keys -X` implementation lives in the loop's own
    /// `CtrlReq::SendKeysX` arm. Re-entering it directly is not possible, and
    /// routing back out through `execute_command_string` would make the server
    /// open a TCP connection to itself while the loop is blocked doing so —
    /// a deadlock. Queueing costs one loop iteration and cannot deadlock.
    pub control_tx: Option<mpsc::Sender<CtrlReq>>,
    pub control_port: Option<u16>,
    pub session_key: String,
    /// Receiver for async run-shell results (title, output).
    /// Commands are spawned in background threads and results polled each frame.
    pub run_shell_rx: Option<mpsc::Receiver<(String, String)>>,
    /// Sender cloned into each run-shell background thread.
    pub run_shell_tx: Option<mpsc::Sender<(String, String)>>,
    /// Receiver for async #(command) format-job results (cmd, output).
    /// Preinitialized in the server loop (unlike run_shell_*) because
    /// run_shell_command sees only &AppState and can't create it lazily.
    pub format_job_rx: Option<mpsc::Receiver<(String, String)>>,
    /// Sender cloned into each #() background worker.
    pub format_job_tx: Option<mpsc::Sender<(String, String)>>,
    pub session_name: String,
    /// Numeric session ID (tmux-compatible: $0, $1, $2...).
    pub session_id: usize,
    /// -L socket name for namespace isolation (tmux compatible).
    /// When set, port/key files are stored as `{socket_name}__{session_name}.port`.
    pub socket_name: Option<String>,
    pub attached_clients: usize,
    /// Per-client terminal sizes for multi-client resize tracking.
    pub client_sizes: std::collections::HashMap<u64, (u16, u16)>,
    /// The most recently active client ID (for window_size="latest").
    pub latest_client_id: Option<u64>,
    /// Most recent client that explicitly reported a viewport size.
    pub latest_size_client_id: Option<u64>,
    /// Client registry: all active PERSISTENT and CONTROL clients.
    pub client_registry: std::collections::HashMap<u64, ClientInfo>,
    pub created_at: chrono::DateTime<Local>,
    pub next_win_id: usize,
    pub next_pane_id: usize,
    /// Pane ids already auto-healed once by `@heal-crashed-panes`. A pane is
    /// respawned at most once so a shell that crashes on every startup can't
    /// spin an infinite respawn loop; after one heal it falls through to the
    /// normal reap path.
    pub healed_pane_ids: std::collections::HashSet<usize>,
    /// Whether the attached client is currently in prefix mode (for `client_prefix` format var).
    pub client_prefix_active: bool,
    pub sync_input: bool,
    /// Hooks: map of hook name to list of commands
    pub hooks: std::collections::HashMap<String, Vec<String>>,
    /// Wait-for channels: map of channel name to list of waiting senders
    pub wait_channels: std::collections::HashMap<String, WaitChannel>,
    /// Pipe pane processes
    pub pipe_panes: Vec<PipePaneState>,
    /// Last active window index (for last-window command)
    pub last_window_idx: usize,
    /// Last active pane path (for last-pane command)
    pub last_pane_path: Vec<usize>,
    /// history-limit: scrollback buffer size (default 2000)
    pub history_limit: usize,
    /// display-time: how long messages are shown (ms, default 750)
    pub display_time_ms: u64,
    /// display-panes-time: how long pane overlay is shown (ms, default 1000)
    pub display_panes_time_ms: u64,
    /// pane-base-index: first pane id (default 0)
    pub pane_base_index: usize,
    /// focus-events: pass focus events to apps
    pub focus_events: bool,
    /// mode-keys: vi or emacs (stored for compat, default emacs)
    pub mode_keys: String,
    /// status: whether status bar is shown
    pub status_visible: bool,
    /// status-position: "top" or "bottom" (default "bottom")
    pub status_position: String,
    /// priority: scheduling class for psmux's OWN processes, never for pane
    /// children (issue #608). One of "normal", "above-normal", "high".
    pub priority: String,
    /// status-style: stored for compat
    pub status_style: String,
    /// default-command / default-shell: shell to launch for new panes
    pub default_shell: String,
    /// word-separators: characters that delimit words in copy mode
    pub word_separators: String,
    /// renumber-windows: auto-renumber on close
    pub renumber_windows: bool,
    /// automatic-rename: update window name from active pane's running command
    pub automatic_rename: bool,
    /// allow-rename: allow programs to set window title via escape sequences
    pub allow_rename: bool,
    /// allow-set-title: allow programs to set pane title via OSC 0/2 escape sequences
    pub allow_set_title: bool,
    /// monitor-activity / visual-activity: stored for compat
    pub monitor_activity: bool,
    pub visual_activity: bool,
    /// activity-action: what to do on activity ("any", "none", "current", "other")
    pub activity_action: String,
    /// silence-action: what to do on silence ("any", "none", "current", "other")
    pub silence_action: String,
    /// remain-on-exit: keep panes open after process exits
    pub remain_on_exit: bool,
    /// destroy-unattached: exit server when no clients remain attached
    pub destroy_unattached: bool,
    /// exit-empty: exit server when all panes/windows are empty
    pub exit_empty: bool,
    /// aggressive-resize: resize window to smallest attached client
    pub aggressive_resize: bool,
    /// set-titles: update terminal title
    pub set_titles: bool,
    /// set-titles-string: format for terminal title
    pub set_titles_string: String,
    /// update-environment: list of env var names to update from client on attach
    pub update_environment: Vec<String>,
    /// Environment variables set via set-environment
    pub environment: std::collections::HashMap<String, String>,
    /// User/plugin options (@-prefixed, tmux convention).
    /// Stored separately from `environment` so they are NOT passed as
    /// shell environment variables to child panes (#105).
    pub user_options: std::collections::HashMap<String, String>,
    /// Tracks which options have been explicitly set by the user or config.
    /// Used by set-option -o (only-if-unset) to distinguish defaults from
    /// explicitly configured values.
    pub user_set_options: std::collections::HashSet<String>,
    /// pane-border-style: style for inactive pane borders
    pub pane_border_style: String,
    /// pane-active-border-style: style for active pane borders
    pub pane_active_border_style: String,
    /// pane-border-hover-style: style for border hover highlight
    pub pane_border_hover_style: String,
    /// window-status-format: format for inactive window tabs
    pub window_status_format: String,
    /// window-status-current-format: format for active window tab
    pub window_status_current_format: String,
    /// window-status-separator: between window status entries
    pub window_status_separator: String,
    /// window-status-style: style for inactive window status
    pub window_status_style: String,
    /// window-status-current-style: style for active window status
    pub window_status_current_style: String,
    /// window-status-activity-style: style for windows with activity
    pub window_status_activity_style: String,
    /// window-status-bell-style: style for windows with bell
    pub window_status_bell_style: String,
    /// window-status-last-style: style for last active window
    pub window_status_last_style: String,
    /// message-style: style for status-line messages
    pub message_style: String,
    /// message-command-style: style for command prompt
    pub message_command_style: String,
    /// mode-style: style for copy-mode highlighting
    pub mode_style: String,
    /// status-left-style: style for status-left area
    pub status_left_style: String,
    /// status-right-style: style for status-right area
    pub status_right_style: String,
    /// Marked pane: (window_index, pane_id) — set by select-pane -m
    pub marked_pane: Option<(usize, usize)>,
    /// monitor-silence: seconds of silence before flagging (0 = off)
    pub monitor_silence: u64,
    /// bell-action: "any", "none", "current", "other"
    pub bell_action: String,
    /// visual-bell: show visual indicator on bell
    pub visual_bell: bool,
    /// Command prompt history
    pub command_history: Vec<String>,
    /// Command prompt history index (for up/down navigation)
    pub command_history_idx: usize,
    /// Whether the command prompt vi mode is in normal (true) vs insert (false)
    pub command_vi_normal: bool,
    /// status-interval: seconds between status-line refreshes (default 15)
    pub status_interval: u64,
    /// Last time the status-interval hook was fired
    pub last_status_interval_fire: std::time::Instant,
    /// TTL cache for `#(cmd)` shell expansions. Without this the format
    /// engine spawns a fresh subprocess on every state_dirty push (~30/s
    /// during active typing), which serializes a slow helper (e.g. pwsh
    /// at ~280 ms cold-start) onto the server main loop and lags echo.
    /// Keyed by command string; entries expire after `status_interval`.
    pub format_shell_cache: std::sync::Mutex<std::collections::HashMap<String, ShellEntry>>,
    /// status-justify: left, centre, right, absolute-centre
    pub status_justify: String,
    /// main-pane-width: percentage for main pane in main-vertical layout (0 = use 60% heuristic)
    pub main_pane_width: u16,
    /// main-pane-height: percentage for main pane in main-horizontal layout (0 = use 60% heuristic)
    pub main_pane_height: u16,
    /// status-left-length: max display width for status-left (default 10)
    pub status_left_length: usize,
    /// status-right-length: max display width for status-right (default 40)
    pub status_right_length: usize,
    /// status lines: number of status bar lines (default 1, set via `set status N`)
    pub status_lines: usize,
    /// status-format: custom format strings for each status line (index 1+)
    pub status_format: Vec<String>,
    /// window-size: "smallest", "largest", "manual", "latest" (default "latest")
    pub window_size: String,
    /// Requested manual geometry, keyed by window ID. The visible geometry may
    /// be smaller while a control client has a per-window size constraint.
    pub manual_window_sizes: std::collections::HashMap<usize, (u16, u16)>,
    /// allow-passthrough: "on", "off", "all" (default "off")
    pub allow_passthrough: String,
    /// copy-command: command to pipe yanked text to (default empty)
    pub copy_command: String,
    /// command-alias: map of alias name to expansion
    pub command_aliases: std::collections::HashMap<String, String>,
    /// codepoint-widths: server-scope ARRAY of Unicode width overrides, one
    /// entry per element in tmux order (later entries win). Stored as the raw
    /// entry strings so `show-options` can echo them back verbatim; the parsed
    /// lookup table lives in the vt100 crate, which is where every width
    /// decision is made.
    pub codepoint_widths: Vec<String>,
    /// Config parse warnings (unknown command/option, malformed value, missing
    /// args) collected during a config load or source-file, surfaced to the
    /// user instead of being silently ignored (issue #370 follow-up).
    pub config_warnings: Vec<String>,
    /// 1-based line number currently being parsed, used to prefix warnings as
    /// `file:line: message` (file comes from config::current_config_file()).
    /// None when parsing a single runtime command.
    pub config_warn_line: Option<usize>,
    /// set-clipboard: "on", "off", "external" (default "on")
    pub set_clipboard: String,
    /// One-shot clipboard text to be sent to the client via OSC 52 (set by yank, consumed by dump-state).
    pub clipboard_osc52: Option<String>,
    /// One-shot bell forward flag: set when an audible bell should be emitted on the client terminal.
    pub bell_forward: bool,
    /// env-shim: inject a Unix-compatible `env` function into PowerShell panes
    /// so that `env VAR=val command` syntax works (required by Claude Code, etc.).
    /// Default: on
    pub env_shim: bool,
    /// claude-code-fix-tty: inject a Node.js preload script via NODE_OPTIONS
    /// that patches process.stdout.isTTY = true inside ConPTY panes.  Works around
    /// Claude Code's isTTY gate that forces in-process agent mode on Windows
    /// (claude-code#26244).  Once Claude Code fixes the bug upstream, users can
    /// disable this with: set -g claude-code-fix-tty off
    /// Default: on
    pub claude_code_fix_tty: bool,
    /// claude-code-force-interactive: set CLAUDE_CODE_FORCE_INTERACTIVE=1 in
    /// pane environments so Claude Code treats the session as interactive even
    /// when its own heuristics disagree.  This prevents the non-interactive
    /// fast-path that bypasses teammateMode entirely.
    /// Once Claude Code fixes the bug upstream, disable with:
    ///   set -g claude-code-force-interactive off
    /// Default: on
    pub claude_code_force_interactive: bool,
    /// Last mouse hover position (col, row) for same-coordinate deduplication.
    /// Windows Terminal suppresses consecutive MOUSE_MOVED at the same position.
    pub last_hover_pos: Option<(u16, u16)>,
    /// Last mouse event position (col, row) for #{mouse_x}, #{mouse_y} format variables.
    pub last_mouse_x: u16,
    pub last_mouse_y: u16,
    /// Transient status-bar message from display-message (without -p).
    /// Tuple of (message_text, timestamp_when_set, optional per_message_duration_ms).
    pub status_message: Option<(String, std::time::Instant, Option<u64>)>,
    /// Whether warm pane/server pre-spawning is enabled (default: on).
    /// When off, new sessions/windows always cold-spawn a fresh shell.
    pub warm_enabled: bool,
    /// Whether DEC private modes 47 / 1049 (alternate screen) are honoured
    /// for new panes (default: on).  When off, full-screen TUI apps that
    /// would normally enter the alt screen instead write straight to the
    /// main grid, so their output ends up in scrollback and is reachable
    /// by `capture-pane -S` and copy-mode (psmux issue #88).  Mirrors
    /// tmux's `set -g alternate-screen on/off`.
    pub allow_alternate_screen: bool,
    /// Pool of pre spawned spare shells, ready for an instant new-window or
    /// split. Depth comes from the `warm-pool-size` option; see [`WarmPool`].
    pub warm_pane: WarmPool,
    /// Where a finished background spare is posted back to the server loop.
    /// Held here rather than only in `run_server` so a claim can schedule its
    /// own refill the moment it happens: the claim that misses goes on to cold
    /// spawn, which blocks the loop for a whole shell startup, and a refill
    /// scheduled after that is a refill scheduled far too late.
    pub warm_refill_tx: Option<std::sync::mpsc::Sender<Option<WarmPane>>>,
    /// The matching receiver, so a claim that found nothing can wait for a spare
    /// that is already on its way instead of spawning a further shell. Taken out
    /// and put back around each use, as `control_rx` is.
    pub warm_refill_rx: Option<std::sync::mpsc::Receiver<Option<WarmPane>>>,
    /// Plugin .ps1 scripts queued during config loading for post-startup execution.
    /// These need the server to be running (TCP listener) before they can apply.
    pub pending_plugin_scripts: Vec<String>,
    /// Connected control mode clients (keyed by client_id).
    pub control_clients: HashMap<u64, ControlClient>,
    /// Session group name (set by `new-session -t target` for tmux group semantics).
    /// Sessions in the same group logically share a window list.
    pub session_group: Option<String>,
    /// When true, hardcoded default keybindings are suppressed (set by unbind-key -a).
    pub defaults_suppressed: bool,
    /// Panes extracted for cross-session forwarding, keyed by forward_id.
    /// The source server keeps these alive so the real ConPTY continues running.
    pub forwarded_panes: HashMap<u64, ForwardedPane>,
    /// Counter for generating unique forward IDs.
    pub next_forward_id: u64,
}

/// What a tmux window target spec resolved to, the output of
/// [`AppState::resolve_window_spec`].
///
/// The two cases are tmux's: an existing window, or (only where tmux sets
/// `CMD_FIND_WINDOW_INDEX`, i.e. move-window/link-window `-t`) a destination
/// number that no window holds yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowTarget {
    /// Vec position of an existing window.
    Pos(usize),
    /// A display index that currently holds no window.
    FreeIndex(usize),
}

impl WindowTarget {
    /// Vec position of the existing window, or None for a free index.
    pub fn pos(self) -> Option<usize> {
        match self { WindowTarget::Pos(p) => Some(p), WindowTarget::FreeIndex(_) => None }
    }
}

impl AppState {
    /// Whether this is the hidden `__warm__` pre-spawn server: a server started
    /// ahead of time so the next `new-session` can claim it instead of paying a
    /// cold-start. It has no client and no visible session until it is claimed
    /// and renamed to a real session, so it must sit out anything that assumes a
    /// real, client-facing session - status-interval timers, startup
    /// client-attached/session-created hooks, and the like.
    pub fn is_warm_server(&self) -> bool {
        self.session_name == "__warm__"
    }

    /// Whether this server should run the periodic `status-interval` timer,
    /// which fires user `status-interval` hooks and re-renders the status line
    /// so time formats (`%H:%M:%S`, `%r`, ...) stay current.
    ///
    /// The `__warm__` server has no clients and no visible status bar, so it must
    /// not run this timer: otherwise a global `status-interval` hook fires twice -
    /// once on the real server and once on the warm one. Once claimed and renamed,
    /// it is no longer warm and runs the timer normally.
    pub fn should_run_status_interval_timer(&self) -> bool {
        self.status_interval > 0 && !self.is_warm_server()
    }

    /// Whether a pane whose shell exited on its own should have its surviving
    /// descendant processes (backgrounded children) force-terminated when the
    /// pane is pruned. Controlled by the `@kill-descendants` user option;
    /// defaults to on because Windows has no SIGHUP/pty process groups, so
    /// without the sweep those descendants (and their conhosts) leak.
    ///
    /// `set -g @kill-descendants off` restores tmux-on-Unix semantics, where a
    /// deliberately backgrounded process outlives its pane's shell. Explicit
    /// kill-pane/kill-window/kill-session paths always kill the full tree,
    /// matching psmux's long-standing behavior, and are not affected by this.
    pub fn kill_descendants_on_exit(&self) -> bool {
        match self.user_options.get("@kill-descendants") {
            Some(v) => !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "off" | "0" | "false" | "no"
            ),
            None => true,
        }
    }

    /// Opt-in self-heal for shells that crash immediately after spawn (issue
    /// #450). When a newly created pane's shell exits within
    /// `HEAL_CRASHED_PANE_GRACE` of being spawned, treat it as a
    /// crash-on-startup (rather than a deliberate `exit`) and respawn a fresh
    /// shell in place instead of pruning the window. Defaults OFF because it is
    /// only needed on environments where pwsh's non-PSReadLine fallback reader
    /// FailFasts on the first ConPTY read; enable per-user with
    /// `set -g @heal-crashed-panes on`.
    pub fn heal_crashed_panes(&self) -> bool {
        match self.user_options.get("@heal-crashed-panes") {
            Some(v) => matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "on" | "1" | "true" | "yes"
            ),
            None => false,
        }
    }

    /// Register a newly attached client exactly once.
    ///
    /// A persistent connection can deliver more than one attach command for
    /// the same server-assigned client id.  Treating each command as a new
    /// client desynchronizes `attached_clients` from `client_registry`, because
    /// the registry entry is naturally unique while the counter is not.
    /// Returns `true` only when a new registry entry was inserted.
    pub fn register_client(&mut self, cid: u64, is_control: bool) -> bool {
        if self.client_registry.contains_key(&cid) {
            return false;
        }

        let tty = format!("/dev/pts/{}", cid);
        self.client_registry.insert(cid, ClientInfo {
            id: cid,
            width: self.client_area.width,
            height: self.client_area.height,
            connected_at: std::time::Instant::now(),
            last_activity: std::time::Instant::now(),
            tty_name: tty,
            is_control,
            last_session: None,
        });
        self.attached_clients = self.attached_clients.saturating_add(1);
        // Preserve the existing distinction between an interactive TUI client
        // and a control-mode client: only the former becomes the latest client
        // used for terminal sizing/input routing.
        if !is_control {
            self.latest_client_id = Some(cid);
        }
        true
    }

    /// Reap a dead client's `client_registry` entry exactly once, keeping the
    /// `attached_clients` counter in lock-step with the registry.
    ///
    /// Returns `true` only when an entry was actually present and removed, so
    /// callers run teardown side effects (resize, hooks, destroy-unattached)
    /// only on a real reap. It is idempotent: a second reap of the same `cid`
    /// is a safe no-op that leaves `attached_clients` untouched. This prevents
    /// the over-decrement that a duplicate `ClientDetach` for one `cid` would
    /// otherwise cause (registry entry present ⟺ counted, guaranteed by
    /// `ClientAttach` incrementing and inserting together).
    pub fn reap_client(&mut self, cid: u64) -> bool {
        self.client_sizes.remove(&cid);
        if self.client_registry.remove(&cid).is_some() {
            self.attached_clients = self.attached_clients.saturating_sub(1);
            self.client_prefix_active = false;
            // A detaching client takes its `switch-client -T` latch with it
            // (issue #640): the next client starts in the default table.
            self.current_key_table = None;
            if self.latest_client_id == Some(cid) {
                self.latest_client_id = self.client_registry.keys().max().copied();
            }
            true
        } else {
            false
        }
    }

    /// True when `window_indices` is a valid parallel array for `windows`.
    /// When false (e.g. a mock AppState that pushed windows directly), the
    /// index helpers use the legacy affine mapping so existing behavior holds.
    pub fn window_indices_valid(&self) -> bool {
        self.window_indices.len() == self.windows.len() && !self.windows.is_empty()
    }

    /// move-window: give the active window display index `target`, then keep the
    /// arrays sorted by index. Refuses (returns false) if another window already
    /// holds `target`, matching tmux. Returns false when indices are not tracked
    /// so the caller can use the legacy Vec-position move.
    pub fn move_active_window_to_index(&mut self, target: usize) -> bool {
        let pos = self.active_idx;
        self.move_window_to_index(pos, target).is_ok()
    }

    /// move-window for an arbitrary source: give the window at Vec position
    /// `pos` the display index `target`, then keep the arrays sorted by index.
    ///
    /// tmux (server_link_window) refuses when another window already holds the
    /// destination index unless the caller killed the occupant first (-k), and
    /// reports it as `index in use: N` at exit 1. That refusal used to be a
    /// bare `false` that every caller discarded, so the command exited 0 having
    /// done nothing (issue #602).
    pub fn move_window_to_index(&mut self, pos: usize, target: usize) -> Result<(), String> {
        if self.windows.is_empty() { return Err("can't find window".to_string()); }
        if !self.window_indices_valid() {
            // A state that pushed windows straight onto the Vec (mock AppState,
            // and any path that skipped `on_window_appended`) has no parallel
            // array. `win_pos`/`win_display_index` already fall back to the
            // affine pos+base mapping there, so materialise exactly that rather
            // than refusing: the alternative was a legacy Vec-position splice
            // whose result did not match tmux for any layout with a gap.
            let base = self.window_base_index;
            self.window_indices = (0..self.windows.len()).map(|i| i + base).collect();
        }
        if pos >= self.windows.len() {
            return Err(format!("can't find window: {}", pos));
        }
        if let Some(p) = self.win_pos(target) {
            if p != pos { return Err(format!("index in use: {}", target)); }
            return Ok(()); // already at target
        }
        self.window_indices[pos] = target;
        self.resort_windows_by_index();
        Ok(())
    }

    /// Make room at display index `idx` by pushing every window at `idx` or
    /// above one index higher, tmux's `winlink_shuffle_up`. Used by
    /// move-window/link-window `-a` (after) and `-b` (before).
    pub fn shuffle_window_indices_up(&mut self, idx: usize) {
        if !self.window_indices_valid() { return; }
        for wi in self.window_indices.iter_mut() {
            if *wi >= idx { *wi += 1; }
        }
    }

    /// Renumber every window contiguously from `base-index`, tmux's
    /// `session_renumber_windows` (what `move-window -r` runs).
    pub fn renumber_windows(&mut self) {
        if !self.window_indices_valid() { return; }
        self.renumber_windows_contiguous();
    }

    /// Apply one move-window request, tmux cmd-move-window.c order of
    /// operations, shared by the server, the CLI and the in-process command
    /// prompt so all three agree (issue #602).
    ///
    /// `src`/`dst` are RAW target specs. The error string is what tmux prints
    /// on stderr and exits 1 with; `-k` (kill the occupant of an index already
    /// in use) is the caller's job, because only the server owns PTY teardown:
    /// it sees `index in use: N` and may kill and retry.
    pub fn move_window(
        &mut self,
        src: Option<&str>,
        dst: Option<&str>,
        detach: bool,
        renumber: bool,
        after: bool,
        before: bool,
    ) -> Result<(), String> {
        // -r renumbers the session and ignores the window target entirely.
        if renumber {
            self.renumber_windows();
            return Ok(());
        }
        if self.windows.is_empty() { return Err("can't find window".to_string()); }
        // Source: -s, else the current window (tmux's `.source` default).
        let spos = match src {
            Some(s) => self.resolve_window_spec(s, false)?.pos()
                .ok_or_else(|| format!("can't find window: {}", s))?,
            None => self.active_idx.min(self.windows.len() - 1),
        };
        // Destination resolved with CMD_FIND_WINDOW_INDEX, so a number naming
        // no window is a free slot rather than an error. A bare `move-window`
        // with no -t takes the next free index.
        let mut idx = match dst {
            Some(d) => match self.resolve_window_spec(d, true)? {
                WindowTarget::Pos(p) => self.win_display_index(p),
                WindowTarget::FreeIndex(i) => i,
            },
            None => self.alloc_window_index(),
        };
        // -a / -b push the destination and everything above it up by one so the
        // moved window lands after / before the target (winlink_shuffle_up).
        if after || before {
            if after { idx += 1; }
            self.shuffle_window_indices_up(idx);
        }
        if self.win_display_index(spos) == idx { return Ok(()); }
        let moved_id = self.windows.get(spos).map(|w| w.id);
        // Occupied and no -k: tmux's "index in use: N" at exit 1. This used to
        // be a discarded `false`, so the command exited 0 having done nothing.
        if let Some(occupant) = self.win_pos(idx) {
            if occupant != spos { return Err(format!("index in use: {}", idx)); }
        }
        self.move_window_to_index(spos, idx)?;
        // Without -d the moved window becomes current and the window that WAS
        // current becomes the last window (tmux passes `!dflag` to
        // server_link_window as its select flag).
        if !detach {
            if let Some(id) = moved_id {
                if let Some(newpos) = self.windows.iter().position(|w| w.id == id) {
                    let prev = self.active_idx;
                    if prev != newpos {
                        self.last_window_idx = prev;
                        self.active_idx = newpos;
                    }
                }
            }
        }
        Ok(())
    }

    /// Vec position of the window a move-window `-k` would have to kill first:
    /// the current occupant of the destination index, when it is not the source
    /// itself. Only the server can act on it (PTY teardown lives there).
    pub fn move_window_kill_target(
        &self,
        src: Option<&str>,
        dst: Option<&str>,
    ) -> Option<usize> {
        let d = dst?;
        let spos = match src {
            Some(s) => self.resolve_window_spec(s, false).ok()?.pos()?,
            None => self.active_idx,
        };
        match self.resolve_window_spec(d, true).ok()? {
            WindowTarget::Pos(p) if p != spos => Some(p),
            _ => None,
        }
    }

    /// Resolve a tmux window target spec against this session, the single
    /// resolver every window-target path shares (issue #602).
    ///
    /// `spec` is a whole `-t`/`-s` value: an optional `session:` prefix is
    /// dropped, because routing already picked the server. What is left is
    /// resolved the way tmux's `cmd_find_get_window_with_session` does, in the
    /// same order: `@id`, `+N`/`-N` offsets, the symbolic `!`/`^`/`$` (and
    /// their `{last}`/`{start}`/`{end}`/`{next}`/`{previous}` spellings), a
    /// display index, then an exact window name.
    ///
    /// `index_ok` mirrors tmux's `CMD_FIND_WINDOW_INDEX`, which only
    /// move-window's and link-window's `-t` set:
    ///   * set   - `+N`/`-N` are ARITHMETIC on the current window's display
    ///             index, and a number naming no window is a free destination
    ///             index rather than an error.
    ///   * clear - `+N`/`-N` step N places through the session's ordered window
    ///             list, wrapping (tmux `winlink_next_by_number`), and a number
    ///             naming no window is `can't find window: N`.
    ///
    /// Before this existed, `swap-window -t +1` reached the server as the
    /// unsigned 1 (Rust's `usize` parser accepts a leading `+`), `-t -1` never
    /// left the CLI because it was read as a session name, and an index that
    /// named no window silently fell back to a raw Vec position.
    pub fn resolve_window_spec(&self, spec: &str, index_ok: bool) -> Result<WindowTarget, String> {
        let raw = spec.trim();
        // Drop a leading `session:`; tmux splits on the FIRST colon too.
        let body = match raw.find(':') { Some(p) => &raw[p + 1..], None => raw };
        let body = body.trim();
        // tmux reports only the WINDOW part: `swap-window -t p:77` prints
        // "can't find window: 77", not the whole "p:77".
        let missing = || format!("can't find window: {}", body);
        if self.windows.is_empty() { return Err(missing()); }
        let last_pos = self.windows.len() - 1;
        // `sess:` with nothing after it (and a bare `-t sess`) means the
        // session's current window.
        if body.is_empty() { return Ok(WindowTarget::Pos(self.active_idx.min(last_pos))); }
        // tmux's symbolic spellings (cmd-find.c window_table).
        let body = match body {
            "{start}" => "^",
            "{last}" => "!",
            "{end}" => "$",
            "{next}" => "+",
            "{previous}" => "-",
            other => other,
        };
        if let Some(id) = body.strip_prefix('@') {
            let id: usize = id.parse().map_err(|_| missing())?;
            return self.windows.iter().position(|w| w.id == id)
                .map(WindowTarget::Pos).ok_or_else(missing);
        }
        if let Some(digits) = body.strip_prefix(['+', '-']) {
            let forward = body.starts_with('+');
            if !digits.is_empty() && !digits.chars().all(|c| c.is_ascii_digit()) {
                return Err(missing());
            }
            let n: usize = if digits.is_empty() { 1 } else { digits.parse().map_err(|_| missing())? };
            let cur = self.active_idx.min(last_pos);
            if index_ok {
                // CMD_FIND_WINDOW_INDEX: arithmetic on the display index.
                let base = self.win_display_index(cur);
                let idx = if forward {
                    base.checked_add(n).ok_or_else(missing)?
                } else {
                    base.checked_sub(n).ok_or_else(missing)?
                };
                return Ok(match self.win_pos(idx) {
                    Some(p) => WindowTarget::Pos(p),
                    None => WindowTarget::FreeIndex(idx),
                });
            }
            // winlink_next_by_number / winlink_previous_by_number: N steps
            // through the ordered list, wrapping at either end. NOT index
            // arithmetic: with windows 0, 2, 9, 10, `+1` from 2 is 9.
            let len = self.windows.len();
            let step = n % len;
            let pos = if forward { (cur + step) % len } else { (cur + len - step) % len };
            return Ok(WindowTarget::Pos(pos));
        }
        match body {
            "!" => {
                let p = self.last_window_idx;
                if p < self.windows.len() { Ok(WindowTarget::Pos(p)) } else { Err(missing()) }
            }
            "^" => Ok(WindowTarget::Pos(0)),
            "$" => Ok(WindowTarget::Pos(last_pos)),
            _ => {
                if body.chars().all(|c| c.is_ascii_digit()) {
                    if let Ok(idx) = body.parse::<usize>() {
                        if let Some(p) = self.win_pos(idx) { return Ok(WindowTarget::Pos(p)); }
                        if index_ok { return Ok(WindowTarget::FreeIndex(idx)); }
                        return Err(missing());
                    }
                }
                // Exact name. tmux refuses an ambiguous match rather than
                // picking one, so more than one hit is still "can't find".
                let mut hit = None;
                for (i, w) in self.windows.iter().enumerate() {
                    if w.name == body {
                        if hit.is_some() { return Err(missing()); }
                        hit = Some(i);
                    }
                }
                hit.map(WindowTarget::Pos).ok_or_else(missing)
            }
        }
    }

    /// Display (tmux-style) index of the window at Vec position `pos`.
    pub fn win_display_index(&self, pos: usize) -> usize {
        if self.window_indices_valid() {
            self.window_indices.get(pos).copied()
                .unwrap_or(pos + self.window_base_index)
        } else {
            pos + self.window_base_index
        }
    }

    /// Vec position of the window whose display index is `display`, if any.
    pub fn win_pos(&self, display: usize) -> Option<usize> {
        if self.window_indices_valid() {
            self.window_indices.iter().position(|&x| x == display)
        } else if display >= self.window_base_index {
            let pos = display - self.window_base_index;
            if pos < self.windows.len() { Some(pos) } else { None }
        } else {
            None
        }
    }

    /// Next display index for an appended window: one past the current highest
    /// so the parallel array stays sorted without reordering. (tmux also fills
    /// interior gaps; psmux appends to avoid reshuffling `active_idx`, which the
    /// detached new-window restore relies on. Gaps from kills still persist.)
    ///
    /// Derived purely from `window_indices` (not `windows.len()`): this is called
    /// from `on_window_appended` *after* the window was pushed, so the two arrays
    /// are momentarily out of sync and a length-based computation would collide
    /// with an existing index.
    pub fn alloc_window_index(&self) -> usize {
        self.window_indices.iter().copied().max()
            .map(|m| m + 1)
            .unwrap_or(self.window_base_index)
    }

    /// Rewrite indices to contiguous base, base+1, ... in Vec order.
    /// tmux does this only when `renumber-windows` is on.
    fn renumber_windows_contiguous(&mut self) {
        for i in 0..self.window_indices.len() {
            self.window_indices[i] = i + self.window_base_index;
        }
    }

    /// Shift every existing window's display index by the delta between the
    /// old and new `base-index` value. Without this, `window_indices` (baked
    /// in at window-creation time) keeps showing the base-index that was in
    /// effect when each window was created, so `set-option base-index` after
    /// session start silently had no visible effect on #I / find-window
    /// (task #7 batch A bug 3). Real tmux applies base-index the same way:
    /// existing gaps between window numbers are preserved, only the origin
    /// moves.
    pub fn rebase_window_indices(&mut self, new_base: usize) {
        let old_base = self.window_base_index;
        if !self.window_indices_valid() || new_base == old_base { return; }
        let delta = new_base as isize - old_base as isize;
        for wi in self.window_indices.iter_mut() {
            let shifted = *wi as isize + delta;
            *wi = shifted.max(0) as usize;
        }
    }

    /// Keep `windows` and `window_indices` sorted ascending by index, preserving
    /// which window is active by re-resolving `active_idx` via the window id.
    /// `last_window_idx` is a Vec position too and is re-resolved the same way,
    /// or a move-window would leave `#{window_last_flag}` pointing at whichever
    /// window happened to slide into the old slot.
    fn resort_windows_by_index(&mut self) {
        if !self.window_indices_valid() { return; }
        let active_id = self.windows.get(self.active_idx).map(|w| w.id);
        let last_id = self.windows.get(self.last_window_idx).map(|w| w.id);
        let mut order: Vec<usize> = (0..self.windows.len()).collect();
        order.sort_by_key(|&i| self.window_indices[i]);
        if order.iter().enumerate().all(|(i, &o)| i == o) { return; } // already sorted
        let mut new_windows: Vec<Window> = Vec::with_capacity(self.windows.len());
        let mut new_indices: Vec<usize> = Vec::with_capacity(self.windows.len());
        for &i in &order {
            new_indices.push(self.window_indices[i]);
        }
        // Move windows out in the new order without cloning.
        let mut taken: Vec<Option<Window>> = self.windows.drain(..).map(Some).collect();
        for &i in &order {
            new_windows.push(taken[i].take().unwrap());
        }
        self.windows = new_windows;
        self.window_indices = new_indices;
        if let Some(aid) = active_id {
            if let Some(p) = self.windows.iter().position(|w| w.id == aid) {
                self.active_idx = p;
            }
        }
        if let Some(lid) = last_id {
            if let Some(p) = self.windows.iter().position(|w| w.id == lid) {
                self.last_window_idx = p;
            }
        }
    }

    /// Call right after a new window was pushed onto `windows`. Assigns it the
    /// next display index (append semantics; see `alloc_window_index`). Does not
    /// touch `active_idx` — the caller owns that. Only maintains the parallel
    /// array when it was already in sync (or when this is the first window), so
    /// mock AppState that pushes windows directly stays in affine-fallback mode.
    pub fn on_window_appended(&mut self) {
        if self.window_indices.len() + 1 == self.windows.len() {
            let idx = self.alloc_window_index();
            self.window_indices.push(idx);
        }
    }

    /// Call right after `windows.remove(pos)`. Drops the parallel index and,
    /// when `renumber-windows` is on, renumbers the survivors contiguously.
    pub fn on_window_removed(&mut self, pos: usize) {
        if self.window_indices.len() == self.windows.len() + 1 && pos < self.window_indices.len() {
            self.window_indices.remove(pos);
            if self.renumber_windows {
                self.renumber_windows_contiguous();
            }
        }
        let live_ids: std::collections::HashSet<usize> =
            self.windows.iter().map(|window| window.id).collect();
        self.manual_window_sizes
            .retain(|window_id, _| live_ids.contains(window_id));
    }

    /// Create a new AppState with sensible defaults.
    /// Caller should set `session_name` and call `load_config()` after construction.
    pub fn new(session_name: String) -> Self {
        Self {
            windows: Vec::new(),
            active_idx: 0,
            temp_focus_saved_active: None,
            mode: Mode::Passthrough,
            escape_time_ms: 500,
            repeat_time_ms: 500,
            prefix_repeating: false,
            prefix_key: (crossterm::event::KeyCode::Char('b'), crossterm::event::KeyModifiers::CONTROL),
            prefix2_key: None,
            prediction_dimming: std::env::var("PSMUX_DIM_PREDICTIONS")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            allow_predictions: false,
            drag: None,
            float_drag: None,
            client_area: Rect { x: 0, y: 0, width: 120, height: 30 },
            last_window_area: Rect { x: 0, y: 0, width: 120, height: 30 },
            mouse_enabled: true,
            bold_is_bright: true,
            host_colors: std::env::var("PSMUX_HOST_COLORS").ok()
                .map(|s| HostColors::from_spec(&s))
                .filter(|hc| hc.has_any() || hc.dark.is_some()),
            scroll_enter_copy_mode: true,
            mouse_drag_enter_copy_mode: false,
            pwsh_mouse_selection: false,
            mouse_selection: true,
            mouse_selection_force: false,
            paste_detection: true,
            choose_tree_preview: false,
            paste_buffers: Vec::new(),
            named_buffers: std::collections::HashMap::new(),
            paste_next_index: 0,
            status_left: "[#S] ".to_string(),
            status_right: "#{?window_bigger,[#{window_offset_x}#,#{window_offset_y}] ,}\"#{=21:pane_title}\" %H:%M %d-%b-%y".to_string(),
            window_base_index: 0,
            window_indices: Vec::new(),
            copy_anchor: None,
            copy_anchor_scroll_offset: 0,
            copy_pos_scroll_offset: None,
            copy_pos: None,
            copy_mouse_down_cell: None,
            copy_pos_published: None,
            copy_scroll_offset: 0,
            copy_selection_mode: SelectionMode::Char,
            copy_count: None,
            copy_search_query: String::new(),
            copy_search_matches: Vec::new(),
            copy_search_idx: 0,
            copy_search_forward: true,
            copy_find_char_pending: None,
            copy_text_object_pending: None,
            copy_register_pending: false,
            copy_register: None,
            copy_mark: None,
            copy_last_jump: None,
            copy_refresh_live: false,
            named_registers: std::collections::HashMap::new(),
            display_map: Vec::new(),
            key_tables: std::collections::HashMap::new(),
            current_key_table: None,
            control_rx: None,
            control_tx: None,
            control_port: None,
            session_key: String::new(),
            run_shell_rx: None,
            run_shell_tx: None,
            format_job_rx: None,
            format_job_tx: None,
            session_name,
            session_id: crate::session::allocate_session_id(),
            socket_name: None,
            attached_clients: 0,
            client_sizes: std::collections::HashMap::new(),
            latest_client_id: None,
            latest_size_client_id: None,
            client_registry: std::collections::HashMap::new(),
            created_at: Local::now(),
            next_win_id: 1,
            next_pane_id: 1,
            healed_pane_ids: std::collections::HashSet::new(),
            client_prefix_active: false,
            sync_input: false,
            hooks: std::collections::HashMap::new(),
            wait_channels: std::collections::HashMap::new(),
            pipe_panes: Vec::new(),
            last_window_idx: 0,
            last_pane_path: Vec::new(),
            history_limit: 2000,
            display_time_ms: 750,
            display_panes_time_ms: 1000,
            pane_base_index: 0,
            focus_events: false,
            mode_keys: "emacs".to_string(),
            status_visible: true,
            status_position: "bottom".to_string(),
            // Deliberately the compile-time default, not the environment: a
            // fresh AppState must match the catalog default no matter what
            // PSMUX_PRIORITY happens to be set to in the process that built it
            // (tests-rs/test_option_default_parity.rs). The environment is
            // consulted where the class is actually applied, at startup.
            priority: crate::platform::DEFAULT_PRIORITY.to_string(),
            status_style: "bg=green,fg=black".to_string(),
            default_shell: String::new(),
            word_separators: " -_@".to_string(),
            renumber_windows: false,
            automatic_rename: true,
            allow_rename: true,
            allow_set_title: false,
            monitor_activity: false,
            visual_activity: false,
            activity_action: "other".to_string(),
            silence_action: "other".to_string(),
            remain_on_exit: false,
            destroy_unattached: false,
            exit_empty: true,
            aggressive_resize: false,
            set_titles: false,
            set_titles_string: String::new(),
            update_environment: vec![
                "DISPLAY".to_string(),
                "KRB5CCNAME".to_string(),
                "SSH_ASKPASS".to_string(),
                "SSH_AUTH_SOCK".to_string(),
                "SSH_AGENT_PID".to_string(),
                "SSH_CONNECTION".to_string(),
                "WINDOWID".to_string(),
                "XAUTHORITY".to_string(),
            ],
            environment: std::collections::HashMap::new(),
            user_options: std::collections::HashMap::new(),
            user_set_options: std::collections::HashSet::new(),
            pane_border_style: String::new(),
            pane_active_border_style: "fg=green".to_string(),
            pane_border_hover_style: "fg=yellow".to_string(),
            window_status_format: "#I:#W#{?window_flags,#{window_flags}, }".to_string(),
            window_status_current_format: "#I:#W#{?window_flags,#{window_flags}, }".to_string(),
            window_status_separator: " ".to_string(),
            window_status_style: String::new(),
            window_status_current_style: String::new(),
            window_status_activity_style: "reverse".to_string(),
            window_status_bell_style: "reverse".to_string(),
            window_status_last_style: String::new(),
            message_style: "bg=yellow,fg=black".to_string(),
            message_command_style: "bg=black,fg=yellow".to_string(),
            mode_style: "bg=yellow,fg=black".to_string(),
            status_left_style: String::new(),
            status_right_style: String::new(),
            marked_pane: None,
            monitor_silence: 0,
            bell_action: "any".to_string(),
            visual_bell: false,
            command_history: Vec::new(),
            command_history_idx: 0,
            command_vi_normal: false,
            status_interval: 15,
            last_status_interval_fire: std::time::Instant::now(),
            format_shell_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            status_justify: "left".to_string(),
            main_pane_width: 0,
            main_pane_height: 0,
            status_left_length: 10,
            status_right_length: 40,
            status_lines: 1,
            status_format: Vec::new(),
            window_size: "latest".to_string(),
            manual_window_sizes: std::collections::HashMap::new(),
            allow_passthrough: "off".to_string(),
            copy_command: String::new(),
            command_aliases: std::collections::HashMap::new(),
            codepoint_widths: Vec::new(),
            config_warnings: Vec::new(),
            config_warn_line: None,
            set_clipboard: "on".to_string(),
            clipboard_osc52: None,
            bell_forward: false,
            env_shim: true,
            claude_code_fix_tty: true,
            claude_code_force_interactive: true,
            last_hover_pos: None,
            last_mouse_x: 0,
            last_mouse_y: 0,
            status_message: None,
            warm_enabled: std::env::var("PSMUX_NO_WARM").map(|v| v != "1" && v != "true").unwrap_or(true),
            allow_alternate_screen: true,
            warm_pane: WarmPool::new(default_warm_pool_size()),
            warm_refill_tx: None,
            warm_refill_rx: None,
            pending_plugin_scripts: Vec::new(),
            control_clients: HashMap::new(),
            session_group: None,
            defaults_suppressed: false,
            forwarded_panes: HashMap::new(),
            next_forward_id: 1,
        }
    }

    /// Get the port/key file base name, incorporating socket_name for -L namespace isolation.
    /// When socket_name is set (via -L flag), files are stored as `{socket_name}__{session_name}`.
    /// Otherwise, just the session_name is used.
    pub fn port_file_base(&self) -> String {
        if let Some(ref sn) = self.socket_name {
            format!("{}__{}", sn, self.session_name)
        } else {
            self.session_name.clone()
        }
    }
}

pub struct DragState {
    pub split_path: Vec<usize>,
    pub kind: LayoutKind,
    pub index: usize,
    pub start_x: u16,
    pub start_y: u16,
    pub left_initial: u16,
    pub _right_initial: u16,
    /// Total pixel dimension of the parent split area along the split axis.
    pub total_pixels: u16,
}

/// An in-progress mouse drag on a floating pane (tmux moves/resizes floats by
/// dragging). Grabbing the body moves it; grabbing the bottom/right edge resizes.
#[derive(Clone, Copy)]
pub struct FloatDrag {
    /// Index into the active window's `floating` vec.
    pub index: usize,
    pub mode: FloatDragMode,
}

#[derive(Clone, Copy)]
pub enum FloatDragMode {
    /// Move: the cursor's offset (dx, dy) from the float's top-left at grab time.
    Move { dx: u16, dy: u16 },
    /// Resize from the bottom-right corner.
    Resize,
}

#[derive(Clone)]
pub enum Action { 
    DisplayPanes, 
    MoveFocus(FocusDir),
    /// Execute an arbitrary tmux-style command string
    Command(String),
    /// Execute multiple tmux-style commands in sequence (`;` chaining)
    CommandChain(Vec<String>),
    /// Common actions with direct handling
    NewWindow,
    SplitHorizontal,
    SplitVertical,
    KillPane,
    NextWindow,
    PrevWindow,
    CopyMode,
    Paste,
    Detach,
    RenameWindow,
    WindowChooser,
    SessionChooser,
    ZoomPane,
    /// Switch to a named key table (switch-client -T)
    SwitchTable(String),
}

#[derive(Clone)]
pub struct Bind { pub key: (KeyCode, KeyModifiers), pub action: Action, pub repeat: bool }

/// Tiled layout, border settings, and focus state used by chooser previews.
/// Persistent floating panes and pane title bars are not included.
#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct PreviewWindowState {
    pub layout: crate::layout::LayoutJson,
    pub border_lines: String,
    pub border_indicators: crate::pane_border::PaneBorderIndicators,
    #[serde(default)]
    pub pane_border_style: Option<String>,
    #[serde(default)]
    pub pane_active_border_style: Option<String>,
    pub floating_pane_focused: bool,
}

/// Parse preview state while accepting the layout-only `window-dump <id>`
/// response.
pub(crate) fn parse_preview_window_state(raw: &str) -> Option<PreviewWindowState> {
    if let Ok(state) = serde_json::from_str(raw) {
        return Some(state);
    }
    let layout = serde_json::from_str(raw).ok()?;
    Some(PreviewWindowState {
        layout,
        border_lines: crate::border_lines::DEFAULT.to_string(),
        border_indicators: crate::pane_border::PaneBorderIndicators::default(),
        pane_border_style: None,
        pane_active_border_style: None,
        floating_pane_focused: false,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowDumpFormat {
    Layout,
    PreviewState,
}

pub enum CtrlReq {
    /// A pane parser thread published new screen state.
    ///
    /// Carries nothing: the data itself travels through `PTY_DATA_READY` and
    /// the panes' own buffers. Its only job is to make the server loop's
    /// `recv_timeout` return NOW instead of at the end of its poll interval,
    /// so pty output reaches the render on the event rather than on a timer.
    /// See `wake_server_loop`.
    PtyWake,
    /// A client sent a request that was not a bare pointer sample.
    ///
    /// `window-size latest` must follow the client the user is actually
    /// using (tmux keeps the most recently active client at the head of
    /// `clients->latest`).  Without this, typing and clicking never
    /// refreshed the size choice, so a phone client that narrowed the
    /// window kept it narrow after the user went back to the desktop
    /// (only a real resize, `client-size`, ever updated it).
    ClientActivity(u64),
    NewWindow(Option<String>, Option<String>, bool, Option<String>, Option<String>, bool, Vec<(String, String)>),  // cmd, name, detached, start_dir, title (-T), empty (-E), env (-e, #489)
    NewWindowPrint(Option<String>, Option<String>, bool, Option<String>, Option<String>, mpsc::Sender<String>, Option<String>, bool, Vec<(String, String)>),  // cmd, name, detached, start_dir, format, resp, title (-T), empty (-E), env (-e, #489)
    SplitWindow(LayoutKind, Option<String>, bool, Option<String>, Option<(u16, bool)>, mpsc::Sender<String>, Option<String>, Vec<(String, String)>, bool),  // kind, cmd, detached, start_dir, size (value, is_percent), error_resp, title (-T), env (-e, #489), zoom (-Z)
    SplitWindowPrint(LayoutKind, Option<String>, bool, Option<String>, Option<(u16, bool)>, Option<String>, mpsc::Sender<String>, Option<String>, Vec<(String, String)>, bool),  // kind, cmd, detached, start_dir, size (value, is_percent), format, resp, title (-T), env (-e, #489), zoom (-Z)
    /// new-pane: create a floating pane over the active window's layout.
    /// Flags match tmux: `-X`/`-Y` position, `-x`/`-y` size, `-B` border,
    /// `-T` title, `-c` dir, `-d` detached, `-P` print (returns the pane id).
    NewFloat {
        command: String,
        /// -X x-position (top-left column); None = centre horizontally.
        x: Option<u16>,
        /// -Y y-position (top-left row); None = centre vertically.
        y: Option<u16>,
        /// -x width (outer, incl. border).
        w: Option<u16>,
        /// -y height (outer, incl. border).
        h: Option<u16>,
        border: String,
        title: Option<String>,
        start_dir: Option<String>,
        detached: bool,
        /// -E: create an empty pane (no command / process).
        empty: bool,
        /// -P: reply with the new pane id over `resp`.
        resp: Option<mpsc::Sender<String>>,
    },
    KillPane,
    KillPaneById(usize),
    /// Capture a pane's plain text. Fields: resp, target pane id
    /// (`-t %N`, None = active pane), preserve trailing spaces (`-N`).
    CapturePane(mpsc::Sender<String>, Option<usize>, bool),
    /// Capture a pane's text with ANSI SGR sequences. Fields: resp,
    /// start (-S), end (-E), target pane id (`-t %N`, None = active pane),
    /// preserve trailing spaces (`-N`).
    CapturePaneStyled(mpsc::Sender<String>, Option<i32>, Option<i32>, Option<usize>, bool),
    FocusWindow(usize),
    /// Focus window by @N id lookup
    FocusWindowById(usize),
    /// Focus window by name lookup
    FocusWindowByName(String),
    FocusPane(usize),
    FocusPaneByIndex(usize),
    /// Temporary focus for generic -t targeting, with validation (issue #545).
    /// The server resolves the window and/or pane FIRST and replies Err when
    /// the target does not exist, so the connection thread can report
    /// "can't find window/pane: X" and skip the follow-on command instead of
    /// letting it run against whatever window happens to be active (the old
    /// FocusWindow*Temp handlers silently no-opped on a miss, so kill-pane,
    /// send-keys, rename-window, capture-pane etc. with a stale target
    /// destroyed/typed-into/read the ACTIVE window at rc=0). On success the
    /// focus is applied with the same temp-restore bookkeeping as before.
    /// `win` carries an index or, when `win_is_id` is set, an @id; `win_name`
    /// carries a window-name target. `pane` carries a %id when `pane_is_id`
    /// is set, otherwise a positional pane index within the target window.
    FocusTargetTemp {
        win: Option<usize>,
        win_is_id: bool,
        win_name: Option<String>,
        pane: Option<usize>,
        pane_is_id: bool,
        resp: mpsc::Sender<Result<(), String>>,
    },
    SessionInfo(mpsc::Sender<String>),
    /// `list-sessions -F <fmt>` — render the session row using a tmux format
    /// string. Drop-in compat with iTerm2 and other CC clients that always
    /// pass `-F` to get structured output.
    SessionInfoFormat(mpsc::Sender<String>, String),
    /// Capture a plain-text row range. Fields: resp, start (-S), end (-E),
    /// target pane id (`-t %N`, None = active pane), preserve trailing
    /// spaces (`-N`).
    CapturePaneRange(mpsc::Sender<String>, Option<i32>, Option<i32>, Option<usize>, bool),
    ClientAttach(u64),
    ClientDetach(u64),
    DumpLayout(mpsc::Sender<String>),
    DumpState(mpsc::Sender<String>, bool, u64),  // (resp, allow_nc, client_id)
    SendText(String),
    SendKey(String),
    SendPaste(String),
    ZoomPane,
    PrefixBegin,
    PrefixEnd,
    CopyEnter,
    CopyEnterPageUp,
    CopyMove(i16, i16),
    CopyAnchor,
    CopyYank,
    CopyRectToggle,
    ClientSize(u64, u16, u16),
    /// Issue #473: a client reporting its host terminal's colors (spec string
    /// in `HostColors::to_spec` form), gathered by querying the host terminal
    /// at attach time.
    HostColors(String),
    FocusPaneCmd(usize),
    FocusWindowCmd(usize),
    MouseDown(u64,u16,u16),
    MouseDownRight(u64,u16,u16),
    MouseDownMiddle(u64,u16,u16),
    MouseDrag(u64,u16,u16),
    MouseUp(u64,u16,u16),
    MouseUpRight(u64,u16,u16),
    MouseUpMiddle(u64,u16,u16),
    MouseMove(u64,u16,u16),
    ScrollUp(u64,u16, u16),
    ScrollDown(u64,u16, u16),
    /// Client-side semantic mouse event: pane-relative coordinates, targeted by pane ID.
    /// Fields: client_id, pane_id, sgr_button, col_0based, row_0based, press
    PaneMouse(u64, usize, u8, i16, i16, bool),
    /// Client-side semantic scroll: targeted by pane ID.
    /// Fields: client_id, pane_id, up (true=up, false=down), pointer position as
    /// pane-relative 0-based (col, row).  The position is optional because older
    /// clients send `pane-scroll PANE up|down` with no coordinates; when it is
    /// absent the server falls back to the pane centre.
    PaneScroll(u64, usize, bool, Option<(i16, i16)>),
    /// Hand a normal-mode client-side drag selection off to server-side copy
    /// mode because the pointer crossed the pane's top edge — or, when the
    /// view is direct-scrolled (scroll-enter-copy-mode off, #193), its
    /// bottom edge (the selection must continue into scrollback).
    /// Fields: client_id, pane_id,
    /// anchor col, anchor row, current col, current row (pane-relative
    /// 0-based, may be out of range), rect (block) selection flag.
    CopyDragBegin(u64, usize, i16, i16, i16, i16, bool),
    /// Client-side semantic split resize: set sizes at a tree path.
    /// Fields: client_id, path, new sizes
    SplitSetSizes(u64, Vec<usize>, Vec<u16>),
    /// Client signals border drag is complete — trigger PTY resize.
    /// Fields: client_id
    SplitResizeDone(u64),
    NextWindow,
    PrevWindow,
    RenameWindow(String),
    ListWindows(mpsc::Sender<String>),
    ListWindowsTmux(mpsc::Sender<String>),
    ListWindowsFormat(mpsc::Sender<String>, String),
    ListTree(mpsc::Sender<String>),
    /// Issue #257: simplified layout (split kind/sizes + pane ids)
    /// for a specific window, used for choose-tree preview rendering.
    WindowLayout(usize, mpsc::Sender<String>),
    /// Issue #257: fully styled layout or preview state for a specific window.
    WindowDump(usize, WindowDumpFormat, mpsc::Sender<String>),
    ToggleSync,
    SetPaneTitle(String),
    SetPaneStyle(String),
    /// select-pane -T/-P without activation (#592, tmux parity): applies
    /// title and/or style to the active pane in ONE request so the pair
    /// survives a single temp-focus window (restore fires after the first
    /// non-temp request, so two separate sends would mis-target the second).
    SetPaneAttrs { title: Option<String>, style: Option<String> },
    // send-keys arguments as SEPARATE tokens (#490): each token is either a
    // named key (Enter, C-c, Up, ...) matched in its entirety or literal
    // text typed verbatim with its whitespace intact. Never re-join and
    // re-split on whitespace — that collapsed runs of spaces inside quoted
    // arguments and stripped leading/trailing spaces.
    SendKeys(Vec<String>, bool),
    /// send-keys -H: hexadecimal operands already decoded to raw bytes,
    /// written to the pane verbatim.
    SendBytes(Vec<u8>),
    SendKeysX(String),  // send-keys -X copy-mode-command
    SelectPane(String, bool),
    SelectWindow(usize),
    ListPanes(mpsc::Sender<String>),
    ListPanesFormat(mpsc::Sender<String>, String),
    ListAllPanes(mpsc::Sender<String>),
    ListAllPanesFormat(mpsc::Sender<String>, String),
    KillWindow,
    /// kill-window with an explicit window target. The server resolves the
    /// target itself and reports an unresolvable one as an error instead of
    /// killing whatever window happens to be active (tmux parity: tmux says
    /// "can't find window: X" and kills nothing). `win` carries an index or,
    /// when `win_is_id` is set, an @id; `name` carries a window-name target.
    KillWindowTarget {
        win: Option<usize>,
        win_is_id: bool,
        name: Option<String>,
        resp: mpsc::Sender<Result<(), String>>,
    },
    KillSession,
    HasSession(mpsc::Sender<bool>),
    RenameSession(String),
    /// Claim a warm server: rename session + send response so CLI knows it's done.
    /// Fields: session name, optional client CWD, optional client priority,
    /// optional path to the client's environment block, optional initial
    /// window name, response sender.
    ///
    /// The priority rides along for the same reason the CWD does (#608): a warm
    /// standby was spawned ahead of time, in an environment that predates the
    /// claiming client, so anything the user set in the shell they ran psmux
    /// from can only reach the already running process through the claim.
    ///
    /// The environment file is the general case of that same problem (#659):
    /// a cold spawned server inherits the client's whole environment block, so
    /// a claimed standby has to be handed it too or it stays poisoned for life.
    ///
    /// The window name is there for the third instance of the same rule (#674):
    /// `new-session -n NAME` names the initial window and disables
    /// automatic-rename for it before the session exists, so applying it inside
    /// this claim, before the OK, is the only way the warm path can match the
    /// cold one. A follow up `rename-window` was observably not equivalent: it
    /// left the session visible under the standby's pool name, and lost the
    /// name entirely whenever that second request did not land.
    ClaimSession(String, Option<String>, Option<String>, Option<String>, Option<String>, mpsc::Sender<String>),
    SwapPane(String),
    /// swap-pane -t <target>: swap the active pane with the pane identified by
    /// (target, pane_is_id).  When `pane_is_id` is true the value is a pane id
    /// (`%N`); otherwise it is a user-facing pane index that is normalized
    /// using pane-base-index before resolving a positional pane path.
    SwapPaneTarget(usize, bool),
    /// swap-pane -s <src> -t <dst>: swap the two explicit panes named by
    /// `-s` and `-t` (issue #442).  Each pane is a (value, is_id) pair
    /// resolved the same way as `SwapPaneTarget`.  `detach` is true when
    /// `-d` was given: the active pane is left unchanged (following its pane
    /// to the new slot); otherwise, per tmux, the `-t` pane becomes active.
    SwapPaneSrcDst { src: usize, src_is_id: bool, dst: usize, dst_is_id: bool, detach: bool },
    /// swap-pane -t <token>: swap the active pane with the pane at a layout
    /// position token (e.g. `{top-right}`).  Layout-independent.
    SwapPanePosition(String),
    ResizePane(String, u16),
    SetBuffer(String),
    /// Set a named buffer: (name, content)
    SetNamedBuffer(String, String),
    ListBuffers(mpsc::Sender<String>),
    ListBuffersFormat(mpsc::Sender<String>, String),
    ShowBuffer(mpsc::Sender<String>),
    /// `None` means no buffer exists at that index (issue #264: lets callers
    /// like paste-buffer distinguish "buffer not found" from "empty buffer").
    ShowBufferAt(mpsc::Sender<Option<String>>, usize),
    /// Show a named buffer by name. `None` means no such named buffer exists.
    ShowNamedBuffer(mpsc::Sender<Option<String>>, String),
    DeleteBuffer,
    DeleteBufferAt(usize),
    /// Delete a named buffer by name
    DeleteNamedBuffer(String),
    PasteBufferAt(usize),
    DisplayMessage(mpsc::Sender<String>, String, Option<usize>, bool, Option<u64>),  // resp, format, target_pane_idx, set_status_bar, duration_override_ms
    /// Like DisplayMessage but resolves -t %N pane ID instead of position. (Issue #332.)
    DisplayMessageById(mpsc::Sender<String>, String, usize, bool, Option<u64>),  // resp, format, pane_id, set_status_bar, duration_override_ms
    LastWindow,
    LastPane,
    /// `rotate-window`. The flag is tmux's `-U` (its default): true moves the
    /// first pane to the last cell, false is `-D`.
    RotateWindow(bool),
    DisplayPanes,
    DisplayPaneSelect(usize),
    BreakPane,
    /// join-pane: move a pane from source window into target window as a split.
    /// Fields: src_win (window index), src_pane (positional pane index), target_win,
    /// target_pane, horizontal (true = -h side-by-side, false = -v stacked).
    JoinPane {
        src_win: Option<usize>,
        src_pane: Option<usize>,
        target_win: Option<usize>,
        target_pane: Option<usize>,
        horizontal: bool,
    },
    /// respawn-pane. Fields: optional workdir (-c), kill flag (-k), command
    /// (`--`/positional shell-command), empty (-E), and the per-request reply.
    ///
    /// The reply channel exists because every way this can fail is ROUTINE and
    /// user-caused (`pane ... still active` without -k, a bad `-c` directory, a
    /// command that will not spawn). Those used to travel out of the server
    /// event loop on a `?` and terminate the whole server — every window and
    /// pane destroyed — while the client still exited 0 with empty output.
    /// tmux answers `respawn pane failed: <cause>` at exit 1 and keeps running.
    RespawnPane(Option<String>, bool, Option<String>, bool, mpsc::Sender<Result<(), String>>),
    /// set-option -p (issue #580): pane-scoped option. Fields: raw -t pane
    /// target ("" = active pane), option name, value ("" = unset via -u/-U),
    /// reply ("" on success, "ERROR: ..." otherwise). Unwired pane options
    /// are rejected loudly instead of stored as silent no-ops.
    SetPaneOption(String, String, String, mpsc::Sender<String>),
    /// show-options -p (issue #580): list a pane's scoped options.
    ShowPaneOptions(String, mpsc::Sender<String>),
    BindKey(String, String, String, bool),  // table, key, command, repeat
    UnbindKey(String, Option<String>),  // key, optional table (None = prefix)
    UnbindAll,
    UnbindAllInTable(String),
    ListKeys(mpsc::Sender<String>),
    SetOption(String, String),
    SetOptionQuiet(String, String, bool),  // set-option with quiet flag
    SetOptionUnset(String),  // set-option -u
    SetOptionAppend(String, String),  // set-option -a
    /// `set-option -o`: apply only when the option is not already set.
    ///
    /// The third field is an OPTIONAL reply channel. tmux fails a `-o` on an
    /// option that is already set (`already set: <name>`, exit 1) and only
    /// swallows it under `-q`, so a caller that wants that answer passes a
    /// channel and gets `""` on success or `ERROR: already set: <name>` on the
    /// refusal. `-q` callers pass `None` and the request stays fire and
    /// forget, which is what every caller used to do unconditionally: the
    /// refusal was invisible on every route (#619 follow up).
    SetOptionOnlyIfUnset(String, String, Option<mpsc::Sender<String>>),  // set-option -o
    SetOptionToggle(String),  // set-option <bool-option> with no value (#535)
    /// Per-window `window-size` override; None unsets the local value.
    SetWindowSize(Option<String>),
    /// `set-option -w [-u|-a|-o] [-t <window>] <name> [value]` (#648).
    ///
    /// `target` is the RAW `-t` string so the server can resolve a window by
    /// name (`s:zero`), by index (`s:1`), by id (`@3`) or by any of tmux's
    /// symbolic spellings. The old window path carried a pre-parsed index and
    /// dropped the name form entirely, which is why the reporter's
    /// `-t "s:zero"` silently acted on the ACTIVE window. Empty means "the
    /// window the client is looking at".
    SetWindowOption {
        target: String,
        option: String,
        value: String,
        unset: bool,
        append: bool,
        only_if_unset: bool,
        quiet: bool,
        resp: mpsc::Sender<String>,
    },
    ShowOptions(mpsc::Sender<String>),
    ShowWindowOptions(mpsc::Sender<String>),
    /// `show-options -w [-A] [-g] [-t <window>]`: the window option listing for
    /// one resolved window.
    ///
    /// The third field says WHICH table to print, the question tmux answers in
    /// `options_scope_from_flags` plus `cmd_show_options_all`: the window's own
    /// values, those plus every inherited value with a `*` marker (`-A`), or the
    /// global window table (`-g`). It used to be a bare "mark inherited" bool,
    /// so `-g` was dropped on the floor and a plain `-w` printed every name
    /// including the ones the window does not own (#648, #655).
    ShowWindowOptionsFor(mpsc::Sender<String>, String, crate::server::options::WindowListing),
    SourceFile(String),
    /// Expand `#{...}` format variables against the live server state and send
    /// the result back: `(format_string, reply)`.
    ///
    /// Connection threads parse commands without access to `AppState` (it is an
    /// owned local of the server loop, not shared behind a lock), so anything
    /// they need format-expanded has to make this round trip. `run-shell` is the
    /// motivating caller: its command was never expanded at all, so a bind like
    /// `run-shell "helper --path '#{pane_current_path}'"` handed the helper that
    /// literal string.
    ExpandFormat(String, mpsc::Sender<String>),
    /// move-window. `src`/`dst` are RAW tmux target specs, resolved on the
    /// server against the live window list by `AppState::resolve_window_spec`.
    ///
    /// It used to carry only the destination as a plain number, so `-s` had
    /// nowhere to go and the handler always moved the ACTIVE window; `-t +1`
    /// arrived as the unsigned 1; and there was no reply channel, so every
    /// refusal (`index in use`, unknown source) exited 0 (issue #602).
    MoveWindow {
        src: Option<String>,
        dst: Option<String>,
        /// -d: leave the current window alone instead of selecting the moved one.
        detach: bool,
        /// -k: kill whatever window already holds the destination index.
        kill: bool,
        /// -r: renumber the session's windows contiguously (ignores src/dst).
        renumber: bool,
        /// -a / -b: insert after / before the destination instead of at it.
        after: bool,
        before: bool,
        resp: mpsc::Sender<Result<(), String>>,
    },
    /// swap-window. `src` (`-s`, default the current window) and `dst` (`-t`)
    /// are RAW tmux target specs. #559: the reply reports "can't find window: N"
    /// when either side does not resolve, so swap-window can exit 1 instead of
    /// silently no-opping (tmux parity).
    SwapWindow {
        src: Option<String>,
        dst: String,
        /// -d. tmux's swap-window selects the destination index only WITH -d
        /// (cmd-swap-window.c `if (args_has(args, 'd'))`); without it the
        /// current window number does not move.
        detach: bool,
        resp: mpsc::Sender<Result<(), String>>,
    },
    /// link-window: (source window index, target insertion index)
    LinkWindow(Option<usize>, Option<usize>),
    UnlinkWindow,
    /// Set session group (used by new-session -t)
    SetSessionGroup(String),
    FindWindow(mpsc::Sender<String>, String),
    /// move-pane: alias for join-pane
    MovePane {
        src_win: Option<usize>,
        src_pane: Option<usize>,
        target_win: Option<usize>,
        target_pane: Option<usize>,
        horizontal: bool,
    },
    /// Extract a pane and start I/O forwarding for cross-session transfer.
    /// Fields: window index, pane index, response channel.
    /// Response: "FORWARD <id> <port> <pid> <title> <rows> <cols> <screen_b64_len>\n<screen_b64>"
    PaneForwardExtract(usize, usize, mpsc::Sender<String>),
    /// Inject a proxy pane from a cross-session transfer.
    /// Fields: source_session, source_addr, source_key, forward_id, fwd_port,
    ///         pid, title, rows, cols, screen_b64, target_window, target_pane, horizontal
    PaneForwardInject {
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
        target_win: Option<usize>,
        target_pane: Option<usize>,
        horizontal: bool,
    },
    /// Resize a forwarded pane's real PTY. Fields: forward_id, rows, cols.
    PaneForwardResize(u64, u16, u16),
    /// Query child status of a forwarded pane. Fields: forward_id, response channel.
    PaneForwardStatus(u64, mpsc::Sender<String>),
    /// Kill a forwarded pane's child process. Fields: forward_id.
    PaneForwardKill(u64),
    /// pipe-pane. Fields: command, -I (stdin), -O (stdout), -o (toggle),
    /// optional response channel. The handler answers "" on acceptance and
    /// "ERROR: ..." when a direct file sink cannot be opened, so the
    /// one-shot CLI can exit non-zero instead of recording a dead pipe with
    /// rc 0 (same shape as #559/#566). Shell-sink spawns are answered
    /// BEFORE spawning (CreateProcess can stall on a cold AV scan); their
    /// failures are reported on the status bar and never recorded.
    PipePane(String, bool, bool, bool, Option<mpsc::Sender<String>>),
    SelectLayout(String),
    NextLayout,
    ListClients(mpsc::Sender<String>),
    ListClientsFormat(mpsc::Sender<String>, String),
    ForceDetachClient(u64),
    /// detach-client -t <tty>: force-detach a client by tty_name (e.g. "/dev/pts/2").
    /// `kill_parent` is the tmux `-P` flag: also tell the client to kill its parent
    /// shell before exiting (issue #275).
    ForceDetachClientByTty(String, bool),
    /// detach-client -a (or no-flag CLI invocation): detach every attached client
    /// of THIS session except the one whose ID is given.  Pass `u64::MAX` from the
    /// CLI one-shot path (no "current" client to exclude).  `kill_parent` honors
    /// the tmux `-P` flag for force-detached clients.
    DetachAllOtherClients(u64, bool),
    /// detach-client -s <session> (where session matches THIS server) or
    /// `psmux detach-client` from CLI: detach every attached client of this session.
    /// `kill_parent` honors the tmux `-P` flag.
    DetachAllClients(bool),
    /// Record, on the given client's registry entry, the session it arrived
    /// FROM (issue #566). Sent by the attach handshake when the client has a
    /// previous session, so `switch-client -l` and `#{client_last_session}`
    /// can answer per client instead of from a machine-wide file.
    SetClientLastSession(u64, String),
    /// switch-client -t <target> / -n / -p / -l: switch the attached client to another session.
    /// The String carries the resolved target session name (or "" for -n/-p/-l to be
    /// resolved server-side), and the second field carries the flag: 't', 'n', 'p', or 'l'.
    ///
    /// The optional channel reports "OK" or "ERROR <reason>" so a one-shot CLI
    /// caller can exit non-zero. Without it, `-l`/`-n`/`-p` were dispatched
    /// fire-and-forget and their only failure report was a TUI status line that
    /// a CLI caller never sees, so "switched", "did nothing" and "switched
    /// somewhere unintended" were all exit 0 with empty output (issue #566).
    /// `None` is for in-process callers that have no one to answer.
    SwitchClient(String, char, Option<mpsc::Sender<String>>),
    /// `switch-client -t <target>` where the target is a full
    /// `session:window.pane` / `@window` / `%pane` spec (#483). The server loop
    /// switches the client's session AND selects the addressed window/pane,
    /// validates the target exists, and reports "OK" or "ERROR <reason>" back on
    /// the channel so the CLI can exit non-zero on an unresolvable target.
    SwitchClientTarget(String, mpsc::Sender<String>),
    LockClient,
    RefreshClient,
    /// `refresh-client -B name:what:format` subscription management.
    ControlSubscribe {
        client_id: u64,
        name: String,
        target: String,
        format: String,
    },
    /// `refresh-client -B name:` remove subscription.
    ControlUnsubscribe {
        client_id: u64,
        name: String,
    },
    /// `refresh-client -f pause-after=N` set pause-after flag.
    ControlSetPauseAfter {
        client_id: u64,
        pause_after_secs: Option<u64>,
    },
    /// `refresh-client -A '%N:continue'` resume paused pane output.
    ControlContinuePane {
        client_id: u64,
        pane_id: usize,
    },
    SuspendClient,
    CopyModePageUp,
    ClearHistory,
    SaveBuffer(String),
    LoadBuffer(String),
    SetEnvironment(String, String),
    UnsetEnvironment(String),
    ShowEnvironment(mpsc::Sender<String>),
    SetHook(String, String),
    AppendHook(String, String),
    ShowHooks(mpsc::Sender<String>),
    RemoveHook(String),
    /// End THIS server only. What a one-shot CLI connection asks for: the CLI
    /// has already worked out the scope and is fanning the request out itself,
    /// so a server that fanned out again would recurse.
    KillServer,
    /// `kill-server` typed by an ATTACHED client: end every server on this
    /// client's socket — its `-L` namespace, or the default one — and then this
    /// one. `true` is `-a`, the psmux-only sweep of every namespace (#649).
    KillServerScoped(bool),
    WaitFor(String, WaitForOp),
    DisplayMenu(String, Option<i16>, Option<i16>),
    DisplayMenuDirect(Menu),
    DisplayPopup(String, String, String, bool, Option<String>),
    ConfirmBefore(String, String),
    ClockMode,
    ResizePaneAbsolute(String, u16),
    ResizePanePercent(String, u8), // axis, percentage (0-100)
    ShowOptionValue(mpsc::Sender<String>, String),
    /// Read a window-scoped option value. The third field is the RAW `-t`
    /// target so the server resolves `s:zero`, `s:1`, `@3` and tmux's
    /// symbolic window spellings alike; empty means the active window.
    /// Required so per-window values — `automatic-rename` (implicitly off for
    /// `-n NAME` windows, #266) and every option in the window's own store
    /// (#648) — are reported instead of the global one.
    ShowWindowOptionValue(mpsc::Sender<String>, String, String),
    ChooseBuffer(mpsc::Sender<String>),
    ServerInfo(mpsc::Sender<String>),
    SendPrefix,
    PrevLayout,
    /// `switch-client -T <table>`. The optional reply channel carries tmux's
    /// "table %s doesn't exist" so a one-shot CLI caller can exit non-zero
    /// (issue #640); the attached client sends its own latch updates over the
    /// persistent connection and passes `None`.
    SwitchClientTable(String, Option<mpsc::Sender<String>>),
    ListCommands(mpsc::Sender<String>),
    ResizeWindow(
        crate::resize_window::ResizeWindowRequest,
        mpsc::Sender<Result<(), String>>,
    ),
    /// Control-mode client viewport update from `refresh-client -C`.
    /// `window_id = None` changes the default; a per-window `size = None`
    /// clears that override.
    ControlClientResize {
        client_id: u64,
        window_id: Option<usize>,
        size: Option<(u16, u16)>,
    },
    /// respawn-window. Fields: optional workdir (-c), optional shell-command
    /// operand, per-request reply. Shares `respawn_active_pane` with
    /// `RespawnPane` and therefore shared its server-killing `?`; the spawn
    /// failures (bad `-c`, unspawnable command) are routine and belong to the
    /// requesting client. tmux: `respawn window failed: <cause>`, exit 1.
    RespawnWindow(Option<String>, Option<String>, mpsc::Sender<Result<(), String>>),
    FocusIn,
    FocusOut,
    CommandPrompt(String),
    ShowMessages(mpsc::Sender<String>),
    /// Forward raw bytes to the popup PTY (base64-decoded by connection handler)
    PopupInput(Vec<u8>),
    /// Close the current overlay (popup, menu, confirm, etc.)
    OverlayClose,
    /// Respond to confirm-before prompt (true = yes, false = no)
    ConfirmRespond(bool),
    /// Select a menu item by index
    MenuSelect(usize),
    /// Navigate menu up/down (delta: -1 = up, +1 = down)
    MenuNavigate(i32),
    /// Show static text in a popup overlay (title, content).
    /// Used by the persistent client command prompt for list-* commands.
    ShowTextPopup(String, String),
    /// Set status bar message (fire-and-forget, no response channel needed).
    StatusMessage(String),
    /// Clear the command prompt history.
    ClearPromptHistory,
    /// Show the command prompt history in a popup.
    ShowPromptHistory(bool),
    /// Register a control mode client.
    ControlRegister {
        client_id: u64,
        echo: bool,
        notif_tx: mpsc::SyncSender<ControlNotification>,
    },
    /// Deregister a control mode client.
    ControlDeregister {
        client_id: u64,
    },
    /// Open customize-mode (interactive options editor)
    CustomizeMode,
    /// Navigate customize-mode (delta: -1 = up, +1 = down)
    CustomizeNavigate(i32),
    /// Begin editing the selected option in customize-mode
    CustomizeEdit,
    /// Update the edit buffer text in customize-mode
    CustomizeEditUpdate(String),
    /// Confirm the edit (apply value) in customize-mode
    CustomizeEditConfirm,
    /// Cancel the edit in customize-mode
    CustomizeEditCancel,
    /// Reset selected option to default in customize-mode
    CustomizeResetDefault,
    /// Set filter string in customize-mode
    CustomizeFilter(String),
    /// Run an arbitrary command through the server-side execute_command_string
    /// path (same path as keybindings and command prompt).  Response channel
    /// carries "OK" on success or an error string.
    RunCommand(String, mpsc::Sender<String>),
}

/// Global flag set by PTY reader threads when new output arrives.
/// The server loop checks this to use a shorter recv_timeout, reducing
/// keystroke-to-display latency for nested shells (e.g. WSL inside pwsh).
pub static PTY_DATA_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sender for the server loop's control channel, published once at server
/// start so threads that have no `AppState` can wake the loop.
///
/// A `mpsc::Sender` is `Send` but not `Sync`, hence the mutex; it is taken only
/// once per coalesced wake, so the lock is never contended in practice.
static WAKE_TX: std::sync::OnceLock<std::sync::Mutex<mpsc::Sender<CtrlReq>>> =
    std::sync::OnceLock::new();

/// True while a `CtrlReq::PtyWake` is queued and not yet consumed.
static WAKE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Publish the control sender. Called once, from the server loop's setup.
pub fn set_wake_sender(tx: mpsc::Sender<CtrlReq>) {
    let _ = WAKE_TX.set(std::sync::Mutex::new(tx));
}

/// Wake the server loop because a pane just published new screen state.
///
/// Without this the loop learns about pty output only when its `recv_timeout`
/// expires, so every echoed keystroke waited out the poll interval on its way
/// to the render, for a hop whose real work is tens of microseconds.
///
/// Measured with `pty_trace`, from the parser publishing a batch (`p`) to the
/// loop having the frame built, 40 single keystrokes into a raw echo child:
///
///   before   1.06ms median, and the echo reached the client as a dump-state
///            RESPONSE (47 of 50 frames) rather than as a push
///   after    0.22ms median, and 49 of 50 frames went out as pushes
///
/// At most ONE wake is ever in flight (`WAKE_PENDING`), so a pane flooding the
/// screen cannot flood the control channel: the first batch queues a wake, and
/// every batch until the loop consumes it is free.
pub fn wake_server_loop() {
    // Nobody is watching: the frame this would produce has no destination, and
    // the loop's own 16ms/50ms cadence still picks the data up for alerts and
    // activity. Waking here would only spend CPU on a detached session.
    if !has_frame_receivers() {
        return;
    }
    if WAKE_PENDING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let sent = WAKE_TX
        .get()
        .and_then(|m| m.lock().ok().map(|tx| tx.send(CtrlReq::PtyWake).is_ok()))
        .unwrap_or(false);
    if !sent {
        // No loop to wake (not the server process, or it is shutting down).
        // Clear the latch so a later wake is not swallowed forever.
        WAKE_PENDING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Called by the server loop when it takes the wake off the channel, which
/// re-arms `wake_server_loop`.
pub fn clear_wake_pending() {
    WAKE_PENDING.store(false, std::sync::atomic::Ordering::Release);
}

/// Set by the parser thread when any pane's `cpr_pending` flag is raised.
/// Lets the server loop skip the tree walk when no CPR response is needed.
pub static CPR_DATA_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Issue #473: set by the parser thread when any pane's `color_query_pending`
/// bitmask is raised.  Lets the server loop skip the tree walk when no color
/// query response is needed.
pub static COLOR_QUERY_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Issue #473: the host terminal color spec captured by the client at startup
/// (before the input pump starts), consumed by `establish_connection` which
/// reports it to the server on every (re)connect.  `None` inside means the
/// query ran but the host reported nothing usable.
pub static HOST_COLORS_SPEC: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Bit assignments for `Pane::color_query_pending` (issue #473).
/// Bits 0-15 are the OSC 4 palette indexes.
pub const COLOR_QUERY_FG: u32 = 1 << 16;   // OSC 10;?
pub const COLOR_QUERY_BG: u32 = 1 << 17;   // OSC 11;?
pub const COLOR_QUERY_SCHEME: u32 = 1 << 18; // CSI ?996n

/// Issue #556: process-wide snapshot of `AppState::host_colors`, readable from
/// pane reader threads so color queries can be answered synchronously at
/// detection instead of waiting for a server-loop tick.  `None` means no
/// client has reported host colors yet — `shared_host_colors()` then falls
/// back to the `PSMUX_HOST_COLORS` override / Campbell defaults, the same
/// resolution the server loop applies to `app.host_colors`.
pub static HOST_COLORS_SHARED: std::sync::Mutex<Option<HostColors>> = std::sync::Mutex::new(None);

/// Resolve the colors to answer pane color queries with, from any thread.
pub fn shared_host_colors() -> HostColors {
    if let Ok(g) = HOST_COLORS_SHARED.lock() {
        if let Some(hc) = g.as_ref() { return hc.clone(); }
    }
    std::env::var("PSMUX_HOST_COLORS").ok()
        .map(|s| HostColors::from_spec(&s))
        .filter(|hc| hc.has_any() || hc.dark.is_some())
        .unwrap_or_else(HostColors::campbell)
}

/// Publish the latest client-reported host colors for reader threads.
pub fn set_shared_host_colors(hc: Option<HostColors>) {
    if let Ok(mut g) = HOST_COLORS_SHARED.lock() { *g = hc; }
}

/// Issue #473: the host terminal's colors, as reported by an attached client
/// (which queries its host terminal with OSC 10/11/4 at attach time), or the
/// `PSMUX_HOST_COLORS` environment override.  Used to answer terminal color
/// queries issued by pane applications.  All values are RGB triples.
#[derive(Clone, Debug, PartialEq)]
pub struct HostColors {
    pub fg: Option<(u8, u8, u8)>,
    pub bg: Option<(u8, u8, u8)>,
    pub palette: [Option<(u8, u8, u8)>; 16],
    /// Some(true) = dark scheme, Some(false) = light. None = derive from bg.
    pub dark: Option<bool>,
}

impl HostColors {
    pub fn empty() -> Self {
        Self { fg: None, bg: None, palette: [None; 16], dark: None }
    }

    /// True when enough colors are known to be worth reporting.
    pub fn has_any(&self) -> bool {
        self.fg.is_some() || self.bg.is_some() || self.palette.iter().any(|p| p.is_some())
    }

    /// Windows Terminal "Campbell" defaults, used when no host colors are known.
    /// A valid (if generic) palette beats no reply: applications at least get a
    /// well-formed response instead of timing out.
    pub fn campbell() -> Self {
        Self {
            fg: Some((0xCC, 0xCC, 0xCC)),
            bg: Some((0x0C, 0x0C, 0x0C)),
            palette: [
                Some((0x0C, 0x0C, 0x0C)), Some((0xC5, 0x0F, 0x1F)),
                Some((0x13, 0xA1, 0x0E)), Some((0xC1, 0x9C, 0x00)),
                Some((0x00, 0x37, 0xDA)), Some((0x88, 0x17, 0x98)),
                Some((0x3A, 0x96, 0xDD)), Some((0xCC, 0xCC, 0xCC)),
                Some((0x76, 0x76, 0x76)), Some((0xE7, 0x48, 0x56)),
                Some((0x16, 0xC6, 0x0C)), Some((0xF9, 0xF1, 0xA5)),
                Some((0x3B, 0x78, 0xFF)), Some((0xB4, 0x00, 0x9E)),
                Some((0x61, 0xD6, 0xD6)), Some((0xF2, 0xF2, 0xF2)),
            ],
            dark: Some(true),
        }
    }

    /// True when the scheme is dark.  Uses the explicit `dark` flag when the
    /// host reported one (CSI ?997 response), else relative luminance of bg.
    pub fn is_dark(&self) -> bool {
        if let Some(d) = self.dark { return d; }
        match self.bg {
            Some((r, g, b)) => {
                // ITU-R BT.709 relative luminance, 0-255 scale.
                let lum = 0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64;
                lum < 128.0
            }
            None => true,
        }
    }

    /// Serialize to the compact single-token wire form used by the client's
    /// `host-colors` control line: `fg=RRGGBB,bg=RRGGBB,0=RRGGBB,...,dark=1`.
    pub fn to_spec(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some((r, g, b)) = self.fg { parts.push(format!("fg={:02x}{:02x}{:02x}", r, g, b)); }
        if let Some((r, g, b)) = self.bg { parts.push(format!("bg={:02x}{:02x}{:02x}", r, g, b)); }
        for (i, p) in self.palette.iter().enumerate() {
            if let Some((r, g, b)) = p { parts.push(format!("{}={:02x}{:02x}{:02x}", i, r, g, b)); }
        }
        if let Some(d) = self.dark { parts.push(format!("dark={}", if d { 1 } else { 0 })); }
        parts.join(",")
    }

    /// Parse the wire form produced by `to_spec`.  Unknown keys are ignored.
    pub fn from_spec(spec: &str) -> Self {
        let mut hc = Self::empty();
        for part in spec.split(',') {
            let Some((key, val)) = part.split_once('=') else { continue };
            if key == "dark" {
                hc.dark = match val { "1" => Some(true), "0" => Some(false), _ => None };
                continue;
            }
            let Some(rgb) = parse_hex_rgb(val) else { continue };
            match key {
                "fg" => hc.fg = Some(rgb),
                "bg" => hc.bg = Some(rgb),
                _ => {
                    if let Ok(i) = key.parse::<usize>() {
                        if i < 16 { hc.palette[i] = Some(rgb); }
                    }
                }
            }
        }
        hc
    }
}

/// Parse `RRGGBB` (6 hex digits, no `#`).
pub fn parse_hex_rgb(s: &str) -> Option<(u8, u8, u8)> {
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Parse an X11-style color reply payload: `rgb:RR/GG/BB`, `rgb:RRRR/GGGG/BBBB`
/// (1-4 hex digits per channel, scaled to 8-bit), or `#RRGGBB`.
pub fn parse_x11_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_rgb(hex);
    }
    let body = s.strip_prefix("rgb:")?;
    let mut chans = body.split('/');
    let mut out = [0u8; 3];
    for slot in out.iter_mut() {
        let c = chans.next()?;
        if c.is_empty() || c.len() > 4 || !c.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
        let v = u16::from_str_radix(c, 16).ok()?;
        // Scale to 8-bit based on the number of digits given.
        let max = (16u32.pow(c.len() as u32) - 1) as u32;
        *slot = ((v as u32 * 255 + max / 2) / max) as u8;
    }
    if chans.next().is_some() { return None; }
    Some((out[0], out[1], out[2]))
}

/// Issue #440: `pipe-pane` output routing.
///
/// A pane's PTY reader thread tees every raw output chunk to any pipe writer
/// registered under its `pane_id`, so `pipe-pane -o '<cmd>'` actually receives
/// the pane transcript on the child's stdin (previously the child was spawned
/// with a piped stdin that nothing ever wrote to, so it blocked on an empty
/// pipe forever and the sink stayed 0 bytes).
///
/// `PIPE_PANE_COUNT` is a cheap gate: it lets every reader thread skip the mutex
/// entirely in the overwhelmingly common case where no pipe is active, so panes
/// that are not being piped pay nothing. The server handler (`CtrlReq::PipePane`)
/// pushes/removes `(pane_id, child_stdin)` entries and keeps the count in sync.
pub static PIPE_PANE_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Writers are boxed so the same tee path serves both pipe-pane child stdins
/// and cross-session forward tunnels (TcpStream) without a second reader
/// competing for the ConPTY output pipe.
pub static PIPE_WRITERS: Mutex<Vec<(usize, Box<dyn std::io::Write + Send>)>> = Mutex::new(Vec::new());

/// Issue #685: the low sixteen OSC 4 palette entries of every pane that has
/// set any, mirrored out of the pane's `vt100::Screen` so the colour query
/// responder can read them without taking the parser lock.
///
/// tmux answers an `OSC 4;<i>;?` query from the pane's own palette first
/// (`input.c:2947` `colour_palette_get`) and only asks the real terminal when
/// the pane has no entry of its own (`input_add_request`,
/// `INPUT_REQUEST_PALETTE`).  psmux's responder lives in the pane READER
/// thread (`pane.rs` `scan_color_queries` plus
/// `server::helpers::answer_color_queries_sync`, issues #473 and #556), which
/// runs ahead of the parser thread and must not block on it, so the parser
/// thread publishes here after each batch that changed the palette.
///
/// `PANE_PALETTE_ANY` is the same cheap gate `PIPE_PANE_COUNT` is: while no
/// pane in this server has ever set a palette entry, the responder never takes
/// the mutex, so nothing about the existing path changes.  Only indexes 0..=15
/// are mirrored because that is the whole range the query scanner recognises.
pub static PANE_PALETTE_ANY: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static PANE_PALETTES: Mutex<Vec<(usize, [Option<(u8, u8, u8)>; 16])>> =
    Mutex::new(Vec::new());

/// Publish (or, with an all-`None` table, withdraw) one pane's mirrored
/// palette.  Called from the pane parser thread only when the palette
/// generation moved.
pub fn publish_pane_palette(pane_id: usize, entries: [Option<(u8, u8, u8)>; 16]) {
    let any = entries.iter().any(Option::is_some);
    let Ok(mut table) = PANE_PALETTES.lock() else { return };
    match table.iter_mut().find(|(id, _)| *id == pane_id) {
        Some(slot) if any => slot.1 = entries,
        Some(_) => {
            table.retain(|(id, _)| *id != pane_id);
        }
        None if any => table.push((pane_id, entries)),
        None => {}
    }
    PANE_PALETTE_ANY.store(!table.is_empty(), std::sync::atomic::Ordering::Release);
}

/// This pane's mirrored palette, if it has one.
#[must_use]
pub fn pane_palette(pane_id: usize) -> Option<[Option<(u8, u8, u8)>; 16]> {
    if !PANE_PALETTE_ANY.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let table = PANE_PALETTES.lock().ok()?;
    table.iter().find(|(id, _)| *id == pane_id).map(|(_, e)| *e)
}

/// Tracked persistent client TCP streams.
/// Connection handlers register clones here so the server can explicitly
/// `shutdown()` them before `process::exit(0)`.  Without this, Windows
/// does not reliably deliver TCP RST on loopback sockets when a process
/// exits, leaving the client's blocking `read_line()` stuck forever.
static PERSISTENT_STREAMS: std::sync::Mutex<Vec<(u64, std::net::TcpStream)>> = std::sync::Mutex::new(Vec::new());

/// Register a persistent client stream tagged with client_id (call from connection handler).
pub fn register_persistent_stream(client_id: u64, stream: &std::net::TcpStream) {
    if let Ok(cloned) = stream.try_clone() {
        if let Ok(mut v) = PERSISTENT_STREAMS.lock() {
            v.push((client_id, cloned));
        }
    }
}

/// Remove a specific client's entry from PERSISTENT_STREAMS without shutting
/// it down. Called by the writer-thread Guard on normal disconnect — the socket
/// is already shut down via ws_shutdown at that point, so we only need to drop
/// the dead clone from the Vec to prevent unbounded accumulation.
pub fn deregister_persistent_stream(client_id: u64) {
    if let Ok(mut v) = PERSISTENT_STREAMS.lock() {
        v.retain(|(cid, _)| *cid != client_id);
    }
}

/// Shut down all tracked persistent client streams so their readers get EOF.
pub fn shutdown_persistent_streams() {
    if let Ok(mut v) = PERSISTENT_STREAMS.lock() {
        for (_, s) in v.drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Shut down a specific client's persistent stream and remove its frame sender.
/// Used by force-detach to disconnect a targeted client.
pub fn shutdown_client_stream(client_id: u64) {
    if let Ok(mut v) = PERSISTENT_STREAMS.lock() {
        v.retain(|(cid, s)| {
            if *cid == client_id {
                let _ = s.shutdown(std::net::Shutdown::Both);
                false
            } else {
                true
            }
        });
    }
    if let Ok(mut v) = FRAME_PUSH_SLOTS.lock() {
        v.retain(|(cid, _, _)| *cid != client_id);
    }
    remove_directive_channel(client_id);
}

/// Server-push frame slot for persistent (attached) clients.
///
/// Each slot holds at most one pending frame. `push_frame()` overwrites any
/// unconsumed frame; frames are full snapshots, so a stale ready frame has
/// no value once a newer one exists. Memory is bounded to O(clients), not
/// O(frames).
///
/// The slot uses `Mutex<Option<String>>`. The producer (main loop's
/// `push_frame`) locks, replaces, unlocks. The consumer (writer thread)
/// locks, takes, unlocks, then writes to TCP *outside* the lock. Neither
/// side ever holds the lock across blocking I/O, so a slow client cannot
/// stall the main event loop.
///
/// Design constraints:
///   - The lock must not be held across blocking I/O. The writer thread
///     does TCP writes that can block (slow client, full kernel buffer).
///     A design where the writer holds a lock during the write -- and the
///     producer also takes that lock to enqueue -- lets a slow client
///     stall the server main loop.
///   - Per-client storage must be bounded. An unbounded queue (e.g. plain
///     `mpsc::channel`) leaks memory under sustained producer-faster-than-
///     consumer load (rapid copy-mode scroll).
///   - `std::sync::atomic` has no atomic-swap for owned heap values;
///     `AtomicPtr<String>` would require `unsafe` ownership management.
///     `arc-swap` would be lock-free but adds a third-party dependency
///     for a path that is not measured-hot.
pub type FrameSlot = std::sync::Arc<std::sync::Mutex<Option<String>>>;

/// What can wake a persistent connection's writer thread.
///
/// The writer has two producers, command responses and pushed frames, and an
/// `mpsc::Receiver` can only block on one channel. Before this enum the frame
/// slot was the producer it could NOT block on, so every pushed frame waited
/// out the response channel's 5ms `recv_timeout`, for a hop whose work is one
/// `write!`. Carrying both on one channel makes the writer wake on whichever
/// arrives first.
///
/// Measured with `pty_trace`, from the push filling the slot (`f`) to the
/// writer putting the frame on the socket (`t`): 3.20ms median before (n=3,
/// because before this change almost nothing took the push path at all),
/// 0.068ms median after (n=49).
pub enum WriterWake {
    /// A command response is coming; the receiver carries its text.
    Resp(mpsc::Receiver<String>),
    /// A frame was pushed into this client's slot. Carries nothing, because
    /// the slot always holds the newest snapshot and the writer reads it.
    Frame,
}

static FRAME_PUSH_SLOTS: std::sync::Mutex<Vec<(u64, FrameSlot, mpsc::Sender<WriterWake>)>> =
    std::sync::Mutex::new(Vec::new());

/// Register a frame slot for a persistent connection's writer thread.
///
/// `poke` is the writer's own wake channel, used to tell it a frame landed.
/// Returns the slot Arc for the writer thread to consume from.
pub fn register_frame_channel(client_id: u64, poke: mpsc::Sender<WriterWake>) -> FrameSlot {
    let slot: FrameSlot = std::sync::Arc::new(std::sync::Mutex::new(None));
    if let Ok(mut v) = FRAME_PUSH_SLOTS.lock() {
        v.push((client_id, slot.clone(), poke));
    }
    slot
}

/// Push a serialized frame to all persistent clients.
/// Overwrites any unconsumed frame; frames are full snapshots, so only
/// the latest matters. The lock is held only for the duration of an
/// Option::replace, never across I/O.
/// Dead slots (poisoned mutex) are pruned automatically.
pub fn push_frame(frame: &str) {
    if let Ok(mut slots) = FRAME_PUSH_SLOTS.lock() {
        slots.retain(|(_, slot, poke)| {
            match slot.lock() {
                Ok(mut s) => {
                    // Only an empty->full transition needs a poke. If a frame
                    // is already waiting the writer has not drained it yet, so
                    // it is either running or already has a wake queued: this
                    // is what keeps a flooding pane from queueing one message
                    // per batch.
                    let was_empty = s.is_none();
                    *s = Some(frame.to_string());
                    drop(s);
                    if was_empty {
                        let _ = poke.send(WriterWake::Frame);
                    }
                    true
                }
                Err(_) => false, // writer thread panicked; prune
            }
        });
    }
}

/// Check if any persistent clients are registered for push.
pub fn has_frame_receivers() -> bool {
    FRAME_PUSH_SLOTS.lock().map_or(false, |v| !v.is_empty())
}

/// Remove the frame slot for a specific client. Called by the writer thread
/// on exit so the server stops pushing to dead slots and has_frame_receivers()
/// returns false when no live clients remain.
pub fn deregister_frame_channel(client_id: u64) {
    if let Ok(mut v) = FRAME_PUSH_SLOTS.lock() {
        v.retain(|(cid, _, _)| *cid != client_id);
    }
}

/// Per-client directive channels (queued, not overwritten like frame slots).
/// Used for sending commands/directives (e.g. SWITCH) to specific persistent clients
/// without risk of being overwritten by frame pushes.
static DIRECTIVE_CHANNELS: std::sync::Mutex<Vec<(u64, std::sync::mpsc::Sender<String>)>> =
    std::sync::Mutex::new(Vec::new());

/// Register a directive channel for a persistent client. Returns the receiver
/// for the writer thread to poll.
pub fn register_directive_channel(client_id: u64) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    if let Ok(mut v) = DIRECTIVE_CHANNELS.lock() {
        v.push((client_id, tx));
    }
    rx
}

/// Send a directive to a specific persistent client. Returns true if sent.
pub fn send_directive_to_client(client_id: u64, directive: &str) -> bool {
    if let Ok(channels) = DIRECTIVE_CHANNELS.lock() {
        for (cid, tx) in channels.iter() {
            if *cid == client_id {
                return tx.send(directive.to_string()).is_ok();
            }
        }
    }
    false
}

/// Send a directive to ALL persistent clients.
pub fn send_directive_to_all_clients(directive: &str) {
    if let Ok(channels) = DIRECTIVE_CHANNELS.lock() {
        for (_, tx) in channels.iter() {
            let _ = tx.send(directive.to_string());
        }
    }
}

/// Remove a client's directive channel (called on disconnect).
pub fn remove_directive_channel(client_id: u64) {
    if let Ok(mut v) = DIRECTIVE_CHANNELS.lock() {
        v.retain(|(cid, _)| *cid != client_id);
    }
}

/// Global counter shared by interactive and control-mode clients.
static NEXT_CLIENT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Allocate a process-wide unique client ID.
pub fn next_client_id() -> u64 {
    NEXT_CLIENT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Wait-for operation types
#[derive(Clone, Copy)]
pub enum WaitForOp {
    Wait,
    Lock,
    Signal,
    Unlock,
}

/// Parsed target specification from -t argument.
#[derive(Debug, Clone, Default)]
pub struct ParsedTarget {
    pub session: Option<String>,
    pub window: Option<usize>,
    pub window_name: Option<String>,
    pub pane: Option<usize>,
    pub pane_is_id: bool,
    pub window_is_id: bool,
}

#[cfg(test)]
#[path = "../tests-rs/test_pr267_backpressure_proof.rs"]
mod tests_pr267_backpressure;

#[cfg(test)]
#[path = "../tests-rs/test_issue434_reap_client.rs"]
mod tests_issue434_reap_client;

#[cfg(test)]
#[path = "../tests-rs/test_kill_descendants_option.rs"]
mod tests_kill_descendants_option;

#[cfg(test)]
#[path = "../tests-rs/test_issue450_heal_option.rs"]
mod tests_issue450_heal_option;

#[cfg(test)]
#[path = "../tests-rs/test_base_index_rebase.rs"]
mod tests_base_index_rebase;

#[cfg(test)]
#[path = "../tests-rs/test_issue601_602_move_swap_window.rs"]
mod tests_issue601_602_move_swap_window;

#[cfg(test)]
#[path = "../tests-rs/test_preview_window_state.rs"]
mod tests_preview_window_state;
