// #795 follow up: user options carry the scope of their FLAGS.
//
// tmux 3.4 (measured, `tmux -L r795sc`):
//
//     setw -t s:0 @w win0
//       show -w -t s:0          -> "@w win0"
//       show -wv -t s:0 @w      -> "win0"
//       show -wqv -t s:1 @w     -> ""          (the other window has none)
//       show -qv -t s @w        -> ""          (the session has none)
//       display -t s:1 #{@w}    -> ""
//     set -t s @w sess          -> s:0 still "win0", s:1 "sess"
//     set -gw @w gwin           -> s:1 "gwin"  (global window beats session)
//     set -u -t s @w            -> s:1 "gwin"
//     set -p -t s:1 @w pane1    -> "pane1"; set -w -t s:1 @w win1 -> "pane1"
//     set -u -p -t s:1 @w       -> "win1"
//     set -u -w -t s:0 @w       -> show -w -t s:0 empty, #{@w} "gwin"
//     show -gw @w               -> "@w gwin"
//     show -wA -t s:0           -> no @w line (only the window's OWN user
//                                  options are listed, never inherited ones)
//
// psmux stored every `@` write in the session store, so `show -w` was empty
// and every window saw the value.

use super::*;
use crate::types::{AppState, LayoutKind, Node};

fn make_window(name: &str, id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
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

fn two_windows() -> AppState {
    let mut app = AppState::new("s".to_string());
    app.window_base_index = 0;
    app.windows.clear();
    app.windows.push(make_window("zero", 0));
    app.windows.push(make_window("one", 1));
    app.active_idx = 0;
    app
}

fn fmt(app: &AppState, window: usize, name: &str) -> String {
    pane_user_option(app, app.windows.get(window), None, name).unwrap_or_default()
}

#[test]
fn user_options_take_the_scope_of_the_window_flag() {
    assert!(is_window_scoped_write("@w"));
    assert!(is_global_window_user_write(true, true, "@w"));
    assert!(!is_global_window_user_write(false, true, "@w"));
    assert!(!is_global_window_user_write(true, true, "remain-on-exit"));
}

#[test]
fn setw_user_option_lands_on_the_target_window_only() {
    let mut app = two_windows();
    assert_eq!(apply_set_window_option(&mut app, "s:0", "@w", "win0", false, false, false, false), "");
    assert_eq!(window_option_value_for_target(&app, "@w", "s:0"), "win0");
    assert_eq!(window_option_value_for_target(&app, "@w", "s:1"), "");
    assert!(app.user_options.get("@w").is_none(), "the session store must not see a window write");
    assert_eq!(fmt(&app, 0, "@w"), "win0");
    assert_eq!(fmt(&app, 1, "@w"), "");
    let listing = render_window_options_for(&app, Some(0), WindowListing::Local);
    assert!(listing.contains("@w win0\n"), "show -w lists it: {listing:?}");
    let other = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert!(!other.contains("@w"), "the other window lists nothing: {other:?}");
}

#[test]
fn lookup_order_is_pane_window_global_window_session() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:0", "@w", "win0", false, false, false, false);
    app.user_options.insert("@w".into(), "sess".into());
    assert_eq!(fmt(&app, 0, "@w"), "win0", "window beats session");
    assert_eq!(fmt(&app, 1, "@w"), "sess");
    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@w", "gwin", false, false, false, false);
    assert_eq!(fmt(&app, 1, "@w"), "gwin", "global window beats session like tmux");
    assert_eq!(fmt(&app, 0, "@w"), "win0");
    app.user_options.remove("@w");
    assert_eq!(fmt(&app, 1, "@w"), "gwin");
    // set -u -w falls back to the global window table
    apply_set_window_option(&mut app, "s:0", "@w", "", true, false, false, false);
    assert_eq!(window_option_value_for_target(&app, "@w", "s:0"), "");
    assert_eq!(fmt(&app, 0, "@w"), "gwin");
    assert_eq!(window_option_value_for_target(&app, "@w", GLOBAL_WINDOW_TABLE), "gwin");
    // no window in hand: global window, then session
    assert_eq!(pane_user_option(&app, None, None, "@w").as_deref(), Some("gwin"));
}

#[test]
fn global_window_listing_and_inherited_marker() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@g", "gv", false, false, false, false);
    apply_set_window_option(&mut app, "s:1", "@w", "win 1", false, false, false, false);
    let global = render_window_options_for(&app, None, WindowListing::Global);
    assert!(global.contains("@g gv\n"), "{global:?}");
    // tmux 3.4: `show -wA` lists only the window's OWN user options.
    let all = render_window_options_for(&app, Some(1), WindowListing::LocalAndInherited);
    assert!(all.contains("@w \"win 1\"\n"), "{all:?}");
    assert!(!all.contains("@g"), "{all:?}");
    let own = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert!(!own.contains("@g"), "{own:?}");
}

#[test]
fn append_and_only_if_unset_on_window_and_global_window_tables() {
    let mut app = two_windows();
    app.user_options.insert("@w".into(), "sess".into());
    apply_set_window_option(&mut app, "s:0", "@w", "a", false, true, false, false);
    assert_eq!(window_option_value_for_target(&app, "@w", "s:0"), "a", "append reads the window's own value");
    apply_set_window_option(&mut app, "s:0", "@w", "b", false, true, false, false);
    assert_eq!(window_option_value_for_target(&app, "@w", "s:0"), "ab");
    let reply = apply_set_window_option(&mut app, "s:0", "@w", "c", false, false, true, false);
    assert!(reply.contains("already set"), "{reply:?}");
    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@g", "x", false, true, false, false);
    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@g", "y", false, true, false, false);
    assert_eq!(app.global_window_user_options.get("@g").map(String::as_str), Some("xy"));
    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@g", "", true, false, false, false);
    assert!(app.global_window_user_options.get("@g").is_none());
}

#[test]
fn display_message_unknown_flags_are_refused_like_tmux() {
    use crate::cli::unknown_flag;
    assert_eq!(unknown_flag("display-message", &["-p", "-x=foo"]), Some('x'));
    assert_eq!(unknown_flag("display-message", &["-p", "-x", "hello"]), Some('x'));
    assert_eq!(unknown_flag("display", &["-px"]), Some('x'));
    // a message after -- or after the first operand is data
    assert_eq!(unknown_flag("display-message", &["-p", "--", "-x=foo"]), None);
    assert_eq!(unknown_flag("display-message", &["-p", "hello", "-x"]), None);
    // a value-taking flag's value is not a flag
    assert_eq!(unknown_flag("display-message", &["-t", "-x", "-p"]), None);
    assert_eq!(unknown_flag("display-message", &["-pt", "s", "-v", "hi"]), None);
    assert_eq!(unknown_flag("display-message", &["-d", "500", "-aCINlvF", "#S"]), None);
}

// ── show-options on a user option nothing holds (tmux 3.4 measured) ─────────
//
//     show -t s @unset      -> rc 1 "invalid option: @unset"  (-q: rc 0, silent)
//     set -t s @empty ''    -> show @empty prints "@empty ''"
//     set -gw @g x          -> show -w @g: invalid; show -wA @g: "@g* x"

#[test]
fn a_user_option_nothing_holds_is_not_found_in_any_table() {
    let mut app = two_windows();
    assert_eq!(lookup_user_option(&app, "@unset", "", UserOptionTable::Session), None);
    assert_eq!(lookup_user_option(&app, "@unset", "s:0", UserOptionTable::Window { inherited: false }), None);
    assert_eq!(lookup_user_option(&app, "@unset", "s:0", UserOptionTable::Window { inherited: true }), None);
    assert_eq!(lookup_user_option(&app, "@unset", "", UserOptionTable::GlobalWindow), None);

    app.user_options.insert("@empty".into(), String::new());
    assert_eq!(
        lookup_user_option(&app, "@empty", "", UserOptionTable::Session),
        Some((String::new(), false)),
        "an empty value is still set",
    );
    assert_eq!(render_user_option_line("@empty", "", false, false), "@empty ''\n");
    assert_eq!(render_user_option_line("@empty", "", false, true), "\n");
    assert_eq!(render_user_option_line("@k", "a b", false, false), "@k \"a b\"\n");

    apply_set_window_option(&mut app, GLOBAL_WINDOW_TABLE, "@g", "x", false, false, false, false);
    assert_eq!(lookup_user_option(&app, "@g", "s:0", UserOptionTable::Window { inherited: false }), None);
    assert_eq!(
        lookup_user_option(&app, "@g", "s:0", UserOptionTable::Window { inherited: true }),
        Some(("x".to_string(), true)),
    );
    assert_eq!(render_user_option_line("@g", "x", true, false), "@g* x\n");
    assert_eq!(lookup_user_option(&app, "@g", "", UserOptionTable::GlobalWindow), Some(("x".to_string(), false)));
}

// ── display-message flags (cmd-display-message.c, tmux 3.4 measured) ───────

#[test]
fn display_message_flags_are_flags_never_message_text() {
    use crate::cli::parse_display_message_args;
    let p = parse_display_message_args(&["-p", "-t", "s", "-l", "#{session_name} %H"]).unwrap();
    assert!(p.print && p.literal);
    assert_eq!(p.target.as_deref(), Some("s"));
    assert_eq!(p.template().as_deref(), Some("#{session_name} %H"));

    let p = parse_display_message_args(&["-pv", "#S"]).unwrap();
    assert!(p.print && p.verbose);
    let p = parse_display_message_args(&["-aCN", "-d", "500"]).unwrap();
    assert!(p.list_all && p.no_freeze && p.ignore_keys);
    assert_eq!(p.delay.as_deref(), Some("500"));
    assert_eq!(p.template(), None);

    // -F takes a value; -F and an argument together is tmux's error
    let p = parse_display_message_args(&["-p", "-F", "#{session_name}"]).unwrap();
    assert_eq!(p.template().as_deref(), Some("#{session_name}"));
    let p = parse_display_message_args(&["-pF#S"]).unwrap();
    assert_eq!(p.format.as_deref(), Some("#S"));
    assert_eq!(
        parse_display_message_args(&["-p", "-F", "#S", "extra"]).unwrap_err(),
        "only one of -F or argument must be given",
    );
    assert_eq!(parse_display_message_args(&["-p", "-x", "hi"]).unwrap_err(), "unknown flag -x");
    assert_eq!(parse_display_message_args(&["-p", "-t"]).unwrap_err(), "-t expects an argument");

    // after -- and after the first operand everything is message
    let p = parse_display_message_args(&["-p", "--", "-v", "-l"]).unwrap();
    assert!(!p.verbose && !p.literal);
    assert_eq!(p.template().as_deref(), Some("-v -l"));
    let p = parse_display_message_args(&["-p", "a", "-l"]).unwrap();
    assert!(!p.literal);
    assert_eq!(p.template().as_deref(), Some("a -l"));
}

#[test]
fn display_all_variables_are_sorted_and_unique() {
    let vars = crate::cli::DISPLAY_ALL_VARIABLES;
    assert!(vars.len() > 100);
    assert!(vars.windows(2).all(|w| w[0] < w[1]), "sorted like tmux's -a listing");
    for name in ["session_name", "window_index", "pane_id", "cursor_x", "pane_current_path"] {
        assert!(vars.contains(&name), "{name}");
    }
}

#[test]
fn display_verbose_trace_brackets_the_result() {
    assert_eq!(
        crate::util::display_verbose_trace("#{session_name}", "s"),
        "# expanding format: #{session_name}\n# result is: s\n",
    );
}

#[test]
fn a_key_press_closes_the_status_message_unless_it_was_shown_with_n() {
    use crate::server::helpers::clear_status_message_on_key;
    let mut app = two_windows();
    let now = std::time::Instant::now();
    app.status_message = Some(("plain".into(), now, None));
    clear_status_message_on_key(&mut app);
    assert!(app.status_message.is_none(), "a key closes an ordinary message");

    let shown = std::time::Instant::now();
    app.status_message = Some(("sticky".into(), shown, Some(5000)));
    app.status_message_ignore_keys = Some(shown);
    clear_status_message_on_key(&mut app);
    assert_eq!(app.status_message.as_ref().map(|m| m.0.as_str()), Some("sticky"), "-N ignores keys");

    // a later message is not sticky just because an earlier one was
    std::thread::sleep(std::time::Duration::from_millis(2));
    app.status_message = Some(("later".into(), std::time::Instant::now(), None));
    clear_status_message_on_key(&mut app);
    assert!(app.status_message.is_none());
}
