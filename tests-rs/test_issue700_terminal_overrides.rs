// Issue #700: `set -ga terminal-overrides ',*:smcup@:rmcup@'` must keep the
// attached client on the host terminal's main screen, as it does in tmux.
//
// These cover the parser (tmux tty_term_override_next / tty_term_apply), the
// fnmatch on TERM, the array semantics of set -g / set -ga on the config and
// runtime routes, the show-options rendering, the wire field, and the exact
// bytes the client emits on start and stop. The E2E counterpart that counts
// ESC[?1049h in a real client's output is
// tests/test_issue700_terminal_overrides_smcup.ps1.

use super::*;
use crate::types::AppState;

fn app() -> AppState {
    AppState::new("test_700".to_string())
}

/// The elements a value without an index gives, split the way
/// `options_array_assign` splits it (crate::option_array).
fn elements(value: &str) -> Vec<String> {
    let mut array = crate::option_array::OptionArray::default();
    array.assign(value, ",");
    array.iter().map(|(_, entry)| entry.to_string()).collect()
}

/// The server's terminal-overrides elements in index order.
fn values(a: &AppState) -> Vec<String> {
    a.terminal_overrides.iter().map(|(_, entry)| entry.to_string()).collect()
}

// ── parser ──────────────────────────────────────────────────────────────────

#[test]
fn assign_drops_empty_elements_and_keeps_spaces_like_options_array_assign() {
    assert_eq!(elements(",*:smcup@:rmcup@"), vec!["*:smcup@:rmcup@"]);
    assert_eq!(elements("a:x@,,b:y@,"), vec!["a:x@", "b:y@"]);
    assert!(elements("").is_empty());
    // strsep splits and nothing trims (options.c:531), so tmux 3.7c shows
    // `terminal-overrides[1] " b:rmcup@ "` for this value.
    assert_eq!(elements("a:smcup@, b:rmcup@ ,c"), vec!["a:smcup@", " b:rmcup@ ", "c"]);
}

#[test]
fn double_colon_is_a_literal_colon() {
    assert_eq!(split_fields("xterm*:Ss=\\E[%p1%d q:Se::x"), vec!["xterm*", "Ss=\\E[%p1%d q", "Se:x"]);
}

#[test]
fn parse_entry_reads_remove_set_and_flag_fields() {
    let e = parse_entry("xterm*:smcup@:Tc:Ss=\\E[5 q").unwrap();
    assert_eq!(e.pattern, "xterm*");
    assert_eq!(
        e.caps,
        vec![
            ("smcup".to_string(), CapOp::Remove),
            ("Tc".to_string(), CapOp::Set(String::new())),
            ("Ss".to_string(), CapOp::Set("\x1b[5 q".to_string())),
        ]
    );
}

#[test]
fn unescape_handles_terminfo_escapes() {
    assert_eq!(unescape("\\E[?1049h"), "\x1b[?1049h");
    assert_eq!(unescape("\\033[m"), "\x1b[m");
    assert_eq!(unescape("a\\\\b"), "a\\b");
    assert_eq!(unescape("\\q"), "\\q");
}

// ── fnmatch ─────────────────────────────────────────────────────────────────

#[test]
fn fnmatch_matches_like_libc() {
    assert!(fnmatch("*", "xterm-256color"));
    assert!(fnmatch("*", ""), "an unset TERM is the empty string and * matches it");
    assert!(fnmatch("xterm*", "xterm-256color"));
    assert!(!fnmatch("xterm*", ""));
    assert!(!fnmatch("screen*", "xterm-256color"));
    assert!(fnmatch("xterm-?56color", "xterm-256color"));
    assert!(fnmatch("[sx]term*", "xterm"));
    assert!(fnmatch("[!s]term", "xterm"));
    assert!(!fnmatch("[!x]term", "xterm"));
    assert!(fnmatch("xterm-[0-9]*", "xterm-256color"));
    assert!(fnmatch("a\\*b", "a*b"));
    assert!(!fnmatch("a\\*b", "axxb"));
    assert!(fnmatch("xterm-256color", "xterm-256color"));
    assert!(!fnmatch("xterm", "xterm-256color"), "fnmatch anchors the whole name");
}

// ── smcup / rmcup decision ─────────────────────────────────────────────────

#[test]
fn no_overrides_keeps_the_alternate_screen() {
    let none: Vec<String> = Vec::new();
    assert_eq!(alt_screen_caps(&none, "xterm-256color"), AltScreenCaps { smcup: true, rmcup: true });
}

#[test]
fn star_smcup_rmcup_removal_disables_both() {
    let o = elements(",*:smcup@:rmcup@");
    assert_eq!(alt_screen_caps(&o, "xterm-256color"), AltScreenCaps { smcup: false, rmcup: false });
    assert_eq!(alt_screen_caps(&o, ""), AltScreenCaps { smcup: false, rmcup: false });
}

#[test]
fn non_matching_pattern_is_ignored() {
    let o = elements("screen*:smcup@:rmcup@");
    assert_eq!(alt_screen_caps(&o, "xterm-256color"), AltScreenCaps { smcup: true, rmcup: true });
    let o = elements("xterm*:smcup@:rmcup@");
    assert_eq!(alt_screen_caps(&o, ""), AltScreenCaps { smcup: true, rmcup: true },
        "xterm* does not match a console with no TERM");
}

#[test]
fn later_entries_win_and_caps_are_independent() {
    let o = elements("*:smcup@:rmcup@,xterm*:smcup=\\E[?1049h");
    assert_eq!(alt_screen_caps(&o, "xterm-256color"), AltScreenCaps { smcup: true, rmcup: false });
    let o = elements("*:rmcup@");
    assert_eq!(alt_screen_caps(&o, "xterm"), AltScreenCaps { smcup: true, rmcup: false });
    // An empty value emits nothing in tmux, so it counts as removed.
    let o = elements("*:smcup=");
    assert_eq!(alt_screen_caps(&o, "xterm"), AltScreenCaps { smcup: false, rmcup: true });
    // Other capabilities are parsed and ignored.
    let o = elements("xterm*:Tc:RGB:Ss=\\E[%p1%d q");
    assert_eq!(alt_screen_caps(&o, "xterm"), AltScreenCaps { smcup: true, rmcup: true });
}

// ── set -g / set -ga, config and runtime ────────────────────────────────────

#[test]
fn config_set_ga_appends_and_set_g_replaces() {
    let mut a = app();
    assert_eq!(values(&a), vec!["linux*:AX@"], "tmux's default (options-table.c:522)");
    crate::config::parse_config_content(&mut a, "set -ga terminal-overrides ',*:smcup@:rmcup@'\n");
    assert_eq!(values(&a), vec!["linux*:AX@", "*:smcup@:rmcup@"]);
    crate::config::parse_config_content(&mut a, "set -ga terminal-overrides 'xterm*:Tc'\n");
    assert_eq!(values(&a), vec!["linux*:AX@", "*:smcup@:rmcup@", "xterm*:Tc"],
        "-a without a leading comma still adds a new element");
    crate::config::parse_config_content(&mut a, "set -ga terminal-overrides ',screen*:Tc'\n");
    assert_eq!(values(&a).len(), 4);
    crate::config::parse_config_content(&mut a, "set -g terminal-overrides 'foo:smcup@'\n");
    assert_eq!(values(&a), vec!["foo:smcup@"], "set -g replaces the whole array");
    crate::config::parse_config_content(&mut a, "set -gu terminal-overrides\n");
    assert_eq!(values(&a), vec!["linux*:AX@"], "-u restores tmux's default");
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert!(!a.environment.contains_key("terminal-overrides"));
    assert!(!a.user_options.contains_key("terminal-overrides"));
}

#[test]
fn config_sets_and_unsets_one_element_by_index() {
    // tmux's own spelling in a config file; it used to be kept as text.
    let mut a = app();
    crate::config::parse_config_content(
        &mut a,
        "set -g terminal-overrides[5] '*:smcup@:rmcup@'\nset -ga terminal-overrides[5] ':Tc'\n",
    );
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert_eq!(a.terminal_overrides.get(0), Some("linux*:AX@"));
    assert_eq!(a.terminal_overrides.get(5), Some("*:smcup@:rmcup@:Tc"), "-a on an element appends text");
    assert!(!a.user_options.contains_key("terminal-overrides[5]"));
    crate::config::parse_config_content(&mut a, "set -gu terminal-overrides[0]\n");
    let indexes: Vec<u32> = a.terminal_overrides.iter().map(|(index, _)| index).collect();
    assert_eq!(indexes, vec![5], "the other elements keep their indexes");
}

#[test]
fn runtime_set_option_replaces_the_array() {
    let mut a = app();
    crate::server::options::apply_set_option(&mut a, "terminal-overrides", "a:smcup@,b:rmcup@", false).unwrap();
    assert_eq!(values(&a), vec!["a:smcup@", "b:rmcup@"]);
    // `#{terminal-overrides}` joins the elements with a space, as tmux's
    // options_to_string does (options.c:590-614).
    assert_eq!(crate::server::options::get_option_value(&a, "terminal-overrides"), "a:smcup@ b:rmcup@");
    assert_eq!(crate::server::options::get_option_value(&a, "terminal-overrides[1]"), "b:rmcup@");
    assert!(!a.user_options.contains_key("terminal-overrides"));
}

#[test]
fn runtime_index_append_and_unset_follow_tmux() {
    use crate::server::options::{apply_set_option, reset_option_to_default};
    let mut a = app();
    apply_set_option(&mut a, "terminal-overrides", "a:smcup@,b:rmcup@", false).unwrap();
    apply_set_option(&mut a, "terminal-overrides[10]", "e:smcup@", false).unwrap();
    reset_option_to_default(&mut a, "terminal-overrides[0]");
    crate::server::options::set_array_option(&mut a, "terminal-overrides", "f:Tc", true).unwrap();
    let items: Vec<(u32, String)> =
        a.terminal_overrides.iter().map(|(index, entry)| (index, entry.to_string())).collect();
    assert_eq!(
        items,
        vec![(0, "f:Tc".to_string()), (1, "b:rmcup@".to_string()), (10, "e:smcup@".to_string())],
        "-a takes the lowest free index, as tmux 3.7c does after -u [0]",
    );
    let error = apply_set_option(&mut a, "terminal-overrides[x]", "z", false).unwrap_err();
    assert!(error.contains("invalid option"), "{error}");
    // The CLI validates before it sends, so the refusal has to come from here
    // for `psmux set -s terminal-overrides[x] z` to fail as tmux's does.
    let error = crate::server::option_catalog::validate_option_value("terminal-overrides[x]", "z").unwrap_err();
    assert_eq!(error, "invalid option: terminal-overrides[x]");
    assert!(crate::server::option_catalog::validate_option_value("terminal-overrides[3]", "z").is_ok());
    reset_option_to_default(&mut a, "terminal-overrides");
    assert_eq!(values(&a), vec!["linux*:AX@"]);
}

// ── show-options rendering ─────────────────────────────────────────────────

#[test]
fn show_prints_array_lines_like_tmux() {
    use crate::server::options::{apply_set_option, array_option_show_lines as show};
    let mut a = app();
    assert_eq!(show(&a, "terminal-overrides", false), "terminal-overrides[0] linux*:AX@\n");
    apply_set_option(&mut a, "terminal-overrides", "a:smcup@,b:rmcup@", false).unwrap();
    apply_set_option(&mut a, "terminal-overrides[10]", "e:smcup@", false).unwrap();
    assert_eq!(
        show(&a, "terminal-overrides", false),
        "terminal-overrides[0] a:smcup@\nterminal-overrides[1] b:rmcup@\nterminal-overrides[10] e:smcup@\n",
        "each element keeps its own index",
    );
    assert_eq!(show(&a, "terminal-overrides", true), "a:smcup@\nb:rmcup@\ne:smcup@\n");
    assert_eq!(show(&a, "terminal-overrides[10]", false), "terminal-overrides[10] e:smcup@\n");
    assert_eq!(show(&a, "terminal-overrides[10]", true), "e:smcup@\n");
    // An index with no element: tmux prints `terminal-overrides[99] ''`, and
    // an empty line with -v.
    assert_eq!(show(&a, "terminal-overrides[99]", false), "terminal-overrides[99] ''\n");
    assert_eq!(show(&a, "terminal-overrides[99]", true), "\n");
    apply_set_option(&mut a, "terminal-overrides", "a:smcup@, b:rmcup@ ", false).unwrap();
    assert_eq!(
        show(&a, "terminal-overrides", false),
        "terminal-overrides[0] a:smcup@\nterminal-overrides[1] \" b:rmcup@ \"\n",
    );
    apply_set_option(&mut a, "terminal-overrides", "", false).unwrap();
    assert_eq!(show(&a, "terminal-overrides", false), "terminal-overrides\n");
    assert_eq!(show(&a, "terminal-overrides", true), "");
    assert_eq!(show(&a, "status-left", false), "");
}

// ── wire ───────────────────────────────────────────────────────────────────

#[test]
fn render_options_carry_overrides_only_when_set() {
    let mut a = app();
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(json.contains("\"tov\":[\"linux*:AX@\"]"), "the default element rides too: {json}");
    a.terminal_overrides.clear();
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(!json.contains("\"tov\""), "an empty array adds nothing to the frame: {json}");
    a.terminal_overrides.apply_set(None, "*:smcup@:rmcup@", false, ",");
    let json = serde_json::to_string(&crate::server::helpers::expand_status_formats(&a, "").client_render_options).unwrap();
    assert!(json.contains("\"tov\":[\"*:smcup@:rmcup@\"]"), "{json}");
    let back: crate::render_state::ClientRenderOptions = serde_json::from_str(&json).unwrap();
    assert_eq!(back.terminal_overrides.as_deref(), Some(&["*:smcup@:rmcup@".to_string()][..]));
}

// ── client bytes ────────────────────────────────────────────────────────────

#[test]
fn client_start_and_stop_bytes() {
    let _env = crate::util::lock_test_env();
    let saved = std::env::var("TERM").ok();
    std::env::set_var("TERM", "xterm-256color");

    // Default: exactly the alternate screen pair, nothing else.
    arm_client_screen();
    let mut out: Vec<u8> = Vec::new();
    client_screen_start(&mut out, &[]);
    client_screen_start(&mut out, &[]); // later frames are no-ops
    client_screen_stop(&mut out);
    client_screen_stop(&mut out);
    assert_eq!(String::from_utf8_lossy(&out), "\x1b[?1049h\x1b[?1049l");

    // smcup@ rmcup@: never touch the alternate screen; clear on both ends
    // the way tmux's tty_start_tty / tty_stop_tty do.
    arm_client_screen();
    let mut out: Vec<u8> = Vec::new();
    let o = elements(",*:smcup@:rmcup@");
    client_screen_start(&mut out, &o);
    client_screen_stop(&mut out);
    let s = String::from_utf8_lossy(&out).to_string();
    assert!(!s.contains("\x1b[?1049"), "{s:?}");
    assert_eq!(s, "\x1b[H\x1b[2J\x1b[H\x1b[2J");

    // Unset TERM matches `*` as the empty string.
    std::env::remove_var("TERM");
    arm_client_screen();
    let mut out: Vec<u8> = Vec::new();
    client_screen_start(&mut out, &o);
    client_screen_stop(&mut out);
    assert!(!String::from_utf8_lossy(&out).contains("\x1b[?1049"));

    // A client that never drew a frame emits nothing on exit.
    arm_client_screen();
    let mut out: Vec<u8> = Vec::new();
    client_screen_stop(&mut out);
    assert!(out.is_empty());

    match saved {
        Some(v) => std::env::set_var("TERM", v),
        None => std::env::remove_var("TERM"),
    }
}
