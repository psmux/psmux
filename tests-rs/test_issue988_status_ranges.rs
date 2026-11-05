// Issue #988: status line click ranges.
//
// tmux keeps every range type of status-format (style.c style_parse), picks
// the mouse key from the range under the pointer (server-client.c
// server_client_check_mouse) and reports it through #{mouse_status_range},
// #{mouse_status_line}, #{mouse_x} and #{mouse_y} (format.c). psmux kept only
// range=window|N and named every status click MouseDown1Status.

use super::*;
use crate::style::{layout_format_line, parse_range_value, StatusRangeType as R};
use crate::config::{format_key_binding, parse_config_content, populate_default_bindings};
use ratatui::style::Style;

fn app() -> crate::types::AppState {
    let mut app = crate::types::AppState::new("test_session".to_string());
    populate_default_bindings(&mut app);
    // One (paneless) window, so window scoped formats resolve.
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: "w".to_string(),
        id: 0,
        area: ratatui::layout::Rect::new(0, 0, 120, 30),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    });
    app
}

const ISSUE_FORMAT: &str = "#[range=window|0]WINDOW#[norange] #[range=user|alpha]USERA#[norange] #[range=user|beta]USERB#[norange] #[range=left]LEFT#[norange] #[range=right]RIGHT#[norange] #[range=session|$0]SESSION#[norange] #[range=pane|%1]PANE#[norange] #[range=control|3]CONTROL#[norange] NORANGE";

#[test]
fn every_tmux_range_type_parses() {
    assert_eq!(parse_range_value("left"), Some(R::Left));
    assert_eq!(parse_range_value("RIGHT"), Some(R::Right));
    assert_eq!(parse_range_value("window|3"), Some(R::Window(3)));
    assert_eq!(parse_range_value("session|$2"), Some(R::Session(2)));
    assert_eq!(parse_range_value("pane|%7"), Some(R::Pane(7)));
    assert_eq!(parse_range_value("user|alpha"), Some(R::User("alpha".into())));
    assert_eq!(parse_range_value("control|0"), Some(R::Control(0)));
    assert_eq!(parse_range_value("control|9"), Some(R::Control(9)));
}

#[test]
fn ranges_tmux_rejects_are_dropped() {
    for bad in [
        "left|1", "right|x", "window", "window|", "window|x", "session|2", "session|$",
        "pane|7", "pane|%", "control|10", "control|-1", "control", "user", "user|", "bogus|1",
    ] {
        assert_eq!(parse_range_value(bad), None, "{} must be rejected", bad);
    }
}

#[test]
fn user_range_keeps_at_most_15_bytes() {
    // tmux.1: "X must be at most 15 bytes in length" (range_string[16]).
    assert_eq!(parse_range_value("user|0123456789abcdefXYZ"), Some(R::User("0123456789abcde".into())));
}

#[test]
fn issue_format_lays_out_every_range_at_its_word() {
    let layout = layout_format_line(ISSUE_FORMAT, 120, Style::default());
    let got: Vec<(R, u16, u16)> = layout.ranges.clone();
    assert_eq!(got, vec![
        (R::Window(0), 0, 6),
        (R::User("alpha".into()), 7, 12),
        (R::User("beta".into()), 13, 18),
        (R::Left, 19, 23),
        (R::Right, 24, 29),
        (R::Session(0), 30, 37),
        (R::Pane(1), 38, 42),
        (R::Control(3), 43, 50),
    ]);
}

#[test]
fn a_new_range_ends_the_one_before_it() {
    // tmux format_draw: a change of range ends the current one, so two
    // ranges with no norange between them are two click targets.
    let layout = layout_format_line("#[range=window|0]A#[range=window|1]B", 20, Style::default());
    assert_eq!(layout.ranges, vec![(R::Window(0), 0, 1), (R::Window(1), 1, 2)]);
}

fn cells() -> Vec<StatusRangeCell> {
    layout_format_line(ISSUE_FORMAT, 120, Style::default())
        .ranges
        .into_iter()
        .map(|(r, s, e)| (0u16, r, s, e))
        .collect()
}

#[test]
fn a_click_resolves_the_issue_table() {
    // (column, key location, #{mouse_status_range}, target), from the
    // issue's tmux 3.8 output.
    let table = [
        (1, "Status", Some("window"), ":0"),
        (8, "Status", Some("alpha"), ":"),
        (14, "Status", Some("beta"), ":"),
        (20, "StatusLeft", Some("left"), ":"),
        (25, "StatusRight", Some("right"), ":"),
        (31, "Status", Some("session"), "$0"),
        (39, "Status", Some("pane"), "%1"),
        (44, "Control3", Some("control"), ":"),
        (52, "StatusDefault", None, ":"),
    ];
    let ranges = cells();
    for (col, loc, range, target) in table {
        let hit = status_hit(&ranges, 0, 0, col, &[0]).expect("a key");
        assert_eq!(hit.location(), loc, "column {}", col);
        assert_eq!(hit.range.as_ref().map(|r| r.format_name()), range, "column {}", col);
        assert_eq!(hit.target, target, "column {}", col);
        assert_eq!((hit.x, hit.line), (col, 0));
    }
}

#[test]
fn a_window_range_naming_no_window_gives_no_key() {
    let ranges = vec![(0u16, R::Window(7), 0, 3)];
    assert_eq!(status_hit(&ranges, 0, 0, 1, &[0, 1]), None);
}

#[test]
fn a_bottom_status_line_reports_its_own_line_number() {
    // Two status lines at rows 28 and 29: a click on row 29 is line 1.
    let ranges = vec![(29u16, R::User("b".into()), 0, 5)];
    let hit = status_hit(&ranges, 28, 29, 2, &[]).unwrap();
    assert_eq!((hit.x, hit.line), (2, 1));
    assert_eq!(hit.range, Some(R::User("b".into())));
}

#[test]
fn the_status_mouse_line_carries_the_range_as_one_hex_word() {
    let hit = StatusHit { range: Some(R::User("a b".into())), target: ":".into(), x: 4, line: 0 };
    assert_eq!(hit.command("MouseDown1Status", None), "status-mouse MouseDown1Status - : 4 0 612062\n");
    let none = StatusHit { range: None, target: ":".into(), x: 9, line: 1 };
    assert_eq!(none.command("WheelUpStatusDefault", Some("WheelUpStatus")),
        "status-mouse WheelUpStatusDefault WheelUpStatus : 9 1 -\n");
}

#[test]
fn control_keys_are_mouse_keys() {
    for name in ["MouseDown1Control0", "MouseDown1Control3", "MouseUp2Control9",
                 "DoubleClick1Control5", "WheelUpControl1", "M-MouseDown3Control8"] {
        let key = parse(name).unwrap_or_else(|| panic!("{} must parse", name));
        assert_eq!(format_key_binding(&key), name);
        assert!(is_status_char(match key.0 { crossterm::event::KeyCode::Char(c) => c, _ => unreachable!() }));
    }
    assert!(parse("MouseDown1Control10").is_none());
    // The largest mouse key is still not a user key (they start right after
    // the mouse keys, key_base::user_base()).
    let top = parse("MouseMoveControl9").unwrap();
    let crossterm::event::KeyCode::Char(c) = top.0 else { unreachable!() };
    assert!(!crate::user_keys::is_user_char(c));
    assert!(parse("MouseDown11Control9").map_or(false, |k| !crate::user_keys::is_user_key(&k)));
    // The keys that existed before keep their names.
    for name in ["MouseDown1Pane", "TripleClick1Pane", "MouseDragEnd1Border", "WheelDownStatus", "MouseMoveEmpty"] {
        assert_eq!(format_key_binding(&parse(name).unwrap()), name);
    }
}

#[test]
fn bound_status_keys_reach_the_client_and_unbound_ones_do_not() {
    let mut app = app();
    assert_eq!(client_overrides(&app), "{}", "nothing bound, nothing sent");
    parse_config_content(&mut app, "bind -n MouseDown1StatusLeft set -g @a 1\nbind -n M-MouseDown3StatusRight set -g @b 1\nbind -n MouseDown1Control3 set -g @c 1\nbind -n MouseUp1Status set -g @d 1\n");
    let o = client_overrides(&app);
    for name in ["MouseDown1StatusLeft", "M-MouseDown3StatusRight", "MouseDown1Control3", "MouseUp1Status"] {
        assert!(o.contains(&format!("\"{}\":\"UU\"", name)), "{} missing from {}", name, o);
    }
    assert!(!o.contains("MouseDown1StatusDefault"));
}

fn run(app: &mut crate::types::AppState, name: &str, fallback: Option<&str>, target: &str, x: u16, range: Option<&str>) {
    crate::window_ops::handle_status_mouse(app, &crate::window_ops::StatusMouse {
        name: name.into(),
        fallback: fallback.map(|s| s.into()),
        target: target.into(),
        x,
        line: 0,
        range: range.map(|s| s.into()),
    });
}

fn opt(app: &crate::types::AppState, name: &str) -> String {
    crate::format::expand_format(&format!("#{{{}}}", name), app)
}

#[test]
fn a_status_binding_sees_the_mouse_format_variables() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n MouseDown1StatusLeft set -gF @hit 'L [#{mouse_status_range}] [#{mouse_status_line}] [#{mouse_x}] [#{mouse_y}]'\n");
    run(&mut app, "MouseDown1StatusLeft", None, ":", 20, Some("left"));
    assert_eq!(opt(&app, "@hit"), "L [left] [0] [20] [0]");
}

#[test]
fn an_unbound_range_key_does_nothing_but_its_fallback_can_run() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n MouseDown1Status set -gF @hit 'S [#{mouse_status_range}]'\n");
    // With status-format: StatusDefault unbound, nothing (tmux).
    run(&mut app, "MouseDown1StatusDefault", None, ":", 52, None);
    assert_eq!(opt(&app, "@hit"), "");
    // psmux's own layout passes the Status key as the fallback.
    run(&mut app, "MouseDown1StatusDefault", Some("MouseDown1Status"), ":", 52, None);
    assert_eq!(opt(&app, "@hit"), "S []");
}

#[test]
fn a_session_or_pane_range_naming_nothing_gives_no_key() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n MouseDown1Status set -gF @hit 'ran'\n");
    let other = format!("${}", app.session_id + 1);
    run(&mut app, "MouseDown1Status", None, &other, 1, Some("session"));
    assert_eq!(opt(&app, "@hit"), "");
    run(&mut app, "MouseDown1Status", None, "%999", 1, Some("pane"));
    assert_eq!(opt(&app, "@hit"), "");
    let own = format!("${}", app.session_id);
    run(&mut app, "MouseDown1Status", None, &own, 1, Some("session"));
    assert_eq!(opt(&app, "@hit"), "ran");
}

#[test]
fn mouse_variables_are_empty_without_a_mouse_event() {
    // tmux's callbacks return NULL when the format has no mouse event.
    let app = app();
    for v in ["mouse_x", "mouse_y", "mouse_status_line", "mouse_status_range", "mouse_line", "mouse_word"] {
        assert_eq!(opt(&app, v), "", "{}", v);
    }
}

#[test]
fn key_location_and_format_name_match_tmux() {
    assert_eq!(R::Left.key_location(), "StatusLeft");
    assert_eq!(R::Right.key_location(), "StatusRight");
    assert_eq!(R::Control(4).key_location(), "Control4");
    for r in [R::Window(1), R::Session(1), R::Pane(1), R::User("x".into())] {
        assert_eq!(r.key_location(), "Status");
    }
    assert_eq!(R::Window(1).format_name(), "window");
    assert_eq!(R::Session(1).format_name(), "session");
    assert_eq!(R::Pane(1).format_name(), "pane");
    assert_eq!(R::Control(1).format_name(), "control");
    assert_eq!(R::User("tab+".into()).format_name(), "tab+");
}
