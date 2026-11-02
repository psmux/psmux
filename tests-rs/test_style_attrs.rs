// The `overline` and `nooverline` style names (tmux attributes.c:81, and
// style.c:245-254 for `no` and an attribute name, at tag 3.8). psmux took the
// name and did nothing, as ratatui has no overline; it now sets the smuggled
// bit #962 added, which `draw_cells` writes as `CSI 53 m`.
//
// And the attributes of status-style, window-status-style and
// window-status-current-style: tmux takes the whole style for the status
// line's default cell (status.c:248, style_apply at style.c:468-473) and puts
// the window styles into the list format as `#[...]` (options-table.c:129,
// 153-155). The client kept their colours and bold alone.

use ratatui::style::{Color, Modifier, Style};

use crate::rendering::{overline_of, UL_STYLE_MASK};
use crate::style::{parse_inline_styles, parse_tmux_style, parse_tmux_style_components, style_sgr};

#[test]
fn overline_sets_the_bit_and_nooverline_clears_it() {
    assert!(overline_of(parse_tmux_style("overline").add_modifier));
    let s = parse_tmux_style("fg=red,overline");
    assert!(overline_of(s.add_modifier));
    assert_eq!(s.fg, Some(Color::Red));
    assert!(!overline_of(parse_tmux_style("overline,nooverline").add_modifier));
    assert!(!overline_of(parse_tmux_style("nooverline").add_modifier));
    // The other attributes are left alone.
    let s = parse_tmux_style("bold,overline,nooverline");
    assert!(s.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn default_and_none_clear_it_too() {
    assert!(!overline_of(parse_tmux_style("overline,default").add_modifier));
    assert!(!overline_of(parse_tmux_style("overline,none").add_modifier));
}

#[test]
fn an_inline_style_sets_and_clears_it() {
    let spans = parse_inline_styles("#[nooverline]L#[overline]O#[default]D", Style::default());
    let text_and_overline: Vec<(String, bool)> = spans
        .iter()
        .map(|span| (span.content.to_string(), overline_of(span.style.add_modifier)))
        .filter(|(text, _)| !text.is_empty())
        .collect();
    assert_eq!(
        text_and_overline,
        vec![("L".to_string(), false), ("O".to_string(), true), ("D".to_string(), false)]
    );
}

#[test]
fn the_sgr_of_a_style_writes_53() {
    let sgr = style_sgr(parse_tmux_style("overline"));
    assert!(sgr.ends_with(";53m"), "{sgr:?}");
    assert!(!style_sgr(parse_tmux_style("bold")).contains("53"));
}

#[test]
fn the_status_components_keep_every_attribute() {
    let (fg, bg, attrs) =
        parse_tmux_style_components("fg=black,bg=green,reverse,underscore,italics,overline");
    assert_eq!(fg, Some(Color::Black));
    assert_eq!(bg, Some(Color::Green));
    // The colours are the caller's, so the attributes carry none.
    assert_eq!(attrs.fg, None);
    assert_eq!(attrs.bg, None);
    for m in [Modifier::REVERSED, Modifier::UNDERLINED, Modifier::ITALIC] {
        assert!(attrs.add_modifier.contains(m), "{m:?} in {attrs:?}");
    }
    assert!(overline_of(attrs.add_modifier));
}

#[test]
fn the_status_components_keep_bold_and_drop_it_again() {
    let (_, _, attrs) = parse_tmux_style_components("bg=green,bold");
    assert!(attrs.add_modifier.contains(Modifier::BOLD));
    let (_, _, attrs) = parse_tmux_style_components("bold,nobold");
    assert!(!attrs.add_modifier.contains(Modifier::BOLD));
    let (_, _, attrs) = parse_tmux_style_components("reverse,none");
    assert!(attrs.add_modifier.is_empty());
}

#[test]
fn the_status_components_keep_the_underscore_style_and_colour() {
    let (_, _, attrs) = parse_tmux_style_components("bg=green,curly-underscore,us=red");
    assert_eq!(attrs.add_modifier.bits() & UL_STYLE_MASK, 3 << 12);
    assert_eq!(attrs.underline_color, Some(Color::Red));
}

#[test]
fn colours_alone_give_no_attributes() {
    let (fg, bg, attrs) = parse_tmux_style_components("bg=green,fg=black");
    assert_eq!((fg, bg), (Some(Color::Black), Some(Color::Green)));
    assert_eq!(attrs, Style::default());
}

// tmux style.c:243: `none` sets the attributes to 0. A window style is put
// into the window list as `#[...]` over status-style, so `none` in it clears
// the attributes status-style set as well, and only what follows it stays.
#[test]
fn none_in_a_window_style_clears_what_status_style_set() {
    let (_, _, base_attrs) =
        parse_tmux_style_components("bg=green,reverse,bold,overline,dashed-underscore");
    let base = base_attrs.fg(Color::Black).bg(Color::Green);

    let (fg, _, attrs) = parse_tmux_style_components("none,fg=red");
    assert_eq!(fg, Some(Color::Red));
    let drawn = base.patch(attrs);
    for m in [Modifier::REVERSED, Modifier::BOLD, Modifier::UNDERLINED] {
        assert!(!drawn.add_modifier.contains(m), "{m:?} left in {drawn:?}");
    }
    assert!(!overline_of(drawn.add_modifier));
    assert_eq!(drawn.add_modifier.bits() & UL_STYLE_MASK, 0);

    // What comes after `none` is kept.
    let (_, _, attrs) = parse_tmux_style_components("none,italics,curly-underscore");
    let drawn = base.patch(attrs);
    assert!(drawn.add_modifier.contains(Modifier::ITALIC));
    assert!(!drawn.add_modifier.contains(Modifier::REVERSED));
    assert_eq!(drawn.add_modifier.bits() & UL_STYLE_MASK, 3 << 12);

    // Without `none` the window style adds on top and removes nothing.
    let (_, _, attrs) = parse_tmux_style_components("fg=red,italics");
    assert!(attrs.sub_modifier.is_empty());
    let drawn = base.patch(attrs);
    assert!(drawn.add_modifier.contains(Modifier::REVERSED));
    assert!(drawn.add_modifier.contains(Modifier::ITALIC));
}
