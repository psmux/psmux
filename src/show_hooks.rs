//! `show-hooks` output, as tmux 3.8 prints it (cmd-show-options.c).
//!
//! tmux keeps hooks as array options. `show-hooks -g` walks the options
//! table and prints every hook of the scope: a hook with commands prints one
//! `name[i] command` row per command, an empty one prints its bare name.
//! Without -g only the hooks set on the session, window or pane itself are
//! printed, so nothing at all when none is set. psmux printed `(no hooks)`,
//! ignored -g, -w, -p and a hook name, and used `name -> command` rows.
//!
//! psmux keeps every hook in one map whatever scope set-hook was given, so
//! the scope is taken from the hook's place in tmux's table: session hooks
//! for `show-hooks [-g]`, window and pane hooks for `-w`, pane hooks for `-p`.

use std::collections::HashMap;

/// Where a hook lives in tmux's options table (options-table.c).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HookClass {
    /// OPTIONS_TABLE_HOOK: session options.
    Session,
    /// OPTIONS_TABLE_WINDOW_HOOK: window options.
    Window,
    /// OPTIONS_TABLE_PANE_HOOK: window and pane options.
    Pane,
}

use HookClass::{Pane as P, Session as S, Window as W};

/// tmux 3.8's hooks in options table order, which is the order show-hooks
/// prints them in.
pub static HOOK_TABLE: &[(&str, HookClass)] = &[
    ("after-bind-key", S), ("after-capture-pane", S), ("after-copy-mode", S),
    ("after-display-message", S), ("after-display-panes", S), ("after-kill-pane", S),
    ("after-list-buffers", S), ("after-list-clients", S), ("after-list-keys", S),
    ("after-list-panes", S), ("after-list-sessions", S), ("after-list-windows", S),
    ("after-load-buffer", S), ("after-lock-server", S), ("after-new-session", S),
    ("after-new-window", S), ("after-paste-buffer", S), ("after-pipe-pane", S),
    ("after-refresh-client", S), ("after-rename-session", S), ("after-rename-window", S),
    ("after-resize-pane", S), ("after-resize-window", S), ("after-save-buffer", S),
    ("after-select-layout", S), ("after-select-pane", S), ("after-select-window", S),
    ("after-send-keys", S), ("after-set-buffer", S), ("after-set-environment", S),
    ("after-set-hook", S), ("after-set-option", S), ("after-show-environment", S),
    ("after-show-messages", S), ("after-show-options", S), ("after-split-window", S),
    ("after-unbind-key", S),
    ("alert-activity", S), ("alert-bell", S), ("alert-silence", S),
    ("client-active", S), ("client-attached", S), ("client-created", S),
    ("client-closed", S), ("client-detached", S), ("client-focus-in", S),
    ("client-focus-out", S), ("client-resized", S), ("client-session-changed", S),
    ("client-light-theme", S), ("client-dark-theme", S),
    ("command-error", S), ("marked-pane-changed", S),
    ("pane-activity", P), ("pane-bell", P), ("pane-command-finished", P),
    ("pane-command-started", P), ("pane-created", P), ("pane-died", P),
    ("pane-exited", P), ("pane-focus-in", P), ("pane-focus-out", P),
    ("pane-mode-changed", P), ("pane-mode-entered", P), ("pane-mode-exited", P),
    ("pane-moved", P), ("pane-prompt-closed", P), ("pane-prompt-opened", P),
    ("pane-resized", P), ("pane-set-clipboard", P), ("pane-shell-prompt", P),
    ("pane-title-changed", P),
    ("session-closed", S), ("session-created", S), ("session-added-to-group", S),
    ("session-renamed", S), ("session-removed-from-group", S),
    ("session-window-changed", S),
    ("window-created", W), ("window-closed", W), ("window-layout-changed", W),
    ("window-linked", S), ("window-pane-changed", W), ("window-renamed", W),
    ("window-resized", W), ("window-unzoomed", W), ("window-zoomed", W),
    ("window-unlinked", S),
];

/// The class of a hook. A name outside tmux's table (psmux's extra
/// `before-` and `after-` hooks) is a session hook, as tmux's after hooks are.
pub fn hook_class(name: &str) -> HookClass {
    HOOK_TABLE.iter().find(|(n, _)| *n == name).map_or(S, |(_, c)| *c)
}

/// What a show-hooks command line asks for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShowHooksArgs {
    pub global: bool,
    pub window: bool,
    pub pane: bool,
    pub name: Option<String>,
}

impl ShowHooksArgs {
    /// Parse the arguments after `show-hooks` (`Bgpt:w [hook]`).
    pub fn parse<S: AsRef<str>>(args: &[S]) -> ShowHooksArgs {
        let mut out = ShowHooksArgs::default();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_ref();
            if a == "--" {
                out.name = args.get(i + 1).map(|s| s.as_ref().to_string());
                break;
            }
            if a == "-t" {
                i += 2;
                continue;
            }
            if a.len() > 1 && a.starts_with('-') {
                if a.starts_with("-t") {
                    i += 1;
                    continue;
                }
                for ch in a[1..].chars() {
                    match ch {
                        'g' => out.global = true,
                        'w' => out.window = true,
                        'p' => out.pane = true,
                        _ => {}
                    }
                }
                i += 1;
                continue;
            }
            out.name = Some(a.to_string());
            break;
        }
        out
    }

    /// Whether a hook of `class` belongs to the scope these flags pick
    /// (options_scope_from_flags: -p is the pane, -w the window, else the
    /// session; tmux ignores -g with -p).
    fn shows(&self, class: HookClass) -> bool {
        if self.pane {
            class == P
        } else if self.window {
            class == W || class == P
        } else {
            class == S
        }
    }

    /// The global options of the scope: every hook there exists, empty or not.
    fn lists_empty(&self) -> bool {
        self.global && !self.pane
    }
}

fn push_rows(out: &mut String, name: &str, cmds: &[String]) {
    for (i, cmd) in cmds.iter().enumerate() {
        out.push_str(&format!("{}[{}] {}\n", name, i, cmd));
    }
}

/// The show-hooks output for `args` from psmux's hook map. Empty when
/// nothing is to be printed, as tmux prints nothing then.
pub fn format_show_hooks(hooks: &HashMap<String, Vec<String>>, args: &ShowHooksArgs) -> String {
    let mut out = String::new();
    let set = |name: &str| hooks.get(name).filter(|v| !v.is_empty());

    if let Some(arg) = args.name.as_deref() {
        let (base, key) = match arg.split_once('[') {
            Some((b, rest)) => (b, Some(rest.trim_end_matches(']'))),
            None => (arg, None),
        };
        match (set(base), key) {
            (Some(cmds), Some(k)) => {
                if let Some(cmd) = k.parse::<usize>().ok().and_then(|i| cmds.get(i)) {
                    out.push_str(&format!("{}[{}] {}\n", base, k, cmd));
                }
            }
            (Some(cmds), None) => push_rows(&mut out, base, cmds),
            // An empty hook exists only in the global options.
            (None, None) if args.global => {
                out.push_str(base);
                out.push('\n');
            }
            (None, _) => {}
        }
        return out;
    }

    for (name, class) in HOOK_TABLE {
        if !args.shows(*class) {
            continue;
        }
        match set(name) {
            Some(cmds) => push_rows(&mut out, name, cmds),
            None if args.lists_empty() => {
                out.push_str(name);
                out.push('\n');
            }
            None => {}
        }
    }
    // Hooks psmux fires that tmux's table does not carry, when set.
    let mut extra: Vec<&String> = hooks
        .iter()
        .filter(|(n, v)| !v.is_empty() && !HOOK_TABLE.iter().any(|(t, _)| *t == n.as_str()))
        .filter(|(n, _)| args.shows(hook_class(n)))
        .map(|(n, _)| n)
        .collect();
    extra.sort();
    for name in extra {
        push_rows(&mut out, name, &hooks[name]);
    }
    out
}

#[cfg(test)]
#[path = "../tests-rs/test_show_hooks_parity.rs"]
mod tests_show_hooks_parity;
