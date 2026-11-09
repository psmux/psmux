// What a client writes for the progress indicator, from `terminal-features`
// and `terminal-overrides`. Every expected value here was measured from tmux
// 3.8 under script(1), `-f` a config holding the same lines, except the default
// element, which is psmux's own. Measured with TERM=xterm-256color.

use super::*;
use crate::terminal_features::{progressbar_enabled, TerminalFeatures};

const TERM: &str = "xterm-256color";

fn feature(entries: &[&str]) -> bool {
    progressbar_enabled(entries, TERM)
}

/// What a client writes for 1;50, with these two options.
fn written(features: &[&str], overrides: &[&str]) -> Option<String> {
    let features: Vec<String> = features.iter().map(|s| s.to_string()).collect();
    let overrides: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    progress_sequence(Some(features.as_slice()), &overrides, TERM, "1;50")
}

#[test]
fn the_default_turns_the_feature_on_for_any_term() {
    let default = crate::option_array::default_values("terminal-features");
    assert_eq!(default.last(), Some(&"*:progressbar"));
    assert!(progressbar_enabled(&default, TERM));
    // An unset TERM, a native Windows console, is matched as the empty string.
    assert!(progressbar_enabled(&default, ""));
    assert_eq!(
        progress_sequence::<String>(None, &[], "", "1;50").as_deref(),
        Some("\x1b]9;4;1;50\x1b\\"),
    );
}

#[test]
fn tmuxs_own_default_elements_leave_it_off() {
    assert!(!feature(&["xterm*:clipboard:ccolour:cstyle:focus:title", "screen*:title", "rxvt*:ignorefkeys"]));
    assert!(!feature(&[]));
    assert!(!feature(&["xterm*:title"]));
}

#[test]
fn a_matching_element_adds_it() {
    assert!(feature(&["*:progressbar"]));
    assert!(feature(&["xterm*:progressbar"]));
    assert!(feature(&["*:PROGRESSBAR"]));
    assert!(feature(&["*:title:progressbar"]));
    assert!(!feature(&["nomatch*:progressbar"]));
}

#[test]
fn removing_it_wins_whichever_comes_first() {
    assert!(!feature(&["*:progressbar", "*:progressbar@"]));
    assert!(!feature(&["*:progressbar@", "*:progressbar"]));
    assert!(!feature(&["*:progressbar:progressbar@"]));
    assert!(!feature(&["*:progressbar@:progressbar"]));
    assert!(!feature(&["*:PROGRESSBAR@", "*:progressbar"]));
    // A removal for another TERM touches nothing.
    assert!(feature(&["*:progressbar", "nomatch*:progressbar@"]));
}

#[test]
fn a_name_tmux_does_not_know_ends_the_element() {
    assert!(!feature(&["*:bogus:progressbar"]));
    assert!(feature(&["*:progressbar:bogus"]));
    assert!(feature(&["*:bogus", "*:progressbar"]));
    assert!(!feature(&["*:title::progressbar"]));
    assert!(!feature(&["*:@:progressbar"]));
    assert!(!feature(&["*:"]));
    assert!(!feature(&["*"]));
    assert!(!feature(&[""]));
}

#[test]
fn a_double_colon_is_part_of_the_pattern() {
    assert_eq!(split_pattern("a::b:c:d"), Some(("a:b".to_string(), "c:d")));
    assert_eq!(split_pattern("a"), Some(("a".to_string(), "")));
    assert_eq!(split_pattern(""), None);
    // `*:` and then `progressbar`, all one pattern.
    assert!(!feature(&["*::progressbar"]));
    assert!(progressbar_enabled(["a::b:progressbar"], "a:b"));
}

#[test]
fn overrides_apply_after_the_features() {
    assert_eq!(written(&["*:progressbar"], &[]).as_deref(), Some("\x1b]9;4;1;50\x1b\\"));
    assert_eq!(written(&[], &[]), None);
    assert_eq!(written(&["*:progressbar"], &["*:Spb@"]), None);
    assert_eq!(written(&["*:progressbar"], &["*:Spb="]), None);
    assert_eq!(written(&["*:progressbar"], &["*:Spb"]), None);
    assert_eq!(written(&["*:progressbar"], &["nomatch*:Spb@"]).as_deref(), Some("\x1b]9;4;1;50\x1b\\"));
    assert_eq!(written(&["*:progressbar@"], &["*:Spb=x%p2%d"]).as_deref(), Some("x50"));
}

#[test]
fn an_override_writes_with_no_feature() {
    assert_eq!(written(&[], &[r"*:Spb=\E]9;4;%p1%d;%p2%d\a"]).as_deref(), Some("\x1b]9;4;1;50\x07"));
    assert_eq!(written(&[], &[r"*:Spb=\E]9;4;%p1%d;%p2%d\E\\"]).as_deref(), Some("\x1b]9;4;1;50\x1b\\"));
}

#[test]
fn overrides_apply_in_order() {
    assert_eq!(written(&[], &["*:Spb@", "*:Spb=x%p2%d"]).as_deref(), Some("x50"));
    assert_eq!(written(&[], &["*:Spb=x%p2%d", "*:Spb@"]), None);
    assert_eq!(written(&[], &["*:Spb=x%p2%d", "*:Spb=y%p2%d"]).as_deref(), Some("y50"));
}

#[test]
fn the_parameters_are_filled_in_as_tiparm_does() {
    assert_eq!(expand_progress_cap("PBX%p2%d-%p1%d%%", 1, 50).as_deref(), Some("PBX50-1%"));
    assert_eq!(expand_progress_cap("PBX%p2%d", 1, 50).as_deref(), Some("PBX50"));
    assert_eq!(expand_progress_cap(PROGRESS_CAP, 3, 0).as_deref(), Some("\x1b]9;4;3;0\x1b\\"));
}

#[test]
fn a_string_without_parameter_two_writes_nothing() {
    assert_eq!(expand_progress_cap("PBXplain", 1, 50), None);
    assert_eq!(expand_progress_cap("PBX%p1%d", 1, 50), None);
    assert_eq!(expand_progress_cap("", 1, 50), None);
}

#[test]
fn an_operator_psmux_does_not_run_writes_nothing() {
    assert_eq!(expand_progress_cap("%p2%c", 1, 50), None);
    assert_eq!(expand_progress_cap("%p1%{1}%+%p2%d", 1, 50), None);
    assert_eq!(expand_progress_cap("%p3%d%p2%d", 1, 50), None);
    assert_eq!(expand_progress_cap("%p2%d%", 1, 50), None);
    assert_eq!(expand_progress_cap("%p", 1, 50), None);
}

#[test]
fn a_progress_that_is_no_pair_writes_nothing() {
    for progress in ["", "1", "1;", ";50", "x;50", "1;x"] {
        assert_eq!(progress_sequence::<String>(None, &[], TERM, progress), None, "{progress:?}");
    }
}

#[test]
fn term_comes_from_the_environment_block() {
    let env = |name: &str, value: &str| vec![(name.to_string(), value.to_string())];
    assert_eq!(environ_term(&env("TERM", "xterm")), "xterm");
    assert_eq!(environ_term(&env("Term", "xterm")), "xterm");
    assert_eq!(environ_term(&env("TERMINAL", "xterm")), "");
    assert_eq!(environ_term(&[]), "");
}

#[test]
fn the_feature_mask_names_it_and_can_clear_it() {
    let base = TerminalFeatures::from_bits(TerminalFeatures::TITLE.bits());
    assert_eq!(base.with(TerminalFeatures::PROGRESSBAR, true).to_string(), "title,progressbar");
    assert_eq!(base.with(TerminalFeatures::PROGRESSBAR, false).to_string(), "title");
    // A client's report of the bit counts for nothing once the option is read.
    let reported = TerminalFeatures::from_bits(u64::MAX);
    assert!(!reported.with(TerminalFeatures::PROGRESSBAR, false).contains(TerminalFeatures::PROGRESSBAR));
}
