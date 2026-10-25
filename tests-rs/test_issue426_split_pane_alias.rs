// Issue #426: split-pane / splitp are tmux default command-aliases for
// split-window. psmux was missing them, so `psmux split-pane` errored with
// "unknown command" and Claude Code agent-team / manual claude panes fell
// back to a bare PowerShell prompt. They are now elements of the default
// command-alias array, and a binding expands them when it is made, as tmux
// parses a bound command list at bind time (cmd-bind-key.c). These tests
// prove the binding parse path (bind_commands, then parse_command_to_action)
// treats the aliases exactly like split-window across the bare, -h, and
// command-argument forms.

use super::*;

/// The action a binding to `command` gets, with tmux's default aliases.
fn bound(command: &str) -> Option<Action> {
    let aliases = std::sync::Arc::new(std::sync::RwLock::new(
        crate::command_alias::CommandAliases::with_defaults(),
    ));
    let commands = crate::config::bind_commands(&aliases, command);
    assert_eq!(commands.len(), 1, "{:?}", commands);
    parse_command_to_action(&commands[0])
}

#[test]
fn split_pane_bare_maps_to_split_vertical() {
    // Bare split-window is a vertical split; split-pane must match.
    assert!(matches!(bound("split-window"), Some(Action::SplitVertical)));
    assert!(matches!(bound("split-pane"),   Some(Action::SplitVertical)));
    assert!(matches!(bound("splitp"),       Some(Action::SplitVertical)));
}

#[test]
fn split_pane_dash_h_maps_to_split_horizontal() {
    assert!(matches!(bound("split-window -h"), Some(Action::SplitHorizontal)));
    assert!(matches!(bound("split-pane -h"),   Some(Action::SplitHorizontal)));
    assert!(matches!(bound("splitp -h"),       Some(Action::SplitHorizontal)));
}

#[test]
fn split_pane_with_command_preserves_full_string() {
    // Extra args (a shell command, -c, -d, -P ...) must be preserved verbatim
    // as Action::Command so the alias behaves identically to split-window.
    match bound("split-pane -h -P -F #{pane_id}") {
        Some(Action::Command(s)) => assert_eq!(s, "split-window -h -P -F #{pane_id}"),
        other => panic!("expected Action::Command for split-pane with extra args, got {:?}", other.as_ref().map(format_action)),
    }
    match bound("splitp -c C:/tmp claude") {
        Some(Action::Command(s)) => assert_eq!(s, "split-window -c C:/tmp claude"),
        other => panic!("expected Action::Command for splitp with a command argument, got {:?}", other.as_ref().map(format_action)),
    }
    // Parity check: split-window with the same extras behaves the same way.
    match bound("split-window -h -P -F #{pane_id}") {
        Some(Action::Command(_)) => {}
        _ => panic!("split-window baseline changed"),
    }
}

#[test]
fn the_choosers_and_info_expand_too() {
    assert!(matches!(bound("choose-window"), Some(Action::WindowChooser)));
    assert!(matches!(bound("choose-session"), Some(Action::SessionChooser)));
    // The alias appends the caller's words: `choose-tree -w -s`.
    assert!(matches!(bound("choose-window -s"), Some(Action::SessionChooser)));
    match bound("info") {
        Some(Action::Command(s)) => assert_eq!(s, "show-messages -JT"),
        other => panic!("expected show-messages -JT, got {:?}", other.as_ref().map(format_action)),
    }
}
