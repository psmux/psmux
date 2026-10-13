//! tmux mouse key names as bindable keys.
//!
//! tmux turns every mouse event into a KEY (server-client.c
//! server_client_check_mouse) named from an event, an optional button and the
//! location it happened on: `MouseDown1Pane`, `MouseDrag1Border`,
//! `WheelUpPane`, `WheelDownStatus`, `DoubleClick1Pane`, with `C-`, `M-` and
//! `S-` modifiers.  Those keys live in the ordinary key tables, so
//! `bind -n WheelUpPane ...`, `unbind -n WheelUpPane` and `list-keys` treat
//! them like any other key.
//!
//! psmux keys are `(KeyCode, KeyModifiers)` pairs.  A mouse key is stored as a
//! `KeyCode::Char` in the supplementary private use plane (U+F0000 up), which
//! no keyboard or IME produces as a single key event, so every existing table
//! operation (bind, unbind, list-keys, notes, the client sync) carries mouse
//! keys without a separate store.  `parse` / `name` convert to and from the
//! tmux spelling.

use crossterm::event::{KeyCode, KeyModifiers};

const BASE: u32 = 0xF0000;

/// Event names, in tmux spelling.  The first seven take a button number.
const EVENTS: &[&str] = &[
    "MouseDown", "MouseUp", "MouseDrag", "MouseDragEnd",
    "SecondClick", "DoubleClick", "TripleClick",
    "WheelUp", "WheelDown", "MouseMove",
];
const BUTTON_EVENTS: usize = 7;
const MAX_BUTTON: u32 = 11;

/// Location suffixes, in tmux spelling (KEYC_MOUSE_STRING in tmux.h).
const LOCATIONS: &[&str] = &[
    "Pane", "Status", "StatusLeft", "StatusRight", "StatusDefault",
    "ScrollbarUp", "ScrollbarSlider", "ScrollbarDown", "Border", "Empty",
];

pub const EV_MOUSE_DOWN: usize = 0;
pub const EV_MOUSE_DRAG: usize = 2;
pub const EV_MOUSE_DRAG_END: usize = 3;
pub const EV_SECOND_CLICK: usize = 4;
/// tmux KEYC_CLICK_TIMEOUT (tmux.h): the double click window, a constant in
/// tmux (no option, measured: `show -g double-click-time` is an invalid option
/// in tmux 3.4).
pub const CLICK_TIMEOUT_MS: u64 = 300;
pub const EV_DOUBLE_CLICK: usize = 5;
pub const EV_TRIPLE_CLICK: usize = 6;
pub const EV_WHEEL_UP: usize = 7;
pub const EV_WHEEL_DOWN: usize = 8;
pub const LOC_PANE: usize = 0;
pub const LOC_STATUS: usize = 1;
pub const LOC_BORDER: usize = 8;

fn encode(event: usize, button: u32, location: usize) -> char {
    let code = ((event as u32 * 16 + button) * 16) + location as u32;
    char::from_u32(BASE + code).expect("mouse key code is a valid scalar")
}

/// True when `c` is the stored form of a mouse key.
pub fn is_mouse_char(c: char) -> bool {
    let v = c as u32;
    v >= BASE && v < BASE + (EVENTS.len() as u32) * 256
}

/// True when `key` is a mouse key.
pub fn is_mouse_key(key: &(KeyCode, KeyModifiers)) -> bool {
    matches!(key.0, KeyCode::Char(c) if is_mouse_char(c))
}

/// The tmux name of a stored mouse key character, without modifiers.
pub fn name(c: char) -> Option<String> {
    if !is_mouse_char(c) {
        return None;
    }
    let code = c as u32 - BASE;
    let location = (code % 16) as usize;
    let button = (code / 16) % 16;
    let event = (code / 256) as usize;
    let ev = EVENTS.get(event)?;
    let loc = LOCATIONS.get(location)?;
    if event < BUTTON_EVENTS {
        Some(format!("{}{}{}", ev, button, loc))
    } else {
        Some(format!("{}{}", ev, loc))
    }
}

/// Parse a tmux mouse key name (case insensitive, like tmux's strcasecmp
/// lookup), with any `C-` / `M-` / `S-` prefixes.  `None` for anything that
/// is not a mouse key.
pub fn parse(name: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut rest = name.trim();
    let mut mods = KeyModifiers::NONE;
    loop {
        let head: String = rest.chars().take(2).collect::<String>().to_ascii_lowercase();
        match head.as_str() {
            "c-" => mods |= KeyModifiers::CONTROL,
            "m-" => mods |= KeyModifiers::ALT,
            "s-" => mods |= KeyModifiers::SHIFT,
            _ => break,
        }
        rest = &rest[2..];
    }
    let lower = rest.to_ascii_lowercase();
    // Longest event name first, so MouseDragEnd is not read as MouseDrag.
    let mut events: Vec<(usize, &str)> = EVENTS.iter().copied().enumerate().collect();
    events.sort_by_key(|(_, e)| std::cmp::Reverse(e.len()));
    for (ei, ev) in events {
        let Some(after) = lower.strip_prefix(&ev.to_ascii_lowercase()) else { continue };
        let (button, loc_str) = if ei < BUTTON_EVENTS {
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                continue;
            }
            let Ok(b) = digits.parse::<u32>() else { continue };
            if b == 0 || b > MAX_BUTTON {
                continue;
            }
            (b, &after[digits.len()..])
        } else {
            (0, after)
        };
        if let Some(li) = LOCATIONS.iter().position(|l| l.eq_ignore_ascii_case(loc_str)) {
            return Some((KeyCode::Char(encode(ei, button, li)), mods));
        }
    }
    None
}

/// The tmux mouse modifier bits (input-keys.c / tmux.h MOUSE_MASK_*) of
/// `mods`: shift 4, meta 8, ctrl 16.  Added to an SGR button number.
pub fn sgr_mod_bits(mods: KeyModifiers) -> u8 {
    let mut b = 0u8;
    if mods.contains(KeyModifiers::SHIFT) { b |= 4; }
    if mods.contains(KeyModifiers::ALT) { b |= 8; }
    if mods.contains(KeyModifiers::CONTROL) { b |= 16; }
    b
}

/// The inverse of `sgr_mod_bits`, for the modifier mask a client sends.
pub fn mods_from_mask(mask: u8) -> KeyModifiers {
    let mut m = KeyModifiers::NONE;
    if mask & 4 != 0 { m |= KeyModifiers::SHIFT; }
    if mask & 8 != 0 { m |= KeyModifiers::ALT; }
    if mask & 16 != 0 { m |= KeyModifiers::CONTROL; }
    m
}

/// The key a wheel notch over a pane produces: `WheelUpPane` /
/// `WheelDownPane` with the held modifiers.
pub fn wheel_pane_key(up: bool, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    let ev = if up { EV_WHEEL_UP } else { EV_WHEEL_DOWN };
    (KeyCode::Char(encode(ev, 0, LOC_PANE)), mods)
}

/// A mouse key from its parts: event (EV_*), button (1 based, 0 for the wheel
/// and motion events) and location (LOC_*).
pub fn key(event: usize, button: u32, location: usize, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    (KeyCode::Char(encode(event, button, location)), mods)
}

/// `MouseDragEnd1Pane`: the end of a left drag over a pane.
pub fn drag_end_pane_key() -> (KeyCode, KeyModifiers) {
    key(EV_MOUSE_DRAG_END, 1, LOC_PANE, KeyModifiers::NONE)
}

/// Whether the end of a drag in copy mode still runs the built-in default
/// (copy-pipe-and-cancel). The client asks so a drag it paints itself can be
/// handed to copy mode when the user unbound or rebound MouseDragEnd1Pane.
pub fn drag_end_is_default(app: &crate::types::AppState) -> bool {
    resolve(app, drag_end_pane_key(), true) == Dispatch::Builtin
}

/// The built-in mouse bindings psmux dispatches itself, listed per table the
/// way `list-keys` prints them.  The command text is tmux's default for the
/// same key (key-bindings.c), with psmux's scroll amount; the behaviour behind
/// it is the hardcoded wheel handler, which is what runs while the binding is
/// in place.  `unbind` removes one, after which the key does what an unbound
/// mouse key does in tmux.
pub const ROOT_MOUSE_DEFAULTS: &[(&str, &str)] = &[
    ("WheelUpPane", "if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -e }"),
    ("WheelUpStatus", "previous-window"),
    ("WheelDownStatus", "next-window"),
    ("MouseDown1Status", "select-window -t ="),
    ("MouseDrag1Border", "resize-pane -M"),
    // psmux's right click: the program gets it when it wants the mouse,
    // otherwise copy the client selection or paste the clipboard (tmux's
    // default opens a menu).
    ("MouseDown3Pane", "if-shell -F -t = \"#{mouse_any_flag}\" { select-pane -t = ; send-keys -M } { paste-buffer }"),
    ("MouseDown1Pane", "select-pane -t = \\; send-keys -M"),
    ("MouseDrag1Pane", "if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -M }"),
    ("DoubleClick1Pane", "select-pane -t = \\; if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -H ; send-keys -X select-word ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel }"),
    ("TripleClick1Pane", "select-pane -t = \\; if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -H ; send-keys -X select-line ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel }"),
];
pub const COPY_MODE_MOUSE_DEFAULTS: &[(&str, &str)] = &[
    ("WheelUpPane", "select-pane \\; send-keys -X -N 3 scroll-up"),
    ("WheelDownPane", "select-pane \\; send-keys -X -N 3 scroll-down"),
    ("MouseDragEnd1Pane", "send-keys -X copy-pipe-and-cancel"),
    ("MouseDown1Pane", "select-pane"),
    ("MouseDrag1Pane", "select-pane \\; send-keys -X begin-selection"),
    ("DoubleClick1Pane", "select-pane \\; send-keys -X select-word \\; run-shell -d 0.3 \\; send-keys -X copy-pipe-and-cancel"),
    ("TripleClick1Pane", "select-pane \\; send-keys -X select-line \\; run-shell -d 0.3 \\; send-keys -X copy-pipe-and-cancel"),
];

/// tmux's own default commands for keys psmux handles with a built-in
/// behaviour. A ported config often repeats them (`bind -n MouseDrag1Border
/// resize-pane -M`); such a binding means "the default", so it keeps psmux's
/// built-in handling instead of running a command psmux does not model the
/// same way. (table, key, tmux command), compared after `canonical_command`.
const DEFAULT_EQUIVALENTS: &[(&str, &str, &str)] = &[
    ("root", "MouseDrag1Border", "resize-pane -M"),
    ("root", "MouseDown1Status", "switch-client -t ="),
    ("root", "MouseDown1Status", "select-window -t ="),
    ("root", "MouseDown1Pane", "select-pane -t = ; send-keys -M"),
    ("root", "MouseDrag1Pane", "if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -M }"),
    ("root", "WheelUpPane", "if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -e }"),
    ("root", "WheelUpPane", "if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -e }"),
    ("root", "WheelUpStatus", "previous-window"),
    ("root", "WheelDownStatus", "next-window"),
    ("copy-mode", "MouseDragEnd1Pane", "send-keys -X copy-pipe-and-cancel"),
    ("copy-mode-vi", "MouseDragEnd1Pane", "send-keys -X copy-pipe-and-cancel"),
    ("copy-mode", "MouseDown1Pane", "select-pane"),
    ("copy-mode-vi", "MouseDown1Pane", "select-pane"),
    ("copy-mode", "MouseDrag1Pane", "select-pane ; send-keys -X begin-selection"),
    ("copy-mode-vi", "MouseDrag1Pane", "select-pane ; send-keys -X begin-selection"),
    ("root", "DoubleClick1Pane", "select-pane -t = ; if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -H ; send-keys -X select-word ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel }"),
    ("root", "TripleClick1Pane", "select-pane -t = ; if-shell -F \"#{||:#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -H ; send-keys -X select-line ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel }"),
    ("copy-mode", "DoubleClick1Pane", "select-pane ; send-keys -X select-word ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel"),
    ("copy-mode-vi", "DoubleClick1Pane", "select-pane ; send-keys -X select-word ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel"),
    ("copy-mode", "TripleClick1Pane", "select-pane ; send-keys -X select-line ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel"),
    ("copy-mode-vi", "TripleClick1Pane", "select-pane ; send-keys -X select-line ; run-shell -d 0.3 ; send-keys -X copy-pipe-and-cancel"),
];

/// A command line in one spelling for comparing defaults: whitespace runs
/// folded, `send` / `if` / `selectp` spelled out, `-t=` as `-t =`, `\;` as
/// `;`, quotes dropped.
fn canonical_command(cmd: &str) -> String {
    // Quotes dropped: the CLI route passes a format like #{...} unquoted.
    let cmd = cmd.replace(['\'', '"'], "");
    let mut out: Vec<String> = Vec::new();
    for tok in cmd.split_whitespace() {
        // `a;` ends a command like a separate `;` (but `\;` at the end of a
        // word is a literal semicolon).
        match tok.strip_suffix(';') {
            Some(w) if !w.is_empty() && !w.ends_with('\\') => {
                out.extend(canonical_word(w));
                out.push(";".into());
            }
            _ => out.extend(canonical_word(tok)),
        }
    }
    out.join(" ")
}
fn canonical_word(tok: &str) -> Vec<String> {
    match tok {
        "\\;" => vec![";".into()],
        "send" => vec!["send-keys".into()],
        "if" => vec!["if-shell".into()],
        "run" => vec!["run-shell".into()],
        "-d0.3" => vec!["-d".into(), "0.3".into()],
        "selectp" => vec!["select-pane".into()],
        "-t=" => vec!["-t".into(), "=".into()],
        other => vec![other.to_string()],
    }
}

/// Whether a user binding of `key` in `table` is tmux's default command for
/// it (see DEFAULT_EQUIVALENTS).
fn is_default_equivalent(table: &str, key: (KeyCode, KeyModifiers), cmd: &str) -> bool {
    if key.1 != KeyModifiers::NONE {
        return false;
    }
    let canon = canonical_command(cmd);
    DEFAULT_EQUIVALENTS.iter().any(|(t, k, c)| {
        *t == table && parse(k).map_or(false, |pk| pk == key) && canonical_command(c) == canon
    })
}

/// Mouse keys whose built-in behaviour lives in the client (it hit-tests the
/// status line, borders and panes itself). The server tells the client which
/// of them no longer have their built-in default (`client_overrides`), and
/// the client sends those to the server as `mouse-key` instead of acting.
pub const CLIENT_ROUTED: &[&str] = &[
    "MouseDown1Status",
    "MouseDrag1Border",
    "MouseDown3Pane",
    "MouseDown1Pane",
    "MouseDrag1Pane",
];

/// Status line multi clicks: tmux has no default for them, so the client
/// only hears about one that is BOUND (an unbound one does nothing there:
/// there is no pane under the pointer to hand it to).
pub const CLIENT_ROUTED_STATUS_CLICKS: &[&str] = &[
    "SecondClick1Status",
    "DoubleClick1Status",
    "TripleClick1Status",
];

/// One letter per dispatch, for the client: B built-in, U user binding,
/// F unbound in root (to the program if it wants the mouse), D dropped.
fn dispatch_code(d: &Dispatch) -> char {
    match d {
        Dispatch::Builtin => 'B',
        Dispatch::User(_) => 'U',
        Dispatch::Forward => 'F',
        Dispatch::Drop => 'D',
    }
}

/// The client routed keys that do not have their built-in default, as a JSON
/// object `{"Name": "<root><copy>"}` (two dispatch letters: outside and inside
/// copy mode). Empty with no mouse key rebound or unbound, the common case.
pub fn client_overrides(app: &crate::types::AppState) -> String {
    let mut parts: Vec<String> = Vec::new();
    for name in CLIENT_ROUTED {
        let Some(k) = parse(name) else { continue };
        let r = dispatch_code(&resolve(app, k, false));
        let c = dispatch_code(&resolve(app, k, true));
        if r != 'B' || c != 'B' {
            parts.push(format!("\"{}\":\"{}{}\"", name, r, c));
        }
    }
    for name in CLIENT_ROUTED_STATUS_CLICKS {
        let Some(k) = parse(name) else { continue };
        if let Dispatch::User(_) = resolve(app, k, false) {
            parts.push(format!("\"{}\":\"UU\"", name));
        }
    }
    // A routed key bound WITH modifiers (`bind -n M-MouseDown3Pane ...`):
    // the client looks those up by their full name before the plain one.
    let mut seen: Vec<(KeyCode, KeyModifiers)> = Vec::new();
    for table in ["root", "copy-mode", "copy-mode-vi"] {
        let Some(binds) = app.key_tables.get(table) else { continue };
        for b in binds {
            let KeyCode::Char(ch) = b.key.0 else { continue };
            if b.key.1.is_empty() || seen.contains(&b.key) {
                continue;
            }
            let Some(base) = name(ch) else { continue };
            if !CLIENT_ROUTED.contains(&base.as_str()) {
                continue;
            }
            seen.push(b.key);
            let r = dispatch_code(&resolve(app, b.key, false));
            let c = dispatch_code(&resolve(app, b.key, true));
            parts.push(format!("\"{}\":\"{}{}\"", crate::config::format_key_binding(&b.key), r, c));
        }
    }
    format!("{{{}}}", parts.join(","))
}

/// What a mouse key does, resolved the way tmux's server_client_key_callback
/// walks its tables: the copy-mode table first while the pane is in copy mode,
/// then root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// A built-in default binding is in effect: run psmux's own handler.
    Builtin,
    /// A user binding: run this command with the mouse as its target.
    User(String),
    /// Not bound in root: hand the event to the pane (tmux forward_key),
    /// which only reaches a program that enabled mouse reporting.
    Forward,
    /// Not bound in the copy-mode table nor in root: tmux drops the key.
    Drop,
}

/// Resolve `key` against `app`'s tables.  `in_copy` is whether the pane the
/// event belongs to is in copy mode.
pub fn resolve(app: &crate::types::AppState, key: (KeyCode, KeyModifiers), in_copy: bool) -> Dispatch {
    let mode_table = if in_copy { Some(crate::config::active_copy_mode_table(app)) } else { None };
    for table in mode_table.into_iter().chain(std::iter::once("root")) {
        if let Some(b) = app.key_tables.get(table).and_then(|v| v.iter().find(|b| b.key == key)) {
            let cmd = crate::commands::format_action(&b.action);
            if is_default_equivalent(table, key, &cmd) {
                return Dispatch::Builtin;
            }
            return Dispatch::User(cmd);
        }
        let builtin = crate::config::copy_mode_default_keys(table).iter().any(|(k, _, _)| *k == key);
        if builtin && !app.copy_mode_defaults_unbound.contains(&(table.to_string(), key)) {
            // Reached root from a copy-mode table: root's default
            // WheelUpPane would send -M, and a pane in copy mode takes no
            // mouse keys that way (copy mode has no mode->key in tmux's
            // window-copy.c, so window_pane_key drops it).
            if table == "root" && in_copy {
                // Reached root from a copy-mode table. tmux's root defaults
                // for these send the event to the pane (`send -M`), which a
                // pane in copy mode does not take; MouseDown1Pane's also
                // selects the pane first.
                if key == wheel_pane_key(true, KeyModifiers::NONE)
                    || parse("MouseDrag1Pane") == Some(key)
                {
                    return Dispatch::Drop;
                }
                if parse("MouseDown1Pane") == Some(key) {
                    return Dispatch::User("select-pane -t =".into());
                }
            }
            return Dispatch::Builtin;
        }
    }
    if mode_table.is_some() { Dispatch::Drop } else { Dispatch::Forward }
}

/// Point a bound command's mouse targets at `pane_id`.  tmux resolves `-t =`
/// (also written `-t=`, or as the tail of a flag cluster like `-Ft=`) and
/// `{mouse}` to the pane the mouse event happened in; psmux commands take a
/// pane id there instead.
pub fn resolve_mouse_targets(cmd: &str, pane_id: usize) -> String {
    resolve_mouse_targets_to(cmd, &format!("%{}", pane_id))
}

/// `resolve_mouse_targets` with any target spelling: a pane `%N` for a pane
/// event, a window `:N` for a status line event.
pub fn resolve_mouse_targets_to(cmd: &str, target: &str) -> String {
    let target = target.to_string();
    let mut out = String::with_capacity(cmd.len() + 8);
    let mut prev_was_t = false;
    // Whitespace separated words, rebuilt as written. A word can carry quote,
    // brace or `;` punctuation from the text around it (`'select-pane -t=;`
    // inside a quoted if-shell branch), so the rule applies to its core and
    // the punctuation is put back.
    let mut rest = cmd;
    while !rest.is_empty() {
        let ws_len = rest.len() - rest.trim_start().len();
        out.push_str(&rest[..ws_len]);
        rest = &rest[ws_len..];
        if rest.is_empty() {
            break;
        }
        let tok_len = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let tok = &rest[..tok_len];
        rest = &rest[tok_len..];
        let is_punct = |c: char| matches!(c, '\'' | '"' | '{' | '}' | ';');
        let lead = tok.len() - tok.trim_start_matches(|c: char| matches!(c, '\'' | '"')).len();
        let core_end = tok.trim_end_matches(is_punct).len().max(lead);
        let (pre, core, post) = (&tok[..lead], &tok[lead..core_end], &tok[core_end..]);
        if tok.contains("{mouse}") {
            out.push_str(&tok.replace("{mouse}", &target));
            prev_was_t = false;
            continue;
        }
        let replaced: Option<String> = if prev_was_t && core == "=" {
            Some(target.clone())
        } else if let Some(flags) = core.strip_prefix('-').and_then(|t| t.strip_suffix("t=")) {
            flags.chars().all(|c| c.is_ascii_alphabetic()).then(|| {
                if flags.is_empty() {
                    format!("-t {}", target)
                } else {
                    format!("-{} -t {}", flags, target)
                }
            })
        } else {
            None
        };
        match replaced {
            Some(r) => {
                out.push_str(pre);
                out.push_str(&r);
                out.push_str(post);
            }
            None => out.push_str(tok),
        }
        prev_was_t = post.is_empty()
            && core.len() > 1
            && core.starts_with('-')
            && core.ends_with('t')
            && core[1..].chars().all(|c| c.is_ascii_alphabetic());
    }
    out
}

#[cfg(test)]
#[path = "../tests-rs/test_mouse_key_bindings.rs"]
mod tests_mouse_key_bindings;
