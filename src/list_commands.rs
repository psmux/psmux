//! `list-commands [-F format] [command]`, as tmux 3.8 prints it
//! (cmd-list-commands.c).
//!
//! One row per command in tmux's command table order, from the template
//! `#{command_list_name}#{?command_list_alias, (#{command_list_alias}),}
//! #{command_list_usage}` or the -F format. A command argument lists that
//! command alone, found the way tmux's cmd_find finds it (an alias, an exact
//! name or a unique prefix); any other is an error. psmux printed a help
//! style list and ignored -F and the command.

/// tmux 3.8's commands: name, alias (empty for none) and usage, in the
/// order of cmd_table (cmd.c), the usage strings from each cmd_entry with
/// the CMD_*_USAGE macros expanded (tmux.h).
pub static COMMAND_TABLE: &[(&str, &str, &str)] = &[
    ("attach-session", "attach", "[-dErx] [-c working-directory] [-f flags] [-t target-session]"),
    ("bind-key", "bind", "[-nr] [-T key-table] [-N note] key [command [argument ...]]"),
    ("break-pane", "breakp", "[-abdPW] [-F format] [-n window-name] [-s src-pane] [-t dst-window] [-x width] [-y height] [-X x-position] [-Y y-position]"),
    ("capture-pane", "capturep", "[-aCeFHJLMNpPqRT] [-b buffer-name] [-E end-line] [-S start-line] [-t target-pane]"),
    ("choose-buffer", "", "[-kNrZ] [-F format] [-f filter] [-K key-format] [-O sort-order] [-t target-pane] [template]"),
    ("choose-client", "", "[-hikNrZ] [-F format] [-f filter] [-K key-format] [-O sort-order] [-t target-pane] [template]"),
    ("choose-tree", "", "[-GhkNrswZ] [-F format] [-f filter] [-K key-format] [-O sort-order] [-t target-pane] [template]"),
    ("clear-history", "clearhist", "[-H] [-t target-pane]"),
    ("clear-prompt-history", "clearphist", "[-T prompt-type]"),
    ("clock-mode", "", "[-t target-pane]"),
    ("command-prompt", "", "[-1CbeFiklNP] [-I inputs] [-p prompts] [-t target-client] [-T prompt-type] [template]"),
    ("confirm-before", "confirm", "[-by] [-c confirm-key] [-p prompt] [-t target-client] command"),
    ("copy-mode", "", "[-dekHMqSu] [-s src-pane] [-t target-pane]"),
    ("customize-mode", "", "[-kNZ] [-F format] [-f filter] [-t target-pane]"),
    ("delete-buffer", "deleteb", "[-b buffer-name]"),
    ("detach-client", "detach", "[-aP] [-E shell-command] [-s target-session] [-t target-client]"),
    ("display-menu", "menu", "[-MO] [-b border-lines] [-c target-client] [-C starting-choice] [-H selected-style] [-s style] [-S border-style] [-t target-pane] [-T title] [-x position] [-y position] name [key] [command] ..."),
    ("display-message", "display", "[-aCIlNpv] [-c target-client] [-d delay] [-F format] [-t target-pane] [message]"),
    ("display-popup", "popup", "[-BCEkN] [-b border-lines] [-c target-client] [-d start-directory] [-e environment] [-h height] [-s style] [-S border-style] [-t target-pane] [-T title] [-w width] [-x position] [-y position] [shell-command [argument ...]]"),
    ("display-panes", "displayp", "[-kNZ] [-d duration] [-s source-window] [-t target-pane] [template]"),
    ("find-window", "findw", "[-CiNrTZ] [-t target-pane] match-string"),
    ("has-session", "has", "[-t target-session]"),
    ("if-shell", "if", "[-bF] [-t target-pane] shell-command command [command]"),
    ("join-pane", "joinp", "[-bdfhv] [-l size] [-s src-pane] [-t dst-pane]"),
    ("kill-pane", "killp", "[-a] [-f filter] [-t target-pane]"),
    ("kill-server", "", ""),
    ("kill-session", "", "[-aCg] [-f filter] [-t target-session]"),
    ("kill-window", "killw", "[-a] [-f filter] [-t target-window]"),
    ("last-pane", "lastp", "[-deZ] [-t target-window]"),
    ("last-window", "last", "[-t target-session]"),
    ("link-window", "linkw", "[-abdk] [-s src-window] [-t dst-window]"),
    ("list-buffers", "lsb", "[-F format] [-f filter] [-O order]"),
    ("list-clients", "lsc", "[-F format] [-f filter] [-O order][-t target-session]"),
    ("list-commands", "lscm", "[-F format] [command]"),
    ("list-keys", "lsk", "[-1aNr] [-F format] [-O order] [-P prefix-string][-T key-table] [key]"),
    ("list-panes", "lsp", "[-asr] [-F format] [-f filter] [-O order][-t target-window]"),
    ("list-sessions", "ls", "[-r] [-F format] [-f filter] [-O order]"),
    ("list-windows", "lsw", "[-ar] [-F format] [-f filter] [-O order][-t target-session]"),
    ("load-buffer", "loadb", "[-b buffer-name] [-t target-client] path"),
    ("lock-client", "lockc", "[-t target-client]"),
    ("lock-server", "lock", ""),
    ("lock-session", "locks", "[-t target-session]"),
    ("move-pane", "movep", "[-bdfhMv] [-D lines] [-l size] [-L columns] [-P position] [-R columns] [-s src-pane] [-t dst-pane] [-U lines] [-X x-position] [-Y y-position] [-z z-index]"),
    ("move-window", "movew", "[-abdkr] [-s src-window] [-t dst-window]"),
    ("new-pane", "newp", "[-bdefhIklMOPvWZ] [-B border-lines] [-c start-directory] [-e environment] [-F format] [-l size] [-m message] [-p percentage] [-s style] [-S active-border-style] [-R inactive-border-style] [-T title] [-x width] [-y height] [-X x-position] [-Y y-position] [-t target-pane] [shell-command [argument ...]]"),
    ("new-session", "new", "[-AdDEPX] [-c start-directory] [-e environment] [-F format] [-f flags] [-n window-name] [-s session-name] [-t target-session] [-x width] [-y height] [shell-command [argument ...]]"),
    ("new-window", "neww", "[-abdEkPS] [-c start-directory] [-e environment] [-F format] [-n window-name] [-t target-window] [shell-command [argument ...]]"),
    ("next-layout", "nextl", "[-t target-window]"),
    ("next-window", "next", "[-a] [-t target-session]"),
    ("paste-buffer", "pasteb", "[-dprS] [-s separator] [-b buffer-name] [-t target-pane]"),
    ("pipe-pane", "pipep", "[-IOo] [-t target-pane] [shell-command]"),
    ("previous-layout", "prevl", "[-t target-window]"),
    ("previous-window", "prev", "[-a] [-t target-session]"),
    ("refresh-client", "refresh", "[-cDlLRSU] [-A pane:state] [-B name:what:format] [-C XxY] [-f flags] [-r pane:report] [-t target-client] [adjustment]"),
    ("rename-session", "rename", "[-t target-session] new-name"),
    ("rename-window", "renamew", "[-t target-window] new-name"),
    ("resize-pane", "resizep", "[-MTZ] [-D lines] [-L columns] [-R columns] [-U lines] [-x width] [-y height] [-t target-pane]"),
    ("resize-window", "resizew", "[-aADLRU] [-x width] [-y height] [-t target-window] [adjustment]"),
    ("respawn-pane", "respawnp", "[-Ek] [-c start-directory] [-e environment] [-t target-pane] [shell-command [argument ...]]"),
    ("respawn-window", "respawnw", "[-Ek] [-c start-directory] [-e environment] [-t target-window] [shell-command [argument ...]]"),
    ("rotate-window", "rotatew", "[-DUZ] [-t target-window]"),
    ("run-shell", "run", "[-bCE] [-c start-directory] [-d delay] [-t target-pane] [shell-command [argument ...]]"),
    ("save-buffer", "saveb", "[-a] [-b buffer-name] path"),
    ("select-layout", "selectl", "[-Enop] [-t target-pane] [layout-name]"),
    ("select-pane", "selectp", "[-DdeLlMmRUZ] [-T title] [-t target-pane]"),
    ("select-window", "selectw", "[-lnpT] [-t target-window]"),
    ("send-keys", "send", "[-FHKlMRX] [-c target-client] [-N repeat-count] [-t target-pane] [key ...]"),
    ("send-prefix", "", "[-2] [-t target-pane]"),
    ("server-access", "", "[-adglrw] [-t target-pane] [user|group]"),
    ("set-buffer", "setb", "[-aw] [-b buffer-name] [-n new-buffer-name] [-t target-client] [data]"),
    ("set-environment", "setenv", "[-Fhgru] [-t target-session] variable [value]"),
    ("set-hook", "", "[-agpERTuw] [-B name:what:format] [-t target-pane] [hook] [command]"),
    ("set-option", "set", "[-aFgopqsuUw] [-t target-pane] option [value]"),
    ("set-window-option", "setw", "[-aFgoqu] [-t target-window] option [value]"),
    ("show-buffer", "showb", "[-b buffer-name]"),
    ("show-environment", "showenv", "[-hgs] [-t target-session] [variable]"),
    ("show-hooks", "", "[-Bgpw] [-t target-pane] [hook]"),
    ("show-messages", "showmsgs", "[-JT] [-t target-client]"),
    ("show-options", "show", "[-AgHpqsvw] [-t target-pane] [option]"),
    ("show-prompt-history", "showphist", "[-T prompt-type]"),
    ("show-window-options", "showw", "[-gv] [-t target-window] [option]"),
    ("source-file", "source", "[-Fnqv] [-t target-pane] path ..."),
    ("split-window", "splitw", "[-bdefhIklPvWZ] [-B border-lines] [-c start-directory] [-e environment] [-F format] [-l size] [-m message] [-p percentage] [-s style] [-S active-border-style] [-R inactive-border-style] [-T title] [-t target-pane] [shell-command [argument ...]]"),
    ("start-server", "start", ""),
    ("suspend-client", "suspendc", "[-t target-client]"),
    ("swap-pane", "swapp", "[-dDUZ] [-s src-pane] [-t dst-pane]"),
    ("swap-window", "swapw", "[-d] [-s src-window] [-t dst-window]"),
    ("switch-client", "switchc", "[-ElnprZ] [-c target-client] [-t target-session] [-T key-table] [-O order]"),
    ("switch-mode", "", "[-kswZ] [-F format] [-t target-pane] [command]"),
    ("unbind-key", "unbind", "[-anq] [-T key-table] key"),
    ("unlink-window", "unlinkw", "[-k] [-t target-window]"),
    ("wait-for", "wait", "[-ELSUlv] [-F format] [-w waiter] name"),
];

/// The default template (LIST_COMMANDS_TEMPLATE).
pub const DEFAULT_TEMPLATE: &str =
    "#{command_list_name}#{?command_list_alias, (#{command_list_alias}),} #{command_list_usage}";

/// What a list-commands command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ListCommandsArgs {
    pub format: Option<String>,
    pub command: Option<String>,
}

impl ListCommandsArgs {
    /// Parse the arguments after `list-commands` (`F: [command]`).
    pub fn parse<S: AsRef<str>>(args: &[S]) -> ListCommandsArgs {
        let mut out = ListCommandsArgs::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_ref();
            if a == "--" {
                out.command = args.get(i + 1).map(|s| s.as_ref().to_string());
                break;
            }
            if a == "-F" {
                out.format = args.get(i + 1).map(|s| s.as_ref().to_string());
                i += 2;
                continue;
            }
            if let Some(f) = a.strip_prefix("-F") {
                out.format = Some(f.to_string());
                i += 1;
                continue;
            }
            if a.len() > 1 && a.starts_with('-') {
                i += 1;
                continue;
            }
            out.command = Some(a.to_string());
            break;
        }
        out
    }
}

/// Find a command the way tmux's cmd_find does: an alias wins at once, else
/// the names that start with `name`, an exact one ending the search. More
/// than one match is ambiguous.
pub fn find_command(name: &str) -> Result<&'static (&'static str, &'static str, &'static str), String> {
    let mut found: Option<&(&str, &str, &str)> = None;
    let mut ambiguous = false;
    for entry in COMMAND_TABLE {
        if !entry.1.is_empty() && entry.1 == name {
            ambiguous = false;
            found = Some(entry);
            break;
        }
        if !entry.0.starts_with(name) {
            continue;
        }
        if found.is_some() {
            ambiguous = true;
        }
        found = Some(entry);
        if entry.0 == name {
            break;
        }
    }
    if ambiguous {
        let names: Vec<&str> = COMMAND_TABLE
            .iter()
            .filter(|e| e.0.starts_with(name))
            .map(|e| e.0)
            .collect();
        return Err(format!("ambiguous command: {}, could be: {}", name, names.join(", ")));
    }
    found.ok_or_else(|| format!("unknown command: {}", name))
}

/// A row from the default template, without the format engine.
pub fn default_row(entry: &(&str, &str, &str)) -> String {
    if entry.1.is_empty() {
        format!("{} {}", entry.0, entry.2)
    } else {
        format!("{} ({}) {}", entry.0, entry.1, entry.2)
    }
}

/// The list-commands output: one row per command, each ending in a
/// newline, a row that expands to nothing left out (as tmux does). `expand`
/// turns the -F format into a row for one entry. Without -F the default
/// template is used, which needs no format engine.
pub fn render(
    args: &ListCommandsArgs,
    expand: &mut dyn FnMut(&str, &(&str, &str, &str)) -> String,
) -> Result<String, String> {
    let entries: Vec<&(&str, &str, &str)> = match args.command.as_deref() {
        Some(name) => vec![find_command(name)?],
        None => COMMAND_TABLE.iter().collect(),
    };
    let mut out = String::new();
    for entry in entries {
        let row = match args.format.as_deref() {
            Some(fmt) => expand(fmt, entry),
            None => default_row(entry),
        };
        if !row.is_empty() {
            out.push_str(&row);
            out.push('\n');
        }
    }
    Ok(out)
}

/// The list-commands output for rgs, -F expanded by the format engine
/// with the row's command in #{command_list_name}, #{command_list_alias}
/// and #{command_list_usage}.
pub fn render_with_app(app: &crate::types::AppState, args: &ListCommandsArgs) -> Result<String, String> {
    let mut expand = |fmt: &str, e: &(&str, &str, &str)| {
        crate::format::set_command_list_override(Some((e.0.to_string(), e.1.to_string(), e.2.to_string())));
        let row = crate::format::expand_format(fmt, app);
        crate::format::set_command_list_override(None);
        row
    };
    render(args, &mut expand)
}

#[cfg(test)]
#[path = "../tests-rs/test_list_commands_parity.rs"]
mod tests_list_commands_parity;