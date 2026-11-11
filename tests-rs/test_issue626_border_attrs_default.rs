// Issue #626: two pane border style gaps, filed while reviewing PR #624.
//
// Gap 1. `pane-border-style 'fg=red,bold'` rendered red but never bold.
// `complete_pane_border_colors` rebuilt the border style from
// `Style::default()`, carrying only `fg` and `bg` across, so `add_modifier`,
// `sub_modifier` and the `us=` underline colour were dropped on the floor.
// tmux does not replace the attribute half of a border style, it merges it,
// in style.c:459:
//
//     if (sy->gc.fg != 8)
//             gc->fg = sy->gc.fg;
//     if (sy->gc.bg != 8)
//             gc->bg = sy->gc.bg;
//     if (sy->gc.us != 8)
//             gc->us = sy->gc.us;
//     gc->attr |= sy->gc.attr;
//
// so the colours are absolute (which is what #624 needed, an omitted `fg=` in
// an explicit style has to reset rather than inherit the pane underneath) but
// the attributes OR in.
//
// Gap 2. `set -gu pane-border-style` landed on the terminal default foreground
// instead of the grey a fresh session draws. `reset_option_to_default` restores
// `OPTION_CATALOG`'s `default` field, and that field held the literal string
// "default". `parse_tmux_style("default")` yields an EMPTY style, which the
// renderer then pins to `Color::Reset`, while a fresh server leaves the option
// unset and the client supplies its built in `DarkGray`. tmux avoids the whole
// class by giving the option a real default style, options-table.c:1605
// `.default_str = "fg=themelightgrey"`, so unset and startup cannot disagree.
// `pane-active-border-style` already agreed (`fg=green` in the catalog and in
// `AppState::new`) and is pinned here so it stays that way.
//
// The live CLI, TCP and attached-client routes are covered by
// tests/test_issue626_border_attrs_default.ps1.

#[allow(unused_imports)]
use super::*;

use crate::layout::LayoutJson;
use crate::server::option_catalog::default_for;
use crate::server::options::{apply_set_option, get_option_value, reset_option_to_default};
use crate::style::parse_tmux_style;
use crate::types::AppState;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Terminal;

fn fresh_app() -> AppState {
    AppState::new("i626_probe".to_string())
}

/// The style the client resolves for a border from a raw option value, i.e.
/// exactly what `run_client` feeds `render_layout_json`.
fn resolve(raw: Option<&str>, active: bool) -> Style {
    parse_pane_border_colors(raw, pane_border_default_style(active))
}

// ---------------------------------------------------------------------------
// Gap 1: attributes survive the colour completion.
// ---------------------------------------------------------------------------

#[test]
fn bold_survives_on_the_inactive_border() {
    let style = resolve(Some("fg=red,bold"), false);
    assert_eq!(style.fg, Some(Color::Red));
    assert!(
        style.add_modifier.contains(Modifier::BOLD),
        "tmux ORs border attributes in (style.c:459), bold must reach the cell",
    );
}

#[test]
fn every_attribute_in_a_border_style_reaches_the_cell() {
    let style = resolve(
        Some("fg=yellow,bg=blue,bold,underscore,italics,reverse,strikethrough,dim,blink"),
        true,
    );
    assert_eq!(style.fg, Some(Color::Yellow));
    assert_eq!(style.bg, Some(Color::Blue));
    for (label, modifier) in [
        ("bold", Modifier::BOLD),
        ("underscore", Modifier::UNDERLINED),
        ("italics", Modifier::ITALIC),
        ("reverse", Modifier::REVERSED),
        ("strikethrough", Modifier::CROSSED_OUT),
        ("dim", Modifier::DIM),
        ("blink", Modifier::SLOW_BLINK),
    ] {
        assert!(
            style.add_modifier.contains(modifier),
            "{label} was dropped by the border colour completion",
        );
    }
}

#[test]
fn underline_colour_survives_on_the_border() {
    // tmux copies the underscore colour across in the same block that ORs the
    // attributes in: `if (sy->gc.us != 8) gc->us = sy->gc.us;`.
    let style = resolve(Some("fg=white,underscore,us=red"), false);
    assert_eq!(style.fg, Some(Color::White));
    assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(
        style.underline_color,
        Some(Color::Red),
        "us= must reach the border cell (tmux style.c:459)",
    );
}

#[test]
fn a_negating_attribute_still_reaches_the_border() {
    let style = resolve(Some("fg=red,nobold"), false);
    assert_eq!(style.fg, Some(Color::Red));
    assert!(
        style.sub_modifier.contains(Modifier::BOLD),
        "nobold must not be swallowed by the colour completion",
    );
}

#[test]
fn colours_stay_absolute_even_though_attributes_merge() {
    // The #624 contract: an explicit style that names no fg must RESET the
    // foreground rather than inherit the pane behind the separator. Merging the
    // attributes must not weaken that.
    let style = resolve(Some("bg=red,bold"), true);
    assert_eq!(style.fg, Some(Color::Reset));
    assert_eq!(style.bg, Some(Color::Red));
    assert!(style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn bold_border_style_paints_bold_border_cells() {
    // End to end through the real renderer: the separator cell itself has to
    // carry the modifier, not just the resolved Style struct.
    let layout = LayoutJson::Split {
        kind: "Horizontal".to_string(),
        sizes: vec![50, 50],
        children: vec![border_leaf(0, true), border_leaf(1, false)],
    };
    let inactive = resolve(Some("fg=red,bold"), false);
    let active = resolve(None, true);
    let backend = TestBackend::new(10, 6);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let area = Rect::new(0, 0, 10, 6);
            let active_rect = compute_active_rect_json(&layout, area);
            render_layout_json(
                frame,
                &layout,
                area,
                false,
                inactive,
                active,
                false,
                Color::Reset,
                active_rect,
                "",
                false,
                "off",
                "",
                2,
                crate::border_lines::border_chars("single"),
                None,
                WindowContentStyles::default(),
                crate::pane_border::PaneBorderIndicators::default(),
            );
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let separator_x = (0..10)
        .find(|&x| buffer[(x, 0)].symbol() == "│")
        .expect("vertical separator");
    let bold_cells = (0..6)
        .filter(|&y| buffer[(separator_x, y)].style().add_modifier.contains(Modifier::BOLD))
        .count();
    assert!(
        bold_cells > 0,
        "no border cell carried BOLD from pane-border-style 'fg=red,bold'",
    );
    let red_cells = (0..6)
        .filter(|&y| buffer[(separator_x, y)].style().fg == Some(Color::Red))
        .count();
    assert_eq!(
        bold_cells, red_cells,
        "every cell that took the red foreground must also take the bold",
    );
}

fn border_leaf(id: usize, active: bool) -> LayoutJson {
    LayoutJson::Leaf {
        id,
        rows: 6,
        cols: 4,
        cursor_row: 0,
        cursor_col: 0,
        alternate_screen: false,
        wants_mouse: false,
        hide_cursor: true,
        cursor_shape: 0, cursor_colour: None,
        active,
        copy_mode: false,
        scroll_offset: 0,
        view_offset: 0,
        sel_start_row: None,
        sel_start_col: None,
        sel_end_row: None,
        sel_end_col: None,
        sel_mode: None,
        copy_cursor_row: None,
        copy_cursor_col: None,
        content: Vec::new(),
        rows_v2: Vec::new(),
        title: None, copy_hl: Vec::new(), images: vec![],
    }
}

// ---------------------------------------------------------------------------
// Gap 2: unset restores exactly what startup renders.
// ---------------------------------------------------------------------------

#[test]
fn catalog_default_renders_as_the_built_in_border_style() {
    for (name, active) in [
        ("pane-border-style", false),
        ("pane-active-border-style", true),
    ] {
        let catalog = default_for(name).unwrap_or_else(|| panic!("{name} missing from the catalog"));
        assert!(
            !catalog.is_empty(),
            "{name} must advertise a real style, not an empty one",
        );
        assert_eq!(
            parse_tmux_style(catalog).fg,
            pane_border_default_style(active).fg,
            "the catalog default for {name} ({catalog:?}) must render like a fresh session",
        );
    }
}

#[test]
fn unset_restores_the_startup_border_render() {
    for (name, active) in [
        ("pane-border-style", false),
        ("pane-active-border-style", true),
    ] {
        let startup = fresh_app();
        let startup_render = resolve(
            Some(get_option_value(&startup, name).as_str()),
            active,
        );

        let mut app = fresh_app();
        apply_set_option(&mut app, name, "fg=colour244,bg=colour235", false);
        let overridden = resolve(Some(get_option_value(&app, name).as_str()), active);
        assert_ne!(
            overridden, startup_render,
            "precondition: setting {name} must actually change the render",
        );

        reset_option_to_default(&mut app, name);
        let after_unset = resolve(Some(get_option_value(&app, name).as_str()), active);
        assert_eq!(
            after_unset, startup_render,
            "set -u {name} must land back on what a fresh session renders",
        );
        assert_ne!(
            after_unset.fg,
            Some(Color::Reset),
            "set -u {name} must not leave the border on the terminal default fg",
        );
    }
}

#[test]
fn unset_and_startup_report_the_same_option_value() {
    for name in ["pane-border-style", "pane-active-border-style"] {
        let startup = get_option_value(&fresh_app(), name);
        let mut app = fresh_app();
        apply_set_option(&mut app, name, "fg=colour214", false);
        reset_option_to_default(&mut app, name);
        assert_eq!(
            get_option_value(&app, name),
            startup,
            "show-options must report the same {name} before a set and after the unset",
        );
    }
}

#[test]
fn the_word_default_still_means_the_terminal_default() {
    // tmux keeps these distinct: `set pane-border-style default` applies
    // style_default over grid_default_cell, i.e. the terminal's own colours,
    // while an UNSET option falls back to the options-table default style. The
    // #626 fix must not collapse the two.
    let explicit = resolve(Some("default"), false);
    assert_eq!(explicit.fg, Some(Color::Reset));
    assert_ne!(
        explicit.fg,
        pane_border_default_style(false).fg,
        "an explicit `default` is not the same thing as an unset option",
    );
}

#[test]
fn an_empty_option_value_still_falls_back_to_the_built_in() {
    // Older servers and `set pane-border-style ""` both reach the client with
    // an empty string, which has to keep taking the built in pair. The bg is
    // completed to Reset on every border style, unset included (#624).
    for active in [false, true] {
        let built_in = pane_border_default_style(active).fg;
        for raw in [Some(""), None] {
            let style = resolve(raw, active);
            assert_eq!(style.fg, built_in, "raw {raw:?}, active {active}");
            assert_eq!(style.bg, Some(Color::Reset), "raw {raw:?}, active {active}");
            assert!(style.add_modifier.is_empty(), "raw {raw:?}, active {active}");
        }
    }
}
