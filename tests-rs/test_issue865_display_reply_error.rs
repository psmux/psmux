// #865: `display-message -p` output that starts with "ERROR" is output, as in
// tmux; only the server's own refusals make the CLI exit 1.

use super::display_reply_error;

#[test]
fn a_printed_message_that_starts_with_error_is_output() {
    assert_eq!(display_reply_error("ERROR: m40\n", "ERROR: m40", false), None);
    assert_eq!(display_reply_error("ERROR m40\n", "ERROR m40", false), None);
    assert_eq!(display_reply_error("ERRORS: 3\n", "ERRORS: #{e}", false), None);
}

#[test]
fn an_expanded_format_that_starts_with_error_is_output() {
    // e.g. -p '#{pane_title}' on a pane titled "ERROR: build failed"
    assert_eq!(display_reply_error("ERROR: build failed\n", "#{pane_title}", false), None);
}

#[test]
fn a_target_that_went_away_is_an_error() {
    assert_eq!(
        display_reply_error("ERROR: can't find window: 99\n", "hi", false),
        Some("can't find window: 99".to_string())
    );
    assert_eq!(
        display_reply_error("ERROR: can't find pane: %9\n", "#{pane_id}", false),
        Some("can't find pane: %9".to_string())
    );
    assert_eq!(
        display_reply_error("ERROR: server is shutting down\n", "#{pane_id}", false),
        Some("server is shutting down".to_string())
    );
}

#[test]
fn a_literal_template_is_output_even_when_it_reads_like_the_error() {
    let t = "ERROR: can't find window: 9";
    assert_eq!(display_reply_error("ERROR: can't find window: 9\n", t, false), None);
    // -l prints the template unexpanded, '#' included.
    let t = "ERROR: can't find #{x}";
    assert_eq!(display_reply_error("ERROR: can't find #{x}\n", t, true), None);
}

#[test]
fn more_than_one_line_is_output() {
    assert_eq!(
        display_reply_error("ERROR: can't find a\nsecond\n", "#{a}\nsecond", false),
        None
    );
}

#[test]
fn ordinary_output_is_output() {
    assert_eq!(display_reply_error("hello\n", "hello", false), None);
    assert_eq!(display_reply_error("\n", "", false), None);
}
