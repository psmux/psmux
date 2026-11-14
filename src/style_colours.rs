//! What `window-style` and `window-active-style` give each pane's default
//! foreground and background, pushed into the pane's screen so that a
//! program's `OSC 10 ; ?` and `OSC 11 ; ?` are answered with them, as tmux
//! answers from `tty_default_colours` (tty.c:3164 at tag 3.8, through
//! `window_pane_get_bg`, window.c:2699). The order of these and the program's
//! own colours is vt100's `StyleColour`. psmux reads both options globally,
//! as its client draws with them, and a float's `new-pane -s` style over
//! both.

use std::collections::HashMap;

use crate::pane_colours::Applied;
use crate::types::{AppState, Node, Pane};

/// The colours one style names, as tmux's `colour_force_rgb` gives them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Colours {
    pub(crate) fg: Option<(u8, u8, u8)>,
    pub(crate) bg: Option<(u8, u8, u8)>,
}

impl Colours {
    pub(crate) fn of(style: &str) -> Self {
        let style = crate::style::parse_tmux_style(style);
        Self {
            fg: style.fg.and_then(crate::style::colour_force_rgb),
            bg: style.bg.and_then(crate::style::colour_force_rgb),
        }
    }

    fn any(self) -> bool {
        self.fg.is_some() || self.bg.is_some()
    }

    /// These with `over`'s colours laid on top.
    fn under(self, over: Self) -> Self {
        Self { fg: over.fg.or(self.fg), bg: over.bg.or(self.bg) }
    }
}

type Pushed = (vt100::StyleColour, vt100::StyleColour);

/// What has been pushed into the panes.
#[derive(Debug, Default)]
pub(crate) struct StyleColours {
    /// The two options as last read, so a pass that finds them unchanged
    /// costs two lookups and two compares.
    values: Option<(String, String)>,
    /// `window-style`.
    style: Colours,
    /// `window-active-style`.
    active_style: Colours,
    /// Set once a style has named a colour. Until then every screen has none
    /// from the styles, and [`reconcile`] returns after the compare.
    engaged: bool,
    /// What each pane was given, and into which parser.
    applied: HashMap<usize, (Applied, Pushed)>,
}

/// What the styles give a pane's foreground and background: the active
/// pane's `window-active-style` over the program's colour and `window-style`
/// under it; any other pane's `window-style` over the program's colour.
pub(crate) fn for_pane(style: Colours, active_style: Colours, active: bool) -> Pushed {
    let one = |style: Option<(u8, u8, u8)>, active_style: Option<(u8, u8, u8)>| {
        if active {
            vt100::StyleColour { over: active_style, under: style }
        } else {
            vt100::StyleColour { over: style, under: None }
        }
    };
    (one(style.fg, active_style.fg), one(style.bg, active_style.bg))
}

fn push(applied: &mut HashMap<usize, (Applied, Pushed)>, window: usize, pane: &Pane, now: Pushed) {
    let live = pane.live_parser();
    if applied.get(&pane.id).is_some_and(|(a, was)| *was == now && a.is(window, live, 0)) {
        return;
    }
    for term in pane.each_term() {
        if let Ok(mut parser) = term.lock() {
            parser.screen_mut().set_style_colours(now.0, now.1);
        }
    }
    applied.insert(pane.id, (Applied::new(window, live, 0), now));
}

/// Push the styles' colours into every pane whose window, parser or colours
/// moved since the last push.
pub(crate) fn reconcile(app: &mut AppState) {
    let read = |key: &str| app.user_options.get(key).map(String::as_str).unwrap_or("");
    let (style, active_style) = (read("window-style"), read("window-active-style"));
    let same = app
        .style_colours
        .values
        .as_ref()
        .is_some_and(|(s, a)| s == style && a == active_style);
    if !same {
        // Expanded as the client's copy is (server/helpers.rs).
        let colours = Colours::of(&crate::format::expand_format(style, app));
        let active_colours = Colours::of(&crate::format::expand_format(active_style, app));
        let values = (style.to_string(), active_style.to_string());
        let state = &mut app.style_colours;
        state.values = Some(values);
        state.style = colours;
        state.active_style = active_colours;
        state.engaged |= colours.any() || active_colours.any();
    }
    let state = &mut app.style_colours;
    let any_float_style = app
        .windows
        .iter()
        .any(|w| w.floating.iter().any(|f| !f.styles.style.is_empty()));
    if !state.engaged && !any_float_style {
        return;
    }
    state.engaged = true;
    let (global, global_active) = (state.style, state.active_style);
    let mut seen = 0usize;
    for window in &app.windows {
        let float_focused = window.floating_focus.is_some();
        let active_id = crate::tree::get_active_pane_id_at_path(&window.root, &window.active_path);
        let mut tiled = |pane: &Pane| {
            seen += 1;
            // A focused float is the active pane, so no tiled pane is.
            let active = !float_focused && Some(pane.id) == active_id;
            push(&mut state.applied, window.id, pane, for_pane(global, global_active, active));
        };
        walk(&window.root, &mut tiled);
        for (i, float) in window.floating.iter().enumerate() {
            seen += 1;
            let own = Colours::of(&float.styles.style);
            let now = for_pane(global.under(own), global_active.under(own), window.floating_focus == Some(i));
            push(&mut state.applied, window.id, &float.pane, now);
        }
    }
    if state.applied.len() > seen {
        let mut live = std::collections::HashSet::new();
        for window in &app.windows {
            crate::pane_colours::window_panes(window, |pane| {
                live.insert(pane.id);
            });
        }
        state.applied.retain(|id, _| live.contains(id));
    }
}

fn walk(node: &Node, visit: &mut impl FnMut(&Pane)) {
    match node {
        Node::Leaf(pane) => visit(pane),
        Node::Split { children, .. } => {
            for child in children {
                walk(child, visit);
            }
        }
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_style_colours.rs"]
mod tests;
