// pane-colours, a pane's default colour palette (tmux 3.8
// options-table.c:1629-1635, colour.c:1251-1322). The expected values are
// what tmux 3.8 took, refused, showed and drew for the same commands.

use super::*;
use crate::types::AppState;

fn app() -> AppState {
    AppState::new("test_pane_colours".to_string())
}

#[test]
fn an_element_names_a_colour_as_tmux_reads_it() {
    use vt100::Color::{Default, Idx, Rgb};
    assert_eq!(element_colour("#00ff00"), Some(Rgb(0, 255, 0)));
    assert_eq!(element_colour("#00FF00"), Some(Rgb(0, 255, 0)));
    assert_eq!(element_colour("blue"), Some(Idx(4)));
    assert_eq!(element_colour("colour200"), Some(Idx(200)));
    assert_eq!(element_colour("color7"), Some(Idx(7)));
    assert_eq!(element_colour("brightred"), Some(Idx(9)));
    assert_eq!(element_colour("91"), Some(Idx(9)));
    assert_eq!(element_colour("1"), Some(Idx(1)));
    assert_eq!(element_colour("default"), Some(Default));
    assert_eq!(element_colour("terminal"), Some(Default));
    // tmux 3.8: `bad colour: 9` and `bad colour: zzz`.
    assert_eq!(element_colour("9"), None);
    assert_eq!(element_colour("zzz"), None);
    assert_eq!(element_colour(""), None);
}

#[test]
fn a_value_that_is_no_colour_is_refused() {
    assert_eq!(validate("pane-colours[1]", "zzz"), Err("bad colour: zzz".to_string()));
    assert_eq!(validate("pane-colours[1]", "9"), Err("bad colour: 9".to_string()));
    assert_eq!(validate("pane-colours[1]", "#00ff00"), Ok(()));
    assert_eq!(validate("pane-colours", "red,blue green"), Ok(()));
    assert_eq!(validate("pane-colours", "red zzz"), Err("bad colour: zzz".to_string()));
    assert_eq!(validate("pane-colors[1]", "red"), Ok(()));
}

#[test]
fn the_other_name_is_the_same_array() {
    assert!(is_option("pane-colours"));
    assert!(is_option("pane-colours[3]"));
    assert!(is_option("pane-colors"));
    assert!(is_option("pane-colors[3]"));
    assert!(!is_option("pane-colorsx"));
    assert_eq!(crate::option_array::parse_name("pane-colors[3]").unwrap().0, OPTION);
}

#[test]
fn the_global_array_is_set_split_appended_and_unset() {
    let mut a = app();
    crate::server::options::set_array_option(&mut a, OPTION, "red,blue green", false).unwrap();
    crate::server::options::set_array_option(&mut a, OPTION, "yellow", true).unwrap();
    assert_eq!(a.pane_colours.values(), vec!["red", "blue", "green", "yellow"]);
    assert_eq!(crate::server::options::get_option_value(&a, OPTION), "red blue green yellow");
    assert!(crate::server::options::set_array_option(&mut a, "pane-colours[1]", "zzz", false).is_err());
    assert_eq!(a.pane_colours.get_index(1), Some("blue"));
    assert!(a.pane_colours_generation > 0);
    crate::server::options::unset_array_option(&mut a, OPTION).unwrap();
    assert!(a.pane_colours.is_empty());
}

#[test]
fn a_palette_holds_indexes_0_to_255_only() {
    let mut array = OptionArray::default();
    assert_eq!(default_palette(&array), None);
    let spec = crate::option_array::spec(OPTION).unwrap();
    array.apply_set(Some(crate::option_array::ArrayKey::Index(1)), "#00ff00", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(255)), "blue", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(300)), "red", false, spec);
    let palette = default_palette(&array).unwrap();
    assert_eq!(palette[1], Some(vt100::Color::Rgb(0, 255, 0)));
    assert_eq!(palette[255], Some(vt100::Color::Idx(4)));
    assert_eq!(palette.iter().filter(|e| e.is_some()).count(), 2);
}

#[test]
fn a_pane_uses_its_own_then_its_windows_then_the_global_array_whole() {
    let mut a = app();
    crate::server::options::set_array_option(&mut a, "pane-colours[1]", "#00ff00", false).unwrap();
    assert_eq!(effective(&a, 7, 3).get_index(1), Some("#00ff00"));
    set_window(&mut a, 7, "pane-colours[2]", "#0000ff", false, false, false, false);
    // The window's array is used whole: [1] from the global one is not merged in.
    assert_eq!(effective(&a, 7, 3).get_index(1), None);
    assert_eq!(effective(&a, 7, 3).get_index(2), Some("#0000ff"));
    assert_eq!(effective(&a, 8, 3).get_index(1), Some("#00ff00"));
    set_pane(&mut a, 3, "pane-colours[3]", "#ff0000", false, false, false, false);
    assert_eq!(effective(&a, 7, 3).values(), vec!["#ff0000"]);
    assert_eq!(effective(&a, 7, 4).values(), vec!["#0000ff"]);
}

#[test]
fn window_and_pane_tables_take_unset_append_and_only_if_unset() {
    let mut a = app();
    assert_eq!(set_pane(&mut a, 3, "pane-colours[1]", "zzz", false, false, false, false), "ERROR: bad colour: zzz");
    assert!(!a.pane_pane_colours.contains_key(&3));
    assert_eq!(set_pane(&mut a, 3, "pane-colours", "red blue", false, false, false, false), "");
    assert_eq!(set_pane(&mut a, 3, "pane-colours", "green", false, true, false, false), "");
    assert_eq!(a.pane_pane_colours[&3].values(), vec!["red", "blue", "green"]);
    assert_eq!(set_pane(&mut a, 3, "pane-colours", "x", false, false, true, false), "ERROR: already set: pane-colours");
    assert_eq!(set_pane(&mut a, 3, "pane-colours[0]", "", true, false, false, false), "");
    assert_eq!(a.pane_pane_colours[&3].values(), vec!["blue", "green"]);
    assert_eq!(set_pane(&mut a, 3, "pane-colours", "", true, false, false, false), "");
    assert!(!a.pane_pane_colours.contains_key(&3));
    assert_eq!(set_window(&mut a, 7, "pane-colors[5]", "colour100", false, false, false, false), "");
    assert_eq!(a.window_pane_colours[&7].get_index(5), Some("colour100"));
}

#[test]
fn show_prints_a_table_and_marks_what_it_inherits() {
    let mut a = app();
    crate::server::options::set_array_option(&mut a, "pane-colours[1]", "#00ff00", false).unwrap();
    set_window(&mut a, 7, "pane-colours[2]", "#0000ff", false, false, false, false);
    set_pane(&mut a, 3, "pane-colours[3]", "#ff0000", false, false, false, false);
    let pane = ShowScope::Pane { pane_id: 3, window_id: 7 };
    assert_eq!(show(&a, &ShowScope::Global, OPTION, false, false), "pane-colours[1] #00ff00\n");
    assert_eq!(show(&a, &ShowScope::Window(7), OPTION, false, false), "pane-colours[2] #0000ff\n");
    assert_eq!(show(&a, &pane, OPTION, false, false), "pane-colours[3] #ff0000\n");
    assert_eq!(show(&a, &pane, OPTION, false, true), "pane-colours[3] #ff0000\n");
    set_pane(&mut a, 3, OPTION, "", true, false, false, false);
    assert_eq!(show(&a, &pane, OPTION, false, false), "");
    assert_eq!(show(&a, &pane, OPTION, false, true), "pane-colours[2]* #0000ff\n");
    set_window(&mut a, 7, OPTION, "", true, false, false, false);
    assert_eq!(show(&a, &pane, OPTION, false, true), "pane-colours[1]* #00ff00\n");
    assert_eq!(show(&a, &pane, OPTION, true, true), "#00ff00\n");
    assert_eq!(show(&a, &ShowScope::Window(7), "pane-colours[1]", false, true), "pane-colours[1]* #00ff00\n");
    let empty = app();
    assert_eq!(show(&empty, &ShowScope::Global, OPTION, false, false), "pane-colours\n");
    assert_eq!(show(&empty, &ShowScope::Global, OPTION, true, false), "");
}

#[test]
fn the_osc_4_entry_wins_and_the_default_is_used_as_it_is() {
    let mut parser = vt100::Parser::new(5, 20, 0);
    let mut palette: Box<vt100::DefaultPalette> = Box::new([None; 256]);
    palette[1] = Some(vt100::Color::Rgb(0, 255, 0));
    palette[2] = Some(vt100::Color::Idx(4));
    palette[3] = Some(vt100::Color::Default);
    assert!(parser.screen_mut().set_default_palette(Some(palette)));
    let view = parser.screen().palette_view();
    assert_eq!(view.resolve(vt100::Color::Idx(1)), vt100::Color::Rgb(0, 255, 0));
    assert_eq!(view.resolve(vt100::Color::Idx(2)), vt100::Color::Idx(4));
    assert_eq!(view.resolve(vt100::Color::Idx(3)), vt100::Color::Default);
    assert_eq!(view.resolve(vt100::Color::Idx(5)), vt100::Color::Idx(5));
    assert_eq!(view.resolve(vt100::Color::Rgb(1, 2, 3)), vt100::Color::Rgb(1, 2, 3));
    parser.process(b"\x1b]4;1;rgb:11/22/33\x1b\\");
    assert_eq!(parser.screen().palette_view().resolve(vt100::Color::Idx(1)), vt100::Color::Rgb(0x11, 0x22, 0x33));
    // OSC 104 and RIS drop the OSC 4 entries only (input.c:1408, :3454).
    parser.process(b"\x1b]104;1\x1b\\");
    assert_eq!(parser.screen().palette_view().resolve(vt100::Color::Idx(1)), vt100::Color::Rgb(0, 255, 0));
    parser.process(b"\x1b]4;1;rgb:11/22/33\x1b\\\x1bc");
    assert_eq!(parser.screen().palette_view().resolve(vt100::Color::Idx(1)), vt100::Color::Rgb(0, 255, 0));
}

#[test]
fn the_query_replies_see_the_low_sixteen_as_rgb() {
    let mut palette: Box<vt100::DefaultPalette> = Box::new([None; 256]);
    palette[1] = Some(vt100::Color::Rgb(0, 255, 0));
    palette[2] = Some(vt100::Color::Idx(4));
    palette[3] = Some(vt100::Color::Default);
    palette[20] = Some(vt100::Color::Rgb(9, 9, 9));
    let low = low_sixteen(Some(&palette));
    assert_eq!(low[1], Some((0, 255, 0)));
    // An index is the xterm colour of that index (colour.c:189-200).
    assert_eq!(low[2], Some(crate::style::colour_256_to_rgb(4)));
    assert_eq!(low[3], None);
    assert_eq!(low.iter().filter(|e| e.is_some()).count(), 2);
    // A pane's own OSC 4 entry wins over the published default.
    let pane_id = 900_001;
    let owner = new_owner();
    publish(owner, pane_id, low);
    let own = Some({
        let mut own = [None; 16];
        own[1] = Some((1, 2, 3));
        own
    });
    let merged = merged_reply_palette(pane_id, own).unwrap();
    assert_eq!(merged[1], Some((1, 2, 3)));
    assert_eq!(merged[2], Some(crate::style::colour_256_to_rgb(4)));
    assert_eq!(reply_colour(pane_id, 1), Some((0, 255, 0)));
    publish(owner, pane_id, [None; 16]);
    assert_eq!(reply_colour(pane_id, 1), None);
}
