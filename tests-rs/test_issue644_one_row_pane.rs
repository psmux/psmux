// Issue #644: "A pane exactly 1 row tall is never repainted by the client".
//
// `split-window -t r:0.0 -l 9999` leaves the parent pane one row tall, which is
// what tmux does too: `layout_split_sizes` clamps an oversized size to
// `ss - 2` and gives the remaining `ss - 1 - s2` rows to the pane being split
// (tmux layout.c:1321-1325), and `PANE_MINIMUM` is 1 (tmux tmux.h:110), so one
// row is a legal pane there and is painted like any other.
//
// psmux got the layout right and then contradicted it twice:
//
//   1. `resize_window_panes` rounded the pane's pseudoconsole and parser up to
//      `MIN_PANE_DIM` (2) while the slot stayed one row, so `list-panes`
//      reported `pane_height` 2 with `pane_top` and `pane_bottom` both 0.
//   2. The client saw a source taller than its rect, took that for an
//      oversized preview, and `downscale_rows_v2` handed it the BOTTOM row of
//      the two: for a program that writes to its first row that row is blank,
//      so the client painted nothing for ever while the pane kept running and
//      `capture-pane` kept showing live content.
//
// The first group below pins the sizing rule, the second renders a real one row
// leaf through the real client renderer and asserts the text lands on screen.

use ratatui::layout::Rect;

use crate::layout::{CellRunJson, LayoutJson, RowRunsJson};
use crate::pane::{MIN_PANE_DIM, MIN_PTY_COLS, MIN_PTY_DIM};
use crate::tree::split_with_gaps;
use crate::tree::pane_inner_size;

// ── the sizing rule ─────────────────────────────────────────────────────────

#[test]
fn a_one_row_slot_gets_a_one_row_pane() {
    // The exact #644 geometry: the 30 row window of `new-session -y 30` split
    // with `-l 9999`, which leaves `120x1` for the pane being split.
    let (h, w) = pane_inner_size(Rect::new(0, 0, 120, 1), 0);
    assert_eq!(h, 1, "a one row slot must hold a one row pane, not {h}");
    assert_eq!(w, 120);
}

#[test]
fn a_one_column_slot_is_widened_to_the_conpty_floor() {
    // The column axis is the one place the pane is NOT exactly its slot: a one
    // column pseudoconsole holding a wide glyph never echoes again after it is
    // grown back (test_issue534_shrink_wide_erase caught it), so the width
    // floor is two. The layout applies the same floor to the cell, see
    // `a_horizontal_cell_is_never_one_column`, so in practice the two agree.
    let (h, w) = pane_inner_size(Rect::new(0, 0, 1, 30), 0);
    assert_eq!(w, MIN_PTY_COLS, "a one column slot is widened to the floor, got {w}");
    assert_eq!(MIN_PTY_COLS, 2);
    assert_eq!(h, 30);
}

#[test]
fn a_horizontal_cell_is_never_one_column() {
    // Every route to a one column cell goes through split_with_gaps: the
    // sizes below are what `resize-pane -x 1` and `split-window -h -l 9999`
    // leave in the tree. With room for two columns per child, the narrow one
    // is widened at the expense of the largest sibling.
    for sizes in [vec![1u16, 79], vec![79, 1], vec![1, 1, 78], vec![0, 100]] {
        let rects = split_with_gaps(true, &sizes, Rect::new(0, 0, 80, 24));
        for (i, r) in rects.iter().enumerate() {
            assert!(r.width >= MIN_PTY_COLS, "sizes {sizes:?}: child {i} is {} columns wide", r.width);
        }
        let used: u16 = rects.iter().map(|r| r.width).sum::<u16>() + (rects.len() as u16 - 1);
        assert_eq!(used, 80, "sizes {sizes:?}: the widened cell must come out of a sibling, not thin air");
    }
    // A one ROW cell is still legal (tmux PANE_MINIMUM 1), so the vertical
    // axis keeps handing out exactly what the sizes ask for.
    let rects = split_with_gaps(false, &[1, 28], Rect::new(0, 0, 120, 30));
    assert_eq!(rects[0].height, 1, "a one row cell must survive on the vertical axis");
}

#[test]
fn pane_size_equals_slot_size_for_every_height() {
    // The invariant #644 broke: a pane is exactly as big as its cell. Anything
    // that rounds a slot up makes `pane_height` disagree with
    // `pane_bottom - pane_top + 1`, which is the symptom the reporter saw.
    for height in 1..=60u16 {
        let (h, _) = pane_inner_size(Rect::new(0, 0, 80, height), 0);
        assert_eq!(h, height, "slot of {height} rows must give {height} rows, got {h}");
    }
    // Columns: exact from the floor upwards. Below it the ConPTY floor wins,
    // and the layout never produces such a cell anyway.
    for width in MIN_PTY_COLS..=200u16 {
        let (_, w) = pane_inner_size(Rect::new(0, 0, width, 24), 0);
        assert_eq!(w, width, "slot of {width} cols must give {width} cols, got {w}");
    }
}

#[test]
fn the_border_label_row_comes_out_of_the_slot() {
    // pane-border-status top/bottom takes one row for the label (#288). A two
    // row slot then leaves one row of content, which must survive as one row.
    let (h, _) = pane_inner_size(Rect::new(0, 0, 80, 2), 1);
    assert_eq!(h, 1, "two row slot minus a label row is one row of content, got {h}");
}

#[test]
fn a_slot_can_never_produce_a_zero_sized_pseudoconsole() {
    // The one thing that is still clamped: a pseudoconsole of zero rows or
    // zero columns is not a terminal. Everything at or above one is honoured.
    let (h, w) = pane_inner_size(Rect::new(0, 0, 1, 1), 1);
    assert_eq!(h, MIN_PTY_DIM);
    assert_eq!(w, MIN_PTY_COLS, "columns floor at two, see MIN_PTY_COLS");
    assert_eq!(MIN_PTY_DIM, 1, "tmux's PANE_MINIMUM is 1 (tmux.h:110)");
    assert!(MIN_PANE_DIM > MIN_PTY_DIM, "the new pane target stays a policy number above the floor");
}

// ── the render ──────────────────────────────────────────────────────────────

fn run(text: &str) -> CellRunJson {
    CellRunJson {
        text: text.to_string(),
        fg: "default".to_string(),
        bg: "default".to_string(),
        flags: 0,
        width: text.chars().count() as u16,
        link: None,
        ul: 0,
        ulc: None, ol: false,
    }
}

/// A leaf `rows` x `cols` whose first row holds `text` and whose remaining rows
/// are blank: the shape of the reporter's ticker, which rewrites row one only.
fn ticker_leaf(rows: u16, cols: u16, text: &str) -> LayoutJson {
    let mut rows_v2: Vec<RowRunsJson> = Vec::new();
    let pad = " ".repeat(cols.saturating_sub(text.chars().count() as u16) as usize);
    rows_v2.push(RowRunsJson { runs: vec![run(text), run(&pad)] });
    for _ in 1..rows {
        rows_v2.push(RowRunsJson { runs: vec![run(&" ".repeat(cols as usize))] });
    }
    LayoutJson::Leaf {
        id: 1, rows, cols,
        cursor_row: 0, cursor_col: 0,
        alternate_screen: false, wants_mouse: false, hide_cursor: true, cursor_shape: 0, cursor_colour: None, default_colours: None,
        active: true, copy_mode: false, scroll_offset: 0, view_offset: 0,
        sel_start_row: None, sel_start_col: None, sel_end_row: None, sel_end_col: None,
        sel_mode: None, copy_cursor_row: None, copy_cursor_col: None,
        content: Vec::new(), rows_v2, title: None, copy_hl: Vec::new(), images: vec![],
    }
}

/// Render a leaf into an `w` x `h` area through the real client renderer and
/// read every row back as text. This is the client's own paint path, the one
/// that was handing the ConPTY a blank row.
fn painted_rows(leaf: &LayoutJson, w: u16, h: u16) -> Vec<String> {
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Style};
    use ratatui::Terminal;

    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| {
        let area = Rect::new(0, 0, w, h);
        let active_rect = crate::client::compute_active_rect_json(leaf, area);
        crate::client::render_layout_json(
            f, leaf, area, false,
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::Green),
            false, Color::Reset, active_rect, "", false, "off", "", 1,
            crate::border_lines::border_chars("single"), None,
            crate::client::WindowContentStyles::default(),
            crate::pane_border::PaneBorderIndicators::Colour,
        );
    }).unwrap();
    let buf = term.backend().buffer().clone();
    let aw = buf.area.width as usize;
    (0..h as usize)
        .map(|r| {
            (0..aw)
                .map(|c| buf.content[r * aw + c].symbol().chars().next().unwrap_or(' '))
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

#[test]
fn a_one_row_pane_paints_its_only_row() {
    // The #644 pane: one row of slot, one row of screen, content on it.
    let leaf = ticker_leaf(1, 120, "AAAAAAAA");
    let rows = painted_rows(&leaf, 120, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], "AAAAAAAA", "the single row must carry the pane's text, got {:?}", rows[0]);
}

#[test]
fn a_one_row_pane_repaints_when_its_row_changes() {
    // The reporter measured updates, not one frame: alternate the word the way
    // the ticker does and require every frame to differ.
    let mut seen: Vec<String> = Vec::new();
    for i in 0..6 {
        let word = if i % 2 == 0 { "AAAAAAAA" } else { "BBBBBBBB" };
        let leaf = ticker_leaf(1, 120, word);
        seen.push(painted_rows(&leaf, 120, 1).remove(0));
    }
    let transitions = seen.windows(2).filter(|w| w[0] != w[1]).count();
    assert_eq!(transitions, 5, "every ticker frame must reach the screen, got {seen:?}");
}

#[test]
fn a_two_row_pane_still_paints_both_rows() {
    // The control the reporter used: at two rows this always worked, and must
    // keep working.
    let leaf = ticker_leaf(2, 120, "AAAAAAAA");
    let rows = painted_rows(&leaf, 120, 2);
    assert_eq!(rows[0], "AAAAAAAA");
    assert_eq!(rows[1], "", "the pane's blank second row stays blank");
}

#[test]
fn a_one_column_pane_paints_its_only_column() {
    let leaf = ticker_leaf(3, 1, "X");
    let rows = painted_rows(&leaf, 1, 3);
    assert_eq!(rows[0], "X", "the single column must carry the pane's text, got {:?}", rows[0]);
}

#[test]
fn a_screen_taller_than_its_slot_is_what_used_to_lose_the_row() {
    // The failing combination, pinned so it cannot come back by another route:
    // a two row screen in a one row slot goes down the preview path, and that
    // path keeps the BOTTOM row, which for a top anchored program is blank.
    // This is why the sizing rule above matters: the renderer is behaving
    // correctly for a preview, it was simply never meant to see a live pane.
    let leaf = ticker_leaf(2, 120, "AAAAAAAA");
    let rows = painted_rows(&leaf, 120, 1);
    assert_eq!(rows[0], "", "documents the old symptom: the blank row wins");

    // And the fix, in one line: size the pane to its slot and the text is there.
    let sized = ticker_leaf(pane_inner_size(Rect::new(0, 0, 120, 1), 0).0, 120, "AAAAAAAA");
    assert_eq!(painted_rows(&sized, 120, 1)[0], "AAAAAAAA");
}
