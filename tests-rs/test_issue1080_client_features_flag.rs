// Issue #1080: tmux's `-T features` client flag (tmux.c:476 at tag 3.8).
// psmux read the flag's value as the command, so `psmux -T 256,RGB attach`
// ended with "unknown command: 256,RGB".

use crate::cli::{normalize_attached_global_args, normalize_flag_equals};
use crate::terminal_features::{progressbar_enabled, progressbar_enabled_for, TerminalFeatures};

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

fn named(value: &str) -> String {
    TerminalFeatures::from_bits(0).with_client_flag(value).to_string()
}

#[test]
fn the_value_of_the_flag_is_not_the_command() {
    let args = strings(&["psmux", "-T", "256,RGB", "attach", "-t", "=a"]);
    assert_eq!(crate::process_command_index(&args), Some(3));
    let args = strings(&["psmux", "-L", "ns", "-T", "RGB", "-T", "title", "ls"]);
    assert_eq!(crate::process_command_index(&args), Some(7));
    // A value that is itself a command name is still the value.
    let args = strings(&["psmux", "-T", "ls", "attach"]);
    assert_eq!(crate::process_command_index(&args), Some(3));
}

#[test]
fn the_attached_and_equals_spellings_are_split() {
    assert_eq!(
        normalize_attached_global_args(strings(&["psmux", "-T256,RGB", "attach"])),
        strings(&["psmux", "-T", "256,RGB", "attach"])
    );
    assert_eq!(
        normalize_flag_equals(strings(&["psmux", "-T=256,RGB", "attach"])),
        strings(&["psmux", "-T", "256,RGB", "attach"])
    );
    // The detached value is not the command: the flag after it is still global.
    assert_eq!(
        normalize_attached_global_args(strings(&["psmux", "-T", "RGB", "-Lns", "ls"])),
        strings(&["psmux", "-T", "RGB", "-L", "ns", "ls"])
    );
    // After the command, -T belongs to the command.
    assert_eq!(
        normalize_attached_global_args(strings(&["psmux", "select-pane", "-Ttitle"])),
        strings(&["psmux", "select-pane", "-Ttitle"])
    );
}

#[test]
fn names_are_split_at_colon_or_comma_in_any_case() {
    assert_eq!(named("256,RGB"), "256,RGB");
    assert_eq!(named("256:RGB"), "256,RGB");
    assert_eq!(named("rgb,HYPERLINKS:Title"), "RGB,hyperlinks,title");
    assert_eq!(named("progressbar"), "progressbar");
    assert_eq!(named(""), "");
}

#[test]
fn a_name_tmux_does_not_know_ends_the_list() {
    assert_eq!(named("bogus,RGB"), "");
    assert_eq!(named("RGB,bogus,title"), "RGB");
    assert_eq!(named("RGB,,title"), "RGB");
    // A feature tmux knows and psmux keeps no bit for does not end it.
    assert_eq!(named("sync,RGB,clipboard,title"), "RGB,title");
}

#[test]
fn a_name_with_an_at_sign_takes_the_feature_back_out() {
    assert_eq!(named("256,RGB,256@"), "RGB");
    assert_eq!(named("RGB@,RGB"), "RGB");
    assert_eq!(named("sync@,title"), "title");
    assert_eq!(named("@,title"), "");
}

#[test]
fn the_flag_adds_to_what_the_client_already_reports() {
    let detected = TerminalFeatures::from_bits(
        TerminalFeatures::COLOR_256.bits() | TerminalFeatures::RGB.bits() | TerminalFeatures::TITLE.bits(),
    );
    assert_eq!(detected.with_client_flag("hyperlinks").to_string(), "256,RGB,hyperlinks,title");
    assert_eq!(detected.with_client_flag("256,RGB").to_string(), "256,RGB,title");
    // Bits psmux has no name for are kept.
    let unknown = TerminalFeatures::from_bits(1u64 << 63);
    assert_eq!(unknown.with_client_flag("RGB").bits(), (1u64 << 63) | TerminalFeatures::RGB.bits());
}

#[test]
fn terminal_features_is_read_on_top_of_a_reported_progressbar() {
    let none: [&str; 0] = [];
    assert!(!progressbar_enabled_for(false, none, "xterm-256color"));
    assert!(progressbar_enabled_for(true, none, "xterm-256color"));
    assert!(progressbar_enabled_for(true, ["xterm*:title"], "xterm-256color"));
    assert!(!progressbar_enabled_for(true, ["*:progressbar@"], "xterm-256color"));
    assert!(progressbar_enabled_for(true, ["vt100:progressbar@"], "xterm-256color"));
    // Without a report the answer is the one it was.
    for entries in [vec!["*:progressbar"], vec!["*:progressbar@"], vec!["xterm*:title"]] {
        assert_eq!(
            progressbar_enabled_for(false, entries.iter(), "xterm-256color"),
            progressbar_enabled(entries.iter(), "xterm-256color")
        );
    }
}
