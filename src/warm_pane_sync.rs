//! Centralised lifecycle for the warm pane.
//!
//! The warm pane is a snapshot of server state at spawn time: shell
//! binary, environment, terminal dimensions, vt100 scrollback cap.
//! When any of those change, the snapshot becomes stale.  Without a
//! single owner of "what does the warm pane need now?", each
//! invalidation site grew its own ad-hoc kill+respawn — which led to
//! gaps:
//!
//!   * `set-option default-shell` killed the warm pane but never
//!     respawned (only handled in one of two SetOption paths).
//!   * `set-option allow-predictions` was reconciled at boot only,
//!     never at runtime.
//!   * `set-option default-terminal` updated `app.environment["TERM"]`
//!     but the warm pane kept the old TERM forever.
//!   * `set-option history-limit` was not propagated at all (#271).
//!   * the host terminal palette (`PSMUX_HOST_COLORS`, planted on the child at
//!     spawn time) was never registered here at all, so a spare spawned before
//!     a client reported its colours kept an empty palette and handed it to the
//!     first window created afterwards.
//!
//! This module is the only place that decides what to do, and the
//! only place that mutates `app.warm_pane`.
//!
//! Three response kinds, in increasing cost:
//!
//!   `Noop`             — change does not affect the warm pane.
//!   `Patch(...)`       — mutate the running pane in place (cheap,
//!                         keeps the shell warm).  Used for state
//!                         that only lives in the vt100 parser.
//!   `Respawn(reason)`  — kill the child shell and pre-spawn a new
//!                         one with current `AppState`.  Required for
//!                         anything that affects the child process
//!                         (env vars, shell binary, predictions).
//!
//! `apply` honours `app.warm_enabled`: if warm panes are disabled,
//! `Respawn` degrades to a kill with no respawn.

use crate::types::AppState;

/// Decision returned by the `for_*` helpers.  Apply via [`apply`].
pub enum WarmPaneSync {
    Noop,
    Patch(WarmPanePatch),
    /// Kill the pooled spares AND refuse the refills still in flight: what
    /// changed was captured by the child at `CreateProcessW` (its environment
    /// block, its shell binary, its command line), so a spare issued before
    /// the change is stale however fresh it lands. The claim's environment
    /// adoption (#659), `default-shell`, `set-environment`, predictions and
    /// the post config respawns are this kind.
    Respawn(&'static str),
    /// Kill the pooled spares only; a refill still in flight is kept when it
    /// lands. For changes the landing already reconciles: a client resize is
    /// absorbed by the transplant (`need_resize`), and a host colour change is
    /// checked per spare by `land_spare`, which drops a stale palette. Refusing
    /// the in flight spare on these cost an attached launch its first warm
    /// creation, because the client reports its colours right after the
    /// trickle has issued the first spare (test_pr255_active_border, sweeps
    /// 2026-10-05_19-58-06 and 2026-10-06_05-29-09).
    RespawnKeepInflight(&'static str),
}

/// In-place mutations safe to perform on a running warm pane.
#[derive(Clone)]
pub enum WarmPanePatch {
    /// Resize the vt100 parser's scrollback cap.  Trims oldest rows
    /// if shrinking.  See `vt100::Screen::set_scrollback_len`.
    HistoryLimit(usize),
    /// Toggle whether DEC 47/1049 alt-screen mode switches are honoured.
    /// Off → TUI app output lands in main scrollback (#88).  Cheap to
    /// apply: a single field flip on the parser, no shell restart.
    AllowAlternateScreen(bool),
}

/// Decide what the warm pane needs given that a server option changed.
/// The caller has already mutated `app` so this reads the new value
/// straight off `AppState`, not from the raw `value` string.
///
/// Adding a new option that affects the warm pane?  Add it here.
/// Forgetting to do so leaves the warm pane stale until the next
/// kill-everything event (server restart, env-var change, resize),
/// which is exactly the class of bug this module exists to prevent.
pub fn for_option_change(name: &str, app: &AppState) -> WarmPaneSync {
    match name {
        // Parser-only: patch in place, no shell restart needed.
        // Kept cheap because users may set this in the prompt and we
        // do not want to throw away ~470ms of shell init for it.
        "history-limit" => WarmPaneSync::Patch(WarmPanePatch::HistoryLimit(app.history_limit)),

        // Parser-only flag — cheap to flip on a running pane.
        // Drives whether TUI apps render to alt grid (default) or
        // straight to main grid + scrollback (#88).
        "alternate-screen" => {
            WarmPaneSync::Patch(WarmPanePatch::AllowAlternateScreen(app.allow_alternate_screen))
        }

        // The shell binary itself differs — must respawn.
        "default-shell" => WarmPaneSync::Respawn("default-shell changed"),

        // We send a different PSReadLine init script depending on this
        // option (PSRL_FIX vs PSRL_CRASH_GUARD).  The script runs once
        // at shell startup, so a running shell is stuck with whichever
        // it got — only a fresh spawn picks up the new value (#165).
        "allow-predictions" => WarmPaneSync::Respawn("allow-predictions changed"),

        // default-terminal feeds `TERM` into the child env at spawn
        // time.  An already-running child has the old TERM baked in.
        "default-terminal" => WarmPaneSync::Respawn("default-terminal changed"),

        // Claude Code TTY-shim flags are read by `set_tmux_env` at
        // spawn time — running children miss the change.
        "claude-code-fix-tty" | "claude-code-force-interactive" => {
            WarmPaneSync::Respawn("claude-code option changed")
        }

        // Everything else either does not affect the warm pane (status
        // styles, key tables, hooks, etc.) or is read live (mouse,
        // status-visible) so no warm-pane action is required.
        _ => WarmPaneSync::Noop,
    }
}

/// `set-environment` / `unset-environment` always require a respawn:
/// you cannot mutate a running process's environment block from
/// outside (kernel-level constraint), so the child must be re-execed.
/// Already-handled in the codebase prior to this module (#137); now
/// consolidated through one entry point.
pub fn for_env_change() -> WarmPaneSync {
    WarmPaneSync::Respawn("environment changed")
}

/// When the client terminal resizes, the warm pane's parser grid is
/// at the old dimensions.  Respawn at the new size so the next
/// transplant lands pixel-perfect on the first frame with no reflow.
pub fn for_resize(app: &AppState, new_rows: u16, new_cols: u16) -> WarmPaneSync {
    // A pool is only "already the right size" when EVERY spare is, otherwise
    // the odd one out would transplant at the stale grid and reflow on its
    // first frame. An empty pool needs nothing killed but does need the
    // refill to happen at the new size, which `respawn` arranges.
    if !app.warm_pane.is_empty() && app.warm_pane.iter().all(|wp| wp.rows == new_rows && wp.cols == new_cols) {
        WarmPaneSync::Noop
    } else {
        WarmPaneSync::RespawnKeepInflight("client resized")
    }
}

/// The host terminal's palette reached the server (a client attached and
/// reported it, or the report changed), so every spare whose shell was spawned
/// with a different palette is stale.
///
/// `PSMUX_HOST_COLORS` is planted into a pane child's environment at SPAWN time
/// by `pane::set_host_colors_env`, and an environment block cannot be edited
/// from outside a running process.  A spare therefore carries whatever the
/// server knew when that spare was spawned, forever.  That is normally NOTHING:
/// a server learns its palette from the first client's `CtrlReq::HostColors`,
/// which arrives after the pool has already been filled — and for a claimed
/// `__warm__` standby the pool was filled by the standby, before it had a
/// session at all.  Transplanting one of those spares hands the new pane's
/// shell an empty palette while the server is busy answering OSC 10/11 with the
/// real one, which is how a nested psmux client in a fresh window lost its
/// parent's fg/bg.
///
/// Same shape as [`for_resize`]: an already-correct pool needs nothing, and an
/// empty pool still wants the refill to happen with the new palette, which
/// `Respawn` arranges.
pub fn for_host_colors_change(app: &AppState) -> WarmPaneSync {
    if app.warm_pane.iter().all(|wp| wp.host_colors == app.host_colors) {
        WarmPaneSync::Noop
    } else {
        WarmPaneSync::RespawnKeepInflight("host colors changed")
    }
}

/// After the user's config has been parsed at server boot, the
/// early-warm pane (born with all defaults) needs to be reconciled
/// with whatever the config actually set.  Returns the cheapest
/// action that gets the warm pane to a state consistent with `app`.
///
/// Order matters: respawn-class triggers are checked first because a
/// respawn implicitly applies all patch-class state to the new pane.
pub fn for_post_config(app: &AppState) -> WarmPaneSync {
    // Config disabled warm panes — `apply` handles this by killing
    // without respawning when `warm_enabled` is false.
    if !app.warm_enabled {
        return WarmPaneSync::Respawn("warm panes disabled by config");
    }

    // Custom default-shell set: the early warm pane has the wrong
    // shell binary.  Respawn to get the right one.  Doing this at
    // post-config time (rather than killing-and-deferring) keeps
    // create_window's fast path warm.
    if !app.default_shell.is_empty() {
        return WarmPaneSync::Respawn("post-config: custom default-shell");
    }

    // Config injected env vars (e.g. via set -g default-terminal,
    // set-environment in the config, or update-environment passing
    // through client env): the early child has them missing.
    // Names the update-environment seed wrote (#775) carry the start
    // environment's values, which the early child inherited already; a seeded
    // removal is a name the start environment never had.  Config removals and
    // hidden entries, though, are names the early child still has.
    let seeded = &app.env_scopes.seeded;
    let needs_env = app.environment.iter().any(|(k, _)| {
        !k.starts_with("PSMUX_TARGET_SESSION") && k != "TMUX" && k != "TMUX_PANE"
            && !seeded.contains(k)
    }) || app.env_scopes.session_removed.iter().chain(app.env_scopes.session_hidden.keys())
        .any(|k| !seeded.contains(k))
        || !app.env_scopes.global_removed.is_empty();
    if needs_env {
        return WarmPaneSync::Respawn("post-config: env vars set");
    }

    // Config flipped allow-predictions on — the early pwsh got the
    // wrong PSReadLine init.
    if app.allow_predictions {
        return WarmPaneSync::Respawn("post-config: predictions enabled");
    }

    // history-limit only differs in the parser cap, no respawn.
    // alternate-screen only differs in a parser flag, also no respawn.
    // If both differ from defaults, the consume-time helper will
    // reconcile both via `reconcile_consumed_parser`; here we only
    // need to tell the policy module that *something* parser-level
    // wants patching.  We bias to history-limit because it is the
    // more common config knob in real-world setups.
    if app.history_limit != 2000 {
        return WarmPaneSync::Patch(WarmPanePatch::HistoryLimit(app.history_limit));
    }
    if !app.allow_alternate_screen {
        return WarmPaneSync::Patch(WarmPanePatch::AllowAlternateScreen(false));
    }

    WarmPaneSync::Noop
}

/// The single mutation point for `app.warm_pane`.  Every other call
/// site outside of pre-warm boot and consume paths goes through here.
pub fn apply(
    app: &mut AppState,
    pty_system: &dyn portable_pty::PtySystem,
    sync: WarmPaneSync,
) {
    match sync {
        WarmPaneSync::Noop => {}
        WarmPaneSync::Patch(patch) => apply_patch(app, patch),
        WarmPaneSync::Respawn(_reason) => respawn(app, pty_system, true),
        WarmPaneSync::RespawnKeepInflight(_reason) => respawn(app, pty_system, false),
    }
}

fn apply_patch(app: &mut AppState, patch: WarmPanePatch) {
    // Both patch kinds also need to be applied to *every existing
    // pane*, not just the warm pane — otherwise the user's
    // `set -g alternate-screen off` would only affect the next pane
    // they open, surprising anyone who issued the change with a TUI
    // already running.  These are O(panes × O(1)) parser flag flips,
    // bounded and cheap.
    apply_patch_to_existing_panes(app, &patch);

    // Patch every spare, not just the head of the pool: any of them can be
    // the one the next new-window claims.
    for wp in app.warm_pane.iter() {
        match patch {
            WarmPanePatch::HistoryLimit(n) => {
                if let Ok(mut parser) = wp.term.lock() {
                    if parser.screen().scrollback_len() != n {
                        parser.screen_mut().set_scrollback_len(n);
                    }
                }
            }
            WarmPanePatch::AllowAlternateScreen(allowed) => {
                if let Ok(mut parser) = wp.term.lock() {
                    if parser.screen().allow_alternate_screen() != allowed {
                        parser.screen_mut().set_allow_alternate_screen(allowed);
                    }
                }
            }
        }
    }
}

/// Walk every live pane and apply the patch.  Critical for options
/// that change perceived behaviour from the user's point of view —
/// `alternate-screen off` would be useless if it only affected
/// future panes.  history-limit propagation matches tmux semantics:
/// existing buffers grow / shrink to the new cap.
fn apply_patch_to_existing_panes(app: &mut AppState, patch: &WarmPanePatch) {
    use crate::types::Node;
    fn walk(node: &mut Node, patch: &WarmPanePatch) {
        match node {
            Node::Leaf(p) => {
                // Reaches the live screen too while a copy-mode snapshot is in
                // `term` (see `Pane::each_term`).
                for term in p.each_term() {
                    if let Ok(mut parser) = term.lock() {
                        match patch {
                            WarmPanePatch::HistoryLimit(n) => {
                                if parser.screen().scrollback_len() != *n {
                                    parser.screen_mut().set_scrollback_len(*n);
                                }
                            }
                            WarmPanePatch::AllowAlternateScreen(allowed) => {
                                if parser.screen().allow_alternate_screen() != *allowed {
                                    parser.screen_mut().set_allow_alternate_screen(*allowed);
                                }
                            }
                        }
                    }
                }
            }
            Node::Split { children, .. } => {
                for child in children.iter_mut() {
                    walk(child, patch);
                }
            }
        }
    }
    for win in app.windows.iter_mut() {
        walk(&mut win.root, patch);
    }
}

fn respawn(app: &mut AppState, _pty_system: &dyn portable_pty::PtySystem, refuse_inflight: bool) {
    // Kill every spare — there is no in-place way to swap shell binaries or
    // environment blocks, and a pool where only the head was refreshed would
    // hand a stale shell to the second creation.
    //
    // Nothing is spawned here. The server loop notices the deficit on its next
    // tick and hands the spawns to the background spawner, which is what keeps
    // an expensive event (a client resize, a `set-option`) from stalling the
    // loop for the length of a CreateProcess. `warm_enabled` and
    // `warm-pool-size 0` are honoured there: a target of zero simply never
    // produces a deficit, so an opted-out user's pool stays empty.
    let killed = app.warm_pane.len();
    app.warm_pane.kill_all();
    // The spares still being spawned are just as stale: a refill issued before
    // this point captured the old environment block (or shell) the moment its
    // CreateProcessW began, and it lands in the pool AFTER `kill_all` has run.
    // A standby trickles its pool one spare at a time, each spawn 75 to 270 ms
    // inside CreateProcessW, so a claim that adopts the client's environment
    // (#659) has a real chance of arriving inside that window; the spare then
    // landed with the standby's environment and `new-window` handed it out
    // (test_issue659_warm_claim_environment, 3 of 10 runs). Every spare issued
    // so far has an id below `next_pane_id`, so raising the floor there makes
    // `WarmPool::push` refuse and kill each one on arrival, the same gate a
    // cold creation uses against late arrivals (ebce952).
    //
    // Not for every reason, though: see `RespawnKeepInflight`. A client resize
    // and a host colour report are reconciled when the spare lands, and both
    // fire right after an attached launch has issued its first trickled spare,
    // so refusing it there cost the first creation its warm shell.
    let inflight = app.warm_pane.inflight;
    if refuse_inflight {
        let floor = app.next_pane_id;
        let refused_floor = app.warm_pane.set_issued_floor(floor);
        crate::warm_trace!(
            "pool: respawn requested, killed {} spare(s), {} in flight refused below floor {} (dropped {}), target={}",
            killed, inflight, floor, refused_floor, app.warm_pane.target
        );
    } else {
        crate::warm_trace!(
            "pool: respawn requested, killed {} spare(s), {} in flight kept, target={}",
            killed, inflight, app.warm_pane.target
        );
    }
}

/// Helper for warm-pane consume sites in `pane.rs`.  When a warm
/// pane is transplanted into a real session, its parser may still
/// hold stale flags (history-limit raised, alt-screen toggled) after
/// the pane was born.  This is the safety net that guarantees
/// consume-time consistency even if a future caller forgets to
/// invoke `apply` on a state change.
pub fn reconcile_consumed_parser(parser: &mut vt100::Parser, app: &AppState) {
    let screen = parser.screen();
    let need_history = screen.scrollback_len() != app.history_limit;
    let need_alt = screen.allow_alternate_screen() != app.allow_alternate_screen;
    if need_history || need_alt {
        let s = parser.screen_mut();
        if need_history {
            s.set_scrollback_len(app.history_limit);
        }
        if need_alt {
            s.set_allow_alternate_screen(app.allow_alternate_screen);
        }
    }
}

/// Spares whose shell exists but which the server does not own yet (#686).
///
/// The pool's own `kill_all` can only reach spares that are already in
/// `AppState`. A surge hands eight spawns to background threads, and each one
/// is a real `CreateProcessW` long before its `WarmPane` reaches the server
/// loop: the shell exists, its conhost exists, and the server knows nothing
/// about it. `kill-server` in that window killed the windows and the pool and
/// then called `process::exit`, which stops the spawner threads mid flight and
/// leaves those shells parented to a dead psmux, idle at a prompt forever. The
/// measured leak was six orphan `pwsh` over ten rounds of "new-session, six
/// new-window, kill-server".
///
/// So the spawn is tracked from the moment it is ISSUED, not from the moment it
/// lands:
///
///   * [`issue`] records the pane id with no pid yet, on the loop thread.
///   * [`record_pid`] fills the pid in on the spawner thread, as soon as
///     `CreateProcessW` has returned one. It answers `false` when the server has
///     already begun to die, which tells the spawner to kill the child it just
///     created rather than post it to a loop that will never read it.
///   * [`release`] drops the entry once the spare is the server's (it landed in
///     `AppState`, where `WarmPool::kill_all` covers it) or once the spawn has
///     failed.
///   * [`reap`] is the teardown: it closes the registry to new pids and returns
///     every pid still in it, for the caller to kill by pid through the kill
///     guard. It waits briefly for any spawn caught inside `CreateProcessW`,
///     because that one's pid appears a few milliseconds late.
pub mod inflight {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    static SPAWNS: Mutex<Option<HashMap<usize, Option<u32>>>> = Mutex::new(None);
    static TEARING_DOWN: AtomicBool = AtomicBool::new(false);

    fn with<R>(f: impl FnOnce(&mut HashMap<usize, Option<u32>>) -> R) -> R {
        let mut g = SPAWNS.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(HashMap::new))
    }

    /// True once teardown has begun: no spawn issued from here on is ever
    /// adopted.
    pub fn is_tearing_down() -> bool {
        TEARING_DOWN.load(Ordering::SeqCst)
    }

    /// Record that a spare spawn has been handed to the spawner. Called on the
    /// loop thread, before the thread exists, so there is no window in which a
    /// spawn is running untracked.
    pub fn issue(pane_id: usize) {
        with(|m| {
            m.insert(pane_id, None);
        });
    }

    /// The spawner has a pid. Returns false when the server is tearing down, in
    /// which case the caller owns the kill: the entry is dropped here so the
    /// reaper does not also chase a pid the spawner is already killing.
    ///
    /// On the teardown answer the entry is deliberately LEFT in place: the
    /// caller kills its child and then calls [`release`]. Removing it here
    /// let a reaper that was still polling see an empty registry, return,
    /// and `process::exit` before the spawner's kill had run.
    pub fn record_pid(pane_id: usize, pid: Option<u32>) -> bool {
        if is_tearing_down() {
            return false;
        }
        with(|m| {
            if let Some(slot) = m.get_mut(&pane_id) {
                *slot = pid;
            }
        });
        true
    }

    /// This spawn is no longer in flight: it landed in `AppState` (the pool
    /// owns the child now) or it failed.
    pub fn release(pane_id: usize) {
        with(|m| {
            m.remove(&pane_id);
        });
    }

    /// Pane ids still in flight, for diagnostics and tests.
    pub fn pending() -> Vec<usize> {
        let mut v = with(|m| m.keys().copied().collect::<Vec<_>>());
        v.sort_unstable();
        v
    }

    /// Pids recorded so far, without disturbing the registry.
    pub fn pids() -> Vec<u32> {
        let mut v = with(|m| m.values().filter_map(|p| *p).collect::<Vec<_>>());
        v.sort_unstable();
        v
    }

    /// Close the registry and drain it.
    ///
    /// Returns the pids to kill. Entries with no pid yet are spawns sitting
    /// inside `CreateProcessW`: they cannot be killed because nothing knows
    /// what to kill, so the caller polls again until they have either
    /// registered a pid (reaped on that pass) or seen the teardown flag and
    /// killed their own child (which removes the entry). `budget` bounds that
    /// wait, because a shutdown must never hang on a spawn that wedged.
    pub fn reap(budget: std::time::Duration, mut sleep: impl FnMut(std::time::Duration)) -> Vec<u32> {
        TEARING_DOWN.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + budget;
        let mut killed = Vec::new();
        loop {
            let (ready, waiting) = with(|m| {
                let ready: Vec<(usize, u32)> =
                    m.iter().filter_map(|(id, p)| p.map(|p| (*id, p))).collect();
                for (id, _) in &ready {
                    m.remove(id);
                }
                (ready, m.len())
            });
            killed.extend(ready.into_iter().map(|(_, p)| p));
            if waiting == 0 || std::time::Instant::now() >= deadline {
                break;
            }
            sleep(std::time::Duration::from_millis(5));
        }
        killed
    }

    /// Tests share one process, so the registry has to be resettable.
    #[cfg(test)]
    pub fn reset_for_test() {
        TEARING_DOWN.store(false, Ordering::SeqCst);
        with(|m| m.clear());
    }
}

/// Kill every spare the server does not own yet, and stop any spawn still in
/// flight from surviving this process (#686).
///
/// Called from the shutdown path, after the pool's own spares are killed. The
/// kill goes through the platform kill guard, which validates each pid's
/// creation time before terminating, so a pid recycled between the spawn and
/// this call is never touched.
/// How long a teardown waits for spare spawns caught inside CreateProcessW.
///
/// Such a spawn cannot be killed until CreateProcessW returns its pid, and for
/// the Store pwsh (which refuses the job list, see portable-pty spawn_command)
/// the shell it creates is SUSPENDED AND OUTSIDE ITS JOB until the spawner
/// assigns it. A `process::exit` in that window ends the spawner thread right
/// there, so the shell is never assigned, never resumed and never killed: an
/// orphan of a dead server, for ever. The budget therefore has to outlast a
/// slow CreateProcessW, not a typical one. It was 400 ms (twice, about 900 ms
/// in all), and a sweep whose median CreateProcessW was 736 ms left 3 orphan
/// pwsh in one round. `PSMUX_TEST_SLOW_CREATE_MS` reproduces that window.
///
/// The wait only happens while a spawn is in flight without a pid; an idle
/// shutdown returns at once, and each spawn ends the wait the moment it either
/// registers its pid or kills its own child.
pub const INFLIGHT_REAP_BUDGET: std::time::Duration = std::time::Duration::from_millis(4000);

/// How long a one-shot `kill-server` connection is held open before it is
/// answered anyway. The caller force-kills the server 50 ms after this socket
/// closes, so it must outlast the whole shutdown, reaper wait included, or the
/// force-kill lands in the window the reaper is waiting out.
pub const KILL_SERVER_HOLD: std::time::Duration = std::time::Duration::from_millis(5000);

pub fn reap_inflight_spares() -> usize {
    // The shutdown path calls this twice with its client courtesies in between;
    // the first call does any waiting, the second sweeps a pid that registered
    // on the way out.
    let pids = inflight::reap(INFLIGHT_REAP_BUDGET, std::thread::sleep);
    for pid in &pids {
        crate::platform::process_kill::kill_pid_tree(*pid);
    }
    if !pids.is_empty() {
        crate::warm_trace!("pool: reaped {} in flight spare(s) on teardown: {:?}", pids.len(), pids);
    }
    pids.len()
}

#[cfg(test)]
#[path = "../tests-rs/test_warm_pane_sync.rs"]
mod test_warm_pane_sync;

#[cfg(test)]
#[path = "../tests-rs/test_issue686_pool_reap.rs"]
mod tests_issue686_pool_reap;

#[cfg(test)]
#[path = "../tests-rs/test_warm_pool_depth.rs"]
mod test_warm_pool_depth;

#[cfg(test)]
#[path = "../tests-rs/test_issue661_warm_pool_depth.rs"]
mod test_issue661_warm_pool_depth;

#[cfg(test)]
#[path = "../tests-rs/test_pane_id_monotonic_claim.rs"]
mod test_pane_id_monotonic_claim;

#[cfg(test)]
#[path = "../tests-rs/test_issue659_respawn_refuses_inflight.rs"]
mod test_issue659_respawn_refuses_inflight;
