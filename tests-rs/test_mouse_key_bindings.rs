// tmux mouse key bindings: WheelUpPane and friends are real keys in the key
// tables (bind, unbind, list-keys), and a wheel notch over a pane resolves
// through them the way tmux's server_client_key_callback does.

use super::*;
use crate::config::{
    format_key_binding, list_keys_entries, normalize_key_for_binding, parse_config_content,
    parse_key_name, parse_key_string, populate_default_bindings,
};
use crossterm::event::{KeyCode, KeyModifiers};

fn app() -> crate::types::AppState {
    let mut app = crate::types::AppState::new("test_session".to_string());
    populate_default_bindings(&mut app);
    app
}

fn wheel(up: bool, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    wheel_pane_key(up, mods)
}

#[test]
fn every_tmux_mouse_name_round_trips() {
    for name in [
        "MouseDown1Pane", "MouseUp3Status", "MouseDrag1Border", "MouseDragEnd1Pane",
        "SecondClick1Pane", "DoubleClick1Pane", "TripleClick1Pane", "MouseDown11StatusRight",
        "WheelUpPane", "WheelDownPane", "WheelUpStatus", "WheelDownStatus",
        "MouseMovePane", "MouseDown3StatusLeft", "MouseDown1StatusDefault",
        "MouseDown1ScrollbarUp", "MouseDrag1ScrollbarSlider", "MouseDown1ScrollbarDown",
        "MouseDrag1Empty",
    ] {
        let key = parse(name).unwrap_or_else(|| panic!("{} must parse", name));
        assert_eq!(key.1, KeyModifiers::NONE, "{}", name);
        assert_eq!(format_key_binding(&key), name);
    }
}

#[test]
fn modifiers_and_case_follow_tmux() {
    let k = parse("C-WheelUpPane").unwrap();
    assert_eq!(k.1, KeyModifiers::CONTROL);
    assert_eq!(format_key_binding(&k), "C-WheelUpPane");
    let k = parse("c-m-s-wheeluppane").unwrap();
    assert_eq!(k.1, KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(format_key_binding(&k), "C-M-S-WheelUpPane");
    // MouseDragEnd is not read as MouseDrag + "End1Pane".
    assert_eq!(format_key_binding(&parse("mousedragend1pane").unwrap()), "MouseDragEnd1Pane");
}

#[test]
fn not_mouse_names_are_rejected() {
    for bad in ["MouseDown0Pane", "MouseDown12Pane", "WheelUp1Pane", "MouseDownPane",
                "WheelUpWindow", "MouseDown1", "Up", "a", "C-a", "F1"] {
        assert!(parse(bad).is_none(), "{} must not be a mouse key", bad);
    }
}

#[test]
fn both_key_parsers_accept_mouse_names_and_keep_modifiers() {
    for name in ["WheelUpPane", "S-WheelUpPane", "C-M-WheelDownPane", "M-MouseDrag1Border"] {
        let a = parse_key_name(name).map(normalize_key_for_binding);
        let b = parse_key_string(name).map(normalize_key_for_binding);
        assert_eq!(a, parse(name), "parse_key_name {}", name);
        assert_eq!(b, parse(name), "parse_key_string {}", name);
    }
    // Keyboard names are untouched.
    assert_eq!(parse_key_name("C-a"), Some((KeyCode::Char('a'), KeyModifiers::CONTROL)));
}

#[test]
fn defaults_resolve_to_the_builtin_handler() {
    let app = app();
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), false), Dispatch::Builtin);
    // tmux has no root WheelDownPane: the event goes to the pane.
    assert_eq!(resolve(&app, wheel(false, KeyModifiers::NONE), false), Dispatch::Forward);
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), true), Dispatch::Builtin);
    assert_eq!(resolve(&app, wheel(false, KeyModifiers::NONE), true), Dispatch::Builtin);
}

#[test]
fn ctrl_wheel_is_unbound_like_tmux() {
    let app = app();
    // Root: forwarded to the pane (reaches only a program with mouse on).
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::CONTROL), false), Dispatch::Forward);
    // Copy mode: not in copy-mode, not in root, so tmux drops it.
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::CONTROL), true), Dispatch::Drop);
}

#[test]
fn a_user_binding_wins_in_root_and_copy_mode() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n WheelUpPane set -g @hit yes");
    parse_config_content(&mut app, "bind -n C-WheelUpPane set -g @ctrl yes");
    parse_config_content(&mut app, "bind -T copy-mode WheelUpPane send -N1 -X scroll-up");
    assert!(app.config_warnings.is_empty(), "{:?}", app.config_warnings);
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), false), Dispatch::User("set -g @hit yes".into()));
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::CONTROL), false), Dispatch::User("set -g @ctrl yes".into()));
    // mode-keys emacs (default): copy-mode table.
    match resolve(&app, wheel(true, KeyModifiers::NONE), true) {
        Dispatch::User(c) => assert!(c.contains("scroll-up"), "{}", c),
        other => panic!("expected the copy-mode binding, got {:?}", other),
    }
    // A copy-mode-vi binding is not consulted under emacs keys.
    parse_config_content(&mut app, "bind -T copy-mode-vi WheelDownPane set -g @vi yes");
    assert_eq!(resolve(&app, wheel(false, KeyModifiers::NONE), true), Dispatch::Builtin);
    app.mode_keys = "vi".into();
    assert_eq!(resolve(&app, wheel(false, KeyModifiers::NONE), true), Dispatch::User("set -g @vi yes".into()));
}

#[test]
fn unbind_restores_tmux_semantics() {
    let mut app = app();
    parse_config_content(&mut app, "unbind -n WheelUpPane");
    // No root binding: the wheel no longer enters copy mode, it goes to the
    // pane (a shell gets nothing).
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), false), Dispatch::Forward);
    parse_config_content(&mut app, "unbind -T copy-mode WheelUpPane");
    // In copy mode the root default is gone too: dropped.
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), true), Dispatch::Drop);
    // Re-seeding the defaults (source-file) brings them back.
    populate_default_bindings(&mut app);
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), false), Dispatch::Builtin);
}

#[test]
fn copy_mode_unbind_falls_back_to_root_default_which_drops() {
    let mut app = app();
    parse_config_content(&mut app, "unbind -T copy-mode WheelUpPane");
    // tmux: copy-mode misses, root's WheelUpPane runs `send -M`, and a pane
    // in copy mode takes no mouse key that way. Nothing happens.
    assert_eq!(resolve(&app, wheel(true, KeyModifiers::NONE), true), Dispatch::Drop);
}

#[test]
fn list_keys_shows_default_and_user_mouse_bindings() {
    let mut app = app();
    let has = |app: &crate::types::AppState, table: &str, key: &str| {
        list_keys_entries(app).iter().any(|(t, k, _, _)| t == table && k == key)
    };
    assert!(has(&app, "root", "WheelUpPane"));
    assert!(has(&app, "copy-mode", "WheelUpPane"));
    assert!(has(&app, "copy-mode-vi", "WheelDownPane"));
    assert!(!has(&app, "root", "WheelDownPane"));
    parse_config_content(&mut app, "bind -n C-WheelUpPane set -g @x 1");
    assert!(has(&app, "root", "C-WheelUpPane"));
    // A user binding replaces the default line rather than adding a second.
    parse_config_content(&mut app, "bind -n WheelUpPane set -g @y 1");
    let rows: Vec<_> = list_keys_entries(&app).into_iter()
        .filter(|(t, k, _, _)| t == "root" && k == "WheelUpPane").collect();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].2.contains("@y"), "{:?}", rows[0]);
    parse_config_content(&mut app, "unbind -n WheelUpPane");
    assert!(!has(&app, "root", "WheelUpPane"));
}

#[test]
fn mouse_targets_resolve_to_the_pane() {
    assert_eq!(resolve_mouse_targets("select-pane -t =", 7), "select-pane -t %7");
    assert_eq!(resolve_mouse_targets("select-pane -t=", 7), "select-pane -t %7");
    assert_eq!(resolve_mouse_targets("resize-pane -Z -t {mouse}", 7), "resize-pane -Z -t %7");
    assert_eq!(
        resolve_mouse_targets("if -Ft= '#{pane_in_mode}' 'send -M' 'copy-mode -e'", 3),
        "if -F -t %3 '#{pane_in_mode}' 'send -M' 'copy-mode -e'"
    );
    assert_eq!(
        resolve_mouse_targets("if-shell -F -t = \"#{mouse_any_flag}\" \"send-keys -M\"", 3),
        "if-shell -F -t %3 \"#{mouse_any_flag}\" \"send-keys -M\""
    );
    // The tmux 2.1 era recipe: `-t=;` inside a quoted branch.
    assert_eq!(
        resolve_mouse_targets("if -Ft= '#{pane_in_mode}' 'send-keys -M' 'select-pane -t=; copy-mode -e; send-keys -M'", 4),
        "if -F -t %4 '#{pane_in_mode}' 'send-keys -M' 'select-pane -t %4; copy-mode -e; send-keys -M'"
    );
    assert_eq!(resolve_mouse_targets("select-pane -t= \\; send-keys -M", 2), "select-pane -t %2 \\; send-keys -M");
    // An `=` that is not a -t value, and text without targets, are untouched.
    assert_eq!(resolve_mouse_targets("set -g @a =", 1), "set -g @a =");
    assert_eq!(resolve_mouse_targets("copy-mode -e", 1), "copy-mode -e");
}

#[test]
fn modifier_mask_round_trips() {
    for m in [KeyModifiers::NONE, KeyModifiers::CONTROL, KeyModifiers::ALT | KeyModifiers::SHIFT,
              KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT] {
        assert_eq!(mods_from_mask(sgr_mod_bits(m)), m);
    }
    assert_eq!(sgr_mod_bits(KeyModifiers::CONTROL), 16);
}

#[test]
fn resolving_a_wheel_key_is_cheap() {
    // The lookup runs once per wheel notch. With the defaults plus a config's
    // worth of user bindings it must stay far below anything measurable on a
    // notch (a notch costs milliseconds end to end).
    let mut app = app();
    for i in 0..60 {
        parse_config_content(&mut app, &format!("bind -n F{} set -g @k{} 1", 1 + i % 12, i));
        parse_config_content(&mut app, &format!("bind -T copy-mode-vi M-{} send -X cancel", (b'a' + (i % 26) as u8) as char));
    }
    let up = wheel(true, KeyModifiers::NONE);
    let n = 100_000u32;
    let start = std::time::Instant::now();
    let mut builtin = 0u32;
    for i in 0..n {
        if resolve(&app, up, i % 2 == 0) == Dispatch::Builtin { builtin += 1; }
    }
    let per = start.elapsed() / n;
    eprintln!("mouse_keys::resolve: {:?} per call", per);
    assert_eq!(builtin, n);
    assert!(per < std::time::Duration::from_micros(20), "resolve took {:?} per call", per);
}

#[test]
fn drag_end_default_and_unbind() {
    let mut app = app();
    let end = drag_end_pane_key();
    assert_eq!(format_key_binding(&end), "MouseDragEnd1Pane");
    assert!(drag_end_is_default(&app));
    assert_eq!(resolve(&app, end, true), Dispatch::Builtin);
    assert!(list_keys_entries(&app).iter().any(|(t, k, c, _)| t == "copy-mode-vi" && k == "MouseDragEnd1Pane" && c.contains("copy-pipe-and-cancel")));
    // mode-keys emacs: the copy-mode table decides.
    parse_config_content(&mut app, "unbind -T copy-mode-vi MouseDragEnd1Pane");
    assert!(drag_end_is_default(&app), "copy-mode-vi is not the active table under emacs keys");
    app.mode_keys = "vi".into();
    assert!(!drag_end_is_default(&app));
    assert_eq!(resolve(&app, end, true), Dispatch::Drop, "unbound: the selection stays");
    parse_config_content(&mut app, "bind -T copy-mode-vi MouseDragEnd1Pane send -X copy-selection-no-clear");
    assert!(!drag_end_is_default(&app));
    assert_eq!(resolve(&app, end, true), Dispatch::User("send -X copy-selection-no-clear".into()));
}

#[test]
fn status_wheel_defaults_follow_tmux() {
    let mut app = app();
    let up = key(EV_WHEEL_UP, 0, LOC_STATUS, KeyModifiers::NONE);
    let down = key(EV_WHEEL_DOWN, 0, LOC_STATUS, KeyModifiers::NONE);
    assert_eq!(format_key_binding(&up), "WheelUpStatus");
    assert_eq!(resolve(&app, up, false), Dispatch::Builtin);
    assert_eq!(resolve(&app, down, false), Dispatch::Builtin);
    // In copy mode the copy-mode table has no status keys; root's default runs
    // (it is not a send -M default like WheelUpPane).
    assert_eq!(resolve(&app, up, true), Dispatch::Builtin);
    // Modified: unbound, nothing.
    assert_eq!(resolve(&app, key(EV_WHEEL_UP, 0, LOC_STATUS, KeyModifiers::CONTROL), false), Dispatch::Forward);
    let rows = list_keys_entries(&app);
    assert!(rows.iter().any(|(t, k, c, _)| t == "root" && k == "WheelUpStatus" && c == "previous-window"));
    assert!(rows.iter().any(|(t, k, c, _)| t == "root" && k == "WheelDownStatus" && c == "next-window"));
    parse_config_content(&mut app, "unbind -n WheelUpStatus");
    assert_eq!(resolve(&app, up, false), Dispatch::Forward);
    parse_config_content(&mut app, "bind -n WheelDownStatus select-window -t 0");
    assert_eq!(resolve(&app, down, false), Dispatch::User("select-window -t 0".into()));
}

#[test]
fn client_routed_keys_report_only_changes() {
    let mut app = app();
    assert_eq!(client_overrides(&app), "{}", "nothing changed: nothing sent");
    assert!(list_keys_entries(&app).iter().any(|(t, k, c, _)| t == "root" && k == "MouseDown1Status" && c == "select-window -t ="));
    parse_config_content(&mut app, "bind -n MouseDown1Status set -g @st hit");
    assert_eq!(client_overrides(&app), r#"{"MouseDown1Status":"UU"}"#);
    parse_config_content(&mut app, "unbind -n MouseDown1Status");
    assert_eq!(client_overrides(&app), r#"{"MouseDown1Status":"FD"}"#);
    populate_default_bindings(&mut app);
    assert_eq!(client_overrides(&app), "{}");
}

#[test]
fn a_status_target_is_a_window() {
    assert_eq!(resolve_mouse_targets_to("select-window -t =", ":2"), "select-window -t :2");
}

#[test]
fn a_binding_to_tmux_default_command_keeps_the_built_in() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n MouseDrag1Border resize-pane -M");
    parse_config_content(&mut app, "bind -n MouseDown1Status switch-client -t=");
    parse_config_content(&mut app, "bind -n WheelUpPane { if -F '#{||:#{pane_in_mode},#{mouse_any_flag}}' { send -M } { copy-mode -e } }");
    parse_config_content(&mut app, "bind -T copy-mode-vi MouseDragEnd1Pane send -X copy-pipe-and-cancel");
    let b = |n: &str, c: bool| resolve(&app, parse(n).unwrap(), c);
    assert_eq!(b("MouseDrag1Border", false), Dispatch::Builtin);
    assert_eq!(b("MouseDown1Status", false), Dispatch::Builtin);
    assert_eq!(b("WheelUpPane", false), Dispatch::Builtin);
    app.mode_keys = "vi".into();
    assert_eq!(resolve(&app, drag_end_pane_key(), true), Dispatch::Builtin);
    // Anything else is the user's own command.
    parse_config_content(&mut app, "bind -n MouseDrag1Border resize-pane -Z");
    assert_eq!(resolve(&app, parse("MouseDrag1Border").unwrap(), false), Dispatch::User("resize-pane -Z".into()));
}

#[test]
fn border_and_right_button_are_client_routed() {
    let mut app = app();
    for (k, cmd) in [("MouseDrag1Border", "resize-pane -M"), ("MouseDown3Pane", "paste-buffer")] {
        assert!(CLIENT_ROUTED.contains(&k));
        assert!(list_keys_entries(&app).iter().any(|(t, n, c, _)| t == "root" && n == k && c.contains(cmd)), "{} listed", k);
    }
    parse_config_content(&mut app, "unbind -n MouseDown3Pane\nbind -n MouseDrag1Border display x");
    let o = client_overrides(&app);
    assert!(o.contains(r#""MouseDown3Pane":"FD""#) && o.contains(r#""MouseDrag1Border":"UU""#), "{}", o);
}

#[test]
fn a_modified_routed_key_is_reported_by_its_full_name() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n M-MouseDown3Pane { refresh-client -l ; paste-buffer -p }");
    let o = client_overrides(&app);
    assert!(o.contains(r#""M-MouseDown3Pane":"UU""#), "{}", o);
    assert!(!o.contains(r#""MouseDown3Pane""#), "the plain key keeps its default: {}", o);
    assert_eq!(
        resolve(&app, parse("M-MouseDown3Pane").unwrap(), false),
        Dispatch::User("refresh-client -l \\; paste-buffer -p".into())
    );
}

// ── tmux's click timer (server-client.c click_timer, KEYC_CLICK_TIMEOUT) ──

fn opt(app: &crate::types::AppState, name: &str) -> Option<String> {
    app.user_options.get(name).cloned()
}

#[test]
fn multi_click_defaults_follow_tmux() {
    let app = app();
    for (n, copy) in [("DoubleClick1Pane", false), ("TripleClick1Pane", false), ("DoubleClick1Pane", true), ("TripleClick1Pane", true)] {
        assert_eq!(resolve(&app, parse(n).unwrap(), copy), Dispatch::Builtin, "{} copy={}", n, copy);
    }
    // SecondClick has no default in tmux.
    assert_eq!(resolve(&app, parse("SecondClick1Pane").unwrap(), false), Dispatch::Forward);
    let rows = list_keys_entries(&app);
    assert!(rows.iter().any(|(t, k, c, _)| t == "copy-mode-vi" && k == "TripleClick1Pane" && c.contains("select-line")));
    assert!(rows.iter().any(|(t, k, c, _)| t == "root" && k == "DoubleClick1Pane" && c.contains("select-word")));
    assert_eq!(CLICK_TIMEOUT_MS, 300);
}

#[test]
fn second_then_double_after_the_timer() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n SecondClick1Pane set -g @sc hit\nbind -n DoubleClick1Pane set -g @dc hit\nbind -n TripleClick1Pane set -g @tc hit");
    crate::window_ops::note_pane_press(&mut app, 4, 3, 2);
    assert_eq!(opt(&app, "@sc"), None, "one press is MouseDown1Pane only");
    crate::window_ops::note_pane_press(&mut app, 4, 3, 2);
    assert_eq!(opt(&app, "@sc").as_deref(), Some("hit"), "the second press is SecondClick1Pane");
    assert_eq!(opt(&app, "@dc"), None, "DoubleClick waits for the timer");
    // The timer runs out.
    if let Some(s) = app.click_state.as_mut() { s.3 = std::time::Instant::now() - std::time::Duration::from_millis(1); }
    assert!(crate::window_ops::click_timer_tick(&mut app));
    assert_eq!(opt(&app, "@dc").as_deref(), Some("hit"));
    assert_eq!(opt(&app, "@tc"), None);
    assert!(app.click_state.is_none());
}

#[test]
fn a_third_press_is_triple_and_no_double() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n DoubleClick1Pane set -g @dc hit\nbind -n TripleClick1Pane set -g @tc hit");
    for _ in 0..3 { crate::window_ops::note_pane_press(&mut app, 4, 3, 2); }
    assert_eq!(opt(&app, "@tc").as_deref(), Some("hit"));
    assert!(app.click_state.is_none());
    assert!(!crate::window_ops::click_timer_tick(&mut app));
    assert_eq!(opt(&app, "@dc"), None);
}

#[test]
fn presses_outside_the_window_or_in_another_pane_start_over() {
    let mut app = app();
    parse_config_content(&mut app, "bind -n SecondClick1Pane set -g @sc hit");
    crate::window_ops::note_pane_press(&mut app, 4, 3, 2);
    crate::window_ops::note_pane_press(&mut app, 5, 3, 2);
    assert_eq!(opt(&app, "@sc"), None, "another pane");
    if let Some(s) = app.click_state.as_mut() { s.3 = std::time::Instant::now() - std::time::Duration::from_millis(1); }
    crate::window_ops::note_pane_press(&mut app, 5, 3, 2);
    assert_eq!(opt(&app, "@sc"), None, "after the window");
}

#[test]
fn status_multi_clicks_reach_the_client_only_when_bound() {
    let mut app = app();
    assert_eq!(client_overrides(&app), "{}");
    parse_config_content(&mut app, "bind -n DoubleClick1Status display hi");
    assert_eq!(client_overrides(&app), r#"{"DoubleClick1Status":"UU"}"#);
}
