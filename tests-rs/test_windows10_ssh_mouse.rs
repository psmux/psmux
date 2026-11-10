use super::*;

fn feed(parser: &mut VtParser, text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for ch in text.chars() {
        parser.feed(ch, &mut |event| events.push(event));
    }
    events
}

fn mouse_kind(event: &Event) -> Option<MouseEventKind> {
    match event {
        Event::Mouse(mouse) => Some(mouse.kind),
        _ => None,
    }
}

#[test]
fn sgr_wheel_up_and_down_parse_with_zero_based_coordinates() {
    let mut parser = VtParser::new();
    let events = feed(&mut parser, "\x1b[<64;10;5M\x1b[<65;12;7M");

    assert_eq!(events.len(), 2);
    assert_eq!(mouse_kind(&events[0]), Some(MouseEventKind::ScrollUp));
    assert_eq!(mouse_kind(&events[1]), Some(MouseEventKind::ScrollDown));
    match &events[0] {
        Event::Mouse(mouse) => assert_eq!((mouse.column, mouse.row), (9, 4)),
        other => panic!("expected mouse event, got {other:?}"),
    }
}

#[test]
fn repeated_sgr_wheel_reports_remain_distinct_events() {
    let mut parser = VtParser::new();
    let events = feed(
        &mut parser,
        "\x1b[<64;3;2M\x1b[<64;3;2M\x1b[<64;3;2M\x1b[<65;3;2M",
    );
    let kinds: Vec<_> = events.iter().filter_map(mouse_kind).collect();
    assert_eq!(
        kinds,
        vec![
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
        ]
    );
}

#[test]
fn partial_sgr_wheel_waits_for_the_final_byte() {
    let mut parser = VtParser::new();
    assert!(feed(&mut parser, "\x1b[<64;10;").is_empty());

    let events = feed(&mut parser, "5M");
    assert_eq!(events.len(), 1);
    assert_eq!(mouse_kind(&events[0]), Some(MouseEventKind::ScrollUp));
}

#[test]
fn malformed_sgr_mouse_is_dropped_and_parser_recovers() {
    let mut parser = VtParser::new();
    let malformed = feed(&mut parser, "\x1b[<64;10M");
    assert!(malformed
        .iter()
        .all(|event| !matches!(event, Event::Mouse(_))));

    let recovered = feed(&mut parser, "\x1b[<65;4;3M");
    assert_eq!(recovered.len(), 1);
    assert_eq!(mouse_kind(&recovered[0]), Some(MouseEventKind::ScrollDown));
}

#[test]
fn sgr_horizontal_wheel_is_not_read_as_scroll_down() {
    // A trackpad's vertical swipe drifts sideways and the terminal reports
    // that drift as wheel left/right (66/67) between the wheel up reports.
    let mut parser = VtParser::new();
    let events = feed(
        &mut parser,
        "\x1b[<64;3;2M\x1b[<66;3;2M\x1b[<64;3;2M\x1b[<67;3;2M\x1b[<64;3;2M",
    );
    let kinds: Vec<_> = events.iter().filter_map(mouse_kind).collect();
    assert_eq!(
        kinds,
        vec![
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollRight,
            MouseEventKind::ScrollUp,
        ]
    );
}

#[test]
fn x10_wheel_buttons_parse_all_four_directions() {
    // X10 adds 32 to the button and to each 1-based coordinate.
    let mut parser = VtParser::new();
    let events = feed(&mut parser, "\x1b[M`*%\x1b[Ma*%\x1b[Mb*%\x1b[Mc*%");
    let kinds: Vec<_> = events.iter().filter_map(mouse_kind).collect();
    assert_eq!(
        kinds,
        vec![
            MouseEventKind::ScrollUp,
            MouseEventKind::ScrollDown,
            MouseEventKind::ScrollLeft,
            MouseEventKind::ScrollRight,
        ]
    );
}

#[cfg(windows)]
#[test]
fn pipe_mode_override_still_controls_detection() {
    let _lock = crate::util::lock_test_env();
    let previous = std::env::var("PSMUX_PIPE_VT").ok();

    std::env::set_var("PSMUX_PIPE_VT", "1");
    assert!(stdin_is_vt_pipe());
    std::env::set_var("PSMUX_PIPE_VT", "0");
    assert!(!stdin_is_vt_pipe());

    match previous {
        Some(value) => std::env::set_var("PSMUX_PIPE_VT", value),
        None => std::env::remove_var("PSMUX_PIPE_VT"),
    }
}
