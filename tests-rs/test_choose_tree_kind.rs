//! choose-tree `-s` and `-w` pick the chooser, as in tmux's window_tree_init
//! (window-tree.c):
//!
//! ```text
//! if (args_has(args, 's'))
//!         data->type = WINDOW_TREE_SESSION;
//! else if (args_has(args, 'w'))
//!         data->type = WINDOW_TREE_WINDOW;
//! ```
//!
//! tmux's default command aliases make choose-window `choose-tree -w` and
//! choose-session `choose-tree -s` (options-table.c), and its default prefix+s
//! and prefix+w are `choose-tree -Zs` and `choose-tree -Zw` (key-bindings.c).
//! Before the fix no route read either flag: a binding to `choose-tree -s`
//! opened the window tree.

use super::*;

/// The chooser `name args...` opens. choose-window and choose-session reach
/// choose-tree through the default command aliases, as on every route.
fn kind(name: &str, args: &[&str]) -> ChooserKind {
    let mut words: Vec<String> = std::iter::once(name).chain(args.iter().copied()).map(String::from).collect();
    if let Some(mut commands) = crate::command_alias::CommandAliases::with_defaults().expand(&words) {
        words = commands.remove(0);
    }
    assert_eq!(words[0], "choose-tree");
    chooser_kind(&words[1..])
}

#[test]
fn no_flag_and_w_open_the_tree() {
    assert_eq!(kind("choose-tree", &[]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-w"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-Zw"]), ChooserKind::Tree);
    assert_eq!(kind("choose-window", &[]), ChooserKind::Tree);
}

#[test]
fn s_opens_the_session_list() {
    assert_eq!(kind("choose-tree", &["-s"]), ChooserKind::Session);
    assert_eq!(kind("choose-tree", &["-Zs"]), ChooserKind::Session);
    assert_eq!(kind("choose-tree", &["-sZ"]), ChooserKind::Session);
    assert_eq!(kind("choose-tree", &["-Z", "-s"]), ChooserKind::Session);
    assert_eq!(kind("choose-session", &[]), ChooserKind::Session);
}

#[test]
fn s_wins_over_w() {
    assert_eq!(kind("choose-tree", &["-w", "-s"]), ChooserKind::Session);
    assert_eq!(kind("choose-tree", &["-s", "-w"]), ChooserKind::Session);
    assert_eq!(kind("choose-tree", &["-ws"]), ChooserKind::Session);
    // The aliases append the caller's words: `choose-window -s` is
    // `choose-tree -w -s`, `choose-session -w` is `choose-tree -s -w`.
    assert_eq!(kind("choose-window", &["-s"]), ChooserKind::Session);
    assert_eq!(kind("choose-session", &["-w"]), ChooserKind::Session);
}

#[test]
fn an_s_inside_a_value_is_not_the_flag() {
    assert_eq!(kind("choose-tree", &["-F", "#{s}"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-F#{s}"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-f", "-s"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-t", "s:1", "-w"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-ZF", "-s"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["-F", "x", "-s"]), ChooserKind::Session);
}

#[test]
fn flags_end_at_the_template() {
    assert_eq!(kind("choose-tree", &["switch-client -t '%%'", "-s"]), ChooserKind::Tree);
    assert_eq!(kind("choose-tree", &["--", "-s"]), ChooserKind::Tree);
}

#[test]
fn directives_name_the_chooser() {
    for k in [ChooserKind::Tree, ChooserKind::Session] {
        assert_eq!(ChooserKind::from_directive(k.directive()), Some(k));
    }
    assert_eq!(ChooserKind::from_directive("SWITCH x"), None);
}
