//! `pane-colours`, a pane's default colour palette (tmux 3.8
//! options-table.c:1629-1635: a window and pane option of colours, an array
//! with no default).
//!
//! Each numbered element from 0 to 255 names the colour that index is drawn
//! in, unless the pane's program set that index itself with OSC 4, which
//! wins (colour.c:1251-1268, `colour_palette_get`); OSC 104, RIS and
//! `send-keys -R` drop the OSC 4 entries only, so the default shows again.
//!
//! The option lives in three tables, as in tmux: the global window table
//! (`set -gw`), a window's own (`set -w`) and a pane's own (`set -p`). A pane
//! uses the first of its own, its window's and the global array that exists,
//! whole, as tmux's `options_get` walks the tables (the arrays are not
//! merged). psmux keeps the window and pane arrays in `AppState`, keyed by
//! window and pane id.
//!
//! Each pane's default palette is built from its array when the option or
//! the pane's window changes (tmux's `colour_palette_from_option`, called from
//! options_push_changes, options.c:1433-1436, and when a pane is created or
//! moved) and handed to the pane's vt100 screen, where the frame serialiser
//! reads it with the OSC 4 palette (`Screen::palette_view`). The low sixteen,
//! as RGB, are published for the OSC 4 query replies.

use std::sync::Mutex;

use crate::option_array::OptionArray;
use crate::types::AppState;

pub const OPTION: &str = "pane-colours";

/// tmux's other name for the option (options-table.c:288,
/// options_other_names).
pub const ALIAS: &str = "pane-colors";

/// True for `pane-colours`, `pane-colors` or one element of either.
pub fn is_option(name: &str) -> bool {
    crate::option_array::is_array_name(name).is_some_and(|spec| spec.name == OPTION)
}

/// The colour one element names, in the palette's terms: an index, an RGB,
/// or the default colour for `default` and `terminal`. `None` for a value
/// that is no colour psmux reads (tmux's `colour_fromstring`,
/// colour.c:387-462, through `style::option_colour_name`).
pub fn element_colour(value: &str) -> Option<vt100::Color> {
    const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    let name = crate::style::option_colour_name(value)?;
    if name == "default" || name == "terminal" {
        return Some(vt100::Color::Default);
    }
    if let Some(hex) = name.strip_prefix('#') {
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
        return Some(vt100::Color::Rgb(byte(0)?, byte(2)?, byte(4)?));
    }
    if let Some(number) = name.strip_prefix("colour") {
        return number.parse::<u8>().ok().map(vt100::Color::Idx);
    }
    if let Some(base) = name.strip_prefix("bright") {
        let i = NAMES.iter().position(|n| *n == base)?;
        return Some(vt100::Color::Idx(8 + i as u8));
    }
    NAMES.iter().position(|n| *n == name).map(|i| vt100::Color::Idx(i as u8))
}

/// Refuse a value that is no colour, as tmux's set does with `bad colour: X`
/// (options.c:1205-1210 for a colour option). A value without an index is
/// checked piece by piece, split as it will be stored.
pub fn validate(name: &str, value: &str) -> Result<(), String> {
    let (_, key) = crate::option_array::parse_name(name)?;
    let bad = |piece: &str| format!("bad colour: {}", piece);
    if key.is_some() {
        return element_colour(value).map(|_| ()).ok_or_else(|| bad(value));
    }
    for piece in value.split([' ', ',']).filter(|piece| !piece.is_empty()) {
        element_colour(piece).ok_or_else(|| bad(piece))?;
    }
    Ok(())
}

/// The array a pane uses: its own, else its window's, else the global one.
pub fn effective(app: &AppState, window_id: usize, pane_id: usize) -> &OptionArray {
    app.pane_pane_colours
        .get(&pane_id)
        .or_else(|| app.window_pane_colours.get(&window_id))
        .unwrap_or(&app.pane_colours)
}

/// The default palette an array gives, `None` when it names no colour for
/// any index from 0 to 255 (colour.c:1293-1322).
pub fn default_palette(array: &OptionArray) -> Option<Box<vt100::DefaultPalette>> {
    let mut palette: Box<vt100::DefaultPalette> = Box::new([None; 256]);
    let mut any = false;
    for (index, value) in array.numbered() {
        let Ok(index) = usize::try_from(index) else { continue };
        if index > 255 {
            continue;
        }
        if let Some(colour) = element_colour(value) {
            palette[index] = Some(colour);
            any = true;
        }
    }
    any.then_some(palette)
}

/// The low sixteen entries as RGB, for the OSC 4 query replies: an index
/// is turned into the xterm colour of that index and the default colour into
/// no entry, as tmux's `colour_force_rgb` does (colour.c:189-200).
fn low_sixteen(palette: Option<&vt100::DefaultPalette>) -> [Option<(u8, u8, u8)>; 16] {
    let mut out = [None; 16];
    if let Some(palette) = palette {
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = match palette[i] {
                Some(vt100::Color::Rgb(r, g, b)) => Some((r, g, b)),
                Some(vt100::Color::Idx(n)) => Some(crate::style::colour_256_to_rgb(n)),
                _ => None,
            };
        }
    }
    out
}

/// The low sixteen of each pane's default palette, for the OSC 4 query
/// replies, which the pane reader threads build (pane.rs `pane_replies`).
type LowSixteen = [Option<(u8, u8, u8)>; 16];

/// One entry per pane: its id, the owner that published it (the
/// `AppState::pane_colours_owner` of the session holding the pane) and the
/// colours. The table is for the whole process, and a shared server keeps
/// several sessions in it, each with its own panes (pane ids are unique
/// across them, server/shared/ids.rs), so a session drops only the entries
/// it published itself.
static REPLY_COLOURS: Mutex<Vec<(usize, u64, LowSixteen)>> = Mutex::new(Vec::new());

static NEXT_OWNER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A new owner for [`REPLY_COLOURS`] entries, one per `AppState`.
pub fn new_owner() -> u64 {
    NEXT_OWNER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Publish `entries` for `pane_id` on behalf of `owner`, replacing what any
/// owner published for that pane (a pane moved to another session is now the
/// new session's).
fn publish(owner: u64, pane_id: usize, entries: [Option<(u8, u8, u8)>; 16]) {
    let Ok(mut table) = REPLY_COLOURS.lock() else { return };
    table.retain(|(id, _, _)| *id != pane_id);
    if entries.iter().any(Option::is_some) {
        table.push((pane_id, owner, entries));
    }
}

/// Drop the entries `owner` published for panes it no longer holds.
fn prune_replies(owner: u64, panes: &std::collections::HashSet<usize>) {
    if let Ok(mut table) = REPLY_COLOURS.lock() {
        table.retain(|(id, by, _)| *by != owner || panes.contains(id));
    }
}

/// The pane-colours colour an OSC 4 query for `index` reports when the pane
/// set no entry of its own (0 to 15).
pub fn reply_colour(pane_id: usize, index: usize) -> Option<(u8, u8, u8)> {
    let table = REPLY_COLOURS.lock().ok()?;
    table.iter().find(|(id, _, _)| *id == pane_id).and_then(|(_, _, e)| *e.get(index)?)
}

/// A pane's own OSC 4 entries with its pane-colours colours under them, for
/// the server side query replies.
pub fn merged_reply_palette(
    pane_id: usize,
    own: Option<[Option<(u8, u8, u8)>; 16]>,
) -> Option<[Option<(u8, u8, u8)>; 16]> {
    let defaults = REPLY_COLOURS
        .lock()
        .ok()
        .and_then(|table| table.iter().find(|(id, _, _)| *id == pane_id).map(|(_, _, e)| *e));
    match (own, defaults) {
        (own, None) => own,
        (None, defaults) => defaults,
        (Some(mut own), Some(defaults)) => {
            for (slot, d) in own.iter_mut().zip(defaults) {
                if slot.is_none() {
                    *slot = d;
                }
            }
            Some(own)
        }
    }
}

/// Note a change to any pane-colours table, so [`sync`] rebuilds every
/// pane's palette.
pub fn changed(app: &mut AppState) {
    app.pane_colours_generation = app.pane_colours_generation.wrapping_add(1).max(1);
}

/// Give every pane the default palette its array names, where the option or
/// the pane's window changed since the last time. Nothing to do on a server
/// where pane-colours was never written, which is every server until someone
/// sets it.
pub fn sync(app: &mut AppState) {
    let generation = app.pane_colours_generation;
    if generation == 0 {
        return;
    }
    type Term = std::sync::Arc<Mutex<vt100::Parser>>;
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut updates: Vec<(usize, usize, Term, Option<Box<vt100::DefaultPalette>>)> = Vec::new();
    for window in &app.windows {
        let window_id = window.id;
        let mut visit = |pane: &crate::types::Pane| {
            seen.insert(pane.id);
            if app.pane_colours_applied.get(&pane.id) == Some(&(generation, window_id)) {
                return;
            }
            let palette = default_palette(effective(app, window_id, pane.id));
            updates.push((pane.id, window_id, pane.term.clone(), palette));
        };
        crate::tree::for_each_pane(&window.root, &mut visit);
        for floating in &window.floating {
            visit(&floating.pane);
        }
    }
    let owner = app.pane_colours_owner;
    for (pane_id, window_id, term, palette) in updates {
        publish(owner, pane_id, low_sixteen(palette.as_deref()));
        if let Ok(mut parser) = term.lock() {
            parser.screen_mut().set_default_palette(palette);
        }
        app.pane_colours_applied.insert(pane_id, (generation, window_id));
    }
    // Panes that are gone take their tables and their reply colours along;
    // the reply colours of another session's panes stay.
    app.pane_colours_applied.retain(|id, _| seen.contains(id));
    app.pane_pane_colours.retain(|id, _| seen.contains(id));
    prune_replies(owner, &seen);
    let windows: std::collections::HashSet<usize> = app.windows.iter().map(|window| window.id).collect();
    app.window_pane_colours.retain(|id, _| windows.contains(id));
}

/// `set-option -w|-p [-u|-a|-o] pane-colours[key] [value]` on one window's
/// or one pane's own array, `id` naming it in `store`. The table has no
/// array until the first write, which starts it empty; `-u` of the whole
/// option removes it so the window or pane inherits again (tmux's
/// `options_remove` on a table that is not a global one), and `-u` of one
/// element removes that element. Returns `""` or `ERROR: ...`.
#[allow(clippy::too_many_arguments)]
fn set_own(
    store: &mut std::collections::HashMap<usize, OptionArray>,
    id: usize,
    name: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let (key, spec) = match (crate::option_array::parse_name(name), crate::option_array::spec(OPTION)) {
        (Ok((_, key)), Some(spec)) => (key, spec),
        (Err(error), _) => return format!("ERROR: {}", error),
        (_, None) => return format!("ERROR: invalid option: {}", name),
    };
    if unset {
        match key {
            Some(key) => {
                if let Some(array) = store.get_mut(&id) {
                    array.set(key, None, false);
                }
            }
            None => {
                store.remove(&id);
            }
        }
        return String::new();
    }
    // -o: the table's array, or with an index that one element
    // (cmd-set-option.c:300-318 at tag 3.8).
    let already = match &key {
        None => store.contains_key(&id),
        Some(key) => store.get(&id).is_some_and(|array| array.get(key).is_some()),
    };
    if only_if_unset && already {
        return if quiet { String::new() } else { format!("ERROR: already set: {}", name) };
    }
    if let Err(error) = validate(name, value) {
        return format!("ERROR: {}", error);
    }
    let append = appends(&key, append);
    store.entry(id).or_default().apply_set(key, value, append, spec);
    String::new()
}

/// Whether `-a` appends. On the whole array it adds elements, as for every
/// array; on one element of a colour array tmux sets the colour given, since
/// only a string element takes text on its end (options.c:600-629 at tag
/// 3.8).
pub fn appends(key: &Option<crate::option_array::ArrayKey>, append: bool) -> bool {
    append && key.is_none()
}

/// [`set_own`] on the window with id `window_id`.
#[allow(clippy::too_many_arguments)]
pub fn set_window(
    app: &mut AppState,
    window_id: usize,
    name: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let reply = set_own(&mut app.window_pane_colours, window_id, name, value, unset, append, only_if_unset, quiet);
    changed(app);
    reply
}

/// [`set_own`] on the pane with id `pane_id`.
#[allow(clippy::too_many_arguments)]
pub fn set_pane(
    app: &mut AppState,
    pane_id: usize,
    name: &str,
    value: &str,
    unset: bool,
    append: bool,
    only_if_unset: bool,
    quiet: bool,
) -> String {
    let reply = set_own(&mut app.pane_pane_colours, pane_id, name, value, unset, append, only_if_unset, quiet);
    changed(app);
    reply
}

/// The table a `show-options` of pane-colours reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShowScope {
    /// `-g`/`-gw`: the global window table.
    Global,
    /// `-w`: one window's own table, by id.
    Window(usize),
    /// `-p`: one pane's own table, by pane and window id.
    Pane { pane_id: usize, window_id: usize },
}

/// `show-options` text for pane-colours in one table (cmd-show-options.c:
/// 156-197): its own array's lines, nothing when the table has none, and
/// with `mark_inherited` (`-A`) the array it inherits instead, each line's
/// name followed by tmux's `*`.
pub fn show(app: &AppState, scope: &ShowScope, name: &str, values_only: bool, mark_inherited: bool) -> String {
    let Ok((_, key)) = crate::option_array::parse_name(name) else { return String::new() };
    let (own, inherited): (Option<&OptionArray>, &OptionArray) = match scope {
        ShowScope::Global => (Some(&app.pane_colours), &app.pane_colours),
        ShowScope::Window(window_id) => (app.window_pane_colours.get(window_id), &app.pane_colours),
        ShowScope::Pane { pane_id, window_id } => (
            app.pane_pane_colours.get(pane_id),
            app.window_pane_colours.get(window_id).unwrap_or(&app.pane_colours),
        ),
    };
    let lines = |array: &OptionArray| match &key {
        None => array.show_lines(OPTION, values_only),
        Some(key) => match array.get(key) {
            Some(value) if values_only => format!("{}\n", value),
            Some(value) => format!("{}[{}] {}\n", OPTION, key, crate::util::quote_arg_if_needed(value)),
            None if values_only => "\n".to_string(),
            None => format!("{}[{}] ''\n", OPTION, key),
        },
    };
    match own {
        Some(array) => lines(array),
        None if mark_inherited => {
            let text = lines(inherited);
            if values_only {
                return text;
            }
            text.split_inclusive('\n')
                .map(|line| match line.find([' ', '\n']) {
                    Some(at) => format!("{}*{}", &line[..at], &line[at..]),
                    None => format!("{}*", line),
                })
                .collect()
        }
        None => String::new(),
    }
}

/// The table a `show-options` names, with its raw `-t` target, as the
/// connection threads send it (`CtrlReq::ShowPaneColours`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShowTarget {
    Global,
    Window(String),
    Pane(String),
}

/// Answer a `CtrlReq::ShowPaneColours`: resolve the target and print the
/// table's lines with [`show`]. A target that names nothing prints nothing.
pub fn show_request(app: &mut AppState, target: &ShowTarget, name: &str, values_only: bool, mark_inherited: bool) -> String {
    let scope = match target {
        ShowTarget::Global => ShowScope::Global,
        ShowTarget::Window(raw) => {
            let Ok(index) = crate::server::options::resolve_option_target_window(app, raw) else {
                return String::new();
            };
            let Some(window) = app.windows.get(index) else { return String::new() };
            ShowScope::Window(window.id)
        }
        ShowTarget::Pane(raw) => {
            let Some(pane_id) = crate::server::options::resolve_option_target_pane(app, raw).map(|pane| pane.id) else {
                return String::new();
            };
            let Some((index, _)) = crate::tree::find_pane_by_id_global(app, pane_id) else {
                return String::new();
            };
            let Some(window) = app.windows.get(index) else { return String::new() };
            ShowScope::Pane { pane_id, window_id: window.id }
        }
    };
    show(app, &scope, name, values_only, mark_inherited)
}

#[cfg(test)]
#[path = "../tests-rs/test_pane_colours.rs"]
mod tests_pane_colours;
