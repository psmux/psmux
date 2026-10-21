// `psmux set-hook -g after-new-window 'set -ga @a x \; set -ga @b y'` stored
// only `set -ga @a x` and ran `set -ga @b y` at once: the CLI joined argv with
// bare spaces, and the server's line splitter cut the line at the `\;`. tmux
// keeps an argv element whole unless it ENDS in `;` (cmd-parse.y:1094-1103),
// so the hook holds the whole list. The CLI now sends quoted_wire_line.
// End to end in tests/test_set_hook_cli_command_list.ps1.

use super::*;

const HOOK_CMD: &str = "set -ga @a x \\; set -ga @b y";

fn argv() -> Vec<&'static str> {
    vec!["set-hook", "-g", "after-new-window", HOOK_CMD]
}

#[test]
fn the_wire_line_is_one_command() {
    let line = crate::util::quoted_wire_line(&argv());
    assert_eq!(crate::config::split_chained_commands_pub(&line).len(), 1, "line: {}", line);
    // The bare join the CLI used to send is two commands.
    assert_eq!(crate::config::split_chained_commands_pub(&argv().join(" ")).len(), 2);
}

/// What the server's connection handler stores for this wire line: the line
/// is split into commands, tokenized, and set-hook's arguments read.
fn stored_hook(line: &str) -> (usize, Option<(String, String)>) {
    let commands = crate::config::split_chained_commands_pub(line);
    let tokens = parse_command_line(&commands[0]);
    let args: Vec<&str> = tokens[1..].iter().map(String::as_str).collect();
    let hook = match crate::server::connection::set_hook_request(&args) {
        Some(crate::types::CtrlReq::SetHook(name, cmd)) => Some((name, cmd)),
        _ => None,
    };
    (commands.len(), hook)
}

#[test]
fn the_hook_stores_the_whole_list() {
    let (n, hook) = stored_hook(&crate::util::quoted_wire_line(&argv()));
    assert_eq!(n, 1, "nothing runs besides set-hook");
    assert_eq!(hook, Some(("after-new-window".to_string(), HOOK_CMD.to_string())));
}

#[test]
fn plain_semicolon_inside_the_argument_stays_too() {
    let cmd = "set -ga @a x ; set -ga @b y";
    let (n, hook) = stored_hook(&crate::util::quoted_wire_line(&["set-hook", "-g", "after-new-window", cmd]));
    assert_eq!(n, 1);
    assert_eq!(hook, Some(("after-new-window".to_string(), cmd.to_string())));
}

#[test]
fn backslash_paths_survive() {
    let cmd = "run-shell \"echo C:\\temp\\x y\"";
    let (_, hook) = stored_hook(&crate::util::quoted_wire_line(&["set-hook", "-g", "after-new-window", cmd]));
    assert_eq!(hook, Some(("after-new-window".to_string(), cmd.to_string())));
}

#[test]
fn lone_separators_and_plain_words_are_sent_as_they_were() {
    assert_eq!(crate::util::quoted_wire_line(&["set-hook", "-g", "x", "display", "hi", "\\;", "display", "bye"]),
        "set-hook -g x display hi \\; display bye");
    assert_eq!(crate::util::quoted_wire_line(&[";"]), ";");
}
