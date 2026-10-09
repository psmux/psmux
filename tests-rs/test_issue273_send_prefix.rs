// Issue #273: Pressing the prefix key twice should jump to the start of the
// command line in the inner shell (e.g. nushell with prefix=C-a, where C-a is
// "go to start of line").  In tmux this works because there's a default
// `bind C-b send-prefix` in the prefix table; users who change the prefix
// typically also `bind <new-prefix> send-prefix`.
//
// psmux now does both:
//   1. Ships `bind C-b send-prefix` as a default (matches tmux exactly).
//   2. Auto-binds the prefix key to send-prefix whenever it changes via
//      `set -g prefix <key>`, so `set -g prefix C-a` "just works".

use super::*;
use crate::config::{ensure_prefix_self_binding, populate_default_bindings};
use crate::types::Action;
use crossterm::event::{KeyCode, KeyModifiers};

fn fresh_app() -> AppState {
    let mut app = AppState::new("issue273".to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    populate_default_bindings(&mut app);
    app
}

fn prefix_table_send_prefix_keys(app: &AppState) -> Vec<(KeyCode, KeyModifiers)> {
    app.key_tables
        .get("prefix")
        .map(|t| {
            t.iter()
                .filter_map(|b| match &b.action {
                    Action::Command(c) if c == "send-prefix" => Some(b.key),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn default_prefix_table_binds_c_b_to_send_prefix() {
    // tmux ships `bind C-b send-prefix` by default; psmux must too.
    let app = fresh_app();
    let send_prefix_keys = prefix_table_send_prefix_keys(&app);
    let c_b = (KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(
        send_prefix_keys.contains(&c_b),
        "expected default prefix table to contain C-b -> send-prefix, got {:?}",
        send_prefix_keys
    );
}

#[test]
fn changing_prefix_to_c_a_auto_binds_c_a_to_send_prefix() {
    // The user's reported case: `set -g prefix C-a` should make pressing
    // C-a twice forward a literal C-a to the inner shell.
    let mut app = fresh_app();
    let c_a = (KeyCode::Char('a'), KeyModifiers::CONTROL);

    // Simulate the option change.
    app.prefix_key = c_a;
    ensure_prefix_self_binding(&mut app);

    let send_prefix_keys = prefix_table_send_prefix_keys(&app);
    assert!(
        send_prefix_keys.contains(&c_a),
        "expected C-a -> send-prefix after `set -g prefix C-a`, got {:?}",
        send_prefix_keys
    );
}

#[test]
fn ensure_prefix_self_binding_does_not_clobber_user_override() {
    // If the user has explicitly bound the prefix key to something else,
    // we must NOT overwrite it with send-prefix.
    let mut app = fresh_app();
    let c_a = (KeyCode::Char('a'), KeyModifiers::CONTROL);
    app.prefix_key = c_a;

    // User binds C-a to a custom command first.
    let table = app.key_tables.entry("prefix".to_string()).or_default();
    table.push(crate::types::Bind {
        key: c_a,
        action: Action::Command("display-message custom".into()),
        repeat: false,
        note: None,
    });

    ensure_prefix_self_binding(&mut app);

    let table = app.key_tables.get("prefix").expect("prefix table");
    let bind_for_c_a = table.iter().find(|b| b.key == c_a).expect("C-a bound");
    match &bind_for_c_a.action {
        Action::Command(c) => assert_eq!(c, "display-message custom",
            "user override must be preserved, but got {:?}", c),
        _ => panic!("expected user's Command action for C-a"),
    }

    // And there should be exactly one binding for C-a (no duplicate added).
    let c_a_count = table.iter().filter(|b| b.key == c_a).count();
    assert_eq!(c_a_count, 1, "user override must not be duplicated");
}

#[test]
fn send_prefix_command_dispatches_with_c_a_prefix() {
    // The send-prefix command itself must work — i.e. dispatch without panic
    // when the prefix is C-a (regression for the issue's primary use case).
    let mut app = fresh_app();
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: "shell".into(),
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
    app.prefix_key = (KeyCode::Char('a'), KeyModifiers::CONTROL);

    crate::commands::execute_command_string(&mut app, "send-prefix").unwrap();
}
