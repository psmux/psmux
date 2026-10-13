//! A command's -t target, resolved and applied by the server loop.
//!
//! psmux's handlers act on "the active pane", so a command with -t runs with
//! the focus switched to its target for exactly as long as the command runs.
//! Each request a targeted command sends carries the target with it
//! (`CtrlReq::Targeted`) and the loop does resolve, focus, run, restore in
//! one step; see the doc on `CtrlReq::Targeted` for the defect this replaced.
//! The connection side lives in `TargetedSender` (server::connection).

use crate::types::{AppState, TempTarget};

/// Resolve `t` against the current state.  Ok carries the same target in
/// stable id form (`TempTarget::is_resolved`); Err carries tmux's message.
pub(crate) fn resolve_temp_target(app: &AppState, t: &TempTarget) -> Result<TempTarget, String> {
    if let Some(ref s) = t.session {
        if !session_names_this_server(app, s) {
            return Err(format!("can't find session: {}", s));
        }
    }
    let has_win = t.win.is_some() || t.win_name.is_some();
    let win_idx: Option<usize> = if let Some(w) = t.win {
        if t.win_is_id {
            app.windows.iter().position(|x| x.id == w)
        } else {
            app.win_pos(w)
        }
    } else if let Some(ref n) = t.win_name {
        app.windows.iter().position(|x| x.name == *n)
    } else if app.active_idx < app.windows.len() {
        Some(app.active_idx)
    } else {
        None
    };
    let Some(idx) = win_idx else {
        let spec = match t.win {
            Some(w) if t.win_is_id => format!("@{}", w),
            Some(w) => w.to_string(),
            None => t.win_name.clone().unwrap_or_default(),
        };
        return Err(format!("can't find window: {}", spec));
    };
    if t.pane_check_only {
        // The command resolves the pane itself, floating panes included, so
        // only answer whether it exists. A floating pane's position follows
        // the tiled panes (#767), as #{pane_index} numbers it.
        if let Some(p) = t.pane {
            let found = if t.pane_is_id {
                crate::tree::find_pane_by_id_global(app, p).is_some()
                    || app.windows.iter().any(|w| w.floating.iter().any(|fp| fp.id == p))
            } else {
                let w = &app.windows[idx];
                p < crate::tree::pane_paths(&w.root).len() + w.floating.len()
            };
            if !found {
                let spec = if t.pane_is_id { format!("%{}", p) } else { p.to_string() };
                return Err(format!("can't find pane: {}", spec));
            }
        }
        return Ok(TempTarget {
            win: if has_win { Some(app.windows[idx].id) } else { None },
            win_is_id: has_win,
            ..Default::default()
        });
    }
    let pane_id: Option<usize> = match t.pane {
        Some(p) if t.pane_is_id => {
            // A floating pane is a pane of its window too (tmux new-pane):
            // a -t naming one runs the request with it focused.
            if crate::tree::find_pane_by_id_global(app, p).is_none()
                && crate::tree::find_floating_by_id(app, p).is_none()
            {
                return Err(format!("can't find pane: %{}", p));
            }
            Some(p)
        }
        Some(p) => {
            // Positional index within the target window, the order
            // focus_pane_by_index and #{pane_index} use; floating panes
            // follow the tiled ones (#767).
            let win = &app.windows[idx];
            let root = &win.root;
            let paths = crate::tree::pane_paths(root);
            match paths.get(p).and_then(|path| crate::tree::get_active_pane_id(root, path)) {
                Some(id) => Some(id),
                None => match p.checked_sub(paths.len()).and_then(|fi| win.floating.get(fi)) {
                    Some(fp) => Some(fp.id),
                    None => return Err(format!("can't find pane: {}", p)),
                },
            }
        }
        None => None,
    };
    Ok(TempTarget {
        win: if has_win { Some(app.windows[idx].id) } else { None },
        win_is_id: has_win,
        win_name: None,
        pane: pane_id,
        pane_is_id: pane_id.is_some(),
        ..Default::default()
    })
}

/// Does the session part of a target name the session this server holds?
///
/// Issue #793: one server holds one session, so a target naming any other
/// session reached the wrong server and must fail like tmux's
/// `cmd_find_get_session` miss. Accepted: the name itself, the `<ns>__name`
/// registry base a resolved `$N` carries, the `$N` id, and a prefix of the
/// name when no session of exactly that name exists in the namespace (tmux
/// tries the exact name first, then a prefix; switch-client forwards a
/// prefix it matched to the destination server).
pub(crate) fn session_names_this_server(app: &AppState, s: &str) -> bool {
    let s = crate::cli::strip_exact_match_prefix(s);
    if s == app.session_name || s == app.port_file_base() || s == format!("${}", app.session_id) {
        return true;
    }
    if !s.is_empty() && app.session_name.starts_with(s) {
        let base = crate::session::namespaced_session_base(app.socket_name.as_deref(), s);
        return !std::path::Path::new(&crate::paths::port_file(&base)).exists();
    }
    false
}

/// The real focus a temporary -t focus replaced: the active window index, its
/// active tiled pane id, and every window's focused floating pane (by pane
/// id, since the request may close one and shift the others).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SavedFocus {
    pub active_idx: usize,
    pub pane_id: usize,
    pub floating: Vec<(usize, Option<usize>)>,
}

/// Switch the focus to `t` for one request, remembering the real focus in
/// `saved` for `restore_temp_focus`. On Err nothing has changed.
///
/// A floating pane target gets the input focus of its window
/// (`floating_focus`), which is what every pane request already follows
/// first; a tiled pane target takes that focus away for the request, or a
/// focused float would have received a `send-keys -t %1` meant for %1
/// (issue #793 follow up).
pub(crate) fn apply_temp_target(
    app: &mut AppState,
    saved: &mut Option<SavedFocus>,
    t: &TempTarget,
) -> Result<(), String> {
    let r = resolve_temp_target(app, t)?;
    if saved.is_none() && app.active_idx < app.windows.len() {
        let w = &app.windows[app.active_idx];
        let pane_id = crate::tree::get_active_pane_id(&w.root, &w.active_path).unwrap_or(usize::MAX);
        let floating = app.windows.iter()
            .map(|w| (w.id, w.floating_focus.and_then(|fi| w.floating.get(fi)).map(|fp| fp.id)))
            .collect();
        *saved = Some(SavedFocus { active_idx: app.active_idx, pane_id, floating });
        // The REAL active window, so #{window_active} and the `*` flag are
        // not fooled by the temporary switch (issue #551).
        app.temp_focus_saved_active = Some(app.active_idx);
    }
    if let Some(wid) = r.win {
        if let Some(i) = app.windows.iter().position(|x| x.id == wid) {
            app.active_idx = i;
        }
    }
    if let Some(p) = r.pane {
        // No MRU update: a -t target is not the user visiting the pane (#71).
        crate::tree::focus_pane_by_id_no_mru(app, p);
    }
    app.last_window_area = app.windows[app.active_idx].area;
    Ok(())
}

/// Put back the focus `apply_temp_target` saved, if any.  By pane id, not
/// path, because the request may have restructured the tree (#71); if that
/// pane is gone the window keeps whatever active pane the request left.
pub(crate) fn restore_temp_focus(app: &mut AppState, saved: &mut Option<SavedFocus>) {
    if let Some(s) = saved.take() {
        if s.active_idx < app.windows.len() {
            app.active_idx = s.active_idx;
            let win = &mut app.windows[s.active_idx];
            if let Some(path) = crate::tree::find_path_by_id(&win.root, s.pane_id) {
                win.active_path = path;
            }
            app.last_window_area = win.area;
        }
        for (wid, float_id) in s.floating {
            if let Some(w) = app.windows.iter_mut().find(|w| w.id == wid) {
                w.floating_focus = float_id.and_then(|id| w.floating.iter().position(|fp| fp.id == id));
            }
        }
        app.temp_focus_saved_active = None;
    }
}

#[cfg(test)]
#[path = "../../tests-rs/test_stalled_target_routing.rs"]
mod tests;
