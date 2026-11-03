//! The `pane-colours` option (issue #980), also spelled `pane-colors`.
//!
//! tmux 3.8 declares it a window and pane option holding an array of colours
//! with no default (options-table.c:1614-1620, the other name at :284). Its
//! elements 0 to 255 are a pane's DEFAULT palette: an indexed colour the pane
//! draws goes through the pane's OSC 4 entry first, then through this table
//! (colour_palette_get, colour.c:1251-1268), and an OSC 4 query reports the
//! same colour (input.c:2948). RIS and OSC 104 drop only the OSC 4 entries
//! (colour.c:1227, input.c:1407, input.c:3453).
//!
//! Three tables can hold it, as in tmux: the global window table (`set -gw`),
//! a window's own (`set -w`) and a pane's own (`set -p`). A pane uses the
//! first of its own, its window's and the global one that holds the option,
//! whole; the arrays are never merged (options_get walks the parents and
//! stops at the first table with the entry).
//!
//! psmux keeps the global array beside the other arrays of
//! crate::option_array, and the window and pane tables here keyed by window
//! and pane id, so no `Window` or `Pane` grows a field. The palette is pushed
//! into each pane's vt100 screen by [`reconcile`], which the server loop calls
//! once per iteration. It returns at its first line until somebody writes the
//! option, so a user who never sets it pays one boolean test per iteration
//! and no pane screen is touched.

use std::collections::HashMap;

use crate::option_array::{ArrayKey, OptionArray};
use crate::types::{AppState, Node, Pane, Window};

/// The option's name as tmux shows it.
pub(crate) const OPTION: &str = "pane-colours";
/// The other spelling tmux accepts (options-table.c:284).
pub(crate) const ALIAS: &str = "pane-colors";

/// The part of `name` after the option's base name (`""` or `[N]`), when
/// `name` is this option under either spelling.
fn suffix(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(OPTION).or_else(|| name.strip_prefix(ALIAS))?;
    (rest.is_empty() || rest.starts_with('[')).then_some(rest)
}

/// True when `name` is this option, bare or with an index, either spelling.
pub(crate) fn is_name(name: &str) -> bool {
    suffix(name).is_some()
}

/// `name` with the base spelled `pane-colours`, as tmux prints it.
pub(crate) fn canonical_name(name: &str) -> Option<String> {
    suffix(name).map(|rest| format!("{}{}", OPTION, rest))
}

/// One colour as tmux keeps it (colour_fromstring then colour_tostring,
/// colour.c:387-462), or tmux's error for a value that is no colour.
fn canonical_colour(value: &str) -> Result<String, String> {
    crate::style::option_colour_name(value).ok_or_else(|| format!("bad colour: {}", value))
}

/// The value a `set` stores: one colour for an element, or, for the whole
/// array, every piece between spaces and commas checked and kept in tmux's
/// spelling (the option names no separator, so options_array_assign splits
/// on `" ,"`, options.c:623-625). A piece that is no colour fails the whole
/// set, so nothing is half written.
pub(crate) fn canonical_value(key: Option<&ArrayKey>, value: &str) -> Result<String, String> {
    if key.is_some() {
        return canonical_colour(value);
    }
    let pieces = value
        .split([' ', ','])
        .filter(|piece| !piece.is_empty())
        .map(canonical_colour)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pieces.join(","))
}

/// The check `set` makes before anything is sent to the server.
pub(crate) fn validate(name: &str, value: &str) -> Result<(), String> {
    let (_, key) = crate::option_array::parse_name(name)?;
    canonical_value(key.as_ref(), value).map(|_| ())
}

/// A colour in tmux's spelling as the vt100 colour it draws with.
fn draw_colour(name: &str) -> Option<vt100::Color> {
    const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    if let Some(hex) = name.strip_prefix('#') {
        let n = u32::from_str_radix(hex, 16).ok()?;
        return Some(vt100::Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    if let Some(number) = name.strip_prefix("colour") {
        return number.parse::<u8>().ok().map(vt100::Color::Idx);
    }
    if name == "default" || name == "terminal" {
        return Some(vt100::Color::Default);
    }
    if let Some(i) = NAMES.iter().position(|n| *n == name) {
        return Some(vt100::Color::Idx(i as u8));
    }
    let rest = name.strip_prefix("bright")?;
    NAMES.iter().position(|n| *n == rest).map(|i| vt100::Color::Idx(8 + i as u8))
}

/// A pane's default palette from the array that reaches it: elements 0 to
/// 255 (colour_palette_from_option, colour.c:1293-1322), `None` when there
/// is none.
pub(crate) fn palette_of(array: &OptionArray) -> Option<vt100::ColourPalette> {
    let mut palette = vt100::ColourPalette::default();
    let mut any = false;
    for (index, value) in array.numbered() {
        let Ok(index) = u8::try_from(index) else { break };
        if let Some(colour) = draw_colour(value) {
            palette.set_colour(index, Some(colour));
            any = true;
        }
    }
    any.then_some(palette)
}

/// What a pane last had installed, so [`reconcile`] does nothing for a pane
/// whose window, parser and option tables have not moved.
///
/// The parser is held weakly: while this lives its allocation cannot be
/// handed to a new parser, so a parser that replaced it (respawn, a dead
/// pane's restart) never compares equal by address.
#[derive(Debug)]
struct Applied {
    window: usize,
    parser: std::sync::Weak<std::sync::Mutex<vt100::Parser>>,
    generation: u64,
}

impl Applied {
    fn is(&self, window: usize, parser: &std::sync::Arc<std::sync::Mutex<vt100::Parser>>, generation: u64) -> bool {
        self.window == window && self.generation == generation && std::ptr::eq(self.parser.as_ptr(), std::sync::Arc::as_ptr(parser))
    }
}

/// The window and pane tables, and what has been pushed into the panes.
#[derive(Debug, Default)]
pub(crate) struct PaneColours {
    /// The global window table (`set -gw`), which always holds the option,
    /// empty by default (options-table.c:1617 `.default_str = ""`).
    pub(crate) global: OptionArray,
    /// A window's own table, by window id. An entry is there from the first
    /// `set -w` until `set -wu pane-colours` removes it, even when its
    /// elements have all been unset, as tmux keeps the option entry.
    windows: HashMap<usize, OptionArray>,
    /// A pane's own table, by pane id, the same way.
    panes: HashMap<usize, OptionArray>,
    /// Set by the first write to any table. Until then [`reconcile`] returns
    /// at once.
    engaged: bool,
    /// Bumped by every write, so every pane is looked at again.
    generation: u64,
    applied: HashMap<usize, Applied>,
}

impl PaneColours {
    /// Note a write to any table.
    pub(crate) fn changed(&mut self) {
        self.engaged = true;
        self.generation = self.generation.wrapping_add(1);
    }
}

/// Which table a `set` or `show` names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Global,
    Window,
    Pane,
}

fn walk<'a>(node: &'a Node, visit: &mut impl FnMut(&'a Pane)) {
    match node {
        Node::Leaf(pane) => visit(pane),
        Node::Split { children, .. } => {
            for child in children {
                walk(child, visit);
            }
        }
    }
}

fn window_panes<'a>(window: &'a Window, mut visit: impl FnMut(&'a Pane)) {
    walk(&window.root, &mut visit);
    for float in &window.floating {
        visit(&float.pane);
    }
}

/// The window id and pane id a `-p` target names: "" for the active pane, a
/// `%N` or `N` pane id, or a pane spec, as
/// crate::server::options::resolve_option_target_pane reads them.
fn find_pane(app: &AppState, target: &str) -> Result<(usize, usize), String> {
    let raw = target.trim().trim_matches('"');
    let miss = || format!("can't find pane: {}", raw);
    let pane_id = if raw.is_empty() {
        let window = app.windows.get(app.active_idx).ok_or_else(miss)?;
        crate::tree::get_active_pane_id_at_path(&window.root, &window.active_path).ok_or_else(miss)?
    } else if let Ok(id) = raw.trim_start_matches('%').parse::<usize>() {
        id
    } else {
        let (window_pos, path) = crate::window_ops::resolve_pane_spec(app, raw)?;
        let window = app.windows.get(window_pos).ok_or_else(miss)?;
        crate::tree::get_active_pane_id_at_path(&window.root, &path).ok_or_else(miss)?
    };
    let (window_index, _) = crate::tree::find_pane_by_id_global(app, pane_id).ok_or_else(miss)?;
    Ok((app.windows[window_index].id, pane_id))
}

/// The window id a `-w` target names.
fn find_window(app: &AppState, target: &str) -> Result<usize, String> {
    let index = crate::server::options::resolve_option_target_window(app, target)?;
    app.windows.get(index).map(|w| w.id).ok_or_else(|| format!("can't find window: {}", target))
}

/// The table a pane draws with and whether it is the pane's own: its own,
/// else its window's, else the global one (options_get).
fn effective<'a>(tables: &'a PaneColours, window_id: usize, pane_id: usize) -> (&'a OptionArray, Scope) {
    if let Some(array) = tables.panes.get(&pane_id) {
        return (array, Scope::Pane);
    }
    if let Some(array) = tables.windows.get(&window_id) {
        return (array, Scope::Window);
    }
    (&tables.global, Scope::Global)
}

/// `set-option -w` or `-p` on the option (cmd-set-option.c): the reply the
/// caller prints, empty on success. `-g` never comes here; the global array
/// goes through crate::server::options::set_array_option like the others.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_set(
    app: &mut AppState,
    scope: Scope,
    target: &str,
    option: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let error = |e: String| format!("ERROR: {}", e);
    let Some(name) = canonical_name(option) else {
        return error(format!("invalid option: {}", option));
    };
    let key = match crate::option_array::parse_name(&name) {
        Ok((_, key)) => key,
        Err(e) => return error(e),
    };
    let id = match scope {
        Scope::Window => find_window(app, target),
        Scope::Pane => find_pane(app, target).map(|(_, pane)| pane),
        Scope::Global => Ok(0),
    };
    let id = match id {
        Ok(id) => id,
        Err(e) => return error(e),
    };
    let spec = crate::option_array::spec(OPTION).expect("pane-colours is in ARRAY_OPTIONS");
    let tables = &mut app.pane_colours;
    let table = match scope {
        Scope::Window => &mut tables.windows,
        Scope::Pane => &mut tables.panes,
        Scope::Global => {
            // Not a route anything takes; the global array has its own.
            return error(format!("invalid option: {}", option));
        }
    };
    if unset {
        match key {
            // One element goes; the table keeps the option.
            Some(key) => {
                if let Some(array) = table.get_mut(&id) {
                    array.set(key, None, false);
                }
            }
            // The whole option leaves the table so the parent shows through
            // (options_remove_or_default, options.c:1275).
            None => {
                table.remove(&id);
            }
        }
        tables.changed();
        return String::new();
    }
    if only_if_unset {
        let held = table.get(&id).is_some_and(|array| match &key {
            Some(key) => array.get(key).is_some(),
            None => true,
        });
        if held {
            return if quiet { String::new() } else { error(format!("already set: {}", option)) };
        }
    }
    let value = match canonical_value(key.as_ref(), value) {
        Ok(value) => value,
        Err(e) => return error(e),
    };
    // `-a` on an element sets it: only a string element appends
    // (options_array_set, options.c). On the whole array it adds without
    // clearing first (cmd-set-option.c).
    let append = append && key.is_none();
    table.entry(id).or_default().apply_set(key, &value, append, spec);
    tables.changed();
    String::new()
}

/// `set-option -g` on the global array checked and spelled as tmux keeps it.
/// The caller then hands the result to the array's own `apply_set`.
pub(crate) fn global_value(key: Option<&ArrayKey>, value: &str) -> Result<String, String> {
    canonical_value(key, value)
}

/// `show-options` for the option (cmd-show-options.c:118-140): the table the
/// flags name, or with `inherited` the first table up the chain that holds
/// it, its lines marked `*` when it is not the one named. A table that does
/// not hold the option prints nothing.
pub(crate) fn show(
    app: &AppState,
    scope: Scope,
    target: &str,
    option: &str,
    inherited: bool,
    values_only: bool,
) -> String {
    let Some(name) = canonical_name(option) else {
        return format!("ERROR: invalid option: {}\n", option);
    };
    let key = match crate::option_array::parse_name(&name) {
        Ok((_, key)) => key,
        Err(e) => return format!("ERROR: {}\n", e),
    };
    let tables = &app.pane_colours;
    let (array, parent) = match scope {
        Scope::Global => (Some(&tables.global), false),
        Scope::Window => {
            let id = match find_window(app, target) {
                Ok(id) => id,
                Err(e) => return format!("ERROR: {}\n", e),
            };
            match tables.windows.get(&id) {
                Some(array) => (Some(array), false),
                None if inherited => (Some(&tables.global), true),
                None => (None, false),
            }
        }
        Scope::Pane => {
            let (window_id, pane_id) = match find_pane(app, target) {
                Ok(ids) => ids,
                Err(e) => return format!("ERROR: {}\n", e),
            };
            match tables.panes.get(&pane_id) {
                Some(array) => (Some(array), false),
                None if inherited => (Some(effective(tables, window_id, pane_id).0), true),
                None => (None, false),
            }
        }
    };
    let Some(array) = array else {
        return String::new();
    };
    let mark = if parent { "*" } else { "" };
    let line = |key: &ArrayKey, value: &str| {
        if values_only {
            format!("{}\n", value)
        } else {
            format!("{}[{}]{} {}\n", OPTION, key, mark, value)
        }
    };
    match key {
        Some(key) => line(&key, array.get(&key).unwrap_or_default()),
        None if array.is_empty() => {
            if values_only { String::new() } else { format!("{}{}\n", OPTION, mark) }
        }
        None => array.iter().map(|(key, value)| line(key, value)).collect(),
    }
}

/// `#{pane-colours}` for one pane: the array that reaches it joined with
/// spaces, or one element (options_to_string, options.c:590-614).
pub(crate) fn format_value(app: &AppState, window: Option<&Window>, pane: Option<&Pane>, var: &str) -> String {
    let Some(name) = canonical_name(var) else { return String::new() };
    let Ok((_, key)) = crate::option_array::parse_name(&name) else { return String::new() };
    let tables = &app.pane_colours;
    let array = match (window, pane) {
        (Some(window), Some(pane)) => effective(tables, window.id, pane.id).0,
        (Some(window), None) => tables.windows.get(&window.id).unwrap_or(&tables.global),
        _ => &tables.global,
    };
    match key {
        Some(key) => array.get(&key).unwrap_or_default().to_string(),
        None => array.joined(),
    }
}

/// Push each pane's default palette into its screen when it is new, moved to
/// another window, has a new parser, or the tables changed (tmux rebuilds it
/// at the same moments: options.c:1411-1414, window.c:1333,
/// cmd-break-pane.c:178, cmd-join-pane.c:508, cmd-swap-pane.c:181-182).
/// Returns `true` when any pane's palette changed, so the frame is redrawn.
pub(crate) fn reconcile(app: &mut AppState) -> bool {
    if !app.pane_colours.engaged {
        return false;
    }
    let tables = &mut app.pane_colours;
    let generation = tables.generation;
    let mut redraw = false;
    let mut seen = 0usize;
    for window in &app.windows {
        window_panes(window, |pane| {
            seen += 1;
            let live = pane.live_parser();
            if tables.applied.get(&pane.id).is_some_and(|a| a.is(window.id, live, generation)) {
                return;
            }
            let now = Applied { window: window.id, parser: std::sync::Arc::downgrade(live), generation };
            let palette = palette_of(effective(tables, window.id, pane.id).0);
            for term in pane.each_term() {
                if let Ok(mut parser) = term.lock() {
                    if parser.screen_mut().set_pane_colours(palette.clone()) {
                        redraw = true;
                    }
                }
            }
            if let Ok(parser) = pane.live_parser().lock() {
                crate::types::publish_pane_palette(pane.id, crate::pane::palette16(parser.screen()));
            }
            tables.applied.insert(pane.id, now);
        });
    }
    if tables.applied.len() > seen {
        let mut live = std::collections::HashSet::new();
        for window in &app.windows {
            window_panes(window, |pane| {
                live.insert(pane.id);
            });
        }
        tables.applied.retain(|id, _| live.contains(id));
        tables.panes.retain(|id, _| live.contains(id));
        let windows: std::collections::HashSet<usize> = app.windows.iter().map(|w| w.id).collect();
        tables.windows.retain(|id, _| windows.contains(id));
    }
    redraw
}

#[cfg(test)]
#[path = "../tests-rs/test_issue980_pane_colours.rs"]
mod tests;
