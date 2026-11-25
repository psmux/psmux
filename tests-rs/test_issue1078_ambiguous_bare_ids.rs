// Issue #1078: a bare `%N` / `@N` that several sessions own went by recency.
//
// With sessions a, b and c alive `%1` exists three times (one server per
// session, ids counted per session). Outside any pane the client picked the
// newest session for `-t %1` at exit 0, and only the commands on the #545
// validation list asked who owns the id at all; `set-option -p -t %1`,
// `run-shell -t %1`, `kill-window -t @1` and the `-s` of break-pane,
// join-pane and swap-pane acted on the newest session's own `%1` / `@1`.
//
// These pin the pure pieces: which target strings count as a bare id (and
// which id decides the session), which commands take a client rather than a
// pane as `-t`, and which take an existing pane or window as `-s`. The routing
// itself needs live servers: tests/test_issue1078_ambiguous_bare_ids.ps1.

use super::{bare_id_kind, bare_id_of_target, bare_id_target_is_client, source_id_command};

#[test]
fn bare_pane_and_window_ids_are_recognised() {
    assert_eq!(bare_id_of_target("%1"), Some("%1"));
    assert_eq!(bare_id_of_target("%123"), Some("%123"));
    assert_eq!(bare_id_of_target("@1"), Some("@1"));
    // The window id decides the session for the window.pane forms.
    assert_eq!(bare_id_of_target("@2.1"), Some("@2"));
    assert_eq!(bare_id_of_target("@2.%3"), Some("@2"));
}

#[test]
fn qualified_relative_and_named_targets_are_not_bare_ids() {
    for t in [
        "a:%1", "=a:%1", "a:@1", "=a:0.0", "a:", ":%1", "%", "%x", "%1x", "@", "@dev", "@2x",
        "0", "1.0", "{last}", "!", "+", "-", "", "$1",
    ] {
        assert_eq!(bare_id_of_target(t), None, "{t:?} is not a bare id");
    }
}

#[test]
fn the_kind_follows_the_sigil() {
    assert_eq!(bare_id_kind("%4"), "pane");
    assert_eq!(bare_id_kind("@4"), "window");
}

#[test]
fn client_target_commands_keep_their_own_routing() {
    for c in [
        "switch-client", "switchc", "detach-client", "detach", "command-prompt",
        "show-messages", "refresh-client", "suspend-client", "lock-client", "set-buffer",
        "confirm-before", "display-panes",
    ] {
        assert!(bare_id_target_is_client(c), "{c} takes a client -t");
    }
}

#[test]
fn pane_and_window_commands_are_checked() {
    for c in [
        "set-option", "set", "show-options", "show", "run-shell", "run", "if-shell", "if",
        "kill-window", "respawn-pane", "respawn-window", "display-popup", "swap-pane",
        "join-pane", "break-pane", "new-window", "move-window", "send-keys", "display-message",
    ] {
        assert!(!bare_id_target_is_client(c), "{c} takes a pane or window -t");
    }
}

#[test]
fn source_commands_are_the_ones_whose_s_names_a_pane_or_window() {
    for c in [
        "join-pane", "joinp", "move-pane", "movep", "swap-pane", "swapp", "break-pane", "breakp",
        "move-window", "movew", "link-window", "linkw", "swap-window", "swapw",
    ] {
        assert!(source_id_command(c), "{c} -s is a pane or window");
    }
    // `-s` is a session NAME for these, never an id to resolve.
    for c in ["new-session", "new", "rename-session", "attach-session", "send-keys", "split-window"] {
        assert!(!source_id_command(c), "{c} -s is not a source pane");
    }
}
