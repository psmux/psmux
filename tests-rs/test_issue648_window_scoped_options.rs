// Issue #648 (split out of #647 WIN-01) — window scoped options must live on
// the window.
//
// WHAT WAS BROKEN
//
// psmux kept ONE ordinary option store per server. `-w` and `-g` selected the
// same map, so a write aimed at one window landed on every window and on the
// global:
//
//     tmux new-session -d -s s -n zero
//     tmux new-window -d -t "s:" -n one
//     tmux set-option -w -t "s:zero" remain-on-exit on
//
//     psmux: zero=on  one=on          global=on
//     tmux:  zero=on  one=<inherited> global=off
//
// That is not a reporting wart. `reap_children` read the one flag for every
// window, so the reporter's ordinary PowerShell panes — in a DIFFERENT window
// from the gateway — stopped closing after `exit`.
//
// Two independent defects fed it:
//
//   1. no per-window storage at all, so `-w` had nowhere to write;
//   2. the `-t` target was parsed for its NUMERIC half only, so `-t "s:zero"`
//      lost the name and silently acted on the ACTIVE window even once storage
//      existed.
//
// WHAT THESE TESTS PIN
//
// tmux 3.4 was the oracle for every expectation below (`tmux -L p648`):
//
//     set -w -t s:zero remain-on-exit on
//       show -w -v -t s:zero  -> "on"      (the targeted window)
//       show -w -v -t s:one   -> ""        (inherits, unset)
//       show -g -v            -> "off"     (global untouched)
//       show -wA -t s:one     -> "remain-on-exit* off"   (`*` = inherited)
//       show -wA -t s:zero    -> "remain-on-exit on"     (no marker = local)
//     set -w -u -t s:zero remain-on-exit
//       show -w -v -t s:zero  -> ""        (inherits again)
//     set -wg remain-on-exit on
//       show -g -v            -> "on"
//       show -w -v -t s:zero  -> ""        (the global table, not the window's)
//     set -p beats set -w on the pane that has it.
//
// A `-v <name>` query reports the RESOLVED value where tmux reports a blank for
// an unset window option (tmux's own `show -wA -v` reports the resolved value
// too), a deliberate deviation libtmux/tmuxp depend on, see #321. The LISTING
// follows tmux exactly since #655: `-w` is the window's own table, `-A` merges
// the inherited values in with a `*`, `-wg` is the global window table.

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

/// The reporter's exact session: two windows, `zero` active, `one` alongside.
fn two_windows() -> AppState {
    let mut app = AppState::new("s".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.clear();
    app.windows.push(make_window("zero", 0));
    app.windows.push(make_window("one", 1));
    app.active_idx = 0;
    app
}

fn show_w(app: &AppState, window: usize, name: &str) -> String {
    get_window_option_value_for(app, name, Some(window))
}

// ── 1. `-w -t <window>` writes exactly one window ────────────────────────────

#[test]
fn set_w_with_a_window_target_writes_only_that_window() {
    let mut app = two_windows();
    let reply = apply_set_window_option(
        &mut app, "s:zero", "remain-on-exit", "on", false, false, false, false,
    );
    assert_eq!(reply, "", "targeted set must succeed, got {:?}", reply);

    assert_eq!(show_w(&app, 0, "remain-on-exit"), "on", "the targeted window");
    assert_eq!(
        show_w(&app, 1, "remain-on-exit"),
        "off",
        "BUG #648: the sibling window inherited the targeted write",
    );
    assert_eq!(
        get_option_value(&app, "remain-on-exit"),
        "off",
        "BUG #648: a window targeted write changed the GLOBAL store",
    );
    assert!(
        !app.remain_on_exit,
        "BUG #648: a window targeted write flipped the session-wide field",
    );
}

#[test]
fn a_window_target_resolves_by_name_not_by_position() {
    // The reporter targeted `s:zero` while `zero` was NOT the active window in
    // one of their runs. The old route kept only the numeric half of the parsed
    // target, so the name was dropped and the ACTIVE window was written.
    let mut app = two_windows();
    app.active_idx = 0;
    apply_set_window_option(
        &mut app, "s:one", "monitor-activity", "on", false, false, false, false,
    );
    assert_eq!(show_w(&app, 1, "monitor-activity"), "on", "named window got it");
    assert_eq!(
        show_w(&app, 0, "monitor-activity"),
        "off",
        "BUG #648: the write landed on the ACTIVE window, not the named one",
    );
}

#[test]
fn a_window_target_resolves_by_index_and_by_id() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:1", "monitor-silence", "5", false, false, false, false);
    assert_eq!(show_w(&app, 1, "monitor-silence"), "5");
    assert_eq!(show_w(&app, 0, "monitor-silence"), "0");

    apply_set_window_option(&mut app, "@0", "monitor-silence", "7", false, false, false, false);
    assert_eq!(show_w(&app, 0, "monitor-silence"), "7");
    assert_eq!(show_w(&app, 1, "monitor-silence"), "5", "id form must not disturb the other");
}

#[test]
fn an_empty_target_means_the_active_window() {
    let mut app = two_windows();
    app.active_idx = 1;
    apply_set_window_option(&mut app, "", "remain-on-exit", "on", false, false, false, false);
    assert_eq!(show_w(&app, 1, "remain-on-exit"), "on");
    assert_eq!(show_w(&app, 0, "remain-on-exit"), "off");
}

#[test]
fn an_unresolvable_target_is_reported_not_swallowed() {
    let mut app = two_windows();
    let reply = apply_set_window_option(
        &mut app, "s:nosuchwindow", "remain-on-exit", "on", false, false, false, false,
    );
    assert!(
        reply.starts_with("ERROR: can't find window"),
        "a bad target must be reported, got {:?}",
        reply,
    );
    assert_eq!(show_w(&app, 0, "remain-on-exit"), "off", "nothing was written");
}

// ── 2. `-wg` writes the global window table ──────────────────────────────────

#[test]
fn set_wg_writes_the_global_table_and_no_window() {
    let mut app = two_windows();
    // `-wg` never reaches apply_set_window_option: scope routing sends it down
    // the global path, which is the config parser's `is_global` branch.
    crate::config::parse_config_line(&mut app, "set -wg remain-on-exit on");
    assert_eq!(get_option_value(&app, "remain-on-exit"), "on", "global table took it");
    assert!(app.remain_on_exit);
    assert!(
        window_local_option(&app, 0, "remain-on-exit").is_none(),
        "BUG: -wg wrote the WINDOW's own table",
    );
    assert!(window_local_option(&app, 1, "remain-on-exit").is_none());
}

// ── 3. inheritance ───────────────────────────────────────────────────────────

#[test]
fn a_window_without_its_own_value_follows_the_global() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "set -g monitor-activity on");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "on");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "on");
    assert!(window_flag(&app, 1, "monitor-activity", app.monitor_activity));

    // ... and a window that DOES have one keeps it when the global moves.
    apply_set_window_option(&mut app, "s:one", "monitor-activity", "off", false, false, false, false);
    crate::config::parse_config_line(&mut app, "set -g monitor-activity on");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "on", "still inheriting");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "off", "own value outranks the global");
}

#[test]
fn window_number_and_window_flag_fall_back_to_the_global() {
    let mut app = two_windows();
    app.monitor_silence = 9;
    assert_eq!(window_number(&app, 0, "monitor-silence", app.monitor_silence), 9);
    apply_set_window_option(&mut app, "s:zero", "monitor-silence", "2", false, false, false, false);
    assert_eq!(window_number(&app, 0, "monitor-silence", app.monitor_silence), 2);
    assert_eq!(window_number(&app, 1, "monitor-silence", app.monitor_silence), 9);
}

// ── 4. `-u` removes only that window's value ─────────────────────────────────

#[test]
fn unset_on_a_window_restores_inheritance_and_leaves_the_global_alone() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "set -g remain-on-exit on");
    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "off", false, false, false, false);
    assert_eq!(show_w(&app, 0, "remain-on-exit"), "off");

    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "", true, false, false, false);
    assert!(
        window_local_option(&app, 0, "remain-on-exit").is_none(),
        "-u must REMOVE the window entry, not write a default over it",
    );
    assert_eq!(show_w(&app, 0, "remain-on-exit"), "on", "the window inherits again");
    assert_eq!(get_option_value(&app, "remain-on-exit"), "on", "the global is untouched");
}

#[test]
fn unset_on_one_window_does_not_touch_another() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:zero", "monitor-activity", "on", false, false, false, false);
    apply_set_window_option(&mut app, "s:one", "monitor-activity", "on", false, false, false, false);
    apply_set_window_option(&mut app, "s:zero", "monitor-activity", "", true, false, false, false);
    assert_eq!(show_w(&app, 0, "monitor-activity"), "off", "unset window inherits");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "on", "sibling keeps its own value");
}

// ── 5. `-p` still wins over `-w` ─────────────────────────────────────────────

#[test]
fn a_pane_option_outranks_the_window_option() {
    // The documented #647 workaround must keep working: `-w off` on the window,
    // `-p on` on the one pane that should survive. tree::keep_dead_pane is the
    // single place reap_children resolves the chain.
    assert!(
        crate::tree::keep_dead_pane(Some("on"), false, true),
        "pane `on` must beat window `off`",
    );
    assert!(
        !crate::tree::keep_dead_pane(Some("off"), true, true),
        "pane `off` must beat window `on`",
    );
    assert!(
        crate::tree::keep_dead_pane(Some("failed"), false, false),
        "pane `failed` keeps a nonzero exit even when the window says off",
    );
    assert!(
        !crate::tree::keep_dead_pane(Some("failed"), true, true),
        "pane `failed` closes a clean exit even when the window says on",
    );
    // ... and with no pane entry the WINDOW decides, which is the #648 fix:
    // this argument used to be one session-wide flag for every window.
    assert!(crate::tree::keep_dead_pane(None, true, true));
    assert!(!crate::tree::keep_dead_pane(None, false, false));
}

#[test]
fn the_reporter_scenario_leaves_the_other_window_closing_panes() {
    // "our gateway manager enabled window scoped remain-on-exit to preserve
    // shutdown logs. On psmux, ordinary PowerShell panes created later also
    // remained dead after `exit`." Window `zero` keeps its dead panes; a pane
    // in window `one` must still close.
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "on", false, false, false, false);
    let zero = window_flag(&app, 0, "remain-on-exit", app.remain_on_exit);
    let one = window_flag(&app, 1, "remain-on-exit", app.remain_on_exit);
    assert!(zero, "the targeted window keeps dead panes");
    assert!(!one, "BUG #648: a later pane in ANOTHER window stopped closing");
    assert!(crate::tree::keep_dead_pane(None, zero, true));
    assert!(!crate::tree::keep_dead_pane(None, one, true));
}

// ── 6. show-options -w / -wA output ──────────────────────────────────────────

#[test]
fn show_w_reports_the_targeted_windows_own_value() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "on", false, false, false, false);
    let zero = render_window_options_for(&app, Some(0), WindowListing::Local);
    let one = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert!(
        zero.lines().any(|l| l == "remain-on-exit on"),
        "show -w -t s:zero must read the write back:\n{}",
        zero,
    );
    // #655 supersedes the original #648 expectation here: tmux prints nothing
    // for an option the window does not own, and the inherited value is what
    // `-A` (below) or `-v remain-on-exit` answers with.
    assert!(
        !one.lines().any(|l| l.starts_with("remain-on-exit")),
        "BUG #648: show -w -t s:one reported the other window's value:\n{}",
        one,
    );
    assert_eq!(
        get_window_option_value_for(&app, "remain-on-exit", Some(1)),
        "off",
        "the sibling window still RESOLVES to the inherited off",
    );
}

#[test]
fn show_wa_marks_an_inherited_option_with_a_star() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "on", false, false, false, false);
    let zero = render_window_options_for(&app, Some(0), WindowListing::LocalAndInherited);
    let one = render_window_options_for(&app, Some(1), WindowListing::LocalAndInherited);
    assert!(
        zero.lines().any(|l| l == "remain-on-exit on"),
        "a window-local value carries NO marker (tmux cmd-show-options.c):\n{}",
        zero,
    );
    assert!(
        one.lines().any(|l| l == "remain-on-exit* off"),
        "an inherited value carries the `*` marker:\n{}",
        one,
    );
    // Every other option is inherited on both windows, so both listings mark it.
    assert!(zero.lines().any(|l| l.starts_with("monitor-activity* ")), "{}", zero);
    assert!(one.lines().any(|l| l.starts_with("monitor-activity* ")), "{}", one);
}

#[test]
fn show_w_without_dash_a_never_marks_anything() {
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:zero", "remain-on-exit", "on", false, false, false, false);
    let listing = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert!(
        !listing.contains('*'),
        "the `*` marker belongs to -A only:\n{}",
        listing,
    );
}

#[test]
fn every_window_option_name_is_listed_for_a_window() {
    // libtmux and tmuxp probe window scope and expect an answer for each name
    // (#321). After #655 the COMPLETE listing is what `-A` and `-wg` print;
    // a plain `-w` is the window's own table the way tmux prints it.
    let app = two_windows();
    for listing in [
        render_window_options_for(&app, Some(1), WindowListing::LocalAndInherited),
        render_window_options_for(&app, Some(1), WindowListing::Global),
    ] {
        for name in crate::server::option_catalog::WINDOW_OPTION_NAMES {
            assert!(
                listing
                    .lines()
                    .any(|l| l.starts_with(&format!("{} ", name))
                        || l.starts_with(&format!("{}* ", name))),
                "the listing dropped {}:\n{}",
                name,
                listing,
            );
        }
    }
}

// ── 7. the config file route ─────────────────────────────────────────────────

#[test]
fn a_config_line_with_dash_w_and_a_target_writes_one_window() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "set -w -t \"s:one\" remain-on-exit on");
    assert_eq!(show_w(&app, 1, "remain-on-exit"), "on");
    assert_eq!(
        show_w(&app, 0, "remain-on-exit"),
        "off",
        "BUG #648: a config `set -w -t` line still wrote every window",
    );
    assert!(!app.remain_on_exit, "BUG #648: it also wrote the global");
}

#[test]
fn the_setw_spelling_takes_the_same_route() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "setw -t s:one monitor-activity on");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "on");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "off");
}

#[test]
fn a_config_line_without_a_target_writes_the_active_window() {
    let mut app = two_windows();
    app.active_idx = 1;
    crate::config::parse_config_line(&mut app, "setw monitor-silence 3");
    assert_eq!(show_w(&app, 1, "monitor-silence"), "3");
    assert_eq!(show_w(&app, 0, "monitor-silence"), "0");
}

#[test]
fn a_config_dash_w_unset_restores_inheritance() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "set -g monitor-activity on");
    crate::config::parse_config_line(&mut app, "setw -t s:zero monitor-activity off");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "off");
    crate::config::parse_config_line(&mut app, "setw -u -t s:zero monitor-activity");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "on", "inherits again");
    assert!(app.monitor_activity, "the global is untouched");
}

#[test]
fn a_session_option_under_dash_w_still_lands_in_the_session_store() {
    // tmux derives scope from the option NAME (options_scope_from_name), so
    // `set -w` on a session option is not a window write. Every config that
    // has ever spelled `setw status-left ...` must keep working.
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "setw status-left \"[#S] here\"");
    assert_eq!(app.status_left, "[#S] here");
    assert!(window_local_option(&app, 0, "status-left").is_none());
}

#[test]
fn a_window_boolean_named_with_no_value_toggles_that_window_only() {
    let mut app = two_windows();
    app.active_idx = 0;
    crate::config::parse_config_line(&mut app, "setw monitor-activity");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "on", "toggled off -> on");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "off", "sibling untouched");
    crate::config::parse_config_line(&mut app, "setw monitor-activity");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "off", "toggled back");
}

#[test]
fn append_at_window_scope_appends_to_the_windows_own_value() {
    let mut app = two_windows();
    crate::config::parse_config_line(&mut app, "setw -t s:zero window-status-format \"#I\"");
    crate::config::parse_config_line(&mut app, "setw -a -t s:zero window-status-format \":#W\"");
    assert_eq!(show_w(&app, 0, "window-status-format"), "#I:#W");
    assert_eq!(
        show_w(&app, 1, "window-status-format"),
        app.window_status_format,
        "the sibling still inherits the server wide format",
    );
}

#[test]
fn only_if_unset_at_window_scope_refuses_a_second_write_on_that_window() {
    let mut app = two_windows();
    let first = apply_set_window_option(
        &mut app, "s:zero", "monitor-activity", "on", false, false, true, false,
    );
    assert_eq!(first, "", "the first -o write applies");
    let second = apply_set_window_option(
        &mut app, "s:zero", "monitor-activity", "off", false, false, true, false,
    );
    assert_eq!(second, "ERROR: already set: monitor-activity");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "on", "the value did not move");
    // The sibling has nothing of its own, so -o applies there.
    let sibling = apply_set_window_option(
        &mut app, "s:one", "monitor-activity", "on", false, false, true, false,
    );
    assert_eq!(sibling, "", "-o judges the WINDOW's own table, not the global");
    // -q swallows the refusal, as it does at every other scope (#619).
    let quiet = apply_set_window_option(
        &mut app, "s:zero", "monitor-activity", "off", false, false, true, true,
    );
    assert_eq!(quiet, "");
}

#[test]
fn an_invalid_window_value_is_refused_not_stored() {
    let mut app = two_windows();
    let reply = apply_set_window_option(
        &mut app, "s:zero", "monitor-silence", "banana", false, false, false, false,
    );
    assert!(reply.starts_with("ERROR: "), "expected a refusal, got {:?}", reply);
    assert!(window_local_option(&app, 0, "monitor-silence").is_none());
}

// ── 8. command chaining ──────────────────────────────────────────────────────

#[test]
fn chained_window_writes_each_reach_their_own_window() {
    let mut app = two_windows();
    for command in crate::config::split_chained_commands_pub(
        "set -w -t s:zero remain-on-exit on \\; set -w -t s:one monitor-activity on",
    ) {
        crate::config::parse_config_line(&mut app, &command);
    }
    assert_eq!(show_w(&app, 0, "remain-on-exit"), "on");
    assert_eq!(show_w(&app, 1, "remain-on-exit"), "off");
    assert_eq!(show_w(&app, 1, "monitor-activity"), "on");
    assert_eq!(show_w(&app, 0, "monitor-activity"), "off");
    assert!(!app.remain_on_exit && !app.monitor_activity, "globals untouched");
}

// ── the options that were already per window keep working ────────────────────

#[test]
fn automatic_rename_still_reports_off_for_an_explicitly_named_window() {
    // #266: a window born with `-n NAME` sets manual_rename and must report
    // automatic-rename off without anyone writing the option. The window store
    // must not have displaced that.
    let mut app = two_windows();
    app.windows[0].manual_rename = true;
    assert_eq!(show_w(&app, 0, "automatic-rename"), "off");
    assert_eq!(show_w(&app, 1, "automatic-rename"), "on");
    // And it counts as window-local for `-A`, because it is.
    assert!(!window_option_is_inherited(&app, 0, "automatic-rename"));
    assert!(window_option_is_inherited(&app, 1, "automatic-rename"));
}

#[test]
fn setting_automatic_rename_on_for_a_window_clears_its_manual_rename() {
    let mut app = two_windows();
    app.windows[0].manual_rename = true;
    apply_set_window_option(&mut app, "s:zero", "automatic-rename", "on", false, false, false, false);
    assert!(!app.windows[0].manual_rename, "the rename loop is re-armed for THIS window");
    assert!(app.windows[1].manual_rename == false);
    assert_eq!(show_w(&app, 0, "automatic-rename"), "on");
}

#[test]
fn window_size_at_window_scope_keeps_its_dedicated_field_in_step() {
    // resize-window and the layout code read Window::window_size directly, so a
    // `set -w window-size` write has to land in both places or they disagree.
    let mut app = two_windows();
    apply_set_window_option(&mut app, "s:one", "window-size", "manual", false, false, false, false);
    assert_eq!(app.windows[1].window_size.as_deref(), Some("manual"));
    assert_eq!(show_w(&app, 1, "window-size"), "manual");
    assert_eq!(show_w(&app, 0, "window-size"), "latest", "sibling inherits");
    apply_set_window_option(&mut app, "s:one", "window-size", "", true, false, false, false);
    assert_eq!(app.windows[1].window_size, None, "-u clears the field too");
    assert_eq!(show_w(&app, 1, "window-size"), "latest");
}

// ── the scope predicate ──────────────────────────────────────────────────────

#[test]
fn only_catalog_window_names_are_window_scoped_writes() {
    for name in crate::server::option_catalog::WINDOW_OPTION_NAMES {
        assert!(is_window_scoped_write(name), "{} is declared window scope", name);
    }
    for name in ["status-left", "prefix", "escape-time", "mouse", "pane-border-style"] {
        assert!(!is_window_scoped_write(name), "{} must stay where it was", name);
    }
    // User options take the scope of the flags, as in tmux: `setw @k` is the
    // window's own (#795 follow up); `#{@k}` resolves through the window.
    assert!(is_window_scoped_write("@mouse-force"));
}
