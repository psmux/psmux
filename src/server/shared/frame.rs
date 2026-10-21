//! Frames for the attached clients of one session of the shared server.
//!
//! Copied from run_server (server/mod.rs at 0145958f: the `CtrlReq::DumpState`
//! arm and the server push at the end of the loop iteration), with the frame
//! cache held per session instead of in run_server's locals. One change in
//! behaviour, forced by the process model: `types::push_frame` hands a frame
//! to every attached client of the PROCESS, which in a shared server would
//! paint one session's screen on another session's client. A pushed frame of
//! the shared server goes only to the clients attached to its session,
//! through their own directive channel (`send_directive_to_client`), which
//! the connection's writer puts on the wire as a line exactly like a frame.

use super::super::*;
use super::server::SessionLoop;

/// The per session copies of run_server's frame cache locals.
pub(crate) struct FrameCache {
    pub cached_dump_state: String,
    pub cached_data_version: u64,
    pub cached_windows_json: String,
    pub cached_program_status_version: u64,
    pub cached_tree_json: String,
    pub cached_prefix_str: String,
    pub cached_prefix2_str: String,
    pub cached_base_index: usize,
    pub cached_pred_dim: bool,
    pub cached_status_style: String,
    pub cached_bindings_json: String,
    pub combined_buf: String,
}

impl FrameCache {
    pub fn new() -> Self {
        FrameCache {
            cached_dump_state: String::new(),
            cached_data_version: 0,
            cached_windows_json: String::new(),
            cached_program_status_version: 0,
            cached_tree_json: String::new(),
            cached_prefix_str: String::new(),
            cached_prefix2_str: String::new(),
            cached_base_index: 0,
            cached_pred_dim: false,
            cached_status_style: String::new(),
            cached_bindings_json: String::from("[]"),
            combined_buf: String::with_capacity(32768),
        }
    }
}

/// Build the frame JSON into `c.combined_buf` and cache it. Copied from the
/// DumpState arm (the push path builds the same object without
/// `defaults_suppressed`; the client reads either).
fn build_frame(app: &mut AppState, c: &mut FrameCache, meta_dirty: &mut bool) -> io::Result<()> {
    // Rebuild metadata cache if structural changes happened.
    if *meta_dirty {
        c.cached_windows_json = list_windows_json_with_tabs(app)?;
        c.cached_tree_json = list_tree_json(app)?;
        c.cached_prefix_str = crate::config::format_option_key(&app.prefix_key);
        c.cached_prefix2_str = app.prefix2_key.as_ref().map(format_key_binding).unwrap_or_default();
        c.cached_base_index = app.window_base_index;
        c.cached_pred_dim = app.prediction_dimming;
        c.cached_status_style = app.status_style.clone();
        c.cached_bindings_json = serialize_bindings_json(app);
        *meta_dirty = false;
    }
    let layout_json = dump_layout_json_fast(app)?;
    c.combined_buf.clear();
    let sf = helpers::expand_status_formats(app, &c.cached_status_style);
    let ss_escaped = json_escape_string(&sf.status_style);
    let sl_expanded = json_escape_string(&sf.status_left);
    let sr_expanded = json_escape_string(&sf.status_right);
    let pbs_escaped = json_escape_string(&sf.pane_border_style);
    let pabs_escaped = json_escape_string(&sf.pane_active_border_style);
    let pbhs_escaped = json_escape_string(&sf.pane_border_hover_style);
    let wsf_escaped = json_escape_string(&app.window_status_format);
    let wscf_escaped = json_escape_string(&app.window_status_current_format);
    let wss_escaped = json_escape_string(&sf.window_status_separator);
    let ws_style_escaped = json_escape_string(&sf.window_status_style);
    let wsc_style_escaped = json_escape_string(&sf.window_status_current_style);
    let mode_style_escaped = json_escape_string(&sf.mode_style);
    let message_style_escaped = json_escape_string(&sf.message_style);
    let status_position_escaped = json_escape_string(&app.status_position);
    let status_justify_escaped = json_escape_string(&app.status_justify);
    let status_format_json = &sf.status_format_json;
    let cursor_style_code = crate::rendering::configured_cursor_code();
    let cursor_blink_mode = match crate::rendering::blink_only_mode() {
        Some(true) => "true",
        Some(false) => "false",
        None => "null",
    };
    let _ = std::fmt::Write::write_fmt(&mut c.combined_buf, format_args!(
        "{{\"layout\":{},\"windows\":{},\"prefix\":\"{}\",\"prefix2\":\"{}\",\"tree\":{},\"base_index\":{},\"pane_base_index\":{},\"prediction_dimming\":{},\"status_style\":\"{}\",\"status_left\":\"{}\",\"status_right\":\"{}\",\"pane_border_style\":\"{}\",\"pane_active_border_style\":\"{}\",\"pane_border_hover_style\":\"{}\",\"wsf\":\"{}\",\"wscf\":\"{}\",\"wss\":\"{}\",\"ws_style\":\"{}\",\"wsc_style\":\"{}\",\"clock_mode\":{},\"bindings\":{},\"status_left_length\":{},\"status_right_length\":{},\"status_lines\":{},\"status_format\":{},\"mode_style\":\"{}\",\"message_style\":\"{}\",\"status_position\":\"{}\",\"status_justify\":\"{}\",\"cursor_style_code\":{},\"cursor_blink_mode\":{},\"status_visible\":{},\"repeat_time\":{},\"escape_time\":{},\"zoomed\":{},\"defaults_suppressed\":{},\"pwsh_mouse_selection\":{},\"mouse_selection\":{},\"mouse_selection_force\":{},\"paste_detection\":{},\"choose_tree_preview\":{},\"scroll_enter_copy_mode\":{},\"mouse_drag_enter_copy_mode\":{},\"ctrl_wheel_zoom\":{},\"drag_end_default\":{},\"mouse_overrides\":{},\"bold_is_bright\":{}}}",
        layout_json, c.cached_windows_json, c.cached_prefix_str, c.cached_prefix2_str, c.cached_tree_json, c.cached_base_index, app.pane_base_index_of(app.active_idx), c.cached_pred_dim, ss_escaped, sl_expanded, sr_expanded, pbs_escaped, pabs_escaped, pbhs_escaped, wsf_escaped, wscf_escaped, wss_escaped, ws_style_escaped, wsc_style_escaped,
        matches!(app.mode, Mode::ClockMode), c.cached_bindings_json,
        app.status_left_length, app.status_right_length, app.status_lines, status_format_json,
        mode_style_escaped, message_style_escaped, status_position_escaped, status_justify_escaped,
        cursor_style_code, cursor_blink_mode, app.status_visible, app.repeat_time_ms, app.escape_time_ms,
        app.windows.get(app.active_idx).map_or(false, |w| w.zoom_saved.is_some()),
        app.defaults_suppressed,
        app.pwsh_mouse_selection,
        app.mouse_selection,
        app.mouse_selection_force,
        app.paste_detection,
        app.choose_tree_preview,
        app.scroll_enter_copy_mode,
        app.mouse_drag_enter_copy_mode,
        app.ctrl_wheel_zoom,
        crate::mouse_keys::drag_end_is_default(app),
        crate::mouse_keys::client_overrides(app),
        app.bold_is_bright,
    ));
    if let Err(e) = helpers::append_client_render_options_json(&mut c.combined_buf, &sf.client_render_options) {
        if crate::debug_log::server_log_enabled() {
            crate::debug_log::server_log("render", &format!("client render options omitted: {e}"));
        }
    }
    helpers::append_session_name_json(&app.session_name, &mut c.combined_buf);
    helpers::append_key_table_json(app.current_key_table.as_deref(), &mut c.combined_buf);
    helpers::append_default_key_table_json(helpers::default_key_table(app), &mut c.combined_buf);
    let buf = &mut c.combined_buf;
    if let Some(cc) = app.user_options.get("clock-mode-colour") {
        if buf.ends_with('}') {
            buf.pop();
            buf.push_str(",\"clock_colour\":\"");
            buf.push_str(&json_escape_string(cc));
            buf.push_str("\"}");
        }
    }
    if let Some(pbs) = app.user_options.get("pane-border-status") {
        if buf.ends_with('}') {
            buf.pop();
            buf.push_str(",\"pane_border_status\":\"");
            buf.push_str(&json_escape_string(pbs));
            buf.push('"');
            if let Some(pbf) = app.user_options.get("pane-border-format") {
                buf.push_str(",\"pane_border_format\":\"");
                buf.push_str(&json_escape_string(pbf));
                buf.push('"');
            }
            buf.push('}');
        }
    }
    if let Some(pbl) = app.user_options.get("pane-border-lines") {
        if buf.ends_with('}') {
            buf.pop();
            buf.push_str(",\"pane_border_lines\":\"");
            buf.push_str(&json_escape_string(pbl));
            buf.push('"');
            buf.push('}');
        }
    }
    helpers::append_copy_ln_json(app, buf);
    helpers::append_floats_json(app, buf);
    if let Some(title) = sf.host_title.as_deref() {
        if buf.ends_with('}') {
            buf.pop();
            buf.push_str(",\"host_title\":\"");
            buf.push_str(&json_escape_string(title));
            buf.push_str("\"}");
        }
    }
    if buf.ends_with('}') {
        if let Some((s, v)) = helpers::active_pane_progress(app) {
            buf.pop();
            buf.push_str(",\"host_progress\":\"");
            buf.push_str(&format!("{};{}", s, v));
            buf.push_str("\"}");
        }
    }
    let overlay_json = serialize_overlay_json(app);
    if !overlay_json.is_empty() && buf.ends_with('}') {
        buf.pop();
        buf.push_str(&overlay_json);
        buf.push('}');
    }
    c.cached_dump_state.clear();
    c.cached_dump_state.push_str(&c.combined_buf);
    // OSC 52 from pane children, then the one-shot fields that ride on this
    // frame only (never on the cached copy).
    crate::server::helpers::drain_osc52(app);
    // They go last in the frame, where a frame that supersedes this one
    // unread picks them up (frame_oneshot::supersede), as in run_server.
    crate::frame_oneshot::append(
        &mut c.combined_buf,
        &crate::server::helpers::take_frame_oneshots(app),
    );
    c.cached_data_version = combined_data_version(app);
    Ok(())
}

/// Send a frame to this session's attached clients other than `except`.
pub(crate) fn push_to_session_clients(lp: &SessionLoop, frame: &str, except: Option<u64>) {
    for cid in lp.clients.iter() {
        if Some(*cid) == except {
            continue;
        }
        crate::types::send_directive_to_client(*cid, frame);
    }
}

/// The DumpState request (copied arm). `Ok(())` once `resp` was answered.
pub(crate) fn dump_state(
    app: &mut AppState,
    lp: &mut SessionLoop,
    resp: mpsc::Sender<String>,
    allow_nc: bool,
    dump_client_id: u64,
) -> io::Result<()> {
    lp.state_dirty |= helpers::refresh_program_status_tabs(
        app,
        &mut lp.frame.cached_program_status_version,
        &mut lp.frame.cached_windows_json,
    )?;
    let alerts = helpers::check_window_activity(app);
    if alerts.flags_changed {
        lp.meta_dirty = true;
    }
    for event in &alerts.hooks {
        if let Some(cmds) = app.hooks.get(*event) {
            let cmds = cmds.clone();
            for cmd in &cmds {
                let _ = execute_command_string(app, cmd);
            }
        }
    }
    if helpers::propagate_osc_titles(app) {
        lp.state_dirty = true;
    }
    if auto_rename_windows(app) {
        lp.meta_dirty = true;
        lp.state_dirty = true;
    }
    crate::copy_mode::sync_copy_snapshot(app);
    if crate::copy_mode::tick_auto_refresh(app) {
        lp.state_dirty = true;
    }
    let has_squelch = app.windows.get(app.active_idx)
        .and_then(|w| crate::tree::active_pane(&w.root, &w.active_path))
        .map_or(false, |p| p.squelch_until.is_some());
    if nc_allowed(NcInputs {
        allow_nc,
        state_dirty: lp.state_dirty,
        meta_dirty: lp.meta_dirty,
        bell_forward: app.bell_forward,
        has_squelch,
        have_cached_frame: !lp.frame.cached_dump_state.is_empty(),
        version_matches: lp.frame.cached_data_version == combined_data_version(app),
        seen_full_frame: lp.dump_state_seen_full.contains(&dump_client_id),
    }) {
        let _ = resp.send("NC".to_string());
        return Ok(());
    }
    build_frame(app, &mut lp.frame, &mut lp.meta_dirty)?;
    lp.state_dirty = false;
    // The other clients of THIS session see the update too (run_server pushes
    // to every client of the process; here that would cross sessions).
    push_to_session_clients(lp, &lp.frame.combined_buf, Some(dump_client_id));
    let _ = resp.send(lp.frame.combined_buf.clone());
    lp.dump_state_seen_full.insert(dump_client_id);
    lp.last_push = std::time::Instant::now();
    Ok(())
}

/// The push at the end of a loop iteration (copied), for one session.
pub(crate) fn push(app: &mut AppState, lp: &mut SessionLoop) -> io::Result<()> {
    if lp.state_dirty {
        crate::copy_mode::tick_auto_refresh(app);
    }
    lp.state_dirty |= helpers::refresh_program_status_tabs(
        app,
        &mut lp.frame.cached_program_status_version,
        &mut lp.frame.cached_windows_json,
    )?;
    let push_alerts = helpers::check_window_activity(app);
    if push_alerts.flags_changed {
        lp.meta_dirty = true;
    }
    for event in &push_alerts.hooks {
        crate::commands::fire_hooks(app, event);
    }
    build_frame(app, &mut lp.frame, &mut lp.meta_dirty)?;
    lp.state_dirty = false;
    push_to_session_clients(lp, &lp.frame.combined_buf, None);
    lp.last_push = std::time::Instant::now();
    Ok(())
}

/// automatic-rename / allow-rename, copied from the DumpState arm. True when
/// a window name changed.
fn auto_rename_windows(app: &mut AppState) -> bool {
    let mut changed = false;
    let in_copy = app.mode.in_copy();
    let global_auto_rename = app.automatic_rename;
    let allow_rename = app.allow_rename;
    let any_auto_rename = global_auto_rename
        || app.windows.iter().any(|w| {
            crate::server::options::win_flag(w, "automatic-rename", global_auto_rename)
        });
    if (any_auto_rename || allow_rename) && !in_copy {
        for win in app.windows.iter_mut() {
            if win.manual_rename { continue; }
            let auto_rename =
                crate::server::options::win_flag(win, "automatic-rename", global_auto_rename);
            if !auto_rename && !allow_rename { continue; }
            let last_output = win.last_output_time;
            if let Some(p) = crate::tree::active_pane_mut(&mut win.root, &win.active_path) {
                if p.dead { continue; }
                if !helpers::window_name_check_due(last_output, p.last_title_check, 1000) { continue; }
                p.last_title_check = std::time::Instant::now();
                if p.child_pid.is_none() {
                    p.child_pid = crate::platform::mouse_inject::get_child_pid(&*p.child);
                }
                let new_name = if auto_rename {
                    if let Some(pid) = p.child_pid {
                        match crate::platform::process_info::get_foreground_process_name(pid) {
                            Some(name) => name,
                            None => continue,
                        }
                    } else if allow_rename && !p.title.is_empty() {
                        p.title.clone()
                    } else {
                        continue;
                    }
                } else if allow_rename {
                    if let Ok(parser) = p.term.lock() {
                        let title = parser.screen().title();
                        if !title.is_empty() {
                            title.to_string()
                        } else {
                            continue;
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };
                let new_name = crate::util::clean_name(&new_name);
                if !new_name.is_empty() && win.name != new_name {
                    win.name = new_name;
                    changed = true;
                }
            }
        }
    }
    changed
}
