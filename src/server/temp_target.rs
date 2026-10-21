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
                p.checked_sub(app.pane_base_index_of(idx))
                    .is_some_and(|pos| pos < crate::tree::pane_paths(&w.root).len() + w.floating.len())
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
            // The pane index as #{pane_index} shows it, so pane-base-index
            // comes off before it is a position in the order
            // focus_pane_by_index uses; floating panes follow the tiled
            // ones (#767).
            let pos = p.checked_sub(app.pane_base_index_of(idx))
                .ok_or_else(|| format!("can't find pane: {}", p))?;
            let win = &app.windows[idx];
            let root = &win.root;
            let paths = crate::tree::pane_paths(root);
            match paths.get(pos).and_then(|path| crate::tree::get_active_pane_id(root, path)) {
                Some(id) => Some(id),
                None => match pos.checked_sub(paths.len()).and_then(|fi| win.floating.get(fi)) {
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

/// The window a `select-window -t` selects: `spec` is the window part and
/// `pane` the pane part of the target, which tmux only resolves and never
/// focuses (cmd-select-window.c uses the target's window alone).  The pane
/// part must exist in that window (cmd-find.c:592-598, 1189-1217), and a
/// bare `%N` with no window part names the window holding that pane
/// (cmd-find.c:1103-1104).  A `+`/`-` pane part always resolves.
/// [`select_window_pos`], where no window part and no pane part is the
/// current window, tmux's default target (`select-window -T` with no -t).
pub(crate) fn select_window_target_pos(app: &AppState, spec: &str, pane: Option<&str>) -> Result<usize, String> {
    if spec.trim().is_empty() && pane.is_none() {
        return Ok(app.active_idx);
    }
    select_window_pos(app, spec, pane)
}

/// The window a `select-window` whose target resolved to `pos` lands on: the
/// target itself, or, with `-T` (`toggle`) and a target that is already the
/// current window, the last window, which is the error "no last window" when
/// there is none (cmd-select-window.c:131-138).
pub(crate) fn select_window_destination(app: &AppState, pos: usize, toggle: bool) -> Result<usize, String> {
    if toggle && pos == app.active_idx {
        return app.last_window_pos().ok_or_else(|| "no last window".to_string());
    }
    Ok(pos)
}

pub(crate) fn select_window_pos(app: &AppState, spec: &str, pane: Option<&str>) -> Result<usize, String> {
    let missing_pane = |p: &str| format!("can't find pane: {}", p);
    if let Some(id) = pane.and_then(|p| p.strip_prefix('%')) {
        if spec.trim().is_empty() {
            let id: usize = id.parse().map_err(|_| missing_pane(pane.unwrap_or_default()))?;
            return crate::tree::find_pane_by_id_global(app, id)
                .or_else(|| crate::tree::find_floating_by_id(app, id))
                .map(|(wi, _)| wi)
                .ok_or_else(|| missing_pane(pane.unwrap_or_default()));
        }
    }
    let pos = app.resolve_window_spec(spec, false)
        .and_then(|w| w.pos().ok_or_else(|| format!("can't find window: {}", spec)))?;
    let Some(p) = pane else { return Ok(pos) };
    if matches!(p, "+" | "-") { return Ok(pos); }
    let win = &app.windows[pos];
    let found = if let Some(id) = p.strip_prefix('%') {
        id.parse::<usize>().is_ok_and(|id| {
            crate::tree::get_pane_position_in_window(&win.root, id).is_some()
                || win.floating.iter().any(|fp| fp.id == id)
        })
    } else {
        p.parse::<usize>().ok()
            .and_then(|n| n.checked_sub(app.pane_base_index_of(pos)))
            .is_some_and(|n| n < crate::tree::pane_paths(&win.root).len() + win.floating.len())
    };
    if found { Ok(pos) } else { Err(missing_pane(p)) }
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
    /// The focused pane's copy or clock mode was parked and the target's own
    /// adopted (#828), so restoring must move them back.
    pub modes_moved: bool,
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
        *saved = Some(SavedFocus { active_idx: app.active_idx, pane_id, floating, modes_moved: false });
        // The REAL active window, so #{window_active} and the `*` flag are
        // not fooled by the temporary switch (issue #551).
        app.temp_focus_saved_active = Some(app.active_idx);
    }
    // #828: the target pane answers with its OWN copy or clock mode, and
    // the pane that had focus keeps its own, as tmux's `window_pane_key`
    // sends a key to the mode of the pane it targets.
    let moved = crate::copy_mode::switch_focus_keeping_modes(app, false, |app| {
        if let Some(wid) = r.win {
            if let Some(i) = app.windows.iter().position(|x| x.id == wid) {
                app.active_idx = i;
            }
        }
        if let Some(p) = r.pane {
            // No MRU update: a -t target is not the user visiting the pane (#71).
            crate::tree::focus_pane_by_id_no_mru(app, p);
        }
    });
    if moved {
        if let Some(s) = saved.as_mut() {
            s.modes_moved = true;
        }
    }
    app.last_window_area = app.windows[app.active_idx].area;
    Ok(())
}

/// Is `pane_id` the active pane of window `win_idx` as the user left it,
/// looking through a temporary -t focus? The focused window's real active
/// pane is the saved one. A window the -t focus moved to had its active pane
/// changed for the request, so its own is not known and this says no.
pub(crate) fn is_real_active_pane(app: &AppState, saved: &Option<SavedFocus>, win_idx: usize, pane_id: usize) -> bool {
    match saved {
        Some(s) if s.active_idx == win_idx => s.pane_id == pane_id,
        Some(_) if app.active_idx == win_idx => false,
        _ => app.windows.get(win_idx)
            .and_then(|w| crate::tree::get_active_pane_id(&w.root, &w.active_path))
            == Some(pane_id),
    }
}

/// Put back the focus `apply_temp_target` saved, if any.  By pane id, not
/// path, because the request may have restructured the tree (#71); if that
/// pane is gone the window keeps whatever active pane the request left.
pub(crate) fn restore_temp_focus(app: &mut AppState, saved: &mut Option<SavedFocus>) {
    if let Some(s) = saved.take() {
        if s.active_idx < app.windows.len() {
            // #828: each pane's mode goes back with the focus (see above).
            let switch = |app: &mut AppState| {
                app.active_idx = s.active_idx;
                let win = &mut app.windows[s.active_idx];
                if let Some(path) = crate::tree::find_path_by_id(&win.root, s.pane_id) {
                    win.active_path = path;
                }
            };
            if s.modes_moved {
                crate::copy_mode::switch_focus_keeping_modes(app, true, switch);
            } else {
                switch(app);
            }
            app.last_window_area = app.windows[s.active_idx].area;
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

#[cfg(test)]
#[path = "../../tests-rs/test_issue828_temp_focus_modes.rs"]
mod tests_issue828;
