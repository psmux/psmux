//! `command-alias`, tmux's server scope ARRAY option (options-table.c:
//! `OPTIONS_TABLE_IS_ARRAY`). Every element is `name=command`, and a command
//! whose name matches an element runs that command instead (cmd.c
//! `cmd_get_alias`, cmd-parse.y `cmd_parse_expand_alias`).
//!
//! The array itself is [`crate::option_array`], tmux's options.c array
//! handling, so `set`, `-a`, `-u` and `show` behave as for any array option:
//! `set -s command-alias[100] ...` from a tmux config lands at 100, a value
//! without an index replaces the whole array (split on `,`), and `-a` adds at
//! the lowest free index.
//!
//! The server keeps one [`SharedAliases`] that its connection threads read, so
//! a write is seen by every route at once.

use std::sync::{Arc, RwLock};

use crate::option_array::{self, OptionArray};

pub const OPTION: &str = "command-alias";

/// The aliases the server's main loop writes and its connection threads read.
pub type SharedAliases = Arc<RwLock<CommandAliases>>;

/// Prefixed to a command line that came out of an alias, so the route that
/// queues it does not expand it a second time (tmux `CMD_PARSE_NOALIAS`).
pub const EXPANDED_MARK: char = '\u{2}';

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandAliases {
    array: OptionArray,
}

/// True for `command-alias` and `command-alias[N]`, also when N is no index
/// at all, so `set` can answer that with `invalid option`.
pub fn is_alias_option(name: &str) -> bool {
    name == OPTION
        || name
            .strip_prefix(OPTION)
            .is_some_and(|rest| rest.starts_with('[') && rest.ends_with(']'))
}

/// The index `name` gives, `None` for the bare name.
fn option_index(name: &str) -> Result<Option<u32>, String> {
    let (_, index) = option_array::parse_name(name)?;
    Ok(index)
}

/// command-alias's entry in [`option_array::ARRAY_OPTIONS`].
fn spec() -> &'static option_array::ArraySpec {
    option_array::spec(OPTION).expect("command-alias is in ARRAY_OPTIONS")
}

impl CommandAliases {
    /// The aliases a server starts with, tmux's default array:
    /// split-pane, splitp, server-info, info, choose-window, choose-session.
    pub fn with_defaults() -> Self {
        Self { array: OptionArray::with_default(spec()) }
    }

    /// The command `name` stands for. The lowest index wins when two elements
    /// define the same name, as tmux's `cmd_get_alias` walks the array.
    pub fn get(&self, name: &str) -> Option<String> {
        self.array.iter().find_map(|(_, entry)| {
            let (alias, command) = entry.split_once('=')?;
            (alias == name).then(|| command.to_string())
        })
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.array.is_empty()
    }

    /// The elements in index order.
    pub fn items(&self) -> impl Iterator<Item = (u32, &str)> {
        self.array.iter()
    }

    /// `set-option` on `command-alias` or `command-alias[N]`, as for any
    /// array option ([`OptionArray::apply_set`]).
    pub fn set(&mut self, name: &str, value: &str, append: bool) -> Result<(), String> {
        let index = option_index(name)?;
        self.array.apply_set(index, value, append, spec());
        Ok(())
    }

    /// `set-option -u`: one element for `command-alias[N]`; the bare name
    /// puts tmux's default aliases back.
    pub fn unset(&mut self, name: &str) -> Result<(), String> {
        let index = option_index(name)?;
        self.array.apply_unset(index, spec());
        Ok(())
    }

    /// The value of `name` as a string: one element, or the whole array
    /// joined with spaces, as tmux's `#{command-alias}` gives it.
    pub fn value(&self, name: &str) -> String {
        match option_index(name) {
            Ok(Some(index)) => self.array.get(index).unwrap_or_default().to_string(),
            Ok(None) => self.array.joined(),
            Err(_) => String::new(),
        }
    }

    /// `show-options` text for the whole array.
    pub fn show_lines(&self, values_only: bool) -> String {
        self.array.show_lines(OPTION, values_only)
    }

    /// The commands `words` runs when its first word is an alias, as tmux's
    /// `cmd_parse_expand_alias` builds them: the alias's command is parsed like
    /// any command string (quotes, `;` between commands) and the words after
    /// the alias name go on the end of its last command. `None` when the first
    /// word is no alias. An alias with an empty command gives no commands.
    pub fn expand(&self, words: &[String]) -> Option<Vec<Vec<String>>> {
        let command = self.get(words.first()?)?;
        let mut commands: Vec<Vec<String>> = crate::config::split_chained_commands_pub(&command)
            .iter()
            .map(|line| crate::commands::parse_command_line(line))
            .filter(|words| !words.is_empty())
            .collect();
        if let Some(last) = commands.last_mut() {
            last.extend(words[1..].iter().cloned());
        }
        Some(commands)
    }

    /// [`CommandAliases::expand`] as command lines that parse back to the
    /// same words.
    pub fn expand_lines(&self, words: &[String]) -> Option<Vec<String>> {
        self.expand(words).map(|commands| commands.iter().map(|words| command_line(words)).collect())
    }
}

/// One command's words as a line that parses back to the same words.
pub fn command_line(words: &[String]) -> String {
    words
        .iter()
        .map(|word| crate::util::quote_arg_if_needed(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A command an alias produced, queued behind the one that runs now, marked
/// with [`EXPANDED_MARK`].
pub fn queued_line(words: &[String]) -> String {
    format!("{}{}", EXPANDED_MARK, command_line(words))
}

/// Read access that survives a poisoned lock: aliases are plain data, and a
/// panic elsewhere must not switch them off.
pub fn read(shared: &SharedAliases) -> std::sync::RwLockReadGuard<'_, CommandAliases> {
    shared.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn write(shared: &SharedAliases) -> std::sync::RwLockWriteGuard<'_, CommandAliases> {
    shared.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The alias table from `show-options -s command-alias` output, for the CLI,
/// which has no table of its own and has to ask the server for one.
pub fn from_show_output(text: &str) -> CommandAliases {
    let mut aliases = CommandAliases::default();
    for line in text.lines() {
        let words = crate::commands::parse_command_line(line);
        if let [name, value] = words.as_slice() {
            if name != OPTION && is_alias_option(name) {
                let _ = aliases.set(name, value, false);
            }
        }
    }
    aliases
}

#[cfg(test)]
#[path = "../tests-rs/test_command_alias.rs"]
mod tests;
