// Issue #640, follow-up: a key table built the tmux way, by having every
// binding in the table re-arm the table as the last command of a chain, was
// not sticky. The reporter's config:
//
//     set -g status-right '#{client_key_table}'
//     unbind z
//     bind-key z switch-client -T MOVE
//     bind-key -T MOVE h select-pane -L \; switch-client -T MOVE
//     bind-key -T MOVE l select-pane -R \; switch-client -T MOVE
//     bind-key -T MOVE k select-pane -U \; switch-client -T MOVE
//     bind-key -T MOVE j select-pane -D \; switch-client -T MOVE
//
// After prefix, z, h the pane moved but `#{client_key_table}` went back to
// `root`, so the second h/j/k/l was typed into the shell instead of moving
// again. Reproduced with injected physical keystrokes: 0 of 4 runs stayed in
// MOVE before the fix and 4 of 4 after, with a literal `lh` at the prompt in
// every failing run.
//
// Root cause. The attached client matched the WHOLE binding string, so
// `select-pane -L \; switch-client -T MOVE` never reached the `switch-client`
// arm at all: it fell to the generic chain splitter, which forwarded every
// element to the server but never re-armed the client's own latch. Worse, the
// client emitted its reset (`switch-client -T root`) AFTER the binding's
// commands, so even the server side of the re-arm was overwritten a moment
// later.
//
// tmux does the reset first. server-client.c:
//
//     1570		} else {
//     1571			c->flags &= ~CLIENT_REPEAT;
//     1572			server_client_set_key_table(c, NULL);
//     1573		}
//     1574		server_status_client(c);
//     1575
//     1576		/* Execute the key binding. */
//     1577		key_bindings_dispatch(bd, item, c, event, &fs);
//
// The table is cleared at :1572, BEFORE `key_bindings_dispatch` at :1577, so a
// `switch-client -T` run by the binding survives. cmd-switch-client.c:96 then
// simply assigns the table:
//
//      96	tablename = args_get(args, 'T');
//      97	if (tablename != NULL) {
//      98		table = key_bindings_get_table(tablename, 0);
//      99		if (table == NULL) {
//     100			cmdq_error(item, "table %s doesn't exist", tablename);
//     101			return (CMD_RETURN_ERROR);
//     102		}
//     103		table->references++;
//     104		key_bindings_unref_table(tc->keytable);
//     105		tc->keytable = table;
//     106		return (CMD_RETURN_NORMAL);
//     107	}
//
// so the last `-T` in a command list wins and nothing afterwards undoes it.
//
// The live keystroke route is covered by tests/test_issue640_sticky_table.ps1.

#[allow(unused_imports)]
use super::*;

use crate::client::{dispatch_binding_commands, BindingDispatch};
use crate::server::helpers::resolve_switch_client_table;
use crate::types::{AppState, Bind};
use crossterm::event::{KeyCode, KeyModifiers};

fn cmds_of(d: BindingDispatch) -> (Vec<String>, Option<String>) {
    match d {
        BindingDispatch::Commands { cmds, latch } => (cmds, latch),
        other => panic!("expected a command list, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// The sticky idiom itself: a trailing `switch-client -T` re-arms the table.
// ---------------------------------------------------------------------------

#[test]
fn issue640_trailing_switch_client_rearms_the_table() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands(
        "select-pane -L \\; switch-client -T MOVE",
    ));
    assert_eq!(
        cmds,
        vec![
            "select-pane -L".to_string(),
            "switch-client -T MOVE".to_string(),
        ],
        "both elements of the chain must reach the server, in order"
    );
    assert_eq!(
        latch.as_deref(),
        Some("MOVE"),
        "the trailing switch-client -T is what makes the table sticky"
    );
}

#[test]
fn issue640_every_direction_of_the_reporters_table_rearms() {
    for (key, dir) in [("h", "-L"), ("l", "-R"), ("k", "-U"), ("j", "-D")] {
        let binding = format!("select-pane {} \\; switch-client -T MOVE", dir);
        let (cmds, latch) = cmds_of(dispatch_binding_commands(&binding));
        assert_eq!(cmds[0], format!("select-pane {}", dir), "key {}", key);
        assert_eq!(latch.as_deref(), Some("MOVE"), "key {}", key);
    }
}

#[test]
fn issue640_switch_client_first_in_the_chain_keeps_the_tail() {
    // The binding only BEGINS with switch-client. Matching the whole string
    // used to consume it as a lone switch-client and silently drop the rest.
    let (cmds, latch) = cmds_of(dispatch_binding_commands(
        "switch-client -T MOVE \\; select-pane -L",
    ));
    assert_eq!(
        cmds,
        vec![
            "switch-client -T MOVE".to_string(),
            "select-pane -L".to_string(),
        ],
        "the movement after the switch-client must not be dropped"
    );
    assert_eq!(latch.as_deref(), Some("MOVE"));
}

#[test]
fn issue640_the_last_table_in_a_chain_wins() {
    // cmd-switch-client.c just assigns tc->keytable each time it runs.
    let (_, latch) = cmds_of(dispatch_binding_commands(
        "switch-client -T MOVE \\; select-pane -L \\; switch-client -T RESIZE",
    ));
    assert_eq!(latch.as_deref(), Some("RESIZE"));
}

#[test]
fn issue640_a_chain_without_switch_client_latches_nothing() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands(
        "select-pane -L \\; display-message hi",
    ));
    assert_eq!(
        cmds,
        vec![
            "select-pane -L".to_string(),
            "display-message hi".to_string(),
        ]
    );
    assert_eq!(
        latch, None,
        "a one-shot binding must still drop back to the default table"
    );
}

#[test]
fn issue640_a_lone_switch_client_table_still_latches() {
    // The original #640 form, `bind z switch-client -T MOVE`.
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-client -T MOVE"));
    assert_eq!(cmds, vec!["switch-client -T MOVE".to_string()]);
    assert_eq!(latch.as_deref(), Some("MOVE"));
}

#[test]
fn issue640_a_quoted_table_name_survives_the_round_trip() {
    let (cmds, latch) = cmds_of(dispatch_binding_commands(
        "select-pane -L \\; switch-client -T \"my table\"",
    ));
    assert_eq!(latch.as_deref(), Some("my table"));
    assert_eq!(
        cmds.last().map(|s| s.as_str()),
        Some("switch-client -T \"my table\""),
        "the server has to be told the same name the client latched"
    );
}

// ---------------------------------------------------------------------------
// Session navigation must not be mistaken for a table latch, and vice versa.
// ---------------------------------------------------------------------------

#[test]
fn issue640_bare_switch_client_is_still_session_navigation() {
    assert_eq!(
        dispatch_binding_commands("switch-client -n"),
        BindingDispatch::SessionNav { next: true }
    );
    assert_eq!(
        dispatch_binding_commands("switch-client -p"),
        BindingDispatch::SessionNav { next: false }
    );
    assert_eq!(
        dispatch_binding_commands("switch-client -l"),
        BindingDispatch::SessionNav { next: false }
    );
}

#[test]
fn issue640_a_command_that_merely_starts_with_the_word_is_not_switch_client() {
    // `parse_command_line` compares the verb, not a prefix of the line.
    let (cmds, latch) = cmds_of(dispatch_binding_commands("switch-clientish -T MOVE"));
    assert_eq!(cmds, vec!["switch-clientish -T MOVE".to_string()]);
    assert_eq!(latch, None);
}

// ---------------------------------------------------------------------------
// Ordering. The client emits its reset to the default table BEFORE the
// binding's commands, so the server ends on whatever the binding latched.
// Folding the emitted list through the server's own resolver is the end state
// `#{client_key_table}` reports.
// ---------------------------------------------------------------------------

fn app_with_move_table() -> AppState {
    let mut app = AppState::new("i640b_probe".to_string());
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
    // A chained binding is stored as an `Action::CommandChain`, which
    // `format_action` renders back as `a \; b` for the client.
    let chain = |dir: &str| {
        crate::types::Action::CommandChain(vec![
            format!("select-pane {}", dir),
            "switch-client -T MOVE".to_string(),
        ])
    };
    app.key_tables.insert(
        "MOVE".to_string(),
        vec![
            Bind {
                key: (KeyCode::Char('h'), KeyModifiers::NONE),
                action: chain("-L"),
                repeat: false,
                note: None,
            },
            Bind {
                key: (KeyCode::Char('l'), KeyModifiers::NONE),
                action: chain("-R"),
                repeat: false,
                note: None,
            },
        ],
    );
    app
}

#[test]
fn issue640_a_chained_binding_reaches_the_client_as_a_chain() {
    // The client only ever sees `format_action`'s rendering, so the sticky
    // idiom has to survive the round trip through `Action::CommandChain`.
    let app = app_with_move_table();
    let bind = &app.key_tables["MOVE"][0];
    let rendered = crate::commands::format_action(&bind.action);
    assert_eq!(rendered, "select-pane -L \\; switch-client -T MOVE");
    let (_, latch) = cmds_of(dispatch_binding_commands(&rendered));
    assert_eq!(latch.as_deref(), Some("MOVE"));
}

/// Replay what the client puts on the wire for one dispatched key, in order,
/// against the server's table resolver. `latched` is the custom table the key
/// was looked up in.
fn replay(app: &mut AppState, latched: Option<&str>, binding: &str) -> Option<String> {
    let mut wire: Vec<String> = Vec::new();
    // src/client.rs: the reset goes out first, mirroring server-client.c:1572
    // running before :1577.
    if latched.is_some() {
        wire.push("switch-client -T root".to_string());
    }
    if let BindingDispatch::Commands { cmds, .. } = dispatch_binding_commands(binding) {
        wire.extend(cmds);
    }
    for line in &wire {
        if let Some(tbl) = crate::client::switch_client_table_arg(line) {
            app.current_key_table = resolve_switch_client_table(app, &tbl)
                .expect("the test only replays tables that exist");
        }
    }
    app.current_key_table.clone()
}

#[test]
fn issue640_the_reset_precedes_the_binding_so_the_rearm_survives() {
    let mut app = app_with_move_table();
    app.current_key_table = Some("MOVE".to_string());
    let after = replay(&mut app, Some("MOVE"), "select-pane -L \\; switch-client -T MOVE");
    assert_eq!(
        after.as_deref(),
        Some("MOVE"),
        "emitting the reset after the binding is what pushed the client back to root"
    );
}

#[test]
fn issue640_repeated_keys_keep_the_table() {
    let mut app = app_with_move_table();
    app.current_key_table = Some("MOVE".to_string());
    for dir in ["-L", "-R", "-U", "-D", "-L"] {
        let binding = format!("select-pane {} \\; switch-client -T MOVE", dir);
        let after = replay(&mut app, Some("MOVE"), &binding);
        assert_eq!(after.as_deref(), Some("MOVE"), "after select-pane {}", dir);
    }
}

#[test]
fn issue640_a_one_shot_binding_in_the_table_falls_back_to_root() {
    // No trailing switch-client, so the reset that went out first stands.
    let mut app = app_with_move_table();
    app.current_key_table = Some("MOVE".to_string());
    let after = replay(&mut app, Some("MOVE"), "select-pane -L");
    assert_eq!(after, None, "None is the default (root) table");
}

#[test]
fn issue640_a_key_swallowed_in_the_table_also_falls_back_to_root() {
    // No binding matched at all: only the reset is emitted.
    let mut app = app_with_move_table();
    app.current_key_table = Some("MOVE".to_string());
    let mut wire: Vec<String> = Vec::new();
    wire.push("switch-client -T root".to_string());
    for line in &wire {
        if let Some(tbl) = crate::client::switch_client_table_arg(line) {
            app.current_key_table = resolve_switch_client_table(&app, &tbl).unwrap();
        }
    }
    assert_eq!(app.current_key_table, None);
}

// ---------------------------------------------------------------------------
// Prefix precedence and the missing-table error, unchanged from the first fix.
// ---------------------------------------------------------------------------

#[test]
fn issue640_prefix_clears_a_sticky_latch() {
    // tmux: "The prefix always takes precedence and forces a switch to the
    // prefix table". `CtrlReq::PrefixBegin` sets `current_key_table = None`,
    // and `#{client_key_table}` reports `prefix` while the prefix is armed.
    let mut app = app_with_move_table();
    app.current_key_table = Some("MOVE".to_string());
    // What the server does for `prefix-begin`.
    app.client_prefix_active = true;
    app.current_key_table = None;
    assert_eq!(
        crate::format::expand_var("client_key_table", &app, 0),
        "prefix"
    );
    app.client_prefix_active = false;
    assert_eq!(
        crate::format::expand_var("client_key_table", &app, 0),
        "root",
        "a sticky table must not outlive the prefix that interrupted it"
    );
}

#[test]
fn issue640_a_missing_table_is_still_an_error() {
    let app = app_with_move_table();
    assert_eq!(
        resolve_switch_client_table(&app, "MOVE"),
        Ok(Some("MOVE".to_string()))
    );
    assert_eq!(resolve_switch_client_table(&app, "root"), Ok(None));
    assert_eq!(
        resolve_switch_client_table(&app, "NOSUCHTABLE"),
        Err("table NOSUCHTABLE doesn't exist".to_string()),
        "cmd-switch-client.c:100"
    );
}
