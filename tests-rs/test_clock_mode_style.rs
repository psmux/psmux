// clock-mode-style: the time format of the clock in clock mode. Every
// expectation is what tmux 3.8 does (options-table.c:38-40 and 1330-1336,
// window-clock.c:249-263 and 267-276 at tag 3.8), measured where it says so,
// except the default, which psmux keeps at the format its clock always had.

use super::*;
use crate::commands::execute_command_string;
use crate::server::options::{get_option_value, get_window_option_value_for};
use crate::types::AppState;

fn make_window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: format!("w{id}"),
        id,
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
    }
}

fn app2() -> AppState {
    let mut app = AppState::new("clock_style".to_string());
    app.window_base_index = 0;
    app.windows.push(make_window(0));
    app.windows.push(make_window(1));
    app
}

#[test]
fn the_option_takes_tmux_four_values_as_they_are() {
    for value in CHOICES {
        assert_eq!(
            crate::server::option_catalog::validate_option_value(OPTION, value),
            Ok(()),
            "{value}",
        );
        assert_eq!(ClockStyle::parse(value).map(ClockStyle::name), Some(*value));
    }
    // Measured: `set -g clock-mode-style 13` and `24-with-second` say
    // `unknown value: 13` and `unknown value: 24-with-second`.
    for value in ["13", "24-with-second", " 24", "12 ", "on", ""] {
        assert_eq!(
            crate::server::option_catalog::validate_option_value(OPTION, value),
            Err(format!("unknown value: {value}")),
            "{value:?}",
        );
        assert_eq!(ClockStyle::parse(value), None, "{value:?}");
    }
}

#[test]
fn unset_it_is_psmux_default_with_seconds() {
    let app = app2();
    assert_eq!(get_option_value(&app, OPTION), "24-with-seconds");
    assert_eq!(get_window_option_value_for(&app, OPTION, Some(0)), "24-with-seconds");
    assert_eq!(ClockStyle::default(), ClockStyle::H24Seconds);
    assert_eq!(crate::server::option_catalog::default_for(OPTION), Some(DEFAULT_NAME));
    // An ordinary frame carries no style.
    assert_eq!(frame_value(&app), None);
}

#[test]
fn set_and_unset_go_through_the_global_value() {
    let mut app = app2();
    execute_command_string(&mut app, "set -g clock-mode-style 12").unwrap();
    assert_eq!(get_option_value(&app, OPTION), "12");
    assert_eq!(frame_value(&app), Some(ClockStyle::H12));
    // A refused value leaves the one before.
    let _ = execute_command_string(&mut app, "set -g clock-mode-style 13");
    assert_eq!(get_option_value(&app, OPTION), "12");
    execute_command_string(&mut app, "set -gu clock-mode-style").unwrap();
    assert_eq!(get_option_value(&app, OPTION), "24-with-seconds");
    // Unset is a missing entry, not the default written back.
    assert_eq!(app.user_options.get(OPTION), None);
    assert_eq!(frame_value(&app), None);
}

#[test]
fn a_window_value_wins_for_that_window() {
    // Measured: `set -g clock-mode-style 24` then `set -w clock-mode-style 12`
    // on the window draws `12:47 PM` style digits there.
    let mut app = app2();
    execute_command_string(&mut app, "set -g clock-mode-style 24").unwrap();
    execute_command_string(&mut app, "set -w -t :1 clock-mode-style 12-with-seconds").unwrap();
    assert_eq!(get_window_option_value_for(&app, OPTION, Some(1)), "12-with-seconds");
    assert_eq!(get_window_option_value_for(&app, OPTION, Some(0)), "24");
    assert_eq!(get_option_value(&app, OPTION), "24");
    app.active_idx = 1;
    assert_eq!(frame_value(&app), Some(ClockStyle::H12Seconds));
    app.active_idx = 0;
    assert_eq!(frame_value(&app), Some(ClockStyle::H24));
}

#[test]
fn the_frame_gets_the_style_only_when_one_is_set() {
    let mut app = app2();
    let mut buf = String::from("{\"layout\":1}");
    append_frame_json(&app, &mut buf);
    assert_eq!(buf, "{\"layout\":1}");
    app.user_options.insert(OPTION.to_string(), "12".to_string());
    append_frame_json(&app, &mut buf);
    assert_eq!(buf, "{\"layout\":1,\"clock_style\":\"12\"}");
    // A value that is none of the four, from an old config, is left out.
    let mut buf = String::from("{}");
    app.user_options.insert(OPTION.to_string(), "13".to_string());
    append_frame_json(&app, &mut buf);
    assert_eq!(buf, "{}");
}

#[test]
fn the_text_is_tmux_strftime_text() {
    // window-clock.c:249-263: `%l:%M ` or `%l:%M:%S ` and AM or PM; `%H:%M`
    // or `%H:%M:%S`. Measured at 13:47:26: ` 1:47 PM`, `13:47`,
    // ` 1:47:26 PM`, `13:47:29`.
    let cases = [
        ((0, 5, 9), ["12:05 AM", "00:05", "12:05:09 AM", "00:05:09"]),
        ((9, 5, 9), [" 9:05 AM", "09:05", " 9:05:09 AM", "09:05:09"]),
        ((12, 0, 0), ["12:00 PM", "12:00", "12:00:00 PM", "12:00:00"]),
        ((13, 47, 26), [" 1:47 PM", "13:47", " 1:47:26 PM", "13:47:26"]),
        ((23, 59, 59), ["11:59 PM", "23:59", "11:59:59 PM", "23:59:59"]),
    ];
    let styles = [ClockStyle::H12, ClockStyle::H24, ClockStyle::H12Seconds, ClockStyle::H24Seconds];
    for ((h, m, s), want) in cases {
        for (style, want) in styles.iter().zip(want) {
            assert_eq!(time_text(*style, h, m, s), want, "{style:?} at {h}:{m}:{s}");
        }
    }
}

fn draw(text: &str, w: u16, h: u16) -> Vec<String> {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        crate::client::render_clock_text(f, f.area(), text, ratatui::style::Color::Blue, None);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>())
        .collect()
}

#[test]
fn the_big_clock_widths_follow_the_style() {
    use crate::client::clock_text_width;
    // Digits are 3 wide and the colon 1, with one column between glyphs.
    assert_eq!(clock_text_width("13:47"), 4 + 4 + 2 + 4 + 3);
    assert_eq!(clock_text_width("13:47:26"), 4 + 4 + 2 + 4 + 4 + 2 + 4 + 3);
    // A space is a digit wide; A and P 3 and M 5, as tmux's M.
    assert_eq!(clock_text_width(" 1:47 PM"), 4 + 4 + 2 + 4 + 4 + 4 + 4 + 5);
}

#[test]
fn twelve_hour_digits_end_with_am_or_pm() {
    let rows = draw(" 1:47 PM", 40, 7);
    let rows: Vec<&str> = rows.iter().map(|r| r.trim_end()).collect();
    // Centred: 40 columns, the text 31 wide, so it starts at column 4. The
    // leading space is a blank glyph, so the `1` glyph starts at column 8.
    assert_eq!(rows[0], "");
    assert_eq!(rows[1], "          #   # # ###     ### #   #");
    assert_eq!(rows[2], "          # # # #   #     # # ## ##");
    assert_eq!(rows[3], "          #   ###   #     ### # # #");
    assert_eq!(rows[4], "          # #   #   #     #   #   #");
    assert_eq!(rows[5], "          #     #   #     #   #   #");
    assert_eq!(rows[6], "");
}

#[test]
fn a_small_pane_shows_the_text_centred_as_tmux() {
    // Measured: in a 14 x 3 pane `13:47` starts at column 5, ` 1:47 PM` at
    // column 3 (its own leading space at 3), `13:47:29` at 3.
    let rows = draw("13:47", 14, 3);
    assert_eq!(rows[1], "     13:47    ");
    let rows = draw(" 1:47 PM", 14, 3);
    assert_eq!(rows[1], "    1:47 PM   ");
    let rows = draw("13:47:29", 14, 3);
    assert_eq!(rows[1], "   13:47:29   ");
    assert_eq!(rows[0].trim(), "");
    assert_eq!(rows[2].trim(), "");
}
