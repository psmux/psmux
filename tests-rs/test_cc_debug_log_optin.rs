// The control client diagnostics log (cc_debug.log) is opt in through
// PSMUX_CC_DEBUG and records the kind of each server line, never the line.
// The client end to end is pinned by tests/test_cc_debug_log_optin.ps1.

use super::cc_debug_line_kind;

#[test]
fn notification_lines_keep_only_their_leading_word() {
    assert_eq!(cc_debug_line_kind(b"%begin 1792403230 1 1\n"), "%begin");
    assert_eq!(cc_debug_line_kind(b"%output %1 hello world\n"), "%output");
    assert_eq!(cc_debug_line_kind(b"%paste-buffer-changed buffer0\n"), "%paste-buffer-changed");
    assert_eq!(cc_debug_line_kind(b"%exit\n"), "%exit");
}

#[test]
fn other_lines_are_reported_as_data() {
    assert_eq!(cc_debug_line_kind(b"hello\n"), "data");
    assert_eq!(cc_debug_line_kind(b"send -H -t demo 6e 6f 74\n"), "data");
    assert_eq!(cc_debug_line_kind(b""), "data");
    assert_eq!(cc_debug_line_kind(b"%\n"), "data");
}

#[test]
fn a_long_word_is_cut_so_no_payload_rides_along() {
    let line = format!("%{}\n", "a".repeat(200));
    assert!(cc_debug_line_kind(line.as_bytes()).len() <= 32);
}
