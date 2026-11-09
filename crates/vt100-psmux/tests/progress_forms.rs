// OSC 9;4 as tmux reads it (input_osc_9, input.c:3030, and
// screen_set_progress_bar, screen.c:344, at tag 3.8). Every expected pair below
// was measured from tmux 3.8 with `#{pane_pb_state}` and `#{pane_pb_progress}`.
use vt100_psmux::Parser;

fn after(bytes: &[u8]) -> Option<(u8, u8)> {
    let mut parser = Parser::new(24, 80, 0);
    parser.process(bytes);
    parser.screen().progress()
}

/// The state after `1;50` and then the OSC 9 fields in `form`, with BEL.
fn after_fifty(form: &str) -> Option<(u8, u8)> {
    after(format!("\x1b]9;4;1;50\x07\x1b]{form}\x07").as_bytes())
}

#[test]
fn a_state_with_no_value_keeps_the_last_value() {
    for (form, want) in [
        ("9;4;0", (0, 50)),
        ("9;4;1", (1, 50)),
        ("9;4;2", (2, 50)),
        ("9;4;3", (3, 50)),
        ("9;4;4", (4, 50)),
        ("9;4;0;", (0, 50)),
        ("9;4;1;", (1, 50)),
        ("9;4;3;", (3, 50)),
    ] {
        assert_eq!(after_fifty(form), Some(want), "{form}");
    }
}

#[test]
fn st_ends_a_short_form_as_bel_does() {
    assert_eq!(after(b"\x1b]9;4;1;50\x07\x1b]9;4;3\x1b\\"), Some((3, 50)));
    assert_eq!(after(b"\x1b]9;4;1;50\x07\x1b]9;4;0\x1b\\"), Some((0, 50)));
}

#[test]
fn a_value_is_taken_except_for_indeterminate() {
    for (form, want) in [
        ("9;4;0;70", (0, 70)),
        ("9;4;1;70", (1, 70)),
        ("9;4;2;70", (2, 70)),
        ("9;4;4;70", (4, 70)),
        ("9;4;3;70", (3, 50)),
        ("9;4;1;100", (1, 100)),
        ("9;4;1;0", (1, 0)),
        ("9;4;1;007", (1, 7)),
    ] {
        assert_eq!(after_fifty(form), Some(want), "{form}");
    }
}

#[test]
fn anything_else_leaves_the_indicator_as_it_was() {
    for form in [
        "9;4",
        "9;4;",
        "9;4;5",
        "9;4;9;70",
        "9;4;12;70",
        "9;4;x",
        "9;4;x;70",
        "9;4;;70",
        "9;4;1;101",
        "9;4;1;300",
        "9;4;1;1000",
        "9;4;1;x",
        "9;4;1;70x",
        "9;4;1;70;",
        "9;4;1;70;9",
        "9;4;1;;",
    ] {
        assert_eq!(after_fifty(form), Some((1, 50)), "{form}");
    }
}

#[test]
fn a_pane_with_no_value_yet_starts_from_zero() {
    assert_eq!(after(b"\x1b]9;4;3\x07"), Some((3, 0)));
    assert_eq!(after(b"\x1b]9;4;1\x07"), Some((1, 0)));
    assert_eq!(after(b"\x1b]9;4;0\x07"), Some((0, 0)));
    assert_eq!(after(b"\x1b]9;4;3;70\x07"), Some((3, 0)));
}

#[test]
fn a_form_that_is_ignored_on_a_new_pane_sets_nothing() {
    assert_eq!(after(b"\x1b]9;4\x07"), None);
    assert_eq!(after(b"\x1b]9;4;5\x07"), None);
    assert_eq!(after(b"\x1b]9;4;1;101\x07"), None);
}

#[test]
fn ris_keeps_the_indicator() {
    assert_eq!(after(b"\x1b]9;4;1;50\x07\x1bc"), Some((1, 50)));
    assert_eq!(after(b"\x1b]9;4;1;50\x07\x1bc\x1b]9;4;3\x07"), Some((3, 50)));
}

#[test]
fn a_short_form_split_across_reads_is_still_read() {
    let bytes = b"\x1b]9;4;1;50\x07\x1b]9;4;3\x1b\\";
    for cut in 0..=bytes.len() {
        let mut parser = Parser::new(24, 80, 0);
        parser.process(&bytes[..cut]);
        parser.process(&bytes[cut..]);
        assert_eq!(parser.screen().progress(), Some((3, 50)), "cut {cut}");
    }
}

#[test]
fn other_osc_9_is_not_taken_for_progress() {
    assert_eq!(after(b"\x1b]9;hello\x07"), None);
    assert_eq!(after(b"\x1b]9;9;C:\\\x07"), None);
    assert_eq!(after(b"\x1b]94;1;50\x07"), None);
}
