// Issue #767: floating panes (tmux new-pane) had four defects around them.
//
//  1. No `prefix *` binding. tmux key-bindings.c binds
//     `bind -N 'New floating pane' * { new-pane }`.
//  2. A floating pane was missing from list-panes (plain, -F, -a, -s) and from
//     #{P:}, and #{pane_floating_flag} expanded to nothing. tmux puts a
//     SPAWN_FLOATING pane at the TAIL of w->panes (window.c window_add_pane),
//     so it is listed after the tiled panes, counted by #{window_panes}
//     (window_count_panes(w, 1)), and is w->active while it has focus.
//  3. The client put the terminal cursor on the tiled pane's cursor while the
//     keys went into the focused float: the float's cursor was never sent.
//  4. The float was drawn AFTER the client's own overlays, so a centred float
//     painted over the `kill-pane? (y/n)` box.
//
// These tests drive the real code: the default key table, the real format
// expander over a window holding a real (process less) floating pane, the
// server's float serializer, and the client's cursor placement and render
// order through a headless ratatui backend.

use super::*;
use crate::types::{FloatingPane, Window};

/// A pane with no child process, the same thing `new-pane -E` builds. None
/// when a pseudo console cannot be opened, so the test is skipped instead.
fn empty_pane(app: &mut AppState, rows: u16, cols: u16) -> Option<Pane> {
    let id = app.next_pane_id;
    let pane = crate::popup::create_empty_pane(rows, cols, id)?;
    app.next_pane_id += 1;
    Some(pane)
}

fn window_with(root: Node, id: usize) -> Window {
    let pid = match &root { Node::Leaf(p) => p.id, _ => 0 };
    Window {
        root,
        active_path: vec![],
        name: "w".into(),
        id,
        area: ratatui::layout::Rect { x: 0, y: 0, width: 100, height: 30 },
        window_size: None,
        window_options: Default::default(),
        activity_flag: false,
        bell_flag: false,
        silence_flag: false,
        last_output_time: std::time::Instant::now(),
        last_seen_version: 0,
        manual_rename: false,
        layout_index: 0,
        pane_mru: vec![pid],
        zoom_saved: None,
        linked_from: None,
        floating: Vec::new(),
        floating_focus: None,
    }
}

/// One window: a tiled pane, plus one floating pane at (10,5) 40x12 with the
/// focus. Returns the app and (tiled id, float id).
fn rig_with_float(name: &str, border: &str) -> Option<(AppState, usize, usize)> {
    let mut app = AppState::new(name.to_string());
    app.window_base_index = 0;
    app.pane_base_index = 0;
    app.windows.clear();
    app.active_idx = 0;
    let tiled = empty_pane(&mut app, 30, 100)?;
    let tiled_id = tiled.id;
    app.windows.push(window_with(Node::Leaf(tiled), 1));
    let fpane = empty_pane(&mut app, 10, 38)?;
    let fid = fpane.id;
    let win = &mut app.windows[0];
    win.floating.push(FloatingPane {
        pane: fpane,
        x: 10, y: 5, w: 40, h: 12,
        border: border.to_string(),
        id: fid,
        title: String::new(),
        position: None,
    });
    win.floating_focus = Some(0);
    Some((app, tiled_id, fid))
}

// ── 1. prefix * ────────────────────────────────────────────────────────────

#[test]
fn prefix_star_is_a_default_binding_for_new_pane() {
    assert!(
        crate::help::PREFIX_DEFAULTS.iter().any(|(k, c)| *k == "*" && *c == "new-pane"),
        "tmux binds prefix * to new-pane; PREFIX_DEFAULTS has no such entry"
    );
}

#[test]
fn prefix_star_lands_in_the_prefix_key_table() {
    let mut app = AppState::new("i767_keys".to_string());
    app.key_tables.clear();
    crate::config::populate_default_bindings(&mut app);
    let star = crate::config::normalize_key_for_binding(
        crate::config::parse_key_name("*").expect("* parses as a key"),
    );
    let table = app.key_tables.get("prefix").expect("prefix table seeded");
    let bind = table.iter().find(|b| b.key == star).expect("prefix * is bound");
    match &bind.action {
        crate::types::Action::Command(c) => assert_eq!(c, "new-pane"),
        _ => panic!("prefix * must run the new-pane command"),
    }
}

// ── 2. list-panes / formats ─────────────────────────────────────────────────

#[test]
fn list_panes_format_lists_the_float_after_the_tiled_pane() {
    let Some((app, tiled_id, fid)) = rig_with_float("i767_lsp", "single") else { return };
    let out = format_list_panes(
        &app,
        "#{pane_id} #{pane_index} floating=#{pane_floating_flag} active=#{pane_active}",
        0,
    );
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec![
            format!("%{} 0 floating=0 active=0", tiled_id).as_str(),
            format!("%{} 1 floating=1 active=1", fid).as_str(),
        ],
        "list-panes -F must list the float, flagged and active, after the tiled pane"
    );
}

#[test]
fn pane_floating_flag_is_zero_for_a_window_with_no_float() {
    let Some((mut app, _tiled, _fid)) = rig_with_float("i767_noflt", "single") else { return };
    app.windows[0].floating.clear();
    app.windows[0].floating_focus = None;
    assert_eq!(expand_format("#{pane_floating_flag}", &app), "0",
        "tmux reports 0 for a tiled pane, never an empty string");
    assert_eq!(expand_format("#{pane_active}", &app), "1");
    assert_eq!(expand_format("#{window_panes}", &app), "1");
}

#[test]
fn the_focused_float_is_the_active_pane_for_display_message() {
    let Some((app, _tiled, fid)) = rig_with_float("i767_dm", "single") else { return };
    assert_eq!(expand_format("#{pane_id}", &app), format!("%{}", fid));
    assert_eq!(expand_format("#{pane_floating_flag}", &app), "1");
    assert_eq!(expand_format("#{pane_index}", &app), "1");
    assert_eq!(expand_format("#{window_panes}", &app), "2",
        "tmux window_panes counts floating panes (window_count_panes(w, 1))");
}

#[test]
fn an_unfocused_float_leaves_the_tiled_pane_active() {
    let Some((mut app, tiled_id, fid)) = rig_with_float("i767_unf", "single") else { return };
    app.windows[0].floating_focus = None;
    assert_eq!(expand_format("#{pane_id}", &app), format!("%{}", tiled_id));
    let out = format_list_panes(&app, "#{pane_id}=#{pane_active}", 0);
    assert_eq!(out, format!("%{}=1\n%{}=0", tiled_id, fid));
}

#[test]
fn float_geometry_formats_use_the_float_rect() {
    let Some((app, _tiled, fid)) = rig_with_float("i767_geo", "single") else { return };
    // Content starts one cell inside the border: (11,6), size 38x10.
    let got = expand_format_for_pane_by_id(
        "#{pane_left},#{pane_top},#{pane_right},#{pane_bottom} #{pane_x},#{pane_y},#{pane_z} #{pane_width}x#{pane_height}",
        &app,
        fid,
    );
    assert_eq!(got, "11,6,48,15 11,6,0 38x10");
}

#[test]
fn a_borderless_float_starts_at_its_corner() {
    let Some((app, _tiled, fid)) = rig_with_float("i767_geo_none", "none") else { return };
    let got = expand_format_for_pane_by_id("#{pane_x},#{pane_y}", &app, fid);
    assert_eq!(got, "10,5");
}

#[test]
fn pane_loop_includes_the_float() {
    let Some((app, tiled_id, fid)) = rig_with_float("i767_ploop", "single") else { return };
    assert_eq!(
        expand_format("#{P:#{pane_id}}", &app),
        format!("%{} %{}", tiled_id, fid),
        "tmux format_loop_panes walks w->panes, floats included"
    );
}

#[test]
fn by_id_lookup_finds_a_float() {
    let Some((app, _tiled, fid)) = rig_with_float("i767_byid", "single") else { return };
    assert_eq!(
        expand_format_for_pane_by_id("#{pane_id} #{pane_floating_flag}", &app, fid),
        format!("%{} 1", fid)
    );
}

// ── 3. cursor in the focused float ──────────────────────────────────────────

#[test]
fn float_serializer_ships_the_float_cursor() {
    let Some((app, _tiled, _fid)) = rig_with_float("i767_ser", "single") else { return };
    if let Ok(mut parser) = app.windows[0].floating[0].pane.term.lock() {
        // Move the float's cursor to row 3, col 7 (1-based CUP 4;8).
        parser.process(b"\x1b[4;8H");
    }
    let frag = crate::popup::serialize_floats_json(&app);
    let json = format!("{{\"x\":0{}}}", frag);
    let v: serde_json::Value = serde_json::from_str(&json).expect("floats fragment is valid JSON");
    let fl = &v["floats"][0];
    assert_eq!(fl["cursor_row"], 3, "float cursor row must be serialized: {}", fl);
    assert_eq!(fl["cursor_col"], 7, "float cursor col must be serialized: {}", fl);
    assert_eq!(fl["hide_cursor"], false);
}

fn float_json(x: u16, y: u16, w: u16, h: u16, border: &str, focused: bool, cr: u16, cc: u16) -> crate::client::FloatJson {
    crate::client::FloatJson {
        x, y, w, h,
        border: border.into(),
        focused,
        cursor_row: cr,
        cursor_col: cc,
        ..Default::default()
    }
}

#[test]
fn the_cursor_goes_inside_the_focused_float() {
    let chunk = ratatui::layout::Rect { x: 0, y: 0, width: 100, height: 29 };
    let floats = vec![float_json(15, 4, 70, 22, "single", true, 2, 9)];
    assert_eq!(
        crate::client::focused_float_cursor(chunk, &floats),
        Some(Some((15 + 1 + 9, 4 + 1 + 2))),
        "cursor = float origin + border + the float's own cursor"
    );
}

#[test]
fn no_focused_float_leaves_the_cursor_to_the_tiled_pane() {
    let chunk = ratatui::layout::Rect { x: 0, y: 0, width: 100, height: 29 };
    let floats = vec![float_json(15, 4, 70, 22, "single", false, 2, 9)];
    assert_eq!(crate::client::focused_float_cursor(chunk, &floats), None);
    assert_eq!(crate::client::focused_float_cursor(chunk, &[]), None);
}

#[test]
fn a_hidden_float_cursor_hides_rather_than_falling_back() {
    let chunk = ratatui::layout::Rect { x: 0, y: 0, width: 100, height: 29 };
    let mut fl = float_json(15, 4, 70, 22, "single", true, 2, 9);
    fl.hide_cursor = true;
    assert_eq!(crate::client::focused_float_cursor(chunk, &[fl]), Some(None),
        "a focused float that hides its cursor must not hand it to the pane behind");
}

#[test]
fn the_float_cursor_respects_a_status_line_on_top_and_borderless_floats() {
    // status-position top: the content chunk starts at row 1.
    let chunk = ratatui::layout::Rect { x: 0, y: 1, width: 100, height: 29 };
    let floats = vec![float_json(10, 5, 40, 12, "none", true, 0, 0)];
    assert_eq!(crate::client::focused_float_cursor(chunk, &floats), Some(Some((10, 6))));
    // The cursor is clamped into the float's content area.
    let floats = vec![float_json(10, 5, 40, 12, "single", true, 500, 500)];
    assert_eq!(crate::client::focused_float_cursor(chunk, &floats), Some(Some((10 + 1 + 37, 1 + 5 + 1 + 9))));
}

// ── 4. prompts draw above floats ────────────────────────────────────────────

/// The client draw order, reduced to the two calls that matter: the float
/// overlay, then the client's centred confirm box. This is the order the
/// client now uses; the defect was the reverse.
#[test]
fn a_confirm_box_drawn_after_the_float_stays_visible() {
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};
    use ratatui::Terminal;
    let (cw, ch) = (100u16, 29u16);
    let chunk = Rect::new(0, 0, cw, ch);
    let fl = float_json(15, 4, 70, 22, "single", true, 0, 0);
    let backend = TestBackend::new(cw, ch);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        crate::client::render_float_overlays(
            f, chunk, std::slice::from_ref(&fl),
            crate::client::WindowContentStyles::default(),
            ratatui::style::Style::default(),
            ratatui::style::Style::default(),
            crate::pane_border::PaneBorderIndicators::Colour,
        );
        let overlay = Block::default().borders(Borders::ALL).title("confirm");
        let oa = crate::rendering::centered_rect(50, 3, chunk);
        f.render_widget(Clear, oa);
        f.render_widget(&overlay, oa);
        f.render_widget(Paragraph::new("kill-pane? (y/n)"), overlay.inner(oa));
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let screen: String = (0..ch).map(|y| {
        (0..cw).map(|x| buf.content[(y * cw + x) as usize].symbol().to_string()).collect::<String>()
    }).collect::<Vec<_>>().join("\n");
    assert!(screen.contains("kill-pane? (y/n)"), "confirm box must be on top of the float:\n{}", screen);
    // And the box really is inside the float's area, i.e. the float would
    // have covered it had the order been reversed.
    let oa = crate::rendering::centered_rect(50, 3, chunk);
    assert!(oa.x > 15 && oa.x + oa.width < 15 + 70 && oa.y > 4 && oa.y + oa.height < 4 + 22);
}

/// Source guard for the order itself: in the client's draw closure the float
/// overlay must come before every client side chooser and prompt.
#[test]
fn client_draws_floats_before_its_own_overlays() {
    let src = include_str!("../src/client.rs");
    // Only the draw closure counts: the same flags are tested in the key
    // handling above it.
    let draw_at = src.find("terminal.draw(|f| {").expect("client draw closure");
    let src = &src[draw_at..];
    let floats_at = src.find("if !srv_floats.is_empty() {")
        .expect("float overlay call in the draw closure");
    for marker in [
        "            if session_chooser {",
        "            if tree_chooser {",
        "            if buffer_chooser {",
        "            if keys_viewer {",
        "            if renaming {",
        "            if command_input {",
        // The confirm box takes `chooser_kill` as well as `confirm_cmd` now
        // (#778), so the marker is the start of that line rather than all of
        // it.
        "            if let Some(cmd) = confirm_cmd",
    ] {
        let at = src.find(marker).unwrap_or_else(|| panic!("marker {:?} not found", marker));
        assert!(floats_at < at, "floats must draw before {:?} so it is not painted over", marker);
    }
}
