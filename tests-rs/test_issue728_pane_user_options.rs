// Issue #728: pane scoped user options.
//
// tmux keeps an options table on every pane and stores ANY `@name` user option
// there under `set-option -p`. psmux accepted only `remain-on-exit` and
// `@mouse-force` and refused the rest:
//
//     $ psmux set-option -p -t %1 @omx_team_pane_owner_id 1
//     psmux: pane-scoped option '@omx_team_pane_owner_id' is not supported
//            (supported: remain-on-exit, @mouse-force)            exit 1
//
// which broke Codex `omx team` and the Claude Code teammate backend.
//
// THE ORACLE (tmux 3.4 under WSL, `tmux -L t728ref -f /dev/null`, panes %0 %1)
//
//     set -p -t %0 @omx_team_pane_owner_id 1      rc 0
//     show -p -t %0                               @omx_team_pane_owner_id 1
//     show -qv -p -t %0 @omx_team_pane_owner_id   1
//     show -v -p -t %1 @omx_team_pane_owner_id    invalid option: @omx...  rc 1
//     show -qv -p -t %1 @omx_team_pane_owner_id   (nothing) rc 0
//     list-panes -F '#{pane_id} [#{@omx...}]'     %0 [1] / %1 []
//     set -w W; set S; set -g G                   %0 [1] / %1 [W]
//     show -pvA -t %1 @omx...                     W
//     set -pu -t %0 @omx...                       %0 falls back to the parent
//     set -p @a x; set -pa @a y                   xy
//     set -po @a z                                already set: @a  rc 1
//     set -p bogus-opt 1                          invalid option: bogus-opt
//     set -pu -t %0 @nonexistent                  rc 0
//     move-pane / break-pane / swap-pane          the value follows the pane
//
// psmux keeps every non pane `@name` in one store (`user_options`; a psmux
// server holds one session and `-w @name` lands there as well), so the chain
// these tests pin is pane first, then that store. Registered from
// src/server/options.rs.

use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::{AppState, LayoutKind, Node};
use crate::window_ops::BreakPaneRequest;
use ratatui::layout::Rect;

fn make_pane(id: usize) -> crate::types::Pane {
    let (rows, cols) = (40u16, 80u16);
    let (master, writer) = crate::util::stub_pane_pty(portable_pty::PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
    let child = crate::util::StubChild::exited();
    let term = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
    let epoch = Instant::now() - Duration::from_secs(2);
    crate::types::Pane {
        master,
        writer,
        child,
        term,
        last_rows: rows,
        last_cols: cols,
        id,
        title: format!("pane{id}"),
        title_locked: false,
        child_pid: None,
        data_version: Arc::new(AtomicU64::new(0)),
        last_title_check: epoch,
        last_infer_title: epoch,
        dead: false,
        last_text_input: None,
        last_special_key: None,
        vt_bridge_cache: None,
        vti_mode_cache: None,
        mouse_input_cache: None,
        win32_input_latched: false, input_off: false,
        scroll_fg_cache: None,
        mouse_proto_owner: None,
        wheel_auth: None,
        cursor_shape: Arc::new(AtomicU8::new(0)),
        bell_pending: Arc::new(AtomicBool::new(false)),
        cpr_pending: Arc::new(AtomicBool::new(false)),
        color_query_pending: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        copy_state: None,
        live_term: None,
        pane_style: None,
        pane_options: Default::default(),
        squelch_until: None,
        output_ring: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        spawned_at: None,
        start_command: String::new(),
        cwd_hint: None,
    }
}

fn make_window(id: usize, name: &str) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: name.to_string(),
        id,
        area: Rect::new(0, 0, 160, 40),
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: Instant::now(),
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

/// Window i holds the pane ids given; window 0 is active, its first pane too.
fn app_with_windows(windows: &[&[usize]]) -> AppState {
    let mut app = AppState::new("issue728".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.last_window_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    app.client_area = Rect { x: 0, y: 0, width: 160, height: 40 };
    app.windows.clear();
    app.window_indices.clear();
    for (w, ids) in windows.iter().enumerate() {
        let mut win = make_window(w, &format!("win{w}"));
        if ids.len() == 1 {
            win.root = Node::Leaf(make_pane(ids[0]));
            win.active_path = vec![];
        } else {
            let n = ids.len();
            let children: Vec<Node> = ids.iter().map(|&id| Node::Leaf(make_pane(id))).collect();
            win.root = Node::Split {
                kind: LayoutKind::Horizontal,
                sizes: vec![(100 / n) as u16; n],
                children,
            };
            win.active_path = vec![0];
        }
        win.pane_mru = ids.to_vec();
        app.windows.push(win);
        app.window_indices.push(w);
    }
    app.next_win_id = windows.len();
    app.active_idx = 0;
    app
}

/// `set-option -p` with no extra flags.
fn set(app: &mut AppState, target: &str, name: &str, value: &str) -> String {
    apply_set_pane_option(app, target, name, value, false, false, false, false)
}

/// What the server's `ShowPaneOptions` reply carries for one pane.
fn own(app: &AppState, id: usize, name: &str) -> Option<String> {
    let (w, pos) = crate::tree::find_pane_by_id_global(app, id)?;
    crate::tree::get_nth_pane(&app.windows[w].root, pos)?
        .pane_options
        .get(name)
        .cloned()
}

/// `list-panes -a -F fmt`, one line per pane in window order.
fn list_panes(app: &AppState, fmt: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for (w, window) in app.windows.iter().enumerate() {
        let n = crate::tree::count_panes(&window.root);
        for pos in 0..n {
            lines.push(crate::format::expand_format_for_pane(fmt, app, w, pos));
        }
    }
    lines
}

// ─────────────────────────────────────────────────────────────────────────
// SET / SHOW / UNSET
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn reporter_command_stores_the_value_on_the_pane() {
    let mut app = app_with_windows(&[&[1, 3]]);
    let reply = set(&mut app, "%1", "@omx_team_pane_owner_id", "1");
    assert_eq!(reply, "", "the reporter's exact command must succeed");
    assert_eq!(own(&app, 1, "@omx_team_pane_owner_id").as_deref(), Some("1"));
    assert_eq!(own(&app, 3, "@omx_team_pane_owner_id"), None, "the sibling pane is untouched");
    assert!(
        !app.user_options.contains_key("@omx_team_pane_owner_id"),
        "a pane write must not leak into the global store"
    );
}

#[test]
fn untargeted_write_lands_on_the_active_pane() {
    let mut app = app_with_windows(&[&[1, 3]]);
    app.windows[0].active_path = vec![1];
    assert_eq!(set(&mut app, "", "@cur", "curval"), "");
    assert_eq!(own(&app, 3, "@cur").as_deref(), Some("curval"));
    assert_eq!(own(&app, 1, "@cur"), None);
}

#[test]
fn target_in_another_window_resolves_by_id() {
    let mut app = app_with_windows(&[&[1], &[4, 5]]);
    assert_eq!(set(&mut app, "%5", "@x", "five"), "");
    assert_eq!(own(&app, 5, "@x").as_deref(), Some("five"));
    // `N` without the percent sign is the same pane id, as before #728.
    assert_eq!(set(&mut app, "4", "@x", "four"), "");
    assert_eq!(own(&app, 4, "@x").as_deref(), Some("four"));
}

#[test]
fn session_window_pane_spec_resolves_instead_of_meaning_the_active_pane() {
    let mut app = app_with_windows(&[&[1], &[4, 5]]);
    assert_eq!(set(&mut app, "issue728:1.1", "@x", "spec"), "");
    assert_eq!(own(&app, 5, "@x").as_deref(), Some("spec"));
    assert_eq!(own(&app, 1, "@x"), None, "the active pane must not absorb a targeted write");
}

#[test]
fn missing_pane_is_an_error_not_a_silent_write() {
    let mut app = app_with_windows(&[&[1]]);
    let reply = set(&mut app, "%999", "@x", "1");
    assert_eq!(reply, "ERROR: can't find pane: %999");
    assert!(app.user_options.is_empty() || !app.user_options.contains_key("@x"));
}

#[test]
fn unset_removes_only_the_pane_entry() {
    let mut app = app_with_windows(&[&[1, 3]]);
    set(&mut app, "%1", "@x", "1");
    set(&mut app, "%3", "@x", "3");
    let reply = apply_set_pane_option(&mut app, "%1", "@x", "", true, false, false, false);
    assert_eq!(reply, "");
    assert_eq!(own(&app, 1, "@x"), None);
    assert_eq!(own(&app, 3, "@x").as_deref(), Some("3"));
}

#[test]
fn unset_of_an_absent_option_is_not_an_error() {
    // tmux 3.4: `set -pu -t %0 @nonexistent` is rc 0 with no output.
    let mut app = app_with_windows(&[&[1]]);
    let reply = apply_set_pane_option(&mut app, "%1", "@nonexistent", "", true, false, false, false);
    assert_eq!(reply, "");
}

#[test]
fn explicit_empty_value_is_stored_not_unset() {
    // tmux stores an empty string for `set -p @x ""`; only -u removes.
    let mut app = app_with_windows(&[&[1]]);
    assert_eq!(set(&mut app, "%1", "@x", ""), "");
    assert_eq!(own(&app, 1, "@x").as_deref(), Some(""));
}

#[test]
fn append_extends_the_pane_value() {
    let mut app = app_with_windows(&[&[1]]);
    set(&mut app, "%1", "@a", "x");
    let reply = apply_set_pane_option(&mut app, "%1", "@a", "y", false, true, false, false);
    assert_eq!(reply, "");
    assert_eq!(own(&app, 1, "@a").as_deref(), Some("xy"));
}

#[test]
fn append_never_copies_an_inherited_value() {
    // options_set_string looks the name up with options_get_only: only the
    // pane's OWN value is extended.
    let mut app = app_with_windows(&[&[1]]);
    app.user_options.insert("@a".to_string(), "G".to_string());
    apply_set_pane_option(&mut app, "%1", "@a", "y", false, true, false, false);
    assert_eq!(own(&app, 1, "@a").as_deref(), Some("y"));
    assert_eq!(app.user_options.get("@a").map(String::as_str), Some("G"));
}

#[test]
fn only_if_unset_refuses_an_owned_option_and_q_silences_it() {
    let mut app = app_with_windows(&[&[1]]);
    set(&mut app, "%1", "@a", "xy");
    let reply = apply_set_pane_option(&mut app, "%1", "@a", "z", false, false, true, false);
    assert_eq!(reply, "ERROR: already set: @a");
    let quiet = apply_set_pane_option(&mut app, "%1", "@a", "z", false, false, true, true);
    assert_eq!(quiet, "");
    assert_eq!(own(&app, 1, "@a").as_deref(), Some("xy"), "-o never overwrites");
}

#[test]
fn only_if_unset_writes_when_the_pane_inherits() {
    // A global value does not count as "set" on the pane.
    let mut app = app_with_windows(&[&[1]]);
    app.user_options.insert("@a".to_string(), "G".to_string());
    let reply = apply_set_pane_option(&mut app, "%1", "@a", "z", false, false, true, false);
    assert_eq!(reply, "");
    assert_eq!(own(&app, 1, "@a").as_deref(), Some("z"));
}

// ─────────────────────────────────────────────────────────────────────────
// THE OLD TWO NAMES KEEP THEIR CONTRACT
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn remain_on_exit_and_mouse_force_keep_their_validation() {
    let mut app = app_with_windows(&[&[1]]);
    assert_eq!(set(&mut app, "%1", "remain-on-exit", "on"), "");
    assert_eq!(own(&app, 1, "remain-on-exit").as_deref(), Some("on"));
    assert!(set(&mut app, "%1", "remain-on-exit", "maybe").starts_with("ERROR: set-option -p remain-on-exit: bad value"));
    assert_eq!(set(&mut app, "%1", "@mouse-force", "on"), "");
    assert!(set(&mut app, "%1", "@mouse-force", "loud").starts_with("ERROR: set-option -p @mouse-force: bad value"));
    // An empty value still unsets these two, as it always did.
    assert_eq!(set(&mut app, "%1", "remain-on-exit", ""), "");
    assert_eq!(own(&app, 1, "remain-on-exit"), None);
    assert_eq!(
        apply_set_pane_option(&mut app, "%1", "@mouse-force", "", true, false, false, false),
        ""
    );
    assert_eq!(own(&app, 1, "@mouse-force"), None);
}

#[test]
fn a_catalog_name_psmux_does_not_keep_per_pane_is_still_refused() {
    let mut app = app_with_windows(&[&[1]]);
    let reply = set(&mut app, "%1", "bogus-opt", "1");
    assert!(reply.starts_with("ERROR: pane-scoped option 'bogus-opt' is not supported"), "{reply}");
    assert_eq!(own(&app, 1, "bogus-opt"), None);
    let bare_at = set(&mut app, "%1", "@", "1");
    assert!(bare_at.starts_with("ERROR:"), "a bare `@` is not a user option name: {bare_at}");
}

// ─────────────────────────────────────────────────────────────────────────
// SHOW-OPTIONS -p <name>
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn query_of_a_missing_user_option_is_invalid_option_unless_quiet() {
    let none = |_: &str| -> Option<String> { None };
    assert_eq!(
        pane_option_query_reply("", "@omx_team_pane_owner_id", true, false, false, none),
        "ERROR: invalid option: @omx_team_pane_owner_id\n"
    );
    assert_eq!(pane_option_query_reply("", "@omx_team_pane_owner_id", true, false, true, none), "");
}

#[test]
fn query_of_an_owned_user_option_prints_the_value() {
    let none = |_: &str| -> Option<String> { None };
    let listing = "@a xy\n@omx_team_pane_owner_id 1";
    assert_eq!(pane_option_query_reply(listing, "@omx_team_pane_owner_id", true, false, true, none), "1\n");
    assert_eq!(pane_option_query_reply(listing, "@a", false, false, false, none), "@a xy\n");
}

#[test]
fn query_with_a_inherits_the_parent_value() {
    // tmux: `show -pvA -t %1 @x` prints the window/global value.
    let parent = |_: &str| -> Option<String> { Some("W".to_string()) };
    assert_eq!(pane_option_query_reply("", "@x", true, true, false, parent), "W\n");
}

#[test]
fn query_of_a_missing_catalog_option_still_prints_nothing() {
    // The #647 contract for remain-on-exit is unchanged.
    let none = |_: &str| -> Option<String> { None };
    assert_eq!(pane_option_query_reply("", "remain-on-exit", true, false, false, none), "");
}

// ─────────────────────────────────────────────────────────────────────────
// FORMAT EXPANSION AND THE INHERITANCE ORDER
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn list_panes_expands_each_panes_own_value() {
    let mut app = app_with_windows(&[&[1, 3]]);
    set(&mut app, "%1", "@owner", "lead");
    set(&mut app, "%3", "@owner", "worker");
    assert_eq!(
        list_panes(&app, "#{pane_id} [#{@owner}]"),
        vec!["%1 [lead]".to_string(), "%3 [worker]".to_string()]
    );
}

#[test]
fn pane_value_wins_over_the_inherited_one_and_unset_falls_back() {
    let mut app = app_with_windows(&[&[1, 3]]);
    set(&mut app, "%1", "@x", "1");
    app.user_options.insert("@x".to_string(), "G".to_string());
    assert_eq!(list_panes(&app, "#{pane_id} [#{@x}]"), vec!["%1 [1]", "%3 [G]"]);
    apply_set_pane_option(&mut app, "%1", "@x", "", true, false, false, false);
    assert_eq!(list_panes(&app, "#{pane_id} [#{@x}]"), vec!["%1 [G]", "%3 [G]"]);
}

#[test]
fn unset_everywhere_expands_empty() {
    let app = app_with_windows(&[&[1]]);
    assert_eq!(list_panes(&app, "[#{@nothing}]"), vec!["[]"]);
}

#[test]
fn conditionals_and_comparisons_see_the_pane_value() {
    let mut app = app_with_windows(&[&[1, 3]]);
    set(&mut app, "%3", "@role", "worker");
    assert_eq!(
        list_panes(&app, "#{?#{==:#{@role},worker},W,-}"),
        vec!["-", "W"]
    );
    assert_eq!(list_panes(&app, "#{?@role,set,unset}"), vec!["unset", "set"]);
}

#[test]
fn display_message_for_the_active_pane_uses_its_value() {
    let mut app = app_with_windows(&[&[1, 3]]);
    app.windows[0].active_path = vec![1];
    set(&mut app, "%3", "@x", "three");
    assert_eq!(crate::format::expand_format("#{@x}", &app), "three");
}

#[test]
fn pane_helper_chain_is_pane_then_store() {
    let mut app = app_with_windows(&[&[1]]);
    app.user_options.insert("@x".to_string(), "G".to_string());
    assert_eq!(pane_user_option(&app, None, "@x").as_deref(), Some("G"));
    set(&mut app, "%1", "@x", "P");
    let pane = crate::tree::get_nth_pane(&app.windows[0].root, 0);
    assert_eq!(pane_user_option(&app, pane, "@x").as_deref(), Some("P"));
}

// ─────────────────────────────────────────────────────────────────────────
// THE OPTION LIVES ON THE PANE: IT FOLLOWS IT
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn break_pane_carries_the_option_into_the_new_window() {
    let mut app = app_with_windows(&[&[1, 2]]);
    set(&mut app, "%2", "@mv", "moved");
    let req = BreakPaneRequest { detach: true, src: Some("%2".to_string()), ..Default::default() };
    crate::window_ops::break_pane(&mut app, &req).expect("breaks");
    assert_eq!(app.windows.len(), 2);
    assert_eq!(crate::tree::collect_pane_ids(&app.windows[1].root), vec![2]);
    assert_eq!(own(&app, 2, "@mv").as_deref(), Some("moved"));
    assert_eq!(list_panes(&app, "#{pane_id}=#{@mv}"), vec!["%1=", "%2=moved"]);
}

#[test]
fn swap_pane_across_windows_keeps_each_value_with_its_pane() {
    let mut app = app_with_windows(&[&[1], &[2]]);
    set(&mut app, "%2", "@mv", "moved");
    crate::window_ops::swap_pane_by_spec(&mut app, Some("%1"), "%2", false).expect("swaps");
    assert_eq!(crate::tree::collect_pane_ids(&app.windows[0].root), vec![2]);
    assert_eq!(list_panes(&app, "#{pane_id}=#{@mv}"), vec!["%2=moved", "%1="]);
}

#[test]
fn killing_the_pane_drops_its_options() {
    // The store lives on the Pane, so a pane that leaves the tree takes it
    // along; a later pane never inherits a dead pane's value.
    let mut app = app_with_windows(&[&[1, 2]]);
    set(&mut app, "%2", "@x", "gone");
    if let Node::Split { children, sizes, .. } = &mut app.windows[0].root {
        children.pop();
        sizes.pop();
    }
    assert!(crate::tree::find_pane_by_id_global(&app, 2).is_none());
    assert_eq!(list_panes(&app, "[#{@x}]"), vec!["[]"]);
}

// ─────────────────────────────────────────────────────────────────────────
// COMMAND PROMPT / SOURCE-FILE ROUTE (config.rs parse_set_option)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn config_route_writes_the_pane_not_the_global_store() {
    let mut app = app_with_windows(&[&[1, 3]]);
    crate::config::parse_config_line(&mut app, "set-option -p -t %3 @owner worker");
    assert_eq!(own(&app, 3, "@owner").as_deref(), Some("worker"));
    assert!(!app.user_options.contains_key("@owner"));
    crate::config::parse_config_line(&mut app, "set -pa -t %3 @owner 2");
    assert_eq!(own(&app, 3, "@owner").as_deref(), Some("worker2"));
    crate::config::parse_config_line(&mut app, "set -pu -t %3 @owner");
    assert_eq!(own(&app, 3, "@owner"), None);
}

#[test]
fn config_route_untargeted_write_hits_the_active_pane() {
    let mut app = app_with_windows(&[&[1, 3]]);
    app.windows[0].active_path = vec![1];
    crate::config::parse_config_line(&mut app, "set -p @here yes");
    assert_eq!(own(&app, 3, "@here").as_deref(), Some("yes"));
    assert_eq!(own(&app, 1, "@here"), None);
}
