// Issue #980: a pane's pane-colours default palette under its OSC 4 entries.
//
// tmux keeps both on the pane's `struct colour_palette`: `palette` from OSC 4
// and `default_palette` from the option (colour.c:1293-1322), and
// colour_palette_get (colour.c:1251-1268) reads the OSC 4 entry first, then
// the default. RIS (input.c:1407) and OSC 104 (input.c:3446-3475) clear only
// the OSC 4 entries, so the default shows through again.

use vt100_psmux as vt100;
use vt100::{Color, ColourPalette, Parser};

fn defaults() -> ColourPalette {
    let mut p = ColourPalette::default();
    p.set_colour(1, Some(Color::Rgb(0, 255, 0)));
    p.set_colour(2, Some(Color::Idx(4)));
    p
}

fn resolve(parser: &Parser, c: Color) -> Color {
    match parser.screen().colour_palette() {
        Some(p) => p.resolve(c),
        None => c,
    }
}

#[test]
fn a_screen_without_pane_colours_is_as_before() {
    let mut parser = Parser::new(24, 80, 0);
    assert!(parser.screen().colour_palette().is_none());
    assert!(!parser.screen_mut().set_pane_colours(None));
    // An empty table is no table.
    assert!(!parser.screen_mut().set_pane_colours(Some(ColourPalette::default())));
    assert!(parser.screen().colour_palette().is_none());
    assert_eq!(parser.screen().palette_entry(1), None);
}

#[test]
fn indexed_colours_draw_through_the_defaults() {
    let mut parser = Parser::new(24, 80, 0);
    let gen = parser.screen().palette_generation();
    assert!(parser.screen_mut().set_pane_colours(Some(defaults())));
    assert_ne!(parser.screen().palette_generation(), gen);
    // Installing the same table again changes nothing.
    assert!(!parser.screen_mut().set_pane_colours(Some(defaults())));
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0, 255, 0));
    // An entry that is an index draws as that index (tty.c:2840, one pass).
    assert_eq!(resolve(&parser, Color::Idx(2)), Color::Idx(4));
    assert_eq!(resolve(&parser, Color::Idx(3)), Color::Idx(3));
    assert_eq!(resolve(&parser, Color::Default), Color::Default);
    assert_eq!(resolve(&parser, Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
    assert_eq!(parser.screen().resolve_colour(Color::Idx(1)), Color::Rgb(0, 255, 0));
    // The OSC 4 query answer: RGB, an index through the xterm table.
    assert_eq!(parser.screen().palette_entry(1), Some((0, 255, 0)));
    assert_eq!(parser.screen().palette_entry(2), Some((0, 0, 0x80)));
    assert_eq!(parser.screen().palette_entry(3), None);
}

#[test]
fn osc4_wins_and_osc104_hands_back_the_default() {
    let mut parser = Parser::new(24, 80, 0);
    parser.screen_mut().set_pane_colours(Some(defaults()));
    parser.process(b"\x1b]4;1;rgb:11/22/33\x1b\\");
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(parser.screen().palette_entry(1), Some((0x11, 0x22, 0x33)));
    parser.process(b"\x1b]104;1\x1b\\");
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0, 255, 0));
    assert_eq!(parser.screen().palette_entry(1), Some((0, 255, 0)));
    parser.process(b"\x1b]4;2;#123456\x1b\\");
    assert_eq!(resolve(&parser, Color::Idx(2)), Color::Rgb(0x12, 0x34, 0x56));
    parser.process(b"\x1b]104\x1b\\");
    assert_eq!(resolve(&parser, Color::Idx(2)), Color::Idx(4));
}

#[test]
fn osc4_set_before_the_defaults_still_wins() {
    let mut parser = Parser::new(24, 80, 0);
    parser.process(b"\x1b]4;1;rgb:11/22/33\x1b\\");
    parser.screen_mut().set_pane_colours(Some(defaults()));
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(resolve(&parser, Color::Idx(2)), Color::Idx(4));
    // Dropping the defaults leaves the OSC 4 palette exactly as it was.
    parser.screen_mut().set_pane_colours(None);
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0x11, 0x22, 0x33));
    assert_eq!(resolve(&parser, Color::Idx(2)), Color::Idx(2));
}

#[test]
fn ris_drops_only_the_osc4_entries() {
    let mut parser = Parser::new(24, 80, 0);
    parser.screen_mut().set_pane_colours(Some(defaults()));
    parser.process(b"\x1b]4;1;rgb:11/22/33\x1b\\");
    parser.process(b"\x1bc");
    assert_eq!(resolve(&parser, Color::Idx(1)), Color::Rgb(0, 255, 0));
    assert_eq!(parser.screen().pane_colours(), Some(&defaults()));
}

#[test]
fn a_drawn_cell_keeps_its_index_and_resolves_on_the_way_out() {
    let mut parser = Parser::new(24, 80, 0);
    parser.process(b"\x1b[31mR\x1b[91mB\x1b[38;5;1mI\x1b[0m");
    parser.screen_mut().set_pane_colours(Some(defaults()));
    let cell = parser.screen().cell(0, 0).unwrap();
    assert_eq!(cell.fgcolor(), Color::Idx(1));
    assert_eq!(resolve(&parser, cell.fgcolor()), Color::Rgb(0, 255, 0));
    // SGR 91 is index 9, which this table leaves alone.
    let cell = parser.screen().cell(0, 1).unwrap();
    assert_eq!(resolve(&parser, cell.fgcolor()), Color::Idx(9));
    let cell = parser.screen().cell(0, 2).unwrap();
    assert_eq!(resolve(&parser, cell.fgcolor()), Color::Rgb(0, 255, 0));
}
