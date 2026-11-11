//! The `cursor-colour` option, a colour on a window and on a pane
//! (options-table.c:354 at tag 3.8). tmux keeps the colour it gives in each
//! pane's screen as `s->default_ccolour` (`screen_set_default_cursor`,
//! screen.c:206), under the colour a program sets with `OSC 12`. vt100's
//! screen cannot read options, so this keeps each pane's copy up to date, the
//! way crate::scroll_on_clear keeps its flag.

use std::collections::HashMap;

use crate::pane_colours::{window_panes, Applied};
use crate::types::AppState;

pub(crate) const OPTION: &str = "cursor-colour";

/// What has been pushed into the panes.
#[derive(Debug, Default)]
pub(crate) struct CursorColourOption {
    /// Set by the first write at any scope. Until then every screen has no
    /// default, and [`reconcile`] returns at once.
    engaged: bool,
    /// Bumped by every write, so every pane is looked at again.
    generation: u64,
    applied: HashMap<usize, Applied>,
}

impl CursorColourOption {
    /// Note a write at any scope.
    pub(crate) fn changed(&mut self) {
        self.engaged = true;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// Whether `value` is a colour the option takes: empty (no colour, the
/// default), a format, which tmux checks only once expanded (options.c:1205),
/// or a colour by name, number or `#rrggbb`. tmux refuses anything else with
/// `invalid colour` (options.c:1208).
pub(crate) fn validate(value: &str) -> Result<(), String> {
    if value.is_empty() || value.contains("#{") || crate::style::option_colour_name(value).is_some() {
        Ok(())
    } else {
        Err(format!("invalid colour: {}", value))
    }
}

/// The colour `value` asks the terminal to draw the cursor in, as tmux's
/// `tty_force_cursor_colour` turns it into one with `colour_force_rgb`: a
/// palette colour is its xterm value (`blue` is `#000080`), and `default`,
/// `terminal` or no colour at all is `None`, the terminal's own cursor. psmux
/// does not expand a format here, so a format is `None` too.
pub(crate) fn rgb(value: &str) -> Option<(u8, u8, u8)> {
    const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    let name = crate::style::option_colour_name(value)?;
    if let Some(hex) = name.strip_prefix('#') {
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some(((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    if let Some(n) = name.strip_prefix("colour") {
        return n.parse::<u8>().ok().map(vt100::xterm_rgb);
    }
    if let Some(rest) = name.strip_prefix("bright") {
        let i = NAMES.iter().position(|c| *c == rest)?;
        return Some(vt100::xterm_rgb(8 + i as u8));
    }
    let i = NAMES.iter().position(|c| *c == name)?;
    Some(vt100::xterm_rgb(i as u8))
}

/// `#{cursor_colour}`: the program's colour as `#rrggbb`, then the option's
/// colour by its name, then `none`, as tmux's `format_cb_cursor_colour`
/// writes them with `colour_tostring` (format.c:1296).
pub(crate) fn format_value(program: Option<(u8, u8, u8)>, option: &str) -> String {
    if let Some((r, g, b)) = program {
        return format!("#{r:02x}{g:02x}{b:02x}");
    }
    match crate::style::option_colour_name(option) {
        Some(name) => name,
        None => "none".to_string(),
    }
}

/// The value a pane resolves to: its own, its window's, then the global one
/// (options.c `options_get` walks the parents).
pub(crate) fn pane_value(app: &AppState, window: &crate::types::Window, pane: &crate::types::Pane) -> String {
    pane.pane_options
        .get(OPTION)
        .or_else(|| window.window_options.get(OPTION))
        .cloned()
        .unwrap_or_else(|| crate::server::options::get_option_value(app, OPTION))
}

/// Push the colour each pane resolves to into its screen when the pane is
/// new, moved to another window, has a new parser, or an option changed.
/// Returns `true` when the colour to draw changed for any pane, so the frame
/// is sent again.
pub(crate) fn reconcile(app: &mut AppState) -> bool {
    if !app.cursor_colour.engaged {
        return false;
    }
    let global = crate::server::options::get_option_value(app, OPTION);
    let state = &mut app.cursor_colour;
    let generation = state.generation;
    let mut seen = 0usize;
    let mut redraw = false;
    for window in &app.windows {
        window_panes(window, |pane| {
            seen += 1;
            let live = pane.live_parser();
            if state.applied.get(&pane.id).is_some_and(|a| a.is(window.id, live, generation)) {
                return;
            }
            let now = Applied::new(window.id, live, generation);
            let colour = rgb(pane
                .pane_options
                .get(OPTION)
                .or_else(|| window.window_options.get(OPTION))
                .unwrap_or(&global));
            for term in pane.each_term() {
                if let Ok(mut parser) = term.lock() {
                    redraw |= parser.screen_mut().set_default_cursor_colour(colour);
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
    redraw
}

#[cfg(test)]
#[path = "../tests-rs/test_cursor_colour_option.rs"]
mod tests;
