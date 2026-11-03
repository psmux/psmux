// Issue #980: pane-colours (other name pane-colors) as tmux 3.8 keeps it, a
// window and pane option holding an array of colours with no default
// (options-table.c:1614-1620). Before the fix the name was only in the list
// that keeps a tmux name from being an error, so any text was taken, nothing
// read it, and `set -w` / `set -p` had no table to write.
//
// The expected values are what the issue reports tmux 3.8 printing for the
// same commands, checked against colour.c:387-462 (colour_fromstring) and
// cmd-show-options.c:118-200.

use super::*;
use crate::server::options::{apply_set_option, apply_set_window_option, get_option_value, set_array_option, unset_array_option};
use crate::types::{AppState, LayoutKind, Node};

fn app() -> AppState {
    AppState::new("test_i980".to_string())
}

fn window(id: usize) -> crate::types::Window {
    crate::types::Window {
        root: Node::Split { kind: LayoutKind::Horizontal, sizes: vec![], children: vec![] },
        active_path: vec![],
        name: format!("w{}", id),
        id,
        area: ratatui::layout::Rect::new(0, 0, 80, 24),
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

fn app_with_window() -> AppState {
    let mut a = app();
    a.windows.push(window(7));
    a.active_idx = 0;
    a
}

#[test]
fn both_spellings_are_the_option() {
    assert!(is_name("pane-colours"));
    assert!(is_name("pane-colors"));
    assert!(is_name("pane-colours[3]"));
    assert!(is_name("pane-colors[3]"));
    assert!(!is_name("pane-colourses"));
    assert!(!is_name("pane-border-style"));
    assert_eq!(canonical_name("pane-colors[2]").as_deref(), Some("pane-colours[2]"));
    assert!(crate::option_array::is_array_name("pane-colors[1]").is_some());
    assert_eq!(crate::option_array::spec("pane-colors").map(|s| s.name), Some(OPTION));
}

#[test]
fn an_element_must_be_a_colour() {
    // colour.c:387-462: `zzz` and `9` are no colour; `0` to `7` and `90` to
    // `97` are SGR numbers of colours.
    assert_eq!(validate("pane-colours[1]", "zzz"), Err("bad colour: zzz".to_string()));
    assert_eq!(validate("pane-colours[1]", "9"), Err("bad colour: 9".to_string()));
    assert_eq!(validate("pane-colours", "red,zzz"), Err("bad colour: zzz".to_string()));
    assert!(validate("pane-colours[1]", "#00FF00").is_ok());
    assert!(validate("pane-colours[1]", "colour200").is_ok());
    assert!(validate("pane-colours", "red,blue green").is_ok());
    // The CLI checks through the catalog before it sends anything.
    assert_eq!(
        crate::server::option_catalog::validate_option_value("pane-colors[4]", "zzz"),
        Err("bad colour: zzz".to_string())
    );
}

#[test]
fn values_are_kept_in_tmux_spelling() {
    assert_eq!(canonical_value(None, "RED,blue  GREEN").unwrap(), "red,blue,green");
    assert_eq!(canonical_value(None, "1 91").unwrap(), "red,brightred");
    let key = crate::option_array::ArrayKey::Index(0);
    assert_eq!(canonical_value(Some(&key), "#00FF00").unwrap(), "#00ff00");
}

#[test]
fn the_global_array_is_empty_and_shows_its_bare_name() {
    let a = app();
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, false), "pane-colours\n");
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, true), "");
    assert_eq!(get_option_value(&a, "pane-colours"), "");
}

#[test]
fn a_value_without_an_index_is_split_on_space_and_comma() {
    let mut a = app();
    apply_set_option(&mut a, "pane-colours", "red,blue green", false).unwrap();
    assert_eq!(
        show(&a, Scope::Global, "", "pane-colours", false, false),
        "pane-colours[0] red\npane-colours[1] blue\npane-colours[2] green\n"
    );
    assert_eq!(get_option_value(&a, "pane-colours"), "red blue green");
    assert_eq!(crate::format::expand_format("#{pane-colours}", &a), "red blue green");
    // The other spelling reads and writes the same array.
    set_array_option(&mut a, "pane-colors[1]", "#0000FF", false).unwrap();
    assert_eq!(show(&a, Scope::Global, "", "pane-colors[1]", false, false), "pane-colours[1] #0000ff\n");
}

#[test]
fn a_bad_colour_writes_nothing() {
    let mut a = app();
    apply_set_option(&mut a, "pane-colours[1]", "#00ff00", false).unwrap();
    assert_eq!(apply_set_option(&mut a, "pane-colours[1]", "zzz", false), Err("bad colour: zzz".to_string()));
    assert_eq!(set_array_option(&mut a, "pane-colours", "red,zzz", false), Err("bad colour: zzz".to_string()));
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, false), "pane-colours[1] #00ff00\n");
}

#[test]
fn append_adds_elements_and_sets_an_element() {
    let mut a = app();
    set_array_option(&mut a, "pane-colours", "red", false).unwrap();
    set_array_option(&mut a, "pane-colours", "blue", true).unwrap();
    // `-a` on a colour element sets it; it never glues text together.
    set_array_option(&mut a, "pane-colours[0]", "green", true).unwrap();
    assert_eq!(
        show(&a, Scope::Global, "", "pane-colours", false, false),
        "pane-colours[0] green\npane-colours[1] blue\n"
    );
    unset_array_option(&mut a, "pane-colours[0]").unwrap();
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, false), "pane-colours[1] blue\n");
    unset_array_option(&mut a, "pane-colours").unwrap();
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, false), "pane-colours\n");
}

#[test]
fn a_window_has_its_own_table_and_inherits_the_global_one_whole() {
    let mut a = app_with_window();
    apply_set_option(&mut a, "pane-colours[1]", "#00ff00", false).unwrap();
    // The window holds nothing yet: nothing without -A, the global lines
    // marked `*` with it (cmd-show-options.c:130-134 and :176).
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", false, false), "");
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", true, false), "pane-colours[1]* #00ff00\n");
    let reply = apply_set_window_option(&mut a, "", "pane-colours[2]", "#0000FF", false, false, false, false);
    assert_eq!(reply, "");
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", false, false), "pane-colours[2] #0000ff\n");
    // With the window's own table held, -A shows only that, unmerged.
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", true, false), "pane-colours[2] #0000ff\n");
    // The global table is untouched.
    assert_eq!(show(&a, Scope::Global, "", "pane-colours", false, false), "pane-colours[1] #00ff00\n");
    // A bad colour is refused on this route too.
    let reply = apply_set_window_option(&mut a, "", "pane-colours[3]", "zzz", false, false, false, false);
    assert_eq!(reply, "ERROR: bad colour: zzz");
    // Unsetting an element keeps the (now empty) table; the window still
    // shadows the global one, as tmux keeps the option entry.
    apply_set_window_option(&mut a, "", "pane-colours[2]", "", true, false, false, false);
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", false, false), "pane-colours\n");
    // Unsetting the whole option hands the window back to the global table.
    apply_set_window_option(&mut a, "", "pane-colours", "", true, false, false, false);
    assert_eq!(show(&a, Scope::Window, "", "pane-colours", false, false), "");
}

#[test]
fn only_if_unset_refuses_an_element_the_window_holds() {
    let mut a = app_with_window();
    apply_set_window_option(&mut a, "", "pane-colours[2]", "red", false, false, false, false);
    let reply = apply_set_window_option(&mut a, "", "pane-colours[2]", "blue", false, false, true, false);
    assert_eq!(reply, "ERROR: already set: pane-colours[2]");
    assert_eq!(apply_set_window_option(&mut a, "", "pane-colours[2]", "blue", false, false, true, true), "");
    assert_eq!(show(&a, Scope::Window, "", "pane-colours[2]", false, true), "red\n");
}

#[test]
fn the_palette_holds_elements_0_to_255_as_draw_colours() {
    let mut array = crate::option_array::OptionArray::default();
    assert!(palette_of(&array).is_none());
    let spec = crate::option_array::spec(OPTION).unwrap();
    array.apply_set(Some(crate::option_array::ArrayKey::Index(1)), "#00ff00", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(2)), "blue", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(9)), "brightcyan", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(10)), "colour200", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(11)), "default", false, spec);
    array.apply_set(Some(crate::option_array::ArrayKey::Index(300)), "red", false, spec);
    let palette = palette_of(&array).unwrap();
    assert_eq!(palette.entry(1), Some(vt100::Color::Rgb(0, 255, 0)));
    assert_eq!(palette.entry(2), Some(vt100::Color::Idx(4)));
    assert_eq!(palette.entry(9), Some(vt100::Color::Idx(14)));
    assert_eq!(palette.entry(10), Some(vt100::Color::Idx(200)));
    assert_eq!(palette.entry(11), Some(vt100::Color::Default));
    assert_eq!(palette.entry(0), None);
    // An index above 255 is in the array but in no palette slot.
    assert_eq!(palette.entry(255), None);
    // An OSC 4 query reports an indexed entry through the xterm table, as
    // tmux's colour_force_rgb does, and has nothing for `default`.
    assert_eq!(palette.get(2), Some((0x00, 0x00, 0x80)));
    assert_eq!(palette.get(11), None);
}

#[test]
fn reconcile_costs_nothing_until_the_option_is_written() {
    let mut a = app_with_window();
    assert!(!a.pane_colours.engaged);
    assert!(!reconcile(&mut a));
    // A write to any other array or option leaves it disengaged.
    apply_set_option(&mut a, "status-format[0]", "x", false).unwrap();
    apply_set_option(&mut a, "status-bg", "red", false).unwrap();
    assert!(!a.pane_colours.engaged);
    apply_set_option(&mut a, "pane-colours[1]", "red", false).unwrap();
    assert!(a.pane_colours.engaged);
}

#[test]
fn format_reads_the_window_table_before_the_global_one() {
    let mut a = app_with_window();
    apply_set_option(&mut a, "pane-colours", "red blue", false).unwrap();
    apply_set_window_option(&mut a, "", "pane-colours[5]", "green", false, false, false, false);
    let w = &a.windows[0];
    assert_eq!(format_value(&a, Some(w), None, "pane-colours"), "green");
    assert_eq!(format_value(&a, None, None, "pane-colours"), "red blue");
    assert_eq!(format_value(&a, Some(w), None, "pane-colours[5]"), "green");
}
