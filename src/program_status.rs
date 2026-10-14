#[cfg(test)]
#[path = "../tests-rs/test_program_status.rs"]
mod tests;

use serde::{Deserialize, Serialize};

use crate::types::{AppState, Node, Pane, Window};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Attention {
    pub pane: usize,
    pub id: String,
    pub state: String,
    pub kind: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct RecordInfo {
    id: String,
    state: String,
    kind: Option<String>,
    progress: Option<u8>,
    app: Option<String>,
    title: Option<String>,
    msg: Option<String>,
    seen: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PaneInfo {
    pane: usize,
    records: Vec<RecordInfo>,
}

fn panes(node: &Node, visit: &mut impl FnMut(&Pane)) {
    match node {
        Node::Leaf(pane) => visit(pane),
        Node::Split { children, .. } => {
            for child in children { panes(child, visit); }
        }
    }
}

fn window_panes(window: &Window, mut visit: impl FnMut(&Pane)) {
    panes(&window.root, &mut visit);
    for float in &window.floating { visit(&float.pane); }
}

/// Apply the `program-status` option. Every pane's parser thread also reads
/// the atomic before each batch; switching the live parsers here as well makes
/// the change, and the withdrawal of records when it goes off, immediate.
pub(crate) fn set_enabled(app: &mut AppState, enabled: bool) {
    app.program_status = enabled;
    crate::types::PROGRAM_STATUS_ENABLED.store(enabled, std::sync::atomic::Ordering::Release);
    for window in &app.windows {
        window_panes(window, |pane| {
            if let Ok(mut parser) = pane.live_parser().lock() {
                parser.screen_mut().set_program_status_enabled(enabled);
            }
        });
    }
}

pub(crate) fn window_attention(window: &Window) -> Option<Attention> {
    let mut selected = None;
    let mut best = None;
    window_panes(window, |pane| {
        if let Ok(parser) = pane.live_parser().lock() {
            let statuses = parser.screen().program_status();
            if let Some(status) = statuses.attention() {
                let priority = match status.state {
                    vt100::ProgramState::Blocked => 3,
                    vt100::ProgramState::Error => 2,
                    _ => 1,
                };
                let key = (priority, statuses.attention_changed(), pane.id);
                if best.is_none_or(|previous| key > previous) {
                    best = Some(key);
                    selected = Some(Attention {
                        pane: pane.id,
                        id: status.id.clone(),
                        state: status.state.as_str().to_owned(),
                        kind: status.kind.map(|kind| kind.as_str().to_owned()),
                    });
                }
            }
        }
    });
    selected
}

pub(crate) fn window_records(window: &Window) -> Vec<PaneInfo> {
    let mut result = Vec::new();
    window_panes(window, |pane| {
        if let Ok(parser) = pane.live_parser().lock() {
            let statuses = parser.screen().program_status();
            let records: Vec<_> = statuses.records().map(|status| RecordInfo {
                id: status.id.clone(),
                state: status.state.as_str().to_owned(),
                kind: status.kind.map(|kind| kind.as_str().to_owned()),
                progress: status.progress,
                app: statuses.inherited_app(&status.id).map(str::to_owned),
                title: status.title.clone(),
                msg: status.msg.clone(),
                seen: statuses.seen(&status.id),
            }).collect();
            if !records.is_empty() { result.push(PaneInfo { pane: pane.id, records }); }
        }
    });
    result
}

/// Only semantic changes in background panes invalidate a quiet status frame.
pub(crate) fn version(app: &AppState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut version = std::collections::hash_map::DefaultHasher::new();
    for window in &app.windows {
        window.id.hash(&mut version);
        window_panes(window, |pane| {
            pane.id.hash(&mut version);
            if let Ok(parser) = pane.live_parser().lock() {
                parser.screen().program_status().generation().hash(&mut version);
            }
        });
    }
    version.finish()
}

pub(crate) fn focused_pane(app: &AppState) -> Option<&Pane> {
    let window = app.windows.get(app.active_idx)?;
    if let Some(float) = window.floating_focus.and_then(|index| window.floating.get(index)) {
        Some(&float.pane)
    } else {
        crate::tree::active_pane(&window.root, &window.active_path)
    }
}

pub(crate) fn focused_identity(app: &AppState) -> Option<(usize, usize)> {
    Some((app.windows.get(app.active_idx)?.id, focused_pane(app)?.id))
}

pub(crate) fn acknowledge_focused(app: &AppState) {
    if let Some(pane) = focused_pane(app) {
        if let Ok(mut parser) = pane.live_parser().lock() {
            // The return linearizes at this locked operation, before focus is published.
            parser.screen_mut().program_status_mut().acknowledge();
        }
    }
}

pub(crate) fn pane_format(pane: &Pane, var: &str) -> String {
    let Ok(parser) = pane.live_parser().lock() else { return String::new(); };
    let statuses = parser.screen().program_status();
    if var == "pane_ps_attention_state" {
        return statuses.attention().map_or(String::new(), |s| s.state.as_str().to_owned());
    }
    if var == "pane_ps_attention_id" {
        return statuses.attention().map_or(String::new(), |s| s.id.clone());
    }
    let Some(root) = statuses.get("") else { return String::new(); };
    match var {
        "pane_ps_state" => root.state.as_str().to_owned(),
        "pane_ps_kind" => root.kind.map_or(String::new(), |k| k.as_str().to_owned()),
        "pane_ps_progress" => root.progress.map_or(String::new(), |p| p.to_string()),
        "pane_ps_app" => root.app.clone().unwrap_or_default(),
        _ => String::new(),
    }
}
