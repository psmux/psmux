//! Request handling of the shared server, one request of one session at a time.
//!
//! The arms below marked "copied" are copies of run_server's arms
//! (server/mod.rs at 86a78100; each copy names its source line) so the shared
//! server does exactly what a per session server does for those requests. The
//! owner ruling forbids changing run_server to share them, and copying is the
//! approach the ruling prefers. The arms written for the shared server are
//! marked "shared". Any request without an arm here is dropped: a one shot
//! command is refused before it gets this far (`router`/`reply`), so what can
//! still arrive unhandled is an attached client's request for a feature step 2
//! does not have, which then does nothing, and its reply channel closes.
//!
//! The locals the copied arms use (`state_dirty`, `meta_dirty`, `hook_event`,
//! `notify_events`, `echo_pending_until`, `temp_focus_restore`, `names_pane`)
//! are run_server's per request locals; they are loaded from and stored back
//! to the session's `SessionLoop` around the request.

#![allow(unused_variables, unused_mut, unused_assignments, clippy::all)]

use super::super::*;
use super::server::{Flow, SessionLoop};

pub(crate) fn dispatch(
    // `mut` so the copied arms' `&mut app` / `&app` (run_server owns its
    // AppState by value) coerce to the session's state unchanged.
    mut app: &mut AppState,
    lp: &mut SessionLoop,
    req: CtrlReq,
    pty_system: &dyn portable_pty::PtySystem,
) -> io::Result<Flow> {
    let mut flow = Flow::Continue;
    let mut state_dirty = lp.state_dirty;
    let mut meta_dirty = lp.meta_dirty;
    let mut echo_pending_until = lp.echo_pending_until;
    let mut temp_focus_restore = lp.temp_focus_restore.take();
    let shared_aliases_main = lp.aliases.clone();
    #[allow(clippy::never_loop)]
    for req in std::iter::once(req) {
        temp_target::restore_temp_focus(app, &mut temp_focus_restore);
        let names_pane = matches!(&req, CtrlReq::Targeted(t, _)
            if t.win.is_some() || t.win_name.is_some() || t.pane.is_some());
        let req = match req {
            CtrlReq::Targeted(target, inner) => {
                match temp_target::apply_temp_target(app, &mut temp_focus_restore, &target) {
                    Ok(()) => *inner,
                    Err(msg) => {
                        app.status_message = Some((msg, Instant::now(), None));
                        state_dirty = true;
                        continue;
                    }
                }
            }
            other => other,
        };
        let Some(req) = crate::server::options::divert_shadowed_global_request(app, req) else {
            meta_dirty = true;
            state_dirty = true;
            continue;
        };
        let mutates_state = !matches!(&req,
            CtrlReq::DumpState(..)
            | CtrlReq::SendText(_)
            | CtrlReq::SendKey(_)
            | CtrlReq::SendPaste(_)
            | CtrlReq::WindowDump(..)
            | CtrlReq::WindowLayout(..)
            | CtrlReq::PtyWake
            | CtrlReq::ClientActivity(_)
        );
        let mut hook_event: Option<&str> = None;
        let mut notify_events: Vec<&str> = Vec::new();
        let _prev_active_idx = app.active_idx;
        let _req_tag: &str = "";
        crate::format::set_sessionless(false);
        match req {
                // shared: session and server lifetime
                CtrlReq::KillSession => {
                    if let Some(cmds) = app.hooks.get("session-closed") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(app, cmd); } }
                    flow = Flow::EndSession;
                }
                CtrlReq::KillServer => {
                    flow = Flow::KillServer { scoped_all: None };
                }
                CtrlReq::KillServerScoped(all) => {
                    flow = Flow::KillServer { scoped_all: Some(all) };
                }
                CtrlReq::HasSession(resp) => {
                    // This listener belongs to a live session of this server.
                    let _ = resp.send(true);
                }
                CtrlReq::DumpState(resp, allow_nc, dump_client_id) => {
                    lp.state_dirty = state_dirty;
                    lp.meta_dirty = meta_dirty;
                    super::frame::dump_state(app, lp, resp, allow_nc, dump_client_id)?;
                    state_dirty = lp.state_dirty;
                    meta_dirty = lp.meta_dirty;
                    continue;
                }
                CtrlReq::ClientDetach(cid) => {
                    // shared: run_server's arm, but the destroy-unattached end
                    // ends this session, never the process.
                    lp.dump_state_seen_full.remove(&cid);
                    lp.clients.remove(&cid);
                    if app.reap_client(cid) {
                        if crate::resize_window::refresh_dynamic_window_sizes(app) {
                            resize_all_panes(app);
                        }
                        hook_event = Some("client-detached");
                        if app.attached_clients == 0 && app.destroy_unattached {
                            flow = Flow::EndSession;
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 3223 at 86a78100)
                CtrlReq::NewWindow(cmd, name, detached, start_dir, title, empty, env_sets, placement, outcome) => {
                    if let Some(cmds) = app.hooks.get("before-new-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // Expand format variables like #{pane_current_path} (#111)
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    // Issue #630: do NOT chdir the server process into the start
                    // directory. CreatePseudoConsole() spawns the pane's conhost.exe
                    // from THIS process, so the host inherits the server's Win32
                    // current directory and then holds an open handle on it for the
                    // pane's entire life. That blocks deletion of the directory with
                    // Win32 error 32 ("used by another process") long after the pane's
                    // shell has cd'd away, and no `cd` can ever move the host.
                    //
                    // The start directory reaches the pane's SHELL explicitly instead,
                    // which is the only process that should hold it: a cold spawn gets
                    // it as lpCurrentDirectory through CommandBuilder::cwd() (see
                    // pane.rs, usable_start_dir), and a warm transplant gets it through
                    // pane::silent_rehome(). A shell releases the directory as soon as
                    // it cd's elsewhere, which is the tmux behaviour.
                    let result = run_new_window(
                        &mut app, &*pty_system, cmd.as_deref(), name.as_deref(), detached,
                        start_dir.as_deref(), title, empty, &env_sets, &placement, &mut notify_events);
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    match result {
                        Ok(created) => {
                            // Replenish warm pane pool for next new-window
                            // Warm-pane replenish is deferred OFF the command path: it
                            // runs at the loop top during an idle gap (Tier 3), so a burst
                            // of window-creates never chains blocking spawns that stall
                            // other clients' commands.
                            resize_all_panes(&mut app); meta_dirty = true; state_dirty = true;
                            if created.is_some() { notify_events.push("window-linked"); hook_event = Some("after-new-window"); }
                            if let Some(tx) = outcome { let _ = tx.send(Ok(())); }
                        }
                        Err(msg) => {
                            // An attached client (command prompt, key binding) has
                            // no reply stream: show it like move-window does.
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            resize_all_panes(&mut app); meta_dirty = true; state_dirty = true;
                            if let Some(tx) = outcome { let _ = tx.send(Err(msg)); }
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 3265 at 86a78100)
                CtrlReq::NewWindowPrint(cmd, name, detached, start_dir, format_str, resp, title, empty, env_sets, placement) => {
                    if let Some(cmds) = app.hooks.get("before-new-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    // No server chdir here: the pane's start directory is handed to the
                    // shell alone, never to the ConPTY host. See the #630 note on
                    // CtrlReq::NewWindow above.
                    let result = run_new_window(
                        &mut app, &*pty_system, cmd.as_deref(), name.as_deref(), detached,
                        start_dir.as_deref(), title, empty, &env_sets, &placement, &mut notify_events);
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    match result {
                        Ok(created) => {
                            // Use full format engine for -P output (tmux compatible).
                            // `-S` selecting an existing window prints nothing (tmux
                            // returns before the print).
                            let pane_info = match created {
                                Some(pos) => {
                                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}");
                                    crate::format::expand_format_for_window(fmt, &app, pos)
                                }
                                None => String::new(),
                            };
                            let _ = resp.send(pane_info);
                            resize_all_panes(&mut app); meta_dirty = true; state_dirty = true;
                            if created.is_some() { notify_events.push("window-linked"); hook_event = Some("after-new-window"); }
                        }
                        Err(msg) => {
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            resize_all_panes(&mut app); meta_dirty = true; state_dirty = true;
                            let _ = resp.send(format!("ERROR: {}", msg));
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 3298 at 86a78100)
                CtrlReq::SplitWindow(k, cmd, detached, start_dir, split_size, resp, title, env_sets, zoom_after_split) => {
                    if let Some(cmds) = app.hooks.get("before-split-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // tmux: split-window without -Z permanently unzooms (#82)
                    unzoom_if_zoomed(&mut app);
                    // tmux: split-window INSIDE a floating pane creates ANOTHER
                    // floating pane (offset from it), not a tiled split.
                    let float_src = {
                        let win = &app.windows[app.active_idx];
                        win.floating_focus.and_then(|fi| win.floating.get(fi)).map(|fp| (fp.x, fp.y, fp.w, fp.h, fp.border.clone()))
                    };
                    if let Some((sx, sy, sw, sh, sborder)) = float_src {
                        // Floating panes are overlays, so tiled-window zoom does
                        // not apply to splits created from a floating pane.
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let nx = (sx + 2).min(win_w.saturating_sub(sw));
                        let ny = (sy + 2).min(win_h.saturating_sub(sh));
                        let inner_h = sh.saturating_sub(2).max(1);
                        let inner_w = sw.saturating_sub(2).max(1);
                        let cmdstr = cmd.clone().unwrap_or_default();
                        let sd = start_dir.clone().map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                        let pane_id = app.next_pane_id;
                        if let Some(mut pane) = crate::popup::create_popup_pane(&cmdstr, sd.as_deref(), inner_h, inner_w, pane_id, &app.session_name, &app.environment, &app.env_scopes.child_removals(&app.environment), app.host_colors.as_ref()) {
                            app.next_pane_id += 1;
                            let t = title.clone().unwrap_or_default();
                            if !t.is_empty() { pane.title = t.clone(); pane.title_locked = true; }
                            let win = &mut app.windows[app.active_idx];
                            win.floating.push(crate::types::FloatingPane { pane, x: nx, y: ny, w: sw, h: sh, border: sborder, id: pane_id, title: t, position: None, styles: Default::default() });
                            if !detached { win.floating_focus = Some(win.floating.len() - 1); }
                            state_dirty = true;
                        }
                        let _ = resp.send(String::new());
                    } else {
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    // No server chdir here: the pane's start directory is handed to the
                    // shell alone, never to the ConPTY host. See the #630 note on
                    // CtrlReq::NewWindow above.
                    let prev_pane_id = crate::tree::get_active_pane_id(&app.windows[app.active_idx].root, &app.windows[app.active_idx].active_path);
                    let split_succeeded = match crate::pane::split_active_placed(&mut app, k, cmd.as_deref(), Some(&*pty_system), start_dir.as_deref(), &env_sets, split_size) {
                        Err(e) => {
                            let msg = format!("split-window: {e}");
                            app.status_message = Some((msg.clone(), std::time::Instant::now(), None));
                            let _ = resp.send(format!("psmux: {msg}"));
                            false
                        }
                        Ok(()) => {
                            let _ = resp.send(String::new());
                            true
                        }
                    };
                    // `-l` is applied by the split itself, in cells (tmux
                    // layout_split_sizes); it used to be rewritten here as a
                    // percentage of the WINDOW after a 50/50 split.
                    // -T: the just-created pane is currently active in this
                    // window, so set its title before any detached revert.
                    if let Some(t) = title {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                    }
                    if detached {
                        // Capture new pane ID before reverting focus
                        let new_pane_id = crate::tree::get_active_pane_id(
                            &app.windows[app.active_idx].root,
                            &app.windows[app.active_idx].active_path,
                        );
                        // Revert focus to the previously active pane, found
                        // by id: with -b it is no longer child 0, and a split
                        // into a same-kind parent did not nest it at all.
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = prev_pane_id.and_then(|id| crate::tree::find_path_by_id(&win.root, id)) {
                            win.active_path = p;
                        }
                        // Detached splits never focus the new pane: remove
                        // from MRU entirely so directional nav tie-breaks by
                        // pane_index among equally-unvisited candidates (#70).
                        if let Some(nid) = new_pane_id {
                            let win = &mut app.windows[app.active_idx];
                            win.pane_mru.retain(|&id| id != nid);
                        }
                    } else {
                        // Non-detached: new pane keeps focus.
                        // Cancel temp_focus_restore so -t doesn't revert (#112).
                        // The temporary focus just became permanent.
                        temp_focus_restore = None;
                        app.temp_focus_saved_active = None;
                    }
                    if zoom_after_split && split_succeeded {
                        // Give every pane its new cell BEFORE zooming: a zoom
                        // skips the hidden panes, so the pane just split stayed
                        // at its old height (tmux layout_fix_panes runs first).
                        resize_all_panes(&mut app);
                        toggle_zoom(&mut app);
                    }
                    // Replenish warm pane for the next new-window/split
                    // Warm-pane replenish is deferred OFF the command path: it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                    }
                }
                // copied from server/mod.rs run_server (line 3402 at 86a78100)
                CtrlReq::SplitWindowPrint(k, cmd, detached, start_dir, split_size, format_str, resp, title, env_sets, zoom_after_split) => {
                    if let Some(cmds) = app.hooks.get("before-split-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    unzoom_if_zoomed(&mut app);
                    let start_dir = start_dir.map(|d| expand_format(&d, &app)).filter(|d| !d.is_empty());
                    // No server chdir here: the pane's start directory is handed to the
                    // shell alone, never to the ConPTY host. See the #630 note on
                    // CtrlReq::NewWindow above.
                    let prev_pane_id = crate::tree::get_active_pane_id(&app.windows[app.active_idx].root, &app.windows[app.active_idx].active_path);
                    let split_succeeded = match crate::pane::split_active_placed(&mut app, k, cmd.as_deref(), Some(&*pty_system), start_dir.as_deref(), &env_sets, split_size) {
                        Err(e) => {
                            app.status_message = Some((format!("split-window: {e}"), std::time::Instant::now(), None));
                            eprintln!("psmux: split-window error: {e}");
                            false
                        }
                        Ok(()) => true,
                    };
                    // -T: set the new (currently active) pane's title.
                    if let Some(t) = title {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                    }
                    // Use full format engine for -P output (tmux compatible)
                    let fmt = format_str.as_deref().unwrap_or("#{session_name}:#{window_index}.#{pane_index}");
                    let pane_info = crate::format::expand_format_for_window(fmt, &app, app.active_idx);
                    if detached {
                        // Capture new pane ID before reverting focus
                        let new_pane_id = crate::tree::get_active_pane_id(
                            &app.windows[app.active_idx].root,
                            &app.windows[app.active_idx].active_path,
                        );
                        let win = &mut app.windows[app.active_idx];
                        if let Some(p) = prev_pane_id.and_then(|id| crate::tree::find_path_by_id(&win.root, id)) {
                            win.active_path = p;
                        }
                        // Detached splits: remove from MRU (#70 pane_index tie-break)
                        if let Some(nid) = new_pane_id {
                            let win = &mut app.windows[app.active_idx];
                            win.pane_mru.retain(|&id| id != nid);
                        }
                    } else {
                        temp_focus_restore = None;
                        app.temp_focus_saved_active = None;
                    }
                    if zoom_after_split && split_succeeded {
                        // Give every pane its new cell BEFORE zooming: a zoom
                        // skips the hidden panes, so the pane just split stayed
                        // at its old height (tmux layout_fix_panes runs first).
                        resize_all_panes(&mut app);
                        toggle_zoom(&mut app);
                    }
                    let _ = resp.send(pane_info);
                    // Replenish warm pane
                    // Warm-pane replenish is deferred OFF the command path: it
                    // runs at the loop top during an idle gap (Tier 3), so a burst
                    // of window-creates never chains blocking spawns that stall
                    // other clients' commands.
                    resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-split-window");
                }
                // copied from server/mod.rs run_server (line 3463 at 86a78100)
                CtrlReq::KillPane => {
                    if let Some(cmds) = app.hooks.get("before-kill-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // A focused floating pane is closed by kill-pane instead of a
                    // tiled pane. The child is dropped with the FloatingPane.
                    let closed_float = {
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if fi < win.floating.len() {
                                let mut fp = win.floating.remove(fi);
                                let _ = fp.pane.child.kill();
                                win.floating_focus = if win.floating.is_empty() { None } else { Some(win.floating.len() - 1) };
                                true
                            } else { false }
                        } else { false }
                    };
                    if closed_float {
                        state_dirty = true;
                    } else {
                        let win = &app.windows[app.active_idx];
                        let win_id = win.id;
                        let killed_active = crate::tree::get_active_pane_id(&win.root, &win.active_path)
                            .map_or(false, |pid| temp_target::is_real_active_pane(&app, &temp_focus_restore, app.active_idx, pid));
                        unzoom_if_zoomed(&mut app); let _ = kill_active_pane(&mut app); resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-kill-pane");
                        control::emit_pane_closed(&app, win_id, killed_active);
                    }
                }
                // copied from server/mod.rs run_server (line 3489 at 86a78100)
                CtrlReq::KillPaneById(pid) => {
                    if let Some(cmds) = app.hooks.get("before-kill-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // A floating %id closes that float, as `kill-pane` does
                    // for the focused one. kill_pane_by_id walks the tiled
                    // tree only, so `kill-pane -t <float>` used to do nothing
                    // at exit 0 (issue #793 follow up).
                    if let Some((wi, fi)) = crate::tree::find_floating_by_id(&app, pid) {
                        let win = &mut app.windows[wi];
                        let focused_id = win.floating_focus.and_then(|f| win.floating.get(f)).map(|fp| fp.id);
                        let mut fp = win.floating.remove(fi);
                        let _ = fp.pane.child.kill();
                        win.floating_focus = match focused_id {
                            Some(id) if id != pid => win.floating.iter().position(|f| f.id == id),
                            Some(_) if !win.floating.is_empty() => Some(win.floating.len() - 1),
                            _ => None,
                        };
                        state_dirty = true;
                    } else {
                        let win = app.windows.iter()
                            .position(|w| crate::tree::find_path_by_id(&w.root, pid).is_some())
                            .map(|wi| (app.windows[wi].id, temp_target::is_real_active_pane(&app, &temp_focus_restore, wi, pid)));
                        unzoom_if_zoomed(&mut app); let _ = kill_pane_by_id(&mut app, pid); resize_all_panes(&mut app); meta_dirty = true; hook_event = Some("after-kill-pane");
                        if let Some((win_id, killed_active)) = win { control::emit_pane_closed(&app, win_id, killed_active); }
                    }
                }
                // copied from server/mod.rs run_server (line 3514 at 86a78100)
                CtrlReq::CapturePane(resp, pane_id, preserve_trailing) => {
                    // Note: do NOT gate on is_active_pane_squelched here.
                    // Returning empty during the cd+cls squelch window makes
                    // iTerm2's initial attach paint a blank screen, since
                    // capture-pane is only requested once on attach.  Return
                    // current parser screen content; it's just cell text and
                    // any stale frame is harmless (subsequent %output rewrites).
                    if let Some(text) = capture_active_pane_text(&mut app, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                // copied from server/mod.rs run_server (line 3523 at 86a78100)
                CtrlReq::CapturePaneStyled(resp, s, e, pane_id, preserve_trailing) => {
                    if let Some(text) = capture_active_pane_styled(&mut app, s, e, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                // copied from server/mod.rs run_server (line 3526 at 86a78100)
                CtrlReq::CapturePaneRange(resp, s, e, pane_id, preserve_trailing) => {
                    if let Some(text) = capture_active_pane_range(&mut app, s, e, pane_id, preserve_trailing)? { let _ = resp.send(text); } else { let _ = resp.send(String::new()); }
                }
                // copied from server/mod.rs run_server (line 3529 at 86a78100)
                CtrlReq::CapturePaneSaved { resp, start, end, pane_id, styled, opts } => {
                    // A read that fails answers empty: a ? here would end the server.
                    let text = if styled {
                        crate::copy_mode::capture_pane_styled_from(&mut app, start, end, pane_id, opts, true).unwrap_or(None)
                    } else if start.is_some() || end.is_some() {
                        crate::copy_mode::capture_pane_range_from(&mut app, start, end, pane_id, opts, true).unwrap_or(None)
                    } else {
                        crate::copy_mode::capture_pane_text_from(&mut app, pane_id, opts, true).unwrap_or(None)
                    };
                    let _ = resp.send(text);
                }
                // copied from server/mod.rs run_server (line 3676 at 86a78100)
                CtrlReq::ResolveTarget { kind, spec, resp } => {
                    let result = crate::commands::resolve_in_server_target(&app, kind, &spec);
                    if let Err(ref msg) = result {
                        // The status bar, as for ValidateTarget below.
                        app.status_message = Some((msg.clone(), Instant::now(), None));
                        state_dirty = true;
                    }
                    let _ = resp.send(result);
                }
                // copied from server/mod.rs run_server (line 3685 at 86a78100)
                CtrlReq::ValidateTarget { target, resp } => {
                    match temp_target::resolve_temp_target(&app, &target) {
                        Ok(resolved) => { let _ = resp.send(Ok(resolved)); }
                        Err(msg) => {
                            // Surface in the status bar for attached clients,
                            // same convention as join-pane (#437) and
                            // kill-window (8edd1cb). The Err reply makes the
                            // connection thread skip the command.
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                    }
                }
                // Unwrapped at the top of this loop; TargetedSender never
                // nests one inside another.
                // copied from server/mod.rs run_server (line 3702 at 86a78100)
                CtrlReq::SessionInfo(resp) => {
                    let num_attached = app.client_registry.len();
                    let attached = if num_attached > 0 { " (attached)" } else { "" };
                    let group = if let Some(ref g) = app.session_group {
                        format!(" (group {})", g)
                    } else {
                        String::new()
                    };
                    let windows = app.windows.len();
                    let created = app.created_at.format("%a %b %e %H:%M:%S %Y");
                    let line = format!("{}: {} windows (created {}){}{}\n", app.session_name, windows, created, group, attached);
                    let _ = resp.send(line);
                }
                // copied from server/mod.rs run_server (line 3715 at 86a78100)
                CtrlReq::SessionInfoFormat(resp, fmt) => {
                    let line = crate::format::format_list_sessions(&app, &fmt);
                    let _ = resp.send(format!("{}\n", line));
                }
                // copied from server/mod.rs run_server (line 3719 at 86a78100)
                CtrlReq::ExpandFormat(fmt, resp) => {
                    // Synchronous by design: the caller is a connection thread
                    // that is about to run the expanded string, and it has
                    // already decided the round trip is worth it (it only sends
                    // this when the command actually contains a `#`).
                    // Untimed, like tmux's format_expand: every sender is a
                    // shell command or a set-option -F value, where `%` is
                    // not strftime (issue #734 follow up).
                    let _ = resp.send(crate::format::expand_format_untimed(&fmt, &app));
                }
                // copied from server/mod.rs run_server (line 3729 at 86a78100)
                CtrlReq::ExpandFormatNoTarget(fmt, resp) => {
                    // Untimed, as tmux's format_single_from_target is.
                    let text = crate::format::with_sessionless(true, || crate::format::expand_format_untimed(&fmt, &app));
                    let _ = resp.send(text);
                }
                // copied from server/mod.rs run_server (line 3734 at 86a78100)
                CtrlReq::ClientAttach(cid, pid) => {
                    lp.clients.insert(cid); // shared: frames go to this session's clients only
                    // Registration and the attached counter are one idempotent
                    // operation. A duplicate attach for the same connection
                    // must not leave the session permanently ghost-attached.
                    if app.register_client_with_pid(cid, false, pid) {
                        // A client that attaches here has just changed the
                        // session it is looking at, which is exactly when tmux
                        // fires it: server-client.c:448
                        // `server_client_set_session` calls
                        // `server_client_fire_session_changed(c, old)` at :472
                        // for every set, attach and switch-client alike, and
                        // that fires client-session-changed at :415. psmux runs
                        // one server per session, so the session a client
                        // switches TO is this server and the attach is the
                        // switch (issue #691).
                        notify_events.push("client-session-changed");
                        hook_event = Some("client-attached");
                        // update-environment from the ATTACHING client's
                        // environment (tmux cmd-attach-session.c environ_update
                        // from c->environ, #775), sent as `client-environ` just
                        // before the attach. This used to read the server's own
                        // environment, so it never changed anything, and it
                        // dropped a missing name instead of recording -NAME.
                        if let Some(src) = app.env_scopes.pending_client_environ.take() {
                            let patterns = app.update_environment.clone();
                            crate::environ::update_environment(&mut app.environment, &mut app.env_scopes, &patterns, &src, false);
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 3800 at 86a78100)
                CtrlReq::DumpLayout(resp) => {
                    let json = dump_layout_json(&mut app)?;
                    let _ = resp.send(json);
                }
                // copied from server/mod.rs run_server (line 4146 at 86a78100)
                CtrlReq::SendText(s) => {
                    // A copy-mode prompt is drawn by the client from the
                    // status message, so a key that only edits that prompt
                    // changes no pane cell: the frame diff is empty and
                    // nothing would be pushed until the client's once a
                    // second dump-state poll. Moving the cursor inside the
                    // prompt is exactly that case, so the push is owed
                    // whenever a prompt was open before the key or is open
                    // after it, which also covers the one that opens or
                    // closes it.
                    let prompt_before = crate::copy_mode::copy_prompt_text(&app.mode).is_some();
                    crate::server::helpers::clear_status_message_on_key(&mut app);
                    crate::input::stamp_interactive_text(&mut app);
                    // Typing into a direct-scrolled pane shows its live screen
                    // again, before the key can be taken by a mode it opens.
                    if crate::input::snap_interactive_to_live(&mut app) {
                        state_dirty = true;
                    }
                    send_text_to_active(&mut app, &s)?;
                    if prompt_before || crate::copy_mode::copy_prompt_text(&app.mode).is_some() {
                        state_dirty = true;
                    }
                    echo_pending_until = Some(Instant::now());
                }
                // copied from server/mod.rs run_server (line 4170 at 86a78100)
                CtrlReq::ClientActivity(cid) => {
                    // `#{client_activity}` is the last input from THIS client
                    // (tmux c->activity_time), not its last resize (#724).
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.last_activity = Instant::now();
                    }
                    // Typing, clicking, scrolling: this client is the one in
                    // use, so `window-size latest` must size the window for
                    // it.  The resize only runs when the geometry changes.
                    if crate::resize_window::note_client_activity(&mut app, cid) {
                        resize_all_panes(&mut app);
                        state_dirty = true;
                        meta_dirty = true;
                    }
                }
                // copied from server/mod.rs run_server (line 4185 at 86a78100)
                CtrlReq::PtyWake => { /* the wake itself is the whole point; the
                    PTY_DATA_READY swap above already set state_dirty. */ }
                // copied from server/mod.rs run_server (line 4187 at 86a78100)
                CtrlReq::SendKey(k) => {
                    // Same as SendText above: a prompt edit owes a frame even
                    // though no cell changed.
                    let prompt_before = crate::copy_mode::copy_prompt_text(&app.mode).is_some();
                    crate::pty_trace::mark("g", 0, k.as_bytes());
                    crate::server::helpers::clear_status_message_on_key(&mut app);
                    crate::input::stamp_interactive_key(&mut app, &k);
                    if crate::input::snap_interactive_to_live(&mut app) {
                        state_dirty = true;
                    }
                    send_key_to_active(&mut app, &k)?;
                    if prompt_before || crate::copy_mode::copy_prompt_text(&app.mode).is_some() {
                        state_dirty = true;
                    }
                    echo_pending_until = Some(Instant::now());
                }
                // copied from server/mod.rs run_server (line 4203 at 86a78100)
                CtrlReq::SendPaste(s) => {
                    if crate::input::snap_interactive_to_live(&mut app) {
                        state_dirty = true;
                    }
                    send_paste_to_active(&mut app, &s)?;
                    echo_pending_until = Some(Instant::now());
                }
                // copied from server/mod.rs run_server (line 4223 at 86a78100)
                CtrlReq::ZoomPane => {
                    toggle_zoom(&mut app);
                    // tmux window_zoom makes the zoomed pane its window's
                    // active pane (window_set_active_pane), so a `-t` zoom of
                    // another pane keeps that pane active once the request's
                    // temporary focus is put back; otherwise the window showed
                    // the zoomed pane while another one was active (#824).
                    let win = &mut app.windows[app.active_idx];
                    if win.zoom_saved.is_some() {
                        if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                            crate::tree::touch_mru(&mut win.pane_mru, pid);
                            if let Some(saved) = temp_focus_restore.as_mut() {
                                if saved.active_idx == app.active_idx {
                                    saved.pane_id = pid;
                                }
                            }
                        }
                    }
                    state_dirty = true; meta_dirty = true; hook_event = Some("after-resize-pane");
                }
                // tmux: the prefix forces a switch to the prefix table, which
                // drops any `switch-client -T` latch (issue #640).
                // copied from server/mod.rs run_server (line 4245 at 86a78100)
                CtrlReq::PrefixBegin => { app.client_prefix_active = true; app.current_key_table = None; state_dirty = true; }
                // copied from server/mod.rs run_server (line 4246 at 86a78100)
                CtrlReq::PrefixEnd => { app.client_prefix_active = false; state_dirty = true; }
                // copied from server/mod.rs run_server (line 4275 at 86a78100)
                CtrlReq::ClientSize(cid, w, h) => { 
                    app.client_sizes.insert(cid, (w, h));
                    app.latest_client_id = Some(cid);
                    app.latest_size_client_id = Some(cid);
                    app.client_area = Rect::new(0, 0, w, h);
                    // Update registry with new size and activity timestamp
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.width = w;
                        info.height = h;
                        info.last_activity = std::time::Instant::now();
                    }
                    crate::resize_window::refresh_dynamic_window_sizes(&mut app);
                    resize_all_panes(&mut app);
                    // Reconcile warm pane dimensions through the central
                    // policy module so resize uses the same code path as
                    // every other warm-pane invalidation (#271).
                    let sync = crate::warm_pane_sync::for_resize(
                        &app,
                        app.client_area.height,
                        app.client_area.width,
                    );
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    hook_event = Some("client-resized");
                }
                // copied from server/mod.rs run_server (line 4299 at 86a78100)
                CtrlReq::HostColors(spec) => {
                    // Issue #473: a client reported its host terminal's colors.
                    // Keep the most recent report: the newest attached client
                    // is what the user is actually looking at.
                    let hc = crate::types::HostColors::from_spec(&spec);
                    if hc.has_any() || hc.dark.is_some() {
                        let changed = app.host_colors.as_ref() != Some(&hc);
                        app.host_colors = Some(hc);
                        // Issue #556: pane reader threads answer color queries
                        // synchronously; publish the update where they can see it.
                        crate::types::set_shared_host_colors(app.host_colors.clone());
                        // The palette is also planted on every pane child as
                        // PSMUX_HOST_COLORS at spawn time, so the spares already
                        // sitting in the warm pool are now holding the wrong one
                        // (usually none at all: this report is how the server
                        // learns its colours in the first place). A child's
                        // environment cannot be rewritten from outside, so the
                        // only honest answer is to retire them.
                        if changed {
                            let sync = crate::warm_pane_sync::for_host_colors_change(&app);
                            crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                            // Remember it for the next server in this data root,
                            // so its spares are born with this palette and this
                            // retirement does not happen again. See
                            // `paths::host_colors_file`.
                            if let Some(ref hc) = app.host_colors {
                                let _ = std::fs::write(
                                    crate::paths::host_colors_file(),
                                    hc.to_spec(),
                                );
                            }
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 4394 at 86a78100)
                CtrlReq::ListWindows(resp) => { helpers::propagate_osc_titles(&mut app); let json = list_windows_json(&app)?; let _ = resp.send(json); }
                // copied from server/mod.rs run_server (line 4395 at 86a78100)
                CtrlReq::ListWindowsTmux(resp) => { helpers::propagate_osc_titles(&mut app); let text = list_windows_tmux(&app); let _ = resp.send(text); }
                // copied from server/mod.rs run_server (line 4396 at 86a78100)
                CtrlReq::ListWindowsFormat(resp, fmt) => { helpers::propagate_osc_titles(&mut app); let text = format_list_windows(&app, &fmt); let _ = resp.send(text); }
                // copied from server/mod.rs run_server (line 4397 at 86a78100)
                CtrlReq::ListTree(resp) => { let json = list_tree_json(&app)?; let _ = resp.send(json); }
                // copied from server/mod.rs run_server (line 4498 at 86a78100)
                CtrlReq::SendKeys(keys, literal) => {
                    // #828: tmux's send-keys hands its keys to the target
                    // pane (`window_pane_key`, window.c), and a popup, menu
                    // or confirm prompt belongs to a client, which only that
                    // client's own keystrokes reach (server_client_handle_key).
                    // So a send-keys aimed at a window or pane goes past the
                    // overlay to the pane, and the overlay stays open.  An
                    // untargeted or session-only send-keys still drives the
                    // overlay, as psmux scripts have long relied on (closing
                    // a popup with `send-keys Escape`).
                    let overlay = if names_pane && matches!(app.mode,
                        Mode::PopupMode { .. } | Mode::MenuMode { .. } | Mode::ConfirmMode { .. })
                    {
                        Some(std::mem::replace(&mut app.mode, Mode::Passthrough))
                    } else {
                        None
                    };
                    let in_copy = app.mode.in_copy();
                    if in_copy {
                        // In copy/search mode: route through mode-aware handlers
                        if literal {
                            send_text_to_active(&mut app, &keys.join(""))?;
                        } else {
                            // #490: `keys` holds the send-keys arguments as
                            // separate tokens. Match each WHOLE token as a
                            // named key or send it verbatim: never split a
                            // token on whitespace, which destroyed spacing
                            // inside quoted arguments.
                            let parts: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                            for key in parts.iter() {
                                let key_upper = key.to_uppercase();
                                let normalized = match key_upper.as_str() {
                                    "ENTER" => "enter",
                                    "TAB" => "tab",
                                    "BTAB" | "BACKTAB" => "btab",
                                    "ESCAPE" | "ESC" => "esc",
                                    "SPACE" => "space",
                                    "BSPACE" | "BACKSPACE" => "backspace",
                                    "UP" => "up",
                                    "DOWN" => "down",
                                    "RIGHT" => "right",
                                    "LEFT" => "left",
                                    "HOME" => "home",
                                    "END" => "end",
                                    "PAGEUP" | "PPAGE" => "pageup",
                                    "PAGEDOWN" | "NPAGE" => "pagedown",
                                    "DELETE" | "DC" => "delete",
                                    "INSERT" | "IC" => "insert",
                                    _ => "",
                                };
                                if !normalized.is_empty() {
                                    send_key_to_active(&mut app, normalized)?;
                                } else if key_upper.starts_with("C-") || key_upper.starts_with("M-") || (key_upper.starts_with("F") && key_upper.len() >= 2 && key_upper[1..].chars().all(|c| c.is_ascii_digit())) {
                                    send_key_to_active(&mut app, &key.to_lowercase())?;
                                } else {
                                    // Plain text char: route through send_text_to_active (handles copy mode chars)
                                    send_text_to_active(&mut app, key)?;
                                }
                            }
                        }
                    } else if literal {
                        send_text_to_active(&mut app, &keys.join(""))?;
                    } else {
                        // #490: `keys` holds the send-keys arguments as
                        // separate tokens. A token either matches a named key
                        // in its entirety or is typed verbatim with its
                        // whitespace intact; a single separator space is
                        // still inserted between adjacent PLAIN tokens for
                        // backward compatibility with multi word scripts
                        // (strict tmux would concatenate them).
                        let parts: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                        for (i, key) in parts.iter().enumerate() {
                            let key_upper = key.to_uppercase();
                            let _is_special = matches!(key_upper.as_str(), 
                                "ENTER" | "TAB" | "BTAB" | "BACKTAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                            ) || key_upper.starts_with("C-") || key_upper.starts_with("M-") || key_upper.starts_with("S-");

                            // Issue #860: a pane in an extended key mode gets
                            // tmux's extended form of a modified key
                            // (input-keys.c input_key) instead of the legacy
                            // byte or record the arms below write.
                            if key_upper.starts_with("C-") || key_upper.starts_with("M-") || key_upper.starts_with("S-")
                                || key_upper == "BTAB" || key_upper == "BACKTAB"
                            {
                                if let Some(bytes) = crate::input::active_extended_named_key_bytes(&mut app, key) {
                                    send_text_to_active(&mut app, &String::from_utf8_lossy(&bytes))?;
                                    continue;
                                }
                            }

                            match key_upper.as_str() {
                                "ENTER" => send_text_to_active(&mut app, "\r")?,
                                "TAB" => send_text_to_active(&mut app, "\t")?,
                                "BTAB" | "BACKTAB" => send_text_to_active(&mut app, "\x1b[Z")?,
                                "ESCAPE" | "ESC" => send_text_to_active(&mut app, "\x1b")?,
                                "SPACE" => send_text_to_active(&mut app, " ")?,
                                "BSPACE" | "BACKSPACE" => send_text_to_active(&mut app, "\x7f")?,
                                // DECCKM app-cursor mode: SS3, not CSI (see crate::input::csi_cursor_to_ss3).
                                "UP" => send_key_to_active(&mut app, "up")?,
                                "DOWN" => send_key_to_active(&mut app, "down")?,
                                "RIGHT" => send_key_to_active(&mut app, "right")?,
                                "LEFT" => send_key_to_active(&mut app, "left")?,
                                "HOME" => send_key_to_active(&mut app, "home")?,
                                "END" => send_key_to_active(&mut app, "end")?,
                                "PAGEUP" | "PPAGE" => send_text_to_active(&mut app, "\x1b[5~")?,
                                "PAGEDOWN" | "NPAGE" => send_text_to_active(&mut app, "\x1b[6~")?,
                                "DELETE" | "DC" => send_text_to_active(&mut app, "\x1b[3~")?,
                                "INSERT" | "IC" => send_text_to_active(&mut app, "\x1b[2~")?,
                                "F1" => send_text_to_active(&mut app, "\x1bOP")?,
                                "F2" => send_text_to_active(&mut app, "\x1bOQ")?,
                                "F3" => send_text_to_active(&mut app, "\x1bOR")?,
                                "F4" => send_text_to_active(&mut app, "\x1bOS")?,
                                "F5" => send_text_to_active(&mut app, "\x1b[15~")?,
                                "F6" => send_text_to_active(&mut app, "\x1b[17~")?,
                                "F7" => send_text_to_active(&mut app, "\x1b[18~")?,
                                "F8" => send_text_to_active(&mut app, "\x1b[19~")?,
                                "F9" => send_text_to_active(&mut app, "\x1b[20~")?,
                                "F10" => send_text_to_active(&mut app, "\x1b[21~")?,
                                "F11" => send_text_to_active(&mut app, "\x1b[23~")?,
                                "F12" => send_text_to_active(&mut app, "\x1b[24~")?,
                                // Modifier + special key combos (C-Left, S-Right, C-M-Up, etc.)
                                // must be checked BEFORE the generic C-x / M-x single-char handlers.
                                s if crate::input::parse_modified_special_key(s).is_some() => {
                                    let seq = crate::input::parse_modified_special_key(s).unwrap();
                                    send_text_to_active(&mut app, &seq)?;
                                }
                                s if s.starts_with("C-M-") || s.starts_with("C-m-") => {
                                    if let Some(c) = key.chars().nth(4) {
                                        if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                            send_text_to_active(&mut app, &format!("\x1b{}", ctrl as char))?;
                                        }
                                    }
                                }
                                // Ctrl+Shift+<punctuation/digit> that collapses to a single
                                // C0 byte, e.g. Ctrl+/ delivered by ConPTY terminals
                                // (Alacritty, WezTerm) as "C-S--" (VK_OEM_MINUS + Ctrl +
                                // Shift).  It must reach the child as 0x1f (^_), matching
                                // Ctrl+_ and tmux, so neovim's Ctrl+/ comment toggle fires
                                // (issue #394).  This MUST precede the generic C- arm below,
                                // whose nth(2) extraction would otherwise read the 'S' and
                                // mis-send Ctrl+S.
                                s if (s.starts_with("C-S-") || s.starts_with("C-s-"))
                                    && s.chars().count() == 5
                                    && s.chars().nth(4).map_or(false, |c| !c.is_ascii_alphabetic()) =>
                                {
                                    if let Some(c) = s.chars().nth(4) {
                                        // Issue #623: Ctrl+Shift+<digit> (Far's folder
                                        // shortcuts) has no VT encoding either.
                                        #[cfg(windows)]
                                        let handled = match ctrl_key_record_seq_for_active(&mut app, c, true) {
                                            Some(seq) => {
                                                send_text_to_active(&mut app, &seq)?;
                                                crate::input::mark_win32_input_latched(&mut app);
                                                true
                                            }
                                            None => false,
                                        };
                                        #[cfg(not(windows))]
                                        let handled = false;
                                        if !handled {
                                            if let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) {
                                                send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                            }
                                        }
                                    }
                                }
                                // Ctrl+Space by name: a space cannot survive as the
                                // third character of "C-x" on a whitespace split
                                // command line, and the generic C- arm below would read
                                // the 'S' of "C-Space" and send Ctrl+S.  tmux byte: NUL.
                                // This match is on `key_upper`, so the only spelling
                                // that can ever arrive is "C-SPACE"; the mixed case
                                // forms alone never matched and `send-keys C-Space`
                                // wrote 0x13, an XOFF (issue #860).
                                "C-SPACE" => {
                                    #[cfg(windows)]
                                    let handled = match ctrl_key_record_seq_for_active(&mut app, ' ', false) {
                                        Some(seq) => {
                                            send_text_to_active(&mut app, &seq)?;
                                            crate::input::mark_win32_input_latched(&mut app);
                                            true
                                        }
                                        None => false,
                                    };
                                    #[cfg(not(windows))]
                                    let handled = false;
                                    if !handled {
                                        send_text_to_active(&mut app, "\x00")?;
                                    }
                                }
                                s if s.starts_with("C-") => {
                                    if let Some(c) = s.chars().nth(2) {
                                        let Some(ctrl) = crate::input::ctrl_char_send_keys_byte(c) else { continue };
                                        // On Windows with Win32 input mode, write the key as
                                        // a Win32 input mode escape sequence so ConPTY generates
                                        // a proper KEY_EVENT with VK + LEFT_CTRL_PRESSED (#305).
                                        #[cfg(windows)]
                                        {
                                            if c.is_ascii_alphabetic() {
                                                // Keep Ctrl+C on the legacy interrupt path:
                                                // raw 0x03 + the interrupt router. The router
                                                // runs BEFORE the byte: when it decides "raw
                                                // 0x03 only" it may strip PROCESSED_INPUT from
                                                // the pane console so conhost delivers the byte
                                                // as input instead of converting it into a
                                                // console-wide CTRL_C_EVENT that aborts a
                                                // booting WSL launch (#579).
                                                if ctrl == 0x03 {
                                                    if let Some(win) = app.windows.get_mut(app.active_idx) {
                                                        // The console signal bypasses the pane
                                                        // writer, so it needs the input-off gate
                                                        // send_text_to_active applies to the byte.
                                                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path).filter(|p| p.accepts_input()) {
                                                            if p.child_pid.is_none() {
                                                                p.child_pid = crate::platform::mouse_inject::get_child_pid(&*p.child);
                                                            }
                                                            if let Some(pid) = p.child_pid {
                                                                crate::platform::mouse_inject::send_ctrl_c_event(pid, false);
                                                            }
                                                        }
                                                    }
                                                    send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                                } else if !crate::input::active_pane_host_parses_win32_input(&mut app) {
                                                    // Issue #845: this pane's console host
                                                    // predates win32 input mode (it never
                                                    // asked for ?9001h) and would drop or
                                                    // print the record, so the key goes as
                                                    // tmux's control byte.
                                                    send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                                } else {
                                                    let vk = crate::platform::mouse_inject::char_to_vk(c);
                                                    let scan = crate::platform::mouse_inject::vk_to_scan(vk);
                                                    let u_char = (c.to_ascii_lowercase() as u16) & 0x1F;
                                                    const LEFT_CTRL_PRESSED: u32 = 0x0008;
                                                    let seq = crate::input::win32_input_key_seq(
                                                        vk, scan, u_char, LEFT_CTRL_PRESSED,
                                                    );
                                                    send_text_to_active(&mut app, &seq)?;
                                                    // Writing this LATCHES the receiving ConPTY into
                                                    // win32 input mode for good: conhost stops
                                                    // dispatching a dangling ESC as the Escape key,
                                                    // so every later bare 0x1b would vanish (#588).
                                                    // Remember it, so a lone Escape is written in
                                                    // win32 form too from here on.
                                                    crate::input::mark_win32_input_latched(&mut app);
                                                }
                                            } else if let Some(seq) =
                                                ctrl_key_record_seq_for_active(&mut app, c, false)
                                            {
                                                // Issue #623: Ctrl+<digit> has no VT
                                                // encoding at all, so a record reading
                                                // pane (Far Manager) was handed the
                                                // bare digit and could not tell it from
                                                // an unmodified keypress.
                                                send_text_to_active(&mut app, &seq)?;
                                                crate::input::mark_win32_input_latched(&mut app);
                                            } else {
                                                send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                            }
                                        }
                                        #[cfg(not(windows))]
                                        send_text_to_active(&mut app, &String::from(ctrl as char))?;
                                    }
                                }
                                s if s.starts_with("M-") => {
                                    if let Some(c) = key.chars().nth(2) {
                                        send_text_to_active(&mut app, &format!("\x1b{}", c))?;
                                    }
                                }
                                _ => {
                                    // Plain token: typed VERBATIM (#490: the
                                    // token's own whitespace is untouched).
                                    // Keep the historical single separator
                                    // space between two adjacent plain tokens
                                    // so existing multi word scripts like
                                    // `send-keys echo hi Enter` keep working.
                                    send_text_to_active(&mut app, key)?;
                                    if i + 1 < parts.len() {
                                        let next_upper = parts[i + 1].to_uppercase();
                                        let next_is_special = matches!(next_upper.as_str(),
                                            "ENTER" | "TAB" | "BTAB" | "BACKTAB" | "ESCAPE" | "ESC" | "SPACE" | "BSPACE" | "BACKSPACE" |
                                            "UP" | "DOWN" | "RIGHT" | "LEFT" | "HOME" | "END" |
                                            "PAGEUP" | "PPAGE" | "PAGEDOWN" | "NPAGE" | "DELETE" | "DC" | "INSERT" | "IC" |
                                            "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
                                        ) || next_upper.starts_with("C-") || next_upper.starts_with("M-") || next_upper.starts_with("S-");
                                        if !next_is_special {
                                            send_text_to_active(&mut app, " ")?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    echo_pending_until = Some(Instant::now());
                    if let Some(m) = overlay {
                        app.mode = m;
                    }
                }
                // copied from server/mod.rs run_server (line 4495 at 86a78100)
                CtrlReq::SendBytes(bytes) => {
                    send_bytes_to_active(&mut app, &bytes)?;
                }
                // copied from server/mod.rs run_server (line 4799 at 86a78100)
                CtrlReq::SendKeysX(cmd) => {
                    // A copy-mode key binding's `send-keys -X` (queued by
                    // input::run_copy_mode_binding, so the pane is in copy mode).
                    let _ = run_send_keys_x(&mut app, &cmd, 1);
                }
                // copied from server/mod.rs run_server (line 4804 at 86a78100)
                CtrlReq::SendKeysXRun { cmd, count, resp } => {
                    let outcome = run_send_keys_x(&mut app, &cmd, count);
                    if let Some(resp) = resp { let _ = resp.send(outcome); }
                }
                // copied from server/mod.rs run_server (line 4808 at 86a78100)
                CtrlReq::SetPaneInputOff { target, off, last, resp } => {
                    // select-pane -d / -e.  No hook: tmux returns from
                    // cmd-select-pane.c right after flipping the flag, before
                    // the activation that fires `after-select-pane`.
                    let outcome = set_pane_input_off(&mut app, target.as_ref(), off, last);
                    match resp {
                        Some(r) => { let _ = r.send(outcome); }
                        None => {
                            if let Err(e) = outcome {
                                app.status_message = Some((e, Instant::now(), None));
                            }
                        }
                    }
                    // `#{pane_input_off}` may be on the status line.
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 4825 at 86a78100)
                CtrlReq::SelectPane(dir, keep_zoom) => {
                    if let Some(cmds) = app.hooks.get("before-select-pane") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    let _pane_before = active_pane_identity(&app);
                    // Auto-unzoom when navigating to another pane (tmux behavior).
                    // For directional nav: unzoom first so compute_rects uses
                    // real geometry, then re-zoom only if focus didn't change.
                    // For other cases: only unzoom if focus actually changes.
                    // (fixes #46)
                    match dir.as_str() {
                        "U" | "D" | "L" | "R" => {
                            let focus_dir = match dir.as_str() {
                                "U" => FocusDir::Up, "D" => FocusDir::Down,
                                "L" => FocusDir::Left, _ => FocusDir::Right,
                            };
                            if keep_zoom {
                                let old_path = app.windows[app.active_idx].active_path.clone();
                                switch_with_copy_save(&mut app, |app| {
                                    move_focus_preserving_zoom(app, focus_dir);
                                });
                                if app.windows[app.active_idx].active_path != old_path {
                                    app.last_pane_path = old_path;
                                }
                            } else {
                                let was_zoomed = unzoom_if_zoomed(&mut app);
                                if was_zoomed {
                                // Zoom-aware: check direct neighbor or wrap target (tmux parity: unzoom+wrap).
                                let win = &app.windows[app.active_idx];
                                let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
                                crate::tree::compute_rects(&win.root, app.last_window_area, &mut rects);
                                let active_idx = rects.iter().position(|(path, _)| *path == win.active_path);
                                let has_target = 
                                    if let Some(ai) = active_idx {
                                        let (_, arect) = &rects[ai];
                                        find_best_pane_in_direction(&rects, ai, arect, focus_dir, &[], &[])
                                            .or_else(|| find_wrap_target(&rects, ai, arect, focus_dir, &[], &[]))
                                            .is_some()
                                    } else { false };
                                    if has_target {
                                        let old_path = app.windows[app.active_idx].active_path.clone();
                                        switch_with_copy_save(&mut app, |app| {
                                            move_focus(app, focus_dir);
                                        });
                                        app.last_pane_path = old_path;
                                    } else {
                                        // No reachable pane (single-pane window): re-zoom
                                        toggle_zoom(&mut app);
                                    }
                                } else {
                                    let old_path = app.windows[app.active_idx].active_path.clone();
                                    switch_with_copy_save(&mut app, |app| {
                                        move_focus(app, focus_dir);
                                    });
                                    if app.windows[app.active_idx].active_path != old_path {
                                        app.last_pane_path = old_path;
                                    }
                                }
                            }
                        }
                        "last" => {
                            // select-pane -l: switch to last active pane.  The
                            // target (and the `no last pane` refusal) is
                            // decided in one place, `last_pane_path`, which is
                            // tmux's cmd-select-pane.c:165-177; the request
                            // that carries the diagnostic to the caller is
                            // `CtrlReq::LastPane` (#693 item 5).
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            let target = last_pane_path(&app);
                            switch_with_copy_save(&mut app, |app| {
                                if let Some(t) = target {
                                    let win = &mut app.windows[app.active_idx];
                                    let tmp = win.active_path.clone();
                                    win.active_path = t;
                                    app.last_pane_path = tmp;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                // Update MRU for the newly focused pane
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "mark" => {
                            // select-pane -m: mark the current pane
                            let win = &app.windows[app.active_idx];
                            if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                app.marked_pane = Some((app.active_idx, pid));
                            }
                        }
                        "next" => {
                            // select-pane next: cycle to next pane (like Prefix+o / tmux -t :.+)
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            switch_with_copy_save(&mut app, |app| {
                                let win = &app.windows[app.active_idx];
                                let mut pane_paths = Vec::new();
                                let mut path = Vec::new();
                                collect_pane_paths_server(&win.root, &mut path, &mut pane_paths);
                                if let Some(cur) = pane_paths.iter().position(|p| *p == win.active_path) {
                                    let next = (cur + 1) % pane_paths.len();
                                    let new_path = pane_paths[next].clone();
                                    let win = &mut app.windows[app.active_idx];
                                    app.last_pane_path = win.active_path.clone();
                                    win.active_path = new_path;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "prev" => {
                            // select-pane prev: cycle to previous pane (tmux -t :.-)
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            switch_with_copy_save(&mut app, |app| {
                                let win = &app.windows[app.active_idx];
                                let mut pane_paths = Vec::new();
                                let mut path = Vec::new();
                                collect_pane_paths_server(&win.root, &mut path, &mut pane_paths);
                                if let Some(cur) = pane_paths.iter().position(|p| *p == win.active_path) {
                                    let prev = (cur + pane_paths.len() - 1) % pane_paths.len();
                                    let new_path = pane_paths[prev].clone();
                                    let win = &mut app.windows[app.active_idx];
                                    app.last_pane_path = win.active_path.clone();
                                    win.active_path = new_path;
                                }
                            });
                            if app.windows[app.active_idx].active_path != old_path {
                                let win = &mut app.windows[app.active_idx];
                                if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                    crate::tree::touch_mru(&mut win.pane_mru, pid);
                                }
                                unzoom_if_zoomed(&mut app);
                            }
                        }
                        "unmark" => {
                            // select-pane -M: clear the marked pane
                            app.marked_pane = None;
                        }
                        _ => {}
                    }
                    meta_dirty = true;
                    // tmux fires `after-select-pane` from the activation at
                    // cmd-select-pane.c:276, which line :269 skips when the
                    // pane did not move; `-m` and `-M` return earlier still
                    // (cmd-select-pane.c:100) and never reach it, and neither
                    // do `-e` and `-d` (:180), which are not this request at
                    // all but `CtrlReq::SetPaneInputOff`.
                    if active_pane_identity(&app) != _pane_before {
                        note_pane_focus_change(&app, &mut notify_events);
                        hook_event = Some("after-select-pane");
                    }
                }
                // copied from server/mod.rs run_server (line 5006 at 86a78100)
                CtrlReq::ListPanes(resp) => {
                    helpers::propagate_osc_titles(&mut app);
                    let mut output = String::new();
                    let win = &app.windows[app.active_idx];
                    fn collect_panes(node: &Node, panes: &mut Vec<(usize, u16, u16, vt100::MouseProtocolMode, vt100::MouseProtocolEncoding, bool, usize, usize)>) {
                        match node {
                            Node::Leaf(p) => {
                                let (mode, enc, alt, hist_rows, hist_bytes) = match p.term.lock() {
                                    Ok(term) => {
                                        let screen = term.screen();
                                        (screen.mouse_protocol_mode(), screen.mouse_protocol_encoding(), screen.alternate_screen(), screen.scrollback_filled(), screen.history_bytes())
                                    }
                                    Err(_) => {
                                        // Mutex poisoned: reader thread panicked.  Use safe defaults.
                                        (vt100::MouseProtocolMode::None, vt100::MouseProtocolEncoding::Default, false, 0, 0)
                                    }
                                };
                                panes.push((p.id, p.last_cols, p.last_rows, mode, enc, alt, hist_rows, hist_bytes));
                            }
                            Node::Split { children, .. } => {
                                for c in children { collect_panes(c, panes); }
                            }
                        }
                    }
                    let mut panes = Vec::new();
                    collect_panes(&win.root, &mut panes);
                    // A focused floating pane is the active pane (tmux makes it
                    // w->active), so the tiled pane behind it is not (#767).
                    let focused_float = win.floating_focus.filter(|fi| *fi < win.floating.len());
                    let active_pane_id = match focused_float {
                        Some(fi) => Some(win.floating[fi].id),
                        None => crate::tree::get_active_pane_id(&win.root, &win.active_path),
                    };
                    let tiled_count = panes.len();
                    for (pos, (id, cols, rows, _mode, _enc, _alt, hist_rows, hist_bytes)) in panes.iter().enumerate() {
                        let idx = pos + app.pane_base_index_of(app.active_idx);
                        let active_marker = if active_pane_id == Some(*id) { " (active)" } else { "" };
                        // The rows retained and the bytes they cost, like tmux.
                        // Both used to be faked (the limit, and a literal 0),
                        // which hid the scrollback growth behind issue #641.
                        output.push_str(&format!("{}: [{}x{}] [history {}/{}, {} bytes] %{}{}\n", idx, cols, rows, hist_rows, app.history_limit, hist_bytes, id, active_marker));
                    }
                    // Floating panes follow the tiled ones, in tmux's own
                    // default line: cmd-list-panes.c adds
                    // `#{?pane_floating_flag, #{pane_x},#{pane_y},#{pane_z}}`
                    // inside the size brackets (#767). They were not listed.
                    let nfloat = win.floating.len();
                    for (fi, fp) in win.floating.iter().enumerate() {
                        let idx = tiled_count + fi + app.pane_base_index_of(app.active_idx);
                        let (hist_rows, hist_bytes) = match fp.pane.term.lock() {
                            Ok(term) => (term.screen().scrollback_filled(), term.screen().history_bytes()),
                            Err(_) => (0, 0),
                        };
                        let (ox, oy) = if fp.border == "none" { (fp.x, fp.y) } else { (fp.x + 1, fp.y + 1) };
                        let px = win.area.x + ox;
                        let py = win.area.y + oy;
                        let pz = nfloat - 1 - fi;
                        let active_marker = if active_pane_id == Some(fp.id) { " (active)" } else { "" };
                        output.push_str(&format!("{}: [{}x{} {},{},{}] [history {}/{}, {} bytes] %{}{}\n",
                            idx, fp.pane.last_cols, fp.pane.last_rows, px, py, pz,
                            hist_rows, app.history_limit, hist_bytes, fp.id, active_marker));
                    }
                    let _ = resp.send(output);
                }
                // copied from server/mod.rs run_server (line 5070 at 86a78100)
                CtrlReq::ListPanesFormat(resp, fmt) => {
                    helpers::propagate_osc_titles(&mut app);
                    let text = format_list_panes(&app, &fmt, app.active_idx);
                    let _ = resp.send(text);
                }
                // copied from server/mod.rs run_server (line 5075 at 86a78100)
                CtrlReq::ListAllPanes(resp) => {
                    let mut output = String::new();
                    fn collect_all_panes(node: &Node, panes: &mut Vec<(usize, u16, u16)>) {
                        match node {
                            Node::Leaf(p) => { panes.push((p.id, p.last_cols, p.last_rows)); }
                            Node::Split { children, .. } => { for c in children { collect_all_panes(c, panes); } }
                        }
                    }
                    for (wi, win) in app.windows.iter().enumerate() {
                        let mut panes = Vec::new();
                        collect_all_panes(&win.root, &mut panes);
                        // Floating panes are panes of the window too (#767).
                        panes.extend(win.floating.iter().map(|fp| (fp.id, fp.pane.last_cols, fp.pane.last_rows)));
                        for (id, cols, rows) in panes {
                            output.push_str(&format!("{}:{}: %{} [{}x{}]\n", app.session_name, app.win_display_index(wi), id, cols, rows));
                        }
                    }
                    let _ = resp.send(output);
                }
                // copied from server/mod.rs run_server (line 5094 at 86a78100)
                CtrlReq::ListAllPanesFormat(resp, fmt) => {
                    let mut lines = Vec::new();
                    for wi in 0..app.windows.len() {
                        lines.push(format_list_panes(&app, &fmt, wi));
                    }
                    let _ = resp.send(lines.join("\n"));
                }
                // copied from server/mod.rs run_server (line 5769 at 86a78100)
                CtrlReq::DisplayMessage(resp, fmt, target_pane_idx, set_status_bar, duration_ms, client_sel, flags) => {
                    // Propagate OSC titles so #{pane_title} reflects latest state
                    helpers::propagate_osc_titles(&mut app);
                    // `-c` picks the client the client_* variables describe. A
                    // client tmux cannot find is not an error for this command
                    // (CMD_CLIENT_CANFAIL): the best client answers instead.
                    let client = client_sel.as_ref().and_then(|s| crate::format::resolve_client_sel(&app, s));
                    let result = if flags.literal {
                        fmt.clone()
                    } else {
                        crate::format::with_format_client(client, || if let Some(pane_idx) = target_pane_idx {
                            // -t targeting: evaluate format for the specific pane
                            // using PANE_POS_OVERRIDE so #{pane_active} reflects
                            // the REAL active pane, not the target (#113)
                            match pane_idx.checked_sub(app.pane_base_index_of(app.active_idx)) {
                                Some(pos) => crate::format::expand_format_for_pane(&fmt, &app, app.active_idx, pos),
                                None => String::new(),
                            }
                        } else {
                            expand_format(&fmt, &app)
                        })
                    };
                    if set_status_bar {
                        let shown_at = Instant::now();
                        app.status_message = Some((result.clone(), shown_at, duration_ms));
                        app.status_message_ignore_keys = flags.ignore_keys.then_some(shown_at);
                        state_dirty = true;
                    }
                    let _ = resp.send(result);
                }
                // copied from server/mod.rs run_server (line 5799 at 86a78100)
                CtrlReq::DisplayMessageById(resp, fmt, pane_id, set_status_bar, duration_ms, client_sel, flags) => {
                    // Bare %N pane targeting (#332): resolve the pane ID
                    // globally across all windows and expand the format with
                    // PANE_POS_OVERRIDE pointing at it.
                    helpers::propagate_osc_titles(&mut app);
                    let client = client_sel.as_ref().and_then(|s| crate::format::resolve_client_sel(&app, s));
                    let result = if flags.literal {
                        fmt.clone()
                    } else {
                        crate::format::with_format_client(client, || {
                            crate::format::expand_format_for_pane_by_id(&fmt, &app, pane_id)
                        })
                    };
                    if set_status_bar {
                        let shown_at = Instant::now();
                        app.status_message = Some((result.clone(), shown_at, duration_ms));
                        app.status_message_ignore_keys = flags.ignore_keys.then_some(shown_at);
                        state_dirty = true;
                    }
                    let _ = resp.send(result);
                }
                // copied from server/mod.rs run_server (line 5101 at 86a78100)
                CtrlReq::KillWindow => {
                    let active = app.active_idx;
                    kill_window_at(&mut app, active);
                    // Killing a window changes the active window and the window
                    // list, so resize the now-active window's panes and force a
                    // status-bar/window-list rebuild + push to attached clients.
                    // Without meta_dirty the cached window tabs stay stale and
                    // without state_dirty the no-change fast path skips the frame,
                    // leaving the bottom bar showing the killed window until the
                    // next input (issue #359). Mirrors every other structural
                    // mutation (new-window, kill-pane, select-window) and tmux's
                    // server_kill_window -> server_redraw_session_group.
                    resize_all_panes(&mut app);
                    meta_dirty = true;
                    state_dirty = true;
                    // A killed window leaves the session's window list, which
                    // is what tmux calls unlinking: session.c:349
                    // `session_detach` fires window-unlinked before it removes
                    // the winlink (issue #691).
                    notify_events.push("window-unlinked");
                    hook_event = Some("window-closed");
                }
                // copied from server/mod.rs run_server (line 5123 at 86a78100)
                CtrlReq::KillWindowTarget { win, win_is_id, name, resp } => {
                    // Resolve the target here, on live state. An unresolvable
                    // target must be an error, never a fallback to the active
                    // window: the old temp-focus-then-kill dance silently
                    // no-opped the focus on a bad name/index/@id and then
                    // killed whatever was focused (session death when it was
                    // the last window). tmux: "can't find window: X", kills
                    // nothing, exit 1.
                    let resolved = if let Some(w) = win {
                        if win_is_id {
                            app.windows.iter().position(|x| x.id == w)
                        } else {
                            app.win_pos(w)
                        }
                    } else if let Some(ref n) = name {
                        app.windows.iter().position(|x| x.name == *n)
                    } else {
                        Some(app.active_idx)
                    };
                    match resolved {
                        Some(pos) => {
                            kill_window_at(&mut app, pos);
                            resize_all_panes(&mut app);
                            meta_dirty = true;
                            state_dirty = true;
                            notify_events.push("window-unlinked");
                            hook_event = Some("window-closed");
                            let _ = resp.send(Ok(()));
                        }
                        None => {
                            let spec = if let Some(w) = win {
                                if win_is_id { format!("@{}", w) } else { w.to_string() }
                            } else {
                                name.clone().unwrap_or_default()
                            };
                            let msg = format!("can't find window: {}", spec);
                            // Surface in the status bar for attached clients,
                            // same convention as join-pane (#437).
                            app.status_message = Some((msg.clone(), Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 4339 at 86a78100)
                CtrlReq::FocusWindowCmd(wid) => { switch_with_copy_save(&mut app, |app| { if let Some(idx) = find_window_index_by_id(app, wid) { app.active_idx = idx; } }); resize_all_panes(&mut app); meta_dirty = true; }
                // copied from server/mod.rs run_server (line 4333 at 86a78100)
                CtrlReq::FocusPaneCmd(pid) => {
                    let old_path = app.windows[app.active_idx].active_path.clone();
                    switch_with_copy_save(&mut app, |app| { focus_pane_by_id(app, pid); });
                    if app.windows[app.active_idx].active_path != old_path { unzoom_if_zoomed(&mut app); }
                    meta_dirty = true;
                }
                // copied from server/mod.rs run_server (line 3540 at 86a78100)
                CtrlReq::FocusWindow(wid) => {
                    // wid is a display index (same as tmux window number), convert to internal array index
                    if let Some(internal_idx) = app.win_pos(wid) {
                        focus_window_at(&mut app, internal_idx);
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                // copied from server/mod.rs run_server (line 3555 at 86a78100)
                CtrlReq::FocusWindowById(id) => {
                    if let Some(internal_idx) = app.windows.iter().position(|w| w.id == id) {
                        focus_window_at(&mut app, internal_idx);
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                // One request for the whole `-t` of a `select-pane` (#691).
                // copied from server/mod.rs run_server (line 3548 at 86a78100)
                CtrlReq::FocusWindowByName(ref name) => {
                    if let Some(internal_idx) = app.windows.iter().position(|w| w.name == *name) {
                        focus_window_at(&mut app, internal_idx);
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                // copied from server/mod.rs run_server (line 3651 at 86a78100)
                CtrlReq::FocusPane(pid) => {
                    let old_path = app.windows[app.active_idx].active_path.clone();
                    switch_with_copy_save(&mut app, |app| { focus_pane_by_id(app, pid); });
                    if app.windows[app.active_idx].active_path != old_path { unzoom_if_zoomed(&mut app); }
                    meta_dirty = true;
                }
                // copied from server/mod.rs run_server (line 4374 at 86a78100)
                CtrlReq::NextWindow => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                }
                // copied from server/mod.rs run_server (line 4378 at 86a78100)
                CtrlReq::PrevWindow => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                }
                // copied from server/mod.rs run_server (line 5820 at 86a78100)
                CtrlReq::LastWindow { resp } => {
                    match app.last_window_pos() {
                        Some(last) => {
                            switch_with_copy_save(&mut app, |app| {
                                let tmp = app.active_idx;
                                app.active_idx = last;
                                app.last_window_idx = tmp;
                            });
                            meta_dirty = true;
                            hook_event = Some("after-select-window");
                            if let Some(r) = resp { let _ = r.send(Ok(())); }
                        }
                        // cmd-select-window.c:118-120.  psmux exited 0 in
                        // silence here, with no window to go back to.
                        None => {
                            if let Some(r) = resp { let _ = r.send(Err("no last window".to_string())); }
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 5839 at 86a78100)
                CtrlReq::LastPane { resp } => {
                    match last_pane_path(&app) {
                        Some(target) => {
                            switch_with_copy_save(&mut app, |app| {
                                let win = &mut app.windows[app.active_idx];
                                let tmp = win.active_path.clone();
                                win.active_path = target;
                                app.last_pane_path = tmp;
                            });
                            let win = &mut app.windows[app.active_idx];
                            if let Some(pid) = get_active_pane_id(&win.root, &win.active_path) {
                                crate::tree::touch_mru(&mut win.pane_mru, pid);
                            }
                            unzoom_if_zoomed(&mut app);
                            meta_dirty = true;
                            note_pane_focus_change(&app, &mut notify_events);
                            let _ = resp.send(Ok(()));
                        }
                        // cmd-select-pane.c:175-177.  psmux exited 0 in silence
                        // here (issue #693 item 5).
                        None => { let _ = resp.send(Err("no last pane".to_string())); }
                    }
                }
                // copied from server/mod.rs run_server (line 4382 at 86a78100)
                CtrlReq::RenameWindow(name) => {
                    if let Some(cmds) = app.hooks.get("before-rename-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    // tmux parity (#552): the argument is a format, expanded
                    // against the target window (cmd-rename-window.c runs it
                    // through format_single_from_target). The temporary -t
                    // focus has already made the target the active window
                    // here, so #{window_name} etc. resolve against it :
                    // enabling idioms like `rename-window '#{b:pane_current_path}'`.
                    let name = expand_format(&name, &app);
                    let name = crate::util::clean_name(&name);
                    let win = &mut app.windows[app.active_idx]; win.name = name; win.manual_rename = true; meta_dirty = true; hook_event = Some("after-rename-window");
                }
                // copied from server/mod.rs run_server (line 7711 at 86a78100)
                CtrlReq::ClientEnviron(env) => {
                    app.env_scopes.pending_client_environ = Some(env);
                }
                // copied from server/mod.rs run_server (line 7143 at 86a78100)
                CtrlReq::SetClientReadonly(cid, readonly) => {
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.readonly = readonly;
                    }
                }
                // copied from server/mod.rs run_server (line 7373 at 86a78100)
                CtrlReq::SetClientLastSession(cid, prev) => {
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.last_session = Some(prev);
                    }
                }
                // copied from server/mod.rs run_server (line 4403 at 86a78100)
                CtrlReq::WindowDump(wid, format, resp) => {
                    let result = if format == crate::types::WindowDumpFormat::Layout {
                        crate::layout::dump_window_layout_json(&mut app, wid)
                    } else {
                        crate::layout::dump_window_layout(&mut app, wid)
                            .and_then(|layout| {
                                let border_lines =
                                    options::get_option_value(&app, "pane-border-lines");
                                let border_indicators =
                                    crate::pane_border::PaneBorderIndicators::parse(
                                        &options::get_option_value(
                                            &app,
                                            "pane-border-indicators",
                                        ),
                                    )
                                    .map_err(std::io::Error::other)?;
                                let window_index = app.windows
                                    .iter()
                                    .position(|window| window.id == wid)
                                    .ok_or_else(|| std::io::Error::other("window not found"))?;
                                let _async_fmt = crate::format::AsyncFormatGuard::new();
                                let pane_border_style = if app.pane_border_style.is_empty() {
                                    String::new()
                                } else {
                                    crate::format::expand_format_for_window(
                                        &app.pane_border_style,
                                        &app,
                                        window_index,
                                    )
                                };
                                let pane_active_border_style =
                                    if app.pane_active_border_style.is_empty() {
                                        String::new()
                                    } else {
                                        crate::format::expand_format_for_window(
                                            &app.pane_active_border_style,
                                            &app,
                                            window_index,
                                        )
                                    };
                                let floating_pane_focused =
                                    app.windows[window_index].floating_focus.is_some();
                                serde_json::to_string(&crate::types::PreviewWindowState {
                                    layout,
                                    border_lines,
                                    border_indicators,
                                    pane_border_style: Some(pane_border_style),
                                    pane_active_border_style: Some(pane_active_border_style),
                                    floating_pane_focused,
                                })
                                .map_err(std::io::Error::other)
                            })
                    };
                    let json = result.unwrap_or_else(|_| "{}".to_string());
                    let _ = resp.send(json);
                }
                // copied from server/mod.rs run_server (line 4398 at 86a78100)
                CtrlReq::WindowLayout(wid, resp) => {
                    let json = crate::util::window_layout_json(&app, wid)
                        .unwrap_or_else(|_| "{}".to_string());
                    let _ = resp.send(json);
                }
                // copied from server/mod.rs run_server (line 4460 at 86a78100)
                CtrlReq::SetPaneTitle(title) => {
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        p.title_locked = !title.is_empty();
                        p.title = title;
                    }
                    meta_dirty = true;
                }
                // copied from server/mod.rs run_server (line 4459 at 86a78100)
                CtrlReq::ToggleSync => { app.sync_input = !app.sync_input; }
                // copied from server/mod.rs run_server (line 4372 at 86a78100)
                CtrlReq::SplitSetSizes(cid, path, sizes) => { if app.mouse_enabled { app.latest_client_id = Some(cid); handle_split_set_sizes(&mut app, &path, &sizes); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4373 at 86a78100)
                CtrlReq::SplitResizeDone(cid) => { if app.mouse_enabled { app.latest_client_id = Some(cid); handle_split_resize_done(&mut app); state_dirty = true; meta_dirty = true; } }
                // copied from server/mod.rs run_server (line 4982 at 86a78100)
                CtrlReq::SelectWindow(idx) => {
                    if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    if let Some(internal_idx) = app.win_pos(idx) {
                        if internal_idx != app.active_idx {
                            switch_with_copy_save(&mut app, |app| {
                                app.last_window_idx = app.active_idx;
                                app.active_idx = internal_idx;
                            });
                            // Clear activity/bell/silence flags on the newly
                            // focused window.  This request is now the only one
                            // a `select-window -t <index>` sends (#690), so the
                            // clearing FocusWindow used to do has to happen
                            // here or a selected window would keep its alert.
                            if let Some(win) = app.windows.get_mut(internal_idx) {
                                win.activity_flag = false;
                                win.bell_flag = false;
                                win.silence_flag = false;
                            }
                            resize_all_panes(&mut app);
                        }
                    }
                    meta_dirty = true;
                    hook_event = Some("after-select-window");
                }
                // copied from server/mod.rs run_server (line 8005 at 86a78100)
                CtrlReq::ScopeSessionListing(resp, text, global, mark_inherited) => {
                    let _ = resp.send(crate::server::options::scope_session_listing(&app, &text, global, mark_inherited));
                }
                // copied from server/mod.rs run_server (line 8002 at 86a78100)
                CtrlReq::SessionOptionQuery(resp, name, global) => {
                    let _ = resp.send(crate::server::options::session_option_query(&app, &name, global));
                }
                // copied from server/mod.rs run_server (line 6107 at 86a78100)
                CtrlReq::SetOption(option, value) => {
                    if apply_set_option(&mut app, &option, &value, false).is_ok() {
                        app.user_set_options.insert(option.clone());
                        // Reconcile the warm pane with the new option value.
                        // All option-driven warm-pane lifecycle decisions
                        // route through this single module, see #271.
                        let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                        crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                        // Update shared aliases if command-alias changed
                        if option == "command-alias" {
                            if let Ok(mut map) = shared_aliases_main.write() {
                                *map = app.command_aliases.clone();
                            }
                        }
                        // pane-border-status changes the effective content height (#288)
                        if option == "pane-border-status" {
                            resize_all_panes(&mut app);
                        }
                        if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        meta_dirty = true;
                        state_dirty = true;
                    }
                }
                // copied from server/mod.rs run_server (line 6197 at 86a78100)
                CtrlReq::SetOptionAppend(option, value) => {
                    // Append to existing option value
                    if option.starts_with('@') {
                        let existing = app.user_options.get(&option).cloned().unwrap_or_default();
                        app.user_options.insert(option, format!("{}{}", existing, value));
                    } else {
                        match option.as_str() {
                            // ARRAY option: `-a` appends an item (see the
                            // sibling arm in the main request loop).
                            "codepoint-widths" => {
                                crate::server::options::append_codepoint_widths(&mut app, &value);
                            }
                            "terminal-overrides" => {
                                app.terminal_overrides
                                    .extend(crate::terminal_overrides::split_array(&value));
                            }
                            "status-left" => { app.status_left.push_str(&value); }
                            "status-right" => { app.status_right.push_str(&value); }
                            "status-style" => { app.status_style.push_str(&value); }
                            "pane-border-style" => { app.pane_border_style.push_str(&value); }
                            "pane-active-border-style" => { app.pane_active_border_style.push_str(&value); }
                            "pane-border-hover-style" => { app.pane_border_hover_style.push_str(&value); }
                            "window-style" | "window-active-style" => {
                                app.user_options
                                    .entry(option.clone())
                                    .or_default()
                                    .push_str(&value);
                                app.user_set_options.insert(option.clone());
                                state_dirty = true;
                            }
                            "window-status-format" => { app.window_status_format.push_str(&value); }
                            "window-status-current-format" => { app.window_status_current_format.push_str(&value); }
                            _ => {}
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 6245 at 86a78100)
                CtrlReq::SetOptionOnlyIfUnset(option, value, resp) => {
                    let already_set = if option.starts_with('@') {
                        app.user_options.contains_key(&option)
                    } else {
                        app.user_set_options.contains(&option)
                    };
                    if already_set {
                        // tmux does not merely skip the write, it FAILS:
                        // cmd-set-option.c prints `already set: <name>` through
                        // cmdq_error and returns CMD_RETURN_ERROR unless -q.
                        // psmux swallowed the refusal on every route, so `-o`
                        // could not answer the only question it exists to ask
                        // (#619 follow up). A -q caller passes no channel and
                        // still gets the silent exit 0 tmux gives it.
                        if let Some(r) = resp {
                            let _ = r.send(format!("ERROR: already set: {}", option));
                        }
                    } else {
                        if apply_set_option(&mut app, &option, &value, false).is_ok() {
                            app.user_set_options.insert(option.clone());
                            if option == "command-alias" {
                                if let Ok(mut map) = shared_aliases_main.write() {
                                    *map = app.command_aliases.clone();
                                }
                            }
                            meta_dirty = true;
                            state_dirty = true;
                            if let Some(r) = resp {
                                let _ = r.send(String::new());
                            }
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 6143 at 86a78100)
                CtrlReq::SetOptionQuiet(option, value, quiet) => {
                    if apply_set_option(&mut app, &option, &value, quiet).is_ok() {
                        app.user_set_options.insert(option.clone());
                        // Reconcile the warm pane with the new option value.
                        // Replaces the prior inline default-shell-only kill
                        // (#99) with a uniform table-driven policy that
                        // also covers history-limit (#271), allow-predictions,
                        // default-terminal, and claude-code-* options.
                        let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                        crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                        // Update shared aliases if command-alias changed
                        if option == "command-alias" {
                            if let Ok(mut map) = shared_aliases_main.write() {
                                *map = app.command_aliases.clone();
                            }
                        }
                        // pane-border-status changes the effective content height (#288)
                        if option == "pane-border-status" {
                            resize_all_panes(&mut app);
                        }
                        if option == "window-size" && crate::resize_window::refresh_dynamic_window_sizes(&mut app) {
                            resize_all_panes(&mut app);
                        }
                        meta_dirty = true;
                        state_dirty = true;
                    }
                }
                // copied from server/mod.rs run_server (line 6233 at 86a78100)
                CtrlReq::SetOptionToggle(option) => {
                    // `set -g <bool-option>` with no value flips it (#535).
                    // Only the server knows the current value, so the flip has
                    // to happen here rather than client-side.
                    if crate::server::options::toggle_option(&mut app, &option) {
                        app.user_set_options.insert(option.clone());
                        let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                        crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                        meta_dirty = true;
                        state_dirty = true;
                    }
                }
                // copied from server/mod.rs run_server (line 6170 at 86a78100)
                CtrlReq::SetOptionUnset(option) => {
                    // Restore the table default, or remove an @user-option.
                    //
                    // This arm used to carry a hand written restore table of
                    // about thirty options with a `_ => {}` catch all, so an
                    // option it had never heard of kept its value: `set -s
                    // default-terminal xterm-256color` then `set -su
                    // default-terminal` still read xterm-256color, and every
                    // -u on status-left restored `psmux:#I` where a fresh
                    // server reports `[#S] `. The one restore table now lives
                    // in OPTION_CATALOG, which
                    // tests-rs/test_option_default_parity.rs already pins to a
                    // freshly constructed AppState (#619 follow up).
                    crate::server::options::reset_option_to_default(&mut app, &option);
                    // The value moved, so reconcile the warm pane the same way
                    // a plain set does (default-shell, history-limit and the
                    // claude-code-* options all matter here).
                    let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    if option == "command-alias" {
                        if let Ok(mut map) = shared_aliases_main.write() {
                            *map = app.command_aliases.clone();
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 5977 at 86a78100)
                CtrlReq::SetPaneOption {
                    target,
                    option,
                    value,
                    unset,
                    append,
                    only_if_unset,
                    quiet,
                    resp,
                } => {
                    // #613 `@mouse-force`, #580 `remain-on-exit` and every
                    // `@name` user option (#728) share one writer with the
                    // command prompt and config routes.
                    let reply = crate::server::options::apply_set_pane_option(
                        &mut app, &target, &option, &value, unset, append, only_if_unset, quiet,
                    );
                    if reply.is_empty() {
                        // A user option can feed the status line or a border
                        // format, so a write must reach attached clients.
                        meta_dirty = true;
                        state_dirty = true;
                    }
                    let _ = resp.send(reply);
                }
                // copied from server/mod.rs run_server (line 7990 at 86a78100)
                CtrlReq::SetSessionOption { option, value, unset, append, only_if_unset, quiet, resp } => {
                    let reply = crate::server::options::apply_set_session_option(
                        &mut app, &option, &value, unset, append, only_if_unset, quiet,
                    );
                    // The value the session uses may have moved, so reconcile
                    // the warm pane and redraw, as the global setter does.
                    let sync = crate::warm_pane_sync::for_option_change(&option, &app);
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                    meta_dirty = true;
                    state_dirty = true;
                    let _ = resp.send(reply);
                }
                // copied from server/mod.rs run_server (line 8008 at 86a78100)
                CtrlReq::SetWindowOption { target, option, value, unset, append, only_if_unset, quiet, resp } => {
                    let reply = crate::server::options::apply_set_window_option(
                        &mut app, &target, &option, &value, unset, append, only_if_unset, quiet,
                    );
                    // A window option can change what the status bar and the
                    // layout draw (window-status-*, main-pane-*), so the frame
                    // has to be rebuilt, not just the value stored.
                    meta_dirty = true;
                    state_dirty = true;
                    let _ = resp.send(reply);
                }
                // copied from server/mod.rs run_server (line 7979 at 86a78100)
                CtrlReq::ShowOptionValue(resp, name) => {
                    let val = get_option_value(&app, &name);
                    let _ = resp.send(val);
                }
                // copied from server/mod.rs run_server (line 6278 at 86a78100)
                CtrlReq::ShowOptions(resp) => {
                    let mut output = String::new();
                    output.push_str(&format!("prefix {}\n", crate::config::format_option_key(&app.prefix_key)));
                    if let Some(ref p2) = app.prefix2_key {
                        output.push_str(&format!("prefix2 {}\n", format_key_binding(p2)));
                    }
                    output.push_str(&format!("base-index {}\n", app.window_base_index));
                    output.push_str(&format!("pane-base-index {}\n", app.pane_base_index));
                    output.push_str(&format!("escape-time {}\n", app.escape_time_ms));
                    output.push_str(&format!("mouse {}\n", if app.mouse_enabled { "on" } else { "off" }));
                    output.push_str(&format!("scroll-enter-copy-mode {}\n", if app.scroll_enter_copy_mode { "on" } else { "off" }));
                    output.push_str(&format!("mouse-drag-enter-copy-mode {}\n", if app.mouse_drag_enter_copy_mode { "on" } else { "off" }));
                    output.push_str(&format!("ctrl-wheel-zoom {}\n", if app.ctrl_wheel_zoom { "on" } else { "off" }));
                    output.push_str(&format!("pwsh-mouse-selection {}\n", if app.pwsh_mouse_selection { "on" } else { "off" }));
                    output.push_str(&format!("mouse-selection {}\n", if app.mouse_selection { "on" } else { "off" }));
                    output.push_str(&format!("mouse-selection-force {}\n", if app.mouse_selection_force { "on" } else { "off" }));
                    output.push_str(&format!("paste-detection {}\n", if app.paste_detection { "on" } else { "off" }));
                    output.push_str(&format!("choose-tree-preview {}\n", if app.choose_tree_preview { "on" } else { "off" }));
                    output.push_str(&format!("status {}\n", if app.status_visible { "on" } else { "off" }));
                    output.push_str(&format!("status-position {}\n", app.status_position));
                    output.push_str(&format!("status-left \"{}\"\n", app.status_left));
                    output.push_str(&format!("status-right \"{}\"\n", app.status_right));
                    output.push_str(&format!("history-limit {}\n", app.history_limit));
                    output.push_str(&format!("display-time {}\n", app.display_time_ms));
                    output.push_str(&format!("display-panes-time {}\n", app.display_panes_time_ms));
                    // #606: repeat-time answered `show-options -g repeat-time`
                    // but was missing from this full dump, so the obvious way
                    // to check it (`show-options -g | findstr repeat`) said the
                    // option did not exist. Same shape as #559 for
                    // monitor-silence.
                    output.push_str(&format!("repeat-time {}\n", app.repeat_time_ms));
                    // #608: this dump is hand written, so a new option is
                    // invisible to a bare `show-options -g` unless it is listed
                    // here as well as in get_option_value. Same trap as #606.
                    output.push_str(&format!("priority {}\n", app.priority));
                    output.push_str(&format!("mode-keys {}\n", app.mode_keys));
                    output.push_str(&format!("focus-events {}\n", if app.focus_events { "on" } else { "off" }));
                    output.push_str(&format!("program-status {}\n", if app.program_status { "on" } else { "off" }));
                    output.push_str(&format!("renumber-windows {}\n", if app.renumber_windows { "on" } else { "off" }));
                    output.push_str(&format!("automatic-rename {}\n", if app.automatic_rename { "on" } else { "off" }));
                    output.push_str(&format!("monitor-activity {}\n", if app.monitor_activity { "on" } else { "off" }));
                    // #559: monitor-silence was queryable one-by-one but absent
                    // from this full dump, so `show-options -g | grep monitor`
                    // looked like the option had been silently dropped.
                    output.push_str(&format!("monitor-silence {}\n", app.monitor_silence));
                    output.push_str(&format!("synchronize-panes {}\n", if app.sync_input { "on" } else { "off" }));
                    output.push_str(&format!("remain-on-exit {}\n", if app.remain_on_exit { "on" } else { "off" }));
                    output.push_str(&format!("destroy-unattached {}\n", if app.destroy_unattached { "on" } else { "off" }));
                    output.push_str(&format!("exit-empty {}\n", if app.exit_empty { "on" } else { "off" }));
                    output.push_str(&format!("set-titles {}\n", if app.set_titles { "on" } else { "off" }));
                    if !app.set_titles_string.is_empty() {
                        output.push_str(&format!("set-titles-string \"{}\"\n", app.set_titles_string));
                    }
                    output.push_str(&format!(
                        "prediction-dimming {}\n",
                        if app.prediction_dimming { "on" } else { "off" }
                    ));
                    output.push_str(&format!("allow-predictions {}\n", if app.allow_predictions { "on" } else { "off" }));
                    output.push_str(&format!("cursor-style {}\n", std::env::var("PSMUX_CURSOR_STYLE").unwrap_or_else(|_| "default".to_string())));
                    output.push_str(&format!("cursor-blink {}\n", if crate::rendering::cursor_blink_option()
                        .unwrap_or(crate::rendering::CURSOR_BLINK_DEFAULT_BLINKS) { "on" } else { "off" }));
                    {
                        let shell_val = if app.default_shell.is_empty() {
                            crate::pane::cached_shell().unwrap_or("pwsh.exe").to_string()
                        } else {
                            app.default_shell.clone()
                        };
                        output.push_str(&format!("default-shell {}\n", shell_val));
                    }
                    // Quoted the way tmux shows it: the default holds `"` and
                    // `\`, which go escaped inside the quotes (#862).
                    output.push_str(&format!("word-separators {}\n",
                        crate::util::quote_arg_if_needed(&app.word_separators)));
                    // tmux lists key-table with its root default; the value
                    // itself lives in user_options, skipped in the loop below.
                    output.push_str(&format!("key-table {}\n",
                        crate::util::quote_arg_if_needed(helpers::default_key_table(&app))));
                    if !app.pane_border_style.is_empty() {
                        output.push_str(&format!("pane-border-style \"{}\"\n", app.pane_border_style));
                    }
                    if !app.pane_active_border_style.is_empty() {
                        output.push_str(&format!("pane-active-border-style \"{}\"\n", app.pane_active_border_style));
                    }
                    if !app.pane_border_hover_style.is_empty() {
                        output.push_str(&format!("pane-border-hover-style \"{}\"\n", app.pane_border_hover_style));
                    }
                    if !app.status_style.is_empty() {
                        output.push_str(&format!("status-style \"{}\"\n", app.status_style));
                    }
                    if !app.status_left_style.is_empty() {
                        output.push_str(&format!("status-left-style \"{}\"\n", app.status_left_style));
                    }
                    if !app.status_right_style.is_empty() {
                        output.push_str(&format!("status-right-style \"{}\"\n", app.status_right_style));
                    }
                    output.push_str(&format!("status-interval {}\n", app.status_interval));
                    output.push_str(&format!("status-justify {}\n", app.status_justify));
                    output.push_str(&format!("window-status-format \"{}\"\n", app.window_status_format));
                    output.push_str(&format!("window-status-current-format \"{}\"\n", app.window_status_current_format));
                    if !app.window_status_style.is_empty() {
                        output.push_str(&format!("window-status-style \"{}\"\n", app.window_status_style));
                    }
                    if !app.window_status_current_style.is_empty() {
                        output.push_str(&format!("window-status-current-style \"{}\"\n", app.window_status_current_style));
                    }
                    if !app.window_status_activity_style.is_empty() {
                        output.push_str(&format!("window-status-activity-style \"{}\"\n", app.window_status_activity_style));
                    }
                    if !app.message_style.is_empty() {
                        output.push_str(&format!("message-style \"{}\"\n", app.message_style));
                    }
                    if !app.message_command_style.is_empty() {
                        output.push_str(&format!("message-command-style \"{}\"\n", app.message_command_style));
                    }
                    if !app.mode_style.is_empty() {
                        output.push_str(&format!("mode-style \"{}\"\n", app.mode_style));
                    }
                    // Include @user-options (used by plugins)
                    for (key, val) in &app.user_options {
                        if key == "key-table" { continue; }
                        output.push_str(&format!("{} \"{}\"\n", key, val));
                    }
                    // New options
                    output.push_str(&format!("main-pane-width {}\n", app.main_pane_width));
                    output.push_str(&format!("main-pane-height {}\n", app.main_pane_height));
                    output.push_str(&format!("status-left-length {}\n", app.status_left_length));
                    output.push_str(&format!("status-right-length {}\n", app.status_right_length));
                    output.push_str(&format!("window-size {}\n", app.window_size));
                    output.push_str(&format!("allow-passthrough {}\n", app.allow_passthrough));
                    output.push_str(&format!("set-clipboard {}\n", app.set_clipboard));
                    if !app.copy_command.is_empty() {
                        output.push_str(&format!("copy-command \"{}\"\n", app.copy_command));
                    }
                    output.push_str(&format!("allow-rename {}\n", if app.allow_rename { "on" } else { "off" }));
                    output.push_str(&format!("allow-set-title {}\n", if app.allow_set_title { "on" } else { "off" }));
                    output.push_str(&format!("bell-action {}\n", app.bell_action));
                    output.push_str(&format!("activity-action {}\n", app.activity_action));
                    output.push_str(&format!("silence-action {}\n", app.silence_action));
                    output.push_str(&format!("update-environment \"{}\"\n", app.update_environment.join(" ")));
                    if let Some(ref group) = app.session_group {
                        output.push_str(&format!("session-group \"{}\"\n", group));
                    }
                    for (alias, expansion) in &app.command_aliases {
                        output.push_str(&format!("command-alias \"{}={}\"\n", alias, expansion));
                    }
                    let _ = resp.send(output);
                }
                // copied from server/mod.rs run_server (line 6001 at 86a78100)
                CtrlReq::ShowPaneOptions(raw, resp) => {
                    let pane = crate::server::options::resolve_option_target_pane(&mut app, &raw);
                    let reply = match pane {
                        None => format!("ERROR: can't find pane: {}", raw),
                        Some(p) => {
                            let mut keys: Vec<&String> = p.pane_options.keys().collect();
                            keys.sort();
                            keys.iter()
                                .map(|k| format!("{} {}", k, p.pane_options[*k]))
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    let _ = resp.send(reply);
                }
                // copied from server/mod.rs run_server (line 7983 at 86a78100)
                CtrlReq::ShowWindowOptionValue(resp, name, target) => {
                    let val = crate::server::options::window_option_value_for_target(&app, &name, &target);
                    let _ = resp.send(val);
                }
                // copied from server/mod.rs run_server (line 8022 at 86a78100)
                CtrlReq::ShowWindowOptionsFor(resp, target, listing) => {
                    let index = crate::server::options::resolve_option_target_window(&app, &target).ok();
                    let _ = resp.send(crate::server::options::render_window_options_for(
                        &app, index, listing,
                    ));
                }
                // copied from server/mod.rs run_server (line 7987 at 86a78100)
                CtrlReq::UserOptionLookup(resp, name, target, table) => {
                    let _ = resp.send(crate::server::options::lookup_user_option(&app, &name, &target, table));
                }
                // copied from server/mod.rs run_server (line 7700 at 86a78100)
                CtrlReq::SetEnvironment(args) => {
                    // tmux global vs session scopes, removal markers and
                    // hidden entries (#775); the process environment stays
                    // equal to what a new child inherits.
                    crate::environ::apply_set(&mut app.environment, &mut app.env_scopes, &args);
                    // Env vars affect the child shell's process state,
                    // which can't be patched in place: must respawn.
                    // Centralised through warm_pane_sync (#137 / #271).
                    let sync = crate::warm_pane_sync::for_env_change();
                    crate::warm_pane_sync::apply(&mut app, &*pty_system, sync);
                }
                // copied from server/mod.rs run_server (line 7714 at 86a78100)
                CtrlReq::ShowEnvironment(args, resp) => {
                    let _ = resp.send(crate::environ::show(&app.environment, &app.env_scopes, &args));
                }
                // copied from server/mod.rs run_server (line 7737 at 86a78100)
                CtrlReq::ShowHooks(resp) => {
                    let mut output = String::new();
                    for (name, commands) in &app.hooks {
                        if commands.len() == 1 {
                            output.push_str(&format!("{} -> {}\n", name, commands[0]));
                        } else {
                            for (i, cmd) in commands.iter().enumerate() {
                                output.push_str(&format!("{}[{}] -> {}\n", name, i, cmd));
                            }
                        }
                    }
                    if output.is_empty() {
                        output.push_str("(no hooks)\n");
                    }
                    let _ = resp.send(output);
                }
                // copied from server/mod.rs run_server (line 7717 at 86a78100)
                CtrlReq::SetHook(hook, cmd) => {
                    // Replace (not append) to match tmux semantics, prevents
                    // duplicate hooks on config reload (issue #133).
                    app.hooks.insert(hook, vec![cmd]);
                }
                // copied from server/mod.rs run_server (line 7722 at 86a78100)
                CtrlReq::AppendHook(hook, cmd) => {
                    // -a/-ga: append to existing hook list so multiple
                    // plugins can register separate handlers (tmux semantics).
                    // Skip an identical command that is already registered: a
                    // config re-sourced N times (e.g. a plugin panel firing
                    // "Configuration reloaded" repeatedly) would otherwise
                    // accumulate N copies of the same handler and fire all of
                    // them every tick, spawning a runaway of processes for a
                    // status-interval run-shell hook (issue #459). This mirrors
                    // the replace path's dedup guard for issue #133.
                    let entry = app.hooks.entry(hook).or_insert_with(Vec::new);
                    if !entry.contains(&cmd) {
                        entry.push(cmd);
                    }
                }
                // copied from server/mod.rs run_server (line 7753 at 86a78100)
                CtrlReq::RemoveHook(hook) => {
                    app.hooks.remove(&hook);
                }
                // copied from server/mod.rs run_server (line 6054 at 86a78100)
                CtrlReq::BindKey(table_name, key, command, repeat, note) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let kc = normalize_key_for_binding(kc);
                        // Support `\;` chaining in server-side bind-key
                        let sub_cmds = crate::config::bind_command_list(&command);
                        let action = if sub_cmds.len() > 1 {
                            Some(Action::CommandChain(sub_cmds))
                        } else {
                            sub_cmds.first().and_then(|c| parse_command_to_action(c))
                        };
                        if let Some(act) = action {
                            let table = app.key_tables.entry(table_name).or_default();
                            table.retain(|b| b.key != kc);
                            table.push(Bind { key: kc, action: act, repeat, note });
                        }
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 6078 at 86a78100)
                CtrlReq::UnbindKey(key, table) => {
                    if let Some(kc) = parse_key_string(&key) {
                        let target = table.unwrap_or_else(|| "prefix".to_string());
                        crate::config::unbind_key_in_table(&mut app, &target, kc);
                    }
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 6086 at 86a78100)
                CtrlReq::UnbindAll => {
                    // `unbind-key -a` with no table: the prefix table only,
                    // as tmux cmd-unbind-key.c.
                    crate::config::unbind_all_in_table(&mut app, "prefix");
                    app.defaults_suppressed = true;
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 6094 at 86a78100)
                CtrlReq::UnbindAllInTable(table) => {
                    crate::config::unbind_all_in_table(&mut app, &table);
                    meta_dirty = true;
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 6099 at 86a78100)
                CtrlReq::ListKeys(resp) => {
                    // Key tables plus the built-in copy-mode keys (tmux lists
                    // its copy-mode defaults, key-bindings.c).
                    let entries = crate::config::list_keys_entries(&app);
                    let user_iter = entries.iter().map(|(t, k, c, r)| (t.as_str(), k.clone(), c.clone(), *r));
                    let output = help::build_list_keys_output(user_iter, app.defaults_suppressed);
                    let _ = resp.send(output);
                }
                // copied from server/mod.rs run_server (line 6073 at 86a78100)
                CtrlReq::ListKeyNotes(resp, table, all, key) => {
                    let _ = resp.send(crate::config::list_key_notes(
                        &app, table.as_deref(), all, key.as_deref(),
                    ));
                }
                // copied from server/mod.rs run_server (line 8296 at 86a78100)
                CtrlReq::StatusMessage(msg) => {
                    app.status_message = Some((msg, std::time::Instant::now(), None));
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 7378 at 86a78100)
                CtrlReq::SwitchClient(target, flag, resp_tx) => {
                    // Resolve the target session name based on the flag
                    let current = app.port_file_base();
                    // This server's own `-L` namespace, as tmux's switch-client
                    // only ever sees the sessions of the server it runs on. The
                    // unscoped listing held only default namespace bases, so
                    // under -L `-l` never found the (namespaced) last session
                    // and `-n`/`-p`, finding no current session in the list,
                    // moved the client into the DEFAULT namespace's first one.
                    let all_sessions = crate::session::list_session_names_ns(app.socket_name.as_deref());
                    // Last session, taken from THIS client's own history
                    // (issue #566): never the data-dir-global `last_session`
                    // file, which every attach overwrites with the session
                    // being entered. No recorded previous session is "no last
                    // session", as tmux reports for a client that has not moved.
                    let last = app.latest_client_id
                        .and_then(|cid| app.client_registry.get(&cid))
                        .and_then(|info| info.last_session.clone());
                    let resolved = crate::session::resolve_switch_client(
                        flag,
                        &target,
                        &current,
                        &all_sessions,
                        last.as_deref(),
                        app.socket_name.as_deref(),
                    );
                    // #566: every branch now reports its outcome on the optional
                    // channel as well as on the TUI status line, because a
                    // one-shot CLI caller never sees a status line and so could
                    // not tell a completed switch from a silent no-op.
                    let mut reply = "OK".to_string();
                    match resolved {
                        Some(ref sess) if sess != &current => {
                            // Signal the attached client to switch by sending a directive
                            if let Some(cid) = app.latest_client_id {
                                crate::types::send_directive_to_client(cid, &format!("SWITCH {}", sess));
                            } else {
                                // No specific client ID, send to all attached clients
                                // shared: this session's clients, not every client of the process.
                                for c in lp.clients.iter() {
                                    crate::types::send_directive_to_client(*c, &format!("SWITCH {}", sess));
                                }
                            }
                        }
                        Some(_) => {
                            // Target is the same as current session
                            app.status_message = Some(("switch-client: already on that session".to_string(), std::time::Instant::now(), None));
                            state_dirty = true;
                        }
                        None => {
                            let msg = if flag == 't' && !target.is_empty() {
                                format!("switch-client: session not found: {}", target)
                            } else if flag == 'l' {
                                "switch-client: no last session".to_string()
                            } else if all_sessions.len() <= 1 {
                                "switch-client: only one session available".to_string()
                            } else {
                                "switch-client: no target session".to_string()
                            };
                            reply = format!("ERROR {}", msg);
                            app.status_message = Some((msg, std::time::Instant::now(), None));
                            state_dirty = true;
                        }
                    }
                    if let Some(rtx) = resp_tx {
                        let _ = rtx.send(reply);
                    }
                }
                // copied from server/mod.rs run_server (line 7443 at 86a78100)
                CtrlReq::SwitchClientTarget(raw, resp_tx) => {
                    // #483: `switch-client -t <target>` with a full
                    // session:window.pane / @window / %pane spec. Switch the
                    // client's session AND select the addressed window/pane, and
                    // validate the target exists so the CLI can exit non-zero.
                    let now = std::time::Instant::now();
                    let current = app.port_file_base();
                    // Resolve inside this server's own `-L` namespace. The
                    // unscoped listing skips every namespaced base, so on a
                    // `-L` socket it came back empty and any `-t <session>`
                    // failed with "can't find session" even though the target
                    // was right there in `list-sessions`. Registry bases carry
                    // the `<ns>__` prefix while the user types the bare name,
                    // so the request is qualified before it is matched.
                    let nav_ns = crate::session::session_namespace(&current).map(|s| s.to_string());
                    let all_sessions = crate::session::list_session_names_ns(nav_ns.as_deref());
                    let qualify = |name: &str| match nav_ns.as_deref() {
                        Some(ns) => format!("{}__{}", ns, name),
                        None => name.to_string(),
                    };
                    let pt = crate::cli::parse_target(&raw);
                    let sess_req = pt.session.clone().filter(|s| !s.is_empty());

                    // Resolve destination session: explicit prefix, else current.
                    let target_session = match &sess_req {
                        Some(s) => {
                            let q = qualify(s);
                            if all_sessions.contains(&q) {
                                Some(q)
                            } else {
                                all_sessions.iter().find(|x| x.starts_with(&q)).cloned()
                            }
                        }
                        None => Some(current.clone()),
                    };

                    let outcome: Result<bool, String> = match target_session {
                        None => Err(format!("can't find session: {}", sess_req.clone().unwrap_or_default())),
                        Some(dest) => {
                            let same_session = dest == current;
                            if !same_session {
                                // Cross-session (#483): if the target also names
                                // a window/pane, pre-select it on the DESTINATION
                                // server (one server per session) so the client
                                // lands there after it re-attaches, then signal
                                // the client to re-attach to dest.
                                //
                                // #555: the window/pane component is resolved on
                                // the destination BEFORE anything is signalled.
                                // The old shape forwarded the pre-select
                                // fire-and-forget (FocusWindow silently no-ops on
                                // a miss) and emitted SWITCH unconditionally, so
                                // an unresolvable cross-session component exited
                                // 0 and switched anyway, while the identical
                                // same-session target correctly errored: a
                                // driver's exit code was right exactly half the
                                // time. resolve_session also replaces the
                                // hand-rolled port-file/key reads.
                                // shared (#798 step 2): the destination is a
                                // session of this same server, served by this
                                // very loop, so the per session route (a TCP
                                // check and pre-select on the destination's
                                // listener) would wait on itself. A session
                                // target switches the client; a window or pane
                                // in another session is deferred to step 3.
                                if pt.window.is_some() || pt.window_name.is_some() || pt.pane.is_some() {
                                    Err("switch-client: a window or pane of another session is not supported by the shared server yet (step 2 of #798)".to_string())
                                } else {
                                    if let Some(cid) = app.latest_client_id {
                                        crate::types::send_directive_to_client(cid, &format!("SWITCH {}", dest));
                                    } else {
                                        for c in lp.clients.iter() {
                                            crate::types::send_directive_to_client(*c, &format!("SWITCH {}", dest));
                                        }
                                    }
                                    Ok(false)
                                }
                            } else {
                            // Same-session window/pane selection acts on THIS
                            // server directly.
                                if let Some(pid) = pt.pane {
                                    if pt.pane_is_id {
                                        if crate::tree::find_pane_by_id_global(&app, pid).is_some() {
                                            switch_with_copy_save(&mut app, |app| { crate::tree::focus_pane_by_id(app, pid); });
                                            unzoom_if_zoomed(&mut app);
                                            resize_all_panes(&mut app);
                                            Ok(true)
                                        } else {
                                            Err(format!("can't find pane: %{}", pid))
                                        }
                                    } else {
                                        // tmux (cmd-switch-client.c:140-151):
                                        // window_set_active_pane on the TARGET
                                        // window, then session_set_current to
                                        // it. focus_pane_by_index works on the
                                        // active window, so a `s:1.1` sent from
                                        // window 0 used to pick pane 1 of window
                                        // 0 and never move (test_issue483
                                        // case 3: srcWin=0 srcPane=0). Resolve
                                        // the window part first.
                                        let target_win = if let Some(w) = pt.window {
                                            if pt.window_is_id {
                                                app.windows.iter().position(|x| x.id == w)
                                            } else {
                                                app.win_pos(w)
                                            }
                                        } else if let Some(ref wname) = pt.window_name {
                                            app.windows.iter().position(|x| x.name == *wname)
                                        } else {
                                            Some(app.active_idx)
                                        };
                                        match target_win {
                                            Some(i) => {
                                                switch_with_copy_save(&mut app, |app| {
                                                    if i != app.active_idx {
                                                        app.last_window_idx = app.active_idx;
                                                        app.active_idx = i;
                                                    }
                                                    if let Some(pos) = pid.checked_sub(app.pane_base_index_of(i)) {
                                                        crate::tree::focus_pane_by_index(app, pos);
                                                    }
                                                });
                                                if let Some(win) = app.windows.get_mut(i) {
                                                    win.activity_flag = false; win.bell_flag = false; win.silence_flag = false;
                                                }
                                                unzoom_if_zoomed(&mut app);
                                                resize_all_panes(&mut app);
                                                Ok(true)
                                            }
                                            None => Err(format!("can't find window: {}", raw)),
                                        }
                                    }
                                } else if let Some(w) = pt.window {
                                    let internal = if pt.window_is_id {
                                        app.windows.iter().position(|x| x.id == w)
                                    } else {
                                        app.win_pos(w)
                                    };
                                    match internal {
                                        Some(i) => {
                                            if i != app.active_idx {
                                                switch_with_copy_save(&mut app, |app| {
                                                    app.last_window_idx = app.active_idx;
                                                    app.active_idx = i;
                                                });
                                                if let Some(win) = app.windows.get_mut(i) {
                                                    win.activity_flag = false; win.bell_flag = false; win.silence_flag = false;
                                                }
                                                resize_all_panes(&mut app);
                                            }
                                            Ok(true)
                                        }
                                        None => Err(format!("can't find window: {}", raw)),
                                    }
                                } else if let Some(ref wname) = pt.window_name {
                                    match app.windows.iter().position(|x| x.name == *wname) {
                                        Some(i) => {
                                            if i != app.active_idx {
                                                switch_with_copy_save(&mut app, |app| {
                                                    app.last_window_idx = app.active_idx;
                                                    app.active_idx = i;
                                                });
                                                resize_all_panes(&mut app);
                                            }
                                            Ok(true)
                                        }
                                        None => Err(format!("can't find window: {}", wname)),
                                    }
                                } else {
                                    // Session-only target equal to the current session: no-op.
                                    Ok(false)
                                }
                            }
                        }
                    };

                    match outcome {
                        Ok(selected) => {
                            meta_dirty = true;
                            state_dirty = true;
                            if selected { hook_event = Some("after-select-window"); }
                            let _ = resp_tx.send("OK".to_string());
                        }
                        Err(msg) => {
                            app.status_message = Some((format!("switch-client: {}", msg), now, None));
                            state_dirty = true;
                            let _ = resp_tx.send(format!("ERROR {}", msg));
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 7641 at 86a78100)
                CtrlReq::SwitchClientTable(table, resp_tx) => {
                    // tmux `cmd_switch_client_exec`: `key_bindings_get_table(name, 0)`
                    // does NOT create the table, so naming one that has no
                    // bindings is an error, not a silent latch (issue #640).
                    // "root" and "prefix" always exist in tmux, and "root" is
                    // the default table the client falls back to.
                    match crate::server::helpers::resolve_switch_client_table(&app, &table) {
                        Ok(resolved) => {
                            app.current_key_table = resolved;
                            if let Some(rt) = resp_tx { let _ = rt.send("OK".to_string()); }
                        }
                        Err(msg) => {
                            app.status_message = Some((msg.clone(), std::time::Instant::now(), None));
                            if let Some(rt) = resp_tx { let _ = rt.send(format!("ERROR {}", msg)); }
                        }
                    }
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 3563 at 86a78100)
                CtrlReq::SelectPaneTarget { win, win_is_id, ref win_name, pane, pane_is_id, fire_hook } => {
                    let before = active_pane_identity(&app);
                    let internal_idx = if let Some(w) = win {
                        if win_is_id {
                            app.windows.iter().position(|x| x.id == w)
                        } else {
                            app.win_pos(w)
                        }
                    } else if let Some(name) = win_name.as_deref() {
                        app.windows.iter().position(|x| x.name == name)
                    } else {
                        None
                    };
                    // tmux's cmd-select-pane.c sets the active pane INSIDE the
                    // target window (`window_set_active_pane(w, wp, 1)`, :274,
                    // where `w` is `target->wl->window`) and never calls
                    // `session_select`, so the session's current window does
                    // not move. psmux used to focus the window first, which is
                    // why `select-pane -t s:1.0` switched the user's window
                    // (issue #693 item 3).
                    //
                    // A bare `%id` names its own window (CMD_FIND_PANE), so it
                    // reaches another window the same way an explicit
                    // `sess:N.M` does and must leave the current window alone
                    // just as much.
                    let target_window = match internal_idx {
                        Some(i) => Some(i),
                        None => match (pane, pane_is_id) {
                            (Some(p), true) => crate::tree::find_window_pos_of_pane_id(&app, p),
                            _ => None,
                        },
                    };
                    let other_window = target_window.filter(|i| *i != app.active_idx);
                    if let Some(i) = other_window {
                        let moved = match (pane, pane_is_id) {
                            (Some(p), true) => crate::tree::set_window_active_pane_by_id(&mut app, i, p),
                            (Some(p), false) => p.checked_sub(app.pane_base_index_of(i))
                                .is_some_and(|pos| crate::tree::set_window_active_pane_by_index(&mut app, i, pos)),
                            // `select-pane -t <window>` with no pane part names
                            // that window's already active pane, so nothing
                            // moves anywhere (tmux: `wp == w->active`, :269).
                            (None, _) => false,
                        };
                        meta_dirty = true;
                        if moved {
                            // The active pane of ANOTHER window changed, which
                            // `active_pane_identity` (the current window's) can
                            // never see, so the hook is decided here.
                            note_pane_focus_change(&app, &mut notify_events);
                            if fire_hook {
                                hook_event = Some("after-select-pane");
                            }
                        }
                    } else {
                    if let Some(p) = pane {
                        let old_path = app.windows[app.active_idx].active_path.clone();
                        if pane_is_id {
                            switch_with_copy_save(&mut app, |app| { focus_pane_by_id(app, p); });
                        } else if let Some(pos) = p.checked_sub(app.pane_base_index_of(app.active_idx)) {
                            switch_with_copy_save(&mut app, |app| { focus_pane_by_index(app, pos); });
                        }
                        if app.windows[app.active_idx].active_path != old_path {
                            // tmux pushes the pane we came from onto
                            // `w->last_panes` inside `window_set_active_pane(w,
                            // wp, 1)` (window.c), so a `-t` select IS what
                            // makes a later `select-pane -l` work.  psmux only
                            // recorded it for the directional forms, so two
                            // `-t` selects in a row left `-l` with nothing to
                            // go back to (#693 item 5).
                            app.last_pane_path = old_path;
                            unzoom_if_zoomed(&mut app);
                        }
                        let win = &mut app.windows[app.active_idx];
                        if let Some(pid) = crate::tree::get_active_pane_id(&win.root, &win.active_path) {
                            crate::tree::touch_mru(&mut win.pane_mru, pid);
                        }
                    }
                    meta_dirty = true;
                    // cmd-select-pane.c:269 returns before the hook when the
                    // target pane is already the active one.
                    if active_pane_identity(&app) != before {
                        note_pane_focus_change(&app, &mut notify_events);
                        if fire_hook {
                            hook_event = Some("after-select-pane");
                        }
                    }
                    }
                }
                // copied from server/mod.rs run_server (line 6780 at 86a78100)
                CtrlReq::SelectWindowSpec { ref spec, ref pane, relative, toggle, ref resp } => {
                    // Every symbolic and offset form goes through the same
                    // resolver move-window and swap-window have used since
                    // #602; select-window simply never called it (#693 item 4).
                    // -n/-p/-l keep the hooks of NextWindow, PrevWindow and
                    // LastWindow: before-select-window for the first two only.
                    // A target that names nothing runs no hook at all: tmux
                    // resolves the target before the command runs, so a miss
                    // never reaches it. The target is resolved again after
                    // the hook, which may have moved or killed windows.
                    let resolved = match temp_target::select_window_target_pos(&app, spec, pane.as_deref()) {
                        Err(msg) => Err(msg),
                        Ok(_) => {
                            if relative != Some(crate::types::RelativeWindow::Last) {
                                if let Some(cmds) = app.hooks.get("before-select-window") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                            }
                            temp_target::select_window_target_pos(&app, spec, pane.as_deref())
                        }
                    };
                    match resolved {
                        Ok(pos) => match relative {
                            // `-T` on the current window selects the last
                            // one; with none it is an error and no after
                            // hook runs (cmd-select-window.c:131-138).
                            None => match temp_target::select_window_destination(&app, pos, toggle) {
                                Ok(dest) => {
                                    focus_window_at(&mut app, dest);
                                    meta_dirty = true;
                                    hook_event = Some("after-select-window");
                                    let _ = resp.send(Ok(()));
                                }
                                Err(msg) => {
                                    app.status_message = Some((msg.clone(), std::time::Instant::now(), None));
                                    state_dirty = true;
                                    let _ = resp.send(Err(msg));
                                }
                            },
                            // The target was only checked; the move is the
                            // one NextWindow and PrevWindow make.
                            Some(crate::types::RelativeWindow::Next) => {
                                if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                                let _ = resp.send(Ok(()));
                            }
                            Some(crate::types::RelativeWindow::Prev) => {
                                if !app.windows.is_empty() { switch_with_copy_save(&mut app, |app| { app.last_window_idx = app.active_idx; app.active_idx = (app.active_idx + app.windows.len() - 1) % app.windows.len(); }); resize_all_panes(&mut app); } meta_dirty = true; hook_event = Some("after-select-window");
                                let _ = resp.send(Ok(()));
                            }
                            // And the one LastWindow makes, with its error.
                            Some(crate::types::RelativeWindow::Last) => match app.last_window_pos() {
                                Some(last) => {
                                    switch_with_copy_save(&mut app, |app| {
                                        let tmp = app.active_idx;
                                        app.active_idx = last;
                                        app.last_window_idx = tmp;
                                    });
                                    meta_dirty = true;
                                    hook_event = Some("after-select-window");
                                    let _ = resp.send(Ok(()));
                                }
                                None => { let _ = resp.send(Err("no last window".to_string())); }
                            },
                        },
                        Err(msg) => {
                            app.status_message = Some((format!("select-window: {}", msg), std::time::Instant::now(), None));
                            state_dirty = true;
                            let _ = resp.send(Err(msg));
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 4478 at 86a78100)
                CtrlReq::SetPaneAttrs { title, style } => {
                    // select-pane -T/-P (#592): runs under a temporary -t
                    // focus, so "active pane" here is the target. Both
                    // attributes apply in this single request; the temp
                    // focus restores right after it.
                    let win = &mut app.windows[app.active_idx];
                    if let Some(p) = active_pane_mut(&mut win.root, &win.active_path) {
                        if let Some(t) = title {
                            p.title_locked = !t.is_empty();
                            p.title = t;
                        }
                        if let Some(s) = style {
                            p.pane_style = Some(s);
                        }
                    }
                    meta_dirty = true;
                }
                // copied from server/mod.rs run_server (line 4370 at 86a78100)
                CtrlReq::SendMouse(cid) => { if app.mouse_enabled { app.latest_client_id = Some(cid); crate::window_ops::send_mouse_event(&mut app); state_dirty = true; meta_dirty = true; } }
                // copied from server/mod.rs run_server (line 6132 at 86a78100)
                CtrlReq::SetWindowSize(value) => {
                    match crate::resize_window::set_active_window_size_mode(&mut app, value) {
                        Ok(()) => {
                            meta_dirty = true;
                            state_dirty = true;
                        }
                        Err(error) => {
                            app.status_message = Some((error, Instant::now(), None));
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 8279 at 86a78100)
                CtrlReq::ShowTextPopup(title, content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let width = lines.iter().map(|l| l.len()).max().unwrap_or(40).max(20) as u16 + 4;
                    let height = (lines.len() as u16 + 2).max(5);
                    app.mode = Mode::PopupMode {
                        look: crate::popup::PopupLook::titled(&title),
                        command: title,
                        output: content,
                        process: None,
                        width: width.min(120),
                        height,
                        close_on_exit: false,
                        popup_pane: None,
                        scroll_offset: 0,
                    };
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 4340 at 86a78100)
                CtrlReq::MouseDown(cid,x,y) => { if app.mouse_enabled { let previous = crate::program_status::focused_identity(&app); app.latest_client_id = Some(cid); remote_mouse_down(&mut app, x, y); if crate::program_status::focused_identity(&app) != previous { crate::program_status::acknowledge_focused(&app); } state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4341 at 86a78100)
                CtrlReq::MouseDownRight(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 2, true); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4342 at 86a78100)
                CtrlReq::MouseDownMiddle(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 1, true); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4343 at 86a78100)
                CtrlReq::MouseDrag(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_drag(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4344 at 86a78100)
                CtrlReq::MouseUp(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_up(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4345 at 86a78100)
                CtrlReq::MouseUpRight(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 2, false); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4346 at 86a78100)
                CtrlReq::MouseUpMiddle(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_mouse_button(&mut app, x, y, 1, false); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // #604: a bare pointer move only earns a redraw if it actually
                // reached a pane.  Nothing the server renders depends on the
                // pointer position (border hover highlighting is drawn by the
                // client from its own pointer state), so marking the whole
                // state dirty here pushed a full frame and repainted the entire
                // screen for every pointer sample.  This is the same rule as
                // the PaneMouse arm below, for pointer samples that land
                // outside every pane.
                // copied from server/mod.rs run_server (line 4355 at 86a78100)
                CtrlReq::MouseMove(cid,x,y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); if remote_mouse_motion(&mut app, x, y) { state_dirty = true; echo_pending_until = Some(Instant::now()); } } }
                // copied from server/mod.rs run_server (line 4368 at 86a78100)
                CtrlReq::MouseKey(cid, name, target, col, row, btn) => { if app.mouse_enabled { app.latest_client_id = Some(cid); crate::window_ops::handle_mouse_key(&mut app, &name, &target, col, row, btn); state_dirty = true; meta_dirty = true; } }
                // copied from server/mod.rs run_server (line 4369 at 86a78100)
                CtrlReq::MouseForward(cid, pane_id, btn, col, row, press) => { if app.mouse_enabled { app.latest_client_id = Some(cid); crate::window_ops::forward_mouse_to_pane(&mut app, pane_id, btn, press, Some((col, row))); state_dirty = true; } }
                // copied from server/mod.rs run_server (line 4356 at 86a78100)
                CtrlReq::ScrollUp(cid, x, y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_scroll_up(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4357 at 86a78100)
                CtrlReq::ScrollDown(cid, x, y) => { if app.mouse_enabled { app.latest_client_id = Some(cid); remote_scroll_down(&mut app, x, y); state_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // #604: `pane-mouse <id> 35 ...` is a bare pointer move.  When
                // the pane never asked for any-event tracking it changes
                // nothing, so it must not push a frame and repaint the client.
                // Measured on this tree with PSMUX_CLIENT_DEBUG=1: sweeping the
                // pointer 60 times across a pane running nvim drove 24 full
                // client redraws, against 0 while idle.  With this gate and the
                // client-side one in client.rs it is 1, the same as idle.
                // copied from server/mod.rs run_server (line 4365 at 86a78100)
                CtrlReq::PaneMouse(cid, pane_id, button, col, row, press) => { if app.mouse_enabled { app.latest_client_id = Some(cid); let inert = crate::window_ops::pane_mouse_is_inert_motion(&app, pane_id, button); handle_pane_mouse(&mut app, pane_id, button, col, row, press); if button == 0 && press { crate::window_ops::note_pane_press(&mut app, pane_id, col, row); } if !inert { state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } } }
                // copied from server/mod.rs run_server (line 4371 at 86a78100)
                CtrlReq::PaneScroll(cid, pane_id, up, at, mods) => { if app.mouse_enabled { app.latest_client_id = Some(cid); crate::window_ops::handle_pane_wheel(&mut app, pane_id, up, at, mods); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4247 at 86a78100)
                CtrlReq::CopyEnter => { enter_copy_mode(&mut app); hook_event = Some("pane-mode-changed"); }
                // copied from server/mod.rs run_server (line 4248 at 86a78100)
                CtrlReq::CopyModeCmd(flags) => {
                    match crate::copy_mode::run_copy_mode_command(&mut app, flags) {
                        crate::copy_mode::CopyModeOutcome::ModeChanged => {
                            state_dirty = true;
                            hook_event = Some("pane-mode-changed");
                        }
                        crate::copy_mode::CopyModeOutcome::Nothing => {}
                        crate::copy_mode::CopyModeOutcome::ForwardPageUp => {
                            // scroll-enter-copy-mode is off: forward PageUp to the
                            // active pane so apps like less/vim/WSL receive it (#284).
                            send_text_to_active(&mut app, "\x1b[5~")?;
                            echo_pending_until = Some(Instant::now());
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 7672 at 86a78100)
                CtrlReq::CopyModePageUp => {
                    // Same motion as `copy-mode -u`: enter copy mode and scroll
                    // up one page.  This used to move the cursor 20 rows, which
                    // is neither a page nor anything tmux does.
                    enter_copy_mode(&mut app);
                    crate::copy_mode::page_scroll(&mut app, true, false);
                }
                // copied from server/mod.rs run_server (line 4264 at 86a78100)
                CtrlReq::CopyMove(dx, dy) => { move_copy_cursor(&mut app, dx, dy); }
                // copied from server/mod.rs run_server (line 4265 at 86a78100)
                CtrlReq::CopyAnchor => { if let Some((r,c)) = current_prompt_pos(&mut app) { app.copy_anchor = Some((r,c)); app.copy_anchor_scroll_offset = app.copy_scroll_offset; app.copy_pos = Some((r,c)); } }
                // copied from server/mod.rs run_server (line 4266 at 86a78100)
                CtrlReq::CopyYank => {
                    let _ = yank_selection(&mut app);
                    if let Some(cmds) = app.hooks.get("pane-set-clipboard") { let cmds = cmds.clone(); for cmd in &cmds { let _ = execute_command_string(&mut app, cmd); } }
                    exit_copy_mode(&mut app);
                    hook_event = Some("pane-mode-changed");
                }
                // copied from server/mod.rs run_server (line 4272 at 86a78100)
                CtrlReq::CopyRectToggle => {
                    crate::copy_mode::toggle_rectangle(&mut app);
                }
                // copied from server/mod.rs run_server (line 4366 at 86a78100)
                CtrlReq::CopyDragBegin(cid, pane_id, a_col, a_row, c_col, c_row, rect_sel) => { if app.mouse_enabled { app.latest_client_id = Some(cid); copy_drag_begin(&mut app, pane_id, a_col, a_row, c_col, c_row, rect_sel); state_dirty = true; meta_dirty = true; echo_pending_until = Some(Instant::now()); } }
                // copied from server/mod.rs run_server (line 4263 at 86a78100)
                CtrlReq::ClockMode => { app.mode = Mode::ClockMode; state_dirty = true; hook_event = Some("pane-mode-changed"); }
                // copied from server/mod.rs run_server (line 4367 at 86a78100)
                CtrlReq::StatusWheel(cid, up, mods) => { if app.mouse_enabled { app.latest_client_id = Some(cid); crate::window_ops::handle_status_wheel(&mut app, up, mods); state_dirty = true; meta_dirty = true; } }
                // copied from server/mod.rs run_server (line 8072 at 86a78100)
                CtrlReq::FocusIn => {
                    if app.focus_events {
                        // Forward focus-in escape sequence to all panes in active window
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.writer.write_all(seq); let _ = p.writer.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[I");
                    }
                    hook_event = Some("pane-focus-in");
                }
                // copied from server/mod.rs run_server (line 8086 at 86a78100)
                CtrlReq::FocusOut => {
                    if app.focus_events {
                        let win = &mut app.windows[app.active_idx];
                        fn send_focus_seq(node: &mut Node, seq: &[u8]) {
                            match node {
                                Node::Leaf(p) => { let _ = p.writer.write_all(seq); let _ = p.writer.flush(); }
                                Node::Split { children, .. } => { for c in children { send_focus_seq(c, seq); } }
                            }
                        }
                        send_focus_seq(&mut win.root, b"\x1b[O");
                    }
                    hook_event = Some("pane-focus-out");
                }
                // copied from server/mod.rs run_server (line 7162 at 86a78100)
                CtrlReq::SetClientFocus(cid, focused) => {
                    if let Some(info) = app.client_registry.get_mut(&cid) {
                        info.focused = focused;
                    }
                }
                // copied from server/mod.rs run_server (line 8099 at 86a78100)
                CtrlReq::CommandPrompt(args, wait, resp) => {
                    // Opened on an attached client, which gets it in its next
                    // state frame (#789 follow up). It used to open a prompt in
                    // the server that no client drew, with only -I kept.
                    match helpers::request_client_prompt(&mut app, &args) {
                        Ok(helpers::PromptRequestOutcome::Queued(id)) => {
                            if wait {
                                app.prompt_waiters.push((id, resp));
                            } else {
                                let _ = resp.send(String::new());
                            }
                            state_dirty = true;
                        }
                        Ok(helpers::PromptRequestOutcome::Ignored) => { let _ = resp.send(String::new()); }
                        Err(e) => { let _ = resp.send(format!("ERROR: {}", e)); }
                    }
                }
                // copied from server/mod.rs run_server (line 8116 at 86a78100)
                CtrlReq::PromptDone(id) => {
                    app.finish_prompt_request(id);
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 8221 at 86a78100)
                CtrlReq::OverlayClose => {
                    match app.mode {
                        Mode::PopupMode { .. } | Mode::MenuMode { .. } | Mode::ConfirmMode { .. } | Mode::PaneChooser { .. } | Mode::ClockMode | Mode::CustomizeMode { .. } => {
                            app.mode = Mode::Passthrough;
                            state_dirty = true;
                        }
                        _ => {}
                    }
                }
                // copied from server/mod.rs run_server (line 5647 at 86a78100)
                CtrlReq::ResizePane(dir, amount) => {
                    // A focused floating pane resizes itself (and its PTY) instead
                    // of the tiled layout.
                    let mut handled_float = false;
                    {
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if let Some(fp) = win.floating.get_mut(fi) {
                                let d = amount as i16;
                                match dir.as_str() {
                                    "L" => fp.w = (fp.w as i16 - d).max(3) as u16,
                                    "R" => fp.w = (fp.w as i16 + d).max(3) as u16,
                                    "U" => fp.h = (fp.h as i16 - d).max(3) as u16,
                                    "D" => fp.h = (fp.h as i16 + d).max(3) as u16,
                                    _ => {}
                                }
                                fp.w = fp.w.min(win_w);
                                fp.h = fp.h.min(win_h);
                                let (nx, ny) = crate::floating::clamp_into(fp.x, fp.y, fp.w, fp.h, win_w, win_h);
                                fp.x = nx; fp.y = ny;
                                let inner_h = fp.h.saturating_sub(2).max(1);
                                let inner_w = fp.w.saturating_sub(2).max(1);
                                if fp.pane.last_rows != inner_h || fp.pane.last_cols != inner_w {
                                    let _ = fp.pane.master.resize(portable_pty::PtySize { rows: inner_h, cols: inner_w, pixel_width: 0, pixel_height: 0 });
                                    if let Ok(mut parser) = fp.pane.term.lock() { parser.screen_mut().set_size(inner_h, inner_w); }
                                    fp.pane.last_rows = inner_h;
                                    fp.pane.last_cols = inner_w;
                                }
                                handled_float = true;
                            }
                        }
                    }
                    if handled_float {
                        state_dirty = true;
                    } else {
                        unzoom_if_zoomed(&mut app);
                        match dir.as_str() {
                            "U" | "D" => { resize_pane_vertical(&mut app, if dir == "U" { -(amount as i16) } else { amount as i16 }); }
                            "L" | "R" => { resize_pane_horizontal(&mut app, if dir == "L" { -(amount as i16) } else { amount as i16 }); }
                            _ => {}
                        }
                        resize_all_panes(&mut app); meta_dirty = true;
                        hook_event = Some("after-resize-pane");
                    }
                }
                // copied from server/mod.rs run_server (line 7931 at 86a78100)
                CtrlReq::ResizePaneAbsolute(axis, size) => {
                    // tmux: resize-pane -x/-y sets the focused float's absolute
                    // OUTER size (and its PTY) instead of the tiled pane.
                    let mut handled_float = false;
                    {
                        let win_w = app.last_window_area.width.max(10);
                        let win_h = app.last_window_area.height.max(10);
                        let win = &mut app.windows[app.active_idx];
                        if let Some(fi) = win.floating_focus {
                            if let Some(fp) = win.floating.get_mut(fi) {
                                match axis.as_str() {
                                    "x" => fp.w = size.max(3).min(win_w),
                                    "y" => fp.h = size.max(3).min(win_h),
                                    _ => {}
                                }
                                let (nx, ny) = crate::floating::clamp_into(fp.x, fp.y, fp.w, fp.h, win_w, win_h);
                                fp.x = nx; fp.y = ny;
                                let inner_h = fp.h.saturating_sub(2).max(1);
                                let inner_w = fp.w.saturating_sub(2).max(1);
                                if fp.pane.last_rows != inner_h || fp.pane.last_cols != inner_w {
                                    let _ = fp.pane.master.resize(portable_pty::PtySize { rows: inner_h, cols: inner_w, pixel_width: 0, pixel_height: 0 });
                                    if let Ok(mut parser) = fp.pane.term.lock() { parser.screen_mut().set_size(inner_h, inner_w); }
                                    fp.pane.last_rows = inner_h;
                                    fp.pane.last_cols = inner_w;
                                }
                                handled_float = true;
                            }
                        }
                    }
                    if handled_float {
                        state_dirty = true;
                    } else {
                        unzoom_if_zoomed(&mut app);
                        resize_pane_absolute(&mut app, &axis, size);
                        resize_all_panes(&mut app);
                        hook_event = Some("after-resize-pane");
                    }
                }
                // copied from server/mod.rs run_server (line 7969 at 86a78100)
                CtrlReq::ResizePanePercent(axis, pct) => {
                    unzoom_if_zoomed(&mut app);
                    // Convert percentage to absolute size based on current window dimensions
                    let area = app.last_window_area;
                    let total = if axis == "x" { area.width } else { area.height };
                    let abs_size = ((total as u32) * (pct as u32) / 100).max(1) as u16;
                    resize_pane_absolute(&mut app, &axis, abs_size);
                    resize_all_panes(&mut app);
                    hook_event = Some("after-resize-pane");
                }
                // copied from server/mod.rs run_server (line 8045 at 86a78100)
                CtrlReq::SendPrefix => {
                    // Send the prefix key to the active pane as if typed
                    // Nothing to send when the prefix is None (tmux cmd-send-keys.c).
                    let encoded: Vec<u8> = match app.prefix_key {
                        Some((crossterm::event::KeyCode::Char(c), m)) if m.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            vec![(c.to_ascii_lowercase() as u8) & 0x1F]
                        }
                        Some((crossterm::event::KeyCode::Char(c), _)) => format!("{}", c).into_bytes(),
                        _ => vec![],
                    };
                    if !encoded.is_empty() {
                        let win = &mut app.windows[app.active_idx];
                        // send-prefix is a key (tmux window_pane_key), so an
                        // input-off pane does not get it.
                        if let Some(p) = active_pane_mut(&mut win.root, &win.active_path).filter(|p| p.accepts_input()) {
                            let _ = p.writer.write_all(&encoded);
                            let _ = p.writer.flush();
                        }
                    }
                }
                // copied from server/mod.rs run_server (line 5869 at 86a78100)
                CtrlReq::DisplayPanes => {
                    app.mode = Mode::PaneChooser { opened_at: std::time::Instant::now() };
                    state_dirty = true;
                }
                // copied from server/mod.rs run_server (line 5873 at 86a78100)
                CtrlReq::DisplayPaneSelect(digit) => {
                    let previous = crate::program_status::focused_identity(&app);
                    // User pressed a digit during display-panes overlay: select the matching pane
                    let win = &app.windows[app.active_idx];
                    let mut rects: Vec<(Vec<usize>, ratatui::layout::Rect)> = Vec::new();
                    crate::tree::compute_rects(&win.root, app.last_window_area, &mut rects);
                    for (i, (path, _)) in rects.iter().enumerate() {
                        if i >= 10 { break; }
                        let mapped = (i + app.pane_base_index_of(app.active_idx)) % 10;
                        if mapped == digit {
                            let new_path = path.clone();
                            let old_path = app.windows[app.active_idx].active_path.clone();
                            app.windows[app.active_idx].active_path = new_path;
                            if crate::program_status::focused_identity(&app) != previous {
                                crate::program_status::acknowledge_focused(&app);
                            }
                            if app.windows[app.active_idx].active_path != old_path {
                                app.last_pane_path = old_path;
                            }
                            break;
                        }
                    }
                    app.mode = Mode::Passthrough;
                    state_dirty = true;
                    meta_dirty = true;
                }
                // copied from server/mod.rs run_server (line 5694 at 86a78100)
                CtrlReq::SetBuffer(content) => {
                    app.paste_add(content);
                }
                // copied from server/mod.rs run_server (line 5950 at 5068b6fc)
                CtrlReq::ShowBuffer(resp) => {
                    let content = app.paste_top().unwrap_or_default().to_string();
                    let _ = resp.send(content);
                }
                // copied from server/mod.rs run_server (line 5954 at 5068b6fc)
                CtrlReq::ShowNamedBuffer(resp, name) => {
                    let _ = resp.send(app.paste_get(&name).map(str::to_string));
                }
                // shared: everything step 2 does not handle (see the module
                // comment). Its reply channel, if any, closes here.
                _ => {}
        }
        // copied from run_server (server/mod.rs at 86a78100, after the match):
        // notification hooks, the after-<command> hook, the focus restore.
        for event in std::mem::take(&mut notify_events) {
            let cmds: Vec<String> = app.hooks.get(event).cloned().unwrap_or_default();
            for cmd in cmds {
                let _ = execute_command_string(&mut app, &cmd);
            }
        }
        if let Some(event) = hook_event {
            let cmds: Vec<String> = app.hooks.get(event).cloned().unwrap_or_default();
            for cmd in cmds {
                let _ = execute_command_string(&mut app, &cmd);
            }
        }
        temp_target::restore_temp_focus(&mut app, &mut temp_focus_restore);
        control::sync_window_changes(&mut app);
        if mutates_state {
            state_dirty = true;
        }
        if app.copy_needs_redraw {
            app.copy_needs_redraw = false;
            state_dirty = true;
        }
    }
    lp.state_dirty = state_dirty;
    lp.meta_dirty = meta_dirty;
    lp.echo_pending_until = echo_pending_until;
    lp.temp_focus_restore = temp_focus_restore;
    Ok(flow)
}
