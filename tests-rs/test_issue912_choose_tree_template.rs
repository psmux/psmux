//! choose-tree's template argument (#912). tmux's window_tree_init keeps
//! `args_string(args, 0)` as the command, defaulting to
//! `switch-client -Zt '%%'` (window-tree.c), and Enter runs it with the row's
//! target from window_tree_get_target put in by
//! `cmd_template_replace(template, target, 1)` (mode-tree.c,
//! mode_tree_run_command). psmux read the flags and dropped the template, so
//! Enter always switched.

use super::*;

fn tmpl(args: &[&str]) -> Option<String> {
    chooser_template(args)
}

#[test]
fn no_positional_argument_means_no_template() {
    assert_eq!(tmpl(&[]), None);
    assert_eq!(tmpl(&["-Zs"]), None);
    assert_eq!(tmpl(&["-s", "-Z"]), None);
    assert_eq!(tmpl(&["-F", "#{session_name}"]), None);
    assert_eq!(tmpl(&["-t", "a:0"]), None);
}

#[test]
fn the_first_word_after_the_flags_is_the_template() {
    assert_eq!(tmpl(&["display-message 'picked %%'"]).as_deref(), Some("display-message 'picked %%'"));
    assert_eq!(tmpl(&["-Zs", "set -g @p '%%'"]).as_deref(), Some("set -g @p '%%'"));
    assert_eq!(tmpl(&["-sZ", "move-window -t '%%'"]).as_deref(), Some("move-window -t '%%'"));
}

#[test]
fn a_flag_value_is_not_the_template() {
    // -F, -f, -K, -O and -t take a value; that value is not the template.
    assert_eq!(tmpl(&["-F", "#{window_name}", "join-pane -s '%%'"]).as_deref(), Some("join-pane -s '%%'"));
    assert_eq!(tmpl(&["-O", "name", "-f", "1", "x %%"]).as_deref(), Some("x %%"));
    assert_eq!(tmpl(&["-t", "a:0", "-Zw", "y %%"]).as_deref(), Some("y %%"));
}

#[test]
fn double_dash_ends_the_flags() {
    assert_eq!(tmpl(&["-s", "--", "-not-a-flag %%"]).as_deref(), Some("-not-a-flag %%"));
}

#[test]
fn an_empty_template_is_kept() {
    // tmux: args_count != 0, so data->command is "" and Enter runs nothing.
    assert_eq!(tmpl(&["-s", ""]).as_deref(), Some(""));
}

#[test]
fn row_targets_match_window_tree_get_target() {
    assert_eq!(session_row_target("b"), "=b:");
    assert_eq!(window_row_target("b", 2), "=b:2.");
    assert_eq!(pane_row_target("b", 2, 7), "=b:2.%7");
}

#[test]
fn template_command_replaces_the_first_double_percent_and_every_percent_one() {
    assert_eq!(template_command("display-message 'picked %%'", "=b:"), "display-message 'picked =b:'");
    // cmd_template_replace replaces only the first `%%`.
    assert_eq!(template_command("a %% b %%", "=b:"), "a =b: b %%");
    assert_eq!(template_command("x %1 y %1", "=b:1."), "x =b:1. y =b:1.");
    assert_eq!(template_command("no placeholder", "=b:"), "no placeholder");
}

#[test]
fn a_quote_in_the_target_survives_single_quotes() {
    let cmd = template_command("set -g @p '%%'", "=it's:");
    let words = crate::commands::parse_command_line(&cmd);
    assert_eq!(words, vec!["set", "-g", "@p", "=it's:"]);
}

#[test]
fn directive_without_template_is_unchanged() {
    assert_eq!(directive_line(ChooserKind::Tree, None), "CHOOSE-TREE");
    assert_eq!(directive_line(ChooserKind::Session, None), "CHOOSE-SESSION");
    assert_eq!(parse_directive("CHOOSE-TREE"), Some((ChooserKind::Tree, None)));
    assert_eq!(ChooserKind::from_directive("CHOOSE-SESSION"), Some(ChooserKind::Session));
    assert_eq!(parse_directive("SWITCH x"), None);
}

#[test]
fn directive_carries_any_template_on_one_line() {
    for t in [
        "switch-client -t \"%%\" ; if -F \"##{!=:##{client_session},#{session_name}}\" \"kill-session -t =#{session_name}\"",
        "a\nb",
        "",
        "it's \\ \"quoted\"",
    ] {
        let line = directive_line(ChooserKind::Session, Some(t));
        assert!(!line.contains('\n'), "directive must stay one line: {:?}", line);
        assert_eq!(parse_directive(&line), Some((ChooserKind::Session, Some(t.to_string()))));
        assert_eq!(ChooserKind::from_directive(&line), Some(ChooserKind::Session));
    }
}
