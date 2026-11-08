//! The `scroll-on-clear` option, a flag on a window and on a pane
//! (options-table.c:1723 at tag 3.8). The clear itself is in vt100's screen,
//! which cannot read options, so each pane's screen holds the value and this
//! keeps it up to date, the way crate::pane_colours keeps the default palette.

use std::collections::HashMap;

use crate::pane_colours::{window_panes, Applied};
use crate::types::AppState;

pub(crate) const OPTION: &str = "scroll-on-clear";

/// What has been pushed into the panes.
#[derive(Debug, Default)]
pub(crate) struct ScrollOnClear {
    /// Set by the first write at any scope. Until then every screen has the
    /// default, on, and [`reconcile`] returns at once.
    engaged: bool,
    /// Bumped by every write, so every pane is looked at again.
    generation: u64,
    applied: HashMap<usize, Applied>,
}

impl ScrollOnClear {
    /// Note a write at any scope.
    pub(crate) fn changed(&mut self) {
        self.engaged = true;
        self.generation = self.generation.wrapping_add(1);
    }
}

fn is_on(value: &str) -> bool {
    matches!(value, "on" | "true" | "1" | "yes")
}

/// Push the value each pane resolves to, its own, its window's, then the
/// global one (options.c `options_get` walks the parents), into its screen
/// when the pane is new, moved to another window, has a new parser, or an
/// option changed.
pub(crate) fn reconcile(app: &mut AppState) {
    if !app.scroll_on_clear.engaged {
        return;
    }
    let global = crate::server::options::get_option_value(app, OPTION);
    let state = &mut app.scroll_on_clear;
    let generation = state.generation;
    let mut seen = 0usize;
    for window in &app.windows {
        window_panes(window, |pane| {
            seen += 1;
            let live = pane.live_parser();
            if state.applied.get(&pane.id).is_some_and(|a| a.is(window.id, live, generation)) {
                return;
            }
            let now = Applied::new(window.id, live, generation);
            let on = is_on(
                pane.pane_options
                    .get(OPTION)
                    .or_else(|| window.window_options.get(OPTION))
                    .unwrap_or(&global),
            );
            for term in pane.each_term() {
                if let Ok(mut parser) = term.lock() {
                    parser.screen_mut().set_scroll_on_clear(on);
                }
            }
            state.applied.insert(pane.id, now);
        });
    }
    if state.applied.len() > seen {
        let mut live = std::collections::HashSet::new();
        for window in &app.windows {
            window_panes(window, |pane| {
                live.insert(pane.id);
            });
        }
        state.applied.retain(|id, _| live.contains(id));
    }
}
