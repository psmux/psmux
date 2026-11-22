// Discussion #571: tmux getopt accepts attached short-option arguments
// (`tmux -Lsockname`), and libtmux emits that form.  psmux's exact-match
// global scanners silently DROPPED such tokens, so `-Lfoo new-session`
// landed in the DEFAULT namespace.  normalize_attached_global_args splits
// the attached form in the pre-subcommand region only.

use crate::cli::{normalize_attached_global_args, normalize_flag_equals};

fn norm(args: &[&str]) -> Vec<String> {
    normalize_attached_global_args(args.iter().map(|s| s.to_string()).collect())
}

#[test]
fn attached_socket_name_is_split() {
    assert_eq!(
        norm(&["psmux", "-Ld571", "new-session", "-d", "-s", "a"]),
        vec!["psmux", "-L", "d571", "new-session", "-d", "-s", "a"]
    );
}

#[test]
fn attached_f_s_t_are_split() {
    assert_eq!(
        norm(&["psmux", "-fC:\\conf", "ls"]),
        vec!["psmux", "-f", "C:\\conf", "ls"]
    );
    assert_eq!(
        norm(&["psmux", "-SC:\\sock", "ls"]),
        vec!["psmux", "-S", "C:\\sock", "ls"]
    );
    assert_eq!(
        norm(&["psmux", "-tdev", "attach"]),
        vec!["psmux", "-t", "dev", "attach"]
    );
}

#[test]
fn detached_form_passes_through_and_value_is_not_a_subcommand() {
    // "foo" is -L's value, not the subcommand: the region after it must
    // still be rewritten.
    assert_eq!(
        norm(&["psmux", "-L", "foo", "-tdev", "attach"]),
        vec!["psmux", "-L", "foo", "-t", "dev", "attach"]
    );
}

#[test]
fn post_subcommand_flags_are_never_touched() {
    // select-pane -L takes NO value; command-level attached flags such as
    // new-session -sfoo are command-parser territory and stay as-is.
    assert_eq!(
        norm(&["psmux", "select-pane", "-L"]),
        vec!["psmux", "select-pane", "-L"]
    );
    assert_eq!(
        norm(&["psmux", "new-session", "-sfoo"]),
        vec!["psmux", "new-session", "-sfoo"]
    );
    assert_eq!(
        norm(&["psmux", "-Lns", "resize-pane", "-L", "5"]),
        vec!["psmux", "-L", "ns", "resize-pane", "-L", "5"]
    );
}

#[test]
fn control_mode_and_boolean_flags_pass_through() {
    assert_eq!(
        norm(&["psmux", "-CC", "-Lfoo", "attach"]),
        vec!["psmux", "-CC", "-L", "foo", "attach"]
    );
    assert_eq!(
        norm(&["psmux", "-v", "-Lfoo", "ls"]),
        vec!["psmux", "-v", "-L", "foo", "ls"]
    );
}

#[test]
fn unhandled_letters_and_edge_shapes_pass_through() {
    // -c is not consumed as value-taking by psmux's global scanners:
    // splitting it would fabricate a fake subcommand token. (-T was the
    // same until #1080, see test_issue1080_client_features_flag.rs.)
    assert_eq!(norm(&["psmux", "-cfoo", "ls"]), vec!["psmux", "-cfoo", "ls"]);
    // long flags, bare dash, trailing valueless flag
    assert_eq!(norm(&["psmux", "--Lfoo", "ls"]), vec!["psmux", "--Lfoo", "ls"]);
    assert_eq!(norm(&["psmux", "-", "ls"]), vec!["psmux", "-", "ls"]);
    assert_eq!(norm(&["psmux", "-L"]), vec!["psmux", "-L"]);
}

#[test]
fn composes_with_flag_equals_normalization() {
    // main.rs chains flag_equals FIRST, then attached: `-L=foo` must become
    // `-L foo`, never `-L` + `=foo`.
    let chained = normalize_attached_global_args(normalize_flag_equals(
        vec!["psmux".into(), "-L=foo".into(), "ls".into()],
    ));
    assert_eq!(chained, vec!["psmux", "-L", "foo", "ls"]);
}

// --- command-level attached -t slice ---

use crate::cli::normalize_attached_target_flag;

fn normt(args: &[&str]) -> Vec<String> {
    normalize_attached_target_flag(args.iter().map(|s| s.to_string()).collect())
}

#[test]
fn command_level_attached_t_is_split() {
    assert_eq!(
        normt(&["psmux", "kill-session", "-tvictimA"]),
        vec!["psmux", "kill-session", "-t", "victimA"]
    );
    assert_eq!(
        normt(&["psmux", "display-message", "-tdev", "-p", "#S"]),
        vec!["psmux", "display-message", "-t", "dev", "-p", "#S"]
    );
}

#[test]
fn detached_t_and_other_flags_pass_through() {
    assert_eq!(
        normt(&["psmux", "kill-session", "-t", "a"]),
        vec!["psmux", "kill-session", "-t", "a"]
    );
    // -T is boolean in some commands: never rewritten by this pass
    assert_eq!(
        normt(&["psmux", "split-window", "-Ttitle"]),
        vec!["psmux", "split-window", "-Ttitle"]
    );
}

#[test]
fn dashdash_protects_literals() {
    assert_eq!(
        normt(&["psmux", "send-keys", "-t", "a", "--", "-tliteral"]),
        vec!["psmux", "send-keys", "-t", "a", "--", "-tliteral"]
    );
    assert_eq!(
        normt(&["psmux", "new-session", "--", "cat", "-tfoo"]),
        vec!["psmux", "new-session", "--", "cat", "-tfoo"]
    );
}

#[test]
fn global_region_is_left_to_the_global_pass() {
    // chained as in main.rs: globals split by attached_global first, then
    // the target pass leaves the already-detached pair alone
    let chained = normalize_attached_target_flag(
        crate::cli::normalize_attached_global_args(
            ["psmux", "-Lns", "kill-session", "-tx"].iter().map(|s| s.to_string()).collect(),
        ),
    );
    assert_eq!(chained, vec!["psmux", "-L", "ns", "kill-session", "-t", "x"]);
}
