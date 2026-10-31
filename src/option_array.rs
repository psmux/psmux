//! Array options, as tmux keeps them in options.c. An option whose table
//! entry carries `OPTIONS_TABLE_IS_ARRAY` holds numbered elements, and every
//! such option goes through the same functions: `name[N]` names one element
//! (`options_parse`), a value without an index replaces the whole array
//! (`options_array_assign`), `-a` and `-u` work on one element or on all of
//! them (cmd-set-option.c), and `show-options` prints a `name[N] value` line
//! per element (cmd-show-options.c). Line numbers below are tag 3.7c.
//!
//! [`ARRAY_OPTIONS`] lists the options built on this so far. tmux 3.7c has
//! more, and they move here one at a time: terminal-features and user-keys
//! split on `,`; status-format,
//! update-environment and pane-colours on space and `,`; every hook is an
//! array of commands that does not split at all.

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
    // options-table.c:518-525
    ArraySpec {
        name: "terminal-overrides",
        separator: ",",
        default: "linux*:AX@",
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

/// `name` split into its base name and its index, `None` for no index
/// (options.c:624-647, `options_parse`). An index must end the name, be
/// digits and fit; anything else is no option at all.
pub fn parse_name(name: &str) -> Result<(&str, Option<u32>), String> {
    let invalid = || format!("invalid option: {}", name);
    if name.is_empty() {
        return Err(invalid());
    }
    let Some(open) = name.find('[') else {
        return Ok((name, None));
    };
    let index = name[open + 1..].strip_suffix(']').ok_or_else(invalid)?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let index = index.parse::<u32>().map_err(|_| invalid())?;
    Ok((&name[..open], Some(index)))
}

/// The elements of one array option, by index. Values are strings
/// (`OPTIONS_TABLE_STRING`); hooks and pane-colours widen this when they move
/// here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptionArray {
    items: BTreeMap<u32, String>,
}

impl OptionArray {
    /// The array as the option starts out (options.c:258-275,
    /// `options_default`): its default assigned like any value.
    pub fn with_default(spec: &ArraySpec) -> Self {
        let mut array = Self::default();
        array.assign(spec.default, spec);
        array
    }

    pub fn get(&self, index: u32) -> Option<&str> {
        self.items.get(&index).map(String::as_str)
    }

    /// The elements in index order.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
        self.items.iter().map(|(index, value)| (*index, value.as_str()))
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
    pub fn set(&mut self, index: u32, value: Option<&str>, append: bool) {
        match value {
            None => {
                self.items.remove(&index);
            }
            Some(value) if append => self.items.entry(index).or_default().push_str(value),
            Some(value) => {
                self.items.insert(index, value.to_string());
            }
        }
    }

    /// The lowest index with no element, where `options_array_assign` puts
    /// each new one.
    fn first_free(&self) -> u32 {
        let mut index = 0;
        for used in self.items.keys() {
            if *used != index {
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
            self.set(index, Some(value), false);
            return;
        }
        for piece in value.split(|c| spec.separator.contains(c)) {
            let piece = if spec.trim { piece.trim() } else { piece };
            if piece.is_empty() {
                continue;
            }
            let index = self.first_free();
            self.set(index, Some(piece), false);
        }
    }

    /// `set-option NAME VALUE` (cmd-set-option.c:194-225): without an index
    /// the array is cleared, unless `-a`, and the value assigned; with one,
    /// that element is set, its value trimmed when `spec.trim` says so.
    pub fn apply_set(&mut self, index: Option<u32>, value: &str, append: bool, spec: &ArraySpec) {
        match index {
            None => {
                if !append {
                    self.clear();
                }
                self.assign(value, spec);
            }
            Some(index) => {
                let value = if spec.trim { value.trim() } else { value };
                self.set(index, Some(value), append);
            }
        }
    }

    /// `set-option -u` (options.c:1275, `options_remove_or_default`): one
    /// element, the others keeping their indexes, or, without an index, the
    /// whole array back to its default, as for a global option.
    pub fn apply_unset(&mut self, index: Option<u32>, spec: &ArraySpec) {
        match index {
            Some(index) => self.set(index, None, false),
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
            .map(|(index, value)| {
                if values_only {
                    format!("{}\n", value)
                } else {
                    format!("{}[{}] {}\n", name, index, crate::util::quote_arg_if_needed(value))
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests-rs/test_option_array.rs"]
mod tests;
