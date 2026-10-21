//! The shared server process (#798 step 2): one per namespace, holding every
//! session of the namespace, each an `AppState` of its own.
//!
//! Shape, and why it is not run_server on a thread per session: run_server is
//! written for a process that IS one session (it exits the process on
//! kill-session, pushes frames to every client of the process, sets the
//! process environment and directory for its session, owns the process's one
//! wake channel). Making it reentrant would change the default path, which
//! the owner ruling forbids. So the shared server has its own loop, here, and
//! calls the per session building blocks unchanged:
//!
//! * every session gets its own loopback listener and its own registry files,
//!   exactly as a per session server writes them (see `registry`), and each
//!   accepted connection goes to the per session protocol handler,
//!   `connection::handle_connection`, unchanged, with that session's channel;
//! * one forwarding thread per session tags its requests with the session
//!   and hands them to the one loop thread, which runs them one at a time;
//! * before a request runs, the loop "enters" the session: it lends the
//!   server wide id counters to the session (`ids`), and points the process
//!   wide things a pane child inherits (PSMUX_TARGET_SESSION, the job
//!   environment's $TMUX, the working directory) at that session, so a pane
//!   spawned for session B never gets session A's values;
//! * each request runs inside `catch_unwind`: a panic ends that one session
//!   (its panes, files and clients) and the others keep running.
//!
//! The server exits when its last session ends (tmux with exit-empty), or on
//! kill-server. It never uses the per session `__warm__` standby and keeps no
//! spare panes in step 2.

use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::super::*;
use super::ids::ServerIds;
use super::reply::{self, Outcome};
use super::{registry, router};

static IN_SHARED_SERVER: AtomicBool = AtomicBool::new(false);

/// True inside the shared server process.
pub(crate) fn in_shared_server() -> bool {
    IN_SHARED_SERVER.load(Ordering::Relaxed)
}

/// The session whose request is running, for the panic hook's report.
static CURRENT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// What a request asks the loop to do once it has run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flow {
    Continue,
    /// End this session (kill-session, destroy-unattached).
    EndSession,
    /// End the server. `scoped_all` is an attached client's kill-server
    /// (`Some(-a)`), which also ends the other servers of the namespace.
    KillServer { scoped_all: Option<bool> },
}

/// run_server's per session loop locals, kept per session.
pub(crate) struct SessionLoop {
    pub state_dirty: bool,
    pub meta_dirty: bool,
    pub echo_pending_until: Option<Instant>,
    pub temp_focus_restore: Option<temp_target::SavedFocus>,
    pub dump_state_seen_full: HashSet<u64>,
    pub frame: super::frame::FrameCache,
    /// Attached (persistent) clients of this session: the only receivers of
    /// its pushed frames.
    pub clients: HashSet<u64>,
    pub last_push: Instant,
    /// The session's command aliases, shared with its connection threads
    /// (run_server's `shared_aliases_main`).
    pub aliases: Aliases,
    last_alert_check: Instant,
    last_reap: Instant,
    last_registry_check: Instant,
}

type Aliases = Arc<std::sync::RwLock<std::collections::HashMap<String, String>>>;


impl SessionLoop {
    fn new(aliases: Aliases) -> Self {
        let now = Instant::now();
        SessionLoop {
            aliases,
            state_dirty: true,
            meta_dirty: true,
            echo_pending_until: None,
            temp_focus_restore: None,
            dump_state_seen_full: HashSet::new(),
            frame: super::frame::FrameCache::new(),
            clients: HashSet::new(),
            last_push: now,
            last_alert_check: now,
            last_reap: now,
            last_registry_check: now,
        }
    }
}

struct Session {
    app: AppState,
    lp: SessionLoop,
    /// The directory a pane of this session starts in when no `-c` says
    /// otherwise: the session's `-c`, or the directory the creating CLI ran in.
    cwd: Option<std::path::PathBuf>,
    port: u16,
    stop: Arc<AtomicBool>,
    _guard: Option<crate::platform::SessionMutex>,
}

use super::router::NewSessionArgs;

enum ControlCmd {
    NewSession(Box<NewSessionArgs>),
    Ping,
}

enum Msg {
    Req(usize, CtrlReq),
    Wake,
    Control(ControlCmd, mpsc::Sender<Outcome>),
}

/// How a connection thread reaches another session of the server: for
/// applying a server wide write (`router::is_global_write`) to every session.
#[derive(Clone)]
struct Peer {
    sid: usize,
    name: String,
    port: u16,
    key: String,
}

/// Deliver one server wide write to a session through its own listener,
/// exactly as a client sends it, and wait for the reply. Registered as an
/// echo first, so the session runs it without propagating it again.
fn deliver(globals: &Globals, p: &Peer, line: &str) {
    globals.expect_echo(p.sid, line);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], p.port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = write!(s, "AUTH {}\n{}\n", p.key, line);
    let _ = s.flush();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 4096];
    while let Ok(k) = io::Read::read(&mut s, &mut sink) {
        if k == 0 {
            break;
        }
    }
}

/// What every session's connection threads share about the server.
#[derive(Default)]
struct Globals {
    peers: std::sync::RwLock<Vec<Peer>>,
    /// Server wide writes so far, replayed on every session created later.
    replay: std::sync::Mutex<Vec<String>>,
    /// Writes this server itself is delivering to a session (a propagation,
    /// a replay, or `execute_command_string` handing a `set` to its own
    /// listener): that session must run them, not propagate them again.
    /// Entries older than ECHO_TTL are dropped.
    echoes: std::sync::Mutex<Vec<(usize, String, Instant)>>,
}

const ECHO_TTL: Duration = Duration::from_secs(10);

impl Globals {
    fn expect_echo(&self, sid: usize, line: &str) {
        if let Ok(mut e) = self.echoes.lock() {
            e.retain(|(_, _, at)| at.elapsed() < ECHO_TTL);
            e.push((sid, line.trim().to_string(), Instant::now()));
        }
    }

    fn take_echo(&self, sid: usize, line: &str) -> bool {
        let Ok(mut e) = self.echoes.lock() else { return false };
        e.retain(|(_, _, at)| at.elapsed() < ECHO_TTL);
        match e.iter().position(|(s, l, _)| *s == sid && l == line.trim()) {
            Some(i) => {
                e.remove(i);
                true
            }
            None => false,
        }
    }
}

/// `psmux server --shared-server [-L ns]`. Never returns.
pub(crate) fn run_from_args() -> ! {
    let args: Vec<String> = std::env::args().collect();
    let ns = args.iter().position(|a| a == "-L").and_then(|i| args.get(i + 1)).cloned();
    run(ns)
}

fn random_key() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64,
    );
    h.write_u64(std::process::id() as u64);
    format!("{:016x}", h.finish())
}

#[cfg(windows)]
fn clear_inherit_socket(s: &impl std::os::windows::io::AsRawSocket) {
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(h: *mut core::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    unsafe {
        SetHandleInformation(s.as_raw_socket() as *mut core::ffi::c_void, 1, 0);
    }
}
#[cfg(not(windows))]
fn clear_inherit_socket<T>(_s: &T) {}

fn namespace_label(ns: &Option<String>) -> String {
    match ns {
        Some(n) => format!("shared-server -L {}", n),
        None => "shared-server".to_string(),
    }
}

fn run(ns: Option<String>) -> ! {
    IN_SHARED_SERVER.store(true, Ordering::Relaxed);
    #[cfg(windows)]
    crate::platform::mouse_inject::SERVER_PROCESS.store(true, Ordering::Relaxed);
    let label = namespace_label(&ns);
    {
        // Log only: a caught panic ends one session (see `run_request`), and
        // removing registry files here, as run_server's hook does, would
        // unpublish every session of the server.
        let label = label.clone();
        std::panic::set_hook(Box::new(move |info| {
            let current = CURRENT.lock().map(|g| g.clone()).unwrap_or_default();
            let bt = std::backtrace::Backtrace::force_capture();
            append_data_dir_report(
                "crash.log",
                &label,
                &format!("session {current:?}: {info}\n\nBacktrace:\n{bt}"),
            );
        }));
    }
    crate::platform::clear_inherited_ctrl_c_ignore();
    install_console_ctrl_handler();
    let _ = SERVER_NAMESPACE.set(ns.clone());
    let _ = crate::socket_path::server_recorded_path(ns.as_deref());
    let priority = crate::platform::resolve_priority(None, true);
    crate::platform::set_process_priority(&priority);

    // One shared server per namespace and data directory.
    let mutex_name = format!("{}@shared-server", ns.as_deref().unwrap_or(""));
    let _server_guard = match crate::platform::acquire_session_mutex(&mutex_name) {
        Some(g) => g,
        None => std::process::exit(0),
    };
    let dir = crate::paths::psmux_dir();
    let _ = std::fs::create_dir_all(&dir);

    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    // The pane parsers wake "the server loop" through one process wide
    // sender; here it wakes this loop.
    {
        let (wtx, wrx) = mpsc::channel::<CtrlReq>();
        crate::types::set_wake_sender(wtx);
        let fwd = msg_tx.clone();
        thread::spawn(move || {
            for _ in wrx {
                if fwd.send(Msg::Wake).is_err() {
                    break;
                }
            }
        });
    }

    // The control listener: where a CLI asks this server for a new session.
    let control = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("psmux: shared server: {e}");
            std::process::exit(1);
        }
    };
    let control_port = control.local_addr().map(|a| a.port()).unwrap_or(0);
    let control_key = random_key();
    let pid = std::process::id();
    let marker_path = registry::marker_path(ns.as_deref());
    let marker = registry::Marker {
        pid,
        creation: crate::platform::process_kill::process_creation_time(pid).unwrap_or(0),
        port: control_port,
        key: control_key.clone(),
    };
    let globals: Arc<Globals> = Arc::new(Globals::default());
    {
        let tx = msg_tx.clone();
        let key = control_key.clone();
        let globals = globals.clone();
        thread::spawn(move || {
            for conn in control.incoming() {
                let Ok(stream) = conn else { continue };
                clear_inherit_socket(&stream);
                let tx = tx.clone();
                let key = key.clone();
                let globals = globals.clone();
                thread::spawn(move || serve_control(stream, &key, tx, &globals));
            }
        });
    }
    if let Err(e) = registry::write_marker(&marker_path, &marker) {
        eprintln!("psmux: shared server: cannot write {}: {e}", marker_path.display());
        std::process::exit(1);
    }

    let pty_system = native_pty_system();
    let mut ids = ServerIds::new();
    let mut sessions: BTreeMap<usize, Session> = BTreeMap::new();
    let mut had_session = false;
    let started = Instant::now();
    let mut last_client_activity = Instant::now();

    loop {
        let data_ready = crate::types::PTY_DATA_READY.swap(false, Ordering::AcqRel);
        if data_ready {
            let cpr = crate::types::CPR_DATA_PENDING.swap(false, Ordering::AcqRel);
            let color = crate::types::COLOR_QUERY_PENDING.swap(false, Ordering::AcqRel);
            for s in sessions.values_mut() {
                s.lp.state_dirty = true;
                data_ready_work(&mut s.app, cpr, color);
            }
        }
        if crate::types::DEVICE_REPLY_PENDING.swap(false, Ordering::AcqRel) {
            for s in sessions.values_mut() {
                device_reply_work(&mut s.app);
            }
            crate::types::rearm_device_replies();
        }
        let echo_active = sessions
            .values()
            .any(|s| s.lp.echo_pending_until.is_some_and(|t| t.elapsed().as_millis() < 50));
        crate::timer_res::set_high(crate::types::has_frame_receivers());
        let timeout_ms: u64 = if echo_active || data_ready {
            1
        } else if last_client_activity.elapsed().as_secs() < 2 {
            5
        } else if crate::types::has_frame_receivers() {
            16
        } else {
            50
        };
        let mut pending: Vec<Msg> = Vec::new();
        if let Ok(m) = msg_rx.recv_timeout(Duration::from_millis(timeout_ms)) {
            pending.push(m);
            while let Ok(m) = msg_rx.try_recv() {
                pending.push(m);
            }
        }
        // Keys and commands before frames, as run_server orders them.
        pending.sort_by_key(|m| match m {
            Msg::Req(_, CtrlReq::DumpState(..)) => 1,
            _ => 0,
        });
        let mut ended: Vec<(usize, &'static str)> = Vec::new();
        for m in pending {
            match m {
                Msg::Wake => crate::types::clear_wake_pending(),
                Msg::Req(sid, req) => {
                    if !matches!(req, CtrlReq::PtyWake | CtrlReq::DumpState(..)) {
                        last_client_activity = Instant::now();
                    }
                    if matches!(req, CtrlReq::PtyWake) {
                        crate::types::clear_wake_pending();
                    }
                    let Some(s) = sessions.get_mut(&sid) else { continue };
                    match run_request(s, &mut ids, &ns, |app, lp| {
                        super::dispatch::dispatch(app, lp, req, &*pty_system)
                    }) {
                        Flow::Continue => {}
                        Flow::EndSession => ended.push((sid, "kill-session")),
                        Flow::KillServer { scoped_all } => {
                            shutdown(&mut sessions, &marker_path, pid, &ns, scoped_all)
                        }
                    }
                }
                Msg::Control(cmd, resp) => {
                    let outcome = match cmd {
                        ControlCmd::Ping => Outcome::Done(String::new()),
                        ControlCmd::NewSession(args) => {
                            match create_session(&args, &mut ids, &ns, &msg_tx, &*pty_system, &globals) {
                                Ok(s) => {
                                    let sid = s.app.session_id;
                                    let text = new_session_output(&s.app, &args);
                                    sessions.insert(sid, s);
                                    had_session = true;
                                    Outcome::Done(text)
                                }
                                Err(msg) => Outcome::Failed(msg),
                            }
                        }
                    };
                    let _ = resp.send(outcome);
                }
            }
        }
        // Per session upkeep: alerts, reaping exited panes, the registry
        // self heal, frames for attached clients.
        let sids: Vec<usize> = sessions.keys().copied().collect();
        for sid in sids {
            if ended.iter().any(|(e, _)| *e == sid) {
                continue;
            }
            let Some(s) = sessions.get_mut(&sid) else { continue };
            let flow = run_request(s, &mut ids, &ns, |app, lp| upkeep(app, lp, &*pty_system));
            if flow == Flow::EndSession {
                ended.push((sid, "exit-empty"));
            }
        }
        for (sid, _why) in ended {
            if let Some(s) = sessions.remove(&sid) {
                if let Ok(mut p) = globals.peers.write() {
                    p.retain(|p| p.sid != sid);
                }
                end_session(s, sessions.is_empty(), pid);
            }
        }
        if sessions.is_empty() {
            // tmux exit-empty: the last session ended. A server nobody gave
            // a session to (start-server) waits a little for one.
            if had_session || started.elapsed() > Duration::from_secs(30) {
                shutdown(&mut sessions, &marker_path, pid, &ns, None);
            }
        }
    }
}

/// Run `f` for one session with the session entered (see the module comment),
/// a panic caught and reported, and the server wide counters taken back.
fn run_request<F>(s: &mut Session, ids: &mut ServerIds, ns: &Option<String>, f: F) -> Flow
where
    F: FnOnce(&mut AppState, &mut SessionLoop) -> io::Result<Flow>,
{
    enter(s, ids, ns);
    let app = &mut s.app;
    let lp = &mut s.lp;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(app, lp)));
    ids.take_back(s.app.next_win_id, s.app.next_pane_id);
    match result {
        Ok(Ok(flow)) => flow,
        Ok(Err(e)) => {
            if crate::debug_log::server_log_enabled() {
                crate::debug_log::server_log("shared", &format!("request failed: {e}"));
            }
            Flow::Continue
        }
        // Quarantine: the panic hook logged it to crash.log; this session's
        // state may be half updated, so it ends, and only it.
        Err(_) => Flow::EndSession,
    }
}

/// Point the process wide state a request may read or a pane child may
/// inherit at session `s`.
fn enter(s: &mut Session, ids: &ServerIds, ns: &Option<String>) {
    ids.lend(&mut s.app.next_win_id, &mut s.app.next_pane_id);
    let base = s.app.port_file_base();
    if let Ok(mut g) = CURRENT.lock() {
        g.clear();
        g.push_str(&base);
    }
    std::env::set_var("PSMUX_TARGET_SESSION", &base);
    crate::commands::publish_job_env(s.app.control_port, ns.as_deref());
    if let Some(ref d) = s.cwd {
        let _ = std::env::set_current_dir(d);
    }
    crate::types::set_shared_host_colors(s.app.host_colors.clone());
}

/// One session's upkeep for one loop pass (copied from the tail of
/// run_server's loop iteration, server/mod.rs at 0145958f).
fn upkeep(app: &mut AppState, lp: &mut SessionLoop, pty_system: &dyn portable_pty::PtySystem) -> io::Result<Flow> {
    crate::format::set_sessionless(false);
    // copied from run_server (cc32087b): a set-option a hook or a key binding
    // ran in process already wrote its value; this is the rest of what the
    // SetOption request does.
    for option in std::mem::take(&mut app.in_process_option_writes) {
        let sync = crate::warm_pane_sync::for_option_change(&option, app);
        crate::warm_pane_sync::apply(app, pty_system, sync);
        if option == "command-alias" {
            if let Ok(mut map) = lp.aliases.write() {
                *map = app.command_aliases.clone();
            }
        }
        if option == "pane-border-status" {
            resize_all_panes(app);
        }
        if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(app) {
            resize_all_panes(app);
        }
        lp.meta_dirty = true;
        lp.state_dirty = true;
    }
    if lp.last_alert_check.elapsed() >= Duration::from_secs(1) {
        lp.last_alert_check = Instant::now();
        let alerts = helpers::check_window_activity(app);
        if alerts.flags_changed {
            lp.meta_dirty = true;
        }
        for event in &alerts.hooks {
            crate::commands::fire_hooks(app, event);
        }
    }
    if lp.last_registry_check.elapsed() >= Duration::from_secs(5) {
        lp.last_registry_check = Instant::now();
        ensure_session_registry_files(app);
    }
    if let Some(rx) = app.format_job_rx.as_ref() {
        let mut fresh = Vec::new();
        while let Ok(item) = rx.try_recv() {
            fresh.push(item);
        }
        for (cmd, output) in fresh {
            if let Ok(mut guard) = app.format_shell_cache.lock() {
                guard.insert(cmd, crate::types::ShellEntry { at: Instant::now(), value: output, running: false });
            }
            lp.state_dirty = true;
        }
    }
    // Reap exited floating panes (copied).
    for win in app.windows.iter_mut() {
        if win.floating.is_empty() { continue; }
        let before = win.floating.len();
        win.floating.retain_mut(|fp| !matches!(fp.pane.child.try_wait(), Ok(Some(_))));
        if win.floating.len() != before {
            win.floating_focus = if win.floating.is_empty() { None } else { Some(win.floating.len() - 1) };
            lp.state_dirty = true;
        }
    }
    if lp.last_reap.elapsed() >= Duration::from_millis(100) {
        lp.last_reap = Instant::now();
        crate::server::helpers::drain_osc52(app);
        let (all_empty, any_pruned, any_newly_dead) = tree::reap_children(app)?;
        if any_pruned {
            resize_all_panes(app);
        }
        if any_pruned || any_newly_dead {
            lp.state_dirty = true;
            lp.meta_dirty = true;
            crate::commands::fire_hooks(app, "pane-died");
            crate::commands::fire_hooks(app, "pane-exited");
        }
        control::sync_window_changes(app);
        if app.exit_empty && all_empty {
            return Ok(Flow::EndSession);
        }
    }
    // The push (copied, sent to this session's clients only).
    if !lp.clients.is_empty()
        && (lp.state_dirty || lp.meta_dirty)
        && lp.last_push.elapsed() >= Duration::from_millis(4)
    {
        super::frame::push(app, lp)?;
    }
    Ok(Flow::Continue)
}

/// The data-ready work at the top of run_server's loop (copied): mouse
/// protocol attribution and the CPR and colour query answers, for one session.
fn data_ready_work(app: &mut AppState, cpr: bool, color: bool) {
    for win in &mut app.windows {
        crate::tree::for_each_pane_mut(&mut win.root, &mut |pane| {
            crate::window_ops::update_mouse_proto_owner(pane);
        });
        for fp in win.floating.iter_mut() {
            crate::window_ops::update_mouse_proto_owner(&mut fp.pane);
        }
    }
    if cpr {
        for win in &mut app.windows {
            helpers::drain_cpr_pending(&mut win.root);
        }
        if let Mode::PopupMode { popup_pane: Some(ref mut pane), .. } = app.mode {
            if pane.cpr_pending.swap(false, Ordering::AcqRel) {
                let (r, c) = pane.term.lock().map(|g| g.screen().cursor_position()).unwrap_or((0, 0));
                let response = format!("\x1b[{};{}R", r + 1, c + 1);
                let _ = pane.writer.write_all(response.as_bytes());
                let _ = pane.writer.flush();
            }
        }
        if let Some(win) = app.windows.get_mut(app.active_idx) {
            for fp in win.floating.iter_mut() {
                if fp.pane.cpr_pending.swap(false, Ordering::AcqRel) {
                    let (r, c) = fp.pane.term.lock().map(|g| g.screen().cursor_position()).unwrap_or((0, 0));
                    let response = format!("\x1b[{};{}R", r + 1, c + 1);
                    let _ = fp.pane.writer.write_all(response.as_bytes());
                    let _ = fp.pane.writer.flush();
                }
            }
        }
    }
    if color {
        let colors = app.host_colors.clone().unwrap_or_else(crate::types::HostColors::campbell);
        for win in &mut app.windows {
            helpers::drain_color_queries(&mut win.root, &colors);
            for fp in win.floating.iter_mut() {
                let bits = fp.pane.color_query_pending.swap(0, Ordering::AcqRel);
                if bits != 0 {
                    let own = crate::types::pane_palette(fp.pane.id);
                    helpers::answer_color_queries_for_pane(bits, &mut *fp.pane.writer, fp.pane.child_pid, &colors, own);
                }
            }
        }
        if let Mode::PopupMode { popup_pane: Some(ref mut pane), .. } = app.mode {
            let bits = pane.color_query_pending.swap(0, Ordering::AcqRel);
            if bits != 0 {
                let own = crate::types::pane_palette(pane.id);
                helpers::answer_color_queries_for_pane(bits, &mut *pane.writer, pane.child_pid, &colors, own);
            }
        }
    }
}

/// The device reply drain of run_server's loop (copied), for one session.
fn device_reply_work(app: &mut AppState) {
    for win in &mut app.windows {
        helpers::drain_device_replies(&mut win.root);
        for fp in win.floating.iter_mut() {
            if let Some(b) = crate::types::take_device_replies(fp.pane.id) {
                let _ = fp.pane.writer.write_all(&b);
                let _ = fp.pane.writer.flush();
            }
        }
    }
    if let Mode::PopupMode { popup_pane: Some(ref mut pane), .. } = app.mode {
        if let Some(b) = crate::types::take_device_replies(pane.id) {
            let _ = pane.writer.write_all(&b);
            let _ = pane.writer.flush();
        }
    }
}

/// The reply to the CLI's new-session: the session's name (the CLI attaches
/// to it), then what `-P` prints (tmux: `#{session_name}:` by default).
fn new_session_output(app: &AppState, args: &NewSessionArgs) -> String {
    let mut out = format!("session={}", app.session_name);
    if args.print {
        let fmt = args.format.as_deref().unwrap_or("#{session_name}:");
        out.push('\n');
        out.push_str(&expand_format(fmt, app));
    }
    out
}

fn session_alive_elsewhere(base: &str) -> bool {
    let Ok(port) = std::fs::read_to_string(crate::paths::port_file(base)) else { return false };
    let Ok(port) = port.trim().parse::<u16>() else { return false };
    let Ok(addr) = format!("127.0.0.1:{}", port).parse::<std::net::SocketAddr>() else { return false };
    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// Create a session in this server (run_server's start up, copied and cut
/// down to what step 2 serves).
fn create_session(
    args: &NewSessionArgs,
    ids: &mut ServerIds,
    ns: &Option<String>,
    msg_tx: &mpsc::Sender<Msg>,
    pty_system: &dyn portable_pty::PtySystem,
    globals: &Arc<Globals>,
) -> Result<Session, String> {
    let name = match args.name.as_deref().filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => crate::session::next_session_name(ns.as_deref()),
    };
    let base = match ns {
        Some(n) => format!("{}__{}", n, name),
        None => name.clone(),
    };
    if session_alive_elsewhere(&base) {
        return Err(format!("duplicate session: {}", name));
    }
    let guard = match crate::platform::acquire_session_mutex(&base) {
        Some(g) => Some(g),
        None => return Err(format!("duplicate session: {}", name)),
    };
    let cwd = args
        .start_dir
        .as_deref()
        .or(args.cwd.as_deref())
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir());

    let mut app = AppState::new(name.clone());
    if app.host_colors.is_none() {
        if let Ok(spec) = std::fs::read_to_string(crate::paths::host_colors_file()) {
            let hc = crate::types::HostColors::from_spec(spec.trim());
            if hc.has_any() || hc.dark.is_some() {
                app.host_colors = Some(hc);
            }
        }
    }
    {
        let (fjtx, fjrx) = mpsc::channel();
        app.format_job_tx = Some(fjtx);
        app.format_job_rx = Some(fjrx);
    }
    app.socket_name = ns.clone();
    app.session_id = crate::session::allocate_session_id();
    app.attached_clients = 0;
    let sid = app.session_id;

    let (tx, rx) = mpsc::channel::<CtrlReq>();
    app.control_tx = Some(tx.clone());
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    app.control_port = Some(port);
    let session_key = random_key();
    app.session_key = session_key.clone();

    // Enter the new session before anything can spawn a child or allocate
    // an id (load_config may run run-shell, create_window spawns the shell).
    ids.lend(&mut app.next_win_id, &mut app.next_pane_id);
    std::env::set_var("PSMUX_TARGET_SESSION", &base);
    crate::commands::publish_job_env(app.control_port, ns.as_deref());
    if let Some(ref d) = cwd {
        let _ = std::env::set_current_dir(d);
    }
    if let Ok(mut g) = CURRENT.lock() {
        g.clear();
        g.push_str(&base);
    }

    // .key before .port (issue #496), then the per session files.
    let _ = std::fs::write(crate::paths::key_file(&base), session_key.as_bytes());
    ensure_session_registry_files(&app);

    // Requests of this session, tagged and handed to the loop.
    {
        let fwd = msg_tx.clone();
        thread::spawn(move || {
            for req in rx {
                if fwd.send(Msg::Req(sid, req)).is_err() {
                    break;
                }
            }
        });
    }
    let aliases: Aliases = Arc::new(std::sync::RwLock::new(std::collections::HashMap::new()));
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let tx = tx.clone();
        let key = session_key.clone();
        let aliases = aliases.clone();
        let globals = globals.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                if stop.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                clear_inherit_socket(&stream);
                let tx = tx.clone();
                let key = key.clone();
                let aliases = aliases.clone();
                let globals = globals.clone();
                thread::spawn(move || {
                    serve_session_connection(stream, tx, &key, aliases, sid, &globals)
                });
            }
        });
    }

    let result = (|| -> Result<(), String> {
        if let (Some(w), Some(h)) = (args.width, args.height) {
            let area = Rect { x: 0, y: 0, width: w, height: h };
            app.client_area = area;
            app.last_window_area = area;
        } else if args.width.is_some() || args.height.is_some() {
            let area = Rect { x: 0, y: 0, width: args.width.unwrap_or(80), height: args.height.unwrap_or(24) };
            app.client_area = area;
            app.last_window_area = area;
        }
        crate::util::merge_session_env_into_app(&mut app, &args.env);
        crate::config::populate_default_bindings(&mut app);
        load_config(&mut app);
        {
            let patterns = app.update_environment.clone();
            crate::environ::seed_session_from_start_env(&mut app.environment, &mut app.env_scopes, &patterns);
        }
        // The server wide writes made so far (set -g, setenv -g, set-hook -g,
        // bind): tmux keeps them once per server, so a new session sees them.
        // The server wide writes made so far are delivered to this session by
        // the control connection that asked for it (`serve_control`), through
        // its listener like any other delivery, before the CLI gets its reply.
        // No spare panes in step 2 (the refill channel is never set).
        app.warm_pane.target = 0;
        if let Ok(mut w) = aliases.write() {
            *w = app.command_aliases.clone();
        }
        resize_all_panes(&mut app);
        let created = if let Some(ref raw) = args.raw {
            create_window_raw(pty_system, &mut app, raw)
        } else {
            create_window(pty_system, &mut app, args.command.as_deref(), args.start_dir.as_deref(), false)
        };
        created.map_err(|e| format!("create window failed: {e}"))?;
        resize_all_panes(&mut app);
        apply_initial_window_name(&mut app, args.window_name.as_deref());
        crate::commands::fire_hooks(&mut app, "client-attached");
        crate::commands::fire_hooks(&mut app, "session-created");
        Ok(())
    })();
    ids.take_back(app.next_win_id, app.next_pane_id);
    if let Err(msg) = result {
        stop.store(true, Ordering::Release);
        let _ = std::net::TcpStream::connect(("127.0.0.1", port));
        crate::session::remove_session_registry_files(std::path::Path::new(&crate::paths::port_file(&base)));
        tree::kill_all_children_batch(&mut app.windows);
        return Err(msg);
    }
    if let Ok(mut p) = globals.peers.write() {
        p.push(Peer { sid, name: name.clone(), port, key: session_key });
    }
    Ok(Session { app, lp: SessionLoop::new(aliases), cwd, port, stop, _guard: guard })
}

/// End one session: its registry files first (so nothing new reaches it),
/// its clients detached, its panes killed, its listener closed.
fn end_session(mut s: Session, last: bool, pid: u32) {
    if last {
        // The orphan reaper must never see this process claimed by the data
        // dir with no `.port` naming it (run_server's shutdown order, #686).
        let _ = std::fs::remove_file(crate::paths::server_marker_file(pid));
    }
    let base = s.app.port_file_base();
    crate::session::remove_session_registry_files(std::path::Path::new(&crate::paths::port_file(&base)));
    for cid in s.lp.clients.iter() {
        crate::types::send_directive_to_client(*cid, "DETACH");
    }
    tree::kill_all_children_batch(&mut s.app.windows);
    s.stop.store(true, Ordering::Release);
    let _ = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], s.port)),
        Duration::from_millis(200),
    );
    if !s.lp.clients.is_empty() {
        std::thread::sleep(Duration::from_millis(50));
        for cid in s.lp.clients.iter() {
            crate::types::teardown_client_connection(*cid);
        }
    }
}

/// End the server: every session, the namespace marker, then the process.
fn shutdown(
    sessions: &mut BTreeMap<usize, Session>,
    marker_path: &std::path::Path,
    pid: u32,
    ns: &Option<String>,
    scoped_all: Option<bool>,
) -> ! {
    let _ = std::fs::remove_file(crate::paths::server_marker_file(pid));
    registry::remove_marker_if_ours(marker_path, pid);
    let all: Vec<usize> = sessions.keys().copied().collect();
    for sid in all {
        if let Some(s) = sessions.remove(&sid) {
            end_session(s, false, pid);
        }
    }
    if let Some(all) = scoped_all {
        // An attached client's kill-server ends every server on its socket
        // (#649), as run_server's KillServerScoped does. This server's files
        // are gone already, so the sweep only reaches the others.
        let scope = if all {
            crate::session::KillScope::All
        } else {
            crate::session::KillScope::Namespace(ns.as_deref())
        };
        crate::session::kill_servers_in_scope(std::path::Path::new(&crate::paths::psmux_dir()), scope, None);
    }
    crate::types::shutdown_persistent_streams();
    std::thread::sleep(Duration::from_millis(10));
    std::process::exit(0);
}

/// Peek at a session connection's opening; refuse what the shared server
/// does not run, hand everything else to the per session protocol handler.
fn serve_session_connection(
    stream: std::net::TcpStream,
    tx: mpsc::Sender<CtrlReq>,
    key: &str,
    aliases: Aliases,
    sid: usize,
    globals: &Globals,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
    let mut buf = vec![0u8; 16 * 1024];
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut n = 0usize;
    loop {
        match stream.peek(&mut buf) {
            Ok(0) => return,
            Ok(got) => {
                n = got;
                if router::opening_complete(&buf[..n]) || n == buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let refusal = match router::parse_opening(&buf[..n]) {
        router::Opening::Command { key: k, line, lines } if k == key => {
            let refused = router::first_unsupported(&line).map(|name| (Outcome::Unsupported(name), lines));
            if refused.is_none() {
                apply_global_writes(&line, sid, globals);
            }
            refused
        }
        router::Opening::Control { key: k } if k == key => {
            Some((Outcome::Unsupported("control-mode".to_string()), 2))
        }
        _ => None,
    };
    match refusal {
        Some((outcome, lines)) => refuse(stream, &outcome, lines),
        None => connection::handle_connection(stream, tx, key, aliases),
    }
}

/// A server wide write (`router::is_global_write`) arriving on one session's
/// listener is delivered first to every OTHER session of the server, through
/// their own listeners exactly as a client would send it (so each runs the
/// per session handler), and remembered for sessions created later; then the
/// connection goes on to run it on its own session as usual, so its reply is
/// the per session one. Runs on the connection thread, never on the loop, so
/// the loop is free to serve the deliveries. A delivery this server makes is
/// registered as an echo and is not propagated again.
fn apply_global_writes(line: &str, sid: usize, globals: &Globals) {
    if globals.take_echo(sid, line) {
        return;
    }
    let writes = router::global_writes(line);
    if writes.is_empty() {
        return;
    }
    if let Ok(mut r) = globals.replay.lock() {
        r.extend(writes.iter().cloned());
    }
    let others: Vec<Peer> = globals
        .peers
        .read()
        .map(|p| p.iter().filter(|p| p.sid != sid).cloned().collect())
        .unwrap_or_default();
    for p in others {
        for w in &writes {
            deliver(globals, &p, w);
        }
    }
}

/// Answer a refused connection: the `OK` of a good AUTH, then the reply.
fn refuse(stream: std::net::TcpStream, outcome: &Outcome, opening_lines: usize) {
    use std::io::BufRead;
    let mut w = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut r = io::BufReader::new(stream);
    let mut line = String::new();
    for _ in 0..opening_lines {
        line.clear();
        let _ = r.read_line(&mut line);
    }
    let mut out = String::from("OK\n");
    out.push_str(&reply::reply_text(outcome));
    let _ = w.write_all(out.as_bytes());
    let _ = w.flush();
    let _ = w.shutdown(std::net::Shutdown::Write);
    let _ = r.get_ref().set_read_timeout(Some(Duration::from_millis(10)));
    let mut sink = [0u8; 4096];
    while let Ok(k) = io::Read::read(&mut r, &mut sink) {
        if k == 0 {
            break;
        }
    }
}

/// The control listener protocol, spoken only by the shared CLI:
/// `AUTH <key>`, then `new-session` followed by `field=value` lines (values
/// escaped by `router::encode_field`) and `end`, or `ping`. The reply is
/// written by `reply::reply_text`.
fn serve_control(stream: std::net::TcpStream, key: &str, tx: mpsc::Sender<Msg>, globals: &Globals) {
    use std::io::BufRead;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut w = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut r = io::BufReader::new(stream);
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    if line.trim_end().strip_prefix("AUTH ") != Some(key) {
        let _ = w.write_all(b"ERROR: Invalid session key\n");
        return;
    }
    line.clear();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let cmd = match line.trim() {
        "ping" => ControlCmd::Ping,
        "new-session" => {
            let mut fields: Vec<(String, String)> = Vec::new();
            loop {
                line.clear();
                match r.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                let l = line.trim_end_matches(['\r', '\n']);
                if l == "end" {
                    break;
                }
                if let Some((k, v)) = l.split_once('=') {
                    fields.push((k.to_string(), router::decode_field(v)));
                }
            }
            ControlCmd::NewSession(Box::new(router::new_session_args_from_fields(&fields)))
        }
        other => {
            let _ = w.write_all(reply::reply_text(&Outcome::Unsupported(other.to_string())).as_bytes());
            return;
        }
    };
    let (rtx, rrx) = mpsc::channel::<Outcome>();
    if tx.send(Msg::Control(cmd, rtx)).is_err() {
        let _ = w.write_all(reply::reply_text(&Outcome::Failed("server is shutting down".into())).as_bytes());
        return;
    }
    let outcome = rrx
        .recv_timeout(Duration::from_secs(60))
        .unwrap_or_else(|_| Outcome::Failed("no response from server (timed out)".into()));
    // A new session gets the server wide writes made so far (tmux keeps them
    // once per server), delivered from this thread through its listener so
    // the loop is free to serve them, before the CLI hears it was made.
    if let Outcome::Done(ref text) = outcome {
        let name = text.lines().next().and_then(|l| l.strip_prefix("session="));
        let peer = name.and_then(|n| {
            globals.peers.read().ok().and_then(|p| p.iter().find(|p| p.name == n).cloned())
        });
        if let Some(peer) = peer {
            let writes: Vec<String> = globals.replay.lock().map(|r| r.clone()).unwrap_or_default();
            for w in &writes {
                deliver(globals, &peer, w);
            }
        }
    }
    let mut text = String::from("OK\n");
    text.push_str(&reply::reply_text(&outcome));
    let _ = w.write_all(text.as_bytes());
    let _ = w.flush();
    let _ = w.shutdown(std::net::Shutdown::Write);
}

#[cfg(test)]
#[path = "../../../tests-rs/test_issue798_shared_server_requests.rs"]
mod tests_issue798_shared_server_requests;
