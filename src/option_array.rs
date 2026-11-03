//! Array options, as tmux keeps them in options.c. An option whose table
//! entry carries `OPTIONS_TABLE_IS_ARRAY` holds elements, each keyed by a
//! number or, since tmux 3.8 (20ef19b4), by a name, and every such option goes
//! through the same functions: `name[N]` or `name[KEY]` names one element
//! (`options_parse`), a value without an index replaces the whole array
//! (`options_array_assign`), `-a` and `-u` work on one element or on all of
//! them (cmd-set-option.c), and `show-options` prints a `name[N] value` line
//! per element (cmd-show-options.c). Line numbers below are tag 3.7c unless
//! they say 3.8.
//!
//! [`ARRAY_OPTIONS`] lists the options built on this so far. tmux 3.7c has
//! more, and they move here one at a time: user-keys splits on `,`;
//! pane-colours on space and `,`; every hook is an array of commands that
//! does not split at all.

use std::collections::BTreeMap;

/// An array option's name, the characters a value without an index is split
/// on (options-table.c `.separator`; tmux uses `" ,"` when an option names
/// none, options.c:518-519), and its default (`.default_str`), which is split
/// the same way.
///
/// `trim` is psmux's own: each element is kept without the spaces around it.
/// tmux keeps them (options.c:531, `strsep`), so a space after a `,` becomes
/// part of the next element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArraySpec {
    pub name: &'static str,
    pub separator: &'static str,
    pub default: &'static str,
    pub trim: bool,
}

pub static ARRAY_OPTIONS: &[ArraySpec] = &[
    // options-table.c:297-310
    ArraySpec {
        name: crate::command_alias::OPTION,
        separator: ",",
        default: "split-pane=split-window,\
                  splitp=split-window,\
                  server-info=show-messages -JT,\
                  info=show-messages -JT,\
                  choose-window=choose-tree -w,\
                  choose-session=choose-tree -s",
        trim: false,
    },
    // options-table.c:337-344 at tag 3.8. The spaces are trimmed, as psmux
    // did before this was an array, so `U+2500=2, U+25CF=2` keeps both
    // entries; in tmux the second one, ` U+25CF=2`, is no codepoint and is
    // skipped (utf8.c, utf8_add_to_width_cache).
    ArraySpec {
        name: "codepoint-widths",
        separator: ",",
        default: "",
        trim: true,
    },
    // options-table.c:1030-1040 at tag 3.8. It names no separator, so a
    // value without an index is split on space and `,` (options.c:640-641).
    // The default is psmux's own and empty: tmux's is three formats
    // (options-table.c:238-299, the window list line and the `P:` and `S:`
    // lines for `status 2` and `3`), while psmux draws its own status-left,
    // window list and status-right whenever status-format[0] is empty, and
    // has no pane or session list line to show on the others.
    ArraySpec {
        name: "status-format",
        separator: " ,",
        default: "",
        trim: false,
    },
    // options-table.c:553-563 at tag 3.8, a server option split on `,`.
    // psmux keeps it as tmux does and reads nothing from it: the client's
    // features come from the attaching client's own report (#763).
    ArraySpec {
        name: "terminal-features",
        separator: ",",
        default: "xterm*:clipboard:ccolour:cstyle:focus:title,\
                  screen*:title,\
                  rxvt*:ignorefkeys",
        trim: false,
    },
    // options-table.c:518-525
    ArraySpec {
        name: "terminal-overrides",
        separator: ",",
        default: "linux*:AX@",
        trim: false,
    },
    // options-table.c:1210-1220 at tag 3.8, a session option. It names no
    // separator, so a value without an index is split on space and `,`.
    // The default is tmux 3.8's list without MSYSTEM, for a Windows reason:
    // psmux clients attach from mixed shells (PowerShell, Git Bash, MSYS2),
    // so copying it either removed MINGW64 from a usrin or MSYS2 bash
    // pane (no mingw64/bin on its PATH) or leaked MINGW64 into PowerShell
    // panes. `set -ga update-environment MSYSTEM` restores tmux's list.
    ArraySpec {
        name: "update-environment",
        separator: " ,",
        default: "DISPLAY KRB5CCNAME SSH_ASKPASS SSH_AUTH_SOCK \
                  SSH_AGENT_PID SSH_CONNECTION WAYLAND_DISPLAY \
                  WINDOWID XAUTHORITY XDG_CURRENT_DESKTOP \
                  XDG_SESSION_DESKTOP XDG_SESSION_TYPE",
        trim: false,
    },
];

/// The array option `name` is, judged by its base name.
pub fn spec(name: &str) -> Option<&'static ArraySpec> {
    let (base, _) = parse_name(name).ok()?;
    ARRAY_OPTIONS.iter().find(|spec| spec.name == base)
}

/// The array option `name` names, as the bare name or as `name[...]` with
/// anything between the brackets, so `set` can answer a bad index with
/// `invalid option` from [`parse_name`] instead of keeping it as text.
pub fn is_array_name(name: &str) -> Option<&'static ArraySpec> {
    ARRAY_OPTIONS.iter().find(|spec| {
        name == spec.name
            || name
                .strip_prefix(spec.name)
                .is_some_and(|rest| rest.starts_with('[') && rest.ends_with(']'))
    })
}

/// The key of one array element: a number, or a name (tmux 3.8, 20ef19b4).
///
/// The order is tmux's `options_array_cmp` (options.c:78-98 at tag 3.8):
/// every numbered element comes first, by number, then the named ones,
/// compared byte by byte as `strcmp` compares them. The derived order gives
/// exactly that, `Index` being declared first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArrayKey {
    Index(u32),
    Name(String),
}

impl std::fmt::Display for ArrayKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArrayKey::Index(index) => write!(f, "{}", index),
            ArrayKey::Name(name) => f.write_str(name),
        }
    }
}

impl ArrayKey {
    /// A key as tmux 3.8 corrects it (`options_array_correct_key`,
    /// options.c:41-76): digits only are a number from 0 to `u32::MAX`, kept
    /// as that number (`007` is 7); anything else is a name, kept as it is.
    /// An empty key, or digits too large for the number, is no key.
    pub fn parse(key: &str) -> Option<ArrayKey> {
        if key.is_empty() {
            return None;
        }
        if key.bytes().all(|b| b.is_ascii_digit()) {
            return key.parse::<u32>().ok().map(ArrayKey::Index);
        }
        Some(ArrayKey::Name(key.to_string()))
    }
}

/// `name` split into its base name and its key, `None` for no key
/// (options.c:759-784 at tag 3.8, `options_parse`). The key runs from the
/// first `[` to the first `]` after it, which must end the name; a key that
/// [`ArrayKey::parse`] refuses, or text after the `]`, is no option at all.
pub fn parse_name(name: &str) -> Result<(&str, Option<ArrayKey>), String> {
    let invalid = || format!("invalid option: {}", name);
    if name.is_empty() {
        return Err(invalid());
    }
    let Some(open) = name.find('[') else {
        return Ok((name, None));
    };
    let rest = &name[open + 1..];
    let close = rest.find(']').ok_or_else(invalid)?;
    if close + 1 != rest.len() {
        return Err(invalid());
    }
    let key = ArrayKey::parse(&rest[..close]).ok_or_else(invalid)?;
    Ok((&name[..open], Some(key)))
}

/// The elements of one array option, by key. Values are strings
/// (`OPTIONS_TABLE_STRING`); hooks and pane-colours widen this when they move
/// here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionArray {
    items: BTreeMap<ArrayKey, String>,
}

impl OptionArray {
    /// The array as the option starts out (options.c:258-275,
    /// `options_default`): its default assigned like any value.
    pub fn with_default(spec: &ArraySpec) -> Self {
        let mut array = Self::default();
        array.assign(spec.default, spec);
        array
    }

    pub fn get(&self, key: &ArrayKey) -> Option<&str> {
        self.items.get(key).map(String::as_str)
    }

    /// The element numbered `index`, as tmux reads an array line by line
    /// (`options_array_getv(o, "%u", i)`, status.c:277 at tag 3.8 for
    /// status-format).
    pub fn get_index(&self, index: u32) -> Option<&str> {
        self.get(&ArrayKey::Index(index))
    }

    /// Every element in key order: the numbered ones, then the named ones.
    pub fn iter(&self) -> impl Iterator<Item = (&ArrayKey, &str)> {
        self.items.iter().map(|(key, value)| (key, value.as_str()))
    }

    /// Every element's value in key order, as tmux walks an array with
    /// options_array_first and options_array_next (environ.c:197-213 at tag
    /// 3.8 for update-environment). Owned, so a caller can let go of the
    /// option before it changes what the values are applied to.
    pub fn values(&self) -> Vec<String> {
        self.items.values().cloned().collect()
    }

    /// The numbered elements alone, in order.
    pub fn numbered(&self) -> impl Iterator<Item = (u32, &str)> {
        self.items.iter().filter_map(|(key, value)| match key {
            ArrayKey::Index(index) => Some((*index, value.as_str())),
            ArrayKey::Name(_) => None,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// options.c:409-419, `options_array_clear`.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// One element (options.c:434-509, `options_array_set`, the string case).
    /// `None` removes it; `append` adds `value` to the end of its text.
    /// `value` is stored as given; [`OptionArray::apply_set`] trims it first
    /// for an option whose spec says so.
    pub fn set(&mut self, key: ArrayKey, value: Option<&str>, append: bool) {
        match value {
            None => {
                self.items.remove(&key);
            }
            Some(value) if append => self.items.entry(key).or_default().push_str(value),
            Some(value) => {
                self.items.insert(key, value.to_string());
            }
        }
    }

    /// The lowest number with no element, where `options_array_assign` puts
    /// each new one (options.c:638-683 at tag 3.8 tries `%u` from 0 up, so a
    /// named element takes no number).
    fn first_free(&self) -> u32 {
        let mut index = 0;
        for (used, _) in self.numbered() {
            if used != index {
                break;
            }
            index += 1;
        }
        index
    }

    /// A value given without an index (options.c:511-550,
    /// `options_array_assign`). With no separator the value is one element;
    /// otherwise it is split on any of the separator characters, empty pieces
    /// are skipped, and each piece takes the lowest free index. With
    /// `spec.trim` each piece loses its surrounding spaces first, so a piece
    /// of spaces only is skipped too.
    pub fn assign(&mut self, value: &str, spec: &ArraySpec) {
        let value = if spec.trim { value.trim() } else { value };
        if value.is_empty() {
            return;
        }
        if spec.separator.is_empty() {
            let index = self.first_free();
            self.set(ArrayKey::Index(index), Some(value), false);
            return;
        }
        for piece in value.split(|c| spec.separator.contains(c)) {
            let piece = if spec.trim { piece.trim() } else { piece };
            if piece.is_empty() {
                continue;
            }
            let index = self.first_free();
            self.set(ArrayKey::Index(index), Some(piece), false);
        }
    }

    /// `set-option NAME VALUE` (cmd-set-option.c:194-225): without a key the
    /// array is cleared, unless `-a`, and the value assigned; with one, that
    /// element is set, its value trimmed when `spec.trim` says so.
    pub fn apply_set(&mut self, key: Option<ArrayKey>, value: &str, append: bool, spec: &ArraySpec) {
        match key {
            None => {
                if !append {
                    self.clear();
                }
                self.assign(value, spec);
            }
            Some(key) => {
                let value = if spec.trim { value.trim() } else { value };
                self.set(key, Some(value), append);
            }
        }
    }

    /// `set-option -u` (options.c:1275, `options_remove_or_default`): one
    /// element, the others keeping their keys, or, without a key, the whole
    /// array back to its default, as for a global option.
    pub fn apply_unset(&mut self, key: Option<ArrayKey>, spec: &ArraySpec) {
        match key {
            Some(key) => self.set(key, None, false),
            None => *self = Self::with_default(spec),
        }
    }

    /// The whole array as one string, the elements joined with a space
    /// (options.c:590-614, `options_to_string` with no index).
    pub fn joined(&self) -> String {
        self.items.values().map(String::as_str).collect::<Vec<_>>().join(" ")
    }

    /// `show-options` text for the whole array (cmd-show-options.c:156-192,
    /// `cmd_show_options_print`): a `name[N] value` line per element, the name
    /// alone when there is none, and only the values with `-v`.
    pub fn show_lines(&self, name: &str, values_only: bool) -> String {
        if self.items.is_empty() {
            return if values_only { String::new() } else { format!("{}\n", name) };
        }
        self.items
            .iter()
            .map(|(key, value)| {
                if values_only {
                    format!("{}\n", value)
                } else {
                    format!("{}[{}] {}\n", name, key, crate::util::quote_arg_if_needed(value))
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_option_array.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests-rs/test_array_named_keys.rs"]
mod tests_named_keys;
