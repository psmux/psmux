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

pub const EV_WHEEL_UP: usize = 7;
pub const EV_WHEEL_DOWN: usize = 8;
pub const LOC_PANE: usize = 0;

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

/// The built-in mouse bindings psmux dispatches itself, listed per table the
/// way `list-keys` prints them.  The command text is tmux's default for the
/// same key (key-bindings.c), with psmux's scroll amount; the behaviour behind
/// it is the hardcoded wheel handler, which is what runs while the binding is
/// in place.  `unbind` removes one, after which the key does what an unbound
/// mouse key does in tmux.
pub const ROOT_MOUSE_DEFAULTS: &[(&str, &str)] = &[
    ("WheelUpPane", "if-shell -F \"#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}\" { send-keys -M } { copy-mode -e }"),
];
pub const COPY_MODE_MOUSE_DEFAULTS: &[(&str, &str)] = &[
    ("WheelUpPane", "select-pane \\; send-keys -X -N 3 scroll-up"),
    ("WheelDownPane", "select-pane \\; send-keys -X -N 3 scroll-down"),
];

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
            return Dispatch::User(crate::commands::format_action(&b.action));
        }
        let builtin = crate::config::copy_mode_default_keys(table).iter().any(|(k, _, _)| *k == key);
        if builtin && !app.copy_mode_defaults_unbound.contains(&(table.to_string(), key)) {
            // Reached root from a copy-mode table: root's default
            // WheelUpPane would send -M, and a pane in copy mode takes no
            // mouse keys that way (copy mode has no mode->key in tmux's
            // window-copy.c, so window_pane_key drops it).
            if table == "root" && in_copy {
                return Dispatch::Drop;
            }
            return Dispatch::Builtin;
        }
    }
    if mode_table.is_some() { Dispatch::Drop } else { Dispatch::Forward }
}

/// A binding written as a brace block, `bind -n WheelUpPane { cmd; cmd }`, is
/// a command list in tmux; the braces only group it.  The server's line parser
/// takes brace blocks as if-shell arguments but not as a whole line, so a
/// block that wraps the entire binding is unwrapped into a `\;` chain.
/// Anything else is returned unchanged.
pub fn unwrap_command_block(cmd: &str) -> String {
    let t = cmd.trim();
    if !(t.starts_with('{') && t.ends_with('}')) {
        return cmd.to_string();
    }
    // The opening brace must close at the very end (outside quotes), or this
    // is `{ a } { b }` style text that is not one block.
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for (i, c) in t.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 && i + 1 != t.len() {
                        return cmd.to_string();
                    }
                }
                _ => {}
            },
        }
    }
    let inner = t[1..t.len() - 1].trim();
    let subs = crate::config::split_chained_commands_pub(inner);
    subs.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" \\; ")
}

/// Point a bound command's mouse targets at `pane_id`.  tmux resolves `-t =`
/// (also written `-t=`, or as the tail of a flag cluster like `-Ft=`) and
/// `{mouse}` to the pane the mouse event happened in; psmux commands take a
/// pane id there instead.
pub fn resolve_mouse_targets(cmd: &str, pane_id: usize) -> String {
    let target = format!("%{}", pane_id);
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
