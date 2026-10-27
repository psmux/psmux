// Issue #655: `show-options -w` printed every window scope option, inherited
// values included, and `-wA` printed that listing and then appended the WHOLE
// global listing after it.
//
// THE ORACLE
//
// Measured on tmux 3.4 under WSL (`tmux -L parity`) before any code was
// written, with two windows created as `-n zero` / `-n one` and one local
// override on zero:
//
//     set -w -t s:zero remain-on-exit on
//       show -w  -t s:zero -> 2 lines:  automatic-rename off / remain-on-exit on
//       show -w  -t s:one  -> 1 line:   automatic-rename off
//       show -wA -t s:one  -> 55 lines, ONE merged window scope list, the
//                             inherited entries starred: remain-on-exit* off
//       show -wA -t s:zero -> 55 lines, remain-on-exit unstarred
//       show -wg           -> 55 lines, the GLOBAL window table, nothing
//                             starred, automatic-rename on, remain-on-exit off
//       show -p  -t s:zero -> 0 lines
//       show -pA -t s:zero -> 13 lines, remain-on-exit* on inherited from the
//                             WINDOW, not from the global store
//
// psmux's window table holds 16 names where tmux's holds 55, so the counts
// differ; the RULE is what these tests pin. `automatic-rename off` is a window
// LOCAL on both windows because both were born with `-n`, on tmux and on psmux
// alike (`Window::manual_rename`, #266).
//
// The measured psmux 3.3.8 (e70323c) answers to the same transcript were 16, 16,
// 77 (16 + 61 global), 77 and 16-of-the-active-window respectively.

use super::*;
use crate::server::option_catalog::WINDOW_OPTION_NAMES;
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

/// The reporter's session: `zero` active with one local override, `one` clean.
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

fn with_override() -> AppState {
    let mut app = two_windows();
    apply_set_window_option(
        &mut app, "s:zero", "remain-on-exit", "on", false, false, false, false,
    );
    app
}

fn names(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.split(' ').next().unwrap_or("").to_string())
        .collect()
}

// ── 1. `-w` prints the window's OWN table and nothing else ───────────────────

#[test]
fn show_w_lists_only_the_windows_own_values() {
    let app = with_override();
    let zero = render_window_options_for(&app, Some(0), WindowListing::Local);
    assert_eq!(
        zero.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>(),
        vec!["remain-on-exit on"],
        "BUG #655: show -w -t s:zero listed inherited values too:\n{}",
        zero,
    );
}

#[test]
fn show_w_on_a_window_with_no_locals_prints_nothing() {
    let app = with_override();
    let one = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert_eq!(
        one, "",
        "BUG #655: show -w -t s:one printed a listing where tmux prints nothing:\n{}",
        one,
    );
}

#[test]
fn a_window_born_with_a_name_owns_automatic_rename() {
    // tmux 3.4 prints `automatic-rename off` for a `-n`-named window on a plain
    // `-w`, because creating it with a name writes that option on the window.
    // psmux keeps the same fact in `Window::manual_rename` (#266), and the
    // listing must treat it as a LOCAL, not as an inherited value.
    let mut app = two_windows();
    app.windows[0].manual_rename = true;
    let zero = render_window_options_for(&app, Some(0), WindowListing::Local);
    assert_eq!(
        zero.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>(),
        vec!["automatic-rename off"],
        "{}",
        zero,
    );
    assert_eq!(
        render_window_options_for(&app, Some(1), WindowListing::Local),
        "",
        "the sibling was never named, so it owns nothing",
    );
}

#[test]
fn a_window_size_override_is_a_local_too() {
    let mut app = two_windows();
    app.windows[1].window_size = Some("manual".to_string());
    let one = render_window_options_for(&app, Some(1), WindowListing::Local);
    assert_eq!(
        one.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>(),
        vec!["window-size manual"],
        "{}",
        one,
    );
}

#[test]
fn show_w_never_emits_the_marker() {
    let app = with_override();
    for index in [0, 1] {
        let listing = render_window_options_for(&app, Some(index), WindowListing::Local);
        assert!(
            !listing.contains('*'),
            "the `*` marker belongs to -A only:\n{}",
            listing,
        );
    }
}

// ── 2. `-wA` is ONE merged list ──────────────────────────────────────────────

#[test]
fn show_wa_is_one_merged_window_scope_list() {
    let app = with_override();
    for index in [0, 1] {
        let listing = render_window_options_for(&app, Some(index), WindowListing::LocalAndInherited);
        let listed = names(&listing);
        assert_eq!(
            listed.len(),
            WINDOW_OPTION_NAMES.len(),
            "BUG #655: -A printed {} entries for a {} entry window table:\n{}",
            listed.len(),
            WINDOW_OPTION_NAMES.len(),
            listing,
        );
        for (got, want) in listed.iter().zip(WINDOW_OPTION_NAMES) {
            assert_eq!(
                got.trim_end_matches('*'),
                *want,
                "-A must print the window table in table order:\n{}",
                listing,
            );
        }
    }
}

#[test]
fn show_wa_never_appends_the_session_listing() {
    // The 3.3.8 bug: the window listing was followed by the entire global one,
    // so names that are not window scoped at all showed up under `-wA`.
    let app = with_override();
    let listing = render_window_options_for(&app, Some(1), WindowListing::LocalAndInherited);
    for leaked in ["prefix", "status-left", "escape-time", "default-shell", "mouse"] {
        assert!(
            !names(&listing).iter().any(|n| n.trim_end_matches('*') == leaked),
            "BUG #655: the session option {} leaked into a -wA listing:\n{}",
            leaked,
            listing,
        );
    }
}

#[test]
fn show_wa_marks_inherited_and_leaves_locals_plain() {
    let app = with_override();
    let zero = render_window_options_for(&app, Some(0), WindowListing::LocalAndInherited);
    let one = render_window_options_for(&app, Some(1), WindowListing::LocalAndInherited);
    assert!(zero.lines().any(|l| l == "remain-on-exit on"), "{}", zero);
    assert!(one.lines().any(|l| l == "remain-on-exit* off"), "{}", one);
    assert!(zero.lines().any(|l| l.starts_with("monitor-activity* ")), "{}", zero);
}

// ── 3. `-wg` is the GLOBAL window table ──────────────────────────────────────

#[test]
fn show_wg_reads_the_global_table_not_the_window() {
    // 3.3.8 answered `-wg` from the active window, so a window-local override
    // was reported as if the global table held it.
    let app = with_override();
    let global = render_window_options_for(&app, Some(0), WindowListing::Global);
    assert!(
        global.lines().any(|l| l == "remain-on-exit off"),
        "BUG #655: -wg reported the window's local value:\n{}",
        global,
    );
    assert!(
        !global.contains('*'),
        "the global table owns every entry, so nothing is marked:\n{}",
        global,
    );
    assert_eq!(
        names(&global).len(),
        WINDOW_OPTION_NAMES.len(),
        "-wg lists the whole window table:\n{}",
        global,
    );
}

#[test]
fn render_window_options_is_the_global_table() {
    // The no-target helper (the option default audits, #559) must not depend on
    // whatever the active window happens to own.
    let app = with_override();
    assert_eq!(
        render_window_options(&app),
        render_window_options_for(&app, Some(0), WindowListing::Global),
    );
}

// ── 4. the pane scope prints by the same rule ────────────────────────────────

#[test]
fn show_p_prints_only_what_the_pane_owns() {
    let none = |_: &str| None;
    assert_eq!(render_pane_options("", false, none), "");
    assert_eq!(
        render_pane_options("remain-on-exit on", false, none),
        "remain-on-exit on\n",
    );
}

#[test]
fn show_pa_adds_the_inherited_pane_option_with_a_marker() {
    // tmux 3.4: `show -pA` in a window with `remain-on-exit on` prints
    // `remain-on-exit* on`. 3.3.8 printed nothing at all.
    let inherited = |name: &str| {
        if name == "remain-on-exit" { Some("on".to_string()) } else { None }
    };
    assert_eq!(
        render_pane_options("", true, inherited),
        "remain-on-exit* on\n",
    );
}

#[test]
fn show_pa_leaves_a_pane_local_unmarked_and_does_not_duplicate_it() {
    // remain-on-exit-format is a pane option too, so a pane that does not
    // own it lists the inherited text with a marker.
    let inherited = |name: &str| match name {
        "remain-on-exit" => Some("off".to_string()),
        "remain-on-exit-format" => Some("bye".to_string()),
        _ => None,
    };
    assert_eq!(
        render_pane_options("remain-on-exit failed", true, inherited),
        "remain-on-exit failed\nremain-on-exit-format* bye\n",
    );
}

#[test]
fn show_pa_keeps_user_options_and_never_invents_one() {
    // `@mouse-force` is a user option: it is printed when the pane stores it
    // and never conjured from a parent scope (cmd-show-options.c prints user
    // options from the table's OWN entries).
    let inherited = |_: &str| None;
    assert_eq!(
        render_pane_options("@mouse-force on", true, inherited),
        "@mouse-force on\n",
    );
}

#[test]
fn a_pane_listing_refusal_is_handed_straight_back() {
    let inherited = |_: &str| Some("off".to_string());
    assert_eq!(
        render_pane_options("ERROR: can't find pane: %99", true, inherited),
        "ERROR: can't find pane: %99\n",
    );
}

// ── 5. the resolved single-value query is untouched ──────────────────────────

#[test]
fn the_named_query_still_resolves_through_the_parent() {
    // #321: libtmux and tmuxp probe window scope for options psmux keeps
    // session wide, so `-v <name>` must keep answering with the resolved value
    // even though the LISTING no longer does.
    let app = with_override();
    assert_eq!(get_window_option_value_for(&app, "remain-on-exit", Some(0)), "on");
    assert_eq!(get_window_option_value_for(&app, "remain-on-exit", Some(1)), "off");
    assert_eq!(
        get_window_option_value_for(&app, "window-status-format", Some(1)),
        get_option_value(&app, "window-status-format"),
    );
}

#[test]
fn an_unset_then_reset_window_returns_to_an_empty_listing() {
    let mut app = with_override();
    apply_set_window_option(
        &mut app, "s:zero", "remain-on-exit", "", true, false, false, false,
    );
    assert_eq!(
        render_window_options_for(&app, Some(0), WindowListing::Local),
        "",
        "set -w -u gives the window its empty own table back",
    );
    assert!(
        render_window_options_for(&app, Some(0), WindowListing::LocalAndInherited)
            .lines()
            .any(|l| l == "remain-on-exit* off"),
        "and -A then reports it as inherited",
    );
}
