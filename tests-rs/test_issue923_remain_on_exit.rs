// Issue #923: remain-on-exit takes tmux's five values and remain-on-exit-format
// exists.
//
// tmux 3.8: remain-on-exit is a CHOICE of off, on, failed, key and failed-key
// (options-table.c:93-95, :1699-1708; failed-key is new in 3.8); anything
// else, `1` and `true` included, is `unknown value: X`. `set remain-on-exit`
// with no value swaps off and on and leaves the others alone
// (options.c:1263-1266). remain-on-exit-format is the window and pane option
// written on the dead pane's bottom line (options-table.c:1710-1721,
// server-fn.c:395-417). Measured on tmux 3.8 in WSL.
//
// psmux had remain-on-exit as on/off for the session and the window, `failed`
// only at pane scope, no `key` or `failed-key`, and no remain-on-exit-format.
// The dead pane path itself (a real exit, the key that closes it,
// split-window -k/-m) is covered by tests/test_issue923_remain_on_exit.ps1.

use crate::server::option_catalog::{default_for, validate_option_value, REMAIN_ON_EXIT_FORMAT_DEFAULT};
use crate::server::options::{
    apply_set_option, apply_set_pane_option, get_option_value, reset_option_to_default, toggle_option,
};
use crate::types::{AppState, RemainOnExit};

fn app() -> AppState {
    AppState::new("remain_key".to_string())
}

#[test]
fn the_five_values_and_what_each_keeps() {
    for (name, value) in [
        ("off", RemainOnExit::Off),
        ("on", RemainOnExit::On),
        ("failed", RemainOnExit::Failed),
        ("key", RemainOnExit::Key),
        ("failed-key", RemainOnExit::FailedKey),
    ] {
        assert_eq!(RemainOnExit::parse(name), Some(value));
        assert_eq!(value.as_str(), name);
    }
    assert_eq!(RemainOnExit::parse("true"), None);
    // server-fn.c:385-394: failed and failed-key keep only a nonzero exit,
    // on and key always.
    assert!(!RemainOnExit::Off.keeps(false));
    assert!(RemainOnExit::On.keeps(true));
    assert!(RemainOnExit::Key.keeps(true));
    assert!(RemainOnExit::Failed.keeps(false));
    assert!(!RemainOnExit::Failed.keeps(true));
    assert!(RemainOnExit::FailedKey.keeps(false));
    assert!(!RemainOnExit::FailedKey.keeps(true));
    // server-client.c:1423: a typed key closes key and failed-key panes only.
    let closing: Vec<RemainOnExit> = [
        RemainOnExit::Off,
        RemainOnExit::On,
        RemainOnExit::Failed,
        RemainOnExit::Key,
        RemainOnExit::FailedKey,
    ]
    .into_iter()
    .filter(|value| value.closes_on_key())
    .collect();
    assert_eq!(closing, vec![RemainOnExit::Key, RemainOnExit::FailedKey]);
}

#[test]
fn values_tmux_refuses_are_refused_with_its_wording() {
    // tmux 3.8: `set -g remain-on-exit 1` prints `unknown value: 1`, and so
    // do true, yes and any other word.
    for bad in ["1", "true", "yes", "bogus"] {
        assert_eq!(
            validate_option_value("remain-on-exit", bad),
            Err(format!("unknown value: {}", bad)),
        );
    }
    for good in ["off", "on", "failed", "key", "failed-key"] {
        assert!(validate_option_value("remain-on-exit", good).is_ok(), "{good}");
    }
}

#[test]
fn global_set_show_and_config_take_every_value() {
    let mut a = app();
    for value in ["on", "failed", "key", "failed-key", "off"] {
        apply_set_option(&mut a, "remain-on-exit", value, false).unwrap();
        assert_eq!(get_option_value(&a, "remain-on-exit"), value);
    }
    let mut a = app();
    crate::config::parse_config_content(&mut a, "set -g remain-on-exit key\n");
    assert!(a.config_warnings.is_empty(), "{:?}", a.config_warnings);
    assert_eq!(a.remain_on_exit, RemainOnExit::Key);
    crate::config::parse_config_content(&mut a, "set -g remain-on-exit bogus\n");
    assert!(
        a.config_warnings.iter().any(|w| w.contains("unknown value: bogus")),
        "{:?}",
        a.config_warnings
    );
    assert_eq!(a.remain_on_exit, RemainOnExit::Key, "a refused value changes nothing");
}

#[test]
fn a_bare_set_swaps_off_and_on_and_keeps_the_others() {
    let mut a = app();
    assert!(toggle_option(&mut a, "remain-on-exit"));
    assert_eq!(a.remain_on_exit, RemainOnExit::On);
    assert!(toggle_option(&mut a, "remain-on-exit"));
    assert_eq!(a.remain_on_exit, RemainOnExit::Off);
    for value in [RemainOnExit::Failed, RemainOnExit::Key, RemainOnExit::FailedKey] {
        a.remain_on_exit = value;
        assert!(toggle_option(&mut a, "remain-on-exit"));
        assert_eq!(a.remain_on_exit, value, "options.c:1265 leaves choices past on alone");
    }
}

#[test]
fn the_pane_window_global_chain_resolves_key_too() {
    use crate::tree::keep_dead_pane;
    assert!(keep_dead_pane(Some("key"), RemainOnExit::Off, true));
    assert!(keep_dead_pane(None, RemainOnExit::Key, true));
    assert!(!keep_dead_pane(Some("off"), RemainOnExit::Key, false));
    assert!(!keep_dead_pane(Some("failed"), RemainOnExit::Key, true));
    assert!(keep_dead_pane(Some("failed-key"), RemainOnExit::Off, false));
    assert!(!keep_dead_pane(None, RemainOnExit::FailedKey, true));
    // A pane value that is no value leaves the window's.
    assert!(keep_dead_pane(Some("bogus"), RemainOnExit::Key, true));
}

#[test]
fn the_format_has_tmux_default_and_an_empty_value_stays_empty() {
    let mut a = app();
    assert_eq!(default_for("remain-on-exit-format"), Some(REMAIN_ON_EXIT_FORMAT_DEFAULT));
    assert_eq!(get_option_value(&a, "remain-on-exit-format"), REMAIN_ON_EXIT_FORMAT_DEFAULT);
    assert_eq!(
        REMAIN_ON_EXIT_FORMAT_DEFAULT,
        "Pane is dead (#{?#{!=:#{pane_dead_status},},status #{pane_dead_status},}\
         #{?#{!=:#{pane_dead_signal},},signal #{pane_dead_signal},}, #{t:pane_dead_time})",
        "options-table.c:1713-1718",
    );
    apply_set_option(&mut a, "remain-on-exit-format", "", false).unwrap();
    assert_eq!(get_option_value(&a, "remain-on-exit-format"), "", "empty writes nothing, it is no unset");
    apply_set_option(&mut a, "remain-on-exit-format", "bye #{pane_dead_status}", false).unwrap();
    assert_eq!(get_option_value(&a, "remain-on-exit-format"), "bye #{pane_dead_status}");
    reset_option_to_default(&mut a, "remain-on-exit-format");
    assert_eq!(get_option_value(&a, "remain-on-exit-format"), REMAIN_ON_EXIT_FORMAT_DEFAULT);
}

#[test]
fn pane_scope_takes_key_and_the_format() {
    // No pane exists in a bare AppState, so the target refusal is all a
    // pane write can show here; the value check comes first for a bad value
    // only once a pane is found, which the E2E suite covers.
    let mut a = app();
    let reply = apply_set_pane_option(&mut a, "%99", "remain-on-exit", "key", false, false, false, false);
    assert!(reply.starts_with("ERROR: can't find pane"), "{reply}");
    assert!(crate::server::options::PANE_OPTION_NAMES.contains(&"remain-on-exit-format"));
}

#[test]
fn the_dead_pane_text_goes_on_a_new_bottom_line_cut_to_the_width() {
    use crate::server::helpers::remain_on_exit_bytes;
    // server-fn.c:405-413: scroll region reset, cursor to the last row, a
    // line feed, the text, then the cursor hidden.
    assert_eq!(
        remain_on_exit_bytes(Some("bye 7"), 10, 80),
        "\x1b[r\x1b[10;1H\n\x1b[0m\x1b[39;49mbye 7\x1b[0m\x1b[?25l",
    );
    // An empty format writes nothing but still hides the cursor.
    assert_eq!(remain_on_exit_bytes(None, 10, 80), "\x1b[?25l");

    let mut parser = vt100::Parser::new(5, 10, 0);
    parser.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
    parser.process(remain_on_exit_bytes(Some("#[fg=red]Pane is dead (status 3)"), 5, 10).as_bytes());
    let rows: Vec<String> = parser.screen().rows(0, 10).collect();
    assert_eq!(rows, vec!["two", "three", "four", "five", "Pane is de"], "a new line, cut to 10 cells");
    assert_eq!(parser.screen().cell(4, 0).unwrap().fgcolor(), vt100::Color::Idx(1), "#[fg=red] applies");
    assert!(parser.screen().hide_cursor());
}

#[test]
fn a_style_turned_off_in_the_format_is_off_on_the_dead_line() {
    use crate::server::helpers::remain_on_exit_bytes;
    // Each span is written from a reset: style_sgr only sets what is on, so
    // without the reset B stayed bold after #[bold]A#[default]B.
    for format in ["#[bold]A#[default]B", "#[bold]A#[nobold]B"] {
        let mut parser = vt100::Parser::new(3, 10, 0);
        parser.process(remain_on_exit_bytes(Some(format), 3, 10).as_bytes());
        let a = parser.screen().cell(2, 0).unwrap();
        let b = parser.screen().cell(2, 1).unwrap();
        assert_eq!((a.contents(), a.bold()), ("A", true), "{format}");
        assert_eq!((b.contents(), b.bold()), ("B", false), "{format}");
    }
}

fn window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: crate::types::Node::Split { kind: crate::types::LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
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

#[test]
fn set_w_keeps_the_format_on_the_targeted_window() {
    use crate::server::options::{
        apply_set_window_option, get_window_option_value_for, is_window_scoped_write,
        render_window_options_for, WindowListing,
    };
    // tmux 3.8, `set -w -t s:a remain-on-exit-format WIN`:
    //   show -wv -t s:a -> WIN, show -wv -t s:0 -> "", show -gv -> the default,
    //   show -w -t s:a lists `remain-on-exit-format WIN`.
    // The name was missing from the window table, so the write went to the
    // global store and every window wrote WIN on its dead panes.
    let mut a = app();
    a.window_base_index = 0;
    a.windows.clear();
    a.windows.push(window("zero", 0));
    a.windows.push(window("a", 1));
    assert!(is_window_scoped_write("remain-on-exit-format"));
    let reply = apply_set_window_option(&mut a, "remain_key:a", "remain-on-exit-format", "WIN", false, false, false, false);
    assert_eq!(reply, "");
    assert_eq!(a.windows[1].window_options.get("remain-on-exit-format").map(String::as_str), Some("WIN"));
    assert!(a.windows[0].window_options.get("remain-on-exit-format").is_none());
    assert_eq!(get_option_value(&a, "remain-on-exit-format"), REMAIN_ON_EXIT_FORMAT_DEFAULT, "the global stays");
    assert_eq!(get_window_option_value_for(&a, "remain-on-exit-format", Some(1)), "WIN");
    let local = render_window_options_for(&a, Some(1), WindowListing::Local);
    assert!(local.lines().any(|l| l == "remain-on-exit-format WIN"), "{local}");
    let other = render_window_options_for(&a, Some(0), WindowListing::Local);
    assert!(!other.contains("remain-on-exit-format"), "{other}");
}
