// Issue #640: `switch-client -T <table>` never switched the client's key table.
//
// The reporter's config was
//
//     bind-key s switch-client -T SPLIT
//     bind-key -T SPLIT v split-window -h -c "#{pane_current_path}"
//     bind-key -T SPLIT h split-window -v -c "#{pane_current_path}"
//
// and after prefix + s the next key went straight to the shell: a literal `v`
// appeared at the prompt instead of a horizontal split.
//
// Root cause: the attached client's key dispatcher (`src/client.rs`) knew only
// the `root` and `prefix` tables. Every binding whose command started with
// `switch-client` was routed to the session-navigation branch
// (`do_session_nav = Some(cmd.contains("-n"))`), so `-T SPLIT` was read as
// "previous session", nothing was latched, and the following key fell through
// to the pane.
//
// tmux parity, cmd-switch-client.c:
//
//     tablename = args_get(args, 'T');
//     if (tablename != NULL) {
//             table = key_bindings_get_table(tablename, 0);
//             if (table == NULL) {
//                     cmdq_error(item, "table %s doesn't exist", tablename);
//                     return (CMD_RETURN_ERROR);
//             }
//             table->references++;
//             key_bindings_unref_table(tc->keytable);
//             tc->keytable = table;
//             return (CMD_RETURN_NORMAL);
//     }
//
// `-T` returns immediately, before any of the `-n`/`-p`/`-l` session handling,
// and `key_bindings_get_table(..., 0)` does NOT create the table, so an unknown
// name is an error rather than a latch that eats the next key.
//
// server-client.c then keeps the client in that table until the next key is
// dispatched: the prefix always wins and forces the prefix table, a key found
// in the custom table fires and drops the client back to the default table, a
// key missing there is retried in `root`, and a key missing in both is
// swallowed rather than forwarded to the pane ("if (first != table) ... goto
// out").
//
// The live keystroke route is covered by
// tests/test_issue640_switch_client_table.ps1.

#[allow(unused_imports)]
use super::*;

use crate::format::expand_var;
use crate::server::helpers::resolve_switch_client_table;
use crate::types::{AppState, Bind};

fn fresh_app() -> AppState {
    let mut app = AppState::new("i640_probe".to_string());
    // `expand_var` resolves client variables against a window, so a bare
    // `AppState::new` (no windows) would answer every one of them with "".
    app.windows.push(crate::types::Window {
        root: crate::types::Node::Split {
            kind: crate::types::LayoutKind::Horizontal,
            sizes: vec![],
            children: vec![],
        },
        active_path: vec![],
        name: "w0".to_string(),
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
    app.key_tables.insert(
        "SPLIT".to_string(),
        vec![Bind {
            key: (crossterm::event::KeyCode::Char('v'), crossterm::event::KeyModifiers::NONE),
            action: crate::types::Action::Command("split-window -h".to_string()),
            repeat: false,
            note: None,
        }],
    );
    app
}

// ---------------------------------------------------------------------------
// The client-side parse: which `switch-client` forms latch a table.
// ---------------------------------------------------------------------------

#[test]
fn issue640_dash_t_table_is_extracted() {
    assert_eq!(
        switch_client_table_arg("switch-client -T SPLIT"),
        Some("SPLIT".to_string()),
        "the reporter's binding must be recognised as a key-table switch"
    );
}

#[test]
fn issue640_alias_and_glued_forms_are_extracted() {
    assert_eq!(switch_client_table_arg("switchc -T SPLIT"), Some("SPLIT".to_string()));
    // tmux's getopt accepts the value glued to the flag.
    assert_eq!(switch_client_table_arg("switch-client -TSPLIT"), Some("SPLIT".to_string()));
}

#[test]
fn issue640_quoted_table_name_survives_tokenizing() {
    assert_eq!(
        switch_client_table_arg("switch-client -T \"my table\""),
        Some("my table".to_string()),
        "the table name is tokenized the same way the server dispatcher does"
    );
}

#[test]
fn issue640_session_navigation_forms_do_not_latch() {
    // These are the default `(` and `)` bindings and must keep going to the
    // session-navigation branch.
    assert_eq!(switch_client_table_arg("switch-client -n"), None);
    assert_eq!(switch_client_table_arg("switch-client -p"), None);
    assert_eq!(switch_client_table_arg("switch-client -l"), None);
    assert_eq!(switch_client_table_arg("switch-client -t other"), None);
    // A bare `-T` with no operand is not a table switch either.
    assert_eq!(switch_client_table_arg("switch-client -T"), None);
}

#[test]
fn issue640_other_commands_are_never_treated_as_switch_client() {
    // `-T` means something else entirely on these.
    assert_eq!(switch_client_table_arg("send-keys -T Escape"), None);
    assert_eq!(switch_client_table_arg("split-window -h"), None);
}

// ---------------------------------------------------------------------------
// The server-side resolve: tmux errors on an unknown table.
// ---------------------------------------------------------------------------

#[test]
fn issue640_unknown_table_is_an_error_not_a_latch() {
    let app = fresh_app();
    let err = resolve_switch_client_table(&app, "NOPE").unwrap_err();
    assert_eq!(
        err, "table NOPE doesn't exist",
        "tmux cmd-switch-client.c reports the missing table verbatim"
    );
}

#[test]
fn issue640_existing_table_resolves_to_itself() {
    let app = fresh_app();
    assert_eq!(resolve_switch_client_table(&app, "SPLIT"), Ok(Some("SPLIT".to_string())));
}

#[test]
fn issue640_root_resolves_to_the_default_table() {
    let app = fresh_app();
    assert_eq!(
        resolve_switch_client_table(&app, "root"),
        Ok(None),
        "root is the default table, stored as None so the pane's own mode still shows"
    );
}

#[test]
fn issue640_prefix_table_always_exists() {
    let mut app = fresh_app();
    app.key_tables.remove("prefix");
    assert_eq!(
        resolve_switch_client_table(&app, "prefix"),
        Ok(Some("prefix".to_string())),
        "root and prefix are always live in tmux even with no bindings left"
    );
}

// ---------------------------------------------------------------------------
// `#{client_key_table}` must report the latched table, which is how the E2E
// test observes the latch without guessing at the screen.
// ---------------------------------------------------------------------------

#[test]
fn issue640_client_key_table_reports_the_latched_table() {
    let mut app = fresh_app();
    assert_eq!(expand_var("client_key_table", &app, 0), "root");
    app.current_key_table = Some("SPLIT".to_string());
    assert_eq!(expand_var("client_key_table", &app, 0), "SPLIT");
    app.current_key_table = None;
    assert_eq!(expand_var("client_key_table", &app, 0), "root");
}

#[test]
fn issue640_prefix_outranks_the_latched_table() {
    // tmux: "The prefix always takes precedence and forces a switch to the
    // prefix table".
    let mut app = fresh_app();
    app.current_key_table = Some("SPLIT".to_string());
    app.client_prefix_active = true;
    assert_eq!(expand_var("client_key_table", &app, 0), "prefix");
}

#[test]
fn issue640_detaching_client_drops_the_latch() {
    let mut app = fresh_app();
    app.current_key_table = Some("SPLIT".to_string());
    app.client_prefix_active = true;
    let cid = 1u64;
    assert!(app.register_client(cid, false));
    assert!(app.reap_client(cid));
    assert_eq!(
        app.current_key_table, None,
        "a half-typed chord must not survive the client that started it"
    );
}
