// new-window -t sess:N, -a, -b, -k and -S against tmux 3.4.
//
// Measured on 23a65aa5 (PSMUX_NO_WARM=1, -L, isolated PSMUX_DATA_DIR), after
// `new-session -d -s sa -n w0`:
//
//   new-window -d -t sa:1        rc 0, NO window      tmux: 0 1
//   new-window -d -t sa:7        rc 0, NO window      tmux: 0 1 7
//   new-window -d -t :3          rc 0, NO window      tmux: 0 1 3 7
//   new-window -d -t sa:0        rc 0, appended one   tmux: rc 1 "create window failed: index 0 in use"
//   new-window -d -k -t sa:0     appended one         tmux: replaces window 0
//   new-window -d -a -t sa:1     appended one         tmux: 0=w0 1=w1 2=A 3=w2 5=w5
//   new-window -d -b -t sa:1     appended one         tmux: 0=w0 1=B 2=w1 3=A 4=w2 5=w5
//
// The connection validated new-window's -t as an EXISTING window (so a free
// index was "can't find window" and the CLI, which never read the reply,
// exited 0), and the server ignored -t/-a/-b/-k/-S and always appended.
// tmux resolves the -t with CMD_FIND_WINDOW_INDEX (cmd-new-window.c) and
// spawn_window refuses an index in use unless SPAWN_KILL (spawn.c).

use super::*;

fn make_window(name: &str, id: usize) -> Window {
    Window {
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

fn app_from(spec: &[(usize, &str)]) -> AppState {
    let mut app = AppState::new("sa".to_string());
    for (i, (_, name)) in spec.iter().enumerate() {
        app.windows.push(make_window(name, i + 1));
    }
    app.window_indices = spec.iter().map(|(idx, _)| *idx).collect();
    app.window_base_index = 0;
    app.ids.set_next_window(spec.len() + 1);
    app
}

fn layout(app: &AppState) -> String {
    (0..app.windows.len())
        .map(|p| format!("{}={}", app.win_display_index(p), app.windows[p].name))
        .collect::<Vec<_>>()
        .join(" ")
}

fn place(t: &str) -> NewWindowPlacement {
    NewWindowPlacement { target: Some(t.to_string()), ..Default::default() }
}

/// What the server does after the plan, minus the PTY: drop a `-k` victim,
/// append the window, then move it to the planned index.
fn apply(app: &mut AppState, plan: NewWindowPlan, name: &str) {
    if let NewWindowPlan::Create { index, kill_pos } = plan {
        if let Some(kp) = kill_pos {
            app.windows.remove(kp);
            app.on_window_removed(kp);
        }
        let id = app.ids.alloc_window();
        app.windows.push(make_window(name, id));
        app.on_window_appended();
        if let Some(i) = index {
            let pos = app.windows.len() - 1;
            app.move_window_to_index(pos, i).expect("planned index must be free");
        }
    }
}

fn run(app: &mut AppState, p: NewWindowPlacement, name: &str) -> Result<(), String> {
    let plan = app.plan_new_window(&p, Some(name), true)?;
    apply(app, plan, name);
    Ok(())
}

#[test]
fn free_index_target_creates_at_that_index() {
    // tmux: 0:w0 then -t sa:1, -t sa:7, -t :3 -> 0 1 3 7
    let mut app = app_from(&[(0, "w0")]);
    run(&mut app, place("sa:1"), "w1").unwrap();
    run(&mut app, place("sa:7"), "w7").unwrap();
    run(&mut app, place(":3"), "w3").unwrap();
    assert_eq!(layout(&app), "0=w0 1=w1 3=w3 7=w7");
}

#[test]
fn bare_number_target_is_a_window_index() {
    // tmux cmd_find_get_window: a bare token is a window of the current
    // session before it is a session.
    let mut app = app_from(&[(0, "w0")]);
    run(&mut app, place("5"), "w5").unwrap();
    assert_eq!(layout(&app), "0=w0 5=w5");
}

#[test]
fn session_only_target_takes_the_next_index() {
    let mut app = app_from(&[(0, "w0"), (1, "w1")]);
    run(&mut app, place("sa"), "n").unwrap();
    run(&mut app, place("sa:"), "m").unwrap();
    run(&mut app, NewWindowPlacement::default(), "o").unwrap();
    assert_eq!(layout(&app), "0=w0 1=w1 2=n 3=m 4=o");
}

#[test]
fn index_in_use_is_refused_with_tmuxs_message() {
    // tmux: create window failed: index 0 in use (rc 1), nothing changes.
    let mut app = app_from(&[(0, "w0"), (3, "w3")]);
    let err = app.plan_new_window(&place("sa:0"), None, true).unwrap_err();
    assert_eq!(err, "create window failed: index 0 in use");
    let err = app.plan_new_window(&place(":3"), None, true).unwrap_err();
    assert_eq!(err, "create window failed: index 3 in use");
    assert_eq!(layout(&app), "0=w0 3=w3");
}

#[test]
fn kill_flag_replaces_the_window_in_use() {
    // tmux: new-window -k -t sa:0 -> 0=K, the rest untouched.
    let mut app = app_from(&[(0, "w0"), (1, "w1"), (5, "w5")]);
    let p = NewWindowPlacement { kill: true, ..place("sa:1") };
    let plan = app.plan_new_window(&p, Some("K"), true).unwrap();
    assert_eq!(plan, NewWindowPlan::Create { index: Some(1), kill_pos: Some(1) });
    apply(&mut app, plan, "K");
    assert_eq!(layout(&app), "0=w0 1=K 5=w5");
}

#[test]
fn after_and_before_shuffle_like_winlink_shuffle_up() {
    // tmux 3.4 sequence, quoted:
    //   0=w0 1=w1 2=w2 5=w5
    //   -a -t sa:1 -> 0=w0 1=w1 2=A 3=w2 5=w5
    //   -b -t sa:1 -> 0=w0 1=B 2=w1 3=A 4=w2 5=w5
    //   -a -t sa:9 -> ... 9=C        (no window 9: no shuffle, index 9)
    //   -b -t sa:5 -> ... 5=D 6=w5
    let mut app = app_from(&[(0, "w0"), (1, "w1"), (2, "w2"), (5, "w5")]);
    run(&mut app, NewWindowPlacement { after: true, ..place("sa:1") }, "A").unwrap();
    assert_eq!(layout(&app), "0=w0 1=w1 2=A 3=w2 5=w5");
    run(&mut app, NewWindowPlacement { before: true, ..place("sa:1") }, "B").unwrap();
    assert_eq!(layout(&app), "0=w0 1=B 2=w1 3=A 4=w2 5=w5");
    run(&mut app, NewWindowPlacement { after: true, ..place("sa:9") }, "C").unwrap();
    assert_eq!(layout(&app), "0=w0 1=B 2=w1 3=A 4=w2 5=w5 9=C");
    run(&mut app, NewWindowPlacement { before: true, ..place("sa:5") }, "D").unwrap();
    assert_eq!(layout(&app), "0=w0 1=B 2=w1 3=A 4=w2 5=D 6=w5 9=C");
}

#[test]
fn after_without_index_inserts_after_the_current_window() {
    let mut app = app_from(&[(0, "w0"), (1, "w1"), (2, "w2")]);
    app.active_idx = 0;
    run(&mut app, NewWindowPlacement { after: true, ..place("sa") }, "A").unwrap();
    assert_eq!(layout(&app), "0=w0 1=A 2=w1 3=w2");
}

#[test]
fn select_existing_needs_a_name_and_no_index() {
    // tmux: -S -n w5 -t sa selects w5; -S -n w5 -t sa:5 is "index 5 in use"
    // when 5 is taken; -S naming no window creates it.
    let mut app = app_from(&[(0, "w0"), (6, "w5")]);
    let p = NewWindowPlacement { select_existing: true, ..place("sa") };
    assert_eq!(app.plan_new_window(&p, Some("w5"), false).unwrap(), NewWindowPlan::SelectExisting(Some(1)));
    assert_eq!(app.plan_new_window(&p, Some("w5"), true).unwrap(), NewWindowPlan::SelectExisting(None));
    let p6 = NewWindowPlacement { select_existing: true, ..place("sa:6") };
    assert_eq!(app.plan_new_window(&p6, Some("w5"), true).unwrap_err(), "create window failed: index 6 in use");
    run(&mut app, NewWindowPlacement { select_existing: true, ..place("sa") }, "fresh").unwrap();
    assert_eq!(layout(&app), "0=w0 6=w5 7=fresh");
    app.windows[0].name = "w5".to_string();
    assert_eq!(app.plan_new_window(&p, Some("w5"), true).unwrap_err(), "multiple windows named w5");
}

#[test]
fn unknown_window_name_is_cant_find() {
    let mut app = app_from(&[(0, "w0")]);
    assert_eq!(app.plan_new_window(&place("sa:abc"), None, true).unwrap_err(), "can't find window: abc");
}

#[test]
fn placement_flags_parse_from_every_spelling() {
    let p = NewWindowPlacement::from_args(&["-d", "-a", "-n", "x", "-t", "s:3"], None);
    assert_eq!(p, NewWindowPlacement { target: Some("s:3".into()), after: true, ..Default::default() });
    let p = NewWindowPlacement::from_args(&["-dkS", "-n", "-a"], Some(":4"));
    assert_eq!(p, NewWindowPlacement { target: Some(":4".into()), kill: true, select_existing: true, ..Default::default() });
    // Flags of the pane's own command are not new-window's.
    let p = NewWindowPlacement::from_args(&["-b", "pwsh", "-a", "-k"], None);
    assert_eq!(p, NewWindowPlacement { before: true, ..Default::default() });
    let p = NewWindowPlacement::from_args(&["-d", "--", "prog", "-k"], None);
    assert_eq!(p, NewWindowPlacement::default());
}

#[test]
fn new_window_is_a_window_target_command() {
    assert_eq!(crate::cli::coerce_bare_window_target("new-window", "3"), ":3");
    assert_eq!(crate::cli::coerce_bare_window_target("neww", "3"), ":3");
    assert_eq!(crate::cli::coerce_bare_window_target("new-window", "sa"), "sa");
}
